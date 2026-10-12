-- Yulius schema version 13 (SPEC-040): two-factor guards the vault. Each person has their own
-- authenticator seed, sealed like a secret (the user id as associated data); `confirmed_at` is set
-- by the first correct code, and `last_counter` refuses a code used before.
CREATE TABLE vault_two_factor (
    user_id TEXT PRIMARY KEY REFERENCES users(id) ON DELETE CASCADE,
    kek_id INTEGER NOT NULL,
    wrapped_key BLOB NOT NULL,
    ciphertext BLOB NOT NULL,
    created_at INTEGER NOT NULL,
    confirmed_at INTEGER,
    last_counter INTEGER
);

-- When a browser session last unlocked the vault with a two-factor code; revealing a secret needs
-- one within 10 minutes.
CREATE TABLE vault_unlock (
    session_hash TEXT PRIMARY KEY,
    at INTEGER NOT NULL
);
