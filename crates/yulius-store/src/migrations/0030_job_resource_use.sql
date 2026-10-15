-- SPEC-154 (ADR-0127): what a CI job or a CD run used, as its process host measured it when it
-- reaped it, stored with its end and added to its span's figures by the end trigger. NULL is
-- unknown, and `json_patch` leaves a NULL figure out of the span (never 0, never null).
ALTER TABLE ci_jobs ADD COLUMN cpu_ms INTEGER;
ALTER TABLE ci_jobs ADD COLUMN memory_peak_mb INTEGER;
ALTER TABLE ci_jobs ADD COLUMN disk_written_mb INTEGER;
ALTER TABLE cd_runs ADD COLUMN cpu_ms INTEGER;
ALTER TABLE cd_runs ADD COLUMN memory_peak_mb INTEGER;
ALTER TABLE cd_runs ADD COLUMN disk_written_mb INTEGER;

DROP TRIGGER spans_ci_job_end;
CREATE TRIGGER spans_ci_job_end AFTER UPDATE OF ended_at ON ci_jobs
WHEN OLD.ended_at IS NULL AND NEW.ended_at IS NOT NULL
BEGIN
    UPDATE spans SET ended_at = MAX(started_at, NEW.ended_at), outcome = NEW.state,
        detail = json_patch(
            json_set(COALESCE(detail, '{}'), '$.exit_code', NEW.exit_code, '$.reason', NEW.reason),
            json_object('cpu_ms', NEW.cpu_ms, 'memory_peak_mb', NEW.memory_peak_mb,
                'disk_written_mb', NEW.disk_written_mb))
    WHERE subject_kind = 'ci_run' AND subject_id = NEW.run_id AND kind = 'job' AND ended_at IS NULL
        AND substr(key, 1, length('job:' || NEW.id || '@')) = 'job:' || NEW.id || '@';
    -- Stages inside the job end with it, with its outcome; those that ended before take it only
    -- when it passed.
    UPDATE spans SET ended_at = COALESCE(ended_at, MAX(started_at, NEW.ended_at)),
        outcome = COALESCE(outcome, CASE WHEN ended_at IS NULL OR NEW.state = 'passed' THEN NEW.state END)
    WHERE subject_kind = 'ci_run' AND subject_id = NEW.run_id AND (ended_at IS NULL OR outcome IS NULL)
        AND substr(key, 1, length('job:' || NEW.id || '@')) = 'job:' || NEW.id || '@';
END;

DROP TRIGGER spans_cd_run_end;
CREATE TRIGGER spans_cd_run_end AFTER UPDATE OF ended_at ON cd_runs
WHEN OLD.ended_at IS NULL AND NEW.ended_at IS NOT NULL
BEGIN
    UPDATE spans SET detail = json_patch(
            json_set(COALESCE(detail, '{}'), '$.exit_code', NEW.exit_code, '$.reason', NEW.reason),
            json_object('cpu_ms', NEW.cpu_ms, 'memory_peak_mb', NEW.memory_peak_mb,
                'disk_written_mb', NEW.disk_written_mb))
    WHERE subject_kind = 'cd_run' AND subject_id = NEW.id AND key = 'run';
    -- What was still open ends with the run and its outcome; what ended before takes it only when
    -- the run passed.
    UPDATE spans SET ended_at = COALESCE(ended_at, MAX(started_at, NEW.ended_at)),
        outcome = COALESCE(outcome, CASE WHEN ended_at IS NULL OR NEW.state = 'passed' THEN NEW.state END)
    WHERE subject_kind = 'cd_run' AND subject_id = NEW.id AND (ended_at IS NULL OR outcome IS NULL);
END;
