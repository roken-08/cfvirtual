use super::{
    app::{Action, App, Screen},
    io, render,
    terminal::{Tty, Worker},
};
use crate::{
    client::net::{Client, ReconnectSession},
    domain::{Bot, Contest, ContestProblem, Participant, Problem, ProblemKey, Standing, Tier},
    server::proto::{ClientMessage, Phase, ServerMessage, Snapshot},
};
use anyhow::{bail, Result};
use chrono::Utc;
use crossterm::event::{Event, EventStream};
use futures_util::StreamExt;
use std::{path::PathBuf, time::Duration};
use tokio::sync::mpsc;

enum Command {
    Send(ClientMessage),
    Reconnect,
}
enum Network {
    Message(Box<ServerMessage>),
    Connected,
    Disconnected,
    Reconnecting,
    Clock { offset_ms: i64, rtt_ms: i64 },
}

async fn resume(url: &str, session: Option<&ReconnectSession>) -> Option<Client> {
    let session = session?;
    tokio::time::timeout(Duration::from_secs(20), Client::resume(url, session))
        .await
        .ok()?
        .ok()
}

async fn transport(
    mut client: Client,
    url: String,
    mut commands: mpsc::Receiver<Command>,
    tx: mpsc::Sender<Network>,
) {
    let mut session = client.session().cloned();
    let mut connected = true;
    let mut ping = tokio::time::interval(Duration::from_secs(15));
    ping.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut outstanding = None;
    let mut last_response = tokio::time::Instant::now();
    loop {
        tokio::select! {
            command = commands.recv() => {
                match command {
                    None => { let _ = client.close().await; return; }
                    Some(Command::Reconnect) => {
                        if tx.send(Network::Reconnecting).await.is_err() { return; }
                        // Do not log transport errors: URLs and reconnect frames can
                        // carry credentials. The UI receives a credential-free status.
                        if let Some(resumed) = resume(&url, session.as_ref()).await {
                            client = resumed;
                            connected = true;
                            outstanding = None;
                            last_response = tokio::time::Instant::now();
                            session = client.session().cloned();
                            if tx.send(Network::Connected).await.is_err() { return; }
                            if let Some(state) = client.snapshot().cloned() {
                                if tx.send(Network::Message(Box::new(ServerMessage::Tick {
                                    server_time: state.server_time,
                                    standings: state.standings.clone(),
                                    problem_status: state.problem_status.clone(), state,
                                }))).await.is_err() { return; }
                            }
                            ping.reset_immediately();
                        } else {
                            connected = false;
                            if tx.send(Network::Disconnected).await.is_err() { return; }
                        }
                    }
                    Some(Command::Send(message)) if connected => {
                        if !matches!(tokio::time::timeout(Duration::from_secs(5), client.send(message)).await, Ok(Ok(()))) {
                            connected = false;
                            if tx.send(Network::Disconnected).await.is_err() { return; }
                        }
                    }
                    _ => {}
                }
            }
            message = client.recv(), if connected => {
                match message {
                    Ok(Some(message)) => {
                        last_response = tokio::time::Instant::now();
                        session = client.session().cloned().or(session);
                        if let ServerMessage::Pong { t, server_time } = &message {
                            let now = Utc::now().timestamp_millis();
                            let rtt = now.saturating_sub(*t);
                            if outstanding == Some(*t) && (0..=30_000).contains(&rtt) {
                                let offset_ms = server_time.saturating_sub(t.saturating_add(rtt / 2));
                                if tx.send(Network::Clock { offset_ms, rtt_ms: rtt }).await.is_err() { return; }
                                outstanding = None;
                            }
                        } else if tx.send(Network::Message(Box::new(message))).await.is_err() { return; }
                    }
                    _ => {
                        connected = false;
                        if tx.send(Network::Disconnected).await.is_err() { return; }
                    }
                }
            }
            _ = ping.tick(), if connected => {
                if last_response.elapsed() >= Duration::from_secs(40) {
                    connected = false;
                    if tx.send(Network::Disconnected).await.is_err() { return; }
                    continue;
                }
                let t = Utc::now().timestamp_millis();
                outstanding = Some(t);
                if !matches!(tokio::time::timeout(Duration::from_secs(5), client.send(ClientMessage::Ping { t })).await, Ok(Ok(()))) {
                    connected = false;
                    if tx.send(Network::Disconnected).await.is_err() { return; }
                }
            }
        }
    }
}

// A participant projection is deliberately namespaced apart from the server's
// complete contest. Other players' aggregate totals stay authoritative; unknown
// per-problem results remain absent, never inferred or recomputed.
fn record(state: &Snapshot, handle: &str) -> (Contest, Vec<Standing>) {
    let rating = state
        .standings
        .iter()
        .find(|s| !s.is_bot && s.name.eq_ignore_ascii_case(handle))
        .map_or(1200, |s| s.rating);
    let own = state
        .problem_status
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case(handle))
        .map(|(_, statuses)| statuses);
    let standings = state
        .standings
        .iter()
        .map(|s| Standing {
            name: s.name.clone(),
            is_bot: s.is_bot,
            rating: s.rating,
            solved: s.solved,
            penalty: s.penalty,
            last_ac: s.last_ac,
            problems: if !s.is_bot && s.name.eq_ignore_ascii_case(handle) {
                own.cloned().unwrap_or_default()
            } else {
                Default::default()
            },
        })
        .collect();
    let contest = Contest {
        id: format!(
            "{}:participant:{}",
            state.contest_id,
            handle.to_ascii_lowercase()
        ),
        code: Some(state.code.clone()),
        // Store statistics are owner-based. Preserve the source room host in the
        // mode description while this local record's owner is the participant.
        host_handle: handle.into(),
        host_rating: rating,
        started_at: state.started_at,
        duration_sec: state.settings.duration_sec,
        problems: state
            .problems
            .iter()
            .map(|p| ContestProblem {
                slot: p.slot,
                tier: Tier::Core,
                problem: Problem {
                    key: ProblemKey {
                        contest_id: p.contest_id,
                        index: p.index.clone(),
                    },
                    name: p.name.clone(),
                    rating: p.rating,
                    tags: p.tags.clone(),
                    solved_count: None,
                },
            })
            .collect(),
        participants: state
            .participants
            .iter()
            .map(|p| Participant {
                handle: p.handle.clone(),
                is_host: p.is_host,
                is_spectator: p.is_spectator,
                token: String::new(),
                connected: p.connected,
            })
            .collect(),
        bots: state
            .bots
            .iter()
            .map(|b| Bot {
                name: b.name.clone(),
                rating: b.rating,
                solves: Vec::new(),
            })
            .collect(),
        seed: 0,
        mode: format!("room-participant (room host: {})", state.host_handle),
        blind: state.settings.blind,
        ended_at: state.ended_at,
    };
    (contest, standings)
}

fn snapshot(app: &mut App, state: Snapshot, jobs: &mut io::Jobs, db_path: &std::path::Path) {
    if !app.clock_synced {
        app.offset_ms = state
            .server_time
            .saturating_mul(1000)
            .saturating_sub(Utc::now().timestamp_millis());
    }
    let phase = state.phase;
    let spectator = state.participants.iter().any(|participant| {
        participant.handle.eq_ignore_ascii_case(&app.handle) && participant.is_spectator
    });
    let (contest, standings) = record(&state, &app.handle);
    app.contest = Some(contest);
    app.standings = standings;
    app.room = Some(state);
    app.complete = phase == Phase::Ended;
    if app.complete {
        if app.base_screen() != Screen::Results {
            app.set_screen(Screen::Contest);
        }
        if spectator {
            app.saved = true;
            app.status = "Spectator view ended; no participant result was saved.".into();
        } else if !app.saved && !app.saving {
            io::save(jobs, app, db_path.to_path_buf());
        }
    } else {
        app.set_screen(if phase == Phase::Lobby {
            Screen::Lobby
        } else {
            Screen::Contest
        });
        if phase == Phase::Finishing {
            app.status =
                "Server is reconciling final verdicts; waiting for authoritative results.".into();
        }
    }
}

fn message(app: &mut App, message: ServerMessage, jobs: &mut io::Jobs, path: &std::path::Path) {
    match message {
        ServerMessage::Welcome { state, .. }
        | ServerMessage::Tick { state, .. }
        | ServerMessage::ContestEnded { state, .. } => snapshot(app, state, jobs, path),
        ServerMessage::Roster {
            participants,
            bots,
            settings,
        } => {
            if let Some(state) = &mut app.room {
                state.participants = participants;
                state.bots = bots;
                state.settings = settings;
            }
        }
        ServerMessage::ContestStarted {
            contest_id,
            started_at,
            duration_sec,
            problems,
            blind,
            server_time,
        } => {
            if let Some(mut state) = app.room.clone() {
                state.contest_id = contest_id;
                state.started_at = started_at;
                state.deadline = Some(started_at.saturating_add(i64::from(duration_sec)));
                state.settings.duration_sec = duration_sec;
                state.settings.blind = blind;
                state.problems = problems;
                state.server_time = server_time;
                state.phase = Phase::Running;
                snapshot(app, state, jobs, path);
            }
        }
        ServerMessage::Error { code, message } => app.status = format!("Server {code}: {message}"),
        _ => {}
    }
}

pub(super) async fn run(
    terminal: &mut Tty,
    client: Client,
    server_url: super::RoomUrls,
    handle: String,
    db_path: PathBuf,
    theme: &str,
    ascii: bool,
) -> Result<()> {
    let mut app = App::new(handle, theme, ascii);
    app.server_url = Some(public_url(&server_url.advertise));
    app.status = "Connected. Room clock and scores are authoritative; r reconnects.".into();
    let mut jobs = io::Jobs::new();
    if let Some(state) = client.snapshot().cloned() {
        snapshot(&mut app, state, &mut jobs, &db_path);
    } else {
        app.set_screen(Screen::Lobby);
    }
    let (commands, rx) = mpsc::channel(8);
    let (tx, mut network) = mpsc::channel(32);
    let _worker = Worker(tokio::spawn(transport(client, server_url.connect, rx, tx)));
    let mut input = EventStream::new();
    let mut frame = tokio::time::interval(Duration::from_millis(100));
    frame.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        super::terminal::check()?;
        terminal.draw(|frame| render::draw(frame, &app, Utc::now().timestamp_millis()))?;
        tokio::select! {
            signal = tokio::signal::ctrl_c() => { signal?; return Ok(()); }
            event = input.next() => {
                let Some(event) = event else { bail!("Terminal input closed"); };
                let action = match event? { Event::Key(key) => app.key(key), _ => Action::None };
                match action {
                    Action::Quit => return Ok(()),
                    Action::Retry if app.complete && !app.saved => io::save(&mut jobs, &mut app, db_path.clone()),
                    Action::Retry => {
                        if !app.reconnecting && commands.try_send(Command::Reconnect).is_ok() {
                            app.reconnecting = true;
                            app.status = "Reconnect queued...".into();
                        }
                    }
                    Action::Start | Action::End if app.connected && app.is_host() => {
                        let message = if action == Action::Start { ClientMessage::Start } else { ClientMessage::End };
                        if commands.try_send(Command::Send(message)).is_ok() {
                            app.status = "Request sent; waiting for the authoritative server snapshot.".into();
                        }
                    }
                    Action::Export => io::export(&mut jobs, &app, db_path.clone()),
                    Action::Queue => io::queue(&mut jobs, &app, db_path.clone()),
                    Action::Open | Action::Copy => if let Some(url) = app.selected_url() {
                        io::link(&mut jobs, url, action == Action::Copy);
                    },
                    _ => {}
                }
            }
            Some(event) = network.recv() => match event {
                Network::Message(msg) => message(&mut app, *msg, &mut jobs, &db_path),
                Network::Connected => {
                    app.connected = true; app.reconnecting = false; app.clock_synced = false; app.notice.clear();
                    app.status = "Reconnected with the in-memory session. Snapshot restored.".into();
                }
                Network::Reconnecting => { app.connected = false; app.status = "Reconnecting with the in-memory participant session...".into(); }
                Network::Disconnected => {
                    app.connected = false; app.reconnecting = false;
                    app.notice = "Disconnected. Scores shown are the last server snapshot; press r to reconnect.".into();
                }
                Network::Clock { offset_ms, rtt_ms } => {
                    app.offset_ms = offset_ms; app.clock_synced = true;
                    if !app.complete { app.status = format!("Connected | clock synchronized (ping {rtt_ms}ms) | r reconnect"); }
                }
            },
            Some(event) = jobs.join_next(), if !jobs.is_empty() => match event {
                Ok(event) => io::apply(&mut app, event),
                Err(_) => { app.saving = false; app.status = "Local operation failed; r retries saving results.".into(); }
            },
            _ = frame.tick() => {}
        }
    }
}

fn public_url(input: &str) -> String {
    match url::Url::parse(input) {
        Ok(mut url) => {
            let _ = url.set_username("");
            let _ = url.set_password(None);
            url.set_query(None);
            url.set_fragment(None);
            url.to_string()
        }
        Err(_) => "<server>".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::server::proto::*;
    use std::collections::BTreeMap;

    pub(super) fn state() -> Snapshot {
        let demo = super::super::demo::demo_contest(7);
        Snapshot {
            code: "ABC234".into(),
            contest_id: "room-fixture".into(),
            host_handle: "room_host".into(),
            phase: Phase::Ended,
            started_at: demo.started_at,
            deadline: Some(demo.deadline()),
            ended_at: Some(demo.deadline()),
            end_reason: Some(EndReason::Timeout),
            server_time: demo.deadline(),
            settings: Settings {
                duration_sec: 90,
                problem_count: 6,
                blind: true,
                password_required: false,
            },
            participants: vec![PublicParticipant {
                participant_id: "public-id".into(),
                handle: "room_guest".into(),
                is_host: false,
                is_spectator: false,
                connected: true,
            }],
            bots: Vec::new(),
            problems: demo
                .problems
                .iter()
                .map(|p| PublicProblem {
                    slot: p.slot,
                    contest_id: p.problem.key.contest_id,
                    index: p.problem.key.index.clone(),
                    name: p.problem.name.clone(),
                    url: p.problem.url(),
                    rating: p.problem.rating,
                    tags: p.problem.tags.clone(),
                })
                .collect(),
            standings: vec![
                PublicStanding {
                    name: "room_host".into(),
                    is_bot: false,
                    rating: 1400,
                    solved: 5,
                    penalty: 1234,
                    last_ac: 80,
                },
                PublicStanding {
                    name: "room_guest".into(),
                    is_bot: false,
                    rating: 1200,
                    solved: 1,
                    penalty: 15,
                    last_ac: 15,
                },
            ],
            problem_status: BTreeMap::from([(
                "room_guest".into(),
                BTreeMap::from([(
                    'A',
                    crate::domain::ProblemStatus {
                        solved: true,
                        accepted_at_sec: Some(15),
                        attempts: 1,
                        rejected: 0,
                        first_attempt_at_sec: Some(15),
                    },
                )]),
            )]),
        }
    }

    #[test]
    fn participant_record_keeps_authority_and_does_not_overwrite_host_snapshot() {
        let state = state();
        let (local, scores) = record(&state, "room_guest");
        assert_ne!(local.id, state.contest_id);
        assert_eq!(scores[0].solved, 5);
        assert_eq!(scores[0].penalty, 1234);
        assert!(scores[0].problems.is_empty());
        assert_eq!(scores[1].problems[&'A'].accepted_at_sec, Some(15));
        let dir = tempfile::tempdir().expect("temp directory");
        let mut store = crate::store::Store::open(&dir.path().join("test.db")).expect("store");
        let mut full = local.clone();
        full.id = state.contest_id.clone();
        full.seed = 42;
        store.save_contest(&full, &scores).expect("host snapshot");
        store
            .save_contest(&local, &scores)
            .expect("participant snapshot");
        assert_eq!(
            store
                .load_contest(&state.contest_id)
                .expect("load")
                .expect("host")
                .0
                .seed,
            42
        );
        assert!(local.participants.iter().all(|p| p.token.is_empty()));
    }

    #[test]
    fn guests_cannot_start_or_end_and_clock_uses_server_offset() {
        let mut app = App::new("room_guest".into(), "default", false);
        app.server_url = Some("ws://example.invalid/".into());
        let mut state = state();
        state.phase = Phase::Lobby;
        app.room = Some(state.clone());
        app.screen = Screen::Lobby;
        assert_eq!(
            app.key(crossterm::event::KeyEvent::from(
                crossterm::event::KeyCode::Enter
            )),
            Action::None
        );
        assert_eq!(
            app.key(crossterm::event::KeyEvent::from(
                crossterm::event::KeyCode::Char('x')
            )),
            Action::None
        );
        app.contest = Some(record(&state, "room_guest").0);
        app.offset_ms = 10_000;
        assert_eq!(app.remaining((state.started_at + 20) * 1000), 60);
        assert_eq!(
            public_url("ws://user:secret@example.invalid/path?token=hidden"),
            "ws://example.invalid/path"
        );
    }

    #[test]
    fn host_end_requires_confirmation_and_leaving_is_a_separate_action() {
        use crossterm::event::{KeyCode, KeyEvent};
        let mut app = App::new("room_guest".into(), "default", true);
        app.server_url = Some("ws://example.invalid/".into());
        let mut state = state();
        state.participants[0].is_host = true;
        state.phase = Phase::Lobby;
        app.room = Some(state);
        app.screen = Screen::Lobby;
        assert_eq!(app.key(KeyEvent::from(KeyCode::Enter)), Action::Start);
        app.room.as_mut().expect("room").phase = Phase::Running;
        app.screen = Screen::Contest;
        assert_eq!(app.key(KeyEvent::from(KeyCode::Char('x'))), Action::None);
        assert_eq!(app.confirm, Some(super::super::app::Confirm::End));
        assert_eq!(app.key(KeyEvent::from(KeyCode::Enter)), Action::End);
        assert_eq!(app.key(KeyEvent::from(KeyCode::Char('q'))), Action::None);
        assert_eq!(app.confirm, Some(super::super::app::Confirm::Leave));
        assert_eq!(app.key(KeyEvent::from(KeyCode::Char('n'))), Action::None);
        assert!(app.confirm.is_none());
    }

    #[tokio::test]
    async fn room_results_are_saved_before_the_results_screen() {
        let directory = tempfile::tempdir().expect("temporary database directory");
        let path = directory.path().join("room.db");
        let mut app = App::new("room_guest".into(), "default", true);
        app.server_url = Some("ws://example.invalid/".into());
        let mut jobs = io::Jobs::new();
        snapshot(&mut app, state(), &mut jobs, &path);
        assert_eq!(app.screen, Screen::Contest);
        assert!(app.saving);
        assert!(!app.saved);
        let event = jobs
            .join_next()
            .await
            .expect("save job")
            .expect("save worker");
        io::apply(&mut app, event);
        assert!(app.saved);
        assert_eq!(app.screen, Screen::Results);
        let store = crate::store::Store::open(&path).expect("reopened database");
        let saved = store
            .load_contest(&app.contest.as_ref().expect("contest").id)
            .expect("load")
            .expect("saved");
        assert_eq!(saved.1[1].problems[&'A'].accepted_at_sec, Some(15));
    }

    #[tokio::test]
    async fn transport_receives_while_pinging_and_resumes_its_session() -> Result<()> {
        use crate::server::proto::{ClientEnvelope, ServerEnvelope};
        use axum::{
            extract::ws::{Message, WebSocketUpgrade},
            routing::get,
            Router,
        };
        use std::sync::{
            atomic::{AtomicUsize, Ordering},
            Arc,
        };
        let connections = Arc::new(AtomicUsize::new(0));
        let count = Arc::clone(&connections);
        let router = Router::new().route(
            "/",
            get(move |upgrade: WebSocketUpgrade| {
                let count = Arc::clone(&count);
                async move {
                    upgrade.on_upgrade(move |mut socket| async move {
                        count.fetch_add(1, Ordering::SeqCst);
                        while let Some(Ok(Message::Text(text))) = socket.recv().await {
                            let request: ClientEnvelope =
                                serde_json::from_str(&text).expect("request envelope");
                            let response = match request.message {
                                ClientMessage::Hello { .. } => ServerMessage::Welcome {
                                    participant_id: "fixture-id".into(),
                                    token: "fixture-session".into(),
                                    state: state(),
                                },
                                ClientMessage::Reconnect { code, token } => {
                                    assert_eq!(code, "ABC234");
                                    assert_eq!(token, "fixture-session");
                                    ServerMessage::Welcome {
                                        participant_id: "fixture-id".into(),
                                        token,
                                        state: state(),
                                    }
                                }
                                ClientMessage::Ping { t } => ServerMessage::Pong {
                                    t,
                                    server_time: Utc::now().timestamp_millis() + 5_000,
                                },
                                _ => continue,
                            };
                            let text = serde_json::to_string(&ServerEnvelope::from(response))
                                .expect("response envelope");
                            if socket.send(Message::Text(text.into())).await.is_err() {
                                return;
                            }
                        }
                    })
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await?;
        let url = format!("ws://{}/", listener.local_addr()?);
        let _server = Worker(tokio::spawn(async move {
            axum::serve(listener, router)
                .await
                .expect("mock room server");
        }));
        let mut client = Client::connect(&url).await?;
        client
            .send(ClientMessage::Hello {
                code: "ABC234".into(),
                handle: "room_guest".into(),
                password: None,
            })
            .await?;
        assert!(matches!(
            client.recv().await?,
            Some(ServerMessage::Welcome { .. })
        ));
        let (commands, rx) = mpsc::channel(4);
        let (tx, mut events) = mpsc::channel(8);
        let _worker = Worker(tokio::spawn(transport(client, url, rx, tx)));
        let offset = tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if let Some(Network::Clock { offset_ms, .. }) = events.recv().await {
                    break offset_ms;
                }
            }
        })
        .await?;
        assert!((4_500..=5_500).contains(&offset));
        commands
            .send(Command::Reconnect)
            .await
            .map_err(|_| anyhow::anyhow!("worker closed"))?;
        let restored = tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if let Some(Network::Message(message)) = events.recv().await {
                    if let ServerMessage::Tick { state, .. } = *message {
                        break state;
                    }
                }
            }
        })
        .await?;
        assert_eq!(restored.standings[0].solved, 5);
        assert_eq!(connections.load(Ordering::SeqCst), 2);
        Ok(())
    }
}
