-- Sessions as read-only documents (SPEC-001 SES-001..006, F 2026-10-10): task sessions and
-- Claude Code sessions people upload with `yulius onboard`, with one full-text index (FTS5).
CREATE TABLE session_docs (
    id TEXT PRIMARY KEY,
    sandbox_id TEXT NOT NULL REFERENCES sandboxes(id),
    kind TEXT NOT NULL,                        -- task | imported
    task_id TEXT REFERENCES tasks(id) ON DELETE CASCADE,          -- task sessions
    harness TEXT NOT NULL,
    session_id TEXT NOT NULL,
    title TEXT NOT NULL,                       -- imported: first prompt; task: live task title
    author_id TEXT REFERENCES users(id),       -- uploader (imported) or task creator
    git_branch TEXT,
    cwd_relative TEXT,
    base TEXT,                                 -- imported: revision a continuation starts from
    transcript_path TEXT NOT NULL,
    sha256 TEXT,                               -- imported: digest of the uploaded transcript
    bytes INTEGER NOT NULL DEFAULT 0,
    redactions INTEGER NOT NULL DEFAULT 0,
    started_at INTEGER,
    ended_at INTEGER,
    continued_task_id TEXT REFERENCES tasks(id) ON DELETE SET NULL,
    indexed_bytes INTEGER NOT NULL DEFAULT 0,  -- transcript prefix already in the index
    indexed_lines INTEGER NOT NULL DEFAULT 0,
    index_full INTEGER NOT NULL DEFAULT 0,     -- the per-session index bound was reached
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);
CREATE INDEX session_docs_sandbox ON session_docs(sandbox_id, kind);
CREATE UNIQUE INDEX session_docs_imported ON session_docs(sandbox_id, harness, session_id)
    WHERE kind = 'imported';
CREATE UNIQUE INDEX session_docs_task ON session_docs(task_id, harness, session_id)
    WHERE kind = 'task';

-- Indexed passages (one per transcript line, text bounded); FTS5 external content.
CREATE TABLE session_chunks (
    id INTEGER PRIMARY KEY,
    doc_id TEXT NOT NULL REFERENCES session_docs(id) ON DELETE CASCADE,
    line INTEGER NOT NULL,
    role TEXT NOT NULL,
    text TEXT NOT NULL
);
CREATE INDEX session_chunks_doc ON session_chunks(doc_id, line);
CREATE VIRTUAL TABLE session_fts USING fts5(
    text,
    content = 'session_chunks',
    content_rowid = 'id',
    tokenize = 'unicode61 remove_diacritics 2'
);
CREATE TRIGGER session_chunks_insert AFTER INSERT ON session_chunks BEGIN
    INSERT INTO session_fts(rowid, text) VALUES (new.id, new.text);
END;
CREATE TRIGGER session_chunks_delete AFTER DELETE ON session_chunks BEGIN
    INSERT INTO session_fts(session_fts, rowid, text) VALUES ('delete', old.id, old.text);
END;
