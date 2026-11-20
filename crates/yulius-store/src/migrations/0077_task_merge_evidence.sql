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

-- Share the projection between live reads and retention. The run's note explains tree or
-- train reuse. Keep job verdicts in gate order, including checks that did not run.
CREATE VIEW task_merge_ci_refs AS
SELECT task_id, sandbox_id, merged_sha, merged_sha AS sha, 'branch/' || branch AS reference FROM task_merges
UNION ALL
SELECT task_id, sandbox_id, merged_sha, head_sha, 'pr/' || pr_number FROM task_merges WHERE pr_number IS NOT NULL;

CREATE VIEW task_merge_ci AS
SELECT m.task_id, m.merged_sha, r.id AS run_id, r.queued_at AS at, r.rowid AS seq,
    json_object('run_id', r.id, 'kind', 'ci', 'reference', r.reference, 'sha', r.sha,
        'state', r.state, 'reason', r.reason, 'checks', (
        SELECT json_group_array(json_object('name', name, 'state', state, 'reason', reason) ORDER BY ordinal)
        FROM ci_jobs WHERE run_id = r.id)) AS result
FROM task_merge_ci_refs m JOIN ci_runs r
    ON r.sandbox_id = m.sandbox_id AND r.sha = m.sha AND r.reference = m.reference;

-- The CD wrapper exits zero only after each configured command succeeded. On other outcomes
-- use the recorded phase span where available. A rollback or drill runs a different contract.
CREATE VIEW task_merge_cd AS
SELECT m.task_id, m.merged_sha, r.id AS run_id, r.started_at AS at, r.rowid AS seq,
    json_object('run_id', r.id, 'kind', 'cd', 'reference', r.environment, 'sha', r.sha,
        'branch', r.branch, 'stage', r.stage, 'state', r.state, 'reason', r.reason, 'checks', (
        SELECT json_group_array(json_object('name', phase, 'state', CASE
            WHEN nullif(trim(CASE phase WHEN 'deploy' THEN r.deploy ELSE r.verify END), '') IS NULL THEN 'not configured'
            WHEN r.state = 'passed' AND r.exit_code = 0 AND r.ended_at IS NOT NULL THEN 'passed'
            ELSE coalesce((SELECT outcome FROM spans WHERE subject_kind = 'cd_run'
                AND subject_id = r.id AND key = 'run/' || phase), 'unconfirmed') END))
        FROM (SELECT 'deploy' AS phase UNION ALL SELECT 'verify'))) AS result
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
    INSERT INTO task_merge_archived_runs SELECT * FROM task_merge_cd WHERE run_id = OLD.id
    ON CONFLICT DO NOTHING;
END;
-- The newest deployment can outlive its phase spans. Freeze the result before the first
-- contributing span is removed, so that the stable link keeps the observed phase outcomes.
CREATE TRIGGER archive_task_merge_cd_phase BEFORE DELETE ON spans
WHEN OLD.subject_kind = 'cd_run' AND OLD.key IN ('run/deploy', 'run/verify') BEGIN
    INSERT INTO task_merge_archived_runs SELECT * FROM task_merge_cd
    WHERE run_id = OLD.subject_id AND EXISTS (
        SELECT 1 FROM cd_runs WHERE id = OLD.subject_id AND ended_at IS NOT NULL)
    ON CONFLICT DO NOTHING;
END;
CREATE VIEW task_merge_runs AS
    SELECT * FROM task_merge_ci UNION ALL SELECT * FROM task_merge_cd cd
    WHERE NOT EXISTS (SELECT 1 FROM task_merge_archived_runs a
        WHERE a.task_id = cd.task_id AND a.merged_sha = cd.merged_sha AND a.run_id = cd.run_id)
    UNION ALL SELECT * FROM task_merge_archived_runs
    UNION ALL
    SELECT m.task_id, m.merged_sha, r.id, r.started_at, r.rowid,
        json_object('run_id', r.id, 'kind', 'environment command', 'reference', r.environment,
            'sha', r.commit_sha, 'state', r.state, 'checks', json_array(
                json_object('name', 'command', 'state', r.state)))
    FROM task_merges m JOIN environment_runs r ON r.sandbox_id = m.sandbox_id AND r.commit_sha = m.merged_sha
    WHERE r.kind = 'command';
