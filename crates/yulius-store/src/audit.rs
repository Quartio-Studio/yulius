//! Audit log.

use async_trait::async_trait;
use rusqlite::{OptionalExtension as _, params};
use yulius_app::automations::AuditEntry;
use yulius_app::error::AppResult;
use yulius_app::ports::store::AuditStore;
use yulius_domain::time::Timestamp;

use crate::{SqliteStore, db};

#[async_trait]
impl AuditStore for SqliteStore {
    async fn audit(
        &self,
        at: Timestamp,
        actor: &str,
        action: &str,
        target: Option<&str>,
        detail: Option<&str>,
    ) -> AppResult<()> {
        let (actor, action, target, detail) = (
            actor.to_owned(),
            action.to_owned(),
            target.map(str::to_owned),
            detail.map(str::to_owned),
        );
        self.call(move |c| {
            c.execute(
                "INSERT INTO audit_log (at, actor, action, target, detail) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![at.0, actor, action, target, detail],
            )
            .map_err(db)?;
            Ok(())
        })
        .await
    }

    async fn audit_details(
        &self,
        target: &str,
        action: &str,
        limit: u32,
    ) -> AppResult<Vec<String>> {
        let (target, action) = (target.to_owned(), action.to_owned());
        self.call(move |c| {
            let mut stmt = c
                .prepare(
                    "SELECT detail FROM audit_log WHERE target = ?1 AND action = ?2 \
                     AND detail IS NOT NULL ORDER BY id DESC LIMIT ?3",
                )
                .map_err(db)?;
            let rows = stmt
                .query_map(params![target, action, i64::from(limit)], |r| r.get(0))
                .map_err(db)?;
            rows.collect::<Result<Vec<String>, _>>().map_err(db)
        })
        .await
    }

    async fn last_audit(
        &self,
        target: &str,
        action: &str,
        since: Timestamp,
    ) -> AppResult<Option<AuditEntry>> {
        let (target, action) = (target.to_owned(), action.to_owned());
        self.call(move |c| {
            c.query_row(
                "SELECT at, actor, action, detail FROM audit_log WHERE target = ?1 AND action = ?2 \
                 AND at >= ?3 ORDER BY id DESC LIMIT 1",
                params![target, action, since.0],
                |r| {
                    Ok(AuditEntry {
                        at: Timestamp(r.get(0)?),
                        actor: r.get(1)?,
                        action: r.get(2)?,
                        detail: r.get(3)?,
                    })
                },
            )
            .optional()
            .map_err(db)
        })
        .await
    }
}
