-- Yulius schema version 36 (SPEC-217 VSI-002): the Vidoc findings outside a change that Yulius CI
-- filed as GitHub issues, one row per sandbox and fingerprint. A row is claimed before the issue
-- is opened (`issue_number` set once it is) and deleted when opening fails, so a finding is filed
-- once, also when its issue is closed later.
CREATE TABLE security_findings (
    sandbox_id TEXT NOT NULL,
    fingerprint TEXT NOT NULL,
    issue_number INTEGER,
    claimed_at INTEGER NOT NULL,
    PRIMARY KEY (sandbox_id, fingerprint)
);
