-- Yulius schema version 28 (SPEC-137, ADR-0112): every measured stage of a task, a CI run or a
-- CD run is one span. State-derived spans are written by the triggers below, in the same
-- transaction as the state change they measure, with the time the application wrote; stages
-- inside a job's or a deploy's output are written by Yulius as it reads the output. Nothing else
-- writes this table. Existing history is backfilled at the end, marked `backfilled`.
CREATE TABLE spans (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    sandbox_id TEXT NOT NULL,
    subject_kind TEXT NOT NULL,     -- task | ci_run | cd_run
    subject_id TEXT NOT NULL,
    key TEXT NOT NULL,              -- unique within the subject; a child's starts with its parent's + '/'
    parent_key TEXT,
    kind TEXT NOT NULL,             -- state | step | run | pause | delivery | wait | update | queue | prepare | job | stage | promote | deploy | verify
    name TEXT NOT NULL,
    environment TEXT,               -- dev | prod (CD)
    job TEXT,                       -- the CI job's definition name
    started_at INTEGER NOT NULL,
    ended_at INTEGER,               -- NULL while it runs
    outcome TEXT,
    source TEXT NOT NULL DEFAULT 'recorded', -- recorded | backfilled
    detail TEXT,                    -- JSON object of figures (tokens, tests, reason, commit, trigger)
    UNIQUE (subject_kind, subject_id, key)
);
CREATE INDEX spans_sandbox ON spans(sandbox_id, started_at);
CREATE INDEX spans_open ON spans(subject_kind, subject_id, kind) WHERE ended_at IS NULL;
CREATE INDEX spans_kind ON spans(sandbox_id, subject_kind, kind, started_at);
-- What the triggers and the backfill look up (review H1, L10).
CREATE INDEX IF NOT EXISTS step_events_step ON step_events(step_id, id);
CREATE INDEX IF NOT EXISTS metrics_task ON metrics(task_id);
-- A key that follows a state carries the span's own sequence (`state#<n>`), so changes in the same
-- millisecond each keep theirs.

-- ---- Tasks: the state track. Each change of status or activity ends the open state span and
-- opens the next; an ending status (closed, stopped, failed) only ends it.
CREATE TRIGGER spans_task_insert AFTER INSERT ON tasks
WHEN (CASE WHEN NEW.status IN ('queued', 'chained', 'prepared', 'starting', 'switching', 'stopping', 'waiting_capacity', 'running') THEN 1 END) IS NOT NULL
BEGIN
    INSERT INTO spans (sandbox_id, subject_kind, subject_id, key, kind, name, started_at)
    SELECT NEW.sandbox_id,
        'task',
        NEW.id,
        'state#' || (SELECT COALESCE(MAX(id), 0) + 1 FROM spans),
        'state',
        CASE
            WHEN NEW.status = 'waiting_capacity' THEN 'paused'
            WHEN NEW.status = 'running' THEN CASE NEW.activity WHEN 'waiting' THEN 'idle'
                WHEN 'needs_attention' THEN 'needs_you' WHEN 'done' THEN 'done' ELSE 'working' END
            ELSE NEW.status
        END,
        NEW.created_at
    WHERE NOT EXISTS (SELECT 1 FROM spans x WHERE x.subject_kind = 'task' AND x.subject_id = NEW.id AND x.key = 'state#' || (SELECT COALESCE(MAX(id), 0) + 1 FROM spans));
END;

CREATE TRIGGER spans_task_state AFTER UPDATE OF status, activity ON tasks
WHEN OLD.status IS NOT NEW.status OR OLD.activity IS NOT NEW.activity
BEGIN
    UPDATE spans SET ended_at = MAX(started_at, NEW.updated_at),
        outcome = CASE WHEN NEW.status IN ('queued', 'chained', 'prepared', 'starting', 'switching',
            'stopping', 'waiting_capacity', 'running') THEN NULL ELSE NEW.status END
    WHERE subject_kind = 'task' AND subject_id = NEW.id AND kind = 'state' AND ended_at IS NULL;
    INSERT INTO spans (sandbox_id, subject_kind, subject_id, key, kind, name, started_at)
    SELECT NEW.sandbox_id, 'task', NEW.id, 'state#' || (SELECT COALESCE(MAX(id), 0) + 1 FROM spans), 'state',
        CASE
            WHEN NEW.status = 'waiting_capacity' THEN 'paused'
            WHEN NEW.status = 'running' THEN CASE NEW.activity WHEN 'waiting' THEN 'idle'
                WHEN 'needs_attention' THEN 'needs_you' WHEN 'done' THEN 'done' ELSE 'working' END
            ELSE NEW.status
        END, NEW.updated_at
    WHERE NEW.status IN ('queued', 'chained', 'prepared', 'starting', 'switching', 'stopping',
        'waiting_capacity', 'running')
        AND NOT EXISTS (SELECT 1 FROM spans x WHERE x.subject_kind = 'task' AND x.subject_id = NEW.id AND x.key = 'state#' || (SELECT COALESCE(MAX(id), 0) + 1 FROM spans));
END;

-- An agent's request to a person (`needs_you`, SPEC-134): from the request to its answer. The
-- request's own time opens it; the time the store passes with the clearing closes it.
CREATE TRIGGER spans_task_needs_you AFTER UPDATE OF needs_you ON tasks
WHEN OLD.needs_you IS NOT NEW.needs_you
BEGIN
    UPDATE spans SET ended_at = MAX(started_at, COALESCE(json_extract(NEW.needs_you, '$.at'), NEW.updated_at)),
        outcome = CASE WHEN NEW.needs_you IS NULL THEN 'answered' ELSE 'replaced' END
    WHERE subject_kind = 'task' AND subject_id = NEW.id AND kind = 'wait' AND name = 'needs_you'
        AND ended_at IS NULL;
    INSERT INTO spans (sandbox_id, subject_kind, subject_id, key, kind, name, started_at, detail)
    SELECT NEW.sandbox_id, 'task', NEW.id, 'needs_you#' || (SELECT COALESCE(MAX(id), 0) + 1 FROM spans), 'wait',
        'needs_you', json_extract(NEW.needs_you, '$.at'),
        json_object('action', json_extract(NEW.needs_you, '$.action'))
    WHERE NEW.needs_you IS NOT NULL AND json_valid(NEW.needs_you)
        AND json_extract(NEW.needs_you, '$.at') IS NOT NULL
        AND NOT EXISTS (SELECT 1 FROM spans x WHERE x.subject_kind = 'task' AND x.subject_id = NEW.id AND x.key = 'needs_you#' || (SELECT COALESCE(MAX(id), 0) + 1 FROM spans));
END;

-- ---- Steps: in_progress opens a step span with the task's token and working-time totals at
-- that moment; the step's next status ends it with what the task's runs recorded meanwhile.
CREATE TRIGGER spans_step_event AFTER INSERT ON step_events
BEGIN
    UPDATE spans SET ended_at = MAX(started_at, NEW.at), outcome = NEW.to_status,
        detail = json_set(COALESCE(detail, '{}'),
            '$.input_tokens', (SELECT COALESCE(SUM(input_tokens), 0) FROM metrics WHERE task_id = NEW.task_id)
                - COALESCE(json_extract(detail, '$.input_tokens_start'), 0),
            '$.output_tokens', (SELECT COALESCE(SUM(output_tokens), 0) FROM metrics WHERE task_id = NEW.task_id)
                - COALESCE(json_extract(detail, '$.output_tokens_start'), 0),
            '$.active_ms', (SELECT COALESCE(SUM(active_ms), 0) FROM metrics WHERE task_id = NEW.task_id)
                - COALESCE(json_extract(detail, '$.active_ms_start'), 0))
    WHERE subject_kind = 'task' AND subject_id = NEW.task_id AND kind = 'step'
        AND substr(key, 1, length('step:' || NEW.step_id || '@')) = 'step:' || NEW.step_id || '@'
        AND ended_at IS NULL AND NEW.to_status != 'in_progress';
    INSERT INTO spans (sandbox_id, subject_kind, subject_id, key, kind, name, started_at, detail)
    SELECT t.sandbox_id, 'task', NEW.task_id, 'step:' || NEW.step_id || '@' || NEW.at, 'step',
        COALESCE((SELECT title FROM steps WHERE id = NEW.step_id), 'step'), NEW.at,
        json_object('step_id', NEW.step_id,
            'effort', (SELECT effort FROM steps WHERE id = NEW.step_id),
            'input_tokens_start', (SELECT COALESCE(SUM(input_tokens), 0) FROM metrics WHERE task_id = NEW.task_id),
            'output_tokens_start', (SELECT COALESCE(SUM(output_tokens), 0) FROM metrics WHERE task_id = NEW.task_id),
            'active_ms_start', (SELECT COALESCE(SUM(active_ms), 0) FROM metrics WHERE task_id = NEW.task_id))
    FROM tasks t WHERE t.id = NEW.task_id AND NEW.to_status = 'in_progress'
        AND NOT EXISTS (SELECT 1 FROM spans s WHERE s.subject_kind = 'task' AND s.subject_id = NEW.task_id
            AND s.kind = 'step' AND s.ended_at IS NULL
            AND substr(s.key, 1, length('step:' || NEW.step_id || '@')) = 'step:' || NEW.step_id || '@')
        AND NOT EXISTS (SELECT 1 FROM spans x WHERE x.subject_kind = 'task' AND x.subject_id = NEW.task_id AND x.key = 'step:' || NEW.step_id || '@' || NEW.at);
END;

-- ---- Runs (attempts) and their figures.
CREATE TRIGGER spans_attempt_insert AFTER INSERT ON attempts
BEGIN
    INSERT INTO spans (sandbox_id, subject_kind, subject_id, key, kind, name, started_at, ended_at, outcome, detail)
    SELECT t.sandbox_id, 'task', NEW.task_id, 'run:' || NEW.id, 'run', 'run', NEW.started_at,
        NEW.ended_at, CASE WHEN NEW.ended_at IS NULL THEN NULL ELSE NEW.state END,
        json_object('seq', NEW.seq, 'launch', NEW.launch_kind, 'account_id', NEW.account_id)
    FROM tasks t WHERE t.id = NEW.task_id
        AND NOT EXISTS (SELECT 1 FROM spans x WHERE x.subject_kind = 'task' AND x.subject_id = NEW.task_id AND x.key = 'run:' || NEW.id);
END;

CREATE TRIGGER spans_attempt_end AFTER UPDATE OF ended_at, state ON attempts
WHEN NEW.ended_at IS NOT NULL
BEGIN
    UPDATE spans SET ended_at = MAX(started_at, NEW.ended_at), outcome = NEW.state,
        detail = json_set(COALESCE(detail, '{}'), '$.exit_reason', NEW.exit_reason, '$.exit_code', NEW.exit_code)
    WHERE subject_kind = 'task' AND subject_id = NEW.task_id AND key = 'run:' || NEW.id;
END;

CREATE TRIGGER spans_metrics_insert AFTER INSERT ON metrics
BEGIN
    UPDATE spans SET detail = json_set(COALESCE(detail, '{}'), '$.model', NEW.model,
        '$.input_tokens', NEW.input_tokens, '$.output_tokens', NEW.output_tokens,
        '$.cache_read', NEW.cache_read, '$.cache_write', NEW.cache_write,
        '$.turns', NEW.turns, '$.active_ms', NEW.active_ms)
    WHERE subject_kind = 'task' AND subject_id = NEW.task_id AND key = 'run:' || NEW.attempt_id;
END;

CREATE TRIGGER spans_metrics_update AFTER UPDATE ON metrics
BEGIN
    UPDATE spans SET detail = json_set(COALESCE(detail, '{}'), '$.model', NEW.model,
        '$.input_tokens', NEW.input_tokens, '$.output_tokens', NEW.output_tokens,
        '$.cache_read', NEW.cache_read, '$.cache_write', NEW.cache_write,
        '$.turns', NEW.turns, '$.active_ms', NEW.active_ms)
    WHERE subject_kind = 'task' AND subject_id = NEW.task_id AND key = 'run:' || NEW.attempt_id;
END;

-- ---- Capacity and usage-limit pauses (SPEC-034, ROT-010).
CREATE TRIGGER spans_pause_insert AFTER INSERT ON capacity_pauses
BEGIN
    INSERT INTO spans (sandbox_id, subject_kind, subject_id, key, kind, name, started_at)
    SELECT t.sandbox_id, 'task', NEW.task_id, 'pause:' || NEW.id, 'pause', NEW.kind, NEW.started_at
    FROM tasks t WHERE t.id = NEW.task_id
        AND NOT EXISTS (SELECT 1 FROM spans x WHERE x.subject_kind = 'task' AND x.subject_id = NEW.task_id AND x.key = 'pause:' || NEW.id);
END;

CREATE TRIGGER spans_pause_update AFTER UPDATE ON capacity_pauses
BEGIN
    UPDATE spans SET name = NEW.kind,
        ended_at = CASE WHEN NEW.ended_at IS NULL THEN NULL ELSE MAX(started_at, NEW.ended_at) END,
        outcome = NEW.outcome
    WHERE subject_kind = 'task' AND subject_id = NEW.task_id AND key = 'pause:' || NEW.id;
END;

-- ---- Delivery: each stage of a task's delivery until the next (pr_open: open → merged).
CREATE TRIGGER spans_delivery_insert AFTER INSERT ON delivery
WHEN NEW.stage != 'none'
BEGIN
    INSERT INTO spans (sandbox_id, subject_kind, subject_id, key, kind, name, started_at, detail)
    SELECT t.sandbox_id, 'task', NEW.task_id, 'delivery#' || (SELECT COALESCE(MAX(id), 0) + 1 FROM spans), 'delivery',
        NEW.stage, NEW.updated_at, json_object('pr', NEW.pr_number)
    FROM tasks t WHERE t.id = NEW.task_id AND NEW.stage != 'production_done'
        AND NOT EXISTS (SELECT 1 FROM spans x WHERE x.subject_kind = 'task' AND x.subject_id = NEW.task_id AND x.key = 'delivery#' || (SELECT COALESCE(MAX(id), 0) + 1 FROM spans));
END;

CREATE TRIGGER spans_delivery_update AFTER UPDATE OF stage ON delivery
WHEN OLD.stage IS NOT NEW.stage
BEGIN
    UPDATE spans SET ended_at = MAX(started_at, NEW.updated_at), outcome = NEW.stage
    WHERE subject_kind = 'task' AND subject_id = NEW.task_id AND kind = 'delivery' AND ended_at IS NULL;
    INSERT INTO spans (sandbox_id, subject_kind, subject_id, key, kind, name, started_at, detail)
    SELECT t.sandbox_id, 'task', NEW.task_id, 'delivery#' || (SELECT COALESCE(MAX(id), 0) + 1 FROM spans), 'delivery',
        NEW.stage, NEW.updated_at, json_object('pr', NEW.pr_number)
    FROM tasks t WHERE t.id = NEW.task_id AND NEW.stage NOT IN ('none', 'production_done')
        AND NOT EXISTS (SELECT 1 FROM spans x WHERE x.subject_kind = 'task' AND x.subject_id = NEW.task_id AND x.key = 'delivery#' || (SELECT COALESCE(MAX(id), 0) + 1 FROM spans));
END;

-- ---- CI runs: the queue (why it waited: the disk note, else a busy lane), the run, each job.
-- A result reused from an identical tree is inserted already passed and marked `reused`.
CREATE TRIGGER spans_ci_run_insert AFTER INSERT ON ci_runs
BEGIN
    INSERT INTO spans (sandbox_id, subject_kind, subject_id, key, kind, name, started_at, ended_at, outcome, detail)
    SELECT NEW.sandbox_id,
        'ci_run',
        NEW.id,
        'queue@' || NEW.queued_at,
        'queue',
        'queue',
        NEW.queued_at,
        CASE WHEN NEW.ended_at IS NOT NULL THEN COALESCE(NEW.started_at, NEW.ended_at) END,
        CASE WHEN NEW.ended_at IS NOT NULL THEN NEW.state END,
        json_object('reference', NEW.reference, 'sha', NEW.sha, 'requested_by', NEW.requested_by,
            'reused', CASE WHEN NEW.state = 'passed' AND NEW.reason LIKE 'not run again:%' THEN json('true') ELSE json('false') END)
    WHERE NOT EXISTS (SELECT 1 FROM spans x WHERE x.subject_kind = 'ci_run' AND x.subject_id = NEW.id AND x.key = 'queue@' || NEW.queued_at);
    INSERT INTO spans (sandbox_id, subject_kind, subject_id, key, kind, name, started_at, ended_at, outcome, detail)
    SELECT NEW.sandbox_id, 'ci_run', NEW.id, 'run@' || NEW.started_at, 'run', 'run', NEW.started_at,
        NEW.ended_at, CASE WHEN NEW.ended_at IS NOT NULL THEN NEW.state END,
        json_object('reference', NEW.reference, 'sha', NEW.sha, 'requested_by', NEW.requested_by,
            'reused', CASE WHEN NEW.state = 'passed' AND NEW.reason LIKE 'not run again:%' THEN json('true') ELSE json('false') END,
            'reason', NEW.reason)
    WHERE NEW.started_at IS NOT NULL
        AND NOT EXISTS (SELECT 1 FROM spans x WHERE x.subject_kind = 'ci_run' AND x.subject_id = NEW.id AND x.key = 'run@' || NEW.started_at);
END;

CREATE TRIGGER spans_ci_run_start AFTER UPDATE OF started_at ON ci_runs
WHEN OLD.started_at IS NULL AND NEW.started_at IS NOT NULL
BEGIN
    UPDATE spans SET ended_at = MAX(started_at, NEW.started_at),
        detail = json_set(COALESCE(detail, '{}'), '$.reason', OLD.reason)
    WHERE subject_kind = 'ci_run' AND subject_id = NEW.id AND kind = 'queue' AND ended_at IS NULL;
    INSERT INTO spans (sandbox_id, subject_kind, subject_id, key, kind, name, started_at, detail)
    SELECT NEW.sandbox_id,
        'ci_run',
        NEW.id,
        'run#' || (SELECT COALESCE(MAX(id), 0) + 1 FROM spans),
        'run',
        'run',
        NEW.started_at,
        json_object('reference', NEW.reference, 'sha', NEW.sha, 'requested_by', NEW.requested_by,
            'reused', json('false'))
    WHERE NOT EXISTS (SELECT 1 FROM spans x WHERE x.subject_kind = 'ci_run' AND x.subject_id = NEW.id AND x.key = 'run#' || (SELECT COALESCE(MAX(id), 0) + 1 FROM spans));
END;

CREATE TRIGGER spans_ci_run_end AFTER UPDATE OF ended_at ON ci_runs
WHEN OLD.ended_at IS NULL AND NEW.ended_at IS NOT NULL
BEGIN
    UPDATE spans SET ended_at = MAX(started_at, NEW.ended_at), outcome = NEW.state,
        detail = CASE WHEN kind = 'run' THEN json_set(COALESCE(detail, '{}'), '$.reason', NEW.reason) ELSE detail END
    WHERE subject_kind = 'ci_run' AND subject_id = NEW.id AND ended_at IS NULL;
END;

ALTER TABLE ci_runs ADD COLUMN requeued_at INTEGER;
-- A run put back in the queue (disk, restart): what ran so far ends, a new queue span opens at
-- the time the store passes as `requeued_at`.
CREATE TRIGGER spans_ci_run_requeue AFTER UPDATE OF requeued_at ON ci_runs
WHEN NEW.requeued_at IS NOT NULL AND OLD.requeued_at IS NOT NEW.requeued_at
BEGIN
    UPDATE spans SET ended_at = MAX(started_at, NEW.requeued_at), outcome = 'requeued'
    WHERE subject_kind = 'ci_run' AND subject_id = NEW.id AND ended_at IS NULL;
    INSERT INTO spans (sandbox_id, subject_kind, subject_id, key, kind, name, started_at, detail)
    SELECT NEW.sandbox_id,
        'ci_run',
        NEW.id,
        'queue#' || (SELECT COALESCE(MAX(id), 0) + 1 FROM spans),
        'queue',
        'queue',
        NEW.requeued_at,
        json_object('reference', NEW.reference, 'sha', NEW.sha, 'requested_by', NEW.requested_by,
            'requeued', json('true'), 'reused', json('false'))
    WHERE NOT EXISTS (SELECT 1 FROM spans x WHERE x.subject_kind = 'ci_run' AND x.subject_id = NEW.id AND x.key = 'queue#' || (SELECT COALESCE(MAX(id), 0) + 1 FROM spans));
END;

CREATE TRIGGER spans_ci_job_start AFTER UPDATE OF started_at ON ci_jobs
WHEN OLD.started_at IS NULL AND NEW.started_at IS NOT NULL
BEGIN
    INSERT INTO spans (sandbox_id, subject_kind, subject_id, key, parent_key, kind, name, job, started_at, detail)
    SELECT r.sandbox_id, 'ci_run', NEW.run_id, 'job:' || NEW.id || '@' || NEW.started_at, NULL, 'job',
        NEW.name, NEW.job_key, NEW.started_at,
        json_object('matrix', NEW.matrix, 'cpus', NEW.cpus, 'memory_mb', NEW.memory_mb)
    FROM ci_runs r WHERE r.id = NEW.run_id
        AND NOT EXISTS (SELECT 1 FROM spans x WHERE x.subject_kind = 'ci_run' AND x.subject_id = NEW.run_id AND x.key = 'job:' || NEW.id || '@' || NEW.started_at);
END;

CREATE TRIGGER spans_ci_job_end AFTER UPDATE OF ended_at ON ci_jobs
WHEN OLD.ended_at IS NULL AND NEW.ended_at IS NOT NULL
BEGIN
    UPDATE spans SET ended_at = MAX(started_at, NEW.ended_at), outcome = NEW.state,
        detail = json_set(COALESCE(detail, '{}'), '$.exit_code', NEW.exit_code, '$.reason', NEW.reason)
    WHERE subject_kind = 'ci_run' AND subject_id = NEW.run_id AND kind = 'job' AND ended_at IS NULL
        AND substr(key, 1, length('job:' || NEW.id || '@')) = 'job:' || NEW.id || '@';
    -- Stages inside the job end with it, with its outcome; those that ended before take it only
    -- when it passed.
    UPDATE spans SET ended_at = COALESCE(ended_at, MAX(started_at, NEW.ended_at)),
        outcome = COALESCE(outcome, CASE WHEN ended_at IS NULL OR NEW.state = 'passed' THEN NEW.state END)
    WHERE subject_kind = 'ci_run' AND subject_id = NEW.run_id AND (ended_at IS NULL OR outcome IS NULL)
        AND substr(key, 1, length('job:' || NEW.id || '@')) = 'job:' || NEW.id || '@';
END;

-- ---- CD runs: the run with its environment, commit and trigger; its promotion, deploy, verify
-- and their stages are written by Yulius as they happen and end with the run at the latest.
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
            'task_id', NEW.task_id)
    WHERE NOT EXISTS (SELECT 1 FROM spans x WHERE x.subject_kind = 'cd_run' AND x.subject_id = NEW.id AND x.key = 'run');
END;

CREATE TRIGGER spans_cd_run_end AFTER UPDATE OF ended_at ON cd_runs
WHEN OLD.ended_at IS NULL AND NEW.ended_at IS NOT NULL
BEGIN
    UPDATE spans SET detail = json_set(COALESCE(detail, '{}'), '$.exit_code', NEW.exit_code, '$.reason', NEW.reason)
    WHERE subject_kind = 'cd_run' AND subject_id = NEW.id AND key = 'run';
    -- What was still open ends with the run and its outcome; what ended before takes it only when
    -- the run passed.
    UPDATE spans SET ended_at = COALESCE(ended_at, MAX(started_at, NEW.ended_at)),
        outcome = COALESCE(outcome, CASE WHEN ended_at IS NULL OR NEW.state = 'passed' THEN NEW.state END)
    WHERE subject_kind = 'cd_run' AND subject_id = NEW.id AND (ended_at IS NULL OR outcome IS NULL);
END;

-- ---- Backfill what the existing tables say reliably, marked `backfilled`.
INSERT OR IGNORE INTO spans (sandbox_id, subject_kind, subject_id, key, kind, name, started_at, ended_at, outcome, source, detail)
SELECT sandbox_id, 'ci_run', id, 'queue@' || queued_at, 'queue', 'queue', queued_at,
    COALESCE(started_at, ended_at), CASE WHEN started_at IS NULL AND ended_at IS NOT NULL THEN state END,
    'backfilled',
    json_object('reference', reference, 'sha', sha, 'requested_by', requested_by,
        'reused', CASE WHEN state = 'passed' AND reason LIKE 'not run again:%' THEN json('true') ELSE json('false') END)
FROM ci_runs;
INSERT OR IGNORE INTO spans (sandbox_id, subject_kind, subject_id, key, kind, name, started_at, ended_at, outcome, source, detail)
SELECT sandbox_id, 'ci_run', id, 'run@' || started_at, 'run', 'run', started_at, ended_at,
    CASE WHEN ended_at IS NOT NULL THEN state END, 'backfilled',
    json_object('reference', reference, 'sha', sha, 'requested_by', requested_by, 'reason', reason,
        'reused', CASE WHEN state = 'passed' AND reason LIKE 'not run again:%' THEN json('true') ELSE json('false') END)
FROM ci_runs WHERE started_at IS NOT NULL;
INSERT OR IGNORE INTO spans (sandbox_id, subject_kind, subject_id, key, kind, name, job, started_at, ended_at, outcome, source, detail)
SELECT r.sandbox_id, 'ci_run', j.run_id, 'job:' || j.id || '@' || j.started_at, 'job', j.name, j.job_key,
    j.started_at, j.ended_at, CASE WHEN j.ended_at IS NOT NULL THEN j.state END, 'backfilled',
    json_object('matrix', j.matrix, 'cpus', j.cpus, 'memory_mb', j.memory_mb, 'exit_code', j.exit_code, 'reason', j.reason)
FROM ci_jobs j JOIN ci_runs r ON r.id = j.run_id WHERE j.started_at IS NOT NULL;
INSERT OR IGNORE INTO spans (sandbox_id, subject_kind, subject_id, key, kind, name, environment, started_at, ended_at, outcome, source, detail)
SELECT sandbox_id, 'cd_run', id, 'run', 'run', 'run', environment, started_at, ended_at,
    CASE WHEN ended_at IS NOT NULL THEN state END, 'backfilled',
    json_object('branch', branch, 'sha', sha, 'requested_by', requested_by, 'task_id', task_id,
        'exit_code', exit_code, 'reason', reason)
FROM cd_runs;
INSERT OR IGNORE INTO spans (sandbox_id, subject_kind, subject_id, key, kind, name, started_at, ended_at, outcome, source, detail)
SELECT t.sandbox_id, 'task', a.task_id, 'run:' || a.id, 'run', 'run', a.started_at, a.ended_at,
    CASE WHEN a.ended_at IS NOT NULL THEN a.state END, 'backfilled',
    json_object('seq', a.seq, 'launch', a.launch_kind, 'account_id', a.account_id,
        'exit_reason', a.exit_reason, 'exit_code', a.exit_code, 'model', m.model,
        'input_tokens', m.input_tokens, 'output_tokens', m.output_tokens, 'cache_read', m.cache_read,
        'cache_write', m.cache_write, 'turns', m.turns, 'active_ms', m.active_ms)
FROM attempts a JOIN tasks t ON t.id = a.task_id LEFT JOIN metrics m ON m.attempt_id = a.id;
INSERT OR IGNORE INTO spans (sandbox_id, subject_kind, subject_id, key, kind, name, started_at, ended_at, outcome, source)
SELECT t.sandbox_id, 'task', p.task_id, 'pause:' || p.id, 'pause', p.kind, p.started_at, p.ended_at,
    p.outcome, 'backfilled'
FROM capacity_pauses p JOIN tasks t ON t.id = p.task_id;
-- A step from each in_progress event to the step's next other status (its tokens are not
-- attributable after the fact).
INSERT OR IGNORE INTO spans (sandbox_id, subject_kind, subject_id, key, kind, name, started_at, ended_at, outcome, source, detail)
SELECT t.sandbox_id, 'task', e.task_id, 'step:' || e.step_id || '@' || e.at, 'step',
    COALESCE(s.title, 'step'), e.at,
    (SELECT MIN(n.at) FROM step_events n WHERE n.step_id = e.step_id AND n.id > e.id AND n.to_status != 'in_progress'),
    (SELECT n.to_status FROM step_events n WHERE n.step_id = e.step_id AND n.id > e.id AND n.to_status != 'in_progress' ORDER BY n.id LIMIT 1),
    'backfilled', json_object('step_id', e.step_id, 'effort', s.effort)
FROM step_events e JOIN tasks t ON t.id = e.task_id LEFT JOIN steps s ON s.id = e.step_id
WHERE e.to_status = 'in_progress'
    AND COALESCE((SELECT p.to_status FROM step_events p WHERE p.step_id = e.step_id AND p.id < e.id
        ORDER BY p.id DESC LIMIT 1), '') != 'in_progress';
-- A live task's current state opens its state track (its earlier states are not stored).
INSERT OR IGNORE INTO spans (sandbox_id, subject_kind, subject_id, key, kind, name, started_at, source)
SELECT sandbox_id, 'task', id, 'state@' || state_at || ':' || status || ':' || COALESCE(activity, ''), 'state',
    CASE
        WHEN status = 'waiting_capacity' THEN 'paused'
        WHEN status = 'running' THEN CASE activity WHEN 'waiting' THEN 'idle'
            WHEN 'needs_attention' THEN 'needs_you' WHEN 'done' THEN 'done' ELSE 'working' END
        ELSE status
    END, state_at, 'backfilled'
FROM tasks WHERE status IN ('queued', 'chained', 'prepared', 'starting', 'switching', 'stopping',
    'waiting_capacity', 'running') AND archived_at IS NULL;
