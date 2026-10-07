//! Five-field cron schedules evaluated in a fixed UTC offset (SPEC-001 AUT-003, SPEC-002 §6.4).
//!
//! Syntax: `minute hour day-of-month month day-of-week`, each field a comma list of `*`,
//! `N`, `A-B`, with an optional `/STEP` (`*/15`, `0-30/10`, and `5/15` meaning `5-59/15`).
//! Months accept `JAN`–`DEC` and weekdays `SUN`–`SAT` (case-insensitive, also in ranges);
//! weekday `7` is Sunday. The macros `@yearly`, `@annually`, `@monthly`, `@weekly`, `@daily`,
//! `@midnight` and `@hourly` are accepted. `L`, `W`, `#`, `?` and seconds are not.
//!
//! Day matching follows Vixie cron: a day field counts as *unrestricted* when its text starts
//! with `*` (so `*/2` is unrestricted). When both day fields are restricted a day matches if
//! **either** matches; otherwise both must match.
//!
//! Time is a fixed UTC offset in minutes (no DST rules). Schedules are evaluated on local civil
//! time and converted back to UTC [`Timestamp`]s, always at whole minutes.
//!
//! Intended scheduler use (SPEC-002 §6.4): on startup call [`Schedule::catch_up`] with the
//! last recorded `scheduled_for` (or the automation's creation time) to obtain at most one
//! catch-up run; while running, call [`Schedule::due_runs`] from the previous tick and insert a
//! unique `(automation, scheduled_for)` row per returned slot before creating the task.

mod civil;

use std::fmt;

pub use civil::{CivilDate, days_in_month, is_leap_year};

use crate::time::Timestamp;
use civil::{MINUTES_PER_DAY, local_minute, timestamp_of_local_minute};

/// Longest accepted expression.
pub const MAX_EXPRESSION_BYTES: usize = 256;
/// Largest plausible UTC offset (±18:00); callers should reject offsets beyond it at the API
/// boundary. Evaluation itself accepts any offset.
pub const MAX_UTC_OFFSET_MINUTES: i32 = 18 * 60;
/// Months searched before [`Schedule::next_after`] gives up: one full 400-year Gregorian cycle
/// (in which every month/day/weekday combination recurs) plus one month.
const SEARCH_MONTHS: u32 = 400 * 12 + 1;

const MONTH_NAMES: [&str; 12] = [
    "JAN", "FEB", "MAR", "APR", "MAY", "JUN", "JUL", "AUG", "SEP", "OCT", "NOV", "DEC",
];
const WEEKDAY_NAMES: [&str; 7] = ["SUN", "MON", "TUE", "WED", "THU", "FRI", "SAT"];

/// One of the five cron fields.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CronField {
    /// Minute, 0–59.
    Minute,
    /// Hour, 0–23.
    Hour,
    /// Day of the month, 1–31.
    DayOfMonth,
    /// Month, 1–12 or `JAN`–`DEC`.
    Month,
    /// Day of the week, 0–7 (0 and 7 are Sunday) or `SUN`–`SAT`.
    DayOfWeek,
}

impl CronField {
    const ALL: [Self; 5] = [
        Self::Minute,
        Self::Hour,
        Self::DayOfMonth,
        Self::Month,
        Self::DayOfWeek,
    ];

    fn bounds(self) -> (u32, u32) {
        match self {
            Self::Minute => (0, 59),
            Self::Hour => (0, 23),
            Self::DayOfMonth => (1, 31),
            Self::Month => (1, 12),
            Self::DayOfWeek => (0, 7),
        }
    }

    fn name_value(self, text: &str) -> Option<u32> {
        let (names, first): (&[&str], u32) = match self {
            Self::Month => (&MONTH_NAMES, 1),
            Self::DayOfWeek => (&WEEKDAY_NAMES, 0),
            _ => return None,
        };
        let index = names
            .iter()
            .position(|name| name.eq_ignore_ascii_case(text))?;
        u32::try_from(index).ok().map(|index| index + first)
    }

    /// Stable human-readable name.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Minute => "minute",
            Self::Hour => "hour",
            Self::DayOfMonth => "day-of-month",
            Self::Month => "month",
            Self::DayOfWeek => "day-of-week",
        }
    }
}

impl fmt::Display for CronField {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A rejected cron expression. Messages are stable and suitable for showing to an agent.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CronError {
    /// Nothing but whitespace.
    #[error("cron expression is empty")]
    Empty,
    /// Longer than [`MAX_EXPRESSION_BYTES`].
    #[error("cron expression is longer than {MAX_EXPRESSION_BYTES} bytes")]
    TooLong,
    /// Not exactly five fields.
    #[error(
        "cron expression has {found} fields; expected 5 (minute hour day-of-month month day-of-week)"
    )]
    FieldCount {
        /// Number of whitespace-separated fields found.
        found: usize,
    },
    /// An `@` macro that is not supported.
    #[error(
        "unknown cron macro '{0}'; use @yearly, @annually, @monthly, @weekly, @daily, @midnight or @hourly"
    )]
    UnknownMacro(String),
    /// A field that does not parse or is out of range.
    #[error("invalid {field} field '{text}': {reason}")]
    InvalidField {
        /// The field.
        field: CronField,
        /// The field text as written.
        text: String,
        /// What is wrong.
        reason: String,
    },
    /// The day-of-month never exists in the selected months (for example `30 2`).
    #[error("cron expression never runs: the day-of-month does not exist in the selected months")]
    NeverRuns,
}

/// A parsed five-field cron schedule.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Schedule {
    source: String,
    minutes: u64,
    hours: u64,
    days: u64,
    months: u64,
    weekdays: u64,
    dom_restricted: bool,
    dow_restricted: bool,
}

impl Schedule {
    /// Parses a cron expression.
    ///
    /// # Errors
    /// Returns [`CronError`] describing the first problem found.
    ///
    /// # Examples
    /// ```
    /// use yulius_domain::cron::Schedule;
    /// let schedule = Schedule::parse("0 9 * * MON-FRI")?;
    /// assert_eq!(schedule.as_str(), "0 9 * * MON-FRI");
    /// assert!(Schedule::parse("0 9 * *").is_err());
    /// # Ok::<(), yulius_domain::cron::CronError>(())
    /// ```
    pub fn parse(expression: &str) -> Result<Self, CronError> {
        let source = expression.trim();
        if source.is_empty() {
            return Err(CronError::Empty);
        }
        if source.len() > MAX_EXPRESSION_BYTES {
            return Err(CronError::TooLong);
        }
        let expanded = if source.starts_with('@') {
            match source.to_ascii_lowercase().as_str() {
                "@yearly" | "@annually" => "0 0 1 1 *",
                "@monthly" => "0 0 1 * *",
                "@weekly" => "0 0 * * 0",
                "@daily" | "@midnight" => "0 0 * * *",
                "@hourly" => "0 * * * *",
                _ => return Err(CronError::UnknownMacro(source.to_owned())),
            }
        } else {
            source
        };
        let fields: Vec<&str> = expanded.split_whitespace().collect();
        if fields.len() != CronField::ALL.len() {
            return Err(CronError::FieldCount {
                found: fields.len(),
            });
        }
        let mut bits = [0_u64; 5];
        for ((slot, field), text) in bits.iter_mut().zip(CronField::ALL).zip(&fields) {
            *slot = parse_field(field, text)?;
        }
        let [minutes, hours, days, months, mut weekdays] = bits;
        if weekdays & (1 << 7) != 0 {
            weekdays = (weekdays & !(1 << 7)) | 1;
        }
        let schedule = Self {
            source: source.to_owned(),
            minutes,
            hours,
            days,
            months,
            weekdays,
            dom_restricted: !fields[2].starts_with('*'),
            dow_restricted: !fields[4].starts_with('*'),
        };
        if !schedule.can_run() {
            return Err(CronError::NeverRuns);
        }
        Ok(schedule)
    }

    /// The expression as written (trimmed).
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.source
    }

    /// Whether some calendar day can match. Only the day-of-month/month combination can be
    /// impossible: every weekday occurs in every month, and over a 400-year cycle every valid
    /// month/day falls on every weekday.
    fn can_run(&self) -> bool {
        if self.dom_restricted && self.dow_restricted {
            return true;
        }
        (1..=12_u8).any(|month| {
            self.months & (1 << month) != 0
                && (1..=days_in_month(2000, month)).any(|day| self.days & (1 << day) != 0)
        })
    }

    fn day_matches(&self, date: CivilDate) -> bool {
        let dom = self.days & (1 << date.day()) != 0;
        let dow = self.weekdays & (1 << date.weekday()) != 0;
        if self.dom_restricted && self.dow_restricted {
            dom || dow
        } else {
            dom && dow
        }
    }

    /// Whether the schedule fires at the minute containing `ts` in the given offset.
    #[must_use]
    pub fn matches(&self, ts: Timestamp, utc_offset_minutes: i32) -> bool {
        let Some(minute) = local_minute(ts, utc_offset_minutes) else {
            return false;
        };
        let Some(date) = CivilDate::from_days(minute.div_euclid(MINUTES_PER_DAY)) else {
            return false;
        };
        let of_day = minute.rem_euclid(MINUTES_PER_DAY);
        self.months & (1 << date.month()) != 0
            && self.day_matches(date)
            && self.hours & (1 << (of_day / 60)) != 0
            && self.minutes & (1 << (of_day % 60)) != 0
    }

    /// The first scheduled time strictly after `ts`, evaluated in a fixed UTC offset.
    ///
    /// Returns `None` only when the result would overflow the timestamp range.
    #[must_use]
    pub fn next_after(&self, ts: Timestamp, utc_offset_minutes: i32) -> Option<Timestamp> {
        let start = local_minute(ts, utc_offset_minutes)?.checked_add(1)?;
        let found = self.next_local_minute(start)?;
        timestamp_of_local_minute(found, utc_offset_minutes)
    }

    /// The first matching local minute at or after `start` (local minutes since the epoch).
    fn next_local_minute(&self, start: i64) -> Option<i64> {
        let first_day = CivilDate::from_days(start.div_euclid(MINUTES_PER_DAY))?;
        let of_day = start.rem_euclid(MINUTES_PER_DAY);
        let (mut year, mut month, mut day) = (first_day.year(), first_day.month(), first_day.day());
        let (mut hour, mut minute) = (of_day / 60, of_day % 60);
        for _ in 0..SEARCH_MONTHS {
            if self.months & (1 << month) != 0 {
                while day <= days_in_month(year, month) {
                    let date = CivilDate::new(year, month, day)?;
                    if self.day_matches(date)
                        && let Some(at) = self.first_time_from(hour, minute)
                    {
                        return date
                            .days_since_epoch()
                            .checked_mul(MINUTES_PER_DAY)?
                            .checked_add(at);
                    }
                    day += 1;
                    hour = 0;
                    minute = 0;
                }
            }
            month += 1;
            if month > 12 {
                month = 1;
                year = year.checked_add(1)?;
            }
            day = 1;
            hour = 0;
            minute = 0;
        }
        None
    }

    /// Minutes after midnight of the first matching hour:minute at or after `hour:minute`.
    fn first_time_from(&self, hour: i64, minute: i64) -> Option<i64> {
        for candidate_hour in hour..24 {
            if self.hours & (1 << candidate_hour) == 0 {
                continue;
            }
            let from = if candidate_hour == hour { minute } else { 0 };
            let remaining = self.minutes >> from;
            if remaining != 0 {
                let candidate_minute = from + i64::from(remaining.trailing_zeros());
                if candidate_minute < 60 {
                    return Some(candidate_hour * 60 + candidate_minute);
                }
            }
        }
        None
    }

    /// Scheduled times `s` with `last_run < s <= now`, earliest first, at most `max`.
    ///
    /// `last_run` is the last handled scheduled time (or the automation's creation time).
    /// `None` means the automation has no anchor yet: only a slot in the minute containing
    /// `now` is due, and no history is replayed.
    #[must_use]
    pub fn due_runs(
        &self,
        last_run: Option<Timestamp>,
        now: Timestamp,
        utc_offset_minutes: i32,
        max: usize,
    ) -> Vec<Timestamp> {
        let mut runs = Vec::new();
        let anchor = match last_run {
            Some(anchor) => anchor,
            None => {
                let minute_start = now.0.div_euclid(60_000).saturating_mul(60_000);
                Timestamp(minute_start.saturating_sub(1))
            }
        };
        let mut cursor = anchor;
        while runs.len() < max {
            match self.next_after(cursor, utc_offset_minutes) {
                Some(slot) if slot <= now => {
                    runs.push(slot);
                    cursor = slot;
                }
                _ => break,
            }
        }
        runs
    }

    /// The single catch-up run owed after downtime, if any (SPEC-002 §6.4).
    ///
    /// The first missed run is the first scheduled time after `last_scheduled`. It is caught
    /// up only if it was missed by less than one interval, i.e. `now` is still before the
    /// following scheduled time; when two or more runs were missed the stale runs are skipped
    /// and the automation resumes at its next regular time. Returns the missed scheduled time
    /// (the deduplication key), never more than one.
    #[must_use]
    pub fn catch_up(
        &self,
        last_scheduled: Timestamp,
        now: Timestamp,
        utc_offset_minutes: i32,
    ) -> Option<Timestamp> {
        let missed = self.next_after(last_scheduled, utc_offset_minutes)?;
        if missed > now {
            return None;
        }
        match self.next_after(missed, utc_offset_minutes) {
            Some(following) if following <= now => None,
            _ => Some(missed),
        }
    }
}

impl fmt::Display for Schedule {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.source)
    }
}

fn parse_field(field: CronField, text: &str) -> Result<u64, CronError> {
    let invalid = |reason: String| CronError::InvalidField {
        field,
        text: text.to_owned(),
        reason,
    };
    let (min, max) = field.bounds();
    let mut bits = 0_u64;
    for item in text.split(',') {
        if item.is_empty() {
            return Err(invalid("empty list item".to_owned()));
        }
        let (range, step) = match item.split_once('/') {
            Some((range, step)) => (range, Some(parse_step(step).map_err(invalid)?)),
            None => (item, None),
        };
        let (low, high) = if range == "*" {
            (min, max)
        } else if let Some((low, high)) = range.split_once('-') {
            let low = parse_value(field, low).map_err(invalid)?;
            let high = parse_value(field, high).map_err(invalid)?;
            if low > high {
                return Err(invalid(format!(
                    "range {low}-{high} is reversed; write the smaller value first"
                )));
            }
            (low, high)
        } else {
            let value = parse_value(field, range).map_err(invalid)?;
            (value, if step.is_some() { max } else { value })
        };
        let step = step.unwrap_or(1);
        let mut value = low;
        while value <= high {
            bits |= 1 << value;
            value += step;
        }
    }
    Ok(bits)
}

fn parse_step(text: &str) -> Result<u32, String> {
    if text.is_empty() || text.len() > 4 || !text.bytes().all(|b| b.is_ascii_digit()) {
        return Err(format!("step '{text}' is not a number"));
    }
    match text.parse::<u32>() {
        Ok(0) => Err("step must be at least 1".to_owned()),
        Ok(step) => Ok(step),
        Err(_) => Err(format!("step '{text}' is not a number")),
    }
}

fn parse_value(field: CronField, text: &str) -> Result<u32, String> {
    let (min, max) = field.bounds();
    if text.is_empty() {
        return Err("missing value".to_owned());
    }
    let value = if text.bytes().all(|b| b.is_ascii_digit()) {
        if text.len() > 4 {
            return Err(format!("value '{text}' is out of range {min}-{max}"));
        }
        text.parse::<u32>()
            .map_err(|_| format!("value '{text}' is not a number"))?
    } else {
        field
            .name_value(text)
            .ok_or_else(|| format!("'{text}' is not a number or a valid name"))?
    };
    if value < min || value > max {
        return Err(format!("value {value} is out of range {min}-{max}"));
    }
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    /// UTC timestamp of a civil date-time interpreted in `offset` minutes east of UTC.
    fn at(
        year: i64,
        month: u8,
        day: u8,
        hour: i64,
        minute: i64,
        offset: i32,
    ) -> Result<Timestamp, String> {
        let date = CivilDate::new(year, month, day).ok_or("bad date")?;
        let local = date.days_since_epoch() * MINUTES_PER_DAY + hour * 60 + minute;
        timestamp_of_local_minute(local, offset).ok_or_else(|| "overflow".to_owned())
    }

    fn utc(year: i64, month: u8, day: u8, hour: i64, minute: i64) -> Result<Timestamp, String> {
        at(year, month, day, hour, minute, 0)
    }

    fn next(expr: &str, from: Timestamp, offset: i32) -> Result<Option<Timestamp>, CronError> {
        Ok(Schedule::parse(expr)?.next_after(from, offset))
    }

    #[test]
    fn daily_schedule_in_positive_offset() -> TestResult {
        // 06:00Z is 08:00 at UTC+2; the next 09:00 local is 07:00Z the same day.
        let from = utc(2026, 10, 7, 6, 0)?;
        assert_eq!(next("0 9 * * *", from, 120)?, Some(utc(2026, 10, 7, 7, 0)?));
        // Already past 09:00 local: tomorrow.
        let late = utc(2026, 10, 7, 7, 0)?;
        assert_eq!(next("0 9 * * *", late, 120)?, Some(utc(2026, 10, 8, 7, 0)?));
        Ok(())
    }

    #[test]
    fn next_is_strictly_after_and_minute_aligned() -> TestResult {
        let slot = utc(2026, 10, 7, 9, 0)?;
        let schedule = Schedule::parse("0 9 * * *")?;
        assert_eq!(schedule.next_after(slot, 0), Some(utc(2026, 10, 8, 9, 0)?));
        let just_before = Timestamp(slot.0 - 1);
        assert_eq!(schedule.next_after(just_before, 0), Some(slot));
        let mid_minute = slot.plus_millis(30_000);
        assert_eq!(
            Schedule::parse("* * * * *")?.next_after(mid_minute, 0),
            Some(slot.plus_millis(60_000))
        );
        assert!(schedule.matches(slot.plus_millis(59_999), 0));
        assert!(!schedule.matches(slot.plus_millis(60_000), 0));
        Ok(())
    }

    #[test]
    fn negative_and_fractional_offsets_cross_dates() -> TestResult {
        // 09:00 at UTC-10 is 19:00Z.
        let from = utc(2026, 10, 7, 20, 0)?;
        assert_eq!(
            next("0 9 * * *", from, -600)?,
            Some(utc(2026, 10, 8, 19, 0)?)
        );
        // Nepal (UTC+05:45): midnight local is 18:15Z the previous day.
        let from = utc(2026, 10, 7, 12, 0)?;
        assert_eq!(
            next("0 0 * * *", from, 345)?,
            Some(utc(2026, 10, 7, 18, 15)?)
        );
        // Marquesas (UTC-09:30), weekday evaluated locally: Monday 00:30 local.
        let from = utc(2026, 10, 7, 0, 0)?; // Wednesday
        let expected = at(2026, 10, 12, 0, 30, -570)?;
        assert_eq!(next("30 0 * * MON", from, -570)?, Some(expected));
        // Kiribati (UTC+14): Saturday local begins on Friday UTC.
        let from = utc(2026, 10, 9, 0, 0)?; // Friday 00:00Z = Friday 14:00 local
        assert_eq!(
            next("0 0 * * 6", from, 840)?,
            Some(utc(2026, 10, 9, 10, 0)?)
        );
        Ok(())
    }

    #[test]
    fn february_29_and_month_lengths() -> TestResult {
        let from = utc(2026, 3, 1, 0, 0)?;
        assert_eq!(next("0 0 29 2 *", from, 0)?, Some(utc(2028, 2, 29, 0, 0)?));
        // 2100 is not a leap year.
        let from = utc(2096, 3, 1, 0, 0)?;
        assert_eq!(next("0 0 29 2 *", from, 0)?, Some(utc(2104, 2, 29, 0, 0)?));
        // The 31st skips 30-day months and February.
        let from = utc(2026, 1, 31, 12, 0)?;
        assert_eq!(next("0 0 31 * *", from, 0)?, Some(utc(2026, 3, 31, 0, 0)?));
        let from = utc(2026, 3, 31, 1, 0)?;
        assert_eq!(next("0 0 31 * *", from, 0)?, Some(utc(2026, 5, 31, 0, 0)?));
        // Year boundary.
        let from = utc(2026, 12, 31, 23, 59)?;
        assert_eq!(next("0 0 1 1 *", from, 0)?, Some(utc(2027, 1, 1, 0, 0)?));
        assert_eq!(next("@yearly", from, 0)?, Some(utc(2027, 1, 1, 0, 0)?));
        Ok(())
    }

    #[test]
    fn leap_day_with_weekday_under_both_semantics() -> TestResult {
        let from = utc(2026, 1, 1, 0, 0)?;
        // Both day fields restricted: OR — the first Monday of February 2026 wins.
        assert_eq!(next("0 0 29 2 1", from, 0)?, Some(utc(2026, 2, 2, 0, 0)?));
        // `*/7` in day-of-week is star-prefixed (unrestricted per Vixie) but selects only
        // Sunday (0 and 7), so both fields must match: a February 29 that is a Sunday.
        assert_eq!(
            next("0 0 29 2 */7", from, 0)?,
            Some(utc(2032, 2, 29, 0, 0)?)
        );
        let after = utc(2032, 2, 29, 0, 0)?;
        assert_eq!(
            next("0 0 29 2 */7", after, 0)?,
            Some(utc(2060, 2, 29, 0, 0)?)
        );
        Ok(())
    }

    #[test]
    fn vixie_or_semantics_when_both_day_fields_restricted() -> TestResult {
        let schedule = Schedule::parse("0 0 13 * 5")?; // the 13th OR any Friday
        let from = utc(2026, 10, 7, 0, 0)?; // Wednesday
        let friday = utc(2026, 10, 9, 0, 0)?;
        assert_eq!(schedule.next_after(from, 0), Some(friday));
        assert_eq!(
            schedule.next_after(friday, 0),
            Some(utc(2026, 10, 13, 0, 0)?)
        ); // Tuesday the 13th
        // Unrestricted weekday: only the 13th.
        let only_13th = Schedule::parse("0 0 13 * *")?;
        assert_eq!(
            only_13th.next_after(from, 0),
            Some(utc(2026, 10, 13, 0, 0)?)
        );
        // A star-prefixed day-of-month with a step counts as unrestricted: AND semantics.
        let odd_fridays = Schedule::parse("0 0 */2 * FRI")?;
        assert_eq!(
            odd_fridays.next_after(from, 0),
            Some(utc(2026, 10, 9, 0, 0)?)
        ); // the 9th is odd and a Friday
        let after = odd_fridays.next_after(utc(2026, 10, 9, 0, 0)?, 0);
        assert_eq!(after, Some(utc(2026, 10, 23, 0, 0)?)); // 16th is even, 23rd odd
        Ok(())
    }

    #[test]
    fn steps_ranges_lists_and_names() -> TestResult {
        let from = utc(2026, 10, 7, 10, 7)?;
        assert_eq!(
            next("*/15 * * * *", from, 0)?,
            Some(utc(2026, 10, 7, 10, 15)?)
        );
        assert_eq!(
            next("5/15 * * * *", from, 0)?,
            Some(utc(2026, 10, 7, 10, 20)?)
        );
        assert_eq!(
            next("0-30/10 * * * *", from, 0)?,
            Some(utc(2026, 10, 7, 10, 10)?)
        );
        assert_eq!(
            next("10-50/20 * * * *", from, 0)?,
            Some(utc(2026, 10, 7, 10, 10)?)
        );
        assert_eq!(
            next("1,2,59 * * * *", from, 0)?,
            Some(utc(2026, 10, 7, 10, 59)?)
        );
        assert_eq!(
            next("*/90 * * * *", from, 0)?,
            Some(utc(2026, 10, 7, 11, 0)?)
        );
        assert_eq!(
            next("59 23 * * *", from, 0)?,
            Some(utc(2026, 10, 7, 23, 59)?)
        );
        // Weekday names and ranges, case-insensitive; Wednesday 10:07 → Thursday.
        assert_eq!(
            next("0 12 * * thu-Fri", from, 0)?,
            Some(utc(2026, 10, 8, 12, 0)?)
        );
        assert_eq!(
            next("0 12 1 jan,JUL *", from, 0)?,
            Some(utc(2027, 1, 1, 12, 0)?)
        );
        assert_eq!(
            next("0 0 * DEC-DEC *", from, 0)?,
            Some(utc(2026, 12, 1, 0, 0)?)
        );
        Ok(())
    }

    #[test]
    fn sunday_is_zero_seven_and_name() -> TestResult {
        let from = utc(2026, 10, 7, 0, 0)?;
        let sunday = Some(utc(2026, 10, 11, 0, 0)?);
        for expr in [
            "0 0 * * 0",
            "0 0 * * 7",
            "0 0 * * SUN",
            "@weekly",
            "0 0 * * 6-7",
        ] {
            let found = next(expr, from, 0)?;
            if expr == "0 0 * * 6-7" {
                assert_eq!(found, Some(utc(2026, 10, 10, 0, 0)?), "{expr}");
            } else {
                assert_eq!(found, sunday, "{expr}");
            }
        }
        assert_eq!(Schedule::parse("0 0 * * 0-7")?.weekdays, 0b111_1111);
        Ok(())
    }

    #[test]
    fn macros() -> TestResult {
        let from = utc(2026, 10, 7, 10, 7)?;
        assert_eq!(next("@hourly", from, 0)?, Some(utc(2026, 10, 7, 11, 0)?));
        assert_eq!(next("@daily", from, 0)?, Some(utc(2026, 10, 8, 0, 0)?));
        assert_eq!(next("@MIDNIGHT", from, 0)?, Some(utc(2026, 10, 8, 0, 0)?));
        assert_eq!(next("@monthly", from, 0)?, Some(utc(2026, 11, 1, 0, 0)?));
        assert_eq!(next("@annually", from, 0)?, Some(utc(2027, 1, 1, 0, 0)?));
        assert!(matches!(
            Schedule::parse("@reboot"),
            Err(CronError::UnknownMacro(_))
        ));
        Ok(())
    }

    #[test]
    fn rejects_invalid_expressions_with_precise_errors() {
        let field_error = |expr: &str| match Schedule::parse(expr) {
            Err(CronError::InvalidField { field, .. }) => Some(field),
            _ => None,
        };
        assert_eq!(Schedule::parse("   "), Err(CronError::Empty));
        assert_eq!(
            Schedule::parse("* * * *"),
            Err(CronError::FieldCount { found: 4 })
        );
        assert_eq!(
            Schedule::parse("0 * * * * *"),
            Err(CronError::FieldCount { found: 6 })
        );
        assert_eq!(field_error("60 * * * *"), Some(CronField::Minute));
        assert_eq!(field_error("* 24 * * *"), Some(CronField::Hour));
        assert_eq!(field_error("* * 0 * *"), Some(CronField::DayOfMonth));
        assert_eq!(field_error("* * 32 * *"), Some(CronField::DayOfMonth));
        assert_eq!(field_error("* * * 13 *"), Some(CronField::Month));
        assert_eq!(field_error("* * * 0 *"), Some(CronField::Month));
        assert_eq!(field_error("* * * * 8"), Some(CronField::DayOfWeek));
        assert_eq!(field_error("*/0 * * * *"), Some(CronField::Minute));
        assert_eq!(field_error("5-1 * * * *"), Some(CronField::Minute));
        assert_eq!(field_error("1,,2 * * * *"), Some(CronField::Minute));
        assert_eq!(field_error("MON * * * *"), Some(CronField::Minute));
        assert_eq!(field_error("* * * * MON-"), Some(CronField::DayOfWeek));
        assert_eq!(field_error("* * * * FRIDAY"), Some(CronField::DayOfWeek));
        assert_eq!(field_error("* * * * */x"), Some(CronField::DayOfWeek));
        assert_eq!(field_error("* * L * *"), Some(CronField::DayOfMonth));
        assert_eq!(field_error("99999 * * * *"), Some(CronField::Minute));
        assert_eq!(Schedule::parse(&"0 ".repeat(200)), Err(CronError::TooLong));
        let message = Schedule::parse("5-1 * * * *")
            .map(|_| ())
            .err()
            .map(|e| e.to_string());
        assert_eq!(
            message.as_deref(),
            Some(
                "invalid minute field '5-1': range 5-1 is reversed; write the smaller value first"
            )
        );
    }

    #[test]
    fn impossible_dates_never_run() -> TestResult {
        assert_eq!(Schedule::parse("0 0 30 2 *"), Err(CronError::NeverRuns));
        assert_eq!(
            Schedule::parse("0 0 31 4,6,9,11 *"),
            Err(CronError::NeverRuns)
        );
        // With a restricted weekday the OR semantics make it run on that weekday.
        let mondays = Schedule::parse("0 0 31 2 1")?;
        let from = utc(2026, 10, 7, 0, 0)?;
        assert_eq!(mondays.next_after(from, 0), Some(utc(2027, 2, 1, 0, 0)?));
        Ok(())
    }

    #[test]
    fn extreme_timestamps_do_not_panic() -> TestResult {
        let schedule = Schedule::parse("* * * * *")?;
        assert_eq!(schedule.next_after(Timestamp(i64::MAX), 0), None);
        assert_eq!(schedule.next_after(Timestamp(i64::MAX - 1), 840), None);
        assert!(schedule.next_after(Timestamp(i64::MIN), 0).is_some());
        let before_epoch = utc(1969, 12, 31, 23, 59)?;
        assert_eq!(
            Schedule::parse("0 0 1 1 *")?.next_after(before_epoch, 0),
            Some(Timestamp(0))
        );
        assert_eq!(
            Schedule::parse("0 0 29 2 *")?.next_after(utc(1899, 3, 1, 0, 0)?, 0),
            Some(utc(1904, 2, 29, 0, 0)?)
        );
        Ok(())
    }

    #[test]
    fn next_after_agrees_with_minute_by_minute_scan() -> TestResult {
        let expressions = [
            "*/7 * * * *",
            "0 */5 * * *",
            "15,45 9-17 * * MON-FRI",
            "0 0 * * 0",
            "30 2 1-3 * *",
            "0 12 13 * FRI",
            "5/20 1-23/4 */2 * *",
            "0 6 * * 6,7",
        ];
        let offsets = [0, 120, -300, 345, 840, -570];
        // Deterministic pseudo-random start times between 1969 and 2100.
        let mut seed: u64 = 0x5eed_cafe;
        for expression in expressions {
            let schedule = Schedule::parse(expression)?;
            for round in 0..12 {
                seed = seed
                    .wrapping_mul(6_364_136_223_846_793_005)
                    .wrapping_add(1_442_695_040_888_963_407);
                let span = 4_102_444_800_000_i64; // ms from 1970 to 2100
                let start = Timestamp(
                    i64::try_from(seed >> 22).map_err(|e| e.to_string())? % span - 86_400_000,
                );
                let offset = offsets[round % offsets.len()];
                let found = schedule.next_after(start, offset).ok_or("no next")?;
                assert!(found > start && found.0 % 60_000 == 0);
                assert!(schedule.matches(found, offset), "{expression} at {found:?}");
                let mut probe = Timestamp(start.0.div_euclid(60_000) * 60_000 + 60_000);
                while probe < found {
                    assert!(
                        !schedule.matches(probe, offset),
                        "{expression}: {probe:?} matches before {found:?}"
                    );
                    probe = probe.plus_millis(60_000);
                }
            }
        }
        Ok(())
    }

    #[test]
    fn due_runs_lists_missed_slots_with_cap() -> TestResult {
        let schedule = Schedule::parse("0 * * * *")?;
        let last = utc(2026, 10, 7, 9, 0)?;
        let now = utc(2026, 10, 7, 12, 30)?;
        assert_eq!(
            schedule.due_runs(Some(last), now, 0, 10),
            vec![
                utc(2026, 10, 7, 10, 0)?,
                utc(2026, 10, 7, 11, 0)?,
                utc(2026, 10, 7, 12, 0)?
            ]
        );
        assert_eq!(
            schedule.due_runs(Some(last), now, 0, 2),
            vec![utc(2026, 10, 7, 10, 0)?, utc(2026, 10, 7, 11, 0)?]
        );
        assert!(schedule.due_runs(Some(last), now, 0, 0).is_empty());
        assert!(schedule.due_runs(Some(now), now, 0, 5).is_empty());
        // The slot equal to `now` is due; nothing after it.
        let at_slot = utc(2026, 10, 7, 12, 0)?;
        assert_eq!(
            schedule.due_runs(Some(last), at_slot, 0, 10).last(),
            Some(&at_slot)
        );
        Ok(())
    }

    #[test]
    fn due_runs_without_anchor_only_covers_current_minute() -> TestResult {
        let schedule = Schedule::parse("0 9 * * *")?;
        let in_slot_minute = utc(2026, 10, 7, 9, 0)?.plus_millis(5_000);
        assert_eq!(
            schedule.due_runs(None, in_slot_minute, 0, 5),
            vec![utc(2026, 10, 7, 9, 0)?]
        );
        let later = utc(2026, 10, 7, 9, 1)?;
        assert!(schedule.due_runs(None, later, 0, 5).is_empty());
        Ok(())
    }

    #[test]
    fn catch_up_runs_at_most_one_recent_missed_slot() -> TestResult {
        let daily = Schedule::parse("0 9 * * *")?;
        let last = utc(2026, 10, 6, 9, 0)?;
        // Down from 08:00 to 15:00 on the 7th: one run missed by 6 h < 24 h → caught up.
        let now = utc(2026, 10, 7, 15, 0)?;
        assert_eq!(daily.catch_up(last, now, 0), Some(utc(2026, 10, 7, 9, 0)?));
        // Down for three days: several runs missed → stale, none caught up.
        let much_later = utc(2026, 10, 9, 15, 0)?;
        assert_eq!(daily.catch_up(last, much_later, 0), None);
        // Exactly one interval late (the following slot is due now) → not caught up.
        let next_slot = utc(2026, 10, 8, 9, 0)?;
        assert_eq!(daily.catch_up(last, next_slot, 0), None);
        // Nothing missed.
        assert_eq!(daily.catch_up(last, utc(2026, 10, 7, 8, 59)?, 0), None);
        // Evaluated in the automation's offset: 09:00 at UTC+2 is 07:00Z.
        let last_local = utc(2026, 10, 6, 7, 0)?;
        assert_eq!(
            daily.catch_up(last_local, utc(2026, 10, 7, 7, 30)?, 120),
            Some(utc(2026, 10, 7, 7, 0)?)
        );
        Ok(())
    }
}
