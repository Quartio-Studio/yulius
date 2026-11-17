//! The independent code review's agents and the model of each round (SPEC-493 fix 1).
//!
//! Every task session is launched knowing two reviewer agents that Yulius defines at the launch
//! (`--agents`), out of the agent's reach: [`REVIEWER_AGENT`] runs [`REVIEW_CLAUDE_MODEL`] (Fable
//! 5.2) and [`LATE_REVIEWER_AGENT`] runs Opus 5.5, both at [`REVIEWER_EFFORT`] and by full model
//! id. The agent always starts the review with [`REVIEWER_AGENT`]; Yulius sees the start and
//! decides the round's model ([`round_agent`]): rounds 1 and 2 of a pull request on Fable 5.2,
//! later rounds on Opus 5.5 (the person's decision of 2026-11-13).

use crate::harness::{FALLBACK_CLAUDE_MODEL, REVIEW_CLAUDE_MODEL};

/// The agent the independent code review is started with.
pub const REVIEWER_AGENT: &str = "yulius-reviewer";

/// The agent a review round runs as from round [`LATE_ROUND`] on, or when no subscription can
/// run the reviewer's model.
pub const LATE_REVIEWER_AGENT: &str = "yulius-reviewer-opus";

/// The model of [`LATE_REVIEWER_AGENT`]: Opus 5.5 by its full id.
pub const LATE_REVIEWER_MODEL: &str = FALLBACK_CLAUDE_MODEL;

/// The first round that runs on [`LATE_REVIEWER_AGENT`].
pub const LATE_ROUND: u32 = 3;

/// A reviewer's effort: high, never max (the request: max at review time ran out of usage most
/// often, 35 of 62 first reviews).
pub const REVIEWER_EFFORT: &str = "high";

/// What a reviewer is told, whatever its model: its contract.
pub const REVIEWER_PROMPT: &str = "You are the independent code reviewer of one change. You have \
no shared context with its author: review only the diff range, the spec and the instructions you \
are given, and load the skill `om-code-review` (and the repository's own review skill when it has \
one). Work read-only: do not edit, commit, stash, check out, reset or push. Mark every finding own \
code or pre-existing, with its severity, place and a concrete failure. Do not ask for changes \
because of CI alone: when you found nothing to fix and the head's Yulius CI run is still going or \
failed in a way the change does not cause, approve and say what CI still has to do (the merge \
needs it green); a failure the change causes is a blocker. End your report with a last line that is \
exactly `Verdict: approve` or `Verdict: request changes`.";

/// What Yulius tells an author after a red run, a failed merge candidate or a dropped armed
/// merge, once the fix is committed (SPEC-506 FST-001): the approval it had still counts across a
/// small fix, so it calls `delivery_merge` again instead of asking for a new round (ADR-0345).
pub const KEEP_APPROVAL: &str = "call `delivery_merge` with the approved head as `reviewed_head`: \
an approval carries across a fix of at most 20 product lines (tests and documents do not count), \
and the call says when the fix needs a new round";

/// The model of each reviewer agent, by name.
pub const REVIEWER_MODELS: [(&str, &str); 2] = [
    (REVIEWER_AGENT, REVIEW_CLAUDE_MODEL),
    (LATE_REVIEWER_AGENT, LATE_REVIEWER_MODEL),
];

/// The agent round `round` (1 for a pull request's first) runs as.
#[must_use]
pub fn round_agent(round: u32) -> &'static str {
    if round >= LATE_ROUND {
        LATE_REVIEWER_AGENT
    } else {
        REVIEWER_AGENT
    }
}

/// The model `agent` runs, when it is one of Yulius's reviewers.
#[must_use]
pub fn reviewer_model(agent: &str) -> Option<&'static str> {
    REVIEWER_MODELS
        .iter()
        .find(|(name, _)| *name == agent)
        .map(|(_, model)| *model)
}

/// A review round's verdict.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// `Verdict: approve`.
    Approve,
    /// `Verdict: request changes`.
    RequestChanges,
}

impl Verdict {
    /// Stable name: `approve` or `request_changes`.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Approve => "approve",
            Self::RequestChanges => "request_changes",
        }
    }

    /// Parses [`Self::as_str`].
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "approve" => Some(Self::Approve),
            "request_changes" => Some(Self::RequestChanges),
            _ => None,
        }
    }
}

/// The verdict a report ends with: its last line that is not empty, read as
/// `Verdict: approve` or `Verdict: request changes` (any case, Markdown emphasis or code marks
/// around it allowed). `None` for a report without one.
#[must_use]
pub fn verdict_of(report: &str) -> Option<Verdict> {
    let last = report.lines().rev().find(|line| !line.trim().is_empty())?;
    let plain: String = last
        .chars()
        .filter(|c| !matches!(c, '*' | '_' | '`' | '>'))
        .collect::<String>()
        .to_ascii_lowercase();
    let rest = plain.trim().strip_prefix("verdict:")?.trim();
    let rest = rest.trim_end_matches('.').trim();
    match rest {
        "approve" | "approved" => Some(Verdict::Approve),
        "request changes" | "request_changes" | "changes requested" => {
            Some(Verdict::RequestChanges)
        }
        _ => None,
    }
}

/// One review round as the numbers read it (SPEC-493 REV-006).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoundFact {
    /// The pull request it reviewed: its task and number.
    pub pull: (String, u64),
    /// Its number among the pull request's rounds.
    pub round: u32,
    /// The model that answered, else the one asked for.
    pub model: String,
    /// Its verdict, when its report had one.
    pub verdict: Option<Verdict>,
    /// It ended with a report.
    pub reported: bool,
    /// It ran on the Opus reviewer because no Fable could run.
    pub no_fable: bool,
}

/// The review's numbers over the pull requests merged in a window (SPEC-493 REV-006).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ReviewNumbers {
    /// Pull requests with at least one recorded round.
    pub pull_requests: u32,
    /// Pull requests whose first round's (last) verdict asked for changes.
    pub first_rounds_requesting_changes: u32,
    /// Pull requests whose first round never reported (every reviewer of it cut off or lost).
    pub first_rounds_without_report: u32,
    /// The median of the rounds per pull request (its highest round number).
    pub median_rounds: Option<f64>,
    /// Rounds by the model that ran them, most first: a round once per model, however many of
    /// its reviewers ran.
    pub rounds_by_model: Vec<(String, u32)>,
    /// Rounds run on the Opus reviewer because no Fable could run.
    pub no_fable_rounds: u32,
}

/// Works out [`ReviewNumbers`] from the rounds of merged pull requests.
#[must_use]
pub fn review_numbers(rounds: &[RoundFact]) -> ReviewNumbers {
    use std::collections::BTreeMap;
    let mut pulls: BTreeMap<&(String, u64), Vec<&RoundFact>> = BTreeMap::new();
    for fact in rounds {
        pulls.entry(&fact.pull).or_default().push(fact);
    }
    let mut numbers = ReviewNumbers {
        pull_requests: u32::try_from(pulls.len()).unwrap_or(u32::MAX),
        ..ReviewNumbers::default()
    };
    let mut per_pull: Vec<u32> = Vec::new();
    for facts in pulls.values() {
        let first: Vec<&&RoundFact> = facts.iter().filter(|f| f.round == 1).collect();
        if !first.is_empty() && !first.iter().any(|f| f.reported) {
            numbers.first_rounds_without_report += 1;
        }
        // The last report of round 1 is its verdict.
        if first
            .iter()
            .rev()
            .find(|f| f.reported)
            .and_then(|f| f.verdict)
            == Some(Verdict::RequestChanges)
        {
            numbers.first_rounds_requesting_changes += 1;
        }
        per_pull.push(facts.iter().map(|f| f.round).max().unwrap_or(0));
    }
    per_pull.sort_unstable();
    numbers.median_rounds = match per_pull.len() {
        0 => None,
        n if n % 2 == 1 => Some(f64::from(per_pull[n / 2])),
        n => Some((f64::from(per_pull[n / 2 - 1]) + f64::from(per_pull[n / 2])) / 2.0),
    };
    // A round counts once per model, however many of its reviewers ran (a cut-off one started
    // again): rounds, not reviewer runs, as the rounds per pull request count them.
    let mut seen: std::collections::BTreeSet<(&(String, u64), u32, &str)> =
        std::collections::BTreeSet::new();
    let mut no_fable: std::collections::BTreeSet<(&(String, u64), u32)> =
        std::collections::BTreeSet::new();
    let mut by_model: BTreeMap<&str, u32> = BTreeMap::new();
    for fact in rounds {
        if seen.insert((&fact.pull, fact.round, fact.model.as_str())) {
            *by_model.entry(fact.model.as_str()).or_default() += 1;
        }
        if fact.no_fable {
            no_fable.insert((&fact.pull, fact.round));
        }
    }
    numbers.no_fable_rounds = u32::try_from(no_fable.len()).unwrap_or(u32::MAX);
    let mut by_model: Vec<(String, u32)> = by_model
        .into_iter()
        .map(|(model, n)| (model.to_owned(), n))
        .collect();
    by_model.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    numbers.rounds_by_model = by_model;
    numbers
}

#[cfg(test)]
mod tests {
    use super::*;

    /// SPEC-493 REV-002: rounds 1 and 2 of a pull request run on Fable 5.2, later rounds on
    /// Opus 5.5, each by its full id.
    #[test]
    fn the_first_two_rounds_run_fable_and_later_ones_opus() {
        assert_eq!(round_agent(1), REVIEWER_AGENT);
        assert_eq!(round_agent(2), REVIEWER_AGENT);
        assert_eq!(round_agent(3), LATE_REVIEWER_AGENT);
        assert_eq!(round_agent(9), LATE_REVIEWER_AGENT);
        assert_eq!(reviewer_model(REVIEWER_AGENT), Some("claude-fable-5-2"));
        assert_eq!(reviewer_model(LATE_REVIEWER_AGENT), Some("claude-opus-5-5"));
        assert_eq!(reviewer_model("general-purpose"), None);
        assert_eq!(REVIEWER_EFFORT, "high");
    }

    /// SPEC-493 REV-004: a report's verdict is its last line, as the reviewer's contract asks;
    /// anything else is no verdict.
    #[test]
    fn a_reports_verdict_is_its_last_line() {
        for (report, verdict) in [
            ("Findings…\n\nVerdict: approve", Some(Verdict::Approve)),
            (
                "x\nVerdict: request changes\n\n",
                Some(Verdict::RequestChanges),
            ),
            (
                "x\n**Verdict: Request changes**",
                Some(Verdict::RequestChanges),
            ),
            ("x\n`Verdict: approve.`", Some(Verdict::Approve)),
            ("Verdict: approve\nbut one more line", None),
            ("Verdict: maybe", None),
            ("I would approve this", None),
            ("", None),
        ] {
            assert_eq!(verdict_of(report), verdict, "{report:?}");
        }
        for verdict in [Verdict::Approve, Verdict::RequestChanges] {
            assert_eq!(Verdict::parse(verdict.as_str()), Some(verdict));
        }
        assert!(REVIEWER_PROMPT.contains("`Verdict: approve` or `Verdict: request changes`"));
    }

    /// SPEC-493 REV-006: the numbers of the request, from the rounds of merged pull requests.
    #[test]
    fn the_review_numbers_of_merged_pull_requests() {
        let fact =
            |task: &str, pr: u64, round: u32, model: &str, verdict: Option<Verdict>| RoundFact {
                pull: (task.to_owned(), pr),
                round,
                model: model.to_owned(),
                verdict,
                reported: verdict.is_some(),
                no_fable: false,
            };
        let (f, o) = ("claude-fable-5-2", "claude-opus-5-5");
        let (ok, no) = (Some(Verdict::Approve), Some(Verdict::RequestChanges));
        let rounds = [
            // Three rounds: the first asked for changes.
            fact("t1", 1, 1, f, no),
            fact("t1", 1, 2, f, no),
            fact("t1", 1, 3, o, ok),
            // Approved at once.
            fact("t2", 2, 1, f, ok),
            // Round 1 cut off, then reported by its reviewer started again.
            fact("t3", 3, 1, f, None),
            fact("t3", 3, 1, f, ok),
            // Round 1 never reported; round 2 on Opus for want of Fable.
            fact("t4", 4, 1, f, None),
            RoundFact {
                no_fable: true,
                ..fact("t4", 4, 2, o, ok)
            },
        ];
        let numbers = review_numbers(&rounds);
        assert_eq!(numbers.pull_requests, 4);
        assert_eq!(numbers.first_rounds_requesting_changes, 1);
        assert_eq!(numbers.first_rounds_without_report, 1);
        assert_eq!(numbers.median_rounds, Some(1.5));
        // t3's round 1 ran twice and counts once.
        assert_eq!(
            numbers.rounds_by_model,
            [(f.to_owned(), 5), (o.to_owned(), 2)]
        );
        assert_eq!(numbers.no_fable_rounds, 1);
        assert_eq!(review_numbers(&[]), ReviewNumbers::default());
    }
}
