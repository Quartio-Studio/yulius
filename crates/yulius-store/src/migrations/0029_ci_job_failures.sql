-- SPEC-145: a CI job's failing tests' output, kept from its whole output (bounded, ADR-0119).
ALTER TABLE ci_jobs ADD COLUMN failures TEXT;
