-- Parallel CI runs per sandbox (SPEC-021 CI-106): each lane has its own job checkouts, and the
-- dispatcher bounds a sandbox's executing runs (`--ci-lanes`), so the index that allowed one
-- running run per sandbox goes.
DROP INDEX IF EXISTS one_running_ci_run;
