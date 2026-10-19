use super::{
    app::{Action, App, Screen},
    demo, io, render,
    terminal::{Tty, Worker},
};
use crate::{
    cf::CfClient,
    domain::{scoring, Contest, Submission},
};
use anyhow::{bail, Result};
use chrono::Utc;
use crossterm::event::{Event, EventStream};
use futures_util::StreamExt;
use std::{collections::BTreeMap, path::PathBuf, time::Duration};
use tokio::{
    sync::mpsc,
    time::{Instant, MissedTickBehavior},
};

enum Command {
    Poll,
    Finish,
}
struct Poll {
    submissions: Vec<Submission>,
    error: Option<String>,
    pass: u8,
    done: bool,
}

async fn poller(
    api: CfClient,
    handle: String,
    mut commands: mpsc::Receiver<Command>,
    tx: mpsc::Sender<Poll>,
) {
    while let Some(command) = commands.recv().await {
        let passes = if matches!(command, Command::Finish) {
            2
        } else {
            1
        };
        for pass in 1..=passes {
            let (submissions, error) =
                match tokio::time::timeout(Duration::from_secs(45), api.recent(&handle)).await {
                    Ok(Ok(submissions)) => (submissions, None),
                    Ok(Err(error)) => (Vec::new(), Some(error.to_string())),
                    Err(_) => (Vec::new(), Some("Codeforces polling timed out".into())),
                };
            if tx
                .send(Poll {
                    submissions,
                    error,
                    pass: if passes == 2 { pass } else { 0 },
                    done: passes == 2 && pass == 2,
                })
                .await
                .is_err()
            {
                return;
            }
        }
    }
}

// Keep the last known judged verdict when a later page contains an older pending
// snapshot. In-window submission IDs are bounded, never an unbounded poll log.
fn merge(contest: &Contest, known: &mut BTreeMap<u64, Submission>, incoming: Vec<Submission>) {
    for submission in incoming {
        if submission.created_at < contest.started_at
            || submission.created_at > contest.deadline()
            || !contest
                .problems
                .iter()
                .any(|p| p.problem.key == submission.problem)
            || !submission
                .handles
                .iter()
                .any(|h| h.eq_ignore_ascii_case(&contest.host_handle))
        {
            continue;
        }
        if let Some(old) = known.get(&submission.id) {
            let pending =
                |s: &Submission| matches!(s.verdict.as_deref(), None | Some("TESTING" | ""));
            if old.verdict.as_deref() == Some("OK") || (!pending(old) && pending(&submission)) {
                continue;
            }
        } else if known.len() >= 10_000 {
            continue;
        }
        known.insert(submission.id, submission);
    }
}

pub(super) async fn run(
    terminal: &mut Tty,
    mut contest: Contest,
    api: CfClient,
    db_path: PathBuf,
    theme: &str,
    ascii: bool,
    is_demo: bool,
) -> Result<()> {
    if is_demo {
        contest.host_handle = "demo_player".into();
        contest.participants = demo::demo_contest(contest.seed).participants;
        contest.mode = "demo".into();
    }
    contest.ended_at = None;
    let mut app = App::new(contest.host_handle.clone(), theme, ascii);
    app.contest = Some(contest.clone());
    app.status = if is_demo {
        "Offline demo: synthetic submissions and BOT schedules; no network.".into()
    } else {
        "Starting in 3 seconds. Submissions are manual in your browser.".into()
    };
    let mut input = EventStream::new();
    let mut frame = tokio::time::interval(Duration::from_millis(100));
    frame.set_missed_tick_behavior(MissedTickBehavior::Skip);
    let countdown = Instant::now() + Duration::from_secs(3);
    let mut next_poll = countdown;
    let mut pending = false;
    let mut finishing = false;
    let mut started = false;
    let mut known = BTreeMap::new();
    let (commands, rx) = mpsc::channel(2);
    let (tx, mut polls) = mpsc::channel(4);
    let _worker = if is_demo {
        None
    } else {
        Some(Worker(tokio::spawn(poller(
            api,
            app.handle.clone(),
            rx,
            tx,
        ))))
    };
    let mut jobs = io::Jobs::new();
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
                    Action::Retry if !is_demo && started && !finishing && !pending => {
                        if commands.try_send(Command::Poll).is_ok() {
                            pending = true;
                            app.status = "Retrying Codeforces in the background...".into();
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
            Some(poll) = polls.recv(), if !is_demo => {
                pending = false;
                merge(&contest, &mut known, poll.submissions);
                if let Some(error) = poll.error {
                    app.notice = format!("Offline polling: {error}. Local scores kept; r retries. Missing verdicts may be incomplete.");
                    app.status = "Cached problems remain playable; live acceptances need Codeforces connectivity.".into();
                } else {
                    app.notice.clear();
                    app.status = if poll.pass > 0 { format!("Final verdict reconciliation {}/2", poll.pass) }
                        else { "Codeforces updated. Next automatic poll in 20s.".into() };
                }
                app.standings = scoring::score(&contest, &known.values().cloned().collect::<Vec<_>>(), Utc::now().timestamp());
                if poll.done {
                    app.complete = true;
                    if !app.notice.is_empty() {
                        app.notice = format!("Final verdicts may be incomplete. {}", app.notice.replace("; r retries.", "."));
                    }
                    app.contest = Some(contest.clone());
                    io::save(&mut jobs, &mut app, db_path.clone());
                }
            }
            Some(event) = jobs.join_next(), if !jobs.is_empty() => match event {
                Ok(event) => io::apply(&mut app, event),
                Err(_) => { app.saving = false; app.status = "Background operation failed. Press r to retry saving results.".into(); }
            },
            _ = frame.tick() => {
                let instant = Instant::now();
                if !started {
                    app.countdown = countdown.saturating_duration_since(instant).as_secs().saturating_add(1).min(3);
                    if instant >= countdown {
                        contest.started_at = Utc::now().timestamp();
                        app.contest = Some(contest.clone());
                        app.set_screen(Screen::Contest);
                        started = true;
                        if is_demo { known = demo::submissions(&contest).into_iter().map(|s| (s.id, s)).collect(); }
                    }
                }
                if started && !finishing {
                    let now = Utc::now().timestamp();
                    app.standings = scoring::score(&contest, &known.values().cloned().collect::<Vec<_>>(), now);
                    if now >= contest.deadline() {
                        finishing = true;
                        contest.ended_at = Some(contest.deadline());
                        app.contest = Some(contest.clone());
                        app.paused = false;
                        app.confirm = None;
                        if is_demo {
                            app.complete = true;
                            io::save(&mut jobs, &mut app, db_path.clone());
                        } else {
                            commands.send(Command::Finish).await.map_err(|_| anyhow::anyhow!("Polling worker stopped"))?;
                            app.status = "Time is up. Two final polls reconcile in-window submissions (each bounded to 45s).".into();
                        }
                    } else if !is_demo && instant >= next_poll && !pending
                        && commands.try_send(Command::Poll).is_ok() {
                        pending = true;
                        next_poll = instant + Duration::from_secs(20);
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn finishing_runs_exactly_two_bounded_polls_even_when_the_api_is_unavailable() {
        let (commands, rx) = mpsc::channel(2);
        let (tx, mut events) = mpsc::channel(4);
        // Empty handles fail client-side validation before any network request.
        let worker = Worker(tokio::spawn(poller(CfClient::new(), String::new(), rx, tx)));
        commands
            .send(Command::Finish)
            .await
            .expect("finish command");
        let first = events.recv().await.expect("first final pass");
        let second = events.recv().await.expect("second final pass");
        assert_eq!(first.pass, 1);
        assert!(!first.done);
        assert_eq!(second.pass, 2);
        assert!(second.done);
        assert!(first.error.is_some() && second.error.is_some());
        assert!(events.try_recv().is_err());
        drop(worker);
    }

    #[test]
    fn final_polls_preserve_late_verdicts_and_enforce_the_deadline() {
        let mut contest = demo::demo_contest(3);
        contest.ended_at = Some(contest.deadline());
        let mut accepted = demo::submissions(&contest).remove(1);
        accepted.created_at = contest.deadline();
        let mut pending = accepted.clone();
        pending.verdict = None;
        let mut known = BTreeMap::new();
        merge(&contest, &mut known, vec![pending.clone()]);
        merge(&contest, &mut known, vec![accepted.clone()]);
        merge(&contest, &mut known, vec![pending]);
        let mut late = accepted;
        late.id = 99;
        late.created_at += 1;
        merge(&contest, &mut known, vec![late]);
        assert_eq!(known.len(), 1);
        let scores = scoring::score(
            &contest,
            &known.into_values().collect::<Vec<_>>(),
            contest.deadline() + 60,
        );
        let human = scores.iter().find(|s| !s.is_bot).expect("human");
        assert_eq!(human.solved, 1);
        assert_eq!(human.problems[&'A'].accepted_at_sec, Some(90));
    }
}
