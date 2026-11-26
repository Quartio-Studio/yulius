//! Brokered commands (SPEC-626, migration 0089): a project's command definitions.

use async_trait::async_trait;
use rusqlite::{OptionalExtension, Row, params};
use yulius_app::error::{AppError, AppResult};
use yulius_app::ports::store_ext::{
    CommandRecord, CommandRunEnd, CommandRunRecord, CommandStore, RunInsert,
};
use yulius_domain::ids::{SandboxId, TaskId};
use yulius_domain::time::Timestamp;

use crate::{DbError, SqliteStore, opt_ts, text_parsed};

const COLUMNS: &str =
    "id, sandbox_id, name, definition, secret_id, created_by, created_at, updated_by, updated_at";

fn command_row(row: &Row<'_>) -> rusqlite::Result<CommandRecord> {
    Ok(CommandRecord {
        id: row.get(0)?,
        sandbox_id: text_parsed(row, 1, "sandbox id", SandboxId::parse)?,
        name: row.get(2)?,
        definition: row.get(3)?,
        secret_id: row.get(4)?,
        created_by: row.get(5)?,
        created_at: Timestamp(row.get(6)?),
        updated_by: row.get(7)?,
        updated_at: Timestamp(row.get(8)?),
    })
}

/// A unique name taken: said as such, not as "conflicting record"; any other failure as any
/// database error.
fn named(err: rusqlite::Error) -> DbError {
    let unique = matches!(
        &err,
        rusqlite::Error::SqliteFailure(code, _)
            if code.extended_code == rusqlite::ffi::SQLITE_CONSTRAINT_UNIQUE
    );
    if unique {
        return AppError::Conflict("this project already has a command of that name".into()).into();
    }
    err.into()
}

const RUN_COLUMNS: &str = "id, sandbox_id, command_id, command, command_updated_at, task_id, \
    task_number, args, line, changes, state, reason, deadline, answered_by, exit_code, stdout, \
    stderr, created_at, started_at, ended_at";

/// A run's columns without its output, for lists (review round 1 of slice 3): its stdout and
/// stderr read as empty.
const LISTED_COLUMNS: &str = "id, sandbox_id, command_id, command, command_updated_at, task_id, \
    task_number, args, line, changes, state, reason, deadline, answered_by, exit_code, '', '', \
    created_at, started_at, ended_at";

/// The ended runs a project keeps (SPEC-626 BRK-011).
const KEEP_RUNS: i64 = 200;

fn run_row(row: &Row<'_>) -> rusqlite::Result<CommandRunRecord> {
    Ok(CommandRunRecord {
        id: row.get(0)?,
        sandbox_id: text_parsed(row, 1, "sandbox id", SandboxId::parse)?,
        command_id: row.get(2)?,
        command: row.get(3)?,
        command_updated_at: Timestamp(row.get(4)?),
        task_id: text_parsed(row, 5, "task id", TaskId::parse)?,
        task_number: row.get(6)?,
        args: text_parsed(row, 7, "args", |text| serde_json::from_str(text))?,
        line: row.get(8)?,
        changes: row.get(9)?,
        state: row.get(10)?,
        reason: row.get(11)?,
        deadline: opt_ts(row, 12)?,
        answered_by: row.get(13)?,
        exit_code: row.get(14)?,
        stdout: row.get(15)?,
        stderr: row.get(16)?,
        created_at: Timestamp(row.get(17)?),
        started_at: opt_ts(row, 18)?,
        ended_at: opt_ts(row, 19)?,
    })
}

#[async_trait]
impl CommandStore for SqliteStore {
    async fn commands_in(&self, sandbox: &SandboxId) -> AppResult<Vec<CommandRecord>> {
        let sandbox = sandbox.as_str().to_owned();
        self.call(move |c| {
            let mut stmt = c.prepare(&format!(
                "SELECT {COLUMNS} FROM commands WHERE sandbox_id = ?1 ORDER BY name"
            ))?;
            let rows = stmt
                .query_map([sandbox], command_row)?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(rows)
        })
        .await
    }

    async fn command(&self, sandbox: &SandboxId, id: &str) -> AppResult<Option<CommandRecord>> {
        let (sandbox, id) = (sandbox.as_str().to_owned(), id.to_owned());
        self.call(move |c| {
            Ok(c.query_row(
                &format!("SELECT {COLUMNS} FROM commands WHERE sandbox_id = ?1 AND id = ?2"),
                [sandbox, id],
                command_row,
            )
            .optional()?)
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
            Ok(c.query_row(
                &format!("SELECT {COLUMNS} FROM commands WHERE sandbox_id = ?1 AND name = ?2"),
                [sandbox, name],
                command_row,
            )
            .optional()?)
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
            let changed = c.execute(
                "DELETE FROM commands WHERE sandbox_id = ?1 AND id = ?2",
                [sandbox, id],
            )?;
            Ok(changed == 1)
        })
        .await
    }

    async fn insert_command_run(
        &self,
        run: &CommandRunRecord,
        max_running: u32,
    ) -> AppResult<RunInsert> {
        let r = run.clone();
        self.call(move |c| {
            let args = serde_json::to_string(&r.args).map_err(AppError::internal)?;
            let tx = c.transaction()?;
            let running: u32 = tx.query_row(
                "SELECT COUNT(*) FROM command_runs WHERE sandbox_id = ?1 AND state = 'running'",
                [r.sandbox_id.as_str()],
                |row| row.get(0),
            )?;
            if r.state == "running" && running >= max_running {
                return Ok(RunInsert::Busy);
            }
            // One run of a task waits for a person at a time, also for calls made together
            // (review round 1 of slice 3).
            let waiting: u32 = tx.query_row(
                "SELECT COUNT(*) FROM command_runs WHERE task_id = ?1 AND state = 'waiting'",
                [r.task_id.as_str()],
                |row| row.get(0),
            )?;
            if r.state == "waiting" && waiting > 0 {
                return Ok(RunInsert::TaskWaits);
            }
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
            )?;
            // A project keeps its newest ended runs; a waiting or running one always stays.
            tx.execute(
                "DELETE FROM command_runs WHERE sandbox_id = ?1 AND ended_at IS NOT NULL \
                 AND id NOT IN (SELECT id FROM command_runs WHERE sandbox_id = ?1 \
                 AND ended_at IS NOT NULL ORDER BY created_at DESC, id DESC LIMIT ?2)",
                params![r.sandbox_id.as_str(), KEEP_RUNS],
            )?;
            tx.commit()?;
            Ok(RunInsert::Stored)
        })
        .await
    }

    async fn latest_command_run_of_task(
        &self,
        task: &TaskId,
    ) -> AppResult<Option<CommandRunRecord>> {
        let task = task.as_str().to_owned();
        self.call(move |c| {
            Ok(c.query_row(
                &format!(
                    "SELECT {RUN_COLUMNS} FROM command_runs WHERE task_id = ?1 \
                     ORDER BY created_at DESC, id DESC LIMIT 1"
                ),
                [task],
                run_row,
            )
            .optional()?)
        })
        .await
    }

    async fn command_run(&self, id: &str) -> AppResult<Option<CommandRunRecord>> {
        let id = id.to_owned();
        self.call(move |c| {
            Ok(c.query_row(
                &format!("SELECT {RUN_COLUMNS} FROM command_runs WHERE id = ?1"),
                [id],
                run_row,
            )
            .optional()?)
        })
        .await
    }

    async fn start_command_run(&self, id: &str, from: &str, at: Timestamp) -> AppResult<bool> {
        let (id, from) = (id.to_owned(), from.to_owned());
        self.call(move |c| {
            let changed = c.execute(
                "UPDATE command_runs SET state = 'running', started_at = ?1 \
                     WHERE id = ?2 AND state = ?3",
                params![at.0, id, from],
            )?;
            Ok(changed == 1)
        })
        .await
    }

    async fn end_command_run(&self, id: &str, from: &str, end: &CommandRunEnd) -> AppResult<bool> {
        let (id, from, e) = (id.to_owned(), from.to_owned(), end.clone());
        self.call(move |c| {
            let changed = c.execute(
                "UPDATE command_runs SET state = ?1, reason = ?2, exit_code = ?3, \
                     stdout = ?4, stderr = ?5, ended_at = ?6, args = '[]' \
                     WHERE id = ?7 AND state = ?8",
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
            )?;
            Ok(changed == 1)
        })
        .await
    }

    async fn command_runs_in_state(&self, state: &str) -> AppResult<Vec<CommandRunRecord>> {
        let state = state.to_owned();
        self.call(move |c| {
            let mut stmt = c.prepare(&format!(
                "SELECT {RUN_COLUMNS} FROM command_runs WHERE state = ?1 ORDER BY created_at"
            ))?;
            let rows = stmt
                .query_map([state], run_row)?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(rows)
        })
        .await
    }

    async fn answer_command_run(
        &self,
        id: &str,
        approve: bool,
        by: &str,
        at: Timestamp,
        max_running: u32,
    ) -> AppResult<bool> {
        let (id, by) = (id.to_owned(), by.to_owned());
        self.call(move |c| {
            let changed = if approve {
                c.execute(
                    "UPDATE command_runs SET state = 'running', started_at = ?1, \
                     answered_by = ?2 WHERE id = ?3 AND state = 'waiting' AND deadline > ?1 \
                     AND (SELECT COUNT(*) FROM command_runs AS other \
                          WHERE other.sandbox_id = command_runs.sandbox_id \
                          AND other.state = 'running') < ?4",
                    params![at.0, by, id, max_running],
                )
            } else {
                c.execute(
                    "UPDATE command_runs SET state = 'rejected', ended_at = ?1, \
                     answered_by = ?2, reason = 'a person rejected it', args = '[]' \
                     WHERE id = ?3 AND state = 'waiting' AND deadline > ?1",
                    params![at.0, by, id],
                )
            }?;
            Ok(changed == 1)
        })
        .await
    }

    async fn expire_command_runs(&self, at: Timestamp) -> AppResult<Vec<CommandRunRecord>> {
        self.call(move |c| {
            let tx = c.transaction()?;
            let due = {
                let mut stmt = tx.prepare(&format!(
                    "SELECT {RUN_COLUMNS} FROM command_runs \
                         WHERE state = 'waiting' AND deadline <= ?1"
                ))?;
                stmt.query_map([at.0], run_row)?
                    .collect::<Result<Vec<_>, _>>()?
            };
            for run in &due {
                tx.execute(
                    "UPDATE command_runs SET state = 'expired', ended_at = ?1, args = '[]', \
                     reason = 'no answer in time, which is a no' \
                     WHERE id = ?2 AND state = 'waiting'",
                    params![at.0, run.id],
                )?;
            }
            tx.commit()?;
            Ok(due)
        })
        .await
    }

    async fn command_runs_in(
        &self,
        sandbox: &SandboxId,
        limit: u32,
    ) -> AppResult<Vec<CommandRunRecord>> {
        let sandbox = sandbox.as_str().to_owned();
        self.call(move |c| {
            let mut stmt = c.prepare(&format!(
                "SELECT {LISTED_COLUMNS} FROM command_runs WHERE sandbox_id = ?1 \
                     AND (ended_at IS NULL OR id IN (SELECT id FROM command_runs \
                     WHERE sandbox_id = ?1 AND ended_at IS NOT NULL \
                     ORDER BY created_at DESC, id DESC LIMIT ?2)) \
                     ORDER BY created_at DESC, id DESC"
            ))?;
            let rows = stmt
                .query_map(params![sandbox, limit], run_row)?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(rows)
        })
        .await
    }

    async fn waiting_command_run_of_task(
        &self,
        task: &TaskId,
    ) -> AppResult<Option<CommandRunRecord>> {
        let task = task.as_str().to_owned();
        self.call(move |c| {
            Ok(c.query_row(
                &format!(
                    "SELECT {RUN_COLUMNS} FROM command_runs WHERE task_id = ?1 \
                     AND state = 'waiting' ORDER BY created_at DESC LIMIT 1"
                ),
                [task],
                run_row,
            )
            .optional()?)
        })
        .await
    }
}
