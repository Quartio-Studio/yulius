//! SQLite persistence adapter: schema, migrations, repositories and backup (SPEC-002 §5).
//!
//! All database access runs on one dedicated thread that owns the connection (single writer,
//! WAL mode). Async callers submit closures through a bounded queue and await the result, so
//! the async executor never blocks on SQLite and concurrent writers never race.

mod accounts;
mod analytics;
mod audit;
mod automations;
mod capacity;
mod ci;
mod coordination;
pub mod crypto;
mod delivery;
mod imports;
mod instance;
mod intake;
mod mcp;
mod notes;
mod sandboxes;
mod session_docs;
mod sessions;
mod skills;
mod tasks;
mod updates;
mod users;
mod vault;
mod workflows;

use std::path::Path;
use std::sync::mpsc;
use std::thread;

use rusqlite::Connection;
use tokio::sync::oneshot;
use yulius_app::error::{AppError, AppResult};

/// Bound of pending database jobs.
const QUEUE_BOUND: usize = 1024;

/// Ordered migrations; index + 1 is the schema version stored in `PRAGMA user_version`.
const MIGRATIONS: &[&str] = &[
    include_str!("migrations/0001_initial.sql"),
    include_str!("migrations/0002_instance_feedback.sql"),
    include_str!("migrations/0003_claude_only.sql"),
    include_str!("migrations/0004_task_intake.sql"),
    include_str!("migrations/0005_subscription_pool.sql"),
    include_str!("migrations/0006_task_auto_choices.sql"),
    include_str!("migrations/0007_step_effort.sql"),
    include_str!("migrations/0008_updates_and_jev_key.sql"),
    include_str!("migrations/0009_secrets_vault.sql"),
    include_str!("migrations/0010_local_ci.sql"),
    include_str!("migrations/0011_session_docs.sql"),
    include_str!("migrations/0012_task_start_rules.sql"),
    include_str!("migrations/0013_ci_lanes.sql"),
    include_str!("migrations/0014_notes.sql"),
    include_str!("migrations/0015_vault_two_factor.sql"),
    include_str!("migrations/0016_device_grant_client.sql"),
    include_str!("migrations/0017_supertask.sql"),
    include_str!("migrations/0018_task_owner.sql"),
    include_str!("migrations/0019_automation_start.sql"),
    include_str!("migrations/0020_record_numbers.sql"),
    include_str!("migrations/0021_task_state_times.sql"),
    include_str!("migrations/0022_ci_job_needs.sql"),
    include_str!("migrations/0023_metrics_recounts.sql"),
];

type Job = Box<dyn FnOnce(&mut Connection) + Send + 'static>;

/// Errors opening the store.
#[derive(Debug, thiserror::Error)]
pub enum OpenError {
    /// SQLite failure.
    #[error("sqlite: {0}")]
    Sqlite(#[from] rusqlite::Error),
    /// The database was written by a newer Yulius.
    #[error("database schema version {found} is newer than supported {supported}")]
    TooNew {
        /// Version found.
        found: i64,
        /// Newest version this build knows.
        supported: i64,
    },
    /// Integrity check failed.
    #[error("database integrity check failed: {0}")]
    Corrupt(String),
    /// Thread spawn failure.
    #[error("failed to start database thread: {0}")]
    Thread(std::io::Error),
}

/// The SQLite store. Cheap to clone; all clones share the database thread.
#[derive(Clone)]
pub struct SqliteStore {
    jobs: mpsc::SyncSender<Job>,
}

impl SqliteStore {
    /// Opens (creating if needed) the database file, applies migrations and starts the thread.
    ///
    /// # Errors
    /// Fails when the file cannot be opened, is corrupt, or has a newer schema.
    pub fn open(path: &Path) -> Result<Self, OpenError> {
        let conn = Connection::open(path)?;
        Self::start(conn)
    }

    /// Opens a private in-memory database (tests).
    ///
    /// # Errors
    /// Fails when SQLite cannot initialize.
    pub fn open_in_memory() -> Result<Self, OpenError> {
        Self::start(Connection::open_in_memory()?)
    }

    fn start(mut conn: Connection) -> Result<Self, OpenError> {
        prepare(&mut conn)?;
        let (tx, rx) = mpsc::sync_channel::<Job>(QUEUE_BOUND);
        thread::Builder::new()
            .name("yulius-db".into())
            .spawn(move || {
                let mut conn = conn;
                while let Ok(job) = rx.recv() {
                    job(&mut conn);
                }
            })
            .map_err(OpenError::Thread)?;
        Ok(Self { jobs: tx })
    }

    /// Runs `f` on the database thread.
    pub(crate) async fn call<R, F>(&self, f: F) -> AppResult<R>
    where
        R: Send + 'static,
        F: FnOnce(&mut Connection) -> AppResult<R> + Send + 'static,
    {
        let (tx, rx) = oneshot::channel();
        let job: Job = Box::new(move |conn| {
            let _ = tx.send(f(conn));
        });
        let sender = self.jobs.clone();
        // `send` blocks only when the bounded queue is full; do that off the executor.
        tokio::task::spawn_blocking(move || sender.send(job))
            .await
            .map_err(AppError::internal)?
            .map_err(|_| AppError::Internal("database thread stopped".into()))?;
        rx.await
            .map_err(|_| AppError::Internal("database job dropped".into()))?
    }

    /// Writes a consistent copy of the database to `dest` (`VACUUM INTO`).
    ///
    /// # Errors
    /// Fails when the destination exists or cannot be written.
    pub async fn backup_to(&self, dest: &Path) -> AppResult<()> {
        let dest = dest.to_string_lossy().into_owned();
        self.call(move |conn| {
            conn.execute("VACUUM INTO ?1", [dest]).map_err(db)?;
            Ok(())
        })
        .await
    }
}

fn prepare(conn: &mut Connection) -> Result<(), OpenError> {
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "synchronous", "NORMAL")?;
    conn.pragma_update(None, "foreign_keys", "ON")?;
    // Deleted and replaced content (vault ciphertext, old tokens) is overwritten in the file,
    // not left in free pages (SPEC-002 ADR-20).
    conn.pragma_update(None, "secure_delete", "ON")?;
    conn.busy_timeout(std::time::Duration::from_secs(5))?;
    let integrity: String = conn.query_row("PRAGMA quick_check", [], |row| row.get(0))?;
    if integrity != "ok" {
        return Err(OpenError::Corrupt(integrity));
    }
    let version: i64 = conn.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    let supported = i64::try_from(MIGRATIONS.len()).unwrap_or(i64::MAX);
    if version > supported {
        return Err(OpenError::TooNew {
            found: version,
            supported,
        });
    }
    for (index, sql) in MIGRATIONS
        .iter()
        .enumerate()
        .skip(usize::try_from(version).unwrap_or(0))
    {
        let tx = conn.transaction()?;
        tx.execute_batch(sql)?;
        tx.pragma_update(
            None,
            "user_version",
            i64::try_from(index + 1).unwrap_or(i64::MAX),
        )?;
        tx.commit()?;
    }
    Ok(())
}

/// Maps a SQLite error to an application error (constraint violations become conflicts).
pub(crate) fn db(err: rusqlite::Error) -> AppError {
    if let rusqlite::Error::SqliteFailure(code, _) = &err
        && code.code == rusqlite::ErrorCode::ConstraintViolation
    {
        return AppError::Conflict("conflicting record".into());
    }
    AppError::internal(err)
}

/// Builds a conversion error for a column holding unexpected text.
pub(crate) fn bad_column(column: &str, value: &str) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(
        0,
        rusqlite::types::Type::Text,
        format!("invalid {column}: {value}").into(),
    )
}

/// Converts `u64` ids to SQLite integers.
pub(crate) fn to_i64(value: u64) -> i64 {
    i64::try_from(value).unwrap_or(i64::MAX)
}

/// Converts SQLite integers back to `u64` (negative → 0).
pub(crate) fn to_u64(value: i64) -> u64 {
    u64::try_from(value).unwrap_or(0)
}

/// Converts SQLite integers to `u32` (out of range → 0).
pub(crate) fn to_u32(value: i64) -> u32 {
    u32::try_from(value).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A version-2 database (deployed before the Claude-only change) keeps its rows when
    /// version 3 drops the harness-switching columns.
    #[tokio::test]
    async fn version_two_databases_migrate_to_claude_only() -> Result<(), Box<dyn std::error::Error>>
    {
        use yulius_app::ports::store::InstanceStore as _;
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("v2.db");
        {
            let conn = rusqlite::Connection::open(&path)?;
            conn.execute_batch(MIGRATIONS[0])?;
            conn.execute_batch(MIGRATIONS[1])?;
            conn.pragma_update(None, "user_version", 2)?;
            conn.execute(
                "INSERT INTO instance (singleton, public_url, auto_harness_switch) VALUES (1, 'https://yulius.test', 1)",
                [],
            )?;
        }
        let store = SqliteStore::open(&path)?;
        let instance = store.instance().await?.ok_or("instance")?;
        assert_eq!(instance.public_url, "https://yulius.test");
        let columns: Vec<String> = store
            .call(|c| {
                let mut statement = c
                    .prepare("SELECT name FROM pragma_table_info('tasks') UNION ALL SELECT name FROM pragma_table_info('instance')")
                    .map_err(db)?;
                let names = statement
                    .query_map([], |r| r.get(0))
                    .map_err(db)?
                    .collect::<Result<Vec<String>, _>>()
                    .map_err(db)?;
                Ok(names)
            })
            .await?;
        assert!(
            !columns
                .iter()
                .any(|c| c == "harness_proposal" || c == "auto_harness_switch")
        );
        assert!(
            columns.iter().any(|c| c == "mode"),
            "version 2 columns that stay are kept"
        );
        Ok(())
    }

    /// A version-3 database migrates through the task intake (4) to the subscription pool (5): it
    /// keeps its accounts (without priority and enabled) and hands a task that was waiting for
    /// capacity to the resume loop (F 2026-10-10).
    #[tokio::test]
    async fn version_three_databases_migrate_to_the_subscription_pool()
    -> Result<(), Box<dyn std::error::Error>> {
        use yulius_app::ports::store::AccountStore as _;
        use yulius_app::ports::store_ext::CapacityStore as _;
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("v3.db");
        {
            let conn = rusqlite::Connection::open(&path)?;
            for sql in &MIGRATIONS[..3] {
                conn.execute_batch(sql)?;
            }
            conn.pragma_update(None, "user_version", 3)?;
            conn.execute_batch(
                "INSERT INTO users (id, display_name, is_superadmin, is_instance_admin, created_at) VALUES ('u1', 'U', 1, 1, 1);
                 INSERT INTO sandboxes (id, slug, repo_owner, repo_name, default_branch, profile, port_base, port_count, created_by, onboarding_state, created_at) VALUES ('s1', 's', 'o', 'r', 'main', '{}', 20000, 10, 'u1', 'not_needed', 1);
                 INSERT INTO accounts (id, provider, label, priority, enabled, status, created_at, updated_at) VALUES ('a1', 'anthropic', 'Claude #1', 7, 0, 'ok', 1, 1);
                 INSERT INTO tasks (id, sandbox_id, number, title, prompt, harness, status, origin, created_by, created_at, updated_at) VALUES ('t1', 's1', 1, 'T', 'p', 'claude', 'waiting_capacity', 'ui', 'u1', 1, 5);",
            )?;
        }
        let store = SqliteStore::open(&path)?;
        let accounts = store.accounts().await?;
        assert_eq!(accounts.len(), 1);
        assert_eq!(accounts[0].label, "Claude #1");
        let pauses = store.open_pauses().await?;
        assert_eq!(pauses.len(), 1);
        assert_eq!(pauses[0].task_id.as_str(), "t1");
        assert_eq!(pauses[0].kind, yulius_app::model::PauseKind::Start);
        assert_eq!(pauses[0].started_at.0, 5);
        Ok(())
    }

    /// A version-16 database migrates through supertasks (17) to task owners (18, SPEC-070): each
    /// task is its creator's, and nobody has asked for control.
    #[tokio::test]
    async fn version_sixteen_databases_give_each_task_its_creator_as_owner()
    -> Result<(), Box<dyn std::error::Error>> {
        use yulius_app::ports::store::TaskStore as _;
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("v16.db");
        {
            let conn = rusqlite::Connection::open(&path)?;
            for sql in &MIGRATIONS[..16] {
                conn.execute_batch(sql)?;
            }
            conn.pragma_update(None, "user_version", 16)?;
            conn.execute_batch(
                "INSERT INTO users (id, display_name, is_superadmin, is_instance_admin, created_at) VALUES ('u1', 'U', 0, 0, 1);
                 INSERT INTO sandboxes (id, slug, repo_owner, repo_name, default_branch, profile, port_base, port_count, created_by, onboarding_state, created_at) VALUES ('s1', 's', 'o', 'r', 'main', '{}', 20000, 10, 'u1', 'not_needed', 1);
                 INSERT INTO tasks (id, sandbox_id, number, title, prompt, harness, status, origin, created_by, created_at, updated_at) VALUES ('t1', 's1', 1, 'T', 'p', 'claude', 'queued', 'ui', 'u1', 1, 5);",
            )?;
        }
        let store = SqliteStore::open(&path)?;
        let task = store
            .task(&yulius_domain::ids::TaskId::parse("t1")?)
            .await?
            .ok_or("task")?;
        assert_eq!(task.owner.as_str(), "u1");
        assert_eq!(store.control_request(&task.id).await?, None);
        Ok(())
    }
    /// A store at version 22 with runs: the upgrade queues each of them for one recount of its
    /// metrics (SPEC-091 RCT-003); a run started after it is not queued.
    fn version_twenty_two_with_runs(path: &Path) -> Result<(), Box<dyn std::error::Error>> {
        let conn = rusqlite::Connection::open(path)?;
        for sql in &MIGRATIONS[..22] {
            conn.execute_batch(sql)?;
        }
        conn.pragma_update(None, "user_version", 22)?;
        conn.execute_batch(
            "INSERT INTO users (id, display_name, is_superadmin, is_instance_admin, created_at) VALUES ('u1', 'U', 0, 0, 1);
             INSERT INTO sandboxes (id, slug, repo_owner, repo_name, default_branch, profile, port_base, port_count, created_by, onboarding_state, created_at) VALUES ('s1', 's', 'o', 'r', 'main', '{}', 20000, 10, 'u1', 'not_needed', 1);
             INSERT INTO tasks (id, sandbox_id, number, title, prompt, harness, status, origin, created_by, created_at, updated_at) VALUES
               ('t1', 's1', 1, 'T', 'p', 'claude', 'done', 'ui', 'u1', 1, 1),
               ('t2', 's1', 2, 'T', 'p', 'claude', 'running', 'ui', 'u1', 2, 2);
             INSERT INTO attempts (id, task_id, seq, harness, launch_kind, state, run_dir, token_hash, host_token, started_at, ended_at) VALUES
               ('a2', 't1', 2, 'claude', 'resume', 'exited', '/r/a2', 'h2', x'00', 300, 400),
               ('a1', 't1', 1, 'claude', 'new', 'exited', '/r/a1', 'h1', x'00', 100, 200),
               ('a3', 't2', 1, 'claude', 'new', 'live', '/r/a3', 'h3', x'00', 500, NULL);
             INSERT INTO metrics (attempt_id, task_id, model, input_tokens, output_tokens, cache_read, cache_write, turns, active_ms, updated_at) VALUES
               ('a1', 't1', 'claude-old', 5, 50, 0, 0, 7, 0, 200);",
        )?;
        Ok(())
    }

    fn recount_row(
        path: &Path,
        attempt: &str,
    ) -> Result<
        (Option<String>, Option<String>, Option<String>, Option<i64>),
        Box<dyn std::error::Error>,
    > {
        let conn = rusqlite::Connection::open(path)?;
        Ok(conn.query_row(
            "SELECT outcome, before, after, done_at FROM metrics_recounts WHERE attempt_id = ?1",
            [attempt],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )?)
    }

    #[tokio::test]
    async fn metrics_recounts_queue_the_runs_recorded_before_the_upgrade_once()
    -> Result<(), Box<dyn std::error::Error>> {
        use yulius_app::model::RecountOutcome;
        use yulius_app::ports::store::TaskStore as _;
        use yulius_domain::ids::{AttemptId, TaskId};
        use yulius_domain::time::Timestamp;
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("v22.db");
        version_twenty_two_with_runs(&path)?;
        let store = SqliteStore::open(&path)?;
        let queued = store.queued_metrics_recounts().await?;
        assert_eq!(
            queued.iter().map(AttemptId::as_str).collect::<Vec<_>>(),
            ["a1", "a2", "a3"],
            "every run recorded before the upgrade, oldest first"
        );
        rusqlite::Connection::open(&path)?.execute_batch(
            "INSERT INTO attempts (id, task_id, seq, harness, launch_kind, state, run_dir, token_hash, host_token, started_at) VALUES
               ('a4', 't1', 3, 'claude', 'resume', 'exited', '/r/a4', 'h4', x'00', 900);",
        )?;
        assert_eq!(
            store.queued_metrics_recounts().await?.len(),
            3,
            "a later run is not"
        );
        assert!(
            store
                .finish_metrics_recount(
                    &AttemptId::parse("a3")?,
                    RecountOutcome::Live,
                    None,
                    Timestamp(950)
                )
                .await?
        );
        assert!(
            !store
                .finish_metrics_recount(
                    &AttemptId::parse("a3")?,
                    RecountOutcome::Gone,
                    None,
                    Timestamp(960)
                )
                .await?,
            "a recorded run is never recounted"
        );
        assert_eq!(
            recount_row(&path, "a3")?,
            (Some("live".into()), None, None, Some(950))
        );
        assert_eq!(store.queued_metrics_recounts().await?.len(), 2);
        drop(store);
        // Reopening migrates nothing again.
        let store = SqliteStore::open(&path)?;
        assert_eq!(store.queued_metrics_recounts().await?.len(), 2);
        assert!(store.delete_task(&TaskId::parse("t1")?).await?);
        assert!(
            store.queued_metrics_recounts().await?.is_empty(),
            "a deleted task's runs go with it"
        );
        Ok(())
    }

    #[tokio::test]
    async fn a_recount_replaces_the_figures_and_records_both()
    -> Result<(), Box<dyn std::error::Error>> {
        use yulius_app::model::{MetricsRecord, RecountOutcome};
        use yulius_app::ports::store::TaskStore as _;
        use yulius_domain::ids::{AttemptId, TaskId};
        use yulius_domain::time::Timestamp;
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("v22.db");
        version_twenty_two_with_runs(&path)?;
        let store = SqliteStore::open(&path)?;
        let counted = MetricsRecord {
            model: None,
            input_tokens: Some(85),
            output_tokens: Some(1_510),
            cache_read: Some(9),
            cache_write: Some(1),
            turns: Some(3),
            active_ms: Some(220_200),
        };
        assert!(
            store
                .finish_metrics_recount(
                    &AttemptId::parse("a1")?,
                    RecountOutcome::Recomputed,
                    Some(&counted),
                    Timestamp(1_000)
                )
                .await?
        );
        let stored = store.task_metrics(&TaskId::parse("t1")?).await?;
        assert_eq!(
            stored,
            vec![MetricsRecord {
                model: Some("claude-old".into()),
                ..counted.clone()
            }],
            "the transcript's figures; a model it does not name keeps the stored one"
        );
        let (outcome, before, after, at) = recount_row(&path, "a1")?;
        assert_eq!((outcome.as_deref(), at), (Some("recomputed"), Some(1_000)));
        let before: serde_json::Value = serde_json::from_str(&before.ok_or("before")?)?;
        let after: serde_json::Value = serde_json::from_str(&after.ok_or("after")?)?;
        assert_eq!(
            before,
            serde_json::json!({"model": "claude-old", "input_tokens": 5, "output_tokens": 50,
                "cache_read": 0, "cache_write": 0, "turns": 7, "active_ms": 0})
        );
        assert_eq!(
            after,
            serde_json::json!({"model": "claude-old", "input_tokens": 85, "output_tokens": 1510,
                "cache_read": 9, "cache_write": 1, "turns": 3, "active_ms": 220200})
        );
        // A run with no row yet gets one; its "before" is nothing.
        assert!(
            store
                .finish_metrics_recount(
                    &AttemptId::parse("a2")?,
                    RecountOutcome::Recomputed,
                    Some(&counted),
                    Timestamp(1_001)
                )
                .await?
        );
        let (_, before, after, _) = recount_row(&path, "a2")?;
        assert_eq!(before, None);
        assert!(after.is_some_and(|a| a.contains("\"model\":null")));
        assert_eq!(store.task_metrics(&TaskId::parse("t1")?).await?.len(), 2);
        Ok(())
    }

    /// A version-19 database's tasks migrate to version 21 (SPEC-087 TIM-001, TIM-002): each is
    /// in its state since its last change, and a live task that does not wait for a person works
    /// since its first run started.
    #[tokio::test]
    async fn version_nineteen_tasks_get_their_state_times() -> Result<(), Box<dyn std::error::Error>>
    {
        use yulius_app::model::TaskTimes;
        use yulius_app::ports::store::TaskStore as _;
        use yulius_domain::ids::TaskId;
        use yulius_domain::time::Timestamp;
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("v19.db");
        {
            let conn = rusqlite::Connection::open(&path)?;
            for sql in &MIGRATIONS[..19] {
                conn.execute_batch(sql)?;
            }
            conn.pragma_update(None, "user_version", 19)?;
            conn.execute_batch(
                "INSERT INTO users (id, display_name, is_superadmin, is_instance_admin, created_at) VALUES ('u1', 'U', 0, 0, 1);
                 INSERT INTO sandboxes (id, slug, repo_owner, repo_name, default_branch, profile, port_base, port_count, created_by, onboarding_state, created_at) VALUES ('s1', 's', 'o', 'r', 'main', '{}', 20000, 10, 'u1', 'not_needed', 1);
                 INSERT INTO tasks (id, sandbox_id, number, title, prompt, harness, status, activity, origin, created_by, created_at, updated_at) VALUES
                   ('working', 's1', 1, 'T', 'p', 'claude', 'running', 'working', 'ui', 'u1', 1, 500),
                   ('asks', 's1', 2, 'T', 'p', 'claude', 'running', 'needs_attention', 'ui', 'u1', 2, 600),
                   ('ended', 's1', 3, 'T', 'p', 'claude', 'closed', NULL, 'ui', 'u1', 3, 700),
                   ('unrun', 's1', 4, 'T', 'p', 'claude', 'starting', NULL, 'ui', 'u1', 4, 800);
                 INSERT INTO attempts (id, task_id, seq, harness, launch_kind, state, run_dir, token_hash, host_token, started_at) VALUES
                   ('a1', 'working', 1, 'claude', 'new', 'exited', '/r/a1', 'h1', x'00', 100),
                   ('a2', 'working', 2, 'claude', 'resume', 'running', '/r/a2', 'h2', x'00', 300);",
            )?;
        }
        let store = SqliteStore::open(&path)?;
        let times = |state: i64, working: Option<i64>| {
            Some(TaskTimes {
                state_at: Timestamp(state),
                working_since: working.map(Timestamp),
            })
        };
        assert_eq!(
            store.task_times(&TaskId::parse("working")?).await?,
            times(500, Some(100))
        );
        assert_eq!(
            store.task_times(&TaskId::parse("asks")?).await?,
            times(600, None)
        );
        assert_eq!(
            store.task_times(&TaskId::parse("ended")?).await?,
            times(700, None)
        );
        assert_eq!(
            store.task_times(&TaskId::parse("unrun")?).await?,
            times(800, Some(4)),
            "no run yet: since it was created"
        );
        Ok(())
    }

    /// A version-18 database's automations migrate to version 19 (SPEC-078) as "start now":
    /// none of the start choice's columns is set.
    #[tokio::test]
    async fn version_eighteen_automations_start_their_tasks_now()
    -> Result<(), Box<dyn std::error::Error>> {
        use yulius_app::automations::AutomationStart;
        use yulius_app::ports::store_ext::AutomationStore as _;
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("v18.db");
        {
            let conn = rusqlite::Connection::open(&path)?;
            for sql in &MIGRATIONS[..18] {
                conn.execute_batch(sql)?;
            }
            conn.pragma_update(None, "user_version", 18)?;
            conn.execute_batch(
                "INSERT INTO users (id, display_name, is_superadmin, is_instance_admin, created_at) VALUES ('u1', 'U', 1, 1, 1);
                 INSERT INTO sandboxes (id, slug, repo_owner, repo_name, default_branch, profile, port_base, port_count, created_by, onboarding_state, created_at) VALUES ('s1', 's', 'o', 'r', 'main', '{}', 20000, 10, 'u1', 'not_needed', 1);
                 INSERT INTO automations (id, sandbox_id, name, cron, utc_offset_minutes, prompt, workflow_id, harness, enabled, created_by, created_via, created_at, last_scheduled) VALUES ('au1', 's1', 'Docs', '0 3 * * *', 120, 'p', NULL, 'claude', 1, 'u1', 'ui', 1, NULL);",
            )?;
        }
        let store = SqliteStore::open(&path)?;
        let automation = store.automation("au1").await?.ok_or("automation")?;
        assert_eq!(automation.start, AutomationStart::Now);
        assert_eq!(automation.name, "Docs");
        Ok(())
    }

    /// A version-5 database (the subscription pool) migrates to updates and the Jev key (ADR-18,
    /// ADM-002): the instance row keeps its values and has no saved key; at most one update run is
    /// open at a time, and a state moves only from the expected one.
    #[tokio::test]
    async fn version_five_databases_migrate_to_updates_and_the_jev_key()
    -> Result<(), Box<dyn std::error::Error>> {
        use yulius_app::model::Sealed;
        use yulius_app::ports::store::InstanceStore as _;
        use yulius_app::ports::store_ext::UpdateStore as _;
        use yulius_app::updates::UpdateRunRecord;
        use yulius_domain::time::Timestamp;
        use yulius_domain::update::RunState;
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("v5.db");
        {
            let conn = rusqlite::Connection::open(&path)?;
            for sql in &MIGRATIONS[..5] {
                conn.execute_batch(sql)?;
            }
            conn.pragma_update(None, "user_version", 5)?;
            conn.execute(
                "INSERT INTO instance (singleton, public_url, owner_login) VALUES (1, 'https://yulius.test', 'Quartio-Studio')",
                [],
            )?;
        }
        let store = SqliteStore::open(&path)?;
        let instance = store.instance().await?.ok_or("instance")?;
        assert_eq!(instance.owner_login.as_deref(), Some("Quartio-Studio"));
        assert!(store.jev_key().await?.is_none());
        store
            .set_jev_key(Some((&Sealed(vec![1, 2, 3]), "superadmin")), Timestamp(7))
            .await?;
        let key = store.jev_key().await?.ok_or("key")?;
        assert_eq!(key.sealed, Sealed(vec![1, 2, 3]));
        assert_eq!(
            (key.set_at, key.set_by.as_deref()),
            (Timestamp(7), Some("superadmin"))
        );
        store.set_jev_key(None, Timestamp(8)).await?;
        assert!(store.jev_key().await?.is_none());

        let run = |id: &str| UpdateRunRecord {
            id: id.into(),
            target_commit: "3".repeat(40),
            from_commit: None,
            summary: "Things".into(),
            state: RunState::Building,
            outcome: None,
            error: None,
            requested_by: "superadmin".into(),
            created_at: Timestamp(10),
            state_since: Timestamp(10),
            finished_at: None,
        };
        assert!(store.insert_update_run(&run("r1")).await?);
        assert!(!store.insert_update_run(&run("r2")).await?, "one open run");
        assert!(
            !store
                .move_update_run(
                    "r1",
                    RunState::Draining,
                    RunState::Installing,
                    None,
                    None,
                    Timestamp(11)
                )
                .await?,
            "only from the expected state"
        );
        assert!(
            store
                .move_update_run(
                    "r1",
                    RunState::Building,
                    RunState::Draining,
                    None,
                    None,
                    Timestamp(12)
                )
                .await?
        );
        assert!(
            store
                .move_update_run(
                    "r1",
                    RunState::Draining,
                    RunState::Cancelled,
                    None,
                    Some("Cancelled by superadmin"),
                    Timestamp(13)
                )
                .await?
        );
        assert!(store.open_update_run().await?.is_none());
        let last = store.last_update_run().await?.ok_or("last")?;
        assert_eq!(last.state, RunState::Cancelled);
        assert_eq!(last.finished_at, Some(Timestamp(13)));
        assert_eq!(last.error.as_deref(), Some("Cancelled by superadmin"));
        assert!(
            store.insert_update_run(&run("r2")).await?,
            "a new run after it ended"
        );
        Ok(())
    }

    #[tokio::test]
    async fn migrates_and_reopens() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("y.db");
        {
            let store = SqliteStore::open(&path)?;
            let version: i64 = store
                .call(|c| {
                    c.query_row("PRAGMA user_version", [], |r| r.get(0))
                        .map_err(db)
                })
                .await?;
            assert_eq!(version, i64::try_from(MIGRATIONS.len())?);
        }
        // Reopen applies nothing new and keeps the version.
        let store = SqliteStore::open(&path)?;
        let version: i64 = store
            .call(|c| {
                c.query_row("PRAGMA user_version", [], |r| r.get(0))
                    .map_err(db)
            })
            .await?;
        assert_eq!(version, i64::try_from(MIGRATIONS.len())?);
        let backup = dir.path().join("backup.db");
        store.backup_to(&backup).await?;
        assert!(backup.exists());
        Ok(())
    }

    #[test]
    fn refuses_newer_schema() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("y.db");
        {
            let conn = Connection::open(&path)?;
            conn.pragma_update(None, "user_version", 99)?;
        }
        assert!(matches!(
            SqliteStore::open(&path),
            Err(OpenError::TooNew { found: 99, .. })
        ));
        Ok(())
    }
}
