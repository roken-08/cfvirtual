use super::{HistoryEntry, HistoryProfile, ReviewEntry, Stats};
use crate::domain::{Contest, Problem, Standing};
use anyhow::{bail, Context, Result};
use chrono::{NaiveDate, Utc};
use rusqlite::{
    config::DbConfig, params, Connection, ErrorCode, OptionalExtension, Transaction,
    TransactionBehavior,
};
use std::collections::BTreeSet;
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const DAY: i64 = 86_400;
const MIGRATIONS: &[&str] = &[include_str!("migrations/001_initial.sql")];

#[derive(Debug)]
pub struct Store {
    connection: Connection,
}

impl Store {
    /// Open and migrate a database. Only confirmed SQLite corruption triggers
    /// recovery; busy, permission, I/O and incompatible-schema errors propagate.
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            fs::create_dir_all(parent)
                .with_context(|| format!("create database directory {}", parent.display()))?;
        }
        let mut connection = match checked_connection(path) {
            Ok(connection) => connection,
            Err(OpenError::Other(error)) => return Err(error),
            Err(OpenError::Corrupt(error)) => {
                let backup = backup_corrupt_database(path)?;
                eprintln!(
                    "Warning: corrupt database {} ({error:#}); backed up to {} and recreating it",
                    path.display(),
                    backup.display()
                );
                match checked_connection(path) {
                    Ok(connection) => connection,
                    Err(OpenError::Other(error) | OpenError::Corrupt(error)) => return Err(error),
                }
            }
        };
        connection.pragma_update(None, "foreign_keys", true)?;
        migrate(&mut connection)?;
        connection.pragma_update(None, "journal_mode", "WAL")?;
        connection.pragma_update(None, "synchronous", "FULL")?;
        Ok(Self { connection })
    }

    /// Atomically replace a contest's snapshot and normalized results. Repeating
    /// an identical save never creates additional history or result rows.
    pub fn save_contest(&mut self, contest: &Contest, standings: &[Standing]) -> Result<()> {
        // Participant's serde contract excludes tokens, including nested ones.
        let snapshot = serde_json::to_string(contest)?;
        let scoreboard = serde_json::to_string(standings)?;
        let problems = serde_json::to_string(&contest.problems)?;
        let transaction = self.connection.transaction()?;
        transaction.execute(
            "INSERT INTO contests
             (id, code, host_handle, started_at, duration_sec, seed, mode,
              problems_json, created_at, snapshot_json, standings_json)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
             ON CONFLICT(id) DO UPDATE SET
               code=excluded.code, host_handle=excluded.host_handle,
               started_at=excluded.started_at, duration_sec=excluded.duration_sec,
               seed=excluded.seed, mode=excluded.mode, problems_json=excluded.problems_json,
               snapshot_json=excluded.snapshot_json, standings_json=excluded.standings_json",
            params![
                contest.id,
                contest.code,
                contest.host_handle,
                contest.started_at,
                contest.duration_sec,
                contest.seed.to_string(),
                contest.mode,
                problems,
                Utc::now().timestamp(),
                snapshot,
                scoreboard,
            ],
        )?;
        transaction.execute("DELETE FROM results WHERE contest_id=?1", [&contest.id])?;
        transaction.execute(
            "DELETE FROM participants WHERE contest_id=?1",
            [&contest.id],
        )?;
        save_participants(&transaction, contest, standings)?;
        save_results(&transaction, contest, standings)?;
        transaction.commit().context("commit contest snapshot")
    }

    pub fn load_contest(&self, id: &str) -> Result<Option<(Contest, Vec<Standing>)>> {
        let stored: Option<(String, String)> = self
            .connection
            .query_row(
                "SELECT snapshot_json, standings_json FROM contests WHERE id=?1",
                [id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        stored
            .map(|(contest, standings)| decode_snapshot(&contest, &standings))
            .transpose()
    }

    /// Store aggregate public-history statistics, not submission histories or tokens.
    pub fn save_history_profile(
        &self,
        handle: &str,
        rating: u32,
        profile: &crate::domain::weakness::WeaknessProfile,
        now: i64,
    ) -> Result<()> {
        let summary = HistoryProfile {
            handle: handle.into(),
            rating,
            updated_at: now,
            tags: profile.tags.clone(),
            weights: profile.weights.clone(),
        };
        self.connection.execute("INSERT INTO meta(key,value) VALUES ('history_profile',?1) ON CONFLICT(key) DO UPDATE SET value=excluded.value", [serde_json::to_string(&summary)?])?;
        Ok(())
    }

    /// Newest contests first, with host scores (not the leaderboard winner).
    pub fn history(&self, limit: usize) -> Result<Vec<HistoryEntry>> {
        let mut statement = self.connection.prepare(
            "SELECT c.id, c.mode, c.started_at, c.problems_json,
                    COALESCE(p.solved, 0), COALESCE(p.penalty, '0'), c.host_handle
             FROM contests c LEFT JOIN participants p
               ON p.contest_id=c.id AND p.handle=c.host_handle
                  AND p.is_bot=0 AND p.is_spectator=0
              WHERE NOT EXISTS (
                SELECT 1 FROM contests source
                WHERE c.id=source.id || ':participant:' || lower(source.host_handle)
                  AND c.host_handle=source.host_handle AND source.mode='room'
              )
              ORDER BY c.started_at DESC, c.id ASC LIMIT ?1",
        )?;
        let mut rows = statement.query([sql_limit(limit)])?;
        let mut history = Vec::new();
        while let Some(row) = rows.next()? {
            let problems: String = row.get(3)?;
            let penalty: String = row.get(5)?;
            history.push(HistoryEntry {
                id: row.get(0)?,
                mode: row.get(1)?,
                started_at: row.get(2)?,
                problem_count: serde_json::from_str::<Vec<serde_json::Value>>(&problems)?.len(),
                solved: row.get(4)?,
                penalty: penalty.parse().context("invalid stored penalty")?,
                host_handle: row.get(6)?,
            });
        }
        Ok(history)
    }

    /// Aggregate each saved contest once using only its human host result.
    /// Tag denominators include every assigned problem, even unattempted ones.
    pub fn stats(&self) -> Result<Stats> {
        let mut stats = Stats::default();
        let mut dates = BTreeSet::new();
        let mut total_penalty = 0.0;
        let mut statement = self
            .connection
            .prepare("SELECT c.snapshot_json, c.standings_json FROM contests c WHERE NOT EXISTS (
                SELECT 1 FROM contests source WHERE c.id=source.id || ':participant:' || lower(source.host_handle)
                AND c.host_handle=source.host_handle AND source.mode='room')")?;
        let mut rows = statement.query([])?;
        while let Some(row) = rows.next()? {
            let (contest, standings) =
                decode_snapshot(&row.get::<_, String>(0)?, &row.get::<_, String>(1)?)?;
            let spectator = contest
                .participants
                .iter()
                .any(|p| p.handle == contest.host_handle && p.is_spectator);
            if spectator {
                continue;
            }
            let host = standings
                .iter()
                .find(|s| s.name == contest.host_handle && !s.is_bot && !spectator);
            stats.total_contests += 1;
            stats.total_problems += contest.problems.len();
            if let Some(host) = host {
                stats.total_solved += host.solved as usize;
                total_penalty += host.penalty as f64;
            }
            if let Some(time) = chrono::DateTime::from_timestamp(contest.started_at, 0) {
                dates.insert(time.date_naive());
            }
            for problem in &contest.problems {
                let solved = host
                    .and_then(|s| s.problems.get(&problem.slot))
                    .is_some_and(|s| s.solved);
                // Duplicate metadata tags must not count a problem twice.
                let tags: BTreeSet<_> = problem.problem.tags.iter().collect();
                for tag in tags {
                    let tag_stats = stats.per_tag.entry(tag.clone()).or_default();
                    tag_stats.problems += 1;
                    tag_stats.solved += usize::from(solved);
                }
            }
        }
        stats.solve_rate = fraction(stats.total_solved, stats.total_problems);
        if stats.total_contests > 0 {
            stats.average_penalty = total_penalty / stats.total_contests as f64;
        }
        for tag in stats.per_tag.values_mut() {
            tag.solve_rate = fraction(tag.solved, tag.problems);
        }
        stats.current_streak = streak(&dates, Utc::now().date_naive());
        let profile: Option<String> = self
            .connection
            .query_row(
                "SELECT value FROM meta WHERE key='history_profile'",
                [],
                |row| row.get(0),
            )
            .optional()?;
        stats.history_profile = profile
            .map(|text| serde_json::from_str(&text))
            .transpose()?;
        Ok(stats)
    }

    /// Queue unsolved problems once. Re-enqueuing never resets an existing
    /// schedule. New entries are due one day after `now`; bots are ignored.
    pub fn enqueue_failed(
        &mut self,
        contest: &Contest,
        standing: &Standing,
        now: i64,
    ) -> Result<usize> {
        if standing.is_bot
            || contest
                .participants
                .iter()
                .any(|p| p.handle == standing.name && p.is_spectator)
        {
            return Ok(0);
        }
        let due_at = due_after(now, 1)?;
        let transaction = self.connection.transaction()?;
        let mut added = 0;
        for problem in &contest.problems {
            if standing
                .problems
                .get(&problem.slot)
                .is_some_and(|s| s.solved)
            {
                continue;
            }
            added += transaction.execute(
                "INSERT INTO review_queue
                 (problem_key, contest_id, added_at, due_at, interval_days, ease, lapses, problem_json)
                 VALUES (?1, ?2, ?3, ?4, 1, 2.5, 0, ?5)
                 ON CONFLICT(problem_key) DO NOTHING",
                params![
                    problem.problem.key.to_string(),
                    contest.id,
                    now,
                    due_at,
                    serde_json::to_string(&problem.problem)?,
                ],
            )?;
        }
        transaction.commit()?;
        Ok(added)
    }

    pub fn review(&self, due_only: bool, limit: usize, now: i64) -> Result<Vec<ReviewEntry>> {
        let mut statement = self.connection.prepare(
            "SELECT problem_key, contest_id, added_at, due_at, interval_days,
                    ease, lapses, problem_json
             FROM review_queue WHERE (?1=0 OR due_at<=?2)
             ORDER BY due_at, added_at, problem_key LIMIT ?3",
        )?;
        let mut rows = statement.query(params![due_only, now, sql_limit(limit)])?;
        let mut entries = Vec::new();
        while let Some(row) = rows.next()? {
            let json: String = row.get(7)?;
            let problem: Problem = serde_json::from_str(&json).context("invalid review problem")?;
            let url = problem.url();
            entries.push(ReviewEntry {
                problem_key: row.get(0)?,
                contest_id: row.get(1)?,
                added_at: row.get(2)?,
                due_at: row.get(3)?,
                interval_days: row.get(4)?,
                ease: row.get(5)?,
                lapses: row.get(6)?,
                name: problem.name,
                url,
                rating: problem.rating,
                tags: problem.tags,
            });
        }
        Ok(entries)
    }

    /// A success advances 1 -> 3 -> 7 -> 14 -> 30 days (capped at 30).
    /// A failure/skip resets to one day and increments lapses.
    pub fn advance_review(&mut self, key: &str, solved: bool, now: i64) -> Result<()> {
        let transaction = self.connection.transaction()?;
        let current: Option<(u32, u32)> = transaction
            .query_row(
                "SELECT interval_days, lapses FROM review_queue WHERE problem_key=?1",
                [key],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        let (interval, lapses) = current.context("review problem not found")?;
        let next = next_interval(interval, solved);
        let lapses = if solved {
            lapses
        } else {
            lapses
                .checked_add(1)
                .context("review lapse count overflow")?
        };
        transaction.execute(
            "UPDATE review_queue SET interval_days=?2, due_at=?3, lapses=?4 WHERE problem_key=?1",
            params![key, next, due_after(now, next)?, lapses],
        )?;
        transaction.commit()?;
        Ok(())
    }
}

fn decode_snapshot(contest: &str, standings: &str) -> Result<(Contest, Vec<Standing>)> {
    Ok((
        serde_json::from_str(contest).context("invalid contest snapshot")?,
        serde_json::from_str(standings).context("invalid standings snapshot")?,
    ))
}

fn sql_limit(limit: usize) -> i64 {
    i64::try_from(limit).unwrap_or(i64::MAX)
}

fn fraction(numerator: usize, denominator: usize) -> f64 {
    if denominator == 0 {
        0.0
    } else {
        numerator as f64 / denominator as f64
    }
}

// Kept independent of domain::review so the store has no scheduling dependency
// on the export/UI implementation. This follows the same fixed interval ladder.
fn next_interval(current: u32, solved: bool) -> u32 {
    if !solved {
        return 1;
    }
    [1, 3, 7, 14, 30]
        .into_iter()
        .find(|&days| days > current)
        .unwrap_or(30)
}

fn due_after(now: i64, interval: u32) -> Result<i64> {
    now.checked_add(i64::from(interval) * DAY)
        .context("review due date overflow")
}

fn streak(dates: &BTreeSet<NaiveDate>, today: NaiveDate) -> u32 {
    let mut date = today;
    if !dates.contains(&date) {
        let Some(yesterday) = date.pred_opt() else {
            return 0;
        };
        date = yesterday;
    }
    let mut count = 0;
    while dates.contains(&date) {
        count += 1;
        let Some(previous) = date.pred_opt() else {
            break;
        };
        date = previous;
    }
    count
}

fn save_participants(
    transaction: &Transaction<'_>,
    contest: &Contest,
    standings: &[Standing],
) -> Result<()> {
    let mut statement = transaction.prepare(
        "INSERT INTO participants
         (contest_id, handle, is_host, is_bot, rating, solved, penalty, last_ac,
          is_spectator, connected)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
    )?;
    let mut roster_names = BTreeSet::new();
    for participant in &contest.participants {
        if !roster_names.insert(&participant.handle) {
            bail!("duplicate participant in contest: {}", participant.handle);
        }
    }
    for standing in standings {
        let participant = contest
            .participants
            .iter()
            .find(|p| p.handle == standing.name);
        if standing.is_bot && participant.is_some() {
            bail!("bot and human share a handle: {}", standing.name);
        }
        statement.execute(params![
            contest.id,
            standing.name,
            !standing.is_bot && standing.name == contest.host_handle,
            standing.is_bot,
            standing.rating,
            standing.solved,
            standing.penalty.to_string(),
            standing.last_ac,
            participant.is_some_and(|p| p.is_spectator),
            participant.is_none_or(|p| p.connected),
        ])?;
    }
    for participant in &contest.participants {
        if standings.iter().any(|s| s.name == participant.handle) {
            continue;
        }
        statement.execute(params![
            contest.id,
            participant.handle,
            participant.is_host,
            false,
            if participant.is_host {
                contest.host_rating
            } else {
                0
            },
            0,
            "0",
            0,
            participant.is_spectator,
            participant.connected,
        ])?;
    }
    for bot in &contest.bots {
        if standings.iter().any(|s| s.name == bot.name) {
            continue;
        }
        statement.execute(params![
            contest.id, bot.name, false, true, bot.rating, 0, "0", 0, false, true,
        ])?;
    }
    Ok(())
}

fn save_results(
    transaction: &Transaction<'_>,
    contest: &Contest,
    standings: &[Standing],
) -> Result<()> {
    let mut statement = transaction.prepare(
        "INSERT INTO results
         (contest_id, handle, slot, accepted_at_sec, rejected_attempts, rating,
          tags_json, solved, attempts, first_attempt_at_sec)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
    )?;
    for standing in standings {
        for (slot, status) in &standing.problems {
            let problem = contest
                .problems
                .iter()
                .find(|p| p.slot == *slot)
                .with_context(|| format!("result references unknown contest slot {slot}"))?;
            statement.execute(params![
                contest.id,
                standing.name,
                slot.to_string(),
                status.accepted_at_sec,
                status.rejected,
                problem.problem.rating,
                serde_json::to_string(&problem.problem.tags)?,
                status.solved,
                status.attempts,
                status.first_attempt_at_sec,
            ])?;
        }
    }
    Ok(())
}

fn migrate(connection: &mut Connection) -> Result<()> {
    // Read the version only after taking the migration lock: a concurrent open
    // may have applied the same migrations while we waited for its transaction.
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let version: usize = transaction.pragma_query_value(None, "user_version", |row| row.get(0))?;
    if version > MIGRATIONS.len() {
        bail!("unsupported database schema version {version}");
    }
    for (index, migration) in MIGRATIONS.iter().enumerate().skip(version) {
        transaction
            .execute_batch(migration)
            .with_context(|| format!("apply database migration {}", index + 1))?;
        transaction.pragma_update(None, "user_version", index + 1)?;
        transaction.execute(
            "INSERT INTO meta (key, value) VALUES ('schema_version', ?1)
             ON CONFLICT(key) DO UPDATE SET value=excluded.value",
            [(index + 1).to_string()],
        )?;
    }
    // Do not treat missing/incompatible application tables as corruption.
    // Preparing these queries validates a claimed current schema even if empty.
    for query in [
        "SELECT id, code, host_handle, started_at, duration_sec, seed, mode,
                problems_json, created_at, snapshot_json, standings_json FROM contests LIMIT 0",
        "SELECT contest_id, handle, is_host, is_bot, rating, solved, penalty,
                last_ac, is_spectator, connected FROM participants LIMIT 0",
        "SELECT contest_id, handle, slot, accepted_at_sec, rejected_attempts,
                rating, tags_json, solved, attempts, first_attempt_at_sec FROM results LIMIT 0",
        "SELECT problem_key, contest_id, added_at, due_at, interval_days, ease,
                lapses, problem_json FROM review_queue LIMIT 0",
        "SELECT key, value FROM meta LIMIT 0",
    ] {
        transaction
            .prepare(query)
            .context("incompatible database schema")?;
    }
    transaction.commit().context("commit database migrations")
}

enum OpenError {
    Corrupt(anyhow::Error),
    Other(anyhow::Error),
}

fn classify_open_error(error: rusqlite::Error) -> OpenError {
    if matches!(
        error.sqlite_error_code(),
        Some(ErrorCode::DatabaseCorrupt | ErrorCode::NotADatabase)
    ) {
        OpenError::Corrupt(error.into())
    } else {
        OpenError::Other(error.into())
    }
}

fn checked_connection(path: &Path) -> std::result::Result<Connection, OpenError> {
    let connection = Connection::open(path).map_err(classify_open_error)?;
    connection
        .busy_timeout(Duration::from_secs(2))
        .map_err(classify_open_error)?;
    // A failed quick_check must not checkpoint, truncate or unlink a WAL when
    // this connection closes. Recovery moves the main file and both sidecars.
    connection
        .set_db_config(DbConfig::SQLITE_DBCONFIG_NO_CKPT_ON_CLOSE, true)
        .map_err(classify_open_error)?;
    let checked = (|| -> rusqlite::Result<Vec<String>> {
        let mut statement = connection.prepare("PRAGMA quick_check")?;
        let messages = statement.query_map([], |row| row.get(0))?;
        messages.collect()
    })();
    match checked {
        Ok(messages) if messages.len() == 1 && messages[0] == "ok" => {}
        Ok(messages) if !messages.is_empty() => {
            return Err(OpenError::Corrupt(anyhow::anyhow!(
                "SQLite quick_check failed: {}",
                messages.join("; ")
            )));
        }
        Ok(_) => {
            return Err(OpenError::Other(anyhow::anyhow!(
                "SQLite quick_check returned no result"
            )));
        }
        Err(error) => return Err(classify_open_error(error)),
    }
    connection
        .set_db_config(DbConfig::SQLITE_DBCONFIG_NO_CKPT_ON_CLOSE, false)
        .map_err(classify_open_error)?;
    Ok(connection)
}

fn append_path(path: &Path, suffix: &str) -> PathBuf {
    let mut name: OsString = path.as_os_str().to_owned();
    name.push(suffix);
    name.into()
}

fn backup_corrupt_database(path: &Path) -> Result<PathBuf> {
    let base = append_path(path, ".bak");
    let timestamp = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
    let mut backup = base.clone();
    let mut attempt = 0_u64;
    while ["", "-wal", "-shm"]
        .iter()
        .map(|suffix| append_path(&backup, suffix).try_exists())
        .collect::<std::io::Result<Vec<_>>>()?
        .into_iter()
        .any(|exists| exists)
    {
        backup = append_path(&base, &format!(".{timestamp}.{attempt}"));
        attempt += 1;
    }
    let mut moved: Vec<(PathBuf, PathBuf)> = Vec::new();
    for suffix in ["", "-wal", "-shm"] {
        let source = append_path(path, suffix);
        let destination = append_path(&backup, suffix);
        if !suffix.is_empty() && !source.try_exists()? {
            continue;
        }
        if let Err(error) = fs::rename(&source, &destination) {
            let mut rollback_errors = Vec::new();
            for (source, destination) in moved.iter().rev() {
                if let Err(error) = fs::rename(destination, source) {
                    rollback_errors.push(format!("{}: {error}", destination.display()));
                }
            }
            bail!(
                "cannot back up corrupt database {} to {}: {error}; rollback errors: {rollback_errors:?}",
                source.display(),
                destination.display()
            );
        }
        moved.push((source, destination));
    }
    Ok(backup)
}
