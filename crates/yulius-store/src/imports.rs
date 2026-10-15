//! Import jobs (`yulius import` uploads, SPEC-002 §6.5).

use std::collections::BTreeMap;

use async_trait::async_trait;
use rusqlite::{OptionalExtension, Row, params};
use yulius_app::error::AppResult;
use yulius_app::ports::store_ext::{ImportJobRecord, ImportStore};
use yulius_domain::ids::{SandboxId, UserId};
use yulius_domain::time::Timestamp;

use crate::{SqliteStore, bad_column, db, to_i64, to_u64};

const COLS: &str =
    "id, sandbox_id, user_id, state, manifest, received, result, created_at, updated_at";

fn received_from_json(text: &str) -> rusqlite::Result<BTreeMap<String, u64>> {
    let map: BTreeMap<String, i64> =
        serde_json::from_str(text).map_err(|_| bad_column("received", text))?;
    Ok(map.into_iter().map(|(k, v)| (k, to_u64(v))).collect())
}

fn received_to_json(map: &BTreeMap<String, u64>) -> String {
    let as_i64: BTreeMap<&str, i64> = map.iter().map(|(k, v)| (k.as_str(), to_i64(*v))).collect();
    serde_json::to_string(&as_i64).unwrap_or_else(|_| "{}".to_owned())
}

fn from_row(row: &Row<'_>) -> rusqlite::Result<ImportJobRecord> {
    let sandbox: Option<String> = row.get("sandbox_id")?;
    let user: String = row.get("user_id")?;
    let received: String = row.get("received")?;
    Ok(ImportJobRecord {
        id: row.get("id")?,
        sandbox_id: sandbox
            .as_deref()
            .map(|s| SandboxId::parse(s).map_err(|_| bad_column("sandbox id", s)))
            .transpose()?,
        user_id: UserId::parse(&user).map_err(|_| bad_column("user id", &user))?,
        state: row.get("state")?,
        manifest: row.get("manifest")?,
        received: received_from_json(&received)?,
        result: row.get("result")?,
        created_at: Timestamp(row.get("created_at")?),
        updated_at: Timestamp(row.get("updated_at")?),
    })
}

#[async_trait]
impl ImportStore for SqliteStore {
    async fn insert_import_job(&self, job: &ImportJobRecord) -> AppResult<()> {
        let job = job.clone();
        self.call(move |c| {
            c.execute(
                &format!(
                    "INSERT INTO import_jobs ({COLS}) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)"
                ),
                params![
                    job.id,
                    job.sandbox_id.as_ref().map(SandboxId::as_str),
                    job.user_id.as_str(),
                    job.state,
                    job.manifest,
                    received_to_json(&job.received),
                    job.result,
                    job.created_at.0,
                    job.updated_at.0,
                ],
            )
            .map_err(db)?;
            Ok(())
        })
        .await
    }

    async fn import_job(&self, id: &str) -> AppResult<Option<ImportJobRecord>> {
        let id = id.to_owned();
        self.call(move |c| {
            c.query_row(
                &format!("SELECT {COLS} FROM import_jobs WHERE id = ?1"),
                [id],
                from_row,
            )
            .optional()
            .map_err(db)
        })
        .await
    }

    async fn import_jobs(
        &self,
        sandbox: &SandboxId,
        limit: u32,
    ) -> AppResult<Vec<ImportJobRecord>> {
        let sandbox = sandbox.as_str().to_owned();
        self.call(move |c| {
            let mut stmt = c
                .prepare(&format!(
                    "SELECT {COLS} FROM import_jobs WHERE sandbox_id = ?1 \
                     ORDER BY created_at DESC, rowid DESC LIMIT ?2"
                ))
                .map_err(db)?;
            let rows = stmt
                .query_map(params![sandbox, i64::from(limit)], from_row)
                .map_err(db)?;
            rows.collect::<Result<Vec<_>, _>>().map_err(db)
        })
        .await
    }

    async fn user_import_jobs(&self, user: &UserId, limit: u32) -> AppResult<Vec<ImportJobRecord>> {
        let user = user.as_str().to_owned();
        self.call(move |c| {
            let mut stmt = c
                .prepare(&format!(
                    "SELECT {COLS} FROM import_jobs WHERE user_id = ?1 \
                     ORDER BY created_at DESC, rowid DESC LIMIT ?2"
                ))
                .map_err(db)?;
            let rows = stmt
                .query_map(params![user, i64::from(limit)], from_row)
                .map_err(db)?;
            rows.collect::<Result<Vec<_>, _>>().map_err(db)
        })
        .await
    }

    async fn advance_import_file(
        &self,
        id: &str,
        name: &str,
        expected: u64,
        received: u64,
        at: Timestamp,
    ) -> AppResult<bool> {
        let (id, name) = (id.to_owned(), name.to_owned());
        self.call(move |c| {
            let tx = c.transaction().map_err(db)?;
            let current: Option<String> = tx
                .query_row(
                    "SELECT received FROM import_jobs WHERE id = ?1",
                    [&id],
                    |r| r.get(0),
                )
                .optional()
                .map_err(db)?;
            let Some(current) = current else {
                return Ok(false);
            };
            let mut map = received_from_json(&current).map_err(db)?;
            if map.get(&name).copied().unwrap_or(0) != expected {
                return Ok(false);
            }
            map.insert(name, received);
            tx.execute(
                "UPDATE import_jobs SET received = ?1, updated_at = ?2 WHERE id = ?3",
                params![received_to_json(&map), at.0, id],
            )
            .map_err(db)?;
            tx.commit().map_err(db)?;
            Ok(true)
        })
        .await
    }

    async fn transition_import_job(
        &self,
        id: &str,
        from: &[&str],
        to: &str,
        expected_updated: Option<Timestamp>,
        result: Option<&str>,
        at: Timestamp,
    ) -> AppResult<bool> {
        let id = id.to_owned();
        let from: Vec<String> = from.iter().map(|s| (*s).to_owned()).collect();
        let to = to.to_owned();
        let result = result.map(str::to_owned);
        self.call(move |c| {
            let tx = c.transaction().map_err(db)?;
            let current: Option<(String, i64)> = tx
                .query_row(
                    "SELECT state, updated_at FROM import_jobs WHERE id = ?1",
                    [&id],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()
                .map_err(db)?;
            let Some((state, updated)) = current else {
                return Ok(false);
            };
            if !from.contains(&state) || expected_updated.is_some_and(|t| t.0 != updated) {
                return Ok(false);
            }
            tx.execute(
                "UPDATE import_jobs SET state = ?1, result = COALESCE(?2, result), updated_at = ?3 WHERE id = ?4",
                params![to, result, at.0, id],
            )
            .map_err(db)?;
            tx.commit().map_err(db)?;
            Ok(true)
        })
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn job(id: &str, at: i64) -> Result<ImportJobRecord, Box<dyn std::error::Error>> {
        Ok(ImportJobRecord {
            id: id.to_owned(),
            sandbox_id: Some(SandboxId::parse("s1")?),
            user_id: UserId::parse("u1")?,
            state: "created".to_owned(),
            manifest: "{}".to_owned(),
            received: BTreeMap::from([("a.bin".to_owned(), 0), ("b.bin".to_owned(), 5)]),
            result: None,
            created_at: Timestamp(at),
            updated_at: Timestamp(at),
        })
    }

    #[tokio::test]
    async fn jobs_round_trip_and_cas_guards_progress() -> Result<(), Box<dyn std::error::Error>> {
        let store = SqliteStore::open_in_memory()?;
        store.insert_import_job(&job("j1", 1)?).await?;
        store.insert_import_job(&job("j2", 2)?).await?;
        let loaded = store.import_job("j1").await?.ok_or("missing job")?;
        assert_eq!(loaded, job("j1", 1)?);
        assert!(store.import_job("nope").await?.is_none());

        let mine = store.user_import_jobs(&UserId::parse("u1")?, 10).await?;
        assert_eq!(
            mine.iter().map(|j| j.id.as_str()).collect::<Vec<_>>(),
            ["j2", "j1"]
        );
        assert!(
            store
                .user_import_jobs(&UserId::parse("u2")?, 10)
                .await?
                .is_empty()
        );
        let listed = store.import_jobs(&SandboxId::parse("s1")?, 10).await?;
        assert_eq!(
            listed.iter().map(|j| j.id.as_str()).collect::<Vec<_>>(),
            vec!["j2", "j1"],
            "newest first"
        );
        assert_eq!(
            store.import_jobs(&SandboxId::parse("s1")?, 1).await?.len(),
            1
        );

        // Progress CAS: only from the expected length.
        assert!(
            store
                .advance_import_file("j1", "a.bin", 0, 10, Timestamp(3))
                .await?
        );
        assert!(
            !store
                .advance_import_file("j1", "a.bin", 0, 20, Timestamp(4))
                .await?
        );
        assert!(
            !store
                .advance_import_file("missing", "a.bin", 0, 1, Timestamp(4))
                .await?
        );
        let loaded = store.import_job("j1").await?.ok_or("missing job")?;
        assert_eq!(loaded.received.get("a.bin"), Some(&10));
        assert_eq!(loaded.updated_at, Timestamp(3));

        // State CAS, including the updated-at guard used for stale `applying` jobs.
        assert!(
            store
                .transition_import_job("j1", &["created"], "applying", None, None, Timestamp(5))
                .await?
        );
        assert!(
            !store
                .transition_import_job("j1", &["created"], "applying", None, None, Timestamp(6))
                .await?
        );
        assert!(
            !store
                .transition_import_job(
                    "j1",
                    &["applying"],
                    "applying",
                    Some(Timestamp(4)),
                    None,
                    Timestamp(6)
                )
                .await?
        );
        assert!(
            store
                .transition_import_job(
                    "j1",
                    &["applying"],
                    "completed",
                    Some(Timestamp(5)),
                    Some("{\"ok\":true}"),
                    Timestamp(7)
                )
                .await?
        );
        let loaded = store.import_job("j1").await?.ok_or("missing job")?;
        assert_eq!(loaded.state, "completed");
        assert_eq!(loaded.result.as_deref(), Some("{\"ok\":true}"));
        Ok(())
    }
}
