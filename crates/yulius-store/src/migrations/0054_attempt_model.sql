-- Yulius migration 0054 (SPEC-482, ADR-0312): the model a run was launched with. A task of a
-- Fable model is launched with its fallback (Opus 5.5) while no subscription can run Fable, so
-- the task's own setting no longer says what a run runs: a usage limit binds the model of the
-- run it was reported to, and a run stays on its subscription by that model. NULL for a run
-- recorded before this version, read as the task's own model (schema version 54).
ALTER TABLE attempts ADD COLUMN model TEXT;
