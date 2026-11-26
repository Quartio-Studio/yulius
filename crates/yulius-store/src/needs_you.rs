//! What a task's agent asked a person to do (SPEC-134 CDT-004, column `tasks.needs_you`).

use async_trait::async_trait;
use rusqlite::{OptionalExtension, params};
use yulius_app::attention::AttentionRecord;
use yulius_app::error::{AppError, AppResult};
#[cfg(test)]
use yulius_app::needs_you::Named;
use yulius_app::needs_you::NeedsYouRecord;
use yulius_app::ports::store_ext::{NeedsYouStore, WaitMessageRecord};
use yulius_domain::ids::{SandboxId, TaskId};
use yulius_domain::time::Timestamp;

use crate::{SqliteStore, bad_column, opt_ts, text_parsed};

#[async_trait]
impl NeedsYouStore for SqliteStore {
    async fn task_needs_you(&self, task: &TaskId) -> AppResult<Option<NeedsYouRecord>> {
        let task = task.as_str().to_owned();
        self.call(move |c| {
            let text: Option<String> = c
                .query_row("SELECT needs_you FROM tasks WHERE id = ?1", [task], |row| {
                    row.get(0)
                })
                .optional()?
                .flatten();
            text.map(|text| {
                serde_json::from_str(&text).map_err(|_| bad_column("needs_you", &text).into())
            })
            .transpose()
        })
        .await
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
                )?;
            Ok(changed == 1)
        })
        .await
    }

    async fn ask_for_live_attempt(
        &self,
        task: &TaskId,
        attempt: &yulius_domain::ids::AttemptId,
        request: &NeedsYouRecord,
    ) -> AppResult<bool> {
        let (task, attempt) = (task.to_string(), attempt.to_string());
        let text = serde_json::to_string(request).map_err(AppError::internal)?;
        let at = request.at;
        self.call(move |c| {
            Ok(c.execute(
                "UPDATE tasks SET needs_you = ?3, updated_at = ?4 WHERE id = ?1 \
                 AND status = 'running' AND needs_you IS NULL AND EXISTS \
                 (SELECT 1 FROM attempts WHERE task_id = ?1 AND id = ?2 AND state = 'live')",
                params![task, attempt, text, at],
            )
            .map(|changed| changed == 1)?)
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
            let changed = c.execute(
                // One statement: a request made after a read of this one is never ended.
                // CASE orders the checks: a row that is not JSON is left alone, not an error.
                "UPDATE tasks SET needs_you = NULL, updated_at = ?3 \
                     WHERE id = ?1 AND CASE WHEN json_valid(needs_you) \
                       THEN json_extract(needs_you, '$.action') END = ?2",
                params![task, action, at.0],
            )?;
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
            let changed = c.execute(
                // One statement: a request asked after the answered one is never ended.
                "UPDATE tasks SET needs_you = NULL, updated_at = ?3 \
                     WHERE id = ?1 AND CASE WHEN json_valid(needs_you) \
                       THEN json_extract(needs_you, '$.at') END = ?2",
                params![task, asked_at, at.0],
            )?;
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
            let changed = c.execute(
                // One statement: a request made, ended or changed after the read is left
                // as it is. An absent `who` is the empty list it was written from.
                "UPDATE tasks SET needs_you = ?2, updated_at = ?3 \
                     WHERE id = ?1 AND CASE WHEN json_valid(needs_you) THEN \
                       json_extract(needs_you, '$.at') = ?4 \
                       AND json_extract(needs_you, '$.action') = ?5 \
                       AND IFNULL(json_extract(needs_you, '$.who'), '[]') = ?6 END",
                params![task, next, at.0, asked_at, action, who],
            )?;
            Ok(changed == 1)
        })
        .await
    }

    async fn needs_you_tasks(&self, sandbox: &SandboxId) -> AppResult<Vec<TaskId>> {
        let sandbox = sandbox.as_str().to_owned();
        self.call(move |c| {
            let mut stmt =
                c.prepare("SELECT id FROM tasks WHERE sandbox_id = ?1 AND needs_you IS NOT NULL")?;
            let ids = stmt
                .query_map([sandbox], |row| {
                    text_parsed(row, 0, "task id", TaskId::parse)
                })?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(ids)
        })
        .await
    }

    async fn record_wait_message(&self, entry: &WaitMessageRecord) -> AppResult<bool> {
        let entry = entry.clone();
        self.call(move |c| {
            let tx = c.transaction()?;
            tx.execute(
                "UPDATE task_wait_messages SET resolved_at = ?2 \
                 WHERE task_id = ?1 AND episode != ?3 AND resolved_at IS NULL",
                params![entry.task_id.as_str(), entry.created_at.0, entry.episode],
            )?;
            let inserted = tx
                .execute(
                    "INSERT OR IGNORE INTO task_wait_messages \
                     (task_id, episode, source, happened, problem, action, link, decision, created_at, resolved_at) \
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                    params![
                        entry.task_id.as_str(),
                        entry.episode,
                        entry.source,
                        entry.happened,
                        entry.problem,
                        entry.action,
                        entry.link,
                        i64::from(entry.decision),
                        entry.created_at.0,
                        entry.resolved_at.map(|t| t.0),
                    ],
                )?;
            tx.commit()?;
            Ok(inserted == 1)
        })
        .await
    }

    async fn resolve_wait_messages(&self, task: &TaskId, at: Timestamp) -> AppResult<bool> {
        let task = task.as_str().to_owned();
        self.call(move |c| {
            let changed = c.execute(
                "UPDATE task_wait_messages SET resolved_at = ?2 \
                     WHERE task_id = ?1 AND resolved_at IS NULL",
                params![task, at.0],
            )?;
            Ok(changed > 0)
        })
        .await
    }

    async fn task_wait_messages(
        &self,
        task: &TaskId,
        limit: u32,
    ) -> AppResult<Vec<WaitMessageRecord>> {
        let task = task.as_str().to_owned();
        self.call(move |c| {
            let mut stmt = c.prepare(
                "SELECT task_id, episode, source, happened, problem, action, link, decision, \
                     created_at, resolved_at \
                     FROM task_wait_messages WHERE task_id = ?1 \
                     ORDER BY created_at DESC, id DESC LIMIT ?2",
            )?;
            let rows = stmt
                .query_map(params![task, i64::from(limit)], |row| {
                    let task_id: String = row.get(0)?;
                    let decision: i64 = row.get(7)?;
                    Ok(WaitMessageRecord {
                        task_id: TaskId::parse(&task_id)
                            .map_err(|_| bad_column("task_id", &task_id))?,
                        episode: row.get(1)?,
                        source: row.get(2)?,
                        happened: row.get(3)?,
                        problem: row.get(4)?,
                        action: row.get(5)?,
                        link: row.get(6)?,
                        decision: decision != 0,
                        created_at: Timestamp(row.get(8)?),
                        resolved_at: opt_ts(row, 9)?,
                    })
                })?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(rows)
        })
        .await
    }

    async fn stored_task_attention(
        &self,
        task: &TaskId,
    ) -> AppResult<Option<(AttentionRecord, Timestamp)>> {
        let task = task.as_str().to_owned();
        let row: Option<(String, Option<i64>, i64)> = self
            .call(move |c| {
                Ok(c.query_row(
                    "SELECT attention, \
                       CASE WHEN json_valid(attention) THEN json_extract(attention, '$.at') END, \
                       COALESCE(state_at, updated_at) \
                     FROM tasks WHERE id = ?1 AND attention IS NOT NULL",
                    [task],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                )
                .optional()?)
            })
            .await?;
        Ok(row.and_then(|(text, at, fallback)| {
            Some((
                serde_json::from_str(&text).ok()?,
                Timestamp(at.unwrap_or(fallback)),
            ))
        }))
    }

    async fn clear_stored_task_attention(&self, task: &TaskId) -> AppResult<bool> {
        let task = task.as_str().to_owned();
        self.call(move |c| {
            let changed = c.execute(
                "UPDATE tasks SET attention = NULL WHERE id = ?1 AND attention IS NOT NULL",
                [task],
            )?;
            Ok(changed == 1)
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
                Ok(c.execute_batch(
                    "INSERT INTO users (id, display_name, created_at) VALUES ('u1', 'U', 1);
                     INSERT INTO sandboxes (id, slug, repo_owner, repo_name, default_branch, profile, port_base, port_count, created_by, onboarding_state, created_at)
                       VALUES ('s1', 's', 'o', 'r', 'main', '{}', 20000, 10, 'u1', 'not_needed', 1);
                     INSERT INTO tasks (id, sandbox_id, number, title, prompt, harness, status, origin, created_by, created_at, updated_at, owner_id)
                       VALUES ('t1', 's1', 1, 'T', 'p', 'claude', 'queued', 'ui', 'u1', 100, 100, 'u1');",
                )?)
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
                Ok(c.execute_batch(
                    "INSERT INTO users (id, display_name, created_at) VALUES ('u1', 'U', 1);
                     INSERT INTO sandboxes (id, slug, repo_owner, repo_name, default_branch, profile, port_base, port_count, created_by, onboarding_state, created_at)
                       VALUES ('s1', 's', 'o', 'r', 'main', '{}', 20000, 10, 'u1', 'not_needed', 1);
                     INSERT INTO tasks (id, sandbox_id, number, title, prompt, harness, status, origin, created_by, created_at, updated_at, owner_id)
                       VALUES ('t1', 's1', 1, 'T', 'p', 'claude', 'queued', 'ui', 'u1', 100, 100, 'u1');",
                )?)
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
                Ok(c.execute_batch(
                    "INSERT INTO users (id, display_name, created_at) VALUES ('u1', 'U', 1);
                     INSERT INTO sandboxes (id, slug, repo_owner, repo_name, default_branch, profile, port_base, port_count, created_by, onboarding_state, created_at)
                       VALUES ('s1', 's', 'o', 'r', 'main', '{}', 20000, 10, 'u1', 'not_needed', 1);
                     INSERT INTO tasks (id, sandbox_id, number, title, prompt, harness, status, origin, created_by, created_at, updated_at, owner_id)
                       VALUES ('t1', 's1', 1, 'T', 'p', 'claude', 'queued', 'ui', 'u1', 100, 100, 'u1');",
                )?)
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
                let mut stmt = c.prepare(
                    "SELECT started_at, ended_at, outcome FROM spans \
                         WHERE subject_id = 't1' AND name = 'needs_you' ORDER BY id",
                )?;
                let rows = stmt
                    .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?
                    .collect::<Result<Vec<_>, _>>()?;
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
                Ok(c.execute(
                    "UPDATE tasks SET needs_you = 'not json' WHERE id = 't1'",
                    [],
                )?)
            })
            .await?;
        assert!(!ended("Send a message", 700).await?);
        Ok(())
    }

    /// SPEC-578: repeated polls of the same wait episode keep a single timestamped record, a
    /// distinct later episode resolves the prior one and inserts a new row, and resolving clears
    /// open episodes idempotently.
    #[tokio::test]
    async fn wait_messages_deduplicate_per_episode_and_record_distinct_waits() -> TestResult {
        let store = SqliteStore::open_in_memory()?;
        store
            .call(|c| {
                Ok(c.execute_batch(
                    "INSERT INTO users (id, display_name, created_at) VALUES ('u1', 'U', 1);
                     INSERT INTO sandboxes (id, slug, repo_owner, repo_name, default_branch, profile, port_base, port_count, created_by, onboarding_state, created_at)
                       VALUES ('s1', 's', 'o', 'r', 'main', '{}', 20000, 10, 'u1', 'not_needed', 1);
                     INSERT INTO tasks (id, sandbox_id, number, title, prompt, harness, status, origin, created_by, created_at, updated_at, owner_id)
                       VALUES ('t1', 's1', 1, 'T', 'p', 'claude', 'running', 'ui', 'u1', 100, 100, 'u1');",
                )?)
            })
            .await?;
        let task = TaskId::parse("t1")?;
        let ep1 = WaitMessageRecord {
            task_id: task.clone(),
            episode: "plan_open:5:1000".into(),
            source: "system".into(),
            happened: "Stopped with 5 open steps.".into(),
            problem: None,
            action: "Reply to continue.".into(),
            link: "https://y.example/s/s/t/1".into(),
            decision: false,
            created_at: Timestamp(1000),
            resolved_at: None,
        };
        assert!(store.record_wait_message(&ep1).await?);
        // Repeated polls of the same episode do not create duplicates.
        assert!(!store.record_wait_message(&ep1).await?);
        assert!(!store.record_wait_message(&ep1).await?);
        let rows = store.task_wait_messages(&task, 10).await?;
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].episode, "plan_open:5:1000");
        assert_eq!(rows[0].resolved_at, None);

        // A distinct later episode resolves the previous open episode and inserts a new row.
        let ep2 = WaitMessageRecord {
            task_id: task.clone(),
            episode: "browser:t1:2000".into(),
            source: "browser".into(),
            happened: "Sign in to Google Calendar.".into(),
            problem: None,
            action: "Open the browser tab.".into(),
            link: "https://y.example/s/s/browser?tab=t1".into(),
            decision: false,
            created_at: Timestamp(2000),
            resolved_at: None,
        };
        assert!(store.record_wait_message(&ep2).await?);
        let rows = store.task_wait_messages(&task, 10).await?;
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].episode, "browser:t1:2000");
        assert_eq!(rows[0].resolved_at, None);
        assert_eq!(rows[1].episode, "plan_open:5:1000");
        assert_eq!(rows[1].resolved_at, Some(Timestamp(2000)));

        // Resolving stamps the open episode once and is idempotent on repeat calls.
        assert!(store.resolve_wait_messages(&task, Timestamp(3000)).await?);
        assert!(!store.resolve_wait_messages(&task, Timestamp(4000)).await?);
        let rows = store.task_wait_messages(&task, 10).await?;
        assert_eq!(rows[0].resolved_at, Some(Timestamp(3000)));
        Ok(())
    }
}
