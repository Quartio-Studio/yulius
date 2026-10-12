//! Notepads (SPEC-041): one row per scope, written by compare-and-set on its revision.

use async_trait::async_trait;
use rusqlite::{OptionalExtension, Row, params};
use yulius_app::error::AppResult;
use yulius_app::notes::NoteRecord;
use yulius_app::ports::store_ext::NoteStore;
use yulius_domain::time::Timestamp;

use crate::{SqliteStore, db, to_i64, to_u64};

const COLS: &str = "scope, body, revision, updated_by, updated_at";

fn from_row(row: &Row<'_>) -> rusqlite::Result<NoteRecord> {
    Ok(NoteRecord {
        scope: row.get("scope")?,
        body: row.get("body")?,
        revision: to_u64(row.get("revision")?),
        updated_by: row.get("updated_by")?,
        updated_at: Timestamp(row.get("updated_at")?),
    })
}

#[async_trait]
impl NoteStore for SqliteStore {
    async fn note(&self, scope: &str) -> AppResult<Option<NoteRecord>> {
        let scope = scope.to_owned();
        self.call(move |c| {
            c.query_row(
                &format!("SELECT {COLS} FROM notes WHERE scope = ?1"),
                [scope],
                from_row,
            )
            .optional()
            .map_err(db)
        })
        .await
    }

    async fn put_note(
        &self,
        scope: &str,
        body: &str,
        base_revision: u64,
        updated_by: &str,
        at: Timestamp,
    ) -> AppResult<Option<NoteRecord>> {
        let record = NoteRecord {
            scope: scope.to_owned(),
            body: body.to_owned(),
            revision: base_revision.saturating_add(1),
            updated_by: updated_by.to_owned(),
            updated_at: at,
        };
        self.call(move |c| {
            // One statement each, so the check and the write cannot interleave with another save.
            let changed = if base_revision == 0 {
                c.execute(
                    &format!(
                        "INSERT INTO notes ({COLS}) VALUES (?1, ?2, 1, ?3, ?4) \
                         ON CONFLICT(scope) DO NOTHING"
                    ),
                    params![
                        record.scope,
                        record.body,
                        record.updated_by,
                        record.updated_at.0
                    ],
                )
            } else {
                c.execute(
                    "UPDATE notes SET body = ?2, revision = revision + 1, updated_by = ?3, \
                     updated_at = ?4 WHERE scope = ?1 AND revision = ?5",
                    params![
                        record.scope,
                        record.body,
                        record.updated_by,
                        record.updated_at.0,
                        to_i64(base_revision)
                    ],
                )
            }
            .map_err(db)?;
            Ok((changed == 1).then_some(record))
        })
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    #[tokio::test]
    async fn a_save_needs_the_stored_revision() -> TestResult {
        let store = SqliteStore::open_in_memory()?;
        assert_eq!(store.note("team").await?, None);

        let first = store
            .put_note("team", "a", 0, "alice", Timestamp(5))
            .await?;
        assert_eq!(first.as_ref().map(|n| n.revision), Some(1));
        // A second "first write" lost the race: refused, the first stays.
        assert_eq!(
            store.put_note("team", "b", 0, "bob", Timestamp(6)).await?,
            None
        );
        assert_eq!(
            store.put_note("team", "b", 2, "bob", Timestamp(6)).await?,
            None
        );
        let second = store.put_note("team", "c", 1, "bob", Timestamp(7)).await?;
        assert_eq!(second.as_ref().map(|n| n.revision), Some(2));

        let stored = store.note("team").await?.ok_or("stored")?;
        assert_eq!((stored.body.as_str(), stored.revision), ("c", 2));
        assert_eq!(
            (stored.updated_by.as_str(), stored.updated_at),
            ("bob", Timestamp(7))
        );
        // Scopes are separate notepads.
        assert_eq!(store.note("sandbox:s1").await?, None);
        Ok(())
    }
}
