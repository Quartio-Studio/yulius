-- Claude Code in sandboxes (SPEC-461).
-- The release channel a superadmin chose in Settings, Instance (CCM-009): NULL follows the
-- server's configuration (YULIUS_CLAUDE_UPDATES).
ALTER TABLE instance ADD COLUMN claude_channel TEXT;
ALTER TABLE instance ADD COLUMN claude_channel_set_at INTEGER;
ALTER TABLE instance ADD COLUMN claude_channel_set_by TEXT;
-- The Claude Code version a run's transcript lines name (CCM-011), beside the model that answered.
ALTER TABLE metrics ADD COLUMN cli_version TEXT;
