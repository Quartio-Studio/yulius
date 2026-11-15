-- Yulius migration 0061, schema version 61 (SPEC-483 slice 35, QC1): the capacity and accounts
-- views read the runs and pauses of a window (since a time) through an index instead of
-- scanning every run and pause ever recorded. Neither table is pruned, so the scans grew with
-- the instance's age.
CREATE INDEX IF NOT EXISTS attempts_ended_at ON attempts(ended_at);
CREATE INDEX IF NOT EXISTS attempts_started_at ON attempts(started_at);
CREATE INDEX IF NOT EXISTS capacity_pauses_ended_at ON capacity_pauses(ended_at);
