//! Waiting on a condition instead of sleeping.

use std::future::Future;
use std::time::Duration;

use super::TestResult;

/// How often [`eventually`] asks again.
const POLL: Duration = Duration::from_millis(50);

/// What a polled condition answers: whether it holds yet, or an error that ends the wait.
pub trait Settled {
    fn settled(self) -> Result<bool, Box<dyn std::error::Error>>;
}

impl Settled for bool {
    fn settled(self) -> Result<bool, Box<dyn std::error::Error>> {
        Ok(self)
    }
}

impl Settled for Result<bool, Box<dyn std::error::Error>> {
    fn settled(self) -> Result<bool, Box<dyn std::error::Error>> {
        self
    }
}

/// Asks `holds` until it answers `true`; fails with "timed out waiting for {what}" once `within`
/// has passed, or at once with the error `holds` returns. It asks once more at the deadline.
pub async fn eventually<F, Fut>(what: &str, within: Duration, mut holds: F) -> TestResult
where
    F: FnMut() -> Fut,
    Fut: Future,
    Fut::Output: Settled,
{
    let deadline = tokio::time::Instant::now() + within;
    loop {
        if holds().await.settled()? {
            return Ok(());
        }
        if tokio::time::Instant::now() >= deadline {
            return Err(format!("timed out waiting for {what}").into());
        }
        tokio::time::sleep(POLL).await;
    }
}
