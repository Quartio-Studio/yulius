//! What `task_get` and `tasks_list` say of a task beyond its brief, and what a task's agent reads
//! of its neighbours (SPEC-625, ADR-0460).
//!
//! Contract:
//! - `task_get` adds the task's owner, delivery (stage, PR, armed merge), the claims it holds
//!   while live or waiting, what it waits for in Yulius's own sentences and its agent's last
//!   words from its transcript; a person's own callers also get its screen's last lines, which a
//!   task's agent never does: a prompt Yulius held and a person's draft show there (PSV-001);
//!   the read changes nothing of the task;
//! - a task's agent reads only while its owner may view the project (PSV-003);
//! - a task that reaches its owner's personal secrets or MCP servers shows a task that does not
//!   hold the same person's only its shape: every free text is withheld (PSV-004).

use serde_json::{Value, json};
use yulius_domain::permission::Action;
use yulius_domain::task::TaskStatus;

use crate::context::App;
use crate::coordination::ProjectCaller;
use crate::error::{AppError, AppResult};
use crate::model::TaskRecord;
use crate::vault::Holder;

/// Lines of a task's screen `task_get` answers.
const SCREEN_LINES: usize = 20;

/// A brief's fields that hold words an agent or a person wrote (PSV-004).
const FREE_TEXT: [&str; 10] = [
    "prompt",
    "steps_list",
    "wait_history",
    "needs_you",
    "needs_you_notice",
    "attention",
    "held_prompt",
    "wait_reason",
    "screen",
    "last_words",
];

impl App {
    /// Refuses a task's agent whose owner may not view its project now: disabled, or without
    /// read access (PSV-003, the rule of SPEC-490). The access is the settled one (a cached
    /// decision within the staleness bound, else GitHub is asked), so reading the task list
    /// spends no GitHub request per call; an access that cannot be decided refuses. A person's
    /// own callers were checked by their bearer.
    ///
    /// # Errors
    /// `Forbidden` for a disabled owner or one without read access; `NotFound` for a removed
    /// project; the errors of [`App::settled_access_level`].
    pub(crate) async fn require_reader(&self, caller: &ProjectCaller) -> AppResult<()> {
        if caller.task.is_none() {
            return Ok(());
        }
        let store = &self.deps.store;
        let user = store
            .user(&caller.user)
            .await?
            .filter(|user| user.disabled_at.is_none())
            .ok_or_else(|| {
                AppError::Forbidden("the person this task acts for is disabled".into())
            })?;
        let sandbox = store
            .sandbox(&caller.sandbox)
            .await?
            .filter(|sandbox| sandbox.removed_at.is_none())
            .ok_or(AppError::NotFound)?;
        let login = user.label();
        let level = self
            .settled_access_level(&user, &sandbox)
            .await
            .map_err(|err| {
                tracing::info!(error = %err, "a task's owner's access could not be settled");
                AppError::Forbidden(format!(
                    "Yulius cannot confirm now that @{login} may view this project (their GitHub \
                     sign-in or GitHub itself did not answer), so their task reads no task \
                     status or list until it can"
                ))
            })?;
        if !level.allows(Action::View) {
            return Err(AppError::Forbidden(format!(
                "@{login} can no longer view this project, so their task reads no task status or \
                 list of it"
            )));
        }
        Ok(())
    }

    /// Adds to `task`'s brief its `owner`, `delivery`, `claims`, `waits_for` and, unless its
    /// free text is `withheld`, `last_words`; for a `person`'s own caller also `screen`
    /// (PSV-001).
    ///
    /// # Errors
    /// Store failures.
    pub(crate) async fn add_peer_status(
        &self,
        task: &TaskRecord,
        brief: &mut Value,
        person: bool,
        withheld: bool,
    ) -> AppResult<()> {
        let store = &self.deps.store;
        let delivery = store.delivery(&task.id).await?;
        let pr = delivery
            .as_ref()
            .and_then(|d| d.pr_number)
            .or_else(|| brief.pointer("/parked_pr/number").and_then(Value::as_u64));
        let merge_armed = task.status.is_live() && self.merge_is_armed(&task.id);
        let claims: Vec<Value> = store
            .active_claims(&task.sandbox_id)
            .await?
            .into_iter()
            // Nothing releases an ended task's claims: it holds them only while live or waiting to
            // start, as `task_create`'s planned overlaps count them.
            .filter(|claim| claim.task_id == task.id && !task.status.is_resting())
            .map(|claim| json!({"pattern": claim.pattern, "note": claim.note}))
            .collect();
        let requests = self.browser_requests(&task.sandbox_id).await;
        let facts = self.task_attention_facts(task, &requests).await?;
        let yulius_wait = facts.yulius_wait;
        brief["owner"] = json!(self.login_of(&task.owner).await);
        brief["delivery"] = json!({
            "stage": delivery.map_or_else(|| "none".to_owned(), |d| d.stage.encode()),
            "pr": pr,
            "merge_armed": merge_armed,
        });
        brief["claims"] = json!(claims);
        let mut waits = waits_for(task.status, brief, merge_armed, yulius_wait);
        if facts.candidate_pending {
            // SPEC-621 FIN-004: why an achieved goal is not Done yet.
            waits.push(
                "Yulius's re-check of its failed integration candidate, which it tells the agent"
                    .to_owned(),
            );
        }
        brief["waits_for"] = json!(waits);
        if withheld {
            return Ok(());
        }
        brief["last_words"] = json!(self.last_agent_words(task).await);
        if person && let Some(sandbox) = store.sandbox(&task.sandbox_id).await? {
            let screen = self.task_screen_of(&sandbox, task, SCREEN_LINES).await?;
            brief["screen"] = json!({"from": screen["screen_from"], "text": screen["screen"]});
        }
        Ok(())
    }

    /// Whether `peer`'s free text is withheld from `caller` (PSV-004): the caller is a task that
    /// does not hold the same person's secrets as `peer`, and `peer` reaches its owner's personal
    /// secrets or MCP servers.
    ///
    /// # Errors
    /// Store failures.
    pub(crate) async fn withholds_from(
        &self,
        caller: &ProjectCaller,
        peer: &TaskRecord,
    ) -> AppResult<bool> {
        match &caller.task {
            Some(reader) => self.withholds_text(reader, peer).await,
            None => Ok(false),
        }
    }

    /// [`App::withholds_from`] for the task `reader` (also the briefing's recent changes).
    ///
    /// # Errors
    /// Store failures.
    pub(crate) async fn withholds_text(
        &self,
        reader: &TaskRecord,
        peer: &TaskRecord,
    ) -> AppResult<bool> {
        if reader.id == peer.id || Holder::of_task(reader) == Holder::of_task(peer) {
            return Ok(false);
        }
        self.reaches_owners_personal(peer).await
    }

    /// [`withhold`] on `brief` when `peer`'s free text is withheld from `caller`.
    ///
    /// # Errors
    /// Store failures.
    pub(crate) async fn withhold_from(
        &self,
        caller: &ProjectCaller,
        peer: &TaskRecord,
        brief: &mut Value,
    ) -> AppResult<()> {
        if self.withholds_from(caller, peer).await? {
            withhold(brief, peer.number, &self.login_of(&peer.owner).await);
        }
        Ok(())
    }

    /// Refuses `caller` the transcript of `peer` when its free text is withheld (PSV-004).
    ///
    /// # Errors
    /// `Forbidden`, saying why; store failures.
    pub(crate) async fn require_transcript_reader(
        &self,
        caller: &ProjectCaller,
        peer: &TaskRecord,
    ) -> AppResult<()> {
        if self.withholds_from(caller, peer).await? {
            return Err(AppError::Forbidden(format!(
                "task #{} reaches @{}'s personal secrets or MCP servers, which your task does not \
                 hold: its transcript stays with tasks that hold them",
                peer.number,
                self.login_of(&peer.owner).await
            )));
        }
        Ok(())
    }
}

/// Removes from a brief every free text (PSV-004) and says why in `withheld`: its shape stays.
pub(crate) fn withhold(brief: &mut Value, number: u32, owner: &str) {
    if let Some(fields) = brief.as_object_mut() {
        for key in FREE_TEXT {
            fields.remove(key);
        }
    }
    if let Some(steps) = brief.get_mut("steps").and_then(Value::as_object_mut) {
        steps.insert("current".into(), Value::Null);
    }
    if let Some(claims) = brief.get_mut("claims").and_then(Value::as_array_mut) {
        for claim in claims.iter_mut().filter_map(Value::as_object_mut) {
            claim.remove("note");
        }
    }
    brief["withheld"] = json!(format!(
        "Task #{number} reaches @{owner}'s personal secrets or MCP servers, which your task does \
         not hold: its request, plan, questions, claim notes, screen and transcript stay with \
         tasks that hold them."
    ));
}

/// What the task waits for, in Yulius's own sentences: no agent's or person's words.
fn waits_for(
    status: TaskStatus,
    brief: &Value,
    merge_armed: bool,
    yulius_wait: bool,
) -> Vec<String> {
    let text = |key: &str| brief.get(key).and_then(Value::as_str);
    let mut waits = Vec::new();
    if merge_armed {
        waits.push("its armed merge, which Yulius makes once its checks pass".to_owned());
    } else if yulius_wait {
        waits.push("Yulius's news of its pull request: checks, delivery or merge".to_owned());
    }
    if let Some(stage) = text("staging_wait") {
        waits.push(format!("staging: {stage}"));
    }
    if let Some(reason) = text("queue_reason") {
        waits.push(reason.to_owned());
    }
    if status == TaskStatus::Chained {
        if let Some(rule) = text("start_rule") {
            waits.push(format!("its start rule `{rule}`"));
        } else if let Some(after) = brief.get("runs_after").and_then(Value::as_u64) {
            waits.push(format!("task #{after}"));
        }
    }
    if status == TaskStatus::WaitingCapacity {
        waits.push("a subscription with capacity".to_owned());
    }
    if let Some(pause) = brief.get("pause").filter(|p| p.is_object()) {
        waits.push(match pause.get("by").and_then(Value::as_str) {
            Some("update") => "an update of Yulius to finish".to_owned(),
            _ => "a person to unpause it".to_owned(),
        });
    }
    if text("shows") == Some("needs_you") {
        waits.push("its person".to_owned());
    }
    waits
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_withheld_brief_keeps_its_shape_and_loses_every_free_text() {
        let mut brief = json!({
            "number": 7, "title": "Fix login", "status": "running", "shows": "needs_you",
            "branch": "feat/7", "prompt": "secret plan", "steps": {"done": 1, "total": 2,
            "current": "read the mail"}, "steps_list": [{"title": "read the mail"}],
            "wait_history": [{}], "needs_you": {"action": "a"}, "needs_you_notice": {},
            "attention": {}, "held_prompt": {}, "wait_reason": "w", "screen": {"text": "s"},
            "claims": [{"pattern": "docs/**", "note": "mail"}], "owner": "alice",
            "delivery": {"stage": "pr_open", "pr": 4}, "waits_for": ["its person"],
        });
        withhold(&mut brief, 7, "alice");
        for key in FREE_TEXT {
            assert!(brief.get(key).is_none(), "{key}: {brief}");
        }
        assert_eq!(
            brief["steps"],
            json!({"done": 1, "total": 2, "current": null})
        );
        assert_eq!(brief["claims"], json!([{"pattern": "docs/**"}]));
        for kept in [
            "title",
            "status",
            "shows",
            "branch",
            "owner",
            "delivery",
            "waits_for",
        ] {
            assert!(brief.get(kept).is_some(), "{kept} stays: {brief}");
        }
        assert!(
            brief["withheld"]
                .as_str()
                .is_some_and(|why| why.contains("@alice's personal secrets")),
            "{brief}"
        );
    }

    #[test]
    fn waits_are_yuliuss_own_sentences() {
        let brief = json!({"shows": "needs_you", "staging_wait": "waiting for staging",
            "queue_reason": "Waiting for a free session: 8 of 8 in use",
            "pause": {"by": "person"}, "start_rule": "#12 merged", "runs_after": 12});
        assert_eq!(
            waits_for(TaskStatus::Chained, &brief, true, true),
            [
                "its armed merge, which Yulius makes once its checks pass",
                "staging: waiting for staging",
                "Waiting for a free session: 8 of 8 in use",
                "its start rule `#12 merged`",
                "a person to unpause it",
                "its person",
            ]
        );
        assert_eq!(
            waits_for(TaskStatus::Running, &json!({"runs_after": 3}), false, true),
            ["Yulius's news of its pull request: checks, delivery or merge"]
        );
        assert!(waits_for(TaskStatus::Running, &json!({}), false, false).is_empty());
    }
}
