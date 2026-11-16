//! The merge queue's numbers (SPEC-493 MQC-005): merge candidates formed, merged and dissolved
//! (by why) per day, and the minutes from arming a merge to the merge, read from the audit
//! entries the queue writes.

use std::collections::BTreeMap;

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
    /// A member left: its own run did not pass, it was armed again or is no longer armed.
    MemberLeft,
    /// Its run failed and it was bisected.
    Bisected,
    /// The integration branch moved in a way it did not predict.
    BaseMoved,
    /// Its run errored or was cancelled, or it could not go on.
    Errored,
    /// It took too long (its run, a member's own run, GitHub).
    TooLong,
}

impl Dissolved {
    /// Every kind, in the order the numbers list them.
    pub const ALL: [Self; 5] = [
        Self::MemberLeft,
        Self::Bisected,
        Self::BaseMoved,
        Self::Errored,
        Self::TooLong,
    ];

    /// The kind's name in an audit entry and in the read.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::MemberLeft => "member_left",
            Self::Bisected => "bisected",
            Self::BaseMoved => "base_moved",
            Self::Errored => "errored",
            Self::TooLong => "too_long",
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
                    *entry
                        .dissolved
                        .entry(Dissolved::of_detail(&event.detail).as_str())
                        .or_default() += 1;
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
    QueueNumbers {
        days: days.into_values().collect(),
        merges: u32::try_from(minutes.len()).unwrap_or(u32::MAX),
        median_arm_to_merge_minutes: median,
    }
}

/// How a CI run ended, for its minutes (SPEC-497 OFC-003).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunOutcome {
    /// It passed.
    Passed,
    /// It failed or errored, not only for the infrastructure.
    Failed,
    /// It failed or errored only for the infrastructure (every such job met one again).
    FailedOutside,
    /// It was cancelled (superseded, its candidate dissolved, `dev` moved).
    Cancelled,
}

/// One ended CI run: when it started, how long it took, how it ended, and the milliseconds its
/// jobs spent on runs an infrastructure failure threw away (SPEC-497 OFC-001).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RunMinutes {
    /// Its start.
    pub started: Timestamp,
    /// From its start to its end.
    pub ms: i64,
    /// How it ended.
    pub outcome: RunOutcome,
    /// Job time on runs thrown away for the infrastructure.
    pub rerun_ms: i64,
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

    fn add(&mut self, run: &RunMinutes) {
        #[expect(clippy::cast_precision_loss, reason = "minutes of a run fit")]
        let (minutes, rerun) = (
            run.ms.max(0) as f64 / 60_000.0,
            run.rerun_ms.max(0) as f64 / 60_000.0,
        );
        match run.outcome {
            RunOutcome::Passed => self.passed += minutes,
            RunOutcome::Failed => self.failed += minutes,
            RunOutcome::FailedOutside => self.failed_outside += minutes,
            RunOutcome::Cancelled => self.cancelled += minutes,
        }
        self.rerun += rerun;
    }
}

/// The minutes of `runs`, per UTC day of their start (oldest first) and in all.
#[must_use]
pub fn minutes_by_day(runs: &[RunMinutes]) -> (Vec<(String, Minutes)>, Minutes) {
    let mut days: BTreeMap<String, Minutes> = BTreeMap::new();
    let mut total = Minutes::default();
    for run in runs {
        let day = format_rfc3339(run.started)
            .chars()
            .take(10)
            .collect::<String>();
        days.entry(day).or_default().add(run);
        total.add(run);
    }
    (days.into_iter().collect(), total)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// SPEC-497 OFC-003: minutes by outcome per day and in all, the rerun time apart, and the
    /// share outside the change (failed only for the infrastructure, or cancelled).
    #[test]
    fn ci_minutes_are_counted_by_how_runs_ended() {
        let run = |at: i64, minutes: i64, outcome, rerun: i64| RunMinutes {
            started: Timestamp(at),
            ms: minutes * MIN,
            outcome,
            rerun_ms: rerun * MIN,
        };
        let runs = [
            run(0, 10, RunOutcome::Passed, 2),
            run(MIN, 6, RunOutcome::Failed, 0),
            run(2 * MIN, 3, RunOutcome::FailedOutside, 3),
            run(DAY, 1, RunOutcome::Cancelled, 0),
        ];
        let (days, total) = minutes_by_day(&runs);
        assert_eq!(
            days.iter().map(|(day, _)| day.as_str()).collect::<Vec<_>>(),
            ["1970-01-01", "1970-01-02"]
        );
        assert_eq!(
            total,
            Minutes {
                passed: 10.0,
                failed: 6.0,
                failed_outside: 3.0,
                cancelled: 1.0,
                rerun: 5.0,
            }
        );
        assert_eq!(total.outside_share(), Some(0.2));
        assert_eq!(days[1].1.cancelled, 1.0);
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

    #[test]
    fn a_kind_reads_back_from_its_detail() {
        for kind in Dissolved::ALL {
            let detail = format!("merge train #1: something (why: {})", kind.as_str());
            assert_eq!(Dissolved::of_detail(&detail), kind);
        }
    }
}
