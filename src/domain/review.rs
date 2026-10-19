//! Reproducible Markdown derived solely from a contest and its scored snapshot.
//! No wall clock, inferred time-on-task, or unrecorded queue state is used.

use super::{Contest, Standing};
use chrono::{DateTime, Utc};
use std::{collections::BTreeMap, fmt::Write};

/// A failed/skipped review resets to one day. Success advances to the next
/// strictly larger step, saturating at 30; a new (zero-day) entry starts at one.
pub fn next_interval(current_days: u32, solved: bool) -> u32 {
    if !solved {
        return 1;
    }
    [1, 3, 7, 14, 30]
        .into_iter()
        .find(|days| *days > current_days)
        .unwrap_or(30)
}

pub fn markdown(contest: &Contest, standings: &[Standing], handle: &str) -> String {
    let selected = standings
        .iter()
        .enumerate()
        .find(|(_, standing)| !standing.is_bot && standing.name.eq_ignore_ascii_case(handle))
        .or_else(|| {
            standings
                .iter()
                .enumerate()
                .find(|(_, standing)| standing.name.eq_ignore_ascii_case(handle))
        });
    let timestamp = DateTime::<Utc>::from_timestamp(contest.started_at, 0)
        .map(|time| time.format("%Y-%m-%d %H:%M UTC").to_string())
        .unwrap_or_else(|| format!("Unix {}", contest.started_at));
    let mut output = String::new();
    // Writing into String cannot fail; ignore only fmt::Result, never input errors.
    let _ = writeln!(output, "# CFVirtual review — {timestamp}\n");
    let _ = writeln!(
        output,
        "Mode: {} · Host: {} · Duration: {}",
        escape(&contest.mode),
        escape(&contest.host_handle),
        duration(contest.duration_sec)
    );
    if let Some(code) = &contest.code {
        let _ = writeln!(output, "Room: {}", escape(code));
    }
    let _ = writeln!(
        output,
        "Contest: {} · Seed: {}",
        escape(&contest.id),
        contest.seed
    );
    if let Some((rank, standing)) = selected {
        let _ = writeln!(
            output,
            "Player: {}{}\nProblems: {} · Solved: {}/{} · Penalty: {} s · Rank: {}/{}\n",
            escape(&standing.name),
            if standing.is_bot { " (bot)" } else { "" },
            contest.problems.len(),
            standing.solved,
            contest.problems.len(),
            standing.penalty,
            rank + 1,
            standings.len()
        );
    } else {
        let _ = writeln!(
            output,
            "Player: {}\nProblems: {} · Solved: unavailable · Penalty: unavailable · Rank: unavailable/{}\n",
            escape(handle), contest.problems.len(), standings.len()
        );
    }
    if let Some(ended_at) = contest.ended_at {
        let effective = ended_at.min(
            contest
                .started_at
                .saturating_add(i64::from(contest.duration_sec)),
        );
        let elapsed = effective.saturating_sub(contest.started_at).max(0) as u32;
        let _ = writeln!(
            output,
            "Recorded finish: {} after start.\n",
            duration(elapsed)
        );
    }
    let standing = selected.map(|(_, standing)| standing);
    let baseline = standing
        .map(|standing| standing.rating)
        .filter(|rating| *rating > 0)
        .unwrap_or(contest.host_rating);
    let _ = writeln!(
        output,
        "| Slot | Problem | Rating | Tags | Result | Time | Tries | Assessment |\n|------|---------|--------|------|--------|------|-------|------------|"
    );
    let mut focus = BTreeMap::<&str, usize>::new();
    let mut unsolved = Vec::new();
    let mut most_attempts: Option<(char, u32)> = None;
    for problem in &contest.problems {
        let status = standing.and_then(|standing| standing.problems.get(&problem.slot));
        let result = match status {
            Some(status) if status.solved => "AC",
            Some(status) if status.rejected > 0 => "WA",
            Some(_) => "unsolved",
            None => "unavailable",
        };
        let accepted = status
            .and_then(|status| status.accepted_at_sec)
            .map(elapsed)
            .unwrap_or_else(|| "—".to_owned());
        let attempts = status
            .map(|status| status.attempts.to_string())
            .unwrap_or_else(|| "—".to_owned());
        let assessment = match problem.problem.rating {
            Some(rating) if i64::from(rating) <= i64::from(baseline) - 200 => "should solve",
            Some(rating) if i64::from(rating) > i64::from(baseline) + 200 => "stretch",
            Some(_) => "at level",
            None => "unrated",
        };
        let rating = problem
            .problem
            .rating
            .map(|rating| rating.to_string())
            .unwrap_or_else(|| "—".to_owned());
        let _ = writeln!(
            output,
            "| {} | [{} {}]({}) | {} | {} | {} | {} | {} | {} |",
            problem.slot,
            problem.problem.key,
            escape(&problem.problem.name),
            problem.problem.url(),
            rating,
            escape(&problem.problem.tags.join(", ")),
            result,
            accepted,
            attempts,
            assessment
        );
        if let Some(status) = status {
            if most_attempts.is_none_or(|(_, count)| status.attempts > count) {
                most_attempts = Some((problem.slot, status.attempts));
            }
            if !status.solved {
                unsolved.push(problem);
                // One vote per tag per unsolved problem, even for repeated tags.
                let mut tags: Vec<_> = problem.problem.tags.iter().map(String::as_str).collect();
                tags.sort_unstable();
                tags.dedup();
                for tag in tags {
                    *focus.entry(tag).or_default() += 1;
                }
            }
        }
    }
    let _ = writeln!(
        output,
        "\nTimes are elapsed from the authoritative start; penalties are seconds, including 1,200 s per rejection before AC. Tries exclude pending, skipped, and compilation-error submissions, and submissions after AC. Assessments use rating baseline {baseline}: at least 200 below = should solve; over 200 above = stretch. Human ratings use the contest host baseline.\n\n## Standings\n\n| Rank | Player | Solved | Penalty (s) | Last AC |\n|------|--------|--------|-------------|---------|"
    );
    for (index, standing) in standings.iter().enumerate() {
        let last = if standing.solved > 0 {
            elapsed(standing.last_ac)
        } else {
            "—".to_owned()
        };
        let _ = writeln!(
            output,
            "| {} | {}{} | {} | {} | {} |",
            index + 1,
            escape(&standing.name),
            if standing.is_bot { " (bot)" } else { "" },
            standing.solved,
            standing.penalty,
            last
        );
    }
    let _ = writeln!(output, "\n## Where time went\n");
    if let Some((slot, attempts)) = most_attempts.filter(|(_, attempts)| *attempts > 0) {
        let _ = writeln!(output, "- Most attempts: {slot} ({attempts}).");
    } else {
        let _ = writeln!(output, "- No scored attempts recorded for this player.");
    }
    if let Some(standing) = standing.filter(|standing| standing.solved > 0) {
        let _ = writeln!(
            output,
            "- Last acceptance: {} after start.",
            elapsed(standing.last_ac)
        );
    }
    let _ = writeln!(
        output,
        "- Time-on-task and longest-unsolved effort are not recorded; submission timestamps do not measure thinking time.\n\n## Next focus\n"
    );
    let mut focus: Vec<_> = focus.into_iter().collect();
    focus.sort_by(|left, right| right.1.cmp(&left.1).then_with(|| left.0.cmp(right.0)));
    if !focus.is_empty() {
        let tags = focus
            .iter()
            .map(|(tag, _)| escape(tag))
            .collect::<Vec<_>>()
            .join(", ");
        let _ = writeln!(output, "- Tags to revisit (unsolved problems): {tags}.");
    } else if !contest.problems.is_empty()
        && standing.is_some_and(|standing| {
            contest.problems.iter().all(|problem| {
                standing
                    .problems
                    .get(&problem.slot)
                    .is_some_and(|status| status.solved)
            })
        })
    {
        let _ = writeln!(
            output,
            "- All recorded problems solved; try a modest difficulty increase next contest."
        );
    } else {
        let _ = writeln!(
            output,
            "- No per-tag recommendation is available from the recorded results."
        );
    }
    if !unsolved.is_empty() {
        let suggested = unsolved
            .iter()
            .map(|problem| format!("[{}]({})", problem.problem.key, problem.problem.url()))
            .collect::<Vec<_>>()
            .join(", ");
        let _ = writeln!(output, "- Suggested review problems: {suggested}.");
        let _ = writeln!(
            output,
            "- Suggested review intervals: 1, 3, 7, 14, 30 days; a failed review resets to 1 day."
        );
    }
    let _ = writeln!(output, "- Queue persistence/status is not present in this model; recommendations above do not confirm queue entries.\n\n## Links\n");
    for problem in &contest.problems {
        let _ = writeln!(output, "- {}: {}", problem.slot, problem.problem.url());
    }
    output
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('\\', "\\\\")
        .replace('|', "\\|")
        .replace('[', "\\[")
        .replace(']', "\\]")
        .replace('*', "\\*")
        .replace('_', "\\_")
        .replace('`', "\\`")
        .replace(['\r', '\n'], " ")
}

fn elapsed(seconds: u32) -> String {
    format!("{}m {:02}s", seconds / 60, seconds % 60)
}

fn duration(seconds: u32) -> String {
    if seconds.is_multiple_of(60) {
        format!("{} min", seconds / 60)
    } else {
        elapsed(seconds)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{
        scoring, Bot, BotSolve, ContestProblem, Participant, Problem, ProblemKey, Submission, Tier,
    };

    fn contest() -> Contest {
        Contest {
            id: "test-contest".to_owned(),
            code: None,
            host_handle: "player".to_owned(),
            host_rating: 1200,
            started_at: 0,
            duration_sec: 7200,
            seed: 42,
            mode: "solo".to_owned(),
            blind: true,
            ended_at: None,
            problems: ['A', 'B']
                .into_iter()
                .map(|slot| ContestProblem {
                    slot,
                    tier: Tier::Core,
                    problem: Problem {
                        key: ProblemKey {
                            contest_id: 1234,
                            index: slot.to_string(),
                        },
                        name: format!("Problem {slot}"),
                        rating: Some(if slot == 'A' { 1000 } else { 1500 }),
                        tags: vec!["dp".to_owned()],
                        solved_count: None,
                    },
                })
                .collect(),
            participants: vec![Participant {
                handle: "player".to_owned(),
                is_host: true,
                is_spectator: false,
                token: "private-session-token".to_owned(),
                connected: true,
            }],
            bots: vec![Bot {
                name: "amber_kite_07".to_owned(),
                rating: 1300,
                solves: vec![BotSolve {
                    slot: 'A',
                    accepted_at_sec: 300,
                    rejected_attempts: 0,
                }],
            }],
        }
    }

    #[test]
    fn review_intervals_advance_saturate_and_reset() {
        for (current, expected) in [
            (0, 1),
            (1, 3),
            (3, 7),
            (7, 14),
            (14, 30),
            (30, 30),
            (u32::MAX, 30),
            (2, 3),
        ] {
            assert_eq!(next_interval(current, true), expected);
            assert_eq!(next_interval(current, false), 1);
        }
    }

    #[test]
    fn markdown_contains_full_reproducible_summary_and_no_invented_activity() {
        let contest = contest();
        let submissions = [Submission {
            id: 1,
            problem: contest.problems[0].problem.key.clone(),
            created_at: 360,
            verdict: Some("OK".to_owned()),
            handles: vec!["PLAYER".to_owned()],
            tags: vec![],
            rating: None,
        }];
        let standings = scoring::score(&contest, &submissions, 7200);
        let markdown = markdown(&contest, &standings, "PLAYER");
        assert_eq!(markdown, super::markdown(&contest, &standings, "PLAYER"));
        for expected in [
            "1970-01-01 00:00 UTC",
            "Mode: solo",
            "Duration: 120 min",
            "Solved: 1/2",
            "Penalty: 360 s",
            "Rank: 2/2",
            "| Slot | Problem | Rating | Tags | Result | Time | Tries | Assessment |",
            "| AC | 6m 00s | 1 | should solve |",
            "| unsolved | — | 0 | stretch |",
            "## Where time went",
            "## Next focus",
            "## Links",
            "(bot)",
            "https://codeforces.com/problemset/problem/1234/B",
            "Suggested review problems:",
            "Time-on-task and longest-unsolved effort are not recorded",
            "Seed: 42",
        ] {
            assert!(
                markdown.contains(expected),
                "missing {expected:?} in {markdown}"
            );
        }
        assert!(!markdown.contains("Queued for review:"));
        assert!(!markdown.contains("private-session-token"));
    }

    #[test]
    fn missing_results_and_markdown_metacharacters_are_handled_honestly() {
        let mut contest = contest();
        contest.problems[0].problem.name = "A | [link]\n<script>".to_owned();
        let output = markdown(&contest, &[], "spectator");
        assert!(output.contains("Solved: unavailable"));
        assert!(output.contains("A \\| \\[link\\] &lt;script&gt;"));
        assert!(!output.contains("All recorded problems solved"));
    }
}
