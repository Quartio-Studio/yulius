-- Yulius schema version 16 (SPEC-063, ADR-0047): a device grant records the limit key of the
-- client that started it, so live grants are bounded per client (#169). Grants from before keep
-- an empty key; they expire within 10 minutes and are purged.
ALTER TABLE device_grants ADD COLUMN client_key TEXT NOT NULL DEFAULT '';
