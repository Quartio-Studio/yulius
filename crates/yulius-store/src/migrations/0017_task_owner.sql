-- Yulius schema version 17 (SPEC-070, ADR-0053): a task is owned by one person, whose GitHub
-- credentials it uses; existing tasks are their creator's. One person at a time may ask the
-- owner for control (`control_requested_by` and `control_requested_at`, both null otherwise).
ALTER TABLE tasks ADD COLUMN owner_id TEXT NOT NULL DEFAULT '';
UPDATE tasks SET owner_id = created_by;
ALTER TABLE tasks ADD COLUMN control_requested_by TEXT;
ALTER TABLE tasks ADD COLUMN control_requested_at INTEGER;
