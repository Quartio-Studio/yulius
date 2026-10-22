//! Project variables (SPEC-275, ADR-0203): a project's non-secret values, in plain.

use async_trait::async_trait;
use rusqlite::{OptionalExtension, Row, params};
use yulius_app::error::AppResult;
use yulius_app::ports::variables::{
    ProjectVariableRecord, ProjectVariableStore, VariableOutcome, VariableWrite,
};
use yulius_domain::ids::SandboxId;
use yulius_domain::time::Timestamp;

use crate::{SqliteStore, bad_column, db};

const COLS: &str =
    "sandbox_id, name, value, description, created_by, created_at, updated_by, updated_at";

fn from_row(row: &Row<'_>) -> rusqlite::Result<ProjectVariableRecord> {
    let sandbox: String = row.get("sandbox_id")?;
    Ok(ProjectVariableRecord {
        sandbox_id: SandboxId::parse(&sandbox).map_err(|_| bad_column("sandbox id", &sandbox))?,
        name: row.get("name")?,
        value: row.get("value")?,
        description: row.get("description")?,
        created_by: row.get("created_by")?,
        created_at: Timestamp(row.get("created_at")?),
        updated_by: row.get("updated_by")?,
        updated_at: Timestamp(row.get("updated_at")?),
    })
}

#[async_trait]
impl ProjectVariableStore for SqliteStore {
    async fn project_variables(
        &self,
        sandbox: &SandboxId,
    ) -> AppResult<Vec<ProjectVariableRecord>> {
        let sandbox = sandbox.as_str().to_owned();
        self.call(move |c| {
            let mut stmt = c
                .prepare(&format!(
                    "SELECT {COLS} FROM project_variables WHERE sandbox_id = ?1 ORDER BY name"
                ))
                .map_err(db)?;
            let rows = stmt.query_map([sandbox], from_row).map_err(db)?;
            rows.collect::<Result<Vec<_>, _>>().map_err(db)
        })
        .await
    }

    async fn project_variables_put(
        &self,
        sandbox: &SandboxId,
        writes: Vec<VariableWrite>,
        (by, at): (&str, Timestamp),
        only_new: bool,
        cap: usize,
    ) -> AppResult<Option<Vec<(String, VariableOutcome)>>> {
        let (sandbox, by) = (sandbox.as_str().to_owned(), by.to_owned());
        self.call(move |c| {
            let tx = c.transaction().map_err(db)?;
            let mut outcomes = Vec::with_capacity(writes.len());
            for w in writes {
                let before: Option<(String, String)> = tx
                    .query_row(
                        "SELECT value, description FROM project_variables \
                         WHERE sandbox_id = ?1 AND name = ?2",
                        params![sandbox, w.name],
                        |row| Ok((row.get(0)?, row.get(1)?)),
                    )
                    .optional()
                    .map_err(db)?;
                let outcome = match before {
                    None => {
                        tx.execute(
                            "INSERT INTO project_variables (sandbox_id, name, value, \
                             description, created_by, created_at, updated_by, updated_at) \
                             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?5, ?6)",
                            params![
                                sandbox,
                                w.name,
                                w.value,
                                w.description.unwrap_or_default(),
                                by,
                                at.0
                            ],
                        )
                        .map_err(db)?;
                        VariableOutcome::Created
                    }
                    Some(_) if only_new => VariableOutcome::Existing,
                    Some((value, description)) => {
                        let next = w.description.unwrap_or_else(|| description.clone());
                        let mut fields = Vec::new();
                        if value != w.value {
                            fields.push("value");
                        }
                        if description != next {
                            fields.push("description");
                        }
                        if fields.is_empty() {
                            VariableOutcome::Unchanged
                        } else {
                            tx.execute(
                                "UPDATE project_variables SET value = ?3, description = ?4, \
                                 updated_by = ?5, updated_at = ?6 \
                                 WHERE sandbox_id = ?1 AND name = ?2",
                                params![sandbox, w.name, w.value, next, by, at.0],
                            )
                            .map_err(db)?;
                            VariableOutcome::Changed(fields)
                        }
                    }
                };
                outcomes.push((w.name, outcome));
            }
            let held: i64 = tx
                .query_row(
                    "SELECT COUNT(*) FROM project_variables WHERE sandbox_id = ?1",
                    [&sandbox],
                    |row| row.get(0),
                )
                .map_err(db)?;
            if usize::try_from(held).unwrap_or(usize::MAX) > cap {
                // Dropped without commit: rolled back.
                return Ok(None);
            }
            tx.commit().map_err(db)?;
            Ok(Some(outcomes))
        })
        .await
    }

    async fn project_variable_delete(&self, sandbox: &SandboxId, name: &str) -> AppResult<bool> {
        let (sandbox, name) = (sandbox.as_str().to_owned(), name.to_owned());
        self.call(move |c| {
            let n = c
                .execute(
                    "DELETE FROM project_variables WHERE sandbox_id = ?1 AND name = ?2",
                    [sandbox, name],
                )
                .map_err(db)?;
            Ok(n == 1)
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
                    "INSERT INTO users (id, display_name, is_superadmin, is_instance_admin, created_at) VALUES ('u1', 'U', 1, 1, 1);
                     INSERT INTO sandboxes (id, slug, repo_owner, repo_name, default_branch, profile, port_base, port_count, created_by, onboarding_state, created_at) VALUES ('s1', 'a', 'o', 'a', 'main', '{}', 20000, 10, 'u1', 'not_needed', 1);
                     INSERT INTO sandboxes (id, slug, repo_owner, repo_name, default_branch, profile, port_base, port_count, created_by, onboarding_state, created_at) VALUES ('s2', 'b', 'o', 'b', 'main', '{}', 20010, 10, 'u1', 'not_needed', 1);",
                )
                .map_err(db)
            })
            .await?;
        Ok(store)
    }

    fn write(name: &str, value: &str, description: Option<&str>) -> VariableWrite {
        VariableWrite {
            name: name.into(),
            value: value.into(),
            description: description.map(Into::into),
        }
    }

    /// SPEC-275 VAR-001, review round 1: a batch inserts or changes by name per project, decides
    /// each outcome inside its transaction, keeps who created a variable and an unsaid
    /// description, leaves existing ones alone when only new ones are written, writes nothing
    /// over the cap, and deletes by name.
    #[tokio::test]
    async fn variables_are_set_by_name_per_project_within_a_cap() -> TestResult {
        let store = store().await?;
        let (s1, s2) = (SandboxId::parse("s1")?, SandboxId::parse("s2")?);
        let put = |sandbox: &SandboxId, writes: Vec<VariableWrite>, at: i64, only_new: bool| {
            let (store, sandbox) = (&store, sandbox.clone());
            async move {
                store
                    .project_variables_put(
                        &sandbox,
                        writes,
                        (&format!("u{at}"), Timestamp(at)),
                        only_new,
                        3,
                    )
                    .await
            }
        };
        let done = put(
            &s1,
            vec![
                write("REGION", "ovh-ca", Some("OVH region")),
                write("ENV", "production", None),
            ],
            1,
            false,
        )
        .await?;
        assert_eq!(
            done,
            Some(vec![
                ("REGION".into(), VariableOutcome::Created),
                ("ENV".into(), VariableOutcome::Created),
            ])
        );
        put(&s2, vec![write("REGION", "eu", None)], 1, false).await?;

        let done = put(
            &s1,
            vec![
                write("REGION", "ovh-us", None),
                write("ENV", "production", Some("")),
            ],
            2,
            false,
        )
        .await?;
        assert_eq!(
            done,
            Some(vec![
                ("REGION".into(), VariableOutcome::Changed(vec!["value"])),
                ("ENV".into(), VariableOutcome::Unchanged),
            ])
        );
        let held = store.project_variables(&s1).await?;
        let region = held.iter().find(|v| v.name == "REGION").ok_or("REGION")?;
        assert_eq!(
            (
                region.value.as_str(),
                region.description.as_str(),
                region.created_by.as_str(),
                region.updated_by.as_str()
            ),
            ("ovh-us", "OVH region", "u1", "u2")
        );
        assert_eq!(store.project_variables(&s2).await?[0].value, "eu");

        // Only new ones: an existing variable is left as it is.
        let done = put(
            &s1,
            vec![write("REGION", "other", None), write("ZONE", "a", None)],
            3,
            true,
        )
        .await?;
        assert_eq!(
            done,
            Some(vec![
                ("REGION".into(), VariableOutcome::Existing),
                ("ZONE".into(), VariableOutcome::Created),
            ])
        );
        assert_eq!(
            store.project_variables(&s1).await?[1].value,
            "ovh-us",
            "REGION untouched"
        );

        // Over the cap: the whole batch is dropped, the existing change included.
        let over = vec![write("REGION", "changed", None), write("A_B", "a", None)];
        assert_eq!(put(&s1, over, 4, false).await?, None);
        let held = store.project_variables(&s1).await?;
        assert_eq!(held.len(), 3);
        assert_eq!(held[1].value, "ovh-us");

        assert!(store.project_variable_delete(&s1, "ENV").await?);
        assert!(!store.project_variable_delete(&s1, "ENV").await?);
        assert_eq!(store.project_variables(&s1).await?.len(), 2);

        store
            .call(|c| {
                c.execute("DELETE FROM sandboxes WHERE id = 's1'", [])
                    .map_err(db)
            })
            .await?;
        assert!(store.project_variables(&s1).await?.is_empty());
        Ok(())
    }
}
