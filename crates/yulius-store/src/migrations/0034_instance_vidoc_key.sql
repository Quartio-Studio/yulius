-- The instance Vidoc key (SPEC-195 VDK-001): sealed with the master key like the Jev key.
ALTER TABLE instance ADD COLUMN vidoc_key BLOB;
ALTER TABLE instance ADD COLUMN vidoc_key_set_at INTEGER;
ALTER TABLE instance ADD COLUMN vidoc_key_set_by TEXT;
