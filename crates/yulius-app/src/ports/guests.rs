//! A playground's guests persistence port (SPEC-410, ADR-0260): the expiring links a playground's
//! admins create, kept by the hash of their code, and the guest accounts each link let in.

use async_trait::async_trait;
use yulius_domain::ids::{SandboxId, UserId};
use yulius_domain::time::Timestamp;

use crate::error::AppResult;
use crate::model::UserRecord;

/// A guest link as stored (never its code).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlaygroundInviteRecord {
    /// Its id.
    pub id: String,
    /// The playground it lets guests into.
    pub sandbox_id: SandboxId,
    /// SHA-256 of its code (hex).
    pub code_hash: String,
    /// Who made it.
    pub created_by: UserId,
    /// When.
    pub created_at: Timestamp,
    /// When it and its guests' access end.
    pub expires_at: Timestamp,
    /// When an admin revoked it.
    pub revoked_at: Option<Timestamp>,
}

impl PlaygroundInviteRecord {
    /// Whether it still admits and keeps its guests in at `now`: not revoked, not expired.
    #[must_use]
    pub fn live_at(&self, now: Timestamp) -> bool {
        self.revoked_at.is_none() && now < self.expires_at
    }
}

/// A guest a link let in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlaygroundGuestRecord {
    /// The guest's account.
    pub user_id: UserId,
    /// The name they gave.
    pub display_name: String,
    /// The link they came in by.
    pub invite_id: String,
    /// When.
    pub joined_at: Timestamp,
}

/// What adding a guest did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GuestJoin {
    /// The guest account exists, tied to the link.
    Joined,
    /// The link already let in its bound of guests: nothing was written.
    Full,
    /// The link is no longer live (revoked or expired meanwhile): nothing was written.
    Dead,
}

/// A playground's guests and links (SPEC-410 GST-001..005).
#[async_trait]
pub trait PlaygroundGuestStore: Send + Sync {
    /// Stores `invite` unless its playground already has `max_live` links live at its
    /// `created_at`; answers whether it was stored.
    async fn insert_playground_invite(
        &self,
        invite: &PlaygroundInviteRecord,
        max_live: usize,
    ) -> AppResult<bool>;
    /// The link with this code hash, live or not.
    async fn playground_invite_by_code(
        &self,
        code_hash: &str,
    ) -> AppResult<Option<PlaygroundInviteRecord>>;
    /// The links of `sandbox` live at `now`, oldest first.
    async fn live_playground_invites(
        &self,
        sandbox: &SandboxId,
        now: Timestamp,
    ) -> AppResult<Vec<PlaygroundInviteRecord>>;
    /// The guests of the links of `sandbox`, oldest first.
    async fn playground_guests(&self, sandbox: &SandboxId)
    -> AppResult<Vec<PlaygroundGuestRecord>>;
    /// Creates `user` as a guest of `invite_id` in one transaction, unless the link is no longer
    /// live at `at` or already let in `max_guests`.
    async fn insert_playground_guest(
        &self,
        invite_id: &str,
        user: &UserRecord,
        at: Timestamp,
        max_guests: usize,
    ) -> AppResult<GuestJoin>;
    /// The link `user` came in by, when they are a guest.
    async fn guest_invite_of(&self, user: &UserId) -> AppResult<Option<PlaygroundInviteRecord>>;
    /// Revokes the live link `invite_id` of `sandbox` and, in the same transaction, every browser
    /// session and API token of the guests it let in; answers whether a live link was revoked.
    async fn revoke_playground_invite(
        &self,
        sandbox: &SandboxId,
        invite_id: &str,
        at: Timestamp,
    ) -> AppResult<bool>;
    /// The guests `invite_id` let in.
    async fn invite_guest_ids(&self, invite_id: &str) -> AppResult<Vec<UserId>>;
    /// At most `limit` links revoked or expired by `now` whose guests' work was not ended yet,
    /// oldest first.
    async fn unended_expired_invites(
        &self,
        now: Timestamp,
        limit: usize,
    ) -> AppResult<Vec<PlaygroundInviteRecord>>;
    /// Records that what the guests of `invite_id` ran was stopped.
    async fn mark_invite_ended(&self, invite_id: &str, at: Timestamp) -> AppResult<()>;
    /// Whether a guest ever joined `sandbox` (also after it moved to GitHub: what they left stays).
    async fn playground_has_guests(&self, sandbox: &SandboxId) -> AppResult<bool>;
}
