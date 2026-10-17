-- Yulius schema version 36 (SPEC-217 VSI-002, VSI-004): the Vidoc findings outside a change that
-- Yulius CI filed as GitHub issues, one row per sandbox and fingerprint. A row is claimed before
-- the issue is opened (`issue_number` set once it is) and deleted when opening fails, so a finding
-- is filed once; the row of an issue GitHub no longer has is deleted too, to be filed anew.
-- `file` and `anchored` (the fingerprint names a line's text) let Yulius tell when
-- the code is gone from the integration branch: it closes the issue then and sets `resolved_at`;
-- a finding reported again reopens it and clears `resolved_at`.
CREATE TABLE security_findings (
    sandbox_id TEXT NOT NULL,
    fingerprint TEXT NOT NULL,
    file TEXT NOT NULL,
    anchored INTEGER NOT NULL,
    issue_number INTEGER,
    claimed_at INTEGER NOT NULL,
    resolved_at INTEGER,
    PRIMARY KEY (sandbox_id, fingerprint)
);
