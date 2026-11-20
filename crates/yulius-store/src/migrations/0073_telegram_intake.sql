-- Yulius migration 0073, schema version 73 (SPEC-512 TGI-001, TGI-002, ADR-0392): the personal Telegram channel's
-- intake. Whether a person's linked chat may answer and start tasks (off by default) and the
-- project a new message goes to; and the messages the bot sent about a task, so a reply to one
-- reaches that task (the last 500 per chat are kept).
ALTER TABLE telegram_links ADD COLUMN intake INTEGER NOT NULL DEFAULT 0;
ALTER TABLE telegram_links ADD COLUMN intake_sandbox TEXT;
CREATE TABLE telegram_sent (
    chat_id TEXT NOT NULL,
    message_id TEXT NOT NULL,
    task_id TEXT NOT NULL,
    sent_at INTEGER NOT NULL,
    PRIMARY KEY (chat_id, message_id)
);
CREATE INDEX telegram_sent_by_chat ON telegram_sent(chat_id, sent_at);
