//! Notices for people (SPEC-624, migration 0091): one per project, recipient and key (newest
//! wins), hidden once expired and pruned by the tick, dismissed per person; and the bot's last
//! Telegram message per key in each chat.

use async_trait::async_trait;
use rusqlite::{OptionalExtension, Row, params};
use yulius_app::error::{AppError, AppResult};
use yulius_app::ports::store_ext::{NoticeRecord, NoticeStore};
use yulius_domain::ids::{SandboxId, UserId};
use yulius_domain::notice::{NoticeKey, NoticeTone};
use yulius_domain::notify::NoticeKind;
use yulius_domain::time::Timestamp;

use crate::{SqliteStore, db};

const COLS: &str = "id, sandbox_id, user_id, admin_only, notice_key, category, tone, title, \
                    detail, link, decision, created_at, expires_at";

/// The scope's condition on `notices` (`?1` sandbox, `?2` user, both `''` for none).
const SCOPE: &str = "COALESCE(sandbox_id, '') = ?1 AND COALESCE(user_id, '') = ?2";

fn from_row(row: &Row<'_>) -> rusqlite::Result<NoticeRecord> {
    let bad = |index: usize, err: String| {
        rusqlite::Error::FromSqlConversionFailure(
            index,
            rusqlite::types::Type::Text,
            Box::<dyn std::error::Error + Send + Sync>::from(err),
        )
    };
    let sandbox: Option<String> = row.get(1)?;
    let user: Option<String> = row.get(2)?;
    let key: String = row.get(4)?;
    let category: String = row.get(5)?;
    Ok(NoticeRecord {
        id: row.get(0)?,
        sandbox: sandbox
            .map(|s| SandboxId::parse(&s).map_err(|e| bad(1, e.to_string())))
            .transpose()?,
        user: user
            .map(|u| UserId::parse(&u).map_err(|e| bad(2, e.to_string())))
            .transpose()?,
        admin_only: row.get(3)?,
        key: NoticeKey::parse(&key).map_err(|e| bad(4, e))?,
        category: NoticeKind::parse(&category)
            .ok_or_else(|| bad(5, format!("unknown category {category}")))?,
        tone: NoticeTone::parse(&row.get::<_, String>(6)?),
        title: row.get(7)?,
        detail: row.get(8)?,
        link: row.get(9)?,
        decision: row.get(10)?,
        created_at: Timestamp(row.get(11)?),
        expires_at: row.get::<_, Option<i64>>(12)?.map(Timestamp),
    })
}

fn scope_of(sandbox: Option<&SandboxId>, user: Option<&UserId>) -> (String, String) {
    (
        sandbox.map(ToString::to_string).unwrap_or_default(),
        user.map(ToString::to_string).unwrap_or_default(),
    )
}

#[async_trait]
impl NoticeStore for SqliteStore {
    async fn put_notice(
        &self,
        notice: &NoticeRecord,
        keep: usize,
    ) -> AppResult<Option<NoticeRecord>> {
        if notice.decision && notice.expires_at.is_some() {
            return Err(AppError::Invalid(
                "a notice that waits for a decision never expires".into(),
            ));
        }
        let n = notice.clone();
        let keep = i64::try_from(keep.max(1)).unwrap_or(i64::MAX);
        self.call(move |c| {
            let (sandbox, user) = scope_of(n.sandbox.as_ref(), n.user.as_ref());
            let tx = c.transaction().map_err(db)?;
            let replaced = tx
                .query_row(
                    &format!("SELECT {COLS} FROM notices WHERE {SCOPE} AND notice_key = ?3"),
                    params![sandbox, user, n.key.as_str()],
                    from_row,
                )
                .optional()
                .map_err(db)?;
            if let Some(old) = &replaced {
                // Its dismissals go with it (ON DELETE CASCADE): the newer one is unread.
                tx.execute("DELETE FROM notices WHERE id = ?1", [&old.id])
                    .map_err(db)?;
            }
            tx.execute(
                &format!(
                    "INSERT INTO notices ({COLS}) \
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)"
                ),
                params![
                    n.id,
                    n.sandbox.as_ref().map(ToString::to_string),
                    n.user.as_ref().map(ToString::to_string),
                    n.admin_only,
                    n.key.as_str(),
                    n.category.as_str(),
                    n.tone.as_str(),
                    n.title,
                    n.detail,
                    n.link,
                    n.decision,
                    n.created_at.0,
                    n.expires_at.map(|t| t.0),
                ],
            )
            .map_err(db)?;
            // The scope keeps its newest `keep` (NTC-004).
            tx.execute(
                &format!(
                    "DELETE FROM notices WHERE id IN (SELECT id FROM notices WHERE {SCOPE} \
                     ORDER BY created_at DESC, id DESC LIMIT -1 OFFSET ?3)"
                ),
                params![sandbox, user, keep],
            )
            .map_err(db)?;
            tx.commit().map_err(db)?;
            Ok(replaced)
        })
        .await
    }

    async fn remove_notice(
        &self,
        sandbox: Option<&SandboxId>,
        user: Option<&UserId>,
        key: &NoticeKey,
    ) -> AppResult<Option<NoticeRecord>> {
        let (sandbox, user) = scope_of(sandbox, user);
        let key = key.as_str().to_owned();
        self.call(move |c| {
            let tx = c.transaction().map_err(db)?;
            let removed = tx
                .query_row(
                    &format!("SELECT {COLS} FROM notices WHERE {SCOPE} AND notice_key = ?3"),
                    params![sandbox, user, key],
                    from_row,
                )
                .optional()
                .map_err(db)?;
            if let Some(old) = &removed {
                tx.execute("DELETE FROM notices WHERE id = ?1", [&old.id])
                    .map_err(db)?;
            }
            tx.commit().map_err(db)?;
            Ok(removed)
        })
        .await
    }

    async fn notice(&self, id: &str) -> AppResult<Option<NoticeRecord>> {
        let id = id.to_owned();
        self.call(move |c| {
            c.query_row(
                &format!("SELECT {COLS} FROM notices WHERE id = ?1"),
                [id],
                from_row,
            )
            .optional()
            .map_err(db)
        })
        .await
    }

    async fn live_notices(
        &self,
        user: &UserId,
        now: Timestamp,
        limit: usize,
    ) -> AppResult<Vec<NoticeRecord>> {
        let user = user.to_string();
        let limit = i64::try_from(limit).unwrap_or(i64::MAX);
        self.call(move |c| {
            let mut stmt = c
                .prepare(&format!(
                    "SELECT {COLS} FROM notices n \
                     WHERE (n.user_id IS NULL OR n.user_id = ?1) \
                       AND (n.expires_at IS NULL OR n.expires_at > ?2) \
                       AND NOT EXISTS (SELECT 1 FROM notice_dismissals d \
                                       WHERE d.notice_id = n.id AND d.user_id = ?1) \
                     ORDER BY n.created_at DESC, n.id DESC LIMIT ?3"
                ))
                .map_err(db)?;
            stmt.query_map(params![user, now.0, limit], from_row)
                .map_err(db)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(db)
        })
        .await
    }

    async fn notices_by_prefix(
        &self,
        sandbox: Option<&SandboxId>,
        prefix: &str,
        now: Timestamp,
    ) -> AppResult<Vec<NoticeRecord>> {
        let sandbox = sandbox.map(ToString::to_string).unwrap_or_default();
        let prefix = prefix.to_owned();
        self.call(move |c| {
            let mut stmt = c
                .prepare(&format!(
                    "SELECT {COLS} FROM notices \
                     WHERE COALESCE(sandbox_id, '') = ?1 \
                       AND substr(notice_key, 1, length(?2)) = ?2 \
                       AND (expires_at IS NULL OR expires_at > ?3) \
                     ORDER BY created_at DESC, id DESC"
                ))
                .map_err(db)?;
            stmt.query_map(params![sandbox, prefix, now.0], from_row)
                .map_err(db)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(db)
        })
        .await
    }

    async fn dismiss_notice(&self, id: &str, user: &UserId, at: Timestamp) -> AppResult<bool> {
        let id = id.to_owned();
        let user = user.to_string();
        self.call(move |c| {
            let changed = c
                .execute(
                    "INSERT INTO notice_dismissals (notice_id, user_id, dismissed_at) \
                     SELECT id, ?2, ?3 FROM notices WHERE id = ?1 \
                     ON CONFLICT(notice_id, user_id) DO NOTHING",
                    params![id, user, at.0],
                )
                .map_err(db)?;
            if changed > 0 {
                return Ok(true);
            }
            c.query_row("SELECT 1 FROM notices WHERE id = ?1", [id], |_| Ok(()))
                .optional()
                .map(|found| found.is_some())
                .map_err(db)
        })
        .await
    }

    async fn prune_notices(&self, now: Timestamp) -> AppResult<u64> {
        self.call(move |c| {
            c.execute(
                "DELETE FROM notices WHERE expires_at IS NOT NULL AND expires_at <= ?1",
                [now.0],
            )
            .map(|n| u64::try_from(n).unwrap_or(u64::MAX))
            .map_err(db)
        })
        .await
    }

    async fn swap_keyed_message(
        &self,
        chat: &str,
        scope: &str,
        message: &str,
        at: Timestamp,
    ) -> AppResult<Option<String>> {
        let (chat, scope, message) = (chat.to_owned(), scope.to_owned(), message.to_owned());
        self.call(move |c| {
            let tx = c.transaction().map_err(db)?;
            let earlier: Option<String> = tx
                .query_row(
                    "SELECT message_id FROM telegram_keyed WHERE chat_id = ?1 AND scope = ?2",
                    params![chat, scope],
                    |r| r.get(0),
                )
                .optional()
                .map_err(db)?;
            tx.execute(
                "INSERT INTO telegram_keyed (chat_id, scope, message_id, sent_at) \
                 VALUES (?1, ?2, ?3, ?4) ON CONFLICT(chat_id, scope) \
                 DO UPDATE SET message_id = excluded.message_id, sent_at = excluded.sent_at",
                params![chat, scope, message, at.0],
            )
            .map_err(db)?;
            tx.commit().map_err(db)?;
            Ok(earlier.filter(|id| *id != message))
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
                    "INSERT INTO users (id, display_name, created_at) VALUES ('u1', 'U', 1), ('u2', 'V', 1);
                     INSERT INTO sandboxes (id, slug, repo_owner, repo_name, default_branch, profile, port_base, port_count, created_by, onboarding_state, created_at)
                       VALUES ('s1', 's', 'o', 'r', 'main', '{}', 20000, 10, 'u1', 'not_needed', 1),
                              ('s2', 't', 'o', 'q', 'main', '{}', 20010, 10, 'u1', 'not_needed', 1);",
                )
                .map_err(db)
            })
            .await?;
        Ok(store)
    }

    fn notice(id: &str, sandbox: Option<&str>, key: &str, at: i64) -> NoticeRecord {
        NoticeRecord {
            id: id.into(),
            sandbox: sandbox.and_then(|s| SandboxId::parse(s).ok()),
            user: None,
            admin_only: false,
            key: NoticeKey::parse(key).unwrap_or_else(|_| NoticeKey::lossy("bad:", key)),
            category: NoticeKind::Failed,
            tone: NoticeTone::Error,
            title: format!("title {id}"),
            detail: None,
            link: Some("/s/s/ci".into()),
            decision: false,
            created_at: Timestamp(at),
            expires_at: None,
        }
    }

    fn ids(list: &[NoticeRecord]) -> Vec<&str> {
        list.iter().map(|n| n.id.as_str()).collect()
    }

    /// NTC-003, AC-1: the same project, recipient and key keep one notice, the newest, with a
    /// new id, and a dismissal of the older one does not hide it.
    #[tokio::test]
    async fn the_same_key_replaces_and_comes_back_unread() -> TestResult {
        let store = store().await?;
        let u1 = UserId::parse("u1")?;
        let first = notice("nt_1", Some("s1"), "gate:quick::a", 10);
        assert_eq!(store.put_notice(&first, 200).await?, None);
        assert!(store.dismiss_notice("nt_1", &u1, Timestamp(11)).await?);
        assert!(ids(&store.live_notices(&u1, Timestamp(12), 50).await?).is_empty());
        let second = NoticeRecord {
            title: "newer".into(),
            ..notice("nt_2", Some("s1"), "gate:quick::a", 20)
        };
        let replaced = store.put_notice(&second, 200).await?;
        assert_eq!(replaced.map(|r| r.id), Some("nt_1".into()));
        let live = store.live_notices(&u1, Timestamp(21), 50).await?;
        assert_eq!(ids(&live), ["nt_2"]);
        assert_eq!(live[0].title, "newer");
        assert_eq!(store.notice("nt_1").await?, None);
        Ok(())
    }

    /// NTC-003, AC-2: another project, another recipient or no project is another notice.
    #[tokio::test]
    async fn the_key_is_scoped_by_project_and_recipient() -> TestResult {
        let store = store().await?;
        let u1 = UserId::parse("u1")?;
        let u2 = UserId::parse("u2")?;
        store
            .put_notice(&notice("a", Some("s1"), "k", 1), 200)
            .await?;
        store
            .put_notice(&notice("b", Some("s2"), "k", 2), 200)
            .await?;
        store.put_notice(&notice("c", None, "k", 3), 200).await?;
        let mine = NoticeRecord {
            user: Some(u1.clone()),
            ..notice("d", Some("s1"), "k", 4)
        };
        store.put_notice(&mine, 200).await?;
        assert_eq!(
            ids(&store.live_notices(&u1, Timestamp(5), 50).await?),
            ["d", "c", "b", "a"]
        );
        assert_eq!(
            ids(&store.live_notices(&u2, Timestamp(5), 50).await?),
            ["c", "b", "a"],
            "a notice for one person reaches no one else"
        );
        let removed = store
            .remove_notice(
                Some(&SandboxId::parse("s1")?),
                None,
                &NoticeKey::parse("k")?,
            )
            .await?;
        assert_eq!(removed.map(|n| n.id), Some("a".into()));
        assert_eq!(
            ids(&store.live_notices(&u1, Timestamp(5), 50).await?),
            ["d", "c", "b"]
        );
        Ok(())
    }

    /// NTC-004, AC-3: an expired notice is left out at once and pruned; a notice that never
    /// expires stays.
    #[tokio::test]
    async fn an_expired_notice_is_hidden_then_pruned() -> TestResult {
        let store = store().await?;
        let u1 = UserId::parse("u1")?;
        let soon = NoticeRecord {
            expires_at: Some(Timestamp(100)),
            ..notice("soon", Some("s1"), "watch:a", 10)
        };
        store.put_notice(&soon, 200).await?;
        store
            .put_notice(&notice("never", Some("s1"), "watch:b", 11), 200)
            .await?;
        assert_eq!(
            ids(&store.live_notices(&u1, Timestamp(99), 50).await?),
            ["never", "soon"]
        );
        assert_eq!(
            ids(&store.live_notices(&u1, Timestamp(100), 50).await?),
            ["never"]
        );
        assert_eq!(
            ids(&store
                .notices_by_prefix(Some(&SandboxId::parse("s1")?), "watch:", Timestamp(100))
                .await?),
            ["never"]
        );
        assert_eq!(store.prune_notices(Timestamp(100)).await?, 1);
        assert_eq!(store.notice("soon").await?, None);
        assert!(store.notice("never").await?.is_some());
        Ok(())
    }

    /// NTC-005: the store refuses a decision with an expiry.
    #[tokio::test]
    async fn a_decision_with_an_expiry_is_refused() -> TestResult {
        let store = store().await?;
        let bad = NoticeRecord {
            decision: true,
            expires_at: Some(Timestamp(100)),
            ..notice("x", Some("s1"), "account:a", 1)
        };
        assert!(matches!(
            store.put_notice(&bad, 200).await,
            Err(AppError::Invalid(_))
        ));
        Ok(())
    }

    /// NTC-004: a scope keeps its newest notices only; others are untouched.
    #[tokio::test]
    async fn a_scope_keeps_its_newest() -> TestResult {
        let store = store().await?;
        let u1 = UserId::parse("u1")?;
        store
            .put_notice(&notice("other", Some("s2"), "k0", 0), 2)
            .await?;
        for n in 1..=3 {
            store
                .put_notice(
                    &notice(&format!("n{n}"), Some("s1"), &format!("k{n}"), n),
                    2,
                )
                .await?;
        }
        assert_eq!(
            ids(&store.live_notices(&u1, Timestamp(9), 50).await?),
            ["n3", "n2", "other"]
        );
        assert!(!store.dismiss_notice("n1", &u1, Timestamp(9)).await?);
        Ok(())
    }

    /// NTC-007: the bot's next message for a key replaces the earlier one, per chat.
    #[tokio::test]
    async fn a_keyed_message_swaps_the_earlier_one() -> TestResult {
        let store = store().await?;
        assert_eq!(
            store
                .swap_keyed_message("c1", "s1:task:t", "10", Timestamp(1))
                .await?,
            None
        );
        assert_eq!(
            store
                .swap_keyed_message("c2", "s1:task:t", "11", Timestamp(2))
                .await?,
            None
        );
        assert_eq!(
            store
                .swap_keyed_message("c1", "s1:task:t", "12", Timestamp(3))
                .await?,
            Some("10".into())
        );
        assert_eq!(
            store
                .swap_keyed_message("c1", "s1:task:t", "12", Timestamp(4))
                .await?,
            None,
            "the same message is not its own predecessor"
        );
        Ok(())
    }
}
