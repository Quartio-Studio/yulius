-- Yulius schema version 22 (SPEC-082 RES-002): the CPUs and memory a CI job declares
-- (`cpus`, `memory_gb` of its `[[ci.jobs]]` entry, here in MiB). NULL: it declared none and
-- shares the sandbox with its run's other jobs.
ALTER TABLE ci_jobs ADD COLUMN cpus INTEGER;
ALTER TABLE ci_jobs ADD COLUMN memory_mb INTEGER;
