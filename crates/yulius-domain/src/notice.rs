//! A notice's key and expiry (SPEC-624, ADR-0462): what makes a newer notice replace an older one
//! (newest wins per project, recipient and key) and when a notice goes away by itself. A notice
//! that waits for a person's decision never expires.

use crate::time::{Timestamp, parse_rfc3339};

/// Longest key (NTC-001).
pub const KEY_MAX: usize = 120;
const MINUTE_MS: i64 = 60_000;
const HOUR_MS: i64 = 60 * MINUTE_MS;
const DAY_MS: i64 = 24 * HOUR_MS;
/// Longest expiry a notice may be given (NTC-002): 90 days.
pub const EXPIRY_MAX_MS: i64 = 90 * DAY_MS;
/// Default for a notice that reports something (NTC-002, the spec's table).
pub const REPORT_MS: i64 = DAY_MS;
/// Default for good news and the digest.
pub const GOOD_NEWS_MS: i64 = 3 * DAY_MS;
/// Default for a failure.
pub const FAILURE_MS: i64 = 7 * DAY_MS;

/// How a notice reads: its colour in the bell.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoticeTone {
    /// Something to know.
    Info,
    /// Good news.
    Ok,
    /// Something to look at.
    Warn,
    /// A failure.
    Error,
}

impl NoticeTone {
    /// Its wire name.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Info => "info",
            Self::Ok => "ok",
            Self::Warn => "warn",
            Self::Error => "error",
        }
    }

    /// The tone named `text`; an unknown name reads as [`NoticeTone::Info`].
    #[must_use]
    pub fn parse(text: &str) -> Self {
        match text {
            "ok" => Self::Ok,
            "warn" => Self::Warn,
            "error" => Self::Error,
            _ => Self::Info,
        }
    }
}

/// A notice's key (NTC-001): 1 to [`KEY_MAX`] characters of ASCII letters, digits and
/// `:._/@#-`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct NoticeKey(String);

impl NoticeKey {
    /// Parses `text` as a key.
    ///
    /// # Errors
    /// The rule in words, for an empty, long or otherwise malformed key.
    pub fn parse(text: &str) -> Result<Self, String> {
        if text.is_empty() || text.len() > KEY_MAX {
            return Err(format!("a notice key is 1 to {KEY_MAX} characters"));
        }
        if !text
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b":._/@#-".contains(&b))
        {
            return Err(
                "a notice key holds only ASCII letters, digits and the characters : . _ / @ # -"
                    .into(),
            );
        }
        Ok(Self(text.to_owned()))
    }

    /// The key of a task's state notices (NTC-008).
    #[must_use]
    pub fn task(task_id: &str) -> Self {
        Self::lossy("task:", task_id)
    }

    /// `prefix` followed by `rest`, its disallowed characters as `-`, cut to [`KEY_MAX`]: for
    /// Yulius's own keys built from names it does not control (a test's name).
    #[must_use]
    pub fn lossy(prefix: &str, rest: &str) -> Self {
        let mut key: String = prefix
            .chars()
            .chain(rest.chars())
            .map(|c| {
                if c.is_ascii_alphanumeric() || ":._/@#-".contains(c) {
                    c
                } else {
                    '-'
                }
            })
            .take(KEY_MAX)
            .collect();
        if key.is_empty() {
            key.push('-');
        }
        Self(key)
    }

    /// The key as text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for NoticeKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// When a notice goes away by itself (NTC-002).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Expiry {
    /// Never by time.
    Never,
    /// This long after it is posted.
    After(i64),
    /// At this time.
    At(Timestamp),
}

impl Expiry {
    /// Parses `never`, `<n>m`, `<n>h`, `<n>d` (1 minute to 90 days) or an RFC 3339 time.
    ///
    /// # Errors
    /// The accepted forms in words.
    pub fn parse(text: &str) -> Result<Self, String> {
        let text = text.trim();
        if text.eq_ignore_ascii_case("never") {
            return Ok(Self::Never);
        }
        if let Some(at) = parse_rfc3339(text) {
            return Ok(Self::At(at));
        }
        let wrong = || {
            format!(
                "an expiry is `never`, a duration such as `30m`, `2h` or `3d` (1 minute to 90 \
                 days), or an RFC 3339 time; `{text}` is none of these"
            )
        };
        let (digits, unit) = text.split_at(text.len().saturating_sub(1));
        let unit_ms = match unit {
            "m" => MINUTE_MS,
            "h" => HOUR_MS,
            "d" => DAY_MS,
            _ => return Err(wrong()),
        };
        if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
            return Err(wrong());
        }
        let ms = digits
            .parse::<i64>()
            .ok()
            .and_then(|n| n.checked_mul(unit_ms))
            .ok_or_else(wrong)?;
        if !(MINUTE_MS..=EXPIRY_MAX_MS).contains(&ms) {
            return Err(wrong());
        }
        Ok(Self::After(ms))
    }

    /// The absolute expiry of a notice posted at `now`: `None` for never.
    ///
    /// # Errors
    /// A time that is not in the future, or more than 90 days ahead.
    pub fn resolve(self, now: Timestamp) -> Result<Option<Timestamp>, String> {
        match self {
            Self::Never => Ok(None),
            Self::After(ms) => Ok(Some(now.plus_millis(ms))),
            Self::At(at) if at <= now => Err("a notice's expiry must be in the future".into()),
            Self::At(at) if at.millis_since(now) > EXPIRY_MAX_MS => {
                Err("a notice's expiry is at most 90 days ahead".into())
            }
            Self::At(at) => Ok(Some(at)),
        }
    }
}

/// The expiry a posted notice gets (NTC-002, NTC-005): a decision never expires, and posting
/// one with an expiry is refused rather than shortened; otherwise the given expiry or `default`.
///
/// # Errors
/// A decision with an expiry, or an expiry [`Expiry::resolve`] refuses.
pub fn notice_expiry(
    decision: bool,
    given: Option<Expiry>,
    default: Expiry,
    now: Timestamp,
) -> Result<Option<Timestamp>, String> {
    match (decision, given) {
        (true, None | Some(Expiry::Never)) => Ok(None),
        (true, Some(_)) => Err(
            "a notice that waits for a person's decision never expires: post it without an \
             expiry and remove it once it is decided"
                .into(),
        ),
        (false, given) => given.unwrap_or(default).resolve(now),
    }
}

/// Whether a notice with `expires_at` is still shown at `now` (NTC-004).
#[must_use]
pub fn live(expires_at: Option<Timestamp>, now: Timestamp) -> bool {
    expires_at.is_none_or(|at| at > now)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_key_is_short_and_plain() {
        assert!(NoticeKey::parse("watch:nightly-build").is_ok());
        assert!(NoticeKey::parse("gate:quick::a/b@c#1.x_y").is_ok());
        assert!(NoticeKey::parse(&"k".repeat(KEY_MAX)).is_ok());
        for bad in ["", "with space", "ünïcode", "new\nline"] {
            assert!(NoticeKey::parse(bad).is_err(), "{bad:?}");
        }
        assert!(NoticeKey::parse(&"k".repeat(KEY_MAX + 1)).is_err());
    }

    #[test]
    fn a_lossy_key_keeps_its_prefix_and_bound() {
        assert_eq!(
            NoticeKey::lossy("gate:", "suite::a test (x)").as_str(),
            "gate:suite::a-test--x-"
        );
        assert_eq!(
            NoticeKey::lossy("gate:", &"t".repeat(300)).as_str().len(),
            KEY_MAX
        );
        assert!(NoticeKey::parse(NoticeKey::lossy("gate:", "ą b").as_str()).is_ok());
    }

    #[test]
    fn an_expiry_is_never_a_duration_or_a_time() {
        assert_eq!(Expiry::parse("never"), Ok(Expiry::Never));
        assert_eq!(Expiry::parse("30m"), Ok(Expiry::After(30 * MINUTE_MS)));
        assert_eq!(Expiry::parse("2h"), Ok(Expiry::After(2 * HOUR_MS)));
        assert_eq!(Expiry::parse("3d"), Ok(Expiry::After(3 * DAY_MS)));
        assert_eq!(Expiry::parse("90d"), Ok(Expiry::After(EXPIRY_MAX_MS)));
        assert!(matches!(
            Expiry::parse("2026-11-23T10:00:00Z"),
            Ok(Expiry::At(_))
        ));
        for bad in ["0m", "91d", "5w", "m", "-3h", "1.5h", "", "soon"] {
            assert!(Expiry::parse(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn an_expiry_resolves_against_the_clock() {
        let now = Timestamp(1_000_000);
        assert_eq!(Expiry::Never.resolve(now), Ok(None));
        assert_eq!(
            Expiry::After(MINUTE_MS).resolve(now),
            Ok(Some(Timestamp(1_000_000 + MINUTE_MS)))
        );
        assert!(Expiry::At(now).resolve(now).is_err(), "not in the future");
        assert!(
            Expiry::At(now.plus_millis(EXPIRY_MAX_MS + 1))
                .resolve(now)
                .is_err()
        );
        assert_eq!(
            Expiry::At(now.plus_millis(5)).resolve(now),
            Ok(Some(now.plus_millis(5)))
        );
    }

    #[test]
    fn a_decision_never_expires_and_refuses_an_expiry() {
        let now = Timestamp(0);
        let day = Expiry::After(REPORT_MS);
        assert_eq!(notice_expiry(true, None, day, now), Ok(None));
        assert_eq!(notice_expiry(true, Some(Expiry::Never), day, now), Ok(None));
        assert!(notice_expiry(true, Some(Expiry::After(30 * MINUTE_MS)), day, now).is_err());
        assert_eq!(
            notice_expiry(false, None, day, now),
            Ok(Some(Timestamp(REPORT_MS)))
        );
        assert_eq!(
            notice_expiry(false, Some(Expiry::Never), day, now),
            Ok(None)
        );
    }

    #[test]
    fn a_notice_is_live_until_its_time() {
        assert!(live(None, Timestamp(i64::MAX)));
        assert!(live(Some(Timestamp(10)), Timestamp(9)));
        assert!(!live(Some(Timestamp(10)), Timestamp(10)));
    }
}
