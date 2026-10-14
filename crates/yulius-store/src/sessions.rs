//! Web sessions, API tokens, device grants, OAuth states and the permission cache.

use async_trait::async_trait;
use rusqlite::{OptionalExtension, Row, params};
use yulius_app::error::AppResult;
use yulius_app::model::{
    ApiTokenRecord, CachedAccess, DeviceGrantBounds, DeviceGrantInsert, DeviceGrantRecord, Sealed,
    WebSessionRecord,
};
use yulius_app::ports::store::{PermissionStore, SessionStore};
use yulius_domain::ids::{SandboxId, UserId};
use yulius_domain::permission::AccessLevel;
use yulius_domain::time::Timestamp;

use crate::{SqliteStore, bad_column, db};

fn user_id(text: &str) -> rusqlite::Result<UserId> {
    UserId::parse(text).map_err(|_| bad_column("user id", text))
}

fn session_from_row(row: &Row<'_>) -> rusqlite::Result<WebSessionRecord> {
    let uid: String = row.get("user_id")?;
    Ok(WebSessionRecord {
        id_hash: row.get("id_hash")?,
        user_id: user_id(&uid)?,
        csrf_token: row.get("csrf_token")?,
        created_at: Timestamp(row.get("created_at")?),
        expires_at: Timestamp(row.get("expires_at")?),
        revoked_at: row.get::<_, Option<i64>>("revoked_at")?.map(Timestamp),
    })
}

fn grant_from_row(row: &Row<'_>) -> rusqlite::Result<DeviceGrantRecord> {
    let uid: Option<String> = row.get("user_id")?;
    Ok(DeviceGrantRecord {
        user_code: row.get("user_code")?,
        device_code_hash: row.get("device_code_hash")?,
        status: row.get("status")?,
        user_id: uid.as_deref().map(user_id).transpose()?,
        token: row.get::<_, Option<Vec<u8>>>("token_plain")?.map(Sealed),
        client_key: row.get("client_key")?,
        created_at: Timestamp(row.get("created_at")?),
        expires_at: Timestamp(row.get("expires_at")?),
    })
}

const GRANT_COLS: &str = "user_code, device_code_hash, status, user_id, token_plain, created_at, \
     expires_at, client_key";

/// Deletes expired grants (any status) and consumed ones (#169); denied grants stay until they
/// expire so the CLI still reads "denied".
const PURGE_GRANTS: &str =
    "DELETE FROM device_grants WHERE expires_at <= ?1 OR status = 'consumed'";

#[async_trait]
impl SessionStore for SqliteStore {
    async fn insert_web_session(&self, s: &WebSessionRecord) -> AppResult<()> {
        let s = s.clone();
        self.call(move |c| {
            c.execute(
                "INSERT INTO web_sessions (id_hash, user_id, csrf_token, created_at, expires_at) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![s.id_hash, s.user_id.as_str(), s.csrf_token, s.created_at.0, s.expires_at.0],
            )
            .map_err(db)?;
            Ok(())
        })
        .await
    }

    async fn web_session(&self, id_hash: &str) -> AppResult<Option<WebSessionRecord>> {
        let h = id_hash.to_owned();
        self.call(move |c| {
            c.query_row(
                "SELECT id_hash, user_id, csrf_token, created_at, expires_at, revoked_at FROM web_sessions WHERE id_hash = ?1",
                [h],
                session_from_row,
            )
            .optional()
            .map_err(db)
        })
        .await
    }

    async fn revoke_web_session(&self, id_hash: &str, at: Timestamp) -> AppResult<()> {
        let h = id_hash.to_owned();
        self.call_access(move |c| {
            c.execute(
                "UPDATE web_sessions SET revoked_at = ?1 WHERE id_hash = ?2 AND revoked_at IS NULL",
                params![at.0, h],
            )
            .map_err(db)?;
            Ok(())
        })
        .await
    }

    async fn revoke_user_credentials(&self, user: &UserId, at: Timestamp) -> AppResult<()> {
        let u = user.as_str().to_owned();
        self.call_access(move |c| {
            let tx = c.transaction().map_err(db)?;
            tx.execute(
                "UPDATE web_sessions SET revoked_at = ?1 WHERE user_id = ?2 AND revoked_at IS NULL",
                params![at.0, u],
            )
            .map_err(db)?;
            tx.execute(
                "UPDATE api_tokens SET revoked_at = ?1 WHERE user_id = ?2 AND revoked_at IS NULL",
                params![at.0, u],
            )
            .map_err(db)?;
            tx.commit().map_err(db)
        })
        .await
    }

    async fn insert_api_token(&self, t: &ApiTokenRecord) -> AppResult<()> {
        let t = t.clone();
        self.call(move |c| {
            c.execute(
                "INSERT INTO api_tokens (id, user_id, token_hash, label, created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![t.id, t.user_id.as_str(), t.token_hash, t.label, t.created_at.0],
            )
            .map_err(db)?;
            Ok(())
        })
        .await
    }

    async fn api_token(&self, token_hash: &str) -> AppResult<Option<ApiTokenRecord>> {
        let h = token_hash.to_owned();
        self.call(move |c| {
            c.query_row(
                "SELECT id, user_id, token_hash, label, created_at, revoked_at FROM api_tokens WHERE token_hash = ?1 AND revoked_at IS NULL",
                [h],
                |r| {
                    let uid: String = r.get(1)?;
                    Ok(ApiTokenRecord {
                        id: r.get(0)?,
                        user_id: user_id(&uid)?,
                        token_hash: r.get(2)?,
                        label: r.get(3)?,
                        created_at: Timestamp(r.get(4)?),
                        revoked_at: r.get::<_, Option<i64>>(5)?.map(Timestamp),
                    })
                },
            )
            .optional()
            .map_err(db)
        })
        .await
    }

    async fn insert_device_grant(
        &self,
        g: &DeviceGrantRecord,
        bounds: DeviceGrantBounds,
    ) -> AppResult<DeviceGrantInsert> {
        let g = g.clone();
        let per_client = i64::try_from(bounds.per_client).unwrap_or(i64::MAX);
        let total = i64::try_from(bounds.total).unwrap_or(i64::MAX);
        self.call(move |c| {
            let tx = c.transaction().map_err(db)?;
            tx.execute(PURGE_GRANTS, [g.created_at.0]).map_err(db)?;
            let (live, of_client): (i64, i64) = tx
                .query_row(
                    "SELECT COUNT(*), COUNT(*) FILTER (WHERE client_key = ?1) FROM device_grants",
                    [&g.client_key],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .map_err(db)?;
            // A refused start still commits its purge (#169).
            let outcome = if of_client >= per_client {
                DeviceGrantInsert::ClientFull
            } else if live >= total {
                DeviceGrantInsert::InstanceFull
            } else {
                tx.execute(
                    &format!(
                        "INSERT INTO device_grants ({GRANT_COLS}) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)"
                    ),
                    params![
                        g.user_code,
                        g.device_code_hash,
                        g.status,
                        g.user_id.as_ref().map(UserId::as_str),
                        g.token.map(|t| t.0),
                        g.created_at.0,
                        g.expires_at.0,
                        g.client_key
                    ],
                )
                .map_err(db)?;
                DeviceGrantInsert::Inserted
            };
            tx.commit().map_err(db)?;
            Ok(outcome)
        })
        .await
    }

    async fn purge_device_grants(&self, now: Timestamp) -> AppResult<usize> {
        self.call(move |c| c.execute(PURGE_GRANTS, [now.0]).map_err(db))
            .await
    }

    async fn device_grant_by_device(
        &self,
        device_code_hash: &str,
    ) -> AppResult<Option<DeviceGrantRecord>> {
        let h = device_code_hash.to_owned();
        self.call(move |c| {
            c.query_row(
                &format!("SELECT {GRANT_COLS} FROM device_grants WHERE device_code_hash = ?1"),
                [h],
                grant_from_row,
            )
            .optional()
            .map_err(db)
        })
        .await
    }

    async fn device_grant_by_user_code(
        &self,
        user_code: &str,
    ) -> AppResult<Option<DeviceGrantRecord>> {
        let code = user_code.to_owned();
        self.call(move |c| {
            c.query_row(
                &format!("SELECT {GRANT_COLS} FROM device_grants WHERE user_code = ?1"),
                [code],
                grant_from_row,
            )
            .optional()
            .map_err(db)
        })
        .await
    }

    async fn resolve_device_grant(
        &self,
        user_code: &str,
        status: &str,
        user: Option<&UserId>,
        token: Option<&Sealed>,
    ) -> AppResult<bool> {
        let (code, status, uid, token) = (
            user_code.to_owned(),
            status.to_owned(),
            user.map(|u| u.as_str().to_owned()),
            token.map(|t| t.0.clone()),
        );
        self.call(move |c| {
            let n = c
                .execute(
                    "UPDATE device_grants SET status = ?1, user_id = ?2, token_plain = ?3 WHERE user_code = ?4 AND status = 'pending'",
                    params![status, uid, token, code],
                )
                .map_err(db)?;
            Ok(n == 1)
        })
        .await
    }

    async fn consume_device_grant(&self, device_code_hash: &str) -> AppResult<bool> {
        let h = device_code_hash.to_owned();
        self.call(move |c| {
            let n = c
                .execute(
                    "UPDATE device_grants SET status = 'consumed', token_plain = NULL WHERE device_code_hash = ?1 AND status = 'approved'",
                    [h],
                )
                .map_err(db)?;
            Ok(n == 1)
        })
        .await
    }

    async fn insert_oauth_state(
        &self,
        state_hash: &str,
        next_path: &str,
        at: Timestamp,
        expires_at: Timestamp,
    ) -> AppResult<()> {
        let (h, next) = (state_hash.to_owned(), next_path.to_owned());
        self.call(move |c| {
            let tx = c.transaction().map_err(db)?;
            tx.execute("DELETE FROM oauth_states WHERE expires_at < ?1", [at.0]).map_err(db)?;
            tx.execute(
                "INSERT INTO oauth_states (state_hash, next_path, created_at, expires_at) VALUES (?1, ?2, ?3, ?4)",
                params![h, next, at.0, expires_at.0],
            )
            .map_err(db)?;
            tx.commit().map_err(db)
        })
        .await
    }

    async fn take_oauth_state(
        &self,
        state_hash: &str,
        now: Timestamp,
    ) -> AppResult<Option<String>> {
        let h = state_hash.to_owned();
        self.call(move |c| {
            let tx = c.transaction().map_err(db)?;
            let next: Option<String> = tx
                .query_row(
                    "SELECT next_path FROM oauth_states WHERE state_hash = ?1 AND expires_at >= ?2",
                    params![h, now.0],
                    |r| r.get(0),
                )
                .optional()
                .map_err(db)?;
            tx.execute("DELETE FROM oauth_states WHERE state_hash = ?1", [h])
                .map_err(db)?;
            tx.commit().map_err(db)?;
            Ok(next)
        })
        .await
    }
}

#[async_trait]
impl PermissionStore for SqliteStore {
    async fn cached_access(
        &self,
        user: &UserId,
        sandbox: &SandboxId,
    ) -> AppResult<Option<CachedAccess>> {
        let (u, s) = (user.as_str().to_owned(), sandbox.as_str().to_owned());
        self.call(move |c| {
            c.query_row(
                "SELECT level, checked_at FROM permission_cache WHERE user_id = ?1 AND sandbox_id = ?2",
                params![u, s],
                |r| {
                    let level: String = r.get(0)?;
                    Ok(CachedAccess { level: AccessLevel::parse_or_none(&level), checked_at: Timestamp(r.get(1)?) })
                },
            )
            .optional()
            .map_err(db)
        })
        .await
    }

    async fn put_access(
        &self,
        user: &UserId,
        sandbox: &SandboxId,
        level: AccessLevel,
        at: Timestamp,
    ) -> AppResult<()> {
        let (u, s) = (user.as_str().to_owned(), sandbox.as_str().to_owned());
        let epoch = self.access_epoch_handle();
        self.call(move |c| {
            let before: Option<String> = c
                .query_row(
                    "SELECT level FROM permission_cache WHERE user_id = ?1 AND sandbox_id = ?2",
                    params![u, s],
                    |r| r.get(0),
                )
                .optional()
                .map_err(db)?;
            c.execute(
                "INSERT INTO permission_cache (user_id, sandbox_id, level, checked_at) VALUES (?1, ?2, ?3, ?4) \
                 ON CONFLICT(user_id, sandbox_id) DO UPDATE SET level = excluded.level, checked_at = excluded.checked_at",
                params![u, s, level.as_str(), at.0],
            )
            .map_err(db)?;
            // Only a changed level changes who may type (SPEC-118 KEY-002); a first one takes
            // nothing away (no grant was confirmed without it).
            if before.is_some_and(|before| before != level.as_str()) {
                epoch.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            }
            Ok(())
        })
        .await
    }

    async fn clear_access(&self, user: &UserId) -> AppResult<()> {
        let u = user.as_str().to_owned();
        self.call_access(move |c| {
            c.execute("DELETE FROM permission_cache WHERE user_id = ?1", [u])
                .map_err(db)?;
            Ok(())
        })
        .await
    }

    fn access_epoch(&self) -> u64 {
        self.current_access_epoch()
    }
}
