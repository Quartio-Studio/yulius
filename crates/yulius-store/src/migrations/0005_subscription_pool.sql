-- Yulius schema version 5 (SPEC-001 F 2026-10-10, SPEC-002 ADR-17): Claude subscriptions are one
-- self-managing pool. Accounts lose their manual priority and enabled switch (every signed-in
-- account is in the pool; an administrator removes one to take it out). Pauses for lack of
-- capacity and the provider's usage readings are recorded for the resume loop and the forecast.
ALTER TABLE accounts DROP COLUMN priority;
ALTER TABLE accounts DROP COLUMN enabled;

-- One row per pause; at most one open (ended_at IS NULL) per task.
CREATE TABLE capacity_pauses (
    id TEXT PRIMARY KEY,
    task_id TEXT NOT NULL REFERENCES tasks(id),
    kind TEXT NOT NULL,             -- turn | prompt | limit | start
    account_id TEXT,                -- the paused process's account (NULL without a process)
    continuation TEXT,              -- held prompt carried to another account (kind prompt)
    started_at INTEGER NOT NULL,
    ended_at INTEGER,
    outcome TEXT                    -- released | switched | started | cancelled | ended | superseded
);
CREATE UNIQUE INDEX capacity_pauses_open ON capacity_pauses(task_id) WHERE ended_at IS NULL;
CREATE INDEX capacity_pauses_started ON capacity_pauses(started_at);

-- Provider readings of each usage window, kept 8 days (burn rate over 24 h and 7 d).
CREATE TABLE usage_samples (
    account_id TEXT NOT NULL,
    at INTEGER NOT NULL,
    window_name TEXT NOT NULL,      -- five_hour | seven_day | seven_day_opus | seven_day_sonnet
    used_percent REAL NOT NULL,
    resets_at INTEGER,
    source TEXT NOT NULL DEFAULT 'probe'
);
CREATE INDEX usage_samples_at ON usage_samples(at);

-- Tasks that were waiting for capacity under the previous policy are handed to the resume loop:
-- with a live process they wait at a reported limit, without one they wait to start.
INSERT INTO capacity_pauses (id, task_id, kind, account_id, started_at)
SELECT 'migrated-' || t.id, t.id,
       CASE WHEN a.id IS NULL THEN 'start' ELSE 'limit' END,
       a.account_id, t.updated_at
FROM tasks t
LEFT JOIN attempts a ON a.task_id = t.id AND a.state IN ('spawning', 'live')
WHERE t.status = 'waiting_capacity';
