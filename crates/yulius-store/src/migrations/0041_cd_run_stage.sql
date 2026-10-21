-- Yulius schema version 41 (SPEC-257, ADR-0195): CD environments are any `[cd.<name>]` table with a
-- stage (`dev` or `prod`) that carries their rules. `environment` keeps the name; `stage` is new.
-- Runs written before were of `dev` or `prod`, whose stage is their name.
ALTER TABLE cd_runs ADD COLUMN stage TEXT NOT NULL DEFAULT 'prod';
UPDATE cd_runs SET stage = environment WHERE environment IN ('dev', 'prod');

-- A CD run's span carries its stage, so delivery figures count every environment of a stage
-- (`detail.stage`; spans written before keep their name, `dev` or `prod`, which is their stage).
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
            'task_id', NEW.task_id, 'stage', NEW.stage)
    WHERE NOT EXISTS (SELECT 1 FROM spans x WHERE x.subject_kind = 'cd_run' AND x.subject_id = NEW.id AND x.key = 'run');
END;
