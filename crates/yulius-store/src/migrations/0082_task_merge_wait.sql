-- SPEC-558: a watcher needs the last decision for its task across a restart.
CREATE INDEX spans_task_merge_wait ON spans(subject_id, started_at DESC, id DESC)
    WHERE subject_kind = 'task' AND kind = 'wait' AND name = 'merge_armed';
