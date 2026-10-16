-- Yulius schema version 33 (SPEC-191, ADR-0154): a person may pause a task's live run. Who
-- paused it, when, and the attempt it acts on; all null while it is not paused.
ALTER TABLE tasks ADD COLUMN paused_by TEXT;
ALTER TABLE tasks ADD COLUMN paused_at INTEGER;
ALTER TABLE tasks ADD COLUMN paused_attempt TEXT;
