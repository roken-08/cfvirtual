use cfvirtual::domain::{
    scoring::score, Bot, BotSolve, Contest, ContestProblem, Participant, Problem, ProblemKey,
    Standing, Submission, Tier,
};

fn key(slot: char) -> ProblemKey {
    ProblemKey {
        contest_id: 1234,
        index: slot.to_string(),
    }
}

fn participant(handle: &str) -> Participant {
    Participant {
        handle: handle.to_owned(),
        is_host: handle == "player",
        is_spectator: false,
        token: String::new(),
        connected: true,
    }
}

fn contest() -> Contest {
    Contest {
        id: "test-contest".to_owned(),
        code: None,
        host_handle: "player".to_owned(),
        host_rating: 1200,
        started_at: 1000,
        duration_sec: 3600,
        seed: 1,
        mode: "solo".to_owned(),
        blind: false,
        ended_at: None,
        participants: vec![participant("player")],
        bots: vec![],
        problems: ['A', 'B', 'C']
            .into_iter()
            .map(|slot| ContestProblem {
                slot,
                tier: Tier::Core,
                problem: Problem {
                    key: key(slot),
                    name: format!("Problem {slot}"),
                    rating: Some(1200),
                    tags: vec!["dp".to_owned()],
                    solved_count: None,
                },
            })
            .collect(),
    }
}

fn submission(id: u64, slot: char, elapsed: i64, verdict: &str, handle: &str) -> Submission {
    Submission {
        id,
        problem: key(slot),
        created_at: 1000 + elapsed,
        verdict: Some(verdict.to_owned()),
        handles: vec![handle.to_owned()],
        tags: vec![],
        rating: None,
    }
}

fn row<'a>(standings: &'a [Standing], name: &str) -> &'a Standing {
    standings
        .iter()
        .find(|standing| standing.name == name)
        .unwrap()
}

#[test]
fn chronological_first_ac_counts_only_prior_rejections_and_tracks_attempts() {
    let submissions = vec![
        submission(5, 'A', 500, "WRONG_ANSWER", "player"),
        submission(3, 'A', 300, "OK", "player"),
        submission(2, 'A', 200, "TIME_LIMIT_EXCEEDED", "player"),
        submission(1, 'A', 100, "WRONG_ANSWER", "player"),
        submission(4, 'A', 400, "OK", "player"),
        submission(6, 'B', 600, "WRONG_ANSWER", "player"),
    ];
    let standings = score(&contest(), &submissions, 4600);
    let player = &standings[0];
    assert_eq!(
        (player.solved, player.penalty, player.last_ac),
        (1, 2700, 300)
    );
    let status = &player.problems[&'A'];
    assert_eq!((status.attempts, status.rejected), (3, 2));
    assert_eq!(status.accepted_at_sec, Some(300));
    assert_eq!(status.first_attempt_at_sec, Some(100));
    assert_eq!(player.problems[&'B'].rejected, 1);
    assert!(!player.problems[&'B'].solved);
}

#[test]
fn start_and_deadline_are_inclusive_other_times_and_problem_keys_are_excluded() {
    let submissions = vec![
        submission(1, 'A', -1, "OK", "player"),
        submission(2, 'A', 0, "OK", "player"),
        submission(3, 'B', 3600, "OK", "player"),
        submission(4, 'C', 3601, "OK", "player"),
        submission(5, 'Z', 10, "OK", "player"),
        submission(6, 'C', 10, "OK", "outsider"),
    ];
    let standings = score(&contest(), &submissions, 5000);
    assert_eq!((standings[0].solved, standings[0].penalty), (2, 3600));
    assert_eq!(standings[0].problems[&'A'].accepted_at_sec, Some(0));
    assert_eq!(standings[0].problems[&'B'].accepted_at_sec, Some(3600));
    assert_eq!(standings[0].problems[&'C'].attempts, 0);
}

#[test]
fn now_and_recorded_early_end_are_authoritative() {
    let mut contest = contest();
    contest.ended_at = Some(1500);
    let submissions = vec![
        submission(1, 'A', 500, "OK", "player"),
        submission(2, 'B', 501, "OK", "player"),
    ];
    assert_eq!(score(&contest, &submissions, 1499)[0].solved, 0);
    assert_eq!(score(&contest, &submissions, 1500)[0].solved, 1);
    assert_eq!(score(&contest, &submissions, 4600)[0].solved, 1);
    assert_eq!(score(&contest, &submissions, 999)[0].solved, 0);
    contest.ended_at = Some(9000);
    assert_eq!(
        score(&contest, &[submission(3, 'C', 3601, "OK", "player")], 9000)[0].solved,
        0
    );
}

#[test]
fn pending_skipped_and_compile_errors_do_not_count_as_attempts() {
    let mut pending = submission(1, 'A', 10, "", "player");
    pending.verdict = None;
    let submissions = vec![
        pending,
        submission(2, 'A', 20, "TESTING", "player"),
        submission(3, 'A', 30, "COMPILATION_ERROR", "player"),
        submission(4, 'A', 40, "SKIPPED", "player"),
        submission(5, 'A', 50, "OK", "player"),
    ];
    let standings = score(&contest(), &submissions, 4600);
    assert_eq!(standings[0].penalty, 50);
    assert_eq!(standings[0].problems[&'A'].attempts, 1);
    assert_eq!(standings[0].problems[&'A'].first_attempt_at_sec, Some(50));
}

#[test]
fn repeated_poll_ids_and_pending_snapshots_do_not_duplicate_penalties() {
    let wrong = submission(1, 'A', 10, "WRONG_ANSWER", "player");
    let accepted = submission(2, 'A', 20, "OK", "player");
    let mut pending = accepted.clone();
    pending.verdict = None;
    let mut submissions = vec![wrong.clone(), accepted.clone(), pending, wrong, accepted];
    let forward = score(&contest(), &submissions, 4600);
    submissions.reverse();
    assert_eq!(forward, score(&contest(), &submissions, 4600));
    assert_eq!((forward[0].solved, forward[0].penalty), (1, 1220));
    assert_eq!(forward[0].problems[&'A'].attempts, 2);
}

#[test]
fn simultaneous_submissions_use_id_order_and_conflicting_duplicates_prefer_ac() {
    let submissions = vec![
        submission(3, 'A', 100, "WRONG_ANSWER", "player"),
        submission(2, 'A', 100, "OK", "player"),
        submission(1, 'A', 100, "WRONG_ANSWER", "player"),
        submission(2, 'A', 100, "WRONG_ANSWER", "player"),
    ];
    let standings = score(&contest(), &submissions, 4600);
    assert_eq!(standings[0].penalty, 1300);
    assert_eq!(standings[0].problems[&'A'].rejected, 1);
}

#[test]
fn roster_is_case_insensitive_spectators_excluded_and_team_members_count_once() {
    let mut contest = contest();
    let mut spectator = participant("spectator");
    spectator.is_spectator = true;
    let mut disconnected = participant("teammate");
    disconnected.connected = false;
    contest
        .participants
        .extend([participant("PLAYER"), spectator, disconnected]);
    let mut accepted = submission(1, 'A', 100, "OK", "pLaYeR");
    accepted.handles.extend([
        "PLAYER".to_owned(),
        "spectator".to_owned(),
        "teammate".to_owned(),
    ]);
    let standings = score(&contest, &[accepted], 4600);
    assert_eq!(standings.len(), 2);
    assert_eq!(row(&standings, "player").solved, 1);
    assert_eq!(row(&standings, "player").problems[&'A'].attempts, 1);
    assert_eq!(row(&standings, "teammate").solved, 1);
}

#[test]
fn standings_order_is_solved_then_penalty_then_last_ac_then_name() {
    let mut contest = contest();
    contest.participants = ["zeta", "alpha", "beta", "gamma", "delta", "zero"]
        .into_iter()
        .map(participant)
        .collect();
    let submissions = vec![
        submission(1, 'A', 100, "OK", "alpha"),
        submission(2, 'B', 300, "OK", "alpha"),
        submission(3, 'A', 200, "OK", "beta"),
        submission(4, 'B', 200, "OK", "beta"),
        submission(5, 'A', 200, "OK", "gamma"),
        submission(6, 'B', 200, "OK", "gamma"),
        submission(7, 'A', 1, "OK", "delta"),
        submission(8, 'B', 1, "OK", "delta"),
        submission(9, 'C', 3500, "OK", "delta"),
        submission(10, 'A', 2, "OK", "zeta"),
    ];
    let standings = score(&contest, &submissions, 4600);
    assert_eq!(
        standings
            .iter()
            .map(|standing| standing.name.as_str())
            .collect::<Vec<_>>(),
        ["delta", "beta", "gamma", "alpha", "zeta", "zero"]
    );
}

#[test]
fn bot_events_are_hidden_until_elapsed_and_use_identical_icpc_penalties() {
    let mut contest = contest();
    contest.bots.push(Bot {
        name: "amber_kite_07".to_owned(),
        rating: 1300,
        solves: vec![
            BotSolve {
                slot: 'B',
                accepted_at_sec: 400,
                rejected_attempts: 0,
            },
            BotSolve {
                slot: 'A',
                accepted_at_sec: 200,
                rejected_attempts: 2,
            },
            BotSolve {
                slot: 'A',
                accepted_at_sec: 300,
                rejected_attempts: 3,
            },
            BotSolve {
                slot: 'Z',
                accepted_at_sec: 100,
                rejected_attempts: 0,
            },
            BotSolve {
                slot: 'C',
                accepted_at_sec: 3601,
                rejected_attempts: 0,
            },
        ],
    });
    let before = score(&contest, &[], 1199);
    let bot = row(&before, "amber_kite_07");
    assert!(bot.is_bot);
    assert_eq!((bot.solved, bot.problems[&'A'].attempts), (0, 0));
    let at = score(&contest, &[], 1200);
    let bot = row(&at, "amber_kite_07");
    assert_eq!(
        (bot.solved, bot.penalty, bot.problems[&'A'].attempts),
        (1, 2600, 3)
    );
    assert_eq!(bot.problems[&'A'].first_attempt_at_sec, None);
    let finished = score(&contest, &[], 9999);
    let bot = row(&finished, "amber_kite_07");
    assert_eq!((bot.solved, bot.penalty, bot.last_ac), (2, 3000, 400));
    assert_eq!(bot.problems[&'B'].first_attempt_at_sec, Some(400));
    contest.ended_at = Some(1300);
    assert_eq!(row(&score(&contest, &[], 9999), "amber_kite_07").solved, 1);
}

#[test]
fn bot_at_zero_and_deadline_obeys_start_and_end_boundaries() {
    let mut contest = contest();
    contest.bots.push(Bot {
        name: "silent_comet_42".to_owned(),
        rating: 1200,
        solves: vec![
            BotSolve {
                slot: 'A',
                accepted_at_sec: 0,
                rejected_attempts: 0,
            },
            BotSolve {
                slot: 'B',
                accepted_at_sec: 3600,
                rejected_attempts: 1,
            },
        ],
    });
    for (now, solved) in [(999, 0), (1000, 1), (4599, 1), (4600, 2)] {
        assert_eq!(
            row(&score(&contest, &[], now), "silent_comet_42").solved,
            solved
        );
    }
}

#[test]
fn extreme_clock_values_and_penalties_do_not_overflow() {
    let mut contest = contest();
    contest.started_at = i64::MAX - 1;
    contest.bots.push(Bot {
        name: "ivory_fern_01".to_owned(),
        rating: 1200,
        solves: vec![BotSolve {
            slot: 'A',
            accepted_at_sec: 1,
            rejected_attempts: u32::MAX,
        }],
    });
    let standings = score(&contest, &[], i64::MAX);
    let bot = row(&standings, "ivory_fern_01");
    assert_eq!(bot.penalty, 1 + 1200 * u64::from(u32::MAX));
}
