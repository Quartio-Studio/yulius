-- Yulius migration 0058 (SPEC-483 CLT-002): `tasks.closed_at` is the time of the close the task
-- is in. It was set at every close and never cleared: a task continued after a close (or created
-- chained or prepared without a start) kept an old close time, which the analytics export shows
-- as "Closed" and the project list as `closed_at_ms`. From schema version 58 a status change
-- keeps it only while the task stays closed; this clears what earlier versions left.
UPDATE tasks SET closed_at = NULL WHERE status != 'closed' AND closed_at IS NOT NULL;
