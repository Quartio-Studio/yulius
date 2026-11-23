-- SPEC-627 MPR-001: an agent secret's method and path rules, a JSON array of {methods, path};
-- '[]' (every existing secret) is no rules: any request to its bound hosts, as before.
ALTER TABLE vault_secrets ADD COLUMN rules TEXT NOT NULL DEFAULT '[]';
