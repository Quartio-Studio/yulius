//! Failing tests' output kept from a CI job's whole output (SPEC-145, ADR-0119).
//!
//! A job's stored output is the end of what it printed, and a test runner prints a failing
//! test's output when the test fails, long before the job ends: in a large suite that block is
//! gone from the end. [`FailureScanner`] reads everything the job prints, line by line, and keeps
//! the failing tests' blocks, bounded: nextest's (a failure status line such as
//! `FAIL [   0.5s] (290/1670) crate name` and the indented `stdout ───` / `stderr ───` block after
//! it) and libtest's (`---- name stdout ----` until `failures:` or `test result:`).

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
    kept: String,
    omitted: usize,
}

impl FailureScanner {
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
        if nextest_status(line).is_some_and(nextest_failed) || libtest_header(line).is_some() {
            let kind = if libtest_header(line).is_some() {
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
        if self.kept.len() + entry.len() > MAX_JOB_BYTES {
            self.omitted += 1;
        } else {
            self.kept.push_str(&entry);
        }
    }
}

/// A nextest status line's status (`FAIL`, `PASS`, `SIGSEGV`, `TRY 2 FAIL`…): indented capitals
/// before a bracketed duration such as `[   0.547s]` or `[>30.000s]`.
fn nextest_status(line: &str) -> Option<&str> {
    let text = line.trim_start();
    if text.len() == line.len() {
        return None;
    }
    let (status, rest) = text.split_once('[')?;
    let status = status.trim_end();
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

    fn scan(chunks: &[&str]) -> Option<String> {
        let mut scanner = FailureScanner::default();
        for chunk in chunks {
            scanner.push(chunk.as_bytes());
        }
        scanner.finish()
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
