//! The independent code review's rounds as Yulius sees them (SPEC-493 REV-002 to REV-004).
//!
//! A task starts its review with the Agent tool and [`REVIEWER_AGENT`]. Its `PreToolUse` reaches
//! `App::review_gate`, which numbers the round among those of the task's open pull request,
//! decides the reviewer it runs as ([`round_agent`]) and records it; a different reviewer than
//! the one asked for is given back as the tool's new input. `SubagentStart` links the round to
//! Claude Code's subagent, `SubagentStop` ends it with the report's verdict. A merge closes the
//! pull request's rounds, so the next pull request of the task starts at round 1 again.
//!
//! A round is counted once it ended with a report. A reviewer started again for a round that has
//! none (a usage limit cut it off, or it still runs) keeps that round's number and reviewer.
//! When Yulius cannot be asked, the reviewer starts as asked (REV-003).

use std::path::Path;

use serde_json::Value;
use yulius_contracts::hooks::{HookInput, HookOutput, events};
use yulius_domain::harness::claude_model_name;
use yulius_domain::review::{
    LATE_REVIEWER_AGENT, REVIEWER_AGENT, reviewer_model, round_agent, verdict_of,
};
use yulius_domain::usage::ModelFamily;

use crate::context::App;
use crate::coordination::AgentContext;
use crate::error::AppResult;
use crate::model::ReviewRoundRecord;
use yulius_domain::time::Timestamp;

/// Yulius's reviewer agents.
const REVIEWERS: [&str; 2] = [REVIEWER_AGENT, LATE_REVIEWER_AGENT];

impl App {
    /// `PreToolUse` of the Agent tool: a review round's start, or nothing for another subagent.
    /// A failure to decide lets the reviewer start as asked (REV-003).
    pub(crate) async fn review_gate(&self, ctx: &AgentContext, input: &HookInput) -> HookOutput {
        match self.start_review_round(ctx, input).await {
            Ok(output) => output,
            Err(err) => {
                tracing::warn!(error = %err, task = %ctx.task.id, "review round not recorded");
                HookOutput::default()
            }
        }
    }

    async fn start_review_round(
        &self,
        ctx: &AgentContext,
        input: &HookInput,
    ) -> AppResult<HookOutput> {
        let Some(tool_input) = input.tool_input.as_ref().and_then(Value::as_object) else {
            return Ok(HookOutput::default());
        };
        let Some(asked) = tool_input
            .get("subagent_type")
            .and_then(Value::as_str)
            .filter(|asked| REVIEWERS.contains(asked))
        else {
            return Ok(HookOutput::default());
        };
        let store = &self.deps.store;
        let rounds = store.review_rounds(&ctx.task.id, true).await?;
        // The round after the last one that reported: a round reported by two of its
        // reviewers (one resumed after a cut-off, or two started at once) counts once.
        let number = rounds
            .iter()
            .filter(|r| r.ended_at.is_some())
            .map(|r| r.round)
            .max()
            .unwrap_or(0)
            .saturating_add(1);
        let unfinished = rounds
            .iter()
            .rev()
            .find(|r| r.round == number && r.ended_at.is_none());
        let (mut agent, mut reason) = match unfinished {
            Some(round) => (round.agent.clone(), "kept"),
            None => (round_agent(number).to_owned(), "round"),
        };
        // A subagent runs on its session's subscription: one that cannot run Fable now gets the
        // Opus reviewer, which it can, rather than a reviewer refused at its first request
        // (REV-007; moving the session first is its own slice).
        if agent == REVIEWER_AGENT && !self.run_can_take_fable(ctx).await {
            agent = LATE_REVIEWER_AGENT.to_owned();
            reason = "no_fable";
        }
        let model = reviewer_model(&agent).unwrap_or_default().to_owned();
        let head = match ctx.task.worktree_path.as_deref() {
            Some(worktree) => self
                .deps
                .git
                .log(Path::new(worktree), "HEAD", 1)
                .await
                .ok()
                .and_then(|commits| commits.into_iter().next())
                .map(|commit| commit.sha),
            None => None,
        };
        store
            .insert_review_round(&ReviewRoundRecord {
                id: self.deps.crypto.new_id(),
                task_id: ctx.task.id.clone(),
                round: number,
                head,
                agent: agent.clone(),
                model: model.clone(),
                reason: reason.to_owned(),
                subagent_id: None,
                model_answered: None,
                started_at: self.now(),
                ended_at: None,
                verdict: None,
                cut_off: false,
                pr_number: None,
                merged_at: None,
            })
            .await?;
        let mut said = format!(
            "Yulius: this is review round {number} of this pull request; the reviewer runs on {} \
             ({agent}).",
            claude_model_name(&model)
        );
        if reason == "no_fable" {
            said.push_str(
                " This subscription cannot run Fable 5.2 now, so the round runs on Opus 5.5: \
                 say so in the review's summary.",
            );
        }
        if agent == asked {
            return Ok(HookOutput::additional_context(events::PRE_TOOL_USE, said));
        }
        let mut updated = tool_input.clone();
        updated.insert("subagent_type".into(), Value::String(agent));
        Ok(HookOutput::run_tool_with(Value::Object(updated), said))
    }

    /// Whether the subscription the task's run is on can run Fable now: below the pool's
    /// threshold in every window that binds Fable. Unknown (no subscription, a failed read)
    /// counts as yes: the reviewer then starts as asked.
    async fn run_can_take_fable(&self, ctx: &AgentContext) -> bool {
        let Some(account) = ctx.attempt.account_id.as_ref() else {
            return true;
        };
        let Ok(views) = self.account_views().await else {
            return true;
        };
        let fable = ModelFamily::of(Some(yulius_domain::harness::REVIEW_CLAUDE_MODEL));
        views
            .iter()
            .find(|view| &view.id == account)
            .is_none_or(|view| {
                yulius_domain::rotation::assess(
                    view,
                    self.now(),
                    yulius_domain::rotation::THRESHOLD,
                    fable,
                )
                .is_some()
            })
    }

    /// What a subagent's transcript says of a review round's reviewer: the model that answered
    /// it, or that a usage limit cut it off. A subagent that is no round's changes nothing.
    pub(crate) async fn review_subagent_noted(
        &self,
        task: &yulius_domain::ids::TaskId,
        subagent: &str,
        model: Option<&str>,
        cut_off: bool,
    ) {
        if let Err(err) = self
            .deps
            .store
            .note_review_subagent(task, subagent, model, cut_off)
            .await
        {
            tracing::warn!(error = %err, task = %task, "review round not noted");
        }
    }

    /// `SubagentStart`: links a reviewer's subagent to the round that started it. A resumed
    /// subagent (the same id) changes nothing.
    pub(crate) async fn review_subagent_started(&self, ctx: &AgentContext, input: &HookInput) {
        let (Some(agent), Some(id)) = (input.agent_type.as_deref(), input.agent_id.as_deref())
        else {
            return;
        };
        if !REVIEWERS.contains(&agent) {
            return;
        }
        if let Err(err) = self
            .deps
            .store
            .link_review_subagent(&ctx.task.id, &[agent], id)
            .await
        {
            tracing::warn!(error = %err, task = %ctx.task.id, "review subagent not linked");
        }
    }

    /// `SubagentStop`: a reviewer's report ends its round, with the verdict its last line says.
    pub(crate) async fn review_subagent_stopped(&self, ctx: &AgentContext, input: &HookInput) {
        let Some(id) = input.agent_id.as_deref() else {
            return;
        };
        if !input
            .agent_type
            .as_deref()
            .is_some_and(|agent| REVIEWERS.contains(&agent))
        {
            return;
        }
        let verdict = input
            .last_assistant_message
            .as_deref()
            .and_then(verdict_of)
            .map(|verdict| verdict.as_str());
        if let Err(err) = self
            .deps
            .store
            .end_review_round(&ctx.task.id, id, self.now(), verdict, None)
            .await
        {
            tracing::warn!(error = %err, task = %ctx.task.id, "review round not ended");
        }
    }

    /// `GET /api/analytics/reviews`: the review's numbers over the pull requests of the
    /// sandboxes the caller sees (or `sandbox`) merged in `[from, to]` (days, `YYYY-MM-DD`),
    /// SPEC-493 REV-006.
    ///
    /// # Errors
    /// `Invalid` dates, `NotFound` for a sandbox the caller cannot see.
    pub async fn analytics_reviews(
        &self,
        ctx: &crate::auth::AuthContext,
        sandbox: Option<&str>,
        from: Option<&str>,
        to: Option<&str>,
    ) -> AppResult<yulius_contracts::api::analytics::ReviewAnalytics> {
        use yulius_contracts::api::analytics::{ModelCount, ReviewAnalytics};
        let (sandboxes, from_at, to_at, range) =
            self.analytics_window(ctx, sandbox, from, to).await?;
        let rounds = self
            .deps
            .store
            .merged_review_rounds(&sandboxes, from_at, to_at)
            .await?;
        let facts: Vec<yulius_domain::review::RoundFact> = rounds
            .into_iter()
            .map(|r| yulius_domain::review::RoundFact {
                // A pull request is its merge: a playground's merges have no number.
                pull: (
                    r.task_id.to_string(),
                    r.merged_at.map_or(0, |at| u64::try_from(at.0).unwrap_or(0)),
                ),
                round: r.round,
                model: r.model_answered.unwrap_or(r.model),
                verdict: r
                    .verdict
                    .as_deref()
                    .and_then(yulius_domain::review::Verdict::parse),
                reported: r.ended_at.is_some(),
                no_fable: r.reason == "no_fable",
            })
            .collect();
        let numbers = yulius_domain::review::review_numbers(&facts);
        Ok(ReviewAnalytics {
            range,
            pull_requests: numbers.pull_requests,
            first_rounds_requesting_changes: numbers.first_rounds_requesting_changes,
            first_rounds_without_report: numbers.first_rounds_without_report,
            median_rounds: numbers.median_rounds,
            rounds_by_model: numbers
                .rounds_by_model
                .into_iter()
                .map(|(model, rounds)| ModelCount { model, rounds })
                .collect(),
            no_fable_rounds: numbers.no_fable_rounds,
        })
    }

    /// The rounds of `task`'s open pull request, as the delivery view shows them. A failed read
    /// shows none.
    pub(crate) async fn review_round_views(
        &self,
        task: &yulius_domain::ids::TaskId,
    ) -> Vec<yulius_contracts::api::git::ReviewRoundView> {
        self.deps
            .store
            .review_rounds(task, true)
            .await
            .unwrap_or_default()
            .into_iter()
            .map(round_view)
            .collect()
    }

    /// Writes the merge record of `task`'s pull request `pr`, merged at `at` (the time the
    /// store closed its rounds) with `merged` (its head and the integration branch before it)
    /// when known: the models of the runs that worked on it (those still going when the
    /// previous pull request merged, and every later one) and each of its review rounds with its
    /// model, verdict and head (SPEC-493 REV-005). A merge whose head no recorded round approved
    /// (that head, or one followed only by clean merges of the integration branch at
    /// [`MergedHead::base`], the reviewed-head rule) says so; without a known head, any approving
    /// round counts.
    pub async fn record_review_models(
        &self,
        task: &yulius_domain::ids::TaskId,
        pr: Option<u64>,
        at: Timestamp,
        merged: Option<&MergedHead>,
    ) {
        let store = &self.deps.store;
        let Ok(all) = store.review_rounds(task, false).await else {
            return;
        };
        let Ok(Some(record)) = store.task(task).await else {
            return;
        };
        let target = match store.sandbox(&record.sandbox_id).await {
            Ok(Some(sandbox)) => format!("{}#{}", sandbox.slug, record.number),
            _ => task.to_string(),
        };
        let (this, before): (Vec<_>, Vec<_>) =
            all.into_iter().partition(|r| r.merged_at == Some(at));
        // The task's previous pull request: the latest merge before this one, by its rounds or
        // by its own merge record (a merge with no recorded round writes one too).
        let recorded = store
            .last_audit(&target, "delivery.review.models", Timestamp(0))
            .await
            .ok()
            .flatten()
            .map(|entry| entry.at)
            .filter(|earlier| *earlier <= at);
        let since = before
            .iter()
            .filter_map(|r| r.merged_at)
            .filter(|merged| *merged < at)
            .chain(recorded)
            .max();
        let own = record
            .model
            .clone()
            .unwrap_or_else(|| yulius_domain::harness::DEFAULT_CLAUDE_MODEL.to_owned());
        let mut written: Vec<String> = Vec::new();
        for attempt in store.attempts(task).await.unwrap_or_default() {
            // A run that ended before the previous merge wrote that pull request, not this one.
            if since.is_some_and(|since| attempt.ended_at.is_some_and(|ended| ended < since)) {
                continue;
            }
            let model = attempt.model.unwrap_or_else(|| own.clone());
            if !written.contains(&model) {
                written.push(model);
            }
        }
        let rounds: Vec<String> = this
            .iter()
            .map(|r| {
                format!(
                    "{} {} {}{}{}",
                    r.round,
                    r.model_answered.as_deref().unwrap_or(&r.model),
                    r.verdict.as_deref().unwrap_or(if r.ended_at.is_some() {
                        "no verdict"
                    } else {
                        "no report"
                    }),
                    r.head
                        .as_deref()
                        .map(|h| format!(" at {}", h.chars().take(9).collect::<String>()))
                        .unwrap_or_default(),
                    if r.reason == "no_fable" {
                        " (no Fable)"
                    } else {
                        ""
                    }
                )
            })
            .collect();
        let mut approved = false;
        for round in this
            .iter()
            .filter(|r| r.verdict.as_deref() == Some("approve"))
        {
            approved = match (merged, round.head.as_deref()) {
                (None, _) => true,
                (Some(merged), Some(head)) if same_commit(head, &merged.head) => true,
                // Followed only by clean merges of the integration branch at the merge's base;
                // with that base unknown, only the approved head itself counts.
                (Some(merged), Some(head)) => {
                    match (merged.base.as_deref(), record.worktree_path.as_deref()) {
                        (Some(base), Some(worktree)) => {
                            self.review_covers_against(
                                &record,
                                Path::new(worktree),
                                head,
                                &merged.head,
                                Some(base),
                            )
                            .await
                        }
                        _ => false,
                    }
                }
                (Some(_), None) => false,
            };
            if approved {
                break;
            }
        }
        let detail = format!(
            "{}: written by {}; review rounds: {}{}",
            pr.map_or_else(|| "the merge".to_owned(), |n| format!("pull request #{n}")),
            if written.is_empty() {
                "unknown".to_owned()
            } else {
                written.join(", ")
            },
            if rounds.is_empty() {
                "none recorded".to_owned()
            } else {
                rounds.join("; ")
            },
            if approved {
                ""
            } else {
                "; no recorded review round approved its head"
            }
        );
        if let Err(err) = store
            .audit(
                self.now(),
                "yulius",
                "delivery.review.models",
                Some(&target),
                Some(&detail),
            )
            .await
        {
            tracing::warn!(error = %err, task = %task, "merge record of the review not written");
        }
    }
}

/// Whether `a` and `b` name the same commit: the shorter (at least 7 hex digits) is a prefix of
/// the longer, as `delivery_merge` reads a reviewed head.
fn same_commit(a: &str, b: &str) -> bool {
    let (short, long) = if a.len() <= b.len() { (a, b) } else { (b, a) };
    crate::agent_delivery::same_commit(short, long)
}

/// The head a merge merged, and when known a commit of the integration branch that holds none
/// of the pull request's own commits: what a review is judged against (SPEC-493 REV-005).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MergedHead {
    /// The merged head.
    pub head: String,
    /// `<merge>^1` for a GitHub merge: the branch before a squash or merge commit, the previous
    /// (rewritten) copy for a rebase merge. A playground's base after its own squash.
    pub base: Option<String>,
}

/// A round as the delivery view shows it.
fn round_view(r: ReviewRoundRecord) -> yulius_contracts::api::git::ReviewRoundView {
    yulius_contracts::api::git::ReviewRoundView {
        round: r.round,
        agent: r.agent,
        model: r.model,
        model_answered: r.model_answered,
        reason: r.reason,
        head: r.head,
        verdict: r.verdict,
        reported: r.ended_at.is_some(),
        cut_off: r.cut_off,
    }
}
