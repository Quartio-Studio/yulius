-- The instance Vidoc switch (SPEC-356 VIO-001): off while `vidoc_off_at` is set, by whom.
ALTER TABLE instance ADD COLUMN vidoc_off_at INTEGER;
ALTER TABLE instance ADD COLUMN vidoc_off_by TEXT;
