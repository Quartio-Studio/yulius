//! Sandbox onboarding: the preparation meta-task of imported projects (SPEC-001 DSC-010,
//! IMP-005, U-08; SPEC-002 §6.6, §8a).
//!
//! `not_needed | pending → preparing → ready | skipped`. An imported repository whose
//! documentation fails [`validate_project`] gets a meta-task whose prompt asks the agent to adapt
//! the repository to the Yulius format without breaking its conventions. The task starts by
//! itself (no approval step, F 2026-10-10): at once when the repository is added in the UI, when
//! the upload has landed when it comes through `yulius onboard`. Until the task has run and the
//! project validates (`ready`), or an admin skips it, no other task starts in the sandbox: not
//! created, continued, chained or run by an automation ([`App::ensure_tasks_may_start`], U-08).
//! [`App::onboarding_status`] says what is still pending, for the CLI and the UI.

use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use yulius_contracts::api::sandboxes::{Profile, Sandbox};
use yulius_contracts::import::{OnboardingStatus, OnboardingTask};
use yulius_domain::harness::Harness;
use yulius_domain::ids::SandboxId;
use yulius_domain::permission::Action;
use yulius_domain::spec_format::{
    DecisionFiles, DecisionsLocation, MAX_CONFIG_BYTES, NamedFile, ProjectFiles, ProjectReport,
    project_layout, validate_project,
};

use crate::auth::AuthContext;
use crate::context::App;
use crate::error::{AppError, AppResult};
use crate::lifecycle::NewTask;
use crate::model::{SandboxRecord, TaskOrigin, TaskRecord, UserRecord};
use crate::ports::store::TaskFieldUpdate;
use crate::projects::blocking;
use crate::supervisor::RunStart;
use crate::tasks::progress;

/// Bound on one documentation file read for validation.
pub const MAX_DOC_BYTES: u64 = 1024 * 1024;
/// Bound on decision files read from one directory.
pub const MAX_DECISION_FILES: usize = 1_000;
/// Problems listed in the onboarding prompt (the rest are summarized).
const MAX_PROMPT_PROBLEMS: usize = 40;
/// Problems listed in the onboarding status.
const MAX_STATUS_PROBLEMS: usize = 5;
/// Title of the preparation meta-task.
pub const ONBOARDING_TITLE: &str = "Prepare this repository for Yulius";

/// Sandbox onboarding state (SPEC-002 §6.6).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum OnboardingState {
    /// The project validated on import or was created by Yulius.
    NotNeeded,
    /// The preparation task exists and waits for approval.
    Pending,
    /// The preparation task was started.
    Preparing,
    /// The preparation task finished and the project validates.
    Ready,
    /// An admin skipped preparation.
    Skipped,
}

impl OnboardingState {
    /// Wire name.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::NotNeeded => "not_needed",
            Self::Pending => "pending",
            Self::Preparing => "preparing",
            Self::Ready => "ready",
            Self::Skipped => "skipped",
        }
    }

    /// Parses a wire name.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "not_needed" => Some(Self::NotNeeded),
            "pending" => Some(Self::Pending),
            "preparing" => Some(Self::Preparing),
            "ready" => Some(Self::Ready),
            "skipped" => Some(Self::Skipped),
            _ => None,
        }
    }

    /// Whether ordinary tasks are refused in this state (U-08).
    #[must_use]
    pub fn blocks_tasks(self) -> bool {
        matches!(self, Self::Pending | Self::Preparing)
    }
}

/// `root/rel` when it is a regular file (not a link) whose parent resolves inside `root`.
fn regular_file_inside(root: &Path, canonical_root: &Path, rel: &str) -> Option<PathBuf> {
    let path = root.join(rel);
    let meta = fs::symlink_metadata(&path).ok()?;
    if !meta.is_file() {
        return None;
    }
    let parent = fs::canonicalize(path.parent()?).ok()?;
    parent.starts_with(canonical_root).then_some(path)
}

/// `root/rel` when it is a real directory (not a link) resolving inside `root`.
fn directory_inside(root: &Path, canonical_root: &Path, rel: &str) -> Option<PathBuf> {
    let path = root.join(rel);
    let meta = fs::symlink_metadata(&path).ok()?;
    if !meta.is_dir() {
        return None;
    }
    let resolved = fs::canonicalize(&path).ok()?;
    resolved.starts_with(canonical_root).then_some(path)
}

fn read_bounded(path: &Path, max: u64) -> Option<String> {
    let mut bytes = Vec::new();
    File::open(path)
        .ok()?
        .take(max)
        .read_to_end(&mut bytes)
        .ok()?;
    Some(String::from_utf8_lossy(&bytes).into_owned())
}

fn directory_files(dir: &Path) -> Vec<(String, String)> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    entries
        .filter_map(Result::ok)
        .take(MAX_DECISION_FILES)
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_file()))
        .filter_map(|entry| {
            let name = entry.file_name().to_str()?.to_owned();
            let contents = read_bounded(&entry.path(), MAX_DOC_BYTES)?;
            Some((name, contents))
        })
        .collect()
}

enum OwnedDecisions {
    Missing,
    Directory(Vec<(String, String)>),
    File(String),
}

/// Validates the documentation format of a checkout (SPEC-001 §7.2) and returns the report
/// plus the raw `yulius.toml`. Files are read at the layout's paths, bounded in size; symbolic
/// links and paths resolving outside `root` count as missing.
#[must_use]
pub fn validate_checkout(root: &Path) -> (ProjectReport, Option<String>) {
    let canonical = fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    let read = |rel: &str, max: u64| {
        regular_file_inside(root, &canonical, rel).and_then(|path| read_bounded(&path, max))
    };
    let yulius_toml = read(
        "yulius.toml",
        u64::try_from(MAX_CONFIG_BYTES).unwrap_or(u64::MAX),
    );
    let layout = project_layout(yulius_toml.as_deref());
    let agents = read(&layout.agents, MAX_DOC_BYTES);
    let claude = read("CLAUDE.md", MAX_DOC_BYTES);
    let product = read(&layout.product, MAX_DOC_BYTES);
    let architecture = read(&layout.architecture, MAX_DOC_BYTES);
    let changes = read(&layout.changes, MAX_DOC_BYTES);
    let decisions = match &layout.decisions {
        DecisionsLocation::Directory(path) | DecisionsLocation::File(path) => {
            if let Some(dir) = directory_inside(root, &canonical, path) {
                OwnedDecisions::Directory(directory_files(&dir))
            } else if let Some(text) = read(path, MAX_DOC_BYTES) {
                OwnedDecisions::File(text)
            } else {
                OwnedDecisions::Missing
            }
        }
    };
    let named: Vec<NamedFile<'_>> = match &decisions {
        OwnedDecisions::Directory(files) => files
            .iter()
            .map(|(name, contents)| NamedFile { name, contents })
            .collect(),
        _ => Vec::new(),
    };
    let files = ProjectFiles {
        agents: agents.as_deref(),
        claude: claude.as_deref(),
        yulius_toml: yulius_toml.as_deref(),
        product: product.as_deref(),
        architecture: architecture.as_deref(),
        decisions: match &decisions {
            OwnedDecisions::Missing => DecisionFiles::Missing,
            OwnedDecisions::Directory(_) => DecisionFiles::Directory(named),
            OwnedDecisions::File(text) => DecisionFiles::File(text),
        },
        changes: changes.as_deref(),
    };
    let report = validate_project(&files);
    (report, yulius_toml)
}

/// The preparation meta-task prompt (DSC-010, SPEC-002 §8a).
#[must_use]
pub fn onboarding_prompt(repo: &str, profile: &Profile, report: &ProjectReport) -> String {
    let mut problems: Vec<String> = report
        .problems
        .iter()
        .take(MAX_PROMPT_PROBLEMS)
        .map(|problem| format!("- {problem}"))
        .collect();
    if report.problems.len() > MAX_PROMPT_PROBLEMS {
        problems.push(format!(
            "- … and {} more",
            report.problems.len() - MAX_PROMPT_PROBLEMS
        ));
    }
    if problems.is_empty() {
        problems.push("- (none reported; re-check the format anyway)".to_owned());
    }
    let integration = &profile.integration_branch;
    let production = &profile.production_branch;
    format!(
        "Prepare the repository {repo} for Yulius.\n\n\
         This project was imported into Yulius, which enforces one documentation format: \
         yulius.toml, AGENTS.md, a CLAUDE.md that points to AGENTS.md, a product summary, an \
         architecture document with a Mermaid diagram, decision records, a specs directory and a \
         changes log. Adapt this repository to that format WITHOUT breaking its existing \
         conventions.\n\n\
         Problems Yulius found:\n{problems}\n\n\
         How to work:\n\
         1. Read the repository's own instructions first (AGENTS.md, CLAUDE.md, .agents/skills, \
         .claude/skills, CONTRIBUTING and README). They stay authoritative: follow them for \
         branches, commits, tests and reviews.\n\
         2. Ask the user once whether you should reverse-engineer the existing project into these \
         documents (product summary, architecture and decisions derived from the code and its \
         history) or only add short summaries. If nobody answers, reverse-engineer.\n\
         3. Map instead of moving: when equivalent documents already exist (a README summary, an \
         architecture page, a decisions file, a changelog), point to them from yulius.toml [docs] \
         (product, architecture, decisions, specs_dir, changes, agents) instead of renaming or \
         duplicating them.\n\
         4. Add only what is missing: the product summary, an architecture document with at least \
         one ```mermaid diagram, decisions (a directory of NNNN-<slug>.md ADRs with Status, \
         Context, Decision, Alternatives and Consequences, or one file with a heading per \
         decision), the specs directory, the changes log, and a CLAUDE.md pointer to AGENTS.md. \
         Keep every existing rule in AGENTS.md; add a short pointer to the Yulius documents.\n\
         5. Record the delivery profile in yulius.toml [delivery]: preset \"{preset}\" \
         (integration branch {integration}, production branch {production}), and put the \
         repository's real test, lint and build commands in [commands].\n\
         6. git-crypt: encrypted files stay locked. Never unlock or decrypt them, never commit \
         decrypted content, and leave encrypted files byte-for-byte unchanged.\n\
         7. Hooks: when the repository's instructions describe a hooks setup (for example \
         `git config core.hooksPath .githooks`), apply it in your worktree before committing. \
         Never bypass hooks with --no-verify.\n\
         8. Deliver through a pull request from your task branch into {integration}; never push \
         to {integration} or {production}. Change documentation and configuration only, not \
         application code.\n\
         9. Before finishing, make sure every problem listed above is resolved.",
        preset = profile.preset,
        problems = problems.join("\n"),
    )
}

/// Initial steps of the preparation task.
fn onboarding_steps() -> Vec<String> {
    [
        "Read the repository's instructions and conventions",
        "Ask whether to reverse-engineer the project into the docs",
        "Map existing documents in yulius.toml",
        "Add the missing documents",
        "Open a pull request with the changes",
    ]
    .iter()
    .map(|step| (*step).to_owned())
    .collect()
}

impl App {
    /// Creates the preparation meta-task of an imported sandbox: resting (not started) and kept
    /// in the active task list so it can be approved.
    ///
    /// # Errors
    /// Store failures.
    pub(crate) async fn create_onboarding_task(
        self: &Arc<Self>,
        sandbox: &SandboxRecord,
        user: &UserRecord,
        report: &ProjectReport,
    ) -> AppResult<TaskRecord> {
        let profile = self.profile_of(sandbox);
        let repo = format!("{}/{}", sandbox.repo_owner, sandbox.repo_name);
        let task = self
            .create_task_internal(NewTask {
                sandbox: sandbox.id.clone(),
                created_by: user.id.clone(),
                prompt: onboarding_prompt(&repo, &profile, report),
                harness: Harness::ClaudeCode,
                title: Some(ONBOARDING_TITLE.to_owned()),
                workflow_id: None,
                steps: onboarding_steps(),
                model: None,
                effort: None,
                origin: TaskOrigin::Onboarding,
                automation_run: None,
                start: false,
                mode: crate::model::TaskMode::Prompt,
            })
            .await?;
        // Tasks created without a run start archived; the preparation task must stay visible.
        self.deps
            .store
            .update_task(
                &task.id,
                &TaskFieldUpdate {
                    archived_at: Some(None),
                    ..TaskFieldUpdate::default()
                },
                self.now(),
            )
            .await?;
        self.publish_task(&task.id).await;
        self.deps
            .store
            .task(&task.id)
            .await?
            .ok_or(AppError::NotFound)
    }

    async fn set_onboarding_state(
        &self,
        sandbox: &SandboxRecord,
        next: OnboardingState,
        actor: &str,
    ) -> AppResult<()> {
        self.deps
            .store
            .set_onboarding(&sandbox.id, next.as_str(), None)
            .await?;
        self.deps
            .store
            .audit(
                self.now(),
                actor,
                "sandbox.onboarding",
                Some(sandbox.id.as_str()),
                Some(&format!(
                    "{} -> {}",
                    sandbox.onboarding_state,
                    next.as_str()
                )),
            )
            .await?;
        self.publish_resync(Some(sandbox.id.clone()));
        Ok(())
    }

    /// Re-evaluates a `pending`/`preparing` sandbox and returns its (possibly new) onboarding
    /// state: `preparing` once the preparation task has been started (also through
    /// `POST /api/tasks/{tid}/continue`), `ready` once it rests after running and the project
    /// validates in its worktree or in the sandbox clone. Other states are returned unchanged.
    /// Cheap for settled sandboxes (one store read); call it on task close and on reads.
    ///
    /// # Errors
    /// `NotFound` for unknown sandboxes, store failures.
    pub async fn refresh_onboarding(&self, sandbox: &SandboxId) -> AppResult<String> {
        let store = &self.deps.store;
        let record = store.sandbox(sandbox).await?.ok_or(AppError::NotFound)?;
        let Some(state) = OnboardingState::parse(&record.onboarding_state) else {
            return Ok(record.onboarding_state);
        };
        if !state.blocks_tasks() {
            return Ok(record.onboarding_state);
        }
        let Some(task_id) = record.onboarding_task_id.clone() else {
            return Ok(record.onboarding_state);
        };
        let Some(task) = store.task(&task_id).await? else {
            return Ok(record.onboarding_state);
        };
        let resting = task.status.is_resting();
        let started = !resting || !store.attempts(&task.id).await?.is_empty();
        if !started {
            return Ok(record.onboarding_state);
        }
        let mut next = OnboardingState::Preparing;
        if resting {
            let mut candidates: Vec<PathBuf> = Vec::new();
            if let Some(path) = &task.worktree_path {
                candidates.push(PathBuf::from(path));
            }
            candidates.push(self.sandbox_repo(&record.id));
            let valid = blocking(move || {
                Ok(candidates
                    .iter()
                    .filter(|dir| dir.is_dir())
                    .any(|dir| validate_checkout(dir).0.is_valid()))
            })
            .await?;
            if valid {
                next = OnboardingState::Ready;
            }
        }
        if next != state {
            self.set_onboarding_state(&record, next, "system").await?;
        }
        Ok(next.as_str().to_owned())
    }

    /// `POST /api/sandboxes/{sid}/onboarding/skip` (admin): unblocks ordinary tasks; a running
    /// preparation task keeps running.
    ///
    /// # Errors
    /// `NotFound` without access, `Forbidden` without admin.
    pub async fn skip_onboarding(&self, ctx: &AuthContext, sid: &str) -> AppResult<Sandbox> {
        let id = SandboxId::parse(sid).map_err(|_| AppError::NotFound)?;
        let (record, level) = self.require(&ctx.user, &id, Action::Administer).await?;
        if OnboardingState::parse(&record.onboarding_state)
            .is_some_and(OnboardingState::blocks_tasks)
        {
            let actor = self.login_of(&ctx.user.id).await;
            self.set_onboarding_state(&record, OnboardingState::Skipped, &actor)
                .await?;
        }
        let record = self
            .deps
            .store
            .sandbox(&id)
            .await?
            .ok_or(AppError::NotFound)?;
        self.sandbox_view(&record, level).await
    }

    /// The onboarding gate (U-08): while a sandbox is `pending` or `preparing`, no task other
    /// than its preparation task may start — not created, continued, chained or run by an
    /// automation.
    ///
    /// # Errors
    /// `Conflict` while the sandbox is being prepared; `NotFound` for unknown sandboxes.
    pub async fn ensure_tasks_may_start(
        &self,
        sandbox: &SandboxId,
        origin: TaskOrigin,
    ) -> AppResult<()> {
        if origin == TaskOrigin::Onboarding {
            return Ok(());
        }
        let state = self.refresh_onboarding(sandbox).await?;
        if OnboardingState::parse(&state).is_some_and(OnboardingState::blocks_tasks) {
            return Err(AppError::Conflict(
                "this project is being prepared for Yulius; tasks can start once the preparation \
                 task has made it ready (or an admin skips it)"
                    .into(),
            ));
        }
        Ok(())
    }

    /// Starts the preparation task of a `pending` sandbox (creating it when it is missing) and
    /// moves the sandbox to `preparing`. Without a subscription with capacity the task waits,
    /// paused, for one (ROT-010) and the sandbox is still `preparing`. Returns whether it started.
    ///
    /// # Errors
    /// Store failures; the task's start failures (the sandbox then stays `pending`).
    pub(crate) async fn start_onboarding(
        self: &Arc<Self>,
        sandbox: &SandboxId,
        actor: &str,
    ) -> AppResult<bool> {
        let store = &self.deps.store;
        let record = store.sandbox(sandbox).await?.ok_or(AppError::NotFound)?;
        if OnboardingState::parse(&record.onboarding_state) != Some(OnboardingState::Pending) {
            return Ok(false);
        }
        let existing = match &record.onboarding_task_id {
            Some(id) => store.task(id).await?,
            None => None,
        };
        let task = match existing {
            Some(task) => task,
            None => {
                let root = self.sandbox_repo(&record.id);
                let (report, _) = blocking(move || Ok(validate_checkout(&root))).await?;
                let creator = store
                    .user(&record.created_by)
                    .await?
                    .ok_or(AppError::NotFound)?;
                let task = self.create_onboarding_task(&record, &creator, &report).await?;
                store
                    .set_onboarding(&record.id, OnboardingState::Pending.as_str(), Some(&task.id))
                    .await?;
                task
            }
        };
        if task.status.is_resting() {
            self.start_run(&task.id, RunStart::Resume { prompt: None }, None)
                .await?;
        }
        let record = store.sandbox(sandbox).await?.ok_or(AppError::NotFound)?;
        self.set_onboarding_state(&record, OnboardingState::Preparing, actor)
            .await?;
        Ok(true)
    }

    /// The onboarding state of a sandbox and what is still pending, in words (for `yulius
    /// onboard` and the UI).
    ///
    /// # Errors
    /// Store failures.
    pub async fn onboarding_status(&self, sandbox: &SandboxRecord) -> AppResult<OnboardingStatus> {
        let store = &self.deps.store;
        let state = OnboardingState::parse(&sandbox.onboarding_state);
        let task = match &sandbox.onboarding_task_id {
            Some(id) => store.task(id).await?,
            None => None,
        };
        let mut view = None;
        if let Some(task) = &task {
            let steps = progress(&store.steps(&task.id).await?);
            view = Some(OnboardingTask {
                number: task.number,
                title: task.title.clone(),
                status: task.status.as_str().to_owned(),
                steps_done: steps.done,
                steps_total: steps.total,
                url: format!(
                    "{}/s/{}/t/{}",
                    self.config.public_url, sandbox.slug, task.number
                ),
            });
        }
        let label = task
            .as_ref()
            .map_or_else(|| "the preparation task".to_owned(), |t| format!("task #{}", t.number));
        let (summary, pending): (String, Vec<String>) = match state {
            Some(OnboardingState::NotNeeded) => (
                "Ready: the repository already follows the Yulius format.".into(),
                Vec::new(),
            ),
            Some(OnboardingState::Ready) => (
                "Ready: the preparation task adapted the repository.".into(),
                Vec::new(),
            ),
            Some(OnboardingState::Skipped) => (
                "Ready: an admin skipped the preparation.".into(),
                Vec::new(),
            ),
            Some(OnboardingState::Pending) => (
                format!("Not ready yet: {label} has not started."),
                vec![
                    format!("{label} starts when the upload finishes, or when someone starts it on the sandbox page"),
                    "the repository passes the Yulius format check".to_owned(),
                ],
            ),
            Some(OnboardingState::Preparing) => {
                let Some(task) = &task else {
                    return Ok(OnboardingStatus {
                        state: sandbox.onboarding_state.clone(),
                        ready: false,
                        summary: "Not ready yet: the preparation task is missing; an admin can skip preparation.".into(),
                        pending: vec!["an admin skips the preparation".into()],
                        task: view,
                    });
                };
                if task.status == yulius_domain::task::TaskStatus::WaitingCapacity {
                    (
                        format!("Preparing: {label} waits for a Claude subscription with capacity."),
                        vec![
                            "a Claude subscription has capacity (Admin → Accounts)".to_owned(),
                            format!("{label} adapts the repository"),
                            "the repository passes the Yulius format check".to_owned(),
                        ],
                    )
                } else if task.status.is_resting() {
                    let mut candidates: Vec<PathBuf> = Vec::new();
                    if let Some(path) = &task.worktree_path {
                        candidates.push(PathBuf::from(path));
                    }
                    candidates.push(self.sandbox_repo(&sandbox.id));
                    let problems = blocking(move || {
                        Ok(candidates
                            .iter()
                            .find(|dir| dir.is_dir())
                            .map(|dir| validate_checkout(dir).0)
                            .map(|report| {
                                report
                                    .problems
                                    .iter()
                                    .take(MAX_STATUS_PROBLEMS)
                                    .map(ToString::to_string)
                                    .collect::<Vec<_>>()
                            })
                            .unwrap_or_default())
                    })
                    .await?;
                    let mut pending = vec![format!(
                        "{label} is continued until the repository passes the Yulius format check"
                    )];
                    pending.extend(problems.into_iter().map(|p| format!("fix: {p}")));
                    (
                        format!("Preparing: {label} stopped before the repository passed the Yulius format check."),
                        pending,
                    )
                } else {
                    let steps = view
                        .as_ref()
                        .filter(|v| v.steps_total > 0)
                        .map(|v| format!(" (step {} of {})", (v.steps_done + 1).min(v.steps_total), v.steps_total))
                        .unwrap_or_default();
                    (
                        format!("Preparing: {label} is adapting the repository{steps}."),
                        vec![
                            format!("{label} finishes"),
                            "the repository passes the Yulius format check".to_owned(),
                        ],
                    )
                }
            }
            None => (
                format!("Unknown onboarding state {}.", sandbox.onboarding_state),
                Vec::new(),
            ),
        };
        Ok(OnboardingStatus {
            state: sandbox.onboarding_state.clone(),
            ready: !state.is_some_and(OnboardingState::blocks_tasks),
            summary,
            pending,
            task: view,
        })
    }

    /// `POST /api/sandboxes/{sid}/onboarding/approve` (write): starts the preparation task
    /// now (it normally starts by itself) and moves the sandbox to `preparing`.
    ///
    /// # Errors
    /// `Conflict` when nothing awaits preparation or the task cannot start.
    pub async fn approve_onboarding(
        self: &Arc<Self>,
        ctx: &AuthContext,
        sid: &str,
    ) -> AppResult<Sandbox> {
        let id = SandboxId::parse(sid).map_err(|_| AppError::NotFound)?;
        let (record, level) = self.require(&ctx.user, &id, Action::Work).await?;
        match OnboardingState::parse(&record.onboarding_state) {
            Some(OnboardingState::Pending) => {}
            Some(OnboardingState::Preparing) => return self.sandbox_view(&record, level).await,
            _ => {
                return Err(AppError::Conflict(format!(
                    "onboarding is {}; there is nothing to approve",
                    record.onboarding_state
                )));
            }
        }
        let actor = self.login_of(&ctx.user.id).await;
        self.start_onboarding(&id, &actor).await?;
        let record = self
            .deps
            .store
            .sandbox(&id)
            .await?
            .ok_or(AppError::NotFound)?;
        self.sandbox_view(&record, level).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    fn write(root: &Path, rel: &str, text: &str) -> TestResult {
        let path = root.join(rel);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(path, text)?;
        Ok(())
    }

    #[test]
    fn states_round_trip() {
        for state in [
            OnboardingState::NotNeeded,
            OnboardingState::Pending,
            OnboardingState::Preparing,
            OnboardingState::Ready,
            OnboardingState::Skipped,
        ] {
            assert_eq!(OnboardingState::parse(state.as_str()), Some(state));
        }
        assert!(OnboardingState::Pending.blocks_tasks());
        assert!(!OnboardingState::Skipped.blocks_tasks());
        assert_eq!(OnboardingState::parse("bogus"), None);
    }

    #[test]
    fn checkout_validation_reads_mapped_paths_and_ignores_links() -> TestResult {
        let dir = tempfile::tempdir()?;
        let root = dir.path();
        let (report, toml) = validate_checkout(root);
        assert!(!report.is_valid());
        assert!(toml.is_none());

        write(
            root,
            "yulius.toml",
            "[docs]\nproduct = \"README.md\"\ndecisions = \"knowledge/decisions.md\"\n",
        )?;
        write(root, "AGENTS.md", "# Rules\n\nRun the tests.\n")?;
        write(root, "CLAUDE.md", "See AGENTS.md.\n")?;
        write(root, "README.md", "# Shop\n\nSells things.\n")?;
        write(
            root,
            "docs/architecture.md",
            "# A\n\n```mermaid\nflowchart LR\n  A --> B\n```\n",
        )?;
        write(
            root,
            "knowledge/decisions.md",
            "# Decisions\n\n## Use SQLite\n\nYes.\n",
        )?;
        let outside = tempfile::tempdir()?;
        write(outside.path(), "changes.md", "# Changes\n")?;
        std::os::unix::fs::symlink(
            outside.path().join("changes.md"),
            root.join("docs/changes.md"),
        )?;
        let (report, toml) = validate_checkout(root);
        assert!(toml.is_some());
        let problems: Vec<String> = report.problems.iter().map(ToString::to_string).collect();
        assert_eq!(
            problems,
            vec!["docs/changes.md: is missing".to_owned()],
            "{problems:?}"
        );

        fs::remove_file(root.join("docs/changes.md"))?;
        write(root, "docs/changes.md", "# Changes\n")?;
        assert!(validate_checkout(root).0.is_valid());
        Ok(())
    }

    #[test]
    fn prompt_names_problems_profile_and_rules() -> TestResult {
        let dir = tempfile::tempdir()?;
        let (report, _) = validate_checkout(dir.path());
        let profile =
            crate::projects::profile_view(&yulius_domain::delivery::DeliveryProfile::product());
        let prompt = onboarding_prompt("Quartio-Studio/product", &profile, &report);
        assert!(prompt.contains("yulius.toml: is missing"));
        assert!(prompt.contains("preset \"product\""));
        assert!(prompt.contains("into quartio"));
        assert!(prompt.contains("git-crypt"));
        assert!(prompt.contains("reverse-engineer"));
        assert!(prompt.contains("--no-verify"));
        assert!(prompt.len() < crate::lifecycle::MAX_PROMPT_BYTES);
        Ok(())
    }
}
