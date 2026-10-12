-- Start rules (SPEC-017 RUL-001..007): a chained task may wait for a logical sentence over the
-- state of other tasks of its sandbox instead of one predecessor. `after_task_id` stays the
-- first task the rule names; NULL `rule` means "after_task_id is Done".
ALTER TABLE task_chains ADD COLUMN rule TEXT;
