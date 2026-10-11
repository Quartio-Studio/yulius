-- Yulius schema (SPEC-001 DLV-006, SPEC-002 ADR-20): Yulius is the projects' CI. A run tests one
-- commit of a sandbox with the sandbox's CI jobs (in parallel) and is published as the commit
-- status `yulius/ci`.
CREATE TABLE ci_runs (
    id TEXT PRIMARY KEY,
    sandbox_id TEXT NOT NULL,
    reference TEXT NOT NULL,        -- pr/<number> | branch/<name>
    sha TEXT NOT NULL,              -- commit tested
    source TEXT NOT NULL,           -- profile | yulius_toml | pull_request
    state TEXT NOT NULL,            -- queued | running | passed | failed | error | cancelled
    reason TEXT,                    -- why it errored or was cancelled
    requested_by TEXT NOT NULL,     -- 'yulius' or the login of the person or task that asked
    status_for TEXT,                -- run state whose commit status GitHub last accepted
    status_note TEXT,               -- why the last publication failed
    status_attempts INTEGER NOT NULL DEFAULT 0,
    queued_at INTEGER NOT NULL,
    started_at INTEGER,
    ended_at INTEGER
);
CREATE INDEX ci_runs_sandbox ON ci_runs(sandbox_id, queued_at);
CREATE INDEX ci_runs_sha ON ci_runs(sandbox_id, sha);
CREATE INDEX ci_runs_active ON ci_runs(state) WHERE state IN ('queued', 'running');
-- One running run per sandbox: its jobs use the sandbox's CI checkouts.
CREATE UNIQUE INDEX one_running_ci_run ON ci_runs(sandbox_id) WHERE state = 'running';

-- The jobs of a run (matrix values expanded), run in parallel.
CREATE TABLE ci_jobs (
    id TEXT PRIMARY KEY,
    run_id TEXT NOT NULL REFERENCES ci_runs(id) ON DELETE CASCADE,
    ordinal INTEGER NOT NULL,
    name TEXT NOT NULL,             -- e.g. "test (1/2)"
    job_key TEXT NOT NULL,          -- definition name: selects the persistent checkout and cache
    command TEXT NOT NULL,          -- audit copy of the shell text
    matrix TEXT,                    -- value passed as YULIUS_CI_MATRIX
    state TEXT NOT NULL,            -- queued | running | passed | failed | error | cancelled
    reason TEXT,
    exit_code INTEGER,
    output TEXT NOT NULL DEFAULT '', -- bounded tail of the terminal output
    started_at INTEGER,
    ended_at INTEGER,
    UNIQUE (run_id, ordinal)
);
CREATE INDEX ci_jobs_run ON ci_jobs(run_id, ordinal);
