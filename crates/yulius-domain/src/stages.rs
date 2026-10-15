//! Stages (SPEC-137, ADR-0112): the measured parts of a task, a CI run or a CD run.
//!
//! Every stage is a span of one subject in the store's `spans` table. Spans that follow a state
//! (a task's status, a CI job's start and end) are written by the store as the state changes.
//! Stages inside a job's or a deploy's output have no state of their own: a job names them with
//! lines Yulius reads ([`OutputScanner`]), and Yulius times them by its own clock as it reads
//! each line. A job can only divide its own time, which Yulius measures.
//!
//! This module also holds the pure figures computed over spans: [`percentile`] and the delivery
//! figures of an environment ([`delivery_figures`]).

use std::fmt;

/// Stages a subject's output may add; later stage lines are ignored.
pub const MAX_OUTPUT_STAGES: usize = 400;
/// Figures one `::yulius-measure` line may set.
pub const MAX_MEASURES: usize = 16;
/// Longest stage name, in characters.
pub const MAX_STAGE_NAME: usize = 48;
/// Longest output line looked at, in bytes; a longer line is skipped whole.
const MAX_LINE: usize = 4096;
/// Spans older than this are pruned.
pub const RETAIN_MS: i64 = 180 * 24 * 60 * 60 * 1000;

/// What a span belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SubjectKind {
    /// A task (its states, steps, runs, pauses, delivery).
    Task,
    /// A CI run (its queue, jobs and their stages).
    CiRun,
    /// A CD run (its promotion, deploy, verify and their stages).
    CdRun,
}

impl SubjectKind {
    /// All, in the order pages list them.
    pub const ALL: [Self; 3] = [Self::Task, Self::CiRun, Self::CdRun];

    /// Wire and column name.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Task => "task",
            Self::CiRun => "ci_run",
            Self::CdRun => "cd_run",
        }
    }

    /// Parses a wire name.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        match text.trim() {
            "task" => Some(Self::Task),
            "ci_run" | "ci" => Some(Self::CiRun),
            "cd_run" | "cd" => Some(Self::CdRun),
            _ => None,
        }
    }
}

impl fmt::Display for SubjectKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A stage name a job gave: trimmed, lower case, 1 to [`MAX_STAGE_NAME`] characters of letters,
/// digits, spaces and `_ . : -`. `None` for anything else.
#[must_use]
pub fn stage_name(text: &str) -> Option<String> {
    let name = text.trim().to_lowercase();
    let count = name.chars().count();
    let allowed = |c: char| c.is_alphanumeric() || matches!(c, ' ' | '_' | '.' | ':' | '-');
    ((1..=MAX_STAGE_NAME).contains(&count) && name.chars().all(allowed)).then_some(name)
}

/// Which part of a CD run its fixed wrapper says is running.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    /// The environment's deploy command.
    Deploy,
    /// The environment's verify command.
    Verify,
}

impl Phase {
    /// Span kind and name.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Deploy => "deploy",
            Self::Verify => "verify",
        }
    }
}

/// Test counts a runner's summary reported (added up over the summaries of one stage).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TestCounts {
    /// Passed.
    pub passed: u64,
    /// Failed (errors included).
    pub failed: u64,
    /// Skipped or ignored.
    pub skipped: u64,
    /// Passed only on a retry.
    pub flaky: u64,
}

impl TestCounts {
    /// The figures by their stored names.
    #[must_use]
    pub fn figures(self) -> [(&'static str, u64); 4] {
        [
            ("tests_passed", self.passed),
            ("tests_failed", self.failed),
            ("tests_skipped", self.skipped),
            ("tests_flaky", self.flaky),
        ]
    }
}

/// Something a line of output says about stages.
#[derive(Debug, Clone, PartialEq)]
pub enum OutputEvent {
    /// `::yulius-stage <name>`: a stage starts (the one before ends).
    Stage(String),
    /// `::yulius-stage-end`: the open stage ends.
    StageEnd,
    /// `::yulius-measure k=v …`: figures of the open stage (or the job).
    Measures(Vec<(String, f64)>),
    /// A test runner's summary: counts to add to the open stage (or the job).
    Tests(TestCounts),
    /// The CD wrapper starts a phase (`[yulius] deploy: …`, `[yulius] verify: …`).
    PhaseStart(Phase),
    /// The CD wrapper ends the running phase: `failed` when it says so.
    PhaseEnd {
        /// The phase failed.
        failed: bool,
    },
}

/// Reads a job's or a CD run's output as it arrives, in chunks of any size, and yields the
/// [`OutputEvent`]s of its complete lines. Bounded: a line longer than 4 KiB is skipped.
#[derive(Debug, Default)]
pub struct OutputScanner {
    line: Vec<u8>,
    overflow: bool,
    cd: bool,
}

impl OutputScanner {
    /// A scanner of a CI job's output.
    #[must_use]
    pub fn ci() -> Self {
        Self::default()
    }

    /// A scanner of a CD run's output: the wrapper's phase lines count too.
    #[must_use]
    pub fn cd() -> Self {
        Self {
            cd: true,
            ..Self::default()
        }
    }

    /// The events of the lines `bytes` completes.
    pub fn push(&mut self, bytes: &[u8]) -> Vec<OutputEvent> {
        let mut events = Vec::new();
        for &byte in bytes {
            if byte == b'\n' {
                if !self.overflow
                    && let Some(event) = scan_line(&String::from_utf8_lossy(&self.line), self.cd)
                {
                    events.push(event);
                }
                self.line.clear();
                self.overflow = false;
            } else if self.line.len() < MAX_LINE {
                self.line.push(byte);
            } else {
                self.overflow = true;
            }
        }
        events
    }

    /// The event of a last line without a line break, once the output has ended.
    pub fn finish(&mut self) -> Vec<OutputEvent> {
        if self.line.is_empty() {
            return Vec::new();
        }
        self.push(b"\n")
    }
}

fn scan_line(raw: &str, cd: bool) -> Option<OutputEvent> {
    let line = raw.trim();
    if let Some(rest) = line.strip_prefix("::yulius-") {
        return marker(rest);
    }
    if cd && let Some(rest) = raw.strip_prefix("[yulius] ") {
        return if rest.starts_with("deploy: ") {
            Some(OutputEvent::PhaseStart(Phase::Deploy))
        } else if rest.starts_with("verify: ") {
            Some(OutputEvent::PhaseStart(Phase::Verify))
        } else if rest.starts_with("deploy failed") || rest.starts_with("verify failed") {
            Some(OutputEvent::PhaseEnd { failed: true })
        } else if rest == "done" {
            Some(OutputEvent::PhaseEnd { failed: false })
        } else {
            None
        };
    }
    test_summary(line).map(OutputEvent::Tests)
}

fn marker(rest: &str) -> Option<OutputEvent> {
    if rest == "stage-end" {
        return Some(OutputEvent::StageEnd);
    }
    if let Some(name) = rest.strip_prefix("stage ") {
        return stage_name(name).map(OutputEvent::Stage);
    }
    let pairs = rest.strip_prefix("measure ")?;
    let figures: Vec<(String, f64)> = pairs
        .split_whitespace()
        .filter_map(|pair| {
            let (key, value) = pair.split_once('=')?;
            let value: f64 = value.parse().ok()?;
            (figure_name(key) && value.is_finite()).then(|| (key.to_owned(), value))
        })
        .take(MAX_MEASURES)
        .collect();
    (!figures.is_empty()).then_some(OutputEvent::Measures(figures))
}

/// A figure's name: a lower-case letter, then up to 31 lower-case letters, digits or `_`.
#[must_use]
pub fn figure_name(key: &str) -> bool {
    let mut chars = key.chars();
    matches!(chars.next(), Some(c) if c.is_ascii_lowercase())
        && key.len() <= 32
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

/// Counts of the common runners' summaries: cargo nextest (`Summary [ 1.2s] 10 tests run: 9
/// passed (1 flaky), 1 failed, 2 skipped`), cargo test (`test result: ok. 9 passed; 1 failed;
/// 2 ignored; …`), pytest (`=== 9 passed, 1 failed, 2 skipped in 1.2s ===`) and Playwright
/// (`9 passed (1.2s)`, `1 failed`, `1 flaky`, `2 skipped`, a line each).
#[must_use]
pub fn test_summary(line: &str) -> Option<TestCounts> {
    if line.starts_with("Summary [")
        && let Some((_, counts)) = line.split_once(" run: ")
    {
        return counted(counts.split(','));
    }
    if let Some(rest) = line.strip_prefix("test result: ") {
        let (_, counts) = rest.split_once(". ")?;
        return counted(counts.split(';'));
    }
    if line.starts_with('=') && line.ends_with('=') {
        let inner = line.trim_matches('=').trim();
        let (counts, _) = inner.rsplit_once(" in ")?;
        return counted(counts.split(','));
    }
    // Playwright: exactly "<n> <word>" with an optional "(<time>)".
    let words: Vec<&str> = line.split_whitespace().collect();
    match words.as_slice() {
        [n, word] | [n, word, _] if n.chars().all(|c| c.is_ascii_digit()) => {
            if words.len() == 3 && !(words[2].starts_with('(') && words[2].ends_with(')')) {
                return None;
            }
            let n: u64 = n.parse().ok()?;
            let mut counts = TestCounts::default();
            match *word {
                "passed" => counts.passed = n,
                "failed" => counts.failed = n,
                "flaky" => counts.flaky = n,
                "skipped" => counts.skipped = n,
                _ => return None,
            }
            Some(counts)
        }
        _ => None,
    }
}

/// Adds up "<n> <word>" parts; `None` when no part is a count.
fn counted<'a>(parts: impl Iterator<Item = &'a str>) -> Option<TestCounts> {
    let mut counts = TestCounts::default();
    let mut any = false;
    for part in parts {
        // "9 passed (1 flaky)": the bracket is a count of its own.
        for piece in part.split('(') {
            let piece = piece.trim().trim_end_matches(')');
            let mut words = piece.split_whitespace();
            let (Some(n), Some(word)) = (words.next(), words.next()) else {
                continue;
            };
            let Ok(n) = n.parse::<u64>() else { continue };
            let slot = match word {
                "passed" => &mut counts.passed,
                "failed" | "error" | "errors" | "timed" => &mut counts.failed,
                "skipped" | "ignored" => &mut counts.skipped,
                "flaky" => &mut counts.flaky,
                _ => continue,
            };
            *slot += n;
            any = true;
        }
    }
    any.then_some(counts)
}

/// The nearest-rank `p` percentile (0 < p ≤ 100) of `sorted` (ascending); `None` when empty.
#[must_use]
pub fn percentile(sorted: &[i64], p: u32) -> Option<i64> {
    if sorted.is_empty() || p == 0 {
        return None;
    }
    let n = sorted.len();
    let rank = (u64::from(p.min(100)) * n as u64).div_ceil(100);
    let index = usize::try_from(rank).unwrap_or(n).clamp(1, n) - 1;
    sorted.get(index).copied()
}

/// One finished deployment of an environment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Deployment {
    /// When it started (ms).
    pub started_at: i64,
    /// When it ended (ms).
    pub ended_at: i64,
    /// It passed (deployed and verified).
    pub passed: bool,
}

/// An environment's delivery figures over a range.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DeliveryFigures {
    /// Finished deployments.
    pub deployments: u64,
    /// Passed deployments.
    pub passed: u64,
    /// Passed deployments per week of the range.
    pub per_week: Option<f64>,
    /// Lead time from a merge to the end of the first passed deployment that started after it.
    pub lead_time_p50: Option<i64>,
    /// Its 90th percentile.
    pub lead_time_p90: Option<i64>,
    /// Merges that lead time was measured on.
    pub lead_time_count: u64,
    /// Failed deployments over finished ones.
    pub change_failure_rate: Option<f64>,
    /// From a failed deployment to the end of the next passed one (median).
    pub time_to_restore_p50: Option<i64>,
    /// Restores measured.
    pub restores: u64,
}

const WEEK_MS: i64 = 7 * 24 * 60 * 60 * 1000;

/// The delivery figures of one environment: `deployments` and `merges` (times a change was merged
/// into the branch the environment deploys from) in any order; `range_ms` the range's length.
///
/// An environment deploys its branch's head, so a merge is deployed by the first passed
/// deployment that started after it. A failure is restored by the next passed deployment.
#[must_use]
pub fn delivery_figures(
    deployments: &[Deployment],
    merges: &[i64],
    range_ms: i64,
) -> DeliveryFigures {
    let mut runs = deployments.to_vec();
    runs.sort_by_key(|d| (d.started_at, d.ended_at));
    let passed: Vec<&Deployment> = runs.iter().filter(|d| d.passed).collect();
    let mut leads: Vec<i64> = merges
        .iter()
        .filter_map(|&merged| {
            passed
                .iter()
                .find(|d| d.started_at >= merged)
                .map(|d| d.ended_at.saturating_sub(merged).max(0))
        })
        .collect();
    leads.sort_unstable();
    let mut restores: Vec<i64> = Vec::new();
    let mut failing_since: Option<i64> = None;
    for run in &runs {
        match (run.passed, failing_since) {
            (false, None) => failing_since = Some(run.ended_at),
            (true, Some(since)) => {
                restores.push(run.ended_at.saturating_sub(since).max(0));
                failing_since = None;
            }
            _ => {}
        }
    }
    restores.sort_unstable();
    let total = runs.len() as u64;
    let ok = passed.len() as u64;
    #[allow(clippy::cast_precision_loss)] // Counts and lengths far below 2^52.
    let per_week = (range_ms > 0).then(|| ok as f64 * WEEK_MS as f64 / range_ms as f64);
    #[allow(clippy::cast_precision_loss)]
    let change_failure_rate = (total > 0).then(|| (total - ok) as f64 / total as f64);
    DeliveryFigures {
        deployments: total,
        passed: ok,
        per_week,
        lead_time_p50: percentile(&leads, 50),
        lead_time_p90: percentile(&leads, 90),
        lead_time_count: leads.len() as u64,
        change_failure_rate,
        time_to_restore_p50: percentile(&restores, 50),
        restores: restores.len() as u64,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn events(chunks: &[&str], cd: bool) -> Vec<OutputEvent> {
        let mut scanner = if cd {
            OutputScanner::cd()
        } else {
            OutputScanner::ci()
        };
        let mut out = Vec::new();
        for chunk in chunks {
            out.extend(scanner.push(chunk.as_bytes()));
        }
        out.extend(scanner.finish());
        out
    }

    #[test]
    fn stage_lines_split_across_chunks_are_read_once_complete() {
        let got = events(
            &[
                "+ cargo\n::yulius-st",
                "age Lint\nwarning\n::yulius-stage-end\n::yulius-stage unit",
            ],
            false,
        );
        assert_eq!(
            got,
            vec![
                OutputEvent::Stage("lint".into()),
                OutputEvent::StageEnd,
                OutputEvent::Stage("unit".into()),
            ]
        );
    }

    #[test]
    fn stage_names_are_checked() {
        assert_eq!(stage_name("  Build docs "), Some("build docs".into()));
        assert_eq!(stage_name("e2e:chromium"), Some("e2e:chromium".into()));
        assert_eq!(stage_name(""), None);
        assert_eq!(stage_name("a/b"), None);
        assert_eq!(stage_name(&"x".repeat(49)), None);
        assert_eq!(events(&["::yulius-stage <script>\n"], false), vec![]);
    }

    #[test]
    fn measures_keep_valid_figures_only_and_are_bounded() {
        let got = events(
            &["::yulius-measure cache_hits=3 Bad=1 x=nan y=2.5 z=\n"],
            false,
        );
        assert_eq!(
            got,
            vec![OutputEvent::Measures(vec![
                ("cache_hits".into(), 3.0),
                ("y".into(), 2.5)
            ])]
        );
        let many: String = (0..40).map(|i| format!(" k{i}=1")).collect();
        let got = events(&[&format!("::yulius-measure{many}\n")], false);
        assert!(matches!(&got[..], [OutputEvent::Measures(v)] if v.len() == MAX_MEASURES));
        assert_eq!(events(&["::yulius-measure nothing\n"], false), vec![]);
    }

    #[test]
    fn a_too_long_line_is_skipped_whole() {
        let long = format!(
            "::yulius-stage a{}\n::yulius-stage b\n",
            "a".repeat(MAX_LINE)
        );
        assert_eq!(
            events(&[&long], false),
            vec![OutputEvent::Stage("b".into())]
        );
    }

    #[test]
    fn test_summaries_of_common_runners_are_counted() {
        let nextest =
            "     Summary [  12.345s] 1234 tests run: 1230 passed (2 flaky), 4 failed, 3 skipped";
        assert_eq!(
            test_summary(nextest.trim()),
            Some(TestCounts {
                passed: 1230,
                failed: 4,
                skipped: 3,
                flaky: 2
            })
        );
        assert_eq!(
            test_summary(
                "test result: ok. 12 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.01s"
            ),
            Some(TestCounts {
                passed: 12,
                failed: 0,
                skipped: 1,
                flaky: 0
            })
        );
        assert_eq!(
            test_summary("==== 9 passed, 1 failed, 2 skipped, 1 error in 3.21s ===="),
            Some(TestCounts {
                passed: 9,
                failed: 2,
                skipped: 2,
                flaky: 0
            })
        );
        assert_eq!(
            test_summary("12 passed (34.5s)"),
            Some(TestCounts {
                passed: 12,
                ..TestCounts::default()
            })
        );
        assert_eq!(
            test_summary("1 flaky"),
            Some(TestCounts {
                flaky: 1,
                ..TestCounts::default()
            })
        );
        for other in [
            "3 files changed",
            "copied 3 passed files",
            "==== no tests ran in 0.1s ====",
            "12 passed later on",
        ] {
            assert_eq!(test_summary(other), None, "{other}");
        }
    }

    #[test]
    fn cd_wrapper_lines_are_phases_only_in_cd_output() {
        let out = "[yulius] deploy: scripts/cd/deploy.sh dev\n::yulius-stage build\n[yulius] verify: scripts/cd/verify.sh\n[yulius] verify failed with status 3\n";
        assert_eq!(
            events(&[out], true),
            vec![
                OutputEvent::PhaseStart(Phase::Deploy),
                OutputEvent::Stage("build".into()),
                OutputEvent::PhaseStart(Phase::Verify),
                OutputEvent::PhaseEnd { failed: true },
            ]
        );
        assert_eq!(
            events(&[out], false),
            vec![OutputEvent::Stage("build".into())]
        );
        assert_eq!(
            events(&["  [yulius] done\n", "[yulius] done"], true),
            vec![OutputEvent::PhaseEnd { failed: false }]
        );
    }

    #[test]
    fn percentiles_are_nearest_rank() {
        let values: Vec<i64> = (1..=10).collect();
        assert_eq!(percentile(&values, 50), Some(5));
        assert_eq!(percentile(&values, 90), Some(9));
        assert_eq!(percentile(&values, 100), Some(10));
        assert_eq!(percentile(&[7], 90), Some(7));
        assert_eq!(percentile(&[], 50), None);
    }

    #[test]
    fn delivery_figures_follow_merges_to_the_first_passed_deploy_after_them() {
        let h = 3_600_000;
        let deployments = [
            Deployment {
                started_at: 10 * h,
                ended_at: 10 * h + 600_000,
                passed: true,
            },
            Deployment {
                started_at: 20 * h,
                ended_at: 20 * h + 600_000,
                passed: false,
            },
            Deployment {
                started_at: 22 * h,
                ended_at: 22 * h + 600_000,
                passed: true,
            },
        ];
        // Merged at 1 h and 9 h: deployed by the first; at 15 h: the failure skipped, the third.
        // A merge after every deploy is not deployed yet.
        let merges = [h, 9 * h, 15 * h, 30 * h];
        let f = delivery_figures(&deployments, &merges, 7 * 24 * h);
        assert_eq!(f.deployments, 3);
        assert_eq!(f.passed, 2);
        assert_eq!(f.lead_time_count, 3);
        assert_eq!(f.lead_time_p50, Some(7 * h + 600_000));
        assert_eq!(f.lead_time_p90, Some(9 * h + 600_000));
        assert_eq!(f.change_failure_rate, Some(1.0 / 3.0));
        assert_eq!(f.time_to_restore_p50, Some(2 * h));
        assert_eq!(f.restores, 1);
        assert_eq!(f.per_week, Some(2.0));
    }

    #[test]
    fn delivery_figures_without_deployments_are_unknown_not_zero() {
        let f = delivery_figures(&[], &[1, 2], 0);
        assert_eq!(f.deployments, 0);
        assert_eq!(f.lead_time_p50, None);
        assert_eq!(f.change_failure_rate, None);
        assert_eq!(f.per_week, None);
    }
}
