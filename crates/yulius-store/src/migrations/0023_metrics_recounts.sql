-- Runs recorded before SPEC-087 stored figures Yulius kept in memory (SPEC-091): every run
-- recorded so far is queued here once, and counted again from its transcript after start.
-- `outcome` is NULL while queued (recomputed | no_transcript | no_lines | live | gone after);
-- `before` is the stored figures as JSON when queued (NULL: no row), before this start's
-- reconciliation saves anything; `after` is them once recorded.
CREATE TABLE metrics_recounts (
    attempt_id TEXT PRIMARY KEY,
    task_id TEXT NOT NULL,
    outcome TEXT,
    before TEXT,
    after TEXT,
    done_at INTEGER
);
INSERT INTO metrics_recounts (attempt_id, task_id, before)
SELECT a.id, a.task_id,
    CASE WHEN m.attempt_id IS NULL THEN NULL ELSE json_object(
        'model', m.model, 'input_tokens', m.input_tokens, 'output_tokens', m.output_tokens,
        'cache_read', m.cache_read, 'cache_write', m.cache_write, 'turns', m.turns,
        'active_ms', m.active_ms) END
FROM attempts a LEFT JOIN metrics m ON m.attempt_id = a.id;
