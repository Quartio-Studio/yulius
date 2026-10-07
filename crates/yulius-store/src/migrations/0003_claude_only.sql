-- Yulius schema version 3 (SPEC-001 F 2026-10-08): Yulius runs Claude Code only, so tasks never
-- change harness — the harness proposals and the instance switch of version 2 are gone.
ALTER TABLE tasks DROP COLUMN harness_proposal;
ALTER TABLE instance DROP COLUMN auto_harness_switch;
