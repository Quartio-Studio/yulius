-- Expand the staging state constraint and preserve every existing receipt.
ALTER TABLE staging_requests RENAME TO staging_requests_before_inheritance;
CREATE TABLE staging_requests (
    id TEXT PRIMARY KEY,
    sandbox_id TEXT NOT NULL REFERENCES sandboxes(id) ON DELETE CASCADE,
    environment TEXT NOT NULL,
    -- A deleted task keeps its requests (what staging verified stays known), without the task.
    task_id TEXT REFERENCES tasks(id) ON DELETE SET NULL,
    revision TEXT NOT NULL,
    state TEXT NOT NULL CHECK (state IN ('testing', 'ready', 'deploying', 'verified', 'included',
        'gate_failed', 'inherited', 'deploy_failed', 'superseded')),
    -- The full gate's run that judged the revision (any reference), once known.
    ci_run_id TEXT,
    -- The CD run that deployed it, or the verified one that includes it.
    cd_run_id TEXT,
    attempts INTEGER NOT NULL DEFAULT 0,
    note TEXT,
    requested_by TEXT NOT NULL,
    -- Whether its outcome was told to its task's session.
    told INTEGER NOT NULL DEFAULT 0,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);
INSERT INTO staging_requests SELECT * FROM staging_requests_before_inheritance;
DROP TABLE staging_requests_before_inheritance;

-- A repeated request is the same row: one per requester and revision while not superseded.
CREATE UNIQUE INDEX staging_request_once ON staging_requests(
    sandbox_id, environment, COALESCE(task_id, id), revision
) WHERE state != 'superseded';
-- One successful staging deployment per task and environment.
CREATE UNIQUE INDEX staging_task_success ON staging_requests(sandbox_id, environment, task_id)
    WHERE task_id IS NOT NULL AND state IN ('verified', 'included');
-- One deploy at a time per environment.
CREATE UNIQUE INDEX staging_one_deploying ON staging_requests(sandbox_id, environment)
    WHERE state = 'deploying';
CREATE INDEX staging_pending ON staging_requests(sandbox_id, environment, state);
CREATE INDEX staging_of_task ON staging_requests(task_id);

-- Retention keeps the compact first-parent comparison evidence after job logs expire.
CREATE TABLE archived_gate_verdicts (
    sandbox_id TEXT NOT NULL REFERENCES sandboxes(id) ON DELETE CASCADE,
    reference TEXT NOT NULL,
    sha TEXT NOT NULL,
    run_id TEXT NOT NULL,
    state TEXT NOT NULL,
    queued_at INTEGER NOT NULL,
    sequence INTEGER NOT NULL,
    jobs TEXT NOT NULL,
    PRIMARY KEY (sandbox_id, sha, reference)
);

-- A task's deletion removes task_merges. Keep its public number with its landing for receipts.
CREATE TABLE gate_merge_owners (
    sandbox_id TEXT NOT NULL REFERENCES sandboxes(id) ON DELETE CASCADE,
    branch TEXT NOT NULL,
    sha TEXT NOT NULL,
    task_number INTEGER NOT NULL,
    PRIMARY KEY (sandbox_id, branch, sha)
);
INSERT OR IGNORE INTO gate_merge_owners
SELECT m.sandbox_id, m.branch, m.merged_sha, t.number
FROM task_merges m JOIN tasks t ON t.id = m.task_id ORDER BY m.rowid;
CREATE TRIGGER keep_gate_merge_owner AFTER INSERT ON task_merges BEGIN
    INSERT OR IGNORE INTO gate_merge_owners
    SELECT NEW.sandbox_id, NEW.branch, NEW.merged_sha, number FROM tasks WHERE id = NEW.task_id;
END;

-- Reconsider failures still waiting for delivery when the instance upgrades. A receipt
-- already delivered records what the old instance actually told its task.
UPDATE staging_requests SET state = 'testing'
WHERE state = 'gate_failed' AND told = 0 AND EXISTS (
    SELECT 1 FROM ci_runs r WHERE r.id = staging_requests.ci_run_id
      AND r.sha = staging_requests.revision AND r.sandbox_id = staging_requests.sandbox_id
      AND r.state IN ('failed', 'error')
);

-- Machine-generated input is restricted to live work. A newer open PR can follow an
-- earlier landing from the same task. An explicitly open step keeps a merged task working.
CREATE VIEW machine_notice_tasks AS
SELECT t.id, t.sandbox_id, t.number FROM tasks t LEFT JOIN delivery d ON d.task_id = t.id
WHERE t.status = 'running' AND t.archived_at IS NULL AND (
    EXISTS (SELECT 1 FROM steps s WHERE s.task_id = t.id AND s.status NOT IN ('done', 'skipped'))
    OR d.stage = 'pr_open'
    OR (COALESCE(d.stage, 'none') = 'none' AND NOT EXISTS (
        SELECT 1 FROM task_merges m WHERE m.task_id = t.id))
);
CREATE VIEW gate_workers AS
SELECT n.* FROM machine_notice_tasks n JOIN tasks t ON t.id = n.id
JOIN attempts a ON a.task_id = t.id AND a.state = 'live'
WHERE COALESCE(t.activity, '') != 'needs_attention' AND t.needs_you IS NULL
AND (t.paused_attempt IS NULL OR t.paused_attempt != a.id)
AND NOT EXISTS (SELECT 1 FROM prompt_holds h WHERE h.task_id = t.id AND h.state = 'pending')
AND (COALESCE(t.activity, 'working') = 'working' OR EXISTS (
    SELECT 1 FROM steps s WHERE s.task_id = t.id AND s.status NOT IN ('done','skipped'))
    OR EXISTS (SELECT 1 FROM delivery d WHERE d.task_id = t.id AND d.stage = 'pr_open'));

-- One repair owner per project while the integration branch is red. Delivery receipts
-- expire with the assignment. This bounds them by the number of workers in the project.
CREATE TABLE gate_repairs (
    sandbox_id TEXT PRIMARY KEY REFERENCES sandboxes(id) ON DELETE CASCADE,
    owner_id TEXT NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
    detail TEXT NOT NULL
);
CREATE TABLE gate_repair_told (
    sandbox_id TEXT NOT NULL REFERENCES gate_repairs(sandbox_id) ON DELETE CASCADE,
    task_id TEXT NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
    PRIMARY KEY (sandbox_id, task_id)
);
