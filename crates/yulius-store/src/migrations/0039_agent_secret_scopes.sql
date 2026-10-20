-- Yulius schema version 38 (SPEC-245, ADR-0186): an agent secret is a project's (`sandbox_id`),
-- one person's (`owner_id`) or global (neither). Global and personal secrets reach a project only
-- while they are switched on for it (`vault_enabled`). Names are unique within a scope.
ALTER TABLE vault_secrets ADD COLUMN owner_id TEXT REFERENCES users(id) ON DELETE CASCADE;

CREATE TABLE vault_enabled (
    secret_id TEXT NOT NULL REFERENCES vault_secrets(id) ON DELETE CASCADE,
    sandbox_id TEXT NOT NULL REFERENCES sandboxes(id) ON DELETE CASCADE,
    enabled_by TEXT NOT NULL,
    enabled_at INTEGER NOT NULL,
    PRIMARY KEY (secret_id, sandbox_id)
);
CREATE INDEX vault_enabled_sandbox ON vault_enabled(sandbox_id);

DROP INDEX vault_secrets_agent_name;
CREATE UNIQUE INDEX vault_secrets_agent_name
    ON vault_secrets(COALESCE(sandbox_id, ''), COALESCE(owner_id, ''), name) WHERE section = 'agent';

-- Every team-wide agent secret applied to every sandbox: as a global secret it stays on for every
-- sandbox there is, so no run loses one it had. Project secrets and team secrets are unchanged.
INSERT INTO vault_enabled (secret_id, sandbox_id, enabled_by, enabled_at)
    SELECT v.id, s.id, 'yulius', v.updated_at
    FROM vault_secrets v CROSS JOIN sandboxes s
    WHERE v.section = 'agent' AND v.sandbox_id IS NULL AND s.removed_at IS NULL;
