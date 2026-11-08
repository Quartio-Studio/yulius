-- Yulius schema version 49 (SPEC-440, ADR-0284): a copy of a production secret an agent asked
-- for, as a new agent secret of its project (`name`, bound to `hosts`), waiting for a person who
-- manages both to allow or dismiss it. One per secret and project; nothing here holds a value.
CREATE TABLE vault_copy_requests (
    secret_id TEXT NOT NULL REFERENCES vault_secrets(id) ON DELETE CASCADE,
    sandbox_id TEXT NOT NULL REFERENCES sandboxes(id) ON DELETE CASCADE,
    name TEXT NOT NULL,
    hosts TEXT NOT NULL, -- JSON array
    requested_by TEXT NOT NULL, -- the askers, e.g. '#3, #7'
    reason TEXT NOT NULL DEFAULT '',
    asked_at INTEGER NOT NULL,
    PRIMARY KEY (secret_id, sandbox_id)
);
CREATE INDEX vault_copy_requests_sandbox ON vault_copy_requests(sandbox_id);
