//! Notices for people (SPEC-624, ADR-0462): each has a key (newest wins per project, recipient
//! and key) and an expiry (hidden once it passes; a decision never expires).

use serde::{Deserialize, Serialize};

/// `GET /api/me/notices`, the `notice.posted` event and `notices_list`: one live notice.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NoticeView {
    /// Its id; a replacement has a new one.
    pub id: String,
    /// Its project; `None`: instance-wide.
    #[serde(default)]
    pub sandbox_id: Option<String>,
    /// Its key, unique per project and recipient.
    pub key: String,
    /// The channel choice it falls under: `needs_you`, `done`, `failed` or `account`.
    pub category: String,
    /// `info`, `ok`, `warn` or `error`.
    pub tone: String,
    /// One line.
    pub title: String,
    /// More, if any.
    #[serde(default)]
    pub detail: Option<String>,
    /// A path in Yulius it leads to, if any.
    #[serde(default)]
    pub link: Option<String>,
    /// It waits for a person's decision: it never expires.
    #[serde(default)]
    pub decision: bool,
    /// Addressed to this person only.
    #[serde(default)]
    pub personal: bool,
    /// When it was posted (RFC 3339).
    pub created_at: String,
    /// When it expires (RFC 3339); `None`: never.
    #[serde(default)]
    pub expires_at: Option<String>,
}
