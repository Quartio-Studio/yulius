-- Yulius schema version 11 (SPEC-001 STP-006..008, F 2026-10-10; SPEC-002 ADR-19): steps of a
-- plan declare their effort (1 = a normal step, up to 3 for a step that cannot be split) so
-- progress is weighted by effort. When a step started and finished is read from step_events,
-- which gets an index for that per-task read.
ALTER TABLE steps ADD COLUMN effort INTEGER NOT NULL DEFAULT 1;
CREATE INDEX step_events_task ON step_events(task_id, step_id);
