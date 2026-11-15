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
code or pre-existing, with its severity, place and a concrete failure. End your report with a last \
line that is exactly `Verdict: approve` or `Verdict: request changes`.";

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
}
