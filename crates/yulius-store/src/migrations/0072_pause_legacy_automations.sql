-- Pre-upgrade rows record the creator but cannot establish who authorized the saved prompt.
UPDATE automations SET enabled = 0 WHERE enabled = 1;

-- Preserve the outcome if task creation committed just before shutdown. The unique
-- tasks.automation_run key is the same key the scheduler uses for crash recovery.
UPDATE automation_runs SET task_id = (
    SELECT id FROM tasks WHERE automation_run = automation_runs.automation_id || '@' || automation_runs.scheduled_for
) WHERE state = 'scheduled';
UPDATE automation_runs SET state = 'task_created' WHERE state = 'scheduled' AND task_id IS NOT NULL;
UPDATE automation_runs SET state = 'skipped_disabled', error = 'Schedule paused on upgrade for creator review'
WHERE state = 'scheduled' AND task_id IS NULL;
