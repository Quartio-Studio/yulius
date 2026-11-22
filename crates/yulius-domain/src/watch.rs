//! Script watches (SPEC-630, ADR-0465): a shell script on a schedule, run by Yulius in its
//! project's sandbox without an agent, whose output becomes a notice keyed `watch:<name>` for its
//! owner. This module decides what needs no I/O: names, schedules and when a watch is due, the
//! bounds of a run and why a run failed, and what a run's end does (WCH-001, WCH-003, WCH-005,
//! WCH-007, WCH-009).

use crate::cron::{MAX_UTC_OFFSET_MINUTES, Schedule};
use crate::time::Timestamp;

/// Longest name (WCH-001).
pub const NAME_MAX: usize = 40;
/// Longest script, in bytes (WCH-001).
pub const SCRIPT_MAX_BYTES: usize = 16 * 1024;
/// Most watches per project (WCH-001).
pub const MAX_PER_PROJECT: usize = 20;
/// Shortest interval, in seconds (WCH-001).
pub const EVERY_MIN_SECONDS: i64 = 60;
/// Longest interval, in seconds: 7 days (WCH-001).
pub const EVERY_MAX_SECONDS: i64 = 7 * 24 * 3600;
/// Shortest run timeout, in seconds (WCH-001).
pub const TIMEOUT_MIN_SECONDS: u32 = 1;
/// Longest run timeout, in seconds: an hour (WCH-001).
pub const TIMEOUT_MAX_SECONDS: u32 = 3600;
/// A run's timeout when none is given: 5 minutes (WCH-001).
pub const TIMEOUT_DEFAULT_SECONDS: u32 = 300;
/// Failures in a row that post the one warning (WCH-007).
pub const FAILURES_BEFORE_WARNING: u32 = 3;
/// The exit code that posts and ends the watch (WCH-005).
pub const DONE_EXIT: i32 = 3;
/// The wrapper's exit code when it could not set the run's limits (WCH-003).
pub const LIMITS_EXIT: i32 = 125;
/// Characters of a run's output that are posted (WCH-003, WCH-006).
pub const POSTED_CHARS: usize = 4000;
/// Characters of a run's error output kept with its state (the tail).
pub const ERROR_TAIL_CHARS: usize = 2000;
/// The key prefix of a watch's notices (SPEC-624's table).
pub const KEY_PREFIX: &str = "watch:";
const SIGXCPU: i32 = 24;
const SIGXFSZ: i32 = 25;
const SIGKILL: i32 = 9;
const SIGSEGV: i32 = 11;
const SIGABRT: i32 = 6;

/// A watch's name (WCH-001): 1 to [`NAME_MAX`] lowercase letters, digits and `-`, starting with a
/// letter or a digit.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct WatchName(String);

impl WatchName {
    /// Parses `text` (trimmed) as a name.
    ///
    /// # Errors
    /// The rule in words.
    pub fn parse(text: &str) -> Result<Self, String> {
        let text = text.trim();
        let ok = !text.is_empty()
            && text.len() <= NAME_MAX
            && text
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
            && !text.starts_with('-');
        if ok {
            Ok(Self(text.to_owned()))
        } else {
            Err(format!(
                "a watch's name is 1 to {NAME_MAX} lowercase letters, digits and dashes, starting \
                 with a letter or a digit, like disk-report"
            ))
        }
    }

    /// The name.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The key of its reports: `watch:<name>` (WCH-006).
    #[must_use]
    pub fn report_key(&self) -> String {
        format!("{KEY_PREFIX}{}", self.0)
    }

    /// The key of its failure warning: `watch:<name>:failing` (WCH-007).
    #[must_use]
    pub fn warning_key(&self) -> String {
        format!("{KEY_PREFIX}{}:failing", self.0)
    }
}

impl std::fmt::Display for WatchName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Checks a script (WCH-001).
///
/// # Errors
/// An empty (blank) or too long script, or one holding a NUL byte (it could not be an argument).
pub fn check_script(script: &str) -> Result<(), String> {
    if script.trim().is_empty() {
        return Err("a watch needs a script".into());
    }
    if script.len() > SCRIPT_MAX_BYTES {
        return Err(format!(
            "a watch's script is at most {} KiB",
            SCRIPT_MAX_BYTES / 1024
        ));
    }
    if script.contains('\0') {
        return Err("a watch's script cannot hold a NUL byte".into());
    }
    Ok(())
}

/// Checks a run timeout in seconds (WCH-001).
///
/// # Errors
/// Outside [`TIMEOUT_MIN_SECONDS`]..=[`TIMEOUT_MAX_SECONDS`].
pub fn check_timeout(seconds: u32) -> Result<(), String> {
    if (TIMEOUT_MIN_SECONDS..=TIMEOUT_MAX_SECONDS).contains(&seconds) {
        Ok(())
    } else {
        Err(format!(
            "a watch's timeout is {TIMEOUT_MIN_SECONDS} s to {} minutes",
            TIMEOUT_MAX_SECONDS / 60
        ))
    }
}

/// When a watch runs (WCH-001, WCH-009).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WatchSchedule {
    /// Every this many seconds after its last run started.
    Every(i64),
    /// At each slot of a cron expression, in a fixed UTC offset (minutes).
    Cron(Schedule, i32),
}

impl WatchSchedule {
    /// An interval given as `30m`, `2h`, `1d` or a number of seconds with `s` (`90s`).
    ///
    /// # Errors
    /// A malformed interval, or one outside 1 minute to 7 days.
    pub fn every(text: &str) -> Result<Self, String> {
        let text = text.trim();
        let rule = "an interval is a number and s, m, h or d, like 15m, from 1 minute to 7 days";
        let Some(unit) = text.chars().last() else {
            return Err(rule.into());
        };
        let size = match unit {
            's' => 1,
            'm' => 60,
            'h' => 3600,
            'd' => 86_400,
            _ => return Err(rule.into()),
        };
        let digits = &text[..text.len() - 1];
        if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
            return Err(rule.into());
        }
        let seconds = digits
            .parse::<i64>()
            .ok()
            .and_then(|n| n.checked_mul(size))
            .ok_or_else(|| rule.to_owned())?;
        Self::every_seconds(seconds).map_err(|_| rule.to_owned())
    }

    /// An interval of `seconds`.
    ///
    /// # Errors
    /// Outside 1 minute to 7 days.
    pub fn every_seconds(seconds: i64) -> Result<Self, String> {
        if (EVERY_MIN_SECONDS..=EVERY_MAX_SECONDS).contains(&seconds) {
            Ok(Self::Every(seconds))
        } else {
            Err("a watch's interval is 1 minute to 7 days".into())
        }
    }

    /// A cron expression in a UTC offset.
    ///
    /// # Errors
    /// A malformed expression or an offset past ±18 h.
    pub fn cron(expression: &str, utc_offset_minutes: i32) -> Result<Self, String> {
        if utc_offset_minutes.abs() > MAX_UTC_OFFSET_MINUTES {
            return Err("the UTC offset is at most 18 hours either way".into());
        }
        let schedule = Schedule::parse(expression).map_err(|e| e.to_string())?;
        Ok(Self::Cron(schedule, utc_offset_minutes))
    }

    /// When the watch is next due, after its last run started at `last` (or, for a watch that
    /// never ran, was created or resumed at `since` with `last` `None`) (WCH-009). A watch that
    /// never ran on an interval is due at `since` itself: it runs at the next tick.
    #[must_use]
    pub fn next_due(&self, last: Option<Timestamp>, since: Timestamp) -> Option<Timestamp> {
        match (self, last) {
            (Self::Every(_), None) => Some(since),
            (Self::Every(seconds), Some(last)) => Some(last.plus_millis(seconds * 1000)),
            (Self::Cron(schedule, offset), last) => {
                schedule.next_after(last.unwrap_or(since).max(since), *offset)
            }
        }
    }

    /// `15m`-style text of an interval; the expression of a cron schedule.
    #[must_use]
    pub fn describe(&self) -> String {
        match self {
            Self::Every(seconds) => short(*seconds),
            Self::Cron(schedule, _) => schedule.as_str().to_owned(),
        }
    }
}

/// `seconds` in the largest unit that divides it: `1d`, `2h`, `15m`, `90s`.
#[must_use]
pub fn short(seconds: i64) -> String {
    for (unit, size) in [("d", 86_400), ("h", 3600), ("m", 60)] {
        if seconds >= size && seconds % size == 0 {
            return format!("{}{unit}", seconds / size);
        }
    }
    format!("{seconds}s")
}

/// The bounds of one run (WCH-003), set by the wrapper as soft and hard limits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RunBounds {
    /// CPU time, seconds (`ulimit -t`).
    pub cpu_seconds: u32,
    /// Data segment, KiB (`ulimit -d`).
    pub memory_kib: u64,
    /// Largest output file, KiB (`ulimit -f`, in bash's 1024-byte blocks).
    pub output_kib: u64,
}

impl Default for RunBounds {
    fn default() -> Self {
        Self {
            cpu_seconds: 120,
            memory_kib: 1024 * 1024,
            output_kib: 1024,
        }
    }
}

/// How a run's process ended, as the runner saw it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunEnd {
    /// It exited with a code.
    Exited(i32),
    /// A signal ended it.
    Signaled(i32),
    /// It ran past its timeout and was killed.
    TimedOut,
    /// An output file reached its bound (a write past it was refused or killed the writer).
    OutputLimit,
    /// It could not be started, or its end was lost (a restart): the reason.
    Failed(String),
}

impl RunEnd {
    /// The exit code, when it exited.
    #[must_use]
    pub fn code(&self) -> Option<i32> {
        match self {
            Self::Exited(code) => Some(*code),
            _ => None,
        }
    }

    /// Why a run that ended this way failed, naming the bound it went past (WCH-003, AC-6);
    /// `None` for exit 0 and 3. A shell reports a child killed by signal `n` as exit `128 + n`.
    #[must_use]
    pub fn failure(&self, bounds: RunBounds, timeout_seconds: u32) -> Option<String> {
        let signal = match self {
            Self::Exited(0 | DONE_EXIT) => return None,
            Self::Exited(LIMITS_EXIT) => {
                return Some(format!(
                    "exit {LIMITS_EXIT}: the run's limits could not be set (or the script exited \
                     {LIMITS_EXIT})"
                ));
            }
            Self::Exited(code) if (129..=159).contains(code) => code - 128,
            Self::Exited(code) => return Some(format!("exit {code}")),
            Self::Signaled(signal) => *signal,
            Self::TimedOut => {
                return Some(format!(
                    "stopped: it ran past its timeout of {}",
                    short(i64::from(timeout_seconds))
                ));
            }
            Self::OutputLimit => {
                return Some(format!(
                    "stopped: its output went past {} KiB",
                    bounds.output_kib
                ));
            }
            Self::Failed(reason) => return Some(reason.clone()),
        };
        Some(match signal {
            SIGXCPU => format!("stopped: it used its CPU time of {} s", bounds.cpu_seconds),
            SIGXFSZ => format!("stopped: its output went past {} KiB", bounds.output_kib),
            SIGKILL => format!(
                "killed (signal 9): its CPU time of {} s or its memory of {} MiB ran out, or it \
                 was killed",
                bounds.cpu_seconds,
                bounds.memory_kib / 1024
            ),
            SIGSEGV | SIGABRT => format!(
                "ended by signal {signal}: it may have run out of its memory of {} MiB",
                bounds.memory_kib / 1024
            ),
            other => format!("ended by signal {other}"),
        })
    }
}

/// What a watch remembers between runs to decide the next one (WCH-005, WCH-007).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct FailureState {
    /// Failures in a row.
    pub failures: u32,
    /// The warning was posted for this streak.
    pub warned: bool,
}

/// What a run's end does (WCH-005, WCH-007).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outcome {
    /// Post the output (trimmed, cut to [`POSTED_CHARS`]) under the report key.
    pub post: Option<String>,
    /// Post the failure warning, with this text.
    pub warn: Option<String>,
    /// Remove the failure warning: the streak ended.
    pub clear_warning: bool,
    /// End the watch, with this reason.
    pub end: Option<String>,
    /// The failure state after the run.
    pub state: FailureState,
    /// Why the run failed, if it did.
    pub failure: Option<String>,
}

/// Decides what the end of a run of `name` does (WCH-005, WCH-007): `output` is what it printed on
/// stdout, `errors` the tail of its stderr.
#[must_use]
pub fn outcome(
    name: &WatchName,
    end: &RunEnd,
    (output, errors): (&str, &str),
    before: FailureState,
    (bounds, timeout_seconds): (RunBounds, u32),
) -> Outcome {
    if let Some(failure) = end.failure(bounds, timeout_seconds) {
        let failures = before.failures.saturating_add(1);
        let warn = (failures >= FAILURES_BEFORE_WARNING && !before.warned).then(|| {
            let errors = errors.trim();
            let tail = if errors.is_empty() {
                "(no error output)"
            } else {
                errors
            };
            format!(
                "Watch {name} is failing: {failures} runs in a row. The last one: {failure}.\n\n{}",
                cut_tail(tail, ERROR_TAIL_CHARS)
            )
        });
        return Outcome {
            post: None,
            clear_warning: false,
            end: None,
            state: FailureState {
                failures,
                warned: before.warned || warn.is_some(),
            },
            warn,
            failure: Some(failure),
        };
    }
    let output = output.trim();
    Outcome {
        post: (!output.is_empty()).then(|| cut(output, POSTED_CHARS)),
        warn: None,
        clear_warning: before.warned,
        end: (end.code() == Some(DONE_EXIT)).then(|| "the script said it is done (exit 3)".into()),
        state: FailureState::default(),
        failure: None,
    }
}

/// The title of a post: `<name>: <first non-empty line>` (WCH-006).
#[must_use]
pub fn post_title(name: &WatchName, output: &str) -> String {
    let first = output
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or_default();
    format!("{name}: {first}")
}

/// `text` cut to `max` characters, with `…` when cut.
#[must_use]
pub fn cut(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_owned();
    }
    let mut out: String = text.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

/// The last `max` characters of `text`, with `…` in front when cut.
#[must_use]
pub fn cut_tail(text: &str, max: usize) -> String {
    let count = text.chars().count();
    if count <= max {
        return text.to_owned();
    }
    let mut out = String::from("…");
    out.extend(text.chars().skip(count - max.saturating_sub(1)));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const MIN: i64 = 60_000;

    fn name() -> WatchName {
        WatchName::parse("disk-report").unwrap_or_else(|e| panic!("{e}"))
    }

    #[test]
    fn names_follow_the_rule() {
        for good in ["a", "disk-report", "479-merge-queue", &"x".repeat(NAME_MAX)] {
            assert!(WatchName::parse(good).is_ok(), "{good}");
        }
        for bad in [
            "",
            "-x",
            "Disk",
            "a b",
            "a_b",
            "é",
            &"x".repeat(NAME_MAX + 1),
        ] {
            assert!(WatchName::parse(bad).is_err(), "{bad}");
        }
        assert_eq!(name().report_key(), "watch:disk-report");
        assert_eq!(name().warning_key(), "watch:disk-report:failing");
    }

    #[test]
    fn scripts_and_timeouts_are_bounded() {
        assert!(check_script("echo hi").is_ok());
        assert!(check_script("  \n").is_err());
        assert!(check_script("a\0b").is_err());
        assert!(check_script(&"x".repeat(SCRIPT_MAX_BYTES)).is_ok());
        assert!(check_script(&"x".repeat(SCRIPT_MAX_BYTES + 1)).is_err());
        assert!(check_timeout(0).is_err());
        assert!(check_timeout(1).is_ok());
        assert!(check_timeout(3600).is_ok());
        assert!(check_timeout(3601).is_err());
    }

    #[test]
    fn intervals_parse_within_bounds() {
        assert_eq!(WatchSchedule::every("15m"), Ok(WatchSchedule::Every(900)));
        assert_eq!(WatchSchedule::every("1d"), Ok(WatchSchedule::Every(86_400)));
        assert_eq!(WatchSchedule::every("60s"), Ok(WatchSchedule::Every(60)));
        assert_eq!(
            WatchSchedule::every("7d"),
            Ok(WatchSchedule::Every(EVERY_MAX_SECONDS))
        );
        for bad in [
            "",
            "m",
            "30s",
            "8d",
            "5w",
            "1.5h",
            "-5m",
            "２h",
            "99999999999999999999d",
        ] {
            assert!(WatchSchedule::every(bad).is_err(), "{bad}");
        }
        assert_eq!(WatchSchedule::Every(900).describe(), "15m");
        assert_eq!(short(90), "90s");
        assert!(WatchSchedule::cron("0 9 * * *", 0).is_ok());
        assert!(WatchSchedule::cron("0 9 * *", 0).is_err());
        assert!(WatchSchedule::cron("0 9 * * *", 19 * 60).is_err());
    }

    /// AC-1: an interval watch runs at once, then `every` after each start.
    #[test]
    fn an_interval_is_due_after_each_start() {
        let every = WatchSchedule::Every(300);
        let since = Timestamp(1_000_000);
        assert_eq!(every.next_due(None, since), Some(since));
        let started = Timestamp(2_000_000);
        assert_eq!(
            every.next_due(Some(started), since),
            Some(started.plus_millis(5 * MIN))
        );
    }

    /// AC-1: a cron watch is due at its next slot after its last start or its creation.
    #[test]
    fn a_cron_watch_is_due_at_its_next_slot() {
        let cron = WatchSchedule::cron("0 9 * * *", 0).unwrap_or_else(|e| panic!("{e}"));
        // 2026-11-22T08:00:00Z and 09:00:00Z.
        let eight = Timestamp(1_795_334_400_000);
        let nine = eight.plus_millis(60 * MIN);
        assert_eq!(cron.next_due(None, eight), Some(nine));
        assert_eq!(
            cron.next_due(Some(nine), eight),
            Some(nine.plus_millis(24 * 60 * MIN))
        );
        // Resumed after its last run: counted from the resume, never replayed.
        let later = nine.plus_millis(3 * 24 * 60 * MIN);
        assert_eq!(
            cron.next_due(Some(nine), later),
            Some(later.plus_millis(24 * 60 * MIN))
        );
    }

    fn end(end: RunEnd, output: &str, before: FailureState) -> Outcome {
        outcome(
            &name(),
            &end,
            (output, "boom"),
            before,
            (RunBounds::default(), 300),
        )
    }

    /// AC-2: exit 0 posts its output, no output posts nothing, exit 3 posts and ends.
    #[test]
    fn exit_codes_post_skip_or_end() {
        let posted = end(
            RunEnd::Exited(0),
            "  up 3%\nmore\n",
            FailureState::default(),
        );
        assert_eq!(posted.post.as_deref(), Some("up 3%\nmore"));
        assert_eq!(posted.end, None);
        assert_eq!(posted.failure, None);
        let quiet = end(RunEnd::Exited(0), " \n", FailureState::default());
        assert_eq!(quiet.post, None);
        assert_eq!(quiet.end, None);
        let done = end(RunEnd::Exited(3), "due today", FailureState::default());
        assert_eq!(done.post.as_deref(), Some("due today"));
        assert!(done.end.is_some());
        let silent_done = end(RunEnd::Exited(3), "", FailureState::default());
        assert_eq!(silent_done.post, None);
        assert!(silent_done.end.is_some());
        for failed in [
            RunEnd::Exited(1),
            RunEnd::Exited(2),
            RunEnd::Signaled(15),
            RunEnd::TimedOut,
        ] {
            let out = end(failed.clone(), "partial", FailureState::default());
            assert_eq!(out.post, None, "{failed:?}");
            assert!(out.failure.is_some(), "{failed:?}");
            assert_eq!(out.state.failures, 1);
        }
        let long = end(
            RunEnd::Exited(0),
            &"x".repeat(POSTED_CHARS + 10),
            FailureState::default(),
        );
        assert_eq!(long.post.map(|p| p.chars().count()), Some(POSTED_CHARS));
    }

    /// AC-3: the third failure in a row warns once; a success clears it.
    #[test]
    fn three_failures_warn_once_and_a_success_clears() {
        let mut state = FailureState::default();
        let mut warnings = 0;
        for _ in 0..5 {
            let out = end(RunEnd::Exited(1), "", state);
            warnings += usize::from(out.warn.is_some());
            if state.failures + 1 == FAILURES_BEFORE_WARNING {
                assert!(
                    out.warn
                        .as_deref()
                        .is_some_and(|w| w.contains("3 runs in a row") && w.contains("boom"))
                );
            }
            state = out.state;
        }
        assert_eq!(warnings, 1);
        assert_eq!(
            state,
            FailureState {
                failures: 5,
                warned: true
            }
        );
        let ok = end(RunEnd::Exited(0), "", state);
        assert!(ok.clear_warning);
        assert_eq!(ok.state, FailureState::default());
        assert!(!end(RunEnd::Exited(0), "", FailureState::default()).clear_warning);
    }

    /// AC-6: each bound gives a reason that names it.
    #[test]
    fn failures_name_their_bound() {
        let bounds = RunBounds {
            cpu_seconds: 2,
            memory_kib: 65_536,
            output_kib: 64,
        };
        let reason = |e: RunEnd| e.failure(bounds, 10).unwrap_or_default();
        assert!(reason(RunEnd::Signaled(24)).contains("CPU time of 2 s"));
        assert!(reason(RunEnd::Exited(128 + 24)).contains("CPU time"));
        assert!(reason(RunEnd::Signaled(25)).contains("output went past 64 KiB"));
        assert!(reason(RunEnd::Exited(128 + 25)).contains("output"));
        assert!(reason(RunEnd::OutputLimit).contains("output went past 64 KiB"));
        assert!(reason(RunEnd::Signaled(9)).contains("memory of 64 MiB"));
        assert!(reason(RunEnd::TimedOut).contains("timeout of 10s"));
        assert!(reason(RunEnd::Exited(125)).contains("limits"));
        assert_eq!(reason(RunEnd::Exited(1)), "exit 1");
        assert_eq!(RunEnd::Exited(0).failure(bounds, 10), None);
        assert_eq!(RunEnd::Exited(3).failure(bounds, 10), None);
    }

    #[test]
    fn titles_take_the_first_line() {
        assert_eq!(
            post_title(&name(), "\n  v2.1 is out \nnotes"),
            "disk-report: v2.1 is out"
        );
        assert_eq!(cut_tail("abcdef", 4), "…def");
        assert_eq!(cut("abcdef", 4), "abc…");
    }
}
