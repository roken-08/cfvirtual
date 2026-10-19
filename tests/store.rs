use anyhow::Result;
use cfvirtual::domain::{
    Bot, BotSolve, Contest, ContestProblem, Participant, Problem, ProblemKey, ProblemStatus,
    Standing, Tier,
};
use cfvirtual::store::Store;
use chrono::Utc;
use rusqlite::{config::DbConfig, Connection};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use tempfile::tempdir;

const DAY: i64 = 86_400;

#[test]
fn public_history_medians_and_weights_survive_restart_without_raw_history() -> Result<()> {
    use cfvirtual::domain::{weakness, Submission};
    let directory = tempdir()?;
    let path = directory.path().join("profile.db");
    let history = vec![Submission {
        id: 1,
        problem: ProblemKey {
            contest_id: 2001,
            index: "A".into(),
        },
        created_at: 100,
        verdict: Some("OK".into()),
        handles: vec!["practice_player".into()],
        tags: vec!["math".into()],
        rating: Some(1400),
    }];
    let profile = weakness::analyze(&history, 101);
    Store::open(&path)?.save_history_profile("practice_player", 1500, &profile, 101)?;
    let stats = Store::open(&path)?.stats()?;
    let summary = stats.history_profile.expect("saved profile");
    assert_eq!(summary.tags["math"].median_solved_rating, Some(1400));
    assert_eq!(summary.weights["math"], 1.0);
    assert_eq!(summary.updated_at, 101);
    assert_eq!(stats.total_contests, 0);
    let value: String = Connection::open(path)?.query_row(
        "SELECT value FROM meta WHERE key='history_profile'",
        [],
        |row| row.get(0),
    )?;
    assert!(!value.contains("created_at"));
    assert!(!value.contains("contest_id"));
    Ok(())
}

fn fixture() -> (Contest, Vec<Standing>) {
    let problems = [
        ('A', 2001, "First problem", vec!["math"], Tier::Weak),
        (
            'B',
            2002,
            "Second problem",
            vec!["greedy", "math"],
            Tier::Core,
        ),
        ('C', 2003, "Third problem", vec!["dp"], Tier::Stretch),
    ]
    .into_iter()
    .map(|(slot, contest_id, name, tags, tier)| ContestProblem {
        slot,
        problem: Problem {
            key: ProblemKey {
                contest_id,
                index: slot.to_string(),
            },
            name: name.into(),
            rating: Some(1200 + (slot as u32 - 'A' as u32) * 200),
            tags: tags.into_iter().map(str::to_owned).collect(),
            solved_count: Some(1234),
        },
        tier,
    })
    .collect();
    let contest = Contest {
        id: "contest-one".into(),
        code: Some("K7QM2X".into()),
        host_handle: "practice_player".into(),
        host_rating: 1400,
        started_at: 1_730_000_000,
        duration_sec: 7200,
        problems,
        participants: vec![Participant {
            handle: "practice_player".into(),
            is_host: true,
            is_spectator: false,
            token: "host-session-secret-never-persist".into(),
            connected: false,
        }],
        bots: vec![Bot {
            name: "violet_orbit_17".into(),
            rating: 1500,
            solves: vec![
                BotSolve {
                    slot: 'A',
                    accepted_at_sec: 20,
                    rejected_attempts: 0,
                },
                BotSolve {
                    slot: 'B',
                    accepted_at_sec: 30,
                    rejected_attempts: 0,
                },
                BotSolve {
                    slot: 'C',
                    accepted_at_sec: 40,
                    rejected_attempts: 0,
                },
            ],
        }],
        seed: u64::MAX,
        mode: "room".into(),
        blind: true,
        ended_at: Some(1_730_001_800),
    };
    let host = Standing {
        name: contest.host_handle.clone(),
        is_bot: false,
        rating: 1400,
        solved: 1,
        penalty: 1260,
        last_ac: 60,
        problems: BTreeMap::from([
            (
                'A',
                ProblemStatus {
                    solved: true,
                    accepted_at_sec: Some(60),
                    rejected: 1,
                    attempts: 2,
                    first_attempt_at_sec: Some(10),
                },
            ),
            (
                'B',
                ProblemStatus {
                    rejected: 2,
                    attempts: 2,
                    first_attempt_at_sec: Some(200),
                    ..ProblemStatus::default()
                },
            ),
            ('C', ProblemStatus::default()),
        ]),
    };
    let bot = Standing {
        name: contest.bots[0].name.clone(),
        is_bot: true,
        rating: 1500,
        solved: 3,
        penalty: 90,
        last_ac: 40,
        problems: contest.bots[0]
            .solves
            .iter()
            .map(|solve| {
                (
                    solve.slot,
                    ProblemStatus {
                        solved: true,
                        accepted_at_sec: Some(solve.accepted_at_sec),
                        rejected: 0,
                        attempts: 1,
                        first_attempt_at_sec: Some(solve.accepted_at_sec),
                    },
                )
            })
            .collect(),
    };
    // Persist the authoritative leaderboard order, including the winning bot.
    (contest, vec![bot, host])
}

fn host(standings: &[Standing]) -> &Standing {
    standings.iter().find(|s| !s.is_bot).expect("fixture host")
}

fn appended(path: &Path, suffix: &str) -> PathBuf {
    let mut result = path.as_os_str().to_owned();
    result.push(suffix);
    result.into()
}

fn backups(path: &Path) -> Result<Vec<PathBuf>> {
    let name = path
        .file_name()
        .expect("database filename")
        .to_string_lossy();
    let prefix = format!("{name}.bak");
    let mut paths = Vec::new();
    for entry in fs::read_dir(path.parent().expect("database parent"))? {
        let entry = entry?;
        if entry.file_name().to_string_lossy().starts_with(&prefix) {
            paths.push(entry.path());
        }
    }
    paths.sort();
    Ok(paths)
}

#[test]
fn empty_database_and_zero_limits_are_readable() -> Result<()> {
    let directory = tempdir()?;
    let path = directory.path().join("nested/data.sqlite");
    let store = Store::open(&path)?;
    assert!(store.history(10)?.is_empty());
    assert!(store.history(0)?.is_empty());
    assert!(store.review(false, 0, 0)?.is_empty());
    assert!(store.review(true, 10, 0)?.is_empty());
    assert!(store.load_contest("missing")?.is_none());
    let stats = store.stats()?;
    assert_eq!(stats.total_contests, 0);
    assert_eq!(stats.total_problems, 0);
    assert_eq!(stats.total_solved, 0);
    assert_eq!(stats.solve_rate, 0.0);
    assert_eq!(stats.average_penalty, 0.0);
    assert_eq!(stats.current_streak, 0);
    assert!(stats.per_tag.is_empty());
    serde_json::to_string(&stats)?;
    Ok(())
}

#[test]
fn restart_preserves_every_export_input_and_bot_schedule_without_tokens() -> Result<()> {
    let directory = tempdir()?;
    let path = directory.path().join("contest.sqlite");
    let (contest, mut standings) = fixture();
    // Both unsigned fields must round-trip even above SQLite's signed range.
    standings[1].penalty = u64::MAX;
    let export_inputs = serde_json::to_value((&contest, &standings))?;
    let markdown = cfvirtual::domain::review::markdown(&contest, &standings, &contest.host_handle);
    {
        let mut store = Store::open(&path)?;
        store.save_contest(&contest, &standings)?;
    }
    let store = Store::open(&path)?;
    let (loaded, loaded_standings) = store.load_contest(&contest.id)?.expect("saved contest");
    assert_eq!(
        serde_json::to_value((&loaded, &loaded_standings))?,
        export_inputs
    );
    assert_eq!(loaded.seed, u64::MAX);
    assert_eq!(loaded_standings, standings);
    assert_eq!(loaded.bots, contest.bots);
    assert_eq!(
        cfvirtual::domain::review::markdown(&loaded, &loaded_standings, &loaded.host_handle),
        markdown
    );
    assert!(loaded.participants.iter().all(|p| p.token.is_empty()));
    let history = store.history(usize::MAX)?;
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].id, contest.id);
    assert_eq!(history[0].mode, "room");
    assert_eq!(history[0].started_at, contest.started_at);
    assert_eq!(history[0].problem_count, 3);
    assert_eq!(history[0].host_handle, contest.host_handle);
    assert_eq!(history[0].solved, 1);
    assert_eq!(history[0].penalty, u64::MAX);

    let connection = Connection::open(&path)?;
    let (seed, seed_type, snapshot): (String, String, String) = connection.query_row(
        "SELECT seed, typeof(seed), snapshot_json FROM contests",
        [],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
    )?;
    assert_eq!(seed, u64::MAX.to_string());
    assert_eq!(seed_type, "text");
    assert!(!snapshot.contains("token"));
    assert!(!snapshot.contains(&contest.participants[0].token));
    let first_attempt: u32 = connection.query_row(
        "SELECT first_attempt_at_sec FROM results WHERE handle=?1 AND slot='B'",
        [&contest.host_handle],
        |row| row.get(0),
    )?;
    assert_eq!(first_attempt, 200);
    drop(connection);
    drop(store);
    assert!(!fs::read(&path)?
        .windows(contest.participants[0].token.len())
        .any(|window| window == contest.participants[0].token.as_bytes()));
    Ok(())
}

#[test]
fn repeated_saves_are_idempotent_and_updates_replace_old_results() -> Result<()> {
    let directory = tempdir()?;
    let path = directory.path().join("contest.sqlite");
    let (mut contest, standings) = fixture();
    let mut store = Store::open(&path)?;
    store.save_contest(&contest, &standings)?;
    let connection = Connection::open(&path)?;
    connection.execute("UPDATE contests SET created_at=42", [])?;
    store.save_contest(&contest, &standings)?;
    store.save_contest(&contest, &standings)?;
    assert_eq!(store.history(10)?.len(), 1);
    let counts: (u32, u32, u32, i64) = connection.query_row(
        "SELECT (SELECT count(*) FROM contests), (SELECT count(*) FROM participants),
                (SELECT count(*) FROM results), created_at FROM contests",
        [],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
    )?;
    assert_eq!(counts, (1, 2, 6, 42));

    contest.bots.clear();
    let mut updated = host(&standings).clone();
    updated.solved = 0;
    updated.penalty = 0;
    updated.problems.clear();
    store.save_contest(&contest, &[updated.clone()])?;
    assert_eq!(store.history(10)?[0].solved, 0);
    assert_eq!(
        store.load_contest(&contest.id)?.expect("contest").1,
        vec![updated]
    );
    let counts: (u32, u32) = connection.query_row(
        "SELECT (SELECT count(*) FROM participants), (SELECT count(*) FROM results)",
        [],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    assert_eq!(counts, (1, 0));
    Ok(())
}

#[test]
fn failed_save_rolls_back_snapshot_and_all_normalized_rows() -> Result<()> {
    let directory = tempdir()?;
    let path = directory.path().join("contest.sqlite");
    let (contest, standings) = fixture();
    let mut store = Store::open(&path)?;
    store.save_contest(&contest, &standings)?;
    let mut changed = contest.clone();
    changed.problems[0].problem.name = "Uncommitted change".into();
    let duplicate = vec![host(&standings).clone(), host(&standings).clone()];
    assert!(store.save_contest(&changed, &duplicate).is_err());
    let loaded = store.load_contest(&contest.id)?.expect("original contest");
    assert_eq!(
        serde_json::to_value(loaded)?,
        serde_json::to_value((&contest, &standings))?
    );
    assert_eq!(store.history(10)?[0].penalty, 1260);
    let connection = Connection::open(&path)?;
    let result_count: u32 =
        connection.query_row("SELECT count(*) FROM results", [], |r| r.get(0))?;
    assert_eq!(result_count, 6);
    Ok(())
}

#[test]
fn history_is_newest_first_stable_and_limited() -> Result<()> {
    let directory = tempdir()?;
    let (mut contest, standings) = fixture();
    let mut store = Store::open(&directory.path().join("contest.sqlite"))?;
    for (id, start) in [("old", 100), ("new-b", 200), ("new-a", 200)] {
        contest.id = id.into();
        contest.started_at = start;
        store.save_contest(&contest, &standings)?;
    }
    let history = store.history(2)?;
    assert_eq!(
        history.iter().map(|e| e.id.as_str()).collect::<Vec<_>>(),
        ["new-a", "new-b"]
    );
    assert!(store.history(0)?.is_empty());
    Ok(())
}

#[test]
fn review_queue_survives_restart_and_keeps_progress_when_reenqueued() -> Result<()> {
    let directory = tempdir()?;
    let path = directory.path().join("contest.sqlite");
    let (mut contest, standings) = fixture();
    let now = 1_730_100_000;
    {
        let mut store = Store::open(&path)?;
        assert_eq!(store.enqueue_failed(&contest, &standings[0], now)?, 0);
        // Queuing does not require the contest to have already been saved.
        assert_eq!(store.enqueue_failed(&contest, host(&standings), now)?, 2);
        assert!(store.review(true, 10, now + DAY - 1)?.is_empty());
        assert_eq!(store.review(true, 10, now + DAY)?.len(), 2);
        assert_eq!(store.review(false, 1, now)?.len(), 1);
        store.advance_review("2002B", true, now + DAY)?;
    }
    let mut store = Store::open(&path)?;
    contest.id = "another-contest".into();
    contest.problems[1].problem.name = "Changed metadata must not reset queue".into();
    assert_eq!(
        store.enqueue_failed(&contest, host(&standings), now + 10 * DAY)?,
        0
    );
    let entries = store.review(false, usize::MAX, now)?;
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0].problem_key, "2003C");
    assert_eq!(entries[0].due_at, now + DAY);
    let entry = &entries[1];
    assert_eq!(entry.problem_key, "2002B");
    assert_eq!(entry.contest_id, "contest-one");
    assert_eq!(entry.name, "Second problem");
    assert_eq!(
        entry.url,
        "https://codeforces.com/problemset/problem/2002/B"
    );
    assert_eq!(entry.rating, Some(1400));
    assert_eq!(entry.tags, ["greedy", "math"]);
    assert_eq!(entry.added_at, now);
    assert_eq!(entry.interval_days, 3);
    assert_eq!(entry.due_at, now + 4 * DAY);
    assert_eq!(entry.lapses, 0);
    assert_eq!(entry.ease, 2.5);
    serde_json::to_string(&entries)?;
    assert_eq!(store.review(true, 10, now + DAY)?.len(), 1);
    Ok(())
}

#[test]
fn review_advances_full_ladder_caps_resets_and_updates_atomically() -> Result<()> {
    let directory = tempdir()?;
    let path = directory.path().join("contest.sqlite");
    let (contest, standings) = fixture();
    let mut store = Store::open(&path)?;
    let now = 1_730_100_000;
    store.enqueue_failed(&contest, host(&standings), now)?;
    for days in [3, 7, 14, 30, 30] {
        store.advance_review("2002B", true, now)?;
        let entry = store
            .review(false, 10, now)?
            .into_iter()
            .find(|e| e.problem_key == "2002B")
            .expect("queued problem");
        assert_eq!(entry.interval_days, days);
        assert_eq!(entry.due_at, now + i64::from(days) * DAY);
        assert_eq!(entry.lapses, 0);
    }
    store.advance_review("2002B", false, now + DAY)?;
    store.advance_review("2002B", false, now + 2 * DAY)?;
    let before = store.review(false, 10, now)?;
    let entry = before
        .iter()
        .find(|e| e.problem_key == "2002B")
        .expect("queued problem");
    assert_eq!(entry.interval_days, 1);
    assert_eq!(entry.due_at, now + 3 * DAY);
    assert_eq!(entry.lapses, 2);
    assert!(store.advance_review("missing", true, now).is_err());
    assert!(store.advance_review("2002B", false, i64::MAX).is_err());
    assert!(store
        .enqueue_failed(&contest, host(&standings), i64::MAX)
        .is_err());
    assert_eq!(
        serde_json::to_value(store.review(false, 10, now)?)?,
        serde_json::to_value(before)?
    );
    Ok(())
}

#[test]
fn stats_use_only_local_host_results_excluding_bots_guests_and_spectators() -> Result<()> {
    let directory = tempdir()?;
    let (mut contest, mut standings) = fixture();
    for (handle, is_spectator) in [("guest_player", false), ("watching_player", true)] {
        contest.participants.push(Participant {
            handle: handle.into(),
            is_host: false,
            is_spectator,
            token: "guest-session-secret".into(),
            connected: true,
        });
        let mut standing = standings[0].clone();
        standing.name = handle.into();
        standing.is_bot = false;
        standings.push(standing);
    }
    // Duplicate source tags must not inflate a tag's denominator.
    contest.problems[0].problem.tags.push("math".into());
    let mut store = Store::open(&directory.path().join("contest.sqlite"))?;
    store.save_contest(&contest, &standings)?;
    store.save_contest(&contest, &standings)?;
    let stats = store.stats()?;
    assert_eq!(stats.total_contests, 1);
    assert_eq!(stats.total_problems, 3);
    assert_eq!(stats.total_solved, 1);
    assert!((stats.solve_rate - 1.0 / 3.0).abs() < f64::EPSILON);
    assert_eq!(stats.average_penalty, 1260.0);
    assert_eq!(stats.current_streak, 0);
    assert_eq!(stats.per_tag["math"].problems, 2);
    assert_eq!(stats.per_tag["math"].solved, 1);
    assert_eq!(stats.per_tag["math"].solve_rate, 0.5);
    assert_eq!(stats.per_tag["greedy"].problems, 1);
    assert_eq!(stats.per_tag["greedy"].solve_rate, 0.0);
    assert_eq!(stats.per_tag["dp"].solve_rate, 0.0);
    Ok(())
}

#[test]
fn stats_streak_counts_distinct_utc_dates_and_allows_yesterday() -> Result<()> {
    let directory = tempdir()?;
    let (mut contest, standings) = fixture();
    let today = Utc::now()
        .date_naive()
        .and_hms_opt(12, 0, 0)
        .expect("noon")
        .and_utc()
        .timestamp();
    let mut store = Store::open(&directory.path().join("contest.sqlite"))?;
    for (index, days_ago) in [5, 3, 2, 1, 1].into_iter().enumerate() {
        contest.id = format!("contest-{index}");
        contest.started_at = today - days_ago * DAY;
        store.save_contest(&contest, &standings)?;
    }
    assert_eq!(store.stats()?.current_streak, 3);
    contest.id = "today".into();
    contest.started_at = today;
    store.save_contest(&contest, &standings)?;
    let stats = store.stats()?;
    assert_eq!(stats.current_streak, 4);
    assert_eq!(stats.total_contests, 6);
    assert_eq!(stats.total_solved, 6);
    assert_eq!(stats.average_penalty, 1260.0);
    Ok(())
}

#[test]
fn corruption_is_backed_up_and_existing_backups_are_never_overwritten() -> Result<()> {
    let directory = tempdir()?;
    let path = directory.path().join("contest.sqlite");
    let first = b"this is not a SQLite database; preserve these bytes";
    fs::write(&path, first)?;
    {
        let store = Store::open(&path)?;
        assert!(store.history(10)?.is_empty());
    }
    let original_backup = appended(&path, ".bak");
    assert_eq!(fs::read(&original_backup)?, first);
    let second = b"another corrupt database; keep both backups";
    fs::write(&path, second)?;
    let mut store = Store::open(&path)?;
    assert_eq!(fs::read(&original_backup)?, first);
    let copies = backups(&path)?;
    assert_eq!(copies.len(), 2);
    assert!(copies
        .iter()
        .any(|copy| fs::read(copy).is_ok_and(|bytes| bytes == second)));
    let (contest, standings) = fixture();
    store.save_contest(&contest, &standings)?;
    assert_eq!(store.history(10)?.len(), 1);
    Ok(())
}

#[test]
fn corruption_backup_preserves_wal_and_shared_memory_sidecars() -> Result<()> {
    let directory = tempdir()?;
    let path = directory.path().join("contest.sqlite");
    let damaged = b"not a database; recovery must preserve sidecars too";
    let wal = b"WAL forensic evidence";
    let shm = b"shared memory forensic evidence";
    fs::write(&path, damaged)?;
    fs::write(appended(&path, "-wal"), wal)?;
    fs::write(appended(&path, "-shm"), shm)?;
    let store = Store::open(&path)?;
    assert!(store.history(10)?.is_empty());
    assert_eq!(fs::read(appended(&path, ".bak"))?, damaged);
    assert_eq!(fs::read(appended(&path, ".bak-wal"))?, wal);
    // SQLite may rebuild the disposable shared-memory index while checking the
    // database. Preserve that diagnostic file, but WAL bytes must stay exact.
    assert!(fs::metadata(appended(&path, ".bak-shm"))?.len() > 0);
    Ok(())
}

#[test]
fn quick_check_integrity_diagnostics_trigger_recovery() -> Result<()> {
    let directory = tempdir()?;
    let path = directory.path().join("contest.sqlite");
    {
        let connection = Connection::open(&path)?;
        connection.execute_batch(
            "CREATE TABLE damaged(value INTEGER CHECK(value > 0));
             PRAGMA ignore_check_constraints=ON;
             INSERT INTO damaged VALUES(-1);",
        )?;
    }
    let store = Store::open(&path)?;
    assert!(store.history(10)?.is_empty());
    let backup = Connection::open(appended(&path, ".bak"))?;
    let saved: i64 = backup.query_row("SELECT value FROM damaged", [], |row| row.get(0))?;
    assert_eq!(saved, -1);
    Ok(())
}

#[test]
fn committed_wal_data_is_loaded_without_false_corruption_recovery() -> Result<()> {
    let directory = tempdir()?;
    let path = directory.path().join("contest.sqlite");
    let (contest, standings) = fixture();
    let mut store = Store::open(&path)?;
    let keeper = Connection::open(&path)?;
    keeper.set_db_config(DbConfig::SQLITE_DBCONFIG_NO_CKPT_ON_CLOSE, true)?;
    keeper.execute_batch("PRAGMA wal_checkpoint(TRUNCATE)")?;
    store.save_contest(&contest, &standings)?;
    drop(store);
    drop(keeper);
    assert!(fs::metadata(appended(&path, "-wal"))?.len() > 0);
    let store = Store::open(&path)?;
    let loaded = store
        .load_contest(&contest.id)?
        .expect("contest committed in WAL");
    assert_eq!(loaded.1, standings);
    assert_eq!(loaded.0.seed, u64::MAX);
    assert!(backups(&path)?.is_empty());
    Ok(())
}

#[test]
fn confirmed_corruption_keeps_committed_wal_results_in_the_backup() -> Result<()> {
    let directory = tempdir()?;
    let path = directory.path().join("contest.sqlite");
    let (mut contest, standings) = fixture();
    let mut store = Store::open(&path)?;
    store.save_contest(&contest, &standings)?;
    let keeper = Connection::open(&path)?;
    keeper.set_db_config(DbConfig::SQLITE_DBCONFIG_NO_CKPT_ON_CLOSE, true)?;
    keeper.execute_batch(
        "CREATE TABLE damaged(payload TEXT);
         INSERT INTO damaged VALUES('old page, separate from contest results');
         PRAGMA wal_checkpoint(TRUNCATE);",
    )?;
    let page_size: usize = keeper.pragma_query_value(None, "page_size", |row| row.get(0))?;
    let root_page: usize = keeper.query_row(
        "SELECT rootpage FROM sqlite_schema WHERE name='damaged'",
        [],
        |row| row.get(0),
    )?;
    contest.id = "wal-only-contest".into();
    store.save_contest(&contest, &standings)?;
    drop(store);
    drop(keeper);
    let wal = fs::read(appended(&path, "-wal"))?;
    assert!(!wal.is_empty());
    // Damage an unrelated btree page. The WAL contains a committed contest that
    // has never been checkpointed into the main file and must survive recovery.
    let mut damaged = fs::read(&path)?;
    damaged[(root_page - 1) * page_size] = 0xff;
    fs::write(&path, &damaged)?;
    let store = Store::open(&path)?;
    assert!(store.history(10)?.is_empty());
    assert_eq!(fs::read(appended(&path, ".bak"))?, damaged);
    assert_eq!(fs::read(appended(&path, ".bak-wal"))?, wal);
    let backup = Connection::open(appended(&path, ".bak"))?;
    let preserved: String = backup.query_row(
        "SELECT snapshot_json FROM contests WHERE id=?1",
        [&contest.id],
        |row| row.get(0),
    )?;
    assert_eq!(serde_json::from_str::<Contest>(&preserved)?.id, contest.id);
    Ok(())
}

#[test]
fn locked_databases_error_without_backup_or_recreation() -> Result<()> {
    let directory = tempdir()?;
    let path = directory.path().join("contest.sqlite");
    let (contest, standings) = fixture();
    {
        let mut store = Store::open(&path)?;
        store.save_contest(&contest, &standings)?;
    }
    let locker = Connection::open(&path)?;
    locker.execute_batch("PRAGMA journal_mode=DELETE; BEGIN EXCLUSIVE;")?;
    assert!(Store::open(&path).is_err());
    assert!(backups(&path)?.is_empty());
    locker.execute_batch("ROLLBACK")?;
    drop(locker);
    let store = Store::open(&path)?;
    assert_eq!(
        store
            .load_contest(&contest.id)?
            .expect("untouched contest")
            .1,
        standings
    );
    Ok(())
}

#[test]
fn incompatible_schemas_error_without_backup_or_recreation() -> Result<()> {
    let directory = tempdir()?;
    for (name, schema) in [
        ("conflicting", "CREATE TABLE contests (unrelated TEXT); INSERT INTO contests VALUES('keep');"),
        ("future", "PRAGMA user_version=999; CREATE TABLE sentinel(value TEXT); INSERT INTO sentinel VALUES('keep');"),
        ("incomplete", "PRAGMA user_version=1; CREATE TABLE sentinel(value TEXT); INSERT INTO sentinel VALUES('keep');"),
    ] {
        let path = directory.path().join(format!("{name}.sqlite"));
        {
            let connection = Connection::open(&path)?;
            connection.execute_batch(schema)?;
        }
        let original = fs::read(&path)?;
        assert!(Store::open(&path).is_err(), "{name}");
        assert!(backups(&path)?.is_empty(), "{name}");
        assert_eq!(fs::read(&path)?, original, "{name}");
    }
    Ok(())
}

#[test]
fn unopenable_path_is_not_treated_as_corruption() -> Result<()> {
    let directory = tempdir()?;
    let path = directory.path().join("a-directory.sqlite");
    fs::create_dir(&path)?;
    let sentinel = path.join("keep.txt");
    fs::write(&sentinel, b"keep")?;
    assert!(Store::open(&path).is_err());
    assert_eq!(fs::read(sentinel)?, b"keep");
    assert!(backups(&path)?.is_empty());
    Ok(())
}
