-- Yulius schema version 41 (SPEC-257, ADR-0195): CD environments are any `[cd.<name>]` table with a
-- stage (`dev` or `prod`) that carries their rules. `environment` keeps the name; `stage` is new.
-- Runs written before were of `dev` or `prod`, whose stage is their name.
ALTER TABLE cd_runs ADD COLUMN stage TEXT NOT NULL DEFAULT 'dev';
UPDATE cd_runs SET stage = environment WHERE environment IN ('dev', 'prod');
