//! Minimal RFC 3339 parsing and UTC formatting for transcript timestamps.

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

/// Formats Unix milliseconds as RFC 3339 UTC.
pub(crate) fn format_millis(millis: i64) -> String {
    let days = millis.div_euclid(86_400_000);
    let in_day = millis.rem_euclid(86_400_000);
    let (year, month, day) = civil_from_days(days);
    let hour = in_day / 3_600_000;
    let minute = in_day / 60_000 % 60;
    let second = in_day / 1000 % 60;
    let milli = in_day % 1000;
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}.{milli:03}Z")
}

/// Parses an RFC 3339 date-time (`YYYY-MM-DDTHH:MM:SS[.frac](Z|±HH:MM)`) into Unix milliseconds.
/// Fractions beyond milliseconds are truncated; a leap second is clamped to `:59`.
pub(crate) fn parse_rfc3339_millis(text: &str) -> Option<i64> {
    let bytes = text.as_bytes();
    if bytes.len() < 20 {
        return None;
    }
    let year = digits(bytes, 0, 4)?;
    let month = digits(bytes, 5, 2)?;
    let day = digits(bytes, 8, 2)?;
    let hour = digits(bytes, 11, 2)?;
    let minute = digits(bytes, 14, 2)?;
    let second = digits(bytes, 17, 2)?.min(59);
    if bytes[4] != b'-'
        || bytes[7] != b'-'
        || !matches!(bytes[10], b'T' | b't' | b' ')
        || bytes[13] != b':'
        || bytes[16] != b':'
        || !(1..=12).contains(&month)
        || day == 0
        || day > days_in_month(year, month)
        || hour > 23
        || minute > 59
    {
        return None;
    }
    let mut index = 19;
    let mut millis = 0;
    if bytes.get(index) == Some(&b'.') {
        index += 1;
        let start = index;
        while bytes.get(index).is_some_and(u8::is_ascii_digit) {
            if index - start < 3 {
                millis = millis * 10 + i64::from(bytes[index] - b'0');
            }
            index += 1;
        }
        if index == start {
            return None;
        }
        for _ in (index - start)..3 {
            millis *= 10;
        }
    }
    let offset_minutes = match bytes.get(index)? {
        b'Z' | b'z' if index + 1 == bytes.len() => 0,
        sign @ (b'+' | b'-') if index + 6 == bytes.len() && bytes[index + 3] == b':' => {
            let hours = digits(bytes, index + 1, 2)?;
            let minutes = digits(bytes, index + 4, 2)?;
            if hours > 23 || minutes > 59 {
                return None;
            }
            let total = hours * 60 + minutes;
            if *sign == b'-' { -total } else { total }
        }
        _ => return None,
    };
    let days = days_from_civil(year, month, day);
    let seconds = days * 86_400 + hour * 3600 + minute * 60 + second - offset_minutes * 60;
    Some(seconds * 1000 + millis)
}

fn digits(bytes: &[u8], start: usize, len: usize) -> Option<i64> {
    let slice = bytes.get(start..start + len)?;
    slice.iter().try_fold(0_i64, |value, b| {
        b.is_ascii_digit()
            .then(|| value * 10 + i64::from(b - b'0'))
    })
}

fn is_leap(year: i64) -> bool {
    (year % 4 == 0 && year % 100 != 0) || year % 400 == 0
}

fn days_in_month(year: i64, month: i64) -> i64 {
    match month {
        2 if is_leap(year) => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

/// Days since 1970-01-01 for a proleptic Gregorian date (H. Hinnant's algorithm).
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let year_of_era = year.rem_euclid(400);
    let shifted_month = if month > 2 { month - 3 } else { month + 9 };
    let day_of_year = (153 * shifted_month + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

/// Inverse of [`days_from_civil`].
fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let shifted = days + 719_468;
    let era = shifted.div_euclid(146_097);
    let day_of_era = shifted.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let shifted_month = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * shifted_month + 2) / 5 + 1;
    let month = if shifted_month < 10 {
        shifted_month + 3
    } else {
        shifted_month - 9
    };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    (year, month, day)
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
