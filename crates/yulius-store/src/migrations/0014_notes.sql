-- Auto-saved notes (SPEC-041): one notepad per scope, `team` or `sandbox:<sid>`. Every stored edit
-- bumps `revision`; a save based on another revision is refused (compare-and-set).
CREATE TABLE notes (
    scope      TEXT PRIMARY KEY,
    body       TEXT NOT NULL,
    revision   INTEGER NOT NULL,
    updated_by TEXT NOT NULL,
    updated_at INTEGER NOT NULL
);
