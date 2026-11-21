-- SPEC-558: history cleanup retains the latest completed merge wait of an existing task.
CREATE INDEX spans_task_merge_wait ON spans(subject_id, started_at DESC, id DESC)
    WHERE subject_kind = 'task' AND kind = 'wait' AND name = 'merge_armed' AND ended_at IS NOT NULL;
