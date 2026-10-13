-- Yulius schema version 17 (SPEC-071, ADR-0054): a supertask waits for the running work of its
-- project, then runs alone while the project's other tasks wait for it.
ALTER TABLE tasks ADD COLUMN supertask INTEGER NOT NULL DEFAULT 0;
