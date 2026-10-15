//! Sandboxes.

use async_trait::async_trait;
use rusqlite::{OptionalExtension, Row, params};
use yulius_app::error::{AppError, AppResult};
use yulius_app::model::SandboxRecord;
use yulius_app::ports::store::SandboxStore;
use yulius_domain::ids::{SandboxId, TaskId, UserId};
use yulius_domain::time::Timestamp;

use crate::{SqliteStore, bad_column, db, to_i64, to_u64};

const COLS: &str = "id, slug, repo_owner, repo_name, github_repo_id, default_branch, profile, port_base, \
    port_count, onboarding_state, onboarding_task_id, created_by, created_at, removed_at";

fn from_row(row: &Row<'_>) -> rusqlite::Result<SandboxRecord> {
    let id: String = row.get("id")?;
    let by: String = row.get("created_by")?;
    let otask: Option<String> = row.get("onboarding_task_id")?;
    let port = |name: &str| -> rusqlite::Result<u16> {
        let v: i64 = row.get(name)?;
        u16::try_from(v).map_err(|_| bad_column(name, &v.to_string()))
    };
    Ok(SandboxRecord {
        id: SandboxId::parse(&id).map_err(|_| bad_column("sandbox id", &id))?,
        slug: row.get("slug")?,
        repo_owner: row.get("repo_owner")?,
        repo_name: row.get("repo_name")?,
        github_repo_id: row.get::<_, Option<i64>>("github_repo_id")?.map(to_u64),
        default_branch: row.get("default_branch")?,
        profile_json: row.get("profile")?,
        port_base: port("port_base")?,
        port_count: port("port_count")?,
        onboarding_state: row.get("onboarding_state")?,
        onboarding_task_id: otask
            .as_deref()
            .map(|t| TaskId::parse(t).map_err(|_| bad_column("task id", t)))
            .transpose()?,
        created_by: UserId::parse(&by).map_err(|_| bad_column("user id", &by))?,
        created_at: Timestamp(row.get("created_at")?),
        removed_at: row.get::<_, Option<i64>>("removed_at")?.map(Timestamp),
    })
}

impl SqliteStore {
    async fn sandbox_where(
        &self,
        clause: &'static str,
        args: Vec<String>,
    ) -> AppResult<Option<SandboxRecord>> {
        self.call(move |c| {
            c.query_row(
                &format!("SELECT {COLS} FROM sandboxes WHERE {clause}"),
                rusqlite::params_from_iter(args.iter()),
                from_row,
            )
            .optional()
            .map_err(db)
        })
        .await
    }
}

#[async_trait]
impl SandboxStore for SqliteStore {
    async fn insert_sandbox(&self, s: &SandboxRecord) -> AppResult<()> {
        let s = s.clone();
        self.call(move |c| {
            c.execute(
                &format!("INSERT INTO sandboxes ({COLS}) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)"),
                params![
                    s.id.as_str(),
                    s.slug,
                    s.repo_owner,
                    s.repo_name,
                    s.github_repo_id.map(to_i64),
                    s.default_branch,
                    s.profile_json,
                    i64::from(s.port_base),
                    i64::from(s.port_count),
                    s.onboarding_state,
                    s.onboarding_task_id.as_ref().map(TaskId::as_str),
                    s.created_by.as_str(),
                    s.created_at.0,
                    s.removed_at.map(|t| t.0),
                ],
            )
            .map_err(db)?;
            Ok(())
        })
        .await
    }

    async fn sandbox(&self, id: &SandboxId) -> AppResult<Option<SandboxRecord>> {
        self.sandbox_where("id = ?1", vec![id.as_str().to_owned()])
            .await
    }

    async fn sandbox_by_slug(&self, slug: &str) -> AppResult<Option<SandboxRecord>> {
        self.sandbox_where("slug = ?1 AND removed_at IS NULL", vec![slug.to_owned()])
            .await
    }

    async fn sandbox_by_repo(&self, owner: &str, name: &str) -> AppResult<Option<SandboxRecord>> {
        self.sandbox_where(
            "repo_owner = ?1 COLLATE NOCASE AND repo_name = ?2 COLLATE NOCASE AND removed_at IS NULL",
            vec![owner.to_owned(), name.to_owned()],
        )
        .await
    }

    async fn sandboxes(&self) -> AppResult<Vec<SandboxRecord>> {
        self.call(|c| {
            let mut stmt = c
                .prepare(&format!(
                    "SELECT {COLS} FROM sandboxes WHERE removed_at IS NULL ORDER BY created_at"
                ))
                .map_err(db)?;
            let rows = stmt.query_map([], from_row).map_err(db)?;
            rows.collect::<Result<Vec<_>, _>>().map_err(db)
        })
        .await
    }

    async fn next_port_base(&self, start: u16, count: u16) -> AppResult<u16> {
        self.call(move |c| {
            let mut stmt = c.prepare("SELECT port_base FROM sandboxes").map_err(db)?;
            let used: Vec<i64> = stmt
                .query_map([], |r| r.get(0))
                .map_err(db)?
                .collect::<Result<_, _>>()
                .map_err(db)?;
            let mut base = i64::from(start);
            let step = i64::from(count.max(1));
            while used.contains(&base) {
                base += step;
            }
            u16::try_from(base).map_err(|_| AppError::Capacity("no free port range".into()))
        })
        .await
    }

    async fn set_profile(&self, id: &SandboxId, profile_json: &str) -> AppResult<()> {
        let (id, p) = (id.as_str().to_owned(), profile_json.to_owned());
        self.call(move |c| {
            c.execute(
                "UPDATE sandboxes SET profile = ?1 WHERE id = ?2",
                params![p, id],
            )
            .map_err(db)?;
            Ok(())
        })
        .await
    }

    async fn set_onboarding(
        &self,
        id: &SandboxId,
        state: &str,
        task: Option<&TaskId>,
    ) -> AppResult<()> {
        let (id, st, t) = (
            id.as_str().to_owned(),
            state.to_owned(),
            task.map(|t| t.as_str().to_owned()),
        );
        self.call(move |c| {
            c.execute(
                "UPDATE sandboxes SET onboarding_state = ?1, onboarding_task_id = COALESCE(?2, onboarding_task_id) WHERE id = ?3",
                params![st, t, id],
            )
            .map_err(db)?;
            Ok(())
        })
        .await
    }

    async fn transition_onboarding(&self, id: &SandboxId, from: &str, to: &str) -> AppResult<bool> {
        let (id, from, to) = (id.as_str().to_owned(), from.to_owned(), to.to_owned());
        self.call(move |c| {
            let changed = c
                .execute(
                    "UPDATE sandboxes SET onboarding_state = ?1 WHERE id = ?2 AND onboarding_state = ?3",
                    params![to, id, from],
                )
                .map_err(db)?;
            Ok(changed > 0)
        })
        .await
    }

    async fn remove_sandbox(&self, id: &SandboxId, at: Timestamp) -> AppResult<()> {
        let id = id.as_str().to_owned();
        self.call_access(move |c| {
            c.execute(
                "UPDATE sandboxes SET removed_at = ?1 WHERE id = ?2 AND removed_at IS NULL",
                params![at.0, id],
            )
            .map_err(db)?;
            Ok(())
        })
        .await
    }

    async fn release_removed_slug(&self, slug: &str, owner: &str, name: &str) -> AppResult<bool> {
        let (slug, owner, name) = (slug.to_owned(), owner.to_owned(), name.to_owned());
        self.call(move |c| {
            let changed = c
                .execute(
                    "UPDATE sandboxes SET slug = slug || '-removed-' || removed_at \
                     WHERE slug = ?1 AND removed_at IS NOT NULL \
                     AND lower(repo_owner) = lower(?2) AND lower(repo_name) = lower(?3)",
                    params![slug, owner, name],
                )
                .map_err(db)?;
            Ok(changed > 0)
        })
        .await
    }
}
