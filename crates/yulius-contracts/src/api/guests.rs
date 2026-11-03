//! A playground's guests (SPEC-410, ADR-0260): expiring links that let teammates without GitHub
//! try one playground.

use serde::{Deserialize, Serialize};

/// `POST /api/sandboxes/{sid}/guest-invites`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GuestInviteCreate {
    /// How long the link and its guests' access last: 1, 7 (the default) or 30 days.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub days: Option<u32>,
}

/// A guest a link let in.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GuestView {
    /// The name they gave.
    pub name: String,
    /// When they joined.
    pub joined_at: String,
}

/// A live guest link (never its code).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GuestInviteView {
    /// Its id.
    pub id: String,
    /// Who made it.
    pub created_by: String,
    /// When.
    pub created_at: String,
    /// When it and its guests' access end.
    pub expires_at: String,
    /// The guests it let in, oldest first.
    pub guests: Vec<GuestView>,
}

/// `POST /api/sandboxes/{sid}/guest-invites` (201): the new link, whose URL is shown only now.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GuestInviteCreated {
    /// The link.
    pub invite: GuestInviteView,
    /// `<public_url>/join?code=<code>`: whoever holds it joins as a guest until it ends.
    pub url: String,
}

/// `GET /api/sandboxes/{sid}/guest-invites`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GuestInvites {
    /// The live links, oldest first.
    pub invites: Vec<GuestInviteView>,
    /// How many may be live at once.
    pub max_live: usize,
    /// How many guests one link lets in.
    pub max_guests: usize,
}

/// `GET /api/guest-invites/{code}`: what a live link lets its holder into.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GuestInvitePeek {
    /// The playground's name (its slug).
    pub playground: String,
    /// Who made the link.
    pub invited_by: String,
    /// When it ends.
    pub expires_at: String,
}

/// `POST /api/guest-invites/{code}/join`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GuestJoinRequest {
    /// The guest's name: 1 to 60 characters, no control character.
    pub name: String,
}
