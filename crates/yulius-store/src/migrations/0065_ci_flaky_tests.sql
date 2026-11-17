-- Yulius migration 0065, schema version 65 (SPEC-497 OFC-002): flaky tests. Each failing test a
-- CI job's blocks named, with the run's reference and commit, kept 7 days (forgotten when its
-- quarantine ends); and the tests quarantined as
-- flaky, each with the issue filed for it, until that issue is closed.
CREATE TABLE IF NOT EXISTS ci_test_failures (
    sandbox_id TEXT NOT NULL,
    job_key TEXT NOT NULL,
    test TEXT NOT NULL,
    reference TEXT NOT NULL,
    run_id TEXT NOT NULL,
    sha TEXT NOT NULL,
    at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS ci_test_failures_test ON ci_test_failures (sandbox_id, job_key, test, at);
CREATE INDEX IF NOT EXISTS ci_test_failures_at ON ci_test_failures (at);
CREATE TABLE IF NOT EXISTS ci_quarantine (
    sandbox_id TEXT NOT NULL,
    test TEXT NOT NULL,
    job_key TEXT NOT NULL,
    issue_number INTEGER,
    issue_url TEXT,
    since INTEGER NOT NULL,
    PRIMARY KEY (sandbox_id, test)
);
