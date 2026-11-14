//! GitHub timestamps (`2026-10-07T12:00:00Z`) as the fakes write them; they are read by
//! `yulius_domain::time::parse_rfc3339`, the one reader (SPEC-483 slice 31).

use yulius_domain::cron::CivilDate;

/// Formats Unix seconds as `YYYY-MM-DDTHH:MM:SSZ`, as GitHub writes its timestamps.
pub(crate) fn format_rfc3339(unix_seconds: i64) -> String {
    let rem = unix_seconds.rem_euclid(86_400);
    let Some(date) = CivilDate::from_days(unix_seconds.div_euclid(86_400)) else {
        return String::new();
    };
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
        date.year(),
        date.month(),
        date.day(),
        rem / 3600,
        (rem % 3600) / 60,
        rem % 60
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use yulius_domain::time::{Timestamp, parse_rfc3339};

    #[test]
    fn round_trips_known_instants() {
        assert_eq!(
            parse_rfc3339("2016-07-11T22:14:10Z"),
            Some(Timestamp(1_468_275_250_000))
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
}
