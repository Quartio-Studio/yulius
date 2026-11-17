//! Flaky tests (SPEC-497 OFC-002): a test that fails on unrelated changes is quarantined with an
//! issue at once, so it stops blocking every pull request; it is never run again until it passes.
//!
//! A test is flaky when it failed in the runs of [`FLAKY_PULLS`] different pull requests within
//! [`FLAKY_WINDOW_MS`], in the same job, while the integration branch did not fail it in that
//! window (a test the integration branch fails is broken there, not flaky). Its failures are
//! named as the failing tests' blocks name them (nextest's `crate tests::name`, libtest's
//! `tests::name`, [`super::failures::failed_test_names`]).
//!
//! A quarantined test is told to every CI job of its project in its environment: the names one
//! per line in [`QUARANTINE_ENV`], and a nextest filter of them in [`QUARANTINE_NEXTEST_ENV`]
//! (`-E "not ($YULIUS_CI_QUARANTINE_NEXTEST)"`); never through a file the job could have linked
//! elsewhere. A project's gate leaves them out; this repository's does. The quarantine ends when
//! its issue is closed.
//!
//! Pure: the caller records the failures, files the issue and keeps the list.

use crate::time::Timestamp;

/// How far back a test's failures in other pull requests count: 48 hours.
pub const FLAKY_WINDOW_MS: i64 = 48 * 60 * 60 * 1000;
/// The different pull requests a test must have failed in.
pub const FLAKY_PULLS: usize = 2;
/// The variable holding the quarantined tests' names, one per line (empty: none).
pub const QUARANTINE_ENV: &str = "YULIUS_CI_QUARANTINE";
/// The variable holding a nextest filter matching every quarantined test (empty: none).
pub const QUARANTINE_NEXTEST_ENV: &str = "YULIUS_CI_QUARANTINE_NEXTEST";

/// One failure of a test: the reference whose run failed it (`pr/<n>`, `branch/<name>`) and when.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TestFailure {
    /// The run's reference.
    pub reference: String,
    /// When the job ended.
    pub at: Timestamp,
}

/// Whether a test that failed `failures` (any order, this job's) is flaky at `now`: it failed in
/// the runs of at least [`FLAKY_PULLS`] different pull requests within [`FLAKY_WINDOW_MS`], and
/// the integration branch (`integration`, its reference `branch/<name>`) did not fail it then.
///
/// # Examples
/// ```
/// use yulius_domain::ci::flaky::{TestFailure, is_flaky};
/// use yulius_domain::time::Timestamp;
/// let failed = |reference: &str, at: i64| TestFailure { reference: reference.into(), at: Timestamp(at) };
/// let now = Timestamp(10_000_000);
/// assert!(is_flaky(&[failed("pr/3", 9_000_000), failed("pr/7", 9_500_000)], "branch/dev", now));
/// assert!(!is_flaky(&[failed("pr/3", 9_000_000), failed("pr/3", 9_500_000)], "branch/dev", now));
/// ```
#[must_use]
pub fn is_flaky(failures: &[TestFailure], integration: &str, now: Timestamp) -> bool {
    let recent: Vec<&TestFailure> = failures
        .iter()
        .filter(|f| now.0.saturating_sub(f.at.0) <= FLAKY_WINDOW_MS)
        .collect();
    if recent.iter().any(|f| f.reference == integration) {
        return false;
    }
    let mut pulls: Vec<&str> = recent
        .iter()
        .map(|f| f.reference.as_str())
        .filter(|r| r.starts_with("pr/"))
        .collect();
    pulls.sort_unstable();
    pulls.dedup();
    pulls.len() >= FLAKY_PULLS
}

/// A nextest filter matching every test in `names` (`crate tests::name` as
/// `(binary_id(crate) & test(=tests::name))`, a bare `tests::name` as `test(=tests::name)`),
/// joined with `|`; empty for none. Names with characters a filter cannot hold are left out (a
/// gate never fails on a malformed filter).
///
/// # Examples
/// ```
/// use yulius_domain::ci::flaky::nextest_filter;
/// assert_eq!(
///     nextest_filter(&["yulius::main local_ci::slow".into(), "tests::a".into()]),
///     "(binary_id(yulius::main) & test(=local_ci::slow)) | test(=tests::a)"
/// );
/// assert_eq!(nextest_filter(&[]), "");
/// ```
#[must_use]
pub fn nextest_filter(names: &[String]) -> String {
    let safe = |part: &str| {
        !part.is_empty()
            && part
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | ':' | '-' | '.' | '/'))
    };
    names
        .iter()
        .filter_map(|name| match name.split_once(' ') {
            Some((binary, test)) if safe(binary) && safe(test) => {
                Some(format!("(binary_id({binary}) & test(={test}))"))
            }
            None if safe(name) => Some(format!("test(={name})")),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join(" | ")
}

#[cfg(test)]
mod tests {
    use super::*;

    const HOUR: i64 = 60 * 60 * 1000;

    fn failed(reference: &str, at: i64) -> TestFailure {
        TestFailure {
            reference: reference.into(),
            at: Timestamp(at),
        }
    }

    /// SPEC-497 OFC-002: two pull requests within 48 hours make a test flaky; one pull request's
    /// runs, failures older than the window, or the integration branch failing it do not.
    #[test]
    fn a_test_failing_on_two_pull_requests_is_flaky() {
        let now = Timestamp(100 * HOUR);
        assert!(is_flaky(
            &[failed("pr/3", 99 * HOUR), failed("pr/7", 60 * HOUR)],
            "branch/dev",
            now
        ));
        // One pull request, however many runs.
        assert!(!is_flaky(
            &[
                failed("pr/3", 99 * HOUR),
                failed("pr/3", 98 * HOUR),
                failed("pr/3", 97 * HOUR)
            ],
            "branch/dev",
            now
        ));
        // The other pull request's failure is older than 48 hours.
        assert!(!is_flaky(
            &[failed("pr/3", 99 * HOUR), failed("pr/7", 51 * HOUR)],
            "branch/dev",
            now
        ));
        // The integration branch failed it too: broken there, not flaky.
        assert!(!is_flaky(
            &[
                failed("pr/3", 99 * HOUR),
                failed("pr/7", 98 * HOUR),
                failed("branch/dev", 90 * HOUR)
            ],
            "branch/dev",
            now
        ));
        // Another branch's failure counts for nothing either way.
        assert!(!is_flaky(
            &[failed("pr/3", 99 * HOUR), failed("branch/main", 98 * HOUR)],
            "branch/dev",
            now
        ));
    }

    /// SPEC-497 OFC-002: the nextest filter of quarantined tests; a name a filter cannot hold
    /// is left out.
    #[test]
    fn the_quarantine_is_a_nextest_filter() {
        assert_eq!(
            nextest_filter(&[
                "browser images_and_proxy::browser_goes_through_the_proxy".into(),
                "person_lang::a_person_added".into(),
                "evil) | all() | (x".into(),
            ]),
            "(binary_id(browser) & test(=images_and_proxy::browser_goes_through_the_proxy)) | \
             test(=person_lang::a_person_added)"
        );
    }
}
