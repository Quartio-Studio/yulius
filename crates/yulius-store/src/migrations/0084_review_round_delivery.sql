-- SPEC-605: retain the last review boundary even for a merge with zero recorded rounds.
ALTER TABLE delivery ADD COLUMN review_merge_at INTEGER;
CREATE INDEX review_rounds_landing ON review_rounds(task_id, pr_number, merged_at);
