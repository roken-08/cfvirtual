use super::{proto::*, room::Connection, *};
use crate::domain::*;
use std::sync::{
    atomic::{AtomicBool, AtomicUsize, Ordering},
    Arc,
};
use tokio::sync::{mpsc, watch};

#[derive(Default)]
struct CountingBackend {
    calls: AtomicUsize,
    stall: AtomicBool,
}

impl RoomBackend for CountingBackend {
    fn validate_handle<'a>(&'a self, handle: &'a str) -> BackendFuture<'a, ValidatedHandle> {
        Box::pin(async move {
            Ok(ValidatedHandle {
                handle: handle.into(),
                rating: 1200,
            })
        })
    }
    fn recent<'a>(&'a self, _: &'a str) -> BackendFuture<'a, Vec<Submission>> {
        Box::pin(async move {
            self.calls.fetch_add(1, Ordering::SeqCst);
            if self.stall.load(Ordering::SeqCst) {
                std::future::pending::<()>().await;
            }
            Ok(Vec::new())
        })
    }
    fn prepare<'a>(&'a self, _: &'a ContestOptions) -> BackendFuture<'a, Contest> {
        Box::pin(async { Ok(contest()) })
    }
}

fn contest() -> Contest {
    Contest {
        id: uuid::Uuid::new_v4().to_string(),
        code: None,
        host_handle: "fixture_host".into(),
        host_rating: 1200,
        started_at: 0,
        duration_sec: 7200,
        problems: vec![ContestProblem {
            slot: 'A',
            tier: Tier::Core,
            problem: Problem {
                key: ProblemKey {
                    contest_id: 1000,
                    index: "A".into(),
                },
                name: "Fixture".into(),
                rating: Some(1200),
                tags: vec![],
                solved_count: None,
            },
        }],
        participants: vec![],
        bots: vec![],
        seed: 1,
        mode: "room".into(),
        blind: true,
        ended_at: None,
    }
}

async fn settle() {
    for _ in 0..10 {
        tokio::task::yield_now().await;
    }
}

#[tokio::test]
async fn normal_room_start_enters_a_real_preparation_countdown() {
    let directory = tempfile::tempdir().expect("fixture directory");
    let state = State::with_backend_and_prestart_wait(
        Arc::new(CountingBackend::default()),
        directory.path().into(),
        crate::service::PRE_CONTEST_WAIT_SEC,
    );
    let created = state.create_room(contest(), None).await.expect("room");
    let room = state.lookup(&created.code).await.expect("lookup");
    let before = chrono::Utc::now().timestamp();
    room.lock().await.start(0).expect("host start");
    let guard = room.lock().await;
    assert_eq!(guard.phase, Phase::Countdown);
    assert!(guard.contest.started_at >= before + 59);
    assert!(guard.contest.started_at <= before + 61);
}

#[tokio::test(start_paused = true)]
async fn polls_every_twenty_seconds_and_sends_five_second_full_snapshots() {
    let directory = tempfile::tempdir().expect("fixture directory");
    let backend = Arc::new(CountingBackend::default());
    let state = State::with_backend(backend.clone(), directory.path().into());
    let created = state.create_room(contest(), None).await.expect("room");
    let room = state.lookup(&created.code).await.expect("room lookup");
    let (tx, mut rx) = mpsc::channel(OUTBOUND_CAPACITY);
    let (stop, _stopped) = watch::channel(false);
    {
        let mut guard = room.lock().await;
        guard.attach(
            0,
            Connection {
                generation: uuid::Uuid::new_v4(),
                tx,
                stop,
            },
        );
        guard.start(0).expect("host start");
    }
    settle().await;
    assert_eq!(backend.calls.load(Ordering::SeqCst), 1);
    while rx.try_recv().is_ok() {}
    tokio::time::advance(std::time::Duration::from_secs(5)).await;
    settle().await;
    assert!(
        matches!(rx.try_recv(), Ok(ServerMessage::Tick { state, .. }) if state.phase == Phase::Running)
    );
    tokio::time::advance(std::time::Duration::from_secs(14)).await;
    settle().await;
    assert_eq!(backend.calls.load(Ordering::SeqCst), 1);
    tokio::time::advance(std::time::Duration::from_secs(1)).await;
    settle().await;
    assert_eq!(backend.calls.load(Ordering::SeqCst), 2);
}

#[tokio::test(start_paused = true)]
async fn stalled_api_cannot_hold_finalization_open_forever() {
    let directory = tempfile::tempdir().expect("fixture directory");
    let backend = Arc::new(CountingBackend::default());
    backend.stall.store(true, Ordering::SeqCst);
    let state = State::with_backend(backend, directory.path().into());
    let created = state.create_room(contest(), None).await.expect("room");
    let room = state.lookup(&created.code).await.expect("lookup");
    room.lock().await.start(0).expect("start");
    settle().await;
    room.lock().await.end(0).expect("end");
    tokio::time::advance(std::time::Duration::from_secs(15)).await;
    settle().await;
    tokio::time::advance(std::time::Duration::from_secs(180)).await;
    settle().await;
    // The API drain uses virtual time; the durable SQLite write uses a real
    // blocking thread. Give that write bounded wall time before final delivery.
    tokio::time::resume();
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while room.lock().await.phase != Phase::Ended {
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("finalized after bounded polling and durable save");
    assert!(
        crate::store::Store::open(&directory.path().join("cfvirtual.db"))
            .expect("saved database")
            .load_contest(&room.lock().await.contest.id)
            .expect("load contest")
            .is_some()
    );
}

#[tokio::test]
async fn overflowing_bounded_outbound_queue_disconnects_only_that_socket() {
    let directory = tempfile::tempdir().expect("fixture directory");
    let state = State::with_backend(
        Arc::new(CountingBackend::default()),
        directory.path().into(),
    );
    let created = state.create_room(contest(), None).await.expect("room");
    let room = state.lookup(&created.code).await.expect("lookup");
    let (tx, rx) = mpsc::channel(OUTBOUND_CAPACITY);
    let (stop, stopped) = watch::channel(false);
    let generation = uuid::Uuid::new_v4();
    let mut guard = room.lock().await;
    guard.attach(
        0,
        Connection {
            generation,
            tx,
            stop,
        },
    );
    for _ in 0..OUTBOUND_CAPACITY {
        guard.send(
            0,
            ServerMessage::Pong {
                t: 1,
                server_time: 1,
            },
        );
    }
    assert_eq!(rx.len(), OUTBOUND_CAPACITY);
    assert!(*stopped.borrow());
    assert!(!guard.active(0, generation));
    assert!(!guard.snapshot(0).participants[0].connected);
}
