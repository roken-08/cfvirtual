use cfvirtual::domain::{bots::generate, ContestProblem, Problem, ProblemKey, Tier};
use std::collections::HashSet;

fn problems(rating: u32) -> Vec<ContestProblem> {
    (0..26)
        .map(|index| {
            let slot = char::from(b'A' + index);
            ContestProblem {
                slot,
                tier: Tier::Core,
                problem: Problem {
                    key: ProblemKey {
                        contest_id: 1234,
                        index: slot.to_string(),
                    },
                    name: format!("Synthetic problem {slot}"),
                    rating: Some(rating),
                    tags: vec!["dp".to_owned()],
                    solved_count: None,
                },
            }
        })
        .collect()
}

#[test]
fn bot_generation_is_seeded_including_names_ratings_and_schedules() {
    let problems = problems(1200);
    let bots = generate(16, 1200, &problems, 7200, 1234);
    assert_eq!(bots, generate(16, 1200, &problems, 7200, 1234));
    assert_ne!(bots, generate(16, 1200, &problems, 7200, 1235));
    assert_eq!(bots.len(), 16);
}

#[test]
fn names_are_unique_synthetic_adjective_noun_two_digit_strings() {
    let bots = generate(1000, 1200, &[], 7200, 12);
    let names: HashSet<_> = bots.iter().map(|bot| &bot.name).collect();
    assert_eq!(names.len(), bots.len());
    for bot in bots {
        let parts: Vec<_> = bot.name.split('_').collect();
        assert_eq!(parts.len(), 3);
        assert!(parts[0].bytes().all(|byte| byte.is_ascii_lowercase()));
        assert!(parts[1].bytes().all(|byte| byte.is_ascii_lowercase()));
        assert_eq!(parts[2].len(), 2);
        assert!(parts[2].bytes().all(|byte| byte.is_ascii_digit()));
    }
}

#[test]
fn schedules_are_chronological_unique_and_inside_duration() {
    for duration in [1, 90, 120, 7200] {
        let bots = generate(64, 1200, &problems(1200), duration, 4321);
        for bot in bots {
            assert!((950..=1450).contains(&bot.rating));
            assert!(bot
                .solves
                .windows(2)
                .all(|pair| pair[0].accepted_at_sec <= pair[1].accepted_at_sec));
            assert_eq!(
                bot.solves
                    .iter()
                    .map(|event| event.slot)
                    .collect::<HashSet<_>>()
                    .len(),
                bot.solves.len()
            );
            for event in bot.solves {
                assert!((duration.min(300)..=duration).contains(&event.accepted_at_sec));
                assert!(event.rejected_attempts <= 3);
            }
        }
    }
}

#[test]
fn logistic_probability_has_correct_direction_and_lognormal_has_spread() {
    // A broad deterministic population detects an inverted logistic or an
    // accidentally constant solve time without flaky single-bot assertions.
    let easy = generate(256, 1800, &problems(800), 7200, 77);
    let hard = generate(256, 1800, &problems(3000), 7200, 77);
    let easy_solves: usize = easy.iter().map(|bot| bot.solves.len()).sum();
    let hard_solves: usize = hard.iter().map(|bot| bot.solves.len()).sum();
    assert!(easy_solves > hard_solves * 5);
    let varied = generate(256, 1200, &problems(1200), 7200, 88);
    let times: HashSet<_> = varied
        .iter()
        .flat_map(|bot| bot.solves.iter().map(|event| event.accepted_at_sec))
        .collect();
    assert!(times.len() > 100);
    let total: usize = varied.iter().map(|bot| bot.solves.len()).sum();
    let rejected = varied
        .iter()
        .flat_map(|bot| &bot.solves)
        .filter(|event| event.rejected_attempts > 0)
        .count();
    let fraction = rejected as f64 / total as f64;
    assert!((0.20..0.30).contains(&fraction));
}

#[test]
fn stronger_bots_can_create_tight_five_to_ten_minute_opening_race() {
    let bots = generate(256, 1200, &problems(800), 7200, 2026);
    assert!(bots
        .iter()
        .flat_map(|bot| bot.solves.iter())
        .any(|event| (300..=600).contains(&event.accepted_at_sec)));
    assert!(bots
        .iter()
        .flat_map(|bot| bot.solves.iter())
        .any(|event| event.accepted_at_sec >= 1_200));
}

#[test]
fn rating_clamps_empty_inputs_and_zero_duration_are_safe() {
    for rating in [0, 800, 3500, u32::MAX] {
        let bots = generate(64, rating, &problems(1200), 0, 1);
        assert!(bots
            .iter()
            .all(|bot| (800..=3500).contains(&bot.rating) && bot.solves.is_empty()));
    }
    assert!(generate(0, 1200, &problems(1200), 7200, 1).is_empty());
    assert!(generate(5, 1200, &[], 7200, 1)
        .iter()
        .all(|bot| bot.solves.is_empty()));
}
