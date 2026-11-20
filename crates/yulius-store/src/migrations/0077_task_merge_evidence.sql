-- The commit landed on the integration branch, recorded alongside the delivery transition.
CREATE TABLE task_merges (
    task_id TEXT NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
    sandbox_id TEXT NOT NULL,
    merged_sha TEXT NOT NULL,
    branch TEXT NOT NULL,
    pr_number INTEGER,
    head_sha TEXT NOT NULL,
    PRIMARY KEY (task_id, merged_sha)
);
CREATE INDEX task_merges_landing ON task_merges(sandbox_id, merged_sha);
CREATE INDEX task_merges_head ON task_merges(sandbox_id, head_sha, pr_number);
CREATE INDEX task_merges_pr ON task_merges(task_id, pr_number);
CREATE INDEX cd_runs_commit ON cd_runs(sandbox_id, sha);
CREATE INDEX environment_runs_commit ON environment_runs(sandbox_id, commit_sha);

-- Preserve the integration branch when Yulius opens or adopts a task PR.
-- At upgrade, use the integration branch in the task's original PR-opening audit event.
-- A new merge waiting on GitHub additionally has the accepted head and returned commit.
CREATE TABLE task_merge_recovery (
    task_id TEXT NOT NULL REFERENCES tasks(id) ON DELETE CASCADE, pr_number INTEGER NOT NULL,
    branch TEXT NOT NULL, head_sha TEXT, merged_sha TEXT,
    PRIMARY KEY (task_id, pr_number)
);
-- delivery.pr.open records "pull request #N HEAD_BRANCH -> BASE at SHA". An absent,
-- ambiguous or unrecognised historical event supplies no expected integration branch.
WITH openings AS (
    SELECT d.task_id, d.pr_number, a.detail,
           'pull request #' || d.pr_number || ' ' || t.branch || ' -> ' AS prefix
    FROM delivery d JOIN tasks t ON t.id = d.task_id JOIN sandboxes s ON s.id = t.sandbox_id
    JOIN audit_log a ON a.action = 'delivery.pr.open' AND a.target = s.slug || '#' || t.number
    WHERE d.pr_number IS NOT NULL AND d.stage != 'none'
), targets AS (
    SELECT task_id, pr_number, substr(detail, length(prefix) + 1) AS tail FROM openings
    WHERE substr(detail, 1, length(prefix)) = prefix
), identities AS (
    SELECT task_id, pr_number, substr(tail, 1, instr(tail, ' at ') - 1) AS branch,
           substr(tail, instr(tail, ' at ') + 4) AS opening_sha FROM targets
    WHERE instr(tail, ' at ') > 1
)
INSERT INTO task_merge_recovery(task_id, pr_number, branch)
SELECT task_id, pr_number, min(branch) FROM identities
WHERE length(opening_sha) IN (40, 64) AND opening_sha NOT GLOB '*[^0-9a-fA-F]*'
GROUP BY task_id, pr_number HAVING min(branch) = max(branch);
CREATE TRIGGER forget_resolved_task_merge AFTER INSERT ON task_merges BEGIN
    DELETE FROM task_merge_recovery WHERE task_id = NEW.task_id AND pr_number = NEW.pr_number;
END;

-- Share the projection between live reads and retention. The run's note explains tree or
-- train reuse. Keep job verdicts in gate order, including checks that did not run.
CREATE VIEW task_merge_ci_refs AS
SELECT task_id, sandbox_id, merged_sha, merged_sha AS sha, 'branch/' || branch AS reference FROM task_merges
UNION ALL
SELECT task_id, sandbox_id, merged_sha, head_sha, 'pr/' || pr_number FROM task_merges WHERE pr_number IS NOT NULL;

CREATE VIEW task_merge_ci AS
SELECT m.task_id, m.merged_sha, r.id AS run_id, r.queued_at AS at, r.rowid AS seq,
    json_object('run_id', r.id, 'kind', 'ci', 'reference', r.reference, 'sha', r.sha,
        'state', r.state, 'reason', r.reason, 'checks', json((
        SELECT json_group_array(json_object('name', name, 'state', state, 'reason', reason) ORDER BY ordinal)
        FROM ci_jobs WHERE run_id = r.id))) AS result
FROM task_merge_ci_refs m JOIN ci_runs r
    ON r.sandbox_id = m.sandbox_id AND r.sha = m.sha AND r.reference = m.reference;

-- The CD wrapper exits zero only after each configured command succeeded. On other outcomes
-- project output and its diagnostic spans cannot attest individual command exits.
CREATE VIEW task_merge_cd AS
SELECT m.task_id, m.merged_sha, r.id AS run_id, r.started_at AS at, r.rowid AS seq,
    json_object('run_id', r.id, 'kind', 'cd', 'reference', r.environment, 'sha', r.sha,
        'branch', r.branch, 'stage', r.stage, 'state', r.state, 'reason', r.reason, 'checks', json((
        SELECT json_group_array(json_object('name', phase, 'state', CASE
            WHEN nullif(trim(CASE phase WHEN 'deploy' THEN r.deploy ELSE r.verify END), '') IS NULL THEN 'not configured'
            WHEN r.state = 'passed' AND r.exit_code = 0 AND r.ended_at IS NOT NULL THEN 'passed'
            ELSE 'unconfirmed' END))
        FROM (SELECT 'deploy' AS phase UNION ALL SELECT 'verify')))) AS result
FROM task_merges m JOIN cd_runs r ON r.sandbox_id = m.sandbox_id AND r.sha = m.merged_sha
WHERE r.kind = 'deploy';

-- Keep only compact verdicts when a linked run is pruned. Logs, scripts and credentials
-- follow normal retention. A failed archive rolls back the deletion in the same statement.
CREATE TABLE task_merge_archived_runs (
    task_id TEXT NOT NULL, merged_sha TEXT NOT NULL, run_id TEXT NOT NULL,
    at INTEGER NOT NULL, seq INTEGER NOT NULL, result TEXT NOT NULL,
    PRIMARY KEY (task_id, merged_sha, run_id),
    FOREIGN KEY (task_id, merged_sha) REFERENCES task_merges(task_id, merged_sha) ON DELETE CASCADE
);
CREATE TRIGGER archive_task_merge_ci BEFORE DELETE ON ci_runs BEGIN
    INSERT INTO task_merge_archived_runs SELECT * FROM task_merge_ci WHERE run_id = OLD.id;
END;
CREATE TRIGGER archive_task_merge_cd BEFORE DELETE ON cd_runs BEGIN
    INSERT INTO task_merge_archived_runs SELECT * FROM task_merge_cd WHERE run_id = OLD.id;
END;
CREATE VIEW task_merge_runs AS
    SELECT * FROM task_merge_ci UNION ALL SELECT * FROM task_merge_cd
    UNION ALL SELECT * FROM task_merge_archived_runs
    UNION ALL
    SELECT m.task_id, m.merged_sha, r.id, r.started_at, r.rowid,
        json_object('run_id', r.id, 'kind', 'environment command', 'reference', r.environment,
            'sha', r.commit_sha, 'state', r.state, 'checks', json_array(
                json_object('name', 'command', 'state', r.state)))
    FROM task_merges m JOIN environment_runs r ON r.sandbox_id = m.sandbox_id AND r.commit_sha = m.merged_sha
    WHERE r.kind = 'command';
