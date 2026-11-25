//! Which waiting staging request the full gate judges next, and which need no run of their own
//! (SPEC-653 NWF-001 to NWF-005), decided purely.
//!
//! The caller lists first-parent revisions of the integration branch with their *position*
//! (as in [`crate::staging`]), the full gate's newest verdict on each, and whether a `testing`
//! staging request waits at it. Every revision at or below a position contains every revision
//! below it, so:
//!
//! - a request at or below a revision whose full gate passed needs no run (*covered*);
//! - the newest request above every judged revision is judged first;
//! - a request under a red revision needs a run only to name the merge that brought a new
//!   failure: the red revision's range (down to the judged revision below it) is bisected over
//!   its waiting requests, and until then nobody is told who broke the gate (*unresolved*).

use std::collections::BTreeSet;

/// The full gate's newest verdict on a revision.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Gate {
    /// A run judges it now (queued or running).
    Active,
    /// It passed without skipping a job.
    Passed,
    /// It failed, or passed while skipping an unresolved job, naming these failures: tests, jobs
    /// that did not identify their tests, or the run's own failure.
    Failed(BTreeSet<String>),
}

/// One first-parent revision of the integration branch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Point {
    /// Its first-parent position.
    pub position: u64,
    /// The full gate's newest verdict on it; `None`: nothing judged it.
    pub gate: Option<Gate>,
    /// A `testing` staging request waits at it.
    pub asked: bool,
}

/// What the queue does with its `testing` requests now.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Judging {
    /// Asked positions (not passed themselves) at or below a passed revision, with the nearest
    /// such revision above them: they need no run of their own (NWF-001).
    pub covered: Vec<(u64, u64)>,
    /// Asked positions nothing judged, whose nearest judged revision above them failed with no
    /// failure new to its range, with that red revision: they need no run (NWF-003).
    pub inherited: Vec<(u64, u64)>,
    /// Red positions whose range still holds a waiting request: who broke the gate is not known
    /// until that request is judged (NWF-004).
    pub unresolved: Vec<u64>,
    /// The position the next full-gate run judges, if the lane is free (NWF-002, NWF-004).
    pub run: Option<u64>,
}

/// Plans the full gate's judging of the waiting requests (SPEC-653) from first-parent
/// revisions in any order; a position listed twice counts once (asked if either is, the first
/// verdict given).
#[must_use]
pub fn judging(points: &[Point]) -> Judging {
    let mut sorted: Vec<Point> = Vec::with_capacity(points.len());
    let mut by_position: Vec<&Point> = points.iter().collect();
    by_position.sort_by_key(|p| p.position);
    for point in by_position {
        match sorted.last_mut() {
            Some(last) if last.position == point.position => {
                last.asked |= point.asked;
                if last.gate.is_none() {
                    last.gate.clone_from(&point.gate);
                }
            }
            _ => sorted.push(point.clone()),
        }
    }
    let mut out = Judging::default();
    // Narrowing points of each red revision, oldest red revision first.
    let mut ranges: Vec<(u64, Vec<u64>)> = Vec::new();
    for (at, point) in sorted.iter().enumerate() {
        if !point.asked || point.gate == Some(Gate::Passed) {
            continue;
        }
        let above = &sorted[at + 1..];
        if let Some(green) = above.iter().find(|p| p.gate == Some(Gate::Passed)) {
            out.covered.push((point.position, green.position));
            continue;
        }
        if point.gate.is_some() {
            continue;
        }
        let Some((offset, judged)) = above.iter().enumerate().find(|(_, p)| p.gate.is_some())
        else {
            continue;
        };
        let Some(Gate::Failed(failures)) = &judged.gate else {
            continue;
        };
        match brings_new_failures(failures, sorted[..at + 1 + offset].iter().rev()) {
            None => {}
            Some(false) => out.inherited.push((point.position, judged.position)),
            Some(true) => match ranges.iter_mut().find(|(red, _)| *red == judged.position) {
                Some((_, waiting)) => waiting.push(point.position),
                None => ranges.push((judged.position, vec![point.position])),
            },
        }
    }
    ranges.sort_by_key(|(red, _)| *red);
    out.unresolved = ranges.iter().map(|(red, _)| *red).collect();
    out.run = match ranges.first() {
        // The middle waiting request of the oldest unresolved range (NWF-004).
        Some((_, waiting)) => waiting.get(waiting.len() / 2).copied(),
        // The newest request above every judged revision (NWF-002).
        None => {
            let judged_top = sorted.iter().rev().find(|p| p.gate.is_some());
            sorted
                .iter()
                .rev()
                .take_while(|p| judged_top.is_none_or(|top| p.position > top.position))
                .find(|p| p.asked)
                .map(|p| p.position)
        }
    };
    out
}

/// Whether `failures` hold one the nearest judged revision in `below` (newest first) did not:
/// `None` while that revision is still being judged; every failure is new when nothing below
/// was judged or it passed.
fn brings_new_failures<'a>(
    failures: &BTreeSet<String>,
    mut below: impl Iterator<Item = &'a Point>,
) -> Option<bool> {
    match below.find_map(|p| p.gate.as_ref()) {
        Some(Gate::Active) => None,
        Some(Gate::Failed(earlier)) => Some(!failures.is_subset(earlier)),
        Some(Gate::Passed) | None => Some(!failures.is_empty()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn failed(tests: &[&str]) -> Option<Gate> {
        Some(Gate::Failed(
            tests.iter().map(|t| (*t).to_owned()).collect(),
        ))
    }

    fn point(position: u64, gate: Option<Gate>, asked: bool) -> Point {
        Point {
            position,
            gate,
            asked,
        }
    }

    /// A position as an index (positions in these tests are small).
    fn ix(at: u64) -> usize {
        usize::try_from(at).unwrap_or(usize::MAX)
    }

    fn waiting(position: u64) -> Point {
        point(position, None, true)
    }

    /// NWF-001: a request at or below a green revision needs no run, also one whose own gate
    /// failed or still runs; a request above it is judged.
    #[test]
    fn a_request_under_a_green_newer_revision_gets_no_run() {
        let plan = judging(&[
            waiting(1),
            point(2, failed(&["t"]), true),
            point(3, Some(Gate::Active), true),
            point(4, Some(Gate::Passed), false),
            point(5, Some(Gate::Passed), true),
            waiting(6),
        ]);
        assert_eq!(plan.covered, [(1, 4), (2, 4), (3, 4)]);
        assert!(plan.inherited.is_empty() && plan.unresolved.is_empty());
        assert_eq!(plan.run, Some(6));
        assert_eq!(
            judging(&[waiting(1), point(2, Some(Gate::Passed), false)]).run,
            None,
            "nothing is left to judge"
        );
    }

    /// NWF-002: among several waiting requests the newest is judged first, and nothing older
    /// gets a run before its verdict.
    #[test]
    fn the_newest_goes_first_among_several() {
        let plan = judging(&[
            point(1, Some(Gate::Passed), false),
            waiting(2),
            waiting(4),
            point(3, None, false),
            waiting(5),
        ]);
        assert_eq!(plan.run, Some(5));
        assert_eq!(
            plan,
            Judging {
                run: Some(5),
                ..Judging::default()
            }
        );
        let running = judging(&[waiting(2), waiting(4), point(5, Some(Gate::Active), false)]);
        assert_eq!(
            running,
            Judging::default(),
            "an active run above holds them"
        );
        let newer = judging(&[waiting(2), point(3, Some(Gate::Active), true), waiting(4)]);
        assert_eq!(
            newer.run,
            Some(4),
            "a newer request is next once the lane frees"
        );
        assert!(newer.covered.is_empty() && newer.inherited.is_empty());
        assert_eq!(
            judging(&[waiting(7), waiting(7), point(7, None, false)]).run,
            Some(7),
            "one position listed twice counts once"
        );
    }

    /// NWF-003: under a red revision that brings no new failure, a waiting request needs no
    /// run; a new failure (also one under no judged revision, or above a green one) makes the
    /// range unresolved.
    #[test]
    fn a_request_under_a_red_revision_needs_a_run_only_for_a_new_failure() {
        let old = judging(&[
            point(1, failed(&["a", "b"]), false),
            waiting(2),
            waiting(3),
            point(4, failed(&["a"]), true),
        ]);
        assert_eq!(old.inherited, [(2, 4), (3, 4)]);
        assert!(old.unresolved.is_empty());
        assert_eq!(old.run, None);
        let new = judging(&[
            point(1, failed(&["a"]), false),
            waiting(2),
            point(3, failed(&["a", "c"]), false),
        ]);
        assert_eq!(
            (new.unresolved.as_slice(), new.run),
            ([3].as_slice(), Some(2))
        );
        let unbounded = judging(&[waiting(2), point(3, failed(&["a"]), false)]);
        assert_eq!(
            unbounded.run,
            Some(2),
            "nothing judged below: every failure is new"
        );
        let after_green = judging(&[
            point(1, Some(Gate::Passed), false),
            waiting(2),
            point(3, failed(&["a"]), false),
        ]);
        assert_eq!(after_green.unresolved, [3]);
        let busy_below = judging(&[
            point(1, Some(Gate::Active), false),
            waiting(2),
            point(3, failed(&["a"]), false),
        ]);
        assert_eq!(
            busy_below,
            Judging::default(),
            "the range's floor is still being judged"
        );
    }

    /// NWF-004: the oldest unresolved range is bisected first, before a newer request, and its
    /// middle waiting request is judged.
    #[test]
    fn a_new_failure_narrows_the_oldest_range_before_newer_requests() {
        let plan = judging(&[
            point(1, Some(Gate::Passed), false),
            waiting(2),
            waiting(3),
            waiting(4),
            point(5, failed(&["x"]), true),
            waiting(6),
            point(7, failed(&["x", "y"]), false),
            waiting(8),
        ]);
        assert_eq!(plan.unresolved, [5, 7]);
        assert_eq!(plan.run, Some(3));
        assert!(plan.covered.is_empty() && plan.inherited.is_empty());
    }

    /// Narrowing never costs more than `ceil(log2(k + 1))` runs for `k` waiting requests, and
    /// it ends on the request whose merge brought the failure.
    #[test]
    fn narrowing_is_logarithmic_and_finds_the_breaking_merge() {
        for k in 1..=20_u64 {
            let bound = u64::from(u64::BITS - k.leading_zeros());
            for breaker in 1..=k + 1 {
                let truth = |at: u64| {
                    if at >= breaker {
                        failed(&["x"])
                    } else {
                        Some(Gate::Passed)
                    }
                };
                let mut points = vec![point(0, Some(Gate::Passed), false)];
                points.extend((1..=k).map(waiting));
                points.push(point(k + 1, truth(k + 1), true));
                let mut runs = 0;
                loop {
                    let plan = judging(&points);
                    for (at, _) in plan.covered.iter().chain(&plan.inherited) {
                        points[ix(*at)].asked = false;
                    }
                    if plan.unresolved.is_empty() {
                        break;
                    }
                    assert!(
                        plan.run.is_some(),
                        "an unresolved range has a narrowing run"
                    );
                    let Some(at) = plan.run else { break };
                    points[ix(at)].gate = truth(at);
                    runs += 1;
                }
                assert!(
                    runs <= bound,
                    "k={k} breaker={breaker}: {runs} runs > {bound}"
                );
                let origin = points
                    .iter()
                    .find(|p| matches!(p.gate, Some(Gate::Failed(_))))
                    .map(|p| p.position);
                assert_eq!(origin, Some(breaker), "k={k}");
                assert!(
                    points[1..=ix(k)]
                        .iter()
                        .all(|p| !p.asked || p.gate.is_some()),
                    "every waiting request is decided without a run of its own or judged"
                );
            }
        }
    }

    /// The morning of 2026-11-25 in quartio-v2 (SPEC-653 Context), replayed at its real times.
    mod morning {
        use super::*;

        /// The integration branch's first-parent merges from 04:16 to 07:22, oldest first.
        const MERGES: [(&str, &str); 17] = [
            ("180c2e537", "04:16:11"),
            ("0d26f0ad9", "04:22:22"),
            ("f7f3a4624", "04:25:53"),
            ("3346ca6a5", "04:38:31"),
            ("a751700ae", "04:48:17"),
            ("fe0a6c731", "05:05:13"),
            ("083a1d1c8", "05:14:28"),
            ("867dfe486", "05:36:48"),
            ("d17bd39c2", "05:44:12"),
            ("2adf189d8", "05:51:05"),
            ("d2bffda38", "06:02:55"),
            ("0a0afa473", "06:05:48"),
            ("26576219e", "06:12:08"),
            ("5d1294696", "06:36:06"),
            ("d40122574", "06:57:19"),
            ("bc811347d", "07:15:46"),
            ("b8e2e7cf7", "07:21:56"),
        ];

        /// The staging requests that got runs that morning, asked 30 s after their merge
        /// (`5d1294696` was asked at 06:36).
        const ASKED: [&str; 8] = [
            "0d26f0ad9",
            "3346ca6a5",
            "867dfe486",
            "d17bd39c2",
            "2adf189d8",
            "d2bffda38",
            "5d1294696",
            "d40122574",
        ];

        /// The verdicts: a test `0d26f0ad9` broke and `083a1d1c8` no longer failed, the new
        /// failure of `867dfe486` until the repair `5d1294696`, and that revision's own.
        fn truth(revision: &str) -> Gate {
            let at = position(revision);
            let mut failing = BTreeSet::new();
            if (position("0d26f0ad9")..position("083a1d1c8")).contains(&at) {
                failing.insert("mail::reset_link".to_owned());
            }
            if (position("867dfe486")..position("5d1294696")).contains(&at) {
                failing.insert("engine::progress_card".to_owned());
            }
            if at >= position("5d1294696") {
                failing.insert("engine::progress_resume".to_owned());
            }
            if failing.is_empty() {
                Gate::Passed
            } else {
                Gate::Failed(failing)
            }
        }

        /// A full run, and the time from one run's end to the next run's start (the staging
        /// tick and CI admission); a request kicks its queue at once.
        const RUN: u64 = 15 * 60;
        const LATENCY: u64 = 30;
        /// Until 05:34 the integration branch's runs hold the lane; the last one judges
        /// `083a1d1c8` and passes.
        const HELD_UNTIL: &str = "05:34:00";
        const END: &str = "07:37:00";

        fn clock(text: &str) -> u64 {
            text.split(':').fold(0, |total, part| {
                total * 60 + part.parse::<u64>().unwrap_or(u64::MAX)
            })
        }

        fn position(revision: &str) -> u64 {
            let at = MERGES
                .iter()
                .position(|(sha, _)| *sha == revision)
                .unwrap_or(MERGES.len());
            u64::try_from(at).unwrap_or(u64::MAX) + 1
        }

        fn merged_at(revision: &str) -> u64 {
            clock(MERGES[ix(position(revision) - 1)].1)
        }

        #[derive(Clone, Copy, PartialEq, Eq, Debug)]
        enum Outcome {
            Testing,
            NoRunNeeded,
            Ready,
            Inherited,
            GateFailed,
        }

        #[derive(Default)]
        struct Replay {
            /// Runs: revision and start.
            runs: Vec<(&'static str, u64)>,
            /// Each request's outcome and when it came.
            told: Vec<(&'static str, Outcome, u64)>,
        }

        impl Replay {
            fn outcome(&self, revision: &str) -> (Outcome, u64) {
                self.told
                    .iter()
                    .find(|(sha, _, _)| *sha == revision)
                    .map_or((Outcome::Testing, 0), |(_, outcome, at)| (*outcome, *at))
            }

            fn ran(&self, revision: &str) -> Option<u64> {
                self.runs
                    .iter()
                    .find(|(sha, _)| *sha == revision)
                    .map(|(_, at)| *at)
            }
        }

        /// Replays the morning second by second, with the newest-first plan or the old
        /// oldest-first order.
        fn replay(newest_first: bool) -> Replay {
            let mut gates: Vec<Option<Gate>> = vec![None; MERGES.len()];
            let mut requests: Vec<(&'static str, Outcome)> = Vec::new();
            let mut running: Option<(&'static str, u64)> = None;
            let mut free_at = clock(HELD_UNTIL) + LATENCY;
            let mut out = Replay::default();
            let index = |revision: &str| ix(position(revision) - 1);
            for now in clock("04:16:00")..clock(END) {
                if now == merged_at("083a1d1c8") {
                    gates[index("083a1d1c8")] = Some(Gate::Active);
                }
                if now == clock(HELD_UNTIL) {
                    gates[index("083a1d1c8")] = Some(Gate::Passed);
                }
                if let Some((revision, end)) = running
                    && now == end
                {
                    gates[index(revision)] = Some(truth(revision));
                    running = None;
                    free_at = now + LATENCY;
                }
                for revision in ASKED {
                    if now == merged_at(revision) + 30 {
                        requests.push((revision, Outcome::Testing));
                    }
                }
                let merged = MERGES
                    .iter()
                    .filter(|(sha, _)| merged_at(sha) <= now)
                    .count();
                let points: Vec<Point> = (0..merged)
                    .map(|at| Point {
                        position: u64::try_from(at).unwrap_or(u64::MAX) + 1,
                        gate: gates[at].clone(),
                        asked: requests
                            .iter()
                            .any(|(sha, state)| *state == Outcome::Testing && index(sha) == at),
                    })
                    .collect();
                let plan = judging(&points);
                let mut next = None;
                requests.sort_by_key(|(sha, _)| position(sha));
                for (revision, state) in &mut requests {
                    if *state != Outcome::Testing {
                        continue;
                    }
                    let at = position(revision);
                    let decided = if newest_first
                        && plan
                            .covered
                            .iter()
                            .chain(&plan.inherited)
                            .any(|(p, _)| *p == at)
                    {
                        Some(Outcome::NoRunNeeded)
                    } else {
                        match &gates[index(revision)] {
                            Some(Gate::Passed) => Some(Outcome::Ready),
                            Some(Gate::Failed(_))
                                if newest_first && plan.unresolved.contains(&at) =>
                            {
                                None
                            }
                            Some(Gate::Failed(failing)) => {
                                let below = points[..index(revision)].iter().rev();
                                Some(if brings_new_failures(failing, below) == Some(false) {
                                    Outcome::Inherited
                                } else {
                                    Outcome::GateFailed
                                })
                            }
                            Some(Gate::Active) => {
                                if !newest_first {
                                    next.get_or_insert(None);
                                }
                                None
                            }
                            None => {
                                if !newest_first {
                                    next.get_or_insert(Some(*revision));
                                }
                                None
                            }
                        }
                    };
                    if let Some(outcome) = decided {
                        *state = outcome;
                        out.told.push((revision, outcome, now));
                    }
                }
                let next = if newest_first {
                    plan.run.map(|at| MERGES[ix(at - 1)].0)
                } else {
                    next.flatten()
                };
                if let Some(revision) = next
                    && running.is_none()
                    && now >= free_at
                {
                    gates[index(revision)] = Some(Gate::Active);
                    running = Some((revision, now + RUN));
                    out.runs.push((revision, now));
                }
            }
            out
        }

        /// The old order reproduces that morning's runs: their revisions, and their starts and
        /// the two reports within two minutes of the real ones.
        #[test]
        fn the_old_order_reproduces_the_mornings_runs() {
            let old = replay(false);
            let real = [
                ("0d26f0ad9", "05:35"),
                ("3346ca6a5", "05:51"),
                ("867dfe486", "06:07"),
                ("d17bd39c2", "06:22"),
                ("2adf189d8", "06:37"),
                ("d2bffda38", "06:52"),
                ("5d1294696", "07:07"),
                ("d40122574", "07:22"),
            ];
            let near = |at: u64, real: &str| at.abs_diff(clock(&format!("{real}:00"))) <= 120;
            assert_eq!(old.runs.len(), real.len(), "{:?}", old.runs);
            for ((revision, start), (sha, when)) in old.runs.iter().zip(real) {
                assert_eq!(*revision, sha);
                assert!(
                    near(*start, when),
                    "{revision} started at {start}, really {when}"
                );
            }
            assert_eq!(
                old.outcome("0d26f0ad9").0,
                Outcome::GateFailed,
                "task #402 was told"
            );
            let (state, at) = old.outcome("867dfe486");
            assert!(state == Outcome::GateFailed && near(at, "06:21"), "{at}");
            let (state, at) = old.outcome("5d1294696");
            assert!(state == Outcome::GateFailed && near(at, "07:21"), "{at}");
        }

        /// SPEC-653 acceptance: no run on `0d26f0ad9` or `3346ca6a5`, the new failure at
        /// `867dfe486` reported by 05:52 and the one at `5d1294696` by 06:51, with fewer runs.
        #[test]
        fn newest_first_reports_the_new_failures_one_run_after_their_merge() {
            let new = replay(true);
            for covered in ["0d26f0ad9", "3346ca6a5"] {
                assert_eq!(new.ran(covered), None, "{covered}");
                assert_eq!(
                    new.outcome(covered),
                    (Outcome::NoRunNeeded, clock("05:34:00")),
                    "{covered}: 083a1d1c8 passed and contains it"
                );
            }
            let (state, at) = new.outcome("867dfe486");
            assert_eq!(state, Outcome::GateFailed);
            assert!(at < clock("05:53:00"), "867dfe486 reported at {at}");
            let (state, at) = new.outcome("5d1294696");
            assert_eq!(state, Outcome::GateFailed);
            assert!(at < clock("06:52:00"), "5d1294696 reported at {at}");
            for again in ["d17bd39c2", "2adf189d8", "d2bffda38"] {
                assert!(
                    matches!(
                        new.outcome(again).0,
                        Outcome::NoRunNeeded | Outcome::Inherited
                    ),
                    "{again}: its failure is 867dfe486's"
                );
            }
            assert_eq!(
                new.ran("d17bd39c2"),
                None,
                "2adf189d8's verdict answered it"
            );
            let old = replay(false);
            assert!(
                new.runs.len() < old.runs.len(),
                "{:?} vs {:?}",
                new.runs,
                old.runs
            );
        }
    }
}
