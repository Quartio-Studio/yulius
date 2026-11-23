-- SPEC-619 CSA-4: the parts of a run's tokens that another model than its own answered (a Codex
-- subagent's), as a JSON array of {model, input_tokens, output_tokens, cache_read, cache_write};
-- NULL when there are none. The run's token columns include them.
ALTER TABLE metrics ADD COLUMN model_parts TEXT;
-- CSA-7: Codex runs that ended in the last 7 days counted their main rollout only. Each is
-- queued for one recount after start (SPEC-091), which adds its subagents' rollouts.
INSERT INTO metrics_recounts (attempt_id, task_id, before)
SELECT a.id, a.task_id,
    CASE WHEN m.attempt_id IS NULL THEN NULL ELSE json_object(
        'model', m.model, 'input_tokens', m.input_tokens, 'output_tokens', m.output_tokens,
        'cache_read', m.cache_read, 'cache_write', m.cache_write, 'turns', m.turns,
        'active_ms', m.active_ms) END
FROM attempts a LEFT JOIN metrics m ON m.attempt_id = a.id
WHERE a.harness = 'codex'
    AND a.ended_at >= (CAST(strftime('%s', 'now') AS INTEGER) - 7 * 86400) * 1000
ON CONFLICT (attempt_id) DO UPDATE SET outcome = NULL, before = excluded.before, after = NULL,
    done_at = NULL;
