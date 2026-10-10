-- Yulius schema version with chained tasks and held prompts (SPEC-001 F 2026-10-10, TSK-012,
-- TSK-013; SPEC-002 ADR-16). Tasks waiting in a chain have status 'chained'.
CREATE TABLE task_chains (
    task_id TEXT PRIMARY KEY REFERENCES tasks(id),
    after_task_id TEXT REFERENCES tasks(id),   -- NULL once the predecessor was deleted
    after_number INTEGER NOT NULL,
    blocked_reason TEXT,                       -- set when the task needs a person
    created_at INTEGER NOT NULL
);
CREATE INDEX task_chains_after ON task_chains(after_task_id);

CREATE TABLE prompt_holds (
    id TEXT PRIMARY KEY,
    task_id TEXT NOT NULL REFERENCES tasks(id),
    class TEXT NOT NULL,                       -- new_scope, drops_scope, redirect
    prompt TEXT NOT NULL,
    prompt_sha256 TEXT NOT NULL,
    percent INTEGER NOT NULL,
    typed_by TEXT,
    state TEXT NOT NULL,                       -- pending, sent, consumed, parallel, after, ...
    created_at INTEGER NOT NULL,
    resolved_by TEXT,
    resolved_at INTEGER,
    new_task_id TEXT
);
CREATE INDEX prompt_holds_task ON prompt_holds(task_id, state);
