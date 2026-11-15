//! The independent code review's rounds as Yulius sees them (SPEC-493 REV-002 to REV-004).
//!
//! A task starts its review with the Agent tool and [`REVIEWER_AGENT`]. Its `PreToolUse` reaches
//! [`App::review_gate`], which numbers the round among those of the task's open pull request,
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

use crate::context::App;
use crate::coordination::AgentContext;
use crate::error::AppResult;
use crate::model::ReviewRoundRecord;

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
        let reported = rounds.iter().filter(|r| r.ended_at.is_some()).count();
        let number = u32::try_from(reported)
            .unwrap_or(u32::MAX)
            .saturating_add(1);
        let unfinished = rounds
            .iter()
            .rev()
            .find(|r| r.round == number && r.ended_at.is_none());
        let (agent, reason) = match unfinished {
            Some(round) => (round.agent.clone(), "kept"),
            None => (round_agent(number).to_owned(), "round"),
        };
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
        let said = format!(
            "Yulius: this is review round {number} of this pull request; the reviewer runs on {} \
             ({agent}).",
            claude_model_name(&model)
        );
        if agent == asked {
            return Ok(HookOutput::additional_context(events::PRE_TOOL_USE, said));
        }
        let mut updated = tool_input.clone();
        updated.insert("subagent_type".into(), Value::String(agent));
        Ok(HookOutput::run_tool_with(Value::Object(updated), said))
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
}
