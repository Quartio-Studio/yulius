//! Per-sandbox skill settings (absent rows mean enabled).

use async_trait::async_trait;
use rusqlite::params;
use yulius_app::error::AppResult;
use yulius_app::ports::store_ext::SkillStore;
use yulius_domain::ids::SandboxId;

use crate::SqliteStore;

#[async_trait]
impl SkillStore for SqliteStore {
    async fn skill_settings(&self, sandbox: &SandboxId) -> AppResult<Vec<(String, bool)>> {
        let sandbox = sandbox.as_str().to_owned();
        self.call(move |c| {
            let mut stmt = c.prepare(
                "SELECT name, enabled FROM skill_settings WHERE sandbox_id = ?1 ORDER BY name",
            )?;
            let rows = stmt.query_map([sandbox], |r| Ok((r.get(0)?, r.get(1)?)))?;
            Ok(rows.collect::<Result<Vec<_>, _>>()?)
        })
        .await
    }

    async fn set_skill_enabled(
        &self,
        sandbox: &SandboxId,
        name: &str,
        enabled: bool,
    ) -> AppResult<()> {
        let (sandbox, name) = (sandbox.as_str().to_owned(), name.to_owned());
        self.call(move |c| {
            c.execute(
                "INSERT INTO skill_settings (sandbox_id, name, enabled) VALUES (?1, ?2, ?3) \
                 ON CONFLICT (sandbox_id, name) DO UPDATE SET enabled = excluded.enabled",
                params![sandbox, name, enabled],
            )?;
            Ok(())
        })
        .await
    }
}
