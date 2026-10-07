//! SQLite persistence adapter: schema, migrations, repositories and backup (SPEC-002 §5).
//!
//! All database access runs on one dedicated thread that owns the connection (single writer,
//! WAL mode). Async callers submit closures through a bounded queue and await the result, so
//! the async executor never blocks on SQLite and concurrent writers never race.

mod accounts;
mod audit;
mod coordination;
pub mod crypto;
mod ext;
mod imports;
mod instance;
mod sandboxes;
mod sessions;
mod tasks;
mod users;

use std::path::Path;
use std::sync::mpsc;
use std::thread;

use rusqlite::Connection;
use tokio::sync::oneshot;
use yulius_app::error::{AppError, AppResult};

/// Bound of pending database jobs.
const QUEUE_BOUND: usize = 1024;

/// Ordered migrations; index + 1 is the schema version stored in `PRAGMA user_version`.
const MIGRATIONS: &[&str] = &[include_str!("migrations/0001_initial.sql")];

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
            assert_eq!(version, 1);
        }
        // Reopen applies nothing new and keeps the version.
        let store = SqliteStore::open(&path)?;
        let version: i64 = store
            .call(|c| {
                c.query_row("PRAGMA user_version", [], |r| r.get(0))
                    .map_err(db)
            })
            .await?;
        assert_eq!(version, 1);
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
