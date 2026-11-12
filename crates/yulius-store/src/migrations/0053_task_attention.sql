-- Yulius migration 0053 (SPEC-472, ADR-0306): why a task reads `needs_attention`. The reason of
-- a flag as JSON (`kind`, `problem`, `action`, `link`) with `at`, the time the flag was raised:
-- it is written by the statement that raises the flag and read only while the task still reads
-- that flag (`activity = 'needs_attention'` and `state_at = at`), so no other statement clears
-- it. NULL for a task never flagged and for a flag raised without a reason (schema version 53).
ALTER TABLE tasks ADD COLUMN attention TEXT;
