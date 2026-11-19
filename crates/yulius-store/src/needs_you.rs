//! What a task's agent asked a person to do (SPEC-134 CDT-004, column `tasks.needs_you`).

use async_trait::async_trait;
use rusqlite::{OptionalExtension, params};
use yulius_app::error::{AppError, AppResult};
#[cfg(test)]
use yulius_app::needs_you::Named;
use yulius_app::needs_you::NeedsYouRecord;
use yulius_app::ports::store_ext::NeedsYouStore;
use yulius_domain::ids::{SandboxId, TaskId};
use yulius_domain::time::Timestamp;

use crate::{SqliteStore, bad_column, db};

#[async_trait]
impl NeedsYouStore for SqliteStore {
    async fn task_needs_you(&self, task: &TaskId) -> AppResult<Option<NeedsYouRecord>> {
        let task = task.as_str().to_owned();
        let text: Option<String> = self
            .call(move |c| {
                c.query_row("SELECT needs_you FROM tasks WHERE id = ?1", [task], |row| {
                    row.get::<_, Option<String>>(0)
                })
                .optional()
                .map(Option::flatten)
                .map_err(db)
            })
            .await?;
        text.map(|text| serde_json::from_str(&text).map_err(|_| db(bad_column("needs_you", &text))))
            .transpose()
    }

    async fn set_task_needs_you(
        &self,
        task: &TaskId,
        request: Option<&NeedsYouRecord>,
        at: Timestamp,
    ) -> AppResult<bool> {
        let task = task.as_str().to_owned();
        let text = request
            .map(serde_json::to_string)
            .transpose()
            .map_err(AppError::internal)?;
        self.call(move |c| {
            let changed = c
                .execute(
                    // `updated_at` dates the change: the request's span ends then (SPEC-137).
                    "UPDATE tasks SET needs_you = ?2, updated_at = ?3 WHERE id = ?1 AND needs_you IS NOT ?2",
                    params![task, text, at.0],
                )
                .map_err(db)?;
            Ok(changed == 1)
        })
        .await
    }

    async fn end_task_needs_you_asking(
        &self,
        task: &TaskId,
        action: &str,
        at: Timestamp,
    ) -> AppResult<bool> {
        let (task, action) = (task.as_str().to_owned(), action.to_owned());
        self.call(move |c| {
            let changed = c
                .execute(
                    // One statement: a request made after a read of this one is never ended.
                    // CASE orders the checks: a row that is not JSON is left alone, not an error.
                    "UPDATE tasks SET needs_you = NULL, updated_at = ?3 \
                     WHERE id = ?1 AND CASE WHEN json_valid(needs_you) \
                       THEN json_extract(needs_you, '$.action') END = ?2",
                    params![task, action, at.0],
                )
                .map_err(db)?;
            Ok(changed == 1)
        })
        .await
    }

    async fn end_task_needs_you_at(
        &self,
        task: &TaskId,
        asked_at: i64,
        at: Timestamp,
    ) -> AppResult<bool> {
        let task = task.as_str().to_owned();
        self.call(move |c| {
            let changed = c
                .execute(
                    // One statement: a request asked after the answered one is never ended.
                    "UPDATE tasks SET needs_you = NULL, updated_at = ?3 \
                     WHERE id = ?1 AND CASE WHEN json_valid(needs_you) \
                       THEN json_extract(needs_you, '$.at') END = ?2",
                    params![task, asked_at, at.0],
                )
                .map_err(db)?;
            Ok(changed == 1)
        })
        .await
    }

    async fn replace_task_needs_you(
        &self,
        task: &TaskId,
        read: &NeedsYouRecord,
        next: &NeedsYouRecord,
        at: Timestamp,
    ) -> AppResult<bool> {
        let task = task.as_str().to_owned();
        let next = serde_json::to_string(next).map_err(AppError::internal)?;
        let who = serde_json::to_string(&read.named.who).map_err(AppError::internal)?;
        let (asked_at, action) = (read.at, read.action.clone());
        self.call(move |c| {
            let changed = c
                .execute(
                    // One statement: a request made, ended or changed after the read is left
                    // as it is. An absent `who` is the empty list it was written from.
                    "UPDATE tasks SET needs_you = ?2, updated_at = ?3 \
                     WHERE id = ?1 AND CASE WHEN json_valid(needs_you) THEN \
                       json_extract(needs_you, '$.at') = ?4 \
                       AND json_extract(needs_you, '$.action') = ?5 \
                       AND IFNULL(json_extract(needs_you, '$.who'), '[]') = ?6 END",
                    params![task, next, at.0, asked_at, action, who],
                )
                .map_err(db)?;
            Ok(changed == 1)
        })
        .await
    }

    async fn needs_you_tasks(&self, sandbox: &SandboxId) -> AppResult<Vec<TaskId>> {
        let sandbox = sandbox.as_str().to_owned();
        self.call(move |c| {
            let mut stmt = c
                .prepare("SELECT id FROM tasks WHERE sandbox_id = ?1 AND needs_you IS NOT NULL")
                .map_err(db)?;
            let ids = stmt
                .query_map([sandbox], |row| {
                    let id: String = row.get(0)?;
                    TaskId::parse(&id).map_err(|_| bad_column("task id", &id))
                })
                .map_err(db)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(db)?;
            Ok(ids)
        })
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// SPEC-512 ANS-005 (review of part 2): a withdrawal replaces the request only while it is
    /// the one read, with the people it named then: a new request, one ended meanwhile, or
    /// another withdrawal of the same request is never overwritten.
    #[tokio::test]
    async fn a_request_is_replaced_only_while_it_is_the_one_read() -> TestResult {
        let store = SqliteStore::open_in_memory()?;
        store
            .call(|c| {
                c.execute_batch(
                    "INSERT INTO users (id, display_name, created_at) VALUES ('u1', 'U', 1);
                     INSERT INTO sandboxes (id, slug, repo_owner, repo_name, default_branch, profile, port_base, port_count, created_by, onboarding_state, created_at)
                       VALUES ('s1', 's', 'o', 'r', 'main', '{}', 20000, 10, 'u1', 'not_needed', 1);
                     INSERT INTO tasks (id, sandbox_id, number, title, prompt, harness, status, origin, created_by, created_at, updated_at, owner_id)
                       VALUES ('t1', 's1', 1, 'T', 'p', 'claude', 'queued', 'ui', 'u1', 100, 100, 'u1');",
                )
                .map_err(db)
            })
            .await?;
        let task = TaskId::parse("t1")?;
        let named = |action: &str, at: i64, who: &[&str]| NeedsYouRecord {
            at,
            named: Named {
                who: who.iter().map(|w| (*w).to_owned()).collect(),
                withdrawn: Vec::new(),
            },
            ..asks(action)
        };
        let read = named("Decide", 200, &["bob", "carol"]);
        store
            .set_task_needs_you(&task, Some(&read), Timestamp(200))
            .await?;
        let without_bob = named("Decide", 200, &["carol"]);
        // A new request came meanwhile: it stays.
        let newer = named("Decide again", 300, &["bob", "carol"]);
        store
            .set_task_needs_you(&task, Some(&newer), Timestamp(300))
            .await?;
        assert!(
            !store
                .replace_task_needs_you(&task, &read, &without_bob, Timestamp(310))
                .await?
        );
        assert_eq!(store.task_needs_you(&task).await?, Some(newer.clone()));
        // The same action asked again later is another request: it stays.
        let same_later = named("Decide", 250, &["bob", "carol"]);
        store
            .set_task_needs_you(&task, Some(&same_later), Timestamp(305))
            .await?;
        assert!(
            !store
                .replace_task_needs_you(&task, &read, &without_bob, Timestamp(306))
                .await?
        );
        store
            .set_task_needs_you(&task, Some(&newer), Timestamp(307))
            .await?;
        // The one read, still as read: replaced once.
        let without_carol = named("Decide again", 300, &["bob"]);
        assert!(
            store
                .replace_task_needs_you(&task, &newer, &without_carol, Timestamp(320))
                .await?
        );
        assert_eq!(store.task_needs_you(&task).await?, Some(without_carol));
        // A second withdrawal from the same read finds its people changed: refused.
        let without_bob_again = named("Decide again", 300, &["carol"]);
        assert!(
            !store
                .replace_task_needs_you(&task, &newer, &without_bob_again, Timestamp(330))
                .await?
        );
        // Ended meanwhile: nothing comes back.
        store
            .set_task_needs_you(&task, None, Timestamp(340))
            .await?;
        assert!(
            !store
                .replace_task_needs_you(&task, &newer, &without_carol_of(&newer), Timestamp(350))
                .await?
        );
        assert_eq!(store.task_needs_you(&task).await?, None);
        Ok(())
    }

    /// SPEC-512 ANS-004 (review of part 2): an answer ends the request it answered only: one
    /// asked after it, even with the same action, stays.
    #[tokio::test]
    async fn an_answer_ends_only_the_request_asked_at_its_time() -> TestResult {
        let store = SqliteStore::open_in_memory()?;
        store
            .call(|c| {
                c.execute_batch(
                    "INSERT INTO users (id, display_name, created_at) VALUES ('u1', 'U', 1);
                     INSERT INTO sandboxes (id, slug, repo_owner, repo_name, default_branch, profile, port_base, port_count, created_by, onboarding_state, created_at)
                       VALUES ('s1', 's', 'o', 'r', 'main', '{}', 20000, 10, 'u1', 'not_needed', 1);
                     INSERT INTO tasks (id, sandbox_id, number, title, prompt, harness, status, origin, created_by, created_at, updated_at, owner_id)
                       VALUES ('t1', 's1', 1, 'T', 'p', 'claude', 'queued', 'ui', 'u1', 100, 100, 'u1');",
                )
                .map_err(db)
            })
            .await?;
        let task = TaskId::parse("t1")?;
        let at = |at: i64| NeedsYouRecord {
            at,
            ..asks("Decide")
        };
        store
            .set_task_needs_you(&task, Some(&at(300)), Timestamp(300))
            .await?;
        assert!(
            !store
                .end_task_needs_you_at(&task, 200, Timestamp(310))
                .await?
        );
        assert_eq!(store.task_needs_you(&task).await?, Some(at(300)));
        assert!(
            store
                .end_task_needs_you_at(&task, 300, Timestamp(320))
                .await?
        );
        assert_eq!(store.task_needs_you(&task).await?, None);
        Ok(())
    }

    fn without_carol_of(record: &NeedsYouRecord) -> NeedsYouRecord {
        let mut next = record.clone();
        next.named.who.retain(|w| w != "carol");
        next
    }

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    fn asks(action: &str) -> NeedsYouRecord {
        NeedsYouRecord {
            action: action.into(),
            happened: None,
            problem: None,
            decision: false,
            link: None,
            at: 200,
            cause: None,
            named: Default::default(),
        }
    }

    /// SPEC-469 GPW-003: a request ends by its action only while it is the open one. Another
    /// request, made meanwhile or open all along, stays; with none open nothing changes.
    #[tokio::test]
    async fn a_request_ends_by_its_action_only_while_it_is_the_open_one() -> TestResult {
        let store = SqliteStore::open_in_memory()?;
        store
            .call(|c| {
                c.execute_batch(
                    "INSERT INTO users (id, display_name, created_at) VALUES ('u1', 'U', 1);
                     INSERT INTO sandboxes (id, slug, repo_owner, repo_name, default_branch, profile, port_base, port_count, created_by, onboarding_state, created_at)
                       VALUES ('s1', 's', 'o', 'r', 'main', '{}', 20000, 10, 'u1', 'not_needed', 1);
                     INSERT INTO tasks (id, sandbox_id, number, title, prompt, harness, status, origin, created_by, created_at, updated_at, owner_id)
                       VALUES ('t1', 's1', 1, 'T', 'p', 'claude', 'queued', 'ui', 'u1', 100, 100, 'u1');",
                )
                .map_err(db)
            })
            .await?;
        let task = TaskId::parse("t1")?;
        let ended = |action: &'static str, at: i64| {
            let (store, task) = (&store, &task);
            async move {
                store
                    .end_task_needs_you_asking(task, action, Timestamp(at))
                    .await
            }
        };
        assert!(!ended("Send a message", 300).await?, "none is open");
        store
            .set_task_needs_you(&task, Some(&asks("Add DEV_HOST")), Timestamp(200))
            .await?;
        assert!(!ended("Send a message", 300).await?, "another's stays");
        assert_eq!(
            store.task_needs_you(&task).await?,
            Some(asks("Add DEV_HOST"))
        );
        store
            .set_task_needs_you(&task, Some(&asks("Send a message")), Timestamp(400))
            .await?;
        assert!(ended("Send a message", 500).await?);
        assert_eq!(store.task_needs_you(&task).await?, None);
        assert!(!ended("Send a message", 600).await?, "ended once");
        // Its wait is measured like one a person answered (SPEC-137): the span ends then.
        let waits: Vec<(i64, Option<i64>, Option<String>)> = store
            .call(|c| {
                let mut stmt = c
                    .prepare(
                        "SELECT started_at, ended_at, outcome FROM spans \
                         WHERE subject_id = 't1' AND name = 'needs_you' ORDER BY id",
                    )
                    .map_err(db)?;
                let rows = stmt
                    .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
                    .map_err(db)?
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(db)?;
                Ok(rows)
            })
            .await?;
        assert_eq!(
            waits.last(),
            Some(&(200, Some(500), Some("answered".to_owned()))),
            "{waits:?}"
        );
        // A row that is not JSON is left alone, without an error.
        store
            .call(|c| {
                c.execute(
                    "UPDATE tasks SET needs_you = 'not json' WHERE id = 't1'",
                    [],
                )
                .map_err(db)
            })
            .await?;
        assert!(!ended("Send a message", 700).await?);
        Ok(())
    }
}
