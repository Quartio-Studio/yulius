//! Injected time values.

/// Milliseconds since the Unix epoch, supplied by the caller's clock.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Timestamp(pub i64);

impl Timestamp {
    /// This timestamp plus `millis` milliseconds (saturating).
    #[must_use]
    pub fn plus_millis(self, millis: i64) -> Self {
        Self(self.0.saturating_add(millis))
    }

    /// Milliseconds from `earlier` to `self` (negative when `earlier` is later).
    #[must_use]
    pub fn millis_since(self, earlier: Self) -> i64 {
        self.0.saturating_sub(earlier.0)
    }
}

/// Parses an RFC 3339 date-time, `YYYY-MM-DD(T|t| )HH:MM:SS[.fraction](Z|z|±HH:MM)`, into a
/// [`Timestamp`]: every field padded, the day within its month, a fraction cut to milliseconds,
/// a leap second read as `:59`; `None` for anything else. The one reader of every adapter
/// (transcripts, GitHub, imports; SPEC-483 slice 31).
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
    let date = crate::cron::CivilDate::new(
        num(0..4)?,
        u8::try_from(num(5..7)?).ok()?,
        u8::try_from(num(8..10)?).ok()?,
    )?;
    let (hour, minute, second) = (num(11..13)?, num(14..16)?, num(17..19)?);
    if hour > 23 || minute > 59 || second > 60 {
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
    let seconds = date.days_since_epoch() * 86_400 + hour * 3_600 + minute * 60 + second.min(59)
        - offset_minutes * 60;
    Some(Timestamp(seconds.checked_mul(1_000)?.checked_add(millis)?))
}

/// Formats a timestamp as `YYYY-MM-DDTHH:MM:SS.mmmZ` (UTC), which [`parse_rfc3339`] reads back for
/// the years 0000 to 9999 (a clock's; others print with more or fewer digits).
#[must_use]
pub fn format_rfc3339(timestamp: Timestamp) -> String {
    let millis = timestamp.0.rem_euclid(1_000);
    let seconds = timestamp.0.div_euclid(1_000);
    let of_day = seconds.rem_euclid(86_400);
    let Some(date) = crate::cron::CivilDate::from_days(seconds.div_euclid(86_400)) else {
        return String::new();
    };
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.{millis:03}Z",
        date.year(),
        date.month(),
        date.day(),
        of_day / 3_600,
        of_day % 3_600 / 60,
        of_day % 60
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// SPEC-483 slice 31 (QC3): the one RFC 3339 reader, with the inputs the four copies it
    /// replaces disagreed on.
    #[test]
    fn one_rfc3339_reader_takes_the_same_inputs_everywhere() {
        let at = |text| parse_rfc3339(text).map(|t| t.0);
        assert_eq!(at("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(at("2026-10-07T09:12:00.123Z"), Some(1_791_364_320_123));
        assert_eq!(
            at("2026-10-07T11:12:00.123456+02:00"),
            Some(1_791_364_320_123)
        );
        assert_eq!(at("2026-10-07T09:12:00.5Z"), Some(1_791_364_320_500));
        assert_eq!(at("1969-12-31T23:59:59.999Z"), Some(-1));
        // A space or a lower-case `t` separates date and time, and `z` is UTC.
        assert_eq!(at("2026-10-07 09:12:00z"), Some(1_791_364_320_000));
        assert_eq!(at("2026-10-07t09:12:00Z"), Some(1_791_364_320_000));
        // A leap second reads as :59.
        assert_eq!(at("2026-10-07T09:12:60Z"), Some(1_791_364_379_000));
        for bad in [
            "",
            "2026-10-07",
            "2026-13-07T09:12:00Z",
            "2026-02-30T09:12:00Z",
            "2026-02-31T09:12:00Z",
            "2026-10-07T24:00:00Z",
            "2026-10-07T09:12:00",
            "2026-10-07T09:12:00.Z",
            "2026-10-07T09:12:00+0200",
            "2026-10-07T09:12:00+24:00",
            "2026-10-07X09:12:00Z",
            "2026-10-07T09:12:00Zjunk",
            "2026-1-7T9:12:00Z",
            // Long enough for every position, one field unpadded.
            "2026-1-07T09:12:00.0Z",
            "2026-10-07T9:12:00.00Z",
            "+026-10-07T09:12:00Z",
        ] {
            assert_eq!(parse_rfc3339(bad), None, "{bad}");
        }
    }

    #[test]
    fn rfc3339_text_round_trips() {
        for millis in [0, 1_791_364_320_123, 951_782_400_000, -1, 4_102_444_799_999] {
            let text = format_rfc3339(Timestamp(millis));
            assert_eq!(parse_rfc3339(&text), Some(Timestamp(millis)), "{text}");
        }
        assert_eq!(
            format_rfc3339(Timestamp(1_791_364_320_123)),
            "2026-10-07T09:12:00.123Z"
        );
    }
}
