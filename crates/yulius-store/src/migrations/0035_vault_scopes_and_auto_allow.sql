-- Yulius schema version 35 (SPEC-216): agents set an agent secret up themselves, and secrets
-- belong to a sandbox unless team-wide.
-- Whether agents bind the hosts they ask for at once and production CD runs get the value without
-- a person's allowance. New secrets get it unless their creator turns it off; existing ones keep
-- the person's confirmation.
ALTER TABLE vault_secrets ADD COLUMN auto_allow INTEGER NOT NULL DEFAULT 0;

-- The team secrets when this migration ran (all of them applied to every sandbox): Yulius copies
-- each into every sandbox once (re-sealed under the copy's id, which a migration cannot do) and
-- removes it with its row here. Team-wide secrets created later are never in it, nor agent
-- secrets an administrator made team-wide: copies would let every sandbox's writers reveal and
-- rebind them.
CREATE TABLE vault_scope_pending (
    id TEXT PRIMARY KEY
);
INSERT INTO vault_scope_pending (id)
    SELECT id FROM vault_secrets WHERE section = 'team' AND sandbox_id IS NULL;
