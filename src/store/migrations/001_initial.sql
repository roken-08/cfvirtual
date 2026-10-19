-- Decimal TEXT preserves all u64 seeds and penalties without SQLite's signed
-- INTEGER / floating-point coercion. JSON snapshots are the export authority.
CREATE TABLE contests (
    id TEXT PRIMARY KEY NOT NULL,
    code TEXT,
    host_handle TEXT NOT NULL,
    started_at INTEGER NOT NULL,
    duration_sec INTEGER NOT NULL,
    seed TEXT NOT NULL,
    mode TEXT NOT NULL,
    problems_json TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    snapshot_json TEXT NOT NULL,
    standings_json TEXT NOT NULL
);

CREATE TABLE participants (
    contest_id TEXT NOT NULL REFERENCES contests(id) ON DELETE CASCADE,
    handle TEXT NOT NULL,
    is_host INTEGER NOT NULL,
    is_bot INTEGER NOT NULL,
    rating INTEGER NOT NULL,
    solved INTEGER NOT NULL,
    penalty TEXT NOT NULL,
    last_ac INTEGER NOT NULL,
    is_spectator INTEGER NOT NULL,
    connected INTEGER NOT NULL,
    PRIMARY KEY (contest_id, handle)
);

CREATE TABLE results (
    contest_id TEXT NOT NULL,
    handle TEXT NOT NULL,
    slot TEXT NOT NULL,
    accepted_at_sec INTEGER,
    rejected_attempts INTEGER NOT NULL,
    rating INTEGER,
    tags_json TEXT NOT NULL,
    solved INTEGER NOT NULL,
    attempts INTEGER NOT NULL,
    first_attempt_at_sec INTEGER,
    PRIMARY KEY (contest_id, handle, slot),
    FOREIGN KEY (contest_id, handle)
        REFERENCES participants(contest_id, handle) ON DELETE CASCADE
);

-- No foreign key to contests: a post-mortem may queue a problem before saving
-- the contest, and the queue carries its own immutable problem metadata.
CREATE TABLE review_queue (
    problem_key TEXT PRIMARY KEY NOT NULL,
    contest_id TEXT NOT NULL,
    added_at INTEGER NOT NULL,
    due_at INTEGER NOT NULL,
    interval_days INTEGER NOT NULL,
    ease REAL NOT NULL,
    lapses INTEGER NOT NULL,
    problem_json TEXT NOT NULL
);

CREATE TABLE meta (key TEXT PRIMARY KEY NOT NULL, value TEXT NOT NULL);
CREATE INDEX contests_history ON contests(started_at DESC, id);
CREATE INDEX review_due ON review_queue(due_at, added_at, problem_key);
