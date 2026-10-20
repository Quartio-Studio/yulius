-- Yulius schema version 40 (SPEC-247, ADR-0188): what an API token may do. `cli` is a token of `yulius login`
-- (device approval) and every token made before: the REST API as before, never the remote MCP.
-- `read` and `operate` are made on purpose in Settings for a person's local agent.
ALTER TABLE api_tokens ADD COLUMN scope TEXT NOT NULL DEFAULT 'cli';
CREATE INDEX api_tokens_user ON api_tokens (user_id, revoked_at);
