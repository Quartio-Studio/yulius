-- Yulius migration 0062, schema version 62 (SPEC-493 REV-002, REV-004): each round of a task's
-- independent code review, as Yulius saw it start and end. The rounds of a pull request are the
-- ones not yet closed by a merge (`merged_at`); a round that ended without a report (cut off by a
-- usage limit, or still running) keeps its number when the reviewer is started again.
CREATE TABLE IF NOT EXISTS review_rounds (
    id TEXT PRIMARY KEY,
    task_id TEXT NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
    round INTEGER NOT NULL,
    head TEXT,
    agent TEXT NOT NULL,
    model TEXT NOT NULL,
    reason TEXT NOT NULL,
    subagent_id TEXT,
    model_answered TEXT,
    started_at INTEGER NOT NULL,
    ended_at INTEGER,
    verdict TEXT,
    cut_off INTEGER NOT NULL DEFAULT 0,
    pr_number INTEGER,
    merged_at INTEGER
);
CREATE INDEX IF NOT EXISTS review_rounds_task ON review_rounds(task_id, started_at);
CREATE INDEX IF NOT EXISTS review_rounds_subagent ON review_rounds(subagent_id);
