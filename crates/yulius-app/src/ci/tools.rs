//! Agent tools for CI and repository hygiene (SPEC-002 §7.4): `ci_status`, `ci_rerun`,
//! `ci_setup` (onboarding: move GitHub Actions into Yulius CI), `repo_hygiene` and
//! `repo_cleanup`. Agents act as `task:<number>`; every removal is audited.

use std::path::Path;

use serde_json::{Value, json};
use yulius_contracts::api::ci::{HygieneCleanup, HygieneTarget};
use yulius_domain::ci::actions::{MAX_WORKFLOW_BYTES, summarize_workflow};
use yulius_domain::ci::{self as dom};
use yulius_domain::spec_format::{MAX_CONFIG_BYTES, parse_yulius_toml};

use super::{MAX_WORKFLOW_FILES, WORKFLOWS_DIR, run_view};
use crate::context::App;
use crate::coordination::AgentContext;
use crate::error::{AppError, AppResult};
use crate::model::SandboxRecord;

/// Output tail per job returned to an agent.
const TOOL_OUTPUT_BYTES: usize = 16 * 1024;
/// Runs listed by `ci_status`.
const TOOL_RUNS: u32 = 10;

/// How an agent moves a project's GitHub Actions into Yulius CI (F 2026-10-10: Yulius is the
/// only CI; no GitHub Actions).
pub const CI_SETUP_STEPS: &[&str] = &[
    "Read every workflow below (and the files themselves) and list what each job checks: its run steps, matrix and the tools its `uses:` steps install.",
    "Write the checks as `[[ci.jobs]]` entries in yulius.toml (name, command, optional matrix = \"a b c\" passed as $YULIUS_CI_MATRIX, optional cpus = N and memory_gb = N when a job needs more or less than an equal share of the machine); one job per independent check so they run in parallel; prefer the repository's own scripts. A single command may instead be `[commands] gate`.",
    "Size builds and test runners to the budget Yulius gives each job and agent session, not to the machine: they read $YULIUS_CPUS and $YULIUS_MEMORY_MB, and `[resources.env]` in yulius.toml sets the tools' own variables from them (for example CARGO_BUILD_JOBS = \"{cpus}\", PYTEST_XDIST_AUTO_NUM_WORKERS = \"{cpus}\", MAKEFLAGS = \"-j{cpus}\").",
    "Make each command work on the Yulius machine: no GitHub expressions (${{ }}) or services from Actions; a secret becomes an agent secret the job names with `secrets = \"NAME\"` (it gets a placeholder through the egress proxy, never the value, and only once the integration branch has the line; SPEC-142); tools a `uses:` step installed must be installed by the command or be on the machine; caches go under $HOME or $YULIUS_CI_CACHE, which persist between runs.",
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
            // Its pull request's runs and the merge trains it is in (SPEC-084).
            let mine = pr.is_some_and(|n| run.reference.covers(n));
            let mut value =
                serde_json::to_value(run_view(&run, &jobs, false)).map_err(AppError::internal)?;
            value["this_task"] = Value::Bool(mine);
            runs.push(value);
        }
        runs.sort_by_key(|run| !run["this_task"].as_bool().unwrap_or(false));
        Ok(json!({
            "ci": view.gate.as_ref().map(|gate| json!({
                "source": gate.source.as_str(),
                "jobs": gate.jobs.iter().map(|j| json!({"name": j.name, "command": j.command, "matrix": j.matrix})).collect::<Vec<_>>(),
            })),
            "problem": view.problem,
            "github_actions_workflows": view.workflows,
            "pull_request": pr,
            "runs": runs,
            "note": "Merges that require checks wait for the latest run at the pull request head to pass. Read a run's output with run_id.",
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
    /// A task without a worktree.
    pub async fn tool_ci_setup(&self, ctx: &AgentContext) -> AppResult<Value> {
        let sandbox = self.tool_sandbox(ctx).await?;
        let worktree = ctx
            .task
            .worktree_path
            .as_deref()
            .map(Path::new)
            .ok_or_else(|| AppError::Conflict("this task has no worktree yet".into()))?;
        let workflows = read_workflows(worktree);
        let config = match read_small(&worktree.join("yulius.toml"), MAX_CONFIG_BYTES) {
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
            self.hygiene_scan(&sandbox).await
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

/// A regular file (not a symlink) of at most `max` bytes.
fn read_small(path: &Path, max: usize) -> Option<String> {
    let meta = std::fs::symlink_metadata(path).ok()?;
    if !meta.is_file() || meta.len() > u64::try_from(max).unwrap_or(u64::MAX) {
        return None;
    }
    std::fs::read_to_string(path).ok()
}

/// The workflows of a worktree, summarized (bounded; symlinks are skipped).
fn read_workflows(worktree: &Path) -> Vec<Value> {
    let dir = worktree.join(WORKFLOWS_DIR);
    let Ok(meta) = std::fs::symlink_metadata(&dir) else {
        return Vec::new();
    };
    if !meta.is_dir() {
        return Vec::new();
    }
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut names: Vec<String> = entries
        .filter_map(Result::ok)
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.ends_with(".yml") || n.ends_with(".yaml"))
        .collect();
    names.sort();
    names
        .into_iter()
        .take(MAX_WORKFLOW_FILES)
        .map(|name| {
            let path = dir.join(&name);
            let Some(text) = read_small(&path, MAX_WORKFLOW_BYTES) else {
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
