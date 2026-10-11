-- Yulius schema (SPEC-001 F 2026-10-10 items 1(e) and 2; SPEC-002 ADR-18): instance updates from
-- `main` of Quartio-Studio/yulius, and the Jev API key managed by the superadmin.

-- The Jev key, sealed with the instance master key (XChaCha20-Poly1305) like every other secret.
-- NULL: no key saved here (YULIUS_JEV_API_KEY_FILE is used when it is set).
ALTER TABLE instance ADD COLUMN jev_key BLOB;
ALTER TABLE instance ADD COLUMN jev_key_set_at INTEGER;
ALTER TABLE instance ADD COLUMN jev_key_set_by TEXT;

-- One row per update; at most one runs (finished_at IS NULL) at a time.
CREATE TABLE update_runs (
    id TEXT PRIMARY KEY,
    slot INTEGER NOT NULL DEFAULT 1,  -- constant: the unique index below allows one open run
    target_commit TEXT NOT NULL,
    from_commit TEXT,
    summary TEXT NOT NULL DEFAULT '',
    state TEXT NOT NULL,              -- building | draining | installing | resuming | succeeded | failed | rolled_back | cancelled
    outcome TEXT,                     -- while resuming: the state it ends in
    error TEXT,
    requested_by TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    state_since INTEGER NOT NULL,
    finished_at INTEGER
);
CREATE UNIQUE INDEX update_runs_one_open ON update_runs(slot) WHERE finished_at IS NULL;
CREATE INDEX update_runs_created ON update_runs(created_at);
