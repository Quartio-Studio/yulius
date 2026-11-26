//! The merge queue's numbers (SPEC-493 MQC-005): merge candidates formed, merged and dissolved
//! (by why) per day, and the minutes from arming a merge to the merge, read from the audit
//! entries the queue writes.

use std::collections::BTreeMap;

use crate::scope::{Pattern, pattern_matches};
use crate::time::{Timestamp, format_rfc3339};

/// The audit action of a candidate formed.
pub const FORMED: &str = "delivery.train.formed";
/// The audit action of a candidate whose members all merged.
pub const MERGED: &str = "delivery.train.merged";
/// The audit action of a candidate dissolved.
pub const DISSOLVED: &str = "delivery.train.dissolved";
/// The audit actions of an armed merge, by an agent and by a person.
pub const ARMED: [&str; 2] = ["delivery.agent.merge_armed", "delivery.person.merge_armed"];
/// The audit actions of a pull request's merge.
pub const MERGE: [&str; 3] = [
    "delivery.agent.merge",
    "delivery.person.merge",
    "delivery.merge",
];

/// Why a candidate was dissolved, as its audit entry says it (`(why: <kind>)`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Dissolved {
    /// A member left: it was armed again, is no longer armed or moved on (before SPEC-497, also
    /// an own run that did not pass).
    MemberLeft,
    /// A member's own run failed, its change's verdict (SPEC-497 MQW-004).
    MemberFailed,
    /// Its run failed and it was bisected.
    Bisected,
    /// The integration branch moved in a way it did not predict.
    BaseMoved,
    /// Its run errored or was cancelled, or it could not go on.
    Errored,
    /// It took too long (its run, a member's own run, GitHub).
    TooLong,
    /// The after-merge gate turned red while it held a pull request that is not the repair
    /// (SPEC-676 RGS-006).
    GateRed,
}

impl Dissolved {
    /// Every kind, in the order the numbers list them.
    pub const ALL: [Self; 7] = [
        Self::MemberLeft,
        Self::MemberFailed,
        Self::Bisected,
        Self::BaseMoved,
        Self::Errored,
        Self::TooLong,
        Self::GateRed,
    ];

    /// The kind's name in an audit entry and in the read.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::MemberLeft => "member_left",
            Self::MemberFailed => "member_failed",
            Self::Bisected => "bisected",
            Self::BaseMoved => "base_moved",
            Self::Errored => "errored",
            Self::TooLong => "too_long",
            Self::GateRed => "gate_red",
        }
    }

    /// The kind an entry's detail names (`(why: <kind>)`); `Errored` for an entry written
    /// before the kind was.
    #[must_use]
    pub fn of_detail(detail: &str) -> Self {
        Self::ALL
            .into_iter()
            .find(|kind| detail.contains(&format!("(why: {})", kind.as_str())))
            .unwrap_or(Self::Errored)
    }
}

/// One audit entry of the queue.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueueEvent {
    /// When.
    pub at: Timestamp,
    /// The entry's target: the sandbox (`<slug>`) or a pull request's task (`<slug>#<n>`).
    pub target: String,
    /// The action.
    pub action: String,
    /// The detail.
    pub detail: String,
}

/// One day's counts.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct QueueDay {
    /// The UTC day, `YYYY-MM-DD`.
    pub day: String,
    /// Candidates formed.
    pub formed: u32,
    /// Candidates whose members all merged.
    pub merged: u32,
    /// Candidates dissolved, by why.
    pub dissolved: BTreeMap<&'static str, u32>,
}

/// The numbers over a range.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct QueueNumbers {
    /// Per day, oldest first; days with nothing are left out.
    pub days: Vec<QueueDay>,
    /// Pull requests merged after an armed merge.
    pub merges: u32,
    /// The median minutes from a merge's last arming to the merge.
    pub median_arm_to_merge_minutes: Option<f64>,
    /// The last [`RECENT_DISSOLVED`] candidates dissolved in the range, newest first, each with
    /// its why and its whole reason (SPEC-497 MQW-004).
    pub dissolved_recent: Vec<(Dissolved, QueueEvent)>,
}

/// Most dissolved candidates listed with their reason.
pub const RECENT_DISSOLVED: usize = 10;

/// A pull request held out of the next merge candidate (SPEC-497 MQW-003).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeldApart {
    /// The pull request held out.
    pub pr: u64,
    /// The earlier member that changes files under the same pattern.
    pub with: u64,
    /// That pattern (`[merge_queue] apart`).
    pub pattern: String,
}

/// Which of the armed pull requests `candidates` (in arm order, each with the files it changes)
/// go into one merge candidate when `apart` names derived files (SPEC-497 MQW-003: generated
/// lists, formatted tables): a pull request that changes a file under a pattern an earlier one
/// in the candidate also changes is held out, and waits for the next candidate on top of the
/// first's merge. No other member loses a run to their combination; the held one's own candidate
/// still tests it on top of the first.
///
/// # Examples
/// ```
/// use yulius_domain::merge_queue::held_apart;
/// use yulius_domain::scope::Pattern;
/// let apart = [Pattern::parse("skills/defaults/")?];
/// let changed = [
///     (3, vec!["skills/defaults/a.md".to_owned()]),
///     (5, vec!["src/x.rs".to_owned()]),
///     (7, vec!["skills/defaults/b.md".to_owned()]),
/// ];
/// let (kept, held) = held_apart(&apart, &changed);
/// assert_eq!(kept, [3, 5]);
/// assert_eq!((held[0].pr, held[0].with), (7, 3));
/// # Ok::<(), yulius_domain::scope::ScopeError>(())
/// ```
#[must_use]
pub fn held_apart(
    apart: &[Pattern],
    candidates: &[(u64, Vec<String>)],
) -> (Vec<u64>, Vec<HeldApart>) {
    let mut kept: Vec<u64> = Vec::new();
    let mut taken: Vec<(usize, u64)> = Vec::new();
    let mut held = Vec::new();
    for (pr, paths) in candidates {
        let touched: Vec<usize> = apart
            .iter()
            .enumerate()
            .filter(|(_, pattern)| paths.iter().any(|path| pattern_matches(pattern, path)))
            .map(|(index, _)| index)
            .collect();
        if let Some((index, with)) = taken.iter().find(|(index, _)| touched.contains(index)) {
            held.push(HeldApart {
                pr: *pr,
                with: *with,
                pattern: apart[*index].as_str().to_owned(),
            });
            continue;
        }
        taken.extend(touched.into_iter().map(|index| (index, *pr)));
        kept.push(*pr);
    }
    (kept, held)
}

/// A failure of the integration branch's after-merge gate that no later verdict cleared
/// (SPEC-676 RGS-002), as the gate history names it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenFailure {
    /// The failing test; a job's name (`job <name>` before it is attributed) when its tests are
    /// not known, or the run's state (`failed`, `error`) for a run-level failure.
    pub test: String,
    /// The revision that first showed it.
    pub revision: String,
    /// The number of the task whose merge made that revision, when Yulius knows it.
    pub merge_task: Option<u32>,
}

/// The merge queue stopped by a red after-merge gate (SPEC-676): only a repair merges until the
/// integration branch's gate is green again.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MergeStop {
    /// The open failures that stop it, none of them a quarantined test.
    pub failures: Vec<OpenFailure>,
    /// The run to read and to run again.
    pub run: String,
}

impl MergeStop {
    /// The stop of `failures` (RGS-002, RGS-008): `None` when every open failure is a test in
    /// `quarantined` (flaky, ADR-0356) or there is none.
    ///
    /// # Examples
    /// ```
    /// use yulius_domain::merge_queue::{MergeStop, OpenFailure};
    /// let failed = |test: &str| OpenFailure {
    ///     test: test.into(),
    ///     revision: "a1b2c3d4e5f6a7b8".into(),
    ///     merge_task: Some(12),
    /// };
    /// assert!(MergeStop::of(vec![failed("app tests::flaky")], &["app tests::flaky"], "r").is_none());
    /// let stop = MergeStop::of(vec![failed("app tests::real")], &["app tests::flaky"], "r");
    /// assert!(stop.is_some_and(|stop| stop.lets_through(12, false) && !stop.lets_through(3, false)));
    /// ```
    #[must_use]
    pub fn of(failures: Vec<OpenFailure>, quarantined: &[&str], run: &str) -> Option<Self> {
        let failures: Vec<OpenFailure> = failures
            .into_iter()
            .filter(|failure| !quarantined.contains(&failure.test.as_str()))
            .collect();
        (!failures.is_empty()).then(|| Self {
            failures,
            run: run.to_owned(),
        })
    }

    /// The tasks whose merges brought the open failures in, by number, each once.
    #[must_use]
    pub fn breakers(&self) -> Vec<u32> {
        let mut numbers: Vec<u32> = self.failures.iter().filter_map(|f| f.merge_task).collect();
        numbers.sort_unstable();
        numbers.dedup();
        numbers
    }

    /// Whether task `number`'s merge goes through (RGS-005): its agent marked it the repair
    /// (`marked`), or its merge brought an open failure in.
    #[must_use]
    pub fn lets_through(&self, number: u32, marked: bool) -> bool {
        marked || self.breakers().contains(&number)
    }

    /// The open failures in words: each test, where it first failed and whose merge that was.
    #[must_use]
    pub fn failures_text(&self) -> String {
        self.failures
            .iter()
            .map(|failure| {
                let origin = failure
                    .merge_task
                    .map_or_else(|| "an untracked merge".to_owned(), |n| format!("task #{n}"));
                let revision: String = failure.revision.chars().take(12).collect();
                format!("`{}` (first at {revision}, {origin})", failure.test)
            })
            .collect::<Vec<_>>()
            .join(", ")
    }

    /// What a held merge's task is told (RGS-003): why pull request `pr` waits, which failures,
    /// and what ends the wait.
    #[must_use]
    pub fn waiting_note(&self, pr: u64, integration: &str) -> String {
        format!(
            "Pull request #{pr} waits: the after-merge gate of `{integration}` is red ({}; run \
             {}), and the merge queue merges only its repair until a later revision's gate is \
             green. The merge stays armed and keeps its place: it merges by itself then. If this \
             pull request repairs that failure, call `delivery_merge` again with `repairs_gate: \
             true`.",
            self.failures_text(),
            self.run
        )
    }
}

/// The numbers of `events` (any order) at or after `from` (none: all). Entries before `from`
/// still pair an arming with its merge, so a merge in the range is not paired with an arming an
/// earlier merge used (review round 2 of SPEC-493 fix 4).
#[must_use]
pub fn queue_numbers(events: &[QueueEvent], from: Option<Timestamp>) -> QueueNumbers {
    let mut sorted: Vec<&QueueEvent> = events.iter().collect();
    sorted.sort_by_key(|event| event.at);
    let mut days: BTreeMap<String, QueueDay> = BTreeMap::new();
    let mut armed: BTreeMap<&str, Timestamp> = BTreeMap::new();
    let mut minutes = Vec::new();
    let mut recent: Vec<(Dissolved, QueueEvent)> = Vec::new();
    for event in sorted {
        let action = event.action.as_str();
        let counted = from.is_none_or(|from| event.at >= from);
        if [FORMED, MERGED, DISSOLVED].contains(&action) {
            if !counted {
                continue;
            }
            let day = format_rfc3339(event.at)
                .chars()
                .take(10)
                .collect::<String>();
            let entry = days.entry(day.clone()).or_insert_with(|| QueueDay {
                day,
                ..QueueDay::default()
            });
            match action {
                FORMED => entry.formed += 1,
                MERGED => entry.merged += 1,
                _ => {
                    let why = Dissolved::of_detail(&event.detail);
                    *entry.dissolved.entry(why.as_str()).or_default() += 1;
                    recent.push((why, event.clone()));
                }
            }
        } else if ARMED.contains(&action) {
            armed.insert(event.target.as_str(), event.at);
        } else if MERGE.contains(&action)
            && let Some(at) = armed.remove(event.target.as_str())
            && counted
        {
            #[expect(clippy::cast_precision_loss, reason = "minutes of a merge fit")]
            minutes.push(event.at.millis_since(at) as f64 / 60_000.0);
        }
    }
    minutes.sort_by(f64::total_cmp);
    let median = match minutes.len() {
        0 => None,
        n if n % 2 == 1 => Some(minutes[n / 2]),
        n => Some(f64::midpoint(minutes[n / 2 - 1], minutes[n / 2])),
    };
    recent.reverse();
    recent.truncate(RECENT_DISSOLVED);
    QueueNumbers {
        days: days.into_values().collect(),
        merges: u32::try_from(minutes.len()).unwrap_or(u32::MAX),
        median_arm_to_merge_minutes: median,
        dissolved_recent: recent,
    }
}

/// CI minutes by how the runs ended (SPEC-497 OFC-003).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Minutes {
    /// Of runs that passed.
    pub passed: f64,
    /// Of runs that failed for the change (or for a reason not told apart).
    pub failed: f64,
    /// Of runs that failed only for the infrastructure.
    pub failed_outside: f64,
    /// Of runs that were cancelled.
    pub cancelled: f64,
    /// Of job runs an infrastructure failure threw away, within any run (job minutes).
    pub rerun: f64,
}

impl Minutes {
    /// The share of the runs' minutes that went to runs failed outside the change or cancelled
    /// (the target: at most 15 %); `None` without any minutes.
    #[must_use]
    pub fn outside_share(&self) -> Option<f64> {
        let total = self.passed + self.failed + self.failed_outside + self.cancelled;
        (total > 0.0).then(|| (self.failed_outside + self.cancelled) / total)
    }

    /// Adds `run_ms` of runs that ended `outcome` (`passed`, `failed`, `failed_outside`,
    /// `cancelled`; another counts as failed) and `rerun_ms` of thrown-away job time.
    pub fn add(&mut self, outcome: &str, run_ms: i64, rerun_ms: i64) {
        #[expect(clippy::cast_precision_loss, reason = "minutes of runs fit")]
        let (minutes, rerun) = (
            run_ms.max(0) as f64 / 60_000.0,
            rerun_ms.max(0) as f64 / 60_000.0,
        );
        match outcome {
            "passed" => self.passed += minutes,
            "failed_outside" => self.failed_outside += minutes,
            "cancelled" => self.cancelled += minutes,
            _ => self.failed += minutes,
        }
        self.rerun += rerun;
    }
}

/// The audit action of a sandbox whose CI has no gate because of its configuration (SPEC-504
/// CFG-004); its detail is why.
pub const CONFIG_BLOCKED: &str = "ci.config_blocked";
/// The audit action of that sandbox's CI having a gate again.
pub const CONFIG_UNBLOCKED: &str = "ci.config_unblocked";

/// The minutes within `[from, to]` of the episodes in `events` (time and whether it says
/// blocked, any order) in which CI had no gate because of its configuration (SPEC-504 CFG-004):
/// an episode begins at the first `blocked` after an `unblocked` (or none) and ends at the next
/// `unblocked`, or at `to` while none came. Restarts that say `blocked` again inside an episode
/// change nothing.
#[must_use]
pub fn blocked_minutes(
    events: &[(Timestamp, bool)],
    from: Option<Timestamp>,
    to: Timestamp,
) -> u64 {
    let mut sorted = events.to_vec();
    sorted.sort_by_key(|(at, _)| at.0);
    let mut since: Option<i64> = None;
    let mut total_ms: i64 = 0;
    let close = |start: i64, end: i64, total_ms: &mut i64| {
        let start = from.map_or(start, |from| start.max(from.0));
        let end = end.min(to.0);
        *total_ms += (end - start).max(0);
    };
    for (at, blocked) in sorted {
        match (blocked, since) {
            (true, None) => since = Some(at.0),
            (false, Some(start)) => {
                close(start, at.0, &mut total_ms);
                since = None;
            }
            _ => {}
        }
    }
    if let Some(start) = since {
        close(start, to.0, &mut total_ms);
    }
    u64::try_from(total_ms / 60_000).unwrap_or(0)
}

#[cfg(test)]
mod tests {

    /// SPEC-504 CFG-004: episodes from the first `blocked` to the next `unblocked`, clipped to
    /// the range; an open one runs to its end; repeats inside an episode count once.
    #[test]
    fn blocked_minutes_add_up_episodes_within_the_range() {
        const MIN: i64 = 60_000;
        let at = |minute: i64| Timestamp(minute * MIN);
        let events = [
            (at(10), true),
            (at(15), true),
            (at(40), false),
            (at(100), true),
            (at(130), false),
            (at(200), true),
        ];
        assert_eq!(blocked_minutes(&events, None, at(260)), 30 + 30 + 60);
        assert_eq!(blocked_minutes(&events, Some(at(20)), at(120)), 20 + 20);
        assert_eq!(blocked_minutes(&[(at(5), false)], None, at(10)), 0);
        assert_eq!(blocked_minutes(&[], None, at(10)), 0);
    }
    use super::*;

    /// SPEC-497 OFC-003: minutes by outcome, the rerun time apart, and the share outside the
    /// change (failed only for the infrastructure, or cancelled).
    #[test]
    fn ci_minutes_are_counted_by_how_runs_ended() {
        let mut total = Minutes::default();
        total.add("passed", 10 * MIN, 2 * MIN);
        total.add("failed", 6 * MIN, 0);
        total.add("failed_outside", 3 * MIN, 3 * MIN);
        total.add("cancelled", MIN, 0);
        total.add("unknown", MIN, 0);
        assert_eq!(
            total,
            Minutes {
                passed: 10.0,
                failed: 7.0,
                failed_outside: 3.0,
                cancelled: 1.0,
                rerun: 5.0,
            }
        );
        assert!((total.outside_share().unwrap_or_default() - 4.0 / 21.0).abs() < 1e-9);
        assert_eq!(Minutes::default().outside_share(), None);
    }

    const DAY: i64 = 86_400_000;
    const MIN: i64 = 60_000;

    fn event(at: i64, target: &str, action: &str, detail: &str) -> QueueEvent {
        QueueEvent {
            at: Timestamp(at),
            target: target.to_owned(),
            action: action.to_owned(),
            detail: detail.to_owned(),
        }
    }

    /// SPEC-493 MQC-005: candidates formed, merged and dissolved by why, per UTC day; the median
    /// minutes from a merge's last arming to it; an entry written before the why was says
    /// errored.
    #[test]
    fn the_queue_counts_candidates_per_day_and_the_minutes_to_merge() {
        let day1 = 20_000 * DAY;
        let events = [
            event(day1 + 10 * MIN, "app", FORMED, "merge train #1, #2"),
            event(
                day1 + 20 * MIN,
                "app",
                DISSOLVED,
                "merge train #1, #2: #2's own run ended failed (why: member_left)",
            ),
            event(day1 + 21 * MIN, "app", FORMED, "merge train #1"),
            event(day1 + 40 * MIN, "app", MERGED, "merge train #1 merged"),
            event(day1 + DAY, "app", DISSOLVED, "an old entry without its why"),
            event(day1, "app#1", "delivery.agent.merge_armed", ""),
            event(day1 + 5 * MIN, "app#1", "delivery.agent.merge_armed", ""),
            event(day1 + 41 * MIN, "app#1", "delivery.agent.merge", ""),
            event(day1 + 2 * MIN, "app#3", "delivery.person.merge_armed", ""),
            event(day1 + 12 * MIN, "app#3", "delivery.merge", ""),
            // A merge with no arming before it is no queue's.
            event(day1 + 30 * MIN, "app#4", "delivery.person.merge", ""),
        ];
        let numbers = queue_numbers(&events, None);
        assert_eq!(numbers.days.len(), 2);
        let first = &numbers.days[0];
        assert_eq!((first.formed, first.merged), (2, 1));
        assert_eq!(first.dissolved.get("member_left"), Some(&1));
        assert_eq!(numbers.days[1].dissolved.get("errored"), Some(&1));
        assert_eq!(numbers.merges, 2);
        // 36 minutes (from the last arming) and 10: the median of two is their mean.
        assert_eq!(numbers.median_arm_to_merge_minutes, Some(23.0));
        assert_eq!(queue_numbers(&[], None), QueueNumbers::default());
        // From the second day: the first day's candidates and merges are not counted, and a
        // merge there with no arming of its own is not paired with an arming used before.
        let later = [
            event(day1, "app#5", "delivery.agent.merge_armed", ""),
            event(day1 + 10 * MIN, "app#5", "delivery.agent.merge", ""),
            event(day1 + DAY + MIN, "app#5", "delivery.merge", ""),
        ];
        let mut both = events.to_vec();
        both.extend(later);
        let numbers = queue_numbers(&both, Some(Timestamp(day1 + DAY)));
        assert_eq!(numbers.days.len(), 1);
        assert_eq!(numbers.days[0].dissolved.get("errored"), Some(&1));
        assert_eq!(
            (numbers.merges, numbers.median_arm_to_merge_minutes),
            (0, None)
        );
    }

    /// SPEC-497 MQW-004: the dissolved candidates in the range are listed newest first with
    /// their why and whole reason, at most ten; a failed member's own run has its own why.
    #[test]
    fn each_dissolved_candidate_is_listed_with_its_reason() {
        let day = 20_000 * DAY;
        let mut events: Vec<QueueEvent> = (0..12)
            .map(|i| {
                event(
                    day + i * MIN,
                    "app",
                    DISSOLVED,
                    &format!("merge train #{i}: its CI run failed (why: bisected)"),
                )
            })
            .collect();
        events.push(event(
            day + 20 * MIN,
            "app",
            DISSOLVED,
            "merge train #1, #2: #2's own run at abc ended failed: it leaves the candidate \
             (why: member_failed)",
        ));
        events.push(event(
            day - DAY,
            "app",
            DISSOLVED,
            "before the range (why: errored)",
        ));
        let numbers = queue_numbers(&events, Some(Timestamp(day)));
        assert_eq!(numbers.dissolved_recent.len(), RECENT_DISSOLVED);
        let (why, newest) = &numbers.dissolved_recent[0];
        assert_eq!(*why, Dissolved::MemberFailed);
        assert!(newest.detail.contains("#2's own run"), "{newest:?}");
        assert!(
            numbers.dissolved_recent[1]
                .1
                .detail
                .starts_with("merge train #11")
        );
        assert!(
            numbers
                .dissolved_recent
                .iter()
                .all(|(_, e)| !e.detail.contains("before the range"))
        );
        assert_eq!(numbers.days[0].dissolved.get("member_failed"), Some(&1));
    }

    /// SPEC-497 MQW-003: pull requests that change files under the same `apart` pattern never
    /// share a candidate; under different patterns, or outside them, they do; the first in arm
    /// order goes first.
    #[test]
    fn derived_files_keep_their_changes_apart() -> Result<(), crate::scope::ScopeError> {
        let apart = [Pattern::parse("gen/")?, Pattern::parse("Cargo.lock")?];
        let changed =
            |pr: u64, paths: &[&str]| (pr, paths.iter().map(|p| (*p).to_owned()).collect());
        let (kept, held) = held_apart(
            &apart,
            &[
                changed(1, &["gen/a.txt", "src/a.rs"]),
                changed(2, &["Cargo.lock"]),
                changed(3, &["gen/b.txt"]),
                changed(4, &["src/b.rs"]),
                changed(5, &["Cargo.lock", "gen/c.txt"]),
            ],
        );
        assert_eq!(kept, [1, 2, 4]);
        assert_eq!(
            held,
            [
                HeldApart {
                    pr: 3,
                    with: 1,
                    pattern: "gen/**".into()
                },
                HeldApart {
                    pr: 5,
                    with: 1,
                    pattern: "gen/**".into()
                },
            ]
        );
        let (kept, held) = held_apart(&[], &[changed(1, &["gen/a"]), changed(2, &["gen/b"])]);
        assert_eq!((kept, held.len()), (vec![1, 2], 0), "nothing declared");
        Ok(())
    }

    fn open(test: &str, task: Option<u32>) -> OpenFailure {
        OpenFailure {
            test: test.to_owned(),
            revision: "0123456789abcdef".to_owned(),
            merge_task: task,
        }
    }

    /// SPEC-676 RGS-002, RGS-008: open failures stop the queue; quarantined tests do not, and
    /// a job without known tests is never one.
    #[test]
    fn open_failures_but_quarantined_tests_stop_the_queue() {
        assert_eq!(MergeStop::of(Vec::new(), &[], "r"), None);
        let flaky = open("app tests::flaky", Some(4));
        assert_eq!(
            MergeStop::of(vec![flaky.clone()], &["app tests::flaky"], "r"),
            None
        );
        let job = open("job tests (1/4)", None);
        let stop = MergeStop::of(vec![flaky, job.clone()], &["app tests::flaky"], "run-9");
        assert_eq!(
            stop,
            Some(MergeStop {
                failures: vec![job],
                run: "run-9".to_owned()
            })
        );
    }

    /// SPEC-676 RGS-005: a marked repair or a breaker's merge goes through, nobody else's.
    #[test]
    fn only_the_repair_goes_through_a_stopped_queue() {
        let stop = MergeStop {
            failures: vec![
                open("a", Some(7)),
                open("b", Some(3)),
                open("c", Some(7)),
                open("d", None),
            ],
            run: "r".to_owned(),
        };
        assert_eq!(stop.breakers(), [3, 7]);
        assert!(stop.lets_through(7, false));
        assert!(stop.lets_through(3, false));
        assert!(!stop.lets_through(5, false));
        assert!(stop.lets_through(5, true));
    }

    /// SPEC-676 RGS-003: the note says why, which failure, whose, and what ends the wait.
    #[test]
    fn a_held_merge_is_told_why_it_waits() {
        let stop = MergeStop {
            failures: vec![open("store::writes", Some(412)), open("job e2e", None)],
            run: "ci_9".to_owned(),
        };
        let note = stop.waiting_note(415, "dev");
        for said in [
            "Pull request #415 waits: the after-merge gate of `dev` is red",
            "`store::writes` (first at 0123456789ab, task #412)",
            "`job e2e` (first at 0123456789ab, an untracked merge)",
            "run ci_9",
            "it merges by itself then",
            "`repairs_gate: true`",
        ] {
            assert!(note.contains(said), "{said}: {note}");
        }
    }

    #[test]
    fn a_kind_reads_back_from_its_detail() {
        for kind in Dissolved::ALL {
            let detail = format!("merge train #1: something (why: {})", kind.as_str());
            assert_eq!(Dissolved::of_detail(&detail), kind);
        }
    }
}
