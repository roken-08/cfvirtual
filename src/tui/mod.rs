//! Keyboard-driven solo and authoritative-room views.
//!
//! `p` hides the solo contest behind an overlay: it never pauses the clock,
//! polling, or opponents. History and review queue browsing remain available
//! through the CLI's `history` and `review` read commands.
//!
//! Solo results use `r` to queue failed problems. Rooms reserve `r` for session
//! reconnect and use `v` for that queue. Room saves are participant projections,
//! namespaced away from the server's full record; unavailable problem details
//! for other players are never synthesized. Their standings totals stay intact.

mod app;
mod demo;
mod io;
mod render;
mod room;
mod solo;
mod terminal;
mod theme;

pub use app::Screen;
pub use demo::{demo_contest, run_demo_headless};

/// Separate a host's local transport from the LAN/tunnel address shown to guests.
pub struct RoomUrls {
    pub connect: String,
    pub advertise: String,
}
impl From<String> for RoomUrls {
    fn from(url: String) -> Self {
        Self {
            connect: url.clone(),
            advertise: url,
        }
    }
}

use crate::{cf::CfClient, client::net::Client, domain::Contest};
use anyhow::Result;
use chrono::Utc;
use crossterm::event::{Event, EventStream};
use futures_util::{FutureExt, StreamExt};
use std::{io::IsTerminal, panic::AssertUnwindSafe, path::PathBuf};
use tokio::time::{interval, Duration, MissedTickBehavior};

/// Run a prepared solo contest after a three-second countdown. Redirected demo
/// invocations automatically use the offline logical-clock runner instead.
pub async fn run_solo(
    contest: Contest,
    api: CfClient,
    db_path: PathBuf,
    theme: &str,
    ascii: bool,
    demo: bool,
) -> Result<()> {
    if demo && (!std::io::stdin().is_terminal() || !std::io::stdout().is_terminal()) {
        run_demo_headless(contest.seed, &db_path).await?;
        return Ok(());
    }
    let mut terminal = terminal::Session::enter()?;
    let result = AssertUnwindSafe(solo::run(
        &mut terminal.terminal,
        contest,
        api,
        db_path,
        theme,
        ascii,
        demo,
    ))
    .catch_unwind()
    .await;
    drop(terminal);
    result.unwrap_or_else(|_| Err(anyhow::anyhow!("The terminal app stopped unexpectedly")))
}

/// Render an already-connected room client. Connection/password setup belongs
/// to the caller; reconnect credentials remain in the transport's memory.
pub async fn run_room(
    client: Client,
    server_url: impl Into<RoomUrls>,
    handle: String,
    db_path: PathBuf,
    theme: &str,
    ascii: bool,
) -> Result<()> {
    let mut terminal = terminal::Session::enter()?;
    let result = AssertUnwindSafe(room::run(
        &mut terminal.terminal,
        client,
        server_url.into(),
        handle,
        db_path,
        theme,
        ascii,
    ))
    .catch_unwind()
    .await;
    drop(terminal);
    result.unwrap_or_else(|_| Err(anyhow::anyhow!("The terminal app stopped unexpectedly")))
}

/// Browse saved contests using the same keyboard-driven terminal state machine
/// as the contest view. Redirected callers should use `history`'s text/JSON mode.
pub async fn run_history(db_path: PathBuf, limit: usize, theme: &str, ascii: bool) -> Result<()> {
    let entries = tokio::task::spawn_blocking(move || {
        crate::store::Store::open(&db_path).and_then(|store| store.history(limit))
    })
    .await??;
    run_browser(Screen::History, entries, Vec::new(), theme, ascii).await
}

/// Browse the persisted spaced-repetition queue. `due` applies the same UTC
/// due-time filter as the read command.
pub async fn run_review(
    db_path: PathBuf,
    due: bool,
    limit: usize,
    theme: &str,
    ascii: bool,
) -> Result<()> {
    let now = Utc::now().timestamp();
    let entries = tokio::task::spawn_blocking(move || {
        crate::store::Store::open(&db_path).and_then(|store| store.review(due, limit, now))
    })
    .await??;
    run_browser(Screen::Review, Vec::new(), entries, theme, ascii).await
}

async fn run_browser(
    screen: Screen,
    history: Vec<crate::store::HistoryEntry>,
    reviews: Vec<crate::store::ReviewEntry>,
    theme: &str,
    ascii: bool,
) -> Result<()> {
    let mut terminal = terminal::Session::enter()?;
    let result = AssertUnwindSafe(async {
        let mut app = app::App::new("local".into(), theme, ascii);
        app.screen = screen;
        app.history_entries = history;
        app.review_entries = reviews;
        let mut input = EventStream::new();
        let mut frame = interval(Duration::from_millis(100));
        frame.set_missed_tick_behavior(MissedTickBehavior::Skip);
        loop {
            terminal.terminal.draw(|frame| render::draw(frame, &app, Utc::now().timestamp_millis()))?;
            tokio::select! {
                signal = tokio::signal::ctrl_c() => { signal?; return Ok(()); }
                event = input.next() => {
                    let Some(event) = event else { anyhow::bail!("Terminal input closed"); };
                    if let Event::Key(key) = event? {
                        let action = app.key(key);
                        match action {
                            app::Action::Quit => return Ok(()),
                            app::Action::Open | app::Action::Copy => {
                                if let Some(url) = app.selected_url() {
                                    // Browser/clipboard failures are converted to a visible status.
                                    let mut jobs = io::Jobs::new();
                                    io::link(&mut jobs, url, action == app::Action::Copy);
                                    if let Some(event) = jobs.join_next().await {
                                        match event {
                                            Ok(event) => io::apply(&mut app, event),
                                            Err(error) => app.status = format!("Background link operation failed: {error}"),
                                        }
                                    }
                                }
                            }
                            _ => {}
                        }
                    }
                }
                _ = frame.tick() => {}
            }
        }
    }).catch_unwind().await;
    drop(terminal);
    result.unwrap_or_else(|_| Err(anyhow::anyhow!("The terminal browser stopped unexpectedly")))
}
