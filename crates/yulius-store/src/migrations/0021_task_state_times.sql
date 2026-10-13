-- Yulius schema version 21 (SPEC-087): since when a task is in its current state, as a person
-- reads it. `state_at` is the last change of its status or activity (a change of anything else,
-- or setting the same value again, leaves it); `updated_at` stays the last change of anything.
-- `working_since` is the start of its current working stretch: the task has not waited for a
-- person since (null while it waits for one, is queued or has ended). Existing tasks take their
-- last change, and a live task its first run's start: the best known start of its stretch.
ALTER TABLE tasks ADD COLUMN state_at INTEGER NOT NULL DEFAULT 0;
UPDATE tasks SET state_at = updated_at;
ALTER TABLE tasks ADD COLUMN working_since INTEGER;
UPDATE tasks SET working_since = COALESCE(
        (SELECT MIN(a.started_at) FROM attempts a WHERE a.task_id = tasks.id), created_at)
    WHERE status IN ('starting', 'running', 'switching', 'waiting_capacity')
      AND (activity IS NULL OR activity != 'needs_attention');
