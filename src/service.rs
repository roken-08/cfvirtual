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
