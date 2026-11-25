//! When a subscription charges its card (SPEC-651, ADR-0478): a charge date the person knows and
//! how often it repeats. No provider tells Yulius the billing date, so the person types one; the
//! next charge is counted from it.

use std::fmt;

use crate::cron::{CivilDate, days_in_month};

/// The earliest year a charge date may have.
const FIRST_YEAR: i64 = 2000;
/// The latest year a charge date may have.
const LAST_YEAR: i64 = 2100;

/// How often a subscription charges its card.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BillingPeriod {
    /// Every month, on the anchor's day (or the month's last day when it has none).
    Month,
    /// Every year, on the anchor's day and month (28 Feb for a 29 Feb anchor in other years).
    Year,
}

impl BillingPeriod {
    /// The wire name: `month` or `year`.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Month => "month",
            Self::Year => "year",
        }
    }

    /// The period of a wire name; `None` for any other text.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "month" => Some(Self::Month),
            "year" => Some(Self::Year),
            _ => None,
        }
    }
}

impl fmt::Display for BillingPeriod {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Why a billing cycle was refused; the message is for people.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum BillingError {
    /// Not a calendar date written `YYYY-MM-DD`.
    #[error("the charge date must be a calendar date written YYYY-MM-DD")]
    Date,
    /// A date outside 2000..=2100.
    #[error("the charge date must be between the years 2000 and 2100")]
    Year,
    /// Neither `month` nor `year`.
    #[error("the period must be `month` or `year`")]
    Period,
}

/// A subscription's billing cycle: one charge date (`anchor`, the last charge or the next one)
/// and how often it repeats. The cycle runs both ways from the anchor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct BillingCycle {
    anchor: CivilDate,
    period: BillingPeriod,
}

impl BillingCycle {
    /// A cycle from a charge date in 2000..=2100.
    ///
    /// # Errors
    /// [`BillingError::Year`] for a date outside those years.
    pub fn new(anchor: CivilDate, period: BillingPeriod) -> Result<Self, BillingError> {
        if !(FIRST_YEAR..=LAST_YEAR).contains(&anchor.year()) {
            return Err(BillingError::Year);
        }
        Ok(Self { anchor, period })
    }

    /// A cycle from its wire form: `anchor` exactly `YYYY-MM-DD`, `period` `month` or `year`.
    ///
    /// # Errors
    /// [`BillingError`] naming the first part refused.
    pub fn parse(anchor: &str, period: &str) -> Result<Self, BillingError> {
        let anchor = parse_date(anchor).ok_or(BillingError::Date)?;
        let period = BillingPeriod::parse(period).ok_or(BillingError::Period)?;
        Self::new(anchor, period)
    }

    /// The charge date the person gave.
    #[must_use]
    pub fn anchor(self) -> CivilDate {
        self.anchor
    }

    /// How often it repeats.
    #[must_use]
    pub fn period(self) -> BillingPeriod {
        self.period
    }

    /// The first charge on or after `today`: a charge due today is today.
    #[must_use]
    pub fn next_charge(self, today: CivilDate) -> CivilDate {
        match self.period {
            BillingPeriod::Month => {
                let this = self.in_month(today.year(), today.month());
                if this >= today {
                    this
                } else if today.month() == 12 {
                    self.in_month(today.year() + 1, 1)
                } else {
                    self.in_month(today.year(), today.month() + 1)
                }
            }
            BillingPeriod::Year => {
                let this = self.in_month(today.year(), self.anchor.month());
                if this >= today {
                    this
                } else {
                    self.in_month(today.year() + 1, self.anchor.month())
                }
            }
        }
    }

    /// The charge in `month` of `year`: the anchor's day, or the month's last day.
    fn in_month(self, year: i64, month: u8) -> CivilDate {
        let day = self.anchor.day().min(days_in_month(year, month));
        // Every month has a day 1..=28 and `day` is at most its last one.
        CivilDate::new(year, month, day).unwrap_or(self.anchor)
    }
}

/// A calendar date written exactly `YYYY-MM-DD` (four, two and two digits).
#[must_use]
pub fn parse_date(text: &str) -> Option<CivilDate> {
    let bytes = text.as_bytes();
    let digits = |range: std::ops::Range<usize>| {
        bytes[range.clone()]
            .iter()
            .all(u8::is_ascii_digit)
            .then(|| text[range].parse::<u16>().ok())
            .flatten()
    };
    if bytes.len() != 10 || bytes[4] != b'-' || bytes[7] != b'-' {
        return None;
    }
    let year = digits(0..4)?;
    let month = u8::try_from(digits(5..7)?).ok()?;
    let day = u8::try_from(digits(8..10)?).ok()?;
    CivilDate::new(i64::from(year), month, day)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn date(text: &str) -> Result<CivilDate, String> {
        parse_date(text).ok_or_else(|| format!("not a date: {text}"))
    }

    fn next(anchor: &str, period: &str, today: &str) -> Result<String, String> {
        let cycle = BillingCycle::parse(anchor, period).map_err(|e| e.to_string())?;
        Ok(cycle.next_charge(date(today)?).to_string())
    }

    /// BIL-002: a monthly charge keeps its day; one due today is today, then next month.
    #[test]
    fn a_monthly_charge_is_today_then_next_month() -> Result<(), String> {
        assert_eq!(next("2026-11-24", "month", "2026-11-24")?, "2026-11-24");
        assert_eq!(next("2026-11-24", "month", "2026-11-25")?, "2026-12-24");
        assert_eq!(next("2026-11-24", "month", "2026-12-31")?, "2027-01-24");
        // The cycle runs backwards from an anchor in the future too.
        assert_eq!(next("2026-11-24", "month", "2026-10-25")?, "2026-11-24");
        assert_eq!(next("2027-03-24", "month", "2026-10-25")?, "2026-11-24");
        assert_eq!(next("2026-11-24", "month", "2026-10-24")?, "2026-10-24");
        Ok(())
    }

    /// BIL-002: a day a month lacks is that month's last day, and the anchor's day comes back.
    #[test]
    fn a_monthly_charge_on_the_31st_falls_on_short_months_last_day() -> Result<(), String> {
        assert_eq!(next("2026-01-31", "month", "2026-02-01")?, "2026-02-28");
        assert_eq!(next("2026-01-31", "month", "2028-02-01")?, "2028-02-29");
        assert_eq!(next("2026-01-31", "month", "2026-03-01")?, "2026-03-31");
        assert_eq!(next("2026-01-31", "month", "2026-04-01")?, "2026-04-30");
        assert_eq!(next("2026-04-30", "month", "2026-05-01")?, "2026-05-30");
        Ok(())
    }

    /// BIL-002: a yearly charge on 29 Feb is on 28 Feb in other years.
    #[test]
    fn a_yearly_charge_keeps_its_day_and_month() -> Result<(), String> {
        assert_eq!(next("2025-11-24", "year", "2026-11-24")?, "2026-11-24");
        assert_eq!(next("2025-11-24", "year", "2026-11-25")?, "2027-11-24");
        assert_eq!(next("2025-11-24", "year", "2026-01-10")?, "2026-11-24");
        assert_eq!(next("2024-02-29", "year", "2024-03-01")?, "2025-02-28");
        assert_eq!(next("2024-02-29", "year", "2027-03-01")?, "2028-02-29");
        Ok(())
    }

    /// BIL-003: only `YYYY-MM-DD` calendar dates in 2000..=2100 and the two periods.
    #[test]
    fn only_real_dates_in_range_and_known_periods_make_a_cycle() {
        for bad in ["2026-02-30", "24.11.2026", "2026-1-24", "2026-11-24 ", "+026-11-24", "2026-13-01", ""] {
            assert_eq!(BillingCycle::parse(bad, "month"), Err(BillingError::Date), "{bad:?}");
        }
        assert_eq!(BillingCycle::parse("1999-12-31", "month"), Err(BillingError::Year));
        assert_eq!(BillingCycle::parse("2101-01-01", "year"), Err(BillingError::Year));
        assert_eq!(BillingCycle::parse("2026-11-24", "week"), Err(BillingError::Period));
        assert!(BillingCycle::parse("2000-01-01", "month").is_ok());
        assert!(BillingCycle::parse("2100-12-31", "year").is_ok());
    }
}
