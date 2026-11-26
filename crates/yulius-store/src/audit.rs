//! Audit log.

use async_trait::async_trait;
use rusqlite::{OptionalExtension as _, params};
use yulius_app::automations::AuditEntry;
use yulius_app::error::AppResult;
use yulius_app::ports::store::AuditStore;
use yulius_domain::time::Timestamp;

use crate::SqliteStore;

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
            )?;
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
            let mut stmt = c.prepare(
                "SELECT detail FROM audit_log WHERE target = ?1 AND action = ?2 \
                     AND detail IS NOT NULL ORDER BY id DESC LIMIT ?3",
            )?;
            let rows = stmt.query_map(params![target, action, i64::from(limit)], |r| r.get(0))?;
            Ok(rows.collect::<Result<Vec<String>, _>>()?)
        })
        .await
    }

    async fn audit_of_sandbox(
        &self,
        slug: &str,
        also: &[String],
        actions: &[&str],
        from: Option<Timestamp>,
        to: Option<Timestamp>,
        limit: u32,
    ) -> AppResult<Vec<(String, AuditEntry)>> {
        if actions.is_empty() {
            return Ok(Vec::new());
        }
        let slug = slug.to_owned();
        let also = also.to_vec();
        let actions: Vec<String> = actions.iter().map(|a| (*a).to_owned()).collect();
        let (from, to) = (from.map_or(i64::MIN, |t| t.0), to.map_or(i64::MAX, |t| t.0));
        self.call(move |c| {
            // ?1 the slug, ?2 and ?3 the range, ?4 the limit, then the actions and the other
            // targets. `<slug>#…` as a range of the target index ('$' follows '#').
            let numbered = |from: usize, n: usize| {
                (0..n)
                    .map(|i| format!("?{}", i + from))
                    .collect::<Vec<_>>()
                    .join(", ")
            };
            let marks = numbered(5, actions.len());
            let others = if also.is_empty() {
                String::new()
            } else {
                format!(
                    " OR target IN ({})",
                    numbered(5 + actions.len(), also.len())
                )
            };
            let sql = format!(
                "SELECT target, at, actor, action, detail FROM ( \
                   SELECT id, target, at, actor, action, detail FROM audit_log \
                   WHERE (target = ?1 OR (target >= ?1 || '#' AND target < ?1 || '$'){others}) \
                   AND at >= ?2 AND at < ?3 AND action IN ({marks}) \
                   ORDER BY at DESC, id DESC LIMIT ?4) ORDER BY at, id"
            );
            let mut stmt = c.prepare(&sql)?;
            let mut values: Vec<rusqlite::types::Value> =
                vec![slug.into(), from.into(), to.into(), i64::from(limit).into()];
            values.extend(actions.into_iter().map(rusqlite::types::Value::from));
            values.extend(also.into_iter().map(rusqlite::types::Value::from));
            let rows = stmt.query_map(rusqlite::params_from_iter(values), |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    AuditEntry {
                        at: Timestamp(r.get(1)?),
                        actor: r.get(2)?,
                        action: r.get(3)?,
                        detail: r.get(4)?,
                    },
                ))
            })?;
            Ok(rows.collect::<Result<Vec<_>, _>>()?)
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
            Ok(c.query_row(
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
            .optional()?)
        })
        .await
    }
}
