-- SPEC-629 RBL-005: the subscription a person chose for a task ("Run on…", "Continue on…").
-- The rebalance never moves a task running on it, and a parked task resumes on it. NULL: none.
ALTER TABLE tasks ADD COLUMN pinned_account_id TEXT;
