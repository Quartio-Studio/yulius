//! Flaky tests (SPEC-497 OFC-002): a test that fails on unrelated changes is quarantined with an
//! issue at once, so it stops blocking every pull request; it is never run again until it passes.
//!
//! A test is flaky when it failed in the runs of [`FLAKY_PULLS`] different pull requests within
//! [`FLAKY_WINDOW_MS`], in the same job, whose heads share no commit outside the integration
//! branch (a chained pull request carries its predecessor's commits, and a break in those fails
//! both), while the integration branch did not fail it in that window (a test the integration
//! branch fails is broken there, not flaky). Its failures are named as the failing tests' blocks
//! name them (nextest's `crate tests::name`, libtest's `tests::name`,
//! [`super::failures::failed_test_names`]); only a name [`quarantinable`] accepts is quarantined.
//! A pull request's failure whose block names a file it changed ([`named_changed_file`]) is taken
//! as the change's own and does not count (SPEC-618, ADR-0453).
//!
//! A quarantined test is told to every CI job of its project in its environment: the names one
//! per line in [`QUARANTINE_ENV`], and a nextest filter of their test paths in
//! [`QUARANTINE_NEXTEST_ENV`] (`-E "not ($YULIUS_CI_QUARANTINE_NEXTEST)"`); never through a file
//! the job could have linked elsewhere. A project's gate leaves them out; this repository's does.
//! The quarantine ends when its issue is closed.
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

/// Longest test name quarantined: its issue's title ("Flaky test quarantined: " and the name)
/// stays within GitHub's 256 (review round 2).
const MAX_NAME_BYTES: usize = 200;

/// One failure of a test: the reference whose run failed it (`pr/<n>`, `branch/<name>`), the
/// commit that run tested, and when.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TestFailure {
    /// The run's reference.
    pub reference: String,
    /// The run's commit.
    pub sha: String,
    /// When the job ended.
    pub at: Timestamp,
}

/// Whether a test that failed `failures` (any order, this job's) is flaky at `now`: it failed in
/// the runs of at least [`FLAKY_PULLS`] different pull requests within [`FLAKY_WINDOW_MS`], two
/// of them at commits that are not `related` (they share a commit outside the integration
/// branch), and the integration branch (`integration`, its reference `branch/<name>`) did not
/// fail it then.
///
/// # Examples
/// ```
/// use yulius_domain::ci::flaky::{TestFailure, is_flaky};
/// use yulius_domain::time::Timestamp;
/// let failed = |reference: &str, sha: &str, at: i64| TestFailure {
///     reference: reference.into(),
///     sha: sha.into(),
///     at: Timestamp(at),
/// };
/// let now = Timestamp(10_000_000);
/// let unrelated = |_: &str, _: &str| false;
/// let two = [failed("pr/3", "a", 9_000_000), failed("pr/7", "b", 9_500_000)];
/// assert!(is_flaky(&two, "branch/dev", now, unrelated));
/// // A chained pull request carries the other's commits.
/// assert!(!is_flaky(&two, "branch/dev", now, |_, _| true));
/// let one = [failed("pr/3", "a", 9_000_000), failed("pr/3", "c", 9_500_000)];
/// assert!(!is_flaky(&one, "branch/dev", now, unrelated));
/// ```
#[must_use]
pub fn is_flaky(
    failures: &[TestFailure],
    integration: &str,
    now: Timestamp,
    mut related: impl FnMut(&str, &str) -> bool,
) -> bool {
    let recent: Vec<&TestFailure> = failures
        .iter()
        .filter(|f| now.0.saturating_sub(f.at.0) <= FLAKY_WINDOW_MS)
        .collect();
    if recent.iter().any(|f| f.reference == integration) {
        return false;
    }
    let pulls: Vec<&TestFailure> = recent
        .into_iter()
        .filter(|f| f.reference.starts_with("pr/"))
        .collect();
    let mut distinct: Vec<&str> = pulls.iter().map(|f| f.reference.as_str()).collect();
    distinct.sort_unstable();
    distinct.dedup();
    if distinct.len() < FLAKY_PULLS {
        return false;
    }
    pulls.iter().enumerate().any(|(i, a)| {
        pulls[i + 1..]
            .iter()
            .any(|b| a.reference != b.reference && !related(&a.sha, &b.sha))
    })
}

/// The first of `changed` (the files a pull request changed, repository paths) that a failing
/// test's kept `block` names: such a failure is the change's own, not a sign that the test is
/// flaky (SPEC-618 FOC-001). A test that checks the repository's content names the file it
/// refuses, and fails in every pull request that breaks its rule.
///
/// A path is named whole, also after any `/` when it is nested (an absolute path of the checkout
/// ends with it), or as a printed relative path of at least two components that ends it (a test
/// running in its crate prints `tests/x.rs` for `crates/app/tests/x.rs`); never continued by a
/// path character or inside a word. So changed `src/lib.rs` is named by `crates/other/src/lib.rs`
/// too: such a match only keeps a failure from counting, a delayed quarantine (FOC-002).
///
/// # Examples
/// ```
/// use yulius_domain::ci::flaky::named_changed_file;
/// let changed = ["crates/app/tests/moves.rs".to_owned(), "README.md".to_owned()];
/// let block = "tests/moves.rs:404: sk-ant- and 35 token characters";
/// assert_eq!(named_changed_file(block, &changed), Some("crates/app/tests/moves.rs"));
/// assert_eq!(named_changed_file("assertion failed: the race lost", &changed), None);
/// assert_eq!(named_changed_file("panicked at crates/web/tests/moves.rs:4", &changed), None);
/// ```
#[must_use]
pub fn named_changed_file<'a>(block: &str, changed: &'a [String]) -> Option<&'a str> {
    changed
        .iter()
        .map(String::as_str)
        .find(|path| names_path(block, path))
}

/// Whether `text` names repository path `path` (see [`named_changed_file`]).
fn names_path(text: &str, path: &str) -> bool {
    if path.is_empty() {
        return false;
    }
    // An absolute path of the checkout ends with a nested path; a top-level file's name alone is
    // too common for that (`x/README.md` is another file).
    if mentions(text, path, path.contains('/')) {
        return true;
    }
    path.match_indices('/')
        .map(|(at, _)| &path[at + 1..])
        .filter(|rest| rest.contains('/'))
        .any(|rest| mentions(text, rest, false))
}

/// Whether `text` holds `word` as a whole path: not continued on its right, and on its left at
/// the text's start, after a character no path holds, after `./`, or (`after_slash`) after `/`.
fn mentions(text: &str, word: &str, after_slash: bool) -> bool {
    let in_path = |c: char| c.is_alphanumeric() || matches!(c, '_' | '-' | '.' | '/');
    text.match_indices(word).any(|(at, _)| {
        let before = &text[..at];
        let starts = match before.chars().next_back() {
            None => true,
            Some('/') if after_slash => true,
            Some('/') => before
                .strip_suffix("./")
                .is_some_and(|b| !b.chars().next_back().is_some_and(in_path)),
            Some(c) => !in_path(c),
        };
        let after = &text[at + word.len()..];
        let ends = match after.chars().next() {
            None => true,
            Some('.') => !after[1..].starts_with(|c: char| c.is_alphanumeric()),
            Some(c) => !in_path(c),
        };
        starts && ends
    })
}

/// Whether `name` (a failing test as its block names it) may be quarantined: letters, digits,
/// `_ : - . /`, and at most one space between a binary and its test, so it is safe in a filter,
/// an issue's text and a job's environment (no backtick, `@`, control character), at most 200
/// bytes.
///
/// # Examples
/// ```
/// use yulius_domain::ci::flaky::quarantinable;
/// assert!(quarantinable("yulius::main local_ci::slow"));
/// assert!(!quarantinable("app tests::x` @someone"));
/// assert!(!quarantinable("app tests::\u{0}x"));
/// ```
#[must_use]
pub fn quarantinable(name: &str) -> bool {
    let safe = |part: &str| {
        !part.is_empty()
            && !part.starts_with('/')
            && part
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | ':' | '-' | '.' | '/'))
    };
    name.len() <= MAX_NAME_BYTES
        && match name.split_once(' ') {
            Some((binary, test)) => safe(binary) && safe(test),
            None => safe(name),
        }
}

/// A nextest filter matching the test path of every name in `names` (`crate tests::name` and a
/// bare `tests::name` both as `test(=tests::name)`), joined with `|`; empty for none. The binary
/// is left out: nextest refuses a filter naming a binary the head does not build, and a gate
/// must never fail on the quarantine; a test of the same path in another binary is left out too.
/// Names [`quarantinable`] refuses are left out.
///
/// # Examples
/// ```
/// use yulius_domain::ci::flaky::nextest_filter;
/// assert_eq!(
///     nextest_filter(&["yulius::main local_ci::slow".into(), "tests::a".into()]),
///     "test(=local_ci::slow) | test(=tests::a)"
/// );
/// assert_eq!(nextest_filter(&[]), "");
/// ```
#[must_use]
pub fn nextest_filter(names: &[String]) -> String {
    let mut paths: Vec<&str> = names
        .iter()
        .filter(|name| quarantinable(name))
        .map(|name| name.split_once(' ').map_or(name.as_str(), |(_, test)| test))
        .collect();
    paths.dedup();
    paths
        .iter()
        .map(|path| format!("test(={path})"))
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
            sha: format!("{reference}@{at}"),
            at: Timestamp(at),
        }
    }

    fn unrelated(_: &str, _: &str) -> bool {
        false
    }

    /// SPEC-497 OFC-002: two pull requests within 48 hours make a test flaky; one pull request's
    /// runs, failures older than the window, or the integration branch failing it do not.
    #[test]
    fn a_test_failing_on_two_pull_requests_is_flaky() {
        let now = Timestamp(100 * HOUR);
        assert!(is_flaky(
            &[failed("pr/3", 99 * HOUR), failed("pr/7", 60 * HOUR)],
            "branch/dev",
            now,
            unrelated
        ));
        // One pull request, however many runs.
        assert!(!is_flaky(
            &[
                failed("pr/3", 99 * HOUR),
                failed("pr/3", 98 * HOUR),
                failed("pr/3", 97 * HOUR)
            ],
            "branch/dev",
            now,
            unrelated
        ));
        // The other pull request's failure is older than 48 hours.
        assert!(!is_flaky(
            &[failed("pr/3", 99 * HOUR), failed("pr/7", 51 * HOUR)],
            "branch/dev",
            now,
            unrelated
        ));
        // The integration branch failed it too: broken there, not flaky.
        assert!(!is_flaky(
            &[
                failed("pr/3", 99 * HOUR),
                failed("pr/7", 98 * HOUR),
                failed("branch/dev", 90 * HOUR)
            ],
            "branch/dev",
            now,
            unrelated
        ));
        // Two pull requests whose heads share a commit outside the integration branch (a chained
        // pull request, one recreated from the same branch) are one change (review round 1).
        let two = [failed("pr/3", 99 * HOUR), failed("pr/7", 98 * HOUR)];
        assert!(!is_flaky(&two, "branch/dev", now, |_, _| true));
        // Only the pair of different pull requests is asked; one unrelated pair is enough.
        let three = [
            failed("pr/3", 99 * HOUR),
            failed("pr/7", 98 * HOUR),
            failed("pr/9", 97 * HOUR),
        ];
        let mut asked = Vec::new();
        assert!(is_flaky(&three, "branch/dev", now, |a, b| {
            asked.push((a.to_owned(), b.to_owned()));
            a.starts_with("pr/3") && b.starts_with("pr/7")
        }));
        assert!(asked.iter().all(|(a, b)| a != b), "{asked:?}");
        // Another branch's failure counts for nothing either way.
        assert!(!is_flaky(
            &[failed("pr/3", 99 * HOUR), failed("branch/main", 98 * HOUR)],
            "branch/dev",
            now,
            unrelated
        ));
    }

    /// SPEC-618 FOC-002: a changed file is named whole, after an absolute path's boundary, or as
    /// a relative path of two components or more that the block prints; never inside another
    /// path or word.
    #[test]
    fn a_block_names_a_changed_file_by_its_path() {
        let changed = [
            "crates/yulius/tests/subscription_moves.rs".to_owned(),
            "crates/other/src/lib.rs".to_owned(),
            "README.md".to_owned(),
        ];
        let named = |block: &str| named_changed_file(block, &changed);
        let moves = Some("crates/yulius/tests/subscription_moves.rs");
        // fixture_hygiene's own words (PR #1470), at a line's start, after `./` or a checkout.
        assert_eq!(
            named("    tests/subscription_moves.rs:404: sk-ant- and 35 token characters"),
            moves
        );
        assert_eq!(named("tests/subscription_moves.rs"), moves);
        assert_eq!(named("see ./tests/subscription_moves.rs."), moves);
        assert_eq!(
            named("panicked at crates/yulius/tests/subscription_moves.rs:9:5:"),
            moves
        );
        assert_eq!(
            named("(/ci/job/crates/yulius/tests/subscription_moves.rs)"),
            moves
        );
        assert_eq!(named("`yulius/tests/subscription_moves.rs`"), moves);
        assert_eq!(
            named("crates/other/src/lib.rs:3"),
            Some("crates/other/src/lib.rs")
        );
        // Printed in its crate: two components stand for the nested file.
        assert_eq!(named("src/lib.rs:4"), Some("crates/other/src/lib.rs"));
        assert_eq!(named("README.md is stale"), Some("README.md"));
        for none in [
            // One component is too little to stand for a nested file.
            "subscription_moves.rs:404",
            // Inside another path or word.
            "crates/yulius-app/src/lib.rs:4",
            "web/tests/subscription_moves.rs:1",
            "a./tests/subscription_moves.rs",
            "tests/subscription_moves.rs.bak",
            "tests/subscription_moves.rsx",
            "tests/subscription_moves.rs/x",
            "docs/README.md",
            "NOT_README.md",
            "assertion failed: the race lost",
            "",
        ] {
            assert_eq!(named(none), None, "{none:?}");
        }
        // A nested path after any `/`: a delay at worst, never a wrong quarantine (review round 1).
        let one_crate = ["src/lib.rs".to_owned()];
        assert_eq!(
            named_changed_file("panicked at crates/other/src/lib.rs:3:5", &one_crate),
            Some("src/lib.rs")
        );
        assert_eq!(named_changed_file("x", &[String::new()]), None);
        assert_eq!(named_changed_file("anything", &[]), None);
    }

    /// SPEC-497 OFC-002: the nextest filter of quarantined tests names test paths only (a binary
    /// the head lacks fails nextest, review round 1); a name a filter cannot hold is left out.
    #[test]
    fn the_quarantine_is_a_nextest_filter() {
        assert_eq!(
            nextest_filter(&[
                "browser images_and_proxy::browser_goes_through_the_proxy".into(),
                "person_lang::a_person_added".into(),
                "evil) | all() | (x".into(),
                "/a tests::x".into(),
            ]),
            "test(=images_and_proxy::browser_goes_through_the_proxy) | \
             test(=person_lang::a_person_added)"
        );
    }

    /// SPEC-497 OFC-002: only names safe in a filter, an issue and an environment are
    /// quarantined (review round 1).
    #[test]
    fn only_plain_names_are_quarantined() {
        for good in [
            "app tests::flaky",
            "tests::a",
            "yulius::main local_ci::x_y-z.w/v",
        ] {
            assert!(quarantinable(good), "{good}");
        }
        for bad in [
            "",
            "app tests::x`@someone",
            "app tests::a\u{0}b",
            "app tests::a\nb",
            "app tests::a b",
            "/a tests::x",
            "app /./",
            "app (tests::x)",
        ] {
            assert!(!quarantinable(bad), "{bad:?}");
        }
        assert!(quarantinable(&format!("app tests::{}", "a".repeat(189))));
        assert!(!quarantinable(&format!("app tests::{}", "a".repeat(190))));
    }
}
