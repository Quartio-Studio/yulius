//! What a task's agent asked a person to do (SPEC-134 CDT-004, column `tasks.needs_you`).

use async_trait::async_trait;
use rusqlite::{OptionalExtension, params};
use yulius_app::error::{AppError, AppResult};
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
