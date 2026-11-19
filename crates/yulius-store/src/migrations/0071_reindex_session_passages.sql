-- SPEC-535: older batches could index a dotenv result without its call context.
-- Deleting chunks also removes their FTS5 matches through the session_chunks_delete trigger.
DELETE FROM session_chunks;
UPDATE session_docs SET indexed_bytes = 0, indexed_lines = 0, index_full = 0;
