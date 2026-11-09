-- Yulius schema version 50 (SPEC-444): the instance's Telegram bot, its token sealed with the
-- master key like the Jev and Vidoc keys, with the bot's id and username from `getMe`.
ALTER TABLE instance ADD COLUMN telegram_token BLOB;
ALTER TABLE instance ADD COLUMN telegram_bot_id INTEGER;
ALTER TABLE instance ADD COLUMN telegram_bot_username TEXT;
ALTER TABLE instance ADD COLUMN telegram_set_at INTEGER;
ALTER TABLE instance ADD COLUMN telegram_set_by TEXT;
-- A person's one pending link code, stored as its SHA-256 (TGC-011).
CREATE TABLE telegram_link_codes (
    user_id TEXT PRIMARY KEY REFERENCES users(id) ON DELETE CASCADE,
    code_hash TEXT NOT NULL UNIQUE,
    expires_at INTEGER NOT NULL
);
-- A person's linked chat and what they want to receive there (TGC-012, TGC-020).
CREATE TABLE telegram_links (
    user_id TEXT PRIMARY KEY REFERENCES users(id) ON DELETE CASCADE,
    chat_id TEXT NOT NULL UNIQUE,
    linked_at INTEGER NOT NULL,
    notify_needs_you INTEGER NOT NULL DEFAULT 1,
    notify_done INTEGER NOT NULL DEFAULT 0,
    notify_failed INTEGER NOT NULL DEFAULT 0,
    notify_account INTEGER NOT NULL DEFAULT 0
);
