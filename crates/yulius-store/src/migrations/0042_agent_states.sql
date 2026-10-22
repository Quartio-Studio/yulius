-- Yulius schema version 42 (SPEC-271, ADR-0200): agent states, a project's folders of operator
-- state moved from a laptop. Each holds one gzip tar sealed like a vault value (its own data key,
-- the row's id and version as associated data). Once restored into the sandbox home, `restored_*`
-- names that copy, which is the one written to.
CREATE TABLE agent_states (
    id TEXT PRIMARY KEY,
    sandbox_id TEXT NOT NULL REFERENCES sandboxes(id) ON DELETE CASCADE,
    name TEXT NOT NULL,
    description TEXT NOT NULL DEFAULT '',
    version INTEGER NOT NULL,
    bytes INTEGER NOT NULL,
    files INTEGER NOT NULL,
    kek_id INTEGER NOT NULL,
    wrapped_key BLOB NOT NULL,
    ciphertext BLOB NOT NULL,
    created_by TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    updated_by TEXT NOT NULL,
    updated_at INTEGER NOT NULL,
    restored_path TEXT,
    restored_version INTEGER,
    restored_by TEXT,
    restored_at INTEGER,
    UNIQUE (sandbox_id, name)
);
