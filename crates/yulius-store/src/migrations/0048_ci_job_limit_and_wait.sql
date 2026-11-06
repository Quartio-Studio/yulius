-- Yulius schema version 48 (SPEC-435): a CI job's own time limit (`timeout_minutes` of its
-- `[[ci.jobs]]` entry; NULL: the instance's), whether one run of it runs at a time in the sandbox
-- (`exclusive`), and how long it waited before it started (NULL until it started).
ALTER TABLE ci_jobs ADD COLUMN timeout_minutes INTEGER;
ALTER TABLE ci_jobs ADD COLUMN exclusive INTEGER NOT NULL DEFAULT 0;
ALTER TABLE ci_jobs ADD COLUMN waited_ms INTEGER;
