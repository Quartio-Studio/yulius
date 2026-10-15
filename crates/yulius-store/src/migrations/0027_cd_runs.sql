-- Yulius schema version 27 (SPEC-134): CD (deployment). A CD run deploys one environment (`dev` or
-- `prod`) of a sandbox from its protected branch's head: the deploy then the verify command of
-- `[cd.<env>]` in yulius.toml at that commit, inside the sandbox's containment.
CREATE TABLE cd_runs (
    id TEXT PRIMARY KEY,
    sandbox_id TEXT NOT NULL,
    environment TEXT NOT NULL,      -- dev | prod
    branch TEXT NOT NULL,           -- the branch deployed
    sha TEXT NOT NULL,              -- its head, checked out
    state TEXT NOT NULL,            -- running | passed | failed | error | cancelled
    reason TEXT,                    -- why it errored or was cancelled
    requested_by TEXT NOT NULL,     -- a login, or 'task:<number>'
    task_id TEXT,                   -- the task whose agent asked (`yulius wait` returns on its end)
    deploy TEXT,                    -- audit copies of the commands run
    verify TEXT,
    note TEXT,                      -- what happened first (the promotion)
    output TEXT NOT NULL DEFAULT '', -- bounded tail, agent secrets redacted
    exit_code INTEGER,
    started_at INTEGER NOT NULL,
    ended_at INTEGER,
    reported INTEGER NOT NULL DEFAULT 0 -- `yulius wait` told the task's agent it ended
);
CREATE INDEX cd_runs_sandbox ON cd_runs(sandbox_id, started_at);
-- One running run per environment of a sandbox: they share its checkout.
CREATE UNIQUE INDEX one_running_cd_run ON cd_runs(sandbox_id, environment) WHERE state = 'running';

-- Whether production CD runs get an agent secret (SPEC-134, review H1): a person's allowance of
-- its own, apart from `env_files` (which any agent of the project could then copy into a .env).
ALTER TABLE vault_secrets ADD COLUMN cd_prod INTEGER NOT NULL DEFAULT 0;

-- What a task's agent asked a person to do (SPEC-134 CDT-004, `needs_you`): JSON
-- {"action", "why", "link", "at"}, cleared when a person writes to the task or the agent resolves it.
ALTER TABLE tasks ADD COLUMN needs_you TEXT;
