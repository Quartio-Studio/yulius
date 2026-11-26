//! The end-of-day digest (SPEC-512 part 3, migration 0070): the instance's setting and last run,
//! and each person's digests.

use async_trait::async_trait;
use rusqlite::{OptionalExtension, params};
use yulius_app::error::{AppError, AppResult};
use yulius_app::ports::store_ext::{DigestStore, StoredDigest};
use yulius_domain::ids::UserId;
use yulius_domain::time::Timestamp;

use crate::SqliteStore;

/// How many digests a person keeps (DIG-005).
const KEPT: i64 = 7;

#[async_trait]
impl DigestStore for SqliteStore {
    async fn digest_setting(&self) -> AppResult<(Option<String>, Option<Timestamp>)> {
        self.call(|c| {
            let row = c
                .query_row(
                    "SELECT digest, digest_ran_at FROM instance WHERE singleton = 1",
                    [],
                    |r| Ok((r.get::<_, Option<String>>(0)?, r.get::<_, Option<i64>>(1)?)),
                )
                .optional()?;
            Ok(row.map_or((None, None), |(doc, ran)| (doc, ran.map(Timestamp))))
        })
        .await
    }

    async fn set_digest_setting(&self, doc: &str, ran_at: Timestamp) -> AppResult<()> {
        let doc = doc.to_owned();
        self.call(move |c| {
            let changed = c.execute(
                "UPDATE instance SET digest = ?1, digest_ran_at = ?2 WHERE singleton = 1",
                params![doc, ran_at.0],
            )?;
            if changed == 0 {
                return Err(AppError::Conflict("the instance is not set up yet".into()).into());
            }
            Ok(())
        })
        .await
    }

    async fn take_digest_run(&self, seen: Option<Timestamp>, now: Timestamp) -> AppResult<bool> {
        self.call(move |c| {
            let changed = c.execute(
                "UPDATE instance SET digest_ran_at = ?1, digest_sent_at = ?1 \
                     WHERE singleton = 1 AND digest_ran_at IS ?2",
                params![now.0, seen.map(|t| t.0)],
            )?;
            Ok(changed == 1)
        })
        .await
    }

    async fn digest_sent_at(&self) -> AppResult<Option<Timestamp>> {
        self.call(|c| {
            let at = c
                .query_row(
                    "SELECT digest_sent_at FROM instance WHERE singleton = 1",
                    [],
                    |r| r.get::<_, Option<i64>>(0),
                )
                .optional()?;
            Ok(at.flatten().map(Timestamp))
        })
        .await
    }

    async fn digest_listed_at(&self) -> AppResult<Option<Timestamp>> {
        self.call(|c| {
            let at = c
                .query_row(
                    "SELECT digest_listed_at FROM instance WHERE singleton = 1",
                    [],
                    |r| r.get::<_, Option<i64>>(0),
                )
                .optional()?;
            Ok(at.flatten().map(Timestamp))
        })
        .await
    }

    async fn set_digest_listed_at(&self, at: Timestamp) -> AppResult<()> {
        self.call(move |c| {
            let changed = c.execute(
                "UPDATE instance SET digest_listed_at = ?1 WHERE singleton = 1",
                [at.0],
            )?;
            if changed == 0 {
                return Err(AppError::Conflict("the instance is not set up yet".into()).into());
            }
            Ok(())
        })
        .await
    }

    async fn put_digest(&self, user: &UserId, digest: &StoredDigest) -> AppResult<()> {
        let user = user.as_str().to_owned();
        let digest = digest.clone();
        self.call(move |c| {
            let tx = c.transaction()?;
            tx.execute(
                "INSERT INTO digests (id, user_id, created_at, body) VALUES (?1, ?2, ?3, ?4)",
                params![digest.id, user, digest.created_at.0, digest.body],
            )?;
            tx.execute(
                "DELETE FROM digests WHERE user_id = ?1 AND id NOT IN \
                 (SELECT id FROM digests WHERE user_id = ?1 \
                  ORDER BY created_at DESC, rowid DESC LIMIT ?2)",
                params![user, KEPT],
            )?;
            Ok(tx.commit()?)
        })
        .await
    }

    async fn latest_digest(&self, user: &UserId) -> AppResult<Option<StoredDigest>> {
        let user = user.as_str().to_owned();
        self.call(move |c| {
            Ok(c.query_row(
                "SELECT id, created_at, body FROM digests WHERE user_id = ?1 \
                 ORDER BY created_at DESC, rowid DESC LIMIT 1",
                [user],
                |r| {
                    Ok(StoredDigest {
                        id: r.get(0)?,
                        created_at: Timestamp(r.get(1)?),
                        body: r.get(2)?,
                    })
                },
            )
            .optional()?)
        })
        .await
    }
}

#[cfg(test)]
mod tests {
    use rusqlite::Connection;

    use super::*;

    fn digest(id: &str, at: i64) -> StoredDigest {
        StoredDigest {
            id: id.into(),
            created_at: Timestamp(at),
            body: format!("{{\"id\":\"{id}\"}}"),
        }
    }

    /// DIG-001, DIG-002: the setting is kept with its last run; the day's run is taken once,
    /// only by whoever read the last run as it still is.
    #[tokio::test]
    async fn the_days_run_is_taken_once() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("y.db");
        let store = SqliteStore::open(&path)?;
        assert!(matches!(
            store.set_digest_setting("{}", Timestamp(1)).await,
            Err(AppError::Conflict(_))
        ));
        Connection::open(&path)?.execute_batch(
            "INSERT INTO instance (singleton, public_url) VALUES (1, 'https://yulius.test')",
        )?;
        assert_eq!(store.digest_setting().await?, (None, None));
        store
            .set_digest_setting(r#"{"enabled":true}"#, Timestamp(10))
            .await?;
        assert_eq!(
            store.digest_setting().await?,
            (Some(r#"{"enabled":true}"#.into()), Some(Timestamp(10)))
        );
        assert!(
            store
                .take_digest_run(Some(Timestamp(10)), Timestamp(20))
                .await?
        );
        assert!(
            !store
                .take_digest_run(Some(Timestamp(10)), Timestamp(21))
                .await?,
            "a second taker read the run before the first took it"
        );
        assert!(!store.take_digest_run(None, Timestamp(22)).await?);
        assert_eq!(store.digest_setting().await?.1, Some(Timestamp(20)));
        assert_eq!(
            store.digest_sent_at().await?,
            Some(Timestamp(20)),
            "the claim"
        );
        // Where the listing ended is its own mark: saving the setting leaves it.
        assert_eq!(store.digest_listed_at().await?, None);
        store.set_digest_listed_at(Timestamp(25)).await?;
        store.set_digest_setting("{}", Timestamp(30)).await?;
        assert_eq!(store.digest_listed_at().await?, Some(Timestamp(25)));
        assert_eq!(
            store.digest_sent_at().await?,
            Some(Timestamp(20)),
            "a save leaves it"
        );
        Ok(())
    }

    /// DIG-005: a person keeps their last seven digests; the latest is read back.
    #[tokio::test]
    async fn a_person_keeps_the_last_seven_digests() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("y.db");
        let store = SqliteStore::open(&path)?;
        Connection::open(&path)?.execute_batch(
            "INSERT INTO users (id, display_name, is_superadmin, is_instance_admin, created_at) \
             VALUES ('u1', 'U', 0, 0, 1), ('u2', 'V', 0, 0, 1);",
        )?;
        let u1 = UserId::parse("u1")?;
        let u2 = UserId::parse("u2")?;
        assert_eq!(store.latest_digest(&u1).await?, None);
        for n in 1..=9 {
            store.put_digest(&u1, &digest(&format!("d{n}"), n)).await?;
        }
        store.put_digest(&u2, &digest("other", 100)).await?;
        assert_eq!(store.latest_digest(&u1).await?, Some(digest("d9", 9)));
        let kept: i64 = Connection::open(&path)?.query_row(
            "SELECT COUNT(*) FROM digests WHERE user_id = 'u1' AND id IN ('d3','d4','d5','d6','d7','d8','d9')",
            [],
            |r| r.get(0),
        )?;
        let all: i64 = Connection::open(&path)?.query_row(
            "SELECT COUNT(*) FROM digests WHERE user_id = 'u1'",
            [],
            |r| r.get(0),
        )?;
        assert_eq!((kept, all), (7, 7));
        assert_eq!(store.latest_digest(&u2).await?, Some(digest("other", 100)));
        Ok(())
    }
}
