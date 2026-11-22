//! Brokered commands (SPEC-626, migration 0089): a project's command definitions.

use async_trait::async_trait;
use rusqlite::{OptionalExtension, Row, params};
use yulius_app::error::{AppError, AppResult};
use yulius_app::ports::store_ext::{CommandRecord, CommandRunEnd, CommandRunRecord, CommandStore};
use yulius_domain::ids::{SandboxId, TaskId};
use yulius_domain::time::Timestamp;

use crate::{SqliteStore, bad_column, db};

const COLUMNS: &str =
    "id, sandbox_id, name, definition, secret_id, created_by, created_at, updated_by, updated_at";

fn command_row(row: &Row<'_>) -> rusqlite::Result<CommandRecord> {
    let sandbox: String = row.get(1)?;
    Ok(CommandRecord {
        id: row.get(0)?,
        sandbox_id: SandboxId::parse(&sandbox).map_err(|_| bad_column("sandbox id", &sandbox))?,
        name: row.get(2)?,
        definition: row.get(3)?,
        secret_id: row.get(4)?,
        created_by: row.get(5)?,
        created_at: Timestamp(row.get(6)?),
        updated_by: row.get(7)?,
        updated_at: Timestamp(row.get(8)?),
    })
}

/// A unique name taken: said as such, not as "conflicting record"; any other failure as `db`
/// says it.
fn named(err: rusqlite::Error) -> AppError {
    let unique = matches!(
        &err,
        rusqlite::Error::SqliteFailure(code, _)
            if code.extended_code == rusqlite::ffi::SQLITE_CONSTRAINT_UNIQUE
    );
    if unique {
        return AppError::Conflict("this project already has a command of that name".into());
    }
    db(err)
}

const RUN_COLUMNS: &str = "id, sandbox_id, command_id, command, command_updated_at, task_id, \
    task_number, args, line, changes, state, reason, deadline, answered_by, exit_code, stdout, \
    stderr, created_at, started_at, ended_at";

/// The ended runs a project keeps (SPEC-626 BRK-011).
const KEEP_RUNS: i64 = 200;

fn run_row(row: &Row<'_>) -> rusqlite::Result<CommandRunRecord> {
    let sandbox: String = row.get(1)?;
    let task: String = row.get(5)?;
    let args: String = row.get(7)?;
    Ok(CommandRunRecord {
        id: row.get(0)?,
        sandbox_id: SandboxId::parse(&sandbox).map_err(|_| bad_column("sandbox id", &sandbox))?,
        command_id: row.get(2)?,
        command: row.get(3)?,
        command_updated_at: Timestamp(row.get(4)?),
        task_id: TaskId::parse(&task).map_err(|_| bad_column("task id", &task))?,
        task_number: row.get(6)?,
        args: serde_json::from_str(&args).map_err(|_| bad_column("args", &args))?,
        line: row.get(8)?,
        changes: row.get(9)?,
        state: row.get(10)?,
        reason: row.get(11)?,
        deadline: row.get::<_, Option<i64>>(12)?.map(Timestamp),
        answered_by: row.get(13)?,
        exit_code: row.get(14)?,
        stdout: row.get(15)?,
        stderr: row.get(16)?,
        created_at: Timestamp(row.get(17)?),
        started_at: row.get::<_, Option<i64>>(18)?.map(Timestamp),
        ended_at: row.get::<_, Option<i64>>(19)?.map(Timestamp),
    })
}

#[async_trait]
impl CommandStore for SqliteStore {
    async fn commands_in(&self, sandbox: &SandboxId) -> AppResult<Vec<CommandRecord>> {
        let sandbox = sandbox.as_str().to_owned();
        self.call(move |c| {
            let mut stmt = c
                .prepare(&format!(
                    "SELECT {COLUMNS} FROM commands WHERE sandbox_id = ?1 ORDER BY name"
                ))
                .map_err(db)?;
            let rows = stmt
                .query_map([sandbox], command_row)
                .map_err(db)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(db)?;
            Ok(rows)
        })
        .await
    }

    async fn command(&self, sandbox: &SandboxId, id: &str) -> AppResult<Option<CommandRecord>> {
        let (sandbox, id) = (sandbox.as_str().to_owned(), id.to_owned());
        self.call(move |c| {
            c.query_row(
                &format!("SELECT {COLUMNS} FROM commands WHERE sandbox_id = ?1 AND id = ?2"),
                [sandbox, id],
                command_row,
            )
            .optional()
            .map_err(db)
        })
        .await
    }

    async fn command_named(
        &self,
        sandbox: &SandboxId,
        name: &str,
    ) -> AppResult<Option<CommandRecord>> {
        let (sandbox, name) = (sandbox.as_str().to_owned(), name.to_owned());
        self.call(move |c| {
            c.query_row(
                &format!("SELECT {COLUMNS} FROM commands WHERE sandbox_id = ?1 AND name = ?2"),
                [sandbox, name],
                command_row,
            )
            .optional()
            .map_err(db)
        })
        .await
    }

    async fn insert_command(&self, command: &CommandRecord) -> AppResult<()> {
        let r = command.clone();
        self.call(move |c| {
            c.execute(
                &format!(
                    "INSERT INTO commands ({COLUMNS}) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)"
                ),
                params![
                    r.id,
                    r.sandbox_id.as_str(),
                    r.name,
                    r.definition,
                    r.secret_id,
                    r.created_by,
                    r.created_at.0,
                    r.updated_by,
                    r.updated_at.0,
                ],
            )
            .map_err(named)?;
            Ok(())
        })
        .await
    }

    async fn update_command(&self, command: &CommandRecord, read_at: Timestamp) -> AppResult<bool> {
        let r = command.clone();
        self.call(move |c| {
            let changed = c
                .execute(
                    "UPDATE commands SET name = ?1, definition = ?2, updated_by = ?3, \
                     updated_at = ?4, secret_id = ?8 WHERE id = ?5 AND sandbox_id = ?6 \
                     AND updated_at = ?7",
                    params![
                        r.name,
                        r.definition,
                        r.updated_by,
                        r.updated_at.0,
                        r.id,
                        r.sandbox_id.as_str(),
                        read_at.0,
                        r.secret_id,
                    ],
                )
                .map_err(named)?;
            Ok(changed == 1)
        })
        .await
    }

    async fn delete_command(&self, sandbox: &SandboxId, id: &str) -> AppResult<bool> {
        let (sandbox, id) = (sandbox.as_str().to_owned(), id.to_owned());
        self.call(move |c| {
            let changed = c
                .execute(
                    "DELETE FROM commands WHERE sandbox_id = ?1 AND id = ?2",
                    [sandbox, id],
                )
                .map_err(db)?;
            Ok(changed == 1)
        })
        .await
    }

    async fn insert_command_run(&self, run: &CommandRunRecord) -> AppResult<()> {
        let r = run.clone();
        self.call(move |c| {
            let args = serde_json::to_string(&r.args).map_err(AppError::internal)?;
            let tx = c.transaction().map_err(db)?;
            tx.execute(
                &format!(
                    "INSERT INTO command_runs ({RUN_COLUMNS}) VALUES (?1, ?2, ?3, ?4, ?5, ?6, \
                     ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20)"
                ),
                params![
                    r.id,
                    r.sandbox_id.as_str(),
                    r.command_id,
                    r.command,
                    r.command_updated_at.0,
                    r.task_id.as_str(),
                    r.task_number,
                    args,
                    r.line,
                    r.changes,
                    r.state,
                    r.reason,
                    r.deadline.map(|t| t.0),
                    r.answered_by,
                    r.exit_code,
                    r.stdout,
                    r.stderr,
                    r.created_at.0,
                    r.started_at.map(|t| t.0),
                    r.ended_at.map(|t| t.0),
                ],
            )
            .map_err(db)?;
            // A project keeps its newest ended runs; a waiting or running one always stays.
            tx.execute(
                "DELETE FROM command_runs WHERE sandbox_id = ?1 AND ended_at IS NOT NULL \
                 AND id NOT IN (SELECT id FROM command_runs WHERE sandbox_id = ?1 \
                 AND ended_at IS NOT NULL ORDER BY created_at DESC, id DESC LIMIT ?2)",
                params![r.sandbox_id.as_str(), KEEP_RUNS],
            )
            .map_err(db)?;
            tx.commit().map_err(db)?;
            Ok(())
        })
        .await
    }

    async fn command_run(&self, id: &str) -> AppResult<Option<CommandRunRecord>> {
        let id = id.to_owned();
        self.call(move |c| {
            c.query_row(
                &format!("SELECT {RUN_COLUMNS} FROM command_runs WHERE id = ?1"),
                [id],
                run_row,
            )
            .optional()
            .map_err(db)
        })
        .await
    }

    async fn start_command_run(&self, id: &str, from: &str, at: Timestamp) -> AppResult<bool> {
        let (id, from) = (id.to_owned(), from.to_owned());
        self.call(move |c| {
            let changed = c
                .execute(
                    "UPDATE command_runs SET state = 'running', started_at = ?1 \
                     WHERE id = ?2 AND state = ?3",
                    params![at.0, id, from],
                )
                .map_err(db)?;
            Ok(changed == 1)
        })
        .await
    }

    async fn end_command_run(&self, id: &str, from: &str, end: &CommandRunEnd) -> AppResult<bool> {
        let (id, from, e) = (id.to_owned(), from.to_owned(), end.clone());
        self.call(move |c| {
            let changed = c
                .execute(
                    "UPDATE command_runs SET state = ?1, reason = ?2, exit_code = ?3, \
                     stdout = ?4, stderr = ?5, ended_at = ?6 WHERE id = ?7 AND state = ?8",
                    params![
                        e.state,
                        e.reason,
                        e.exit_code,
                        e.stdout,
                        e.stderr,
                        e.at.0,
                        id,
                        from
                    ],
                )
                .map_err(db)?;
            Ok(changed == 1)
        })
        .await
    }

    async fn command_runs_in_state(&self, state: &str) -> AppResult<Vec<CommandRunRecord>> {
        let state = state.to_owned();
        self.call(move |c| {
            let mut stmt = c
                .prepare(&format!(
                    "SELECT {RUN_COLUMNS} FROM command_runs WHERE state = ?1 ORDER BY created_at"
                ))
                .map_err(db)?;
            let rows = stmt
                .query_map([state], run_row)
                .map_err(db)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(db)?;
            Ok(rows)
        })
        .await
    }
}
