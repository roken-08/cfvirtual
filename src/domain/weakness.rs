//! History-only weakness analysis. Recent failures use a 30-day half-life over
//! an inclusive 90-day window. Tags with fewer than three attempted problems
//! receive enough population-mean pseudo-observations to reach three.
//! Unlike ICPC scoring, these historical submission totals include all non-OK
//! verdicts, including pending and compilation errors, as required by §9.
//!
//! An empty weight map denotes a uniform prior over the selector's candidate
//! tags: history alone cannot enumerate tags that have never been observed.

use super::{scoring::deduplicated, ProblemKey, Submission};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, HashSet};

const DAY: i64 = 86_400;
const HISTORY_LIMIT: usize = 10_000;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct WeaknessProfile {
    pub solved: HashSet<ProblemKey>,
    pub weights: BTreeMap<String, f64>,
    pub tags: BTreeMap<String, TagStats>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TagStats {
    pub attempted: u32,
    pub solved: u32,
    pub subs: u32,
    pub ac_subs: u32,
    pub solve_rate: f64,
    pub efficiency: f64,
    pub recent_fail: f64,
    pub weakness: f64,
    pub median_solved_rating: Option<u32>,
}

#[derive(Default)]
struct ProblemHistory<'a> {
    tags: BTreeSet<String>,
    submissions: Vec<&'a Submission>,
}

pub fn analyze(history: &[Submission], now: i64) -> WeaknessProfile {
    let mut profile = WeaknessProfile::default();
    let history: Vec<_> = deduplicated(history)
        .into_iter()
        .filter(|submission| {
            submission.created_at <= now
                && submission.problem.contest_id != 0
                && !submission.problem.index.is_empty()
        })
        .collect();
    // Keep the complete supplied solved set even when statistics are capped.
    for submission in &history {
        if submission.verdict.as_deref() == Some("OK") {
            profile.solved.insert(submission.problem.clone());
        }
    }
    let mut problems: BTreeMap<&ProblemKey, ProblemHistory<'_>> = BTreeMap::new();
    for submission in history
        .iter()
        .skip(history.len().saturating_sub(HISTORY_LIMIT))
    {
        let problem = problems.entry(&submission.problem).or_default();
        problem.tags.extend(
            submission
                .tags
                .iter()
                .map(|tag| tag.trim().to_ascii_lowercase())
                .filter(|tag| !tag.is_empty()),
        );
        problem.submissions.push(submission);
    }

    let mut recent: BTreeMap<String, (f64, f64)> = BTreeMap::new();
    let mut ratings: BTreeMap<String, Vec<u32>> = BTreeMap::new();
    for problem in problems.values() {
        let accepted = problem
            .submissions
            .iter()
            .filter(|submission| submission.verdict.as_deref() == Some("OK"))
            .count() as u32;
        let solved_rating = problem.submissions.iter().rev().find_map(|submission| {
            (submission.verdict.as_deref() == Some("OK"))
                .then_some(submission.rating)
                .flatten()
        });
        for tag in &problem.tags {
            let stats = profile.tags.entry(tag.clone()).or_default();
            stats.attempted += 1;
            stats.solved += u32::from(accepted > 0);
            stats.subs += problem.submissions.len() as u32;
            stats.ac_subs += accepted;
            if let Some(rating) = solved_rating {
                ratings.entry(tag.clone()).or_default().push(rating);
            }
            let totals = recent.entry(tag.clone()).or_default();
            for submission in &problem.submissions {
                let age = now.saturating_sub(submission.created_at);
                if age <= 90 * DAY {
                    let weight = 2.0_f64.powf(-(age as f64) / (30 * DAY) as f64);
                    totals.1 += weight;
                    if submission.verdict.as_deref() != Some("OK") {
                        totals.0 += weight;
                    }
                }
            }
        }
    }

    // The prior is the unweighted mean of observed per-tag weakness. Raw
    // solve-rate/efficiency statistics remain exactly those in §9; only the
    // combined weakness used for selection is shrunk toward the mean.
    for (tag, stats) in &mut profile.tags {
        stats.solve_rate = f64::from(stats.solved) / f64::from(stats.attempted);
        stats.efficiency = f64::from(stats.ac_subs) / f64::from(stats.subs);
        if let Some(&(failed, total)) = recent.get(tag) {
            if total > 0.0 {
                stats.recent_fail = failed / total;
            }
        }
        stats.weakness = 0.5 * (1.0 - stats.solve_rate)
            + 0.3 * (1.0 - stats.efficiency)
            + 0.2 * stats.recent_fail;
        if let Some(values) = ratings.get_mut(tag) {
            values.sort_unstable();
            let middle = values.len() / 2;
            stats.median_solved_rating = if values.len() % 2 == 0 {
                Some(((u64::from(values[middle - 1]) + u64::from(values[middle])) / 2) as u32)
            } else {
                Some(values[middle])
            };
        }
    }
    if profile.tags.is_empty() {
        return profile;
    }
    let mean = profile
        .tags
        .values()
        .map(|stats| stats.weakness)
        .sum::<f64>()
        / profile.tags.len() as f64;
    for stats in profile.tags.values_mut() {
        if stats.attempted < 3 {
            let observed = f64::from(stats.attempted);
            stats.weakness = (observed * stats.weakness + (3.0 - observed) * mean) / 3.0;
        }
    }
    let total: f64 = profile.tags.values().map(|stats| stats.weakness).sum();
    let uniform = 1.0 / profile.tags.len() as f64;
    for (tag, stats) in &profile.tags {
        profile.weights.insert(
            tag.clone(),
            if total > 0.0 {
                stats.weakness / total
            } else {
                uniform
            },
        );
    }
    profile
}

#[cfg(test)]
mod tests {
    use super::*;

    fn submission(id: u64, index: &str, tag: &str, verdict: &str, age_days: i64) -> Submission {
        Submission {
            id,
            problem: ProblemKey {
                contest_id: 1234,
                index: index.to_owned(),
            },
            created_at: 200 * DAY - age_days * DAY,
            verdict: Some(verdict.to_owned()),
            handles: vec!["player".to_owned()],
            tags: vec![tag.to_owned()],
            rating: Some(1000 + id as u32 * 100),
        }
    }

    #[test]
    fn known_history_orders_weak_tags_and_shrinks_sparse_evidence() {
        let history = vec![
            submission(1, "A", "math", "OK", 1),
            submission(2, "B", "math", "OK", 2),
            submission(3, "C", "math", "OK", 3),
            submission(4, "D", "dp", "WRONG_ANSWER", 1),
            submission(5, "E", "dp", "WRONG_ANSWER", 2),
            submission(6, "F", "dp", "WRONG_ANSWER", 3),
            submission(7, "G", "graphs", "WRONG_ANSWER", 1),
        ];
        let profile = analyze(&history, 200 * DAY);
        assert!(profile.weights["dp"] > profile.weights["graphs"]);
        assert!(profile.weights["graphs"] > profile.weights["math"]);
        assert!((profile.weights.values().sum::<f64>() - 1.0).abs() < 1e-12);
        assert_eq!(profile.tags["math"].median_solved_rating, Some(1200));
        assert_eq!(profile.tags["dp"].median_solved_rating, None);
        assert_eq!(profile.tags["graphs"].attempted, 1);
        assert!((profile.tags["dp"].weakness - 1.0).abs() < 1e-12);
        assert!(profile.tags["graphs"].weakness < 1.0);
        assert_eq!(profile.solved.len(), 3);
    }

    #[test]
    fn distinct_problems_and_submission_efficiency_are_different_measures() {
        let mut first = submission(1, "A", "dp", "WRONG_ANSWER", 1);
        first.tags.push("dp".to_owned());
        let history = vec![
            first,
            submission(2, "A", "dp", "OK", 0),
            submission(3, "A", "dp", "OK", 0),
            submission(4, "B", "dp", "WRONG_ANSWER", 0),
        ];
        let profile = analyze(&history, 200 * DAY);
        let stats = &profile.tags["dp"];
        assert_eq!(
            (stats.attempted, stats.solved, stats.subs, stats.ac_subs),
            (2, 1, 4, 2)
        );
        assert_eq!(stats.solve_rate, 0.5);
        assert_eq!(stats.efficiency, 0.5);
        assert_eq!(profile.solved.len(), 1);
    }

    #[test]
    fn recent_failures_have_a_thirty_day_half_life_and_ninety_day_cutoff() {
        let history = vec![
            submission(1, "A", "dp", "OK", 0),
            submission(2, "B", "dp", "WRONG_ANSWER", 30),
            submission(3, "C", "dp", "WRONG_ANSWER", 90),
            submission(4, "D", "dp", "WRONG_ANSWER", 91),
            submission(5, "E", "dp", "WRONG_ANSWER", -1),
        ];
        let profile = analyze(&history, 200 * DAY);
        assert!((profile.tags["dp"].recent_fail - 0.625 / 1.625).abs() < 1e-12);
        assert_eq!(profile.tags["dp"].attempted, 4);
    }

    #[test]
    fn deduplication_and_input_order_do_not_change_statistics() {
        let accepted = submission(1, "A", "math", "OK", 1);
        let mut pending = accepted.clone();
        pending.verdict = None;
        let mut history = vec![pending, accepted.clone(), accepted];
        let forward = analyze(&history, 200 * DAY);
        history.reverse();
        let reverse = analyze(&history, 200 * DAY);
        assert_eq!(forward.weights, reverse.weights);
        assert_eq!(forward.solved, reverse.solved);
        assert_eq!(forward.tags["math"].subs, 1);
        assert_eq!(forward.tags["math"].ac_subs, 1);
    }

    #[test]
    fn empty_and_all_successful_histories_have_uniform_fallbacks() {
        let empty = analyze(&[], 0);
        assert!(empty.solved.is_empty() && empty.weights.is_empty() && empty.tags.is_empty());
        let mut history = vec![
            submission(1, "A", "math", "OK", 0),
            submission(2, "B", "dp", "OK", 0),
        ];
        history[0].rating = Some(u32::MAX);
        history[1].tags.push("math".to_owned());
        let profile = analyze(&history, 200 * DAY);
        assert_eq!(profile.weights["math"], 0.5);
        assert_eq!(profile.weights["dp"], 0.5);
        assert_eq!(
            profile.tags["math"].median_solved_rating,
            Some(((u64::from(u32::MAX) + 1200) / 2) as u32)
        );
    }

    #[test]
    fn analysis_cap_does_not_forget_older_solved_problems() {
        let history: Vec<_> = (0..=10_000)
            .map(|id| Submission {
                id,
                problem: ProblemKey {
                    contest_id: 1234,
                    index: id.to_string(),
                },
                created_at: id as i64,
                verdict: Some(if id == 0 { "OK" } else { "WRONG_ANSWER" }.to_owned()),
                handles: vec![],
                tags: vec!["dp".to_owned()],
                rating: Some(1200),
            })
            .collect();
        let profile = analyze(&history, 10_000);
        assert_eq!(profile.tags["dp"].subs, 10_000);
        assert!(profile.solved.contains(&history[0].problem));
    }
}
