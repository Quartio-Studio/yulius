-- Yulius schema version 32 (SPEC-171, ADR-0139): a request to delete a removed playground's code
-- for good. Its `origin` and sandbox directory are deleted once no process of the sandbox runs;
-- `purged_at` is set then. The sandbox row, its tasks and the audit trail stay.
CREATE TABLE playground_purges (
    sandbox_id TEXT PRIMARY KEY,
    requested_by TEXT NOT NULL,
    requested_at INTEGER NOT NULL,
    purged_at INTEGER
);
