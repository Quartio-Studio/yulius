//! System clock.

use std::time::{SystemTime, UNIX_EPOCH};

use yulius_app::ports::Clock;
use yulius_domain::time::Timestamp;

/// Wall clock in milliseconds since the Unix epoch.
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> Timestamp {
        let ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0);
        Timestamp(i64::try_from(ms).unwrap_or(i64::MAX))
    }
}
