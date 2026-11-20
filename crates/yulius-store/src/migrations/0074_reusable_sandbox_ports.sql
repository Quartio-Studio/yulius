-- Keep the historical port range. Only a range whose listeners have retired may be reassigned.
CREATE TABLE sandboxes_new (
    id TEXT PRIMARY KEY,
    slug TEXT NOT NULL UNIQUE,
    repo_owner TEXT NOT NULL,
    repo_name TEXT NOT NULL,
    github_repo_id INTEGER,
    default_branch TEXT NOT NULL,
    profile TEXT NOT NULL,          -- JSON delivery profile
    port_base INTEGER NOT NULL,
    port_count INTEGER NOT NULL,
    onboarding_state TEXT NOT NULL,
    onboarding_task_id TEXT,
    created_by TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    removed_at INTEGER,
    ports_released INTEGER NOT NULL DEFAULT 0 CHECK (ports_released = 0 OR removed_at IS NOT NULL)
);
INSERT INTO sandboxes_new (id, slug, repo_owner, repo_name, github_repo_id, default_branch,
    profile, port_base, port_count, onboarding_state, onboarding_task_id, created_by, created_at, removed_at)
SELECT id, slug, repo_owner, repo_name, github_repo_id, default_branch,
    profile, port_base, port_count, onboarding_state, onboarding_task_id, created_by, created_at, removed_at
FROM sandboxes;

DROP TABLE sandboxes;
ALTER TABLE sandboxes_new RENAME TO sandboxes;
CREATE UNIQUE INDEX sandboxes_repo ON sandboxes(repo_owner COLLATE NOCASE, repo_name COLLATE NOCASE)
    WHERE removed_at IS NULL;
CREATE UNIQUE INDEX sandboxes_reserved_port ON sandboxes(port_base) WHERE ports_released = 0;
