//! Epoch conversions used by the provider transcripts; RFC 3339 is read by
//! `yulius_domain::time::parse_rfc3339`.

use yulius_domain::time::Timestamp;

/// Converts Unix epoch seconds (as used by provider reset times) to a [`Timestamp`].
#[must_use]
pub fn from_epoch_seconds(seconds: i64) -> Option<Timestamp> {
    seconds.checked_mul(1_000).map(Timestamp)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn epoch_seconds_convert_or_overflow() {
        assert_eq!(
            from_epoch_seconds(1_760_000_000),
            Some(Timestamp(1_760_000_000_000))
        );
        assert_eq!(from_epoch_seconds(i64::MAX), None);
    }
}
