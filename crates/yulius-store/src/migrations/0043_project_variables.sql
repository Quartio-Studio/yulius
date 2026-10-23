-- Yulius schema version 43 (SPEC-275, ADR-0203): project variables, a project's non-secret values
-- (regions, environment names, ids, base URLs), kept in plain and put into its task runs'
-- environment.
CREATE TABLE project_variables (
    sandbox_id TEXT NOT NULL REFERENCES sandboxes(id) ON DELETE CASCADE,
    name TEXT NOT NULL,
    value TEXT NOT NULL,
    description TEXT NOT NULL DEFAULT '',
    created_by TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    updated_by TEXT NOT NULL,
    updated_at INTEGER NOT NULL,
    PRIMARY KEY (sandbox_id, name)
);
