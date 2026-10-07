//! Minimal RFC 3339 conversion for GitHub timestamps (`2026-10-07T12:00:00Z`).

use yulius_domain::time::Timestamp;

/// Days since 1970-01-01 for a proleptic Gregorian date (Howard Hinnant's algorithm).
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let year_of_era = year - era * 400;
    let month_index = (month + 9) % 12;
    let day_of_year = (153 * month_index + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

#[cfg(any(test, feature = "fake"))]
fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let day_of_era = z - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
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

fn digits(text: &str) -> Option<i64> {
    if text.is_empty() || !text.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    text.parse().ok()
}

/// Parses `YYYY-MM-DDTHH:MM:SS[.fraction](Z|±HH:MM)`.
pub(crate) fn parse_rfc3339(text: &str) -> Option<Timestamp> {
    let (date, time) = text.split_once(['T', 't', ' '])?;
    let mut date_parts = date.split('-');
    let year = digits(date_parts.next()?)?;
    let month = digits(date_parts.next()?)?;
    let day = digits(date_parts.next()?)?;
    if date_parts.next().is_some() || !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    let (clock, offset_minutes) = if let Some(clock) = time.strip_suffix(['Z', 'z']) {
        (clock, 0)
    } else {
        let sign_at = time.rfind(['+', '-'])?;
        let (clock, offset) = time.split_at(sign_at);
        let sign = if offset.starts_with('-') { -1 } else { 1 };
        let (hours, minutes) = offset[1..].split_once(':')?;
        (clock, sign * (digits(hours)? * 60 + digits(minutes)?))
    };
    let clock = clock.split('.').next()?;
    let mut clock_parts = clock.split(':');
    let hour = digits(clock_parts.next()?)?;
    let minute = digits(clock_parts.next()?)?;
    let second = digits(clock_parts.next()?)?;
    if clock_parts.next().is_some() || hour > 23 || minute > 59 || second > 60 {
        return None;
    }
    let seconds = days_from_civil(year, month, day) * 86_400 + hour * 3600 + minute * 60 + second
        - offset_minutes * 60;
    Some(Timestamp(seconds.checked_mul(1000)?))
}

/// Formats Unix seconds as `YYYY-MM-DDTHH:MM:SSZ`.
#[cfg(any(test, feature = "fake"))]
pub(crate) fn format_rfc3339(unix_seconds: i64) -> String {
    let days = unix_seconds.div_euclid(86_400);
    let rem = unix_seconds.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        rem / 3600,
        (rem % 3600) / 60,
        rem % 60
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_known_instants() {
        assert_eq!(
            parse_rfc3339("2016-07-11T22:14:10Z"),
            Some(Timestamp(1_468_275_250_000))
        );
        assert_eq!(parse_rfc3339("1970-01-01T00:00:00Z"), Some(Timestamp(0)));
        assert_eq!(
            parse_rfc3339("2024-02-29T12:00:00.123+02:00"),
            Some(Timestamp(1_709_200_800_000))
        );
        assert_eq!(format_rfc3339(1_468_275_250), "2016-07-11T22:14:10Z");
        assert_eq!(format_rfc3339(1_709_208_000), "2024-02-29T12:00:00Z");
        for seconds in [0, 951_782_400, 1_767_225_599, 4_102_444_800] {
            assert_eq!(
                parse_rfc3339(&format_rfc3339(seconds)),
                Some(Timestamp(seconds * 1000))
            );
        }
    }

    #[test]
    fn rejects_garbage() {
        for bad in [
            "",
            "2016-13-01T00:00:00Z",
            "yesterday",
            "2016-07-11",
            "2016-07-11T25:00:00Z",
        ] {
            assert_eq!(parse_rfc3339(bad), None, "{bad}");
        }
    }
}
