-- Yulius schema version 37 (SPEC-218 MOR-001, #654): where an MCP server came from. `person`: a
-- person added it, or changed what it runs; `mcp_json`: Yulius imported it from the repository's
-- `.mcp.json`, which agents can write, so the MCP Test never runs its command on the host. A
-- server the audit log records as imported is marked `mcp_json`, also if a person edited it since
-- (that person changes its command again to take it over).
ALTER TABLE mcp_servers ADD COLUMN origin TEXT NOT NULL DEFAULT 'person';
UPDATE mcp_servers SET origin = 'mcp_json'
    WHERE id IN (SELECT target FROM audit_log WHERE action = 'mcp.imported' AND target IS NOT NULL);
