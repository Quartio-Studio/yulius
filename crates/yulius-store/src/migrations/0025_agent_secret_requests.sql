-- Yulius schema version 25 (SPEC-121): hosts an agent proposed for an agent secret, waiting for a
-- person who manages it to allow them (they then join `hosts`) or dismiss them. Nothing here is
-- ever used to send a secret.
ALTER TABLE vault_secrets ADD COLUMN requested_hosts TEXT NOT NULL DEFAULT '[]'; -- JSON array
ALTER TABLE vault_secrets ADD COLUMN requested_by TEXT; -- the task that asked last, e.g. '#107'
