//! The end-of-day digest (SPEC-512 part 3, #1413, ADR-0380): the instance's setting and a
//! person's digest.

use serde::{Deserialize, Serialize};

/// `GET`/`PUT /api/admin/digest` (DIG-001): off by default.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DigestSetting {
    /// Whether the digest is sent.
    pub enabled: bool,
    /// When, `HH:MM` in UTC.
    pub at: String,
    /// Tasks quiet this long that wait for nobody are parked by the digest (1 to 720 hours);
    /// `None`: the digest parks nothing.
    #[serde(default)]
    pub park_idle_hours: Option<u32>,
}

impl Default for DigestSetting {
    fn default() -> Self {
        Self {
            enabled: false,
            at: "18:00".into(),
            park_idle_hours: None,
        }
    }
}

/// One task in a digest (DIG-004), its texts redacted (DIG-006).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DigestItem {
    /// The project's repository name.
    pub project: String,
    /// The task's number.
    pub number: u32,
    /// The task's title.
    pub title: String,
    /// The request (waiting), how it ended and what its last message left (parked), or nothing.
    pub detail: Option<String>,
    /// The task's page.
    pub link: String,
    /// Its notice key (`task:<task id>`, SPEC-624); absent in digests stored before.
    #[serde(default)]
    pub key: Option<String>,
}

/// `GET /api/me/digest` and the `digest_ready` event (DIG-005): one person's digest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DigestView {
    /// Its id.
    pub id: String,
    /// When it was made (RFC 3339).
    pub created_at: String,
    /// Tasks waiting for the person, first.
    pub waiting: Vec<DigestItem>,
    /// Tasks parked since the last digest.
    pub parked: Vec<DigestItem>,
    /// How many of the person's tasks still work.
    pub working_count: u32,
    /// Up to ten of them.
    pub working: Vec<DigestItem>,
    /// The person's live notices, newest first, at most twenty (SPEC-624 NTC-009); expired and
    /// dismissed ones are left out. Absent in digests stored before.
    #[serde(default)]
    pub notices: Vec<crate::api::notices::NoticeView>,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// SPEC-624 AC-8: a digest stored before notices had keys still loads.
    #[test]
    fn a_digest_stored_before_keys_still_loads() -> Result<(), serde_json::Error> {
        let old: DigestView = serde_json::from_str(
            r#"{"id":"dg_1","created_at":"2026-11-19T18:00:00Z","waiting":[{"project":"app",
            "number":1,"title":"T","detail":null,"link":"https://y.test/s/app/t/1"}],
            "parked":[],"working_count":0,"working":[]}"#,
        )?;
        assert!(old.notices.is_empty());
        assert_eq!(old.waiting[0].key, None);
        Ok(())
    }
}
