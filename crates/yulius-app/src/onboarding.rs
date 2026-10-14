//! Sandbox onboarding: the preparation meta-task of imported projects (SPEC-001 DSC-010,
//! IMP-005, U-08; SPEC-002 §6.6, §8a).
//!
//! `not_needed | pending → preparing → ready | skipped`. An imported repository whose
//! documentation fails [`validate_project`] gets a meta-task whose prompt asks the agent to adapt
//! the repository to the Yulius format without breaking its conventions. The task starts at once
//! when the repository is added in the UI; one that came through `yulius onboard` waits until a
//! person clicks Start onboarding on the sandbox page in the browser (SPEC-051, ADR-0038: the
//! CLI only uploads, and a CLI token cannot start it). Until the task has finished and
//! the project validates (`ready`), or an admin skips it, no other task starts in the sandbox: a
//! new task — created, chained or run by an automation — waits after the preparation task and
//! starts by itself once the sandbox is ready ([`App::onboarding_wait`], U-08); continuing a
//! resting task or an imported session is refused meanwhile ([`App::ensure_tasks_may_start`]).
//! [`App::onboarding_status`] says what is still pending, for the CLI and the UI.

use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use yulius_contracts::api::sandboxes::{OnboardingPlan, Profile, Sandbox};
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
use yulius_domain::task::{Activity, TaskStatus};

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

/// The development-environment instruction every onboarding task starts with (F 2026-10-11:
/// the preparation agent had no Rust toolchain afterwards).
const ENVIRONMENT_SETUP: &str = "Set up this sandbox's development environment before anything \
else, so you and every later task can build and test with zero obstacles (F 2026-10-12: agents met \
missing toolchains that onboarding should have installed). First check what the sandbox already \
has (`node --version`, `cargo --version`, `python3 --version`, `gh --version`, ...). Then install \
each toolchain and system package the repository needs (its bootstrap script or docs; toolchain \
files such as rust-toolchain.toml, .nvmrc / package.json engines, .python-version / \
pyproject.toml, go.mod, Gemfile), with `sudo apt-get install` for system packages and the official \
installers for toolchains (for example rustup into your home: `curl https://sh.rustup.rs -sSf | sh \
-s -- -y`). ~/.cargo/bin and ~/.local/bin are on PATH for every run of yours; your home persists \
and is shared by every task's worktree, so install tools there once (never per worktree). Install \
what the tests need too (browsers for end-to-end tests, test fixtures, services). System packages \
(a browser's system libraries too) belong to the sandbox: install them now, never in a CI job, \
where an `apt-get` or a browser's `--with-deps` races the other CI run for the dpkg lock; jobs see \
what the sandbox installed. Then run the \
repository's test, lint and build commands until they work and record them in yulius.toml \
[commands]; run every \
`[[ci.jobs]]` command exactly as written, from the repository root, until it passes. Yulius CI \
gives the sandbox's jobs their own HOME and cache ($YULIUS_CI_CACHE), separate from yours: they \
start empty and persist across runs, as does each job's checkout per CI lane with its ignored \
files (build outputs, dependency directories); by default up to two runs at once share that HOME \
and cache, so a job's installs there must tolerate a concurrent run; and the job's PATH lacks that \
home's ~/.cargo/bin and ~/.local/bin. So a job must install what it needs itself (as \
`scripts/bootstrap.py --install-rustup` does) and never depend on anything an earlier run, your \
home or this worktree left behind: such a job passes on the cache and fails on a fresh sandbox or \
after a cache prune. Prove it: run each job command normally, then commit and run it once more \
from the root of a clean checkout with a fresh home and CI's environment, which prints its \
directory and the job's exit status and removes everything it made \
(`d=\"$(mktemp -d \"$HOME/.ci-proof.XXXXXX\")\"; echo \"proof $d\"; mkdir \"$d/home\" \
\"$d/cache\"; git worktree add --detach \"$d/repo\" HEAD && (cd \"$d/repo\" && env -i \
HOME=\"$d/home\" YULIUS_CI_CACHE=\"$d/cache\" PATH=/usr/local/bin:/usr/bin:/bin LANG=C.UTF-8 \
TERM=dumb NO_COLOR=1 CI=true GIT_TERMINAL_PROMPT=0 YULIUS_CI=1 sh -c '<command>' </dev/null); \
s=$?; git worktree remove --force \"$d/repo\"; chmod -R u+w \"$d\"; rm -rf \"$d\"; echo \"exit \
$s\"`, quoting the command as needed; for a matrix job add `YULIUS_CI_MATRIX=<value>` after \
`env -i`, once per matrix value). Run the proof with a long timeout or in the background: if it \
is killed, its cleanup never runs, so remove what it left at the path it printed \
(`d=<path>; git worktree remove --force \"$d/repo\"; chmod -R u+w \"$d\"; rm -rf \"$d\"`). Your \
home is shared with other tasks' running proofs, so remove theirs only when they started over six \
hours ago (`find \"$HOME\" -maxdepth 1 -name '.ci-proof.*' -mmin +360 -exec chmod -R u+w {} \\; \
-exec rm -rf {} \\;`), then run `git worktree prune`. A monorepo's jobs \
may run only its affected project's checks. End with a summary of what works and how it was \
verified; a check that still cannot run is a problem to report there, never a reason to skip it \
silently.";

/// Title of the onboarding task of a repository that already follows the Yulius format.
pub const ENVIRONMENT_TITLE: &str = "Set up this sandbox's environment";

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
         How to work (autonomously: decide, record why, and continue; nobody needs to approve):\n\
         1. Read the repository's own instructions first (AGENTS.md, CLAUDE.md, .agents/skills, \
         .claude/skills, CONTRIBUTING and README). They stay authoritative: follow them for \
         branches, commits, tests and reviews.\n\
         2. {environment}\n\
         3. Reverse-engineer the existing project into these documents: the product summary, an \
         architecture document and decision records derived from the code and its history, so \
         every later task knows the whole product before it builds anything.\n\
         4. Map instead of moving: when equivalent documents already exist (a README summary, an \
         architecture page, a decisions file, a changelog), point to them from yulius.toml [docs] \
         (product, architecture, decisions, specs_dir, changes, agents) instead of renaming or \
         duplicating them.\n\
         5. Add only what is missing: the product summary, an architecture document with at least \
         one ```mermaid diagram, decisions (a directory of NNNN-<slug>.md ADRs with Status, \
         Context, Decision, Alternatives and Consequences, or one file with a heading per \
         decision), the specs directory, the changes log, and a CLAUDE.md pointer to AGENTS.md. \
         Keep every existing rule in AGENTS.md; add a short pointer to the Yulius documents.\n\
         6. Record the delivery profile in yulius.toml [delivery]: preset \"{preset}\" \
         (integration branch {integration}, production branch {production}), and put the \
         repository's real test, lint and build commands in [commands].\n\
         7. Foundations (skill infra-bootstrap): CI runs only on our own machines through Yulius \
         CI, never GitHub Actions. Translate every .github/workflows/* job into the Yulius CI \
         configuration ([commands] gate or [[ci.jobs]] in yulius.toml; ci_setup lists them) so \
         nothing is lost, run it, then delete .github/workflows/ in the same pull request. If \
         GitHub refuses that push because the Yulius GitHub App may not change workflow files \
         (its Workflows permission), keep .github/workflows/, finish everything else, mark that \
         step skipped with this reason, and say in the pull request that the deletion follows \
         once an admin grants the permission. When tests, lint, a dead-code and unused-dependency \
         check (skill dead-code-cleanup names each language's tools) or environments are \
         missing, lay them as the infra-bootstrap skill describes, as follow-up pull requests.\n\
         8. git-crypt: encrypted files stay locked. Never unlock or decrypt them, never commit \
         decrypted content, and leave encrypted files byte-for-byte unchanged.\n\
         9. Hooks: when the repository's instructions describe a hooks setup (for example \
         `git config core.hooksPath .githooks`), apply it in your worktree before committing. \
         Never bypass hooks with --no-verify.\n\
         10. Commit on your task branch and mark every step done as you finish it: Yulius opens \
         the pull request into {integration} by itself once every step is done, and pushes later \
         commits to it. Do not open it yourself, and never push to {integration} or \
         {production}. Change documentation, configuration and CI only, not application code.\n\
         11. Before finishing, make sure every problem listed above is resolved.",
        preset = profile.preset,
        problems = problems.join("\n"),
        environment = ENVIRONMENT_SETUP,
    )
}

/// The onboarding task of a repository that already follows the Yulius format: only the
/// sandbox's development environment (F 2026-10-11: "on onboarding every environmental things
/// should be installed").
#[must_use]
pub fn environment_prompt(repo: &str, profile: &Profile) -> String {
    let integration = &profile.integration_branch;
    let production = &profile.production_branch;
    format!(
        "Set up the sandbox of the repository {repo} for work.\n\n\
         The repository already follows the Yulius documentation format; what this sandbox \
         still needs is its development environment.\n\n\
         1. Read the repository's own instructions first (AGENTS.md, CLAUDE.md, README, \
         CONTRIBUTING and the development docs). They stay authoritative.\n\
         2. {ENVIRONMENT_SETUP}\n\
         3. Change only what the environment needs (yulius.toml [commands], a missing setup \
         note in the docs), never application code. When you changed something, commit it on \
         your task branch and mark every step done: Yulius opens the pull request into \
         {integration} by itself. Do not open it yourself, and never push to {integration} or \
         {production}. When nothing needed to change, just mark the steps done."
    )
}

/// Initial steps of the environment-only onboarding task.
fn environment_steps() -> Vec<String> {
    [
        "Read the repository's instructions and conventions",
        "Install the toolchains and run the repository's own checks",
        "Record the working commands in yulius.toml [commands]",
    ]
    .iter()
    .map(|step| (*step).to_owned())
    .collect()
}

/// Initial steps of the preparation task.
fn onboarding_steps() -> Vec<String> {
    [
        "Read the repository's instructions and conventions",
        "Install the toolchains and run the repository's own checks",
        "Reverse-engineer the product, architecture and decisions into the docs",
        "Map existing documents and commands in yulius.toml",
        "Move GitHub Actions into Yulius CI and delete them",
        "Add the missing documents and check the format",
    ]
    .iter()
    .map(|step| (*step).to_owned())
    .collect()
}

/// The onboarding task's title and steps for a repository: the environment alone when it already
/// follows the format, else the whole preparation.
fn planned(report: &ProjectReport) -> (&'static str, Vec<String>) {
    if report.is_valid() {
        (ENVIRONMENT_TITLE, environment_steps())
    } else {
        (ONBOARDING_TITLE, onboarding_steps())
    }
}

/// Refuses a caller that is not a person in the browser: onboarding starts only from the sandbox
/// page (SPEC-051 OBD-002, ADR-0038).
fn ensure_browser(ctx: &AuthContext) -> AppResult<()> {
    if ctx.is_browser() {
        Ok(())
    } else {
        Err(AppError::Forbidden(
            "Start onboarding on the sandbox page in the browser; the CLI only uploads".into(),
        ))
    }
}

impl App {
    /// Continuing the onboarding task of a `pending` sandbox starts onboarding, so it needs a
    /// person in the browser too (SPEC-051 OBD-002); every other task is unaffected.
    ///
    /// # Errors
    /// `Forbidden` for a bearer token; `NotFound` for unknown sandboxes.
    pub(crate) async fn ensure_onboarding_continue_allowed(
        &self,
        ctx: &AuthContext,
        task: &TaskRecord,
    ) -> AppResult<()> {
        if task.origin != TaskOrigin::Onboarding || ctx.is_browser() {
            return Ok(());
        }
        let record = self
            .deps
            .store
            .sandbox(&task.sandbox_id)
            .await?
            .ok_or(AppError::NotFound)?;
        let starts_onboarding = OnboardingState::parse(&record.onboarding_state)
            == Some(OnboardingState::Pending)
            && record.onboarding_task_id.as_ref() == Some(&task.id);
        if starts_onboarding {
            ensure_browser(ctx)?;
        }
        Ok(())
    }

    /// Creates the preparation meta-task of an imported sandbox: resting (not started) and kept
    /// in the active task list. Called only when onboarding starts or a task waits for it, so a
    /// sandbox nobody has started onboarding has no task (SPEC-097).
    ///
    /// # Errors
    /// Store failures.
    async fn create_onboarding_task(
        self: &Arc<Self>,
        sandbox: &SandboxRecord,
        user: &UserRecord,
        report: &ProjectReport,
    ) -> AppResult<TaskRecord> {
        let profile = self.profile_of(sandbox);
        let repo = format!("{}/{}", sandbox.repo_owner, sandbox.repo_name);
        let (title, steps) = planned(report);
        let prompt = if report.is_valid() {
            environment_prompt(&repo, &profile)
        } else {
            onboarding_prompt(&repo, &profile, report)
        };
        let task = self
            .create_task_internal(NewTask {
                sandbox: sandbox.id.clone(),
                created_by: user.id.clone(),
                prompt,
                harness: Harness::ClaudeCode,
                title: Some(title.to_owned()),
                workflow_id: None,
                steps,
                model: None,
                effort: None,
                origin: TaskOrigin::Onboarding,
                automation_run: None,
                start: false,
                // A goal task ends Done when its work is merged (SPEC-011); in prompt mode a
                // finished preparation sat at "Your turn" and showed as needing a person
                // (F 2026-10-11, task #1).
                mode: crate::model::TaskMode::Goal,
                supertask: false,
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
    /// state: `preparing` once the preparation task has been started (Start onboarding, or
    /// continuing it in the browser), `ready` once it has finished — it rests after running,
    /// or its turn ended with every step of its plan resolved — and the project validates in its
    /// worktree or in the sandbox clone. Other states are returned unchanged.
    /// Cheap for settled sandboxes (one store read); call it on task close and on reads.
    ///
    /// It also repairs what would block the sandbox for good (SPEC-106): a `preparing` sandbox
    /// whose onboarding task is gone (deleted) returns to `pending`, so Start onboarding creates
    /// a new one; an archived onboarding task is listed again.
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
        let task = match &record.onboarding_task_id {
            Some(id) => store.task(id).await?,
            None => None,
        };
        let Some(task) = task else {
            if state != OnboardingState::Preparing {
                return Ok(record.onboarding_state);
            }
            // Under the lock skip and task creation take, read again: an admin's skip landing
            // meanwhile is never written back to `pending`.
            let _serial = self.onboarding_task_lock.lock().await;
            let record = store.sandbox(sandbox).await?.ok_or(AppError::NotFound)?;
            let task = match &record.onboarding_task_id {
                Some(id) => store.task(id).await?,
                None => None,
            };
            if task.is_some()
                || OnboardingState::parse(&record.onboarding_state)
                    != Some(OnboardingState::Preparing)
            {
                return Ok(record.onboarding_state);
            }
            self.set_onboarding_state(&record, OnboardingState::Pending, "system")
                .await?;
            return Ok(OnboardingState::Pending.as_str().to_owned());
        };
        if task.archived_at.is_some() {
            store
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
        }
        let resting = task.status.is_resting();
        let started = !resting || !store.attempts(&task.id).await?.is_empty();
        if !started {
            return Ok(record.onboarding_state);
        }
        // An agent that has done the work waits at its prompt: a finished turn with the whole
        // plan resolved counts as finished, so the sandbox does not wait for someone to close it.
        let finished = resting
            || (task.status == TaskStatus::Running
                && task.activity.is_some_and(Activity::is_idle)
                && {
                    let steps = store.steps(&task.id).await?;
                    !steps.is_empty() && !crate::sandboxes::plan_open(&steps)
                });
        let mut next = OnboardingState::Preparing;
        if finished {
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

    /// Repairs, at server start, every sandbox whose onboarding was left stuck before SPEC-106
    /// (OTR-005): see [`App::refresh_onboarding`]. Returns how many it repaired (an onboarding
    /// task listed again, or a sandbox back to `pending`); one sandbox's failure is logged and
    /// the others are still repaired.
    ///
    /// # Errors
    /// The sandbox list cannot be read.
    pub async fn heal_onboarding(&self) -> AppResult<u32> {
        let store = &self.deps.store;
        let mut healed = 0;
        for record in store.sandboxes().await? {
            if !OnboardingState::parse(&record.onboarding_state)
                .is_some_and(OnboardingState::blocks_tasks)
            {
                continue;
            }
            let archived = match &record.onboarding_task_id {
                Some(id) => store
                    .task(id)
                    .await
                    .ok()
                    .flatten()
                    .is_some_and(|t| t.archived_at.is_some()),
                None => false,
            };
            match self.refresh_onboarding(&record.id).await {
                Ok(state) => {
                    let reset = state == OnboardingState::Pending.as_str()
                        && record.onboarding_state != state;
                    if archived || reset {
                        tracing::info!(sandbox = %record.id, state, "repaired a stuck onboarding");
                        healed += 1;
                    }
                }
                Err(err) => {
                    tracing::warn!(sandbox = %record.id, error = %err, "could not repair onboarding");
                }
            }
        }
        Ok(healed)
    }

    /// Refuses archiving the onboarding task while its sandbox waits for it: archived, it left
    /// the page's task list and the sandbox looked stuck (SPEC-106 OTR-004). Onboarding is
    /// refreshed first, so a task that has finished its work is archived as before.
    ///
    /// # Errors
    /// `Conflict` for the onboarding task of a blocking sandbox; store failures.
    pub(crate) async fn ensure_archive_allowed(&self, task: &TaskRecord) -> AppResult<()> {
        if task.origin != TaskOrigin::Onboarding {
            return Ok(());
        }
        self.refresh_onboarding(&task.sandbox_id).await?;
        let Some(record) = self.deps.store.sandbox(&task.sandbox_id).await? else {
            return Ok(());
        };
        let waits_for_it = record.onboarding_task_id.as_ref() == Some(&task.id)
            && OnboardingState::parse(&record.onboarding_state)
                .is_some_and(OnboardingState::blocks_tasks);
        if waits_for_it {
            return Err(AppError::Conflict(
                "the project waits for this onboarding task, so it stays in the list: continue \
                 it, or delete it to start onboarding again"
                    .into(),
            ));
        }
        Ok(())
    }

    /// `POST /api/sandboxes/{sid}/onboarding/skip` (admin): unblocks ordinary tasks — the ones
    /// waiting for the preparation task start at once; a running preparation task keeps running.
    ///
    /// # Errors
    /// `NotFound` without access, `Forbidden` without admin.
    pub async fn skip_onboarding(
        self: &Arc<Self>,
        ctx: &AuthContext,
        sid: &str,
    ) -> AppResult<Sandbox> {
        let id = SandboxId::parse(sid).map_err(|_| AppError::NotFound)?;
        let (_, level) = self.require(&ctx.user, &id, Action::Administer).await?;
        let actor = self.login_of(&ctx.user.id).await;
        // Under the creation lock, so an onboarding task created meanwhile never writes `pending`
        // back over the skip (SPEC-097); released before the waiting tasks start.
        let skipped = {
            let _serial = self.onboarding_task_lock.lock().await;
            let record = self
                .deps
                .store
                .sandbox(&id)
                .await?
                .ok_or(AppError::NotFound)?;
            let blocks = OnboardingState::parse(&record.onboarding_state)
                .is_some_and(OnboardingState::blocks_tasks);
            if blocks {
                self.set_onboarding_state(&record, OnboardingState::Skipped, &actor)
                    .await?;
            }
            blocks.then_some(record.onboarding_task_id).flatten()
        };
        if let Some(task) = &skipped {
            self.advance_chains(task).await;
        }
        let record = self
            .deps
            .store
            .sandbox(&id)
            .await?
            .ok_or(AppError::NotFound)?;
        self.sandbox_view(&record, level).await
    }

    /// The onboarding gate for new tasks (U-08): while a sandbox is `pending` or `preparing`, a
    /// new task does not start but waits after the preparation task, which this returns; it
    /// starts by itself once the sandbox is ready or an admin skips preparation. `None` when the
    /// task may start now.
    ///
    /// # Errors
    /// `Conflict` when the sandbox waits for preparation without a preparation task;
    /// `NotFound` for unknown sandboxes.
    pub async fn onboarding_wait(
        self: &Arc<Self>,
        sandbox: &SandboxId,
        origin: TaskOrigin,
    ) -> AppResult<Option<TaskRecord>> {
        if origin == TaskOrigin::Onboarding {
            return Ok(None);
        }
        let state = self.refresh_onboarding(sandbox).await?;
        if !OnboardingState::parse(&state).is_some_and(OnboardingState::blocks_tasks) {
            return Ok(None);
        }
        let store = &self.deps.store;
        let record = store.sandbox(sandbox).await?.ok_or(AppError::NotFound)?;
        let task = match &record.onboarding_task_id {
            Some(id) => store.task(id).await?,
            None => None,
        };
        // Nobody started onboarding yet, so it has no task; the new task waits after one created
        // now, resting (SPEC-097 OBT-003).
        let task = match task {
            None if OnboardingState::parse(&record.onboarding_state)
                == Some(OnboardingState::Pending) =>
            {
                // Boxed: creating that task goes through task creation, which called this.
                match Box::pin(self.onboarding_task(sandbox)).await? {
                    Some(task) => Some(task),
                    // Skipped meanwhile: the new task may start now.
                    None => return Ok(None),
                }
            }
            task => task,
        };
        task.map(Some).ok_or_else(|| {
            AppError::Conflict(
                "this project is being prepared for Yulius; tasks can start once it is ready (or \
                 an admin skips preparation)"
                    .into(),
            )
        })
    }

    /// The onboarding gate for resuming (U-08): continuing a resting task or an imported session
    /// is refused while the sandbox is `pending` or `preparing`; new tasks wait instead
    /// ([`App::onboarding_wait`]).
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
                "this project is being prepared for Yulius; continue this once the preparation \
                 task has made it ready (or an admin skips it). New tasks wait for it and start by \
                 themselves"
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
        let Some(task) = self.onboarding_task(sandbox).await? else {
            return Ok(false);
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

    /// The onboarding task of a sandbox, created (resting) by its sandbox's creator when it has
    /// none yet and is still `pending`: the repository's preparation, or its environment alone
    /// when it already follows the format. `None` when it has none and is no longer `pending`
    /// (an admin skipped meanwhile): nothing is created and its state is left as it is.
    ///
    /// # Errors
    /// `NotFound` when the creator is gone; store failures.
    async fn onboarding_task(
        self: &Arc<Self>,
        sandbox: &SandboxId,
    ) -> AppResult<Option<TaskRecord>> {
        let _serial = self.onboarding_task_lock.lock().await;
        let store = &self.deps.store;
        let record = store.sandbox(sandbox).await?.ok_or(AppError::NotFound)?;
        if let Some(id) = &record.onboarding_task_id
            && let Some(task) = store.task(id).await?
        {
            return Ok(Some(task));
        }
        if OnboardingState::parse(&record.onboarding_state) != Some(OnboardingState::Pending) {
            return Ok(None);
        }
        let root = self.sandbox_repo(&record.id);
        let (report, _) = blocking(move || Ok(validate_checkout(&root))).await?;
        let creator = store
            .user(&record.created_by)
            .await?
            .ok_or(AppError::NotFound)?;
        let task = self
            .create_onboarding_task(&record, &creator, &report)
            .await?;
        store
            .set_onboarding(
                &record.id,
                OnboardingState::Pending.as_str(),
                Some(&task.id),
            )
            .await?;
        Ok(Some(task))
    }

    /// `GET /api/sandboxes/{sid}/onboarding/plan` (read): what onboarding does — its task's title
    /// and steps, or, while that task does not exist yet, the ones it would be created with. Reads
    /// only; creates nothing (SPEC-097 OBT-004).
    ///
    /// # Errors
    /// `NotFound` without access; store failures and a failed read of the checkout.
    pub async fn onboarding_plan(&self, ctx: &AuthContext, sid: &str) -> AppResult<OnboardingPlan> {
        let id = SandboxId::parse(sid).map_err(|_| AppError::NotFound)?;
        let (record, _) = self.require(&ctx.user, &id, Action::View).await?;
        let store = &self.deps.store;
        let task = match &record.onboarding_task_id {
            Some(task) => store.task(task).await?,
            None => None,
        };
        if let Some(task) = task {
            let steps = store.steps(&task.id).await?;
            return Ok(OnboardingPlan {
                title: task.title,
                steps: steps.into_iter().map(|step| step.title).collect(),
                task_number: Some(task.number),
            });
        }
        let root = self.sandbox_repo(&record.id);
        let (report, _) = blocking(move || Ok(validate_checkout(&root))).await?;
        let (title, steps) = planned(&report);
        Ok(OnboardingPlan {
            title: title.to_owned(),
            steps,
            task_number: None,
        })
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
        let label = task.as_ref().map_or_else(
            || "the preparation task".to_owned(),
            |t| format!("task #{}", t.number),
        );
        let (summary, pending): (String, Vec<String>) = match state {
            Some(OnboardingState::NotNeeded) => (
                "Ready: the repository already follows the Yulius format.".into(),
                Vec::new(),
            ),
            Some(OnboardingState::Ready) => {
                ("Ready: the preparation task finished.".into(), Vec::new())
            }
            Some(OnboardingState::Skipped) => (
                "Ready: an admin skipped the preparation.".into(),
                Vec::new(),
            ),
            // A person starts onboarding in the browser, never the CLI (SPEC-051 OBD-003).
            Some(OnboardingState::Pending) => (
                format!("Not ready yet: {label} waits for someone to start onboarding."),
                vec![
                    format!(
                        "someone clicks Start onboarding on the sandbox page: {}/s/{}",
                        self.config.public_url, sandbox.slug
                    ),
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
                        format!(
                            "Preparing: {label} waits for a Claude subscription with capacity."
                        ),
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
                        format!(
                            "Preparing: {label} stopped before the repository passed the Yulius format check."
                        ),
                        pending,
                    )
                } else {
                    let steps = view
                        .as_ref()
                        .filter(|v| v.steps_total > 0)
                        .map(|v| {
                            format!(
                                " (step {} of {})",
                                (v.steps_done + 1).min(v.steps_total),
                                v.steps_total
                            )
                        })
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

    /// `POST /api/sandboxes/{sid}/onboarding/approve` (write, browser only): Start onboarding —
    /// starts the onboarding task and moves the sandbox to `preparing`. A person starts it in the
    /// browser; a bearer token (the CLI) is refused before anything is read (SPEC-051 OBD-002).
    ///
    /// # Errors
    /// `Forbidden` without a browser session; `Conflict` when nothing awaits preparation or the
    /// task cannot start.
    pub async fn approve_onboarding(
        self: &Arc<Self>,
        ctx: &AuthContext,
        sid: &str,
    ) -> AppResult<Sandbox> {
        ensure_browser(ctx)?;
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
        assert!(prompt.contains("Reverse-engineer"));
        assert!(prompt.contains("nobody needs to approve"));
        assert!(!prompt.contains("Ask the user"), "onboarding is autonomous");
        assert!(prompt.contains("delete .github/workflows/"));
        // F 2026-10-11: the agent never needs the GitHub CLI and never stops on a permission.
        assert!(prompt.contains("Yulius opens the pull request"));
        assert!(prompt.contains("Do not open it yourself"));
        assert!(prompt.contains("Workflows permission"));
        // F 2026-10-11: the preparation agent had no Rust toolchain afterwards.
        assert!(prompt.contains("development environment") && prompt.contains("rustup"));
        assert!(
            onboarding_steps()
                .iter()
                .any(|step| step.contains("toolchains"))
        );
        assert!(
            !onboarding_steps()
                .iter()
                .any(|step| step.contains("open the pull request"))
        );
        assert!(prompt.contains("infra-bootstrap"));
        // F 2026-10-13 (SPEC-054 DCT-002): onboarding sets up a dead-code check with its tools.
        assert!(prompt.contains("dead-code-cleanup"));
        assert!(prompt.contains("--no-verify"));
        assert!(prompt.len() < crate::lifecycle::MAX_PROMPT_BYTES);
        Ok(())
    }

    /// ONB-001: both onboarding prompts describe the CI home as `ci/exec.rs` sets it up — its
    /// own, starting empty, persistent across runs — and ask for the fresh-home proof.
    #[test]
    fn prompts_describe_the_persistent_ci_home_and_the_fresh_home_proof() -> TestResult {
        let dir = tempfile::tempdir()?;
        let (report, _) = validate_checkout(dir.path());
        let profile =
            crate::projects::profile_view(&yulius_domain::delivery::DeliveryProfile::product());
        for prompt in [
            onboarding_prompt("Quartio-Studio/product", &profile, &report),
            environment_prompt("Quartio-Studio/product", &profile),
        ] {
            assert!(!prompt.contains("empty home"), "the CI home persists");
            for wording in [
                "separate from yours",
                "start empty and persist across runs",
                "$YULIUS_CI_CACHE",
                "ignored files",
                "install what it needs itself",
                "never depend on anything an earlier run",
                "PATH lacks that home's ~/.cargo/bin",
                "run each job command normally",
                "git worktree add --detach",
                "env -i HOME=\"$d/home\" YULIUS_CI_CACHE=\"$d/cache\"",
                // The proof reports the job's status, not the cleanup's, and leaves nothing.
                "</dev/null); s=$?;",
                "rm -rf \"$d\"; echo \"exit $s\"",
                // #272: matrix jobs read their value, which `env -i` would drop.
                "YULIUS_CI_MATRIX=<value>",
                "once per matrix value",
                // #272: a killed proof never cleans up after itself.
                "long timeout or in the background",
                // Its own leftovers by the path it printed; others' only when stale, since the
                // home is shared with other tasks' running proofs.
                "echo \"proof $d\";",
                "at the path it printed",
                "d=<path>; git worktree remove --force \"$d/repo\"",
                "find \"$HOME\" -maxdepth 1 -name '.ci-proof.*' -mmin +360",
                "git worktree prune",
                // #272 (#238): checkouts are per lane, the CI home and cache are shared.
                "checkout per CI lane",
                "by default up to two runs at once share",
                "tolerate a concurrent run",
                // #272 (#299, #300): an apt-get in a job raced the other lane for the dpkg lock.
                "belong to the sandbox: install them now, never in a CI job",
            ] {
                assert!(prompt.contains(wording), "missing {wording:?}");
            }
            assert!(prompt.len() < crate::lifecycle::MAX_PROMPT_BYTES);
        }
        // #272: infra-bootstrap step 2 gives the same proof to every later task.
        let skill = crate::skills::pack::PACKS
            .iter()
            .flat_map(|pack| pack.files.iter())
            .find(|(rel, _)| *rel == "infra-bootstrap/SKILL.md")
            .map(|(_, bytes)| {
                String::from_utf8_lossy(bytes)
                    .split_whitespace()
                    .collect::<Vec<_>>()
                    .join(" ")
            })
            .ok_or("infra-bootstrap is bundled")?;
        for wording in [
            "git worktree add --detach",
            "YULIUS_CI_CACHE=\"$d/cache\"",
            "YULIUS_CI_MATRIX=<value>",
            "once per matrix value",
            "long timeout or in the background",
            "echo \"proof $d\";",
            "at the path it printed",
            "d=<path>; git worktree remove --force \"$d/repo\"",
            "find \"$HOME\" -maxdepth 1 -name '.ci-proof.*' -mmin +360",
            "git worktree prune",
            "checkout per CI lane",
            "default up to two runs at once share",
            "tolerate a concurrent run",
            "never in a CI job",
        ] {
            assert!(
                skill.contains(wording),
                "infra-bootstrap misses {wording:?}"
            );
        }
        Ok(())
    }
}
