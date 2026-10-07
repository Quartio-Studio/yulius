//! Proleptic Gregorian civil-date arithmetic, implemented here without external crates.
//!
//! Day numbers count days since 1970-01-01 (negative before it). The conversions follow Howard
//! Hinnant's `days_from_civil`/`civil_from_days` algorithms, which are exact for every year in
//! the supported range and need only integer arithmetic.

use std::fmt;

use crate::time::Timestamp;

/// Minutes in one civil day (fixed UTC offsets have no DST transitions).
pub(crate) const MINUTES_PER_DAY: i64 = 1_440;
/// Milliseconds in one minute.
pub(crate) const MILLIS_PER_MINUTE: i64 = 60_000;
/// Largest absolute year accepted by [`CivilDate::new`]; far beyond any `Timestamp`.
const MAX_ABS_YEAR: i64 = 1_000_000_000;

/// A proleptic Gregorian calendar date.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CivilDate {
    year: i64,
    month: u8,
    day: u8,
}

/// Whether `year` is a Gregorian leap year.
#[must_use]
pub fn is_leap_year(year: i64) -> bool {
    year.rem_euclid(4) == 0 && (year.rem_euclid(100) != 0 || year.rem_euclid(400) == 0)
}

/// Number of days in `month` (1–12) of `year`; 0 for an invalid month.
#[must_use]
pub fn days_in_month(year: i64, month: u8) -> u8 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if is_leap_year(year) => 29,
        2 => 28,
        _ => 0,
    }
}

impl CivilDate {
    /// Validates and builds a date.
    ///
    /// Returns `None` for a month outside 1–12, a day outside the month, or a year beyond
    /// ±1,000,000,000.
    #[must_use]
    pub fn new(year: i64, month: u8, day: u8) -> Option<Self> {
        if year.abs() > MAX_ABS_YEAR || day == 0 || day > days_in_month(year, month) {
            return None;
        }
        Some(Self { year, month, day })
    }

    /// The year (astronomical numbering: year 0 exists).
    #[must_use]
    pub fn year(self) -> i64 {
        self.year
    }

    /// The month, 1–12.
    #[must_use]
    pub fn month(self) -> u8 {
        self.month
    }

    /// The day of the month, 1–31.
    #[must_use]
    pub fn day(self) -> u8 {
        self.day
    }

    /// Days since 1970-01-01.
    #[must_use]
    pub fn days_since_epoch(self) -> i64 {
        let month = i64::from(self.month);
        let year = if month <= 2 { self.year - 1 } else { self.year };
        let era = year.div_euclid(400);
        let year_of_era = year.rem_euclid(400);
        let shifted_month = if month > 2 { month - 3 } else { month + 9 };
        let day_of_year = (153 * shifted_month + 2) / 5 + i64::from(self.day) - 1;
        let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
        era * 146_097 + day_of_era - 719_468
    }

    /// The date `days` days after 1970-01-01. Day numbers derived from any `Timestamp` and
    /// UTC offset are within range; larger magnitudes yield `None`.
    #[must_use]
    pub fn from_days(days: i64) -> Option<Self> {
        let shifted = days.checked_add(719_468)?;
        let era = shifted.div_euclid(146_097);
        let day_of_era = shifted.rem_euclid(146_097);
        let year_of_era =
            (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
        let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
        let shifted_month = (5 * day_of_year + 2) / 153;
        let day = day_of_year - (153 * shifted_month + 2) / 5 + 1;
        let month = if shifted_month < 10 {
            shifted_month + 3
        } else {
            shifted_month - 9
        };
        let year = era.checked_mul(400)?.checked_add(year_of_era)? + i64::from(month <= 2);
        Self::new(year, u8::try_from(month).ok()?, u8::try_from(day).ok()?)
    }

    /// Day of the week, 0 = Sunday … 6 = Saturday.
    #[must_use]
    pub fn weekday(self) -> u8 {
        // 1970-01-01 was a Thursday (4).
        let weekday = (self.days_since_epoch() + 4).rem_euclid(7);
        u8::try_from(weekday).unwrap_or(0)
    }

    /// The civil date of `ts` in a fixed UTC offset (minutes east of UTC).
    #[must_use]
    pub fn from_timestamp(ts: Timestamp, utc_offset_minutes: i32) -> Option<Self> {
        let local = local_minute(ts, utc_offset_minutes)?;
        Self::from_days(local.div_euclid(MINUTES_PER_DAY))
    }
}

impl fmt::Display for CivilDate {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:04}-{:02}-{:02}", self.year, self.month, self.day)
    }
}

/// Local minutes since the epoch for `ts` in a fixed offset (floor division).
pub(crate) fn local_minute(ts: Timestamp, utc_offset_minutes: i32) -> Option<i64> {
    let offset_ms = i64::from(utc_offset_minutes).checked_mul(MILLIS_PER_MINUTE)?;
    Some(ts.0.checked_add(offset_ms)?.div_euclid(MILLIS_PER_MINUTE))
}

/// The UTC timestamp of the start of a local minute in a fixed offset.
pub(crate) fn timestamp_of_local_minute(minute: i64, utc_offset_minutes: i32) -> Option<Timestamp> {
    let offset_ms = i64::from(utc_offset_minutes).checked_mul(MILLIS_PER_MINUTE)?;
    let local_ms = minute.checked_mul(MILLIS_PER_MINUTE)?;
    Some(Timestamp(local_ms.checked_sub(offset_ms)?))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn date(year: i64, month: u8, day: u8) -> Result<CivilDate, String> {
        CivilDate::new(year, month, day).ok_or_else(|| format!("invalid {year}-{month}-{day}"))
    }

    #[test]
    fn known_day_numbers_and_weekdays() -> Result<(), String> {
        // Reference values computed independently with Python's datetime.
        let cases = [
            (1970, 1, 1, 0, 4),
            (2000, 1, 1, 10_957, 6),
            (2024, 2, 29, 19_782, 4),
            (2026, 10, 7, 20_733, 3),
            (1969, 12, 31, -1, 3),
            (1600, 3, 1, -135_080, 3),
            (2100, 3, 1, 47_541, 1),
        ];
        for (year, month, day, days, weekday) in cases {
            let civil = date(year, month, day)?;
            assert_eq!(civil.days_since_epoch(), days, "{civil}");
            assert_eq!(civil.weekday(), weekday, "{civil}");
            assert_eq!(CivilDate::from_days(days), Some(civil));
        }
        Ok(())
    }

    #[test]
    fn round_trips_across_four_centuries() -> Result<(), String> {
        let start = date(1899, 12, 1)?.days_since_epoch();
        let mut previous: Option<CivilDate> = None;
        for days in start..start + 146_097 + 400 {
            let civil = CivilDate::from_days(days).ok_or("out of range")?;
            assert_eq!(civil.days_since_epoch(), days);
            if let Some(before) = previous {
                assert!(before < civil, "{before} then {civil}");
            }
            previous = Some(civil);
        }
        Ok(())
    }

    #[test]
    fn leap_years_and_month_lengths() {
        assert!(is_leap_year(2024) && is_leap_year(2000) && is_leap_year(1600));
        assert!(!is_leap_year(2100) && !is_leap_year(1900) && !is_leap_year(2026));
        assert_eq!(days_in_month(2024, 2), 29);
        assert_eq!(days_in_month(2100, 2), 28);
        assert_eq!(days_in_month(2026, 4), 30);
        assert_eq!(days_in_month(2026, 13), 0);
        assert!(CivilDate::new(2026, 2, 29).is_none());
        assert!(CivilDate::new(2028, 2, 29).is_some());
        assert!(CivilDate::new(2026, 0, 1).is_none());
        assert!(CivilDate::new(2026, 4, 31).is_none());
    }

    #[test]
    fn timestamps_in_fixed_offsets() -> Result<(), String> {
        // 2026-10-07T23:30:00Z
        let ts = Timestamp(1_791_352_800_000 + 17 * 3_600_000 + 30 * 60_000);
        assert_eq!(CivilDate::from_timestamp(ts, 0), Some(date(2026, 10, 7)?));
        assert_eq!(CivilDate::from_timestamp(ts, 30), Some(date(2026, 10, 8)?));
        assert_eq!(
            CivilDate::from_timestamp(ts, -24 * 60),
            Some(date(2026, 10, 6)?)
        );
        assert_eq!(
            CivilDate::from_timestamp(Timestamp(-1), 0),
            Some(date(1969, 12, 31)?)
        );
        assert_eq!(CivilDate::from_timestamp(Timestamp(i64::MAX), 60), None);
        assert_eq!(date(2026, 1, 5)?.to_string(), "2026-01-05");
        Ok(())
    }
}
