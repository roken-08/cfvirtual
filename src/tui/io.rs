use super::app::{App, Screen};
use crate::{
    domain::{review, Contest, Standing},
    store::Store,
};
use anyhow::{Context, Result};
use chrono::Utc;
use std::{
    path::PathBuf,
    process::{Command, Stdio},
};
use tokio::task::JoinSet;

pub(super) enum Event {
    Saved(Result<()>),
    Exported(Result<PathBuf>),
    Queued(Result<usize>),
    Link {
        status: String,
        clipboard: Option<arboard::Clipboard>,
    },
}

pub(super) type Jobs = JoinSet<Event>;

pub(super) fn save(jobs: &mut Jobs, app: &mut App, path: PathBuf) {
    if app.saving {
        return;
    }
    let Some(contest) = app.contest.clone() else {
        return;
    };
    let standings = app.standings.clone();
    app.saving = true;
    app.status = "Saving final results before opening review...".into();
    jobs.spawn_blocking(move || Event::Saved(save_record(path, contest, standings)));
}

fn save_record(path: PathBuf, contest: Contest, standings: Vec<Standing>) -> Result<()> {
    Store::open(&path)?.save_contest(&contest, &standings)
}

pub(super) fn export(jobs: &mut Jobs, app: &App, path: PathBuf) {
    let Some(id) = app.contest.as_ref().map(|c| c.id.clone()) else {
        return;
    };
    let handle = app.handle.clone();
    jobs.spawn_blocking(move || {
        Event::Exported((|| {
            let (contest, standings) = Store::open(&path)?
                .load_contest(&id)?
                .context("Saved contest not found")?;
            let filename = PathBuf::from(format!(
                "cfvirtual-review-{}.md",
                Utc::now().format("%Y-%m-%d-%H%M")
            ));
            std::fs::write(&filename, review::markdown(&contest, &standings, &handle))?;
            Ok(filename)
        })())
    });
}

pub(super) fn queue(jobs: &mut Jobs, app: &App, path: PathBuf) {
    let (Some(contest), Some(standing)) = (app.contest.clone(), app.own().cloned()) else {
        return;
    };
    jobs.spawn_blocking(move || {
        Event::Queued(Store::open(&path).and_then(|mut store| {
            store.enqueue_failed(&contest, &standing, Utc::now().timestamp())
        }))
    });
}

pub(super) fn link(jobs: &mut Jobs, text: String, copy: bool) {
    jobs.spawn_blocking(move || {
        let mut retained = None;
        let result = if copy {
            arboard::Clipboard::new()
                .and_then(|mut clipboard| {
                    clipboard.set_text(text.clone())?;
                    retained = Some(clipboard);
                    Ok(())
                })
                .map_err(anyhow::Error::from)
        } else {
            open_quietly(&text)
        };
        Event::Link {
            status: match result {
                Ok(()) => format!("{}: {text}", if copy { "Copied" } else { "Opened" }),
                Err(_) => format!(
                    "{} unavailable. Use: {text}",
                    if copy { "Clipboard" } else { "Browser" }
                ),
            },
            clipboard: retained,
        }
    });
}

fn open_quietly(url: &str) -> Result<()> {
    // Null child stdio protects the alternate screen on headless/SSH hosts.
    // The URL is an argument, never shell text; submissions remain manual.
    #[cfg(target_os = "linux")]
    let mut command = Command::new("xdg-open");
    #[cfg(target_os = "macos")]
    let mut command = Command::new("open");
    #[cfg(target_os = "windows")]
    let mut command = {
        let mut command = Command::new("rundll32");
        command.arg("url.dll,FileProtocolHandler");
        command
    };
    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    return open::that(url).map_err(Into::into);
    #[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
    {
        let mut child = command
            .arg(url)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?;
        for _ in 0..50 {
            if let Some(status) = child.try_wait()? {
                anyhow::ensure!(status.success(), "Browser could not open URL");
                return Ok(());
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        // Launchers may stay alive with the browser; reap them off the UI thread.
        std::thread::spawn(move || {
            let _ = child.wait();
        });
        Ok(())
    }
}

pub(super) fn apply(app: &mut App, event: Event) {
    match event {
        Event::Saved(result) => {
            app.saving = false;
            match result {
                Ok(()) => {
                    app.saved = true;
                    app.paused = false;
                    app.confirm = None;
                    app.set_screen(Screen::Results);
                    app.status = if app.is_room() {
                        "Saved your participant view. e export | v queue failed | r reconnect"
                            .into()
                    } else {
                        "Results saved. e export | r queue failed problems".into()
                    };
                }
                Err(error) => {
                    app.status =
                        format!("Could not save results: {error}. Press r to retry saving.")
                }
            }
        }
        Event::Exported(result) => {
            app.status = match result {
                Ok(path) => format!("Review exported: {}", path.display()),
                Err(error) => format!("Export failed: {error}; press e to retry."),
            }
        }
        Event::Queued(result) => {
            app.status = match result {
                Ok(count) => {
                    format!("Queued {count} new failed problems; review is due in one day.")
                }
                Err(error) => format!("Review queue failed: {error}"),
            }
        }
        Event::Link { status, clipboard } => {
            app.status = status;
            if clipboard.is_some() {
                app.clipboard = clipboard;
            }
        }
    }
}
