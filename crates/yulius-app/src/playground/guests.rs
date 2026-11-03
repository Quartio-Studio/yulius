//! A playground's guests (SPEC-410, ADR-0260; amends SPEC-001 ACC-005 for playgrounds only, by
//! the product owner's decision of 2026-11-03). A playground's admin creates an expiring link; a
//! teammate who holds it joins with a name, as a guest: an account with no GitHub link and no
//! password whose browser session ends with the link. A guest works in that playground and reaches
//! nothing else (access in `access.rs`, the route fence in the server's `Auth` extractor).

use yulius_contracts::api::guests::{
    GuestInviteCreated, GuestInvitePeek, GuestInviteView, GuestInvites, GuestView,
};
use yulius_domain::ids::{SandboxId, UserId};
use yulius_domain::permission::Action;

use crate::auth::{AuthContext, NewSession, count_client_attempt, hex_token, rfc3339};
use crate::context::App;
use crate::error::{AppError, AppResult};
use crate::model::{SandboxRecord, UserRecord};
use crate::ports::guests::{GuestJoin, PlaygroundGuestRecord, PlaygroundInviteRecord};

/// How long a link may last, in days (GST-001).
pub const GUEST_LINK_DAYS: [u32; 3] = [1, 7, 30];
/// A link's lifetime when none is chosen.
pub const DEFAULT_GUEST_LINK_DAYS: u32 = 7;
/// Links live at once per playground (GST-001).
pub const MAX_LIVE_GUEST_LINKS: usize = 5;
/// Guests one link lets in (GST-002).
pub const MAX_GUESTS_PER_LINK: usize = 20;
/// Looks at and joins through links per client key per minute (GST-002).
pub const GUEST_ATTEMPTS_PER_MINUTE: usize = 10;
/// The longest guest name, in characters.
pub const MAX_GUEST_NAME: usize = 60;
const DAY_MS: i64 = 24 * 60 * 60 * 1000;
/// What a guest is told where they may not go (GST-004).
pub const GUEST_FENCE: &str = "a guest reaches only the playground they were let into";

/// A guest's name: trimmed, 1 to [`MAX_GUEST_NAME`] characters, no control or format character
/// (which could hide or reorder what others read).
///
/// # Errors
/// `Invalid` otherwise.
pub fn guest_name(raw: &str) -> AppResult<String> {
    let name = raw.trim();
    if name.is_empty() {
        return Err(AppError::Invalid("type your name to join".into()));
    }
    if name.chars().count() > MAX_GUEST_NAME {
        return Err(AppError::Invalid(format!(
            "a name has at most {MAX_GUEST_NAME} characters"
        )));
    }
    if name.chars().any(|c| c.is_control() || is_format(c)) {
        return Err(AppError::Invalid(
            "a name cannot hold control characters".into(),
        ));
    }
    Ok(name.to_owned())
}

/// Unicode format characters (Cf) that matter in a name: bidirectional controls and invisible
/// joiners and marks.
fn is_format(c: char) -> bool {
    matches!(c, '\u{00ad}' | '\u{061c}' | '\u{180e}' | '\u{200b}'..='\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2060}'..='\u{206f}' | '\u{feff}')
}

impl App {
    /// The link `user` came in by, when they are a playground's guest. Only an account with no
    /// GitHub login that is not the superadmin can be one, so nobody else pays a lookup.
    ///
    /// # Errors
    /// Store failures.
    pub async fn guest_link_of(
        &self,
        user: &UserRecord,
    ) -> AppResult<Option<PlaygroundInviteRecord>> {
        if user.is_superadmin || user.github_login.is_some() {
            return Ok(None);
        }
        self.deps.store.guest_invite_of(&user.id).await
    }

    /// Whether `user` is a playground's guest (errors count as yes: the narrower answer).
    pub async fn is_guest(&self, user: &UserRecord) -> bool {
        !matches!(self.guest_link_of(user).await, Ok(None))
    }

    /// `POST /api/sandboxes/{sid}/guest-invites` (GST-001): a new link to the playground, from
    /// one of its admins acting as a person (a browser session or an `operate` token).
    ///
    /// # Errors
    /// `NotFound` for an invisible sandbox, `Forbidden` for a non-admin or a `cli` token, `Conflict`
    /// for a sandbox that is not a playground or one with [`MAX_LIVE_GUEST_LINKS`] live links,
    /// `Invalid` for another number of days.
    pub async fn create_guest_invite(
        &self,
        ctx: &AuthContext,
        sandbox: &str,
        days: Option<u32>,
    ) -> AppResult<GuestInviteCreated> {
        let sid = SandboxId::parse(sandbox).map_err(|_| AppError::NotFound)?;
        let (record, _) = self.require(&ctx.user, &sid, Action::Administer).await?;
        if !ctx.acts_for_person() {
            return Err(AppError::Forbidden(
                "a person lets guests in: create the link in the browser".into(),
            ));
        }
        if !record.is_playground() {
            return Err(AppError::Conflict(
                "only a playground takes guests; this project's access mirrors GitHub".into(),
            ));
        }
        let days = days.unwrap_or(DEFAULT_GUEST_LINK_DAYS);
        if !GUEST_LINK_DAYS.contains(&days) {
            return Err(AppError::Invalid(
                "a guest link lasts 1, 7 or 30 days".into(),
            ));
        }
        let crypto = &self.deps.crypto;
        let code = hex_token(&crypto.random_bytes());
        let now = self.now();
        let invite = PlaygroundInviteRecord {
            id: crypto.new_id(),
            sandbox_id: sid.clone(),
            code_hash: crypto.sha256_hex(code.as_bytes()),
            created_by: ctx.user.id.clone(),
            created_at: now,
            expires_at: now.plus_millis(i64::from(days) * DAY_MS),
            revoked_at: None,
        };
        let store = &self.deps.store;
        if !store
            .insert_playground_invite(&invite, MAX_LIVE_GUEST_LINKS)
            .await?
        {
            return Err(AppError::Conflict(format!(
                "this playground already has {MAX_LIVE_GUEST_LINKS} live guest links; revoke one \
                 first"
            )));
        }
        store
            .audit(
                now,
                &ctx.user.label(),
                "playground.invite.create",
                Some(sid.as_str()),
                Some(&format!("{} for {days} days", invite.id)),
            )
            .await?;
        Ok(GuestInviteCreated {
            invite: self.invite_view(&invite, &[]).await,
            url: format!("{}/join?code={code}", self.config.public_url),
        })
    }

    /// `GET /api/sandboxes/{sid}/guest-invites` (GST-005): the playground's live links and their
    /// guests, for its admins.
    ///
    /// # Errors
    /// `NotFound` for an invisible sandbox, `Forbidden` for a non-admin.
    pub async fn guest_invites(&self, ctx: &AuthContext, sandbox: &str) -> AppResult<GuestInvites> {
        let sid = SandboxId::parse(sandbox).map_err(|_| AppError::NotFound)?;
        self.require(&ctx.user, &sid, Action::Administer).await?;
        let store = &self.deps.store;
        let invites = store.live_playground_invites(&sid, self.now()).await?;
        let guests = store.playground_guests(&sid).await?;
        let mut out = Vec::with_capacity(invites.len());
        for invite in &invites {
            out.push(self.invite_view(invite, &guests).await);
        }
        Ok(GuestInvites {
            invites: out,
            max_live: MAX_LIVE_GUEST_LINKS,
            max_guests: MAX_GUESTS_PER_LINK,
        })
    }

    /// `DELETE /api/sandboxes/{sid}/guest-invites/{id}` (GST-005): revokes a live link; its
    /// guests' sessions end with it, so their next request is a 401 and their open sockets close
    /// at the next re-check.
    ///
    /// # Errors
    /// `NotFound` for an invisible sandbox or no such live link, `Forbidden` for a non-admin.
    pub async fn revoke_guest_invite(
        &self,
        ctx: &AuthContext,
        sandbox: &str,
        invite: &str,
    ) -> AppResult<()> {
        let sid = SandboxId::parse(sandbox).map_err(|_| AppError::NotFound)?;
        self.require(&ctx.user, &sid, Action::Administer).await?;
        let now = self.now();
        let store = &self.deps.store;
        if !store.revoke_playground_invite(&sid, invite, now).await? {
            return Err(AppError::NotFound);
        }
        store
            .audit(
                now,
                &ctx.user.label(),
                "playground.invite.revoke",
                Some(sid.as_str()),
                Some(invite),
            )
            .await
    }

    /// `GET /api/guest-invites/{code}` (GST-002): what a live link lets its holder into, for the
    /// join page; needs no session, and counts against the client's guest attempts.
    ///
    /// # Errors
    /// `Capacity` past [`GUEST_ATTEMPTS_PER_MINUTE`]; `NotFound` for a dead link.
    pub async fn peek_guest_invite(
        &self,
        code: &str,
        client_key: &str,
    ) -> AppResult<GuestInvitePeek> {
        self.count_guest_attempt(client_key)?;
        let (invite, sandbox) = self.live_link(code).await?;
        Ok(GuestInvitePeek {
            playground: sandbox.slug.clone(),
            invited_by: self.person_label(&invite.created_by).await,
            expires_at: rfc3339(invite.expires_at),
        })
    }

    /// `POST /api/guest-invites/{code}/join` (GST-002): a new guest named `name` and their browser
    /// session, which ends when the link does (or earlier, at the session lifetime). Answers the
    /// session and the playground's slug.
    ///
    /// # Errors
    /// `Capacity` past [`GUEST_ATTEMPTS_PER_MINUTE`]; `Invalid` name; `NotFound` for a dead link;
    /// `Conflict` when the link let in [`MAX_GUESTS_PER_LINK`].
    pub async fn join_as_guest(
        &self,
        code: &str,
        name: &str,
        client_key: &str,
    ) -> AppResult<(NewSession, String)> {
        self.count_guest_attempt(client_key)?;
        let name = guest_name(name)?;
        let (invite, sandbox) = self.live_link(code).await?;
        let now = self.now();
        let user = UserRecord {
            id: UserId::parse(&self.deps.crypto.new_id()).map_err(AppError::internal)?,
            github_login: None,
            github_id: None,
            display_name: name,
            avatar_url: None,
            is_superadmin: false,
            is_instance_admin: false,
            password_hash: None,
            created_at: now,
            disabled_at: None,
        };
        let store = &self.deps.store;
        match store
            .insert_playground_guest(&invite.id, &user, now, MAX_GUESTS_PER_LINK)
            .await?
        {
            GuestJoin::Joined => {}
            GuestJoin::Dead => return Err(AppError::NotFound),
            GuestJoin::Full => {
                return Err(AppError::Conflict(format!(
                    "this link already let in {MAX_GUESTS_PER_LINK} guests; ask for a new one"
                )));
            }
        }
        store
            .audit(
                now,
                &user.label(),
                "playground.guest.join",
                Some(sandbox.id.as_str()),
                Some(&invite.id),
            )
            .await?;
        let session = self.new_session_until(&user, invite.expires_at).await?;
        Ok((session, sandbox.slug))
    }

    fn count_guest_attempt(&self, client_key: &str) -> AppResult<()> {
        count_client_attempt(
            &self.guest_attempts,
            client_key,
            self.now(),
            GUEST_ATTEMPTS_PER_MINUTE,
            "too many guest sign-ins; wait a minute",
        )
    }

    /// The live link with `code` and its playground, still a playground and not removed.
    async fn live_link(&self, code: &str) -> AppResult<(PlaygroundInviteRecord, SandboxRecord)> {
        let store = &self.deps.store;
        let hash = self.deps.crypto.sha256_hex(code.trim().as_bytes());
        let invite = store
            .playground_invite_by_code(&hash)
            .await?
            .filter(|i| i.live_at(self.now()))
            .ok_or(AppError::NotFound)?;
        let sandbox = store
            .sandbox(&invite.sandbox_id)
            .await?
            .filter(|s| s.removed_at.is_none() && s.is_playground())
            .ok_or(AppError::NotFound)?;
        Ok((invite, sandbox))
    }

    async fn invite_view(
        &self,
        invite: &PlaygroundInviteRecord,
        guests: &[PlaygroundGuestRecord],
    ) -> GuestInviteView {
        GuestInviteView {
            id: invite.id.clone(),
            created_by: self.person_label(&invite.created_by).await,
            created_at: rfc3339(invite.created_at),
            expires_at: rfc3339(invite.expires_at),
            guests: guests
                .iter()
                .filter(|g| g.invite_id == invite.id)
                .map(|g| GuestView {
                    name: g.display_name.clone(),
                    joined_at: rfc3339(g.joined_at),
                })
                .collect(),
        }
    }

    /// How a person is named to others: [`UserRecord::label`], or "someone" for an account that
    /// cannot be read.
    async fn person_label(&self, user: &UserId) -> String {
        match self.deps.store.user(user).await {
            Ok(Some(user)) => user.label(),
            _ => "someone".into(),
        }
    }
}
