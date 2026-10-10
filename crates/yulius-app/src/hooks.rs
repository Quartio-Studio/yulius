//! Agent hook events (SPEC-002 §7.5, §7.6): briefing, message digests, the Stop discipline gate,
//! activity tracking, and the turn boundary where the subscription pool switches or pauses a
//! task (SPEC-002 ADR-17). A pause holds the hook ([`HookReply::Hold`]): the agent waits at its
//! boundary and is never told anything.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::Ordering;

use yulius_contracts::hooks::{HookInput, HookOutput, events};
use yulius_domain::briefing::{
    BriefingInput, DEFAULT_BUDGET_BYTES, MAX_RECENT_CHANGES, ParallelTask, RecentChange, TaskBrief,
    build_briefing,
};
use yulius_domain::rotation::{self, Decision, Trigger};
use yulius_domain::spec_format::{DecisionFiles, NamedFile, decision_titles, project_layout};
use yulius_domain::spec_format::{DecisionsLocation, validate_spec_for};
use yulius_domain::steps::StepStatus;
use yulius_domain::task::{Activity, TaskStatus};

use crate::capacity::{CONTINUATION_MAX, HOLD_RETRY_SECS};
use crate::context::App;
use crate::coordination::AgentContext;
use crate::error::{AppError, AppResult};
use crate::model::{InboxEntry, PauseKind, TaskMode, TaskRecord};
use crate::ports::store::TaskFieldUpdate;

/// Consecutive Stop-hook blocks per turn before the task needs attention (AC-DSC-3).
pub const MAX_GATE_BLOCKS: u32 = 3;
/// Largest repository document read for the briefing or the gate.
const MAX_DOC_BYTES: u64 = 256 * 1024;
/// Largest number of decision records read.
const MAX_DECISION_FILES: usize = 200;
/// Largest message digest injected into a turn.
const MAX_DIGEST_BYTES: usize = 8 * 1024;

/// Joins a repository-relative path, refusing absolute paths and `..` components.
fn repo_path(root: &Path, relative: &str) -> Option<PathBuf> {
    let rel = Path::new(relative);
    if rel.is_absolute()
        || rel
            .components()
            .any(|c| !matches!(c, std::path::Component::Normal(_)))
    {
        return None;
    }
    Some(root.join(rel))
}

/// Reads `root/relative` as a document of the repository: a regular file (not a link) whose
/// resolved path stays under `root`. Directories linked out of the repository are not followed,
/// and on Linux the opened file's real path is checked again, so a link swapped in after the
/// check cannot lead outside either (#70). These files are agent-written and read by the server
/// outside containment for the briefing, `project_context` and the Stop gate.
fn read_repo_doc(root: &Path, relative: &str) -> Option<String> {
    use std::io::Read;
    let path = repo_path(root, relative)?;
    let canonical_root = std::fs::canonicalize(root).ok()?;
    if !std::fs::symlink_metadata(&path).ok()?.is_file() {
        return None;
    }
    let resolved = std::fs::canonicalize(&path).ok()?;
    if !resolved.starts_with(&canonical_root) {
        return None;
    }
    let file = std::fs::File::open(&resolved).ok()?;
    #[cfg(target_os = "linux")]
    {
        use std::os::fd::AsRawFd;
        let real = std::fs::read_link(format!("/proc/self/fd/{}", file.as_raw_fd())).ok()?;
        if !real.starts_with(&canonical_root) {
            return None;
        }
    }
    let mut text = String::new();
    file.take(MAX_DOC_BYTES).read_to_string(&mut text).ok()?;
    Some(text)
}

/// Whether `root/relative` is a regular file of the repository (see [`read_repo_doc`]).
fn repo_doc_exists(root: &Path, relative: &str) -> bool {
    let Some(path) = repo_path(root, relative) else {
        return false;
    };
    std::fs::symlink_metadata(&path).is_ok_and(|meta| meta.is_file())
        && std::fs::canonicalize(root)
            .ok()
            .zip(std::fs::canonicalize(&path).ok())
            .is_some_and(|(root, path)| path.starts_with(root))
}

fn digest(entries: &[InboxEntry]) -> String {
    let mut out = format!(
        "You have {} new Yulius message(s). Read them in full with the `messages_inbox` tool and reply with `message_send` when an answer is expected.\n",
        entries.len()
    );
    for entry in entries {
        let m = &entry.message;
        let from = if m.from_user.is_some() {
            "the user".to_owned()
        } else {
            m.from_task
                .as_ref()
                .map_or_else(|| "Yulius".to_owned(), |t| format!("task {t}"))
        };
        let body: String = m.body.chars().take(400).collect();
        let line = format!(
            "- [{}] from {from}: {} — {}\n",
            m.kind,
            m.subject,
            body.replace('\n', " ")
        );
        if out.len() + line.len() > MAX_DIGEST_BYTES {
            out.push_str("- (more messages in the inbox)\n");
            break;
        }
        out.push_str(&line);
    }
    out
}

/// What a hook call answers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HookReply {
    /// Print this output (empty = no decision) and let the CLI go on.
    Output(HookOutput),
    /// Keep the CLI at this boundary: the hook client asks again after `retry_after_secs`
    /// (`202 Accepted` with `Retry-After`). Used while no subscription has capacity.
    Hold {
        /// Seconds until the hook client asks again.
        retry_after_secs: u64,
    },
}

fn hold() -> HookReply {
    HookReply::Hold {
        retry_after_secs: HOLD_RETRY_SECS,
    }
}

fn is_urgent(entry: &InboxEntry) -> bool {
    !entry.read
        && !entry.delivered
        && (entry.message.from_user.is_some()
            || matches!(entry.message.kind.as_str(), "question" | "request"))
}

impl App {
    /// Handles one hook event for the run owning `token`. Unknown events answer empty.
    ///
    /// # Errors
    /// `Unauthenticated` for unknown or finished runs; store failures.
    pub async fn hook_event(
        self: &Arc<Self>,
        token: &str,
        harness: &str,
        event: &str,
        input: &HookInput,
    ) -> AppResult<HookReply> {
        if harness != "claude" {
            return Err(AppError::NotFound);
        }
        let ctx = self.agent_context(token).await?;
        // This run reports its turn boundaries through hooks: the Stop hook decides switches
        // and pauses, not the transcript.
        self.runtime(&ctx.task.id)
            .hooks_seen
            .store(true, Ordering::SeqCst);
        match event {
            events::SESSION_START => Ok(HookReply::Output(
                self.hook_session_start(&ctx, input).await?,
            )),
            events::USER_PROMPT_SUBMIT => self.hook_prompt_submit(&ctx, input).await,
            events::STOP => self.hook_stop(&ctx, input).await,
            events::NOTIFICATION => {
                self.set_task_activity(&ctx.task, Activity::NeedsAttention)
                    .await?;
                Ok(HookReply::Output(HookOutput::default()))
            }
            _ => Ok(HookReply::Output(HookOutput::default())),
        }
    }

    async fn set_task_activity(&self, task: &TaskRecord, activity: Activity) -> AppResult<()> {
        if task.status != TaskStatus::Running || task.activity == Some(activity) {
            return Ok(());
        }
        self.deps
            .store
            .set_activity(&task.id, Some(activity), self.now())
            .await?;
        self.publish_task(&task.id).await;
        Ok(())
    }

    async fn hook_session_start(
        &self,
        ctx: &AgentContext,
        input: &HookInput,
    ) -> AppResult<HookOutput> {
        if ctx.attempt.provider_session_id.is_none()
            && let Some(session) = input.session_id.as_deref()
            && !session.is_empty()
            && session.len() <= 128
        {
            self.deps
                .store
                .set_attempt_session(&ctx.attempt.id, session)
                .await?;
        }
        if let Err(err) = self.sync_steps_from_spec(&ctx.task).await {
            tracing::warn!(error = %err, task = %ctx.task.id, "step sync from the spec failed");
        }
        let briefing = self.briefing(&ctx.task, ctx.attempt.harness).await?;
        Ok(HookOutput::additional_context(
            events::SESSION_START,
            briefing,
        ))
    }

    /// The session briefing for a task (SPEC-002 §7.6, COM-004, DSC-009).
    ///
    /// # Errors
    /// Store failures.
    pub async fn briefing(
        &self,
        task: &TaskRecord,
        harness: yulius_domain::harness::Harness,
    ) -> AppResult<String> {
        let store = &self.deps.store;
        let sandbox = store
            .sandbox(&task.sandbox_id)
            .await?
            .ok_or(AppError::NotFound)?;
        let root = task
            .worktree_path
            .as_ref()
            .map_or_else(|| self.sandbox_repo(&task.sandbox_id), PathBuf::from);
        let profile = crate::delivery::delivery_profile(&sandbox);
        let root_for_read = root.clone();
        let repo = tokio::task::spawn_blocking(move || RepoDocs::read(&root_for_read))
            .await
            .map_err(AppError::internal)?;

        let steps = store
            .steps(&task.id)
            .await?
            .into_iter()
            .map(|s| {
                let status = StepStatus::parse(&s.status).unwrap_or(StepStatus::Pending);
                (s.title, status)
            })
            .collect();
        let all = store.tasks(&task.sandbox_id, false).await?;
        let claims = store.active_claims(&task.sandbox_id).await?;
        let mut recent: Vec<&TaskRecord> = all
            .iter()
            .filter(|t| t.status == TaskStatus::Closed && t.id != task.id && t.closed_at.is_some())
            .collect();
        recent.sort_by_key(|t| std::cmp::Reverse(t.closed_at));
        let recent_changes = recent
            .into_iter()
            .take(MAX_RECENT_CHANGES)
            .map(|t| RecentChange {
                title: t.title.clone(),
                summary: t
                    .prompt
                    .lines()
                    .next()
                    .unwrap_or_default()
                    .chars()
                    .take(300)
                    .collect(),
                at: t.closed_at.unwrap_or(t.updated_at),
            })
            .collect();
        let parallel_tasks = all
            .iter()
            .filter(|t| t.id != task.id && t.status.is_live())
            .map(|t| ParallelTask {
                number: t.number,
                title: t.title.clone(),
                harness: t.harness,
                status: t.status,
                branch: t.branch.clone().unwrap_or_default(),
                scopes: claims
                    .iter()
                    .filter(|c| c.task_id == t.id)
                    .map(|c| c.pattern.clone())
                    .collect(),
            })
            .collect();
        let unread = store.unread_count(&task.id).await?;
        let decisions = repo.decision_files();
        let input = BriefingInput {
            project_name: format!("{}/{}", sandbox.repo_owner, sandbox.repo_name),
            harness,
            task: TaskBrief {
                id: task.id.clone(),
                number: task.number,
                title: task.title.clone(),
                goal: task.prompt.clone(),
                spec_path: task.spec_path.clone().unwrap_or_default(),
                steps,
                branch: task.branch.clone().unwrap_or_default(),
            },
            profile: &profile,
            layout: &repo.layout,
            product_head: repo.product.as_deref(),
            architecture_present: repo.architecture_present,
            decision_titles: decision_titles(&decisions),
            changes_head: repo.changes.as_deref(),
            recent_changes,
            parallel_tasks,
            unread_messages: usize::try_from(unread).unwrap_or(usize::MAX),
            port_base: sandbox.port_base,
            port_count: sandbox.port_count,
            repo_instructions: repo.instructions.clone(),
        };
        let workflow = self.workflow_instructions(task).await.map(|text| {
            let clipped: String = text.chars().take(3_000).collect();
            format!("\n\n## Workflow instructions\n\n{clipped}\n")
        });
        let chain = self.chain_briefing(task).await;
        let budget = DEFAULT_BUDGET_BYTES
            .saturating_sub(workflow.as_ref().map_or(0, String::len))
            .saturating_sub(chain.as_ref().map_or(0, String::len));
        let mut briefing = build_briefing(&input, budget);
        if let Some(workflow) = workflow {
            briefing.push_str(&workflow);
        }
        if let Some(chain) = chain {
            briefing.push_str(&chain);
        }
        Ok(briefing)
    }

    async fn hook_prompt_submit(
        self: &Arc<Self>,
        ctx: &AgentContext,
        input: &HookInput,
    ) -> AppResult<HookReply> {
        let store = &self.deps.store;
        // A prompt held for capacity stays held while its pause lasts (also across a restart).
        match store.open_pause(&ctx.task.id).await? {
            Some(pause) if pause.kind == PauseKind::Prompt => return Ok(hold()),
            // A held turn the user interrupted to type something: the new prompt replaces it.
            Some(_) => self.end_pause(&ctx.task.id, "superseded").await,
            None => {}
        }
        // Scope discipline (TSK-013): a held prompt never reaches the agent, so nothing below
        // (capacity, activity, message digests) happens for it.
        if let Some(held) = self.screen_prompt(ctx, input).await? {
            return Ok(HookReply::Output(held));
        }
        // The start of a turn: with no subscription below the threshold, the prompt waits here
        // until capacity returns (ROT-010). A prompt too long to carry to another account runs;
        // a limit the CLI reports pauses it then.
        let carried = input
            .prompt
            .as_deref()
            .filter(|p| !p.trim().is_empty() && p.len() <= CONTINUATION_MAX);
        if carried.is_some() || input.prompt.is_none() {
            let current = ctx.attempt.account_id.clone();
            let pool = self.rotation_input(&ctx.task, current.clone()).await?;
            if let (Decision::Wait { .. }, _) = rotation::decide(&pool, Trigger::Proactive) {
                self.pause_task(
                    &ctx.task.id,
                    PauseKind::Prompt,
                    current,
                    carried.map(str::to_owned),
                )
                .await?;
                return Ok(hold());
            }
        }
        // A typed prompt starts new work: an achieved goal no longer makes the task done
        // (TSK-014). Claude Code's own wake-ups (a background shell ended) keep it.
        if crate::finish::is_typed_prompt(input.prompt.as_deref()) {
            self.prompt_submitted(&ctx.task.id);
        }
        if ctx.task.gate_blocks != 0 {
            let update = TaskFieldUpdate {
                gate_blocks: Some(0),
                ..TaskFieldUpdate::default()
            };
            store.update_task(&ctx.task.id, &update, self.now()).await?;
        }
        self.set_task_activity(&ctx.task, Activity::Working).await?;
        let pending: Vec<InboxEntry> = store
            .inbox(&ctx.task.id, true, 50)
            .await?
            .into_iter()
            .filter(|e| !e.delivered)
            .collect();
        if pending.is_empty() {
            return Ok(HookReply::Output(HookOutput::default()));
        }
        store.mark_delivered(&ctx.task.id, self.now()).await?;
        Ok(HookReply::Output(HookOutput::additional_context(
            events::USER_PROMPT_SUBMIT,
            digest(&pending),
        )))
    }

    /// The discipline gate's reasons at a turn end (urgent unread messages, the spec gate) and
    /// whether urgent messages are among them.
    async fn stop_reasons(&self, task: &TaskRecord) -> AppResult<(Vec<String>, bool)> {
        let mut reasons = Vec::new();
        let urgent: Vec<InboxEntry> = self
            .deps
            .store
            .inbox(&task.id, true, 50)
            .await?
            .into_iter()
            .filter(is_urgent)
            .collect();
        if !urgent.is_empty() {
            reasons.push(digest(&urgent));
        }
        if let Some(problem) = self.spec_gate(task).await? {
            reasons.push(problem);
        }
        Ok((reasons, !urgent.is_empty()))
    }

    /// The prompt for a session resumed on another subscription after a held turn: the gate's
    /// reasons when the gate would have kept the agent working, else none (a goal-mode session
    /// gets its goal again when it launches).
    pub(crate) async fn stop_continuation(&self, task: &TaskRecord) -> Option<String> {
        if task.gate_blocks >= MAX_GATE_BLOCKS {
            return None;
        }
        let (reasons, urgent) = self.stop_reasons(task).await.ok()?;
        if reasons.is_empty() {
            return None;
        }
        if urgent {
            let _ = self.deps.store.mark_delivered(&task.id, self.now()).await;
        }
        Some(reasons.join("\n\n"))
    }

    async fn hook_stop(
        self: &Arc<Self>,
        ctx: &AgentContext,
        _input: &HookInput,
    ) -> AppResult<HookReply> {
        let store = &self.deps.store;
        let task = &ctx.task;
        // A turn held for capacity stays held while its pause lasts (also across a restart).
        if let Some(pause) = store.open_pause(&task.id).await? {
            if matches!(pause.kind, PauseKind::Turn | PauseKind::Limit) {
                return Ok(hold());
            }
            self.end_pause(&task.id, "superseded").await;
        }
        if let Err(err) = self.sync_steps_from_spec(task).await {
            tracing::warn!(error = %err, task = %task.id, "step sync from the spec failed");
        }
        let (reasons, has_urgent) = self.stop_reasons(task).await?;
        let gate_continues = !reasons.is_empty() && task.gate_blocks < MAX_GATE_BLOCKS;
        // Would the agent work on after this boundary (a gate block, or its goal)?
        let continues = gate_continues || task.mode == TaskMode::Goal;

        // The turn boundary: stay, move to another subscription, or pause (ROT-003, ROT-010).
        self.runtime(&task.id)
            .proactive_switch
            .store(false, Ordering::SeqCst);
        let current = ctx.attempt.account_id.clone();
        let pool = self.rotation_input(task, current.clone()).await?;
        match rotation::decide(&pool, Trigger::Proactive) {
            (Decision::Stay, _) => {}
            (Decision::Switch { account }, reason) => {
                // The agent waits here while it moves; the resumed session gets the gate's
                // reasons as its prompt when the gate would have kept it working.
                let continuation = gate_continues.then(|| reasons.join("\n\n"));
                if continuation.is_some() && has_urgent {
                    store.mark_delivered(&task.id, self.now()).await?;
                }
                let app = Arc::clone(self);
                let task_id = task.id.clone();
                let reason = format!("{reason:?}");
                tokio::spawn(async move {
                    if let Err(err) = app
                        .switch_task_now(&task_id, Some(account), &reason, continuation)
                        .await
                    {
                        tracing::warn!(error = %err, task = %task_id, "switch at the turn boundary failed");
                    }
                });
                return Ok(hold());
            }
            (Decision::Wait { .. }, _) if continues => {
                self.pause_task(&task.id, PauseKind::Turn, current, None)
                    .await?;
                return Ok(hold());
            }
            // The agent goes idle anyway; its next prompt is held if capacity is still gone.
            (Decision::Wait { .. }, _) => {}
        }

        if reasons.is_empty() {
            if task.gate_blocks != 0 {
                let update = TaskFieldUpdate {
                    gate_blocks: Some(0),
                    ..TaskFieldUpdate::default()
                };
                store.update_task(&task.id, &update, self.now()).await?;
            }
            // Done when the goal is achieved, waiting otherwise (TSK-014).
            let next = self.activity_after_clean_turn(&task.id);
            self.set_task_activity(task, next).await?;
            let app = Arc::clone(self);
            let task_id = task.id.clone();
            tokio::spawn(async move { app.probe_idle_shells(&task_id).await });
            // A clean stop of a finished task opens its pull request (#35).
            let app = Arc::clone(self);
            let task_id = task.id.clone();
            tokio::spawn(async move {
                app.auto_open_pr(&task_id).await;
                // Tasks chained after this one start once it is Done (TSK-012).
                app.advance_chains(&task_id).await;
            });
            return Ok(HookReply::Output(HookOutput::default()));
        }

        if task.gate_blocks >= MAX_GATE_BLOCKS {
            self.set_task_activity(task, Activity::NeedsAttention)
                .await?;
            self.runtime(&task.id).hub.notice(format!(
                "Yulius: the agent stopped {MAX_GATE_BLOCKS} times without fixing: {}",
                reasons.join(" ").chars().take(300).collect::<String>()
            ));
            return Ok(HookReply::Output(HookOutput::default()));
        }
        let update = TaskFieldUpdate {
            gate_blocks: Some(task.gate_blocks + 1),
            ..TaskFieldUpdate::default()
        };
        store.update_task(&task.id, &update, self.now()).await?;
        if has_urgent {
            store.mark_delivered(&task.id, self.now()).await?;
        }
        self.publish_task(&task.id).await;
        Ok(HookReply::Output(HookOutput::block(reasons.join("\n\n"))))
    }

    /// A turn of a run without hooks ended (seen in its transcript): run a pending proactive
    /// rotation at this boundary (SPEC-002 §6.3).
    pub(crate) fn turn_ended(
        self: &Arc<Self>,
        task_id: &yulius_domain::ids::TaskId,
        account: Option<yulius_domain::ids::AccountId>,
    ) {
        let runtime = self.runtime(task_id);
        if runtime.proactive_switch.swap(false, Ordering::SeqCst) {
            let app = Arc::clone(self);
            let task_id = task_id.clone();
            tokio::spawn(async move {
                app.rotate(&task_id, account, Trigger::Proactive).await;
            });
        }
    }

    /// The discipline gate on the task spec: `Some(reason)` when the worktree has changes and
    /// the spec is missing or invalid.
    async fn spec_gate(&self, task: &TaskRecord) -> AppResult<Option<String>> {
        let (Some(worktree), Some(spec_path)) =
            (task.worktree_path.as_ref(), task.spec_path.as_ref())
        else {
            return Ok(None);
        };
        let worktree = PathBuf::from(worktree);
        let sandbox = self
            .deps
            .store
            .sandbox(&task.sandbox_id)
            .await?
            .ok_or(AppError::NotFound)?;
        let base = format!(
            "origin/{}",
            crate::delivery::delivery_profile(&sandbox).integration_branch
        );
        let dirty = self.deps.git.has_changes(&worktree).await.unwrap_or(false);
        let ahead = self
            .deps
            .git
            .ahead_behind(&worktree, &base, "HEAD")
            .await
            .map(|(ahead, _)| ahead > 0)
            .unwrap_or(false);
        if !dirty && !ahead {
            return Ok(None);
        }
        if repo_path(&worktree, spec_path).is_none() {
            return Ok(Some(format!(
                "The task spec path `{spec_path}` is not a repository path."
            )));
        }
        let expected = task.id.clone();
        let spec_path = spec_path.clone();
        tokio::task::spawn_blocking(move || match read_repo_doc(&worktree, &spec_path) {
            None => Some(format!(
                "You changed code but the task spec `{spec_path}` does not exist. Create it in the Yulius format (frontmatter with `yulius-task: {expected}`, the required sections and a Plan checklist) before finishing."
            )),
            Some(text) => {
                let report = validate_spec_for(&text, &expected);
                (!report.is_valid()).then(|| report.agent_message(&spec_path))
            }
        })
        .await
        .map_err(AppError::internal)
    }
}

impl App {
    /// Keeps the task's steps in sync with the Plan checklist of its spec (STP-004): the
    /// top-level plan items become the steps in order; checked items are `done`; unchecked
    /// items keep the status of the existing step with the same title (agents may mark them
    /// `in_progress` before ticking the box) or start `pending`. Writes only on change. Returns
    /// whether the steps changed.
    ///
    /// # Errors
    /// Store failures.
    pub async fn sync_steps_from_spec(&self, task: &TaskRecord) -> AppResult<bool> {
        let (Some(worktree), Some(spec_path)) =
            (task.worktree_path.as_ref(), task.spec_path.as_ref())
        else {
            return Ok(false);
        };
        let (root, rel) = (PathBuf::from(worktree), spec_path.clone());
        let expected = task.id.clone();
        let plan = tokio::task::spawn_blocking(move || {
            read_repo_doc(&root, &rel).map(|text| validate_spec_for(&text, &expected).plan)
        })
        .await
        .map_err(AppError::internal)?;
        let Some(plan) = plan else { return Ok(false) };
        let items: Vec<_> = plan
            .into_iter()
            .filter(|item| item.depth == 0 && !item.text.trim().is_empty())
            .take(100)
            .collect();
        if items.is_empty() {
            return Ok(false);
        }
        let existing = self.deps.store.steps(&task.id).await?;
        let now = self.now();
        let mut steps = Vec::with_capacity(items.len());
        for (i, item) in items.iter().enumerate() {
            let title: String = item.text.trim().chars().take(300).collect();
            let previous = existing.iter().find(|s| s.title == title);
            let status = if item.checked {
                "done".to_owned()
            } else {
                previous.map_or_else(|| "pending".to_owned(), |p| p.status.clone())
            };
            let unchanged = previous.is_some_and(|p| p.status == status);
            steps.push(crate::model::StepRecord {
                id: match previous {
                    Some(p) => p.id.clone(),
                    None => yulius_domain::ids::StepId::parse(&self.deps.crypto.new_id())
                        .map_err(AppError::internal)?,
                },
                task_id: task.id.clone(),
                ordinal: u32::try_from(i + 1).unwrap_or(u32::MAX),
                title,
                status,
                note: previous.and_then(|p| p.note.clone()),
                updated_by: if unchanged {
                    previous.and_then(|p| p.updated_by.clone())
                } else {
                    Some("spec".into())
                },
                updated_at: if unchanged {
                    previous.and_then(|p| p.updated_at)
                } else {
                    Some(now)
                },
            });
        }
        let same = steps.len() == existing.len()
            && steps
                .iter()
                .zip(&existing)
                .all(|(a, b)| a.title == b.title && a.status == b.status);
        if same {
            return Ok(false);
        }
        self.deps
            .store
            .replace_steps(&task.id, &steps, "spec", now)
            .await?;
        self.publish_steps(task).await;
        Ok(true)
    }
}

/// A Git credential issued to a task (`POST /internal/git-credential`).
#[derive(Debug)]
pub struct GitCredential {
    /// Installation access token.
    pub token: crate::model::Secret,
}

impl App {
    /// Issues the GitHub installation token for the run's own repository (SPEC-002 §8, R-06).
    /// Requests for any other host or repository are refused.
    ///
    /// # Errors
    /// `Unauthenticated` for unknown runs, `Forbidden` for other repositories, GitHub failures.
    pub async fn agent_git_credential(
        &self,
        token: &str,
        host: &str,
        path: Option<&str>,
    ) -> AppResult<GitCredential> {
        let ctx = self.agent_context(token).await?;
        if host != "github.com" {
            return Err(AppError::Forbidden(
                "credentials are only issued for github.com".into(),
            ));
        }
        let sandbox = self
            .deps
            .store
            .sandbox(&ctx.task.sandbox_id)
            .await?
            .ok_or(AppError::NotFound)?;
        if let Some(path) = path {
            let wanted = path
                .trim_matches('/')
                .trim_end_matches(".git")
                .to_ascii_lowercase();
            let own = format!("{}/{}", sandbox.repo_owner, sandbox.repo_name).to_ascii_lowercase();
            if wanted != own {
                return Err(AppError::Forbidden(
                    "credentials are only issued for this project's repository".into(),
                ));
            }
        }
        let token = self.repository_installation_token(&sandbox).await?;
        Ok(GitCredential { token })
    }

    /// An installation token restricted to `sandbox`'s repository (agents' Git, and the
    /// superadmin's shells, ACC-012).
    ///
    /// # Errors
    /// `Conflict` when the App is not installed; GitHub failures.
    pub(crate) async fn repository_installation_token(
        &self,
        sandbox: &crate::model::SandboxRecord,
    ) -> AppResult<crate::model::Secret> {
        let installation = self
            .deps
            .store
            .instance()
            .await?
            .and_then(|i| i.installation_id)
            .ok_or_else(|| AppError::Conflict("the GitHub App is not installed".into()))?;
        let app = self.github_app().await?;
        self.deps
            .github
            .repository_token(&app, installation, &sandbox.repo_name)
            .await
    }
}

/// Repository documents read for the briefing.
struct RepoDocs {
    layout: yulius_domain::spec_format::DocsLayout,
    product: Option<String>,
    architecture_present: bool,
    changes: Option<String>,
    decision_file: Option<String>,
    decision_dir: Vec<(String, String)>,
    instructions: Vec<String>,
}

impl RepoDocs {
    fn read(root: &Path) -> Self {
        let toml = read_repo_doc(root, "yulius.toml");
        let layout = project_layout(toml.as_deref());
        let doc = |rel: &str| read_repo_doc(root, rel);
        let product = doc(&layout.product).map(|t| t.chars().take(4_000).collect());
        let architecture_present = repo_doc_exists(root, &layout.architecture);
        let changes = doc(&layout.changes).map(|t| t.chars().take(4_000).collect());
        let (mut decision_file, mut decision_dir) = (None, Vec::new());
        match &layout.decisions {
            DecisionsLocation::File(rel) => decision_file = doc(rel.as_str()),
            DecisionsLocation::Directory(rel) => {
                // The directory itself must be a real one inside the repository (#70).
                if let Some(dir) = repo_path(root, rel.as_str())
                    && std::fs::symlink_metadata(&dir).is_ok_and(|meta| meta.is_dir())
                    && std::fs::canonicalize(root)
                        .ok()
                        .zip(std::fs::canonicalize(&dir).ok())
                        .is_some_and(|(root, dir)| dir.starts_with(root))
                    && let Ok(entries) = std::fs::read_dir(&dir)
                {
                    let mut names: Vec<String> = entries
                        .filter_map(Result::ok)
                        .filter_map(|e| e.file_name().to_str().map(str::to_owned))
                        .filter(|name| name.ends_with(".md"))
                        .collect();
                    names.sort();
                    for name in names.into_iter().take(MAX_DECISION_FILES) {
                        if let Some(text) = read_repo_doc(root, &format!("{rel}/{name}")) {
                            decision_dir.push((name, text));
                        }
                    }
                }
            }
        }
        let instructions = [layout.agents.as_str(), "CLAUDE.md"]
            .iter()
            .filter(|rel| repo_doc_exists(root, rel))
            .map(|rel| (*rel).to_owned())
            .collect();
        Self {
            layout,
            product,
            architecture_present,
            changes,
            decision_file,
            decision_dir,
            instructions,
        }
    }

    fn decision_files(&self) -> DecisionFiles<'_> {
        if let Some(text) = &self.decision_file {
            return DecisionFiles::File(text);
        }
        if self.decision_dir.is_empty() {
            return DecisionFiles::Missing;
        }
        DecisionFiles::Directory(
            self.decision_dir
                .iter()
                .map(|(name, contents)| NamedFile { name, contents })
                .collect(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::{RepoDocs, repo_path};
    use std::path::Path;

    /// #70: a document path through a linked directory never leaves the repository (the
    /// briefing and `project_context` would hand the agent a host file otherwise).
    #[test]
    fn repo_docs_never_follow_linked_directories_out_of_the_repository()
    -> Result<(), Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let root = tmp.path().join("repo");
        let outside = tmp.path().join("outside");
        std::fs::create_dir_all(&root)?;
        std::fs::create_dir_all(outside.join("decisions"))?;
        std::fs::write(outside.join("secret.md"), "TOP SECRET")?;
        std::fs::write(
            outside.join("decisions/0001-secret.md"),
            "# Secret decision",
        )?;
        std::os::unix::fs::symlink(&outside, root.join("x"))?;
        std::fs::write(
            root.join("yulius.toml"),
            "[docs]\nproduct = \"x/secret.md\"\nchanges = \"x/secret.md\"\ndecisions = \"x/decisions\"\n",
        )?;
        let docs = RepoDocs::read(&root);
        assert_eq!(
            docs.product, None,
            "product read through a linked directory"
        );
        assert_eq!(
            docs.changes, None,
            "changes read through a linked directory"
        );
        assert!(
            docs.decision_dir.is_empty(),
            "decisions listed through a link"
        );
        // A real document inside the repository is still read.
        std::fs::create_dir_all(root.join("docs"))?;
        std::fs::write(root.join("docs/product.md"), "# Product")?;
        std::fs::write(
            root.join("yulius.toml"),
            "[docs]\nproduct = \"docs/product.md\"\n",
        )?;
        assert_eq!(RepoDocs::read(&root).product.as_deref(), Some("# Product"));
        Ok(())
    }

    #[test]
    fn repo_paths_stay_inside_the_root() {
        let root = Path::new("/r");
        assert_eq!(repo_path(root, "docs/a.md"), Some(root.join("docs/a.md")));
        assert_eq!(repo_path(root, "../x"), None);
        assert_eq!(repo_path(root, "/etc/passwd"), None);
        assert_eq!(repo_path(root, "a/../../x"), None);
    }
}
