-- Yulius migration 0064, schema version 63 (SPEC-497 OFC-001, OFC-003): a CI job that ended for
-- an infrastructure reason runs again once by itself. `infra` is that reason (`address_pool`,
-- `docker_socket`, `out_of_memory`, `host_lost`), and `infra_ms` the time of the run it threw
-- away (the whole job's when it ended for one again), counted as CI time outside the change.
ALTER TABLE ci_jobs ADD COLUMN infra TEXT;
ALTER TABLE ci_jobs ADD COLUMN infra_ms INTEGER;
