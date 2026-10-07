-- Yulius schema version 1 (SPEC-002 §5). Times are integer milliseconds since the Unix epoch.
CREATE TABLE instance (
    singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
    owner_login TEXT,
    owner_kind TEXT,
    public_url TEXT NOT NULL,
    github_app BLOB,               -- sealed AppCredentials JSON
    installation_id INTEGER,
    setup_token_hash TEXT,
    setup_completed_at INTEGER
);

CREATE TABLE users (
    id TEXT PRIMARY KEY,
    github_login TEXT UNIQUE COLLATE NOCASE,
    github_id INTEGER UNIQUE,
    display_name TEXT NOT NULL,
    avatar_url TEXT,
    is_superadmin INTEGER NOT NULL DEFAULT 0,
    is_instance_admin INTEGER NOT NULL DEFAULT 0,
    password_hash TEXT,
    created_at INTEGER NOT NULL,
    disabled_at INTEGER
);
CREATE UNIQUE INDEX one_superadmin ON users(is_superadmin) WHERE is_superadmin = 1;

CREATE TABLE github_tokens (
    user_id TEXT PRIMARY KEY REFERENCES users(id) ON DELETE CASCADE,
    access BLOB NOT NULL,
    refresh BLOB,
    expires_at INTEGER,
    refresh_expires_at INTEGER,
    updated_at INTEGER NOT NULL
);

CREATE TABLE web_sessions (
    id_hash TEXT PRIMARY KEY,
    user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    csrf_token TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    expires_at INTEGER NOT NULL,
    revoked_at INTEGER
);
CREATE INDEX web_sessions_user ON web_sessions(user_id);

CREATE TABLE api_tokens (
    id TEXT PRIMARY KEY,
    user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    token_hash TEXT NOT NULL UNIQUE,
    label TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    last_used_at INTEGER,
    revoked_at INTEGER
);

CREATE TABLE device_grants (
    user_code TEXT PRIMARY KEY,
    device_code_hash TEXT NOT NULL UNIQUE,
    status TEXT NOT NULL,           -- pending | approved | denied | consumed
    user_id TEXT REFERENCES users(id) ON DELETE CASCADE,
    token_plain BLOB,               -- sealed token, deleted when consumed
    created_at INTEGER NOT NULL,
    expires_at INTEGER NOT NULL
);

CREATE TABLE oauth_states (
    state_hash TEXT PRIMARY KEY,
    next_path TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    expires_at INTEGER NOT NULL
);

CREATE TABLE permission_cache (
    user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    sandbox_id TEXT NOT NULL REFERENCES sandboxes(id) ON DELETE CASCADE,
    level TEXT NOT NULL,
    checked_at INTEGER NOT NULL,
    PRIMARY KEY (user_id, sandbox_id)
);

CREATE TABLE sandboxes (
    id TEXT PRIMARY KEY,
    slug TEXT NOT NULL UNIQUE,
    repo_owner TEXT NOT NULL,
    repo_name TEXT NOT NULL,
    github_repo_id INTEGER,
    default_branch TEXT NOT NULL,
    profile TEXT NOT NULL,          -- JSON delivery profile
    port_base INTEGER NOT NULL UNIQUE,
    port_count INTEGER NOT NULL,
    onboarding_state TEXT NOT NULL,
    onboarding_task_id TEXT,
    created_by TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    removed_at INTEGER
);
CREATE UNIQUE INDEX sandboxes_repo ON sandboxes(repo_owner COLLATE NOCASE, repo_name COLLATE NOCASE)
    WHERE removed_at IS NULL;

CREATE TABLE tasks (
    id TEXT PRIMARY KEY,
    sandbox_id TEXT NOT NULL REFERENCES sandboxes(id),
    number INTEGER NOT NULL,
    title TEXT NOT NULL,
    prompt TEXT NOT NULL,
    harness TEXT NOT NULL,
    model TEXT,
    effort TEXT,
    workflow_id TEXT,
    status TEXT NOT NULL,
    activity TEXT,
    branch TEXT,
    worktree_path TEXT,
    spec_path TEXT,
    origin TEXT NOT NULL,
    automation_run TEXT,
    created_by TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    closed_at INTEGER,
    archived_at INTEGER,
    queued_at INTEGER,
    gate_blocks INTEGER NOT NULL DEFAULT 0,
    wake_enabled INTEGER NOT NULL DEFAULT 1,
    UNIQUE (sandbox_id, number)
);
CREATE INDEX tasks_sandbox ON tasks(sandbox_id, archived_at);
CREATE INDEX tasks_status ON tasks(status);
-- One task per automation run key (SPEC-002 §6.4, AUT-005).
CREATE UNIQUE INDEX tasks_automation_run ON tasks(automation_run) WHERE automation_run IS NOT NULL;

CREATE TABLE attempts (
    id TEXT PRIMARY KEY,
    task_id TEXT NOT NULL REFERENCES tasks(id),
    seq INTEGER NOT NULL,
    harness TEXT NOT NULL,
    account_id TEXT,
    provider_session_id TEXT,
    launch_kind TEXT NOT NULL,
    state TEXT NOT NULL,
    run_dir TEXT NOT NULL,
    token_hash TEXT NOT NULL UNIQUE,      -- SHA-256 of the agent's MCP/hook token
    host_token BLOB NOT NULL,             -- sealed server<->ptyhost token (restart reattach)
    ptyhost_pid INTEGER,
    child_pid INTEGER,
    started_at INTEGER NOT NULL,
    ended_at INTEGER,
    exit_code INTEGER,
    exit_reason TEXT,
    UNIQUE (task_id, seq)
);
CREATE UNIQUE INDEX one_live_attempt ON attempts(task_id) WHERE state IN ('spawning', 'live');

CREATE TABLE provider_sessions (
    id TEXT PRIMARY KEY,
    sandbox_id TEXT NOT NULL REFERENCES sandboxes(id),
    task_id TEXT NOT NULL REFERENCES tasks(id),
    harness TEXT NOT NULL,
    session_id TEXT NOT NULL,
    transcript_ref TEXT,
    origin TEXT NOT NULL,           -- native | imported | translated
    first_seen INTEGER NOT NULL,
    last_seen INTEGER NOT NULL,
    UNIQUE (sandbox_id, harness, session_id)
);

CREATE TABLE accounts (
    id TEXT PRIMARY KEY,
    provider TEXT NOT NULL,
    label TEXT NOT NULL,
    credential BLOB,                -- sealed token or auth.json
    priority INTEGER NOT NULL DEFAULT 100,
    enabled INTEGER NOT NULL DEFAULT 1,
    status TEXT NOT NULL,
    usage TEXT,                     -- JSON AccountUsage snapshot
    last_error TEXT,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);

CREATE TABLE switch_decisions (
    id TEXT PRIMARY KEY,
    task_id TEXT NOT NULL REFERENCES tasks(id),
    at INTEGER NOT NULL,
    reason TEXT NOT NULL,
    inputs TEXT NOT NULL,
    from_desc TEXT NOT NULL,
    to_desc TEXT NOT NULL,
    outcome TEXT NOT NULL
);

CREATE TABLE steps (
    id TEXT PRIMARY KEY,
    task_id TEXT NOT NULL REFERENCES tasks(id),
    ordinal INTEGER NOT NULL,
    title TEXT NOT NULL,
    status TEXT NOT NULL,
    note TEXT,
    updated_by TEXT,
    updated_at INTEGER
);
CREATE INDEX steps_task ON steps(task_id, ordinal);

CREATE TABLE step_events (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    step_id TEXT NOT NULL,
    task_id TEXT NOT NULL,
    at INTEGER NOT NULL,
    actor TEXT NOT NULL,
    from_status TEXT,
    to_status TEXT NOT NULL,
    note TEXT
);

CREATE TABLE messages (
    id TEXT PRIMARY KEY,
    sandbox_id TEXT NOT NULL REFERENCES sandboxes(id),
    from_task TEXT,
    from_user TEXT,
    to_task TEXT,
    kind TEXT NOT NULL,
    subject TEXT NOT NULL,
    body TEXT NOT NULL,
    created_at INTEGER NOT NULL
);
CREATE INDEX messages_sandbox ON messages(sandbox_id, created_at);

CREATE TABLE message_receipts (
    message_id TEXT NOT NULL REFERENCES messages(id),
    task_id TEXT NOT NULL,
    delivered_at INTEGER,
    read_at INTEGER,
    PRIMARY KEY (message_id, task_id)
);

CREATE TABLE scope_claims (
    id TEXT PRIMARY KEY,
    task_id TEXT NOT NULL REFERENCES tasks(id),
    sandbox_id TEXT NOT NULL,
    pattern TEXT NOT NULL,
    note TEXT,
    created_at INTEGER NOT NULL,
    released_at INTEGER
);

CREATE TABLE workflows (
    id TEXT PRIMARY KEY,
    sandbox_id TEXT,
    name TEXT NOT NULL,
    description TEXT NOT NULL,
    steps TEXT NOT NULL,            -- JSON array of titles
    instructions TEXT NOT NULL,
    gates TEXT NOT NULL,            -- JSON
    created_at INTEGER NOT NULL
);

CREATE TABLE automations (
    id TEXT PRIMARY KEY,
    sandbox_id TEXT NOT NULL REFERENCES sandboxes(id),
    name TEXT NOT NULL,
    cron TEXT NOT NULL,
    utc_offset_minutes INTEGER NOT NULL,
    prompt TEXT NOT NULL,
    workflow_id TEXT,
    harness TEXT NOT NULL,
    enabled INTEGER NOT NULL,
    created_by TEXT NOT NULL,
    created_via TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    last_scheduled INTEGER
);

CREATE TABLE automation_runs (
    automation_id TEXT NOT NULL REFERENCES automations(id),
    scheduled_for INTEGER NOT NULL,
    state TEXT NOT NULL,
    task_id TEXT,
    error TEXT,
    created_at INTEGER NOT NULL,
    PRIMARY KEY (automation_id, scheduled_for)
);

CREATE TABLE skill_settings (
    sandbox_id TEXT NOT NULL,
    name TEXT NOT NULL,
    enabled INTEGER NOT NULL,
    PRIMARY KEY (sandbox_id, name)
);

CREATE TABLE mcp_servers (
    id TEXT PRIMARY KEY,
    sandbox_id TEXT,                -- NULL = instance scope
    name TEXT NOT NULL,
    transport TEXT NOT NULL,
    command TEXT,
    args TEXT NOT NULL,             -- JSON array
    url TEXT,
    env BLOB,                       -- sealed JSON object
    headers BLOB,                   -- sealed JSON object
    enabled INTEGER NOT NULL,
    created_at INTEGER NOT NULL
);

CREATE TABLE metrics (
    attempt_id TEXT PRIMARY KEY REFERENCES attempts(id),
    task_id TEXT NOT NULL,
    model TEXT,
    input_tokens INTEGER,
    output_tokens INTEGER,
    cache_read INTEGER,
    cache_write INTEGER,
    turns INTEGER,
    active_ms INTEGER,
    updated_at INTEGER NOT NULL
);

CREATE TABLE feedback (
    task_id TEXT NOT NULL REFERENCES tasks(id),
    user_id TEXT NOT NULL,
    verdict TEXT NOT NULL,
    comment TEXT NOT NULL,
    at INTEGER NOT NULL,
    PRIMARY KEY (task_id, user_id)
);

CREATE TABLE delivery (
    task_id TEXT PRIMARY KEY REFERENCES tasks(id),
    stage TEXT NOT NULL,
    pr_number INTEGER,
    updated_at INTEGER NOT NULL
);

CREATE TABLE environment_runs (
    id TEXT PRIMARY KEY,
    sandbox_id TEXT NOT NULL,
    environment TEXT NOT NULL,
    kind TEXT NOT NULL,             -- command | manual (a person's confirmation)
    requested_by TEXT NOT NULL,
    command TEXT,                   -- audit copy of the command that ran
    commit_sha TEXT,                -- commit checked out for the command
    state TEXT NOT NULL,            -- running | succeeded | failed | error
    output TEXT NOT NULL,           -- bounded tail
    exit_code INTEGER,
    started_at INTEGER NOT NULL,
    ended_at INTEGER
);
-- At most one command run per sandbox at a time.
CREATE UNIQUE INDEX one_running_environment_run ON environment_runs(sandbox_id) WHERE state = 'running';
CREATE INDEX environment_runs_sandbox ON environment_runs(sandbox_id, started_at);

CREATE TABLE import_jobs (
    id TEXT PRIMARY KEY,
    sandbox_id TEXT,
    user_id TEXT NOT NULL,
    state TEXT NOT NULL,
    manifest TEXT NOT NULL,
    received TEXT NOT NULL,         -- JSON map file -> bytes received
    result TEXT,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);

CREATE TABLE audit_log (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    at INTEGER NOT NULL,
    actor TEXT NOT NULL,
    action TEXT NOT NULL,
    target TEXT,
    detail TEXT
);
