-- Yulius migration 0080 (SPEC-578, ADR-0414): timestamped system and agent Needs You messages
-- recorded in task history, deduplicated per wait episode by `UNIQUE (task_id, episode)`.
CREATE TABLE task_wait_messages (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    task_id TEXT NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
    episode TEXT NOT NULL,
    source TEXT NOT NULL,
    happened TEXT NOT NULL,
    problem TEXT,
    action TEXT NOT NULL,
    link TEXT NOT NULL,
    decision INTEGER NOT NULL DEFAULT 0,
    created_at INTEGER NOT NULL,
    resolved_at INTEGER,
    UNIQUE (task_id, episode)
);
CREATE INDEX task_wait_messages_task ON task_wait_messages(task_id, created_at DESC, id DESC);
