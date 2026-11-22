//! Brokered commands (SPEC-626, migration 0089): a project's command definitions.

use async_trait::async_trait;
use rusqlite::{OptionalExtension, Row, params};
use yulius_app::error::{AppError, AppResult};
use yulius_app::ports::store_ext::{CommandRecord, CommandStore};
use yulius_domain::ids::SandboxId;
use yulius_domain::time::Timestamp;

use crate::{SqliteStore, bad_column, db};

const COLUMNS: &str =
    "id, sandbox_id, name, definition, created_by, created_at, updated_by, updated_at";

fn command_row(row: &Row<'_>) -> rusqlite::Result<CommandRecord> {
    let sandbox: String = row.get(1)?;
    Ok(CommandRecord {
        id: row.get(0)?,
        sandbox_id: SandboxId::parse(&sandbox).map_err(|_| bad_column("sandbox id", &sandbox))?,
        name: row.get(2)?,
        definition: row.get(3)?,
        created_by: row.get(4)?,
        created_at: Timestamp(row.get(5)?),
        updated_by: row.get(6)?,
        updated_at: Timestamp(row.get(7)?),
    })
}

/// A unique name taken: said as such, not as "conflicting record".
fn named(err: rusqlite::Error) -> AppError {
    match db(err) {
        AppError::Conflict(_) => {
            AppError::Conflict("this project already has a command of that name".into())
        }
        other => other,
    }
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
                    "INSERT INTO commands ({COLUMNS}) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)"
                ),
                params![
                    r.id,
                    r.sandbox_id.as_str(),
                    r.name,
                    r.definition,
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
                     updated_at = ?4 WHERE id = ?5 AND sandbox_id = ?6 AND updated_at = ?7",
                    params![
                        r.name,
                        r.definition,
                        r.updated_by,
                        r.updated_at.0,
                        r.id,
                        r.sandbox_id.as_str(),
                        read_at.0,
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
}
