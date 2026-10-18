//! MCP server configuration. Secrets are stored only as sealed blobs produced by the
//! application; names are unique per scope under `yulius_app::mcp_config::name_key`.

use async_trait::async_trait;
use rusqlite::{OptionalExtension, Row, Transaction, params};
use yulius_app::error::{AppError, AppResult};
use yulius_app::mcp_config::{McpOrigin, McpServerRecord, McpTransport, name_key};
use yulius_app::model::Sealed;
use yulius_app::ports::store_ext::McpStore;
use yulius_domain::ids::SandboxId;
use yulius_domain::time::Timestamp;

use crate::{SqliteStore, bad_column, db};

const COLS: &str = "id, sandbox_id, name, transport, command, args, url, env, headers, enabled, \
                    created_at, origin";

fn from_row(row: &Row<'_>) -> rusqlite::Result<McpServerRecord> {
    let sandbox: Option<String> = row.get("sandbox_id")?;
    let transport: String = row.get("transport")?;
    let args: String = row.get("args")?;
    let origin: String = row.get("origin")?;
    Ok(McpServerRecord {
        id: row.get("id")?,
        sandbox_id: sandbox
            .as_deref()
            .map(|s| SandboxId::parse(s).map_err(|_| bad_column("sandbox id", s)))
            .transpose()?,
        name: row.get("name")?,
        transport: McpTransport::parse(&transport)
            .ok_or_else(|| bad_column("transport", &transport))?,
        command: row.get("command")?,
        args: serde_json::from_str(&args).map_err(|_| bad_column("args", &args))?,
        url: row.get("url")?,
        env: row.get::<_, Option<Vec<u8>>>("env")?.map(Sealed),
        headers: row.get::<_, Option<Vec<u8>>>("headers")?.map(Sealed),
        enabled: row.get("enabled")?,
        created_at: Timestamp(row.get("created_at")?),
        origin: McpOrigin::parse(&origin).ok_or_else(|| bad_column("origin", &origin))?,
    })
}

/// Fails with `Conflict` when another server of the same scope has the same name key.
fn ensure_unique(tx: &Transaction<'_>, server: &McpServerRecord) -> AppResult<()> {
    let mut stmt = tx
        .prepare("SELECT id, name FROM mcp_servers WHERE sandbox_id IS ?1")
        .map_err(db)?;
    let rows = stmt
        .query_map([server.sandbox_id.as_ref().map(SandboxId::as_str)], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        })
        .map_err(db)?;
    let key = name_key(&server.name);
    for row in rows {
        let (id, name) = row.map_err(db)?;
        if id != server.id && name_key(&name) == key {
            return Err(AppError::Conflict("duplicate MCP server name".into()));
        }
    }
    Ok(())
}

#[async_trait]
impl McpStore for SqliteStore {
    async fn mcp_servers(&self, sandbox: &SandboxId) -> AppResult<Vec<McpServerRecord>> {
        let sandbox = sandbox.as_str().to_owned();
        self.call(move |c| {
            let mut stmt = c
                .prepare(&format!(
                    "SELECT {COLS} FROM mcp_servers WHERE sandbox_id IS NULL OR sandbox_id = ?1 \
                     ORDER BY sandbox_id IS NOT NULL, name COLLATE NOCASE"
                ))
                .map_err(db)?;
            let rows = stmt.query_map([sandbox], from_row).map_err(db)?;
            rows.collect::<Result<Vec<_>, _>>().map_err(db)
        })
        .await
    }

    async fn mcp_server(&self, id: &str) -> AppResult<Option<McpServerRecord>> {
        let id = id.to_owned();
        self.call(move |c| {
            c.query_row(
                &format!("SELECT {COLS} FROM mcp_servers WHERE id = ?1"),
                [id],
                from_row,
            )
            .optional()
            .map_err(db)
        })
        .await
    }

    async fn insert_mcp_server(
        &self,
        server: &McpServerRecord,
        max_per_scope: u32,
    ) -> AppResult<()> {
        let s = server.clone();
        self.call(move |c| {
            let tx = c.transaction().map_err(db)?;
            let count: i64 = tx
                .query_row(
                    "SELECT COUNT(*) FROM mcp_servers WHERE sandbox_id IS ?1",
                    [s.sandbox_id.as_ref().map(SandboxId::as_str)],
                    |r| r.get(0),
                )
                .map_err(db)?;
            if count >= i64::from(max_per_scope) {
                return Err(AppError::Capacity(format!(
                    "at most {max_per_scope} MCP servers per scope"
                )));
            }
            ensure_unique(&tx, &s)?;
            tx.execute(
                &format!(
                    "INSERT INTO mcp_servers ({COLS}) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)"
                ),
                params![
                    s.id,
                    s.sandbox_id.as_ref().map(SandboxId::as_str),
                    s.name,
                    s.transport.as_str(),
                    s.command,
                    serde_json::Value::from(s.args.clone()).to_string(),
                    s.url,
                    s.env.as_ref().map(|x| x.0.clone()),
                    s.headers.as_ref().map(|x| x.0.clone()),
                    s.enabled,
                    s.created_at.0,
                    s.origin.as_str(),
                ],
            )
            .map_err(db)?;
            tx.commit().map_err(db)
        })
        .await
    }

    async fn update_mcp_server(&self, server: &McpServerRecord) -> AppResult<bool> {
        let s = server.clone();
        self.call(move |c| {
            let tx = c.transaction().map_err(db)?;
            ensure_unique(&tx, &s)?;
            let n = tx
                .execute(
                    "UPDATE mcp_servers SET name = ?2, transport = ?3, command = ?4, args = ?5, \
                     url = ?6, env = ?7, headers = ?8, enabled = ?9, origin = ?10 WHERE id = ?1",
                    params![
                        s.id,
                        s.name,
                        s.transport.as_str(),
                        s.command,
                        serde_json::Value::from(s.args.clone()).to_string(),
                        s.url,
                        s.env.as_ref().map(|x| x.0.clone()),
                        s.headers.as_ref().map(|x| x.0.clone()),
                        s.enabled,
                        s.origin.as_str(),
                    ],
                )
                .map_err(db)?;
            tx.commit().map_err(db)?;
            Ok(n > 0)
        })
        .await
    }

    async fn delete_mcp_server(&self, id: &str) -> AppResult<bool> {
        let id = id.to_owned();
        self.call(move |c| {
            let n = c
                .execute("DELETE FROM mcp_servers WHERE id = ?1", [id])
                .map_err(db)?;
            Ok(n > 0)
        })
        .await
    }
}
