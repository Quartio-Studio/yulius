-- Yulius schema: how "Auto" chose a new task's mode and workflow (SPEC-001 F 2026-10-10, TSK-015;
-- SPEC-002 ADR-16 item 5). One row per task created with its mode or workflow on Auto; tasks
-- created with both picked by a person have none.
CREATE TABLE task_auto_choices (
    task_id TEXT PRIMARY KEY REFERENCES tasks(id),
    mode_by TEXT,          -- jev | default; NULL when the person picked the mode
    workflow_by TEXT,      -- jev | default; NULL when the person picked the workflow
    workflow_name TEXT,    -- the task's workflow when it was created; NULL = none
    created_at INTEGER NOT NULL
);
