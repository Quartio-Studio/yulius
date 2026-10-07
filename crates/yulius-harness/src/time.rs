//! RFC 3339 timestamps and epoch conversions used by the provider transcripts.

use yulius_domain::time::Timestamp;

/// Parses `YYYY-MM-DDTHH:MM:SS[.fraction](Z|±HH:MM)` into milliseconds since the Unix epoch.
/// Fractions beyond milliseconds are truncated. Returns `None` for anything else.
#[must_use]
pub fn parse_rfc3339(text: &str) -> Option<Timestamp> {
    let b = text.as_bytes();
    if b.len() < 20 {
        return None;
    }
    let num = |range: std::ops::Range<usize>| -> Option<i64> {
        let digits = b.get(range)?;
        if digits.is_empty() || !digits.iter().all(u8::is_ascii_digit) {
            return None;
        }
        digits.iter().try_fold(0i64, |acc, d| {
            acc.checked_mul(10)?.checked_add(i64::from(d - b'0'))
        })
    };
    let separators = [(4, b'-'), (7, b'-'), (13, b':'), (16, b':')];
    if separators.iter().any(|&(i, c)| b[i] != c) || !matches!(b[10], b'T' | b't' | b' ') {
        return None;
    }
    let (year, month, day) = (num(0..4)?, num(5..7)?, num(8..10)?);
    let (hour, minute, second) = (num(11..13)?, num(14..16)?, num(17..19)?);
    if !(1..=12).contains(&month)
        || day < 1
        || day > days_in_month(year, month)
        || hour > 23
        || minute > 59
        || second > 60
    {
        return None;
    }
    let mut index = 19;
    let mut millis = 0i64;
    if b[index] == b'.' {
        index += 1;
        let start = index;
        while index < b.len() && b[index].is_ascii_digit() {
            if index - start < 3 {
                millis = millis * 10 + i64::from(b[index] - b'0');
            }
            index += 1;
        }
        let digits = index - start;
        if digits == 0 {
            return None;
        }
        for _ in digits..3 {
            millis *= 10;
        }
    }
    let offset_minutes = match b.get(index..)? {
        [b'Z' | b'z'] => 0,
        [sign @ (b'+' | b'-'), _, _, b':', _, _] => {
            let hours = num(index + 1..index + 3)?;
            let minutes = num(index + 4..index + 6)?;
            if hours > 23 || minutes > 59 {
                return None;
            }
            let total = hours * 60 + minutes;
            if *sign == b'-' { -total } else { total }
        }
        _ => return None,
    };
    let days = days_from_civil(year, month, day);
    let seconds = days * 86_400 + hour * 3_600 + minute * 60 + second.min(59) - offset_minutes * 60;
    Some(Timestamp(seconds.checked_mul(1_000)?.checked_add(millis)?))
}

/// Formats a timestamp as `YYYY-MM-DDTHH:MM:SS.mmmZ` (UTC).
#[must_use]
pub fn format_rfc3339(timestamp: Timestamp) -> String {
    let (year, month, day, hour, minute, second, millis) = civil(timestamp);
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}.{millis:03}Z")
}

/// Converts Unix epoch seconds (as used by provider reset times) to a [`Timestamp`].
#[must_use]
pub fn from_epoch_seconds(seconds: i64) -> Option<Timestamp> {
    seconds.checked_mul(1_000).map(Timestamp)
}

/// UTC calendar fields of a timestamp: year, month, day, hour, minute, second, millisecond.
pub(crate) fn civil(timestamp: Timestamp) -> (i64, i64, i64, i64, i64, i64, i64) {
    let millis = timestamp.0.rem_euclid(1_000);
    let seconds = timestamp.0.div_euclid(1_000);
    let days = seconds.div_euclid(86_400);
    let of_day = seconds.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    (
        year,
        month,
        day,
        of_day / 3_600,
        of_day % 3_600 / 60,
        of_day % 60,
        millis,
    )
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

/// Days since 1970-01-01 of a proleptic Gregorian date (H. Hinnant's algorithm).
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let year_of_era = year.rem_euclid(400);
    let month_index = (month + 9) % 12;
    let day_of_year = (153 * month_index + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let day_of_era = z.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_index = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_index + 2) / 5 + 1;
    let month = if month_index < 10 {
        month_index + 3
    } else {
        month_index - 9
    };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_common_forms() {
        assert_eq!(parse_rfc3339("1970-01-01T00:00:00Z"), Some(Timestamp(0)));
        assert_eq!(
            parse_rfc3339("2026-10-07T09:12:00.123Z"),
            Some(Timestamp(1_791_364_320_123))
        );
        assert_eq!(
            parse_rfc3339("2026-10-07T11:12:00.123456+02:00"),
            Some(Timestamp(1_791_364_320_123))
        );
        assert_eq!(
            parse_rfc3339("2026-10-07T09:12:00.5Z"),
            Some(Timestamp(1_791_364_320_500))
        );
        assert_eq!(
            parse_rfc3339("1969-12-31T23:59:59.999Z"),
            Some(Timestamp(-1))
        );
    }

    #[test]
    fn rejects_invalid_text() {
        for bad in [
            "",
            "2026-10-07",
            "2026-13-07T09:12:00Z",
            "2026-02-30T09:12:00Z",
            "2026-10-07T24:00:00Z",
            "2026-10-07T09:12:00",
            "2026-10-07T09:12:00.Z",
            "2026-10-07T09:12:00+0200",
            "2026-10-07X09:12:00Z",
            "2026-10-07T09:12:00Zjunk",
        ] {
            assert_eq!(parse_rfc3339(bad), None, "{bad}");
        }
    }

    #[test]
    fn format_round_trips() {
        for millis in [0, 1_791_364_320_123, 951_782_400_000, -1, 4_102_444_799_999] {
            let text = format_rfc3339(Timestamp(millis));
            assert_eq!(parse_rfc3339(&text), Some(Timestamp(millis)), "{text}");
        }
        assert_eq!(
            format_rfc3339(Timestamp(1_791_364_320_123)),
            "2026-10-07T09:12:00.123Z"
        );
        assert_eq!(
            from_epoch_seconds(1_760_000_000),
            Some(Timestamp(1_760_000_000_000))
        );
        assert_eq!(from_epoch_seconds(i64::MAX), None);
    }
}
