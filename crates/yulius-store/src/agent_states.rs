//! Agent states (SPEC-271, ADR-0200): a project's folders of operator state, one sealed archive
//! per sandbox and name. Rows hold envelopes produced by the application; nothing here sees an
//! archive's content.

use async_trait::async_trait;
use rusqlite::{OptionalExtension, Row, params};
use yulius_app::error::AppResult;
use yulius_app::ports::Envelope;
use yulius_app::ports::vault::{AgentStateRecord, AgentStateStore, StateRestore};
use yulius_domain::ids::SandboxId;
use yulius_domain::time::Timestamp;

use crate::{SqliteStore, bad_column, db, to_i64, to_u32};

const COLS: &str = "id, sandbox_id, name, description, version, bytes, files, created_by, \
     created_at, updated_by, updated_at, restored_path, restored_version, restored_by, restored_at";

fn from_row(row: &Row<'_>) -> rusqlite::Result<AgentStateRecord> {
    let sandbox: String = row.get("sandbox_id")?;
    let restored = match row.get::<_, Option<String>>("restored_path")? {
        Some(path) => Some(StateRestore {
            path,
            version: to_u32(row.get::<_, Option<i64>>("restored_version")?.unwrap_or(0)),
            by: row
                .get::<_, Option<String>>("restored_by")?
                .unwrap_or_default(),
            at: Timestamp(row.get::<_, Option<i64>>("restored_at")?.unwrap_or(0)),
        }),
        None => None,
    };
    Ok(AgentStateRecord {
        id: row.get("id")?,
        sandbox_id: SandboxId::parse(&sandbox).map_err(|_| bad_column("sandbox id", &sandbox))?,
        name: row.get("name")?,
        description: row.get("description")?,
        version: to_u32(row.get("version")?),
        bytes: u64::try_from(row.get::<_, i64>("bytes")?).unwrap_or(0),
        files: u64::try_from(row.get::<_, i64>("files")?).unwrap_or(0),
        created_by: row.get("created_by")?,
        created_at: Timestamp(row.get("created_at")?),
        updated_by: row.get("updated_by")?,
        updated_at: Timestamp(row.get("updated_at")?),
        restored,
    })
}

#[async_trait]
impl AgentStateStore for SqliteStore {
    async fn agent_states(&self, sandbox: &SandboxId) -> AppResult<Vec<AgentStateRecord>> {
        let sandbox = sandbox.as_str().to_owned();
        self.call(move |c| {
            let mut stmt = c
                .prepare(&format!(
                    "SELECT {COLS} FROM agent_states WHERE sandbox_id = ?1 ORDER BY name"
                ))
                .map_err(db)?;
            let rows = stmt.query_map([sandbox], from_row).map_err(db)?;
            rows.collect::<Result<Vec<_>, _>>().map_err(db)
        })
        .await
    }

    async fn agent_state(
        &self,
        sandbox: &SandboxId,
        name: &str,
    ) -> AppResult<Option<(AgentStateRecord, Envelope)>> {
        let (sandbox, name) = (sandbox.as_str().to_owned(), name.to_owned());
        self.call(move |c| {
            c.query_row(
                &format!(
                    "SELECT {COLS}, kek_id, wrapped_key, ciphertext FROM agent_states \
                     WHERE sandbox_id = ?1 AND name = ?2"
                ),
                [sandbox, name],
                |row| {
                    Ok((
                        from_row(row)?,
                        Envelope {
                            kek_id: to_u32(row.get("kek_id")?),
                            wrapped_key: row.get("wrapped_key")?,
                            ciphertext: row.get("ciphertext")?,
                        },
                    ))
                },
            )
            .optional()
            .map_err(db)
        })
        .await
    }

    async fn agent_state_put(
        &self,
        record: &AgentStateRecord,
        archive: Envelope,
        expect: Option<&AgentStateRecord>,
    ) -> AppResult<bool> {
        let r = record.clone();
        let expect = expect.map(Expected::of);
        self.call(move |c| {
            let restored = r.restored.as_ref();
            let (path, restored_version, by, at) = (
                restored.map(|s| s.path.clone()),
                restored.map(|s| i64::from(s.version)),
                restored.map(|s| s.by.clone()),
                restored.map(|s| s.at.0),
            );
            let n = match expect {
                None => c
                    .execute(
                        "INSERT OR IGNORE INTO agent_states (id, sandbox_id, name, description, \
                         version, bytes, files, kek_id, wrapped_key, ciphertext, created_by, \
                         created_at, updated_by, updated_at, restored_path, restored_version, \
                         restored_by, restored_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, \
                         ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18)",
                        params![
                            r.id,
                            r.sandbox_id.as_str(),
                            r.name,
                            r.description,
                            i64::from(r.version),
                            to_i64(r.bytes),
                            to_i64(r.files),
                            i64::from(archive.kek_id),
                            archive.wrapped_key,
                            archive.ciphertext,
                            r.created_by,
                            r.created_at.0,
                            r.updated_by,
                            r.updated_at.0,
                            path,
                            restored_version,
                            by,
                            at,
                        ],
                    )
                    .map_err(db)?,
                Some(expect) => c
                    .execute(
                        "UPDATE agent_states SET description = ?2, version = ?3, bytes = ?4, \
                         files = ?5, kek_id = ?6, wrapped_key = ?7, ciphertext = ?8, \
                         updated_by = ?9, updated_at = ?10, restored_path = ?11, \
                         restored_version = ?12, restored_by = ?13, restored_at = ?14 \
                         WHERE id = ?1 AND version = ?15 AND restored_path IS ?16 \
                         AND restored_version IS ?17",
                        params![
                            r.id,
                            r.description,
                            i64::from(r.version),
                            to_i64(r.bytes),
                            to_i64(r.files),
                            i64::from(archive.kek_id),
                            archive.wrapped_key,
                            archive.ciphertext,
                            r.updated_by,
                            r.updated_at.0,
                            path,
                            restored_version,
                            by,
                            at,
                            expect.version,
                            expect.path,
                            expect.restored_version,
                        ],
                    )
                    .map_err(db)?,
            };
            Ok(n == 1)
        })
        .await
    }

    async fn agent_state_restored(
        &self,
        expect: &AgentStateRecord,
        restore: &StateRestore,
    ) -> AppResult<bool> {
        let (id, expect, restore) = (expect.id.clone(), Expected::of(expect), restore.clone());
        self.call(move |c| {
            let n = c
                .execute(
                    "UPDATE agent_states SET restored_path = ?1, restored_version = ?2, \
                     restored_by = ?3, restored_at = ?4 WHERE id = ?5 AND version = ?6 \
                     AND restored_path IS ?7 AND restored_version IS ?8",
                    params![
                        restore.path,
                        i64::from(restore.version),
                        restore.by,
                        restore.at.0,
                        id,
                        expect.version,
                        expect.path,
                        expect.restored_version,
                    ],
                )
                .map_err(db)?;
            Ok(n == 1)
        })
        .await
    }
}

/// What a compare-and-set compares: the version and the restored copy as read.
struct Expected {
    version: i64,
    path: Option<String>,
    restored_version: Option<i64>,
}

impl Expected {
    fn of(record: &AgentStateRecord) -> Self {
        Self {
            version: i64::from(record.version),
            path: record.restored.as_ref().map(|r| r.path.clone()),
            restored_version: record.restored.as_ref().map(|r| i64::from(r.version)),
        }
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
                    "INSERT INTO users (id, display_name, is_superadmin, is_instance_admin, created_at) VALUES ('u1', 'U', 1, 1, 1);
                     INSERT INTO sandboxes (id, slug, repo_owner, repo_name, default_branch, profile, port_base, port_count, created_by, onboarding_state, created_at) VALUES ('s1', 'a', 'o', 'a', 'main', '{}', 20000, 10, 'u1', 'not_needed', 1);",
                )
                .map_err(db)
            })
            .await?;
        Ok(store)
    }

    fn record(version: u32) -> Result<AgentStateRecord, Box<dyn std::error::Error>> {
        Ok(AgentStateRecord {
            id: "st1".into(),
            sandbox_id: SandboxId::parse("s1")?,
            name: "OPS".into(),
            description: "operator state".into(),
            version,
            bytes: 10,
            files: 2,
            created_by: "remote:ann".into(),
            created_at: Timestamp(1),
            updated_by: "remote:ann".into(),
            updated_at: Timestamp(version.into()),
            restored: None,
        })
    }

    fn sealed(byte: u8) -> Envelope {
        Envelope {
            kek_id: 1,
            wrapped_key: vec![byte],
            ciphertext: vec![byte; 4],
        }
    }

    /// SPEC-271 OPS-001, OPS-003, OPS-006: a name is taken once; later writes and restores apply
    /// only to the version they read, so two writers never both win.
    #[tokio::test]
    async fn writes_and_restores_apply_only_to_the_version_read() -> TestResult {
        let store = store().await?;
        let sandbox = SandboxId::parse("s1")?;
        assert!(store.agent_state_put(&record(1)?, sealed(1), None).await?);
        assert!(!store.agent_state_put(&record(1)?, sealed(9), None).await?);

        let restore = StateRestore {
            path: "~/.ops".into(),
            version: 1,
            by: "task-1".into(),
            at: Timestamp(5),
        };
        let stale = record(2)?;
        assert!(!store.agent_state_restored(&stale, &restore).await?);
        let read = record(1)?;
        assert!(store.agent_state_restored(&read, &restore).await?);
        let restored = AgentStateRecord {
            restored: Some(restore.clone()),
            ..read.clone()
        };

        let mut next = record(2)?;
        next.restored = Some(StateRestore {
            version: 2,
            ..restore.clone()
        });
        // Read before the restore: lost.
        assert!(!store.agent_state_put(&next, sealed(2), Some(&read)).await?);
        assert!(
            store
                .agent_state_put(&next, sealed(2), Some(&restored))
                .await?
        );
        // The one that read version 1 too lost.
        assert!(
            !store
                .agent_state_put(&next, sealed(3), Some(&restored))
                .await?
        );

        let (found, archive) = store.agent_state(&sandbox, "OPS").await?.ok_or("missing")?;
        assert_eq!(found, next);
        assert_eq!(archive, sealed(2));
        assert_eq!(store.agent_states(&sandbox).await?, vec![next]);
        assert!(store.agent_state(&sandbox, "OTHER").await?.is_none());
        Ok(())
    }

    /// Review of SPEC-271, M1: a write or a restore applies only to the record as it was read,
    /// restore included, so an import racing a restore, or two restores, never both win.
    #[tokio::test]
    async fn a_restore_between_read_and_write_fails_the_write() -> TestResult {
        let store = store().await?;
        let read = record(1)?;
        assert!(store.agent_state_put(&read, sealed(1), None).await?);
        let restore = |path: &str| StateRestore {
            path: path.into(),
            version: 1,
            by: "task-1".into(),
            at: Timestamp(5),
        };
        // Two tasks read it unrestored; the first restore wins, the second loses.
        assert!(store.agent_state_restored(&read, &restore("~/.a")).await?);
        assert!(!store.agent_state_restored(&read, &restore("~/.b")).await?);
        // An import that read it unrestored cannot overwrite the restore.
        assert!(
            !store
                .agent_state_put(&record(2)?, sealed(2), Some(&read))
                .await?
        );
        let (now, _) = store
            .agent_state(&SandboxId::parse("s1")?, "OPS")
            .await?
            .ok_or("missing")?;
        assert_eq!(now.restored, Some(restore("~/.a")));
        assert_eq!(now.version, 1);
        Ok(())
    }

    /// SPEC-271 OPS-001: a state goes with its sandbox.
    #[tokio::test]
    async fn removing_a_sandbox_removes_its_states() -> TestResult {
        let store = store().await?;
        assert!(store.agent_state_put(&record(1)?, sealed(1), None).await?);
        store
            .call(|c| {
                c.execute("DELETE FROM sandboxes WHERE id = 's1'", [])
                    .map_err(db)
            })
            .await?;
        assert!(
            store
                .agent_states(&SandboxId::parse("s1")?)
                .await?
                .is_empty()
        );
        Ok(())
    }
}
