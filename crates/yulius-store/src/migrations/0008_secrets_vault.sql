-- Yulius schema version 8 (SPEC-001 VLT-001..009, SPEC-002 ADR-18): the secrets vault.
-- Every value lives only in `ciphertext`: a JSON payload sealed with its own random data key
-- (XChaCha20-Poly1305, the row's id and section as associated data), the data key wrapped by the
-- vault key-encryption key derived from the master key (`kek_id` names its generation).
CREATE TABLE vault_secrets (
    id TEXT PRIMARY KEY,
    section TEXT NOT NULL,              -- team | agent
    sandbox_id TEXT REFERENCES sandboxes(id) ON DELETE CASCADE, -- agent: one sandbox, NULL = every sandbox
    name TEXT NOT NULL,                 -- team: title; agent: NAME (environment style)
    description TEXT NOT NULL DEFAULT '', -- agent: what it is for (agents read it)
    hosts TEXT NOT NULL DEFAULT '[]',   -- agent: JSON array of host bindings
    injection TEXT NOT NULL DEFAULT 'none', -- agent: none | bearer | basic | header
    header_name TEXT,                   -- agent: the header of `header` injection
    fields TEXT NOT NULL DEFAULT '[]',  -- JSON array naming the fields present (never values)
    kek_id INTEGER NOT NULL,
    wrapped_key BLOB NOT NULL,
    ciphertext BLOB NOT NULL,
    created_by TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    updated_by TEXT NOT NULL,
    updated_at INTEGER NOT NULL,
    value_updated_at INTEGER NOT NULL,
    last_used_at INTEGER
);
CREATE INDEX vault_secrets_section ON vault_secrets(section, sandbox_id);
CREATE UNIQUE INDEX vault_secrets_agent_name
    ON vault_secrets(COALESCE(sandbox_id, ''), name) WHERE section = 'agent';

-- When a browser session last re-authenticated (password or a new GitHub sign-in counts from the
-- session's creation); revealing team secrets needs one within 10 minutes.
CREATE TABLE vault_reauth (
    session_hash TEXT PRIMARY KEY,
    at INTEGER NOT NULL
);

-- Vault-owned key material, sealed like secrets (the egress proxy's certificate authority).
CREATE TABLE vault_meta (
    name TEXT PRIMARY KEY,
    kek_id INTEGER NOT NULL,
    wrapped_key BLOB NOT NULL,
    ciphertext BLOB NOT NULL,
    created_at INTEGER NOT NULL
);
