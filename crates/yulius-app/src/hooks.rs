//! Agent hook events (SPEC-002 §7.5, §7.6): briefing, message digests, the Stop discipline gate,
//! activity tracking and the turn boundary for proactive rotation.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::Ordering;

use yulius_contracts::hooks::{HookInput, HookOutput, events};
use yulius_domain::briefing::{
    BriefingInput, DEFAULT_BUDGET_BYTES, MAX_RECENT_CHANGES, ParallelTask, RecentChange, TaskBrief,
    build_briefing,
};
use yulius_domain::rotation::Trigger;
use yulius_domain::spec_format::{DecisionFiles, NamedFile, decision_titles, project_layout};
use yulius_domain::spec_format::{DecisionsLocation, validate_spec_for};
use yulius_domain::steps::StepStatus;
use yulius_domain::task::{Activity, TaskStatus};

use crate::context::App;
use crate::coordination::AgentContext;
use crate::error::{AppError, AppResult};
use crate::model::{InboxEntry, TaskRecord};
use crate::ports::store::TaskFieldUpdate;

/// Consecutive Stop-hook blocks per turn before the task needs attention (AC-DSC-3).
pub const MAX_GATE_BLOCKS: u32 = 3;
/// Largest repository document read for the briefing or the gate.
const MAX_DOC_BYTES: u64 = 256 * 1024;
/// Largest number of decision records read.
const MAX_DECISION_FILES: usize = 200;
/// Largest message digest injected into a turn.
const MAX_DIGEST_BYTES: usize = 8 * 1024;

fn read_bounded(path: &Path) -> Option<String> {
    use std::io::Read;
    let meta = std::fs::symlink_metadata(path).ok()?;
    if !meta.is_file() {
        return None;
    }
    let mut text = String::new();
    std::fs::File::open(path)
        .ok()?
        .take(MAX_DOC_BYTES)
        .read_to_string(&mut text)
        .ok()?;
    Some(text)
}

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
    ) -> AppResult<HookOutput> {
        if !matches!(harness, "claude" | "codex") {
            return Err(AppError::NotFound);
        }
        let ctx = self.agent_context(token).await?;
        match event {
            events::SESSION_START => self.hook_session_start(&ctx, input).await,
            events::USER_PROMPT_SUBMIT => self.hook_prompt_submit(&ctx).await,
            events::STOP => self.hook_stop(&ctx, input).await,
            events::NOTIFICATION | events::PERMISSION_REQUEST => {
                self.set_task_activity(&ctx.task, Activity::NeedsAttention)
                    .await?;
                Ok(HookOutput::default())
            }
            _ => Ok(HookOutput::default()),
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
        let budget = DEFAULT_BUDGET_BYTES - workflow.as_ref().map_or(0, String::len);
        let mut briefing = build_briefing(&input, budget);
        if let Some(workflow) = workflow {
            briefing.push_str(&workflow);
        }
        Ok(briefing)
    }

    async fn hook_prompt_submit(&self, ctx: &AgentContext) -> AppResult<HookOutput> {
        let store = &self.deps.store;
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
            return Ok(HookOutput::default());
        }
        store.mark_delivered(&ctx.task.id, self.now()).await?;
        Ok(HookOutput::additional_context(
            events::USER_PROMPT_SUBMIT,
            digest(&pending),
        ))
    }

    async fn hook_stop(
        self: &Arc<Self>,
        ctx: &AgentContext,
        _input: &HookInput,
    ) -> AppResult<HookOutput> {
        let store = &self.deps.store;
        let task = &ctx.task;
        let mut reasons = Vec::new();

        let urgent: Vec<InboxEntry> = store
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

        if reasons.is_empty() {
            if task.gate_blocks != 0 {
                let update = TaskFieldUpdate {
                    gate_blocks: Some(0),
                    ..TaskFieldUpdate::default()
                };
                store.update_task(&task.id, &update, self.now()).await?;
            }
            self.set_task_activity(task, Activity::Waiting).await?;
            self.turn_ended(&task.id, ctx.attempt.account_id.clone());
            return Ok(HookOutput::default());
        }

        if task.gate_blocks >= MAX_GATE_BLOCKS {
            self.set_task_activity(task, Activity::NeedsAttention)
                .await?;
            self.runtime(&task.id).hub.notice(format!(
                "Yulius: the agent stopped {MAX_GATE_BLOCKS} times without fixing: {}",
                reasons.join(" ").chars().take(300).collect::<String>()
            ));
            return Ok(HookOutput::default());
        }
        let update = TaskFieldUpdate {
            gate_blocks: Some(task.gate_blocks + 1),
            ..TaskFieldUpdate::default()
        };
        store.update_task(&task.id, &update, self.now()).await?;
        if !urgent.is_empty() {
            store.mark_delivered(&task.id, self.now()).await?;
        }
        self.publish_task(&task.id).await;
        Ok(HookOutput::block(reasons.join("\n\n")))
    }

    /// A turn ended: run a pending proactive rotation at this boundary (SPEC-002 §6.3).
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
        let Some(path) = repo_path(&worktree, spec_path) else {
            return Ok(Some(format!(
                "The task spec path `{spec_path}` is not a repository path."
            )));
        };
        let expected = task.id.clone();
        let spec_path = spec_path.clone();
        tokio::task::spawn_blocking(move || match read_bounded(&path) {
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
        let installation = self
            .deps
            .store
            .instance()
            .await?
            .and_then(|i| i.installation_id)
            .ok_or_else(|| AppError::Conflict("the GitHub App is not installed".into()))?;
        let app = self.github_app().await?;
        let token = self
            .deps
            .github
            .installation_token(&app, installation)
            .await?;
        Ok(GitCredential { token })
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
        let toml = read_bounded(&root.join("yulius.toml"));
        let layout = project_layout(toml.as_deref());
        let doc = |rel: &str| repo_path(root, rel).and_then(|p| read_bounded(&p));
        let product = doc(&layout.product).map(|t| t.chars().take(4_000).collect());
        let architecture_present =
            repo_path(root, &layout.architecture).is_some_and(|p| p.is_file());
        let changes = doc(&layout.changes).map(|t| t.chars().take(4_000).collect());
        let (mut decision_file, mut decision_dir) = (None, Vec::new());
        match &layout.decisions {
            DecisionsLocation::File(rel) => decision_file = doc(rel.as_str()),
            DecisionsLocation::Directory(rel) => {
                if let Some(dir) = repo_path(root, rel.as_str())
                    && let Ok(entries) = std::fs::read_dir(dir)
                {
                    let mut names: Vec<PathBuf> = entries
                        .filter_map(Result::ok)
                        .map(|e| e.path())
                        .filter(|p| p.extension().is_some_and(|x| x == "md"))
                        .collect();
                    names.sort();
                    for path in names.into_iter().take(MAX_DECISION_FILES) {
                        if let (Some(name), Some(text)) = (
                            path.file_name().map(|n| n.to_string_lossy().into_owned()),
                            read_bounded(&path),
                        ) {
                            decision_dir.push((name, text));
                        }
                    }
                }
            }
        }
        let instructions = [layout.agents.as_str(), "CLAUDE.md"]
            .iter()
            .filter(|rel| repo_path(root, rel).is_some_and(|p| p.is_file()))
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
    use super::repo_path;
    use std::path::Path;

    #[test]
    fn repo_paths_stay_inside_the_root() {
        let root = Path::new("/r");
        assert_eq!(repo_path(root, "docs/a.md"), Some(root.join("docs/a.md")));
        assert_eq!(repo_path(root, "../x"), None);
        assert_eq!(repo_path(root, "/etc/passwd"), None);
        assert_eq!(repo_path(root, "a/../../x"), None);
    }
}
