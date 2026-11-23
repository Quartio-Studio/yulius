//! Script watches (SPEC-630, migration 0093): each watch's definition and the state its runs
//! leave, and the posts it could not store yet (WCH-008), newest per notice key.

use async_trait::async_trait;
use rusqlite::{OptionalExtension, Row, params};
use yulius_app::error::{AppError, AppResult};
use yulius_app::ports::store_ext::{
    WatchPending, WatchRecord, WatchRunEnd, WatchState, WatchStore,
};
use yulius_domain::ids::{SandboxId, UserId};
use yulius_domain::time::Timestamp;

use crate::{SqliteStore, bad_column, db};

const COLUMNS: &str = "id, sandbox_id, name, script, every_seconds, cron, utc_offset_minutes, \
     expiry, timeout_seconds, secrets, owner_id, created_via, created_at, updated_at, paused, \
     since, ended_reason, ended_at, run_id, running_since, last_started_at, last_ended_at, \
     last_exit, last_failure, last_output, last_errors, failures, warned, runs, posts, \
     last_posted_at";

fn ts(value: Option<i64>) -> Option<Timestamp> {
    value.map(Timestamp)
}

fn watch_row(row: &Row<'_>) -> rusqlite::Result<WatchRecord> {
    let sandbox: String = row.get(1)?;
    let owner: String = row.get(10)?;
    let secrets: String = row.get(9)?;
    let run_id: Option<String> = row.get(18)?;
    let running_since: Option<i64> = row.get(19)?;
    Ok(WatchRecord {
        id: row.get(0)?,
        sandbox_id: SandboxId::parse(&sandbox).map_err(|_| bad_column("sandbox id", &sandbox))?,
        name: row.get(2)?,
        script: row.get(3)?,
        every_seconds: row.get(4)?,
        cron: row.get(5)?,
        utc_offset_minutes: row.get(6)?,
        expiry: row.get(7)?,
        timeout_seconds: row.get(8)?,
        secrets: serde_json::from_str(&secrets).map_err(|_| bad_column("secrets", &secrets))?,
        owner: UserId::parse(&owner).map_err(|_| bad_column("owner id", &owner))?,
        created_via: row.get(11)?,
        created_at: Timestamp(row.get(12)?),
        updated_at: Timestamp(row.get(13)?),
        paused: row.get(14)?,
        since: Timestamp(row.get(15)?),
        ended_reason: row.get(16)?,
        ended_at: ts(row.get(17)?),
        state: WatchState {
            running: run_id.zip(running_since.map(Timestamp)),
            last_started_at: ts(row.get(20)?),
            last_ended_at: ts(row.get(21)?),
            last_exit: row.get(22)?,
            last_failure: row.get(23)?,
            last_output: row.get(24)?,
            last_errors: row.get(25)?,
            failures: row.get(26)?,
            warned: row.get(27)?,
            runs: row.get(28)?,
            posts: row.get(29)?,
            last_posted_at: ts(row.get(30)?),
        },
    })
}

/// A unique name taken: said as such; any other failure as `db` says it.
fn named(err: rusqlite::Error) -> AppError {
    let unique = matches!(
        &err,
        rusqlite::Error::SqliteFailure(code, _)
            if code.extended_code == rusqlite::ffi::SQLITE_CONSTRAINT_UNIQUE
    );
    if unique {
        return AppError::Conflict("this project already has a watch of that name".into());
    }
    db(err)
}

fn secrets_json(secrets: &[String]) -> AppResult<String> {
    serde_json::to_string(secrets).map_err(|e| AppError::Internal(e.to_string()))
}

fn put_pending(c: &rusqlite::Connection, p: &WatchPending) -> rusqlite::Result<usize> {
    c.execute(
        "INSERT INTO watch_pending (watch_id, notice_key, body, queued_at) VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT (watch_id, notice_key) DO UPDATE SET body = excluded.body,
             queued_at = excluded.queued_at",
        params![p.watch_id, p.key, p.body, p.queued_at.0],
    )
}

#[async_trait]
impl WatchStore for SqliteStore {
    async fn all_watches(&self) -> AppResult<Vec<WatchRecord>> {
        self.call(move |c| {
            let mut stmt = c
                .prepare(&format!(
                    "SELECT {COLUMNS} FROM watches WHERE sandbox_id IN \
                     (SELECT id FROM sandboxes WHERE removed_at IS NULL) ORDER BY sandbox_id, name"
                ))
                .map_err(db)?;
            let rows = stmt
                .query_map([], watch_row)
                .map_err(db)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(db)?;
            Ok(rows)
        })
        .await
    }

    async fn watches_in(&self, sandbox: &SandboxId) -> AppResult<Vec<WatchRecord>> {
        let sandbox = sandbox.as_str().to_owned();
        self.call(move |c| {
            let mut stmt = c
                .prepare(&format!(
                    "SELECT {COLUMNS} FROM watches WHERE sandbox_id = ?1 ORDER BY name"
                ))
                .map_err(db)?;
            let rows = stmt
                .query_map([sandbox], watch_row)
                .map_err(db)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(db)?;
            Ok(rows)
        })
        .await
    }

    async fn watch(&self, id: &str) -> AppResult<Option<WatchRecord>> {
        let id = id.to_owned();
        self.call(move |c| {
            c.query_row(
                &format!("SELECT {COLUMNS} FROM watches WHERE id = ?1"),
                [id],
                watch_row,
            )
            .optional()
            .map_err(db)
        })
        .await
    }

    async fn insert_watch(&self, watch: &WatchRecord, max: usize) -> AppResult<()> {
        let r = watch.clone();
        let secrets = secrets_json(&r.secrets)?;
        let max = i64::try_from(max).unwrap_or(i64::MAX);
        self.call(move |c| {
            let tx = c.transaction().map_err(db)?;
            let count: i64 = tx
                .query_row(
                    "SELECT COUNT(*) FROM watches WHERE sandbox_id = ?1",
                    [r.sandbox_id.as_str()],
                    |row| row.get(0),
                )
                .map_err(db)?;
            if count >= max {
                return Err(AppError::Conflict(format!(
                    "a project has at most {max} watches; remove one first"
                )));
            }
            tx.execute(
                "INSERT INTO watches (id, sandbox_id, name, script, every_seconds, cron,
                     utc_offset_minutes, expiry, timeout_seconds, secrets, owner_id, created_via,
                     created_at, updated_at, paused, since)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)",
                params![
                    r.id,
                    r.sandbox_id.as_str(),
                    r.name,
                    r.script,
                    r.every_seconds,
                    r.cron,
                    r.utc_offset_minutes,
                    r.expiry,
                    r.timeout_seconds,
                    secrets,
                    r.owner.as_str(),
                    r.created_via,
                    r.created_at.0,
                    r.updated_at.0,
                    r.paused,
                    r.since.0,
                ],
            )
            .map_err(named)?;
            tx.commit().map_err(db)
        })
        .await
    }

    async fn update_watch(&self, watch: &WatchRecord, read_at: Timestamp) -> AppResult<bool> {
        let r = watch.clone();
        let secrets = secrets_json(&r.secrets)?;
        self.call(move |c| {
            let changed = c
                .execute(
                    "UPDATE watches SET script = ?1, every_seconds = ?2, cron = ?3,
                         utc_offset_minutes = ?4, expiry = ?5, timeout_seconds = ?6,
                         secrets = ?7, updated_at = ?8, since = ?9
                     WHERE id = ?10 AND updated_at = ?11",
                    params![
                        r.script,
                        r.every_seconds,
                        r.cron,
                        r.utc_offset_minutes,
                        r.expiry,
                        r.timeout_seconds,
                        secrets,
                        r.updated_at.0,
                        r.since.0,
                        r.id,
                        read_at.0,
                    ],
                )
                .map_err(named)?;
            Ok(changed == 1)
        })
        .await
    }

    async fn set_watch_paused(&self, id: &str, paused: bool, at: Timestamp) -> AppResult<bool> {
        let id = id.to_owned();
        self.call(move |c| {
            let changed = if paused {
                c.execute("UPDATE watches SET paused = 1 WHERE id = ?1", [id])
            } else {
                c.execute(
                    "UPDATE watches SET paused = 0, ended_reason = NULL, ended_at = NULL,
                         since = ?2, last_started_at = NULL WHERE id = ?1",
                    params![id, at.0],
                )
            }
            .map_err(db)?;
            Ok(changed == 1)
        })
        .await
    }

    async fn watch_due_again(&self, id: &str, at: Timestamp) -> AppResult<bool> {
        let id = id.to_owned();
        self.call(move |c| {
            let changed = c
                .execute(
                    "UPDATE watches SET since = ?2, last_started_at = NULL
                     WHERE id = ?1 AND paused = 0 AND ended_reason IS NULL AND run_id IS NULL",
                    params![id, at.0],
                )
                .map_err(db)?;
            Ok(changed == 1)
        })
        .await
    }

    async fn delete_watch(&self, id: &str) -> AppResult<bool> {
        let id = id.to_owned();
        self.call(move |c| {
            let tx = c.transaction().map_err(db)?;
            tx.execute("DELETE FROM watch_pending WHERE watch_id = ?1", [&id])
                .map_err(db)?;
            let changed = tx
                .execute("DELETE FROM watches WHERE id = ?1", [&id])
                .map_err(db)?;
            tx.commit().map_err(db)?;
            Ok(changed == 1)
        })
        .await
    }

    async fn watch_run_started(
        &self,
        id: &str,
        run_id: &str,
        at: Timestamp,
        forced: bool,
    ) -> AppResult<bool> {
        let (id, run_id) = (id.to_owned(), run_id.to_owned());
        self.call(move |c| {
            let changed = c
                .execute(
                    "UPDATE watches SET run_id = ?2, running_since = ?3, last_started_at = ?3
                     WHERE id = ?1 AND run_id IS NULL
                         AND (?4 OR (paused = 0 AND ended_reason IS NULL))
                         AND sandbox_id IN (SELECT id FROM sandboxes WHERE removed_at IS NULL)",
                    params![id, run_id, at.0, forced],
                )
                .map_err(db)?;
            Ok(changed == 1)
        })
        .await
    }

    async fn watch_run_ended(&self, id: &str, run_id: &str, end: &WatchRunEnd) -> AppResult<bool> {
        let (id, run_id, end) = (id.to_owned(), run_id.to_owned(), end.clone());
        self.call(move |c| {
            let tx = c.transaction().map_err(db)?;
            let s = &end.state;
            let changed = tx
                .execute(
                    "UPDATE watches SET run_id = NULL, running_since = NULL,
                         last_ended_at = ?3, last_exit = ?4, last_failure = ?5, last_output = ?6,
                         last_errors = ?7, failures = ?8, warned = ?9, runs = ?10,
                         ended_reason = COALESCE(?11, ended_reason),
                         ended_at = CASE WHEN ?11 IS NULL THEN ended_at ELSE ?3 END
                     WHERE id = ?1 AND run_id = ?2",
                    params![
                        id,
                        run_id,
                        s.last_ended_at.map(|t| t.0),
                        s.last_exit,
                        s.last_failure,
                        s.last_output,
                        s.last_errors,
                        s.failures,
                        s.warned,
                        s.runs,
                        end.ended,
                    ],
                )
                .map_err(db)?;
            if changed != 1 {
                return Ok(false);
            }
            for pending in &end.pending {
                put_pending(&tx, pending).map_err(db)?;
            }
            tx.commit().map_err(db)?;
            Ok(true)
        })
        .await
    }

    async fn watch_pending(&self) -> AppResult<Vec<WatchPending>> {
        self.call(move |c| {
            let mut stmt = c
                .prepare(
                    "SELECT watch_id, notice_key, body, queued_at FROM watch_pending
                     ORDER BY queued_at, watch_id, notice_key",
                )
                .map_err(db)?;
            let rows = stmt
                .query_map([], |row| {
                    Ok(WatchPending {
                        watch_id: row.get(0)?,
                        key: row.get(1)?,
                        body: row.get(2)?,
                        queued_at: Timestamp(row.get(3)?),
                    })
                })
                .map_err(db)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(db)?;
            Ok(rows)
        })
        .await
    }

    async fn put_watch_pending(&self, pending: &WatchPending) -> AppResult<()> {
        let pending = pending.clone();
        self.call(move |c| put_pending(c, &pending).map(|_| ()).map_err(db))
            .await
    }

    async fn drop_watch_pending(
        &self,
        watch_id: &str,
        key: &str,
        queued_at: Timestamp,
    ) -> AppResult<()> {
        let (watch_id, key) = (watch_id.to_owned(), key.to_owned());
        self.call(move |c| {
            c.execute(
                "DELETE FROM watch_pending WHERE watch_id = ?1 AND notice_key = ?2
                     AND queued_at = ?3",
                params![watch_id, key, queued_at.0],
            )
            .map(|_| ())
            .map_err(db)
        })
        .await
    }

    async fn watch_posted(&self, id: &str, at: Timestamp) -> AppResult<()> {
        let id = id.to_owned();
        self.call(move |c| {
            c.execute(
                "UPDATE watches SET posts = posts + 1, last_posted_at = ?2 WHERE id = ?1",
                params![id, at.0],
            )
            .map(|_| ())
            .map_err(db)
        })
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    async fn store() -> Result<SqliteStore, Box<dyn std::error::Error>> {
        let store = SqliteStore::open_in_memory()?;
        store
            .call(|c| {
                c.execute_batch(
                    "INSERT INTO users (id, display_name, created_at) VALUES ('u1', 'U', 1);
                     INSERT INTO sandboxes (id, slug, repo_owner, repo_name, default_branch, profile, port_base, port_count, created_by, onboarding_state, created_at)
                       VALUES ('s1', 's', 'o', 'r', 'main', '{}', 20000, 10, 'u1', 'not_needed', 1),
                              ('s2', 't', 'o', 'q', 'main', '{}', 20010, 10, 'u1', 'not_needed', 1);",
                )
                .map_err(db)
            })
            .await?;
        Ok(store)
    }

    fn watch(
        id: &str,
        sandbox: &str,
        name: &str,
    ) -> Result<WatchRecord, Box<dyn std::error::Error>> {
        Ok(WatchRecord {
            id: id.into(),
            sandbox_id: SandboxId::parse(sandbox)?,
            name: name.into(),
            script: "echo hi".into(),
            every_seconds: Some(300),
            cron: None,
            utc_offset_minutes: 0,
            expiry: Some("2h".into()),
            timeout_seconds: 60,
            secrets: vec!["API_KEY".into()],
            owner: UserId::parse("u1")?,
            created_via: "ui".into(),
            created_at: Timestamp(10),
            updated_at: Timestamp(10),
            paused: false,
            since: Timestamp(10),
            ended_reason: None,
            ended_at: None,
            state: WatchState::default(),
        })
    }

    #[tokio::test]
    async fn watches_are_unique_per_project_and_bounded() -> TestResult {
        let store = store().await?;
        let first = watch("wt_1", "s1", "disk")?;
        store.insert_watch(&first, 2).await?;
        assert_eq!(store.watch("wt_1").await?, Some(first.clone()));
        let same = store.insert_watch(&watch("wt_2", "s1", "disk")?, 2).await;
        assert!(matches!(same, Err(AppError::Conflict(m)) if m.contains("name")));
        store.insert_watch(&watch("wt_3", "s2", "disk")?, 2).await?;
        store.insert_watch(&watch("wt_4", "s1", "ci")?, 2).await?;
        let full = store.insert_watch(&watch("wt_5", "s1", "more")?, 2).await;
        assert!(matches!(full, Err(AppError::Conflict(m)) if m.contains("at most 2")));
        let names: Vec<_> = store
            .watches_in(&SandboxId::parse("s1")?)
            .await?
            .into_iter()
            .map(|w| w.name)
            .collect();
        assert_eq!(names, ["ci", "disk"]);
        assert_eq!(store.all_watches().await?.len(), 3);

        let mut edited = first.clone();
        edited.script = "echo changed".into();
        edited.name = "renamed".into();
        edited.updated_at = Timestamp(20);
        assert!(store.update_watch(&edited, Timestamp(10)).await?);
        assert!(!store.update_watch(&edited, Timestamp(10)).await?, "stale");
        let kept = store.watch("wt_1").await?.ok_or("gone")?;
        assert_eq!(
            (kept.name.as_str(), kept.script.as_str()),
            ("disk", "echo changed")
        );
        Ok(())
    }

    /// Review round 1: a removed project's watches are not scheduled and cannot be claimed; a
    /// paused or ended watch is claimed only by Run now (`forced`).
    #[tokio::test]
    async fn claims_skip_removed_projects_and_paused_watches() -> TestResult {
        let store = store().await?;
        store
            .insert_watch(&watch("wt_1", "s1", "disk")?, 20)
            .await?;
        store
            .insert_watch(&watch("wt_2", "s2", "disk")?, 20)
            .await?;
        store
            .call(|c| {
                c.execute("UPDATE sandboxes SET removed_at = 5 WHERE id = 's2'", [])
                    .map_err(db)
            })
            .await?;
        let listed: Vec<_> = store
            .all_watches()
            .await?
            .into_iter()
            .map(|w| w.id)
            .collect();
        assert_eq!(listed, ["wt_1"]);
        assert!(
            !store
                .watch_run_started("wt_2", "r0", Timestamp(90), true)
                .await?
        );

        assert!(store.set_watch_paused("wt_1", true, Timestamp(95)).await?);
        assert!(
            !store
                .watch_run_started("wt_1", "r1", Timestamp(100), false)
                .await?
        );
        assert!(
            !store.watch_due_again("wt_1", Timestamp(100)).await?,
            "paused stays paused"
        );
        assert!(
            store
                .watch_run_started("wt_1", "r2", Timestamp(101), true)
                .await?
        );
        assert!(
            !store.watch_due_again("wt_1", Timestamp(102)).await?,
            "running"
        );
        Ok(())
    }

    /// WCH-010: a run is recorded as going before it runs, one at a time, and its end only by
    /// that run; exit 3 ends the watch, and a resume ends its end.
    #[tokio::test]
    async fn runs_are_claimed_one_at_a_time_and_ended_by_their_own_id() -> TestResult {
        let store = store().await?;
        store
            .insert_watch(&watch("wt_1", "s1", "disk")?, 20)
            .await?;
        assert!(
            store
                .watch_run_started("wt_1", "r1", Timestamp(100), false)
                .await?
        );
        assert!(
            !store
                .watch_run_started("wt_1", "r2", Timestamp(101), false)
                .await?
        );
        let going = store.watch("wt_1").await?.ok_or("gone")?;
        assert_eq!(going.state.running, Some(("r1".into(), Timestamp(100))));
        assert_eq!(going.state.last_started_at, Some(Timestamp(100)));

        let end = WatchRunEnd {
            state: WatchState {
                last_ended_at: Some(Timestamp(150)),
                last_exit: Some(3),
                last_output: Some("due".into()),
                runs: 1,
                ..WatchState::default()
            },
            ended: Some("done".into()),
            pending: vec![WatchPending {
                watch_id: "wt_1".into(),
                key: "watch:disk".into(),
                body: Some("{}".into()),
                queued_at: Timestamp(150),
            }],
        };
        assert!(
            !store.watch_run_ended("wt_1", "r2", &end).await?,
            "not its run"
        );
        assert!(store.watch_pending().await?.is_empty());
        assert!(store.watch_run_ended("wt_1", "r1", &end).await?);
        let ended = store.watch("wt_1").await?.ok_or("gone")?;
        assert_eq!(ended.state.running, None);
        assert_eq!(ended.state.last_exit, Some(3));
        assert_eq!(ended.state.last_started_at, Some(Timestamp(100)));
        assert_eq!(ended.ended_reason.as_deref(), Some("done"));
        assert_eq!(ended.ended_at, Some(Timestamp(150)));
        assert_eq!(store.watch_pending().await?.len(), 1);

        assert!(store.set_watch_paused("wt_1", true, Timestamp(160)).await?);
        assert!(
            store
                .set_watch_paused("wt_1", false, Timestamp(170))
                .await?
        );
        let resumed = store.watch("wt_1").await?.ok_or("gone")?;
        assert!(!resumed.paused);
        assert_eq!(
            (resumed.ended_reason, resumed.since),
            (None, Timestamp(170))
        );
        assert_eq!(resumed.state.last_started_at, None);

        store.watch_posted("wt_1", Timestamp(180)).await?;
        let posted = store.watch("wt_1").await?.ok_or("gone")?;
        assert_eq!(
            (posted.state.posts, posted.state.last_posted_at),
            (1, Some(Timestamp(180)))
        );

        assert!(store.delete_watch("wt_1").await?);
        assert!(
            store.watch_pending().await?.is_empty(),
            "pending posts go with it"
        );
        assert!(!store.delete_watch("wt_1").await?);
        Ok(())
    }

    /// WCH-008: pending posts are kept newest per key, and dropped only as queued.
    #[tokio::test]
    async fn pending_posts_keep_the_newest_per_key() -> TestResult {
        let store = store().await?;
        store
            .insert_watch(&watch("wt_1", "s1", "disk")?, 20)
            .await?;
        let pending = |key: &str, body: Option<&str>, at: i64| WatchPending {
            watch_id: "wt_1".into(),
            key: key.into(),
            body: body.map(Into::into),
            queued_at: Timestamp(at),
        };
        store
            .put_watch_pending(&pending("watch:disk", Some("a"), 1))
            .await?;
        store
            .put_watch_pending(&pending("watch:disk:failing", None, 2))
            .await?;
        store
            .put_watch_pending(&pending("watch:disk", Some("b"), 3))
            .await?;
        let kept = store.watch_pending().await?;
        assert_eq!(
            kept,
            [
                pending("watch:disk:failing", None, 2),
                pending("watch:disk", Some("b"), 3)
            ]
        );
        // A retry of the older one does not drop the newer.
        store
            .drop_watch_pending("wt_1", "watch:disk", Timestamp(1))
            .await?;
        assert_eq!(store.watch_pending().await?.len(), 2);
        store
            .drop_watch_pending("wt_1", "watch:disk", Timestamp(3))
            .await?;
        assert_eq!(
            store.watch_pending().await?,
            [pending("watch:disk:failing", None, 2)]
        );
        Ok(())
    }
}
