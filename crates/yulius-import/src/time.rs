//! Transcript timestamps as Unix milliseconds and `SystemTime`; RFC 3339 is read and written by
//! `yulius_domain::time`.

use std::time::{SystemTime, UNIX_EPOCH};

/// Formats `time` as an RFC 3339 UTC timestamp with millisecond precision
/// (`2026-10-07T09:13:00.000Z`).
#[must_use]
pub fn format_rfc3339(time: SystemTime) -> String {
    format_millis(system_time_millis(time))
}

/// Milliseconds since the Unix epoch (negative before it, saturating).
pub(crate) fn system_time_millis(time: SystemTime) -> i64 {
    match time.duration_since(UNIX_EPOCH) {
        Ok(after) => i64::try_from(after.as_millis()).unwrap_or(i64::MAX),
        Err(before) => i64::try_from(before.duration().as_millis()).map_or(i64::MIN, |ms| -ms),
    }
}

/// The instant `millis` milliseconds after the Unix epoch.
#[cfg(test)]
pub(crate) fn millis_system_time(millis: i64) -> SystemTime {
    let magnitude = std::time::Duration::from_millis(millis.unsigned_abs());
    if millis >= 0 {
        UNIX_EPOCH + magnitude
    } else {
        UNIX_EPOCH - magnitude
    }
}

/// Formats Unix milliseconds as RFC 3339 UTC (the domain's one formatter).
pub(crate) fn format_millis(millis: i64) -> String {
    yulius_domain::time::format_rfc3339(yulius_domain::time::Timestamp(millis))
}

/// Parses an RFC 3339 date-time into Unix milliseconds (the domain's one reader, SPEC-483
/// slice 31).
pub(crate) fn parse_rfc3339_millis(text: &str) -> Option<i64> {
    yulius_domain::time::parse_rfc3339(text).map(|t| t.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_common_transcript_forms() {
        assert_eq!(parse_rfc3339_millis("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(
            parse_rfc3339_millis("2026-09-21T14:13:20.000Z"),
            Some(1_790_000_000_000)
        );
        assert_eq!(
            parse_rfc3339_millis("2026-09-21T14:13:20.123456Z"),
            Some(1_790_000_000_123)
        );
        assert_eq!(
            parse_rfc3339_millis("2026-09-21T16:13:20.5+02:00"),
            Some(1_790_000_000_500)
        );
        assert_eq!(
            parse_rfc3339_millis("2026-09-21T12:13:20-02:00"),
            Some(1_790_000_000_000)
        );
        assert_eq!(
            parse_rfc3339_millis("2024-02-29T00:00:00Z"),
            Some(1_709_164_800_000)
        );
    }

    #[test]
    fn rejects_malformed_values() {
        for bad in [
            "",
            "2026-09-21",
            "2026-09-21T14:13:20",
            "2026-13-01T00:00:00Z",
            "2025-02-29T00:00:00Z",
            "2026-09-21T24:00:00Z",
            "2026-09-21T14:13:20.Z",
            "2026-09-21T14:13:20Zjunk",
            "2026-09-21T14:13:20+0200",
            "２026-09-21T14:13:20Z",
        ] {
            assert_eq!(parse_rfc3339_millis(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn formats_and_round_trips() {
        assert_eq!(format_millis(0), "1970-01-01T00:00:00.000Z");
        assert_eq!(format_millis(1_790_000_000_123), "2026-09-21T14:13:20.123Z");
        assert_eq!(format_millis(-1), "1969-12-31T23:59:59.999Z");
        for millis in [0, 951_782_400_000, 1_709_164_800_000, 4_102_444_799_999] {
            assert_eq!(parse_rfc3339_millis(&format_millis(millis)), Some(millis));
        }
        let time = millis_system_time(1_790_000_000_000);
        assert_eq!(system_time_millis(time), 1_790_000_000_000);
        assert_eq!(format_rfc3339(time), "2026-09-21T14:13:20.000Z");
        assert_eq!(system_time_millis(millis_system_time(-5)), -5);
    }
}
