-- Yulius migration 0076, schema version 74 (SPEC-512 HOF-001..HOF-006, ADR-0395): tasks hand
-- work to another sandbox. The instance's setting as one JSON document (mode off, ask or
-- automatic, the depth and daily bounds; NULL: off) and every handoff: who asked from where, the
-- arguments shown to the owner, its chain (for depth and cycles), its state and the task it made,
-- and whether its outcome was told to the source task.
ALTER TABLE instance ADD COLUMN handoffs TEXT;
CREATE TABLE task_handoffs (
    id TEXT PRIMARY KEY,
    source_task TEXT NOT NULL,
    source_sandbox TEXT NOT NULL,
    target_sandbox TEXT NOT NULL,
    owner TEXT NOT NULL,
    args TEXT NOT NULL,
    chain TEXT NOT NULL,
    depth INTEGER NOT NULL,
    state TEXT NOT NULL,
    reason TEXT,
    target_task TEXT,
    created_at INTEGER NOT NULL,
    decided_at INTEGER,
    outcome_told INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX task_handoffs_by_source ON task_handoffs(source_task, created_at);
CREATE UNIQUE INDEX task_handoffs_by_target ON task_handoffs(target_task) WHERE target_task IS NOT NULL;
CREATE INDEX task_handoffs_by_owner ON task_handoffs(owner, created_at);
