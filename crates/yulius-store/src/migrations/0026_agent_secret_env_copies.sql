-- Yulius schema version 26 (SPEC-129): whether agents may copy an agent secret's value into the
-- project's git-ignored .env files. 'no', 'asked' (an agent asked, `env_requested_by` names the
-- tasks; a person allows or dismisses) or 'allowed' (a person allowed it; Revoke returns to 'no').
ALTER TABLE vault_secrets ADD COLUMN env_files TEXT NOT NULL DEFAULT 'no';
ALTER TABLE vault_secrets ADD COLUMN env_requested_by TEXT; -- e.g. '#111'
