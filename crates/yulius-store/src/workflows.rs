//! Workflows (instance scope = `sandbox_id IS NULL`).

use async_trait::async_trait;
use rusqlite::{OptionalExtension, Row, Transaction, params};
use yulius_app::error::{AppError, AppResult};
use yulius_app::ports::store_ext::WorkflowStore;
use yulius_app::workflows::WorkflowRecord;
use yulius_domain::ids::SandboxId;
use yulius_domain::time::Timestamp;

use crate::{DbResult, SqliteStore, bad_column, text_parsed};

const COLS: &str = "id, sandbox_id, name, description, steps, instructions, gates, created_at";

fn from_row(row: &Row<'_>) -> rusqlite::Result<WorkflowRecord> {
    let sandbox: Option<String> = row.get("sandbox_id")?;
    let gates: String = row.get("gates")?;
    let gates: serde_json::Value =
        serde_json::from_str(&gates).map_err(|_| bad_column("gates", &gates))?;
    Ok(WorkflowRecord {
        id: row.get("id")?,
        sandbox_id: sandbox
            .as_deref()
            .map(|s| SandboxId::parse(s).map_err(|_| bad_column("sandbox id", s)))
            .transpose()?,
        name: row.get("name")?,
        description: row.get("description")?,
        steps: text_parsed(row, "steps", "steps", |text| serde_json::from_str(text))?,
        instructions: row.get("instructions")?,
        require_spec: gates["require_spec"].as_bool().unwrap_or(false),
        require_tests: gates["require_tests"].as_bool().unwrap_or(false),
        created_at: Timestamp(row.get("created_at")?),
        builtin: false,
        when_to_use: String::new(),
    })
}

fn steps_json(w: &WorkflowRecord) -> String {
    serde_json::Value::from(w.steps.clone()).to_string()
}

fn gates_json(w: &WorkflowRecord) -> String {
    serde_json::json!({"require_spec": w.require_spec, "require_tests": w.require_tests})
        .to_string()
}

fn insert(tx: &Transaction<'_>, w: &WorkflowRecord) -> DbResult<()> {
    tx.execute(
        &format!("INSERT INTO workflows ({COLS}) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)"),
        params![
            w.id,
            w.sandbox_id.as_ref().map(SandboxId::as_str),
            w.name,
            w.description,
            steps_json(w),
            w.instructions,
            gates_json(w),
            w.created_at.0,
        ],
    )?;
    Ok(())
}

#[async_trait]
impl WorkflowStore for SqliteStore {
    async fn workflows(&self, sandbox: &SandboxId) -> AppResult<Vec<WorkflowRecord>> {
        let sandbox = sandbox.as_str().to_owned();
        self.call(move |c| {
            let mut stmt = c.prepare(&format!(
                "SELECT {COLS} FROM workflows WHERE sandbox_id IS NULL OR sandbox_id = ?1 \
                     ORDER BY sandbox_id IS NOT NULL, name COLLATE NOCASE, created_at"
            ))?;
            let rows = stmt.query_map([sandbox], from_row)?;
            Ok(rows.collect::<Result<Vec<_>, _>>()?)
        })
        .await
    }

    async fn workflow(&self, id: &str) -> AppResult<Option<WorkflowRecord>> {
        let id = id.to_owned();
        self.call(move |c| {
            Ok(c.query_row(
                &format!("SELECT {COLS} FROM workflows WHERE id = ?1"),
                [id],
                from_row,
            )
            .optional()?)
        })
        .await
    }

    async fn insert_workflow(
        &self,
        workflow: &WorkflowRecord,
        max_per_scope: u32,
    ) -> AppResult<()> {
        let w = workflow.clone();
        self.call(move |c| {
            let tx = c.transaction()?;
            let count: i64 = tx.query_row(
                "SELECT COUNT(*) FROM workflows WHERE sandbox_id IS ?1",
                [w.sandbox_id.as_ref().map(SandboxId::as_str)],
                |r| r.get(0),
            )?;
            if count >= i64::from(max_per_scope) {
                return Err(AppError::Capacity(format!(
                    "at most {max_per_scope} workflows per scope"
                ))
                .into());
            }
            insert(&tx, &w)?;
            Ok(tx.commit()?)
        })
        .await
    }

    async fn update_workflow(&self, workflow: &WorkflowRecord) -> AppResult<bool> {
        let w = workflow.clone();
        self.call(move |c| {
            let n = c.execute(
                "UPDATE workflows SET name = ?2, description = ?3, steps = ?4, \
                     instructions = ?5, gates = ?6 WHERE id = ?1",
                params![
                    w.id,
                    w.name,
                    w.description,
                    steps_json(&w),
                    w.instructions,
                    gates_json(&w)
                ],
            )?;
            Ok(n > 0)
        })
        .await
    }

    async fn delete_workflow(&self, id: &str) -> AppResult<bool> {
        let id = id.to_owned();
        self.call(move |c| {
            let tx = c.transaction()?;
            tx.execute(
                "UPDATE automations SET workflow_id = NULL WHERE workflow_id = ?1",
                [&id],
            )?;
            let n = tx.execute("DELETE FROM workflows WHERE id = ?1", [&id])?;
            tx.commit()?;
            Ok(n > 0)
        })
        .await
    }
}
