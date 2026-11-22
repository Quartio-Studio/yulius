-- Yulius migration 0093 (SPEC-630, ADR-0465): script watches. A watch is a project's shell
-- script on a schedule (an interval in `every_seconds`, or a `cron` expression in
-- `utc_offset_minutes`), run by Yulius in the sandbox without an agent; what it prints becomes
-- a notice keyed `watch:<name>` for its owner. Its state (last run, failures in a row, whether
-- the warning was posted, a run still going) lives on the row, so it survives restarts.
CREATE TABLE watches (
    id TEXT PRIMARY KEY,
    sandbox_id TEXT NOT NULL REFERENCES sandboxes(id) ON DELETE CASCADE,
    name TEXT NOT NULL,
    script TEXT NOT NULL,
    every_seconds INTEGER,
    cron TEXT,
    utc_offset_minutes INTEGER NOT NULL DEFAULT 0,
    expiry TEXT,
    timeout_seconds INTEGER NOT NULL,
    secrets TEXT NOT NULL DEFAULT '[]',
    owner_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    created_via TEXT NOT NULL DEFAULT 'ui',
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    paused INTEGER NOT NULL DEFAULT 0,
    since INTEGER NOT NULL,
    ended_reason TEXT,
    ended_at INTEGER,
    run_id TEXT,
    running_since INTEGER,
    last_started_at INTEGER,
    last_ended_at INTEGER,
    last_exit INTEGER,
    last_failure TEXT,
    last_output TEXT,
    last_errors TEXT,
    failures INTEGER NOT NULL DEFAULT 0,
    warned INTEGER NOT NULL DEFAULT 0,
    runs INTEGER NOT NULL DEFAULT 0,
    posts INTEGER NOT NULL DEFAULT 0,
    last_posted_at INTEGER,
    UNIQUE (sandbox_id, name),
    CHECK ((every_seconds IS NULL) <> (cron IS NULL))
);

-- Posts a watch could not store yet, newest per notice key (WCH-008): `body` is the notice as
-- JSON, NULL to remove the notice of that key.
CREATE TABLE watch_pending (
    watch_id TEXT NOT NULL REFERENCES watches(id) ON DELETE CASCADE,
    notice_key TEXT NOT NULL,
    body TEXT,
    queued_at INTEGER NOT NULL,
    PRIMARY KEY (watch_id, notice_key)
);
