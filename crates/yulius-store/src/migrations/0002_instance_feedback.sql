-- Yulius schema version 2 (SPEC-001 F 2026-10-07): goal mode (TSK-010), harness changes that
-- need approval (ROT-012) and the superadmin recovery code (ACC-011).
ALTER TABLE tasks ADD COLUMN mode TEXT NOT NULL DEFAULT 'prompt';
ALTER TABLE tasks ADD COLUMN harness_proposal TEXT;      -- JSON proposal awaiting approval
ALTER TABLE instance ADD COLUMN recovery_hash TEXT;      -- Argon2id PHC of the 6-digit code
ALTER TABLE instance ADD COLUMN recovery_failures INTEGER NOT NULL DEFAULT 0;
ALTER TABLE instance ADD COLUMN recovery_locked_at INTEGER;
ALTER TABLE instance ADD COLUMN auto_harness_switch INTEGER NOT NULL DEFAULT 0;
