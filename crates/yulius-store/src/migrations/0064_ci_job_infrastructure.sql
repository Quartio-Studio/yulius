-- Yulius migration 0064, schema version 63 (SPEC-497 OFC-001, OFC-003): a CI job that ended for
-- an infrastructure reason runs again once by itself. `infra` is that reason (`address_pool`,
-- `docker_socket`, `out_of_memory`, `host_lost`), `infra_ms` the time of the runs it threw away,
-- and `infra_end` 1 when its end itself was the infrastructure's (it met one again).
ALTER TABLE ci_jobs ADD COLUMN infra TEXT;
ALTER TABLE ci_jobs ADD COLUMN infra_ms INTEGER;
ALTER TABLE ci_jobs ADD COLUMN infra_end INTEGER;
-- CI minutes per sandbox, UTC day of the runs' start and how they ended, added when a run that
-- started ends: runs themselves are pruned to the newest few per sandbox, these stay.
CREATE TABLE IF NOT EXISTS ci_minutes (
    sandbox_id TEXT NOT NULL,
    day TEXT NOT NULL,
    outcome TEXT NOT NULL,
    runs INTEGER NOT NULL DEFAULT 0,
    run_ms INTEGER NOT NULL DEFAULT 0,
    rerun_ms INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (sandbox_id, day, outcome)
);
-- One row a minute: the host's CPUs busy (per mille), the CPUs running CI jobs reserved and
-- measured, and the CI jobs waiting to start (SPEC-497); kept 30 days.
CREATE TABLE IF NOT EXISTS ci_load (
    at INTEGER PRIMARY KEY,
    busy_permille INTEGER,
    held_cpus INTEGER NOT NULL,
    measured_cpus INTEGER,
    waiting_jobs INTEGER NOT NULL
);
