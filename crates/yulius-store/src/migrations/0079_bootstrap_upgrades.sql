-- The Infra bootstrap revision each sandbox was brought to (SPEC-575 MFS-012): a release that
-- raises it creates one upgrade task per sandbox onboarded before it, once, also across
-- restarts. `state`: `laid` (created or onboarded at that revision: nothing to upgrade),
-- `claimed` (its upgrade task is being made: every start finishes it), `linked` (its task was
-- made; deleting that task leaves it linked, never made again).
CREATE TABLE bootstrap_upgrades (
    sandbox_id TEXT PRIMARY KEY REFERENCES sandboxes(id) ON DELETE CASCADE,
    revision INTEGER NOT NULL,
    state TEXT NOT NULL CHECK (state IN ('laid', 'claimed', 'linked')),
    task_id TEXT REFERENCES tasks(id) ON DELETE SET NULL,
    updated_at INTEGER NOT NULL
);
