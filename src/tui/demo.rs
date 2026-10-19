use crate::{domain::*, store::Store};
use anyhow::Result;
use std::path::Path;

const LOGICAL_START: i64 = 1_700_000_000;

/// A reproducible, entirely offline fixture. No real player identity is used.
pub fn demo_contest(seed: u64) -> Contest {
    let fixtures = [
        (1872, "A", "Plus Minus", 800, "math", Tier::Weak),
        (
            1676,
            "E",
            "Eating Queries",
            1100,
            "binary search",
            Tier::Weak,
        ),
        (
            1857,
            "C",
            "Assembly via Minimums",
            1200,
            "sortings",
            Tier::Core,
        ),
        (1832, "C", "Contrast Value", 1200, "greedy", Tier::Core),
        (
            1702,
            "E",
            "Split Into Two Sets",
            1600,
            "graphs",
            Tier::Stretch,
        ),
        (
            1791,
            "F",
            "Range Update Point Query",
            1700,
            "data structures",
            Tier::Stretch,
        ),
    ];
    let problems: Vec<_> = fixtures
        .into_iter()
        .zip('A'..='F')
        .map(
            |((contest_id, index, name, rating, tag, tier), slot)| ContestProblem {
                slot,
                tier,
                problem: Problem {
                    key: ProblemKey {
                        contest_id,
                        index: index.into(),
                    },
                    name: name.into(),
                    rating: Some(rating),
                    tags: vec![tag.into()],
                    solved_count: None,
                },
            },
        )
        .collect();
    // Use the domain's seeded model, then compress its schedules to make events
    // visible throughout the 90-second recording rather than only at the end.
    let mut bots = bots::generate(3, 1200, &problems, 7200, seed);
    for bot in &mut bots {
        for event in &mut bot.solves {
            event.accepted_at_sec = 5 + event.accepted_at_sec * 80 / 7200;
        }
    }
    Contest {
        id: format!("demo-{seed:016x}"),
        code: None,
        host_handle: "demo_player".into(),
        host_rating: 1200,
        started_at: LOGICAL_START,
        duration_sec: 90,
        problems,
        participants: vec![Participant {
            handle: "demo_player".into(),
            is_host: true,
            is_spectator: false,
            token: String::new(),
            connected: true,
        }],
        bots,
        seed,
        mode: "demo".into(),
        blind: false,
        ended_at: None,
    }
}

pub(super) fn submissions(contest: &Contest) -> Vec<Submission> {
    [
        (0, 10, "WRONG_ANSWER"),
        (0, 18, "OK"),
        (1, 29, "WRONG_ANSWER"),
        (1, 44, "OK"),
        (2, 63, "OK"),
        (3, 78, "WRONG_ANSWER"),
    ]
    .into_iter()
    .enumerate()
    .filter_map(|(id, (index, elapsed, verdict))| {
        let problem = &contest.problems.get(index)?.problem;
        Some(Submission {
            id: id as u64 + 1,
            problem: problem.key.clone(),
            created_at: contest.started_at + elapsed,
            verdict: Some(verdict.into()),
            handles: vec!["demo_player".into()],
            tags: problem.tags.clone(),
            rating: problem.rating,
        })
    })
    .collect()
}

/// Advance a logical clock through every second, score to completion, and save.
/// This path has no terminal, network client, sleeps, or wall-clock waits.
pub async fn run_demo_headless(seed: u64, db_path: &Path) -> Result<(Contest, Vec<Standing>)> {
    let mut contest = demo_contest(seed);
    let submissions = submissions(&contest);
    let mut standings = Vec::new();
    for elapsed in 0..=contest.duration_sec {
        standings = scoring::score(
            &contest,
            &submissions,
            contest.started_at + i64::from(elapsed),
        );
    }
    contest.ended_at = Some(contest.deadline());
    let path = db_path.to_path_buf();
    tokio::task::spawn_blocking(move || {
        Store::open(&path)?.save_contest(&contest, &standings)?;
        Ok((contest, standings))
    })
    .await?
}
