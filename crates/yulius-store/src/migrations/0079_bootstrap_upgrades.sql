-- The Infra bootstrap revision each sandbox was brought to (SPEC-575 MFS-012): a release that
-- raises it creates one upgrade task per onboarded sandbox, once, also across restarts.
CREATE TABLE bootstrap_upgrades (
    sandbox_id TEXT PRIMARY KEY REFERENCES sandboxes(id) ON DELETE CASCADE,
    revision INTEGER NOT NULL,
    task_id TEXT REFERENCES tasks(id) ON DELETE SET NULL,
    updated_at INTEGER NOT NULL
);
