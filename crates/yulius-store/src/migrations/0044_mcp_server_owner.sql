-- Yulius schema version 44 (SPEC-332, ADR-0224): an MCP server is for the team (no owner) or one
-- person (`owner_id`), whose own runs alone get it; removed with the user's row. Names are unique
-- per sandbox (NULL: every sandbox) and owner.
ALTER TABLE mcp_servers ADD COLUMN owner_id TEXT REFERENCES users(id) ON DELETE CASCADE;
CREATE INDEX mcp_servers_owner ON mcp_servers(owner_id) WHERE owner_id IS NOT NULL;
