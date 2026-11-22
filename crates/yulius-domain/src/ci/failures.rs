//! Failing tests' output kept from a CI job's whole output (SPEC-145, ADR-0119).
//!
//! A job's stored output is the end of what it printed, and a test runner prints a failing
//! test's output when the test fails, long before the job ends: in a large suite that block is
//! gone from the end. [`FailureScanner`] reads everything the job prints, line by line, and keeps
//! the failing tests' blocks, bounded: nextest's (a failure status line such as
//! `FAIL [   0.5s] (290/1670) crate name` and the indented `stdout ───` / `stderr ───` block after
//! it), libtest's (`---- name stdout ----` until `failures:` or `test result:`), and the browser
//! runner's failure summary headers.

/// Most bytes kept of one failing test's block (its end, where the panic message is).
pub const MAX_TEST_BYTES: usize = 8 * 1024;
/// Most bytes kept for one job; further failing tests are counted, not kept.
pub const MAX_JOB_BYTES: usize = 32 * 1024;
/// Longest line read; the rest of a longer line is dropped.
const MAX_LINE_BYTES: usize = 4 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Nextest,
    Libtest,
}

#[derive(Debug)]
struct Block {
    kind: Kind,
    header: String,
    body: String,
    /// Bytes dropped from the start of `body`.
    cut: usize,
}

impl Block {
    fn push(&mut self, line: &str) {
        self.body.push_str(line);
        self.body.push('\n');
        if self.body.len() > 2 * MAX_TEST_BYTES {
            self.cut_to(MAX_TEST_BYTES);
        }
    }

    fn cut_to(&mut self, max: usize) {
        if self.body.len() <= max {
            return;
        }
        let mut start = self.body.len() - max;
        while !self.body.is_char_boundary(start) {
            start += 1;
        }
        self.cut += start;
        self.body.drain(..start);
    }
}

/// Follows a job's output and keeps its failing tests' blocks (see the module).
#[derive(Debug, Default)]
pub struct FailureScanner {
    /// The line being read (at most [`MAX_LINE_BYTES`]).
    partial: Vec<u8>,
    /// Bytes of the current line past [`MAX_LINE_BYTES`].
    overlong: bool,
    block: Option<Block>,
    /// Inside libtest's `successes:` (passing tests' output shown with `--show-output`).
    successes: bool,
    kept: String,
    omitted: usize,
    /// A line read as a failure in any test runner's or tool's words
    /// ([`crate::ci::infra::reads_as_failure`], SPEC-497).
    failure_seen: bool,
}

impl FailureScanner {
    /// Whether any line read so far reads as a failure in a test runner's or tool's words, kept
    /// as a block or not (SPEC-497 OFC-001: such a job is never run again).
    #[must_use]
    pub fn failure_seen(&self) -> bool {
        // A last line with no newline yet counts too (review round 4).
        self.failure_seen
            || (!self.partial.is_empty()
                && crate::ci::infra::reads_as_failure(&String::from_utf8_lossy(&self.partial)))
    }

    /// Reads the next bytes of the output.
    pub fn push(&mut self, bytes: &[u8]) {
        for chunk in bytes.split_inclusive(|&b| b == b'\n') {
            let (text, ended) = match chunk.strip_suffix(b"\n") {
                Some(text) => (text, true),
                None => (chunk, false),
            };
            let room = MAX_LINE_BYTES.saturating_sub(self.partial.len());
            if text.len() > room {
                self.overlong = true;
            }
            self.partial
                .extend_from_slice(&text[..text.len().min(room)]);
            if ended {
                self.end_line();
            }
        }
    }

    /// The failing tests' output kept, with how many more failed without being kept; `None`
    /// when no failing test was seen.
    #[must_use]
    pub fn finish(mut self) -> Option<String> {
        if !self.partial.is_empty() {
            self.end_line();
        }
        self.close();
        if self.kept.is_empty() && self.omitted == 0 {
            return None;
        }
        let mut out = self.kept;
        if self.omitted > 0 {
            out.push_str(&format!(
                "[yulius] {} more failing test{}' output not kept\n",
                self.omitted,
                if self.omitted == 1 { "" } else { "s" }
            ));
        }
        Some(out)
    }

    fn end_line(&mut self) {
        let bytes = std::mem::take(&mut self.partial);
        let mut line = String::from_utf8_lossy(&bytes).into_owned();
        if std::mem::take(&mut self.overlong) {
            line.push('…');
        }
        self.line(&line);
    }

    fn line(&mut self, line: &str) {
        if !self.failure_seen && crate::ci::infra::reads_as_failure(line) {
            self.failure_seen = true;
        }
        match line {
            "successes:" => self.successes = true,
            "failures:" => self.successes = false,
            summary if summary.starts_with("test result:") => self.successes = false,
            _ => {}
        }
        if let Some(block) = &mut self.block {
            let goes_on = match block.kind {
                // Its output is indented; the next status line, the run's separator or anything
                // printed at the margin ends it.
                Kind::Nextest => {
                    line.trim().is_empty()
                        || (line.starts_with("  ") && nextest_status(line).is_none())
                }
                Kind::Libtest => {
                    line != "failures:"
                        && line != "successes:"
                        && !line.starts_with("test result:")
                        && libtest_header(line).is_none()
                }
            };
            if goes_on {
                block.push(line);
                return;
            }
            self.close();
        }
        // The browser runner lists each failed case in its final summary. The header alone
        // identifies it, even when the rest of the log falls out of the output ring.
        if !self.successes
            && let Some((file, title)) = browser_failure(line)
        {
            // Use the existing stored stdout header format. A raw browser FAIL line inside
            // a Rust test's output stays ordinary output when the watch reads the blocks.
            self.keep(&format!("---- {file} › {title} stdout ----\n\n"));
            return;
        }
        let libtest = !self.successes && libtest_header(line).is_some();
        if libtest || nextest_status(line).is_some_and(nextest_failed) {
            let kind = if libtest {
                Kind::Libtest
            } else {
                Kind::Nextest
            };
            self.block = Some(Block {
                kind,
                header: line.trim().to_owned(),
                body: String::new(),
                cut: 0,
            });
        }
    }

    /// Keeps the current block, unless it has no output (nextest's summary repeats the failed
    /// tests' status lines alone).
    fn close(&mut self) {
        let Some(mut block) = self.block.take() else {
            return;
        };
        if block.body.trim().is_empty() {
            return;
        }
        block.cut_to(MAX_TEST_BYTES);
        let mut entry = format!("{}\n", block.header);
        if block.cut > 0 {
            entry.push_str(&format!(
                "[yulius] the first {} bytes of this test's output are not kept\n",
                block.cut
            ));
        }
        entry.push_str(block.body.trim_end());
        entry.push_str("\n\n");
        self.keep(&entry);
    }

    fn keep(&mut self, entry: &str) {
        if self.kept.len() + entry.len() > MAX_JOB_BYTES {
            self.omitted += 1;
        } else {
            self.kept.push_str(entry);
        }
    }
}

/// The names of the failing tests in output [`FailureScanner::finish`] kept, in order, each once:
/// nextest's `FAIL [   0.1s] (1/4) crate tests::bad` as `crate tests::bad`, libtest's
/// `---- tests::bad stdout ----` as `tests::bad` (SPEC-444 PRN-002). Only the blocks' headers are
/// read: the first line and lines after an empty one, at the margin.
#[must_use]
pub fn failed_test_names(kept: &str) -> Vec<String> {
    failed_test_blocks(kept)
        .into_iter()
        .map(|(name, _)| name)
        .collect()
}

/// Each failing test [`failed_test_names`] reads, in its order, with its kept block: the header
/// and every line up to the next test's header, a test kept twice (a retry) as both blocks
/// joined (SPEC-618 FOC-001). Lines before the first header belong to no test.
///
/// # Examples
/// ```
/// use yulius_domain::ci::failures::failed_test_blocks;
/// let kept = "FAIL [ 0.1s] (1/2) app a\n  stderr ───\n    tests/x.rs:4: bad\n\n\
///             FAIL [ 0.2s] (2/2) app b\n  stderr ───\n    other\n";
/// let blocks = failed_test_blocks(kept);
/// assert_eq!(blocks[0].0, "app a");
/// assert!(blocks[0].1.contains("tests/x.rs:4") && !blocks[0].1.contains("other"));
/// assert!(blocks[1].1.contains("other") && !blocks[1].1.contains("tests/x.rs"));
/// ```
#[must_use]
pub fn failed_test_blocks(kept: &str) -> Vec<(String, String)> {
    let mut blocks: Vec<(String, String)> = Vec::new();
    let mut current: Option<usize> = None;
    let mut starts_block = true;
    for line in kept.lines() {
        let header = std::mem::replace(&mut starts_block, line.is_empty());
        let name = header
            .then(|| libtest_header(line).or_else(|| kept_nextest_name(line)))
            .flatten()
            .map(str::trim)
            .filter(|n| !n.is_empty());
        if let Some(name) = name {
            current = Some(match blocks.iter().position(|(known, _)| known == name) {
                Some(at) => at,
                None => {
                    blocks.push((name.to_owned(), String::new()));
                    blocks.len() - 1
                }
            });
        }
        if let Some(block) = current.and_then(|at| blocks.get_mut(at)) {
            block.1.push_str(line);
            block.1.push('\n');
        }
    }
    blocks
}

/// How many failing tests' output [`FailureScanner::finish`] counted but did not keep (its
/// `[yulius] N more failing tests' output not kept` line); 0 when it kept every one.
///
/// # Examples
/// ```
/// use yulius_domain::ci::failures::omitted_tests;
/// assert_eq!(omitted_tests("FAIL [ 0.1s] (1/9) app a\n[yulius] 17 more failing tests' output not kept\n"), 17);
/// assert_eq!(omitted_tests("FAIL [ 0.1s] (1/9) app a\n"), 0);
/// ```
#[must_use]
pub fn omitted_tests(kept: &str) -> usize {
    kept.lines()
        .rev()
        .find_map(|line| {
            let rest = line.strip_prefix("[yulius] ")?;
            let (count, words) = rest.split_once(' ')?;
            words
                .starts_with("more failing test")
                .then(|| count.parse().ok())
                .flatten()
        })
        .unwrap_or(0)
}

/// The e2e runner's uncolored summary (CI sets NO_COLOR). Allow its single leading space,
/// or the header at the margin, but never an indented test's own output.
fn browser_failure(line: &str) -> Option<(&str, &str)> {
    let summary = line
        .strip_prefix(' ')
        .unwrap_or(line)
        .strip_prefix("FAIL  |")?;
    let (target, case) = summary.split_once("| ")?;
    let (file, title) = case.split_once(" > ")?;
    (!target.is_empty() && file.ends_with(".e2e.ts") && !title.trim().is_empty())
        .then_some((file, title))
}

/// The test a kept nextest header names (`FAIL [   0.1s] (1/4) crate name` at the margin).
fn kept_nextest_name(line: &str) -> Option<&str> {
    if line.starts_with(char::is_whitespace) {
        return None;
    }
    let (status, rest) = line.split_once('[')?;
    let status = status.trim_end();
    let capitals = status.starts_with(|c: char| c.is_ascii_uppercase())
        && status
            .chars()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '-' || c == ' ');
    if !capitals || !nextest_failed(status) {
        return None;
    }
    let (duration, rest) = rest.split_once(']')?;
    if !duration
        .trim_start()
        .trim_start_matches('>')
        .starts_with(|c: char| c.is_ascii_digit())
    {
        return None;
    }
    let rest = rest.trim_start();
    let rest = match rest.strip_prefix('(') {
        Some(counted) => counted.split_once(')')?.1,
        None => rest,
    };
    Some(rest.trim())
}

/// A nextest status line's status (`FAIL`, `PASS`, `SIGSEGV`, `TRY 2 FAIL`…): capitals
/// right-aligned to 12 columns before a bracketed duration such as `[   0.547s]` or `[>30.000s]`.
/// A test's own output is indented by 4, so a status line it printed does not line up.
fn nextest_status(line: &str) -> Option<&str> {
    let text = line.trim_start();
    let indent = line.len() - text.len();
    let (status, rest) = text.split_once('[')?;
    let status = status.trim_end();
    if indent == 0 || (indent + status.len() != 12 && status.len() < 12) {
        return None;
    }
    let duration = rest.trim_start().trim_start_matches('>');
    let capitals = !status.is_empty()
        && status.starts_with(|c: char| c.is_ascii_uppercase())
        && status
            .chars()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '-' || c == ' ');
    (capitals && duration.starts_with(|c: char| c.is_ascii_digit())).then_some(status)
}

fn nextest_failed(status: &str) -> bool {
    status.contains("FAIL")
        || status.starts_with("SIG")
        || status.contains("TIMEOUT")
        || status.contains("ABORT")
}

/// libtest's `---- name stdout ----`.
fn libtest_header(line: &str) -> Option<&str> {
    line.strip_prefix("---- ")?.strip_suffix(" stdout ----")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// SPEC-497 OFC-001 (review round 4): a last line with no newline is read too.
    #[test]
    fn a_failure_on_the_unfinished_last_line_is_seen() {
        let mut scanner = FailureScanner::default();
        scanner.push(b"running\n--- FAIL: TestTotal");
        assert!(scanner.failure_seen());
    }

    /// SPEC-497 OFC-001 (review round 3): a failure in any runner's words is seen wherever it is
    /// printed, kept as a block or not; a passing summary and Docker's own words are not.
    #[test]
    fn a_failure_in_any_runners_words_is_seen() {
        for line in [
            "--- FAIL: TestTotal (0.00s)",
            "FAILED tests/test_x.py::test_total - assert 1 == 2",
            "Tests:       1 failed, 3 passed, 4 total",
            "not ok 3 - totals",
        ] {
            let mut scanner = FailureScanner::default();
            scanner.push(format!("starting\n{line}\nCannot connect to the Docker daemon at unix:///var/run/docker.sock.\n").as_bytes());
            assert!(scanner.failure_seen(), "{line}");
        }
        let mut passing = FailureScanner::default();
        passing.push(
            b"test result: ok. 5 passed; 0 failed; 0 ignored\nError response from daemon: could \
              not find an available, non-overlapping IPv4 address pool among the defaults to \
              assign to the network\n",
        );
        assert!(!passing.failure_seen());
    }

    fn scan(chunks: &[&str]) -> Option<String> {
        let mut scanner = FailureScanner::default();
        for chunk in chunks {
            scanner.push(chunk.as_bytes());
        }
        scanner.finish()
    }

    #[test]
    fn browser_failure_headers_survive_the_scanner_and_name_the_tests() {
        let kept = scan(&[
            " PASS  |chromium| tests/editor.e2e.ts > opens a file\n",
            " FAIL  || tests/editor.e2e.ts > missing target\n",
            " FAIL  |chromium| tests/editor.rs > wrong file type\n",
            " FAIL  |chromium| tests/editor.e2e.ts >    \n",
            " FAIL  |chromium| tests/editor.e2e.ts > opens a folder\n",
            " FAIL  |chromium| tests/tasks.e2e.ts > closes a task\n",
            " FAIL  |chromium| tests/editor.e2e.ts > opens a folder\n",
        ])
        .unwrap_or_default();
        assert_eq!(
            failed_test_names(&kept),
            [
                "tests/editor.e2e.ts › opens a folder",
                "tests/tasks.e2e.ts › closes a task",
            ]
        );
    }

    #[test]
    fn rust_test_output_cannot_contribute_browser_failure_names() {
        let browser = " FAIL  |chromium| tests/example.e2e.ts > example";
        let success =
            format!("successes:\n\n---- passing stdout ----\n\n{browser}\n\ntest result: ok.\n");
        assert_eq!(scan(&[&success]), None);
        let after_success = scan(&[
            &success,
            " FAIL  |chromium| tests/real.e2e.ts > fails after Rust tests\n",
        ])
        .unwrap_or_default();
        assert_eq!(
            failed_test_names(&after_success),
            ["tests/real.e2e.ts › fails after Rust tests"]
        );
        let failed = format!(
            "failures:\n\n---- failing stdout ----\nfirst line\n\n{browser}\n\nfailures:\n"
        );
        assert_eq!(
            failed_test_names(&scan(&[&failed]).unwrap_or_default()),
            ["failing"]
        );
        let mixed = scan(&[&failed, browser, "\n"]).unwrap_or_default();
        assert_eq!(
            failed_test_names(&mixed),
            ["failing", "tests/example.e2e.ts › example"]
        );
    }

    /// cargo-nextest 0.9.146 with `--profile ci`, captured from a run with two failing tests.
    const NEXTEST: &str = "\
    Finished `test` profile [unoptimized + debuginfo] target(s) in 3.31s
────────────
 Nextest run ID 08e7df9d with nextest profile: ci
    Starting 4 tests across 1 binary
        FAIL [   0.093s] (1/4) nt tests::bad2
  stdout ───

    running 1 test
    test tests::bad2 ... FAILED

    failures:

    failures:
        tests::bad2

    test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 3 filtered out; finished in 0.01s

  stderr ───

    thread 'tests::bad2' (2314) panicked at src/lib.rs:6:25:
    second failure
    note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace

        PASS [   0.095s] (2/4) nt tests::ok2
        PASS [   0.103s] (3/4) nt tests::ok1
        FAIL [   0.111s] (4/4) nt tests::bad
  stdout ───

    running 1 test
    some stdout line
    test tests::bad ... FAILED

  stderr ───
    some stderr line

    thread 'tests::bad' (2317) panicked at src/lib.rs:4:85:
    assertion `left == right` failed: the message
      left: 1
     right: 2

────────────
     Summary [   0.121s] 4 tests run: 2 passed, 2 failed, 0 skipped
        FAIL [   0.093s] (1/4) nt tests::bad2
        FAIL [   0.111s] (4/4) nt tests::bad
error: test run failed
";

    #[test]
    fn nextest_failures_are_kept_with_their_output_and_nothing_else() {
        let kept = scan(&[NEXTEST]).unwrap_or_default();
        assert!(
            kept.starts_with("FAIL [   0.093s] (1/4) nt tests::bad2\n  stdout ───"),
            "{kept}"
        );
        assert!(kept.contains("second failure"), "{kept}");
        assert!(
            kept.contains("FAIL [   0.111s] (4/4) nt tests::bad\n"),
            "{kept}"
        );
        assert!(
            kept.contains("assertion `left == right` failed: the message"),
            "{kept}"
        );
        assert!(kept.contains("some stderr line"), "{kept}");
        for absent in [
            "PASS",
            "Summary",
            "error: test run failed",
            "Starting",
            "────────────",
        ] {
            assert!(!kept.contains(absent), "{absent} in {kept}");
        }
        // The summary's repeated status lines (no output) are not kept again.
        assert_eq!(kept.matches("nt tests::bad2").count(), 1, "{kept}");
    }

    #[test]
    fn lines_split_across_reads_are_joined() {
        // Reads of 7 bytes split lines and the three bytes of `─`.
        let mut scanner = FailureScanner::default();
        for chunk in NEXTEST.as_bytes().chunks(7) {
            scanner.push(chunk);
        }
        let whole = scan(&[NEXTEST]);
        assert!(whole.is_some());
        assert_eq!(scanner.finish(), whole);
    }

    #[test]
    fn passing_output_keeps_nothing() {
        let passing = "    Starting 2 tests across 1 binary\n        PASS [   0.1s] (1/2) a b\n        \
                       PASS [   0.2s] (2/2) a c\n────────────\n     Summary [   0.3s] 2 tests run: 2 passed, 0 skipped\n";
        assert_eq!(scan(&[passing]), None);
        // A test whose name says FAIL, and a slow one, are not failures.
        let named = "        PASS [   0.1s] (1/2) a when_it_fails_it_says_FAIL\n  stdout ───\n    x\n        SLOW [>30.000s] (2/2) a slow\n";
        assert_eq!(scan(&[named]), None);
    }

    #[test]
    fn signals_and_timeouts_are_failures() {
        let crash = "     SIGSEGV [   0.2s] (1/1) a crash\n  stderr ───\n    boom\n";
        assert!(scan(&[crash]).is_some_and(|k| k.contains("boom")));
        let timeout = "     TIMEOUT [ 120.0s] (1/1) a hang\n  stdout ───\n    waiting\n";
        assert!(scan(&[timeout]).is_some_and(|k| k.contains("waiting")));
    }

    /// SPEC-444 PRN-002: the names of the failing tests, from what the scanner kept.
    #[test]
    fn failing_test_names_are_read_from_the_kept_headers() {
        let kept = scan(&[NEXTEST]).unwrap_or_default();
        assert_eq!(
            failed_test_names(&kept),
            vec!["nt tests::bad2".to_owned(), "nt tests::bad".to_owned()]
        );
        let libtest = "running 1 test\ntest bad ... FAILED\n\nfailures:\n\n\
                       ---- tests::bad stdout ----\nFAIL [ 1.0s] printed by the test\n\n\
                       failures:\n    tests::bad\n\ntest result: FAILED. 0 passed; 1 failed\n";
        let kept = scan(&[libtest]).unwrap_or_default();
        assert_eq!(failed_test_names(&kept), vec!["tests::bad".to_owned()]);
        let retried = "TRY 2 FAIL [   0.5s] (1/1) a retried\n  stderr ───\n    again\n\n\
                       SIGSEGV [>30.000s] a crashed\n  stderr ───\n    boom\n\n\
                       [yulius] 3 more failing tests' output not kept\n";
        assert_eq!(
            failed_test_names(retried),
            vec!["a retried".to_owned(), "a crashed".to_owned()]
        );
        assert!(failed_test_names("").is_empty());
    }

    /// SPEC-618 FOC-001: each failing test's block, its header to the next test's header; a
    /// test kept twice has both blocks, and the lines before the first header belong to none.
    #[test]
    fn each_failing_test_has_its_block() {
        let kept = scan(&[NEXTEST]).unwrap_or_default();
        let blocks = failed_test_blocks(&kept);
        let names: Vec<&str> = blocks.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(names, ["nt tests::bad2", "nt tests::bad"]);
        assert!(blocks[0].1.contains("second failure"), "{blocks:?}");
        assert!(!blocks[0].1.contains("the message"), "{blocks:?}");
        assert!(blocks[1].1.contains("the message"), "{blocks:?}");
        assert!(blocks[1].1.contains("some stderr line"), "{blocks:?}");
        assert!(!blocks[1].1.contains("second failure"), "{blocks:?}");
        let retried = "noise before\n\nFAIL [ 0.5s] (1/2) a flaky\n  stderr ───\n    first\n\n\
                       FAIL [ 0.1s] (2/2) a other\n  stderr ───\n    middle\n\n\
                       TRY 2 FAIL [ 0.5s] (1/2) a flaky\n  stderr ───\n    again\n";
        let blocks = failed_test_blocks(retried);
        assert_eq!(blocks.len(), 2, "{blocks:?}");
        assert_eq!(blocks[0].0, "a flaky");
        assert!(blocks[0].1.contains("first") && blocks[0].1.contains("again"));
        assert!(!blocks[0].1.contains("middle") && !blocks[0].1.contains("noise"));
        assert_eq!(
            blocks[1],
            (
                "a other".to_owned(),
                "FAIL [ 0.1s] (2/2) a other\n  stderr ───\n    middle\n\n".to_owned()
            )
        );
        let libtest = "---- tests::bad stdout ----\nthread panicked at tests/x.rs:1:1\n\n\
                       failures:\n    tests::bad\n";
        let blocks = failed_test_blocks(libtest);
        assert_eq!(blocks.len(), 1);
        assert!(blocks[0].1.contains("tests/x.rs"), "{blocks:?}");
        assert!(failed_test_blocks("").is_empty());
    }

    #[test]
    fn libtest_failures_are_kept() {
        let libtest = "running 2 tests\ntest ok ... ok\ntest bad ... FAILED\n\nfailures:\n\n\
                       ---- bad stdout ----\n\nthread 'bad' panicked at src/lib.rs:1:1:\nthe message\n\n\
                       failures:\n    bad\n\ntest result: FAILED. 1 passed; 1 failed\n";
        let kept = scan(&[libtest]).unwrap_or_default();
        assert!(kept.starts_with("---- bad stdout ----\n"), "{kept}");
        assert!(kept.contains("the message"), "{kept}");
        assert!(!kept.contains("test result"), "{kept}");
    }

    #[test]
    fn libtest_passing_output_shown_with_show_output_is_not_kept() {
        let libtest = "running 2 tests\ntest ok ... ok\ntest bad ... FAILED\n\nsuccesses:\n\n\
                       ---- ok stdout ----\npassing noise\n\nsuccesses:\n    ok\n\nfailures:\n\n\
                       ---- bad stdout ----\nthe message\n\nfailures:\n    bad\n\n\
                       test result: FAILED. 1 passed; 1 failed\n";
        let kept = scan(&[libtest]).unwrap_or_default();
        assert!(kept.contains("the message"), "{kept}");
        assert!(!kept.contains("passing noise"), "{kept}");
        assert!(!kept.contains("---- ok stdout"), "{kept}");
    }

    #[test]
    fn a_status_like_line_in_a_failing_tests_output_does_not_end_it() {
        // nextest right-aligns its statuses to 12 columns and indents a test's output by 4.
        let nested = "        FAIL [   0.5s] (1/1) a outer\n  stdout ───\n    \
                      PASS [   0.1s] (1/1) inner test printed by the outer one\n    the message\n";
        let kept = scan(&[nested]).unwrap_or_default();
        assert!(kept.contains("the message"), "{kept}");
        let aligned = "   LEAK-FAIL [   0.5s] (1/1) a leaky\n  stderr ───\n    leaked\n\
                       \x20 TRY 2 FAIL [   0.5s] (1/1) a retried\n  stderr ───\n    again\n";
        let kept = scan(&[aligned]).unwrap_or_default();
        assert!(kept.contains("leaked") && kept.contains("again"), "{kept}");
        assert!(kept.contains("TRY 2 FAIL ["), "{kept}");
    }

    #[test]
    fn a_huge_block_keeps_its_end_and_a_job_keeps_a_bounded_amount() {
        let mut huge = String::from("        FAIL [   0.5s] (1/9) a huge\n  stdout ───\n");
        for i in 0..2_000 {
            huge.push_str(&format!("    noise line {i}\n"));
        }
        huge.push_str("    the panic message\n");
        let kept = scan(&[&huge]).unwrap_or_default();
        assert!(kept.contains("the panic message"), "the end is kept");
        assert!(
            kept.contains("bytes of this test's output are not kept"),
            "{kept}"
        );
        assert!(!kept.contains("noise line 0\n"));
        assert!(kept.len() < MAX_TEST_BYTES + 512, "{}", kept.len());

        let many: String = (0..20).map(|_| huge.as_str()).collect();
        let kept = scan(&[&many]).unwrap_or_default();
        assert!(kept.len() <= MAX_JOB_BYTES + 128, "{}", kept.len());
        assert!(
            kept.ends_with("[yulius] 17 more failing tests' output not kept\n"),
            "{kept}"
        );
    }

    #[test]
    fn an_overlong_line_is_cut() {
        let line = format!(
            "        FAIL [   0.5s] (1/1) a b\n  stdout ───\n    {}\n",
            "x".repeat(10_000)
        );
        let kept = scan(&[&line]).unwrap_or_default();
        assert!(kept.len() < MAX_LINE_BYTES + 200, "{}", kept.len());
        assert!(kept.contains('…'));
    }
}
