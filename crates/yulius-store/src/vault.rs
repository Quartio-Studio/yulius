//! Secrets vault storage (SPEC-002 ADR-20). Rows hold envelopes produced by the application;
//! nothing here sees a plaintext value.

use async_trait::async_trait;
use rusqlite::{OptionalExtension, Row, Transaction, params};
use yulius_app::error::{AppError, AppResult};
use yulius_app::ports::Envelope;
use yulius_app::ports::vault::{
    EnvFiles, TwoFactorRecord, VaultAuditEntry, VaultSecretRecord, VaultSection, VaultStore,
};
use yulius_domain::ids::SandboxId;
use yulius_domain::time::Timestamp;

use crate::{SqliteStore, bad_column, db, to_i64, to_u32};

const COLS: &str = "id, section, sandbox_id, name, description, hosts, injection, header_name, \
     fields, kek_id, wrapped_key, ciphertext, created_by, created_at, updated_by, updated_at, \
     value_updated_at, last_used_at, requested_hosts, requested_by, env_files, env_requested_by, \
     auto_allow";

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
        requested_hosts: json_list(row, "requested_hosts")?,
        requested_by: row.get("requested_by")?,
        env_files: {
            let text: String = row.get("env_files")?;
            EnvFiles::parse(&text).ok_or_else(|| bad_column("env_files", &text))?
        },
        env_requested_by: row.get("env_requested_by")?,
        cd_prod: row.get::<_, i64>("cd_prod")? != 0,
        auto_allow: row.get::<_, i64>("auto_allow")? != 0,
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

/// Inserts a row, keeping agent names unique where they apply.
fn insert_row(tx: &Transaction<'_>, r: &VaultSecretRecord) -> AppResult<()> {
    ensure_agent_unique(tx, r)?;
    tx.execute(
        &format!(
            "INSERT INTO vault_secrets ({COLS}, cd_prod) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, \
             ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24)"
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
            to_json(&r.requested_hosts),
            r.requested_by,
            r.env_files.as_str(),
            r.env_requested_by,
            i64::from(r.auto_allow),
            i64::from(r.cd_prod),
        ],
    )
    .map_err(db)?;
    Ok(())
}

#[async_trait]
impl VaultStore for SqliteStore {
    async fn vault_insert(&self, record: &VaultSecretRecord) -> AppResult<()> {
        let r = record.clone();
        self.call(move |c| {
            let tx = c.transaction().map_err(db)?;
            insert_row(&tx, &r)?;
            tx.commit().map_err(db)?;
            Ok(())
        })
        .await
    }

    async fn vault_update(
        &self,
        record: &VaultSecretRecord,
        seen: &VaultSecretRecord,
    ) -> AppResult<bool> {
        let r = record.clone();
        let (seen_hosts, seen_auto) = (to_json(&seen.hosts), i64::from(seen.auto_allow));
        self.call(move |c| {
            let tx = c.transaction().map_err(db)?;
            ensure_agent_unique(&tx, &r)?;
            let changed = tx
                .execute(
                    "UPDATE vault_secrets SET name = ?2, description = ?3, hosts = ?4, injection = ?5, \
                     header_name = ?6, fields = ?7, kek_id = ?8, wrapped_key = ?9, ciphertext = ?10, \
                     updated_by = ?11, updated_at = ?12, value_updated_at = ?13, auto_allow = ?14 \
                     WHERE id = ?1 AND NOT (?14 = 1 AND cd_prod = 1) AND hosts = ?15 \
                     AND auto_allow = ?16",
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
                        i64::from(r.auto_allow),
                        seen_hosts,
                        seen_auto,
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
                &format!("SELECT {COLS}, cd_prod FROM vault_secrets WHERE id = ?1"),
                [id],
                from_row,
            )
            .optional()
            .map_err(db)
        })
        .await
    }

    async fn vault_team(&self, sandbox: Option<&SandboxId>) -> AppResult<Vec<VaultSecretRecord>> {
        let sandbox = sandbox.map(|s| s.as_str().to_owned());
        self.call(move |c| {
            let mut stmt = c
                .prepare(&format!(
                    "SELECT {COLS}, cd_prod FROM vault_secrets WHERE section = 'team' \
                     AND (?1 IS NULL OR sandbox_id IS NULL OR sandbox_id = ?1) \
                     ORDER BY name COLLATE NOCASE, created_at"
                ))
                .map_err(db)?;
            let rows = stmt.query_map([sandbox], from_row).map_err(db)?;
            rows.collect::<Result<Vec<_>, _>>().map_err(db)
        })
        .await
    }

    async fn vault_agent(&self, sandbox: Option<&SandboxId>) -> AppResult<Vec<VaultSecretRecord>> {
        let sandbox = sandbox.map(|s| s.as_str().to_owned());
        self.call(move |c| {
            let sql = match sandbox {
                Some(_) => format!(
                    "SELECT {COLS}, cd_prod FROM vault_secrets WHERE section = 'agent' \
                     AND (sandbox_id IS NULL OR sandbox_id = ?1) ORDER BY name"
                ),
                None => format!(
                    "SELECT {COLS}, cd_prod FROM vault_secrets WHERE section = 'agent' AND ?1 IS NULL ORDER BY name"
                ),
            };
            let mut stmt = c.prepare(&sql).map_err(db)?;
            let rows = stmt.query_map([sandbox], from_row).map_err(db)?;
            rows.collect::<Result<Vec<_>, _>>().map_err(db)
        })
        .await
    }

    async fn vault_set_request(
        &self,
        id: &str,
        from: &[String],
        to: &[String],
        by: Option<&str>,
    ) -> AppResult<bool> {
        let (id, from, to, by) = (
            id.to_owned(),
            to_json(from),
            to_json(to),
            by.map(str::to_owned),
        );
        self.call(move |c| {
            let changed = c
                .execute(
                    "UPDATE vault_secrets SET requested_hosts = ?3, requested_by = ?4 \
                     WHERE id = ?1 AND section = 'agent' AND requested_hosts = ?2",
                    params![id, from, to, by],
                )
                .map_err(db)?;
            Ok(changed == 1)
        })
        .await
    }

    async fn vault_answer_request(
        &self,
        id: &str,
        request: &[String],
        bound: &[String],
        hosts: &[String],
        by: &str,
        at: Timestamp,
    ) -> AppResult<bool> {
        let (id, request, bound, hosts, by) = (
            id.to_owned(),
            to_json(request),
            to_json(bound),
            to_json(hosts),
            by.to_owned(),
        );
        self.call(move |c| {
            let changed = c
                .execute(
                    "UPDATE vault_secrets SET hosts = ?4, requested_hosts = '[]', requested_by = NULL, \
                     updated_by = ?5, updated_at = ?6 \
                     WHERE id = ?1 AND section = 'agent' AND requested_hosts = ?2 AND hosts = ?3",
                    params![id, request, bound, hosts, by, at.0],
                )
                .map_err(db)?;
            Ok(changed == 1)
        })
        .await
    }

    async fn vault_auto_bind(
        &self,
        id: &str,
        (bound, hosts): (&[String], &[String]),
        (waiting, requested): (&[String], &[String]),
    ) -> AppResult<bool> {
        let (id, bound, hosts, waiting, requested) = (
            id.to_owned(),
            to_json(bound),
            to_json(hosts),
            to_json(waiting),
            to_json(requested),
        );
        self.call(move |c| {
            let changed = c
                .execute(
                    "UPDATE vault_secrets SET hosts = ?3, requested_hosts = ?5, \
                     requested_by = CASE WHEN ?5 = '[]' THEN NULL ELSE requested_by END \
                     WHERE id = ?1 AND section = 'agent' AND auto_allow = 1 AND hosts = ?2 \
                     AND requested_hosts = ?4",
                    params![id, bound, hosts, waiting, requested],
                )
                .map_err(db)?;
            Ok(changed == 1)
        })
        .await
    }

    async fn vault_set_env_files(
        &self,
        id: &str,
        from: (EnvFiles, Option<&str>),
        to: (EnvFiles, Option<&str>),
    ) -> AppResult<bool> {
        let (id, from_state, from_by, to_state, to_by) = (
            id.to_owned(),
            from.0.as_str(),
            from.1.map(str::to_owned),
            to.0.as_str(),
            to.1.map(str::to_owned),
        );
        self.call(move |c| {
            let changed = c
                .execute(
                    "UPDATE vault_secrets SET env_files = ?4, env_requested_by = ?5 \
                     WHERE id = ?1 AND section = 'agent' AND env_files = ?2 \
                     AND env_requested_by IS ?3",
                    params![id, from_state, from_by, to_state, to_by],
                )
                .map_err(db)?;
            Ok(changed == 1)
        })
        .await
    }

    async fn vault_set_cd_prod(&self, id: &str, from: bool, to: bool) -> AppResult<bool> {
        let id = id.to_owned();
        self.call(move |c| {
            let changed = c
                .execute(
                    "UPDATE vault_secrets SET cd_prod = ?3 \
                     WHERE id = ?1 AND section = 'agent' AND cd_prod = ?2 AND NOT (?3 = 1 AND auto_allow = 1)",
                    params![id, i64::from(from), i64::from(to)],
                )
                .map_err(db)?;
            Ok(changed == 1)
        })
        .await
    }

    async fn vault_scope_pending(&self) -> AppResult<Vec<String>> {
        self.call(|c| {
            let mut stmt = c
                .prepare("SELECT id FROM vault_scope_pending ORDER BY id")
                .map_err(db)?;
            let rows = stmt.query_map([], |r| r.get(0)).map_err(db)?;
            rows.collect::<Result<Vec<String>, _>>().map_err(db)
        })
        .await
    }

    async fn vault_assign(&self, id: &str, copies: &[VaultSecretRecord]) -> AppResult<bool> {
        let (id, copies) = (id.to_owned(), copies.to_vec());
        self.call(move |c| {
            let tx = c.transaction().map_err(db)?;
            let pending = tx
                .execute("DELETE FROM vault_scope_pending WHERE id = ?1", [&id])
                .map_err(db)?
                == 1;
            let everywhere = tx
                .query_row(
                    "SELECT 1 FROM vault_secrets WHERE id = ?1 AND sandbox_id IS NULL",
                    [&id],
                    |_| Ok(()),
                )
                .optional()
                .map_err(db)?
                .is_some();
            if !(pending && everywhere) {
                tx.commit().map_err(db)?;
                return Ok(false);
            }
            if !copies.is_empty() {
                tx.execute("DELETE FROM vault_secrets WHERE id = ?1", [&id])
                    .map_err(db)?;
                for copy in &copies {
                    insert_row(&tx, copy)?;
                }
            }
            tx.commit().map_err(db)?;
            Ok(true)
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

    async fn vault_two_factor(&self, user_id: &str) -> AppResult<Option<TwoFactorRecord>> {
        let user = user_id.to_owned();
        self.call(move |c| {
            c.query_row(
                "SELECT user_id, kek_id, wrapped_key, ciphertext, created_at, confirmed_at, \
                 last_counter, failed_attempts, failed_at FROM vault_two_factor WHERE user_id = ?1",
                [user],
                |r| {
                    Ok(TwoFactorRecord {
                        user_id: r.get(0)?,
                        envelope: Envelope {
                            kek_id: to_u32(r.get(1)?),
                            wrapped_key: r.get(2)?,
                            ciphertext: r.get(3)?,
                        },
                        created_at: Timestamp(r.get(4)?),
                        confirmed_at: r.get::<_, Option<i64>>(5)?.map(Timestamp),
                        last_counter: r
                            .get::<_, Option<i64>>(6)?
                            .map(|n| u64::try_from(n).unwrap_or(0)),
                        failed_attempts: to_u32(r.get(7)?),
                        failed_at: r.get::<_, Option<i64>>(8)?.map(Timestamp),
                    })
                },
            )
            .optional()
            .map_err(db)
        })
        .await
    }

    async fn vault_put_two_factor(&self, record: &TwoFactorRecord) -> AppResult<bool> {
        let r = record.clone();
        self.call(move |c| {
            let n = c
                .execute(
                    "INSERT INTO vault_two_factor (user_id, kek_id, wrapped_key, ciphertext, \
                     created_at, confirmed_at, last_counter) VALUES (?1, ?2, ?3, ?4, ?5, NULL, NULL) \
                     ON CONFLICT(user_id) DO UPDATE SET kek_id = excluded.kek_id, \
                     wrapped_key = excluded.wrapped_key, ciphertext = excluded.ciphertext, \
                     created_at = excluded.created_at, last_counter = NULL, \
                     failed_attempts = 0, failed_at = NULL \
                     WHERE vault_two_factor.confirmed_at IS NULL",
                    params![
                        r.user_id,
                        i64::from(r.envelope.kek_id),
                        r.envelope.wrapped_key,
                        r.envelope.ciphertext,
                        r.created_at.0,
                    ],
                )
                .map_err(db)?;
            Ok(n == 1)
        })
        .await
    }

    async fn vault_accept_two_factor(
        &self,
        user_id: &str,
        ciphertext: &[u8],
        counter: u64,
        at: Timestamp,
    ) -> AppResult<bool> {
        let user = user_id.to_owned();
        let seed = ciphertext.to_vec();
        let counter = to_i64(counter);
        self.call(move |c| {
            let n = c
                .execute(
                    "UPDATE vault_two_factor SET last_counter = ?2, \
                     confirmed_at = COALESCE(confirmed_at, ?3), failed_attempts = 0, failed_at = NULL \
                     WHERE user_id = ?1 AND ciphertext = ?4 \
                     AND (last_counter IS NULL OR last_counter < ?2)",
                    params![user, counter, at.0, seed],
                )
                .map_err(db)?;
            Ok(n == 1)
        })
        .await
    }

    async fn vault_two_factor_failed(&self, user_id: &str, at: Timestamp) -> AppResult<u32> {
        let user = user_id.to_owned();
        self.call(move |c| {
            c.query_row(
                "UPDATE vault_two_factor SET failed_attempts = failed_attempts + 1, failed_at = ?2 \
                 WHERE user_id = ?1 RETURNING failed_attempts",
                params![user, at.0],
                |r| r.get::<_, i64>(0),
            )
            .optional()
            .map(|n| n.map_or(0, to_u32))
            .map_err(db)
        })
        .await
    }

    async fn vault_reset_two_factor(&self, user_id: &str) -> AppResult<bool> {
        let user = user_id.to_owned();
        self.call(move |c| {
            let tx = c.transaction().map_err(db)?;
            tx.execute(
                "DELETE FROM vault_unlock WHERE session_hash IN \
                 (SELECT id_hash FROM web_sessions WHERE user_id = ?1)",
                [&user],
            )
            .map_err(db)?;
            let removed = tx
                .execute("DELETE FROM vault_two_factor WHERE user_id = ?1", [&user])
                .map_err(db)?;
            tx.commit().map_err(db)?;
            Ok(removed == 1)
        })
        .await
    }

    async fn vault_two_factor_clear_failures(&self, user_id: &str) -> AppResult<()> {
        let user = user_id.to_owned();
        self.call(move |c| {
            c.execute(
                "UPDATE vault_two_factor SET failed_attempts = 0, failed_at = NULL WHERE user_id = ?1",
                [user],
            )
            .map_err(db)?;
            Ok(())
        })
        .await
    }

    async fn vault_set_unlock(&self, session_hash: &str, at: Timestamp) -> AppResult<()> {
        let hash = session_hash.to_owned();
        self.call(move |c| {
            c.execute(
                "INSERT INTO vault_unlock (session_hash, at) VALUES (?1, ?2) \
                 ON CONFLICT(session_hash) DO UPDATE SET at = excluded.at",
                params![hash, at.0],
            )
            .map_err(db)?;
            Ok(())
        })
        .await
    }

    async fn vault_unlock(&self, session_hash: &str) -> AppResult<Option<Timestamp>> {
        let hash = session_hash.to_owned();
        self.call(move |c| {
            c.query_row(
                "SELECT at FROM vault_unlock WHERE session_hash = ?1",
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
            requested_hosts: Vec::new(),
            requested_by: None,
            env_files: EnvFiles::No,
            env_requested_by: None,
            cd_prod: false,
            auto_allow: false,
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

    /// SPEC-216 SCO-004: the upgrade to version 35 lists the team secrets (all of every sandbox
    /// then), and turns agents setting a secret up off for every existing one.
    #[tokio::test]
    async fn the_upgrade_lists_the_team_secrets() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("v34.db");
        {
            let conn = rusqlite::Connection::open(&path)?;
            for sql in &crate::MIGRATIONS[..34] {
                conn.execute_batch(sql)?;
            }
            conn.pragma_update(None, "user_version", 34)?;
            conn.execute_batch(
                "INSERT INTO users (id, display_name, is_superadmin, is_instance_admin, created_at) VALUES ('u1', 'U', 0, 0, 1);
                 INSERT INTO sandboxes (id, slug, repo_owner, repo_name, default_branch, profile, port_base, port_count, created_by, onboarding_state, created_at) VALUES ('s1', 's', 'o', 'r', 'main', '{}', 20000, 10, 'u1', 'not_needed', 1);
                 INSERT INTO vault_secrets (id, section, sandbox_id, name, kek_id, wrapped_key, ciphertext, created_by, created_at, updated_by, updated_at, value_updated_at) VALUES
                   ('team', 'team', NULL, 'Gmail', 1, x'00', x'00', 'u1', 1, 'u1', 1, 1),
                   ('wide', 'agent', NULL, 'WIDE', 1, x'00', x'00', 'u1', 1, 'u1', 1, 1),
                   ('own', 'agent', 's1', 'OWN', 1, x'00', x'00', 'u1', 1, 'u1', 1, 1);",
            )?;
        }
        let store = SqliteStore::open(&path)?;
        // Review round 3: an administrator's team-wide agent secret stays team-wide; copies would
        // let every sandbox's writers reveal and rebind it.
        assert_eq!(store.vault_scope_pending().await?, ["team"]);
        assert!(!store.vault_get("own").await?.ok_or("own")?.auto_allow);
        Ok(())
    }

    /// SPEC-216 SCO-004: a pending secret is replaced by its copies in one step, once; with no
    /// copies it stays team-wide and only leaves the list; one that no longer applies to every
    /// sandbox is left alone.
    #[tokio::test]
    async fn a_pending_secret_is_replaced_by_its_copies_once()
    -> Result<(), Box<dyn std::error::Error>> {
        let store = store_with_sandboxes().await?;
        for r in [
            record("g", VaultSection::Agent, None, "GLOBAL"),
            record("t", VaultSection::Team, None, "Gmail"),
            record("k", VaultSection::Team, None, "Kept"),
            record("m", VaultSection::Team, Some("s1"), "Moved"),
        ] {
            store.vault_insert(&r).await?;
        }
        store
            .call(|c| {
                c.execute_batch(
                    "INSERT INTO vault_scope_pending (id) VALUES ('g'), ('t'), ('k'), ('m'), ('gone')",
                )
                .map_err(db)
            })
            .await?;
        let mut pending = store.vault_scope_pending().await?;
        pending.sort();
        assert_eq!(pending, ["g", "gone", "k", "m", "t"]);

        let copies = [
            record("g1", VaultSection::Agent, Some("s1"), "GLOBAL"),
            record("g2", VaultSection::Agent, Some("s2"), "GLOBAL"),
        ];
        assert!(store.vault_assign("g", &copies).await?);
        assert!(store.vault_get("g").await?.is_none());
        assert_eq!(
            store.vault_agent(Some(&SandboxId::parse("s2")?)).await?[0].id,
            "g2"
        );
        assert!(!store.vault_assign("g", &copies).await?, "only once");
        assert!(store.vault_assign("k", &[]).await?);
        assert!(store.vault_get("k").await?.is_some(), "kept team-wide");
        assert!(
            !store
                .vault_assign(
                    "m",
                    &[record("m2", VaultSection::Team, Some("s2"), "Moved")]
                )
                .await?,
            "no longer applies to every sandbox"
        );
        assert!(store.vault_get("m2").await?.is_none());
        assert!(!store.vault_assign("gone", &[]).await?);
        assert_eq!(store.vault_scope_pending().await?, ["t"]);
        Ok(())
    }

    #[tokio::test]
    async fn env_copies_change_only_from_what_was_seen() -> Result<(), Box<dyn std::error::Error>> {
        let store = store_with_sandboxes().await?;
        store
            .vault_insert(&record("a", VaultSection::Agent, Some("s1"), "KEY"))
            .await?;
        let no = (EnvFiles::No, None);
        assert!(
            store
                .vault_set_env_files("a", no, (EnvFiles::Asked, Some("#1")))
                .await?
        );
        assert!(
            !store
                .vault_set_env_files("a", no, (EnvFiles::Asked, Some("#2")))
                .await?,
            "it is no longer unasked"
        );
        assert!(
            !store
                .vault_set_env_files("a", (EnvFiles::Asked, None), (EnvFiles::Allowed, None))
                .await?,
            "a person who saw no asker answers nothing"
        );
        assert!(
            store
                .vault_set_env_files(
                    "a",
                    (EnvFiles::Asked, Some("#1")),
                    (EnvFiles::Allowed, None)
                )
                .await?
        );
        let got = store.vault_get("a").await?.ok_or("row")?;
        assert_eq!(got.env_files, EnvFiles::Allowed);
        assert_eq!(got.env_requested_by, None);
        assert_eq!(got.updated_at, Timestamp(1), "nothing else changes");
        let mut edited = got.clone();
        edited.description = "edited".into();
        assert!(store.vault_update(&edited, &got).await?);
        assert_eq!(
            store.vault_get("a").await?.ok_or("row")?.env_files,
            EnvFiles::Allowed,
            "an edit keeps the allowance"
        );
        Ok(())
    }

    #[tokio::test]
    async fn requests_are_answered_only_as_seen() -> Result<(), Box<dyn std::error::Error>> {
        let store = store_with_sandboxes().await?;
        let hosts = |l: &[&str]| l.iter().map(|h| (*h).to_owned()).collect::<Vec<String>>();
        store
            .vault_insert(&record("a", VaultSection::Agent, Some("s1"), "KEY"))
            .await?;
        assert!(
            store
                .vault_set_request("a", &[], &hosts(&["new.test"]), Some("#1"))
                .await?
        );
        assert!(
            !store
                .vault_set_request("a", &[], &hosts(&["other.test"]), Some("#2"))
                .await?,
            "the request is no longer empty"
        );
        let both = hosts(&["api.example.com", "new.test"]);
        let bound = hosts(&["api.example.com"]);
        // A stale request or stale bindings change nothing.
        for (request, seen) in [(hosts(&["x.test"]), &bound), (hosts(&["new.test"]), &both)] {
            assert!(
                !store
                    .vault_answer_request("a", &request, seen, &both, "u1", Timestamp(9))
                    .await?
            );
        }
        assert!(
            store
                .vault_answer_request(
                    "a",
                    &hosts(&["new.test"]),
                    &bound,
                    &both,
                    "u1",
                    Timestamp(9)
                )
                .await?
        );
        let got = store.vault_get("a").await?.ok_or("row")?;
        assert_eq!(got.hosts, both);
        assert!(got.requested_hosts.is_empty());
        assert_eq!(got.requested_by, None);
        assert_eq!(got.updated_at, Timestamp(9));
        assert_eq!(
            got.envelope.ciphertext,
            vec![4, 5, 6],
            "the value is untouched"
        );
        Ok(())
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
        assert_eq!(store.vault_team(None).await?.len(), 2);
        Ok(())
    }

    /// SPEC-216 SSA-004, review M2/m1: agents setting a secret up and the production-only
    /// allowance exclude each other in the store; an edit and an automatic binding each change
    /// only what they saw.
    #[tokio::test]
    async fn agents_setting_up_and_production_stay_apart_and_edits_see_what_they_change()
    -> Result<(), Box<dyn std::error::Error>> {
        let store = store_with_sandboxes().await?;
        let mut auto = record("a", VaultSection::Agent, Some("s1"), "AUTO");
        auto.auto_allow = true;
        auto.hosts = Vec::new();
        store.vault_insert(&auto).await?;
        assert!(
            !store.vault_set_cd_prod("a", false, true).await?,
            "no production-only allowance while agents set it up"
        );

        // An edit read while it was on does not turn it back on once a person turned it off.
        let seen = store.vault_get("a").await?.ok_or("row")?;
        let mut off = seen.clone();
        off.auto_allow = false;
        assert!(store.vault_update(&off, &seen).await?);
        let mut stale = seen.clone();
        stale.description = "edited".into();
        assert!(!store.vault_update(&stale, &seen).await?, "stale switch");
        assert!(!store.vault_get("a").await?.ok_or("row")?.auto_allow);

        // Turned off, nothing binds by itself; an edit that read the hosts before a binding does
        // not drop it.
        let none: [String; 0] = [];
        let one = ["api.example.com".to_owned()];
        assert!(
            !store
                .vault_auto_bind("a", (&none, &one), (&none, &none))
                .await?
        );
        let now = store.vault_get("a").await?.ok_or("row")?;
        let mut on = now.clone();
        on.auto_allow = true;
        assert!(store.vault_update(&on, &now).await?);
        let before_binding = store.vault_get("a").await?.ok_or("row")?;
        assert!(
            store
                .vault_auto_bind("a", (&none, &one), (&none, &none))
                .await?
        );
        assert!(
            !store
                .vault_auto_bind("a", (&none, &one), (&none, &none))
                .await?,
            "stale bindings"
        );
        let mut late = before_binding.clone();
        late.description = "late edit".into();
        assert!(
            !store.vault_update(&late, &before_binding).await?,
            "stale hosts"
        );
        assert_eq!(store.vault_get("a").await?.ok_or("row")?.hosts, one);

        // A production secret is never turned into one agents set up.
        let mut prod = record("p", VaultSection::Agent, Some("s1"), "PROD");
        prod.hosts = Vec::new();
        store.vault_insert(&prod).await?;
        assert!(store.vault_set_cd_prod("p", false, true).await?);
        let seen = store.vault_get("p").await?.ok_or("row")?;
        let mut turned = seen.clone();
        turned.auto_allow = true;
        assert!(!store.vault_update(&turned, &seen).await?);
        Ok(())
    }

    #[tokio::test]
    async fn updates_meta_reauth_and_audit() -> Result<(), Box<dyn std::error::Error>> {
        let store = store_with_sandboxes().await?;
        let mut r = record("a", VaultSection::Agent, Some("s1"), "KEY");
        store.vault_insert(&r).await?;
        let seen = r.clone();
        r.name = "RENAMED".into();
        r.envelope.ciphertext = vec![9];
        r.value_updated_at = Timestamp(5);
        assert!(store.vault_update(&r, &seen).await?);
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

        assert_eq!(store.vault_unlock("h").await?, None);
        store.vault_set_unlock("h", Timestamp(5)).await?;
        assert_eq!(store.vault_unlock("h").await?, Some(Timestamp(5)));

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

    #[tokio::test]
    async fn two_factor_confirms_once_and_refuses_replays() -> Result<(), Box<dyn std::error::Error>>
    {
        let store = store_with_sandboxes().await?;
        let seed = |n: u8| TwoFactorRecord {
            user_id: "u1".into(),
            envelope: Envelope {
                kek_id: 1,
                wrapped_key: vec![n],
                ciphertext: vec![n],
            },
            created_at: Timestamp(i64::from(n)),
            confirmed_at: None,
            last_counter: None,
            failed_attempts: 0,
            failed_at: None,
        };
        assert_eq!(store.vault_two_factor("u1").await?, None);
        assert!(store.vault_put_two_factor(&seed(1)).await?);
        // Pending: a new setup replaces it.
        assert!(store.vault_put_two_factor(&seed(2)).await?);
        assert!(
            store
                .vault_accept_two_factor("u1", &[2], 10, Timestamp(9))
                .await?
        );
        let got = store.vault_two_factor("u1").await?.ok_or("missing")?;
        assert_eq!(got.envelope.ciphertext, vec![2]);
        assert_eq!(
            (got.confirmed_at, got.last_counter),
            (Some(Timestamp(9)), Some(10))
        );
        // A used or older step is refused; a later one moves on without re-confirming.
        assert!(
            !store
                .vault_accept_two_factor("u1", &[2], 10, Timestamp(10))
                .await?
        );
        assert!(
            !store
                .vault_accept_two_factor("u1", &[2], 9, Timestamp(10))
                .await?
        );
        assert!(
            store
                .vault_accept_two_factor("u1", &[2], 11, Timestamp(11))
                .await?
        );
        let got = store.vault_two_factor("u1").await?.ok_or("missing")?;
        assert_eq!(
            (got.confirmed_at, got.last_counter),
            (Some(Timestamp(9)), Some(11))
        );
        // Confirmed: it is never replaced.
        assert!(!store.vault_put_two_factor(&seed(3)).await?);
        assert_eq!(
            store
                .vault_two_factor("u1")
                .await?
                .ok_or("missing")?
                .envelope
                .ciphertext,
            vec![2]
        );
        assert!(
            !store
                .vault_accept_two_factor("nobody", &[2], 1, Timestamp(1))
                .await?
        );
        // Wrong codes count up; an accepted code or a clear resets them.
        assert_eq!(store.vault_two_factor_failed("u1", Timestamp(20)).await?, 1);
        assert_eq!(store.vault_two_factor_failed("u1", Timestamp(21)).await?, 2);
        let got = store.vault_two_factor("u1").await?.ok_or("missing")?;
        assert_eq!(
            (got.failed_attempts, got.failed_at),
            (2, Some(Timestamp(21)))
        );
        assert!(
            store
                .vault_accept_two_factor("u1", &[2], 12, Timestamp(22))
                .await?
        );
        assert_eq!(
            store
                .vault_two_factor("u1")
                .await?
                .ok_or("missing")?
                .failed_attempts,
            0
        );
        assert_eq!(store.vault_two_factor_failed("u1", Timestamp(23)).await?, 1);
        store.vault_two_factor_clear_failures("u1").await?;
        let got = store.vault_two_factor("u1").await?.ok_or("missing")?;
        assert_eq!((got.failed_attempts, got.failed_at), (0, None));
        assert_eq!(
            store
                .vault_two_factor_failed("nobody", Timestamp(1))
                .await?,
            0
        );
        Ok(())
    }

    /// #291: a reset removes the person's seed and the unlocks of their sessions, nobody else's.
    #[tokio::test]
    async fn a_reset_removes_the_seed_and_the_persons_unlocks()
    -> Result<(), Box<dyn std::error::Error>> {
        let store = store_with_sandboxes().await?;
        store
            .call(|c| {
                c.execute_batch(
                    "INSERT INTO users (id, display_name, is_superadmin, is_instance_admin, created_at) VALUES ('u2', 'V', 0, 0, 1);
                     INSERT INTO web_sessions (id_hash, user_id, csrf_token, created_at, expires_at) VALUES ('h1a', 'u1', 'c', 1, 99), ('h1b', 'u1', 'c', 1, 99), ('h2', 'u2', 'c', 1, 99);",
                )
                .map_err(db)
            })
            .await?;
        for user in ["u1", "u2"] {
            assert!(
                store
                    .vault_put_two_factor(&TwoFactorRecord {
                        user_id: user.into(),
                        envelope: Envelope {
                            kek_id: 1,
                            wrapped_key: vec![1],
                            ciphertext: vec![1],
                        },
                        created_at: Timestamp(1),
                        confirmed_at: None,
                        last_counter: None,
                        failed_attempts: 0,
                        failed_at: None,
                    })
                    .await?
            );
        }
        for session in ["h1a", "h1b", "h2"] {
            store.vault_set_unlock(session, Timestamp(5)).await?;
        }
        assert!(store.vault_reset_two_factor("u1").await?);
        assert_eq!(store.vault_two_factor("u1").await?, None);
        assert_eq!(store.vault_unlock("h1a").await?, None);
        assert_eq!(store.vault_unlock("h1b").await?, None);
        assert!(store.vault_two_factor("u2").await?.is_some());
        assert_eq!(store.vault_unlock("h2").await?, Some(Timestamp(5)));
        assert!(!store.vault_reset_two_factor("u1").await?);
        Ok(())
    }

    /// #295: a code checked against pending seed A never confirms seed B, set up meanwhile.
    #[tokio::test]
    async fn accepting_a_replaced_seed_is_refused() -> Result<(), Box<dyn std::error::Error>> {
        let store = store_with_sandboxes().await?;
        let seed = |n: u8| TwoFactorRecord {
            user_id: "u1".into(),
            envelope: Envelope {
                kek_id: 1,
                wrapped_key: vec![n],
                ciphertext: vec![n],
            },
            created_at: Timestamp(5),
            confirmed_at: None,
            last_counter: None,
            failed_attempts: 0,
            failed_at: None,
        };
        assert!(store.vault_put_two_factor(&seed(1)).await?);
        assert!(store.vault_put_two_factor(&seed(2)).await?);
        assert!(
            !store
                .vault_accept_two_factor("u1", &[1], 10, Timestamp(9))
                .await?,
            "a code of seed 1 confirmed seed 2"
        );
        let got = store.vault_two_factor("u1").await?.ok_or("missing")?;
        assert_eq!(
            (got.envelope.ciphertext, got.confirmed_at, got.last_counter),
            (vec![2], None, None)
        );
        assert!(
            store
                .vault_accept_two_factor("u1", &[2], 10, Timestamp(9))
                .await?
        );
        Ok(())
    }
}
