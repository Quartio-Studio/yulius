-- Prepared tasks (SPEC-105): a task written now and started later waits with status 'prepared'.
-- One that should start by itself at a time has a row here; one that waits for a person ("when I
-- decide") has none. Rows of tasks that left 'prepared' are kept and ignored.
CREATE TABLE task_schedules (
    task_id TEXT PRIMARY KEY REFERENCES tasks(id),
    start_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);
CREATE INDEX task_schedules_start ON task_schedules(start_at);
