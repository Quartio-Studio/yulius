//! The instance's Telegram bot, link codes and linked chats (SPEC-444, migration 0050).

use async_trait::async_trait;
use rusqlite::{OptionalExtension, Row, params};
use yulius_app::error::AppResult;
use yulius_app::model::Sealed;
use yulius_app::ports::notify::{ChannelIdentity, ChatLink, ChatLinkStore, StoredBot};
use yulius_domain::ids::UserId;
use yulius_domain::notify::NotifyChoices;
use yulius_domain::time::Timestamp;

use crate::{SqliteStore, bad_column, db};

const LINK_COLUMNS: &str = "user_id, chat_id, linked_at, notify_needs_you, notify_done, \
                            notify_failed, notify_account";

fn link_row(row: &Row<'_>) -> rusqlite::Result<ChatLink> {
    let id: String = row.get(0)?;
    Ok(ChatLink {
        user: UserId::parse(&id).map_err(|_| bad_column("user id", &id))?,
        chat: row.get(1)?,
        linked_at: Timestamp(row.get(2)?),
        choices: NotifyChoices {
            needs_you: row.get(3)?,
            done: row.get(4)?,
            failed: row.get(5)?,
            account: row.get(6)?,
        },
    })
}

#[async_trait]
impl ChatLinkStore for SqliteStore {
    async fn chat_bot(&self) -> AppResult<Option<StoredBot>> {
        self.call(|c| {
            c.query_row(
                "SELECT telegram_token, telegram_bot_id, telegram_bot_username, telegram_set_at, \
                 telegram_set_by FROM instance WHERE singleton = 1",
                [],
                |r| {
                    let token: Option<Vec<u8>> = r.get(0)?;
                    Ok(token.map(|bytes| StoredBot {
                        sealed: Sealed(bytes),
                        identity: ChannelIdentity {
                            id: r.get::<_, Option<i64>>(1).ok().flatten().unwrap_or(0),
                            username: r
                                .get::<_, Option<String>>(2)
                                .ok()
                                .flatten()
                                .unwrap_or_default(),
                        },
                        set_at: Timestamp(r.get::<_, Option<i64>>(3).ok().flatten().unwrap_or(0)),
                        set_by: r.get(4).ok().flatten(),
                    }))
                },
            )
            .optional()
            .map(Option::flatten)
            .map_err(db)
        })
        .await
    }

    async fn set_chat_bot(
        &self,
        bot: Option<(&Sealed, &ChannelIdentity, &str)>,
        at: Timestamp,
    ) -> AppResult<()> {
        let bot = bot.map(|(sealed, identity, by)| {
            (
                sealed.0.clone(),
                identity.id,
                identity.username.clone(),
                by.to_owned(),
            )
        });
        self.call(move |c| {
            match bot {
                Some((bytes, id, username, by)) => c.execute(
                    "UPDATE instance SET telegram_token = ?1, telegram_bot_id = ?2, \
                     telegram_bot_username = ?3, telegram_set_at = ?4, telegram_set_by = ?5 \
                     WHERE singleton = 1",
                    params![bytes, id, username, at.0, by],
                ),
                None => c.execute(
                    "UPDATE instance SET telegram_token = NULL, telegram_bot_id = NULL, \
                     telegram_bot_username = NULL, telegram_set_at = NULL, telegram_set_by = NULL \
                     WHERE singleton = 1",
                    [],
                ),
            }
            .map_err(db)?;
            Ok(())
        })
        .await
    }

    async fn put_link_code(
        &self,
        user: &UserId,
        code_hash: &str,
        expires_at: Timestamp,
    ) -> AppResult<()> {
        let user = user.as_str().to_owned();
        let hash = code_hash.to_owned();
        self.call(move |c| {
            c.execute(
                "INSERT INTO telegram_link_codes (user_id, code_hash, expires_at) VALUES (?1, ?2, ?3) \
                 ON CONFLICT(user_id) DO UPDATE SET code_hash = excluded.code_hash, \
                 expires_at = excluded.expires_at",
                params![user, hash, expires_at.0],
            )
            .map_err(db)?;
            Ok(())
        })
        .await
    }

    async fn take_link_code(&self, code_hash: &str, now: Timestamp) -> AppResult<Option<UserId>> {
        let hash = code_hash.to_owned();
        self.call(move |c| {
            let tx = c.transaction().map_err(db)?;
            tx.execute(
                "DELETE FROM telegram_link_codes WHERE expires_at <= ?1",
                [now.0],
            )
            .map_err(db)?;
            let user: Option<String> = tx
                .query_row(
                    "DELETE FROM telegram_link_codes WHERE code_hash = ?1 RETURNING user_id",
                    [hash],
                    |r| r.get(0),
                )
                .optional()
                .map_err(db)?;
            tx.commit().map_err(db)?;
            user.map(|id| UserId::parse(&id).map_err(|_| db(bad_column("user id", &id))))
                .transpose()
        })
        .await
    }

    async fn link_chat(
        &self,
        user: &UserId,
        chat: &str,
        at: Timestamp,
    ) -> AppResult<Option<UserId>> {
        let user = user.as_str().to_owned();
        let chat = chat.to_owned();
        self.call(move |c| {
            let tx = c.transaction().map_err(db)?;
            let earlier: Option<String> = tx
                .query_row(
                    "DELETE FROM telegram_links WHERE chat_id = ?1 AND user_id <> ?2 RETURNING user_id",
                    params![chat, user],
                    |r| r.get(0),
                )
                .optional()
                .map_err(db)?;
            tx.execute(
                "INSERT INTO telegram_links (user_id, chat_id, linked_at) VALUES (?1, ?2, ?3) \
                 ON CONFLICT(user_id) DO UPDATE SET chat_id = excluded.chat_id, \
                 linked_at = excluded.linked_at, notify_needs_you = 1, notify_done = 0, \
                 notify_failed = 0, notify_account = 0",
                params![user, chat, at.0],
            )
            .map_err(db)?;
            tx.commit().map_err(db)?;
            earlier
                .map(|id| UserId::parse(&id).map_err(|_| db(bad_column("user id", &id))))
                .transpose()
        })
        .await
    }

    async fn unlink_user(&self, user: &UserId) -> AppResult<Option<String>> {
        let user = user.as_str().to_owned();
        self.call(move |c| {
            c.query_row(
                "DELETE FROM telegram_links WHERE user_id = ?1 RETURNING chat_id",
                [user],
                |r| r.get(0),
            )
            .optional()
            .map_err(db)
        })
        .await
    }

    async fn chat_link(&self, user: &UserId) -> AppResult<Option<ChatLink>> {
        let user = user.as_str().to_owned();
        self.call(move |c| {
            c.query_row(
                &format!("SELECT {LINK_COLUMNS} FROM telegram_links WHERE user_id = ?1"),
                [user],
                link_row,
            )
            .optional()
            .map_err(db)
        })
        .await
    }

    async fn chat_link_of(&self, chat: &str) -> AppResult<Option<ChatLink>> {
        let chat = chat.to_owned();
        self.call(move |c| {
            c.query_row(
                &format!("SELECT {LINK_COLUMNS} FROM telegram_links WHERE chat_id = ?1"),
                [chat],
                link_row,
            )
            .optional()
            .map_err(db)
        })
        .await
    }

    async fn chat_links(&self) -> AppResult<Vec<ChatLink>> {
        self.call(|c| {
            let mut stmt = c
                .prepare(&format!(
                    "SELECT {LINK_COLUMNS} FROM telegram_links ORDER BY linked_at"
                ))
                .map_err(db)?;
            let links = stmt
                .query_map([], link_row)
                .map_err(db)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(db)?;
            Ok(links)
        })
        .await
    }

    async fn set_chat_choices(&self, user: &UserId, choices: &NotifyChoices) -> AppResult<bool> {
        let user = user.as_str().to_owned();
        let choices = *choices;
        self.call(move |c| {
            let changed = c
                .execute(
                    "UPDATE telegram_links SET notify_needs_you = ?2, notify_done = ?3, \
                     notify_failed = ?4, notify_account = ?5 WHERE user_id = ?1",
                    params![
                        user,
                        choices.needs_you,
                        choices.done,
                        choices.failed,
                        choices.account
                    ],
                )
                .map_err(db)?;
            Ok(changed == 1)
        })
        .await
    }

    async fn clear_chat_links(&self) -> AppResult<u64> {
        self.call(|c| {
            let tx = c.transaction().map_err(db)?;
            tx.execute("DELETE FROM telegram_link_codes", [])
                .map_err(db)?;
            let links = tx.execute("DELETE FROM telegram_links", []).map_err(db)?;
            tx.commit().map_err(db)?;
            Ok(u64::try_from(links).unwrap_or(u64::MAX))
        })
        .await
    }
}

#[cfg(test)]
mod tests {
    use yulius_app::model::UserRecord;
    use yulius_app::ports::store::{InstanceStore as _, UserStore as _};

    use super::*;

    async fn person(store: &SqliteStore, id: &str) -> Result<UserId, Box<dyn std::error::Error>> {
        let id = UserId::parse(id)?;
        store
            .insert_user(&UserRecord {
                id: id.clone(),
                github_login: Some(id.as_str().to_owned()),
                github_id: None,
                display_name: id.as_str().to_owned(),
                avatar_url: None,
                is_superadmin: false,
                is_instance_admin: false,
                password_hash: None,
                created_at: Timestamp(1),
                disabled_at: None,
            })
            .await?;
        Ok(id)
    }

    #[tokio::test]
    async fn the_bot_round_trips_and_goes() -> Result<(), Box<dyn std::error::Error>> {
        let store = SqliteStore::open_in_memory()?;
        store.init_instance("https://y.test", "hash").await?;
        assert!(store.chat_bot().await?.is_none());
        let identity = ChannelIdentity {
            id: 42,
            username: "yulius_bot".into(),
        };
        store
            .set_chat_bot(
                Some((&Sealed(vec![9, 8]), &identity, "admin")),
                Timestamp(5),
            )
            .await?;
        let bot = store.chat_bot().await?.ok_or("bot")?;
        assert_eq!(bot.sealed, Sealed(vec![9, 8]));
        assert_eq!(bot.identity, identity);
        assert_eq!(bot.set_by.as_deref(), Some("admin"));
        store.set_chat_bot(None, Timestamp(6)).await?;
        assert!(store.chat_bot().await?.is_none());
        Ok(())
    }

    #[tokio::test]
    async fn a_code_is_used_once_and_only_before_it_expires()
    -> Result<(), Box<dyn std::error::Error>> {
        let store = SqliteStore::open_in_memory()?;
        let anna = person(&store, "anna").await?;
        store.put_link_code(&anna, "h1", Timestamp(100)).await?;
        // A new code replaces the earlier one.
        store.put_link_code(&anna, "h2", Timestamp(100)).await?;
        assert_eq!(store.take_link_code("h1", Timestamp(10)).await?, None);
        assert_eq!(
            store.take_link_code("h2", Timestamp(10)).await?,
            Some(anna.clone())
        );
        assert_eq!(store.take_link_code("h2", Timestamp(11)).await?, None);
        store.put_link_code(&anna, "h3", Timestamp(100)).await?;
        assert_eq!(store.take_link_code("h3", Timestamp(100)).await?, None);
        Ok(())
    }

    #[tokio::test]
    async fn one_chat_per_person_and_one_person_per_chat() -> Result<(), Box<dyn std::error::Error>>
    {
        let store = SqliteStore::open_in_memory()?;
        let anna = person(&store, "anna").await?;
        let bob = person(&store, "bob").await?;
        assert_eq!(store.link_chat(&anna, "100", Timestamp(1)).await?, None);
        let choices = NotifyChoices {
            done: true,
            ..NotifyChoices::default()
        };
        assert!(store.set_chat_choices(&anna, &choices).await?);
        assert_eq!(
            store.chat_link(&anna).await?.ok_or("link")?.choices,
            choices
        );
        // Anna's new chat replaces her earlier one, with the default choices again.
        assert_eq!(store.link_chat(&anna, "200", Timestamp(2)).await?, None);
        let link = store.chat_link(&anna).await?.ok_or("link")?;
        assert_eq!(link.chat, "200");
        assert_eq!(link.choices, NotifyChoices::default());
        assert!(store.chat_link_of("100").await?.is_none());
        // Bob takes chat 200: Anna is unlinked.
        assert_eq!(
            store.link_chat(&bob, "200", Timestamp(3)).await?,
            Some(anna.clone())
        );
        assert!(store.chat_link(&anna).await?.is_none());
        assert_eq!(store.chat_link_of("200").await?.ok_or("bob")?.user, bob);
        assert!(!store.set_chat_choices(&anna, &choices).await?);
        assert_eq!(store.unlink_user(&bob).await?.as_deref(), Some("200"));
        assert_eq!(store.unlink_user(&bob).await?, None);
        store.link_chat(&anna, "1", Timestamp(4)).await?;
        store.link_chat(&bob, "2", Timestamp(5)).await?;
        assert_eq!(store.chat_links().await?.len(), 2);
        assert_eq!(store.clear_chat_links().await?, 2);
        assert!(store.chat_links().await?.is_empty());
        Ok(())
    }
}
