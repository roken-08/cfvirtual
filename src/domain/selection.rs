//! Seeded 40/40/20 contest assembly. A slot first uses its normal window, then
//! one 100-point expansion, then all remaining eligible problems. Tag filters
//! have OR semantics and remain in force through every fallback.
//!
//! The model has no source-contest title. Reject known nonstandard markers in
//! names/tags, `*special`, Gym keys, missing ratings, and malformed keys here;
//! the CF importer must also filter excluded source-contest titles when known.

use super::{ContestProblem, Problem, ProblemKey, Tier};
use anyhow::{bail, Result};
use rand::{Rng, SeedableRng};
use rand_chacha::ChaCha8Rng;
use std::collections::{BTreeMap, BTreeSet, HashSet};

struct Candidate<'a> {
    problem: &'a Problem,
    rating: u32,
    name: String,
    tags: BTreeSet<String>,
}

pub fn select(
    problems: &[Problem],
    solved: &HashSet<ProblemKey>,
    weights: &BTreeMap<String, f64>,
    rating: u32,
    count: usize,
    tags: &[String],
    seed: u64,
) -> Result<Vec<ContestProblem>> {
    if !(1..=26).contains(&count) {
        bail!("problem count must be between 1 and 26 (display slots A–Z)");
    }
    let filter: BTreeSet<_> = tags
        .iter()
        .map(|tag| normalize(tag))
        .filter(|tag| !tag.is_empty())
        .collect();
    let mut pool = Vec::new();
    for problem in problems {
        let Some(problem_rating) = problem.rating.filter(|rating| *rating > 0) else {
            continue;
        };
        let name = normalize(&problem.name);
        let problem_tags: BTreeSet<_> = problem.tags.iter().map(|tag| normalize(tag)).collect();
        if solved.contains(&problem.key)
            || problem.key.contest_id == 0
            || problem.key.contest_id >= 100_000
            || problem.key.index.is_empty()
            || !problem
                .key
                .index
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric())
            || name.is_empty()
            || nonstandard(&name)
            || problem_tags.iter().any(|tag| nonstandard(tag))
            || (!filter.is_empty() && problem_tags.is_disjoint(&filter))
        {
            continue;
        }
        pool.push(Candidate {
            problem,
            rating: problem_rating,
            name,
            tags: problem_tags
                .into_iter()
                .filter(|tag| !tag.is_empty())
                .collect(),
        });
    }
    // Canonical input order also makes a shuffled CF cache reproduce the set.
    pool.sort_by(|left, right| {
        left.rating
            .cmp(&right.rating)
            .then_with(|| left.problem.key.cmp(&right.problem.key))
            .then_with(|| left.name.cmp(&right.name))
            .then_with(|| left.tags.cmp(&right.tags))
            .then_with(|| left.problem.name.cmp(&right.problem.name))
            .then_with(|| left.problem.tags.cmp(&right.problem.tags))
            .then_with(|| left.problem.solved_count.cmp(&right.problem.solved_count))
    });
    let mut normalized_weights = BTreeMap::new();
    let max_weight = weights
        .values()
        .copied()
        .filter(|value| value.is_finite() && *value > 0.0)
        .fold(0.0_f64, f64::max);
    if max_weight > 0.0 {
        for (tag, weight) in weights {
            if weight.is_finite() && *weight > 0.0 {
                // Scale before summation to keep even large supplied weights finite.
                *normalized_weights.entry(normalize(tag)).or_insert(0.0) += weight / max_weight;
            }
        }
    }

    let weak = count * 2 / 5;
    let stretch = count / 5;
    let core = count - weak - stretch;
    let mut rng = ChaCha8Rng::seed_from_u64(seed);
    let mut keys = HashSet::new();
    let mut names = BTreeSet::new();
    let mut selected = Vec::with_capacity(count);
    // Reserve the narrowest tier first so ordinary core choices cannot consume
    // the sole stretch candidate in an otherwise sufficiently large pool.
    for (tier, slots) in [
        (Tier::Stretch, stretch),
        (Tier::Weak, weak),
        (Tier::Core, core),
    ] {
        for _ in 0..slots {
            let remaining: Vec<_> = pool
                .iter()
                .filter(|candidate| {
                    !keys.contains(&candidate.problem.key) && !names.contains(&candidate.name)
                })
                .collect();
            let candidate =
                choose_window(&remaining, tier, rating, 0, &normalized_weights, &mut rng)
                    .or_else(|| {
                        choose_window(&remaining, tier, rating, 100, &normalized_weights, &mut rng)
                    })
                    .or_else(|| choose_problem(&remaining, &normalized_weights, &mut rng));
            let Some(candidate) = candidate else {
                bail!(
                    "not enough eligible distinct unsolved problems: requested {count}, found {}; try --rating or fewer problems (and check --tags)",
                    selected.len()
                );
            };
            keys.insert(candidate.problem.key.clone());
            names.insert(candidate.name.clone());
            selected.push(ContestProblem {
                slot: 'A',
                problem: candidate.problem.clone(),
                tier,
            });
        }
    }
    selected.sort_by(|left, right| {
        left.problem
            .rating
            .cmp(&right.problem.rating)
            .then_with(|| left.problem.key.cmp(&right.problem.key))
    });
    for (index, problem) in selected.iter_mut().enumerate() {
        problem.slot = char::from(b'A' + index as u8);
    }
    Ok(selected)
}

fn normalize(text: &str) -> String {
    text.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_ascii_lowercase()
}

fn nonstandard(text: &str) -> bool {
    text.contains("april fools")
        || text.contains("kotlin heroes")
        || text.contains("q#")
        || text.contains("unknown language")
        || text == "*special"
}

fn choose_window<'a>(
    remaining: &[&'a Candidate<'a>],
    tier: Tier,
    rating: u32,
    relaxation: u32,
    weights: &BTreeMap<String, f64>,
    rng: &mut ChaCha8Rng,
) -> Option<&'a Candidate<'a>> {
    let low = if tier == Tier::Stretch {
        rating.saturating_add(200).saturating_sub(relaxation)
    } else {
        rating.saturating_sub(200 + relaxation)
    };
    let high = rating.saturating_add(400 + relaxation);
    let mut eligible: Vec<_> = remaining
        .iter()
        .copied()
        .filter(|candidate| (low..=high).contains(&candidate.rating))
        .collect();
    if tier == Tier::Core {
        let preferred: Vec<_> = eligible
            .iter()
            .copied()
            .filter(|candidate| {
                (rating.saturating_sub(100 + relaxation)..=rating.saturating_add(200 + relaxation))
                    .contains(&candidate.rating)
            })
            .collect();
        if !preferred.is_empty() {
            eligible = preferred;
        }
    }
    if tier == Tier::Weak {
        let present: BTreeSet<_> = eligible
            .iter()
            .flat_map(|candidate| &candidate.tags)
            .collect();
        let tag_weights: Vec<_> = present
            .into_iter()
            .filter_map(|tag| {
                let weight = if weights.is_empty() {
                    1.0
                } else {
                    *weights.get(tag)?
                };
                (weight > 0.0).then_some((tag, weight))
            })
            .collect();
        if !tag_weights.is_empty() {
            let index = weighted_index(tag_weights.iter().map(|(_, weight)| *weight), rng)?;
            let tag = tag_weights[index].0;
            eligible.retain(|candidate| candidate.tags.contains(tag));
        } else if !weights.is_empty() {
            return None;
        }
    }
    choose_problem(&eligible, weights, rng)
}

fn choose_problem<'a>(
    eligible: &[&'a Candidate<'a>],
    weights: &BTreeMap<String, f64>,
    rng: &mut ChaCha8Rng,
) -> Option<&'a Candidate<'a>> {
    let scores: Vec<_> = eligible
        .iter()
        .map(|candidate| {
            let weakness = if weights.is_empty() {
                1.0
            } else {
                candidate
                    .tags
                    .iter()
                    .filter_map(|tag| weights.get(tag))
                    .sum()
            };
            weakness + rng.gen_range(0.001..0.051)
        })
        .collect();
    let index = weighted_index(scores.into_iter(), rng)?;
    eligible.get(index).copied()
}

fn weighted_index(weights: impl Iterator<Item = f64>, rng: &mut ChaCha8Rng) -> Option<usize> {
    let weights: Vec<_> = weights.collect();
    let total: f64 = weights.iter().sum();
    if total <= 0.0 || !total.is_finite() {
        return None;
    }
    let mut draw = rng.gen_range(0.0..total);
    for (index, weight) in weights.iter().enumerate() {
        if draw < *weight {
            return Some(index);
        }
        draw -= weight;
    }
    weights.len().checked_sub(1)
}
