-- Yulius migration 0069, schema version 69 (SPEC-525): Codex and Antigravity sessions are read
-- into transcript entries. Until now the reader knew only Claude Code's lines, so the index went
-- over these sessions without a passage and recorded them as indexed; they are indexed again from
-- their start (no passage of theirs exists to remove, but any is dropped with the index).
DELETE FROM session_chunks
    WHERE doc_id IN (SELECT id FROM session_docs WHERE harness IN ('codex', 'antigravity'));
UPDATE session_docs SET indexed_bytes = 0, indexed_lines = 0, index_full = 0
    WHERE harness IN ('codex', 'antigravity');
