//! Agent tools for CI and repository hygiene (SPEC-002 §7.4): `ci_status`, `ci_rerun`,
//! `ci_setup` (onboarding: move GitHub Actions into Yulius CI), `repo_hygiene` and
//! `repo_cleanup`. Agents act as `task:<number>`; every removal is audited.

use std::ffi::OsStr;
use std::io::Read as _;
use std::os::fd::OwnedFd;
use std::path::{Component, Path};

use nix::fcntl::{OFlag, openat};
use nix::sys::stat::Mode;

use serde_json::{Value, json};
use yulius_contracts::api::ci::{HygieneCleanup, HygieneTarget};
use yulius_domain::ci::actions::{MAX_WORKFLOW_BYTES, summarize_workflow};
use yulius_domain::ci::{self as dom};
use yulius_domain::spec_format::{MAX_CONFIG_BYTES, parse_yulius_toml};

use super::{MAX_WORKFLOW_FILES, WORKFLOWS_DIR, run_view};
use crate::context::App;
use crate::coordination::AgentContext;
use crate::error::{AppError, AppResult};
use crate::files;
use crate::model::SandboxRecord;

/// Output tail per job returned to an agent.
const TOOL_OUTPUT_BYTES: usize = 16 * 1024;
/// Runs listed by `ci_status`.
const TOOL_RUNS: u32 = 10;

/// How an agent moves a project's GitHub Actions into Yulius CI (F 2026-10-10: Yulius is the
/// only CI; no GitHub Actions).
pub const CI_SETUP_STEPS: &[&str] = &[
    "Read every workflow below (and the files themselves) and list what each job checks: its run steps, matrix and the tools its `uses:` steps install.",
    "Write the checks as `[[ci.jobs]]` entries in yulius.toml (name, command, optional matrix = \"a b c\" passed as $YULIUS_CI_MATRIX, optional cpus = N and memory_gb = N when a job needs more or less than an equal share of the machine, optional timeout_minutes = N (1-360) for a job that runs longer than the instance's limit, optional exclusive = true for a job two runs must not run at once); one job per independent check so they run in parallel; prefer the repository's own scripts. A single command may instead be `[commands] gate`.",
    "Size builds and test runners to the budget Yulius gives each job and agent session, not to the machine: they read $YULIUS_CPUS and $YULIUS_MEMORY_MB, and `[resources.env]` in yulius.toml sets the tools' own variables from them (for example CARGO_BUILD_JOBS = \"{cpus}\", PYTEST_XDIST_AUTO_NUM_WORKERS = \"{cpus}\", MAKEFLAGS = \"-j{cpus}\").",
    "Make each command work on the Yulius machine: no GitHub expressions (${{ }}) or services from Actions; a secret becomes an agent secret the job names with `secrets = \"NAME\"` (it gets a placeholder through the egress proxy, never the value, and only once the integration branch has the line; SPEC-142); tools a `uses:` step installed must be installed by the command or be on the machine; downloads go under $HOME (shared by the sandbox's concurrent CI runs) and build outputs in the checkout (a Cargo target directory: Yulius sweeps its unused build units) or under $YULIUS_CI_CACHE (the job's own, never shared with a job running at the same time, pruned whole when the disk runs low), which persist between runs.",
    "Delete .github/workflows/ entirely (git rm -r .github/workflows) in the same commit, so no GitHub Actions run any more; commit it and let Yulius push it at the end of your turn (your own token may not change workflow files, ADR-0114).",
    "Commit and let the pull request open: Yulius tests it with the CI it adds (bootstrap). Check the result with ci_status and fix failures until it passes; merging makes it the project's CI.",
];

fn tail(text: &str, max: usize) -> String {
    if text.len() <= max {
        return text.to_owned();
    }
    let mut start = text.len() - max;
    while !text.is_char_boundary(start) {
        start += 1;
    }
    format!("…{}", &text[start..])
}

impl App {
    async fn tool_sandbox(&self, ctx: &AgentContext) -> AppResult<SandboxRecord> {
        self.deps
            .store
            .sandbox(&ctx.task.sandbox_id)
            .await?
            .filter(|s| s.removed_at.is_none())
            .ok_or(AppError::NotFound)
    }

    /// `ci_status`: the CI in force, recent runs (this task's pull request first) or, with
    /// `run_id`, one run with the tail of each job's output and its failing tests' output
    /// (SPEC-145).
    ///
    /// # Errors
    /// Unknown runs, store failures.
    pub async fn tool_ci_status(&self, ctx: &AgentContext, args: &Value) -> AppResult<Value> {
        let sandbox = self.tool_sandbox(ctx).await?;
        if let Some(run_id) = args.get("run_id").and_then(Value::as_str) {
            let run = self.ci_run_of(&sandbox, run_id).await?;
            let mut jobs = self.deps.store.ci_jobs(&run.id, true).await?;
            for job in &mut jobs {
                job.output = tail(&job.output, TOOL_OUTPUT_BYTES);
            }
            return serde_json::to_value(run_view(&run, &jobs, true)).map_err(AppError::internal);
        }
        let view = self.ci_gate(&sandbox).await;
        let pr = self
            .deps
            .store
            .delivery(&ctx.task.id)
            .await?
            .and_then(|record| record.pr_number);
        let mut runs = Vec::new();
        for run in self.deps.store.ci_runs(&sandbox.id, TOOL_RUNS).await? {
            let jobs = self.deps.store.ci_jobs(&run.id, false).await?;
            // Its pull request's runs and the merge trains it is in (SPEC-084), or in a playground
            // the runs of its head merged into the integration branch (SPEC-163).
            let mine = pr.is_some_and(|n| run.reference.covers(n))
                || run.reference == yulius_domain::ci::CiRef::Task(u64::from(ctx.task.number));
            let mut value =
                serde_json::to_value(run_view(&run, &jobs, false)).map_err(AppError::internal)?;
            value["this_task"] = Value::Bool(mine);
            runs.push(value);
        }
        runs.sort_by_key(|run| !run["this_task"].as_bool().unwrap_or(false));
        Ok(json!({
            "ci": view.gate.as_ref().map(|gate| json!({
                "source": gate.source.as_str(),
                "jobs": gate.jobs.iter().map(|j| json!({"name": j.name, "command": j.command, "matrix": j.matrix, "timeout_minutes": j.timeout_minutes, "exclusive": j.exclusive})).collect::<Vec<_>>(),
            })),
            "problem": view.problem,
            "github_actions_workflows": view.workflows,
            "pull_request": pr,
            "playground": sandbox.is_playground(),
            "runs": runs,
            "note": if sandbox.is_playground() {
                "delivery_merge squashes once the run of your head merged into the integration branch passed. Read a run's output with run_id."
            } else {
                "Merges that require checks wait for the latest run at the pull request head to pass. Read a run's output with run_id."
            },
        }))
    }

    /// `ci_rerun`: queues the commit of `run_id` again.
    ///
    /// # Errors
    /// Unknown runs, an active run of that commit, no CI.
    pub async fn tool_ci_rerun(&self, ctx: &AgentContext, args: &Value) -> AppResult<Value> {
        let sandbox = self.tool_sandbox(ctx).await?;
        let run_id = args
            .get("run_id")
            .and_then(Value::as_str)
            .ok_or_else(|| AppError::Invalid("run_id is required".into()))?;
        let run = self.ci_rerun(&sandbox, run_id, &ctx.actor()).await?;
        let jobs = self.deps.store.ci_jobs(&run.id, false).await?;
        serde_json::to_value(run_view(&run, &jobs, false)).map_err(AppError::internal)
    }

    /// `ci_setup`: what onboarding needs to move GitHub Actions into Yulius CI: the workflows
    /// in this task's worktree (jobs, run steps, `uses:` steps, matrices), the CI configured in
    /// its `yulius.toml` and on the integration branch, and the steps.
    ///
    /// # Errors
    /// A task without a worktree on disk inside the sandbox directory (`Conflict`), or one that
    /// cannot be opened from it without following a link.
    pub async fn tool_ci_setup(&self, ctx: &AgentContext) -> AppResult<Value> {
        let sandbox = self.tool_sandbox(ctx).await?;
        // Resolved and inside the sandbox directory, then read only through handles from it.
        let worktree = self
            .sandbox_worktree(&sandbox, ctx.task.id.as_str(), None)
            .await?;
        let sandbox_dir = self.sandbox_dir(&sandbox.id);
        let (workflows, config) =
            tokio::task::spawn_blocking(move || read_setup(&sandbox_dir, &worktree))
                .await
                .map_err(AppError::internal)??;
        let config = match config {
            None => json!({"jobs": [], "problem": null, "file": false}),
            Some(text) => match parse_yulius_toml(&text)
                .map_err(|e| e.to_string())
                .and_then(|c| {
                    dom::project_jobs(c.commands.gate.as_deref(), &c.ci).map_err(|e| e.to_string())
                }) {
                Ok(jobs) => json!({
                    "file": true,
                    "jobs": jobs.iter().map(|j| json!({"name": j.name, "command": j.command, "matrix": j.matrix})).collect::<Vec<_>>(),
                    "problem": null,
                }),
                Err(problem) => json!({"file": true, "jobs": [], "problem": problem}),
            },
        };
        let base = self.ci_gate(&sandbox).await;
        let done = workflows.is_empty()
            && config["jobs"]
                .as_array()
                .is_some_and(|jobs| !jobs.is_empty());
        Ok(json!({
            "github_actions_workflows": workflows,
            "worktree_ci": config,
            "integration_branch_ci": base.gate.as_ref().map(|g| g.jobs.len()),
            "done_in_this_worktree": done,
            "steps": CI_SETUP_STEPS,
            "format": "[[ci.jobs]]\nname = \"test\"\ncommand = \"cargo test\"\nmatrix = \"1/2 2/2\"   # optional",
        }))
    }

    /// `repo_hygiene`: stale branches and worktrees of this project (`refresh` scans now).
    ///
    /// # Errors
    /// Store failures.
    pub async fn tool_repo_hygiene(&self, ctx: &AgentContext, args: &Value) -> AppResult<Value> {
        let sandbox = self.tool_sandbox(ctx).await?;
        let report = if args
            .get("refresh")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        {
            self.hygiene_scan_locked(&sandbox).await
        } else {
            self.hygiene_report(&sandbox).await
        };
        serde_json::to_value(report).map_err(AppError::internal)
    }

    /// `repo_cleanup`: removes stale items (all safe ones without `items`).
    ///
    /// # Errors
    /// Invalid arguments.
    pub async fn tool_repo_cleanup(&self, ctx: &AgentContext, args: &Value) -> AppResult<Value> {
        let sandbox = self.tool_sandbox(ctx).await?;
        let items: Vec<HygieneTarget> = match args.get("items") {
            None | Some(Value::Null) => Vec::new(),
            Some(value) => serde_json::from_value(value.clone()).map_err(|_| {
                AppError::Invalid("items must be a list of {kind, name} from repo_hygiene".into())
            })?,
        };
        let request = HygieneCleanup {
            items,
            force: args.get("force").and_then(Value::as_bool).unwrap_or(false),
        };
        let result = self
            .hygiene_cleanup_as(&sandbox, &request, &ctx.actor())
            .await?;
        serde_json::to_value(result).map_err(AppError::internal)
    }
}

/// The workflows and the `yulius.toml` text of the task worktree `worktree` (canonical, inside
/// the sandbox directory `sandbox_dir`). The server reads them outside containment, and the
/// contained agent can swap any folder or file there for a link at any moment, so nothing is read
/// by path: the worktree is opened from the sandbox directory folder by folder with `O_NOFOLLOW`,
/// and every folder and file below it from its parent's handle the same way (#829, ADR-0174). A
/// link below the worktree reads nothing; blocks.
///
/// # Errors
/// `Conflict` when the worktree cannot be opened that way (a link or a missing folder on the way,
/// or it is not inside the sandbox directory).
fn read_setup(sandbox_dir: &Path, worktree: &Path) -> AppResult<(Vec<Value>, Option<String>)> {
    let unreadable = || {
        AppError::Conflict(
            "the task worktree cannot be read: a link or a missing folder on the way".into(),
        )
    };
    let anchor = std::fs::canonicalize(sandbox_dir).map_err(|_| unreadable())?;
    let below = worktree.strip_prefix(&anchor).map_err(|_| unreadable())?;
    let names: Vec<&OsStr> = below.components().map(Component::as_os_str).collect();
    let dir = files::open_dirs_below(&anchor, &names, false).map_err(|_| unreadable())?;
    Ok((
        read_workflows(&dir),
        read_small(&dir, OsStr::new("yulius.toml"), MAX_CONFIG_BYTES),
    ))
}

/// The regular file `name` of the folder `dir` (not a link, not waiting on a FIFO), of at most
/// `max` bytes: checked on its handle and read to at most `max + 1` bytes, so a file grown since
/// is refused too.
fn read_small(dir: &OwnedFd, name: &OsStr, max: usize) -> Option<String> {
    let flags = OFlag::O_RDONLY | OFlag::O_NOFOLLOW | OFlag::O_NONBLOCK | OFlag::O_CLOEXEC;
    let file = std::fs::File::from(openat(dir, name, flags, Mode::empty()).ok()?);
    let max = u64::try_from(max).unwrap_or(u64::MAX);
    let meta = file.metadata().ok()?;
    if !meta.is_file() || meta.len() > max {
        return None;
    }
    let mut text = String::new();
    file.take(max.saturating_add(1))
        .read_to_string(&mut text)
        .ok()?;
    u64::try_from(text.len())
        .is_ok_and(|len| len <= max)
        .then_some(text)
}

/// The workflows of the worktree folder `worktree`, summarized (bounded; links are skipped).
fn read_workflows(worktree: &OwnedFd) -> Vec<Value> {
    let Ok(github) = files::open_dir_at(worktree, OsStr::new(".github"), false) else {
        return Vec::new();
    };
    let Ok(dir) = files::open_dir_at(&github, OsStr::new("workflows"), false) else {
        return Vec::new();
    };
    let Ok(entries) = files::names_in(&dir, files::MAX_ENTRIES) else {
        return Vec::new();
    };
    let mut names: Vec<String> = entries
        .into_iter()
        .map(|name| name.to_string_lossy().into_owned())
        .filter(|n| n.ends_with(".yml") || n.ends_with(".yaml"))
        .collect();
    names.sort();
    names
        .into_iter()
        .take(MAX_WORKFLOW_FILES)
        .map(|name| {
            let Some(text) = read_small(&dir, OsStr::new(&name), MAX_WORKFLOW_BYTES) else {
                return json!({"file": format!("{WORKFLOWS_DIR}/{name}"), "problem": "not a regular file or larger than 64 KiB; read it directly"});
            };
            let summary = summarize_workflow(&text);
            json!({
                "file": format!("{WORKFLOWS_DIR}/{name}"),
                "name": summary.name,
                "truncated": summary.truncated,
                "jobs": summary.jobs.iter().map(|job| json!({
                    "id": job.id,
                    "runs_on": job.runs_on,
                    "strategy": job.strategy,
                    "steps": job.steps.iter().map(|s| json!({"name": s.name, "run": s.run, "uses": s.uses})).collect::<Vec<_>>(),
                })).collect::<Vec<_>>(),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use super::{CI_SETUP_STEPS, read_setup};

    const WORKFLOW: &str =
        "name: host\njobs:\n  leak:\n    runs-on: x\n    steps:\n      - run: cat /secret\n";

    /// A sandbox directory (canonical) with the task worktree `worktrees/t`, and a host folder
    /// beside it holding a workflow and a `yulius.toml` the sandbox must never get the server to
    /// read.
    fn sandbox() -> (tempfile::TempDir, PathBuf, PathBuf, PathBuf) {
        let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("tempdir: {e}"));
        let base = std::fs::canonicalize(dir.path()).unwrap_or_else(|e| panic!("{e}"));
        let (anchor, host) = (base.join("sandbox"), base.join("host"));
        let worktree = anchor.join("worktrees").join("t");
        for folder in [&worktree, &host.join("workflows")] {
            std::fs::create_dir_all(folder).unwrap_or_else(|e| panic!("{e}"));
        }
        write(&host.join("workflows").join("leak.yml"), WORKFLOW);
        write(
            &host.join("yulius.toml"),
            "[commands]\ngate = \"host secret\"\n",
        );
        (dir, anchor, worktree, host)
    }

    fn write(path: &Path, text: &str) {
        std::fs::write(path, text).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    }

    fn link(target: &Path, at: &Path) {
        std::os::unix::fs::symlink(target, at).unwrap_or_else(|e| panic!("{e}"));
    }

    /// What `read_setup` returned, the worktree being readable.
    fn read(anchor: &Path, worktree: &Path) -> (Vec<serde_json::Value>, Option<String>) {
        read_setup(anchor, worktree).unwrap_or_else(|e| panic!("{e}"))
    }

    /// Whether anything `read_setup` returned came from the host folder.
    fn leaked(read: &(Vec<serde_json::Value>, Option<String>)) -> bool {
        let (workflows, config) = read;
        serde_json::to_string(workflows).is_ok_and(|text| text.contains("leak"))
            || config
                .as_deref()
                .is_some_and(|text| text.contains("host secret"))
    }

    /// SPEC-391 VCT-001 (#829): the worktree's own files are read.
    #[test]
    fn ci_setup_reads_the_worktrees_own_files() {
        let (_dir, anchor, worktree, _host) = sandbox();
        std::fs::create_dir_all(worktree.join(".github/workflows"))
            .unwrap_or_else(|e| panic!("{e}"));
        write(
            &worktree.join(".github/workflows/ci.yml"),
            "name: own\njobs:\n  test:\n    runs-on: x\n",
        );
        write(
            &worktree.join("yulius.toml"),
            "[commands]\ngate = \"cargo test\"\n",
        );
        let (workflows, config) = read(&anchor, &worktree);
        assert_eq!(workflows.len(), 1, "{workflows:?}");
        assert_eq!(workflows[0]["name"], "own");
        assert!(config.is_some_and(|text| text.contains("cargo test")));
    }

    /// SPEC-391 VCT-001 (#829): a folder on the way swapped for a link to a host folder (here
    /// `.github`, which a check of `workflows` by path did not see) is not followed.
    #[test]
    fn ci_setup_follows_no_folder_swapped_for_a_link() {
        let (_dir, anchor, worktree, host) = sandbox();
        link(&host, &worktree.join(".github"));
        let read = read(&anchor, &worktree);
        assert!(!leaked(&read), "{read:?}");
        assert!(read.0.is_empty(), "{read:?}");
    }

    /// SPEC-391 VCT-001 (#829): the worktree folder itself swapped for a link after it was
    /// resolved reads nothing from where the link leads: the worktree cannot be read.
    #[test]
    fn ci_setup_follows_no_worktree_swapped_for_a_link() {
        let (_dir, anchor, worktree, host) = sandbox();
        std::fs::create_dir_all(host.join(".github")).unwrap_or_else(|e| panic!("{e}"));
        std::fs::rename(host.join("workflows"), host.join(".github/workflows"))
            .unwrap_or_else(|e| panic!("{e}"));
        std::fs::remove_dir(&worktree).unwrap_or_else(|e| panic!("{e}"));
        link(&host, &worktree);
        let read = read_setup(&anchor, &worktree);
        assert!(
            matches!(&read, Err(crate::error::AppError::Conflict(_))),
            "{read:?}"
        );
    }

    /// SPEC-391 VCT-001 (#829): a file that is a link or a FIFO yields nothing, without blocking.
    #[test]
    fn ci_setup_reads_no_linked_file_and_waits_on_no_fifo() {
        let (_dir, anchor, worktree, host) = sandbox();
        std::fs::create_dir_all(worktree.join(".github/workflows"))
            .unwrap_or_else(|e| panic!("{e}"));
        link(
            &host.join("workflows/leak.yml"),
            &worktree.join(".github/workflows/a.yml"),
        );
        link(&host.join("yulius.toml"), &worktree.join("yulius.toml"));
        nix::unistd::mkfifo(
            &worktree.join(".github/workflows/b.yml"),
            nix::sys::stat::Mode::from_bits_truncate(0o600),
        )
        .unwrap_or_else(|e| panic!("{e}"));
        let read = read(&anchor, &worktree);
        assert!(!leaked(&read), "{read:?}");
        assert!(read.1.is_none(), "{read:?}");
        assert_eq!(read.0.len(), 2, "both reported as unreadable: {read:?}");
        assert!(read.0.iter().all(|w| w["problem"].is_string()), "{read:?}");
    }

    /// SPEC-351 (#963): `ci_setup` no longer sends build outputs to a cache that concurrent runs
    /// share; `HOME` is the shared one.
    #[test]
    fn ci_setup_keeps_build_outputs_out_of_the_shared_home() {
        let steps = CI_SETUP_STEPS.join(" ");
        for wording in [
            "downloads go under $HOME (shared by the sandbox's concurrent CI runs)",
            "build outputs in the checkout",
            "$YULIUS_CI_CACHE (the job's own, never shared with a job running at the same time",
        ] {
            assert!(steps.contains(wording), "missing {wording:?}");
        }
        assert!(!steps.contains("caches go under $HOME or $YULIUS_CI_CACHE"));
    }
}
