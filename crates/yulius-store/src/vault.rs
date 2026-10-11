//! Secrets vault storage (SPEC-002 ADR-20). Rows hold envelopes produced by the application;
//! nothing here sees a plaintext value.

use async_trait::async_trait;
use rusqlite::{OptionalExtension, Row, Transaction, params};
use yulius_app::error::{AppError, AppResult};
use yulius_app::ports::Envelope;
use yulius_app::ports::vault::{VaultAuditEntry, VaultSecretRecord, VaultSection, VaultStore};
use yulius_domain::ids::SandboxId;
use yulius_domain::time::Timestamp;

use crate::{SqliteStore, bad_column, db, to_i64, to_u32};

const COLS: &str = "id, section, sandbox_id, name, description, hosts, injection, header_name, \
     fields, kek_id, wrapped_key, ciphertext, created_by, created_at, updated_by, updated_at, \
     value_updated_at, last_used_at";

fn json_list(row: &Row<'_>, column: &str) -> rusqlite::Result<Vec<String>> {
    let text: String = row.get(column)?;
    serde_json::from_str(&text).map_err(|_| bad_column(column, &text))
}

fn from_row(row: &Row<'_>) -> rusqlite::Result<VaultSecretRecord> {
    let section: String = row.get("section")?;
    let sandbox: Option<String> = row.get("sandbox_id")?;
    Ok(VaultSecretRecord {
        id: row.get("id")?,
        section: VaultSection::parse(&section).ok_or_else(|| bad_column("section", &section))?,
        sandbox_id: sandbox
            .as_deref()
            .map(|s| SandboxId::parse(s).map_err(|_| bad_column("sandbox id", s)))
            .transpose()?,
        name: row.get("name")?,
        description: row.get("description")?,
        hosts: json_list(row, "hosts")?,
        injection: row.get("injection")?,
        header_name: row.get("header_name")?,
        fields: json_list(row, "fields")?,
        envelope: Envelope {
            kek_id: to_u32(row.get("kek_id")?),
            wrapped_key: row.get("wrapped_key")?,
            ciphertext: row.get("ciphertext")?,
        },
        created_by: row.get("created_by")?,
        created_at: Timestamp(row.get("created_at")?),
        updated_by: row.get("updated_by")?,
        updated_at: Timestamp(row.get("updated_at")?),
        value_updated_at: Timestamp(row.get("value_updated_at")?),
        last_used_at: row.get::<_, Option<i64>>("last_used_at")?.map(Timestamp),
    })
}

fn to_json(list: &[String]) -> String {
    serde_json::to_string(list).unwrap_or_else(|_| "[]".into())
}

/// An agent secret's name must be unique wherever it applies: a sandbox secret may not share a
/// name with an instance-wide one, and an instance-wide name may not exist in any sandbox.
fn ensure_agent_unique(tx: &Transaction<'_>, record: &VaultSecretRecord) -> AppResult<()> {
    if record.section != VaultSection::Agent {
        return Ok(());
    }
    let clash: Option<String> = match &record.sandbox_id {
        Some(sandbox) => tx
            .query_row(
                "SELECT id FROM vault_secrets WHERE section = 'agent' AND name = ?1 AND id <> ?2 \
                 AND (sandbox_id IS NULL OR sandbox_id = ?3) LIMIT 1",
                params![record.name, record.id, sandbox.as_str()],
                |r| r.get(0),
            )
            .optional()
            .map_err(db)?,
        None => tx
            .query_row(
                "SELECT id FROM vault_secrets WHERE section = 'agent' AND name = ?1 AND id <> ?2 LIMIT 1",
                params![record.name, record.id],
                |r| r.get(0),
            )
            .optional()
            .map_err(db)?,
    };
    match clash {
        Some(_) => Err(AppError::Conflict(format!(
            "an agent secret named {} already applies here",
            record.name
        ))),
        None => Ok(()),
    }
}

#[async_trait]
impl VaultStore for SqliteStore {
    async fn vault_insert(&self, record: &VaultSecretRecord) -> AppResult<()> {
        let r = record.clone();
        self.call(move |c| {
            let tx = c.transaction().map_err(db)?;
            ensure_agent_unique(&tx, &r)?;
            tx.execute(
                &format!(
                    "INSERT INTO vault_secrets ({COLS}) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, \
                     ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18)"
                ),
                params![
                    r.id,
                    r.section.as_str(),
                    r.sandbox_id.as_ref().map(SandboxId::as_str),
                    r.name,
                    r.description,
                    to_json(&r.hosts),
                    r.injection,
                    r.header_name,
                    to_json(&r.fields),
                    i64::from(r.envelope.kek_id),
                    r.envelope.wrapped_key,
                    r.envelope.ciphertext,
                    r.created_by,
                    r.created_at.0,
                    r.updated_by,
                    r.updated_at.0,
                    r.value_updated_at.0,
                    r.last_used_at.map(|t| t.0),
                ],
            )
            .map_err(db)?;
            tx.commit().map_err(db)?;
            Ok(())
        })
        .await
    }

    async fn vault_update(&self, record: &VaultSecretRecord) -> AppResult<bool> {
        let r = record.clone();
        self.call(move |c| {
            let tx = c.transaction().map_err(db)?;
            ensure_agent_unique(&tx, &r)?;
            let changed = tx
                .execute(
                    "UPDATE vault_secrets SET name = ?2, description = ?3, hosts = ?4, injection = ?5, \
                     header_name = ?6, fields = ?7, kek_id = ?8, wrapped_key = ?9, ciphertext = ?10, \
                     updated_by = ?11, updated_at = ?12, value_updated_at = ?13 WHERE id = ?1",
                    params![
                        r.id,
                        r.name,
                        r.description,
                        to_json(&r.hosts),
                        r.injection,
                        r.header_name,
                        to_json(&r.fields),
                        i64::from(r.envelope.kek_id),
                        r.envelope.wrapped_key,
                        r.envelope.ciphertext,
                        r.updated_by,
                        r.updated_at.0,
                        r.value_updated_at.0,
                    ],
                )
                .map_err(db)?;
            tx.commit().map_err(db)?;
            Ok(changed == 1)
        })
        .await
    }

    async fn vault_delete(&self, id: &str) -> AppResult<bool> {
        let id = id.to_owned();
        self.call(move |c| {
            let n = c
                .execute("DELETE FROM vault_secrets WHERE id = ?1", [id])
                .map_err(db)?;
            Ok(n == 1)
        })
        .await
    }

    async fn vault_get(&self, id: &str) -> AppResult<Option<VaultSecretRecord>> {
        let id = id.to_owned();
        self.call(move |c| {
            c.query_row(
                &format!("SELECT {COLS} FROM vault_secrets WHERE id = ?1"),
                [id],
                from_row,
            )
            .optional()
            .map_err(db)
        })
        .await
    }

    async fn vault_team(&self) -> AppResult<Vec<VaultSecretRecord>> {
        self.call(|c| {
            let mut stmt = c
                .prepare(&format!(
                    "SELECT {COLS} FROM vault_secrets WHERE section = 'team' ORDER BY name COLLATE NOCASE, created_at"
                ))
                .map_err(db)?;
            let rows = stmt.query_map([], from_row).map_err(db)?;
            rows.collect::<Result<Vec<_>, _>>().map_err(db)
        })
        .await
    }

    async fn vault_agent(&self, sandbox: Option<&SandboxId>) -> AppResult<Vec<VaultSecretRecord>> {
        let sandbox = sandbox.map(|s| s.as_str().to_owned());
        self.call(move |c| {
            let sql = match sandbox {
                Some(_) => format!(
                    "SELECT {COLS} FROM vault_secrets WHERE section = 'agent' \
                     AND (sandbox_id IS NULL OR sandbox_id = ?1) ORDER BY name"
                ),
                None => format!(
                    "SELECT {COLS} FROM vault_secrets WHERE section = 'agent' AND ?1 IS NULL ORDER BY name"
                ),
            };
            let mut stmt = c.prepare(&sql).map_err(db)?;
            let rows = stmt.query_map([sandbox], from_row).map_err(db)?;
            rows.collect::<Result<Vec<_>, _>>().map_err(db)
        })
        .await
    }

    async fn vault_touch_used(&self, id: &str, at: Timestamp) -> AppResult<()> {
        let id = id.to_owned();
        self.call(move |c| {
            c.execute(
                "UPDATE vault_secrets SET last_used_at = ?2 WHERE id = ?1",
                params![id, at.0],
            )
            .map_err(db)?;
            Ok(())
        })
        .await
    }

    async fn vault_set_reauth(&self, session_hash: &str, at: Timestamp) -> AppResult<()> {
        let hash = session_hash.to_owned();
        self.call(move |c| {
            c.execute(
                "INSERT INTO vault_reauth (session_hash, at) VALUES (?1, ?2) \
                 ON CONFLICT(session_hash) DO UPDATE SET at = excluded.at",
                params![hash, at.0],
            )
            .map_err(db)?;
            Ok(())
        })
        .await
    }

    async fn vault_reauth(&self, session_hash: &str) -> AppResult<Option<Timestamp>> {
        let hash = session_hash.to_owned();
        self.call(move |c| {
            c.query_row(
                "SELECT at FROM vault_reauth WHERE session_hash = ?1",
                [hash],
                |r| r.get::<_, i64>(0),
            )
            .optional()
            .map(|at| at.map(Timestamp))
            .map_err(db)
        })
        .await
    }

    async fn vault_meta(&self, name: &str) -> AppResult<Option<Envelope>> {
        let name = name.to_owned();
        self.call(move |c| {
            c.query_row(
                "SELECT kek_id, wrapped_key, ciphertext FROM vault_meta WHERE name = ?1",
                [name],
                |r| {
                    Ok(Envelope {
                        kek_id: to_u32(r.get(0)?),
                        wrapped_key: r.get(1)?,
                        ciphertext: r.get(2)?,
                    })
                },
            )
            .optional()
            .map_err(db)
        })
        .await
    }

    async fn vault_insert_meta(
        &self,
        name: &str,
        envelope: &Envelope,
        at: Timestamp,
    ) -> AppResult<bool> {
        let (name, envelope) = (name.to_owned(), envelope.clone());
        self.call(move |c| {
            let n = c
                .execute(
                    "INSERT OR IGNORE INTO vault_meta (name, kek_id, wrapped_key, ciphertext, created_at) \
                     VALUES (?1, ?2, ?3, ?4, ?5)",
                    params![
                        name,
                        i64::from(envelope.kek_id),
                        envelope.wrapped_key,
                        envelope.ciphertext,
                        at.0
                    ],
                )
                .map_err(db)?;
            Ok(n == 1)
        })
        .await
    }

    async fn vault_audit(&self, limit: u32) -> AppResult<Vec<VaultAuditEntry>> {
        self.call(move |c| {
            let mut stmt = c
                .prepare(
                    "SELECT at, actor, action, target, detail FROM audit_log \
                     WHERE action LIKE 'vault.%' ORDER BY id DESC LIMIT ?1",
                )
                .map_err(db)?;
            let rows = stmt
                .query_map([to_i64(u64::from(limit))], |r| {
                    Ok(VaultAuditEntry {
                        at: Timestamp(r.get(0)?),
                        actor: r.get(1)?,
                        action: r.get(2)?,
                        target: r.get(3)?,
                        detail: r.get(4)?,
                    })
                })
                .map_err(db)?;
            rows.collect::<Result<Vec<_>, _>>().map_err(db)
        })
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use yulius_app::ports::store::AuditStore as _;

    fn record(
        id: &str,
        section: VaultSection,
        sandbox: Option<&str>,
        name: &str,
    ) -> VaultSecretRecord {
        VaultSecretRecord {
            id: id.into(),
            section,
            sandbox_id: sandbox.and_then(|s| SandboxId::parse(s).ok()),
            name: name.into(),
            description: String::new(),
            hosts: vec!["api.example.com".into()],
            injection: "bearer".into(),
            header_name: None,
            fields: vec!["value".into()],
            envelope: Envelope {
                kek_id: 1,
                wrapped_key: vec![1, 2, 3],
                ciphertext: vec![4, 5, 6],
            },
            created_by: "u1".into(),
            created_at: Timestamp(1),
            updated_by: "u1".into(),
            updated_at: Timestamp(1),
            value_updated_at: Timestamp(1),
            last_used_at: None,
        }
    }

    async fn store_with_sandboxes() -> Result<SqliteStore, Box<dyn std::error::Error>> {
        let store = SqliteStore::open_in_memory()?;
        store
            .call(|c| {
                c.execute_batch(
                    "INSERT INTO users (id, display_name, is_superadmin, is_instance_admin, created_at) VALUES ('u1', 'U', 1, 1, 1);
                     INSERT INTO sandboxes (id, slug, repo_owner, repo_name, default_branch, profile, port_base, port_count, created_by, onboarding_state, created_at) VALUES ('s1', 'a', 'o', 'a', 'main', '{}', 20000, 10, 'u1', 'not_needed', 1);
                     INSERT INTO sandboxes (id, slug, repo_owner, repo_name, default_branch, profile, port_base, port_count, created_by, onboarding_state, created_at) VALUES ('s2', 'b', 'o', 'b', 'main', '{}', 20010, 10, 'u1', 'not_needed', 1);",
                )
                .map_err(db)
            })
            .await?;
        Ok(store)
    }

    #[tokio::test]
    async fn agent_names_are_unique_where_they_apply() -> Result<(), Box<dyn std::error::Error>> {
        let store = store_with_sandboxes().await?;
        store
            .vault_insert(&record("a", VaultSection::Agent, Some("s1"), "KEY"))
            .await?;
        // The same name in another sandbox is fine; instance-wide or in s1 again is not.
        store
            .vault_insert(&record("b", VaultSection::Agent, Some("s2"), "KEY"))
            .await?;
        assert!(matches!(
            store
                .vault_insert(&record("c", VaultSection::Agent, None, "KEY"))
                .await,
            Err(AppError::Conflict(_))
        ));
        assert!(matches!(
            store
                .vault_insert(&record("d", VaultSection::Agent, Some("s1"), "KEY"))
                .await,
            Err(AppError::Conflict(_))
        ));
        store
            .vault_insert(&record("e", VaultSection::Agent, None, "GLOBAL"))
            .await?;
        assert!(matches!(
            store
                .vault_insert(&record("f", VaultSection::Agent, Some("s2"), "GLOBAL"))
                .await,
            Err(AppError::Conflict(_))
        ));
        // Team titles may repeat.
        store
            .vault_insert(&record("t1", VaultSection::Team, None, "KEY"))
            .await?;
        store
            .vault_insert(&record("t2", VaultSection::Team, None, "KEY"))
            .await?;
        let s1 = SandboxId::parse("s1")?;
        let names: Vec<String> = store
            .vault_agent(Some(&s1))
            .await?
            .into_iter()
            .map(|r| r.id)
            .collect();
        assert_eq!(names, vec!["e".to_owned(), "a".to_owned()]);
        assert_eq!(store.vault_agent(None).await?.len(), 3);
        assert_eq!(store.vault_team().await?.len(), 2);
        Ok(())
    }

    #[tokio::test]
    async fn updates_meta_reauth_and_audit() -> Result<(), Box<dyn std::error::Error>> {
        let store = store_with_sandboxes().await?;
        let mut r = record("a", VaultSection::Agent, Some("s1"), "KEY");
        store.vault_insert(&r).await?;
        r.name = "RENAMED".into();
        r.envelope.ciphertext = vec![9];
        r.value_updated_at = Timestamp(5);
        assert!(store.vault_update(&r).await?);
        store.vault_touch_used("a", Timestamp(7)).await?;
        let got = store.vault_get("a").await?.ok_or("missing")?;
        assert_eq!(got.name, "RENAMED");
        assert_eq!(got.envelope.ciphertext, vec![9]);
        assert_eq!(got.last_used_at, Some(Timestamp(7)));
        assert!(store.vault_delete("a").await?);
        assert!(!store.vault_delete("a").await?);
        assert!(store.vault_get("a").await?.is_none());

        assert_eq!(store.vault_reauth("h").await?, None);
        store.vault_set_reauth("h", Timestamp(3)).await?;
        store.vault_set_reauth("h", Timestamp(4)).await?;
        assert_eq!(store.vault_reauth("h").await?, Some(Timestamp(4)));

        let env = Envelope {
            kek_id: 1,
            wrapped_key: vec![1],
            ciphertext: vec![2],
        };
        assert!(store.vault_insert_meta("ca", &env, Timestamp(1)).await?);
        assert!(!store.vault_insert_meta("ca", &env, Timestamp(2)).await?);
        assert_eq!(store.vault_meta("ca").await?, Some(env));

        store
            .audit(Timestamp(1), "u", "login.github", None, None)
            .await?;
        store
            .audit(
                Timestamp(2),
                "u",
                "vault.team.reveal",
                Some("t1"),
                Some("Gmail"),
            )
            .await?;
        let audit = store.vault_audit(10).await?;
        assert_eq!(audit.len(), 1);
        assert_eq!(audit[0].action, "vault.team.reveal");
        Ok(())
    }

    #[tokio::test]
    async fn removing_a_sandbox_removes_its_agent_secrets() -> Result<(), Box<dyn std::error::Error>>
    {
        let store = store_with_sandboxes().await?;
        store
            .vault_insert(&record("a", VaultSection::Agent, Some("s2"), "KEY"))
            .await?;
        store
            .call(|c| {
                c.execute("DELETE FROM sandboxes WHERE id = 's2'", [])
                    .map_err(db)
            })
            .await?;
        assert!(store.vault_get("a").await?.is_none());
        Ok(())
    }
}
