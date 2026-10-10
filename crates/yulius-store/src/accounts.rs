//! Provider accounts.

use async_trait::async_trait;
use rusqlite::{OptionalExtension, Row, params};
use yulius_app::error::AppResult;
use yulius_app::model::{AccountRecord, AccountStatus, Sealed};
use yulius_app::ports::store::AccountStore;
use yulius_domain::harness::Provider;
use yulius_domain::ids::AccountId;
use yulius_domain::time::Timestamp;

use crate::{SqliteStore, bad_column, db};

const COLS: &str =
    "id, provider, label, credential, status, usage, last_error, created_at, updated_at";

fn from_row(row: &Row<'_>) -> rusqlite::Result<AccountRecord> {
    let id: String = row.get("id")?;
    let provider: String = row.get("provider")?;
    let status: String = row.get("status")?;
    Ok(AccountRecord {
        id: AccountId::parse(&id).map_err(|_| bad_column("account id", &id))?,
        provider: Provider::parse(&provider).map_err(|_| bad_column("provider", &provider))?,
        label: row.get("label")?,
        credential: row.get::<_, Option<Vec<u8>>>("credential")?.map(Sealed),
        status: AccountStatus::parse(&status),
        usage_json: row.get("usage")?,
        last_error: row.get("last_error")?,
        created_at: Timestamp(row.get("created_at")?),
        updated_at: Timestamp(row.get("updated_at")?),
    })
}

#[async_trait]
impl AccountStore for SqliteStore {
    async fn insert_account(&self, a: &AccountRecord) -> AppResult<()> {
        let a = a.clone();
        self.call(move |c| {
            c.execute(
                &format!(
                    "INSERT INTO accounts ({COLS}) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)"
                ),
                params![
                    a.id.as_str(),
                    a.provider.as_str(),
                    a.label,
                    a.credential.map(|s| s.0),
                    a.status.as_str(),
                    a.usage_json,
                    a.last_error,
                    a.created_at.0,
                    a.updated_at.0
                ],
            )
            .map_err(db)?;
            Ok(())
        })
        .await
    }

    async fn account(&self, id: &AccountId) -> AppResult<Option<AccountRecord>> {
        let id = id.as_str().to_owned();
        self.call(move |c| {
            c.query_row(
                &format!("SELECT {COLS} FROM accounts WHERE id = ?1"),
                [id],
                from_row,
            )
            .optional()
            .map_err(db)
        })
        .await
    }

    async fn accounts(&self) -> AppResult<Vec<AccountRecord>> {
        self.call(|c| {
            let mut stmt = c
                .prepare(&format!(
                    "SELECT {COLS} FROM accounts ORDER BY provider, created_at, id"
                ))
                .map_err(db)?;
            let rows = stmt.query_map([], from_row).map_err(db)?;
            rows.collect::<Result<Vec<_>, _>>().map_err(db)
        })
        .await
    }

    async fn update_account(&self, id: &AccountId, label: &str, at: Timestamp) -> AppResult<()> {
        let (id, label) = (id.as_str().to_owned(), label.to_owned());
        self.call(move |c| {
            c.execute(
                "UPDATE accounts SET label = ?1, updated_at = ?2 WHERE id = ?3",
                params![label, at.0, id],
            )
            .map_err(db)?;
            Ok(())
        })
        .await
    }

    async fn set_account_credential(
        &self,
        id: &AccountId,
        credential: &Sealed,
        status: AccountStatus,
        at: Timestamp,
    ) -> AppResult<()> {
        let (id, cred) = (id.as_str().to_owned(), credential.0.clone());
        self.call(move |c| {
            c.execute(
                "UPDATE accounts SET credential = ?1, status = ?2, last_error = NULL, updated_at = ?3 WHERE id = ?4",
                params![cred, status.as_str(), at.0, id],
            )
            .map_err(db)?;
            Ok(())
        })
        .await
    }

    async fn set_account_state(
        &self,
        id: &AccountId,
        status: AccountStatus,
        usage_json: Option<&str>,
        last_error: Option<&str>,
        at: Timestamp,
    ) -> AppResult<()> {
        let (id, usage, err) = (
            id.as_str().to_owned(),
            usage_json.map(str::to_owned),
            last_error.map(str::to_owned),
        );
        self.call(move |c| {
            c.execute(
                "UPDATE accounts SET status = ?1, usage = COALESCE(?2, usage), last_error = ?3, updated_at = ?4 WHERE id = ?5",
                params![status.as_str(), usage, err, at.0, id],
            )
            .map_err(db)?;
            Ok(())
        })
        .await
    }

    async fn set_account_error(
        &self,
        id: &AccountId,
        last_error: &str,
        at: Timestamp,
    ) -> AppResult<()> {
        let (id, err) = (id.as_str().to_owned(), last_error.to_owned());
        self.call(move |c| {
            c.execute(
                "UPDATE accounts SET last_error = ?1, updated_at = ?2 WHERE id = ?3",
                params![err, at.0, id],
            )
            .map_err(db)?;
            Ok(())
        })
        .await
    }

    async fn account_tokens(
        &self,
        since: Timestamp,
    ) -> AppResult<Vec<yulius_app::model::AccountTokens>> {
        self.call(move |c| {
            let mut statement = c
                .prepare(
                    "SELECT a.account_id, COUNT(*), COUNT(DISTINCT a.task_id), \
                     SUM(m.input_tokens), SUM(m.output_tokens), SUM(m.cache_read), SUM(m.cache_write) \
                     FROM attempts a LEFT JOIN metrics m ON m.attempt_id = a.id \
                     WHERE a.account_id IS NOT NULL AND a.started_at >= ?1 GROUP BY a.account_id",
                )
                .map_err(db)?;
            let rows = statement
                .query_map([since.0], |row| {
                    let opt = |i: usize| row.get::<_, Option<i64>>(i).map(|v| v.map(crate::to_u64));
                    Ok(yulius_app::model::AccountTokens {
                        account_id: row.get(0)?,
                        runs: crate::to_u32(row.get(1)?),
                        tasks: crate::to_u32(row.get(2)?),
                        input: opt(3)?,
                        output: opt(4)?,
                        cache_read: opt(5)?,
                        cache_write: opt(6)?,
                    })
                })
                .map_err(db)?;
            rows.collect::<Result<Vec<_>, _>>().map_err(db)
        })
        .await
    }

    async fn delete_account(&self, id: &AccountId) -> AppResult<()> {
        let id = id.as_str().to_owned();
        self.call(move |c| {
            c.execute("DELETE FROM accounts WHERE id = ?1", [id])
                .map_err(db)?;
            Ok(())
        })
        .await
    }
}
