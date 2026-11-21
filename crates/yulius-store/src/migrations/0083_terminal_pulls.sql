-- Keep the directly observed terminal PR even after a closed delivery returns to 'none'.
CREATE TABLE terminal_pulls (
    task_id TEXT NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
    pr_number INTEGER NOT NULL,
    snapshot TEXT NOT NULL,
    PRIMARY KEY (task_id, pr_number)
);
-- A verified historical landing carries its original branch and accepted head.
INSERT INTO terminal_pulls
SELECT m.task_id, m.pr_number, json_object(
    'number', m.pr_number, 'html_url', 'https://github.com/' || s.repo_owner || '/' || s.repo_name || '/pull/' || m.pr_number,
    'title', t.title, 'state', 'closed', 'merged', json('true'), 'base', m.branch, 'base_sha', '',
    'head', coalesce(t.branch, ''), 'head_sha', m.head_sha, 'mergeable', NULL, 'behind', json('false'),
    'from_fork', json('false'), 'merge_commit_sha', m.merged_sha)
FROM task_merges m JOIN tasks t ON t.id = m.task_id JOIN sandboxes s ON s.id = t.sandbox_id
JOIN delivery d ON d.task_id = t.id AND d.pr_number = m.pr_number
WHERE d.stage NOT IN ('none', 'pr_open') AND m.pr_number IS NOT NULL
ON CONFLICT DO NOTHING;
