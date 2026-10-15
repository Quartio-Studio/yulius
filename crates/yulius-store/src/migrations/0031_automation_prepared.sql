-- SPEC-155: an automation's tasks can wait prepared until a person starts them ("When I decide",
-- SPEC-105). Set only without the other start columns of migration 0019; old rows keep theirs.
ALTER TABLE automations ADD COLUMN prepared INTEGER NOT NULL DEFAULT 0;
