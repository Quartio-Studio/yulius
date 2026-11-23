-- SPEC-628 FRK-004, FRK-008: where a fork came from. The source task's id becomes NULL once it is
-- deleted; its number and title stay. A fork's own deletion removes its row.
CREATE TABLE task_forks (
    task_id TEXT PRIMARY KEY REFERENCES tasks(id),
    source_task_id TEXT REFERENCES tasks(id),
    source_number INTEGER NOT NULL,
    source_title TEXT NOT NULL,
    source_branch TEXT NOT NULL,
    source_commit TEXT NOT NULL,
    source_session_id TEXT NOT NULL,
    message TEXT,
    transcript_ref TEXT,
    created_at INTEGER NOT NULL
);
CREATE INDEX task_forks_source ON task_forks(source_task_id);
