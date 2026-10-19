use crate::{cf::CfClient, domain::*};
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContestOptions {
    pub handle: String,
    pub problems: usize,
    pub duration_sec: u32,
    pub rating: Option<u32>,
    pub tags: Vec<String>,
    pub bots: usize,
    pub seed: u64,
    pub blind: bool,
    pub refresh: bool,
}

pub async fn prepare(
    options: &ContestOptions,
    api: &CfClient,
    data_dir: &Path,
    mode: &str,
) -> Result<Contest> {
    crate::config::validate_handle(&options.handle)?;
    anyhow::ensure!(
        (1..=26).contains(&options.problems),
        "problems must be 1..26"
    );
    anyhow::ensure!(
        (1..=86400).contains(&options.duration_sec),
        "duration must be 1..86400 seconds"
    );
    anyhow::ensure!(options.bots <= 16, "at most 16 bots are supported");
    let profile = match api.user_info(&options.handle).await {
        Err(error) if error.is_invalid_handle() => return Err(error.into()),
        result => result,
    };
    if profile.is_err() && mode != "room" {
        eprintln!("Codeforces profile unavailable; using the provided rating or 1200.");
    }
    let rating = options
        .rating
        .unwrap_or_else(|| profile.as_ref().map(|p| p.rating).unwrap_or(1200));
    if mode == "room" {
        profile?;
    }
    let history = match api.history(&options.handle).await {
        Ok(history) => history,
        Err(error) => {
            if error.is_invalid_handle() {
                return Err(error.into());
            }
            eprintln!("Codeforces history unavailable; using uniform offline weights: {error}");
            Vec::new()
        }
    };
    let problems = api
        .problemset(&data_dir.join("problemset.json"), options.refresh)
        .await?;
    let weaknesses = weakness::analyze(&history, chrono::Utc::now().timestamp());
    let selected = selection::select(
        &problems,
        &weaknesses.solved,
        &weaknesses.weights,
        rating,
        options.problems,
        &options.tags,
        options.seed,
    )?;
    let bots = bots::generate(
        options.bots,
        rating,
        &selected,
        options.duration_sec,
        options.seed,
    );
    Ok(Contest {
        id: uuid::Uuid::new_v4().to_string(),
        code: None,
        host_handle: options.handle.clone(),
        host_rating: rating,
        started_at: 0,
        duration_sec: options.duration_sec,
        problems: selected,
        participants: vec![Participant {
            handle: options.handle.clone(),
            is_host: true,
            is_spectator: false,
            token: String::new(),
            connected: true,
        }],
        bots,
        seed: options.seed,
        mode: mode.into(),
        blind: options.blind,
        ended_at: None,
    })
}

/// Run a real solo contest without terminal control sequences. JSON callers emit
/// the returned final snapshot; diagnostics stay on stderr throughout the timer.
pub async fn run_headless(
    mut contest: Contest,
    api: CfClient,
    db: &Path,
) -> Result<(Contest, Vec<Standing>)> {
    use std::{collections::BTreeMap, time::Duration};
    eprintln!("Contest starts in 3 seconds. Submit in your browser; Ctrl-C ends early.");
    tokio::select! {
        _ = tokio::time::sleep(Duration::from_secs(3)) => {},
        result = tokio::signal::ctrl_c() => { result?; anyhow::bail!("contest cancelled before start"); }
    }
    contest.started_at = chrono::Utc::now().timestamp();
    let (tx, mut rx) = tokio::sync::mpsc::channel(2);
    let worker_api = api.clone();
    let handle = contest.host_handle.clone();
    let worker = tokio::spawn(async move {
        let mut timer = tokio::time::interval(Duration::from_secs(20));
        timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            timer.tick().await;
            let result =
                tokio::time::timeout(Duration::from_secs(45), worker_api.recent(&handle)).await;
            let result = match result {
                Ok(Ok(batch)) => Ok(batch),
                Ok(Err(e)) => Err(e.to_string()),
                Err(_) => Err("poll timed out".into()),
            };
            if tx.send(result).await.is_err() {
                break;
            }
        }
    });
    let mut known = BTreeMap::new();
    let end = loop {
        tokio::select! {
            result = tokio::signal::ctrl_c() => { result?; break chrono::Utc::now().timestamp().min(contest.deadline()); }
            Some(batch) = rx.recv() => match batch {
                Ok(batch) => merge(&contest, &mut known, batch),
                Err(error) => eprintln!("Codeforces polling unavailable; keeping local state: {error}"),
            },
            _ = tokio::time::sleep(Duration::from_millis(250)) => {
                if chrono::Utc::now().timestamp() >= contest.deadline() { break contest.deadline(); }
            }
        }
    };
    worker.abort();
    let _ = worker.await;
    contest.ended_at = Some(end);
    eprintln!("Contest ended. Reconciling in-window verdicts with two final bounded polls...");
    for pass in 0..2 {
        if pass > 0 {
            tokio::time::sleep(Duration::from_secs(2)).await;
        }
        match tokio::time::timeout(Duration::from_secs(45), api.recent(&contest.host_handle)).await
        {
            Ok(Ok(batch)) => merge(&contest, &mut known, batch),
            Ok(Err(error)) => eprintln!("Final verdicts may be incomplete: {error}"),
            Err(_) => {
                eprintln!("Final verdict reconciliation timed out; results may be incomplete")
            }
        }
    }
    let standings = scoring::score(&contest, &known.into_values().collect::<Vec<_>>(), end);
    crate::store::Store::open(db)?.save_contest(&contest, &standings)?;
    Ok((contest, standings))
}

fn merge(
    contest: &Contest,
    known: &mut std::collections::BTreeMap<u64, Submission>,
    batch: Vec<Submission>,
) {
    for submission in batch {
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
        let pending = |s: &Submission| matches!(s.verdict.as_deref(), None | Some("" | "TESTING"));
        if let Some(old) = known.get(&submission.id) {
            if old.verdict.as_deref() == Some("OK") || (!pending(old) && pending(&submission)) {
                continue;
            }
        } else if known.len() >= 10_000 {
            continue;
        }
        known.insert(submission.id, submission);
    }
}
