-- Yulius migration 0066, schema version 66 (SPEC-505 RND-001): the tasks whose harness was drawn
-- at creation (Claude Code or Antigravity among those with a usable subscription), so a task
-- says, and the analytics export lists, whether its agent was drawn or chosen. A task absent
-- here was chosen (every task before this version was).
CREATE TABLE IF NOT EXISTS task_harness_draws (
    task_id TEXT PRIMARY KEY NOT NULL,
    drawn_at INTEGER NOT NULL
);
