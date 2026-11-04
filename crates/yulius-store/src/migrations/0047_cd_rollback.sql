-- Yulius schema version 47 (SPEC-415, ADR-0263): rollback in CD. A CD run deploys, rolls back to a
-- kept release, or drills the rollback after a dev deploy (`kind`). A rollback or drill runs the
-- scripts of `source` (the branch head; NULL: `sha`) and goes to the releases of `steps`, JSON
-- [[part, release, sha], ...] in order.
ALTER TABLE cd_runs ADD COLUMN kind TEXT NOT NULL DEFAULT 'deploy';
ALTER TABLE cd_runs ADD COLUMN source TEXT;
ALTER TABLE cd_runs ADD COLUMN steps TEXT;
CREATE INDEX cd_runs_kind ON cd_runs(sandbox_id, environment, stage, kind, started_at);

-- The releases of each environment, stage and part: what passed deploys shipped (`deploy`) and
-- what passed rollbacks went to (`rollback`), newest last. They outlive the runs' 14 days: they are
-- what a rollback may go to. The newest 20 of each part are kept.
CREATE TABLE cd_releases (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    sandbox_id TEXT NOT NULL,
    environment TEXT NOT NULL,
    stage TEXT NOT NULL,            -- dev | prod
    part TEXT NOT NULL DEFAULT '',  -- '' for an environment of one part
    release TEXT NOT NULL,          -- the release id (a deploy that named none: its commit)
    sha TEXT NOT NULL,              -- the commit it was deployed from
    kind TEXT NOT NULL,             -- deploy | rollback
    requested_by TEXT NOT NULL,
    at INTEGER NOT NULL,            -- when its run ended
    barrier TEXT,                   -- why rolling back past it is unsafe (deploys only)
    run_id TEXT NOT NULL
);
CREATE INDEX cd_releases_part ON cd_releases(sandbox_id, environment, stage, part, id);

-- A CD run's span says what it did.
DROP TRIGGER spans_cd_run_insert;
CREATE TRIGGER spans_cd_run_insert AFTER INSERT ON cd_runs
BEGIN
    INSERT INTO spans (sandbox_id, subject_kind, subject_id, key, kind, name, environment, started_at, ended_at, outcome, detail)
    SELECT NEW.sandbox_id,
        'cd_run',
        NEW.id,
        'run',
        'run',
        'run',
        NEW.environment,
        NEW.started_at,
        NEW.ended_at,
        CASE WHEN NEW.ended_at IS NOT NULL THEN NEW.state END,
        json_object('branch', NEW.branch, 'sha', NEW.sha, 'requested_by', NEW.requested_by,
            'task_id', NEW.task_id, 'stage', NEW.stage, 'kind', NEW.kind)
    WHERE NOT EXISTS (SELECT 1 FROM spans x WHERE x.subject_kind = 'cd_run' AND x.subject_id = NEW.id AND x.key = 'run');
END;
