-- Runs recorded before SPEC-087 stored figures Yulius kept in memory (SPEC-091): every run
-- recorded so far is queued here once, and counted again from its transcript after start.
-- `outcome` is NULL while queued (recomputed | no_transcript | no_lines | live | gone after);
-- `before` and `after` are the stored figures as JSON (NULL: no row).
CREATE TABLE metrics_recounts (
    attempt_id TEXT PRIMARY KEY,
    task_id TEXT NOT NULL,
    outcome TEXT,
    before TEXT,
    after TEXT,
    done_at INTEGER
);
INSERT INTO metrics_recounts (attempt_id, task_id) SELECT id, task_id FROM attempts;
