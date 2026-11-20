//! Tasks hand work to another sandbox of the same instance (SPEC-512 part 5, #1106, ADR-0395,
//! amending ADR-0227 for task agents): behind an instance switch (off by default, ask or
//! automatic) and each sandbox's opt-out, a task's agent calls `sandbox_handoff`; the checks are
//! `sandbox_task_create`'s plus a chain's depth, cycles and a daily bound per owner. Ask stores a
//! request its owner answers in a browser; automatic creates the task as the owner at once. The
//! new task shows where it came from, the source task is told its outcome, and every step is
//! audited `task.handoff` on both sandboxes, never with the prompt.
//!
//! - HOF-001: [`App::set_handoff_setting`]; HOF-002: [`App::set_sandbox_handoffs`];
//! - HOF-003: [`App::tool_sandbox_handoff`]; HOF-004: [`App::answer_handoff`];
//! - HOF-005: [`App::task_handoffs`], [`App::tell_handoff_outcomes`]; HOF-006: `audit_handoff`.

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use yulius_contracts::api::handoffs::{
    HandoffFrom, HandoffSetting, HandoffView, SandboxHandoffs, TaskHandoffs,
};
use yulius_contracts::api::tasks::CreateTask;
use yulius_contracts::events::UiEvent;
use yulius_domain::delivery::DeliveryStage;
use yulius_domain::ids::SandboxId;
use yulius_domain::permission::Action;
use yulius_domain::task::{Activity, TaskStatus};

use crate::auth::{AuthContext, rfc3339};
use crate::context::App;
use crate::coordination::AgentContext;
use crate::error::{AppError, AppResult};
use crate::model::{SandboxRecord, TaskOrigin, TaskRecord, UserRecord};
use crate::ports::store_ext::HandoffRecord;
use crate::task_message::{Sender, Typed};

/// The profile key of a sandbox's opt-out (HOF-002): anything but `false` accepts.
const ACCEPT_KEY: &str = "accept_handoffs";
/// The deepest a chain may be set to go.
pub const MAX_DEPTH_LIMIT: u32 = 5;
/// The most handoffs a day an owner may be allowed.
pub const PER_DAY_LIMIT: u32 = 100;
const DAY_MS: i64 = 24 * 3_600_000;

/// What a handoff creates: `sandbox_handoff`'s arguments, stored and shown as they are.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HandoffArgs {
    /// The target sandbox, as the agent named it (id, slug or repository).
    pub sandbox: String,
    /// The new task's title.
    pub title: String,
    /// Its prompt.
    pub prompt: String,
    /// `task_create`'s options.
    #[serde(default)]
    pub harness: Option<String>,
    /// The model.
    #[serde(default)]
    pub model: Option<String>,
    /// The effort.
    #[serde(default)]
    pub effort: Option<String>,
    /// `goal` or `prompt`.
    #[serde(default)]
    pub mode: Option<String>,
}

/// The mode of a setting, read leniently: anything unknown is off.
fn mode_of(setting: &HandoffSetting) -> &'static str {
    match setting.mode.as_str() {
        "ask" => "ask",
        "automatic" => "automatic",
        _ => "off",
    }
}

/// Whether a sandbox accepts handoffs (HOF-002): on unless its profile says `false`.
#[must_use]
pub fn accepts_handoffs(sandbox: &SandboxRecord) -> bool {
    serde_json::from_str::<Value>(&sandbox.profile_json)
        .ok()
        .and_then(|p| p.get(ACCEPT_KEY).and_then(Value::as_bool))
        != Some(false)
}

/// Why a handoff is refused before anything is stored for its chain (HOF-003), if it is.
#[must_use]
pub fn chain_refusal(chain: &[String], depth: u32, target: &str, max_depth: u32) -> Option<String> {
    if depth > max_depth {
        return Some(format!(
            "a chain of handoffs goes at most {max_depth} deep; this one would be {depth}"
        ));
    }
    if chain.iter().any(|s| s == target) {
        return Some("that sandbox is already in this chain of handoffs (a cycle)".into());
    }
    None
}

impl App {
    /// The instance's handoff setting (HOF-001); off when never set.
    pub(crate) async fn handoffs_setting_now(&self) -> HandoffSetting {
        self.deps
            .store
            .handoff_setting()
            .await
            .ok()
            .flatten()
            .and_then(|doc| serde_json::from_str(&doc).ok())
            .unwrap_or_default()
    }

    /// Whether task agents are served `sandbox_handoff` now (HOF-003).
    pub(crate) async fn handoffs_on(&self) -> bool {
        mode_of(&self.handoffs_setting_now().await) != "off"
    }

    /// `GET /api/admin/handoffs` (HOF-001).
    ///
    /// # Errors
    /// `Forbidden` for anyone but an instance administrator.
    pub async fn handoff_setting(&self, ctx: &AuthContext) -> AppResult<HandoffSetting> {
        self.require_instance_admin(ctx)?;
        Ok(self.handoffs_setting_now().await)
    }

    /// `PUT /api/admin/handoffs` (HOF-001): saved and audited `instance.handoffs.set`.
    ///
    /// # Errors
    /// `Forbidden` for anyone but an instance administrator, `Invalid` for an unknown mode or a
    /// bound out of range.
    pub async fn set_handoff_setting(
        &self,
        ctx: &AuthContext,
        req: &HandoffSetting,
    ) -> AppResult<HandoffSetting> {
        self.require_instance_admin(ctx)?;
        if !matches!(req.mode.as_str(), "off" | "ask" | "automatic") {
            return Err(AppError::Invalid(
                "the mode is off, ask or automatic".into(),
            ));
        }
        if !(1..=MAX_DEPTH_LIMIT).contains(&req.max_depth) {
            return Err(AppError::Invalid(format!(
                "a chain goes 1 to {MAX_DEPTH_LIMIT} deep"
            )));
        }
        if !(1..=PER_DAY_LIMIT).contains(&req.per_day) {
            return Err(AppError::Invalid(format!(
                "an owner's handoffs a day are 1 to {PER_DAY_LIMIT}"
            )));
        }
        let doc = serde_json::to_string(req).map_err(AppError::internal)?;
        self.deps.store.set_handoff_setting(&doc).await?;
        let login = self.login_of(&ctx.user.id).await;
        self.deps
            .store
            .audit(
                self.now(),
                &login,
                "instance.handoffs.set",
                Some("handoffs"),
                Some(&doc),
            )
            .await?;
        Ok(req.clone())
    }

    /// `GET /api/sandboxes/{sid}/handoffs` (HOF-002), for those who may view it.
    ///
    /// # Errors
    /// Access errors.
    pub async fn sandbox_handoffs(
        &self,
        ctx: &AuthContext,
        sid: &str,
    ) -> AppResult<SandboxHandoffs> {
        let sid = SandboxId::parse(sid).map_err(|_| AppError::NotFound)?;
        let (sandbox, _) = self.require_for(ctx, &sid, Action::View).await?;
        Ok(SandboxHandoffs {
            accept: accepts_handoffs(&sandbox),
        })
    }

    /// `PUT /api/sandboxes/{sid}/handoffs` (HOF-002): its writers open or close it to other
    /// sandboxes' handoffs; audited `sandbox.handoffs`.
    ///
    /// # Errors
    /// Access errors: only who may work in it.
    pub async fn set_sandbox_handoffs(
        &self,
        ctx: &AuthContext,
        sid: &str,
        req: &SandboxHandoffs,
    ) -> AppResult<SandboxHandoffs> {
        let sid = SandboxId::parse(sid).map_err(|_| AppError::NotFound)?;
        self.require_for(ctx, &sid, Action::Work).await?;
        let writing = self.profile_writes.lock().await;
        let sandbox = self
            .deps
            .store
            .sandbox(&sid)
            .await?
            .ok_or(AppError::NotFound)?;
        let mut profile = match serde_json::from_str::<Value>(&sandbox.profile_json) {
            Ok(Value::Object(map)) => map,
            _ => serde_json::Map::new(),
        };
        if req.accept {
            profile.remove(ACCEPT_KEY);
        } else {
            profile.insert(ACCEPT_KEY.to_owned(), Value::Bool(false));
        }
        let json = serde_json::to_string(&profile).map_err(AppError::internal)?;
        self.deps.store.set_profile(&sid, &json).await?;
        drop(writing);
        let login = self.login_of(&ctx.user.id).await;
        let detail = if req.accept { "accept" } else { "refuse" };
        self.deps
            .store
            .audit(
                self.now(),
                &login,
                "sandbox.handoffs",
                Some(sid.as_str()),
                Some(detail),
            )
            .await?;
        Ok(*req)
    }

    /// The sandbox `name` names (its id, slug, full repository or a short name only one has).
    async fn sandbox_named(&self, name: &str) -> AppResult<Option<SandboxRecord>> {
        let wanted = name.trim().to_lowercase();
        let mut short = Vec::new();
        for sandbox in self.deps.store.sandboxes().await? {
            if sandbox.removed_at.is_some() {
                continue;
            }
            let full = sandbox.repo_name.to_lowercase();
            if sandbox.id.as_str() == name.trim()
                || sandbox.slug.to_lowercase() == wanted
                || full == wanted
            {
                return Ok(Some(sandbox));
            }
            if full.rsplit('/').next() == Some(wanted.as_str()) {
                short.push(sandbox);
            }
        }
        Ok(if short.len() == 1 { short.pop() } else { None })
    }

    /// `sandbox_handoff` (HOF-003, HOF-004): a task's agent hands work to another sandbox.
    ///
    /// # Errors
    /// The refusals of HOF-003 (each audited), access and creation errors.
    pub async fn tool_sandbox_handoff(
        self: &Arc<Self>,
        ctx: &AgentContext,
        args: &Value,
    ) -> AppResult<Value> {
        let setting = self.handoffs_setting_now().await;
        let mode = mode_of(&setting);
        if mode == "off" {
            return Err(AppError::Invalid("unknown tool: sandbox_handoff".into()));
        }
        let source = &ctx.task;
        let args: HandoffArgs = serde_json::from_value(args.clone()).map_err(|err| {
            AppError::Invalid(format!(
                "sandbox_handoff takes sandbox, title, prompt: {err}"
            ))
        })?;
        crate::cross_sandbox::check_visible(
            &serde_json::to_value(&args).map_err(AppError::internal)?,
        )?;
        if args.title.trim().is_empty() || args.prompt.trim().is_empty() {
            return Err(AppError::Invalid(
                "a handoff needs a title and a prompt".into(),
            ));
        }
        let owner = self
            .deps
            .store
            .user(&source.owner)
            .await?
            .filter(|u| u.disabled_at.is_none())
            .ok_or_else(|| AppError::Forbidden("this task's owner is disabled".into()))?;
        // An automation's task, CI and CD hold nobody: they hand nothing over (HOF-003).
        if crate::vault::Holder::of_task(source) == crate::vault::Holder::Nobody {
            return self
                .refuse_handoff(
                    source,
                    None,
                    &args,
                    "an automation's task hands no work to another sandbox",
                )
                .await;
        }
        let target = match self.sandbox_named(&args.sandbox).await? {
            Some(t) if self.require(&owner, &t.id, Action::Work).await.is_ok() => t,
            // A sandbox the owner cannot work in is not found, as `sandbox_task_create` says.
            _ => {
                return self
                    .refuse_handoff(
                        source,
                        None,
                        &args,
                        &format!("no sandbox {} found", args.sandbox),
                    )
                    .await;
            }
        };
        if target.id == source.sandbox_id {
            return self
                .refuse_handoff(
                    source,
                    Some(&target),
                    &args,
                    "a handoff goes to another sandbox: use task_create here",
                )
                .await;
        }
        if !accepts_handoffs(&target) {
            return self
                .refuse_handoff(
                    source,
                    Some(&target),
                    &args,
                    "that sandbox does not accept handoffs",
                )
                .await;
        }
        let (chain, depth) = match self.deps.store.handoff_of_target(&source.id).await? {
            Some(parent) => {
                let mut chain = parent.chain;
                chain.push(source.sandbox_id.as_str().to_owned());
                (chain, parent.depth + 1)
            }
            None => (vec![source.sandbox_id.as_str().to_owned()], 1),
        };
        if let Some(reason) = chain_refusal(&chain, depth, target.id.as_str(), setting.max_depth) {
            return self
                .refuse_handoff(source, Some(&target), &args, &reason)
                .await;
        }
        let today = self
            .deps
            .store
            .handoffs_since(&owner.id, self.now().plus_millis(-DAY_MS))
            .await?;
        if today >= setting.per_day {
            let reason = format!(
                "its owner's tasks handed off {today} times in 24 hours, the most this instance allows"
            );
            return self
                .refuse_handoff(source, Some(&target), &args, &reason)
                .await;
        }
        let record = HandoffRecord {
            id: self.new_handoff_id(),
            source_task: source.id.clone(),
            source_sandbox: source.sandbox_id.clone(),
            target_sandbox: target.id.clone(),
            owner: owner.id.clone(),
            args: serde_json::to_string(&args).map_err(AppError::internal)?,
            chain,
            depth,
            state: "requested".into(),
            reason: None,
            target_task: None,
            created_at: self.now(),
        };
        self.deps.store.insert_handoff(&record).await?;
        self.audit_handoff(&record, &target, "requested", None)
            .await;
        self.events.publish(
            Some(source.sandbox_id.clone()),
            false,
            UiEvent::HandoffsUpdated {
                task_id: source.id.as_str().to_owned(),
            },
        );
        if mode == "ask" {
            return Ok(json!({
                "state": "requested",
                "id": record.id,
                "sandbox": target.repo_name,
                "note": "Stored for this task's owner, who approves or declines it on this task's \
                         page. Go on with your work, or end with needs_you if you must wait.",
            }));
        }
        let task = self
            .create_handoff_task(&record, &owner, &target, &args)
            .await?;
        Ok(json!({
            "state": "created",
            "id": record.id,
            "sandbox": target.repo_name,
            "task_number": task.number,
            "link": self.task_link(&target, task.number),
            "note": "Created as this task's owner. You will be told its outcome here when it ends.",
        }))
    }

    /// Creates the handoff's task as its owner, with exactly its arguments (HOF-004, HOF-005),
    /// under the tools' hourly bound; records and audits it.
    async fn create_handoff_task(
        self: &Arc<Self>,
        record: &HandoffRecord,
        owner: &UserRecord,
        target: &SandboxRecord,
        args: &HandoffArgs,
    ) -> AppResult<TaskRecord> {
        let slot = self.reserve_tool_task(&target.id)?;
        let req = CreateTask {
            prompt: args.prompt.clone(),
            harness: args.harness.clone().unwrap_or_default(),
            workflow_id: None,
            model: args.model.clone(),
            effort: args.effort.clone(),
            title: Some(args.title.clone()),
            mode: args.mode.clone(),
            after_task_id: None,
            start_rule: None,
            supertask: false,
            prepared: false,
            start_at: None,
        };
        let task = match self
            .create_task_as(owner, &target.id, &req, TaskOrigin::Handoff)
            .await
        {
            Ok(task) => task,
            Err(err) => {
                self.release_tool_task(&target.id, slot);
                let reason = err.public_message();
                self.deps
                    .store
                    .decide_handoff(
                        &record.id,
                        "requested",
                        "refused",
                        Some(&reason),
                        None,
                        self.now(),
                    )
                    .await?;
                self.audit_handoff(record, target, "refused", Some(&reason))
                    .await;
                return Err(err);
            }
        };
        self.deps
            .store
            .decide_handoff(
                &record.id,
                "requested",
                "created",
                None,
                Some(&task.id),
                self.now(),
            )
            .await?;
        self.audit_handoff(
            record,
            target,
            "created",
            Some(&format!("#{}", task.number)),
        )
        .await;
        self.events.publish(
            Some(record.source_sandbox.clone()),
            false,
            UiEvent::HandoffsUpdated {
                task_id: record.source_task.as_str().to_owned(),
            },
        );
        Ok(task)
    }

    /// Records, audits and returns a refusal with its reason (HOF-003, HOF-006).
    async fn refuse_handoff(
        &self,
        source: &TaskRecord,
        target: Option<&SandboxRecord>,
        args: &HandoffArgs,
        reason: &str,
    ) -> AppResult<Value> {
        let record = HandoffRecord {
            id: self.new_handoff_id(),
            source_task: source.id.clone(),
            source_sandbox: source.sandbox_id.clone(),
            target_sandbox: target.map_or_else(|| source.sandbox_id.clone(), |t| t.id.clone()),
            owner: source.owner.clone(),
            args: serde_json::to_string(args).map_err(AppError::internal)?,
            chain: vec![source.sandbox_id.as_str().to_owned()],
            depth: 0,
            state: "refused".into(),
            reason: Some(reason.to_owned()),
            target_task: None,
            created_at: self.now(),
        };
        self.deps.store.insert_handoff(&record).await?;
        let login = format!("task:{}", source.number);
        let detail = format!("refused: {reason}");
        let _ = self
            .deps
            .store
            .audit(
                self.now(),
                &login,
                "task.handoff",
                Some(source.sandbox_id.as_str()),
                Some(&detail),
            )
            .await;
        if let Some(target) = target
            && target.id != source.sandbox_id
        {
            let _ = self
                .deps
                .store
                .audit(
                    self.now(),
                    &login,
                    "task.handoff",
                    Some(target.id.as_str()),
                    Some(&detail),
                )
                .await;
        }
        Err(AppError::Forbidden(format!("handoff refused: {reason}")))
    }

    /// `task.handoff` on both sandboxes (HOF-006), never with the prompt.
    async fn audit_handoff(
        &self,
        record: &HandoffRecord,
        target: &SandboxRecord,
        what: &str,
        extra: Option<&str>,
    ) {
        let source = self
            .deps
            .store
            .task(&record.source_task)
            .await
            .ok()
            .flatten()
            .map_or_else(|| "a task".to_owned(), |t| format!("#{}", t.number));
        let detail = format!(
            "{what}: {} {source} to {}{}",
            record.source_sandbox,
            target.repo_name,
            extra.map(|e| format!(" {e}")).unwrap_or_default()
        );
        for sandbox in [&record.source_sandbox, &record.target_sandbox] {
            if let Err(err) = self
                .deps
                .store
                .audit(
                    self.now(),
                    "yulius",
                    "task.handoff",
                    Some(sandbox.as_str()),
                    Some(&detail),
                )
                .await
            {
                tracing::warn!(error = %err, "a handoff could not be audited");
            }
        }
    }

    fn new_handoff_id(&self) -> String {
        let hex: String = self
            .deps
            .crypto
            .random_bytes()
            .iter()
            .take(8)
            .map(|b| format!("{b:02x}"))
            .collect();
        format!("ho_{hex}")
    }

    fn task_link(&self, sandbox: &SandboxRecord, number: u32) -> String {
        format!(
            "{}/s/{}/t/{number}",
            self.config.public_url.trim_end_matches('/'),
            sandbox.slug
        )
    }

    /// `GET /api/tasks/{tid}/handoffs` (HOF-004, HOF-005): where the task came from and its own
    /// handoffs, for those who may view it.
    ///
    /// # Errors
    /// Access errors.
    pub async fn task_handoffs(&self, ctx: &AuthContext, tid: &str) -> AppResult<TaskHandoffs> {
        let task = self.load_task(ctx, tid, Action::View).await?;
        let store = &self.deps.store;
        let from = match store.handoff_of_target(&task.id).await? {
            Some(h) => match (
                store.task(&h.source_task).await?,
                store.sandbox(&h.source_sandbox).await?,
            ) {
                (Some(source), Some(sandbox)) if self.can_view(&ctx.user, &sandbox.id).await => {
                    Some(HandoffFrom {
                        sandbox: sandbox.repo_name.clone(),
                        number: source.number,
                        link: self.task_link(&sandbox, source.number),
                    })
                }
                _ => None,
            },
            None => None,
        };
        let mut handoffs = Vec::new();
        for h in store.handoffs_of_source(&task.id).await? {
            let Ok(args) = serde_json::from_str::<HandoffArgs>(&h.args) else {
                continue;
            };
            let target = store.sandbox(&h.target_sandbox).await?;
            let made = match (&h.target_task, &target) {
                (Some(id), Some(sandbox)) => store
                    .task(id)
                    .await?
                    .map(|t| (t.number, self.task_link(sandbox, t.number))),
                _ => None,
            };
            handoffs.push(HandoffView {
                id: h.id,
                target_sandbox_id: h.target_sandbox.as_str().to_owned(),
                target_sandbox: target.map_or_else(|| args.sandbox.clone(), |t| t.repo_name),
                title: args.title,
                prompt: args.prompt,
                harness: args.harness,
                model: args.model,
                effort: args.effort,
                mode: args.mode,
                state: h.state,
                reason: h.reason,
                target_number: made.as_ref().map(|m| m.0),
                target_link: made.map(|m| m.1),
                created_at: rfc3339(h.created_at),
            });
        }
        Ok(TaskHandoffs { from, handoffs })
    }

    /// `POST /api/tasks/{tid}/handoffs/{hid}` (HOF-004): the task's owner approves or declines a
    /// stored request, in a browser session only. Approval checks the target again and creates
    /// the task with exactly the stored arguments.
    ///
    /// # Errors
    /// `Forbidden` for an API token or anyone but the owner, `Conflict` for a request already
    /// answered or a target that no longer takes it.
    pub async fn answer_handoff(
        self: &Arc<Self>,
        ctx: &AuthContext,
        tid: &str,
        hid: &str,
        approve: bool,
    ) -> AppResult<TaskHandoffs> {
        if !ctx.is_browser() {
            return Err(AppError::Forbidden(
                "a handoff is answered in Yulius in a browser, not with an API token".into(),
            ));
        }
        let task = self.load_task(ctx, tid, Action::View).await?;
        if task.owner != ctx.user.id {
            return Err(AppError::Forbidden(
                "only the task's owner answers its handoffs".into(),
            ));
        }
        let record = self
            .deps
            .store
            .handoff(hid)
            .await?
            .filter(|h| h.source_task == task.id)
            .ok_or(AppError::NotFound)?;
        if record.state != "requested" {
            return Err(AppError::Conflict(format!(
                "this handoff is {} already",
                record.state
            )));
        }
        let target = self
            .deps
            .store
            .sandbox(&record.target_sandbox)
            .await?
            .ok_or(AppError::NotFound)?;
        if !approve {
            if !self
                .deps
                .store
                .decide_handoff(
                    &record.id,
                    "requested",
                    "declined",
                    Some("declined by its owner"),
                    None,
                    self.now(),
                )
                .await?
            {
                return Err(AppError::Conflict(
                    "this handoff was answered meanwhile".into(),
                ));
            }
            self.audit_handoff(&record, &target, "declined", None).await;
        } else {
            let args: HandoffArgs =
                serde_json::from_str(&record.args).map_err(AppError::internal)?;
            if self.handoffs_setting_now().await.mode == "off" {
                return Err(AppError::Conflict(
                    "handoffs are off on this instance now".into(),
                ));
            }
            if !accepts_handoffs(&target)
                || self
                    .require(&ctx.user, &target.id, Action::Work)
                    .await
                    .is_err()
            {
                return Err(AppError::Conflict(format!(
                    "{} does not take this handoff now",
                    target.repo_name
                )));
            }
            self.audit_handoff(&record, &target, "approved", None).await;
            self.create_handoff_task(&record, &ctx.user, &target, &args)
                .await?;
        }
        self.events.publish(
            Some(task.sandbox_id.clone()),
            false,
            UiEvent::HandoffsUpdated {
                task_id: task.id.as_str().to_owned(),
            },
        );
        self.task_handoffs(ctx, tid).await
    }

    /// HOF-005, from the background tick: tells each created handoff's source task its target's
    /// outcome once the target ended, is Done or merged; a source that cannot take it now is
    /// tried at the next tick, one that ended is not told.
    pub async fn tell_handoff_outcomes(self: &Arc<Self>) -> usize {
        let Ok(untold) = self.deps.store.untold_handoffs().await else {
            return 0;
        };
        let mut told = 0;
        for h in untold {
            let Some(target_id) = &h.target_task else {
                continue;
            };
            let store = &self.deps.store;
            let (Ok(Some(target)), Ok(Some(source))) = (
                store.task(target_id).await,
                store.task(&h.source_task).await,
            ) else {
                continue;
            };
            let delivery = store.delivery(target_id).await.ok().flatten();
            let merged = delivery.as_ref().is_some_and(|d| {
                matches!(
                    d.stage,
                    DeliveryStage::MergedIntegration
                        | DeliveryStage::Promoted
                        | DeliveryStage::ProductionDone
                )
            });
            let outcome = if merged {
                "merged".to_owned()
            } else if target.status.is_resting() {
                format!("ended ({})", target.status.as_str())
            } else if target.activity == Some(Activity::Done) {
                "Done".to_owned()
            } else {
                continue;
            };
            if source.status.is_resting() {
                let _ = store.mark_handoff_told(&h.id).await;
                continue;
            }
            let Ok(Some(sandbox)) = store.sandbox(&h.target_sandbox).await else {
                continue;
            };
            let pr = delivery.and_then(|d| d.pr_number).map_or_else(
                || "none".to_owned(),
                |n| format!("https://github.com/{}/pull/{n}", sandbox.repo_name),
            );
            let text = format!(
                "Yulius: the task you handed off, {} #{} \"{}\", is {outcome}. Pull request: {pr}. \
                 Page: {}",
                sandbox.repo_name,
                target.number,
                target.title,
                self.task_link(&sandbox, target.number)
            );
            let text = self.redact_for_sandbox(&source.sandbox_id, &text).await;
            let delivered = match source.status {
                TaskStatus::Parked => self
                    .resume_parked(&source.id, Some(text))
                    .await
                    .unwrap_or(false),
                TaskStatus::Running => {
                    let paste = format!(
                        "\x1b[200~{}\x1b[201~",
                        crate::task_message::clean_message(&text)
                    );
                    self.type_into_session(&source, paste.as_bytes(), Typed::Prompt, Sender::Yulius)
                        .await
                        .is_ok()
                }
                _ => false,
            };
            if delivered && store.mark_handoff_told(&h.id).await.unwrap_or(false) {
                self.audit_handoff(&h, &sandbox, "outcome", Some(&outcome))
                    .await;
                told += 1;
            }
        }
        told
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// HOF-003: a chain goes at most `max_depth` deep and never back to one of its sandboxes.
    #[test]
    fn a_chain_has_a_depth_and_no_cycle() {
        let chain = vec!["s1".to_owned(), "s2".to_owned()];
        assert_eq!(chain_refusal(&chain, 3, "s3", 3), None);
        assert!(chain_refusal(&chain, 4, "s3", 3).is_some_and(|r| r.contains("at most 3 deep")));
        assert!(chain_refusal(&chain, 3, "s1", 3).is_some_and(|r| r.contains("cycle")));
    }

    /// HOF-001: an unknown mode reads as off.
    #[test]
    fn an_unknown_mode_is_off() {
        let mut s = HandoffSetting::default();
        assert_eq!(mode_of(&s), "off");
        s.mode = "automatic".into();
        assert_eq!(mode_of(&s), "automatic");
        s.mode = "sometimes".into();
        assert_eq!(mode_of(&s), "off");
    }
}
