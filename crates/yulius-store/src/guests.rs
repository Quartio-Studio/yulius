//! A playground's guests (SPEC-410, ADR-0260): its expiring links and the guests they let in.

use async_trait::async_trait;
use rusqlite::{OptionalExtension, Row, params};
use yulius_app::error::AppResult;
use yulius_app::model::UserRecord;
use yulius_app::ports::guests::{
    GuestJoin, PlaygroundGuestRecord, PlaygroundGuestStore, PlaygroundInviteRecord,
};
use yulius_domain::ids::{SandboxId, UserId};
use yulius_domain::time::Timestamp;

use crate::users::insert_user_tx;
use crate::{SqliteStore, bad_column, db};

const COLS: &str = "id, sandbox_id, code_hash, created_by, created_at, expires_at, revoked_at";

fn invite_from_row(row: &Row<'_>) -> rusqlite::Result<PlaygroundInviteRecord> {
    let sandbox: String = row.get("sandbox_id")?;
    let by: String = row.get("created_by")?;
    Ok(PlaygroundInviteRecord {
        id: row.get("id")?,
        sandbox_id: SandboxId::parse(&sandbox).map_err(|_| bad_column("sandbox id", &sandbox))?,
        code_hash: row.get("code_hash")?,
        created_by: UserId::parse(&by).map_err(|_| bad_column("user id", &by))?,
        created_at: Timestamp(row.get("created_at")?),
        expires_at: Timestamp(row.get("expires_at")?),
        revoked_at: row.get::<_, Option<i64>>("revoked_at")?.map(Timestamp),
    })
}

/// Live: not revoked and not expired at `?2`.
const LIVE: &str = "revoked_at IS NULL AND expires_at > ?2";

#[async_trait]
impl PlaygroundGuestStore for SqliteStore {
    async fn insert_playground_invite(
        &self,
        invite: &PlaygroundInviteRecord,
        max_live: usize,
    ) -> AppResult<bool> {
        let invite = invite.clone();
        self.call(move |c| {
            let tx = c.transaction().map_err(db)?;
            let live: i64 = tx
                .query_row(
                    &format!(
                        "SELECT COUNT(*) FROM playground_invites WHERE sandbox_id = ?1 AND {LIVE}"
                    ),
                    params![invite.sandbox_id.as_str(), invite.created_at.0],
                    |row| row.get(0),
                )
                .map_err(db)?;
            if live >= i64::try_from(max_live).unwrap_or(i64::MAX) {
                return Ok(false);
            }
            tx.execute(
                &format!(
                    "INSERT INTO playground_invites ({COLS}) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)"
                ),
                params![
                    invite.id,
                    invite.sandbox_id.as_str(),
                    invite.code_hash,
                    invite.created_by.as_str(),
                    invite.created_at.0,
                    invite.expires_at.0,
                    invite.revoked_at.map(|t| t.0),
                ],
            )
            .map_err(db)?;
            tx.commit().map_err(db)?;
            Ok(true)
        })
        .await
    }

    async fn playground_invite_by_code(
        &self,
        code_hash: &str,
    ) -> AppResult<Option<PlaygroundInviteRecord>> {
        let hash = code_hash.to_owned();
        self.call(move |c| {
            c.query_row(
                &format!("SELECT {COLS} FROM playground_invites WHERE code_hash = ?1"),
                [hash],
                invite_from_row,
            )
            .optional()
            .map_err(db)
        })
        .await
    }

    async fn live_playground_invites(
        &self,
        sandbox: &SandboxId,
        now: Timestamp,
    ) -> AppResult<Vec<PlaygroundInviteRecord>> {
        let sandbox = sandbox.as_str().to_owned();
        self.call(move |c| {
            let mut stmt = c
                .prepare(&format!(
                    "SELECT {COLS} FROM playground_invites WHERE sandbox_id = ?1 AND {LIVE} \
                     ORDER BY created_at, id"
                ))
                .map_err(db)?;
            let rows = stmt
                .query_map(params![sandbox, now.0], invite_from_row)
                .map_err(db)?;
            rows.collect::<Result<Vec<_>, _>>().map_err(db)
        })
        .await
    }

    async fn playground_guests(
        &self,
        sandbox: &SandboxId,
    ) -> AppResult<Vec<PlaygroundGuestRecord>> {
        let sandbox = sandbox.as_str().to_owned();
        self.call(move |c| {
            let mut stmt = c
                .prepare(
                    "SELECT g.user_id, u.display_name, g.invite_id, g.joined_at \
                     FROM playground_guests g \
                     JOIN playground_invites i ON i.id = g.invite_id \
                     JOIN users u ON u.id = g.user_id \
                     WHERE i.sandbox_id = ?1 ORDER BY g.joined_at, g.rowid",
                )
                .map_err(db)?;
            let rows = stmt
                .query_map([sandbox], |row| {
                    let user: String = row.get(0)?;
                    Ok(PlaygroundGuestRecord {
                        user_id: UserId::parse(&user).map_err(|_| bad_column("user id", &user))?,
                        display_name: row.get(1)?,
                        invite_id: row.get(2)?,
                        joined_at: Timestamp(row.get(3)?),
                    })
                })
                .map_err(db)?;
            rows.collect::<Result<Vec<_>, _>>().map_err(db)
        })
        .await
    }

    async fn insert_playground_guest(
        &self,
        invite_id: &str,
        user: &UserRecord,
        at: Timestamp,
        max_guests: usize,
    ) -> AppResult<GuestJoin> {
        let (invite, user) = (invite_id.to_owned(), user.clone());
        self.call(move |c| {
            let tx = c.transaction().map_err(db)?;
            let live: Option<i64> = tx
                .query_row(
                    &format!("SELECT 1 FROM playground_invites WHERE id = ?1 AND {LIVE}"),
                    params![invite, at.0],
                    |row| row.get(0),
                )
                .optional()
                .map_err(db)?;
            if live.is_none() {
                return Ok(GuestJoin::Dead);
            }
            let guests: i64 = tx
                .query_row(
                    "SELECT COUNT(*) FROM playground_guests WHERE invite_id = ?1",
                    [&invite],
                    |row| row.get(0),
                )
                .map_err(db)?;
            if guests >= i64::try_from(max_guests).unwrap_or(i64::MAX) {
                return Ok(GuestJoin::Full);
            }
            insert_user_tx(&tx, &user)?;
            tx.execute(
                "INSERT INTO playground_guests (user_id, invite_id, joined_at) VALUES (?1, ?2, ?3)",
                params![user.id.as_str(), invite, at.0],
            )
            .map_err(db)?;
            tx.commit().map_err(db)?;
            Ok(GuestJoin::Joined)
        })
        .await
    }

    async fn guest_invite_of(&self, user: &UserId) -> AppResult<Option<PlaygroundInviteRecord>> {
        let user = user.as_str().to_owned();
        self.call(move |c| {
            c.query_row(
                &format!(
                    "SELECT {COLS} FROM playground_invites WHERE id = \
                     (SELECT invite_id FROM playground_guests WHERE user_id = ?1)"
                ),
                [user],
                invite_from_row,
            )
            .optional()
            .map_err(db)
        })
        .await
    }

    async fn revoke_playground_invite(
        &self,
        sandbox: &SandboxId,
        invite_id: &str,
        at: Timestamp,
    ) -> AppResult<bool> {
        let (sandbox, invite) = (sandbox.as_str().to_owned(), invite_id.to_owned());
        // An access write: open terminals re-check who may type at once.
        self.call_access(move |c| {
            let tx = c.transaction().map_err(db)?;
            let revoked = tx
                .execute(
                    &format!(
                        "UPDATE playground_invites SET revoked_at = ?2 \
                         WHERE id = ?1 AND sandbox_id = ?3 AND {LIVE}"
                    ),
                    params![invite, at.0, sandbox],
                )
                .map_err(db)?;
            if revoked == 0 {
                return Ok(false);
            }
            for table in ["web_sessions", "api_tokens"] {
                tx.execute(
                    &format!(
                        "UPDATE {table} SET revoked_at = ?2 WHERE revoked_at IS NULL AND user_id IN \
                         (SELECT user_id FROM playground_guests WHERE invite_id = ?1)"
                    ),
                    params![invite, at.0],
                )
                .map_err(db)?;
            }
            tx.commit().map_err(db)?;
            Ok(true)
        })
        .await
    }

    async fn invite_guest_ids(&self, invite_id: &str) -> AppResult<Vec<UserId>> {
        let invite = invite_id.to_owned();
        self.call(move |c| {
            let mut stmt = c
                .prepare(
                    "SELECT user_id FROM playground_guests WHERE invite_id = ?1 ORDER BY rowid",
                )
                .map_err(db)?;
            let rows = stmt
                .query_map([invite], |row| {
                    let user: String = row.get(0)?;
                    UserId::parse(&user).map_err(|_| bad_column("user id", &user))
                })
                .map_err(db)?;
            rows.collect::<Result<Vec<_>, _>>().map_err(db)
        })
        .await
    }

    async fn unended_expired_invites(
        &self,
        now: Timestamp,
        limit: usize,
    ) -> AppResult<Vec<PlaygroundInviteRecord>> {
        let limit = i64::try_from(limit).unwrap_or(i64::MAX);
        self.call(move |c| {
            let mut stmt = c
                .prepare(&format!(
                    "SELECT {COLS} FROM playground_invites WHERE ended_at IS NULL \
                     AND (expires_at <= ?1 OR revoked_at IS NOT NULL) \
                     ORDER BY COALESCE(swept_at, 0), expires_at, id LIMIT ?2"
                ))
                .map_err(db)?;
            let rows = stmt
                .query_map(params![now.0, limit], invite_from_row)
                .map_err(db)?;
            rows.collect::<Result<Vec<_>, _>>().map_err(db)
        })
        .await
    }

    async fn mark_invite_swept(
        &self,
        invite_id: &str,
        at: Timestamp,
        ended: bool,
    ) -> AppResult<()> {
        let invite = invite_id.to_owned();
        self.call(move |c| {
            c.execute(
                "UPDATE playground_invites SET swept_at = ?2, \
                 ended_at = CASE WHEN ?3 THEN ?2 ELSE NULL END \
                 WHERE id = ?1 AND ended_at IS NULL",
                params![invite, at.0, ended],
            )
            .map_err(db)?;
            Ok(())
        })
        .await
    }

    async fn playground_has_guests(&self, sandbox: &SandboxId) -> AppResult<bool> {
        let sandbox = sandbox.as_str().to_owned();
        self.call(move |c| {
            c.query_row(
                "SELECT EXISTS (SELECT 1 FROM playground_guests g \
                 JOIN playground_invites i ON i.id = g.invite_id \
                 WHERE i.sandbox_id = ?1)",
                [sandbox],
                |row| row.get(0),
            )
            .map_err(db)
        })
        .await
    }
}
