-- A playground's guests (SPEC-410, ADR-0260): the expiring links its admins create (only the code's
-- hash is kept) and the guest accounts each link let in.
CREATE TABLE playground_invites (
    id TEXT PRIMARY KEY,
    sandbox_id TEXT NOT NULL REFERENCES sandboxes(id) ON DELETE CASCADE,
    code_hash TEXT NOT NULL UNIQUE,
    created_by TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    created_at INTEGER NOT NULL,
    expires_at INTEGER NOT NULL,
    revoked_at INTEGER,
    -- When what its guests still ran was stopped (at the revoke, or the sweep after expiry).
    ended_at INTEGER
);
CREATE INDEX playground_invites_sandbox ON playground_invites(sandbox_id);

CREATE TABLE playground_guests (
    user_id TEXT PRIMARY KEY REFERENCES users(id) ON DELETE CASCADE,
    invite_id TEXT NOT NULL REFERENCES playground_invites(id) ON DELETE CASCADE,
    joined_at INTEGER NOT NULL
);
CREATE INDEX playground_guests_invite ON playground_guests(invite_id);
