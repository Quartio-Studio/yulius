-- Merge-first staging (SPEC-575 MFS-003 to MFS-008): each request of a task to change its
-- project's shared staging target to a revision of the integration branch.
CREATE TABLE staging_requests (
    id TEXT PRIMARY KEY,
    sandbox_id TEXT NOT NULL REFERENCES sandboxes(id) ON DELETE CASCADE,
    environment TEXT NOT NULL,
    -- A deleted task keeps its requests (what staging verified stays known), without the task.
    task_id TEXT REFERENCES tasks(id) ON DELETE SET NULL,
    revision TEXT NOT NULL,
    state TEXT NOT NULL CHECK (state IN ('testing', 'ready', 'deploying', 'verified', 'included',
        'gate_failed', 'deploy_failed', 'superseded')),
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
