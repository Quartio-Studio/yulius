-- Yulius schema version 19 (SPEC-078): how an automation's tasks start, as New task offers it.
-- At most one of the three is set; none of them: the task starts now (queued).
ALTER TABLE automations ADD COLUMN supertask INTEGER NOT NULL DEFAULT 0;
ALTER TABLE automations ADD COLUMN after_task_id TEXT;
ALTER TABLE automations ADD COLUMN start_rule TEXT;
