//! MCP server configuration. Secrets are stored only as sealed blobs produced by the
//! application; names are unique per scope (sandbox and owner, SPEC-332) under
//! `yulius_app::mcp_config::name_key`.

use async_trait::async_trait;
use rusqlite::{OptionalExtension, Row, Transaction, params};
use yulius_app::error::{AppError, AppResult};
use yulius_app::mcp_config::{McpOrigin, McpServerRecord, McpTransport, name_key};
use yulius_app::model::Sealed;
use yulius_app::ports::store_ext::McpStore;
use yulius_domain::ids::{SandboxId, UserId};
use yulius_domain::time::Timestamp;

use crate::{SqliteStore, bad_column, db};

const COLS: &str = "id, sandbox_id, name, transport, command, args, url, env, headers, enabled, \
                    created_at, origin, owner_id";

fn from_row(row: &Row<'_>) -> rusqlite::Result<McpServerRecord> {
    let sandbox: Option<String> = row.get("sandbox_id")?;
    let transport: String = row.get("transport")?;
    let args: String = row.get("args")?;
    let origin: String = row.get("origin")?;
    let owner: Option<String> = row.get("owner_id")?;
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
        owner_id: owner
            .as_deref()
            .map(|o| UserId::parse(o).map_err(|_| bad_column("owner id", o)))
            .transpose()?,
    })
}

/// The scope columns of a server: its sandbox (`None`: every sandbox) and owner (`None`: team).
fn scope_of(server: &McpServerRecord) -> (Option<&str>, Option<&str>) {
    (
        server.sandbox_id.as_ref().map(SandboxId::as_str),
        server.owner_id.as_ref().map(UserId::as_str),
    )
}

/// Fails with `Conflict` when another server of the same scope has the same name key.
fn ensure_unique(tx: &Transaction<'_>, server: &McpServerRecord) -> AppResult<()> {
    let mut stmt = tx
        .prepare("SELECT id, name FROM mcp_servers WHERE sandbox_id IS ?1 AND owner_id IS ?2")
        .map_err(db)?;
    let rows = stmt
        .query_map(scope_of(server), |r| {
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

/// Fails with `Capacity` when the scope of `server` already holds `max` other servers.
fn ensure_room(tx: &Transaction<'_>, server: &McpServerRecord, max: u32) -> AppResult<()> {
    let count: i64 = tx
        .query_row(
            "SELECT COUNT(*) FROM mcp_servers WHERE sandbox_id IS ?1 AND owner_id IS ?2 AND id != ?3",
            params![scope_of(server).0, scope_of(server).1, server.id],
            |r| r.get(0),
        )
        .map_err(db)?;
    if count >= i64::from(max) {
        return Err(AppError::Capacity(format!(
            "at most {max} MCP servers per scope"
        )));
    }
    Ok(())
}

#[async_trait]
impl McpStore for SqliteStore {
    async fn mcp_servers(
        &self,
        sandbox: &SandboxId,
        person: Option<&UserId>,
    ) -> AppResult<Vec<McpServerRecord>> {
        let sandbox = sandbox.as_str().to_owned();
        let person = person.map(|p| p.as_str().to_owned());
        self.call(move |c| {
            let mut stmt = c
                .prepare(&format!(
                    "SELECT {COLS} FROM mcp_servers \
                     WHERE (sandbox_id IS NULL OR sandbox_id = ?1) \
                       AND (owner_id IS NULL OR owner_id = ?2) \
                     ORDER BY sandbox_id IS NOT NULL, name COLLATE NOCASE"
                ))
                .map_err(db)?;
            let rows = stmt
                .query_map(params![sandbox, person], from_row)
                .map_err(db)?;
            rows.collect::<Result<Vec<_>, _>>().map_err(db)
        })
        .await
    }

    async fn mcp_servers_reaching(&self, sandbox: &SandboxId) -> AppResult<Vec<McpServerRecord>> {
        let sandbox = sandbox.as_str().to_owned();
        self.call(move |c| {
            let mut stmt = c
                .prepare(&format!(
                    "SELECT {COLS} FROM mcp_servers WHERE sandbox_id IS NULL OR sandbox_id = ?1"
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
            ensure_room(&tx, &s, max_per_scope)?;
            ensure_unique(&tx, &s)?;
            tx.execute(
                &format!(
                    "INSERT INTO mcp_servers ({COLS}) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)"
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
                    s.owner_id.as_ref().map(UserId::as_str),
                ],
            )
            .map_err(db)?;
            tx.commit().map_err(db)
        })
        .await
    }

    async fn update_mcp_server(
        &self,
        server: &McpServerRecord,
        max_per_scope: u32,
    ) -> AppResult<bool> {
        let s = server.clone();
        self.call(move |c| {
            let tx = c.transaction().map_err(db)?;
            let moved = tx
                .query_row(
                    "SELECT sandbox_id IS NOT ?2 OR owner_id IS NOT ?3 FROM mcp_servers WHERE id = ?1",
                    params![s.id, scope_of(&s).0, scope_of(&s).1],
                    |r| r.get::<_, bool>(0),
                )
                .optional()
                .map_err(db)?;
            if moved == Some(true) {
                ensure_room(&tx, &s, max_per_scope)?;
            }
            ensure_unique(&tx, &s)?;
            let n = tx
                .execute(
                    "UPDATE mcp_servers SET name = ?2, transport = ?3, command = ?4, args = ?5, \
                     url = ?6, env = ?7, headers = ?8, enabled = ?9, origin = ?10, \
                     sandbox_id = ?11, owner_id = ?12 WHERE id = ?1",
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
                        scope_of(&s).0,
                        scope_of(&s).1,
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

#[cfg(test)]
mod tests {
    use super::*;

    fn server(id: &str, sandbox: Option<&str>, owner: Option<&str>) -> AppResult<McpServerRecord> {
        Ok(McpServerRecord {
            id: id.into(),
            sandbox_id: sandbox
                .map(SandboxId::parse)
                .transpose()
                .map_err(AppError::internal)?,
            name: "fleet".into(),
            transport: McpTransport::Http,
            command: None,
            args: Vec::new(),
            url: Some("https://fleet.test/mcp".into()),
            env: None,
            headers: None,
            enabled: true,
            created_at: Timestamp(1),
            origin: McpOrigin::Person,
            owner_id: owner
                .map(UserId::parse)
                .transpose()
                .map_err(AppError::internal)?,
        })
    }

    /// SPEC-332 MSC-006/008: names are unique per (sandbox, owner), a move checks room, and a
    /// person's servers go with them.
    #[tokio::test]
    async fn personal_servers_are_scoped_by_owner_and_removed_with_it()
    -> Result<(), Box<dyn std::error::Error>> {
        let store = SqliteStore::open_in_memory()?;
        store
            .call(|c| {
                c.execute_batch(
                    "INSERT INTO users (id, display_name, is_superadmin, is_instance_admin, created_at) VALUES ('u1', 'U', 0, 0, 1), ('u2', 'V', 0, 0, 1);
                     INSERT INTO sandboxes (id, slug, repo_owner, repo_name, default_branch, profile, port_base, port_count, created_by, onboarding_state, created_at) VALUES ('s1', 'a', 'o', 'a', 'main', '{}', 20000, 10, 'u1', 'not_needed', 1);",
                )
                .map_err(db)
            })
            .await?;
        store
            .insert_mcp_server(&server("team", None, None)?, 2)
            .await?;
        store
            .insert_mcp_server(&server("mine", None, Some("u1"))?, 2)
            .await?;
        store
            .insert_mcp_server(&server("yours", None, Some("u2"))?, 2)
            .await?;
        let dup = store
            .insert_mcp_server(&server("again", None, Some("u1"))?, 2)
            .await;
        assert!(matches!(dup, Err(AppError::Conflict(_))), "{dup:?}");
        let s1 = SandboxId::parse("s1")?;
        let u1 = UserId::parse("u1")?;
        let ids = |list: Vec<McpServerRecord>| {
            let mut ids: Vec<String> = list.into_iter().map(|s| s.id).collect();
            ids.sort();
            ids
        };
        assert_eq!(ids(store.mcp_servers(&s1, None).await?), ["team"]);
        assert_eq!(
            ids(store.mcp_servers(&s1, Some(&u1)).await?),
            ["mine", "team"]
        );

        // Moving a server into a full scope is refused; within it, an update is not.
        let mut other = server("other", Some("s1"), Some("u1"))?;
        other.name = "other".into();
        store.insert_mcp_server(&other, 1).await?;
        let mut moved = server("mine", Some("s1"), Some("u1"))?;
        let full = store.update_mcp_server(&moved, 1).await;
        assert!(matches!(full, Err(AppError::Capacity(_))), "{full:?}");
        moved.sandbox_id = None;
        moved.enabled = false;
        assert!(store.update_mcp_server(&moved, 1).await?);

        store
            .call(|c| {
                c.execute("DELETE FROM users WHERE id = 'u1'", [])
                    .map_err(db)
            })
            .await?;
        assert!(store.mcp_server("mine").await?.is_none());
        assert!(store.mcp_server("other").await?.is_none());
        assert!(store.mcp_server("yours").await?.is_some());
        assert!(store.mcp_server("team").await?.is_some());
        Ok(())
    }
}
