use cfvirtual::domain::{selection::select, ContestProblem, Problem, ProblemKey, Tier};
use std::collections::{BTreeMap, HashSet};

fn problem(id: u32, rating: u32, tag: &str) -> Problem {
    Problem {
        key: ProblemKey {
            contest_id: id,
            index: "A".to_owned(),
        },
        name: format!("Synthetic problem {id}"),
        rating: Some(rating),
        tags: vec![tag.to_owned()],
        solved_count: None,
    }
}

fn signature(selected: &[ContestProblem]) -> Vec<(char, ProblemKey, Tier)> {
    selected
        .iter()
        .map(|problem| (problem.slot, problem.problem.key.clone(), problem.tier))
        .collect()
}

#[test]
fn selection_is_seeded_sorted_and_independent_of_cache_order() {
    let mut pool: Vec<_> = (1..=100)
        .map(|id| {
            problem(
                id,
                1000 + (id % 7) * 100,
                if id % 2 == 0 { "dp" } else { "math" },
            )
        })
        .collect();
    let weights = BTreeMap::from([("dp".to_owned(), 0.9), ("math".to_owned(), 0.1)]);
    let first = select(&pool, &HashSet::new(), &weights, 1200, 10, &[], 123).unwrap();
    let repeated = select(&pool, &HashSet::new(), &weights, 1200, 10, &[], 123).unwrap();
    pool.reverse();
    let shuffled = select(&pool, &HashSet::new(), &weights, 1200, 10, &[], 123).unwrap();
    assert_eq!(signature(&first), signature(&repeated));
    assert_eq!(signature(&first), signature(&shuffled));
    assert_ne!(
        signature(&first),
        signature(&select(&pool, &HashSet::new(), &weights, 1200, 10, &[], 124).unwrap())
    );
    assert!(first
        .windows(2)
        .all(|pair| pair[0].problem.rating <= pair[1].problem.rating));
    assert_eq!(
        first.iter().map(|problem| problem.slot).collect::<String>(),
        "ABCDEFGHIJ"
    );
}

#[test]
fn forty_forty_twenty_tiers_and_core_remainder_are_preserved() {
    let pool: Vec<_> = (1..=60)
        .map(|id| problem(id, 1000 + (id % 7) * 100, "dp"))
        .collect();
    for (count, expected) in [(5, (2, 2, 1)), (6, (2, 3, 1)), (10, (4, 4, 2))] {
        let selected = select(
            &pool,
            &HashSet::new(),
            &BTreeMap::new(),
            1200,
            count,
            &[],
            1,
        )
        .unwrap();
        let actual = (
            selected
                .iter()
                .filter(|problem| problem.tier == Tier::Weak)
                .count(),
            selected
                .iter()
                .filter(|problem| problem.tier == Tier::Core)
                .count(),
            selected
                .iter()
                .filter(|problem| problem.tier == Tier::Stretch)
                .count(),
        );
        assert_eq!(actual, expected);
        for problem in selected {
            let rating = problem.problem.rating.unwrap();
            if problem.tier == Tier::Stretch {
                assert!((1400..=1600).contains(&rating));
            }
            if problem.tier == Tier::Core {
                assert!((1100..=1400).contains(&rating));
            }
        }
    }
}

#[test]
fn weak_slots_carry_the_weighted_tag_and_stretch_is_reserved_first() {
    let mut pool: Vec<_> = (1..=10).map(|id| problem(id, 1200, "dp")).collect();
    pool.extend((11..=20).map(|id| problem(id, 1200, "math")));
    pool.push(problem(21, 1600, "math"));
    let weights = BTreeMap::from([("dp".to_owned(), 1.0), ("math".to_owned(), 0.0)]);
    for seed in 0..20 {
        let selected = select(&pool, &HashSet::new(), &weights, 1200, 5, &[], seed).unwrap();
        assert!(selected
            .iter()
            .filter(|problem| problem.tier == Tier::Weak)
            .all(|problem| problem.problem.tags == ["dp"]));
        assert_eq!(
            selected
                .iter()
                .find(|problem| problem.tier == Tier::Stretch)
                .unwrap()
                .problem
                .key
                .contest_id,
            21
        );
    }
}

#[test]
fn solved_keys_duplicate_keys_and_case_insensitive_names_are_excluded() {
    let mut pool: Vec<_> = (1..=12).map(|id| problem(id, 1200, "dp")).collect();
    pool[1].name = " Same   title ".to_owned();
    pool[2].name = "same title".to_owned();
    pool.push(pool[3].clone());
    let solved = HashSet::from([pool[0].key.clone()]);
    let selected = select(&pool, &solved, &BTreeMap::new(), 1200, 10, &[], 8).unwrap();
    let keys: HashSet<_> = selected
        .iter()
        .map(|problem| &problem.problem.key)
        .collect();
    let names: HashSet<_> = selected
        .iter()
        .map(|problem| {
            problem
                .problem
                .name
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
                .to_ascii_lowercase()
        })
        .collect();
    assert_eq!(keys.len(), 10);
    assert_eq!(names.len(), 10);
    assert!(!keys.contains(&pool[0].key));
}

#[test]
fn the_single_expansion_precedes_unbounded_remaining_pool_fallback() {
    let pool = [problem(1, 1000, "dp"), problem(2, 500, "dp")];
    for seed in 0..10 {
        let selected =
            select(&pool, &HashSet::new(), &BTreeMap::new(), 1200, 1, &[], seed).unwrap();
        assert_eq!(selected[0].problem.key.contest_id, 1);
    }
    assert!(select(&pool, &HashSet::new(), &BTreeMap::new(), 1200, 2, &[], 10).is_err());
}

#[test]
fn tag_filters_are_or_based_and_survive_all_fallbacks() {
    let pool = [
        problem(1, 1000, "dp"),
        problem(2, 1600, "graphs"),
        problem(3, 1200, "math"),
    ];
    let tags = [" DP ".to_owned(), "graphs".to_owned()];
    let selected = select(&pool, &HashSet::new(), &BTreeMap::new(), 1200, 2, &tags, 3).unwrap();
    assert_eq!(
        selected
            .iter()
            .map(|problem| problem.problem.key.contest_id)
            .collect::<Vec<_>>(),
        [1, 2]
    );
    assert!(select(&pool, &HashSet::new(), &BTreeMap::new(), 1200, 3, &tags, 3).is_err());
}

#[test]
fn invalid_sources_and_missing_ratings_never_reenter_through_fallback() {
    let mut pool = vec![problem(1, 1200, "dp")];
    for (index, marker) in [
        "April Fools",
        "Kotlin Heroes",
        "Q#",
        "Unknown Language",
        "*special",
    ]
    .iter()
    .enumerate()
    {
        let mut invalid = problem(index as u32 + 2, 1200, marker);
        invalid.name = marker.to_string();
        pool.push(invalid);
    }
    let mut unrated = problem(7, 1200, "dp");
    unrated.rating = None;
    pool.extend([unrated, problem(100001, 1200, "dp"), problem(0, 1200, "dp")]);
    let mut invalid_index = problem(8, 1200, "dp");
    invalid_index.key.index = "../A".to_owned();
    pool.push(invalid_index);
    assert_eq!(
        select(&pool, &HashSet::new(), &BTreeMap::new(), 1200, 1, &[], 0).unwrap()[0]
            .problem
            .key
            .contest_id,
        1
    );
    assert!(select(&pool, &HashSet::new(), &BTreeMap::new(), 1200, 2, &[], 0).is_err());
}

#[test]
fn insufficient_pool_errors_suggest_rating_or_fewer_problems() {
    let error = select(
        &[problem(1, 1200, "dp")],
        &HashSet::new(),
        &BTreeMap::new(),
        1200,
        5,
        &[],
        0,
    )
    .unwrap_err()
    .to_string();
    assert!(error.contains("--rating") && error.contains("fewer problems"));
    assert!(select(&[], &HashSet::new(), &BTreeMap::new(), 1200, 1, &[], 0).is_err());
}

#[test]
fn label_limit_zero_counts_and_pathological_weights_are_safe() {
    let pool: Vec<_> = (1..=30).map(|id| problem(id, 1200, "dp")).collect();
    let weights = BTreeMap::from([
        ("dp".to_owned(), f64::NAN),
        ("math".to_owned(), f64::INFINITY),
        ("graphs".to_owned(), -1.0),
    ]);
    let selected = select(&pool, &HashSet::new(), &weights, 1200, 26, &[], 2).unwrap();
    assert_eq!(selected.last().unwrap().slot, 'Z');
    for count in [0, 27] {
        assert!(select(&pool, &HashSet::new(), &weights, u32::MAX, count, &[], 2).is_err());
    }
    let enormous = BTreeMap::from([("dp".to_owned(), f64::MAX)]);
    assert!(select(&pool, &HashSet::new(), &enormous, 1200, 5, &[], 2).is_ok());
}
