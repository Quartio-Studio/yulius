-- Yulius migration 0091 (SPEC-624, ADR-0462): notices for people, each with a key and an expiry.
-- A notice belongs to a project (`sandbox_id`, NULL: instance-wide) and a recipient (`user_id`,
-- NULL: everyone the project's visibility lets see it, or instance administrators with
-- `admin_only`). One notice per scope and key: a newer one replaces it (newest wins).
-- `expires_at` NULL never expires; a decision (`decision = 1`) never has one.
CREATE TABLE notices (
    id TEXT PRIMARY KEY,
    sandbox_id TEXT REFERENCES sandboxes(id) ON DELETE CASCADE,
    user_id TEXT REFERENCES users(id) ON DELETE CASCADE,
    admin_only INTEGER NOT NULL DEFAULT 0,
    notice_key TEXT NOT NULL,
    category TEXT NOT NULL,
    tone TEXT NOT NULL,
    title TEXT NOT NULL,
    detail TEXT,
    link TEXT,
    decision INTEGER NOT NULL DEFAULT 0,
    created_at INTEGER NOT NULL,
    expires_at INTEGER
);
CREATE UNIQUE INDEX notices_scope_key
    ON notices(COALESCE(sandbox_id, ''), COALESCE(user_id, ''), notice_key);
CREATE INDEX notices_expiry ON notices(expires_at) WHERE expires_at IS NOT NULL;

-- A person who dismissed a notice no longer sees it; a replacement is a new notice.
CREATE TABLE notice_dismissals (
    notice_id TEXT NOT NULL REFERENCES notices(id) ON DELETE CASCADE,
    user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    dismissed_at INTEGER NOT NULL,
    PRIMARY KEY (notice_id, user_id)
);

-- The bot's last message per chat and notice scope and key, so the next one replaces it.
CREATE TABLE telegram_keyed (
    chat_id TEXT NOT NULL,
    scope TEXT NOT NULL,
    message_id TEXT NOT NULL,
    sent_at INTEGER NOT NULL,
    PRIMARY KEY (chat_id, scope)
);
