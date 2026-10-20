use super::{
    proto::{EndReason, Phase},
    room::Room,
    State,
};
use crate::{domain::Submission, store::Store};
use std::{sync::Arc, time::Duration};
use tokio::sync::Mutex;

const POLL_INTERVAL: Duration = Duration::from_secs(20);
const FINAL_DRAIN_LIMIT: Duration = Duration::from_secs(180);

pub(crate) async fn drive(state: State, room: Arc<Mutex<Room>>) {
    let poll_state = state.clone();
    let poll_room = room.clone();
    tokio::spawn(async move {
        poller(poll_state, poll_room).await;
    });
    let mut interval = tokio::time::interval(Duration::from_secs(1));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut next_snapshot = tokio::time::Instant::now();
    let mut last_scores = Vec::new();
    loop {
        interval.tick().await;
        let mut guard = room.lock().await;
        if guard.phase == Phase::Ended {
            break;
        }
        if guard.phase == Phase::Countdown
            && chrono::Utc::now().timestamp() >= guard.contest.started_at
        {
            guard.activate();
        }
        if guard.phase == Phase::Running
            && chrono::Utc::now().timestamp() >= guard.contest.deadline()
        {
            guard.begin_finish(EndReason::Timeout);
        }
        let scores = guard.scores();
        let changed = scores != last_scores;
        last_scores = scores;
        let heartbeat = tokio::time::Instant::now() >= next_snapshot;
        if heartbeat || changed {
            guard.broadcast_tick();
        }
        if heartbeat {
            next_snapshot = tokio::time::Instant::now() + Duration::from_secs(5);
        }
    }
}

async fn poller(state: State, room: Arc<Mutex<Room>>) {
    let wake = room.lock().await.wake.clone();
    loop {
        let phase = room.lock().await.phase;
        match phase {
            Phase::Lobby | Phase::Countdown => {
                wake.notified().await;
            }
            Phase::Running => {
                let next = tokio::time::Instant::now() + POLL_INTERVAL;
                poll_all(&state, &room).await;
                while room.lock().await.phase == Phase::Running {
                    tokio::select! {
                        _ = tokio::time::sleep_until(next) => break,
                        _ = wake.notified() => {},
                    }
                }
            }
            Phase::Finishing => {
                // Both passes start after the cutoff, even if an earlier polling
                // cycle was in flight. API verdict updates replace pending ones.
                let drain = async {
                    poll_all(&state, &room).await;
                    tokio::time::sleep(Duration::from_secs(2)).await;
                    poll_all(&state, &room).await;
                };
                if tokio::time::timeout(FINAL_DRAIN_LIMIT, drain)
                    .await
                    .is_err()
                {
                    tracing::warn!(
                        "Room final polling deadline reached; keeping observed verdicts"
                    );
                }
                let (contest, standings) = {
                    let mut guard = room.lock().await;
                    guard.sync_participants();
                    (guard.contest.clone(), guard.scores())
                };
                let path = state.inner.data_dir.join("cfvirtual.db");
                let saved = tokio::task::spawn_blocking(move || {
                    let mut store = Store::open(&path)?;
                    store.save_contest(&contest, &standings)
                })
                .await;
                if !matches!(saved, Ok(Ok(()))) {
                    tracing::warn!("Could not persist room final standings");
                }
                // Publish final results only after the authoritative record's
                // save attempt. Embedded hosts can then safely reuse that record.
                room.lock().await.finish();
                break;
            }
            Phase::Ended => break,
        }
    }
}

async fn poll_all(state: &State, room: &Arc<Mutex<Room>>) {
    let (phase, handles): (Phase, Vec<String>) = {
        let guard = room.lock().await;
        (
            guard.phase,
            guard
                .members
                .iter()
                .filter(|m| !m.is_spectator)
                .map(|m| m.handle.clone())
                .collect(),
        )
    };
    for handle in handles {
        if room.lock().await.phase != phase {
            return;
        }
        let result =
            tokio::time::timeout(Duration::from_secs(15), state.inner.backend.recent(&handle))
                .await;
        match result {
            Ok(Ok(submissions)) => {
                let mut guard = room.lock().await;
                if guard.phase == Phase::Ended {
                    return;
                }
                let start = guard.contest.started_at;
                let end = guard.contest.deadline();
                let keys: Vec<_> = guard
                    .contest
                    .problems
                    .iter()
                    .map(|p| p.problem.key.clone())
                    .collect();
                let kept: Vec<Submission> = submissions
                    .into_iter()
                    .filter(|s| {
                        s.created_at >= start
                            && s.created_at <= end
                            && keys.contains(&s.problem)
                            && s.handles.iter().any(|h| h.eq_ignore_ascii_case(&handle))
                    })
                    .take(10000)
                    .collect();
                let stored = guard.submissions.entry(handle).or_default();
                for submission in kept {
                    // Never regress a previously judged result to a stale
                    // pending snapshot. Domain deduplication also prefers AC.
                    if stored.get(&submission.id).is_some_and(|old| {
                        old.verdict.as_deref() == Some("OK")
                            || (is_judged(old) && !is_judged(&submission))
                    }) {
                        continue;
                    }
                    if stored.len() < 10000 || stored.contains_key(&submission.id) {
                        stored.insert(submission.id, submission);
                    }
                }
                guard.broadcast_tick();
            }
            _ => tracing::warn!("Room polling unavailable; retaining previous scores"),
        }
    }
}

fn is_judged(submission: &Submission) -> bool {
    !matches!(
        submission.verdict.as_deref(),
        None | Some("" | "TESTING" | "COMPILATION_ERROR" | "SKIPPED")
    )
}
