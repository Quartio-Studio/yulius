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
