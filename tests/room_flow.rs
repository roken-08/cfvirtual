use anyhow::{bail, Result};
use cfvirtual::{
    client::net::{Client, ReconnectSession},
    domain::*,
    server::{self, proto::*, BackendFuture, RoomBackend, State, ValidatedHandle},
    service::ContestOptions,
    store::Store,
};
use futures_util::{SinkExt, StreamExt};
use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};
use tempfile::TempDir;
use tokio::{net::TcpListener, task::JoinHandle, time::timeout};
use tokio_tungstenite::{connect_async, tungstenite::Message};

#[derive(Default)]
struct OfflineBackend {
    submissions: Mutex<HashMap<String, Vec<Submission>>>,
    judged: AtomicBool,
    polls: AtomicUsize,
    hold_poll: AtomicBool,
    poll_started: tokio::sync::Notify,
    release_poll: tokio::sync::Notify,
    slow_validation: tokio::sync::Notify,
    release_validation: tokio::sync::Notify,
}

impl RoomBackend for OfflineBackend {
    fn validate_handle<'a>(&'a self, handle: &'a str) -> BackendFuture<'a, ValidatedHandle> {
        Box::pin(async move {
            if handle == "missing" {
                bail!("fixture profile does not exist");
            }
            if handle == "slow_guest" {
                self.slow_validation.notify_one();
                self.release_validation.notified().await;
            }
            Ok(ValidatedHandle {
                handle: handle.into(),
                rating: 1300,
            })
        })
    }
    fn recent<'a>(&'a self, handle: &'a str) -> BackendFuture<'a, Vec<Submission>> {
        Box::pin(async move {
            self.polls.fetch_add(1, Ordering::SeqCst);
            if self.hold_poll.swap(false, Ordering::SeqCst) {
                self.poll_started.notify_one();
                self.release_poll.notified().await;
            }
            let mut submissions = self
                .submissions
                .lock()
                .expect("fixture lock")
                .get(handle)
                .cloned()
                .unwrap_or_default();
            if !self.judged.load(Ordering::SeqCst) && handle == "guest" {
                for submission in &mut submissions {
                    submission.verdict = None;
                }
            }
            Ok(submissions)
        })
    }
    fn prepare<'a>(&'a self, options: &'a ContestOptions) -> BackendFuture<'a, Contest> {
        Box::pin(async move {
            self.validate_handle(&options.handle).await?;
            let mut contest = fixture(options.duration_sec);
            contest.host_handle = options.handle.clone();
            contest.blind = options.blind;
            contest.bots.truncate(options.bots);
            for bot in &mut contest.bots {
                for solve in &mut bot.solves {
                    solve.accepted_at_sec = solve.accepted_at_sec.min(options.duration_sec);
                }
            }
            Ok(contest)
        })
    }
}

fn fixture(duration_sec: u32) -> Contest {
    Contest {
        id: uuid::Uuid::new_v4().to_string(),
        code: None,
        host_handle: "host".into(),
        host_rating: 1200,
        started_at: 0,
        duration_sec,
        problems: vec![ContestProblem {
            slot: 'A',
            tier: Tier::Core,
            problem: Problem {
                key: ProblemKey {
                    contest_id: 1000,
                    index: "A".into(),
                },
                name: "Offline fixture".into(),
                rating: Some(1200),
                tags: vec!["math".into()],
                solved_count: None,
            },
        }],
        participants: vec![],
        bots: vec![Bot {
            name: "amber_fixture_07".into(),
            rating: 1200,
            solves: vec![BotSolve {
                slot: 'A',
                accepted_at_sec: 20,
                rejected_attempts: 2,
            }],
        }],
        seed: 42,
        mode: "room".into(),
        blind: true,
        ended_at: None,
    }
}

struct Harness {
    state: State,
    backend: Arc<OfflineBackend>,
    url: String,
    data: TempDir,
    task: JoinHandle<()>,
}

impl Drop for Harness {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn serve() -> Result<Harness> {
    let data = tempfile::tempdir()?;
    let backend = Arc::new(OfflineBackend::default());
    let state = State::with_backend(backend.clone(), data.path().into());
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let url = format!("ws://{}", listener.local_addr()?);
    let app = server::router(state.clone());
    let task = tokio::spawn(async move {
        axum::serve(listener, app).await.expect("test server");
    });
    Ok(Harness {
        state,
        backend,
        url,
        data,
        task,
    })
}

async fn until(client: &mut Client, predicate: impl Fn(&ServerMessage) -> bool) -> ServerMessage {
    timeout(Duration::from_secs(8), async {
        loop {
            let message = client
                .recv()
                .await
                .expect("receive valid message")
                .expect("connection open");
            if predicate(&message) {
                return message;
            }
        }
    })
    .await
    .expect("expected message within eight seconds")
}

async fn welcome(client: &mut Client) -> (String, Snapshot) {
    match until(client, |m| matches!(m, ServerMessage::Welcome { .. })).await {
        ServerMessage::Welcome { token, state, .. } => (token, state),
        _ => unreachable!(),
    }
}

async fn error(client: &mut Client, expected: &str) {
    let message = until(client, |m| matches!(m, ServerMessage::Error { .. })).await;
    if let ServerMessage::Error { code, .. } = message {
        assert_eq!(code, expected);
    }
}

fn assert_private(state: &Snapshot, own: Option<&str>, tokens: &[&str]) {
    let json = serde_json::to_string(state).expect("serialize snapshot");
    for token in tokens {
        assert!(!json.contains(token), "bearer token leaked");
    }
    assert!(!json.contains("\"token\""));
    assert!(!json.contains("\"solves\""), "bot schedule leaked");
    assert!(!json.contains("\"handles\""), "submission details leaked");
    assert!(state.problem_status.keys().all(|h| Some(h.as_str()) == own));
    let standings = serde_json::to_value(&state.standings).expect("standings JSON");
    for standing in standings.as_array().expect("array") {
        assert!(
            standing.get("problems").is_none(),
            "opponents' per-problem details leaked"
        );
    }
}

#[tokio::test]
async fn authoritative_room_reconnect_spectate_privacy_and_final_drain() -> Result<()> {
    let harness = serve().await?;
    let created = harness
        .state
        .create_room(fixture(120), Some("room-secret".into()))
        .await?;
    assert_eq!(created.code.len(), 6);
    assert!(!created.code.chars().any(|c| "0O1ILU".contains(c)));
    let host_session = ReconnectSession {
        room: created.code.clone(),
        participant_token: created.host_token.clone(),
    };
    let mut host = Client::resume(&harness.url, &host_session).await?;
    assert_eq!(host.snapshot().expect("welcome").phase, Phase::Lobby);
    let mut guest = Client::connect(&format!("{}/ws", harness.url)).await?;
    guest
        .send(ClientMessage::Hello {
            code: created.code.clone(),
            handle: "guest".into(),
            password: None,
        })
        .await?;
    error(&mut guest, "BAD_PASSWORD").await;
    guest
        .send(ClientMessage::Hello {
            code: created.code.clone(),
            handle: "guest".into(),
            password: Some("room-secret".into()),
        })
        .await?;
    let (guest_token, lobby) = welcome(&mut guest).await;
    assert_eq!(lobby.participants.len(), 2);
    assert!(lobby.problems.is_empty());
    assert_private(&lobby, Some("guest"), &[&created.host_token, &guest_token]);
    guest.send(ClientMessage::Start).await?;
    error(&mut guest, "NOT_HOST").await;
    guest.send(ClientMessage::End).await?;
    error(&mut guest, "NOT_HOST").await;
    harness.backend.hold_poll.store(true, Ordering::SeqCst);
    host.send(ClientMessage::Start).await?;
    let start = until(&mut host, |m| {
        matches!(m, ServerMessage::ContestStarted { .. })
    })
    .await;
    let started_at = if let ServerMessage::ContestStarted {
        started_at,
        problems,
        ..
    } = start
    {
        assert!(problems[0].rating.is_none() && problems[0].tags.is_empty());
        started_at
    } else {
        unreachable!()
    };
    timeout(
        Duration::from_secs(2),
        harness.backend.poll_started.notified(),
    )
    .await?;
    // A pending API request cannot block room actions or outbound messages.
    host.send(ClientMessage::Ping { t: 99 }).await?;
    until(&mut host, |m| {
        matches!(m, ServerMessage::Pong { t: 99, .. })
    })
    .await;
    let make_submission = |id, handle: &str, verdict: &str, at| Submission {
        id,
        problem: ProblemKey {
            contest_id: 1000,
            index: "A".into(),
        },
        created_at: at,
        verdict: Some(verdict.into()),
        handles: vec![handle.into()],
        tags: vec!["math".into()],
        rating: Some(1200),
    };
    // Start-time submissions are inclusive; out-of-window submissions never count.
    {
        let mut submissions = harness.backend.submissions.lock().expect("fixture lock");
        submissions.insert(
            "host".into(),
            vec![
                make_submission(1, "host", "WRONG_ANSWER", started_at),
                make_submission(2, "host", "OK", started_at),
            ],
        );
        submissions.insert(
            "guest".into(),
            vec![
                make_submission(3, "guest", "OK", started_at),
                make_submission(4, "guest", "OK", started_at + 121),
            ],
        );
    }
    harness.backend.release_poll.notify_one();
    let scored = until(&mut host, |m| {
        matches!(m, ServerMessage::Tick { standings, .. }
        if standings.iter().any(|s| s.name == "host" && s.solved == 1))
    })
    .await;
    if let ServerMessage::Tick { state, .. } = scored {
        assert_private(&state, Some("host"), &[&created.host_token, &guest_token]);
        assert!(state.problem_status["host"][&'A'].solved);
    }
    // Reconnect replaces a live socket. Old teardown must not mark the new one offline.
    let mut replacement = Client::resume(&harness.url, &host_session).await?;
    assert!(timeout(Duration::from_secs(2), async {
        loop {
            if !matches!(host.recv().await, Ok(Some(_))) {
                break;
            }
        }
    })
    .await
    .is_ok());
    replacement.send(ClientMessage::Ping { t: 123 }).await?;
    until(&mut replacement, |m| {
        matches!(m, ServerMessage::Pong { t: 123, .. })
    })
    .await;
    let mut spectator = Client::connect(&harness.url).await?;
    spectator
        .send(ClientMessage::Spectate {
            code: created.code.clone(),
            password: Some("room-secret".into()),
        })
        .await?;
    let (_, observed) = welcome(&mut spectator).await;
    assert_eq!(observed.phase, Phase::Running);
    assert_private(&observed, None, &[&created.host_token, &guest_token]);
    assert!(
        observed
            .participants
            .iter()
            .find(|p| p.is_host)
            .expect("host")
            .connected
    );
    assert_eq!(
        observed
            .standings
            .iter()
            .find(|s| s.is_bot)
            .expect("bot")
            .solved,
        0
    );
    spectator.send(ClientMessage::End).await?;
    error(&mut spectator, "NOT_HOST").await;
    let mut late = Client::connect(&harness.url).await?;
    late.send(ClientMessage::Hello {
        code: created.code.clone(),
        handle: "late_guest".into(),
        password: Some("room-secret".into()),
    })
    .await?;
    error(&mut late, "CONTEST_STARTED").await;
    let polls_before = harness.backend.polls.load(Ordering::SeqCst);
    replacement.send(ClientMessage::End).await?;
    until(
        &mut replacement,
        |m| matches!(m, ServerMessage::Tick { state, .. } if state.phase == Phase::Finishing),
    )
    .await;
    // Simulate an in-window submission judged only after the contest cutoff.
    harness.backend.judged.store(true, Ordering::SeqCst);
    let ended = until(&mut guest, |m| {
        matches!(m, ServerMessage::ContestEnded { .. })
    })
    .await;
    let (id, final_standings) = match ended {
        ServerMessage::ContestEnded {
            reason,
            state,
            standings,
        } => {
            assert_eq!(reason, EndReason::Host);
            assert_eq!(state.phase, Phase::Ended);
            assert_private(&state, Some("guest"), &[&created.host_token, &guest_token]);
            assert!(state.problem_status["guest"][&'A'].solved);
            assert_eq!(
                standings
                    .iter()
                    .find(|s| s.name == "host")
                    .expect("host")
                    .penalty,
                1200
            );
            assert_eq!(
                standings
                    .iter()
                    .find(|s| s.name == "guest")
                    .expect("guest")
                    .solved,
                1
            );
            assert_eq!(state.problems[0].rating, Some(1200));
            (state.contest_id, standings)
        }
        _ => unreachable!(),
    };
    assert!(
        harness.backend.polls.load(Ordering::SeqCst) >= polls_before + 4,
        "two final passes across both humans"
    );
    let guest_session = guest.session().expect("session").clone();
    guest.close().await?;
    let resumed = Client::resume(&harness.url, &guest_session).await?;
    assert_eq!(resumed.snapshot().expect("snapshot").phase, Phase::Ended);
    // Persistence runs outside the room lock on the blocking pool.
    timeout(Duration::from_secs(3), async {
        loop {
            if let Ok(store) = Store::open(&harness.data.path().join("cfvirtual.db")) {
                if let Some((contest, standings)) = store.load_contest(&id).expect("saved contest")
                {
                    assert!(contest.participants.iter().all(|p| p.token.is_empty()));
                    assert_eq!(
                        standings
                            .iter()
                            .map(|s| (&s.name, s.solved, s.penalty))
                            .collect::<Vec<_>>(),
                        final_standings
                            .iter()
                            .map(|s| (&s.name, s.solved, s.penalty))
                            .collect::<Vec<_>>()
                    );
                    break;
                }
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await?;
    Ok(())
}

#[tokio::test]
async fn remote_create_automatic_timeout_and_protocol_boundaries() -> Result<()> {
    let harness = serve().await?;
    let mut host = Client::connect(&harness.url).await?;
    host.send(ClientMessage::Create {
        options: ContestOptions {
            handle: "remote_host".into(),
            problems: 1,
            duration_sec: 1,
            rating: None,
            tags: vec![],
            bots: 1,
            seed: 7,
            blind: true,
            refresh: false,
        },
        password: None,
    })
    .await?;
    let (_, lobby) = welcome(&mut host).await;
    host.send(ClientMessage::Start).await?;
    let ended = until(&mut host, |m| {
        matches!(m, ServerMessage::ContestEnded { .. })
    })
    .await;
    if let ServerMessage::ContestEnded { reason, state, .. } = ended {
        assert_eq!(reason, EndReason::Timeout);
        assert_eq!(state.ended_at, Some(state.started_at + 1));
        assert_eq!(
            state
                .standings
                .iter()
                .find(|s| s.is_bot)
                .expect("elapsed bot")
                .solved,
            1
        );
    }
    let (mut raw, _) = connect_async(&harness.url).await?;
    raw.send(Message::Text(
        r#"{"v":1,"type":"future_extension","anything":true}"#.into(),
    ))
    .await?;
    raw.send(Message::Text(r#"{"v":1,"type":"ping","t":77}"#.into()))
        .await?;
    let response = timeout(Duration::from_secs(2), raw.next())
        .await?
        .expect("response")?;
    assert!(response.to_text()?.contains("pong"));
    raw.send(Message::Text(
        r#"{"v":2,"type":"hello","code":"ANY","handle":"guest"}"#.into(),
    ))
    .await?;
    let response = timeout(Duration::from_secs(2), raw.next())
        .await?
        .expect("version response")?;
    assert!(response.to_text()?.contains("VERSION_MISMATCH"));
    let mut invalid = Client::connect(&harness.url).await?;
    invalid
        .send(ClientMessage::Reconnect {
            code: lobby.code,
            token: uuid::Uuid::new_v4().to_string(),
        })
        .await?;
    error(&mut invalid, "BAD_TOKEN").await;
    invalid.send(ClientMessage::Start).await?;
    error(&mut invalid, "NOT_JOINED").await;
    Ok(())
}

#[tokio::test]
async fn validation_does_not_lock_room_and_spectator_limit_is_enforced() -> Result<()> {
    let harness = serve().await?;
    let created = harness.state.create_room(fixture(120), None).await?;
    let mut host = Client::resume(
        &harness.url,
        &ReconnectSession {
            room: created.code.clone(),
            participant_token: created.host_token,
        },
    )
    .await?;
    let mut guest = Client::connect(&harness.url).await?;
    guest
        .send(ClientMessage::Hello {
            code: created.code.clone(),
            handle: "missing".into(),
            password: None,
        })
        .await?;
    error(&mut guest, "INVALID_HANDLE").await;
    guest
        .send(ClientMessage::Hello {
            code: created.code.clone(),
            handle: "HOST".into(),
            password: None,
        })
        .await?;
    error(&mut guest, "HANDLE_IN_USE").await;
    guest
        .send(ClientMessage::Hello {
            code: created.code.clone(),
            handle: "slow_guest".into(),
            password: None,
        })
        .await?;
    timeout(
        Duration::from_secs(2),
        harness.backend.slow_validation.notified(),
    )
    .await?;
    host.send(ClientMessage::Start).await?;
    until(&mut host, |m| {
        matches!(m, ServerMessage::ContestStarted { .. })
    })
    .await;
    harness.backend.release_validation.notify_one();
    error(&mut guest, "CONTEST_STARTED").await;
    let mut spectators = Vec::new();
    for _ in 0..8 {
        let mut spectator = Client::connect(&harness.url).await?;
        spectator
            .send(ClientMessage::Spectate {
                code: created.code.clone(),
                password: None,
            })
            .await?;
        welcome(&mut spectator).await;
        spectators.push(spectator);
    }
    let mut ninth = Client::connect(&harness.url).await?;
    ninth
        .send(ClientMessage::Spectate {
            code: created.code,
            password: None,
        })
        .await?;
    error(&mut ninth, "ROOM_FULL").await;
    let mut too_many_bots = fixture(120);
    too_many_bots.bots = vec![too_many_bots.bots[0].clone(); 17];
    assert!(harness
        .state
        .create_room(too_many_bots, None)
        .await
        .is_err());
    Ok(())
}

#[tokio::test]
async fn player_capacity_and_transport_size_limit() -> Result<()> {
    let harness = serve().await?;
    let created = harness.state.create_room(fixture(120), None).await?;
    let mut players = Vec::new();
    for index in 0..31 {
        let mut player = Client::connect(&harness.url).await?;
        player
            .send(ClientMessage::Hello {
                code: created.code.clone(),
                handle: format!("fixture_player_{index}"),
                password: None,
            })
            .await?;
        welcome(&mut player).await;
        players.push(player);
    }
    let mut extra = Client::connect(&harness.url).await?;
    extra
        .send(ClientMessage::Hello {
            code: created.code.clone(),
            handle: "extra_player".into(),
            password: None,
        })
        .await?;
    error(&mut extra, "ROOM_FULL").await;
    let (mut raw, _) = connect_async(&harness.url).await?;
    raw.send(Message::Text("x".repeat(MAX_MESSAGE_BYTES + 1).into()))
        .await?;
    // The transport closes oversized messages instead of accepting/allocating them.
    let response = timeout(Duration::from_secs(2), raw.next()).await?;
    assert!(!matches!(response, Some(Ok(Message::Text(_)))));
    Ok(())
}
