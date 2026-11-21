-- SQLite stores equal index keys in rowid order, the order CI admitted the runs.
CREATE INDEX ci_runs_admission ON ci_runs(sandbox_id);
