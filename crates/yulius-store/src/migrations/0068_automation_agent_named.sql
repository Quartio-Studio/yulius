-- Yulius migration 0068, schema version 68 (SPEC-514 PIN-003, ADR-0368): an automation runs the
-- agent it names. One saved as `auto` drew Claude Code or Antigravity for each run (SPEC-505
-- RND-002); it now runs Claude Code, the agent of a task created without one, and says so.
UPDATE automations SET harness = 'claude' WHERE harness = 'auto';
