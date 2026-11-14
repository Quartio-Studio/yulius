-- Yulius migration 0057 (SPEC-483 QA1): how a task that waits in the queue is to start. A start
-- that finds every session slot taken queues the task; what it was to do (resume past a usage
-- limit, type a held prompt, run on the subscription a person chose) was only in the arguments
-- of that call, and the queue started every task as a plain resume on the pool's choice. The
-- start is kept here as JSON (`kind`, `prompt`, `account`) with `at`, the task's `queued_at`
-- when it was written: it is read only while the task is still queued since then, so no other
-- statement has to clear it. NULL for a task queued without one (schema version 57).
ALTER TABLE tasks ADD COLUMN queued_start TEXT;
