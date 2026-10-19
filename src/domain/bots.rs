//! Pure, seeded synthetic opponents. Solve times are log-normal with the
//! specified difficulty-dependent median (in minutes), sigma 0.45, and a
//! two-minute floor, shortened to the duration for sub-two-minute demos.
//! Rejections are aggregate counts attached to an AC event, not timed events.

use super::{Bot, BotSolve, ContestProblem};
use rand::{Rng, SeedableRng};
use rand_chacha::ChaCha8Rng;
use std::{collections::BTreeSet, f64::consts::TAU};

const ADJECTIVES: [&str; 16] = [
    "amber", "brisk", "cosmic", "dappled", "frosty", "gentle", "hollow", "ivory", "lunar",
    "mellow", "misty", "quaint", "silent", "silver", "velvet", "violet",
];
const NOUNS: [&str; 16] = [
    "comet", "kite", "otter", "badger", "willow", "nebula", "pebble", "lantern", "meadow",
    "quartz", "raven", "thistle", "tulip", "acorn", "moth", "fern",
];

pub fn generate(
    count: usize,
    rating: u32,
    problems: &[ContestProblem],
    duration_sec: u32,
    seed: u64,
) -> Vec<Bot> {
    let mut rng = ChaCha8Rng::seed_from_u64(seed);
    let namespace = ADJECTIVES.len() * NOUNS.len() * 100;
    let offset = rng.gen_range(0..namespace);
    let mut bots = Vec::with_capacity(count);
    for index in 0..count {
        // 7919 is coprime to the 25,600-name namespace: no rejection loop or
        // collisions. Beyond it, fictional echo-prefixed compound nouns extend
        // the namespace while retaining adjective_noun_2digit formatting.
        let name_index = (offset + (index % namespace) * 7_919) % namespace;
        let adjective = ADJECTIVES[name_index / (NOUNS.len() * 100)];
        let noun = NOUNS[(name_index / 100) % NOUNS.len()];
        let extension = "echo".repeat(index / namespace);
        let name = format!("{adjective}_{extension}{noun}_{:02}", name_index % 100);
        let bot_rating = (i64::from(rating) + rng.gen_range(-250..=250)).clamp(800, 3500) as u32;
        let mut solves = Vec::new();
        let mut seen_slots = BTreeSet::new();
        for problem in problems {
            if duration_sec == 0 || !seen_slots.insert(problem.slot) {
                continue;
            }
            let d = f64::from(bot_rating) - f64::from(problem.problem.rating.unwrap_or(rating));
            let probability = (1.0 / (1.0 + (-(d + 100.0) / 250.0).exp())).clamp(0.05, 0.95);
            if !rng.gen_bool(probability) {
                continue;
            }
            let center_minutes = (25.0 - d / 40.0).max(2.0);
            let normal =
                (-2.0 * (1.0 - rng.gen::<f64>()).ln()).sqrt() * (TAU * rng.gen::<f64>()).cos();
            let sampled_seconds = 60.0 * center_minutes * (0.45 * normal).exp();
            let accepted_at_sec = sampled_seconds
                .clamp(f64::from(duration_sec.min(120)), f64::from(duration_sec))
                .round() as u32;
            let rejected_attempts = if rng.gen_bool(0.25) {
                rng.gen_range(1..=3)
            } else {
                0
            };
            solves.push(BotSolve {
                slot: problem.slot,
                accepted_at_sec,
                rejected_attempts,
            });
        }
        solves.sort_by_key(|event| (event.accepted_at_sec, event.slot));
        bots.push(Bot {
            name,
            rating: bot_rating,
            solves,
        });
    }
    bots
}
