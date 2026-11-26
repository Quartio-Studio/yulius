//! Users and GitHub tokens.

use async_trait::async_trait;
use rusqlite::{OptionalExtension, Row, Transaction, params};
use yulius_app::error::{AppError, AppResult};
use yulius_app::model::{GitHubTokenRecord, Sealed, UserRecord};
use yulius_app::ports::store::UserStore;
use yulius_domain::ids::UserId;
use yulius_domain::time::Timestamp;

use crate::{DbResult, SqliteStore, opt_ts, text_parsed, to_i64, to_u64};

const COLS: &str = "id, github_login, github_id, display_name, avatar_url, is_superadmin, \
    is_instance_admin, password_hash, created_at, disabled_at";

pub(crate) fn user_from_row(row: &Row<'_>) -> rusqlite::Result<UserRecord> {
    Ok(UserRecord {
        id: text_parsed(row, "id", "user id", UserId::parse)?,
        github_login: row.get("github_login")?,
        github_id: row.get::<_, Option<i64>>("github_id")?.map(to_u64),
        display_name: row.get("display_name")?,
        avatar_url: row.get("avatar_url")?,
        is_superadmin: row.get("is_superadmin")?,
        is_instance_admin: row.get("is_instance_admin")?,
        password_hash: row.get("password_hash")?,
        created_at: Timestamp(row.get("created_at")?),
        disabled_at: opt_ts(row, "disabled_at")?,
    })
}

pub(crate) fn insert_user_tx(tx: &Transaction<'_>, user: &UserRecord) -> DbResult<()> {
    tx.execute(
        &format!("INSERT INTO users ({COLS}) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)"),
        params![
            user.id.as_str(),
            user.github_login,
            user.github_id.map(to_i64),
            user.display_name,
            user.avatar_url,
            user.is_superadmin,
            user.is_instance_admin,
            user.password_hash,
            user.created_at.0,
            user.disabled_at.map(|t| t.0),
        ],
    )?;
    Ok(())
}

impl SqliteStore {
    async fn user_where(&self, clause: &'static str, arg: String) -> AppResult<Option<UserRecord>> {
        self.call(move |c| {
            Ok(c.query_row(
                &format!("SELECT {COLS} FROM users WHERE {clause}"),
                [arg],
                user_from_row,
            )
            .optional()?)
        })
        .await
    }
}

#[async_trait]
impl UserStore for SqliteStore {
    async fn user(&self, id: &UserId) -> AppResult<Option<UserRecord>> {
        self.user_where("id = ?1", id.as_str().to_owned()).await
    }

    async fn control_ask_first(&self, user: &UserId) -> AppResult<bool> {
        let id = user.as_str().to_owned();
        self.call(move |c| {
            let ask: Option<bool> = c
                .query_row(
                    "SELECT control_ask_first FROM users WHERE id = ?1",
                    [id.as_str()],
                    |r| r.get(0),
                )
                .optional()?;
            Ok(ask.unwrap_or(false))
        })
        .await
    }

    async fn set_control_ask_first(&self, user: &UserId, ask_first: bool) -> AppResult<()> {
        let id = user.as_str().to_owned();
        self.call(move |c| {
            c.execute(
                "UPDATE users SET control_ask_first = ?2 WHERE id = ?1",
                params![id, ask_first],
            )?;
            Ok(())
        })
        .await
    }

    async fn user_by_login(&self, login: &str) -> AppResult<Option<UserRecord>> {
        self.user_where("github_login = ?1 COLLATE NOCASE", login.to_owned())
            .await
    }

    async fn user_by_github_id(&self, github_id: u64) -> AppResult<Option<UserRecord>> {
        let gid = to_i64(github_id);
        self.call(move |c| {
            Ok(c.query_row(
                &format!("SELECT {COLS} FROM users WHERE github_id = ?1"),
                [gid],
                user_from_row,
            )
            .optional()?)
        })
        .await
    }

    async fn superadmin(&self) -> AppResult<Option<UserRecord>> {
        self.call(|c| {
            Ok(c.query_row(
                &format!("SELECT {COLS} FROM users WHERE is_superadmin = 1"),
                [],
                user_from_row,
            )
            .optional()?)
        })
        .await
    }

    async fn users(&self) -> AppResult<Vec<UserRecord>> {
        self.call(|c| {
            let mut stmt = c.prepare(&format!("SELECT {COLS} FROM users ORDER BY created_at"))?;
            let rows = stmt.query_map([], user_from_row)?;
            Ok(rows.collect::<Result<Vec<_>, _>>()?)
        })
        .await
    }

    async fn insert_user(&self, user: &UserRecord) -> AppResult<()> {
        let user = user.clone();
        self.call(move |c| {
            let tx = c.transaction()?;
            insert_user_tx(&tx, &user)?;
            Ok(tx.commit()?)
        })
        .await
    }

    async fn update_user_flags(
        &self,
        id: &UserId,
        disabled_at: Option<Option<Timestamp>>,
        is_instance_admin: Option<bool>,
    ) -> AppResult<()> {
        let id = id.as_str().to_owned();
        self.call_access(move |c| {
            let tx = c.transaction()?;
            if let Some(disabled) = disabled_at {
                tx.execute(
                    "UPDATE users SET disabled_at = ?1 WHERE id = ?2",
                    params![disabled.map(|t| t.0), id],
                )?;
            }
            if let Some(admin) = is_instance_admin {
                tx.execute(
                    "UPDATE users SET is_instance_admin = ?1 WHERE id = ?2",
                    params![admin, id],
                )?;
            }
            Ok(tx.commit()?)
        })
        .await
    }

    async fn set_password_hash(&self, id: &UserId, phc: &str) -> AppResult<()> {
        let (id, phc) = (id.as_str().to_owned(), phc.to_owned());
        self.call(move |c| {
            c.execute(
                "UPDATE users SET password_hash = ?1 WHERE id = ?2",
                params![phc, id],
            )?;
            Ok(())
        })
        .await
    }

    async fn link_github(
        &self,
        id: &UserId,
        github_id: u64,
        login: &str,
        display_name: &str,
        avatar_url: Option<&str>,
    ) -> AppResult<()> {
        let (id, login, name, avatar) = (
            id.as_str().to_owned(),
            login.to_owned(),
            display_name.to_owned(),
            avatar_url.map(str::to_owned),
        );
        self.call_access(move |c| {
            let changed = c
                .execute(
                    "UPDATE users SET github_id = ?1, github_login = ?2, display_name = ?3, avatar_url = ?4 WHERE id = ?5",
                    params![to_i64(github_id), login, name, avatar, id],
                )?;
            if changed == 1 { Ok(()) } else { Err(AppError::NotFound.into()) }
        })
        .await
    }

    async fn save_github_token(&self, token: &GitHubTokenRecord, at: Timestamp) -> AppResult<()> {
        let t = token.clone();
        self.call(move |c| {
            c.execute(
                "INSERT INTO github_tokens (user_id, access, refresh, expires_at, refresh_expires_at, updated_at) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6) ON CONFLICT(user_id) DO UPDATE SET access = excluded.access, \
                 refresh = excluded.refresh, expires_at = excluded.expires_at, \
                 refresh_expires_at = excluded.refresh_expires_at, updated_at = excluded.updated_at",
                params![
                    t.user_id.as_str(),
                    t.access.0,
                    t.refresh.map(|s| s.0),
                    t.expires_at.map(|x| x.0),
                    t.refresh_expires_at.map(|x| x.0),
                    at.0
                ],
            )?;
            Ok(())
        })
        .await
    }

    async fn github_token(&self, user: &UserId) -> AppResult<Option<GitHubTokenRecord>> {
        let id = user.as_str().to_owned();
        let user = user.clone();
        self.call(move |c| {
            Ok(c.query_row(
                "SELECT access, refresh, expires_at, refresh_expires_at FROM github_tokens WHERE user_id = ?1",
                [id],
                |r| {
                    Ok(GitHubTokenRecord {
                        user_id: user.clone(),
                        access: Sealed(r.get(0)?),
                        refresh: r.get::<_, Option<Vec<u8>>>(1)?.map(Sealed),
                        expires_at: opt_ts(r, 2)?,
                        refresh_expires_at: opt_ts(r, 3)?,
                    })
                },
            )
            .optional()?)
        })
        .await
    }
}
