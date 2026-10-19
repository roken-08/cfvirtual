//! Authoritative ICPC scoring. Both endpoints of the contest window are inclusive.
//!
//! Pending (`None`/`TESTING`), skipped, and compilation-error submissions do not
//! count as attempts or penalties. Everything after a problem's first AC is
//! ignored. Handles are matched and ordered case-insensitively. Duplicate IDs
//! prefer a judged result to a pending snapshot, and AC to other results; ties
//! are resolved by submission contents rather than arrival order.

use super::{Contest, ProblemStatus, Standing, Submission};
use std::collections::{BTreeMap, BTreeSet};

pub fn score(contest: &Contest, submissions: &[Submission], now: i64) -> Vec<Standing> {
    let mut standings = Vec::new();
    let mut humans = BTreeMap::new();
    let empty_statuses: BTreeMap<_, _> = contest
        .problems
        .iter()
        .map(|problem| (problem.slot, ProblemStatus::default()))
        .collect();

    for participant in &contest.participants {
        let handle = participant.handle.to_ascii_lowercase();
        if participant.is_spectator || humans.contains_key(&handle) {
            continue;
        }
        humans.insert(handle, standings.len());
        standings.push(Standing {
            name: participant.handle.clone(),
            is_bot: false,
            // Participant has no rating field. The host's rating is the
            // contest's shared difficulty baseline, not a fetched user rating.
            rating: contest.host_rating,
            solved: 0,
            penalty: 0,
            last_ac: 0,
            problems: empty_statuses.clone(),
        });
    }

    let deadline = contest
        .started_at
        .saturating_add(i64::from(contest.duration_sec));
    let cutoff = now.min(deadline).min(contest.ended_at.unwrap_or(deadline));
    let slots: BTreeMap<_, _> = contest
        .problems
        .iter()
        .map(|problem| (&problem.problem.key, problem.slot))
        .collect();

    for submission in deduplicated(submissions) {
        if submission.created_at < contest.started_at || submission.created_at > cutoff {
            continue;
        }
        let Some(&slot) = slots.get(&submission.problem) else {
            continue;
        };
        let Some(verdict) = judged_verdict(submission) else {
            continue;
        };
        let elapsed = submission.created_at.saturating_sub(contest.started_at) as u32;
        let mut seen_handles = BTreeSet::new();
        for handle in &submission.handles {
            let handle = handle.to_ascii_lowercase();
            if !seen_handles.insert(handle.clone()) {
                continue;
            }
            let Some(&index) = humans.get(&handle) else {
                continue;
            };
            let Some(status) = standings[index].problems.get_mut(&slot) else {
                continue;
            };
            if status.solved {
                continue;
            }
            status.attempts = status.attempts.saturating_add(1);
            status.first_attempt_at_sec.get_or_insert(elapsed);
            if verdict == "OK" {
                status.solved = true;
                status.accepted_at_sec = Some(elapsed);
            } else {
                status.rejected = status.rejected.saturating_add(1);
            }
        }
    }

    let mut bot_names = BTreeSet::new();
    for bot in &contest.bots {
        if !bot_names.insert(bot.name.to_ascii_lowercase()) {
            continue;
        }
        let mut standing = Standing {
            name: bot.name.clone(),
            is_bot: true,
            rating: bot.rating,
            solved: 0,
            penalty: 0,
            last_ac: 0,
            problems: empty_statuses.clone(),
        };
        let mut events: Vec<_> = bot.solves.iter().collect();
        events.sort_by_key(|event| (event.accepted_at_sec, event.slot, event.rejected_attempts));
        for event in events {
            if event.accepted_at_sec > contest.duration_sec
                || cutoff < contest.started_at
                || i64::from(event.accepted_at_sec) > cutoff.saturating_sub(contest.started_at)
            {
                continue;
            }
            let Some(status) = standing.problems.get_mut(&event.slot) else {
                continue;
            };
            if status.solved {
                continue;
            }
            status.solved = true;
            status.accepted_at_sec = Some(event.accepted_at_sec);
            status.rejected = event.rejected_attempts;
            status.attempts = event.rejected_attempts.saturating_add(1);
            // BotSolve records one aggregate event, not rejection timestamps.
            // Reveal its attempts only at AC; do not invent first-attempt times.
            if event.rejected_attempts == 0 {
                status.first_attempt_at_sec = Some(event.accepted_at_sec);
            }
        }
        standings.push(standing);
    }

    for standing in &mut standings {
        for status in standing.problems.values() {
            if let Some(accepted) = status.accepted_at_sec.filter(|_| status.solved) {
                standing.solved = standing.solved.saturating_add(1);
                standing.penalty = standing
                    .penalty
                    .saturating_add(u64::from(accepted) + 1_200 * u64::from(status.rejected));
                standing.last_ac = standing.last_ac.max(accepted);
            }
        }
    }
    standings.sort_by(|left, right| {
        right
            .solved
            .cmp(&left.solved)
            .then_with(|| left.penalty.cmp(&right.penalty))
            .then_with(|| left.last_ac.cmp(&right.last_ac))
            .then_with(|| {
                left.name
                    .to_ascii_lowercase()
                    .cmp(&right.name.to_ascii_lowercase())
            })
            .then_with(|| left.name.cmp(&right.name))
            .then_with(|| left.is_bot.cmp(&right.is_bot))
    });
    standings
}

pub(super) fn judged_verdict(submission: &Submission) -> Option<&str> {
    match submission.verdict.as_deref() {
        None | Some("TESTING" | "COMPILATION_ERROR" | "SKIPPED" | "") => None,
        verdict => verdict,
    }
}

fn verdict_priority(submission: &Submission) -> u8 {
    match judged_verdict(submission) {
        Some("OK") => 2,
        Some(_) => 1,
        None => 0,
    }
}

/// Stable across repeated polls, input order, and pending-to-judged transitions.
pub(super) fn deduplicated(submissions: &[Submission]) -> Vec<&Submission> {
    let mut ordered: Vec<_> = submissions.iter().collect();
    ordered.sort_by(|left, right| {
        left.id
            .cmp(&right.id)
            .then_with(|| verdict_priority(right).cmp(&verdict_priority(left)))
            .then_with(|| left.created_at.cmp(&right.created_at))
            .then_with(|| left.problem.cmp(&right.problem))
            .then_with(|| left.verdict.cmp(&right.verdict))
            .then_with(|| left.handles.cmp(&right.handles))
            .then_with(|| left.tags.cmp(&right.tags))
            .then_with(|| left.rating.cmp(&right.rating))
    });
    ordered.dedup_by_key(|submission| submission.id);
    ordered.sort_by_key(|submission| (submission.created_at, submission.id));
    ordered
}
