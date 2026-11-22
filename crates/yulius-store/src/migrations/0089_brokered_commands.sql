-- Yulius migration 0089 (SPEC-626 BRK-001, BRK-011, BRK-014, ADR-0459): brokered commands. A
-- project's command definitions (the definition as one JSON document; names unique in a
-- project) and its runs: who asked, the arguments to run and the command line shown (redacted),
-- the state, an approval's deadline and answer, and the redacted output. Nothing here holds a
-- secret's value.
CREATE TABLE commands (
    id TEXT PRIMARY KEY,
    sandbox_id TEXT NOT NULL REFERENCES sandboxes(id) ON DELETE CASCADE,
    name TEXT NOT NULL,
    definition TEXT NOT NULL,
    created_by TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    updated_by TEXT NOT NULL,
    updated_at INTEGER NOT NULL,
    UNIQUE (sandbox_id, name)
);
CREATE TABLE command_runs (
    id TEXT PRIMARY KEY,
    sandbox_id TEXT NOT NULL REFERENCES sandboxes(id) ON DELETE CASCADE,
    command_id TEXT NOT NULL,
    command TEXT NOT NULL,
    command_updated_at INTEGER NOT NULL,
    task_id TEXT NOT NULL,
    task_number INTEGER NOT NULL,
    args TEXT NOT NULL,
    line TEXT NOT NULL,
    changes INTEGER NOT NULL DEFAULT 0,
    state TEXT NOT NULL,
    reason TEXT,
    deadline INTEGER,
    answered_by TEXT,
    exit_code INTEGER,
    stdout TEXT NOT NULL DEFAULT '',
    stderr TEXT NOT NULL DEFAULT '',
    created_at INTEGER NOT NULL,
    started_at INTEGER,
    ended_at INTEGER
);
CREATE INDEX command_runs_by_sandbox ON command_runs(sandbox_id, created_at);
CREATE INDEX command_runs_by_state ON command_runs(state, deadline);
CREATE INDEX command_runs_by_task ON command_runs(task_id, state);
