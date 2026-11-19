-- Yulius migration 0070, schema version 70 (SPEC-512 DIG-001, DIG-005, ADR-0380): the end-of-day
-- digest. The instance's setting as one JSON document (on or off, the time in UTC, the idle
-- threshold; NULL: off), the run's claim (`digest_ran_at`, also written when the setting is
-- saved), when a run was last claimed (`digest_sent_at`, only by a claim: one digest a UTC day)
-- and where the last digest's listing ended (`digest_listed_at`, the next one lists what
-- was parked since); each person's digests, the last few kept, which the browser reads; and
-- whether a linked Telegram chat gets them.
ALTER TABLE instance ADD COLUMN digest TEXT;
ALTER TABLE instance ADD COLUMN digest_ran_at INTEGER;
ALTER TABLE instance ADD COLUMN digest_sent_at INTEGER;
ALTER TABLE instance ADD COLUMN digest_listed_at INTEGER;
CREATE TABLE digests (
    id TEXT PRIMARY KEY,
    user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    created_at INTEGER NOT NULL,
    body TEXT NOT NULL
);
CREATE INDEX digests_by_user ON digests(user_id, created_at);
ALTER TABLE telegram_links ADD COLUMN notify_digest INTEGER NOT NULL DEFAULT 1;
