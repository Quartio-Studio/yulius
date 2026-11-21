-- SPEC-606. PR numbers own admission places even when their heads share one CI run.
ALTER TABLE sandboxes ADD COLUMN ci_open_pulls TEXT NOT NULL DEFAULT '[]'
    CHECK (json_type(ci_open_pulls) = 'array' AND json_array_length(ci_open_pulls) <= 30);

-- The old scheduler recorded only runs. Carry forward its active PRs in queue order.
UPDATE sandboxes SET ci_open_pulls = (
    SELECT json_group_array(number) FROM (
        SELECT CAST(substr(reference, 4) AS INTEGER) AS number FROM ci_runs
        WHERE sandbox_id = sandboxes.id AND state IN ('queued', 'running') AND reference LIKE 'pr/%'
        GROUP BY reference ORDER BY MIN(queued_at), MIN(rowid) LIMIT 30
    )
);
