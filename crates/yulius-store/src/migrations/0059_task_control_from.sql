-- Yulius schema version 59 (SPEC-490 OWN-009, ADR-0331): who handed a task over on request, so
-- that they can take it back without asking; null when nobody can (never handed over, handed
-- back to its giver, or passed from an owner who could no longer act).
ALTER TABLE tasks ADD COLUMN control_from TEXT;
