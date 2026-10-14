//! Users and GitHub tokens.

use async_trait::async_trait;
use rusqlite::{OptionalExtension, Row, Transaction, params};
use yulius_app::error::{AppError, AppResult};
use yulius_app::model::{GitHubTokenRecord, Sealed, UserRecord};
use yulius_app::ports::store::UserStore;
use yulius_domain::ids::UserId;
use yulius_domain::time::Timestamp;

use crate::{SqliteStore, bad_column, db, to_i64, to_u64};

const COLS: &str = "id, github_login, github_id, display_name, avatar_url, is_superadmin, \
    is_instance_admin, password_hash, created_at, disabled_at";

pub(crate) fn user_from_row(row: &Row<'_>) -> rusqlite::Result<UserRecord> {
    let id: String = row.get("id")?;
    Ok(UserRecord {
        id: UserId::parse(&id).map_err(|_| bad_column("user id", &id))?,
        github_login: row.get("github_login")?,
        github_id: row.get::<_, Option<i64>>("github_id")?.map(to_u64),
        display_name: row.get("display_name")?,
        avatar_url: row.get("avatar_url")?,
        is_superadmin: row.get("is_superadmin")?,
        is_instance_admin: row.get("is_instance_admin")?,
        password_hash: row.get("password_hash")?,
        created_at: Timestamp(row.get("created_at")?),
        disabled_at: row.get::<_, Option<i64>>("disabled_at")?.map(Timestamp),
    })
}

pub(crate) fn insert_user_tx(tx: &Transaction<'_>, user: &UserRecord) -> AppResult<()> {
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
    )
    .map_err(db)?;
    Ok(())
}

impl SqliteStore {
    async fn user_where(&self, clause: &'static str, arg: String) -> AppResult<Option<UserRecord>> {
        self.call(move |c| {
            c.query_row(
                &format!("SELECT {COLS} FROM users WHERE {clause}"),
                [arg],
                user_from_row,
            )
            .optional()
            .map_err(db)
        })
        .await
    }
}

#[async_trait]
impl UserStore for SqliteStore {
    async fn user(&self, id: &UserId) -> AppResult<Option<UserRecord>> {
        self.user_where("id = ?1", id.as_str().to_owned()).await
    }

    async fn user_by_login(&self, login: &str) -> AppResult<Option<UserRecord>> {
        self.user_where("github_login = ?1 COLLATE NOCASE", login.to_owned())
            .await
    }

    async fn user_by_github_id(&self, github_id: u64) -> AppResult<Option<UserRecord>> {
        let gid = to_i64(github_id);
        self.call(move |c| {
            c.query_row(
                &format!("SELECT {COLS} FROM users WHERE github_id = ?1"),
                [gid],
                user_from_row,
            )
            .optional()
            .map_err(db)
        })
        .await
    }

    async fn superadmin(&self) -> AppResult<Option<UserRecord>> {
        self.call(|c| {
            c.query_row(
                &format!("SELECT {COLS} FROM users WHERE is_superadmin = 1"),
                [],
                user_from_row,
            )
            .optional()
            .map_err(db)
        })
        .await
    }

    async fn users(&self) -> AppResult<Vec<UserRecord>> {
        self.call(|c| {
            let mut stmt = c
                .prepare(&format!("SELECT {COLS} FROM users ORDER BY created_at"))
                .map_err(db)?;
            let rows = stmt.query_map([], user_from_row).map_err(db)?;
            rows.collect::<Result<Vec<_>, _>>().map_err(db)
        })
        .await
    }

    async fn insert_user(&self, user: &UserRecord) -> AppResult<()> {
        let user = user.clone();
        self.call(move |c| {
            let tx = c.transaction().map_err(db)?;
            insert_user_tx(&tx, &user)?;
            tx.commit().map_err(db)
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
            let tx = c.transaction().map_err(db)?;
            if let Some(disabled) = disabled_at {
                tx.execute(
                    "UPDATE users SET disabled_at = ?1 WHERE id = ?2",
                    params![disabled.map(|t| t.0), id],
                )
                .map_err(db)?;
            }
            if let Some(admin) = is_instance_admin {
                tx.execute(
                    "UPDATE users SET is_instance_admin = ?1 WHERE id = ?2",
                    params![admin, id],
                )
                .map_err(db)?;
            }
            tx.commit().map_err(db)
        })
        .await
    }

    async fn set_password_hash(&self, id: &UserId, phc: &str) -> AppResult<()> {
        let (id, phc) = (id.as_str().to_owned(), phc.to_owned());
        self.call(move |c| {
            c.execute(
                "UPDATE users SET password_hash = ?1 WHERE id = ?2",
                params![phc, id],
            )
            .map_err(db)?;
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
                )
                .map_err(db)?;
            if changed == 1 { Ok(()) } else { Err(AppError::NotFound) }
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
            )
            .map_err(db)?;
            Ok(())
        })
        .await
    }

    async fn github_token(&self, user: &UserId) -> AppResult<Option<GitHubTokenRecord>> {
        let id = user.as_str().to_owned();
        let user = user.clone();
        self.call(move |c| {
            c.query_row(
                "SELECT access, refresh, expires_at, refresh_expires_at FROM github_tokens WHERE user_id = ?1",
                [id],
                |r| {
                    Ok(GitHubTokenRecord {
                        user_id: user.clone(),
                        access: Sealed(r.get(0)?),
                        refresh: r.get::<_, Option<Vec<u8>>>(1)?.map(Sealed),
                        expires_at: r.get::<_, Option<i64>>(2)?.map(Timestamp),
                        refresh_expires_at: r.get::<_, Option<i64>>(3)?.map(Timestamp),
                    })
                },
            )
            .optional()
            .map_err(db)
        })
        .await
    }
}
