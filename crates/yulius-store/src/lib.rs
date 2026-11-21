//! SQLite persistence adapter: schema, migrations, repositories and backup (SPEC-002 §5).
//!
//! All database access runs on one dedicated thread that owns the connection (single writer,
//! WAL mode). Async callers submit closures through a bounded queue and await the result, so
//! the async executor never blocks on SQLite and concurrent writers never race.

mod accounts;
mod agent_states;
mod analytics;
mod audit;
mod automations;
mod capacity;
mod cd;
mod ci;
mod coordination;
pub mod crypto;
mod delivery;
mod digest;
mod guests;
mod handoffs;
mod imports;
mod instance;
mod intake;
mod mcp;
mod needs_you;
mod notes;
mod sandboxes;
mod session_docs;
mod sessions;
mod skills;
mod spans;
mod staging;
mod tasks;
mod telegram;
mod updates;
mod users;
mod variables;
mod vault;
mod workflows;

use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, mpsc};
use std::thread;

use rusqlite::Connection;
use tokio::sync::oneshot;
use yulius_app::error::{AppError, AppResult};

/// Bound of pending database jobs.
const QUEUE_BOUND: usize = 1024;

/// The newest schema version this build opens (its number of migrations): an instance export
/// records it so that an older build refuses the copy (SPEC-329).
#[must_use]
pub fn schema_version() -> u32 {
    u32::try_from(MIGRATIONS.len()).unwrap_or(u32::MAX)
}

const REUSABLE_SANDBOX_PORTS: &str = include_str!("migrations/0074_reusable_sandbox_ports.sql");

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
    include_str!("migrations/0024_task_schedules.sql"),
    include_str!("migrations/0025_agent_secret_requests.sql"),
    include_str!("migrations/0026_agent_secret_env_copies.sql"),
    include_str!("migrations/0027_cd_runs.sql"),
    include_str!("migrations/0028_spans.sql"),
    include_str!("migrations/0029_ci_job_failures.sql"),
    // Schema version 30: the file number is reserved per task (`record_number`), so a later
    // migration numbered 0030 goes after this line, never before (the position is the version).
    include_str!("migrations/0031_automation_prepared.sql"),
    // Schema version 31 (SPEC-154).
    include_str!("migrations/0030_job_resource_use.sql"),
    // Schema version 32 (SPEC-171).
    include_str!("migrations/0032_playground_purges.sql"),
    // Schema version 33 (SPEC-191).
    include_str!("migrations/0033_task_person_pause.sql"),
    // Schema version 34 (SPEC-195).
    include_str!("migrations/0034_instance_vidoc_key.sql"),
    // Schema version 35 (SPEC-216).
    include_str!("migrations/0035_vault_scopes_and_auto_allow.sql"),
    // Schema version 36 (SPEC-217).
    include_str!("migrations/0036_security_findings.sql"),
    // Schema version 37 (SPEC-218).
    include_str!("migrations/0037_mcp_server_origin.sql"),
    // Schema version 38 (SPEC-244).
    include_str!("migrations/0038_account_email.sql"),
    // Schema version 39 (SPEC-245).
    include_str!("migrations/0039_agent_secret_scopes.sql"),
    // Schema version 40 (SPEC-247).
    include_str!("migrations/0040_api_token_scopes.sql"),
    // Schema version 41 (SPEC-257).
    include_str!("migrations/0041_cd_run_stage.sql"),
    // Schema version 42 (SPEC-271).
    include_str!("migrations/0042_agent_states.sql"),
    // Schema version 43 (SPEC-275).
    include_str!("migrations/0043_project_variables.sql"),
    // Schema version 44 (SPEC-332).
    include_str!("migrations/0044_mcp_server_owner.sql"),
    // Schema version 45 (SPEC-356).
    include_str!("migrations/0045_instance_vidoc_off.sql"),
    // Schema version 46 (SPEC-410).
    include_str!("migrations/0046_playground_guests.sql"),
    // Schema version 47 (SPEC-415).
    include_str!("migrations/0047_cd_rollback.sql"),
    // Schema version 48 (SPEC-435).
    include_str!("migrations/0048_ci_job_limit_and_wait.sql"),
    // Schema version 49 (SPEC-440).
    include_str!("migrations/0049_vault_copy_requests.sql"),
    // Schema version 50 (SPEC-459). The file is 0051, the number `record_number` gave this
    // task (0050 went to another reservation); a version is a migration's place in this list.
    include_str!("migrations/0051_instance_limits.sql"),
    // Schema version 51 (SPEC-461): the file number 0050 was reserved first, version 50 merged
    // first; it goes after it, never before (the position is the version).
    include_str!("migrations/0050_claude_code.sql"),
    // Schema version 52 (SPEC-463). The file is 0052, the number `record_number` gave this
    // task; it goes after every migration `dev` had when it merged (the position is the
    // version).
    include_str!("migrations/0052_account_email_typed.sql"),
    // Schema version 53 (SPEC-472). The file is 0053, the number `record_number` gave this
    // task; it goes after every migration `dev` had when it merged (the position is the
    // version).
    include_str!("migrations/0053_task_attention.sql"),
    // Schema version 54 (SPEC-482). The file is 0054, the number `record_number` gave this
    // task; it goes after every migration `dev` had when it merged (the position is the
    // version).
    include_str!("migrations/0054_attempt_model.sql"),
    // Schema version 55 (SPEC-483 V6). The file is 0055, the number `record_number` gave this
    // task; it goes after every migration `dev` had when it merged (the position is the
    // version).
    include_str!("migrations/0055_production_secrets_have_no_hosts.sql"),
    // Schema version 56 (SPEC-484). The file is 0056, the number `record_number` gave this
    // task; it goes after every migration `dev` had when it merged (the position is the
    // version).
    include_str!("migrations/0056_plan_first_steps.sql"),
    // Schema version 57 (SPEC-483 QA1). The file is 0057, the number `record_number` gave this
    // task; it goes after every migration `dev` had when it merged (the position is the
    // version).
    include_str!("migrations/0057_queued_start.sql"),
    // Schema version 58 (SPEC-483 CLT-002). The file is 0058, the number `record_number` gave
    // this task; it goes after every migration `dev` had when it merged (the position is the
    // version).
    include_str!("migrations/0058_closed_at_only_when_closed.sql"),
    // Schema version 59 (SPEC-491). The file is 0060, the number `record_number` gave this
    // task; it goes after every migration `dev` had when it merged (the position is the
    // version).
    include_str!("migrations/0060_task_spec_status.sql"),
    // Schema version 60 (SPEC-490 OWN-009). The file is 0059, the number `record_number` gave
    // this task; it goes after every migration `dev` had when it merged (the position is the
    // version).
    include_str!("migrations/0059_task_control_from.sql"),
    // Schema version 61 (SPEC-483 EFF-001). The file is 0061, the number `record_number` gave
    // this task; it goes after every migration `dev` had when it merged (the position is the
    // version).
    include_str!("migrations/0061_capacity_window_indexes.sql"),
    // Schema version 62 (SPEC-493 REV-002).
    include_str!("migrations/0062_review_rounds.sql"),
    // Schema version 63 (SPEC-498, from the contributor's SPEC-444). The file is 0063, the number
    // `record_number` gave this task; it goes after every migration `dev` had when it merged (the
    // position is the version).
    include_str!("migrations/0063_telegram_channel.sql"),
    // Schema version 64 (SPEC-497 OFC-001). The file is 0064, the number `record_number` gave
    // this task; it goes after every migration `dev` had when it merged (the position is the
    // version).
    include_str!("migrations/0064_ci_job_infrastructure.sql"),
    // Schema version 65 (SPEC-497 OFC-002).
    include_str!("migrations/0065_ci_flaky_tests.sql"),
    // Schema version 66 (SPEC-505 RND-001).
    include_str!("migrations/0066_task_harness_draws.sql"),
    // Schema version 67 (SPEC-510 TKC-003).
    include_str!("migrations/0067_control_ask_first.sql"),
    // Schema version 68 (SPEC-514 PIN-003).
    include_str!("migrations/0068_automation_agent_named.sql"),
    // Schema version 69 (SPEC-525).
    include_str!("migrations/0069_reindex_codex_antigravity_sessions.sql"),
    // SPEC-534. The position is the schema version and 0072 is the reserved file number.
    include_str!("migrations/0072_pause_legacy_automations.sql"),
    // Schema version 71 (SPEC-512 DIG-001); 0070 is the reserved file number.
    include_str!("migrations/0070_end_of_day_digest.sql"),
    // Schema version 72 (SPEC-535); 0071 is the reserved file number.
    include_str!("migrations/0071_reindex_session_passages.sql"),
    REUSABLE_SANDBOX_PORTS,
    include_str!("migrations/0075_prompt_held_during_turn.sql"),
    // Schema version 75 (SPEC-512 TGI-001); 0073 is the reserved file number.
    include_str!("migrations/0073_telegram_intake.sql"),
    // Schema version 76 (SPEC-512 HOF-001); 0076 is the reserved file number.
    include_str!("migrations/0076_task_handoffs.sql"),
    include_str!("migrations/0077_task_merge_evidence.sql"),
    // Schema version 78 (SPEC-575 MFS-005).
    include_str!("migrations/0078_staging_requests.sql"),
    // Schema version 79 (SPEC-575 MFS-012).
    include_str!("migrations/0079_bootstrap_upgrades.sql"),
    // Schema version 80 (SPEC-578); 0080 is the reserved file number.
    include_str!("migrations/0080_task_wait_messages.sql"),
    include_str!("migrations/0081_ci_run_admission.sql"),
    include_str!("migrations/0082_task_merge_wait.sql"),
    // SPEC-605. The position is the schema version, 0084 is the reserved file number.
    include_str!("migrations/0084_review_round_delivery.sql"),
    // SPEC-609. Schema version 84 follows the already shipped review migration.
    include_str!("migrations/0083_terminal_pulls.sql"),
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
    /// [`yulius_app::ports::store::PermissionStore::access_epoch`].
    access_epoch: Arc<AtomicU64>,
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
        Ok(Self {
            jobs: tx,
            access_epoch: Arc::new(AtomicU64::new(0)),
        })
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

    /// Runs `sql` on the store's connection (tests only: e.g. a trigger that makes a write
    /// fail).
    ///
    /// # Errors
    /// SQLite failures.
    #[cfg(feature = "test-support")]
    pub async fn execute_for_tests(&self, sql: &str) -> AppResult<()> {
        let sql = sql.to_owned();
        self.call(move |c| c.execute_batch(&sql).map_err(db)).await
    }

    /// [`Self::call`] for a write that can change who may type into a terminal: the access epoch
    /// grows on the database thread as soon as the write is done, before the caller resumes.
    pub(crate) async fn call_access<R, F>(&self, f: F) -> AppResult<R>
    where
        R: Send + 'static,
        F: FnOnce(&mut Connection) -> AppResult<R> + Send + 'static,
    {
        let epoch = Arc::clone(&self.access_epoch);
        self.call(move |conn| {
            let result = f(conn);
            epoch.fetch_add(1, Ordering::SeqCst);
            result
        })
        .await
    }

    /// The access epoch now.
    pub(crate) fn current_access_epoch(&self) -> u64 {
        self.access_epoch.load(Ordering::SeqCst)
    }

    /// Grows the access epoch from the database thread (a write that found a change).
    pub(crate) fn access_epoch_handle(&self) -> Arc<AtomicU64> {
        Arc::clone(&self.access_epoch)
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

/// Applies this release's migrations to the existing database file `path`, never creating one,
/// and closes it: the schema it had and the one it has now (SPEC-591: staging measures a
/// release's migration on a copy of the store an older release wrote before it stops anything).
///
/// # Errors
/// Fails when the file does not exist, is damaged or was written by a newer Yulius.
pub fn migrate_file(path: &Path) -> Result<(i64, u32), OpenError> {
    let mut conn = Connection::open_with_flags(
        path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    let from: i64 = conn.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    prepare(&mut conn)?;
    conn.close().map_err(|(_, err)| OpenError::Sqlite(err))?;
    Ok((from, schema_version()))
}

/// Creates the database file `path` at schema `version`, as a Yulius of that version left it,
/// for upgrade tests (the next [`SqliteStore::open`] applies the later migrations).
///
/// # Errors
/// Fails when the file cannot be written or `version` is past the latest.
#[cfg(any(test, feature = "test-support"))]
pub fn create_at_version(path: &Path, version: usize) -> Result<(), OpenError> {
    let mut conn = Connection::open(path)?;
    let tx = conn.transaction()?;
    for sql in MIGRATIONS.get(..version).ok_or(OpenError::TooNew {
        found: i64::try_from(version).unwrap_or(i64::MAX),
        supported: i64::try_from(MIGRATIONS.len()).unwrap_or(i64::MAX),
    })? {
        tx.execute_batch(sql)?;
    }
    tx.pragma_update(
        None,
        "user_version",
        i64::try_from(version).unwrap_or(i64::MAX),
    )?;
    tx.commit()?;
    Ok(())
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
        // Replacing a parent table with foreign keys enabled would cascade deletion
        // into its children. SQLite checks the resulting graph before the rebuild commits.
        let rebuild = *sql == REUSABLE_SANDBOX_PORTS;
        if rebuild {
            conn.pragma_update(None, "foreign_keys", "OFF")?;
        }
        let tx = conn.transaction()?;
        tx.execute_batch(sql)?;
        if rebuild && tx.prepare("PRAGMA foreign_key_check")?.exists([])? {
            return Err(OpenError::Corrupt(
                "invalid references after rebuilding sandbox ports".into(),
            ));
        }
        tx.pragma_update(
            None,
            "user_version",
            i64::try_from(index + 1).unwrap_or(i64::MAX),
        )?;
        tx.commit()?;
        if rebuild {
            conn.pragma_update(None, "foreign_keys", "ON")?;
        }
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

    /// SPEC-483 slice 35 (review): a schema version is a position in [`MIGRATIONS`] and a
    /// database records it, so a migration added before the end would give an existing version
    /// a different migration: databases at that version would run the one after it again.
    /// The latest versions keep their files; a new one goes at the end.
    #[test]
    fn the_latest_schema_versions_keep_their_migrations() {
        let pinned = [
            (57, include_str!("migrations/0057_queued_start.sql")),
            (
                58,
                include_str!("migrations/0058_closed_at_only_when_closed.sql"),
            ),
            (59, include_str!("migrations/0060_task_spec_status.sql")),
            (60, include_str!("migrations/0059_task_control_from.sql")),
            (
                61,
                include_str!("migrations/0061_capacity_window_indexes.sql"),
            ),
            (62, include_str!("migrations/0062_review_rounds.sql")),
            (63, include_str!("migrations/0063_telegram_channel.sql")),
            (
                64,
                include_str!("migrations/0064_ci_job_infrastructure.sql"),
            ),
            (65, include_str!("migrations/0065_ci_flaky_tests.sql")),
            (66, include_str!("migrations/0066_task_harness_draws.sql")),
            (67, include_str!("migrations/0067_control_ask_first.sql")),
            (
                68,
                include_str!("migrations/0068_automation_agent_named.sql"),
            ),
            (
                69,
                include_str!("migrations/0069_reindex_codex_antigravity_sessions.sql"),
            ),
            (
                70,
                include_str!("migrations/0072_pause_legacy_automations.sql"),
            ),
            (71, include_str!("migrations/0070_end_of_day_digest.sql")),
            (
                72,
                include_str!("migrations/0071_reindex_session_passages.sql"),
            ),
            (73, REUSABLE_SANDBOX_PORTS),
            (
                74,
                include_str!("migrations/0075_prompt_held_during_turn.sql"),
            ),
            (75, include_str!("migrations/0073_telegram_intake.sql")),
            (76, include_str!("migrations/0076_task_handoffs.sql")),
            (77, include_str!("migrations/0077_task_merge_evidence.sql")),
            (78, include_str!("migrations/0078_staging_requests.sql")),
            (79, include_str!("migrations/0079_bootstrap_upgrades.sql")),
            (80, include_str!("migrations/0080_task_wait_messages.sql")),
            (81, include_str!("migrations/0081_ci_run_admission.sql")),
            (82, include_str!("migrations/0082_task_merge_wait.sql")),
            (
                83,
                include_str!("migrations/0084_review_round_delivery.sql"),
            ),
            (84, include_str!("migrations/0083_terminal_pulls.sql")),
        ];
        for (version, sql) in pinned {
            assert_eq!(
                MIGRATIONS.get(version - 1),
                Some(&sql),
                "schema version {version}"
            );
        }
        assert_eq!(MIGRATIONS.len(), 84, "a new migration is pinned here too");
    }

    #[test]
    fn terminal_pull_upgrade_keeps_the_confirmed_landing_of_a_parked_task()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut conn = Connection::open_in_memory()?;
        for sql in &MIGRATIONS[..83] {
            conn.execute_batch(sql)?;
        }
        conn.pragma_update(None, "user_version", 83)?;
        conn.execute_batch(
            "INSERT INTO users (id, display_name, is_superadmin, is_instance_admin, created_at)
             VALUES ('u', 'User', 0, 0, 1);
             INSERT INTO sandboxes (id, slug, repo_owner, repo_name, default_branch, profile,
                 port_base, port_count, created_by, onboarding_state, created_at)
             VALUES ('s', 'shop', 'org', 'shop', 'dev', '{}', 20000, 20, 'u', 'ready', 1);
             INSERT INTO tasks (id,sandbox_id,number,title,prompt,harness,status,branch,origin,created_by,created_at,updated_at)
             VALUES ('t','s',1,'Checkout','Change checkout','codex','parked','feature','user','u',1,2);
             INSERT INTO delivery (task_id,stage,pr_number,updated_at) VALUES ('t','merged_integration',41,2);
             INSERT INTO task_merges (task_id,sandbox_id,merged_sha,branch,pr_number,head_sha)
             VALUES ('t','s','aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa','dev',41,'bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb');",
        )?;
        conn.execute_batch(
            "INSERT INTO tasks (id,sandbox_id,number,title,prompt,harness,status,branch,origin,created_by,created_at,updated_at)
             VALUES ('older','s',2,'Renamed task','Old prompt','codex','parked','renamed','user','u',1,2);
             INSERT INTO delivery (task_id,stage,pr_number,updated_at) VALUES ('older','merged_integration',42,2);"
        )?;
        prepare(&mut conn)?;
        let json: String = conn.query_row(
            "SELECT snapshot FROM terminal_pulls WHERE task_id='t'",
            [],
            |r| r.get(0),
        )?;
        let pull: yulius_app::ports::github::GhPull = serde_json::from_str(&json)?;
        assert_eq!(
            (pull.number, pull.merged, pull.base.as_str()),
            (41, true, "dev")
        );
        assert_eq!(pull.head_sha, "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb");
        assert_eq!(
            pull.merge_commit_sha.as_deref(),
            Some("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa")
        );
        let json: String = conn.query_row(
            "SELECT snapshot FROM terminal_pulls WHERE task_id='older'",
            [],
            |r| r.get(0),
        )?;
        let older: yulius_app::ports::github::GhPull = serde_json::from_str(&json)?;
        assert!(older.merged && older.merge_commit_sha.is_none());
        assert_eq!(older.title, "Pull request #42");
        assert!(older.head.is_empty() && older.head_sha.is_empty());
        Ok(())
    }

    /// SPEC-575 (a person's integration finding on PR #1541): a populated database at the
    /// schema before the bootstrap record (78, with staging requests) upgrades to the latest one
    /// with its records kept, and the new table takes its rows. A parallel migration that lands
    /// first takes version 79 and this one moves after it; this test then runs from there.
    #[test]
    fn a_populated_staging_database_upgrades_to_the_bootstrap_record()
    -> Result<(), Box<dyn std::error::Error>> {
        let at = MIGRATIONS
            .iter()
            .position(|sql| sql.contains("CREATE TABLE bootstrap_upgrades"))
            .ok_or("the bootstrap migration")?;
        let mut conn = Connection::open_in_memory()?;
        for sql in &MIGRATIONS[..at] {
            conn.execute_batch(sql)?;
        }
        conn.pragma_update(None, "user_version", i64::try_from(at)?)?;
        conn.execute_batch(
            "INSERT INTO users (id, display_name, is_superadmin, is_instance_admin, created_at)
             VALUES ('u', 'User', 0, 0, 1);
             INSERT INTO sandboxes (id, slug, repo_owner, repo_name, default_branch, profile,
                 port_base, port_count, created_by, onboarding_state, created_at)
             VALUES ('s', 'shop', 'org', 'shop', 'dev', '{}', 20000, 20, 'u', 'ready', 1);
             INSERT INTO staging_requests (id, sandbox_id, environment, task_id, revision, state,
                 attempts, requested_by, told, created_at, updated_at)
             VALUES ('r', 's', 'staging', NULL, 'abc', 'verified', 1, 'task:1', 1, 2, 2);",
        )?;
        prepare(&mut conn)?;
        let version: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        assert_eq!(version, i64::try_from(MIGRATIONS.len())?);
        let kept: String = conn.query_row(
            "SELECT state FROM staging_requests WHERE id = 'r'",
            [],
            |r| r.get(0),
        )?;
        assert_eq!(kept, "verified");
        conn.execute_batch(
            "INSERT INTO bootstrap_upgrades (sandbox_id, revision, state, task_id, updated_at)
             VALUES ('s', 1, 'claimed', NULL, 3);",
        )?;
        let state: String = conn.query_row(
            "SELECT state FROM bootstrap_upgrades WHERE sandbox_id = 's'",
            [],
            |r| r.get(0),
        )?;
        assert_eq!(state, "claimed");
        Ok(())
    }

    #[test]
    fn port_reuse_upgrade_keeps_existing_leases_and_sandbox_children()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut conn = Connection::open_in_memory()?;
        for sql in &MIGRATIONS[..72] {
            conn.execute_batch(sql)?;
        }
        conn.pragma_update(None, "user_version", 72)?;
        conn.execute_batch(
            "INSERT INTO users (id, display_name, is_superadmin, is_instance_admin, created_at)
             VALUES ('u', 'User', 0, 0, 1);
             INSERT INTO sandboxes (id, slug, repo_owner, repo_name, default_branch, profile,
                 port_base, port_count, created_by, onboarding_state, created_at, removed_at)
             VALUES ('s', 'shop', 'org', 'shop', 'dev', '{}', 20000, 20, 'u', 'ready', 1, 2);
             INSERT INTO permission_cache VALUES ('u', 's', 'admin', 1);",
        )?;
        prepare(&mut conn)?;
        let held: (u16, bool) = conn.query_row(
            "SELECT port_base, ports_released FROM sandboxes WHERE id='s'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        assert_eq!(held, (20000, false));
        let owner: String = conn.query_row(
            "SELECT user_id FROM permission_cache WHERE sandbox_id='s'",
            [],
            |r| r.get(0),
        )?;
        assert_eq!(owner, "u");
        let keys: bool = conn.query_row("PRAGMA foreign_keys", [], |r| r.get(0))?;
        assert!(keys);
        Ok(())
    }

    /// SPEC-514 PIN-003: a version-67 database's automations saved as `auto` (drawn for each
    /// run, SPEC-505 RND-002) run Claude Code from version 68 on, and say so; one that names an
    /// agent keeps it.
    #[tokio::test]
    async fn version_sixty_seven_automations_on_auto_run_claude_code()
    -> Result<(), Box<dyn std::error::Error>> {
        use yulius_app::ports::store_ext::AutomationStore as _;
        use yulius_domain::harness::Harness;
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("v67.db");
        {
            let conn = rusqlite::Connection::open(&path)?;
            for sql in &MIGRATIONS[..67] {
                conn.execute_batch(sql)?;
            }
            conn.pragma_update(None, "user_version", 67)?;
            conn.execute_batch(
                "INSERT INTO users (id, display_name, is_superadmin, is_instance_admin, created_at) VALUES ('u1', 'U', 0, 0, 1);
                 INSERT INTO sandboxes (id, slug, repo_owner, repo_name, default_branch, profile, port_base, port_count, created_by, onboarding_state, created_at) VALUES ('s1', 's', 'o', 'r', 'main', '{}', 20000, 10, 'u1', 'not_needed', 1);
                 INSERT INTO automations (id, sandbox_id, name, cron, utc_offset_minutes, prompt, harness, enabled, created_by, created_via, created_at) VALUES
                   ('a-auto', 's1', 'Drawn', '0 9 * * *', 0, 'p', 'auto', 1, 'u1', 'ui', 1),
                   ('a-gemini', 's1', 'Gemini', '0 9 * * *', 0, 'p', 'antigravity', 1, 'u1', 'ui', 1);",
            )?;
        }
        let store = SqliteStore::open(&path)?;
        let drawn = store.automation("a-auto").await?.ok_or("a-auto")?;
        assert_eq!(drawn.harness, Some(Harness::ClaudeCode));
        let named = store.automation("a-gemini").await?.ok_or("a-gemini")?;
        assert_eq!(named.harness, Some(Harness::Antigravity));
        Ok(())
    }

    /// SPEC-534. Upgrading cannot replay a slot whose starter was never recorded.
    #[tokio::test]
    async fn legacy_automation_schedules_wait_for_creator_approval()
    -> Result<(), Box<dyn std::error::Error>> {
        use yulius_app::automations::RunState;
        use yulius_app::ports::store_ext::AutomationStore as _;
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("legacy.db");
        {
            let conn = rusqlite::Connection::open(&path)?;
            for sql in &MIGRATIONS[..69] {
                conn.execute_batch(sql)?;
            }
            conn.pragma_update(None, "user_version", 69)?;
            conn.execute_batch(
                "INSERT INTO users (id, display_name, is_superadmin, is_instance_admin, created_at) VALUES ('u1', 'U', 0, 0, 1);
                 INSERT INTO sandboxes (id, slug, repo_owner, repo_name, default_branch, profile, port_base, port_count, created_by, onboarding_state, created_at) VALUES ('s1', 's', 'o', 'r', 'main', '{}', 20000, 10, 'u1', 'not_needed', 1);
                 INSERT INTO automations (id, sandbox_id, name, cron, utc_offset_minutes, prompt, harness, enabled, created_by, created_via, created_at)
                 VALUES ('a', 's1', 'Summary', '* * * * *', 0, 'summary', 'claude', 1, 'u1', 'ui', 1);
                 INSERT INTO automation_runs (automation_id, scheduled_for, state, created_at)
                 VALUES ('a', 1, 'scheduled', 1), ('a', 2, 'scheduled', 2);
                 INSERT INTO tasks (id, sandbox_id, number, title, prompt, harness, status, origin, automation_run, created_by, owner_id, created_at, updated_at)
                 VALUES ('t', 's1', 1, 'Summary', 'summary', 'claude', 'queued', 'automation', 'a@1', 'u1', 'u1', 1, 1);",
            )?;
        }
        let store = SqliteStore::open(&path)?;
        let saved = store.automation("a").await?.ok_or("automation")?;
        assert!(!saved.enabled);
        assert_eq!(saved.prompt, "summary");
        let runs = store.runs("a", 2).await?;
        assert_eq!(runs.len(), 2);
        assert_eq!(runs[0].state, RunState::SkippedDisabled);
        assert!(runs[0].task_id.is_none());
        assert_eq!(runs[1].state, RunState::TaskCreated);
        assert_eq!(
            runs[1]
                .task_id
                .as_ref()
                .map(yulius_domain::ids::TaskId::as_str),
            Some("t")
        );
        Ok(())
    }

    /// SPEC-525: Codex and Antigravity sessions were indexed with no passages (the reader knew
    /// only Claude Code's lines), so a version-68 database indexes them again from the start;
    /// a Claude Code session keeps its index.
    #[tokio::test]
    async fn version_sixty_eight_indexes_codex_and_antigravity_sessions_again()
    -> Result<(), Box<dyn std::error::Error>> {
        use yulius_app::ports::session_docs::SessionDocStore as _;
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("v68.db");
        {
            let conn = rusqlite::Connection::open(&path)?;
            for sql in &MIGRATIONS[..68] {
                conn.execute_batch(sql)?;
            }
            conn.pragma_update(None, "user_version", 68)?;
            conn.execute_batch(
                "INSERT INTO users (id, display_name, is_superadmin, is_instance_admin, created_at) VALUES ('u1', 'U', 0, 0, 1);
                 INSERT INTO sandboxes (id, slug, repo_owner, repo_name, default_branch, profile, port_base, port_count, created_by, onboarding_state, created_at) VALUES ('s1', 's', 'o', 'r', 'main', '{}', 20000, 10, 'u1', 'not_needed', 1);
                 INSERT INTO session_docs (id, sandbox_id, kind, harness, session_id, title, transcript_path, indexed_bytes, indexed_lines, index_full, created_at, updated_at) VALUES
                   ('d-codex', 's1', 'imported', 'codex', 'c', 'C', '/x/c.jsonl', 900, 12, 0, 1, 1),
                   ('d-agy', 's1', 'imported', 'antigravity', 'a', 'A', '/x/a.jsonl', 700, 9, 1, 1, 1),
                   ('d-claude', 's1', 'imported', 'claude', 'k', 'K', '/x/k.jsonl', 500, 7, 0, 1, 1);
                 INSERT INTO session_chunks (doc_id, line, role, text) VALUES ('d-claude', 1, 'user', 'kept passage'),
                   ('d-codex', 1, 'user', 'stalecodexpassage');",
            )?;
            // Exercise version 69 in isolation before opening the database with the latest schema.
            conn.execute_batch(MIGRATIONS[68])?;
            conn.pragma_update(None, "user_version", 69)?;
            let index = |id: &str| {
                conn.query_row(
                    "SELECT indexed_bytes, indexed_lines, index_full FROM session_docs WHERE id = ?1",
                    [id],
                    |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?, row.get::<_, bool>(2)?)),
                )
            };
            assert_eq!(index("d-codex")?, (0, 0, false));
            assert_eq!(index("d-agy")?, (0, 0, false));
            assert_eq!(index("d-claude")?, (500, 7, false));
            let matches = |term: &str| {
                conn.query_row(
                    "SELECT COUNT(*) FROM session_fts WHERE session_fts MATCH ?1",
                    [term],
                    |row| row.get::<_, i64>(0),
                )
            };
            let chunks = |id: &str| {
                conn.query_row(
                    "SELECT COUNT(*) FROM session_chunks WHERE doc_id = ?1",
                    [id],
                    |row| row.get::<_, i64>(0),
                )
            };
            assert_eq!(chunks("d-claude")?, 1);
            assert_eq!(chunks("d-codex")?, 0);
            assert_eq!(matches("kept")?, 1);
            assert_eq!(matches("stalecodexpassage")?, 0);
            // Bring the fixture to dev's schema, including the automation and digest migrations.
            conn.execute_batch(MIGRATIONS[69])?;
            conn.execute_batch(MIGRATIONS[70])?;
            conn.pragma_update(None, "user_version", 71)?;
            // A full session must become eligible for lazy reindexing too.
            conn.execute(
                "UPDATE session_docs SET index_full = 1 WHERE id = 'd-claude'",
                [],
            )?;
        }
        // SPEC-535: an upgrade clears even a passage already indexed by a version-71 server.
        let store = SqliteStore::open(&path)?;
        let sandbox = yulius_domain::ids::SandboxId::parse("s1")?;
        let docs = store.session_docs(&sandbox).await?;
        assert_eq!(docs.len(), 3);
        for doc in docs {
            assert_eq!(
                (doc.indexed_bytes, doc.indexed_lines, doc.index_full),
                (0, 0, false)
            );
        }
        assert!(
            store
                .search_session_chunks(&sandbox, "kept", 10)
                .await?
                .is_empty()
        );
        let counts: (i64, i64) = store.call(|conn| {
            conn.query_row(
                "SELECT (SELECT COUNT(*) FROM session_chunks), (SELECT COUNT(*) FROM session_fts WHERE session_fts MATCH 'kept')",
                [], |row| Ok((row.get(0)?, row.get(1)?)),
            ).map_err(db)
        }).await?;
        assert_eq!(counts, (0, 0));
        Ok(())
    }

    /// SPEC-483 slice 35 (QC1): the capacity and accounts views read the runs and pauses of
    /// their window through an index, never a scan of every run ever recorded. The plans are
    /// those of a database without statistics, as the store's are: nothing runs `ANALYZE`, and
    /// without statistics a populated database plans as an empty one (review of slice 35).
    #[tokio::test]
    async fn the_capacity_views_read_their_window_through_an_index()
    -> Result<(), Box<dyn std::error::Error>> {
        let store = SqliteStore::open_in_memory()?;
        for sql in [
            crate::capacity::TOKEN_RUNS_SQL.to_owned(),
            crate::capacity::pauses_since_sql(crate::capacity::PAUSE_COLS),
            crate::accounts::ACCOUNT_TOKENS_SQL.to_owned(),
        ] {
            let plan: Vec<String> = store
                .call(move |c| {
                    let mut statement = c
                        .prepare(&format!("EXPLAIN QUERY PLAN {sql}"))
                        .map_err(db)?;
                    let details = statement
                        .query_map([0_i64], |r| r.get::<_, String>(3))
                        .map_err(db)?
                        .collect::<Result<Vec<String>, _>>()
                        .map_err(db)?;
                    Ok(details)
                })
                .await?;
            for step in &plan {
                assert!(
                    !(step.starts_with("SCAN a") || step.starts_with("SCAN capacity_pauses")),
                    "{step} in {plan:?}"
                );
            }
        }
        Ok(())
    }

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
               ('t1', 's1', 1, 'T', 'p', 'claude', 'closed', 'ui', 'u1', 1, 1),
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

    /// A recount's outcome, figures before and after, and when.
    type RecountRow = (Option<String>, Option<String>, Option<String>, Option<i64>);

    fn recount_row(path: &Path, attempt: &str) -> Result<RecountRow, Box<dyn std::error::Error>> {
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
        // The upgrade's start ends a run lost while Yulius was down and saves its empty figures
        // before the recount: "before" is still what the upgrade found.
        rusqlite::Connection::open(&path)?.execute_batch(
            "UPDATE metrics SET input_tokens = NULL, output_tokens = NULL, turns = NULL WHERE attempt_id = 'a1'",
        )?;
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

    /// SPEC-490 OWN-009: a task of a version-58 database (handed over before the column existed)
    /// has nobody who may take it back.
    #[tokio::test]
    async fn version_fifty_eight_tasks_have_no_giver() -> Result<(), Box<dyn std::error::Error>> {
        use yulius_app::ports::store::TaskStore as _;
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("v58.db");
        {
            let conn = rusqlite::Connection::open(&path)?;
            for sql in &MIGRATIONS[..58] {
                conn.execute_batch(sql)?;
            }
            conn.pragma_update(None, "user_version", 58)?;
            conn.execute_batch(
                "INSERT INTO users (id, display_name, is_superadmin, is_instance_admin, created_at) VALUES ('u1', 'U', 0, 0, 1), ('u2', 'V', 0, 0, 1);
                 INSERT INTO sandboxes (id, slug, repo_owner, repo_name, default_branch, profile, port_base, port_count, created_by, onboarding_state, created_at) VALUES ('s1', 's', 'o', 'r', 'main', '{}', 20000, 10, 'u1', 'not_needed', 1);
                 INSERT INTO tasks (id, sandbox_id, number, title, prompt, harness, status, origin, created_by, owner_id, created_at, updated_at) VALUES
                   ('t1', 's1', 1, 'T', 'p', 'claude', 'closed', 'ui', 'u1', 'u2', 1, 500);",
            )?;
        }
        let store = SqliteStore::open(&path)?;
        let task = yulius_domain::ids::TaskId::parse("t1")?;
        assert_eq!(store.control_from(&task).await?, None);
        Ok(())
    }

    /// SPEC-482 FBK-005: a run records the model it was launched with; the runs of a version-53
    /// database have none, and the column round-trips.
    #[tokio::test]
    async fn version_fifty_three_runs_have_no_launch_model()
    -> Result<(), Box<dyn std::error::Error>> {
        use yulius_app::ports::store::TaskStore as _;
        use yulius_domain::ids::TaskId;
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("v53.db");
        {
            let conn = rusqlite::Connection::open(&path)?;
            for sql in &MIGRATIONS[..53] {
                conn.execute_batch(sql)?;
            }
            conn.pragma_update(None, "user_version", 53)?;
            conn.execute_batch(
                "INSERT INTO users (id, display_name, is_superadmin, is_instance_admin, created_at) VALUES ('u1', 'U', 0, 0, 1);
                 INSERT INTO sandboxes (id, slug, repo_owner, repo_name, default_branch, profile, port_base, port_count, created_by, onboarding_state, created_at) VALUES ('s1', 's', 'o', 'r', 'main', '{}', 20000, 10, 'u1', 'not_needed', 1);
                 INSERT INTO tasks (id, sandbox_id, number, title, prompt, harness, status, activity, origin, created_by, created_at, updated_at) VALUES
                   ('t1', 's1', 1, 'T', 'p', 'claude', 'running', 'working', 'ui', 'u1', 1, 500);
                 INSERT INTO attempts (id, task_id, seq, harness, launch_kind, state, run_dir, token_hash, host_token, started_at) VALUES
                   ('a1', 't1', 1, 'claude', 'new', 'live', '/r/a1', 'h1', x'00', 100);",
            )?;
        }
        let store = SqliteStore::open(&path)?;
        let task = TaskId::parse("t1")?;
        let old = store.live_attempt(&task).await?.ok_or("the old run")?;
        assert_eq!(old.model, None);
        store
            .transition_attempt(
                &old.id,
                yulius_domain::task::AttemptState::Exited,
                &yulius_app::ports::store::AttemptUpdate::default(),
            )
            .await?;
        let mut next = old.clone();
        next.id = yulius_domain::ids::AttemptId::parse("a2")?;
        next.seq = 2;
        next.token_hash = "h2".into();
        next.model = Some("claude-opus-5-5".into());
        store.insert_attempt(&next).await?;
        let stored = store.live_attempt(&task).await?.ok_or("the new run")?;
        assert_eq!(stored.model.as_deref(), Some("claude-opus-5-5"));
        Ok(())
    }

    /// SPEC-484 PLN-009: a version-55 task has no first plan size, whatever its steps; its next
    /// step write gives it one (the plan as it is then).
    #[tokio::test]
    async fn version_fifty_five_plans_get_their_first_size_at_the_next_step_write()
    -> Result<(), Box<dyn std::error::Error>> {
        use yulius_app::ports::store::CoordinationStore as _;
        use yulius_domain::ids::{StepId, TaskId};
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("v55.db");
        {
            let conn = rusqlite::Connection::open(&path)?;
            for sql in &MIGRATIONS[..55] {
                conn.execute_batch(sql)?;
            }
            conn.pragma_update(None, "user_version", 55)?;
            conn.execute_batch(
                "INSERT INTO users (id, display_name, is_superadmin, is_instance_admin, created_at) VALUES ('u1', 'U', 0, 0, 1);
                 INSERT INTO sandboxes (id, slug, repo_owner, repo_name, default_branch, profile, port_base, port_count, created_by, onboarding_state, created_at) VALUES ('s1', 's', 'o', 'r', 'main', '{}', 20000, 10, 'u1', 'not_needed', 1);
                 INSERT INTO tasks (id, sandbox_id, number, title, prompt, harness, status, activity, origin, created_by, created_at, updated_at) VALUES
                   ('t1', 's1', 1, 'T', 'p', 'claude', 'running', 'working', 'ui', 'u1', 1, 500);
                 INSERT INTO steps (id, task_id, ordinal, title, status, effort) VALUES
                   ('a', 't1', 1, 'Slice 1', 'done', 1), ('b', 't1', 2, 'Slice 2', 'in_progress', 1),
                   ('c', 't1', 3, 'Slice 3', 'pending', 1);",
            )?;
        }
        let store = SqliteStore::open(&path)?;
        let task = TaskId::parse("t1")?;
        assert_eq!(store.plan_first_steps(&task).await?, None);
        store
            .update_step(
                &StepId::parse("b")?,
                "done",
                None,
                "task:1",
                yulius_domain::time::Timestamp(600),
            )
            .await?;
        assert_eq!(store.plan_first_steps(&task).await?, Some(3));
        Ok(())
    }

    /// SPEC-491 DON-004: a version-58 database gets the column; what a task's spec says of its
    /// work is kept and read back, a write that changes nothing says so, and a task whose spec
    /// was never read has none.
    #[tokio::test]
    async fn a_task_keeps_what_its_spec_says_of_its_work() -> Result<(), Box<dyn std::error::Error>>
    {
        use yulius_app::ports::store::CoordinationStore as _;
        use yulius_domain::ids::TaskId;
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("v58.db");
        {
            let conn = rusqlite::Connection::open(&path)?;
            for sql in &MIGRATIONS[..58] {
                conn.execute_batch(sql)?;
            }
            conn.pragma_update(None, "user_version", 58)?;
            conn.execute_batch(
                "INSERT INTO users (id, display_name, is_superadmin, is_instance_admin, created_at) VALUES ('u1', 'U', 0, 0, 1);
                 INSERT INTO sandboxes (id, slug, repo_owner, repo_name, default_branch, profile, port_base, port_count, created_by, onboarding_state, created_at) VALUES ('s1', 's', 'o', 'r', 'main', '{}', 20000, 10, 'u1', 'not_needed', 1);
                 INSERT INTO tasks (id, sandbox_id, number, title, prompt, harness, status, activity, origin, created_by, created_at, updated_at) VALUES
                   ('t1', 's1', 1, 'T', 'p', 'claude', 'running', 'waiting', 'ui', 'u1', 1, 500);",
            )?;
        }
        let store = SqliteStore::open(&path)?;
        let task = TaskId::parse("t1")?;
        assert_eq!(store.spec_status(&task).await?, None);
        assert!(store.set_spec_status(&task, Some("in-progress")).await?);
        assert!(!store.set_spec_status(&task, Some("in-progress")).await?);
        assert_eq!(
            store.spec_status(&task).await?.as_deref(),
            Some("in-progress")
        );
        assert!(store.set_spec_status(&task, Some("done")).await?);
        assert_eq!(store.spec_status(&task).await?.as_deref(), Some("done"));
        assert!(store.set_spec_status(&task, None).await?);
        assert_eq!(store.spec_status(&task).await?, None);
        assert_eq!(store.spec_status(&TaskId::parse("gone")?).await?, None);
        Ok(())
    }

    /// SPEC-483 CLT-002: a version-57 database loses the close time of tasks that are not
    /// closed (continued since, or created chained or prepared with one); a closed task keeps it.
    #[test]
    fn a_task_that_is_not_closed_loses_its_old_close_time() -> Result<(), Box<dyn std::error::Error>>
    {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("v57.db");
        {
            let conn = rusqlite::Connection::open(&path)?;
            for sql in &MIGRATIONS[..57] {
                conn.execute_batch(sql)?;
            }
            conn.pragma_update(None, "user_version", 57)?;
            conn.execute_batch(
                "INSERT INTO users (id, display_name, is_superadmin, is_instance_admin, created_at) VALUES ('u1', 'U', 0, 0, 1);
                 INSERT INTO sandboxes (id, slug, repo_owner, repo_name, default_branch, profile, port_base, port_count, created_by, onboarding_state, created_at) VALUES ('s1', 's', 'o', 'r', 'main', '{}', 20000, 10, 'u1', 'not_needed', 1);
                 INSERT INTO tasks (id, sandbox_id, number, title, prompt, harness, status, activity, origin, created_by, created_at, updated_at, closed_at) VALUES
                   ('t1', 's1', 1, 'T', 'p', 'claude', 'running', 'working', 'ui', 'u1', 1, 500, 300),
                   ('t2', 's1', 2, 'T', 'p', 'claude', 'chained', NULL, 'ui', 'u1', 1, 500, 1),
                   ('t3', 's1', 3, 'T', 'p', 'claude', 'closed', NULL, 'ui', 'u1', 1, 500, 400);",
            )?;
        }
        drop(SqliteStore::open(&path)?);
        let raw = rusqlite::Connection::open(&path)?;
        let version: u32 = raw.query_row("PRAGMA user_version", [], |row| row.get(0))?;
        assert_eq!(
            version,
            schema_version(),
            "the version-57 database was migrated"
        );
        let mut rows = raw.prepare("SELECT id, closed_at FROM tasks ORDER BY id")?;
        let closed: Vec<(String, Option<i64>)> = rows
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
            .collect::<Result<_, _>>()?;
        assert_eq!(
            closed,
            [
                ("t1".into(), None),
                ("t2".into(), None),
                ("t3".into(), Some(400))
            ]
        );
        Ok(())
    }

    /// SPEC-483 QST-001: a task is queued with how it is to start, in one step. The start is
    /// read while the task waits, and is gone from the row once it left the queue; one written
    /// by hand that this build cannot read gives none.
    #[tokio::test]
    async fn a_queued_task_keeps_its_start_while_it_waits() -> Result<(), Box<dyn std::error::Error>>
    {
        use yulius_app::model::{QueuedStart, QueuedStartKind};
        use yulius_app::ports::store::TaskStore as _;
        use yulius_domain::ids::{AccountId, TaskId};
        use yulius_domain::task::{StopReason, TaskStatus};
        use yulius_domain::time::Timestamp;
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("v56.db");
        {
            let conn = rusqlite::Connection::open(&path)?;
            for sql in &MIGRATIONS[..56] {
                conn.execute_batch(sql)?;
            }
            conn.pragma_update(None, "user_version", 56)?;
            conn.execute_batch(
                "INSERT INTO users (id, display_name, is_superadmin, is_instance_admin, created_at) VALUES ('u1', 'U', 0, 0, 1);
                 INSERT INTO sandboxes (id, slug, repo_owner, repo_name, default_branch, profile, port_base, port_count, created_by, onboarding_state, created_at) VALUES ('s1', 's', 'o', 'r', 'main', '{}', 20000, 10, 'u1', 'not_needed', 1);
                 INSERT INTO tasks (id, sandbox_id, number, title, prompt, harness, status, activity, origin, created_by, created_at, updated_at) VALUES
                   ('t1', 's1', 1, 'T', 'p', 'claude', 'waiting_capacity', NULL, 'ui', 'u1', 1, 500);",
            )?;
        }
        let store = SqliteStore::open(&path)?;
        let raw = rusqlite::Connection::open(&path)?;
        let version: u32 = raw.query_row("PRAGMA user_version", [], |row| row.get(0))?;
        assert_eq!(
            version,
            schema_version(),
            "the version-56 database was migrated"
        );
        let kept = |raw: &rusqlite::Connection| -> rusqlite::Result<Option<String>> {
            raw.query_row(
                "SELECT queued_start FROM tasks WHERE id = 't1'",
                [],
                |row| row.get(0),
            )
        };
        let task = TaskId::parse("t1")?;
        let start = QueuedStart {
            kind: QueuedStartKind::Resume,
            prompt: Some("go on\nwith \"this\" and null".into()),
            account_id: Some(AccountId::parse("a-chosen")?),
        };
        // Not in the status it was read in: nothing changes.
        assert!(
            !store
                .queue_task(&task, TaskStatus::Running, &start, Timestamp(550))
                .await?
        );
        let row = |raw: &rusqlite::Connection| -> rusqlite::Result<(String, Option<i64>)> {
            raw.query_row(
                "SELECT status, queued_at FROM tasks WHERE id = 't1'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
        };
        assert_eq!(row(&raw)?, ("waiting_capacity".to_owned(), None));
        assert_eq!(kept(&raw)?, None);

        assert!(
            store
                .queue_task(&task, TaskStatus::WaitingCapacity, &start, Timestamp(600))
                .await?
        );
        assert_eq!(row(&raw)?, ("queued".to_owned(), Some(600)));
        assert_eq!(store.queued_start(&task).await?, Some(start.clone()));

        // A start this build cannot read (another kind, a subscription id of another shape,
        // no JSON at all) is none, not an error.
        for unreadable in [
            r#"{"kind":"later","prompt":null,"account":null,"at":600}"#,
            r#"{"kind":"resume","prompt":null,"account":"not an id","at":600}"#,
            r#"{"kind":7,"prompt":null,"account":null,"at":600}"#,
            "not json",
        ] {
            raw.execute(
                "UPDATE tasks SET queued_start = ?1 WHERE id = 't1'",
                [unreadable],
            )?;
            assert_eq!(store.queued_start(&task).await?, None, "{unreadable}");
        }
        // One of an earlier wait (another `at`) is not this wait's.
        raw.execute(
            "UPDATE tasks SET queued_start = ?1 WHERE id = 't1'",
            [r#"{"kind":"after_limit","prompt":null,"account":null,"at":599}"#],
        )?;
        assert_eq!(store.queued_start(&task).await?, None);

        // It leaves the queue (stopped): the start is gone from the row, held prompt and all.
        let stopped = TaskStatus::Stopped(StopReason::Cancelled);
        assert!(
            store
                .transition_task(
                    &task,
                    Some(TaskStatus::Queued),
                    stopped,
                    None,
                    Timestamp(700)
                )
                .await?
        );
        assert_eq!(kept(&raw)?, None);
        assert_eq!(store.queued_start(&task).await?, None);
        // Queued again with another start: that one is its start.
        let plain = QueuedStart {
            kind: QueuedStartKind::AfterLimit,
            prompt: None,
            account_id: None,
        };
        assert!(
            store
                .queue_task(&task, stopped, &plain, Timestamp(800))
                .await?
        );
        assert_eq!(store.queued_start(&task).await?, Some(plain));
        assert!(
            store
                .transition_task(
                    &task,
                    Some(TaskStatus::Queued),
                    TaskStatus::Starting,
                    None,
                    Timestamp(900)
                )
                .await?
        );
        assert_eq!(kept(&raw)?, None, "launched: nothing is kept");
        Ok(())
    }

    /// A version-27 database's history is backfilled into spans where it is reliably derivable
    /// (SPEC-137 STG-004), marked `backfilled`: CI queues, runs and jobs (a reused result marked),
    /// CD runs, task runs with their figures, steps from their events, pauses, and a live task's
    /// current state.
    #[tokio::test]
    async fn version_twenty_seven_history_is_backfilled_into_spans()
    -> Result<(), Box<dyn std::error::Error>> {
        use yulius_app::ports::store_ext::SpanStore as _;
        use yulius_domain::ids::SandboxId;
        use yulius_domain::stages::SubjectKind;
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("v27.db");
        {
            let conn = rusqlite::Connection::open(&path)?;
            for sql in &MIGRATIONS[..27] {
                conn.execute_batch(sql)?;
            }
            conn.pragma_update(None, "user_version", 27)?;
            conn.execute_batch(
                "INSERT INTO users (id, display_name, created_at) VALUES ('u1', 'U', 1);
                 INSERT INTO sandboxes (id, slug, repo_owner, repo_name, default_branch, profile, port_base, port_count, created_by, onboarding_state, created_at) VALUES ('s1', 's', 'o', 'r', 'main', '{}', 20000, 10, 'u1', 'not_needed', 1);
                 INSERT INTO tasks (id, sandbox_id, number, title, prompt, harness, status, activity, origin, created_by, created_at, updated_at, state_at) VALUES
                   ('t1', 's1', 1, 'T', 'p', 'claude', 'running', 'waiting', 'ui', 'u1', 1, 900, 900),
                   ('t2', 's1', 2, 'T', 'p', 'claude', 'closed', NULL, 'ui', 'u1', 1, 900, 900);
                 INSERT INTO attempts (id, task_id, seq, harness, launch_kind, state, run_dir, token_hash, host_token, started_at, ended_at) VALUES
                   ('a1', 't1', 1, 'claude', 'new', 'exited', '/r', 'h', x'00', 100, 400);
                 INSERT INTO metrics (attempt_id, task_id, output_tokens, active_ms, updated_at) VALUES ('a1', 't1', 42, 7000, 400);
                 INSERT INTO steps (id, task_id, ordinal, title, status) VALUES ('st1', 't1', 1, 'Spec', 'done');
                 INSERT INTO step_events (step_id, task_id, at, actor, from_status, to_status) VALUES
                   ('st1', 't1', 150, 'a', 'pending', 'in_progress'),
                   ('st1', 't1', 160, 'a', 'in_progress', 'in_progress'),
                   ('st1', 't1', 300, 'a', 'in_progress', 'done');
                 INSERT INTO capacity_pauses (id, task_id, kind, started_at, ended_at, outcome) VALUES ('p1', 't1', 'limit', 200, 250, 'switched');
                 INSERT INTO ci_runs (id, sandbox_id, reference, sha, source, state, reason, requested_by, queued_at, started_at, ended_at) VALUES
                   ('r1', 's1', 'pr/7', 'abc', 'yulius_toml', 'passed', NULL, 'yulius', 10, 20, 80),
                   ('r2', 's1', 'branch/dev', 'abc', 'yulius_toml', 'passed', 'not run again: same tree', 'yulius', 90, 90, 90);
                 INSERT INTO ci_jobs (id, run_id, ordinal, name, job_key, command, state, exit_code, started_at, ended_at) VALUES
                   ('j1', 'r1', 0, 'quality', 'quality', 'make', 'passed', 0, 25, 75);
                 INSERT INTO cd_runs (id, sandbox_id, environment, branch, sha, state, requested_by, started_at, ended_at) VALUES
                   ('cd1', 's1', 'prod', 'main', 'abc', 'passed', 'mkarp', 500, 560);",
            )?;
        }
        let store = SqliteStore::open(&path)?;
        let sid = SandboxId::parse("s1")?;
        let read = |kind, id: &'static str| store.subject_spans(&sid, kind, id, 1_000);
        let shape = |spans: Vec<yulius_app::stages::SpanRecord>| {
            spans
                .into_iter()
                .map(|s| {
                    (
                        s.kind,
                        s.name,
                        s.started_at.0,
                        s.ended_at.map(|t| t.0),
                        s.source,
                    )
                })
                .collect::<Vec<_>>()
        };
        let b = || "backfilled".to_owned();
        assert_eq!(
            shape(read(SubjectKind::CiRun, "r1").await?),
            vec![
                ("queue".into(), "queue".into(), 10, Some(20), b()),
                ("run".into(), "run".into(), 20, Some(80), b()),
                ("job".into(), "quality".into(), 25, Some(75), b()),
            ]
        );
        let reused = read(SubjectKind::CiRun, "r2").await?;
        assert!(reused.iter().all(|s| {
            s.detail
                .as_deref()
                .unwrap_or("")
                .contains("\"reused\":true")
        }));
        assert_eq!(
            shape(read(SubjectKind::CdRun, "cd1").await?),
            vec![("run".into(), "run".into(), 500, Some(560), b())]
        );
        let task = read(SubjectKind::Task, "t1").await?;
        assert_eq!(
            shape(task.clone()),
            vec![
                ("run".into(), "run".into(), 100, Some(400), b()),
                ("step".into(), "Spec".into(), 150, Some(300), b()),
                ("pause".into(), "limit".into(), 200, Some(250), b()),
                ("state".into(), "idle".into(), 900, None, b()),
            ]
        );
        assert!(
            task[0]
                .detail
                .as_deref()
                .unwrap_or("")
                .contains("\"output_tokens\":42")
        );
        assert!(
            read(SubjectKind::Task, "t2").await?.is_empty(),
            "an ended task's states are not stored"
        );
        Ok(())
    }

    /// SPEC-137: each step looks up its next event by index. Count the work done by SQLite
    /// itself so a busy CI host cannot turn a bounded lookup into a test failure.
    #[test]
    fn a_large_step_history_backfills_with_bounded_work() -> Result<(), Box<dyn std::error::Error>>
    {
        use rusqlite::fallible_iterator::FallibleIterator as _;

        let events = 4_000_i64;
        let mut conn = rusqlite::Connection::open_in_memory()?;
        for sql in &MIGRATIONS[..27] {
            conn.execute_batch(sql)?;
        }
        conn.execute_batch(
            "INSERT INTO users (id, display_name, created_at) VALUES ('u1', 'U', 1);
             INSERT INTO sandboxes (id, slug, repo_owner, repo_name, default_branch, profile, port_base, port_count, created_by, onboarding_state, created_at) VALUES ('s1', 's', 'o', 'r', 'main', '{}', 20000, 10, 'u1', 'not_needed', 1);
             INSERT INTO tasks (id, sandbox_id, number, title, prompt, harness, status, origin, created_by, created_at, updated_at) VALUES ('t1', 's1', 1, 'T', 'p', 'claude', 'closed', 'ui', 'u1', 1, 1);",
        )?;
        let tx = conn.transaction()?;
        for i in 0..events {
            let step = format!("st{}", i / 2);
            let to = if i % 2 == 0 { "in_progress" } else { "done" };
            tx.execute(
                "INSERT INTO step_events (step_id, task_id, at, actor, to_status) VALUES (?1, 't1', ?2, 'a', ?3)",
                rusqlite::params![step, i, to],
            )?;
        }
        tx.commit()?;

        let tx = conn.transaction()?;
        let mut migration = rusqlite::Batch::new(&tx, MIGRATIONS[27]);
        let mut instructions = 0_i64;
        while let Some(mut statement) = migration.next()? {
            statement.execute([])?;
            instructions += i64::from(statement.get_status(rusqlite::StatementStatus::VmStep));
        }
        tx.commit()?;
        let completed: i64 = conn.query_row(
            "SELECT count(*) FROM spans WHERE subject_kind = 'task' AND subject_id = 't1'
             AND kind = 'step' AND source = 'backfilled' AND ended_at = started_at + 1
             AND outcome = 'done'",
            [],
            |row| row.get(0),
        )?;
        assert_eq!(completed, events / 2);
        assert!(
            instructions < events * 512,
            "backfilling {events} events used {instructions} SQLite instructions"
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

    /// SPEC-483 V6 (ADR-0324): a production secret that kept hosts from before such secrets
    /// became exclusive loses them at the migration, with a line in the audit log; another
    /// secret's hosts, and a production secret without any, are left as they are.
    #[tokio::test]
    async fn a_production_secrets_old_hosts_are_removed() -> Result<(), Box<dyn std::error::Error>>
    {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("v54.db");
        {
            let conn = rusqlite::Connection::open(&path)?;
            for sql in &MIGRATIONS[..54] {
                conn.execute_batch(sql)?;
            }
            conn.pragma_update(None, "user_version", 54)?;
            for (id, name, hosts, cd_prod) in [
                ("old", "PROD_OLD", r#"["api.example.com","*.old.test"]"#, 1),
                ("prod", "PROD_NEW", "[]", 1),
                ("agent", "AGENT_KEY", r#"["api.example.com"]"#, 0),
            ] {
                conn.execute(
                    "INSERT INTO vault_secrets (id, section, sandbox_id, name, hosts, kek_id, \
                     wrapped_key, ciphertext, created_by, created_at, updated_by, updated_at, \
                     value_updated_at, cd_prod) \
                     VALUES (?1, 'agent', NULL, ?2, ?3, 1, x'00', x'00', 'u', 1, 'u', 1, 1, ?4)",
                    rusqlite::params![id, name, hosts, cd_prod],
                )?;
            }
        }
        drop(SqliteStore::open(&path)?);
        let conn = rusqlite::Connection::open(&path)?;
        let hosts = |id: &str| -> rusqlite::Result<String> {
            conn.query_row(
                "SELECT hosts FROM vault_secrets WHERE id = ?1",
                [id],
                |row| row.get(0),
            )
        };
        assert_eq!(hosts("old")?, "[]");
        assert_eq!(hosts("prod")?, "[]");
        assert_eq!(hosts("agent")?, r#"["api.example.com"]"#);
        let noted: Vec<(String, String, String, i64)> = conn
            .prepare("SELECT actor, target, detail, at FROM audit_log WHERE action = 'vault.agent.hosts_cleared'")?
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)))?
            .collect::<Result<_, _>>()?;
        assert_eq!(noted.len(), 1, "{noted:?}");
        let (actor, target, detail, at) = &noted[0];
        assert_eq!((actor.as_str(), target.as_str()), ("yulius", "old"));
        // A sentence, as every line of the log: the secret and the hosts it had.
        assert_eq!(
            detail,
            "PROD_OLD: api.example.com, *.old.test (a production secret is sent to no host)"
        );
        // Milliseconds, as every time in the log.
        assert!(*at > 1_700_000_000_000, "{at}");
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

    /// A version-36 database's MCP servers get their origin (SPEC-218 MOR-001): those an
    /// `mcp.imported` audit entry names came from `.mcp.json`, every other one from a person.
    #[tokio::test]
    async fn version_thirty_six_imported_mcp_servers_come_from_mcp_json()
    -> Result<(), Box<dyn std::error::Error>> {
        use yulius_app::mcp_config::McpOrigin;
        use yulius_app::ports::store_ext::McpStore as _;
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("v36.db");
        {
            let conn = Connection::open(&path)?;
            for sql in &MIGRATIONS[..36] {
                conn.execute_batch(sql)?;
            }
            conn.pragma_update(None, "user_version", 36)?;
            conn.execute_batch(
                "INSERT INTO mcp_servers (id, sandbox_id, name, transport, command, args, enabled, created_at) VALUES
                   ('added', 's1', 'apify', 'stdio', 'npx', '[]', 1, 1),
                   ('imported', 's1', 'helper', 'stdio', 'sh', '[]', 1, 1);
                 INSERT INTO audit_log (at, actor, action, target, detail) VALUES
                   (1, 'alice', 'mcp.created', 'added', NULL),
                   (2, 'yulius', 'mcp.imported', 'imported', NULL),
                   (3, 'yulius', 'mcp.imported', NULL, NULL);",
            )?;
        }
        let store = SqliteStore::open(&path)?;
        let origin = |id: &'static str| {
            let store = &store;
            async move { Ok::<_, AppError>(store.mcp_server(id).await?.map(|s| s.origin)) }
        };
        assert_eq!(origin("added").await?, Some(McpOrigin::Person));
        assert_eq!(origin("imported").await?, Some(McpOrigin::McpJson));
        Ok(())
    }

    /// An email stored before a person could type one is Claude's (SPEC-463 EML-004): after the
    /// upgrade it is still shown, and the next sign-in still drops it (SPEC-244 SUB-003).
    #[tokio::test]
    async fn emails_stored_before_one_could_be_typed_stay_claudes()
    -> Result<(), Box<dyn std::error::Error>> {
        use yulius_app::model::{AccountStatus, Sealed};
        use yulius_app::ports::store::AccountStore as _;
        use yulius_domain::ids::AccountId;
        use yulius_domain::time::Timestamp;
        let before = MIGRATIONS
            .iter()
            .position(|sql| sql.contains("ADD COLUMN email_typed"))
            .ok_or("the migration that adds email_typed")?;
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("before.db");
        {
            let conn = Connection::open(&path)?;
            for sql in &MIGRATIONS[..before] {
                conn.execute_batch(sql)?;
            }
            conn.pragma_update(None, "user_version", i64::try_from(before)?)?;
            conn.execute_batch(
                "INSERT INTO accounts (id, provider, label, credential, status, email, email_checked_at, created_at, updated_at) VALUES
                   ('known', 'anthropic', 'work Max', x'01', 'ok', 'pawel@example.com', 5, 1, 5),
                   ('unknown', 'anthropic', 'personal', x'02', 'ok', NULL, NULL, 1, 1);",
            )?;
        }
        let store = SqliteStore::open(&path)?;
        let known = AccountId::parse("known")?;
        let kept = store.account(&known).await?.ok_or("account")?;
        assert_eq!(kept.email.as_deref(), Some("pawel@example.com"));
        assert_eq!(kept.email_checked_at, Some(Timestamp(5)));
        assert!(!kept.email_typed);
        store
            .set_account_credential(&known, &Sealed(vec![3]), AccountStatus::Ok, Timestamp(6))
            .await?;
        let signed_in = store.account(&known).await?.ok_or("account")?;
        assert_eq!((signed_in.email, signed_in.email_checked_at), (None, None));
        // Claude's answer for a subscription without one is stored as before.
        let unknown = AccountId::parse("unknown")?;
        assert!(
            store
                .set_account_email(
                    &unknown,
                    &Sealed(vec![2]),
                    Some("p@example.com"),
                    Some(Timestamp(7)),
                    Timestamp(7),
                )
                .await?
        );
        assert_eq!(
            store
                .account(&unknown)
                .await?
                .ok_or("account")?
                .email
                .as_deref(),
            Some("p@example.com")
        );
        Ok(())
    }

    /// A version-49 instance has no limits record (SPEC-459 HSL-009); one saved is read back,
    /// and an instance that is not set up has no row to keep it.
    #[tokio::test]
    async fn version_forty_nine_instance_gets_a_limits_record()
    -> Result<(), Box<dyn std::error::Error>> {
        use yulius_app::ports::store_ext::UpdateStore as _;
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("v49.db");
        {
            let conn = Connection::open(&path)?;
            for sql in &MIGRATIONS[..49] {
                conn.execute_batch(sql)?;
            }
            conn.pragma_update(None, "user_version", 49)?;
            conn.execute_batch(
                "INSERT INTO instance (singleton, public_url) VALUES (1, 'https://yulius.test')",
            )?;
        }
        let store = SqliteStore::open(&path)?;
        assert_eq!(store.limits().await?, None);
        store.set_limits(r#"{"unit":{}}"#).await?;
        assert_eq!(store.limits().await?.as_deref(), Some(r#"{"unit":{}}"#));

        let empty = SqliteStore::open(&dir.path().join("empty.db"))?;
        assert_eq!(empty.limits().await?, None);
        assert!(matches!(
            empty.set_limits("{}").await,
            Err(AppError::Conflict(_))
        ));
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
