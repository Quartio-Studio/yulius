//! Notices for people (SPEC-624, ADR-0462): Yulius's producers, and script watches (#614), post
//! a notice with a key and an expiry. A newer notice with the same project, recipient and key
//! replaces the older one (newest wins); an expired notice is left out of every read and pruned
//! by the tick; a decision never expires. Who sees a notice is the event stream's rule
//! ([`App::envelope_reaches`]) for its project, recipient and `admin_only`.
//!
//! - NTC-002, NTC-005: [`App::post_notice`] resolves the expiry by [`notice_expiry`];
//! - NTC-003: [`crate::ports::store_ext::NoticeStore::put_notice`] replaces in one transaction;
//! - NTC-004: [`App::my_notices`] reads live notices only, [`App::prune_notices_now`] deletes;
//! - NTC-006: [`App::remove_notice`], [`App::dismiss_notice`].

use yulius_contracts::api::notices::NoticeView;
use yulius_contracts::events::UiEvent;
use yulius_domain::ids::{SandboxId, UserId};
use yulius_domain::notice::{
    Expiry, FAILURE_MS, GOOD_NEWS_MS, NoticeKey, NoticeTone, REPORT_MS, live, notice_expiry,
};
use yulius_domain::notify::NoticeKind;

use crate::auth::{AuthContext, rfc3339};
use crate::context::App;
use crate::error::{AppError, AppResult};
use crate::events::{Envelope, Reach};
use crate::model::UserRecord;
use crate::ports::store_ext::{NoticeReach, NoticeRecord};

/// Notices kept per project and recipient (NTC-004); the oldest go first.
pub const NOTICES_KEPT: usize = 200;
/// Notices a read returns at most.
pub const NOTICES_LISTED: usize = 200;
/// Longest title, in characters; a longer one is cut.
pub const TITLE_MAX: usize = 300;
/// Longest detail, in characters; a longer one is cut.
pub const DETAIL_MAX: usize = 4000;
/// Key prefixes of the notices Yulius derives from task state and its digest (the spec's
/// table): a posted notice may not use them, so it never shares their place in a chat.
pub const DERIVED_KEYS: [&str; 4] = ["task:", "control:", "needs-you:", "digest"];

/// Posts are stored and published one at a time, so pages and chats hear two posts of one key in
/// the order the store kept them (review round 1).
static POSTING: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// A notice to post (SPEC-624): what Yulius's producers and script watches (#614) build.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewNotice {
    /// Its project; `None`: instance-wide.
    pub sandbox: Option<SandboxId>,
    /// Its one recipient; `None`: everyone the project (or `admin_only`) lets see it.
    pub to: Option<UserId>,
    /// Only instance administrators see it.
    pub admin_only: bool,
    /// Its key: a newer notice with the same project, recipient and key replaces it.
    pub key: NoticeKey,
    /// Which of a person's channel choices it falls under.
    pub category: NoticeKind,
    /// How it reads.
    pub tone: NoticeTone,
    /// One line.
    pub title: String,
    /// More, if any.
    pub detail: Option<String>,
    /// A path in Yulius (`/…`) it leads to, if any.
    pub link: Option<String>,
    /// It waits for a person's decision: it never expires (NTC-005).
    pub decision: bool,
    /// Its expiry; `None`: its kind's default ([`default_expiry`]).
    pub expiry: Option<Expiry>,
}

impl NewNotice {
    /// A notice of `sandbox` for everyone who sees it, with its kind's default expiry.
    #[must_use]
    pub fn new(
        sandbox: Option<SandboxId>,
        key: NoticeKey,
        category: NoticeKind,
        tone: NoticeTone,
        title: impl Into<String>,
    ) -> Self {
        Self {
            sandbox,
            to: None,
            admin_only: false,
            key,
            category,
            tone,
            title: title.into(),
            detail: None,
            link: None,
            decision: false,
            expiry: None,
        }
    }
}

/// The expiry a notice gets when its producer gives none (the spec's table): never for a
/// decision, seven days for a failure, three for good news, one day for anything else.
#[must_use]
pub fn default_expiry(category: NoticeKind, tone: NoticeTone, decision: bool) -> Expiry {
    if decision {
        return Expiry::Never;
    }
    match (category, tone) {
        (NoticeKind::Failed, _) => Expiry::After(FAILURE_MS),
        (NoticeKind::Done, _) | (_, NoticeTone::Ok) => Expiry::After(GOOD_NEWS_MS),
        _ => Expiry::After(REPORT_MS),
    }
}

/// Whom a notice reaches, as an envelope of the event stream.
fn envelope_of(notice: &NoticeRecord, event: UiEvent) -> Envelope {
    Envelope {
        sandbox: notice.sandbox.clone(),
        admin_only: notice.admin_only,
        user: notice.user.clone(),
        writers_only: false,
        event,
    }
}

/// What people see of a notice.
#[must_use]
pub fn notice_view(notice: &NoticeRecord) -> NoticeView {
    NoticeView {
        id: notice.id.clone(),
        sandbox_id: notice.sandbox.as_ref().map(ToString::to_string),
        key: notice.key.to_string(),
        category: notice.category.as_str().into(),
        tone: notice.tone.as_str().into(),
        title: notice.title.clone(),
        detail: notice.detail.clone(),
        link: notice.link.clone(),
        decision: notice.decision,
        personal: notice.user.is_some(),
        created_at: rfc3339(notice.created_at),
        expires_at: notice.expires_at.map(rfc3339),
    }
}

/// `text` on one line: runs of whitespace and control characters become one space.
fn one_line(text: &str) -> String {
    text.split(|c: char| c.is_whitespace() || c.is_control())
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

fn cut(text: &str, max: usize) -> String {
    let text = text.trim();
    if text.chars().count() <= max {
        return text.to_owned();
    }
    let mut out: String = text.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

impl App {
    /// Posts `new` (SPEC-624): it replaces the notice with its project, recipient and key, and
    /// reaches the open pages and chats of the people who see it. Texts are redacted as the
    /// project's transcripts are, then cut.
    ///
    /// # Errors
    /// `Invalid` for an empty title, a key of the derived notices ([`DERIVED_KEYS`]), a link that
    /// is not a path in Yulius, an expiry that is not in the future, or a decision with an
    /// expiry (NTC-005); store failures.
    pub async fn post_notice(&self, new: NewNotice) -> AppResult<NoticeRecord> {
        if new.title.trim().is_empty() {
            return Err(AppError::Invalid("a notice needs a title".into()));
        }
        if DERIVED_KEYS.iter().any(|p| new.key.as_str().starts_with(p)) {
            return Err(AppError::Invalid(format!(
                "a notice key may not start with {}: Yulius keeps those for its own notices",
                DERIVED_KEYS.join(", ")
            )));
        }
        // A path in Yulius: one `/`, and nothing a browser reads as another host (`/\\host`,
        // control characters).
        if let Some(link) = &new.link
            && (!link.starts_with('/')
                || link.starts_with("//")
                || link.contains('\\')
                || link.chars().any(char::is_control))
        {
            return Err(AppError::Invalid(
                "a notice's link is a path in Yulius, starting with one /".into(),
            ));
        }
        let now = self.now();
        let expires_at = notice_expiry(
            new.decision,
            new.expiry,
            default_expiry(new.category, new.tone, new.decision),
            now,
        )
        .map_err(AppError::Invalid)?;
        let sandbox = new.sandbox.as_ref();
        let (title, detail) = self
            .stored_texts(sandbox, &new.title, new.detail.as_deref())
            .await;
        let id: String = self
            .deps
            .crypto
            .random_bytes()
            .iter()
            .take(8)
            .map(|b| format!("{b:02x}"))
            .collect();
        let notice = NoticeRecord {
            id: format!("nt_{id}"),
            sandbox: new.sandbox,
            user: new.to,
            admin_only: new.admin_only,
            key: new.key,
            category: new.category,
            tone: new.tone,
            title,
            detail,
            link: new.link,
            decision: new.decision,
            created_at: now,
            expires_at,
        };
        let _posting = POSTING.lock().await;
        let put = self.deps.store.put_notice(&notice, NOTICES_KEPT).await?;
        // A notice its scope's decisions leave no room for is dropped at once: refused, and
        // nobody heard of it (review round 2).
        if put.dropped.iter().any(|d| d.id == notice.id) {
            return Err(AppError::Capacity(format!(
                "this project and recipient already hold {NOTICES_KEPT} notices that wait for a \
                 decision; remove some first"
            )));
        }
        for dropped in &put.dropped {
            self.publish_removed(dropped);
        }
        // A replacement that reaches fewer people tells the others the old one is gone.
        if let Some(old) = put
            .replaced
            .as_ref()
            .filter(|old| old.admin_only != notice.admin_only)
        {
            self.publish_removed(old);
        }
        self.events.publish_envelope(envelope_of(
            &notice,
            UiEvent::NoticePosted {
                notice: Box::new(notice_view(&notice)),
            },
        ));
        Ok(notice)
    }

    /// Tells the pages that see `removed` that it is gone.
    fn publish_removed(&self, removed: &NoticeRecord) {
        self.events.publish_envelope(envelope_of(
            removed,
            UiEvent::NoticeRemoved {
                id: removed.id.clone(),
                sandbox_id: removed.sandbox.as_ref().map(ToString::to_string),
                key: removed.key.to_string(),
            },
        ));
    }

    /// A notice's title and detail as they are stored: redacted whole, then cut (a cut first
    /// could leave a secret's prefix that no longer matches, review round 1); the title on one
    /// line (review round 2). What a producer compares a repeat with (review of #1711).
    pub(crate) async fn stored_texts(
        &self,
        sandbox: Option<&SandboxId>,
        title: &str,
        detail: Option<&str>,
    ) -> (String, Option<String>) {
        let title = cut(
            &one_line(&self.redact_posted_notice(sandbox, title.to_owned()).await),
            TITLE_MAX,
        );
        let detail = match detail.map(str::trim) {
            Some(d) if !d.is_empty() => Some(cut(
                &self.redact_posted_notice(sandbox, d.to_owned()).await,
                DETAIL_MAX,
            )),
            _ => None,
        };
        (title, detail)
    }

    /// `text` redacted as `sandbox`'s transcripts are; instance-wide by credential shapes.
    async fn redact_posted_notice(&self, sandbox: Option<&SandboxId>, text: String) -> String {
        match sandbox {
            Some(sandbox) => self.redact_for_sandbox(sandbox, &text).await,
            None => yulius_domain::redact::redact(&text).map_or(text, |r| r.text),
        }
    }

    /// Removes the notice with this project, recipient and key (NTC-006); open pages drop it.
    /// `false` when there was none.
    ///
    /// # Errors
    /// Store failures.
    pub async fn remove_notice(
        &self,
        sandbox: Option<&SandboxId>,
        to: Option<&UserId>,
        key: &NoticeKey,
    ) -> AppResult<bool> {
        let _posting = POSTING.lock().await;
        let Some(removed) = self.deps.store.remove_notice(sandbox, to, key).await? else {
            return Ok(false);
        };
        self.publish_removed(&removed);
        Ok(true)
    }

    /// The live notices `user` sees and has not dismissed, newest first (NTC-004): of the
    /// projects they see (only `sandbox`, when given) and the instance-wide ones.
    ///
    /// # Errors
    /// Store failures.
    pub async fn notices_for(
        &self,
        user: &UserRecord,
        sandbox: Option<&SandboxId>,
    ) -> AppResult<Vec<NoticeRecord>> {
        let now = self.now();
        let mut reach = Reach::default();
        // Where they may come from is decided before the read's limit, so notices of projects
        // the reader cannot see never crowd theirs out (review round 1); `envelope_reaches`
        // below stays the exact rule.
        // Only the projects holding such notices are asked about, so a read costs no
        // permission lookups for the others.
        let held = match sandbox {
            Some(sandbox) => vec![sandbox.clone()],
            None => self.deps.store.notice_sandboxes(&user.id, now).await?,
        };
        let mut sandboxes = Vec::new();
        for id in held {
            if self.can_view(user, &id).await {
                sandboxes.push(id);
            }
        }
        let from = NoticeReach {
            sandboxes,
            instance_wide: !self.is_guest(user),
            admin: user.is_admin(),
        };
        let mut shown = Vec::new();
        for notice in self
            .deps
            .store
            .live_notices(&user.id, &from, now, NOTICES_LISTED)
            .await?
        {
            if !live(notice.expires_at, now)
                || sandbox.is_some_and(|s| notice.sandbox.as_ref().is_some_and(|n| n != s))
            {
                continue;
            }
            let envelope = envelope_of(&notice, UiEvent::Resync);
            if self.envelope_reaches(user, &envelope, &mut reach).await {
                shown.push(notice);
            }
        }
        Ok(shown)
    }

    /// `GET /api/me/notices` (NTC-004): the caller's live notices.
    ///
    /// # Errors
    /// Store failures.
    pub async fn my_notices(&self, ctx: &AuthContext) -> AppResult<Vec<NoticeView>> {
        Ok(self
            .notices_for(&ctx.user, None)
            .await?
            .iter()
            .map(notice_view)
            .collect())
    }

    /// Dismisses notice `id` for `user` only (NTC-006); a replacement shows again.
    ///
    /// # Errors
    /// `NotFound` for a notice they do not see; store failures.
    pub async fn dismiss_notice(&self, user: &UserRecord, id: &str) -> AppResult<()> {
        let store = &self.deps.store;
        let Some(notice) = store.notice(id).await? else {
            return Err(AppError::NotFound);
        };
        let addressed = notice.user.as_ref().is_none_or(|to| *to == user.id);
        let envelope = envelope_of(&notice, UiEvent::Resync);
        if !addressed
            || !self
                .envelope_reaches(user, &envelope, &mut Reach::default())
                .await
        {
            return Err(AppError::NotFound);
        }
        if store.dismiss_notice(id, &user.id, self.now()).await? {
            Ok(())
        } else {
            Err(AppError::NotFound)
        }
    }

    /// The tick's prune (NTC-004): deletes the notices expired by now.
    pub async fn prune_notices_now(&self) {
        match self.deps.store.prune_notices(self.now()).await {
            Ok(0) => {}
            Ok(pruned) => tracing::debug!(pruned, "expired notices pruned"),
            Err(err) => tracing::warn!(error = %err, "expired notices could not be pruned"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_follow_the_table() {
        assert_eq!(
            default_expiry(NoticeKind::Account, NoticeTone::Warn, true),
            Expiry::Never
        );
        assert_eq!(
            default_expiry(NoticeKind::Failed, NoticeTone::Error, false),
            Expiry::After(FAILURE_MS)
        );
        assert_eq!(
            default_expiry(NoticeKind::Done, NoticeTone::Ok, false),
            Expiry::After(GOOD_NEWS_MS)
        );
        assert_eq!(
            default_expiry(NoticeKind::NeedsYou, NoticeTone::Info, false),
            Expiry::After(REPORT_MS)
        );
    }

    #[test]
    fn long_texts_are_cut() {
        assert_eq!(cut("  short  ", 10), "short");
        let long = cut(&"x".repeat(20), 10);
        assert_eq!(long.chars().count(), 10);
        assert!(long.ends_with('…'));
    }
}
