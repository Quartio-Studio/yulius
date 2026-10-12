-- Yulius schema version 14 (SPEC-040): two-factor guards the vault. Each person has their own
-- authenticator seed, sealed like a secret (the user id as associated data); `confirmed_at` is set
-- by the first correct code, `last_counter` refuses a code used before, and `failed_attempts`
-- (wrong codes in a row, the last at `failed_at`) locks codes after 10 until the person confirms
-- it is them again.
CREATE TABLE vault_two_factor (
    user_id TEXT PRIMARY KEY REFERENCES users(id) ON DELETE CASCADE,
    kek_id INTEGER NOT NULL,
    wrapped_key BLOB NOT NULL,
    ciphertext BLOB NOT NULL,
    created_at INTEGER NOT NULL,
    confirmed_at INTEGER,
    last_counter INTEGER,
    failed_attempts INTEGER NOT NULL DEFAULT 0,
    failed_at INTEGER
);

-- When a browser session last unlocked the vault with a two-factor code; revealing a secret needs
-- one within 10 minutes.
CREATE TABLE vault_unlock (
    session_hash TEXT PRIMARY KEY,
    at INTEGER NOT NULL
);
