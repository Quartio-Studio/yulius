CREATE TABLE task_pull_observations (
    task_id TEXT NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
    pr_number INTEGER NOT NULL,
    snapshot TEXT NOT NULL,
    needs_confirmation INTEGER NOT NULL,
    PRIMARY KEY (task_id, pr_number)
);
-- A recorded merge establishes the number even when landing evidence was unavailable.
-- Historical rows have no PR title or source branch. Only a bound landing supplies SHAs.
INSERT INTO task_pull_observations
SELECT d.task_id, d.pr_number, json_object(
    'number', d.pr_number, 'html_url', 'https://github.com/' || s.repo_owner || '/' || s.repo_name || '/pull/' || d.pr_number,
    'title', 'Pull request #' || d.pr_number, 'state', 'closed', 'merged', json('true'),
    'base', coalesce(m.branch, ''), 'base_sha', '', 'head', '', 'head_sha', coalesce(m.head_sha, ''),
    'mergeable', NULL, 'behind', json('false'), 'from_fork', json('false'), 'merge_commit_sha', m.merged_sha), m.merged_sha IS NULL
FROM delivery d JOIN tasks t ON t.id=d.task_id JOIN sandboxes s ON s.id=t.sandbox_id
LEFT JOIN task_merges m ON m.task_id=d.task_id AND m.pr_number=d.pr_number
WHERE d.stage NOT IN ('none', 'pr_open') AND d.pr_number IS NOT NULL
ON CONFLICT DO NOTHING;
