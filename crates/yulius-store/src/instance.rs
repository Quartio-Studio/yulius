//! Instance singleton and setup.

use async_trait::async_trait;
use rusqlite::{OptionalExtension, Row, params};
use yulius_app::error::{AppError, AppResult};
use yulius_app::model::{InstanceRecord, Sealed, UserRecord};
use yulius_app::ports::store::InstanceStore;
use yulius_domain::time::Timestamp;

use crate::users::insert_user_tx;
use crate::{SqliteStore, db, to_i64, to_u64};

fn instance_from_row(row: &Row<'_>) -> rusqlite::Result<InstanceRecord> {
    Ok(InstanceRecord {
        owner_login: row.get("owner_login")?,
        owner_kind: row.get("owner_kind")?,
        public_url: row.get("public_url")?,
        github_app: row.get::<_, Option<Vec<u8>>>("github_app")?.map(Sealed),
        installation_id: row.get::<_, Option<i64>>("installation_id")?.map(to_u64),
        setup_token_hash: row.get("setup_token_hash")?,
        setup_completed_at: row
            .get::<_, Option<i64>>("setup_completed_at")?
            .map(Timestamp),
    })
}

const SELECT: &str = "SELECT owner_login, owner_kind, public_url, github_app, installation_id, \
    setup_token_hash, setup_completed_at FROM instance WHERE singleton = 1";

#[async_trait]
impl InstanceStore for SqliteStore {
    async fn instance(&self) -> AppResult<Option<InstanceRecord>> {
        self.call(|c| {
            c.query_row(SELECT, [], instance_from_row)
                .optional()
                .map_err(db)
        })
        .await
    }

    async fn init_instance(
        &self,
        public_url: &str,
        setup_token_hash: &str,
    ) -> AppResult<InstanceRecord> {
        let (url, hash) = (public_url.to_owned(), setup_token_hash.to_owned());
        self.call(move |c| {
            c.execute(
                "INSERT INTO instance (singleton, public_url, setup_token_hash) VALUES (1, ?1, ?2) \
                 ON CONFLICT(singleton) DO NOTHING",
                params![url, hash],
            )
            .map_err(db)?;
            c.query_row(SELECT, [], instance_from_row).map_err(db)
        })
        .await
    }

    async fn set_setup_token_hash(&self, hash: &str) -> AppResult<()> {
        let hash = hash.to_owned();
        self.call(move |c| {
            c.execute(
                "UPDATE instance SET setup_token_hash = ?1 WHERE singleton = 1 AND setup_completed_at IS NULL",
                [hash],
            )
            .map_err(db)?;
            Ok(())
        })
        .await
    }

    async fn complete_setup(&self, superadmin: &UserRecord, at: Timestamp) -> AppResult<()> {
        let user = superadmin.clone();
        self.call(move |c| {
            let tx = c.transaction().map_err(db)?;
            let done: Option<i64> = tx
                .query_row("SELECT setup_completed_at FROM instance WHERE singleton = 1", [], |r| r.get(0))
                .optional()
                .map_err(db)?
                .flatten();
            if done.is_some() {
                return Err(AppError::Conflict("setup already completed".into()));
            }
            insert_user_tx(&tx, &user)?;
            let changed = tx
                .execute(
                    "UPDATE instance SET setup_completed_at = ?1, setup_token_hash = NULL WHERE singleton = 1",
                    [at.0],
                )
                .map_err(db)?;
            if changed != 1 {
                return Err(AppError::Conflict("instance not initialized".into()));
            }
            tx.commit().map_err(db)
        })
        .await
    }

    async fn set_github_app(
        &self,
        sealed: &Sealed,
        owner_login: &str,
        owner_kind: &str,
    ) -> AppResult<()> {
        let (blob, login, kind) = (
            sealed.0.clone(),
            owner_login.to_owned(),
            owner_kind.to_owned(),
        );
        self.call(move |c| {
            c.execute(
                "UPDATE instance SET github_app = ?1, owner_login = ?2, owner_kind = ?3 WHERE singleton = 1",
                params![blob, login, kind],
            )
            .map_err(db)?;
            Ok(())
        })
        .await
    }

    async fn set_installation(&self, installation_id: Option<u64>) -> AppResult<()> {
        self.call(move |c| {
            c.execute(
                "UPDATE instance SET installation_id = ?1 WHERE singleton = 1",
                [installation_id.map(to_i64)],
            )
            .map_err(db)?;
            Ok(())
        })
        .await
    }

    async fn set_public_url(&self, url: &str) -> AppResult<()> {
        let url = url.to_owned();
        self.call(move |c| {
            c.execute(
                "UPDATE instance SET public_url = ?1 WHERE singleton = 1",
                [url],
            )
            .map_err(db)?;
            Ok(())
        })
        .await
    }
}
