//! Yulius-run CI end to end (SPEC-001 DLV-006/007, AC-DLV-4/5; SPEC-002 ADR-21): a real
//! repository with `[[ci.jobs]]` on the integration branch, the real Git adapter, the real
//! detached ptyhost running the jobs, and the fake GitHub recording commit statuses and branch
//! deletions.

use crate::support;

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::time::Duration;

use axum::http::{Method, StatusCode};
use serde_json::Value;
use support::{
    Creds, FakeRemote, HarnessOptions, call, configure_github_app, harness_with, perms,
    seed_github_user, seed_sandbox,
};
use yulius::wiring::session::PtySessionHost;
use yulius_app::error::{AppError, AppResult};
use yulius_app::model::SandboxRecord;
use yulius_app::ports::github::GhPull;
use yulius_app::ports::isolation::{
    CiUse, DiskUsage, EngineNetworks, LimitCounters, SandboxIsolation, SandboxJoin, SandboxLimits,
    SandboxRef,
};
use yulius_app::ports::store_ext::{AutomationStore as _, CiStore as _};
use yulius_domain::ci::{GateResult, RunState};
use yulius_domain::ids::SandboxId;

type BoxResult<T> = Result<T, Box<dyn std::error::Error>>;
type TestResult = BoxResult<()>;

const WAIT: Duration = Duration::from_secs(60);

fn git(dir: &Path, args: &[&str]) -> BoxResult<String> {
    let out = Command::new("git")
        .args([
            "-c",
            "user.name=Yulius Test",
            "-c",
            "user.email=test@yulius.test",
            "-c",
            "commit.gpgsign=false",
            "-c",
            "init.defaultBranch=main",
        ])
        .args(args)
        .current_dir(dir)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()?;
    if !out.status.success() {
        return Err(format!("git {args:?}: {}", String::from_utf8_lossy(&out.stderr)).into());
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_owned())
}

/// The bare `origin` as the fake GitHub's repository side.
struct Origin(PathBuf);

impl FakeRemote for Origin {
    fn branch_sha(&self, branch: &str) -> Option<String> {
        git(
            &self.0,
            &[
                "rev-parse",
                "--verify",
                "--quiet",
                &format!("refs/heads/{branch}"),
            ],
        )
        .ok()
    }
    fn merge(&self, _: &str, _: &str, _: &str, _: &str) -> Result<String, String> {
        Err("not used".into())
    }
    fn fast_forward(&self, _: &str, _: &str) -> Result<(), String> {
        Err("not used".into())
    }
    fn delete_branch(&self, branch: &str) -> Result<bool, String> {
        if self.branch_sha(branch).is_none() {
            return Ok(false);
        }
        git(
            &self.0,
            &["update-ref", "-d", &format!("refs/heads/{branch}")],
        )
        .map(|_| true)
        .map_err(|e| e.to_string())
    }
}

const YULIUS_TOML: &str = r#"[[ci.jobs]]
name = "lint"
command = "test -f README.md && echo lint-ok"

[[ci.jobs]]
name = "test"
command = 'if [ -f slow ]; then sleep 30; fi; echo "part $YULIUS_CI_MATRIX home $HOME"; if env | grep -q -E "YULIUS_TASK_TOKEN|GITHUB_TOKEN|CLAUDE_CODE_OAUTH_TOKEN|ANTHROPIC"; then exit 9; fi; echo clean-env; printf "\033[32mcolored\033[0m\n"'
matrix = "1 2"
"#;

struct Fixture {
    h: support::Harness,
    alice: Creds,
    sandbox: SandboxRecord,
    origin: PathBuf,
    work: PathBuf,
    data: PathBuf,
}

async fn fixture() -> BoxResult<Fixture> {
    fixture_with(false, |_| YULIUS_TOML.to_owned()).await
}

/// [`fixture`] with containment on or off and `yulius.toml` made from the data directory.
async fn fixture_with(contained: bool, toml: impl Fn(&Path) -> String) -> BoxResult<Fixture> {
    fixture_hosted(contained, toml, None).await
}

/// [`fixture_with`] whose jobs go to `host` when given.
async fn fixture_hosted(
    contained: bool,
    toml: impl Fn(&Path) -> String,
    host: Option<Arc<dyn yulius_app::ports::session::SessionHost>>,
) -> BoxResult<Fixture> {
    fixture_tuned(contained, toml, host, |_| {}).await
}

/// [`fixture_hosted`] with `tune` applied to the configuration after the fixture's own settings.
async fn fixture_tuned(
    contained: bool,
    toml: impl Fn(&Path) -> String,
    host: Option<Arc<dyn yulius_app::ports::session::SessionHost>>,
    tune: impl FnOnce(&mut yulius_app::context::AppConfig) + 'static,
) -> BoxResult<Fixture> {
    // A short base: job sockets live at <data>/runs/ci-<uuid>/pty.sock (Unix path limit).
    let dir = tempfile::Builder::new().prefix("yci").tempdir_in("/tmp")?;
    let root = dir.path().canonicalize()?;
    let exe = PathBuf::from(env!("CARGO_BIN_EXE_yulius"));
    let yulius_exe = exe.clone();
    // Launch the freshly built binary once before anything is timed: macOS scans a new binary
    // on its first launch (XProtect), which can take tens of seconds on a CI runner and would
    // otherwise land inside a job's start or timeout.
    let _ = std::process::Command::new(&exe).arg("version").output();
    let h = harness_with(HarnessOptions {
        session_host: Some(host.unwrap_or_else(|| Arc::new(PtySessionHost::new(exe)))),
        git: Some(Arc::new(crate::support::confined_git())),
        dir: Some(dir),
        configure: Some(Box::new(move |config| {
            config.ci.job_timeout = Duration::from_secs(8);
            config.ci.max_jobs = 3;
            config.contained = contained;
            config.yulius_exe = yulius_exe;
            tune(config);
        })),
        ..HarnessOptions::default()
    })?;
    let data = h.app.config.data_dir.clone();
    support::setup_superadmin_creds(&h).await?;
    configure_github_app(&h).await?;
    let alice = seed_github_user(&h, "u-alice", "alice").await?;
    h.github.set_perm("alice", "acme/app", perms("write"));
    let sandbox = seed_sandbox(&h, "s1", "app", "u-alice").await?;

    // origin: main == dev with the CI configuration; feat/one is pull request #1.
    let origin = root.join("origin.git");
    std::fs::create_dir_all(&origin)?;
    git(&origin, &["init", "--bare", "-q", "-b", "main"])?;
    let work = root.join("work");
    git(
        &root,
        &["clone", "-q", origin.to_str().ok_or("path")?, "work"],
    )?;
    std::fs::write(work.join("README.md"), "# app\n")?;
    std::fs::write(work.join("yulius.toml"), toml(&data))?;
    git(&work, &["add", "-A"])?;
    git(&work, &["commit", "-q", "-m", "init"])?;
    git(&work, &["push", "-q", "origin", "HEAD:main", "HEAD:dev"])?;
    git(&work, &["checkout", "-q", "-b", "feat/one"])?;
    std::fs::write(work.join("feature.txt"), "one\n")?;
    git(&work, &["add", "-A"])?;
    git(&work, &["commit", "-q", "-m", "feature"])?;
    git(&work, &["push", "-q", "origin", "feat/one"])?;
    let head = git(&work, &["rev-parse", "HEAD"])?;
    h.github.set_remote(Arc::new(Origin(origin.clone())));
    if let Ok(mut pulls) = h.github.pulls.lock() {
        pulls.push(GhPull {
            number: 1,
            html_url: "https://github.test/acme/app/pull/1".into(),
            title: "Feature".into(),
            state: "open".into(),
            merged: false,
            base: "dev".into(),
            base_sha: String::new(),
            head: "feat/one".into(),
            head_sha: head,
            mergeable: None,
            behind: false,
            from_fork: false,
            merge_commit_sha: None,
        });
    }
    let repo = data.join("sandboxes/s1/repo");
    std::fs::create_dir_all(repo.parent().ok_or("parent")?)?;
    git(
        &root,
        &[
            "clone",
            "-q",
            origin.to_str().ok_or("path")?,
            repo.to_str().ok_or("path")?,
        ],
    )?;
    Ok(Fixture {
        h,
        alice,
        sandbox,
        origin,
        work,
        data,
    })
}

impl Fixture {
    async fn get(&self, path: &str) -> BoxResult<Value> {
        let r = call(&self.h.router, Method::GET, path, None, &self.alice).await?;
        assert_eq!(r.status, StatusCode::OK, "{path}: {}", r.body);
        Ok(r.body)
    }

    async fn post(&self, path: &str, body: Option<Value>) -> BoxResult<support::Resp> {
        call(&self.h.router, Method::POST, path, body, &self.alice).await
    }

    /// Dispatches until no run is queued or running and every finished run's status is published
    /// (the background loops do not run here).
    async fn settle(&self) -> BoxResult<()> {
        let deadline = tokio::time::Instant::now() + WAIT;
        loop {
            self.h.app.ci_dispatch().await;
            let overview = self.get("/api/sandboxes/s1/ci").await?;
            // A finished run publishes its commit status after recording its end: settled means
            // no run is active and none waits for its status ("pending").
            let active = overview["runs"].as_array().ok_or("runs")?.iter().any(|r| {
                r["state"] == "queued" || r["state"] == "running" || r["status"] == "pending"
            });
            if !active {
                return Ok(());
            }
            if tokio::time::Instant::now() > deadline {
                return Err(format!("CI did not settle: {overview}").into());
            }
            tokio::time::sleep(Duration::from_millis(150)).await;
        }
    }

    /// Waits until a run of `reference` is running.
    async fn running(&self, reference: &str) -> BoxResult<Value> {
        let deadline = tokio::time::Instant::now() + WAIT;
        loop {
            self.h.app.ci_dispatch().await;
            let overview = self.get("/api/sandboxes/s1/ci").await?;
            if let Some(run) = overview["runs"]
                .as_array()
                .ok_or("runs")?
                .iter()
                .find(|r| r["reference"] == reference && r["state"] == "running")
            {
                return Ok(run.clone());
            }
            if tokio::time::Instant::now() > deadline {
                return Err(format!("no running run of {reference}: {overview}").into());
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    fn push_feature(&self, file: &str, contents: Option<&str>) -> BoxResult<String> {
        match contents {
            Some(text) => std::fs::write(self.work.join(file), text)?,
            None => std::fs::remove_file(self.work.join(file))?,
        }
        git(&self.work, &["add", "-A"])?;
        git(&self.work, &["commit", "-q", "-m", "change"])?;
        git(&self.work, &["push", "-q", "origin", "feat/one"])?;
        git(&self.work, &["rev-parse", "HEAD"])
    }

    /// Commits `file` (`None` deletes it) on dev and pushes it; the work tree stays on feat/one.
    fn push_dev(&self, file: &str, contents: Option<&str>) -> BoxResult<String> {
        git(&self.work, &["checkout", "-q", "dev"])?;
        git(&self.work, &["pull", "-q", "origin", "dev"])?;
        match contents {
            Some(text) => std::fs::write(self.work.join(file), text)?,
            None => std::fs::remove_file(self.work.join(file))?,
        }
        git(&self.work, &["add", "-A"])?;
        git(&self.work, &["commit", "-q", "-m", "dev change"])?;
        git(&self.work, &["push", "-q", "origin", "dev"])?;
        let sha = git(&self.work, &["rev-parse", "HEAD"])?;
        git(&self.work, &["checkout", "-q", "feat/one"])?;
        Ok(sha)
    }

    fn last_status(&self, sha: &str) -> Option<(String, String, String)> {
        self.h
            .github
            .statuses()
            .into_iter()
            .rev()
            .find(|(s, _)| s == sha)
            .map(|(_, st)| (st.state, st.description, st.target_url.unwrap_or_default()))
    }
}

#[tokio::test]
async fn runs_jobs_in_parallel_publishes_statuses_and_gates_merges() -> TestResult {
    let f = fixture().await?;
    let main_sha = git(&f.origin, &["rev-parse", "main"])?;
    let pr_sha = git(&f.origin, &["rev-parse", "feat/one"])?;

    // The CI in force comes from the integration branch's yulius.toml.
    let overview = f.get("/api/sandboxes/s1/ci").await?;
    assert_eq!(overview["gate"]["source"], "yulius_toml", "{overview}");
    let names: Vec<&str> = overview["gate"]["jobs"]
        .as_array()
        .ok_or("jobs")?
        .iter()
        .filter_map(|j| j["name"].as_str())
        .collect();
    assert_eq!(names, ["lint", "test (1)", "test (2)"]);
    assert_eq!(overview["can_run"], true);

    // Before any run, a merge waits (pending), whatever GitHub says.
    assert_eq!(
        f.h.app.ci_gate_result(&f.sandbox, &pr_sha).await,
        GateResult::Run(None)
    );

    // One sync: main and dev share a commit (tested once) and the pull request head.
    f.h.app.ci_sync_sandbox(&f.sandbox).await?;
    let queued = f.get("/api/sandboxes/s1/ci").await?;
    assert_eq!(
        queued["runs"].as_array().ok_or("runs")?.len(),
        2,
        "{queued}"
    );
    assert_eq!(f.last_status(&pr_sha).ok_or("pending")?.0, "pending");
    f.settle().await?;

    let overview = f.get("/api/sandboxes/s1/ci").await?;
    let pr_run = overview["runs"]
        .as_array()
        .ok_or("runs")?
        .iter()
        .find(|r| r["reference"] == "pr/1")
        .cloned()
        .ok_or("pr run")?;
    assert_eq!(pr_run["state"], "passed", "{pr_run}");
    assert_eq!(pr_run["label"], "PR #1");
    assert_eq!(pr_run["status"], "published");
    let detail = f
        .get(&format!(
            "/api/sandboxes/s1/ci/runs/{}",
            pr_run["id"].as_str().ok_or("id")?
        ))
        .await?;
    let jobs = detail["jobs"].as_array().ok_or("jobs")?;
    assert_eq!(jobs.len(), 3);
    // Passing jobs keep no failing tests' output (SPEC-145 AC3).
    assert!(jobs.iter().all(|j| j["failures"].is_null()), "{detail}");
    let output = |i: usize| jobs[i]["output"].as_str().unwrap_or_default().to_owned();
    assert!(output(0).contains("lint-ok"), "{}", output(0));
    let ci_home = f.data.join("sandboxes/s1/ci/home");
    for (index, part) in [(1, "part 1"), (2, "part 2")] {
        let text = output(index);
        assert!(text.contains(part), "{text}");
        assert!(
            text.contains("clean-env"),
            "no secret in the environment: {text}"
        );
        assert!(
            text.contains(&format!("home {}", ci_home.display())),
            "{text}"
        );
        // Escape sequences are removed from stored output.
        assert!(
            text.contains("colored") && !text.contains('\u{1b}'),
            "{text:?}"
        );
    }
    // The commit statuses: success on both commits, linking to the run.
    let (state, description, url) = f.last_status(&pr_sha).ok_or("status")?;
    assert_eq!(state, "success");
    assert!(description.starts_with("Passed: 3 jobs"), "{description}");
    assert!(
        url.starts_with("https://yulius.test/s/app/ci?run="),
        "{url}"
    );
    assert_eq!(f.last_status(&main_sha).ok_or("main status")?.0, "success");
    assert_eq!(
        f.h.app.ci_gate_result(&f.sandbox, &pr_sha).await,
        GateResult::Run(Some(RunState::Passed))
    );
    // Persistent per-job checkouts (caches survive), matrix siblings apart.
    for checkout in ["lint", "test.1", "test.2"] {
        assert!(
            f.data
                .join("sandboxes/s1/ci/jobs")
                .join(checkout)
                .join("README.md")
                .exists(),
            "{checkout}"
        );
    }

    // A failing head: the lint job fails, the status says which job, the gate fails.
    let failing = f.push_feature("README.md", None)?;
    f.h.app.ci_sync_sandbox(&f.sandbox).await?;
    f.settle().await?;
    assert_eq!(
        f.h.app.ci_gate_result(&f.sandbox, &failing).await,
        GateResult::Run(Some(RunState::Failed))
    );
    let (state, description, _) = f.last_status(&failing).ok_or("status")?;
    assert_eq!(state, "failure");
    assert!(description.contains("lint (exit code 1)"), "{description}");

    // Run again (write access): queued anew for the same commit, refused while active.
    let overview = f.get("/api/sandboxes/s1/ci").await?;
    let failed_id = overview["runs"][0]["id"].as_str().ok_or("id")?.to_owned();
    let again = f
        .post(
            &format!("/api/sandboxes/s1/ci/runs/{failed_id}/rerun"),
            None,
        )
        .await?;
    assert_eq!(again.status, StatusCode::OK, "{}", again.body);
    let twice = f
        .post(
            &format!("/api/sandboxes/s1/ci/runs/{failed_id}/rerun"),
            None,
        )
        .await?;
    assert_eq!(twice.status, StatusCode::CONFLICT);
    f.settle().await?;

    // A slow head is superseded by a newer push: the running jobs are killed at once.
    std::fs::write(f.work.join("README.md"), "# app\n")?;
    let slow = f.push_feature("slow", Some("1"))?;
    f.h.app.ci_sync_sandbox(&f.sandbox).await?;
    let running = f.running("pr/1").await?;
    assert_eq!(running["sha"], slow);
    let started = tokio::time::Instant::now();
    let fixed = f.push_feature("slow", None)?;
    f.h.app.ci_sync_sandbox(&f.sandbox).await?;
    f.settle().await?;
    assert!(
        started.elapsed() < Duration::from_secs(20),
        "the slow jobs were killed"
    );
    let detail = f
        .get(&format!(
            "/api/sandboxes/s1/ci/runs/{}",
            running["id"].as_str().ok_or("id")?
        ))
        .await?;
    assert_eq!(detail["state"], "cancelled", "{detail}");
    assert!(
        detail["reason"]
            .as_str()
            .is_some_and(|r| r.starts_with("superseded by ")),
        "{detail}"
    );
    assert_eq!(f.last_status(&slow).ok_or("status")?.0, "error");
    assert_eq!(
        f.h.app.ci_gate_result(&f.sandbox, &fixed).await,
        GateResult::Run(Some(RunState::Passed))
    );

    // A job past its timeout is killed and the run errors.
    let timeout = f.push_feature("slow", Some("2"))?;
    f.h.app.ci_sync_sandbox(&f.sandbox).await?;
    f.settle().await?;
    let (state, description, _) = f.last_status(&timeout).ok_or("status")?;
    assert_eq!(state, "error", "{description}");
    assert!(description.contains("timed out"), "{description}");

    // No job host or log is left behind.
    let runs_dir = f.data.join("runs");
    let leftovers: Vec<_> = std::fs::read_dir(&runs_dir)
        .map(|entries| entries.filter_map(Result::ok).collect())
        .unwrap_or_default();
    assert!(leftovers.is_empty(), "{leftovers:?}");
    Ok(())
}

/// SPEC-206 RMV-002 (#624): removing a sandbox cancels its CI runs (here a running one, its jobs
/// killed; a queued one takes `ci_cancel`'s queued path): they went on to their end and
/// published their statuses.
#[tokio::test]
async fn removing_the_sandbox_cancels_its_ci_runs() -> TestResult {
    let f = fixture().await?;
    f.h.github.set_perm("alice", "acme/app", perms("admin"));
    f.push_feature("slow", Some("1"))?;
    f.h.app.ci_sync_sandbox(&f.sandbox).await?;
    f.running("pr/1").await?;
    let active = |runs: &[yulius_app::ci::CiRunRecord]| {
        runs.iter()
            .filter(|r| matches!(r.state, RunState::Queued | RunState::Running))
            .count()
    };
    let before = f.h.store.ci_runs(&f.sandbox.id, 10).await?;
    assert!(active(&before) >= 1, "{before:?}");

    let started = tokio::time::Instant::now();
    let removed = call(
        &f.h.router,
        Method::DELETE,
        "/api/sandboxes/s1",
        None,
        &f.alice,
    )
    .await?;
    assert_eq!(removed.status, StatusCode::NO_CONTENT, "{}", removed.body);
    // No dispatch from here on: only the removal ends the runs, before the jobs' 8 s timeout.
    let deadline = started + Duration::from_secs(5);
    let runs = loop {
        let runs = f.h.store.ci_runs(&f.sandbox.id, 10).await?;
        if active(&runs) == 0 {
            break runs;
        }
        if tokio::time::Instant::now() > deadline {
            return Err(format!("the removed sandbox's runs go on: {runs:?}").into());
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    };
    for run in runs.iter().filter(|r| {
        before
            .iter()
            .any(|b| b.id == r.id && matches!(b.state, RunState::Queued | RunState::Running))
    }) {
        assert_eq!(run.state, RunState::Cancelled, "{run:?}");
        assert_eq!(
            run.reason.as_deref(),
            Some("the sandbox was removed"),
            "{run:?}"
        );
    }
    Ok(())
}

/// SPEC-108 PRL-003 (task #93): a run's queueing and its end change what a pull request's checks
/// say, so each tells the sandbox's browsers to read them again, even when GitHub refuses the
/// commit status.
#[tokio::test]
async fn a_run_announces_its_pull_requests() -> TestResult {
    let f = fixture().await?;
    f.h.github
        .statuses_forbidden
        .store(true, std::sync::atomic::Ordering::SeqCst);
    let mut announced = support::PullAnnouncements::new(&f.h.app, "s1");
    f.h.app.ci_sync_sandbox(&f.sandbox).await?;
    let runs = f.get("/api/sandboxes/s1/ci").await?["runs"]
        .as_array()
        .ok_or("runs")?
        .len();
    assert!(runs >= 1);
    assert_eq!(announced.take(), runs, "each run, queued");
    f.settle().await?;
    // Each run started and ended; its end may be published just after the run settles.
    assert_eq!(
        announced.wait_for(2 * runs, WAIT).await,
        2 * runs,
        "each run, started and ended"
    );
    Ok(())
}

/// SPEC-391 VCT-003 (#842): a pull request from a fork is never tested, even when its head commit
/// is in the sandbox's clone (an agent fetched it to review it), with its own jobs or with the
/// integration branch's and their secrets.
#[tokio::test]
async fn a_fork_pull_request_is_never_tested_even_with_its_head_in_the_clone() -> TestResult {
    let f = fixture().await?;
    if let Ok(mut pulls) = f.h.github.pulls.lock() {
        for pull in pulls.iter_mut() {
            pull.from_fork = true;
        }
    }
    f.h.app.ci_sync_sandbox(&f.sandbox).await?;
    let overview = f.get("/api/sandboxes/s1/ci").await?;
    let runs = overview["runs"].as_array().ok_or("runs")?;
    assert!(
        runs.iter().all(|r| r["reference"] != "pr/1"),
        "the fork's head was queued: {overview}"
    );
    assert!(
        runs.iter().any(|r| r["reference"] == "branch/dev"),
        "the integration branch is still tested: {overview}"
    );
    Ok(())
}

#[tokio::test]
async fn without_the_status_permission_runs_still_gate() -> TestResult {
    let f = fixture().await?;
    f.h.github
        .statuses_forbidden
        .store(true, std::sync::atomic::Ordering::SeqCst);
    let pr_sha = git(&f.origin, &["rev-parse", "feat/one"])?;
    f.h.app.ci_sync_sandbox(&f.sandbox).await?;
    f.settle().await?;
    assert_eq!(
        f.h.app.ci_gate_result(&f.sandbox, &pr_sha).await,
        GateResult::Run(Some(RunState::Passed))
    );
    // The superadmin-free instance: alice is not an administrator, so no access note for her.
    let overview = f.get("/api/sandboxes/s1/ci").await?;
    let run = &overview["runs"][0];
    assert!(
        run["status"]
            .as_str()
            .is_some_and(|s| s.contains("Commit statuses")),
        "{run}"
    );
    assert!(overview["status_access"].is_null());
    Ok(())
}

/// feat/old is merged into dev; feat/closed had a pull request closed without merging;
/// feat/one has an open pull request; feat/wip is recent work without a pull request.
fn stale_branches(f: &Fixture) -> BoxResult<()> {
    for (branch, file) in [
        ("feat/old", "old.txt"),
        ("feat/closed", "closed.txt"),
        ("feat/wip", "wip.txt"),
    ] {
        git(&f.work, &["checkout", "-q", "-B", branch, "origin/dev"])?;
        std::fs::write(f.work.join(file), "x\n")?;
        git(&f.work, &["add", "-A"])?;
        git(&f.work, &["commit", "-q", "-m", branch])?;
        git(&f.work, &["push", "-q", "origin", branch])?;
    }
    git(&f.work, &["push", "-q", "origin", "feat/old:dev"])?;
    if let Ok(mut pulls) = f.h.github.pulls.lock() {
        pulls.push(GhPull {
            number: 2,
            html_url: "https://github.test/acme/app/pull/2".into(),
            title: "Closed".into(),
            state: "closed".into(),
            merged: false,
            base: "dev".into(),
            base_sha: String::new(),
            head: "feat/closed".into(),
            head_sha: git(&f.origin, &["rev-parse", "feat/closed"])?,
            mergeable: None,
            behind: false,
            from_fork: false,
            merge_commit_sha: None,
        });
    }
    Ok(())
}

#[tokio::test]
async fn hygiene_lists_and_cleans_up_stale_branches() -> TestResult {
    let f = fixture().await?;
    stale_branches(&f)?;
    let scan = f.post("/api/sandboxes/s1/hygiene/scan", None).await?;
    assert_eq!(scan.status, StatusCode::OK, "{}", scan.body);
    let items = scan.body["items"].as_array().ok_or("items")?.clone();
    let find = |name: &str| items.iter().find(|i| i["name"] == name).cloned();
    assert_eq!(find("feat/old").ok_or("old")?["reason_code"], "merged");
    let closed = find("feat/closed").ok_or("closed")?;
    assert_eq!(closed["reason_code"], "pull_closed");
    assert_eq!(closed["safe"], true);
    assert!(find("feat/one").is_none(), "open pull request: kept");
    assert!(find("feat/wip").is_none(), "recent work: kept");
    assert!(find("dev").is_none() && find("main").is_none(), "protected");

    let cleaned = f.post("/api/sandboxes/s1/hygiene/cleanup", None).await?;
    assert_eq!(cleaned.status, StatusCode::OK, "{}", cleaned.body);
    let removed: Vec<&str> = cleaned.body["removed"]
        .as_array()
        .ok_or("removed")?
        .iter()
        .filter_map(|i| i["name"].as_str())
        .collect();
    assert!(
        removed.contains(&"feat/old") && removed.contains(&"feat/closed"),
        "{removed:?}"
    );
    let remaining = git(&f.origin, &["branch", "--format=%(refname:short)"])?;
    assert!(!remaining.contains("feat/old") && !remaining.contains("feat/closed"));
    assert!(remaining.contains("feat/one") && remaining.contains("feat/wip"));
    assert!(
        cleaned.body["report"]["items"]
            .as_array()
            .is_some_and(|items| items.iter().all(|i| i["kind"] != "remote_branch")),
        "{}",
        cleaned.body["report"]
    );
    Ok(())
}

/// A pull request of `head` into `base`, `state` closed (merged or not) or open.
fn pull(
    f: &Fixture,
    number: u64,
    head: &str,
    base: &str,
    open: bool,
    merged: bool,
) -> BoxResult<()> {
    let head_sha = git(&f.origin, &["rev-parse", head])?;
    if let Ok(mut pulls) = f.h.github.pulls.lock() {
        pulls.push(GhPull {
            number,
            html_url: format!("https://github.test/acme/app/pull/{number}"),
            title: head.into(),
            state: if open { "open" } else { "closed" }.into(),
            merged,
            base: base.into(),
            base_sha: String::new(),
            head: head.into(),
            head_sha: head_sha.trim().to_owned(),
            mergeable: None,
            behind: false,
            from_fork: false,
            merge_commit_sha: None,
        });
    }
    Ok(())
}

/// SPEC-269 HSC-001: the background round removes what loses nothing, as Yulius, and keeps
/// unmerged work. A branch already in dev that never was a pull request's head (a release or
/// staging branch) is left to a person, and a branch an open pull request targets is never stale
/// (review round 3).
#[tokio::test]
async fn the_hygiene_round_removes_safe_items_itself() -> TestResult {
    let f = fixture().await?;
    stale_branches(&f)?;
    // feat/landed was merged into dev with a merge commit through pull request #4; feat/base is
    // in dev too and an open pull request is stacked on it.
    for (branch, file) in [("feat/landed", "landed.txt"), ("feat/base", "base.txt")] {
        git(&f.work, &["checkout", "-q", "-B", branch, "origin/dev"])?;
        std::fs::write(f.work.join(file), "x\n")?;
        git(&f.work, &["add", "-A"])?;
        git(&f.work, &["commit", "-q", "-m", branch])?;
        git(&f.work, &["push", "-q", "origin", branch])?;
        git(&f.work, &["push", "-q", "origin", &format!("{branch}:dev")])?;
        git(&f.work, &["fetch", "-q", "origin"])?;
    }
    pull(&f, 4, "feat/landed", "dev", false, true)?;
    git(
        &f.work,
        &["checkout", "-q", "-B", "feat/stacked", "origin/feat/base"],
    )?;
    std::fs::write(f.work.join("stacked.txt"), "x\n")?;
    git(&f.work, &["add", "-A"])?;
    git(&f.work, &["commit", "-q", "-m", "stacked"])?;
    git(&f.work, &["push", "-q", "origin", "feat/stacked"])?;
    pull(&f, 5, "feat/stacked", "feat/base", true, false)?;

    f.h.app.hygiene_round().await;
    let remaining = git(&f.origin, &["branch", "--format=%(refname:short)"])?;
    assert!(
        !remaining.contains("feat/landed") && !remaining.contains("feat/closed"),
        "{remaining}"
    );
    assert!(remaining.contains("feat/one") && remaining.contains("feat/wip"));
    assert!(
        remaining.contains("feat/old"),
        "no pull request: a person's call: {remaining}"
    );
    assert!(
        remaining.contains("feat/base"),
        "an open pull request's base: {remaining}"
    );
    let report = f.get("/api/sandboxes/s1/hygiene").await?;
    let items = report["items"].as_array().ok_or("items")?;
    assert!(
        items
            .iter()
            .any(|i| i["name"] == "feat/old" && i["reason_code"] == "merged"),
        "still listed for a person: {report}"
    );
    assert!(items.iter().all(|i| i["name"] != "feat/base"), "{report}");
    let audited: Vec<String> =
        f.h.store
            .automation_audit(&f.sandbox.slug, 100)
            .await?
            .into_iter()
            .filter(|e| e.action == "hygiene.cleanup")
            .map(|e| format!("{} {}", e.actor, e.detail.unwrap_or_default()))
            .collect();
    for branch in ["feat/landed", "feat/closed"] {
        assert!(
            audited
                .iter()
                .any(|a| a.starts_with("yulius remote_branch ") && a.contains(branch)),
            "{audited:?}"
        );
    }
    Ok(())
}

/// SPEC-269 HSC-002: the report says what the last cleanup did and who did it, across scans; an
/// automatic round with nothing to do leaves it, and the round names when it runs next.
#[tokio::test]
async fn a_cleanup_is_kept_on_the_report() -> TestResult {
    let f = fixture().await?;
    stale_branches(&f)?;
    let before = f.get("/api/sandboxes/s1/hygiene").await?;
    assert!(before["last_cleanup"].is_null(), "{before}");
    assert!(
        before["next_cleanup_at"].is_null(),
        "not scheduled here: {before}"
    );

    let cleaned = f.post("/api/sandboxes/s1/hygiene/cleanup", None).await?;
    assert_eq!(cleaned.status, StatusCode::OK, "{}", cleaned.body);
    let last = cleaned.body["report"]["last_cleanup"].clone();
    assert_eq!(last["by"], "alice", "{last}");
    assert_eq!(last["automatic"], false, "{last}");
    let removed: Vec<&str> = last["removed"]
        .as_array()
        .ok_or("removed")?
        .iter()
        .filter_map(|i| i["name"].as_str())
        .collect();
    assert!(
        removed.contains(&"feat/old") && removed.contains(&"feat/closed"),
        "{last}"
    );
    assert!(last["at"].is_string(), "{last}");

    let scanned = f.post("/api/sandboxes/s1/hygiene/scan", None).await?;
    assert_eq!(scanned.body["last_cleanup"], last, "kept across scans");

    f.h.app.hygiene_round().await;
    let after = f.get("/api/sandboxes/s1/hygiene").await?;
    assert_eq!(
        after["last_cleanup"], last,
        "a round with nothing to do keeps it"
    );
    assert!(after["next_cleanup_at"].is_string(), "{after}");
    Ok(())
}

/// SPEC-269 (review round 1): a branch with commits pushed after its pull request closed is
/// unmerged work, and a local copy whose GitHub branch the same cleanup deleted is judged again
/// (its commits are on GitHub no more) and kept.
#[tokio::test]
async fn a_cleanup_never_loses_commits_that_only_a_branch_held() -> TestResult {
    let f = fixture().await?;
    stale_branches(&f)?;
    // feat/later had a pull request closed at its first commit, then got another one.
    git(
        &f.work,
        &["checkout", "-q", "-B", "feat/later", "origin/dev"],
    )?;
    std::fs::write(f.work.join("later.txt"), "1\n")?;
    git(&f.work, &["add", "-A"])?;
    git(&f.work, &["commit", "-q", "-m", "first"])?;
    let closed_at = git(&f.work, &["rev-parse", "HEAD"])?;
    std::fs::write(f.work.join("later.txt"), "2\n")?;
    git(&f.work, &["commit", "-q", "-am", "after closing"])?;
    git(&f.work, &["push", "-q", "origin", "feat/later"])?;
    if let Ok(mut pulls) = f.h.github.pulls.lock() {
        pulls.push(GhPull {
            number: 3,
            html_url: "https://github.test/acme/app/pull/3".into(),
            title: "Later".into(),
            state: "closed".into(),
            merged: false,
            base: "dev".into(),
            base_sha: String::new(),
            head: "feat/later".into(),
            head_sha: closed_at.trim().to_owned(),
            mergeable: None,
            behind: false,
            from_fork: false,
            merge_commit_sha: None,
        });
    }
    // The sandbox clone keeps a local copy of feat/closed, whose only other home is GitHub's
    // branch that this cleanup deletes.
    let clone = f.data.join("sandboxes/s1/repo");
    git(&clone, &["fetch", "-q", "origin"])?;
    git(
        &clone,
        &["branch", "-q", "feat/closed", "origin/feat/closed"],
    )?;

    let scan = f.post("/api/sandboxes/s1/hygiene/scan", None).await?;
    let items = scan.body["items"].as_array().ok_or("items")?.clone();
    assert!(
        items.iter().all(|i| i["name"] != "feat/later"),
        "recent unmerged work: {items:?}"
    );
    assert!(
        items
            .iter()
            .any(|i| i["kind"] == "local_branch" && i["name"] == "feat/closed"),
        "{items:?}"
    );
    let cleaned = f.post("/api/sandboxes/s1/hygiene/cleanup", None).await?;
    assert_eq!(cleaned.status, StatusCode::OK, "{}", cleaned.body);
    let remaining = git(&f.origin, &["branch", "--format=%(refname:short)"])?;
    assert!(remaining.contains("feat/later"), "{remaining}");
    assert!(!remaining.contains("feat/closed"), "{remaining}");
    let local = git(&clone, &["branch", "--format=%(refname:short)"])?;
    assert!(
        local.contains("feat/closed"),
        "the local copy stays: {local}"
    );
    let skipped = cleaned.body["skipped"].as_array().ok_or("skipped")?;
    assert!(
        skipped.iter().any(|s| s["name"] == "feat/closed"
            && s["reason"]
                .as_str()
                .is_some_and(|r| r.contains("no longer on GitHub"))),
        "{}",
        cleaned.body
    );
    Ok(())
}

/// SPEC-269 (review round 2): when the clone cannot be fetched, a branch is judged by the commit
/// Yulius last saw: one that moved on GitHub since (new commits, no pull request) is kept, and
/// local copies are kept as they cannot be confirmed on GitHub.
#[tokio::test]
async fn a_failed_fetch_never_deletes_what_moved_on_github() -> TestResult {
    let f = fixture().await?;
    stale_branches(&f)?;
    let clone = f.data.join("sandboxes/s1/repo");
    let scan = f.post("/api/sandboxes/s1/hygiene/scan", None).await?;
    assert_eq!(scan.status, StatusCode::OK, "{}", scan.body);
    git(&clone, &["branch", "-q", "feat/old", "origin/feat/old"])?;
    // feat/closed gets a commit on GitHub after its pull request closed.
    git(&f.work, &["checkout", "-q", "feat/closed"])?;
    std::fs::write(f.work.join("closed.txt"), "more\n")?;
    git(&f.work, &["commit", "-q", "-am", "after closing"])?;
    git(&f.work, &["push", "-q", "origin", "feat/closed"])?;
    // A ref lock left by another fetch: every fetch of the clone fails now.
    let lock = clone.join(".git/refs/remotes/origin/feat/closed.lock");
    std::fs::write(&lock, "")?;

    let cleaned = f.post("/api/sandboxes/s1/hygiene/cleanup", None).await?;
    std::fs::remove_file(&lock)?;
    assert_eq!(cleaned.status, StatusCode::OK, "{}", cleaned.body);
    let remaining = git(&f.origin, &["branch", "--format=%(refname:short)"])?;
    assert!(remaining.contains("feat/closed"), "{remaining}");
    assert!(
        !remaining.contains("feat/old"),
        "unmoved and merged: {remaining}"
    );
    let skipped = cleaned.body["skipped"].as_array().ok_or("skipped")?;
    let reason = |kind: &str, name: &str| {
        skipped
            .iter()
            .find(|s| s["kind"] == kind && s["name"] == name)
            .and_then(|s| s["reason"].as_str())
            .unwrap_or_default()
            .to_owned()
    };
    assert!(
        reason("remote_branch", "feat/closed").contains("moved on GitHub"),
        "{}",
        cleaned.body
    );
    assert!(
        reason("local_branch", "feat/old").contains("could not be fetched"),
        "{}",
        cleaned.body
    );
    let local = git(&clone, &["branch", "--format=%(refname:short)"])?;
    assert!(local.contains("feat/old"), "{local}");
    Ok(())
}

/// One hygiene scan of `s1`, with the branches whose pull requests it looked up on GitHub (one
/// request each) during it.
async fn scan_counting_lookups(f: &Fixture) -> BoxResult<(support::Resp, Vec<String>)> {
    let lookups = || {
        f.h.github
            .branch_pull_lookups
            .lock()
            .map(|l| l.clone())
            .map_err(|e| e.to_string())
    };
    let before = lookups()?.len();
    let scan = f.post("/api/sandboxes/s1/hygiene/scan", None).await?;
    let looked_up = lookups()?.split_off(before);
    Ok((scan, looked_up))
}

/// SPEC-269 (review round 4): branches already in dev never use up the pull-request lookups an
/// unmerged branch's verdict needs, however many there are.
#[tokio::test]
async fn merged_branches_leave_the_lookups_to_unmerged_ones() -> TestResult {
    let f = fixture().await?;
    let bound = yulius_app::ci::hygiene::MAX_PULL_LOOKUPS;
    // Old release branches at dev's commit, older than anything else.
    let refspecs: Vec<String> = (0..bound)
        .map(|n| format!("origin/dev:refs/heads/release/{n}"))
        .collect();
    let mut push = vec!["push", "-q", "origin"];
    push.extend(refspecs.iter().map(String::as_str));
    git(&f.work, &["fetch", "-q", "origin"])?;
    git(&f.work, &push)?;
    // A newer branch whose pull request was closed: only a lookup can tell.
    git(
        &f.work,
        &["checkout", "-q", "-B", "feat/closed", "origin/dev"],
    )?;
    std::fs::write(f.work.join("closed.txt"), "x\n")?;
    git(&f.work, &["add", "-A"])?;
    // Committed a day later than dev's commit, so the scan, oldest first, meets it last.
    let committed = Command::new("git")
        .args([
            "-c",
            "user.name=T",
            "-c",
            "user.email=t@t.test",
            "-c",
            "commit.gpgsign=false",
        ])
        .args(["commit", "-q", "-m", "closed"])
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_COMMITTER_DATE", "@4000000000 +0000")
        .current_dir(&f.work)
        .output()?;
    assert!(committed.status.success(), "{committed:?}");
    git(&f.work, &["push", "-q", "origin", "feat/closed"])?;
    pull(&f, 2, "feat/closed", "dev", false, false)?;

    let (scan, looked_up) = scan_counting_lookups(&f).await?;
    assert_eq!(scan.status, StatusCode::OK, "{}", scan.body);
    // One GitHub request per lookup: the merged branches got only what feat/closed left.
    assert!(
        looked_up.len() <= bound,
        "{} lookups, bound {bound}: {looked_up:?}",
        looked_up.len()
    );
    assert!(
        !looked_up.iter().any(|b| b == "feat/one"),
        "an open pull request's branch took a lookup: {looked_up:?}"
    );
    let items = scan.body["items"].as_array().ok_or("items")?;
    assert!(
        items
            .iter()
            .any(|i| i["name"] == "feat/closed" && i["reason_code"] == "pull_closed"),
        "{}",
        scan.body
    );
    assert_eq!(
        items
            .iter()
            .filter(|i| i["reason_code"] == "merged")
            .count(),
        bound,
        "{}",
        scan.body
    );
    Ok(())
}

/// SPEC-269 (review round 5): an open pull request's branch takes no lookup, and branches already
/// in the integration or production branch whose pull requests the bound left unread are named in
/// a note (SPEC-379: here one only the production branch has).
#[tokio::test]
async fn the_lookup_bound_counts_only_lookups_and_says_what_it_left() -> TestResult {
    let f = fixture().await?;
    let bound = yulius_app::ci::hygiene::MAX_PULL_LOOKUPS;
    git(&f.work, &["fetch", "-q", "origin"])?;
    // A hotfix on main that dev lacks.
    git(&f.work, &["checkout", "-q", "-B", "hotfix", "origin/main"])?;
    std::fs::write(f.work.join("hotfix.txt"), "x\n")?;
    git(&f.work, &["add", "-A"])?;
    git(&f.work, &["commit", "-q", "-m", "hotfix"])?;
    git(&f.work, &["push", "-q", "origin", "hotfix:main"])?;
    git(&f.work, &["fetch", "-q", "origin"])?;
    // `bound` recent unmerged branches without a pull request (one commit), after feat/one,
    // whose pull request is open, and one release branch at main's commit only.
    git(&f.work, &["checkout", "-q", "-B", "wip", "origin/dev"])?;
    std::fs::write(f.work.join("wip.txt"), "x\n")?;
    git(&f.work, &["add", "-A"])?;
    git(&f.work, &["commit", "-q", "-m", "wip"])?;
    let refspecs: Vec<String> = (0..bound)
        .map(|n| format!("wip:refs/heads/zz/{n:02}"))
        .chain(std::iter::once(
            "origin/main:refs/heads/release/1".to_owned(),
        ))
        .collect();
    let mut push = vec!["push", "-q", "origin"];
    push.extend(refspecs.iter().map(String::as_str));
    git(&f.work, &push)?;

    let (scan, looked_up) = scan_counting_lookups(&f).await?;
    assert_eq!(scan.status, StatusCode::OK, "{}", scan.body);
    assert!(
        looked_up.len() <= bound,
        "{} lookups, bound {bound}: {looked_up:?}",
        looked_up.len()
    );
    assert!(
        !looked_up.iter().any(|b| b == "feat/one"),
        "an open pull request's branch took a lookup: {looked_up:?}"
    );
    let notes: Vec<&str> = scan.body["notes"]
        .as_array()
        .ok_or("notes")?
        .iter()
        .filter_map(|n| n.as_str())
        .collect();
    assert!(
        !notes
            .iter()
            .any(|n| n.starts_with("Pull requests were looked up for")),
        "every unmerged branch got its lookup: {notes:?}"
    );
    assert!(
        notes
            .iter()
            .any(|n| n.contains("1 branch already in the integration or production branch")),
        "{notes:?}"
    );
    Ok(())
}

/// Jobs run inside the sandbox's containment (Linux, bubblewrap): what lies outside the sandbox
/// directory, such as the data directory's run records, is not there, `/tmp` is private, and
/// `HOME` is the sandbox's CI home.
#[cfg(target_os = "linux")]
#[tokio::test]
async fn contained_jobs_see_only_their_sandbox() -> TestResult {
    let probe = std::process::Command::new("bwrap")
        .args([
            "--die-with-parent",
            "--unshare-pid",
            "--ro-bind",
            "/",
            "/",
            "--proc",
            "/proc",
            "--",
            "/bin/true",
        ])
        .output();
    if !probe.is_ok_and(|out| out.status.success()) {
        eprintln!("SKIPPED contained_jobs_see_only_their_sandbox: bubblewrap cannot run here");
        return Ok(());
    }
    let f = fixture_with(true, |data| {
        format!(
            "[[ci.jobs]]\nname = \"iso\"\ncommand = 'touch /tmp/yulius-ci-probe; if [ -e \"{}\" ]; then echo leaked; exit 7; fi; echo \"isolated home=$HOME\"'\n",
            data.join("runs").display()
        )
    })
    .await?;
    let pr_sha = git(&f.origin, &["rev-parse", "feat/one"])?;
    f.h.app.ci_sync_sandbox(&f.sandbox).await?;
    f.settle().await?;
    assert_eq!(
        f.h.app.ci_gate_result(&f.sandbox, &pr_sha).await,
        GateResult::Run(Some(RunState::Passed))
    );
    let overview = f.get("/api/sandboxes/s1/ci").await?;
    let id = overview["runs"][0]["id"].as_str().ok_or("id")?.to_owned();
    let detail = f.get(&format!("/api/sandboxes/s1/ci/runs/{id}")).await?;
    let output = detail["jobs"][0]["output"]
        .as_str()
        .unwrap_or_default()
        .to_owned();
    assert!(output.contains("isolated home="), "{output}");
    assert!(output.contains("/ci/home"), "{output}");
    assert!(
        !std::path::Path::new("/tmp/yulius-ci-probe").exists(),
        "private /tmp"
    );
    Ok(())
}

/// SPEC-145 (#460, #485): a failing test's output stays readable however much the job prints
/// after it. The job prints a nextest failure, then 600 KB (more than the 512 KiB of output it
/// keeps) and fails: its failing tests' output is kept with it. (Its only job: a sibling would be
/// cancelled by the failure, sooner or later; a passing job keeping none is checked in
/// `runs_jobs_in_parallel_publishes_statuses_and_gates_merges`.)
#[tokio::test]
async fn a_failed_jobs_failing_tests_stay_readable_after_long_output() -> TestResult {
    let f = fixture_with(false, |_| {
        r#"[[ci.jobs]]
name = "suite"
command = 'printf "        FAIL [   0.547s] (290/1670) yulius::main cockpit::manages\n  stderr ───\n    thread panicked at tests/cockpit.rs:1258:5:\n    the kept assertion message\n\n"; yes "        PASS [   0.010s] (1/1670) yulius::main filler::line padding padding padding" | head -n 8000; exit 1'
"#
        .to_owned()
    })
    .await?;
    f.h.app.ci_sync_sandbox(&f.sandbox).await?;
    f.settle().await?;
    let overview = f.get("/api/sandboxes/s1/ci").await?;
    let id = overview["runs"][0]["id"].as_str().ok_or("id")?.to_owned();
    let detail = f.get(&format!("/api/sandboxes/s1/ci/runs/{id}")).await?;
    let jobs = detail["jobs"].as_array().ok_or("jobs")?;
    let suite = jobs.iter().find(|j| j["name"] == "suite").ok_or("suite")?;
    assert_eq!(suite["state"], "failed", "{detail}");
    let output = suite["output"].as_str().unwrap_or_default();
    assert!(
        !output.contains("the kept assertion message"),
        "the end of the output no longer holds it"
    );
    let failures = suite["failures"].as_str().unwrap_or_default();
    assert!(
        failures.starts_with("FAIL [   0.547s] (290/1670) yulius::main cockpit::manages\n"),
        "{failures}"
    );
    assert!(
        failures.contains("the kept assertion message"),
        "{failures}"
    );
    assert!(!failures.contains("filler::"), "{failures}");
    Ok(())
}

/// SPEC-162 SKP-001..003: a job that exits with the skip code Yulius gives it ends skipped, not
/// passed; its run passes and the commit status says a job was skipped, also when a merge with
/// the same tree reuses that result.
#[tokio::test]
async fn a_job_that_exits_with_the_skip_code_ends_skipped() -> TestResult {
    let f = fixture_with(false, |_| {
        r#"[[ci.jobs]]
name = "check"
command = "true"

[[ci.jobs]]
name = "scan"
command = 'echo "nothing to scan"; exit "$YULIUS_CI_SKIP_CODE"'

[[ci.jobs]]
name = "split"
matrix = "1 2"
command = 'if [ "$YULIUS_CI_MATRIX" = 2 ]; then exit "$YULIUS_CI_SKIP_CODE"; fi'
"#
        .to_owned()
    })
    .await?;
    f.h.app.ci_sync_sandbox(&f.sandbox).await?;
    f.settle().await?;
    let overview = f.get("/api/sandboxes/s1/ci").await?;
    let run = overview["runs"]
        .as_array()
        .ok_or("runs")?
        .iter()
        .find(|r| r["reference"] == "pr/1")
        .cloned()
        .ok_or("pr/1 run")?;
    assert_eq!(run["state"], "passed", "{run}");
    let detail = f
        .get(&format!(
            "/api/sandboxes/s1/ci/runs/{}",
            run["id"].as_str().ok_or("id")?
        ))
        .await?;
    let jobs = detail["jobs"].as_array().ok_or("jobs")?;
    let job = |name: &str| jobs.iter().find(|j| j["name"] == name).ok_or("job");
    assert_eq!(job("check")?["state"], "passed", "{detail}");
    assert_eq!(job("scan")?["state"], "skipped", "{detail}");
    assert_eq!(job("scan")?["exit_code"], 78, "{detail}");
    assert_eq!(job("split (1)")?["state"], "passed", "{detail}");
    assert_eq!(job("split (2)")?["state"], "skipped", "{detail}");
    let sha = run["sha"].as_str().ok_or("sha")?;
    let (state, description, _) = f.last_status(sha).ok_or("status")?;
    assert_eq!(state, "success", "{description}");
    assert!(
        description.starts_with("Passed: 4 jobs, 2 skipped"),
        "{description}"
    );

    // A merge with the tested tree reuses the result, and the reused job stays skipped.
    git(&f.work, &["checkout", "-q", "-b", "dev", "origin/dev"])?;
    git(&f.work, &["merge", "-q", "--squash", "feat/one"])?;
    git(&f.work, &["commit", "-q", "-m", "feature (#1)"])?;
    git(&f.work, &["push", "-q", "origin", "dev"])?;
    let merged = git(&f.work, &["rev-parse", "HEAD"])?;
    f.h.app.ci_sync_sandbox(&f.sandbox).await?;
    let overview = f.get("/api/sandboxes/s1/ci").await?;
    let reused = overview["runs"]
        .as_array()
        .ok_or("runs")?
        .iter()
        .find(|r| r["sha"] == merged.as_str())
        .cloned()
        .ok_or("dev run")?;
    assert_eq!(reused["state"], "passed", "{reused}");
    let detail = f
        .get(&format!(
            "/api/sandboxes/s1/ci/runs/{}",
            reused["id"].as_str().ok_or("id")?
        ))
        .await?;
    let jobs = detail["jobs"].as_array().ok_or("jobs")?;
    let job = |name: &str| jobs.iter().find(|j| j["name"] == name).ok_or("job");
    assert_eq!(job("check")?["state"], "passed", "{detail}");
    assert_eq!(job("scan")?["state"], "skipped", "{detail}");
    assert_eq!(job("scan")?["exit_code"], 78, "{detail}");
    // Each value of a matrix job keeps its own verdict (they share the definition's key).
    assert_eq!(job("split (1)")?["state"], "passed", "{detail}");
    assert_eq!(job("split (2)")?["state"], "skipped", "{detail}");
    let (_, description, _) = f.last_status(&merged).ok_or("status")?;
    assert!(description.contains("2 skipped"), "{description}");
    Ok(())
}

/// SPEC-497 SHR-001: a job split into shards (`matrix = "1/2 2/2"`) runs them side by side, each
/// told its shard; the run passes only when every shard passed. Each shard leaves a mark and waits
/// for the other's (bounded), so a shard that ran alone says so and fails.
#[tokio::test]
async fn a_sharded_job_runs_its_shards_side_by_side() -> TestResult {
    // One run at a time: two runs' shards each half-admitted would wait for each other (review
    // round 1: each shard needs half the budget).
    let f = fixture_tuned(
        false,
        |data| {
        let marks = data.join("shard-marks");
        let marks = marks.to_string_lossy();
        let dir = format!("{marks}/$YULIUS_CI_SHA");
        let script = format!(
            "mkdir -p {dir}; i=${{YULIUS_CI_MATRIX%/*}}; touch {dir}/$i; other=$((3 - i)); n=0; \
             while [ ! -e {dir}/$other ]; do n=$((n + 1)); if [ $n -gt 50 ]; then \
             echo \"shard $i ran alone\"; exit 1; fi; sleep 0.1; done; \
             echo \"shard $YULIUS_CI_MATRIX met the other\"; \
             if [ -e two.txt ] && [ $i = 2 ]; then echo \"shard 2 fails\"; exit 1; fi"
        );
        format!("[[ci.jobs]]\nname = \"test\"\nmatrix = \"1/2 2/2\"\ncommand = '{script}'\n")
        },
        None,
        |config| config.ci.lanes = 1,
    )
    .await?;
    sync_and_settle(&f).await?;
    let one = f.get("/api/sandboxes/s1/ci").await?;
    let run = one["runs"]
        .as_array()
        .ok_or("runs")?
        .iter()
        .find(|r| r["reference"] == "pr/1")
        .cloned()
        .ok_or("pr/1 run")?;
    assert_eq!(run["state"], "passed", "{run}");
    let jobs = run_jobs(&f, &run).await?;
    assert_eq!(jobs.len(), 2, "{jobs:?}");
    for (job, shard) in jobs.iter().zip(["1/2", "2/2"]) {
        assert_eq!(job["name"], format!("test ({shard})"), "{job}");
        assert!(
            job["output"]
                .as_str()
                .unwrap_or_default()
                .contains(&format!("shard {shard} met the other")),
            "{job}"
        );
    }
    // A shard that fails fails the run.
    open_pull(&f, 2, "feat/two", "origin/dev", "two.txt")?;
    sync_and_settle(&f).await?;
    let two = f.get("/api/sandboxes/s1/ci").await?;
    let run = two["runs"]
        .as_array()
        .ok_or("runs")?
        .iter()
        .find(|r| r["reference"] == "pr/2")
        .cloned()
        .ok_or("pr/2 run")?;
    assert_eq!(run["state"], "failed", "{run}");
    let jobs = run_jobs(&f, &run).await?;
    let second = jobs
        .iter()
        .find(|j| j["name"] == "test (2/2)")
        .ok_or("shard 2")?;
    assert_eq!(second["state"], "failed", "{second}");
    Ok(())
}

/// SPEC-497 SHR-002: a run takes the lane whose checkouts last built what it builds on. #1 and #2
/// run side by side in lanes 0 and 1; then #2 pushes first: its new run, queued first, takes lane
/// 1 (its own previous build), not the first free, and its output says so; #1's takes lane 0.
#[tokio::test]
async fn a_run_takes_the_lane_that_last_built_its_pull_request() -> TestResult {
    let f = fixture_tuned(
        false,
        |_| "[[ci.jobs]]\nname = \"check\"\ncommand = \"true\"\ncpus = 1\n".to_owned(),
        None,
        |config| config.ci.lanes = 2,
    )
    .await?;
    // `dev` and #1 first, so #2's lane does not depend on which of them ends first (review
    // round 1): they end, and #2 takes the first free lane.
    sync_and_settle(&f).await?;
    open_pull(&f, 2, "feat/two", "origin/dev", "two.txt")?;
    sync_and_settle(&f).await?;
    let lane_of = |job: &Value| {
        job["output"]
            .as_str()
            .unwrap_or_default()
            .lines()
            .find(|l| l.starts_with("[yulius] lane "))
            .unwrap_or_default()
            .to_owned()
    };
    let lane_number = |line: &str| -> Option<u32> {
        line.strip_prefix("[yulius] lane ")?
            .split(|c: char| !c.is_ascii_digit())
            .next()?
            .parse()
            .ok()
    };
    let mut lanes = Vec::new();
    for pr in ["pr/1", "pr/2"] {
        let line = lane_of(&job_of(&f, "reference", pr).await?);
        lanes.push(lane_number(&line).ok_or(line)?);
    }
    assert_ne!(lanes[0], lanes[1], "side by side: {lanes:?}");
    // The pull request in lane 1 pushes first, so its run is queued first: the first free lane
    // would be 0.
    let push = |branch: &str, number: u64, file: &str| -> BoxResult<String> {
        git(&f.work, &["checkout", "-q", branch])?;
        std::fs::write(f.work.join(file), "again\n")?;
        git(&f.work, &["add", "-A"])?;
        git(&f.work, &["commit", "-q", "-m", file])?;
        git(&f.work, &["push", "-q", "origin", branch])?;
        let head = git(&f.work, &["rev-parse", "HEAD"])?;
        git(&f.work, &["checkout", "-q", "feat/one"])?;
        if let Ok(mut pulls) = f.h.github.pulls.lock()
            && let Some(pull) = pulls.iter_mut().find(|p| p.number == number)
        {
            pull.head_sha = head.clone();
        }
        Ok(head)
    };
    let mut order = [(1u64, "feat/one", lanes[0]), (2, "feat/two", lanes[1])];
    order.sort_by_key(|(_, _, lane)| std::cmp::Reverse(*lane));
    let mut heads = Vec::new();
    for (number, branch, lane) in order {
        let head = push(branch, number, &format!("again-{number}.txt"))?;
        f.h.app.ci_sync_sandbox(&f.sandbox).await?;
        heads.push((number, head, lane));
    }
    f.settle().await?;
    for (number, head, lane) in heads {
        assert_eq!(
            lane_of(&job_of(&f, "sha", &head).await?),
            format!("[yulius] lane {lane}, last taken by pr/{number}, which this run builds on")
        );
    }
    Ok(())
}

/// SPEC-493 CJP-001 to CJP-004 (AC-16): a job naming the files it tests ends skipped, with the
/// reason and without running, for a pull request touching none of them, which still passes and
/// gates nothing; the integration branch's own run runs it; a pull request touching one of them
/// runs it.
#[tokio::test]
async fn a_job_with_paths_runs_only_for_a_change_to_them() -> TestResult {
    let f = fixture_with(false, |_| {
        r#"[[ci.jobs]]
name = "check"
command = "true"

[[ci.jobs]]
name = "web"
command = 'echo web ran'
paths = ["crates/a/**"]
"#
        .to_owned()
    })
    .await?;
    let job_of = |detail: &Value, name: &str| -> BoxResult<Value> {
        Ok(detail["jobs"]
            .as_array()
            .ok_or("jobs")?
            .iter()
            .find(|j| j["name"] == name)
            .cloned()
            .ok_or("job")?)
    };
    let run_at = |sha: String| {
        let f = &f;
        async move {
            let overview = f.get("/api/sandboxes/s1/ci").await?;
            let run = overview["runs"]
                .as_array()
                .ok_or("runs")?
                .iter()
                .find(|r| r["sha"] == sha.as_str())
                .cloned()
                .ok_or("run")?;
            let detail = f
                .get(&format!(
                    "/api/sandboxes/s1/ci/runs/{}",
                    run["id"].as_str().ok_or("id")?
                ))
                .await?;
            BoxResult::Ok((run, detail))
        }
    };
    // Pull request #1 changes `feature.txt` only.
    f.h.app.ci_sync_sandbox(&f.sandbox).await?;
    f.settle().await?;
    let head = git(&f.origin, &["rev-parse", "feat/one"])?;
    let (run, detail) = run_at(head.clone()).await?;
    assert_eq!(run["state"], "passed", "{run}");
    let web = job_of(&detail, "web")?;
    assert_eq!(web["state"], "skipped", "{detail}");
    let said = web["output"].as_str().unwrap_or_default();
    assert!(
        said.contains("no file this job tests changed (paths: crates/a/**)")
            && !said.contains("web ran"),
        "{web}"
    );
    assert_eq!(job_of(&detail, "check")?["state"], "passed");
    assert_eq!(
        web["exit_code"], 78,
        "the skip code, as for any skipped job: {web}"
    );
    // Its span reads as a job that was skipped, so the stages count runs against skips.
    {
        use yulius_app::ports::store_ext::SpanStore as _;
        let spans =
            f.h.store
                .subject_spans(
                    &yulius_domain::ids::SandboxId::parse("s1")?,
                    yulius_domain::stages::SubjectKind::CiRun,
                    run["id"].as_str().ok_or("id")?,
                    50,
                )
                .await?;
        assert!(
            spans.iter().any(|s| s.kind == "job"
                && s.name == "web"
                && s.outcome.as_deref() == Some("skipped")),
            "{spans:?}"
        );
    }
    let (state, description, _) = f.last_status(&head).ok_or("status")?;
    assert_eq!(state, "success", "{description}");
    assert!(description.contains("1 skipped"), "{description}");

    // The integration branch's own run runs every job.
    let dev = f.push_dev("other.txt", Some("x\n"))?;
    f.h.app.ci_sync_sandbox(&f.sandbox).await?;
    f.settle().await?;
    let (_, detail) = run_at(dev).await?;
    assert_eq!(job_of(&detail, "web")?["state"], "passed", "{detail}");

    // A change touching one of its paths runs it.
    std::fs::create_dir_all(f.work.join("crates/a/src"))?;
    let touched = f.push_feature("crates/a/src/lib.rs", Some("pub fn a() {}\n"))?;
    f.h.app.ci_sync_sandbox(&f.sandbox).await?;
    f.settle().await?;
    let (_, detail) = run_at(touched).await?;
    let web = job_of(&detail, "web")?;
    assert_eq!(web["state"], "passed", "{detail}");
    assert!(
        web["output"]
            .as_str()
            .unwrap_or_default()
            .contains("web ran"),
        "{web}"
    );
    Ok(())
}

/// The run of `reference` and its detail, once there is one.
async fn run_of(f: &Fixture, sha: &str) -> BoxResult<(Value, Value)> {
    let overview = f.get("/api/sandboxes/s1/ci").await?;
    let run = overview["runs"]
        .as_array()
        .ok_or("runs")?
        .iter()
        .find(|r| r["sha"] == sha)
        .cloned()
        .ok_or("run")?;
    let detail = f
        .get(&format!(
            "/api/sandboxes/s1/ci/runs/{}",
            run["id"].as_str().ok_or("id")?
        ))
        .await?;
    Ok((run, detail))
}

fn job_named(detail: &Value, name: &str) -> BoxResult<Value> {
    Ok(detail["jobs"]
        .as_array()
        .ok_or("jobs")?
        .iter()
        .find(|j| j["name"] == name)
        .cloned()
        .ok_or("job")?)
}

/// Review round 2 of SPEC-493 fix 5: a pull request whose change touches none of its jobs'
/// `paths` gets a run that passed when it was queued: no lane, no dispatch, its status success
/// with every job skipped, and the gate's answer Passed.
#[tokio::test]
async fn a_run_of_skipped_jobs_passes_when_it_is_queued() -> TestResult {
    let f = fixture_with(false, |_| {
        "[[ci.jobs]]\nname = \"web\"\ncommand = 'echo web ran'\npaths = [\"crates/a/**\"]\n\n\
         [[ci.jobs]]\nname = \"docs\"\ncommand = 'echo docs ran'\npaths = [\"docs/\"]\n"
            .to_owned()
    })
    .await?;
    f.h.app.ci_sync_sandbox(&f.sandbox).await?;
    let head = git(&f.origin, &["rev-parse", "feat/one"])?;
    // No dispatch: nothing ran it.
    let (run, detail) = run_of(&f, &head).await?;
    assert_eq!(run["state"], "passed", "{run}");
    assert!(
        run["reason"]
            .as_str()
            .is_some_and(|r| r.contains("every job was skipped")),
        "{run}"
    );
    for name in ["web", "docs"] {
        assert_eq!(job_named(&detail, name)?["state"], "skipped", "{detail}");
    }
    let (state, description, _) = f.last_status(&head).ok_or("status")?;
    assert_eq!(state, "success", "{description}");
    assert!(description.contains("2 skipped"), "{description}");
    assert_eq!(
        f.h.app.ci_gate_result(&f.sandbox, &head).await,
        yulius_domain::ci::GateResult::Run(Some(yulius_domain::ci::RunState::Passed))
    );
    Ok(())
}

/// Review round 1 of SPEC-493 fix 5: a run queued again (a restart, the disk quota) keeps a job
/// skipped for its `paths` skipped, with its reason; the other jobs go back to the queue.
#[tokio::test]
async fn a_requeued_run_keeps_its_paths_skip() -> TestResult {
    use yulius_app::ci::{CiJobRecord, CiRunRecord};
    use yulius_app::ports::store_ext::CiStore as _;
    use yulius_domain::ci::{CiRef, GateSource, RunState};
    use yulius_domain::ids::SandboxId;
    use yulius_domain::time::Timestamp;
    let h = support::harness()?;
    support::seed_github_user(&h, "u1", "alice").await?;
    support::seed_sandbox(&h, "s1", "app", "u1").await?;
    let run = CiRunRecord {
        id: "run-1".into(),
        sandbox_id: SandboxId::parse("s1")?,
        reference: CiRef::PullRequest(1),
        sha: "a".repeat(40),
        source: GateSource::ProjectConfig,
        state: RunState::Running,
        reason: None,
        requested_by: "yulius".into(),
        status_for: None,
        status_note: None,
        status_attempts: 0,
        queued_at: Timestamp(1),
        started_at: Some(Timestamp(2)),
        ended_at: None,
    };
    let job = |ordinal: u32, id: &str, state: RunState, reason: Option<&str>| CiJobRecord {
        id: id.into(),
        run_id: "run-1".into(),
        ordinal,
        name: id.into(),
        key: id.into(),
        command: "true".into(),
        matrix: None,
        state,
        reason: reason.map(str::to_owned),
        exit_code: None,
        output: String::new(),
        failures: None,
        started_at: None,
        ended_at: None,
        need: Default::default(),
        timeout_minutes: None,
        exclusive: false,
        waited_ms: None,
    };
    h.store
        .insert_ci_run(
            &run,
            &[
                job(
                    0,
                    "web",
                    RunState::Skipped,
                    Some("no file this job tests changed (paths: a/**)"),
                ),
                job(1, "scan", RunState::Skipped, Some("exited 78")),
                job(2, "check", RunState::Running, None),
            ],
        )
        .await?;
    assert!(
        h.store
            .requeue_ci_run("run-1", "again", Timestamp(3))
            .await?
    );
    let jobs = h.store.ci_jobs("run-1", false).await?;
    let state = |name: &str| jobs.iter().find(|j| j.name == name).map(|j| j.state);
    assert_eq!(state("web"), Some(RunState::Skipped), "{jobs:?}");
    assert_eq!(
        state("scan"),
        Some(RunState::Queued),
        "a job's own skip runs again"
    );
    assert_eq!(state("check"), Some(RunState::Queued));
    Ok(())
}

/// Review round 1 of SPEC-493 fix 5: a squash merge of a pull request that skipped a `paths` job
/// gives the integration branch the tree that pull request's run tested, but that run never ran
/// the job: the integration branch's own run runs it instead of reusing the result (CJP-003).
#[tokio::test]
async fn the_integration_branch_runs_a_paths_job_a_merged_tree_skipped() -> TestResult {
    let f = fixture_with(false, |_| {
        "[[ci.jobs]]\nname = \"check\"\ncommand = \"true\"\n\n\
         [[ci.jobs]]\nname = \"web\"\ncommand = 'echo web ran'\npaths = [\"crates/a/**\"]\n"
            .to_owned()
    })
    .await?;
    f.h.app.ci_sync_sandbox(&f.sandbox).await?;
    f.settle().await?;
    let head = git(&f.origin, &["rev-parse", "feat/one"])?;
    let (_, detail) = run_of(&f, &head).await?;
    assert_eq!(job_named(&detail, "web")?["state"], "skipped", "{detail}");
    git(&f.work, &["checkout", "-q", "-b", "dev", "origin/dev"])?;
    git(&f.work, &["merge", "-q", "--squash", "feat/one"])?;
    git(&f.work, &["commit", "-q", "-m", "feature (#1)"])?;
    git(&f.work, &["push", "-q", "origin", "dev"])?;
    let merged = git(&f.work, &["rev-parse", "HEAD"])?;
    f.h.app.ci_sync_sandbox(&f.sandbox).await?;
    f.settle().await?;
    let (run, detail) = run_of(&f, &merged).await?;
    assert_eq!(run["state"], "passed", "{run}");
    let web = job_named(&detail, "web")?;
    assert_eq!(web["state"], "passed", "{detail}");
    assert!(
        web["output"]
            .as_str()
            .unwrap_or_default()
            .contains("web ran"),
        "{web}"
    );
    Ok(())
}

/// A project with a quick check and an `after_merge` full suite; `staging` adds the merge-first
/// environment (SPEC-575).
fn merge_first_toml(staging: bool) -> String {
    let mut text = "[[ci.jobs]]\nname = \"quick\"\ncommand = \"true\"\n\n\
                    [[ci.jobs]]\nname = \"full\"\ncommand = 'echo full suite ran'\n\
                    after_merge = true\n"
        .to_owned();
    if staging {
        text.push_str(
            "\n[cd.dev]\n[cd.prod]\n\n[cd.staging]\nstage = \"dev\"\nmerge_first = true\n\
             deploy = \"scripts/cd/deploy.sh staging\"\nverify = \"scripts/cd/verify.sh staging\"\n\
             rollback = \"scripts/cd/rollback.sh staging\"\nmigrations = \"none: stateless\"\n",
        );
    }
    text
}

/// Squash-merges `feat/one` into origin's `dev`; the merged commit.
fn squash_feat_one(f: &Fixture) -> BoxResult<String> {
    git(&f.work, &["checkout", "-q", "-b", "dev", "origin/dev"])?;
    git(&f.work, &["merge", "-q", "--squash", "feat/one"])?;
    git(&f.work, &["commit", "-q", "-m", "feature (#1)"])?;
    git(&f.work, &["push", "-q", "origin", "dev"])?;
    git(&f.work, &["rev-parse", "HEAD"])
}

/// SPEC-575 MFS-002 (AC: quick checks on the pull request, full suite once on dev): with a
/// merge-first environment the full suite is skipped on the pull request head, with its reason,
/// the head still passes, and the integration branch's run at the squash commit runs it instead
/// of reusing the pull request's tree: the full suite ran once, on the merged revision.
#[tokio::test]
async fn merge_first_runs_the_full_suite_once_on_the_merged_revision() -> TestResult {
    let f = fixture_with(false, |_| merge_first_toml(true)).await?;
    f.h.app.ci_sync_sandbox(&f.sandbox).await?;
    f.settle().await?;
    let head = git(&f.origin, &["rev-parse", "feat/one"])?;
    let (run, detail) = run_of(&f, &head).await?;
    assert_eq!(run["state"], "passed", "{run}");
    let full = job_named(&detail, "full")?;
    assert_eq!(full["state"], "skipped", "{detail}");
    let said = full["output"].as_str().unwrap_or_default();
    assert!(
        said.contains(yulius_domain::ci::AFTER_MERGE_SKIP_REASON) && !said.contains("suite ran"),
        "{full}"
    );
    assert_eq!(job_named(&detail, "quick")?["state"], "passed", "{detail}");
    let (state, description, _) = f.last_status(&head).ok_or("status")?;
    assert_eq!(state, "success", "{description}");

    let merged = squash_feat_one(&f)?;
    f.h.app.ci_sync_sandbox(&f.sandbox).await?;
    f.settle().await?;
    let (run, detail) = run_of(&f, &merged).await?;
    assert_eq!(run["state"], "passed", "{run}");
    let full = job_named(&detail, "full")?;
    assert_eq!(full["state"], "passed", "{detail}");
    assert!(
        full["output"]
            .as_str()
            .unwrap_or_default()
            .contains("full suite ran"),
        "the merged revision ran it, no tree reuse: {full}"
    );
    // One full-suite execution for this change: every run of the pull request's head skipped
    // it, and only the merged revision's ran it.
    let overview = f.get("/api/sandboxes/s1/ci").await?;
    let mut ran = Vec::new();
    for run in overview["runs"].as_array().ok_or("runs")? {
        if run["sha"] != head.as_str() && run["sha"] != merged.as_str() {
            continue;
        }
        let detail = f
            .get(&format!(
                "/api/sandboxes/s1/ci/runs/{}",
                run["id"].as_str().ok_or("id")?
            ))
            .await?;
        if job_named(&detail, "full")?["state"] == "passed" {
            ran.push(run["sha"].as_str().unwrap_or_default().to_owned());
        }
    }
    assert_eq!(ran, [merged], "{overview}");
    Ok(())
}

/// Whether `detail` (a run's API view) shows the full suite's output.
fn ran_the_full_suite(detail: &Value) -> bool {
    detail["jobs"].as_array().is_some_and(|jobs| {
        jobs.iter().any(|job| {
            job["output"]
                .as_str()
                .unwrap_or_default()
                .contains("full suite ran")
        })
    })
}

/// SPEC-575 MFS-002, review round 1: a pull request that turns the move off (it drops the
/// merge-first environment, or takes the job out of the full suite) is judged by the gate that
/// skips the full suite, so the integration branch's run at its squash commit runs it instead of
/// reusing that pull request's tree: the full suite runs once for the change, never nowhere.
#[tokio::test]
async fn turning_merge_first_off_still_runs_the_full_suite_on_the_merge() -> TestResult {
    let no_target = merge_first_toml(false);
    let no_flag = merge_first_toml(true).replace("after_merge = true\n", "");
    for own in [no_target, no_flag] {
        let f = fixture_with(false, |_| merge_first_toml(true)).await?;
        let head = f.push_feature("yulius.toml", Some(&own))?;
        f.h.app.ci_sync_sandbox(&f.sandbox).await?;
        f.settle().await?;
        let (_, on_pr) = run_of(&f, &head).await?;
        assert_eq!(job_named(&on_pr, "full")?["state"], "skipped", "{on_pr}");
        let merged = squash_feat_one(&f)?;
        f.h.app.ci_sync_sandbox(&f.sandbox).await?;
        f.settle().await?;
        let (run, on_dev) = run_of(&f, &merged).await?;
        assert_eq!(run["state"], "passed", "{run}");
        assert!(
            ran_the_full_suite(&on_dev),
            "the merged revision ran it, no reuse of a tree that skipped it: {on_dev}"
        );
    }
    Ok(())
}

/// SPEC-575 MFS-002, review round 1 (ADR-0360: a job a change adds runs before it merges): a
/// pull request that adopts merge-first judges itself by the integration head's gate, which has
/// no target yet, and its own new `after_merge` job runs beside it; a first pull request that
/// brings CI and merge-first together runs its whole suite.
#[tokio::test]
async fn adopting_merge_first_runs_the_full_suite_on_its_own_pull_request() -> TestResult {
    let f = fixture_with(false, |_| {
        merge_first_toml(false).replace("after_merge = true\n", "")
    })
    .await?;
    let mut own = merge_first_toml(true);
    own.push_str(
        "\n[[ci.jobs]]\nname = \"extra\"\ncommand = 'echo extra ran'\nafter_merge = true\n",
    );
    let head = f.push_feature("yulius.toml", Some(&own))?;
    f.h.app.ci_sync_sandbox(&f.sandbox).await?;
    f.settle().await?;
    let (run, detail) = run_of(&f, &head).await?;
    assert_eq!(run["state"], "passed", "{run}");
    assert_eq!(job_named(&detail, "full")?["state"], "passed", "{detail}");
    assert_eq!(
        job_named(&detail, "extra (this change)")?["state"],
        "passed",
        "{detail}"
    );

    let first = fixture_with(false, |_| String::new()).await?;
    let head = first.push_feature("yulius.toml", Some(&merge_first_toml(true)))?;
    first.h.app.ci_sync_sandbox(&first.sandbox).await?;
    first.settle().await?;
    let (run, detail) = run_of(&first, &head).await?;
    assert_eq!(run["state"], "passed", "{run}");
    assert_eq!(job_named(&detail, "full")?["state"], "passed", "{detail}");
    Ok(())
}

/// SPEC-575 MFS-002: a pull request whose every job is part of the full suite would merge on no
/// check at all, so the file is refused as configuration, naming why.
#[tokio::test]
async fn a_gate_of_only_after_merge_jobs_is_refused() -> TestResult {
    let only_full = merge_first_toml(true).replace(
        "name = \"quick\"\ncommand = \"true\"\n",
        "name = \"quick\"\ncommand = \"true\"\nafter_merge = true\n",
    );
    let f = fixture_with(false, |_| merge_first_toml(true)).await?;
    let head = f.push_feature("yulius.toml", Some(&only_full))?;
    f.h.app.ci_sync_sandbox(&f.sandbox).await?;
    f.settle().await?;
    let (run, detail) = run_of(&f, &head).await?;
    assert_eq!(run["state"], "failed", "{run}");
    let said = job_named(&detail, "yulius.toml")?["output"]
        .as_str()
        .unwrap_or_default()
        .to_owned();
    assert!(said.contains("quick check"), "{detail}");
    Ok(())
}

/// SPEC-575 MFS-002: without a merge-first environment (no shared staging target), the same
/// `after_merge` job runs on the pull request head: the project keeps its full suite there.
#[tokio::test]
async fn without_a_staging_target_the_full_suite_stays_on_the_pull_request() -> TestResult {
    let f = fixture_with(false, |_| merge_first_toml(false)).await?;
    f.h.app.ci_sync_sandbox(&f.sandbox).await?;
    f.settle().await?;
    let head = git(&f.origin, &["rev-parse", "feat/one"])?;
    let (run, detail) = run_of(&f, &head).await?;
    assert_eq!(run["state"], "passed", "{run}");
    let full = job_named(&detail, "full")?;
    assert_eq!(full["state"], "passed", "{detail}");
    assert!(
        full["output"]
            .as_str()
            .unwrap_or_default()
            .contains("full suite ran"),
        "{full}"
    );
    Ok(())
}

/// SPEC-504 review round 2: when Git cannot walk the integration branch's history for its last
/// valid gate (here a commit of it is missing from the clone), a pull request's verdict is
/// pending, never "no CI" (which would let a merge through on no checks).
#[tokio::test]
async fn a_history_git_cannot_walk_leaves_a_head_pending_not_unconfigured() -> TestResult {
    let f = fixture_with(false, |_| one_job("first")).await?;
    let broken = format!("[commands]\ngate = \"make ci\"\n\n{}", one_job("broken"));
    let middle = f.push_dev("yulius.toml", Some(&broken))?;
    f.push_dev("other.txt", Some("later\n"))?;
    let head = git(&f.origin, &["rev-parse", "feat/one"])?;
    let repo = f.data.join("sandboxes/s1/repo");
    git(&repo, &["fetch", "-q", "origin"])?;
    // The middle commit's object goes: walking back from dev's head meets it and fails.
    let object = repo
        .join(".git/objects")
        .join(&middle[..2])
        .join(&middle[2..]);
    std::fs::remove_file(&object).map_err(|e| format!("{}: {e}", object.display()))?;
    assert_eq!(
        f.h.app.ci_gate_result(&f.sandbox, &head).await,
        GateResult::Run(None),
        "pending until Git can tell"
    );
    Ok(())
}

/// SPEC-504 review round 1: an invalid file with no valid one before it names no fallback (the
/// page would otherwise say a gate runs while none does).
#[tokio::test]
async fn an_invalid_file_with_no_earlier_gate_names_no_fallback() -> TestResult {
    let f = fixture_with(false, |_| String::new()).await?;
    f.push_dev(
        "yulius.toml",
        Some(&format!(
            "[commands]\ngate = \"make ci\"\n\n{}",
            one_job("x")
        )),
    )?;
    f.h.app.ci_sync_sandbox(&f.sandbox).await?;
    let overview = f.get("/api/sandboxes/s1/ci").await?;
    assert!(overview["gate"].is_null(), "{overview}");
    assert!(overview["gate_fallback"].is_null(), "{overview}");
    assert!(
        overview["gate_problem"]
            .as_str()
            .is_some_and(|p| !p.is_empty())
    );
    Ok(())
}

/// SPEC-504 CFG-004, review round 1: an episode without a gate that ended while the server was
/// down is closed at the first look after the start, not counted on until the range's end.
#[tokio::test]
async fn an_episode_that_ended_while_the_server_was_down_is_closed() -> TestResult {
    use yulius_app::ports::store::AuditStore as _;
    let f = fixture_with(false, |_| one_job("ok")).await?;
    let now = f.h.app.now();
    f.h.store
        .audit(
            yulius_domain::time::Timestamp(now.0 - 30 * 60_000),
            "yulius",
            yulius_domain::merge_queue::CONFIG_BLOCKED,
            Some(&f.sandbox.slug),
            Some("yulius.toml line 1: broken"),
        )
        .await?;
    f.h.app.ci_sync_sandbox(&f.sandbox).await?;
    f.h.clock.0.fetch_add(60 * 60_000, Ordering::SeqCst);
    let day = |at: i64| {
        let text = yulius_domain::time::format_rfc3339(yulius_domain::time::Timestamp(at));
        text.get(..10).map(str::to_owned).ok_or("day")
    };
    let (from, to) = (day(now.0 - 30 * 60_000)?, day(f.h.app.now().0)?);
    let read = f
        .get(&format!(
            "/api/analytics/merge-queue?sandbox=s1&from={from}&to={to}"
        ))
        .await?;
    assert_eq!(read["config_blocked_minutes"], 30, "{read}");
    Ok(())
}

/// SPEC-504 CFG-002, review round 1: a pull request refused only because the integration
/// branch's file was invalid runs again once that file is valid, without a push.
#[tokio::test]
async fn a_head_refused_for_the_branchs_file_runs_again_once_it_is_valid() -> TestResult {
    let f = fixture_with(false, |_| one_job("first")).await?;
    let broken = format!("[commands]\ngate = \"make ci\"\n\n{}", one_job("broken"));
    f.push_dev("yulius.toml", Some(&broken))?;
    let head = git(&f.origin, &["rev-parse", "feat/one"])?;
    f.h.app.ci_sync_sandbox(&f.sandbox).await?;
    f.settle().await?;
    assert_eq!(run_of_sha(&f, &head).await?["state"], "failed");
    f.push_dev("yulius.toml", Some(&one_job("fixed")))?;
    f.h.app.ci_sync_sandbox(&f.sandbox).await?;
    f.settle().await?;
    let again = run_of_sha(&f, &head).await?;
    assert_eq!(again["state"], "passed", "{again}");
    Ok(())
}

/// Repairing dev invalidates a later configuration refusal even when this
/// same PR head has an older passing ordinary run.
#[tokio::test]
async fn a_second_integration_configuration_repair_tests_the_refused_head_once() -> TestResult {
    let f = fixture_with(false, |_| one_job("first")).await?;
    let head = git(&f.origin, &["rev-parse", "feat/one"])?;
    f.h.app.ci_sync_sandbox(&f.sandbox).await?;
    f.settle().await?;
    let passed = run_of_sha(&f, &head).await?;
    assert_eq!(passed["state"], "passed");
    let id = passed["id"].as_str().ok_or("passed CI run")?;
    let broken = format!("[commands]\ngate = \"make ci\"\n\n{}", one_job("broken"));
    f.push_dev("yulius.toml", Some(&broken))?;
    f.h.app.ci_sync_sandbox(&f.sandbox).await?;
    let rerun = f
        .post(&format!("/api/sandboxes/s1/ci/runs/{id}/rerun"), None)
        .await?;
    assert_eq!(rerun.status, StatusCode::OK, "{}", rerun.body);
    f.settle().await?;
    let refusal = run_of_sha(&f, &head).await?;
    assert_eq!(refusal["state"], "failed", "{refusal}");
    assert!(
        refusal["reason"]
            .as_str()
            .is_some_and(|why| why.starts_with("yulius.toml is invalid on")),
        "{refusal}"
    );

    f.push_dev("yulius.toml", Some(&one_job("repaired")))?;
    f.h.app.ci_sync_sandbox(&f.sandbox).await?;
    let replacement = run_of_sha(&f, &head).await?;
    assert_eq!(replacement["state"], "queued", "{replacement}");
    f.h.app.ci_sync_sandbox(&f.sandbox).await?;
    f.settle().await?;
    let after = run_of_sha(&f, &head).await?;
    assert_eq!(after["id"], replacement["id"]);
    assert_eq!(after["state"], "passed", "{after}");
    f.h.app.ci_sync_sandbox(&f.sandbox).await?;
    assert_eq!(run_of_sha(&f, &head).await?["id"], replacement["id"]);
    Ok(())
}

/// SPEC-504 review round 3 (major): a pull request that changes only a job's time limit runs
/// its twin with the new limit (a change to how a job runs is a change), and a twin is told the
/// job's own name in `YULIUS_CI_JOB`.
#[tokio::test]
async fn a_change_to_a_jobs_time_limit_runs_its_twin() -> TestResult {
    use yulius_app::ports::store_ext::CiStore as _;
    let job = |limit: u32| {
        format!(
            "[[ci.jobs]]\nname = \"check\"\ntimeout_minutes = {limit}\ncommand = 'echo \"job=$YULIUS_CI_JOB\"'\n"
        )
    };
    let f = fixture_with(false, move |_| job(30)).await?;
    let sha = f.push_feature("yulius.toml", Some(&job(20)))?;
    f.h.app.ci_sync_sandbox(&f.sandbox).await?;
    f.settle().await?;
    let outputs = job_outputs(&f, &sha).await?;
    let names: Vec<&str> = outputs.iter().map(|(n, _)| n.as_str()).collect();
    assert_eq!(names, ["check", "check (this change)"], "{outputs:?}");
    assert!(
        output_of(&outputs, "check (this change)")?.contains("job=check\n"),
        "{outputs:?}"
    );
    let run = run_of_sha(&f, &sha).await?;
    let jobs =
        f.h.store
            .ci_jobs(run["id"].as_str().ok_or("id")?, false)
            .await?;
    let limits: Vec<(String, Option<u32>)> = jobs
        .iter()
        .map(|j| (j.name.clone(), j.timeout_minutes))
        .collect();
    assert_eq!(
        limits,
        [
            ("check".to_owned(), Some(30)),
            ("check (this change)".to_owned(), Some(20))
        ]
    );
    Ok(())
}

/// SPEC-504 review round 3: a branch's run that reuses a same-tree verdict takes each job's
/// verdict from the job that ran its command: a change's twin of it, not the integration
/// branch's job of the same key.
#[tokio::test]
async fn a_reused_verdict_comes_from_the_job_that_ran_the_command() -> TestResult {
    use yulius_app::ci::{CiJobRecord, CiRunRecord};
    use yulius_app::ports::store_ext::CiStore as _;
    use yulius_domain::ci::{CiRef, GateSource, RunState};
    use yulius_domain::time::Timestamp;
    let f = fixture_with(false, |_| one_job("old")).await?;
    let changed = f.push_feature("yulius.toml", Some(&one_job("new")))?;
    let run = CiRunRecord {
        id: "run-twins".into(),
        sandbox_id: SandboxId::parse("s1")?,
        reference: CiRef::PullRequest(1),
        sha: changed.clone(),
        source: GateSource::ProjectConfig,
        state: RunState::Passed,
        reason: None,
        requested_by: "yulius".into(),
        status_for: Some(RunState::Passed),
        status_note: Some("recorded by the test".into()),
        status_attempts: 0,
        queued_at: Timestamp(1),
        started_at: Some(Timestamp(1)),
        ended_at: Some(Timestamp(2)),
    };
    let job = |ordinal: u32, name: &str, command: &str, state: RunState| CiJobRecord {
        id: format!("j-{ordinal}"),
        run_id: "run-twins".into(),
        ordinal,
        name: name.into(),
        key: "check".into(),
        command: command.into(),
        matrix: None,
        state,
        reason: None,
        exit_code: Some(if state == RunState::Skipped { 78 } else { 0 }),
        output: String::new(),
        failures: None,
        started_at: Some(Timestamp(1)),
        ended_at: Some(Timestamp(2)),
        need: Default::default(),
        timeout_minutes: None,
        exclusive: false,
        waited_ms: None,
    };
    f.h.store
        .insert_ci_run(
            &run,
            &[
                job(0, "check", "echo old", RunState::Passed),
                job(1, "check (this change)", "echo new", RunState::Skipped),
            ],
        )
        .await?;
    git(&f.work, &["checkout", "-q", "-b", "dev", "origin/dev"])?;
    git(&f.work, &["merge", "-q", "--squash", "feat/one"])?;
    git(&f.work, &["commit", "-q", "-m", "change the job (#1)"])?;
    git(&f.work, &["push", "-q", "origin", "dev"])?;
    let merged = git(&f.work, &["rev-parse", "HEAD"])?;
    git(&f.work, &["checkout", "-q", "feat/one"])?;
    f.h.app.ci_sync_sandbox(&f.sandbox).await?;
    f.settle().await?;
    let (run, detail) = run_of(&f, &merged).await?;
    assert!(
        run["reason"]
            .as_str()
            .is_some_and(|r| r.contains("same tree")),
        "reused: {run}"
    );
    assert_eq!(job_named(&detail, "check")?["state"], "skipped", "{detail}");
    Ok(())
}

/// SPEC-504 review round 1 (major): a squash merge that changed a job lands on the integration
/// branch with a tree some pull request's run passed, but that run never ran the job as the
/// merge now defines it: the branch's run runs again instead of taking that verdict, and the
/// new command runs.
#[tokio::test]
async fn a_branch_never_reuses_a_run_that_did_not_run_its_jobs() -> TestResult {
    use yulius_app::ci::{CiJobRecord, CiRunRecord};
    use yulius_app::ports::store_ext::CiStore as _;
    use yulius_domain::ci::{CiRef, GateSource, RunState};
    use yulius_domain::time::Timestamp;
    let f = fixture_with(false, |_| one_job("old")).await?;
    let changed = f.push_feature("yulius.toml", Some(&one_job("new")))?;
    // A passed run of that head that ran the old command only (as before SPEC-504 CFG-001).
    let run = CiRunRecord {
        id: "run-old-jobs".into(),
        sandbox_id: SandboxId::parse("s1")?,
        reference: CiRef::PullRequest(1),
        sha: changed.clone(),
        source: GateSource::ProjectConfig,
        state: RunState::Passed,
        reason: None,
        requested_by: "yulius".into(),
        status_for: Some(RunState::Passed),
        status_note: Some("recorded by the test".into()),
        status_attempts: 0,
        queued_at: Timestamp(1),
        started_at: Some(Timestamp(1)),
        ended_at: Some(Timestamp(2)),
    };
    let job = CiJobRecord {
        id: "old-check".into(),
        run_id: "run-old-jobs".into(),
        ordinal: 0,
        name: "check".into(),
        key: "check".into(),
        command: "echo old".into(),
        matrix: None,
        state: RunState::Passed,
        reason: None,
        exit_code: Some(0),
        output: String::new(),
        failures: None,
        started_at: Some(Timestamp(1)),
        ended_at: Some(Timestamp(2)),
        need: Default::default(),
        timeout_minutes: None,
        exclusive: false,
        waited_ms: None,
    };
    f.h.store.insert_ci_run(&run, &[job]).await?;
    git(&f.work, &["checkout", "-q", "-b", "dev", "origin/dev"])?;
    git(&f.work, &["merge", "-q", "--squash", "feat/one"])?;
    git(&f.work, &["commit", "-q", "-m", "change the job (#1)"])?;
    git(&f.work, &["push", "-q", "origin", "dev"])?;
    let merged = git(&f.work, &["rev-parse", "HEAD"])?;
    git(&f.work, &["checkout", "-q", "feat/one"])?;
    f.h.app.ci_sync_sandbox(&f.sandbox).await?;
    f.settle().await?;
    let (run, _) = run_of(&f, &merged).await?;
    assert!(
        !run["reason"]
            .as_str()
            .is_some_and(|r| r.contains("same tree")),
        "not reused: {run}"
    );
    let outputs = job_outputs(&f, &merged).await?;
    assert!(output_of(&outputs, "check")?.contains("new"), "{outputs:?}");
    Ok(())
}

/// SPEC-504 review round 4: a branch reuses a same-tree verdict only where every job of its
/// gate ran as it now defines it, its time limit too: a pull request's run that ran the job
/// with the old limit only is no source, and the branch's run runs it again.
#[tokio::test]
async fn a_branch_never_reuses_a_run_that_ran_its_job_with_another_time_limit() -> TestResult {
    use yulius_domain::ci::RunState;
    let f = fixture_with(false, |_| limited_job(30)).await?;
    let changed = f.push_feature("yulius.toml", Some(&limited_job(20)))?;
    let merged = squash_after_run(&f, &changed, &[("check", Some(30), RunState::Passed)]).await?;
    let (run, _) = run_of(&f, &merged).await?;
    assert!(
        !run["reason"]
            .as_str()
            .is_some_and(|r| r.contains("same tree")),
        "not reused: {run}"
    );
    Ok(())
}

/// SPEC-504 review round 4: a reused verdict comes from the job that ran as the branch now
/// defines it, here the twin with the new time limit, not the job of the same command with the
/// old one.
#[tokio::test]
async fn a_reused_verdict_comes_from_the_job_that_ran_with_the_time_limit() -> TestResult {
    use yulius_domain::ci::RunState;
    let f = fixture_with(false, |_| limited_job(30)).await?;
    let changed = f.push_feature("yulius.toml", Some(&limited_job(20)))?;
    let merged = squash_after_run(
        &f,
        &changed,
        &[
            ("check", Some(30), RunState::Passed),
            ("check (this change)", Some(20), RunState::Skipped),
        ],
    )
    .await?;
    let (run, detail) = run_of(&f, &merged).await?;
    assert!(
        run["reason"]
            .as_str()
            .is_some_and(|r| r.contains("same tree")),
        "reused: {run}"
    );
    assert_eq!(job_named(&detail, "check")?["state"], "skipped", "{detail}");
    Ok(())
}

/// One job `check` running `echo same` within `limit` minutes.
fn limited_job(limit: u32) -> String {
    format!("[[ci.jobs]]\nname = \"check\"\ntimeout_minutes = {limit}\ncommand = \"echo same\"\n")
}

/// Records a passed pull-request run of `changed` with `jobs` (name, time limit, verdict; all
/// `check` running `echo same`), squash-merges `feat/one` into `dev` and syncs: the merged head.
async fn squash_after_run(
    f: &Fixture,
    changed: &str,
    jobs: &[(&str, Option<u32>, yulius_domain::ci::RunState)],
) -> BoxResult<String> {
    use yulius_app::ci::{CiJobRecord, CiRunRecord};
    use yulius_app::ports::store_ext::CiStore as _;
    use yulius_domain::ci::{CiRef, GateSource, RunState};
    use yulius_domain::time::Timestamp;
    let run = CiRunRecord {
        id: "run-source".into(),
        sandbox_id: SandboxId::parse("s1")?,
        reference: CiRef::PullRequest(1),
        sha: changed.to_owned(),
        source: GateSource::ProjectConfig,
        state: RunState::Passed,
        reason: None,
        requested_by: "yulius".into(),
        status_for: Some(RunState::Passed),
        status_note: Some("recorded by the test".into()),
        status_attempts: 0,
        queued_at: Timestamp(1),
        started_at: Some(Timestamp(1)),
        ended_at: Some(Timestamp(2)),
    };
    let records: Vec<CiJobRecord> = jobs
        .iter()
        .zip(0u32..)
        .map(|(&(name, timeout_minutes, state), ordinal)| CiJobRecord {
            id: format!("source-{ordinal}"),
            run_id: "run-source".into(),
            ordinal,
            name: name.into(),
            key: "check".into(),
            command: "echo same".into(),
            matrix: None,
            state,
            reason: None,
            exit_code: Some(if state == RunState::Skipped { 78 } else { 0 }),
            output: String::new(),
            failures: None,
            started_at: Some(Timestamp(1)),
            ended_at: Some(Timestamp(2)),
            need: Default::default(),
            timeout_minutes,
            exclusive: false,
            waited_ms: None,
        })
        .collect();
    f.h.store.insert_ci_run(&run, &records).await?;
    git(&f.work, &["checkout", "-q", "-b", "dev", "origin/dev"])?;
    git(&f.work, &["merge", "-q", "--squash", "feat/one"])?;
    git(
        &f.work,
        &["commit", "-q", "-m", "change the time limit (#1)"],
    )?;
    git(&f.work, &["push", "-q", "origin", "dev"])?;
    let merged = git(&f.work, &["rev-parse", "HEAD"])?;
    git(&f.work, &["checkout", "-q", "feat/one"])?;
    f.h.app.ci_sync_sandbox(&f.sandbox).await?;
    f.settle().await?;
    Ok(merged)
}

/// Review round 3 of SPEC-493 fix 5: tree reuse into the integration branch's run looks past a
/// same-tree source that skipped a `paths` job, to an older one that ran it.
#[tokio::test]
async fn a_branch_reuses_a_same_tree_run_that_ran_its_paths_job() -> TestResult {
    use yulius_app::ci::{CiJobRecord, CiRunRecord};
    use yulius_app::ports::store_ext::CiStore as _;
    use yulius_domain::ci::{CiRef, GateSource, RunState};
    use yulius_domain::time::Timestamp;
    let f = fixture_with(false, |_| {
        "[[ci.jobs]]\nname = \"check\"\ncommand = \"true\"\n\n\
         [[ci.jobs]]\nname = \"web\"\ncommand = 'echo web ran'\npaths = [\"crates/a/**\"]\n"
            .to_owned()
    })
    .await?;
    f.h.app.ci_sync_sandbox(&f.sandbox).await?;
    f.settle().await?;
    // Another commit with the pull request's tree, whose (older) passed run ran `web`.
    let tree = git(&f.origin, &["rev-parse", "feat/one^{tree}"])?;
    let alt = git(
        &f.work,
        &["commit-tree", &tree, "-p", "origin/dev", "-m", "alt"],
    )?;
    git(
        &f.work,
        &["push", "-q", "origin", &format!("{alt}:refs/heads/alt")],
    )?;
    let run = CiRunRecord {
        id: "run-alt".into(),
        sandbox_id: SandboxId::parse("s1")?,
        reference: CiRef::Branch("alt".into()),
        sha: alt.clone(),
        source: GateSource::ProjectConfig,
        state: RunState::Passed,
        reason: None,
        requested_by: "yulius".into(),
        status_for: Some(RunState::Passed),
        status_note: Some("recorded by the test".into()),
        status_attempts: 0,
        queued_at: Timestamp(1),
        started_at: Some(Timestamp(1)),
        ended_at: Some(Timestamp(2)),
    };
    let job = |ordinal: u32, name: &str| CiJobRecord {
        id: format!("alt-{name}"),
        run_id: "run-alt".into(),
        ordinal,
        name: name.into(),
        key: name.into(),
        // As the job ran it (a reuse needs every job's command to have run, SPEC-504).
        command: if name == "web" {
            "echo web ran"
        } else {
            "true"
        }
        .into(),
        matrix: None,
        state: RunState::Passed,
        reason: None,
        exit_code: Some(0),
        output: String::new(),
        failures: None,
        started_at: Some(Timestamp(1)),
        ended_at: Some(Timestamp(2)),
        need: Default::default(),
        timeout_minutes: None,
        exclusive: false,
        waited_ms: None,
    };
    f.h.store
        .insert_ci_run(&run, &[job(0, "check"), job(1, "web")])
        .await?;
    git(&f.work, &["checkout", "-q", "-b", "dev", "origin/dev"])?;
    git(&f.work, &["merge", "-q", "--squash", "feat/one"])?;
    git(&f.work, &["commit", "-q", "-m", "feature (#1)"])?;
    git(&f.work, &["push", "-q", "origin", "dev"])?;
    let merged = git(&f.work, &["rev-parse", "HEAD"])?;
    f.h.app.ci_sync_sandbox(&f.sandbox).await?;
    f.settle().await?;
    let (run, detail) = run_of(&f, &merged).await?;
    assert!(
        run["reason"]
            .as_str()
            .is_some_and(|r| r.contains(&alt[..7])),
        "reused from the run that ran web: {run}"
    );
    assert_eq!(job_named(&detail, "web")?["state"], "passed", "{detail}");
    Ok(())
}

/// Review round 1 of SPEC-493 fix 5: a job skipped for its `paths` waits for nothing: not for its
/// `exclusive` turn while another run's job of that name runs, nor for a slot.
#[tokio::test]
async fn a_skipped_exclusive_job_does_not_wait_for_its_turn() -> TestResult {
    let f = fixture_tuned(
        false,
        |_| {
            // Each job a CPU, so the runs' budgets fit beside each other and only the turn of
            // `web` could make the pull request's run wait.
            "[[ci.jobs]]\nname = \"check\"\ncommand = \"true\"\ncpus = 1\n\n\
             [[ci.jobs]]\nname = \"web\"\ncommand = 'sleep 6; echo web ran'\nexclusive = true\n\
             cpus = 1\npaths = [\"crates/a/**\"]\n"
                .to_owned()
        },
        None,
        |config| config.ci.lanes = 3,
    )
    .await?;
    // The integration branch's run takes `web`'s turn for 6 s; the pull request skips it.
    let dev = f.push_dev("other.txt", Some("x\n"))?;
    f.h.app.ci_sync_sandbox(&f.sandbox).await?;
    let head = git(&f.origin, &["rev-parse", "feat/one"])?;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
    loop {
        f.h.app.ci_dispatch().await;
        if let Ok((run, _)) = run_of(&f, &head).await
            && run["state"] != "queued"
            && run["state"] != "running"
        {
            assert_eq!(run["state"], "passed", "{run}");
            break;
        }
        if tokio::time::Instant::now() > deadline {
            return Err("the pull request's run did not end".into());
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let (dev_run, _) = run_of(&f, &dev).await?;
    assert_eq!(
        dev_run["state"], "running",
        "the pull request ended while dev's web still held its turn: {dev_run}"
    );
    f.settle().await?;
    Ok(())
}

/// SPEC-359 VSB-006: a job is told where its sandbox's Vidoc switch is, so a scan whose key
/// fails points there.
#[tokio::test]
async fn a_job_is_told_where_the_sandboxs_vidoc_switch_is() -> TestResult {
    let f = fixture_with(false, |_| {
        "[[ci.jobs]]\nname = \"where\"\ncommand = 'echo \"switch=[$YULIUS_VIDOC_SWITCH]\"'\n"
            .to_owned()
    })
    .await?;
    f.h.app.ci_sync_sandbox(&f.sandbox).await?;
    f.settle().await?;
    let head = git(&f.work, &["rev-parse", "HEAD"])?;
    let outputs = job_outputs(&f, &head).await?;
    let expected = format!("switch=[{}/s/app/ci]", f.h.app.config.public_url);
    assert!(
        output_of(&outputs, "where")?.contains(&expected),
        "{outputs:?}"
    );
    Ok(())
}

/// SPEC-273 NOI-006: a job that exits `$YULIUS_CI_INCOMPLETE_CODE` (a security scan Vidoc's
/// outage cut short) ends error with the files it names, never failed: the status says
/// incomplete, the merge is blocked, and the other job still ends passed.
#[tokio::test]
async fn a_job_that_exits_the_incomplete_code_ends_in_error_naming_what_it_missed() -> TestResult {
    let f = fixture_with(false, |_| {
        r#"[[ci.jobs]]
name = "check"
command = "true"

[[ci.jobs]]
name = "security"
command = 'echo "::yulius-incomplete 2 file(s) not scanned (a.rs, b.rs): HTTP 503"; exit "${YULIUS_CI_INCOMPLETE_CODE:-1}"'
"#
        .to_owned()
    })
    .await?;
    let pr_sha = git(&f.origin, &["rev-parse", "feat/one"])?;
    f.h.app.ci_sync_sandbox(&f.sandbox).await?;
    f.settle().await?;
    let overview = f.get("/api/sandboxes/s1/ci").await?;
    let run = overview["runs"]
        .as_array()
        .ok_or("runs")?
        .iter()
        .find(|r| r["reference"] == "pr/1")
        .cloned()
        .ok_or("pr/1 run")?;
    assert_eq!(run["state"], "error", "{run}");
    let detail = f
        .get(&format!(
            "/api/sandboxes/s1/ci/runs/{}",
            run["id"].as_str().ok_or("id")?
        ))
        .await?;
    let jobs = detail["jobs"].as_array().ok_or("jobs")?;
    let job = |name: &str| jobs.iter().find(|j| j["name"] == name).ok_or("job");
    assert_eq!(job("check")?["state"], "passed", "{detail}");
    assert_eq!(job("security")?["state"], "error", "{detail}");
    assert_eq!(job("security")?["exit_code"], 75, "{detail}");
    assert_eq!(
        job("security")?["reason"],
        "incomplete: 2 file(s) not scanned (a.rs, b.rs): HTTP 503; infrastructure, not a \
         finding: run it again",
        "{detail}"
    );
    assert_eq!(
        f.h.app.ci_gate_result(&f.sandbox, &pr_sha).await,
        GateResult::Run(Some(RunState::Error))
    );
    let (state, description, _) = f.last_status(&pr_sha).ok_or("status")?;
    assert_eq!(state, "error", "{description}");
    assert!(
        description.starts_with("Error: security: incomplete: 2 file(s) not scanned (a.rs, b.rs)"),
        "{description}"
    );
    Ok(())
}

impl Fixture {
    /// Dispatches until every named job of `sha`'s run is in its state; returns the jobs.
    async fn until_jobs(&self, sha: &str, states: &[(&str, &str)]) -> BoxResult<Vec<Value>> {
        let deadline = tokio::time::Instant::now() + WAIT;
        loop {
            self.h.app.ci_dispatch().await;
            let overview = self.get("/api/sandboxes/s1/ci").await?;
            if let Some(run) = overview["runs"]
                .as_array()
                .ok_or("runs")?
                .iter()
                .find(|r| r["sha"] == sha)
            {
                let detail = self
                    .get(&format!(
                        "/api/sandboxes/s1/ci/runs/{}",
                        run["id"].as_str().ok_or("id")?
                    ))
                    .await?;
                let jobs = detail["jobs"].as_array().ok_or("jobs")?.clone();
                let state_of = |name: &str| {
                    jobs.iter()
                        .find(|j| j["name"] == name)
                        .map(|j| j["state"].clone())
                };
                if states
                    .iter()
                    .all(|(name, state)| state_of(name).is_some_and(|s| s == *state))
                {
                    return Ok(jobs);
                }
                if tokio::time::Instant::now() > deadline {
                    return Err(format!("jobs never reached {states:?}: {detail}").into());
                }
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }
}

/// SPEC-273 NOI-007: a failed `security` job no longer stops the other jobs ("stopped early: job
/// security failed" left a commit without a test result), and a failed test job no longer stops
/// the scan; both run to the end and report. Fail-fast among the other jobs stays.
#[tokio::test]
async fn the_security_job_and_the_others_never_cancel_each_other() -> TestResult {
    let f = fixture_tuned(
        false,
        |data| {
            format!(
                r#"[[ci.jobs]]
name = "security"
command = 'if [ -f fail-security ]; then exit 1; fi; while [ ! -f "{release}" ]; do sleep 0.05; done'

[[ci.jobs]]
name = "quality"
command = 'if [ -f fail-quality ]; then exit 1; fi; while [ ! -f "{release}" ]; do sleep 0.05; done'

[[ci.jobs]]
name = "e2e"
command = 'while [ ! -f "{release}" ]; do sleep 0.05; done'
"#,
                release = data.join("release").display()
            )
        },
        None,
        // Only the test ends a held job: the fixture's 8 s limit ended the jobs before a slow
        // host had shown them running (#1079).
        |config| config.ci.job_timeout = Duration::from_secs(3600),
    )
    .await?;
    let release = f.data.join("release");
    // The other runs go first, released: the integration branch's and the pull request's at its
    // present head. Held, the branch's three jobs kept the job slots from the run under test
    // until their limit ended them (#1079).
    std::fs::write(&release, "")?;
    f.h.app.ci_sync_sandbox(&f.sandbox).await?;
    f.settle().await?;
    f.all_passed(2).await?;
    std::fs::remove_file(&release)?;
    // The scan fails at once; quality and e2e are still running when it does.
    let first = f.push_feature("fail-security", Some("x\n"))?;
    f.h.app.ci_sync_sandbox(&f.sandbox).await?;
    // Only this run will hold jobs: every other run has passed, so none competes with it for the
    // job slots (#1079).
    let overview = f.get("/api/sandboxes/s1/ci").await?;
    assert!(
        overview["runs"]
            .as_array()
            .ok_or("runs")?
            .iter()
            .all(|r| r["sha"] == first.as_str() || r["state"] == "passed"),
        "every other run must have passed before this one holds its jobs: {overview}"
    );
    f.until_jobs(
        &first,
        &[
            ("security", "failed"),
            ("quality", "running"),
            ("e2e", "running"),
        ],
    )
    .await?;
    // GitHub hears of the failed scan at once, while the tests run (review round 1).
    let deadline = tokio::time::Instant::now() + WAIT;
    while f.last_status(&first).map(|(_, d, _)| d)
        != Some("Running on Yulius: 0 of 3 jobs passed, security failed".to_owned())
    {
        assert!(
            tokio::time::Instant::now() < deadline,
            "{:?}",
            f.h.github.statuses()
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    std::fs::write(&release, "")?;
    f.settle().await?;
    f.until_jobs(
        &first,
        &[
            ("security", "failed"),
            ("quality", "passed"),
            ("e2e", "passed"),
        ],
    )
    .await?;
    let (state, description, _) = f.last_status(&first).ok_or("status")?;
    assert_eq!(state, "failure", "{description}");
    assert!(
        description.contains("security (exit code 1)"),
        "{description}"
    );

    // Quality fails at once: e2e stops early as before, the scan runs to its end.
    std::fs::remove_file(&release)?;
    f.push_feature("fail-security", None)?;
    let second = f.push_feature("fail-quality", Some("x\n"))?;
    f.h.app.ci_sync_sandbox(&f.sandbox).await?;
    let jobs = f
        .until_jobs(
            &second,
            &[
                ("quality", "failed"),
                ("e2e", "cancelled"),
                ("security", "running"),
            ],
        )
        .await?;
    let e2e = jobs.iter().find(|j| j["name"] == "e2e").ok_or("e2e")?;
    assert_eq!(e2e["reason"], "stopped early: job quality failed", "{e2e}");
    std::fs::write(&release, "")?;
    f.settle().await?;
    f.until_jobs(
        &second,
        &[
            ("quality", "failed"),
            ("e2e", "cancelled"),
            ("security", "passed"),
        ],
    )
    .await?;
    Ok(())
}

/// While a run is going, its commit status counts the jobs that already ended (SPEC-198 RCS-001):
/// GitHub said "0 of 3 jobs passed" for PR #600 long after its security scan had passed, and the
/// Superagent took the scan for one still running (#604). The final status still lands last.
#[tokio::test]
async fn a_running_run_s_status_counts_the_jobs_that_ended() -> TestResult {
    let f = fixture_with(false, held_toml).await?;
    let pr_sha = git(&f.origin, &["rev-parse", "feat/one"])?;
    f.h.app.ci_sync_sandbox(&f.sandbox).await?;
    f.until_counted(&pr_sha).await?;
    std::fs::write(f.data.join("release"), "")?;
    f.settle().await?;
    let (state, description, _) = f.last_status(&pr_sha).ok_or("status")?;
    assert_eq!(state, "success", "{description}");
    assert!(
        description.starts_with("Passed: 3 jobs, 1 skipped"),
        "{description}"
    );
    // The last job's end leaves the status to the run's end: no "2 of 3" just before it.
    let statuses = f.h.github.statuses();
    assert!(
        !statuses
            .iter()
            .any(|(sha, st)| *sha == pr_sha && st.description.contains("2 of 3")),
        "{statuses:?}"
    );
    Ok(())
}

/// The statuses published as jobs end use no retry attempt of the run's final status: one that
/// GitHub refused is published again by the retry loop (SPEC-198 RCS-004).
#[tokio::test]
async fn a_refused_final_status_is_retried_after_the_job_statuses() -> TestResult {
    let f = fixture_with(false, held_toml).await?;
    let pr_sha = git(&f.origin, &["rev-parse", "feat/one"])?;
    f.h.app.ci_sync_sandbox(&f.sandbox).await?;
    f.until_counted(&pr_sha).await?;
    let forbidden = &f.h.github.statuses_forbidden;
    forbidden.store(true, std::sync::atomic::Ordering::SeqCst);
    std::fs::write(f.data.join("release"), "")?;
    f.settle().await?;
    let (state, description, _) = f.last_status(&pr_sha).ok_or("status")?;
    assert_eq!(
        state, "pending",
        "the final status was refused: {description}"
    );
    forbidden.store(false, std::sync::atomic::Ordering::SeqCst);
    f.h.app.ci_retry_statuses().await;
    let (state, description, _) = f.last_status(&pr_sha).ok_or("status")?;
    assert_eq!(state, "success", "{:?}", f.h.github.statuses());
    assert!(
        description.starts_with("Passed: 3 jobs, 1 skipped"),
        "{description}"
    );
    Ok(())
}

/// A passing job, a skipped one and one held until `<data>/release` exists.
fn held_toml(data: &Path) -> String {
    format!(
        r#"[[ci.jobs]]
name = "check"
command = "true"

[[ci.jobs]]
name = "scan"
command = 'exit "$YULIUS_CI_SKIP_CODE"'

[[ci.jobs]]
name = "held"
command = 'while [ ! -f "{}" ]; do sleep 0.05; done'
"#,
        data.join("release").display()
    )
}

impl Fixture {
    /// Dispatches until `sha`'s running status counts the passed and the skipped job of
    /// [`held_toml`] while `held` still runs.
    async fn until_counted(&self, sha: &str) -> BoxResult<()> {
        let deadline = tokio::time::Instant::now() + WAIT;
        loop {
            self.h.app.ci_dispatch().await;
            if let Some((state, description, _)) = self.last_status(sha) {
                if description == "Running on Yulius: 1 of 3 jobs passed, 1 skipped" {
                    assert_eq!(state, "pending", "{description}");
                    return Ok(());
                }
                // `held` timed out: the run ended without ever counting its ended jobs.
                assert_eq!(state, "pending", "{:?}", self.h.github.statuses());
            }
            if tokio::time::Instant::now() > deadline {
                return Err(format!(
                    "no status counted the ended jobs: {:?}",
                    self.h.github.statuses()
                )
                .into());
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }
}

/// A squash merge of an up-to-date pull request has exactly the tree its run tested: the
/// integration branch reuses that result instead of running everything again (F 2026-10-12:
/// every merge cost a second run in the sandbox's one CI lane).
#[tokio::test]
async fn a_merge_with_a_tested_tree_reuses_its_result() -> TestResult {
    let f = fixture().await?;
    f.h.app.ci_sync_sandbox(&f.sandbox).await?;
    f.settle().await?;
    let runs = f.get("/api/sandboxes/s1/ci").await?;
    assert!(
        runs["runs"]
            .as_array()
            .ok_or("runs")?
            .iter()
            .any(|r| r["reference"] == "pr/1" && r["state"] == "passed"),
        "{runs}"
    );

    // Squash-merge feat/one into dev: a new commit with the pull request head's tree.
    git(&f.work, &["checkout", "-q", "-b", "dev", "origin/dev"])?;
    git(&f.work, &["merge", "-q", "--squash", "feat/one"])?;
    git(&f.work, &["commit", "-q", "-m", "feature (#1)"])?;
    git(&f.work, &["push", "-q", "origin", "dev"])?;
    let merged = git(&f.work, &["rev-parse", "HEAD"])?;
    f.h.app.ci_sync_sandbox(&f.sandbox).await?;

    let overview = f.get("/api/sandboxes/s1/ci").await?;
    let dev_run = overview["runs"]
        .as_array()
        .ok_or("runs")?
        .iter()
        .find(|r| r["sha"] == merged.as_str())
        .cloned()
        .ok_or("dev run")?;
    assert_eq!(dev_run["state"], "passed", "{dev_run}");
    let detail = f
        .get(&format!(
            "/api/sandboxes/s1/ci/runs/{}",
            dev_run["id"].as_str().ok_or("id")?
        ))
        .await?;
    let output = detail["jobs"][0]["output"].as_str().unwrap_or_default();
    assert!(output.contains("not run again"), "{output}");
    assert_eq!(f.last_status(&merged).ok_or("status")?.0, "success");
    Ok(())
}

/// A sandbox's disk as the test sets it: starts are refused over the quota. The next `refusals`
/// starts find usage crossed it (between the dispatcher's check and a job's start): the reading
/// goes to 126 GB and they are refused.
struct Disk {
    used: AtomicU64,
    measurements: AtomicU32,
    refusals: AtomicU32,
}

const GB: u64 = 1_000_000_000;

impl Disk {
    fn new(used_gb: u64, refusals: u32) -> Arc<Self> {
        Arc::new(Self {
            used: AtomicU64::new(used_gb * GB),
            measurements: AtomicU32::new(0),
            refusals: AtomicU32::new(refusals),
        })
    }
}

#[async_trait::async_trait]
impl SandboxIsolation for Disk {
    async fn prepare(&self, _: &SandboxRef) -> AppResult<Option<SandboxJoin>> {
        let refuse = self
            .refusals
            .try_update(Ordering::SeqCst, Ordering::SeqCst, |n| n.checked_sub(1))
            .is_ok();
        if refuse {
            self.used.store(126 * GB, Ordering::SeqCst);
        }
        if self.used.load(Ordering::SeqCst) > 100 * GB {
            return Err(AppError::Conflict(
                "this sandbox uses too much of its disk quota".into(),
            ));
        }
        Ok(None)
    }
    async fn disk(&self, _: &SandboxRef) -> Option<DiskUsage> {
        Some(DiskUsage {
            used: self.used.load(Ordering::SeqCst),
            limit: 100 * GB,
        })
    }
    async fn measure_disk(&self, r: &SandboxRef) -> Option<DiskUsage> {
        self.measurements.fetch_add(1, Ordering::SeqCst);
        self.disk(r).await
    }
    async fn largest(&self, _: &SandboxRef) -> Vec<(String, u64)> {
        vec![("worktrees/elsewhere".into(), 35 * GB)]
    }
    fn release(&self, _: &SandboxId) {}
}

impl Fixture {
    /// Dispatches until every run is queued with a reason containing `words`.
    async fn all_waiting(&self, words: &str) -> BoxResult<Value> {
        let deadline = tokio::time::Instant::now() + WAIT;
        loop {
            self.h.app.ci_dispatch().await;
            let overview = self.get("/api/sandboxes/s1/ci").await?;
            let runs = overview["runs"].as_array().ok_or("runs")?;
            if !runs.is_empty()
                && runs.iter().all(|r| {
                    r["state"] == "queued"
                        && r["reason"].as_str().is_some_and(|w| w.contains(words))
                })
            {
                return Ok(overview);
            }
            assert!(
                runs.iter().all(|r| r["state"] != "error"),
                "a run ended in error: {overview}"
            );
            if tokio::time::Instant::now() > deadline {
                return Err(format!("the runs do not wait for disk: {overview}").into());
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    /// Lets time pass, a recheck interval at a time, until no run waits for disk any more. The
    /// check that noted the wait ends in the background and dates the next one from its own end:
    /// one jump of the clock taken before it ended left the next check a minute ahead, forever
    /// in a frozen clock (#485).
    async fn recheck_until_started(&self) -> BoxResult<()> {
        let deadline = tokio::time::Instant::now() + WAIT;
        loop {
            self.h.clock.0.fetch_add(61_000, Ordering::SeqCst);
            self.h.app.ci_dispatch().await;
            let overview = self.get("/api/sandboxes/s1/ci").await?;
            let waiting = overview["runs"].as_array().ok_or("runs")?.iter().any(|r| {
                r["state"] == "queued"
                    && r["reason"]
                        .as_str()
                        .is_some_and(|w| w.starts_with("waiting for disk space"))
            });
            if !waiting {
                return Ok(());
            }
            if tokio::time::Instant::now() > deadline {
                return Err(format!("the runs still wait for disk: {overview}").into());
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    /// Every run passed and none ended otherwise (no error, no rerun).
    async fn all_passed(&self, runs: usize) -> BoxResult<()> {
        let overview = self.get("/api/sandboxes/s1/ci").await?;
        let list = overview["runs"].as_array().ok_or("runs")?;
        assert_eq!(list.len(), runs, "{overview}");
        assert!(list.iter().all(|r| r["state"] == "passed"), "{overview}");
        Ok(())
    }
}

/// SPEC-059 CI-301, CI-302, CI-304 (#240, #256, #258): over the disk quota a run is not started
/// and never ends in error. It stays queued, says it waits for disk and who holds the space, and
/// its commit status stays pending (an armed merge keeps waiting). Once usage is under the quota
/// it starts by itself at the next check and passes, without anyone running it again.
///
/// #485: it failed once in CI after 2.16 s, an assertion whose message was lost. The race found
/// here (a disk check ending after the clock jump, fixed by `recheck_until_started`) only ever
/// times out after a minute (SPEC-145), and probe delays at every publish, the check and a run's
/// start and end reproduced nothing fast (SPEC-294). CI now keeps a failing test's output
/// (`ci_status` with the run's id): a next failure names its assertion.
#[tokio::test]
async fn a_run_over_the_disk_quota_waits_and_starts_when_space_frees() -> TestResult {
    let f = fixture().await?;
    let disk = Disk::new(126, 0);
    assert!(f.h.app.set_isolation(disk.clone()));
    let pr_sha = git(&f.origin, &["rev-parse", "feat/one"])?;
    f.h.app.ci_sync_sandbox(&f.sandbox).await?;

    let overview = f.all_waiting("waiting for disk space").await?;
    let reason = overview["runs"][0]["reason"].as_str().unwrap_or_default();
    assert!(
        reason.contains("126.0 GB of 100.0 GB used")
            && reason.contains("largest: worktree elsewhere 35.0 GB"),
        "{reason}"
    );
    // The note is published right after it is recorded.
    let deadline = tokio::time::Instant::now() + WAIT;
    let (state, description) = loop {
        if let Some((state, description, _)) = f.last_status(&pr_sha)
            && description.contains("waiting for disk space")
        {
            break (state, description);
        }
        if tokio::time::Instant::now() > deadline {
            return Err(format!("no waiting status: {:?}", f.last_status(&pr_sha)).into());
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    };
    assert_eq!(state, "pending", "{description}");

    // Space frees (upkeep, or other tasks). Before the recheck is due nothing starts.
    disk.used.store(80 * GB, Ordering::SeqCst);
    f.h.app.ci_dispatch().await;
    f.all_waiting("waiting for disk space").await?;
    f.recheck_until_started().await?;
    f.settle().await?;
    f.all_passed(2).await?;
    let overview = f.get("/api/sandboxes/s1/ci").await?;
    assert!(
        overview["runs"]
            .as_array()
            .ok_or("runs")?
            .iter()
            .all(|r| r["reason"].is_null()),
        "the wait note goes once a run starts: {overview}"
    );
    let (state, description, _) = f.last_status(&pr_sha).ok_or("no status published")?;
    assert_eq!(state, "success", "{description}");
    Ok(())
}

/// A queued CI run reads each over-quota sandbox once, whether upkeep cleared a cache or found
/// nothing to prune. All names for this payload are in its cache, so the credited bytes are zero.
#[cfg(target_os = "linux")]
#[tokio::test]
async fn ci_uses_one_fresh_quota_reading_after_compiler_cache_upkeep() -> TestResult {
    for populated in [false, true] {
        let f = fixture().await?;
        let cache = f.data.join("sandboxes/s1/rust-compile-cache");
        let artifact = cache.join("artifact");
        if populated {
            std::fs::create_dir(&cache)?;
            std::fs::write(cache.join(".yulius-mbx-v1"), b"mbx-v1\n")?;
            std::fs::write(&artifact, vec![1u8; 256 * 1024])?;
            std::fs::hard_link(&artifact, cache.join("another-name"))?;
        }
        let disk = Disk::new(126, 0);
        assert!(f.h.app.set_isolation(disk.clone()));
        f.h.app.ci_sync_sandbox(&f.sandbox).await?;
        f.all_waiting("waiting for disk space").await?;
        assert!(!artifact.exists());
        assert_eq!(
            disk.measurements.load(Ordering::SeqCst),
            1,
            "populated: {populated}"
        );
    }
    Ok(())
}

/// SPEC-059 CI-302: a run waiting for disk ends like any queued run when its pull request moves
/// on: superseded, not left waiting.
#[tokio::test]
async fn a_run_waiting_for_disk_is_superseded_like_any_queued_run() -> TestResult {
    let f = fixture().await?;
    assert!(f.h.app.set_isolation(Disk::new(126, 0)));
    f.h.app.ci_sync_sandbox(&f.sandbox).await?;
    f.all_waiting("waiting for disk space").await?;
    let old = git(&f.origin, &["rev-parse", "feat/one"])?;
    let new = f.push_feature("more.txt", Some("more\n"))?;
    if let Ok(mut pulls) = f.h.github.pulls.lock() {
        pulls[0].head_sha = new.clone();
    }
    f.h.app.ci_sync_sandbox(&f.sandbox).await?;
    let overview = f.get("/api/sandboxes/s1/ci").await?;
    let runs = overview["runs"].as_array().ok_or("runs")?;
    let superseded = runs
        .iter()
        .find(|r| r["sha"] == old.as_str())
        .ok_or("old run")?;
    assert_eq!(superseded["state"], "cancelled", "{overview}");
    assert!(
        superseded["reason"]
            .as_str()
            .is_some_and(|r| r.starts_with("superseded by")),
        "{overview}"
    );
    assert!(
        runs.iter()
            .any(|r| r["sha"] == new.as_str() && r["state"] == "queued")
    );
    Ok(())
}

/// A refusal that is not the disk quota (strict isolation whose base system is still being
/// built) ends the run in error once, as before: it never puts the run back in the queue, so no
/// run can loop.
#[tokio::test]
async fn a_refusal_that_is_not_the_disk_quota_ends_the_run_in_error() -> TestResult {
    struct Unprepared;
    #[async_trait::async_trait]
    impl SandboxIsolation for Unprepared {
        async fn prepare(&self, _: &SandboxRef) -> AppResult<Option<SandboxJoin>> {
            Err(AppError::Conflict(
                "sandbox isolation is still being prepared".into(),
            ))
        }
        fn release(&self, _: &SandboxId) {}
    }
    let f = fixture().await?;
    assert!(f.h.app.set_isolation(Arc::new(Unprepared)));
    f.h.app.ci_sync_sandbox(&f.sandbox).await?;
    f.settle().await?;
    let overview = f.get("/api/sandboxes/s1/ci").await?;
    let runs = overview["runs"].as_array().ok_or("runs")?;
    assert_eq!(runs.len(), 2, "{overview}");
    assert!(runs.iter().all(|r| r["state"] == "error"), "{overview}");
    Ok(())
}

/// SPEC-059 CI-303: usage crosses the quota between the dispatcher's check and a job's start, and
/// the job is refused. Its run goes back to the queue instead of ending in error, waits for disk,
/// and passes once space frees.
#[tokio::test]
async fn a_job_refused_for_disk_puts_its_run_back_in_the_queue() -> TestResult {
    let f = fixture().await?;
    let disk = Disk::new(50, 1);
    assert!(f.h.app.set_isolation(disk.clone()));
    f.h.app.ci_sync_sandbox(&f.sandbox).await?;
    let deadline = tokio::time::Instant::now() + WAIT;
    loop {
        f.h.app.ci_dispatch().await;
        let overview = f.get("/api/sandboxes/s1/ci").await?;
        let runs = overview["runs"].as_array().ok_or("runs")?;
        assert!(runs.iter().all(|r| r["state"] != "error"), "{overview}");
        let settled = runs.iter().all(|r| r["state"] != "running");
        if disk.refusals.load(Ordering::SeqCst) == 0
            && settled
            && runs.iter().any(|r| r["state"] == "queued")
        {
            break;
        }
        if tokio::time::Instant::now() > deadline {
            return Err(format!("no run went back to the queue: {overview}").into());
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    // The run says why it waits, from the requeue on.
    let deadline = tokio::time::Instant::now() + WAIT;
    loop {
        let overview = f.get("/api/sandboxes/s1/ci").await?;
        let waiting = overview["runs"].as_array().ok_or("runs")?.iter().any(|r| {
            r["state"] == "queued"
                && r["reason"]
                    .as_str()
                    .is_some_and(|w| w.starts_with("waiting for disk space"))
        });
        if waiting {
            break;
        }
        if tokio::time::Instant::now() > deadline {
            return Err(
                format!("the queued run does not say it waits for disk: {overview}").into(),
            );
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    disk.used.store(50 * GB, Ordering::SeqCst);
    f.h.clock.0.fetch_add(61_000, Ordering::SeqCst);
    f.settle().await?;
    f.all_passed(2).await?;
    Ok(())
}

/// A sandbox limited to 4 CPUs and 8 GiB, without containment.
struct Limited;

#[async_trait::async_trait]
impl SandboxIsolation for Limited {
    async fn prepare(&self, _: &SandboxRef) -> AppResult<Option<SandboxJoin>> {
        Ok(None)
    }
    fn release(&self, _: &SandboxId) {}
    fn limits(&self) -> SandboxLimits {
        SandboxLimits {
            cpus: Some(4),
            memory_bytes: Some(8 << 30),
        }
    }
}

/// SPEC-082 RES-002, RES-003, RES-004: CI jobs declare the CPUs they use. Jobs whose budgets do
/// not fit in the sandbox together wait in the queue instead of running at once (two lanes,
/// three job slots: before, all of them ran, each sized for the whole machine), and each job is
/// told its budget in `YULIUS_CPUS`, `YULIUS_MEMORY_MB` and the repository's `[resources.env]`.
#[tokio::test]
async fn jobs_wait_for_their_cpu_budget_and_are_told_it() -> TestResult {
    let f = fixture_with(false, |data| {
        format!(
            "[resources.env]\nMAKEFLAGS = \"-j{{cpus}}\"\n\n\
             [[ci.jobs]]\nname = \"build\"\ncpus = 3\ncommand = 'while [ ! -e \"{go}\" ]; do \
             sleep 0.05; done; echo \"cpus=$YULIUS_CPUS memory=$YULIUS_MEMORY_MB $MAKEFLAGS\"'\n\n\
             [[ci.jobs]]\nname = \"check\"\ncpus = 3\ncommand = 'echo \"cpus=$YULIUS_CPUS\"'\n",
            go = data.join("go").display()
        )
    })
    .await?;
    assert!(f.h.app.set_isolation(Arc::new(Limited)));
    let cpus = f.h.app.capacity().cpus.min(3);
    f.h.app.ci_sync_sandbox(&f.sandbox).await?;
    let runs: Vec<String> = f.get("/api/sandboxes/s1/ci").await?["runs"]
        .as_array()
        .ok_or("runs")?
        .iter()
        .filter_map(|r| r["id"].as_str().map(str::to_owned))
        .collect();
    assert_eq!(runs.len(), 2, "main/dev and the pull request");
    // Until a `build` job holds its 3 CPUs (it waits for `go`), at most one job runs at a time.
    let deadline = tokio::time::Instant::now() + WAIT;
    loop {
        f.h.app.ci_dispatch().await;
        let mut running = Vec::new();
        for id in &runs {
            let detail = f.get(&format!("/api/sandboxes/s1/ci/runs/{id}")).await?;
            for job in detail["jobs"].as_array().ok_or("jobs")? {
                if job["state"] == "running" {
                    running.push(job["name"].as_str().unwrap_or_default().to_owned());
                }
            }
        }
        assert!(
            running.len() <= 1,
            "jobs over the budget ran at once: {running:?}"
        );
        if running.iter().any(|name| name == "build") {
            break;
        }
        if tokio::time::Instant::now() > deadline {
            return Err("no build job started".into());
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    // SPEC-385 WSL-001: the other `build` waits for its CPUs (its job still queued), and its wait
    // is a span already, open. A wait whose job is already running is left out: its start ends it
    // just after (review round 1).
    let deadline = tokio::time::Instant::now() + WAIT;
    let open = loop {
        let mut open = Vec::new();
        for id in &runs {
            let queued: Vec<String> =
                f.h.store
                    .ci_jobs(id, false)
                    .await?
                    .into_iter()
                    .filter(|j| j.state == RunState::Queued)
                    .map(|j| format!("wait:{}@", j.id))
                    .collect();
            open.extend(slot_waits(&f, id).await?.into_iter().filter(|w| {
                w.ended_at.is_none() && queued.iter().any(|prefix| w.key.starts_with(prefix))
            }));
        }
        if open.iter().any(|w| w.job.as_deref() == Some("build")) {
            break open;
        }
        if tokio::time::Instant::now() > deadline {
            return Err(format!("no open job slot wait of a queued job: {open:?}").into());
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    };
    assert!(open.iter().all(|w| w.outcome.is_none()), "{open:?}");
    std::fs::write(f.data.join("go"), "")?;
    f.settle().await?;
    f.all_passed(2).await?;
    // WSL-002, WSL-004: each job's wait ended `started` at its job's start.
    for id in &runs {
        let spans = run_spans(&f, id).await?;
        let jobs: Vec<_> = spans.iter().filter(|s| s.kind == "job").collect();
        let waits: Vec<_> = spans
            .iter()
            .filter(|s| s.kind == "wait" && s.name == "job slot")
            .collect();
        assert_eq!((jobs.len(), waits.len()), (2, 2), "{spans:?}");
        for job in jobs {
            let job_id = job.key["job:".len()..]
                .split('@')
                .next()
                .unwrap_or_default();
            let wait = waits
                .iter()
                .find(|w| w.key.starts_with(&format!("wait:{job_id}@")))
                .ok_or_else(|| format!("no wait of {job:?}"))?;
            assert_eq!(
                (wait.ended_at, wait.outcome.as_deref(), wait.job.as_deref()),
                (Some(job.started_at), Some("started"), job.job.as_deref()),
                "{wait:?}"
            );
        }
    }
    for id in &runs {
        let detail = f.get(&format!("/api/sandboxes/s1/ci/runs/{id}")).await?;
        for job in detail["jobs"].as_array().ok_or("jobs")? {
            let output = job["output"].as_str().unwrap_or_default();
            let memory = 8192 * u64::from(cpus) / 4;
            assert!(
                output.contains(&format!("[yulius] budget: {cpus} CPUs, {memory} MiB")),
                "{output}"
            );
            if job["name"] == "build" {
                assert!(
                    output.contains(&format!("cpus={cpus} memory={memory} -j{cpus}")),
                    "{output}"
                );
            }
        }
    }
    Ok(())
}

/// A CI run's spans (SPEC-137).
async fn run_spans(f: &Fixture, run: &str) -> BoxResult<Vec<yulius_app::stages::SpanRecord>> {
    use yulius_app::ports::store_ext::SpanStore as _;
    Ok(f.h
        .store
        .subject_spans(
            &SandboxId::parse("s1")?,
            yulius_domain::stages::SubjectKind::CiRun,
            run,
            1_000,
        )
        .await?)
}

/// A CI run's waits for a job slot (SPEC-385).
async fn slot_waits(f: &Fixture, run: &str) -> BoxResult<Vec<yulius_app::stages::SpanRecord>> {
    Ok(run_spans(f, run)
        .await?
        .into_iter()
        .filter(|s| s.kind == "wait" && s.name == "job slot")
        .collect())
}

/// SPEC-385 WSL-002: a job cancelled while it waits for a slot (its run superseded by a push)
/// ends its open wait `cancelled` when it is cancelled, and never gets a job span. The wait names
/// the job's definition (`job`) and its display name (`detail.name`, a matrix job's "a (1)";
/// review round 1).
#[tokio::test]
async fn a_job_cancelled_while_it_waits_for_a_slot_ends_its_wait_cancelled() -> TestResult {
    let f = fixture_with(false, |data| {
        format!(
            "[[ci.jobs]]\nname = \"a\"\nmatrix = \"1 2\"\ncpus = 3\n\
             command = 'while [ ! -e \"{go}\" ]; do sleep 0.05; done'\n",
            go = data.join("go").display()
        )
    })
    .await?;
    assert!(f.h.app.set_isolation(Arc::new(Limited)));
    f.h.app.ci_sync_sandbox(&f.sandbox).await?;
    // One job at a time holds 3 of the 4 CPUs until `go`: once one runs, the pull request's run
    // has a job that waits (queued, its wait open) and cannot start before the push cancels it.
    let run = f.running("pr/1").await?;
    let run = run["id"].as_str().ok_or("run id")?.to_owned();
    let deadline = tokio::time::Instant::now() + WAIT;
    let (waiting, name) = loop {
        f.h.app.ci_dispatch().await;
        let mut jobs = Vec::new();
        for other in f.h.store.ci_runs(&f.sandbox.id, 10).await? {
            jobs.extend(f.h.store.ci_jobs(&other.id, false).await?);
        }
        let held = jobs.iter().any(|j| j.state == RunState::Running);
        let open = slot_waits(&f, &run).await?.into_iter().find_map(|w| {
            let job = jobs.iter().find(|j| {
                j.run_id == run
                    && j.state == RunState::Queued
                    && w.key.starts_with(&format!("wait:{}@", j.id))
            })?;
            w.ended_at.is_none().then(|| (w, job.name.clone()))
        });
        if let (true, Some(open)) = (held, open) {
            break open;
        }
        if tokio::time::Instant::now() > deadline {
            return Err("no job of the pull request's run waits".into());
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    };
    assert!(name == "a (1)" || name == "a (2)", "{name}");
    let detail: Value = serde_json::from_str(waiting.detail.as_deref().unwrap_or("{}"))?;
    assert_eq!(
        (waiting.job.as_deref(), detail["name"].as_str()),
        (Some("a"), Some(name.as_str())),
        "{waiting:?}"
    );
    let job_id = waiting.key["wait:".len()..]
        .split('@')
        .next()
        .unwrap_or_default()
        .to_owned();
    f.push_feature("more.txt", Some("more\n"))?;
    f.h.app.ci_sync_sandbox(&f.sandbox).await?;
    let deadline = tokio::time::Instant::now() + WAIT;
    loop {
        if f.h.store.ci_run(&run).await?.map(|r| r.state) == Some(RunState::Cancelled) {
            break;
        }
        if tokio::time::Instant::now() > deadline {
            return Err("the superseded run never ended".into());
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    std::fs::write(f.data.join("go"), "")?;
    f.settle().await?;
    let job =
        f.h.store
            .ci_jobs(&run, false)
            .await?
            .into_iter()
            .find(|j| j.id == job_id)
            .ok_or("the waiting job")?;
    assert_eq!((job.state, job.started_at), (RunState::Cancelled, None));
    let spans = run_spans(&f, &run).await?;
    let wait = spans
        .iter()
        .find(|s| s.key == waiting.key)
        .ok_or("the wait")?;
    // It ended at the job's cancellation, with its word.
    assert_eq!(
        (wait.ended_at, wait.outcome.as_deref()),
        (job.ended_at, Some("cancelled")),
        "{wait:?}"
    );
    assert!(
        !spans
            .iter()
            .any(|s| s.key.starts_with(&format!("job:{job_id}@"))),
        "{spans:?}"
    );
    Ok(())
}

/// A sandbox limited to 4 CPUs that records the CI jobs the app says run in it (SPEC-362).
#[derive(Default)]
struct Weighing {
    jobs: std::sync::Mutex<Vec<(String, u32)>>,
    /// The CPUs held at each call (SPEC-497 HBA-004).
    cpus: std::sync::Mutex<Vec<u32>>,
}

impl Weighing {
    fn history(&self) -> Vec<u32> {
        self.jobs
            .lock()
            .map(|jobs| jobs.iter().map(|(_, n)| *n).collect())
            .unwrap_or_default()
    }
}

#[async_trait::async_trait]
impl SandboxIsolation for Weighing {
    async fn prepare(&self, _: &SandboxRef) -> AppResult<Option<SandboxJoin>> {
        Ok(None)
    }
    fn release(&self, _: &SandboxId) {}
    fn limits(&self) -> SandboxLimits {
        Limited.limits()
    }
    fn ci_jobs_running(&self, sandbox: &SandboxId, jobs: u32, cpus: u32) {
        if let Ok(mut seen) = self.jobs.lock() {
            seen.push((sandbox.as_str().to_owned(), jobs));
        }
        if let Ok(mut held) = self.cpus.lock() {
            held.push(cpus);
        }
    }
}

/// SPEC-362 CSW-002: while a sandbox's CI jobs run, the app tells the isolation how many, so it
/// weighs the sandbox by them; every end counts (passed, timed out, and cancelled when a push
/// supersedes the run, review round 1), and none is left counted.
#[tokio::test]
async fn running_ci_jobs_weigh_on_their_sandbox_until_they_end() -> TestResult {
    let f = fixture_with(false, |data| {
        format!(
            "[[ci.jobs]]\nname = \"build\"\ncpus = 1\ncommand = 'while [ ! -e \"{go}\" ]; do \
             sleep 0.05; done'\n\n\
             [[ci.jobs]]\nname = \"hang\"\ncpus = 1\ncommand = 'while [ ! -e \"{go}\" ]; do \
             sleep 0.05; done; sleep 30'\n",
            go = data.join("go").display()
        )
    })
    .await?;
    let weighing = Arc::new(Weighing::default());
    assert!(f.h.app.set_isolation(weighing.clone()));
    f.h.app.ci_sync_sandbox(&f.sandbox).await?;
    // Two runs of two jobs, one CPU each: the three job slots fill, and the app says so.
    let deadline = tokio::time::Instant::now() + WAIT;
    loop {
        f.h.app.ci_dispatch().await;
        if weighing.history().last() == Some(&3) {
            break;
        }
        if tokio::time::Instant::now() > deadline {
            return Err(format!("never three jobs: {:?}", weighing.history()).into());
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    // A push supersedes the pull request's run: its running jobs are cancelled.
    f.push_feature("more.txt", Some("more\n"))?;
    f.h.app.ci_sync_sandbox(&f.sandbox).await?;
    // Its jobs still wait for `go`, so only the cancellation ends them (review round 2).
    let deadline = tokio::time::Instant::now() + WAIT;
    let superseded = loop {
        let runs = f.h.store.ci_runs(&f.sandbox.id, 10).await?;
        if let Some(run) = runs.into_iter().find(|r| r.state == RunState::Cancelled) {
            break run;
        }
        if tokio::time::Instant::now() > deadline {
            return Err("the superseded run never ended".into());
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    };
    std::fs::write(f.data.join("go"), "")?;
    // `build` jobs pass, `hang` jobs time out (8 s); every end lowers the count.
    f.settle().await?;
    // A job that was running when its run was superseded ended cancelled.
    let detail = f
        .get(&format!("/api/sandboxes/s1/ci/runs/{}", superseded.id))
        .await?;
    assert!(
        detail["jobs"]
            .as_array()
            .ok_or("jobs")?
            .iter()
            .any(|job| job["state"] == "cancelled" && job["duration_ms"].is_u64()),
        "no started job was cancelled: {detail}"
    );
    let seen = weighing.jobs.lock().map_err(|_| "lock")?.clone();
    assert!(seen.iter().all(|(sandbox, _)| sandbox == "s1"), "{seen:?}");
    let history = weighing.history();
    assert_eq!(history.last(), Some(&0), "a job left counted: {history:?}");
    // SPEC-497 HBA-004: the CPUs the jobs hold reach the isolation, one per 1-CPU job.
    let cpus = weighing.cpus.lock().map_err(|_| "lock")?.clone();
    assert_eq!(cpus, history, "{cpus:?} against {history:?}");
    assert!(history.iter().all(|n| *n <= 3), "{history:?}");
    // One call per start and one per end, each one job up or down from none.
    let mut previous = 0;
    for n in &history {
        assert_eq!(n.abs_diff(previous), 1, "{history:?}");
        previous = *n;
    }
    Ok(())
}

/// A contained sandbox's isolation that records which CI jobs got their own group and which it
/// was told ended (SPEC-504 CLB-001, CLB-004), in order.
#[derive(Default)]
struct Grouping {
    calls: std::sync::Mutex<Vec<(&'static str, String)>>,
    /// Its removal never ends (an engine that never answers).
    hangs: bool,
    /// It makes no job group (`ci_job_started` answers `None`).
    no_group: bool,
    /// Its join has no CI group (`ci_cgroup` is `None`).
    no_ci_group: bool,
}

impl Grouping {
    fn calls(&self) -> Vec<(&'static str, String)> {
        self.calls.lock().map(|c| c.clone()).unwrap_or_default()
    }
}

#[async_trait::async_trait]
impl SandboxIsolation for Grouping {
    async fn prepare(&self, _: &SandboxRef) -> AppResult<Option<SandboxJoin>> {
        Ok(Some(SandboxJoin {
            holder_pid: 4242,
            user_ns: 3,
            net_ns: 2,
            mnt_ns: 4,
            cgroup: Some("/cg/s1/agents".into()),
            ci_cgroup: (!self.no_ci_group).then(|| "/cg/s1/ci".into()),
            system_root: "/fake/root".into(),
            hostname: "s1".into(),
            exec_shim: None,
            extra_binds: Vec::new(),
            private_dirs: Vec::new(),
            system_passwd: None,
            system_group: None,
            host_system: None,
            docker_dir: None,
        }))
    }
    fn release(&self, _: &SandboxId) {}
    fn limits(&self) -> SandboxLimits {
        Limited.limits()
    }
    fn ci_job_started(&self, _: &SandboxId, job: &str) -> Option<PathBuf> {
        if let Ok(mut calls) = self.calls.lock() {
            calls.push(("started", job.to_owned()));
        }
        (!self.no_group).then(|| format!("/cg/s1/ci/ci-job-{job}").into())
    }
    async fn ci_job_ended(&self, sandbox: &SandboxId, job: &str) {
        assert_eq!(sandbox.as_str(), "s1");
        if let Ok(mut calls) = self.calls.lock() {
            calls.push(("ended", job.to_owned()));
        }
        if self.hangs {
            std::future::pending::<()>().await;
        }
    }
}

/// SPEC-504 CLB-001, CLB-004: every CI job gets its own group before it starts, and the
/// isolation hears that it ended, after it started, whatever the outcome: passed, failed, timed
/// out, and cancelled when a push supersedes its run.
#[tokio::test]
async fn every_ci_job_is_grouped_and_its_end_told_whatever_the_outcome() -> TestResult {
    let f = fixture_with(false, |data| {
        let job = format!(
            "while [ ! -e \"{}\" ]; do sleep 0.05; done; if [ -f hang ]; then sleep 30; fi; \
             if [ -f bad ]; then exit 1; fi",
            data.join("go").display()
        );
        format!(
            "[[ci.jobs]]\nname = \"a\"\ncpus = 1\ncommand = '{job}'\n\n\
             [[ci.jobs]]\nname = \"b\"\ncpus = 1\ncommand = '{job}'\n"
        )
    })
    .await?;
    let grouping = Arc::new(Grouping::default());
    assert!(f.h.app.set_isolation(grouping.clone()));
    f.h.app.ci_sync_sandbox(&f.sandbox).await?;
    // dev's run and the pull request's: three of their four jobs fill the three job slots.
    let deadline = tokio::time::Instant::now() + WAIT;
    while grouping
        .calls()
        .iter()
        .filter(|(c, _)| *c == "started")
        .count()
        < 3
    {
        f.h.app.ci_dispatch().await;
        if tokio::time::Instant::now() > deadline {
            return Err(format!("never three jobs: {:?}", grouping.calls()).into());
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    // A push supersedes the pull request's run while a job of it waits for `go`: cancelled.
    f.push_feature("bad", Some("1\n"))?;
    f.h.app.ci_sync_sandbox(&f.sandbox).await?;
    let deadline = tokio::time::Instant::now() + WAIT;
    while !f
        .h
        .store
        .ci_runs(&f.sandbox.id, 10)
        .await?
        .iter()
        .any(|r| r.state == RunState::Cancelled)
    {
        f.h.app.ci_dispatch().await;
        if tokio::time::Instant::now() > deadline {
            return Err("the superseded run never ended".into());
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    // dev's jobs pass, the new head's fail.
    std::fs::write(f.data.join("go"), "")?;
    f.settle().await?;
    // A head whose jobs hang past their 8 s limit: timed out.
    f.push_feature("bad", None)?;
    f.push_feature("hang", Some("1\n"))?;
    f.h.app.ci_sync_sandbox(&f.sandbox).await?;
    f.settle().await?;
    let calls = grouping.calls();
    let mut outcomes = std::collections::BTreeSet::new();
    for run in f.h.store.ci_runs(&f.sandbox.id, 10).await? {
        for job in f.h.store.ci_jobs(&run.id, false).await? {
            if job.started_at.is_none() {
                continue;
            }
            let at = |what: &str| calls.iter().position(|(c, id)| *c == what && *id == job.id);
            let (Some(started), Some(ended)) = (at("started"), at("ended")) else {
                return Err(format!(
                    "{} ({:?}) not grouped and ended: {calls:?}",
                    job.name, job.state
                )
                .into());
            };
            assert!(
                started < ended,
                "{} ended before it started: {calls:?}",
                job.name
            );
            let ends = calls
                .iter()
                .filter(|(c, id)| *c == "ended" && *id == job.id)
                .count();
            assert_eq!(ends, 1, "{}: {calls:?}", job.name);
            outcomes.insert(format!("{:?}", job.state));
        }
    }
    for outcome in ["Passed", "Failed", "Cancelled", "Error"] {
        assert!(outcomes.contains(outcome), "no {outcome} job: {outcomes:?}");
    }
    Ok(())
}

/// The cgroups contained CI jobs were launched to join, with `grouping`, and the jobs' ids.
async fn launched_groups(
    grouping: Grouping,
) -> BoxResult<(Vec<PathBuf>, Vec<String>, Vec<(&'static str, String)>)> {
    use yulius_app::ports::session::Containment;
    let recording = Arc::new(crate::cd::Recording {
        inner: PtySessionHost::new(PathBuf::from(env!("CARGO_BIN_EXE_yulius"))),
        seen: std::sync::Mutex::new(Vec::new()),
    });
    let f = fixture_hosted(
        true,
        |_| "[[ci.jobs]]\nname = \"check\"\ncommand = \"true\"\n".to_owned(),
        Some(recording.clone()),
    )
    .await?;
    let grouping = Arc::new(grouping);
    assert!(f.h.app.set_isolation(grouping.clone()));
    f.h.app.ci_sync_sandbox(&f.sandbox).await?;
    f.settle().await?;
    let seen = recording.seen.lock().map(|s| s.clone()).unwrap_or_default();
    let mut groups = Vec::new();
    for containment in seen {
        let Containment::Bubblewrap {
            join: Some(join), ..
        } = containment
        else {
            return Err(format!("a joined containment: {containment:?}").into());
        };
        groups.push(join.cgroup.ok_or("no cgroup")?);
    }
    let mut jobs = Vec::new();
    for run in f.h.store.ci_runs(&f.sandbox.id, 10).await? {
        jobs.extend(
            f.h.store
                .ci_jobs(&run.id, false)
                .await?
                .into_iter()
                .map(|j| j.id),
        );
    }
    Ok((groups, jobs, grouping.calls()))
}

/// SPEC-504 CLB-001 (review round 1, major): a contained CI job is launched to join its own
/// group, `ci/ci-job-<job id>`, on the path production takes (the executor's join, its
/// containment and the CI join); without a job group (none made, or no CI group) it joins the
/// CI group, or the sandbox's group, as before.
#[tokio::test]
async fn a_contained_ci_job_is_launched_in_its_own_group() -> TestResult {
    let (groups, jobs, _) = launched_groups(Grouping::default()).await?;
    assert_eq!(jobs.len(), 2, "dev's run and the pull request's: {jobs:?}");
    let mut want: Vec<PathBuf> = jobs
        .iter()
        .map(|job| PathBuf::from(format!("/cg/s1/ci/ci-job-{job}")))
        .collect();
    let mut groups = groups;
    groups.sort();
    want.sort();
    assert_eq!(groups, want);
    let (groups, _, calls) = launched_groups(Grouping {
        no_group: true,
        ..Grouping::default()
    })
    .await?;
    assert_eq!(
        groups,
        [PathBuf::from("/cg/s1/ci"), PathBuf::from("/cg/s1/ci")]
    );
    assert!(calls.iter().any(|(c, _)| *c == "started"));
    let (groups, _, calls) = launched_groups(Grouping {
        no_ci_group: true,
        ..Grouping::default()
    })
    .await?;
    assert_eq!(
        groups,
        [
            PathBuf::from("/cg/s1/agents"),
            PathBuf::from("/cg/s1/agents")
        ]
    );
    assert!(
        calls.iter().all(|(c, _)| *c != "started"),
        "no job group without a CI group: {calls:?}"
    );
    Ok(())
}

/// SPEC-504 CLB-004: a job a restart of Yulius ended (its run recorded as running, with no
/// executor) is told ended to the isolation when CI starts again; a removal that never ends
/// (an engine that never answers) does not hold the run's requeue or CI's start (review round 1).
#[tokio::test]
async fn a_job_a_restart_ended_is_told_ended() -> TestResult {
    for hangs in [false, true] {
        a_job_a_restart_ended_is_told_ended_with(hangs).await?;
    }
    Ok(())
}

async fn a_job_a_restart_ended_is_told_ended_with(hangs: bool) -> TestResult {
    use yulius_app::ci::{CiJobRecord, CiRunRecord};
    use yulius_domain::ci::{CiRef, GateSource};
    use yulius_domain::time::Timestamp;
    let f = fixture_with(false, |_| {
        "[[ci.jobs]]\nname = \"check\"\ncommand = \"true\"\n".to_owned()
    })
    .await?;
    let grouping = Arc::new(Grouping {
        hangs,
        ..Grouping::default()
    });
    assert!(f.h.app.set_isolation(grouping.clone()));
    let run = CiRunRecord {
        id: "run-1".into(),
        sandbox_id: f.sandbox.id.clone(),
        reference: CiRef::PullRequest(1),
        sha: "a".repeat(40),
        source: GateSource::ProjectConfig,
        state: RunState::Running,
        reason: None,
        requested_by: "yulius".into(),
        status_for: None,
        status_note: None,
        status_attempts: 0,
        queued_at: Timestamp(1),
        started_at: Some(Timestamp(2)),
        ended_at: None,
    };
    let job = CiJobRecord {
        id: "job-1".into(),
        run_id: "run-1".into(),
        ordinal: 0,
        name: "check".into(),
        key: "check".into(),
        command: "true".into(),
        matrix: None,
        state: RunState::Running,
        reason: None,
        exit_code: None,
        output: String::new(),
        failures: None,
        started_at: Some(Timestamp(2)),
        ended_at: None,
        need: Default::default(),
        timeout_minutes: None,
        exclusive: false,
        waited_ms: None,
    };
    f.h.store.insert_ci_run(&run, &[job]).await?;
    f.h.app.start_ci();
    let deadline = tokio::time::Instant::now() + WAIT;
    loop {
        let requeued =
            f.h.store
                .ci_runs(&f.sandbox.id, 10)
                .await?
                .iter()
                .any(|r| r.id == "run-1" && r.state != RunState::Running);
        if requeued && grouping.calls().contains(&("ended", "job-1".to_owned())) {
            return Ok(());
        }
        if tokio::time::Instant::now() > deadline {
            return Err(format!(
                "hangs {hangs}: requeued {requeued}, told {:?}",
                grouping.calls()
            )
            .into());
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// A sandbox limited to 4 CPUs (a CI budget of 3) whose CI and Docker engine use `millis`
/// thousandths of a CPU, and whose engine of 4,096 networks has `networks` of them
/// (`u32::MAX`: no running engine) (SPEC-497).
struct Measured {
    used: std::sync::Mutex<(tokio::time::Instant, u64)>,
    millis: AtomicU32,
    networks: AtomicU32,
    pruned: AtomicU32,
    /// The `ci` groups' CPU pressure in thousandths; `u32::MAX` for none.
    pressure: AtomicU32,
}

impl Measured {
    fn new(millis: u32, networks: u32) -> Arc<Self> {
        Arc::new(Self {
            used: std::sync::Mutex::new((tokio::time::Instant::now(), 0)),
            millis: AtomicU32::new(millis),
            networks: AtomicU32::new(networks),
            pruned: AtomicU32::new(0),
            pressure: AtomicU32::new(u32::MAX),
        })
    }
}

#[async_trait::async_trait]
impl SandboxIsolation for Measured {
    async fn prepare(&self, _: &SandboxRef) -> AppResult<Option<SandboxJoin>> {
        Ok(None)
    }
    fn release(&self, _: &SandboxId) {}
    fn limits(&self) -> SandboxLimits {
        Limited.limits()
    }
    fn ci_use(&self, _: &SandboxId) -> Option<CiUse> {
        let mut used = self.used.lock().ok()?;
        let now = tokio::time::Instant::now();
        let micros = u64::try_from(now.duration_since(used.0).as_micros()).unwrap_or(u64::MAX);
        used.1 += micros * u64::from(self.millis.load(Ordering::SeqCst)) / 1000;
        used.0 = now;
        Some(CiUse {
            cpu_usec: used.1,
            anon_bytes: 0,
            cpu_pressure: Some(self.pressure.load(Ordering::SeqCst)).filter(|p| *p != u32::MAX),
        })
    }
    async fn engine_networks(&self, _: &SandboxId) -> Option<EngineNetworks> {
        let used = self.networks.load(Ordering::SeqCst);
        (used != u32::MAX).then_some(EngineNetworks {
            capacity: 4096,
            used,
        })
    }
    async fn prune_engine_networks(&self, _: &SandboxId) -> Option<u32> {
        self.pruned.fetch_add(1, Ordering::SeqCst);
        Some(0)
    }
}

/// The reason of `sha`'s queued job, once it says `words`.
async fn waits_with(f: &Fixture, sha: &str, words: &str) -> BoxResult<String> {
    let deadline = tokio::time::Instant::now() + WAIT;
    loop {
        f.h.app.ci_dispatch().await;
        for run in f.h.store.ci_runs(&f.sandbox.id, 10).await? {
            if run.sha != sha {
                continue;
            }
            for job in f.h.store.ci_jobs(&run.id, false).await? {
                if job.state == RunState::Queued
                    && let Some(reason) = job.reason.filter(|r| r.contains(words))
                {
                    return Ok(reason);
                }
            }
        }
        if tokio::time::Instant::now() > deadline {
            return Err(format!("no job of {sha} says {words:?}").into());
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// The state of `sha`'s only job.
async fn only_job_state(f: &Fixture, sha: &str) -> BoxResult<Option<RunState>> {
    for run in f.h.store.ci_runs(&f.sandbox.id, 10).await? {
        if run.sha == sha {
            let jobs = f.h.store.ci_jobs(&run.id, false).await?;
            return Ok(jobs.first().map(|j| j.state));
        }
    }
    Ok(None)
}

/// SPEC-497 HBA-002, HBA-005: a job that fits beside the running job's reservation still waits
/// while what CI really uses, with what the job that just started may still bring, leaves no
/// room, says so, and starts once the use drops, while the first job still runs.
#[tokio::test]
async fn a_job_waits_while_ci_uses_more_than_it_reserved() -> TestResult {
    let f = fixture_tuned(
        false,
        |data| {
            format!(
                "[[ci.jobs]]\nname = \"build\"\ncpus = 1\ncommand = 'if [ -e feature.txt ] || \
                 [ -e later.txt ]; then while [ ! -e \"{go}\" ]; do sleep 0.05; done; fi'\n",
                go = data.join("go").display()
            )
        },
        None,
        // The pull request's job runs through two measuring windows.
        |config| config.ci.job_timeout = WAIT,
    )
    .await?;
    // 2.5 CPUs: the measured 3 does not depend on the clocks of two readings (review round 1).
    let measured = Measured::new(2500, u32::MAX);
    assert!(f.h.app.set_isolation(measured.clone()));
    f.h.app.ci_sync_sandbox(&f.sandbox).await?;
    // The pull request's job runs (dev's has nothing to wait for and passes).
    let pull = f.running("pr/1").await?;
    let pull_sha = pull["sha"].as_str().ok_or("sha")?.to_owned();
    // The measuring window: the use is the CPU time over at least this long since the job's
    // start, not a wait for another task to get somewhere.
    tokio::time::sleep(Duration::from_secs(6)).await;
    let later = f.push_dev("later.txt", Some("later\n"))?;
    f.h.app.ci_sync_sandbox(&f.sandbox).await?;
    let reason = waits_with(&f, &later, "CI budget").await?;
    assert_eq!(
        reason,
        "waits for the host's CI budget: CI uses 3 of the host's 3 CPUs for CI and jobs that just \
         started may bring 1 more; this job starts with 1"
    );
    // The use drops: it starts within the next window, the first job still running.
    measured.millis.store(0, Ordering::SeqCst);
    let deadline = tokio::time::Instant::now() + WAIT;
    while only_job_state(&f, &later).await? != Some(RunState::Running) {
        f.h.app.ci_dispatch().await;
        if tokio::time::Instant::now() > deadline {
            return Err("the job never started".into());
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert_eq!(
        only_job_state(&f, &pull_sha).await?,
        Some(RunState::Running)
    );
    // Its wait is no longer its reason.
    for run in f.h.store.ci_runs(&f.sandbox.id, 10).await? {
        if run.sha == later {
            let jobs = f.h.store.ci_jobs(&run.id, false).await?;
            assert!(jobs.iter().all(|j| j.reason.is_none()), "{jobs:?}");
        }
    }
    std::fs::write(f.data.join("go"), "")?;
    f.settle().await?;
    Ok(())
}

/// SPEC-497 HBA-003: a job of a sandbox whose engine can make fewer than 32 more networks waits,
/// saying so, after the networks unused for 10 minutes were pruned, and starts once there is
/// room.
#[tokio::test]
async fn a_job_waits_for_docker_networks() -> TestResult {
    let f = fixture_with(false, |_| {
        "[[ci.jobs]]\nname = \"build\"\ncommand = 'true'\n".to_owned()
    })
    .await?;
    let measured = Measured::new(0, 4086);
    assert!(f.h.app.set_isolation(measured.clone()));
    f.h.app.ci_sync_sandbox(&f.sandbox).await?;
    let head = git(&f.work, &["rev-parse", "feat/one"])?;
    let reason = waits_with(&f, &head, "Docker networks").await?;
    assert_eq!(
        reason,
        "waits for Docker networks: its sandbox's engine can make 10 more of 4096, and a job \
         starts with at least 32 free"
    );
    assert!(measured.pruned.load(Ordering::SeqCst) >= 1);
    measured.networks.store(1, Ordering::SeqCst);
    f.settle().await?;
    assert_eq!(only_job_state(&f, &head).await?, Some(RunState::Passed));
    // Its wait is no longer its reason (review round 2).
    for run in f.h.store.ci_runs(&f.sandbox.id, 10).await? {
        if run.sha == head {
            let jobs = f.h.store.ci_jobs(&run.id, false).await?;
            assert!(jobs.iter().all(|j| j.reason.is_none()), "{jobs:?}");
        }
    }
    Ok(())
}

/// SPEC-497 (review rounds 1 and 2): a run's jobs that declare no `cpus` get shares of the CI
/// budget, so they run at once on an idle host. 4 CPUs give a budget of 3: two jobs of 1 CPU
/// each; shares of the capacity (2 each) would run one after the other.
#[tokio::test]
async fn a_runs_default_shares_run_together() -> TestResult {
    let f = fixture_with(false, |data| {
        format!(
            "[[ci.jobs]]\nname = \"a\"\ncommand = '{hold}; echo \"cpus=$YULIUS_CPUS\"'\n\n\
             [[ci.jobs]]\nname = \"b\"\ncommand = '{hold}; echo \"cpus=$YULIUS_CPUS\"'\n",
            // Only the pull request's jobs hold (dev's end at once).
            hold = format_args!(
                "if [ -e feature.txt ]; then while [ ! -e \"{}\" ]; do sleep 0.05; done; fi",
                data.join("go").display()
            ),
        )
    })
    .await?;
    assert!(f.h.app.set_isolation(Arc::new(Limited)));
    f.h.app.ci_sync_sandbox(&f.sandbox).await?;
    let pull = f.running("pr/1").await?;
    let id = pull["id"].as_str().ok_or("id")?.to_owned();
    let deadline = tokio::time::Instant::now() + WAIT;
    loop {
        f.h.app.ci_dispatch().await;
        let jobs = f.h.store.ci_jobs(&id, false).await?;
        if jobs.iter().all(|j| j.state == RunState::Running) {
            break;
        }
        if tokio::time::Instant::now() > deadline {
            return Err(format!("the run's two jobs never ran together: {jobs:?}").into());
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    std::fs::write(f.data.join("go"), "")?;
    f.settle().await?;
    let detail = f.get(&format!("/api/sandboxes/s1/ci/runs/{id}")).await?;
    for job in detail["jobs"].as_array().ok_or("jobs")? {
        let output = job["output"].as_str().unwrap_or_default();
        assert!(
            output.contains("[yulius] budget: 1 CPU, 2048 MiB") && output.contains("cpus=1"),
            "{output}"
        );
    }
    Ok(())
}

/// SPEC-497 HBA-004 (review round 2): the isolation hears the CPUs a job holds, not the number
/// of jobs.
#[tokio::test]
async fn a_job_weighs_by_the_cpus_it_holds() -> TestResult {
    let f = fixture_with(false, |_| {
        "[[ci.jobs]]\nname = \"build\"\ncpus = 2\ncommand = 'true'\n".to_owned()
    })
    .await?;
    let weighing = Arc::new(Weighing::default());
    assert!(f.h.app.set_isolation(weighing.clone()));
    f.h.app.ci_sync_sandbox(&f.sandbox).await?;
    f.settle().await?;
    let cpus = weighing.cpus.lock().map_err(|_| "lock")?.clone();
    let history = weighing.history();
    assert!(cpus.contains(&2), "{cpus:?}");
    for (cpus, jobs) in cpus.iter().zip(&history) {
        assert_eq!(*cpus, jobs * 2, "{cpus} CPUs for {jobs} jobs");
    }
    assert_eq!(cpus.last(), Some(&0));
    Ok(())
}

/// ADR-0353 (review of the CI admission fix): a job takes its slot only once admitted, so one
/// that cannot start holds none. Two slots, a CI budget of 3: `a` (2 CPUs) runs, `b` (3) cannot
/// start, and `c` (1) runs beside `a` in the second slot. With the slot taken before the
/// budget's wait, `b` held it and `c` never started. `b` and `c` ask for admission only once `a`
/// runs (pause points): `b` asking first took the whole budget 1 run in 4 of the suite (slice D
/// review, 17 Nov 2026).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_job_that_cannot_start_holds_no_slot() -> TestResult {
    let f = fixture_tuned(
        false,
        |data| {
            let hold = format!(
                "if [ -e feature.txt ]; then while [ ! -e \"{}\" ]; do sleep 0.05; done; fi",
                data.join("go").display()
            );
            format!(
                "[[ci.jobs]]\nname = \"a\"\ncpus = 2\ncommand = '{hold}'\n\n\
                 [[ci.jobs]]\nname = \"b\"\ncpus = 3\ncommand = '{hold}'\n\n\
                 [[ci.jobs]]\nname = \"c\"\ncpus = 1\ncommand = '{hold}'\n"
            )
        },
        None,
        |config| {
            config.ci.max_jobs = 2;
            config.ci.job_timeout = WAIT;
        },
    )
    .await?;
    assert!(f.h.app.set_isolation(Arc::new(Limited)));
    // #1's `b` and `c` ask for admission only once its `a` runs.
    let (b_at, b_go) = support::hold_point(yulius_app::race::Point::CiJobAdmission, "pr/1/b");
    let (c_at, c_go) = support::hold_point(yulius_app::race::Point::CiJobAdmission, "pr/1/c");
    f.h.app.ci_sync_sandbox(&f.sandbox).await?;
    let pull = f.running("pr/1").await?;
    let id = pull["id"].as_str().ok_or("id")?.to_owned();
    let deadline = tokio::time::Instant::now() + WAIT;
    let state_of = |name: &'static str| {
        let (store, id) = (f.h.store.clone(), id.clone());
        async move {
            let jobs = store.ci_jobs(&id, false).await?;
            Ok::<_, Box<dyn std::error::Error>>(
                jobs.iter().find(|j| j.name == name).map(|j| j.state),
            )
        }
    };
    while state_of("a").await? != Some(RunState::Running) {
        f.h.app.ci_dispatch().await;
        if tokio::time::Instant::now() > deadline {
            return Err("`a` never started".into());
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    support::point_reached(b_at).await?;
    b_go.send(())?;
    // `b` waits for the budget (its reason says so) before `c` asks (review round 2: with `c`
    // first, `b` never waited and the old slot-before-budget bug passed too).
    loop {
        let jobs = f.h.store.ci_jobs(&id, false).await?;
        let b = jobs.iter().find(|j| j.name == "b").ok_or("b")?;
        if b.state == RunState::Queued
            && b.reason
                .as_deref()
                .is_some_and(|r| r.starts_with("waits for the host's CI budget"))
        {
            break;
        }
        if tokio::time::Instant::now() > deadline {
            return Err(format!("`b` never waited: {b:?}").into());
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    support::point_reached(c_at).await?;
    c_go.send(())?;
    loop {
        f.h.app.ci_dispatch().await;
        let jobs = f.h.store.ci_jobs(&id, false).await?;
        let state = |name: &str| jobs.iter().find(|j| j.name == name).map(|j| j.state);
        if state("a") == Some(RunState::Running)
            && state("c") == Some(RunState::Running)
            && state("b") == Some(RunState::Queued)
        {
            break;
        }
        if tokio::time::Instant::now() > deadline {
            return Err(format!("`c` never ran beside `a` while `b` waited: {jobs:?}").into());
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    std::fs::write(f.data.join("go"), "")?;
    f.settle().await?;
    Ok(())
}

/// ADR-0353 (amended 17 Nov 2026): CI's CPU pressure holds no job back. A job that confines
/// itself to a few CPUs (`taskset`) shows pressure while the host idles; on 17 Nov one project's
/// pinned jobs held every sandbox's jobs back that way. What CI is measured to use decides.
#[tokio::test]
async fn cpu_pressure_holds_no_job_back() -> TestResult {
    let f = fixture_tuned(
        false,
        |data| {
            format!(
                "[[ci.jobs]]\nname = \"build\"\ncpus = 1\ncommand = 'if [ -e feature.txt ] || \
                 [ -e later.txt ]; then while [ ! -e \"{go}\" ]; do sleep 0.05; done; fi'\n",
                go = data.join("go").display()
            )
        },
        None,
        |config| config.ci.job_timeout = WAIT,
    )
    .await?;
    let measured = Measured::new(0, u32::MAX);
    measured.pressure.store(250, Ordering::SeqCst);
    assert!(f.h.app.set_isolation(measured.clone()));
    f.h.app.ci_sync_sandbox(&f.sandbox).await?;
    let pull = f.running("pr/1").await?;
    let pull_sha = pull["sha"].as_str().ok_or("sha")?.to_owned();
    let later = f.push_dev("later.txt", Some("later\n"))?;
    f.h.app.ci_sync_sandbox(&f.sandbox).await?;
    let deadline = tokio::time::Instant::now() + WAIT;
    while only_job_state(&f, &later).await? != Some(RunState::Running) {
        f.h.app.ci_dispatch().await;
        if tokio::time::Instant::now() > deadline {
            return Err("the job never started beside one whose processes wait for CPUs".into());
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert_eq!(
        only_job_state(&f, &pull_sha).await?,
        Some(RunState::Running)
    );
    std::fs::write(f.data.join("go"), "")?;
    f.settle().await?;
    Ok(())
}

/// A job command that counts its attempts per checkout in `<data>/attempts-<checkout>` and
/// runs `first` on the first one, `again` on the later ones.
fn attempts(data: &Path, first: &str, again: &str) -> String {
    format!(
        "[[ci.jobs]]\nname = \"build\"\ncommand = 'm=\"{}/attempts-$(echo \"$PWD\" | cksum | cut -d\" \" -f1)\"; \
         echo x >> \"$m\"; if [ \"$(wc -l < \"$m\")\" -eq 1 ]; then {first}; else {again}; fi'\n",
        data.display()
    )
}

/// The number of attempts recorded in each attempt file.
fn attempts_of(f: &Fixture) -> BoxResult<Vec<usize>> {
    let mut counts = Vec::new();
    for entry in std::fs::read_dir(&f.data)? {
        let entry = entry?;
        if entry.file_name().to_string_lossy().starts_with("attempts-") {
            counts.push(std::fs::read_to_string(entry.path())?.lines().count());
        }
    }
    counts.sort_unstable();
    Ok(counts)
}

/// The pull request's run's only job, from its detail.
async fn pull_job(f: &Fixture) -> BoxResult<Value> {
    let overview = f.get("/api/sandboxes/s1/ci").await?;
    for run in overview["runs"].as_array().ok_or("runs")? {
        if run["reference"] == "pr/1" {
            let jobs = run_jobs(f, run).await?;
            return jobs.into_iter().next().ok_or_else(|| "no job".into());
        }
    }
    Err(format!("no run of the pull request: {overview}").into())
}

const NO_POOL: &str = "echo \"Error response from daemon: could not find an available, \
                       non-overlapping IPv4 address pool among the defaults to assign to the \
                       network\"; exit 1";

/// SPEC-497 OFC-001: a job that met an infrastructure failure runs again once by itself and
/// passes, saying so in its output and its reason; its run passes.
#[tokio::test]
async fn an_infrastructure_failure_runs_the_job_again_once() -> TestResult {
    let f = fixture_with(false, |data| attempts(data, NO_POOL, "echo ok")).await?;
    f.h.app.ci_sync_sandbox(&f.sandbox).await?;
    f.settle().await?;
    let job = pull_job(&f).await?;
    assert_eq!(job["state"], "passed", "{job}");
    assert_eq!(
        job["reason"],
        "passed when run again: its first run ended for an infrastructure failure (Docker had \
         no free address pool for a new network)"
    );
    let output = job["output"].as_str().unwrap_or_default();
    assert!(
        output.contains(
            "[yulius] Docker had no free address pool for a new network: an infrastructure \
             failure, not the change; the job runs again once by itself (SPEC-497)"
        ) && output.contains(
            "[yulius] passed when run again: its first run ended for an infrastructure failure \
             (Docker had no free address pool for a new network)"
        ),
        "{output}"
    );
    // dev's run and the pull request's: twice each.
    assert_eq!(attempts_of(&f)?, [2, 2]);
    Ok(())
}

/// SPEC-497 OFC-001: a second infrastructure failure ends the job as an error saying so, never
/// a third run; a test's failure is never run again.
#[tokio::test]
async fn a_job_runs_again_once_and_only_for_the_infrastructure() -> TestResult {
    let f = fixture_with(false, |data| attempts(data, NO_POOL, NO_POOL)).await?;
    f.h.app.ci_sync_sandbox(&f.sandbox).await?;
    f.settle().await?;
    let job = pull_job(&f).await?;
    assert_eq!(job["state"], "error", "{job}");
    assert_eq!(
        job["reason"],
        "Docker had no free address pool for a new network, again when it was run again: an \
         infrastructure failure, not the change"
    );
    assert_eq!(attempts_of(&f)?, [2, 2]);

    let f = fixture_with(false, |data| {
        attempts(data, "echo \"test x ... FAILED\"; exit 101", "echo ok")
    })
    .await?;
    f.h.app.ci_sync_sandbox(&f.sandbox).await?;
    f.settle().await?;
    let job = pull_job(&f).await?;
    assert_eq!(job["state"], "failed", "{job}");
    assert!(job["reason"].is_null(), "{job}");
    assert_eq!(
        attempts_of(&f)?,
        [1, 1],
        "a test's failure is never run again"
    );
    Ok(())
}

/// A sandbox whose OOM count grows at every look (SPEC-497 OFC-001).
#[derive(Default)]
struct Oomy {
    looks: AtomicU64,
}

#[async_trait::async_trait]
impl SandboxIsolation for Oomy {
    async fn prepare(&self, _: &SandboxRef) -> AppResult<Option<SandboxJoin>> {
        Ok(None)
    }
    fn release(&self, _: &SandboxId) {}
    async fn limit_counters(&self, _: &SandboxId) -> Option<LimitCounters> {
        Some(LimitCounters {
            task_limit_hits: 0,
            oom_kills: self.looks.fetch_add(1, Ordering::SeqCst),
        })
    }
}

/// SPEC-497 OFC-001: a job killed (SIGKILL) while the kernel killed for memory in its sandbox
/// runs again once.
#[tokio::test]
async fn a_job_killed_for_memory_runs_again_once() -> TestResult {
    let f = fixture_with(false, |data| attempts(data, "kill -9 $$", "echo ok")).await?;
    assert!(f.h.app.set_isolation(Arc::new(Oomy::default())));
    f.h.app.ci_sync_sandbox(&f.sandbox).await?;
    f.settle().await?;
    let job = pull_job(&f).await?;
    assert_eq!(job["state"], "passed", "{job}");
    assert_eq!(
        job["reason"],
        "passed when run again: its first run ended for an infrastructure failure (a process of \
         it was killed for memory)"
    );
    Ok(())
}

/// SPEC-497 OFC-001 (review round 1): a Docker error the job went on from, then a test's
/// failure, is the change's: no second run. A first run thrown away for the infrastructure and
/// a second that fails a test is the change's failure, not an infrastructure error.
#[tokio::test]
async fn a_tests_failure_is_never_taken_for_the_infrastructure() -> TestResult {
    let survived = "echo \"Cannot connect to the Docker daemon at unix:///var/run/docker.sock. Is \
                    the docker daemon running?\"; echo \"test flaky ... FAILED\"; exit 101";
    let f = fixture_with(false, |data| attempts(data, survived, "echo ok")).await?;
    f.h.app.ci_sync_sandbox(&f.sandbox).await?;
    f.settle().await?;
    let job = pull_job(&f).await?;
    assert_eq!(job["state"], "failed", "{job}");
    assert_eq!(
        attempts_of(&f)?,
        [1, 1],
        "no second run after a test's failure"
    );

    let f = fixture_with(false, |data| {
        attempts(data, NO_POOL, "echo \"test x ... FAILED\"; exit 101")
    })
    .await?;
    f.h.app.ci_sync_sandbox(&f.sandbox).await?;
    f.settle().await?;
    let job = pull_job(&f).await?;
    assert_eq!(job["state"], "failed", "{job}");
    assert!(job["reason"].is_null(), "{job}");
    assert_eq!(attempts_of(&f)?, [2, 2]);
    Ok(())
}

/// SPEC-497 OFC-001 (review round 2): a Docker error the job went on from, then its own failure
/// in Go's, pytest's or jest's words, is the change's: no second run.
#[tokio::test]
async fn a_failure_in_any_runners_words_is_never_run_again() -> TestResult {
    for failure in [
        "echo \"--- FAIL: TestTotal (0.00s)\"; echo FAIL",
        "echo \"FAILED tests/test_x.py::test_total - assert 1 == 2\"",
        "echo \"FAIL src/x.test.ts\"",
    ] {
        let first = format!(
            "echo \"Cannot connect to the Docker daemon at unix:///var/run/docker.sock. Is the \
             docker daemon running?\"; {failure}; exit 1"
        );
        let f = fixture_with(false, |data| attempts(data, &first, "echo ok")).await?;
        f.h.app.ci_sync_sandbox(&f.sandbox).await?;
        f.settle().await?;
        let job = pull_job(&f).await?;
        assert_eq!(job["state"], "failed", "{failure}: {job}");
        assert_eq!(attempts_of(&f)?, [1, 1], "{failure}");
    }
    Ok(())
}

/// SPEC-497 OFC-001 (review round 2): a job with a failing test kept (nextest's block, with its
/// blank lines) is the change's, even when the test failed on Docker's words: no second run.
#[tokio::test]
async fn a_kept_failing_test_is_never_run_again() -> TestResult {
    let block = "printf \"        FAIL [   0.004s] (1/1) app tests::pool\\n  stdout ───\\n\\n    \
                 running 1 test\\n    thread \\x27tests::pool\\x27 panicked at src/lib.rs:3:5:\\n    \
                 Error response from daemon: could not find an available, non-overlapping IPv4 \
                 address pool among the defaults to assign to the network\\n\\n\"; exit 100";
    let f = fixture_with(false, |data| attempts(data, block, "echo ok")).await?;
    f.h.app.ci_sync_sandbox(&f.sandbox).await?;
    f.settle().await?;
    let job = pull_job(&f).await?;
    assert_eq!(job["state"], "failed", "{job}");
    assert_eq!(attempts_of(&f)?, [1, 1]);
    Ok(())
}

/// SPEC-497 OFC-001 (review round 2): only the second run's own output decides it: a first run
/// thrown away for the infrastructure, then a second that fails with no runner's words, is the
/// change's failure, not "again" the infrastructure.
#[tokio::test]
async fn the_second_run_is_judged_on_its_own_output() -> TestResult {
    let f = fixture_with(false, |data| {
        attempts(
            data,
            NO_POOL,
            "echo \"error: could not compile app\"; exit 1",
        )
    })
    .await?;
    f.h.app.ci_sync_sandbox(&f.sandbox).await?;
    f.settle().await?;
    let job = pull_job(&f).await?;
    assert_eq!(job["state"], "failed", "{job}");
    assert!(job["reason"].is_null(), "{job}");
    assert_eq!(attempts_of(&f)?, [2, 2]);
    Ok(())
}

/// SPEC-497 OFC-001 (review round 2): after a full address pool the second run waits for room
/// in the engine, saying so in the job's output, and a cancellation ends it at once, cancelled,
/// with no second start.
#[tokio::test]
async fn the_wait_before_a_second_run_can_be_cancelled() -> TestResult {
    let f = fixture_tuned(
        false,
        |data| {
            let go = data.join("go").display().to_string();
            let first = format!("while [ ! -e \"{go}\" ]; do sleep 0.05; done; {NO_POOL}");
            format!(
                "[[ci.jobs]]\nname = \"build\"\ncommand = 'if [ -e feature.txt ]; then \
                 m=\"{}/attempts-pr\"; echo x >> \"$m\"; if [ \"$(wc -l < \"$m\")\" -eq 1 ]; \
                 then {first}; else echo ok; fi; fi'\n",
                data.display()
            )
        },
        None,
        |config| config.ci.job_timeout = WAIT,
    )
    .await?;
    let measured = Measured::new(0, 1);
    assert!(f.h.app.set_isolation(measured.clone()));
    f.h.app.ci_sync_sandbox(&f.sandbox).await?;
    let pull = f.running("pr/1").await?;
    let run = pull["id"].as_str().ok_or("id")?.to_owned();
    // The engine fills once the first run has started (review round 3: before, the job could
    // still be at its first look for room); then that run meets the full pool.
    let deadline = tokio::time::Instant::now() + WAIT;
    while !f.data.join("attempts-pr").exists() {
        if tokio::time::Instant::now() > deadline {
            return Err("the first run never started".into());
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    measured.networks.store(4096, Ordering::SeqCst);
    std::fs::write(f.data.join("go"), "")?;
    let deadline = tokio::time::Instant::now() + WAIT;
    loop {
        let jobs = f.h.store.ci_jobs(&run, true).await?;
        if jobs
            .iter()
            .any(|j| j.output.contains("waits for its sandbox's Docker engine"))
        {
            break;
        }
        if tokio::time::Instant::now() > deadline {
            return Err(format!("the job never said it waits: {jobs:?}").into());
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let cancelled = f
        .post(&format!("/api/sandboxes/s1/ci/runs/{run}/cancel"), None)
        .await?;
    assert!(cancelled.status.is_success(), "{}", cancelled.body);
    let deadline = tokio::time::Instant::now() + WAIT;
    loop {
        let jobs = f.h.store.ci_jobs(&run, false).await?;
        if jobs.iter().all(|j| j.state == RunState::Cancelled) {
            break;
        }
        if tokio::time::Instant::now() > deadline {
            return Err(format!("the job never ended cancelled: {jobs:?}").into());
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let tries = std::fs::read_to_string(f.data.join("attempts-pr"))?;
    assert_eq!(tries.lines().count(), 1, "no second start");
    Ok(())
}

/// SPEC-497 OFC-001 (review round 3): a test's failure in Go's or pytest's words before Docker's
/// words at the end (a teardown or a log dump) is the change's: no second run.
#[tokio::test]
async fn a_failure_before_dockers_words_is_never_run_again() -> TestResult {
    for first in [
        "echo \"--- FAIL: TestTotal (0.00s)\"; echo FAIL; echo \"Cannot connect to the Docker \
         daemon at unix:///var/run/docker.sock. Is the docker daemon running?\"; exit 1",
        "echo \"FAILED tests/test_x.py::test_total - assert 1 == 2\"; echo \"1 failed in 0.03s\"; \
         echo \"app-1  | dial unix /var/run/docker.sock: connect: connection refused\"; exit 1",
    ] {
        let f = fixture_with(false, |data| attempts(data, first, "echo ok")).await?;
        f.h.app.ci_sync_sandbox(&f.sandbox).await?;
        f.settle().await?;
        let job = pull_job(&f).await?;
        assert_eq!(job["state"], "failed", "{first}: {job}");
        assert_eq!(attempts_of(&f)?, [1, 1], "{first}");
    }
    Ok(())
}

/// SPEC-497 OFC-001 (review round 3): past the bound of the wait for network room, the job ends
/// error saying its engine had no room, with no second run.
#[tokio::test]
async fn a_second_run_that_finds_no_network_room_is_not_made() -> TestResult {
    let f = fixture_tuned(
        false,
        |data| {
            format!(
                "[[ci.jobs]]\nname = \"build\"\ncommand = 'echo x >> \"{dir}/attempts-$YULIUS_CI_RUN\"; \
                 while [ ! -e \"{dir}/go\" ]; do sleep 0.05; done; {NO_POOL}'\n",
                dir = data.display()
            )
        },
        None,
        |config| {
            config.ci.max_jobs = 1;
            config.ci.job_timeout = WAIT * 3;
            config.ci.rerun_network_wait = Duration::from_millis(500);
        },
    )
    .await?;
    let measured = Measured::new(0, 1);
    assert!(f.h.app.set_isolation(measured.clone()));
    let go = f.data.join("go");
    let refuse_retry = async |started_runs: usize| -> TestResult {
        f.h.app.ci_sync_sandbox(&f.sandbox).await?;
        let deadline = tokio::time::Instant::now() + WAIT;
        while attempts_of(&f)?.len() < started_runs {
            f.h.app.ci_dispatch().await;
            if tokio::time::Instant::now() > deadline {
                return Err("the job never started".into());
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        measured.networks.store(4096, Ordering::SeqCst);
        std::fs::write(&go, "")?;
        f.settle().await
    };
    // Admit the branch first. A one-CPU runner cannot start the pull while the branch holds it.
    let pull =
        f.h.github
            .pulls
            .lock()
            .map_err(|_| "pulls")?
            .pop()
            .ok_or("pull")?;
    refuse_retry(1).await?;
    std::fs::remove_file(&go)?;
    measured.networks.store(1, Ordering::SeqCst);
    f.h.github.pulls.lock().map_err(|_| "pulls")?.push(pull);
    refuse_retry(2).await?;
    let job = pull_job(&f).await?;
    assert_eq!(job["state"], "error", "{job}");
    assert_eq!(
        job["reason"],
        "Docker had no free address pool for a new network, and its sandbox's Docker engine had \
         no room for a network for 1 min, so it was not run again: an infrastructure failure, not \
         the change"
    );
    assert_eq!(attempts_of(&f)?, [1, 1]);
    assert!(
        f.h.app.config.ci.job_timeout > WAIT * 2,
        "the job deadline must outlast both the start and settle waits"
    );
    Ok(())
}

/// A sandbox whose OOM count never grows (SPEC-497 OFC-001, review round 1).
struct NoOom;

#[async_trait::async_trait]
impl SandboxIsolation for NoOom {
    async fn prepare(&self, _: &SandboxRef) -> AppResult<Option<SandboxJoin>> {
        Ok(None)
    }
    fn release(&self, _: &SandboxId) {}
    async fn limit_counters(&self, _: &SandboxId) -> Option<LimitCounters> {
        Some(LimitCounters {
            task_limit_hits: 0,
            oom_kills: 3,
        })
    }
}

/// SPEC-497 OFC-001 (review round 1): a SIGKILL while the kernel killed nothing for memory is
/// the job's own failure: no second run.
#[tokio::test]
async fn a_kill_without_a_kill_for_memory_is_not_run_again() -> TestResult {
    let f = fixture_with(false, |data| attempts(data, "kill -9 $$", "echo ok")).await?;
    assert!(f.h.app.set_isolation(Arc::new(NoOom)));
    f.h.app.ci_sync_sandbox(&f.sandbox).await?;
    f.settle().await?;
    let job = pull_job(&f).await?;
    assert_eq!(job["state"], "failed", "{job}");
    assert_eq!(attempts_of(&f)?, [1, 1]);
    Ok(())
}

/// A job that fails nextest's way on `app tests::flaky` when any of `files` is in its checkout,
/// and prints the nextest filter it was given.
fn flaky_job(files: &[&str]) -> String {
    let block = "printf \"        FAIL [   0.004s] (1/1) app tests::flaky\\n  stdout ───\\n\\n    \
                 assertion failed: the race lost\\n\\n\"; exit 100";
    let test = files
        .iter()
        .map(|f| format!("[ -e {f} ]"))
        .collect::<Vec<_>>()
        .join(" || ");
    format!(
        "[[ci.jobs]]\nname = \"quality\"\ncommand = 'echo \"q=[$YULIUS_CI_QUARANTINE_NEXTEST]\"; \
         if {test}; then {block}; fi'\n"
    )
}

/// Opens pull request `number` from a new branch `branch` made from `from` with `file` added;
/// its head.
fn open_pull(f: &Fixture, number: u64, branch: &str, from: &str, file: &str) -> BoxResult<String> {
    git(&f.work, &["checkout", "-q", "-b", branch, from])?;
    std::fs::write(f.work.join(file), "x\n")?;
    git(&f.work, &["add", "-A"])?;
    git(&f.work, &["commit", "-q", "-m", file])?;
    git(&f.work, &["push", "-q", "origin", branch])?;
    let head = git(&f.work, &["rev-parse", "HEAD"])?;
    git(&f.work, &["checkout", "-q", "feat/one"])?;
    if let Ok(mut pulls) = f.h.github.pulls.lock() {
        pulls.push(GhPull {
            number,
            html_url: format!("https://github.test/acme/app/pull/{number}"),
            title: branch.into(),
            state: "open".into(),
            merged: false,
            base: "dev".into(),
            base_sha: String::new(),
            head: branch.into(),
            head_sha: head.clone(),
            mergeable: None,
            behind: false,
            from_fork: false,
            merge_commit_sha: None,
        });
    }
    Ok(head)
}

/// The only job of the run of `reference` (`pr/2`) or at `sha`, with its output.
async fn job_of(f: &Fixture, key: &str, value: &str) -> BoxResult<Value> {
    let overview = f.get("/api/sandboxes/s1/ci").await?;
    let run = overview["runs"]
        .as_array()
        .ok_or("runs")?
        .iter()
        .find(|r| r[key] == value)
        .ok_or_else(|| format!("no run with {key} {value}: {overview}"))?
        .clone();
    Ok(run_jobs(f, &run).await?.into_iter().next().ok_or("job")?)
}

async fn sync_and_settle(f: &Fixture) -> BoxResult<()> {
    f.h.app.ci_sync_sandbox(&f.sandbox).await?;
    f.settle().await
}

fn issue_titles(f: &Fixture) -> BoxResult<Vec<String>> {
    Ok(f.h
        .github
        .issues
        .lock()
        .map_err(|_| "lock")?
        .iter()
        .map(|(_, issue)| issue.title.clone())
        .collect())
}

/// SPEC-497 OFC-002: a test that fails in the runs of two different pull requests is quarantined
/// at once with one issue, the job that found it says so, a later job of the project is told it
/// in its environment, and closing the issue ends the quarantine. The failing test is never run
/// again by Yulius (each pull request's job ran once).
#[tokio::test]
async fn a_test_failing_on_two_pull_requests_is_quarantined_with_an_issue() -> TestResult {
    let f = fixture_with(false, |_| {
        flaky_job(&["feature.txt", "two.txt", "three.txt", "four.txt"])
    })
    .await?;
    sync_and_settle(&f).await?;
    assert!(
        issue_titles(&f)?.is_empty(),
        "one pull request is not enough"
    );
    // A second pull request, from dev, fails the same test.
    open_pull(&f, 2, "feat/two", "origin/dev", "two.txt")?;
    sync_and_settle(&f).await?;
    assert_eq!(
        issue_titles(&f)?,
        ["Flaky test quarantined: app tests::flaky"]
    );
    let job = job_of(&f, "reference", "pr/2").await?;
    assert_eq!(job["state"], "failed", "{job}");
    assert_eq!(
        job["reason"], "a failing test is now quarantined as flaky (see the output)",
        "{job}"
    );
    let output = job["output"].as_str().unwrap_or_default();
    assert!(
        output.contains("`app tests::flaky` failed in the runs of #1, #2 within 48 hours")
            && output.contains("quarantined as a flaky test (issue #101)"),
        "{output}"
    );
    // Nothing ran twice: the job's own failure is printed once.
    assert_eq!(output.matches("the race lost").count(), 1, "{output}");
    let first = job_of(&f, "reference", "pr/1").await?;
    let first_output = first["output"].as_str().unwrap_or_default();
    assert_eq!(first_output.matches("the race lost").count(), 1, "{first}");
    // The next job of the project is told it, as a filter of test paths.
    let later = f.push_dev("later.txt", Some("later\n"))?;
    sync_and_settle(&f).await?;
    let dev_job = job_of(&f, "sha", &later).await?;
    assert!(
        dev_job["output"]
            .as_str()
            .unwrap_or_default()
            .contains("q=[test(=tests::flaky)]"),
        "{dev_job}"
    );
    // A third pull request failing it files no second issue, and says it is quarantined.
    open_pull(&f, 3, "feat/three", "origin/dev", "three.txt")?;
    sync_and_settle(&f).await?;
    assert_eq!(issue_titles(&f)?.len(), 1);
    let third = job_of(&f, "reference", "pr/3").await?;
    let third_output = third["output"].as_str().unwrap_or_default();
    assert!(
        third_output.contains("`app tests::flaky` is quarantined as flaky (issue #101)"),
        "{third_output}"
    );
    assert!(third["reason"].is_null(), "{third}");
    // An open issue, or one that cannot be read, keeps the quarantine.
    f.h.app.ci_release_quarantines().await;
    assert_eq!(f.h.store.quarantined_tests(None).await?.len(), 1, "open");
    f.h.github.fail_issue_reads.store(true, Ordering::SeqCst);
    f.h.github
        .closed_issues
        .lock()
        .map_err(|_| "lock")?
        .insert(101, Vec::new());
    f.h.app.ci_release_quarantines().await;
    assert_eq!(f.h.store.quarantined_tests(None).await?.len(), 1, "unread");
    // Closing the issue ends the quarantine, and forgets the failures that made it.
    f.h.github.fail_issue_reads.store(false, Ordering::SeqCst);
    f.h.app.ci_release_quarantines().await;
    let left = f.h.store.quarantined_tests(None).await?;
    assert!(left.is_empty(), "{left:?}");
    open_pull(&f, 4, "feat/four", "origin/dev", "four.txt")?;
    sync_and_settle(&f).await?;
    assert_eq!(issue_titles(&f)?.len(), 1, "one failure after the release");
    Ok(())
}

/// SPEC-497 OFC-002 (review round 1): a test the integration branch failed too is broken there,
/// not flaky; two pull requests whose heads share a commit (a chained one) are one change.
#[tokio::test]
async fn no_quarantine_when_the_integration_branch_or_a_shared_commit_fails_it() -> TestResult {
    let f = fixture_with(false, |_| flaky_job(&["devfail.txt"])).await?;
    f.push_dev("devfail.txt", Some("x\n"))?;
    sync_and_settle(&f).await?;
    open_pull(&f, 2, "feat/two", "origin/dev", "two.txt")?;
    open_pull(&f, 3, "feat/three", "origin/dev", "three.txt")?;
    sync_and_settle(&f).await?;
    for reference in ["pr/2", "pr/3"] {
        let job = job_of(&f, "reference", reference).await?;
        assert_eq!(job["state"], "failed", "{job}");
    }
    assert!(
        issue_titles(&f)?.is_empty(),
        "the integration branch fails it"
    );

    let f = fixture_with(false, |_| flaky_job(&["feature.txt"])).await?;
    sync_and_settle(&f).await?;
    // Chained on feat/one: it carries the commit that breaks the test.
    open_pull(&f, 2, "feat/two", "feat/one", "two.txt")?;
    sync_and_settle(&f).await?;
    let job = job_of(&f, "reference", "pr/2").await?;
    assert_eq!(job["state"], "failed", "{job}");
    assert!(
        issue_titles(&f)?.is_empty(),
        "one change in two pull requests"
    );
    // Both merge a newer `dev` commit: their heads then have two merge bases, the shared commit
    // and that one, and plain `git merge-base` names only the latter (review round 2).
    f.push_dev("newer.txt", Some("newer\n"))?;
    for (number, branch) in [(1, "feat/one"), (2, "feat/two")] {
        git(&f.work, &["checkout", "-q", branch])?;
        git(&f.work, &["merge", "-q", "--no-edit", "origin/dev"])?;
        git(&f.work, &["push", "-q", "origin", branch])?;
        let head = git(&f.work, &["rev-parse", "HEAD"])?;
        if let Ok(mut pulls) = f.h.github.pulls.lock()
            && let Some(pull) = pulls.iter_mut().find(|p| p.number == number)
        {
            pull.head_sha = head;
        }
    }
    git(&f.work, &["checkout", "-q", "feat/one"])?;
    sync_and_settle(&f).await?;
    let merged = git(&f.work, &["rev-parse", "feat/two"])?;
    let job = job_of(&f, "sha", &merged).await?;
    assert_eq!(job["state"], "failed", "{job}");
    assert!(issue_titles(&f)?.is_empty(), "still one change");
    // A head the clone does not have cannot be compared: no quarantine rests on it.
    let runs = f.h.store.ci_runs(&f.sandbox.id, 20).await?;
    let first = runs
        .iter()
        .find(|r| r.reference.key() == "pr/1")
        .ok_or("pr/1's run")?
        .clone();
    let job =
        f.h.store
            .ci_jobs(&first.id, false)
            .await?
            .into_iter()
            .next()
            .ok_or("job")?;
    let mut unknown = first.clone();
    unknown.id = "unknown-head".into();
    unknown.reference = yulius_domain::ci::CiRef::PullRequest(9);
    unknown.sha = "0123456789abcdef0123456789abcdef01234567".into();
    let noted =
        f.h.app
            .ci_note_failing_tests(
                &f.sandbox,
                &unknown,
                &job,
                "FAIL [   0.004s] (1/1) app tests::flaky\n  stdout ───\n",
            )
            .await;
    assert!(!noted.quarantined, "{noted:?}");
    assert!(issue_titles(&f)?.is_empty(), "an unknown head");
    Ok(())
}

/// SPEC-497 OFC-002 (review round 1): when the issue cannot be filed nothing is quarantined, and
/// the output says why; a job failing more than 10 tests, kept or only counted, quarantines none.
#[tokio::test]
async fn no_quarantine_without_its_issue_or_for_a_broken_job() -> TestResult {
    let f = fixture_with(false, |_| flaky_job(&["feature.txt", "two.txt"])).await?;
    f.h.github.fail_issues.store(true, Ordering::SeqCst);
    sync_and_settle(&f).await?;
    open_pull(&f, 2, "feat/two", "origin/dev", "two.txt")?;
    sync_and_settle(&f).await?;
    let job = job_of(&f, "reference", "pr/2").await?;
    let output = job["output"].as_str().unwrap_or_default();
    assert!(
        output.contains(
            "`app tests::flaky` looks flaky (it failed in the runs of #1, #2), but it \
                         was not quarantined: its issue could not be filed"
        ),
        "{output}"
    );
    assert!(job["reason"].is_null(), "{job}");
    assert!(f.h.store.quarantined_tests(None).await?.is_empty());

    // Twelve failing tests of 9 KiB each: four kept, eight only counted.
    let many = "for t in 1 2 3 4 5 6 7 8 9 10 11 12; do printf \"        FAIL [   0.004s] (1/12) \
                app tests::t$t\\n  stdout ───\\n\"; for l in $(seq 1 100); do echo \"    \
                line $l of a long failure output that goes on and on and on\"; done; \
                echo; done; exit 100";
    let f = fixture_with(false, move |_| {
        format!(
            "[[ci.jobs]]\nname = \"quality\"\ncommand = 'if [ -e feature.txt ] || \
             [ -e two.txt ]; then {many}; fi'\n"
        )
    })
    .await?;
    sync_and_settle(&f).await?;
    open_pull(&f, 2, "feat/two", "origin/dev", "two.txt")?;
    sync_and_settle(&f).await?;
    let job = job_of(&f, "reference", "pr/2").await?;
    assert_eq!(job["state"], "failed", "{job}");
    assert!(
        issue_titles(&f)?.is_empty(),
        "a broken job, not a flaky test"
    );
    Ok(())
}

/// SPEC-497 OFC-002 (review round 1): two jobs that make the same test flaky at once file one
/// issue; the second says it is quarantined.
#[tokio::test]
async fn two_jobs_at_once_file_one_issue() -> TestResult {
    let f = fixture_with(false, |_| flaky_job(&["feature.txt"])).await?;
    sync_and_settle(&f).await?;
    let runs = f.h.store.ci_runs(&f.sandbox.id, 10).await?;
    let first = runs
        .iter()
        .find(|r| r.reference.key() == "pr/1")
        .ok_or("pr/1's run")?
        .clone();
    let job =
        f.h.store
            .ci_jobs(&first.id, false)
            .await?
            .into_iter()
            .next()
            .ok_or("job")?;
    let dev = git(&f.work, &["rev-parse", "origin/dev"])?;
    let other = |number: u64| {
        let mut run = first.clone();
        run.id = format!("race-{number}");
        run.reference = yulius_domain::ci::CiRef::PullRequest(number);
        run.sha = dev.clone();
        run
    };
    let (two, three) = (other(2), other(3));
    let failures = "FAIL [   0.004s] (1/1) app tests::flaky\n  stdout ───\n";
    let hold = Arc::new(tokio::sync::Notify::new());
    *f.h.github.issue_hold.lock().map_err(|_| "lock")? = Some(Arc::clone(&hold));
    let release = async {
        f.h.github.issue_entered.notified().await;
        if let Ok(mut held) = f.h.github.issue_hold.lock() {
            *held = None;
        }
        hold.notify_waiters();
    };
    let (a, b, ()) = tokio::join!(
        f.h.app
            .ci_note_failing_tests(&f.sandbox, &two, &job, failures),
        f.h.app
            .ci_note_failing_tests(&f.sandbox, &three, &job, failures),
        release
    );
    assert_eq!(issue_titles(&f)?.len(), 1, "{a:?} {b:?}");
    assert!(a.quarantined != b.quarantined, "{a:?} {b:?}");
    let second = if a.quarantined { &b } else { &a };
    assert!(
        second
            .lines
            .iter()
            .any(|l| l.contains("is quarantined as flaky (issue #101)")),
        "{second:?}"
    );
    Ok(())
}

/// The jobs of `run`, from its detail, in order.
async fn run_jobs(f: &Fixture, run: &Value) -> BoxResult<Vec<Value>> {
    let id = run["id"].as_str().ok_or("id")?;
    let detail = f.get(&format!("/api/sandboxes/s1/ci/runs/{id}")).await?;
    Ok(detail["jobs"].as_array().ok_or("jobs")?.clone())
}

/// SPEC-504 review round 2: a pull request that changes the command of an `exclusive` job runs
/// the integration branch's job and its own changed one one at a time: they share the job's turn
/// (its definition and matrix value), not one each by their names.
#[tokio::test]
async fn an_exclusive_job_and_its_changed_twin_share_a_turn() -> TestResult {
    let command = |data: &Path| {
        format!(
            "b=\"{dir}/busy\"; if [ -e \"$b\" ]; then echo overlap; exit 7; fi; touch \"$b\"; \
             sleep 1; rm \"$b\"",
            dir = data.display()
        )
    };
    let f = fixture_tuned(
        false,
        |data| {
            format!(
                "[[ci.jobs]]\nname = \"site\"\ncpus = 1\nexclusive = true\ncommand = '{}'\n",
                command(data)
            )
        },
        None,
        |config| config.ci.lanes = 1,
    )
    .await?;
    let changed = format!(
        "[[ci.jobs]]\nname = \"site\"\ncpus = 1\nexclusive = true\ncommand = '{}; echo changed'\n",
        command(&f.data)
    );
    let sha = f.push_feature("yulius.toml", Some(&changed))?;
    f.h.app.ci_sync_sandbox(&f.sandbox).await?;
    f.settle().await?;
    let detail = run_of_sha(&f, &sha).await?;
    assert_eq!(detail["state"], "passed", "{detail}");
    let names: Vec<&str> = detail["jobs"]
        .as_array()
        .ok_or("jobs")?
        .iter()
        .filter_map(|job| job["name"].as_str())
        .collect();
    assert_eq!(names, ["site", "site (this change)"], "{detail}");
    Ok(())
}

/// SPEC-435 JTL-001/002/004: three runs of an `exclusive` job run one at a time (a job that finds
/// another inside exits 7), and the last one, which waited longer than the 5 s limit for its turn,
/// still passes: the wait is outside its limit, and its record says how long it waited.
#[tokio::test]
async fn an_exclusive_job_waits_its_turn_outside_its_time_limit() -> TestResult {
    let f = fixture_tuned(
        false,
        |data| {
            format!(
                "[[ci.jobs]]\nname = \"site\"\ncpus = 1\nexclusive = true\ncommand = 'b=\"{dir}/busy\"; \
                 if [ -e \"$b\" ]; then echo overlap; exit 7; fi; touch \"$b\"; sleep 3.5; rm \"$b\"'\n",
                dir = data.display()
            )
        },
        None,
        |config| {
            config.ci.job_timeout = Duration::from_secs(5);
            config.ci.lanes = 3;
        },
    )
    .await?;
    // main, dev and the pull request: three runs, each in its own lane.
    f.push_dev("other.txt", Some("dev\n"))?;
    f.h.app.ci_sync_sandbox(&f.sandbox).await?;
    f.settle().await?;
    let overview = f.get("/api/sandboxes/s1/ci").await?;
    let jobs = overview["gate"]["jobs"].as_array().ok_or("gate jobs")?;
    assert_eq!(jobs[0]["exclusive"], true, "{overview}");
    let runs = overview["runs"].as_array().ok_or("runs")?;
    assert_eq!(runs.len(), 3, "{overview}");
    let mut waits = Vec::new();
    for run in runs {
        let jobs = run_jobs(&f, run).await?;
        assert_eq!(run["state"], "passed", "{jobs:?}");
        waits.push(
            jobs[0]["waited_ms"]
                .as_i64()
                .ok_or(format!("waited_ms: {jobs:?}"))?,
        );
    }
    waits.sort_unstable();
    assert!(
        waits[2] > 5_000,
        "the last job waited longer than its limit: {waits:?}"
    );
    Ok(())
}

/// The `site` and `lint` jobs of every run, by run id, once `ready` holds for them all.
async fn until_runs(
    f: &Fixture,
    ready: impl Fn(&[(String, Vec<Value>)]) -> bool,
) -> BoxResult<Vec<(String, Vec<Value>)>> {
    let deadline = tokio::time::Instant::now() + WAIT;
    loop {
        f.h.app.ci_dispatch().await;
        let overview = f.get("/api/sandboxes/s1/ci").await?;
        let mut runs = Vec::new();
        for run in overview["runs"].as_array().ok_or("runs")? {
            runs.push((
                run["id"].as_str().ok_or("id")?.to_owned(),
                run_jobs(f, run).await?,
            ));
        }
        if ready(&runs) {
            return Ok(runs);
        }
        if tokio::time::Instant::now() > deadline {
            return Err(format!("CI never got there: {runs:?}").into());
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// The state of `name` in `jobs`.
fn job_state<'a>(jobs: &'a [Value], name: &str) -> &'a str {
    jobs.iter()
        .find(|j| j["name"] == name)
        .and_then(|j| j["state"].as_str())
        .unwrap_or("?")
}

/// One run's `site` runs and the other's waits for its turn, and, with `linted`, every `lint`
/// passed (it reached the turn: a run starts its jobs together).
fn one_runs_one_waits(runs: &[(String, Vec<Value>)], linted: bool) -> bool {
    let mut sites: Vec<&str> = runs.iter().map(|(_, j)| job_state(j, "site")).collect();
    sites.sort_unstable();
    sites == ["queued", "running"]
        && (!linted || runs.iter().all(|(_, j)| job_state(j, "lint") == "passed"))
}

/// An `exclusive` job that blocks until `<data>/go` exists, and a quick `lint`.
fn turn_toml(data: &Path) -> String {
    format!(
        "[[ci.jobs]]\nname = \"site\"\ncpus = 1\nexclusive = true\n\
         command = 'while [ ! -e \"{dir}/go\" ]; do sleep 0.05; done'\n\n\
         [[ci.jobs]]\nname = \"lint\"\ncpus = 1\ncommand = 'true'\n",
        dir = data.display()
    )
}

/// SPEC-435 JTL-001: a job waiting for its turn holds no job slot. With two slots, one run's
/// `site` runs and the other's waits for its turn, and both runs' `lint` still run meanwhile; a
/// waiting `site` holding a slot would leave `lint` queued until the running one ends.
#[tokio::test]
async fn a_job_waiting_for_its_turn_holds_no_slot() -> TestResult {
    let f = fixture_tuned(false, turn_toml, None, |config| config.ci.max_jobs = 2).await?;
    f.h.app.ci_sync_sandbox(&f.sandbox).await?;
    // Both lints passed while one site runs and the other waits for its turn; before the go, a
    // running site never ends, so this is reached only if the lints got slots meanwhile.
    until_runs(&f, |runs| runs.len() == 2 && one_runs_one_waits(runs, true)).await?;
    std::fs::write(f.data.join("go"), "")?;
    f.settle().await?;
    let overview = f.get("/api/sandboxes/s1/ci").await?;
    for run in overview["runs"].as_array().ok_or("runs")? {
        assert_eq!(run["state"], "passed", "{overview}");
    }
    Ok(())
}

/// Two jobs per run that run until `<data>/go` exists.
fn two_held_toml(data: &Path) -> String {
    let hold = format!(
        "cpus = 1\ncommand = 'while [ ! -e \"{dir}/go\" ]; do sleep 0.05; done'\n",
        dir = data.display()
    );
    format!("[[ci.jobs]]\nname = \"a\"\n{hold}\n[[ci.jobs]]\nname = \"b\"\n{hold}")
}

/// SPEC-459 HSL-006: the limit of CI jobs at once changes while jobs run. With one slot one of
/// the four jobs runs; raised to three in Settings, two of the waiting ones start beside it
/// without a restart and without the running one ending.
#[tokio::test]
async fn a_raised_job_limit_starts_waiting_jobs_while_one_runs() -> TestResult {
    use yulius_domain::limits::Limit;
    let f = fixture_with(false, two_held_toml).await?;
    f.h.app.load_limits([(Limit::CiJobs, 1)].into()).await;
    let admin = support::superadmin_bearer(&f.h).await?;
    f.h.app.ci_sync_sandbox(&f.sandbox).await?;
    let running = |runs: &[(String, Vec<Value>)]| {
        runs.iter()
            .flat_map(|(_, jobs)| jobs)
            .filter(|job| job["state"] == "running")
            .count()
    };
    let queued = |runs: &[(String, Vec<Value>)]| {
        runs.iter()
            .flat_map(|(_, jobs)| jobs)
            .filter(|job| job["state"] == "queued")
            .count()
    };
    until_runs(&f, |runs| running(runs) == 1 && queued(runs) == 3).await?;
    let raised = call(
        &f.h.router,
        Method::PUT,
        "/api/admin/limits/ci_max_jobs",
        Some(serde_json::json!({ "value": 3 })),
        &admin,
    )
    .await?;
    assert_eq!(raised.status, StatusCode::OK, "{}", raised.body);
    until_runs(&f, |runs| running(runs) == 3 && queued(runs) == 1).await?;
    std::fs::write(f.data.join("go"), "")?;
    f.settle().await?;
    let overview = f.get("/api/sandboxes/s1/ci").await?;
    for run in overview["runs"].as_array().ok_or("runs")? {
        assert_eq!(run["state"], "passed", "{overview}");
    }
    Ok(())
}

/// Each job logs its start and end, and waits for its own release or a release of all jobs.
fn two_logged_toml(data: &Path) -> String {
    let dir = data.display();
    let job = |name: &str| {
        format!(
            "[[ci.jobs]]\nname = \"{name}\"\ncpus = 1\ncommand = 'echo s >> \"{dir}/log\"; \
             while [ ! -e \"{dir}/go-$YULIUS_CI_RUN-{name}\" ] && [ ! -e \"{dir}/go-all\" ]; do sleep 0.05; done; echo e >> \"{dir}/log\"'\n"
        )
    };
    format!("{}\n{}", job("slot-waiter"), job("b"))
}

/// SPEC-459 HSL-007: lowered in Settings while jobs run, the limit lets them finish and starts
/// no waiting job beside them: with two of four jobs running and the limit lowered to one, no
/// two jobs run at once after the first of them ended.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_lowered_job_limit_starts_no_job_beside_the_ones_it_lets_finish() -> TestResult {
    use yulius_domain::limits::Limit;
    let f = fixture_tuned(false, two_logged_toml, None, |config| {
        config.ci.job_timeout = WAIT * 3;
    })
    .await?;
    f.h.app.load_limits([(Limit::CiJobs, 2)].into()).await;
    let admin = support::superadmin_bearer(&f.h).await?;
    // Make both runs of b take the initial slots, a valid admission order.
    let (dev_at, dev_go) = support::hold_point(
        yulius_app::race::Point::CiJobAdmission,
        "branch/dev/slot-waiter",
    );
    let (pr_at, pr_go) =
        support::hold_point(yulius_app::race::Point::CiJobAdmission, "pr/1/slot-waiter");
    f.h.app.ci_sync_sandbox(&f.sandbox).await?;
    let in_state = |runs: &[(String, Vec<Value>)], state: &str| {
        runs.iter()
            .flat_map(|(_, jobs)| jobs)
            .filter(|job| job["state"] == state)
            .count()
    };
    let runs = until_runs(&f, |runs| {
        in_state(runs, "running") == 2
            && in_state(runs, "queued") == 2
            && std::fs::read_to_string(f.data.join("log")).is_ok_and(|log| log == "s\ns\n")
    })
    .await?;
    support::point_reached(dev_at).await?;
    support::point_reached(pr_at).await?;
    dev_go.send(())?;
    pr_go.send(())?;
    let lowered = call(
        &f.h.router,
        Method::PUT,
        "/api/admin/limits/ci_max_jobs",
        Some(serde_json::json!({ "value": 1 })),
        &admin,
    )
    .await?;
    assert_eq!(lowered.status, StatusCode::OK, "{}", lowered.body);
    let (run, job) = runs
        .iter()
        .find_map(|(run, jobs)| {
            let job = jobs.iter().find(|job| job["state"] == "running")?;
            Some((run, job["name"].as_str()?))
        })
        .ok_or("a running job to release")?;
    std::fs::write(f.data.join(format!("go-{run}-{job}")), "")?;
    until_runs(&f, |runs| in_state(runs, "passed") >= 1).await?;
    std::fs::write(f.data.join("go-all"), "")?;
    f.settle().await?;

    let log = std::fs::read_to_string(f.data.join("log"))?;
    assert_eq!(
        log.lines().collect::<Vec<_>>(),
        ["s", "s", "e", "e", "s", "e", "s", "e"],
        "both initial jobs must finish before the other jobs run one at a time"
    );
    Ok(())
}

/// SPEC-435 JTL-001: a job cancelled while it waits for its turn ends cancelled without having
/// started, and the turn still comes to a later run of the job.
#[tokio::test]
async fn a_job_cancelled_while_waiting_for_its_turn_never_starts() -> TestResult {
    let f = fixture_with(false, turn_toml).await?;
    f.h.app.ci_sync_sandbox(&f.sandbox).await?;
    // The lints passed: the waiting run's site is past its checkout, waiting for its turn.
    let runs = until_runs(&f, |runs| one_runs_one_waits(runs, true)).await?;
    let (waiting, _) = runs
        .iter()
        .find(|(_, jobs)| job_state(jobs, "site") == "queued")
        .ok_or("a waiting run")?;
    let cancelled = f
        .post(&format!("/api/sandboxes/s1/ci/runs/{waiting}/cancel"), None)
        .await?;
    assert_eq!(cancelled.status, StatusCode::OK, "{}", cancelled.body);
    let runs = until_runs(&f, |runs| {
        runs.iter()
            .any(|(id, jobs)| id == waiting && job_state(jobs, "site") == "cancelled")
    })
    .await?;
    let site = runs
        .iter()
        .find(|(id, _)| id == waiting)
        .and_then(|(_, jobs)| jobs.iter().find(|j| j["name"] == "site"))
        .ok_or("the cancelled site")?;
    assert!(site["waited_ms"].is_null(), "never started: {site}");
    assert!(site["duration_ms"].is_null(), "never started: {site}");
    // A later run of the job, queued while the first still holds the turn, waits for it and gets
    // it once the first ends: the cancelled waiter left nothing behind.
    let again = f
        .post(&format!("/api/sandboxes/s1/ci/runs/{waiting}/rerun"), None)
        .await?;
    assert_eq!(again.status, StatusCode::OK, "{}", again.body);
    let again = again.body["id"].as_str().ok_or("id")?.to_owned();
    until_runs(&f, |runs| {
        runs.iter()
            .any(|(id, jobs)| *id == again && job_state(jobs, "lint") == "passed")
            && one_runs_one_waits(
                &runs
                    .iter()
                    .filter(|(id, _)| id != waiting)
                    .cloned()
                    .collect::<Vec<_>>(),
                false,
            )
    })
    .await?;
    std::fs::write(f.data.join("go"), "")?;
    f.settle().await?;
    let detail = f.get(&format!("/api/sandboxes/s1/ci/runs/{again}")).await?;
    assert_eq!(detail["state"], "passed", "{detail}");
    Ok(())
}

/// SPEC-435 JTL-003/005: a job's `timeout_minutes` replaces the instance's limit (2 s here): the
/// job given a minute passes after 3 s; the other one is killed at 2 s and says how long it ran.
#[tokio::test]
async fn a_jobs_own_time_limit_replaces_the_instance_one() -> TestResult {
    let f = fixture_tuned(
        false,
        |_| {
            "[[ci.jobs]]\nname = \"long\"\ntimeout_minutes = 1\ncommand = 'sleep 3'\n\n\
             [[ci.jobs]]\nname = \"short\"\ncommand = 'sleep 30'\n"
                .to_owned()
        },
        None,
        |config| config.ci.job_timeout = Duration::from_secs(2),
    )
    .await?;
    f.h.app.ci_sync_sandbox(&f.sandbox).await?;
    f.settle().await?;
    let overview = f.get("/api/sandboxes/s1/ci").await?;
    let gate = overview["gate"]["jobs"].as_array().ok_or("gate jobs")?;
    assert_eq!(gate[0]["timeout_minutes"], 1, "{overview}");
    assert!(gate[1]["timeout_minutes"].is_null(), "{overview}");
    let run = overview["runs"]
        .as_array()
        .ok_or("runs")?
        .iter()
        .find(|r| r["reference"] == "pr/1")
        .cloned()
        .ok_or("pr/1 run")?;
    let jobs = run_jobs(&f, &run).await?;
    assert_eq!(jobs[0]["name"], "long");
    assert_eq!(jobs[0]["state"], "passed", "{jobs:?}");
    assert_eq!(jobs[1]["state"], "error", "{jobs:?}");
    let reason = jobs[1]["reason"].as_str().unwrap_or_default();
    assert!(
        reason.starts_with("timed out: ran 2 s, its time limit"),
        "{reason}"
    );
    Ok(())
}

/// SPEC-351 CCH-001, CCH-002 (pomerado-rust #460): two runs in the sandbox's two lanes never share
/// `$YULIUS_CI_CACHE`. Each job leaves a `live-<run>` marker in its cache and waits until both runs
/// are inside the job (one CPU each, so both fit the budget at once); with one cache shared by the
/// lanes each found the other's marker (a Cargo target directory kept there was relinked under the
/// other run). A later run of the same checkout finds its earlier run's `kept-<run>` marker: the
/// cache still persists.
#[tokio::test]
async fn concurrent_runs_never_share_a_ci_cache() -> TestResult {
    let f = fixture_with(false, |data| {
        format!(
            "[[ci.jobs]]\nname = \"build\"\ncpus = 1\ncommand = 'c=\"$YULIUS_CI_CACHE\"; echo \"cache $c\"; \
             ls \"$c\" | grep kept- || true; touch \"$c/live-$YULIUS_CI_RUN\" \
             \"{dir}/here-$YULIUS_CI_RUN\"; while [ ! -e \"{dir}/solo\" ] && [ \"$(ls \"{dir}\" | \
             grep -c ^here-)\" -lt 2 ]; do sleep 0.05; done; if [ \"$(ls \"$c\" | grep -c \
             ^live-)\" -gt 1 ]; then echo shared; exit 7; fi; rm \"$c/live-$YULIUS_CI_RUN\"; \
             touch \"$c/kept-$YULIUS_CI_RUN\"'\n",
            dir = data.display()
        )
    })
    .await?;
    let caches = f.data.join("sandboxes/s1/ci/cache");
    // What the cache every job shared held may be a file named like a job's cache (review
    // round 1): the job still gets a directory.
    std::fs::create_dir_all(&caches)?;
    std::fs::write(caches.join("build"), "left by the shared cache")?;
    f.h.app.ci_sync_sandbox(&f.sandbox).await?;
    f.settle().await?;
    let overview = f.get("/api/sandboxes/s1/ci").await?;
    let runs = overview["runs"].as_array().ok_or("runs")?;
    assert_eq!(runs.len(), 2, "main/dev and the pull request: {overview}");
    let mut details = Vec::new();
    for run in runs {
        let id = run["id"].as_str().ok_or("id")?;
        details.push(f.get(&format!("/api/sandboxes/s1/ci/runs/{id}")).await?);
    }
    let mut seen = Vec::new();
    for detail in &details {
        let output = detail["jobs"][0]["output"].as_str().unwrap_or_default();
        assert!(
            !output.contains("shared"),
            "one cache for two lanes: {output}"
        );
        assert_eq!(detail["state"], "passed", "{details:?}");
        seen.extend(
            output
                .lines()
                .filter_map(|line| line.strip_prefix("cache "))
                .map(str::to_owned),
        );
    }
    seen.sort();
    let expected: Vec<String> = ["build", "build@1"]
        .iter()
        .map(|name| caches.join(name).display().to_string())
        .collect();
    assert_eq!(seen, expected, "each lane's checkout has its own cache");

    std::fs::write(f.data.join("solo"), "")?;
    let later = f.push_feature("feature.txt", Some("two\n"))?;
    f.h.app.ci_sync_sandbox(&f.sandbox).await?;
    f.settle().await?;
    let overview = f.get("/api/sandboxes/s1/ci").await?;
    let latest = overview["runs"]
        .as_array()
        .ok_or("runs")?
        .iter()
        .find(|r| r["sha"] == later.as_str())
        .cloned()
        .ok_or_else(|| format!("no later pull request run: {overview}"))?;
    let detail = f
        .get(&format!(
            "/api/sandboxes/s1/ci/runs/{}",
            latest["id"].as_str().ok_or("id")?
        ))
        .await?;
    let output = detail["jobs"][0]["output"].as_str().unwrap_or_default();
    assert_eq!(latest["state"], "passed", "{detail}");
    assert!(
        output.contains(&format!("cache {}", caches.join("build").display())),
        "{output}"
    );
    assert!(
        output.contains("kept-"),
        "the lane's cache persists: {output}"
    );
    Ok(())
}

/// SPEC-351 review round 2: a link an agent put at `ci/cache` never steers what Yulius removes or
/// makes there. Before, a file named like a job's cache was removed through the link, outside the
/// sandbox; now the link is replaced by a folder and the outside file stays.
#[tokio::test]
async fn a_linked_ci_cache_folder_never_reaches_outside() -> TestResult {
    let f = fixture_with(false, |_| {
        "[[ci.jobs]]\nname = \"build\"\ncpus = 1\ncommand = 'touch \"$YULIUS_CI_CACHE/made\"'\n"
            .to_owned()
    })
    .await?;
    let outside = f.data.join("outside");
    std::fs::create_dir_all(&outside)?;
    for name in ["build", "build@1"] {
        std::fs::write(outside.join(name), "not the sandbox's")?;
    }
    let ci = f.data.join("sandboxes/s1/ci");
    std::fs::create_dir_all(&ci)?;
    std::os::unix::fs::symlink(&outside, ci.join("cache"))?;
    f.h.app.ci_sync_sandbox(&f.sandbox).await?;
    f.settle().await?;
    f.all_passed(2).await?;
    for name in ["build", "build@1"] {
        assert!(outside.join(name).is_file(), "{name} outside the sandbox");
    }
    let cache = std::fs::symlink_metadata(ci.join("cache"))?;
    assert!(cache.is_dir(), "the link was replaced by a folder");
    assert!(ci.join("cache/build/made").exists());
    Ok(())
}

/// The names in `dir`, sorted.
fn names_in(dir: &Path) -> BoxResult<Vec<String>> {
    let mut names = std::fs::read_dir(dir)?
        .map(|entry| Ok(entry?.file_name().to_string_lossy().into_owned()))
        .collect::<BoxResult<Vec<_>>>()?;
    names.sort();
    Ok(names)
}

/// #969 (SPEC-367 CJP-001): a link an agent put at `ci` fails the jobs before Yulius makes
/// anything: before, `<target>/home` (and the checkout) were made at the link's target.
#[tokio::test]
async fn a_linked_ci_folder_fails_the_job_and_makes_nothing_outside() -> TestResult {
    let f = fixture_with(false, |_| {
        "[[ci.jobs]]\nname = \"build\"\ncommand = 'echo ran'\n".to_owned()
    })
    .await?;
    let outside = f.data.join("outside");
    std::fs::create_dir_all(&outside)?;
    std::os::unix::fs::symlink(&outside, f.data.join("sandboxes/s1/ci"))?;
    f.h.app.ci_sync_sandbox(&f.sandbox).await?;
    f.settle().await?;
    let jobs = all_jobs(&f).await?;
    assert!(!jobs.is_empty(), "no job was queued");
    assert!(jobs.iter().all(|job| job["state"] != "passed"), "{jobs:?}");
    assert_eq!(names_in(&outside)?, Vec::<String>::new(), "made outside");
    Ok(())
}

/// #969 (SPEC-367 CJP-002): a link at `ci/home` is replaced by a folder of the sandbox, never the
/// job's `HOME`: before, the job wrote into the link's target.
#[tokio::test]
async fn a_linked_ci_home_is_replaced_by_a_folder() -> TestResult {
    let f = fixture_with(false, |_| {
        "[[ci.jobs]]\nname = \"build\"\ncommand = 'touch \"$HOME/made\"'\n".to_owned()
    })
    .await?;
    let outside = f.data.join("outside");
    std::fs::create_dir_all(&outside)?;
    std::fs::write(outside.join("keep"), "host\n")?;
    let ci = f.data.join("sandboxes/s1/ci");
    std::fs::create_dir_all(&ci)?;
    std::os::unix::fs::symlink(&outside, ci.join("home"))?;
    f.h.app.ci_sync_sandbox(&f.sandbox).await?;
    f.settle().await?;
    f.all_passed(2).await?;
    assert_eq!(
        names_in(&outside)?,
        ["keep"],
        "the target is left as it was"
    );
    assert!(std::fs::symlink_metadata(ci.join("home"))?.is_dir());
    assert!(
        ci.join("home/made").exists(),
        "HOME is the sandbox's folder"
    );
    Ok(())
}

/// #969 (SPEC-367 CJP-002, CJP-003): a link at `ci/jobs` to a folder holding the checkouts' names
/// is replaced by a folder: before, Git added the checkouts at the target, and the next run
/// removed `<target>/<checkout>` through the link as a broken checkout.
#[tokio::test]
async fn a_linked_ci_jobs_folder_is_never_checked_out_or_removed_through() -> TestResult {
    let f = fixture_with(false, |_| {
        "[[ci.jobs]]\nname = \"build\"\ncommand = 'echo ran'\n".to_owned()
    })
    .await?;
    let outside = f.data.join("outside");
    for name in ["build", "build@1"] {
        std::fs::create_dir_all(outside.join(name))?;
        std::fs::write(outside.join(name).join("keep"), "host\n")?;
    }
    let ci = f.data.join("sandboxes/s1/ci");
    std::fs::create_dir_all(&ci)?;
    std::os::unix::fs::symlink(&outside, ci.join("jobs"))?;
    f.h.app.ci_sync_sandbox(&f.sandbox).await?;
    f.settle().await?;
    f.all_passed(2).await?;
    for name in ["build", "build@1"] {
        assert_eq!(names_in(&outside.join(name))?, ["keep"], "{name} outside");
    }
    assert!(std::fs::symlink_metadata(ci.join("jobs"))?.is_dir());
    assert!(ci.join("jobs/build/README.md").exists());
    let registered = git(
        &f.data.join("sandboxes/s1/repo"),
        &["worktree", "list", "--porcelain"],
    )?;
    assert!(
        !registered.contains(&*outside.to_string_lossy()),
        "{registered}"
    );
    Ok(())
}

/// A job that names its stages, a figure and prints a cargo test summary (SPEC-137 STG-203).
/// Each step holds until the test creates its gate, `<data>/<next step>-<run id>`: the unit stage,
/// the end of the stages and the end of the job.
fn staged_toml(data: &Path) -> String {
    format!(
        "[[ci.jobs]]\nname = \"build\"\ncommand = 'gate() {{ while [ ! -e \"{dir}/$1-$YULIUS_CI_RUN\" \
         ]; do sleep 0.05; done; }}; echo \"::yulius-stage compile\"; echo \"::yulius-measure \
         cache_hits=2\"; gate unit; echo \"::yulius-stage unit\"; echo \"test result: ok. 3 \
         passed; 0 failed; 1 ignored; 0 measured\"; echo \"test result: ok. 2 passed; 0 failed; 0 \
         ignored\"; gate end; echo \"::yulius-stage-end\"; gate done; echo done'\n",
        dir = data.display()
    )
}

/// The stage `name` of the run `run`, once Yulius has recorded its start (or its end, `ended`).
async fn stage_recorded(
    f: &Fixture,
    run: &str,
    name: &str,
    ended: bool,
) -> BoxResult<yulius_app::stages::SpanRecord> {
    use yulius_app::ports::store_ext::SpanStore as _;
    use yulius_domain::stages::SubjectKind;
    let sandbox = SandboxId::parse("s1")?;
    let deadline = tokio::time::Instant::now() + WAIT;
    loop {
        f.h.app.ci_dispatch().await;
        let spans =
            f.h.store
                .subject_spans(&sandbox, SubjectKind::CiRun, run, 1_000)
                .await?;
        if let Some(span) = spans
            .into_iter()
            .find(|s| s.kind == "stage" && s.name == name && (!ended || s.ended_at.is_some()))
        {
            return Ok(span);
        }
        if tokio::time::Instant::now() > deadline {
            return Err(format!("the stage {name} of {run} was never recorded").into());
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// SPEC-137 STG-201..203: a CI run's queue, preparation, its job with its slot wait and
/// checkout, and the stages and figures the job's output names are spans timed by Yulius's clock
/// as it reads the output. The test alone moves the clock, each time once Yulius has recorded a
/// stage's start (or the end of the stages) and before the job goes on, so a stage lasts exactly
/// what the test moved the clock by, however late Yulius reads the output (#538).
#[tokio::test]
async fn a_run_s_jobs_record_their_stages_and_test_counts() -> TestResult {
    use yulius_app::ports::store_ext::SpanStore as _;
    use yulius_domain::stages::SubjectKind;
    const STAGE_MS: i64 = 1_000;
    let f = fixture_with(false, staged_toml).await?;
    f.h.app.ci_sync_sandbox(&f.sandbox).await?;
    let runs = f.get("/api/sandboxes/s1/ci").await?["runs"]
        .as_array()
        .ok_or("runs")?
        .clone();
    let id = runs
        .iter()
        .find(|r| r["reference"] == "pr/1")
        .and_then(|r| r["id"].as_str())
        .ok_or("pr/1 run")?
        .to_owned();
    let gate = |run: &str, next: &str| std::fs::write(f.data.join(format!("{next}-{run}")), "");
    for other in runs.iter().filter_map(|r| r["id"].as_str()) {
        if other != id {
            gate(other, "unit")?;
            gate(other, "end")?;
            gate(other, "done")?;
        }
    }
    let mut started = Vec::new();
    for (stage, next) in [("compile", "unit"), ("unit", "end")] {
        started.push(stage_recorded(&f, &id, stage, false).await?.started_at.0);
        f.h.clock.0.fetch_add(STAGE_MS, Ordering::SeqCst);
        gate(&id, next)?;
    }
    // `::yulius-stage-end` ends the stages before the job ends.
    stage_recorded(&f, &id, "unit", true).await?;
    f.h.clock.0.fetch_add(STAGE_MS, Ordering::SeqCst);
    gate(&id, "done")?;
    assert_eq!(
        started[1] - started[0],
        STAGE_MS,
        "unit starts when it is read"
    );
    f.settle().await?;
    let overview = f.get("/api/sandboxes/s1/ci").await?;
    let run = overview["runs"]
        .as_array()
        .ok_or("runs")?
        .iter()
        .find(|r| r["id"] == id.as_str())
        .cloned()
        .ok_or("pr/1 run")?;
    assert_eq!(run["state"], "passed", "{run}");
    let id = id.as_str();
    let spans =
        f.h.store
            .subject_spans(&SandboxId::parse("s1")?, SubjectKind::CiRun, id, 1_000)
            .await?;
    let find = |kind: &str, name: &str| {
        spans
            .iter()
            .find(|s| s.kind == kind && s.name == name)
            .ok_or_else(|| format!("no {kind} {name} in {spans:?}"))
    };
    let job = find("job", "build")?;
    assert_eq!(
        (job.job.as_deref(), job.outcome.as_deref()),
        (Some("build"), Some("passed"))
    );
    for (kind, name) in [
        ("queue", "queue"),
        ("run", "run"),
        ("prepare", "commit"),
        ("wait", "job slot"),
    ] {
        let span = find(kind, name)?;
        assert!(span.ended_at.is_some(), "{span:?}");
    }
    let checkout = find("prepare", "checkout")?;
    assert_eq!(checkout.parent_key.as_deref(), Some(job.key.as_str()));
    let compile = find("stage", "compile")?;
    let unit = find("stage", "unit")?;
    assert_eq!(compile.parent_key.as_deref(), Some(job.key.as_str()));
    assert_eq!(
        (compile.outcome.as_deref(), unit.outcome.as_deref()),
        (Some("passed"), Some("passed"))
    );
    assert_eq!(
        compile.ended_at,
        Some(unit.started_at),
        "a stage ends where the next starts"
    );
    let (compiled, tested) = (
        compile.duration_ms().ok_or("compile ended")?,
        unit.duration_ms().ok_or("unit ended")?,
    );
    assert_eq!(
        (compiled, tested),
        (STAGE_MS, STAGE_MS),
        "each stage lasts the time that passed while it ran"
    );
    assert!(unit.ended_at < job.ended_at && compile.started_at >= job.started_at);
    let figures: Value = serde_json::from_str(compile.detail.as_deref().unwrap_or("{}"))?;
    assert_eq!(figures["cache_hits"], 2.0);
    let tests: Value = serde_json::from_str(unit.detail.as_deref().unwrap_or("{}"))?;
    assert_eq!(
        (
            tests["tests_passed"].as_f64(),
            tests["tests_skipped"].as_f64()
        ),
        (Some(5.0), Some(1.0))
    );
    assert!(
        tests.get("tests_failed").is_none(),
        "zero counts are not written: {tests}"
    );
    assert!(spans.iter().all(|s| s.source == "recorded"));
    Ok(())
}

const BUSY_TOML: &str = r#"[[ci.jobs]]
name = "busy"
command = 'i=0; while [ $i -lt 300000 ]; do i=$((i+1)); done; echo "::yulius-measure cpu_ms=1 memory_peak_mb=1 cache_hits=3"; echo done'
"#;

/// SPEC-154 USE-001..004 (#488): a CI job run through the process host records the CPU time and
/// peak memory its process tree used on its `job` span, as Yulius measured them when the host
/// reaped it; the job's own `::yulius-measure` cannot set them.
#[tokio::test]
async fn a_job_records_the_cpu_and_memory_it_used() -> TestResult {
    use yulius_app::ports::store_ext::SpanStore as _;
    use yulius_domain::stages::SubjectKind;
    let f = fixture_with(false, |_| BUSY_TOML.to_owned()).await?;
    f.h.app.ci_sync_sandbox(&f.sandbox).await?;
    f.settle().await?;
    let overview = f.get("/api/sandboxes/s1/ci").await?;
    let run = overview["runs"]
        .as_array()
        .ok_or("runs")?
        .iter()
        .find(|r| r["reference"] == "pr/1")
        .cloned()
        .ok_or("pr/1 run")?;
    assert_eq!(run["state"], "passed", "{run}");
    let spans =
        f.h.store
            .subject_spans(
                &SandboxId::parse("s1")?,
                SubjectKind::CiRun,
                run["id"].as_str().ok_or("id")?,
                1_000,
            )
            .await?;
    let job = spans
        .iter()
        .find(|s| s.kind == "job" && s.name == "busy")
        .ok_or_else(|| format!("no job span in {spans:?}"))?;
    let figures: Value = serde_json::from_str(job.detail.as_deref().unwrap_or("{}"))?;
    let cpu = figures["cpu_ms"]
        .as_i64()
        .ok_or(format!("no cpu_ms: {figures}"))?;
    let memory = figures["memory_peak_mb"]
        .as_i64()
        .ok_or(format!("no memory_peak_mb: {figures}"))?;
    // The busy loop takes far more than a millisecond of CPU; the job's own "1" is refused.
    assert!(cpu >= 50, "CPU time of the busy loop: {figures}");
    assert!(memory >= 1, "a shell's peak memory: {figures}");
    assert_eq!(figures["cache_hits"], 3.0, "{figures}");
    Ok(())
}

const RUNAWAY_TOML: &str = r#"[[ci.jobs]]
name = "runaway"
command = '/bin/sh -c "while :; do :; done"'
"#;

/// SPEC-154 USE-002, review round 1: a job Yulius kills (here at its timeout) loses the processes
/// its host cannot reap (they go to init with the group kill), so what it measured would read as
/// almost nothing: its use stays unknown, never a near-zero figure.
#[tokio::test]
async fn a_killed_job_leaves_its_use_unknown() -> TestResult {
    use yulius_app::ports::store_ext::SpanStore as _;
    use yulius_domain::stages::SubjectKind;
    let f = fixture_with(false, |_| RUNAWAY_TOML.to_owned()).await?;
    f.h.app.ci_sync_sandbox(&f.sandbox).await?;
    f.settle().await?;
    let overview = f.get("/api/sandboxes/s1/ci").await?;
    let run = overview["runs"]
        .as_array()
        .ok_or("runs")?
        .iter()
        .find(|r| r["reference"] == "pr/1")
        .cloned()
        .ok_or("pr/1 run")?;
    assert_eq!(run["state"], "error", "timed out: {run}");
    let spans =
        f.h.store
            .subject_spans(
                &SandboxId::parse("s1")?,
                SubjectKind::CiRun,
                run["id"].as_str().ok_or("id")?,
                1_000,
            )
            .await?;
    let job = spans
        .iter()
        .find(|s| s.kind == "job" && s.name == "runaway")
        .ok_or_else(|| format!("no job span in {spans:?}"))?;
    let figures: Value = serde_json::from_str(job.detail.as_deref().unwrap_or("{}"))?;
    for figure in ["cpu_ms", "memory_peak_mb", "disk_written_mb"] {
        assert!(figures.get(figure).is_none(), "unknown {figure}: {figures}");
    }
    Ok(())
}

/// The output of each job of the run of commit `sha`, by job name.
async fn job_outputs(f: &Fixture, sha: &str) -> BoxResult<Vec<(String, String)>> {
    let overview = f.get("/api/sandboxes/s1/ci").await?;
    let runs = overview["runs"].as_array().ok_or("runs")?;
    let run = runs
        .iter()
        .find(|r| r["sha"] == sha)
        .ok_or_else(|| format!("no run of {sha}: {overview}"))?;
    let detail = f
        .get(&format!(
            "/api/sandboxes/s1/ci/runs/{}",
            run["id"].as_str().ok_or("id")?
        ))
        .await?;
    Ok(detail["jobs"]
        .as_array()
        .ok_or("jobs")?
        .iter()
        .map(|j| {
            (
                j["name"].as_str().unwrap_or_default().to_owned(),
                j["output"].as_str().unwrap_or_default().to_owned(),
            )
        })
        .collect())
}

fn output_of<'a>(outputs: &'a [(String, String)], name: &str) -> BoxResult<&'a str> {
    outputs
        .iter()
        .find(|(n, _)| n == name)
        .map(|(_, o)| o.as_str())
        .ok_or_else(|| format!("no job {name}: {outputs:?}").into())
}

/// SPEC-142 CSE-002..004: a job the integration branch grants `API_KEY` gets its placeholder, a
/// proxy credential of its own (a `ci:<run>` client while it runs, nothing after) and the CA
/// bundle; a job without a grant gets none of these; a pull request that grants secrets in its own
/// `yulius.toml` gets nothing more than the integration branch granted.
#[tokio::test]
async fn ci_jobs_get_the_secrets_the_integration_branch_grants_them() -> TestResult {
    let report = |extra: &str| {
        format!(
            "echo \"{extra}key=${{API_KEY:-none}} other=${{OTHER_KEY:-none}} \
             proxy=${{HTTPS_PROXY:+set}} ca=${{SSL_CERT_FILE:-none}}\"; \
             if [ -n \"$SSL_CERT_FILE\" ] && [ -r \"$SSL_CERT_FILE\" ]; then echo ca-readable; fi"
        )
    };
    let plain = report("");
    // The integration branch's command of the granted job.
    let trusted = std::sync::OnceLock::new();
    let f = fixture_with(false, |data| {
        let command = trusted.get_or_init(|| {
            format!(
                "{granted}; printf %s \"$HTTPS_PROXY\" > \"{dir}/proxy-$YULIUS_CI_RUN\"; \
                 while [ ! -e \"{dir}/go\" ]; do sleep 0.05; done",
                granted = report(""),
                dir = data.display()
            )
        });
        format!(
            "[[ci.jobs]]\nname = \"granted\"\nsecrets = \"API_KEY\"\ncommand = '{command}'\n\n\
             [[ci.jobs]]\nname = \"plain\"\ncommand = '{plain}'\n"
        )
    })
    .await?;
    for name in ["API_KEY", "OTHER_KEY"] {
        let created = f
            .post(
                "/api/sandboxes/s1/agent-secrets",
                Some(serde_json::json!({
                    "name": name,
                    "hosts": [format!("{}.test", name.to_ascii_lowercase().replace('_', "-"))],
                    "injection": "bearer",
                    "value": format!("{name}-value-0123456789"),
                    "auto_allow": false,
                })),
            )
            .await?;
        assert_eq!(created.status, StatusCode::CREATED, "{}", created.body);
    }
    f.h.app.enable_egress(Some(
        "-----BEGIN CERTIFICATE-----\ntest\n-----END CERTIFICATE-----\n".into(),
    ));
    f.h.app.ci_sync_sandbox(&f.sandbox).await?;

    // While the granted job runs, its credential names a CI client of its run.
    let deadline = tokio::time::Instant::now() + WAIT;
    let (run, url) = loop {
        f.h.app.ci_dispatch().await;
        let written = std::fs::read_dir(&f.data)?
            .filter_map(Result::ok)
            .find_map(|e| {
                let name = e.file_name().to_string_lossy().into_owned();
                let run = name.strip_prefix("proxy-")?.to_owned();
                let url = std::fs::read_to_string(e.path()).ok()?;
                (!url.is_empty()).then_some((run, url))
            });
        if let Some(found) = written {
            break found;
        }
        if tokio::time::Instant::now() > deadline {
            return Err("the granted job never wrote its proxy".into());
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    };
    let token = url
        .strip_prefix("http://yulius-ci:")
        .and_then(|rest| rest.split_once('@'))
        .map(|(token, authority)| {
            assert_eq!(authority, "127.0.0.1:1");
            token.to_owned()
        })
        .ok_or_else(|| format!("unexpected proxy {url}"))?;
    let policy: Arc<dyn yulius_app::ports::egress::EgressPolicy> = f.h.app.clone();
    let client = policy
        .egress_client(&token)
        .await
        .ok_or("the job's credential names no client")?;
    assert_eq!(client.actor(), format!("ci:{run}"));
    assert!(policy.egress_intercepts(&client, "api-key.test", 443).await);
    assert!(
        !policy
            .egress_intercepts(&client, "other-key.test", 443)
            .await
    );
    std::fs::write(f.data.join("go"), "")?;
    f.settle().await?;
    assert!(
        policy.egress_client(&token).await.is_none(),
        "the credential ends with the job"
    );

    let pr_sha = git(&f.origin, &["rev-parse", "feat/one"])?;
    let outputs = job_outputs(&f, &pr_sha).await?;
    let granted = output_of(&outputs, "granted")?;
    assert!(
        granted.contains("key=yulius-secret-API_KEY other=none proxy=set ca=/"),
        "{granted}"
    );
    assert!(granted.contains("ca-readable"), "{granted}");
    assert!(
        granted.contains("[yulius] agent secrets: API_KEY"),
        "{granted}"
    );
    assert!(!granted.contains("API_KEY-value"), "{granted}");
    let plain_out = output_of(&outputs, "plain")?;
    assert!(
        plain_out.contains("key=none other=none proxy= ca=none"),
        "{plain_out}"
    );
    assert!(!plain_out.contains("ca-readable"), "{plain_out}");

    // The pull request grants secrets in its own yulius.toml, reuses the granted job's name for
    // its own command and adds a job (SPEC-264 CSG-001, CSG-002, #659): the integration branch's
    // jobs judge it, with their grant (SPEC-504 CFG-001), and its own run beside them with no
    // secret, since a grant binds the integration branch's name and command.
    let own = format!(
        "[[ci.jobs]]\nname = \"granted\"\nsecrets = \"API_KEY OTHER_KEY\"\ncommand = '{granted}'\n\n\
         [[ci.jobs]]\nname = \"plain\"\nsecrets = \"API_KEY\"\ncommand = '{plain}'\n\n\
         [[ci.jobs]]\nname = \"sneaky\"\nsecrets = \"OTHER_KEY\"\ncommand = '{sneaky}'\n",
        granted = report("own "),
        plain = report("own "),
        sneaky = report("own "),
    );
    let own_sha = f.push_feature("yulius.toml", Some(&own))?;
    f.h.app.ci_sync_sandbox(&f.sandbox).await?;
    std::fs::write(f.data.join("go"), "")?;
    f.settle().await?;
    let outputs = job_outputs(&f, &own_sha).await?;
    let names: Vec<&str> = outputs.iter().map(|(n, _)| n.as_str()).collect();
    assert_eq!(
        names,
        [
            "granted",
            "plain",
            "granted (this change)",
            "plain (this change)",
            "sneaky (this change)"
        ],
        "the integration branch's jobs, and its own beside them"
    );
    for name in [
        "granted (this change)",
        "plain (this change)",
        "sneaky (this change)",
    ] {
        assert!(
            output_of(&outputs, name)?.contains("own key=none other=none proxy= ca=none"),
            "{outputs:?}"
        );
    }
    assert!(
        output_of(&outputs, "granted (this change)")?.contains(
            "[yulius] agent secrets: none; this job's command is not the one the integration \
             branch grants them to"
        ),
        "{outputs:?}"
    );
    let granted = output_of(&outputs, "granted")?;
    assert!(
        granted.contains("key=yulius-secret-API_KEY other=none proxy=set ca=/"),
        "the integration branch's job keeps its grant: {granted}"
    );
    Ok(())
}

/// SPEC-195 VSC-001, VSC-011: every project's CI job runs Yulius's embedded scanner as
/// `yulius security-scan --ci`; without a Vidoc key it says why and ends skipped, never passed.
#[tokio::test]
async fn a_ci_job_runs_the_embedded_security_scan() -> TestResult {
    let f = fixture_with(false, |_| {
        "[[ci.jobs]]\nname = \"security\"\nsecrets = \"VIDOC_API_KEY\"\ncommand = \"yulius security-scan --ci\"\n".to_owned()
    })
    .await?;
    let pr_sha = git(&f.origin, &["rev-parse", "feat/one"])?;
    f.h.app.ci_sync_sandbox(&f.sandbox).await?;
    f.settle().await?;
    let outputs = job_outputs(&f, &pr_sha).await?;
    let security = output_of(&outputs, "security")?;
    assert!(
        security.contains("Security scan not run: No Vidoc key"),
        "{security}"
    );
    assert!(
        security.contains("[yulius] security scan skipped"),
        "{security}"
    );
    let overview = f.get("/api/sandboxes/s1/ci").await?;
    let run = overview["runs"]
        .as_array()
        .and_then(|runs| runs.iter().find(|r| r["sha"] == pr_sha.as_str()))
        .ok_or("run")?;
    let detail = f
        .get(&format!(
            "/api/sandboxes/s1/ci/runs/{}",
            run["id"].as_str().ok_or("id")?
        ))
        .await?;
    assert_eq!(detail["jobs"][0]["state"], "skipped", "{detail}");
    Ok(())
}

/// One `::yulius-finding` line, as the security scan prints it (SPEC-217 VSP-004).
fn finding_line(severity: &str, file: &str, fingerprint: &str) -> String {
    format!(
        "::yulius-finding {}\n",
        serde_json::json!({"severity": severity, "file": file, "line": 4,
            "title": "Weak check", "detail": "Fix it.", "fingerprint": fingerprint,
            "anchored": true})
    )
}

/// SPEC-217 VSI-004 (review round 1, M2): a filed finding's issue follows the integration branch.
/// Closed while Vidoc still reports the finding and its code is on dev, it is reopened; once its
/// line is gone from dev, it is closed, also when a branch behind dev still reports it, and that
/// stale report reopens nothing; a finding already gone from dev is never filed.
#[tokio::test]
async fn filed_issues_are_reopened_while_reported_and_closed_once_the_code_is_gone() -> TestResult {
    let f = fixture_with(false, |_| {
        "[[ci.jobs]]\nname = \"security\"\ncommand = \"cat findings.txt\"\n".to_owned()
    })
    .await?;
    f.push_dev("lib.rs", Some("fn danger() {}\n"))?;
    // README.md's `# app` and lib.rs's `fn danger() {}` are on dev; src/never.rs is not.
    let (readme, lib, never) = ("8464a81d5a585dd2", "c6b08d3eced1a57c", "0000000000000001");
    let all = finding_line("medium", "README.md", readme)
        + &finding_line("low", "lib.rs", lib)
        + &finding_line("low", "src/never.rs", never);
    let first = f.push_feature("findings.txt", Some(&all))?;
    f.h.app.ci_sync_sandbox(&f.sandbox).await?;
    f.settle().await?;
    let titles = |f: &Fixture| -> BoxResult<Vec<String>> {
        Ok(f.h
            .github
            .issues
            .lock()
            .map_err(|_| "issues")?
            .iter()
            .map(|(_, issue)| issue.title.clone())
            .collect())
    };
    assert_eq!(
        titles(&f)?,
        [
            "security: Weak check in README.md",
            "security: Weak check in lib.rs"
        ],
        "src/never.rs is not on dev: not filed"
    );
    let outputs = job_outputs(&f, &first).await?;
    let security = output_of(&outputs, "security")?;
    assert!(
        security.contains("1 finding(s) outside the change not followed"),
        "{security}"
    );
    // A person closes #101 without fixing it; dev drops lib.rs; the branch, behind dev, still
    // reports both.
    f.h.github
        .closed_issues
        .lock()
        .map_err(|_| "closed")?
        .insert(101, vec!["won't fix".into()]);
    f.push_dev("lib.rs", None)?;
    let second = f.push_feature("findings.txt", Some(&(all.clone() + "\n")))?;
    f.h.app.ci_sync_sandbox(&f.sandbox).await?;
    f.settle().await?;
    let outputs = job_outputs(&f, &second).await?;
    let security = output_of(&outputs, "security")?;
    assert!(
        security.contains("reopened issue #101: finding 8464a81d5a585dd2 is reported again"),
        "{security}"
    );
    let closed: Vec<u64> =
        f.h.github
            .closed_issues
            .lock()
            .map_err(|_| "closed")?
            .keys()
            .copied()
            .collect();
    assert_eq!(
        closed,
        [102],
        "#101 reopened, #102 closed (by this run or dev's)"
    );
    // A head that reports nothing: README.md's line is still on dev, so #101 stays open.
    f.push_feature("findings.txt", Some("nothing\n"))?;
    f.h.app.ci_sync_sandbox(&f.sandbox).await?;
    f.settle().await?;
    assert!(
        !f.h.github
            .closed_issues
            .lock()
            .map_err(|_| "closed")?
            .contains_key(&101)
    );
    assert_eq!(titles(&f)?.len(), 2, "nothing refiled");
    assert_eq!(
        f.h.github
            .issue_comments
            .lock()
            .map_err(|_| "comments")?
            .len(),
        2,
        "one reopening, one closing"
    );
    Ok(())
}

/// SPEC-217 VSI-001 (review round 2): failed filings count against the 5 a job may try, and the
/// rest wait for a later run.
#[tokio::test]
async fn a_job_tries_at_most_five_filings_failed_ones_included() -> TestResult {
    let f = fixture_with(false, |_| {
        "[[ci.jobs]]\nname = \"security\"\ncommand = \"cat findings.txt\"\n".to_owned()
    })
    .await?;
    f.push_dev("many.rs", Some("let v1 = 1;\nlet v2 = 2;\nlet v3 = 3;\nlet v4 = 4;\nlet v5 = 5;\nlet v6 = 6;\nlet v7 = 7;\n"))?;
    f.h.github
        .fail_issues
        .store(true, std::sync::atomic::Ordering::SeqCst);
    let lines = String::new()
        + &finding_line("low", "many.rs", "4e318af8a518ee12")
        + &finding_line("low", "many.rs", "d746048c1cbdf9f8")
        + &finding_line("low", "many.rs", "6bd523ab16569c0e")
        + &finding_line("low", "many.rs", "73ba8fc484973a5d")
        + &finding_line("low", "many.rs", "5cb9e593d94b466c")
        + &finding_line("low", "many.rs", "1e75da6953b58fd8")
        + &finding_line("low", "many.rs", "7930599a9fbbd0c5");
    let sha = f.push_feature("findings.txt", Some(&lines))?;
    f.h.app.ci_sync_sandbox(&f.sandbox).await?;
    f.settle().await?;
    let outputs = job_outputs(&f, &sha).await?;
    let security = output_of(&outputs, "security")?;
    assert_eq!(security.matches("not filed: ").count(), 5, "{security}");
    assert!(
        security.contains("2 more finding(s) outside the change wait for a later run"),
        "{security}"
    );
    Ok(())
}

/// SPEC-217 VSI-004 (review round 2): an issue GitHub no longer has (404) resolves its finding,
/// so later jobs make no call for it.
#[tokio::test]
async fn a_finding_whose_issue_is_gone_from_github_is_resolved() -> TestResult {
    let f = fixture_with(false, |_| {
        "[[ci.jobs]]\nname = \"security\"\ncommand = \"cat findings.txt\"\n".to_owned()
    })
    .await?;
    f.push_dev("lib.rs", Some("fn danger() {}\n"))?;
    let lib = finding_line("low", "lib.rs", "c6b08d3eced1a57c");
    f.push_feature("findings.txt", Some(&lib))?;
    f.h.app.ci_sync_sandbox(&f.sandbox).await?;
    f.settle().await?;
    assert_eq!(f.h.github.issues.lock().map_err(|_| "issues")?.len(), 1);
    f.h.github
        .missing_issues
        .lock()
        .map_err(|_| "missing")?
        .push(101);
    f.push_dev("lib.rs", None)?;
    let sha = f.push_feature("findings.txt", Some("nothing\n"))?;
    f.h.app.ci_sync_sandbox(&f.sandbox).await?;
    f.settle().await?;
    let outputs = job_outputs(&f, &sha).await?;
    let security = output_of(&outputs, "security")?;
    assert!(!security.contains("not closed"), "{security}");
    let calls = || {
        f.h.github
            .issue_state_calls
            .load(std::sync::atomic::Ordering::SeqCst)
    };
    let before = calls();
    assert!(before >= 1, "the close was tried");
    f.push_feature("findings.txt", Some("still nothing\n"))?;
    f.h.app.ci_sync_sandbox(&f.sandbox).await?;
    f.settle().await?;
    assert_eq!(calls(), before, "a resolved finding is not looked at again");
    Ok(())
}

/// SPEC-217 VSI-004 (review round 3): a finding filed before is looked at for reopening even once
/// the job's filings are used up.
#[tokio::test]
async fn a_closed_finding_is_reopened_past_the_filing_cap() -> TestResult {
    let f = fixture_with(false, |_| {
        "[[ci.jobs]]\nname = \"security\"\ncommand = \"cat findings.txt\"\n".to_owned()
    })
    .await?;
    f.push_dev("lib.rs", Some("fn danger() {}\n"))?;
    f.push_dev(
        "many.rs",
        Some("let v1 = 1;\nlet v2 = 2;\nlet v3 = 3;\nlet v4 = 4;\nlet v5 = 5;\n"),
    )?;
    let lib = finding_line("low", "lib.rs", "c6b08d3eced1a57c");
    f.push_feature("findings.txt", Some(&lib))?;
    f.h.app.ci_sync_sandbox(&f.sandbox).await?;
    f.settle().await?;
    f.h.github
        .closed_issues
        .lock()
        .map_err(|_| "closed")?
        .insert(101, vec!["won't fix".into()]);
    f.h.github
        .fail_issues
        .store(true, std::sync::atomic::Ordering::SeqCst);
    let lines = String::new()
        + &finding_line("low", "many.rs", "4e318af8a518ee12")
        + &finding_line("low", "many.rs", "d746048c1cbdf9f8")
        + &finding_line("low", "many.rs", "6bd523ab16569c0e")
        + &finding_line("low", "many.rs", "73ba8fc484973a5d")
        + &finding_line("low", "many.rs", "5cb9e593d94b466c")
        + &lib;
    let sha = f.push_feature("findings.txt", Some(&lines))?;
    f.h.app.ci_sync_sandbox(&f.sandbox).await?;
    f.settle().await?;
    let outputs = job_outputs(&f, &sha).await?;
    let security = output_of(&outputs, "security")?;
    assert_eq!(security.matches("not filed: ").count(), 5, "{security}");
    assert!(
        security.contains("reopened issue #101: finding c6b08d3eced1a57c is reported again"),
        "{security}"
    );
    assert!(!security.contains("wait for a later run"), "{security}");
    Ok(())
}

/// SPEC-217 VSI-004 (review round 3): a finding whose issue is gone from GitHub (deleted) and that
/// is reported again is filed anew.
#[tokio::test]
async fn a_reported_finding_whose_issue_was_deleted_is_filed_again() -> TestResult {
    let f = fixture_with(false, |_| {
        "[[ci.jobs]]\nname = \"security\"\ncommand = \"cat findings.txt\"\n".to_owned()
    })
    .await?;
    f.push_dev("lib.rs", Some("fn danger() {}\n"))?;
    let lib = finding_line("low", "lib.rs", "c6b08d3eced1a57c");
    f.push_feature("findings.txt", Some(&lib))?;
    f.h.app.ci_sync_sandbox(&f.sandbox).await?;
    f.settle().await?;
    f.h.github
        .missing_issues
        .lock()
        .map_err(|_| "missing")?
        .push(101);
    let sha = f.push_feature("findings.txt", Some(&(lib.clone() + "\n")))?;
    f.h.app.ci_sync_sandbox(&f.sandbox).await?;
    f.settle().await?;
    let outputs = job_outputs(&f, &sha).await?;
    let security = output_of(&outputs, "security")?;
    assert!(
        security.contains("filed issue #102 for the low finding c6b08d3eced1a57c"),
        "{security}"
    );
    assert_eq!(f.h.github.issues.lock().map_err(|_| "issues")?.len(), 2);
    Ok(())
}

/// SPEC-217 VSI-002 (review round 1, m8): when GitHub refuses an issue, the claim is released and
/// a later run files it.
#[tokio::test]
async fn a_finding_github_did_not_take_is_filed_by_a_later_run() -> TestResult {
    let f = fixture_with(false, |_| {
        "[[ci.jobs]]\nname = \"security\"\ncommand = \"cat findings.txt\"\n".to_owned()
    })
    .await?;
    f.h.github
        .fail_issues
        .store(true, std::sync::atomic::Ordering::SeqCst);
    let lines = finding_line("medium", "README.md", "8464a81d5a585dd2");
    let sha = f.push_feature("findings.txt", Some(&lines))?;
    f.h.app.ci_sync_sandbox(&f.sandbox).await?;
    f.settle().await?;
    let outputs = job_outputs(&f, &sha).await?;
    let security = output_of(&outputs, "security")?;
    assert!(
        security.contains("finding 8464a81d5a585dd2 not filed"),
        "{security}"
    );
    f.h.github
        .fail_issues
        .store(false, std::sync::atomic::Ordering::SeqCst);
    f.push_feature("findings.txt", Some(&(lines.clone() + "\n")))?;
    f.h.app.ci_sync_sandbox(&f.sandbox).await?;
    f.settle().await?;
    assert_eq!(f.h.github.issues.lock().map_err(|_| "issues")?.len(), 1);
    Ok(())
}

/// SPEC-217 VSI-001..003: the `security` job's findings outside the change become one issue per
/// fingerprint, with the Definition-of-Ready labels; a later run files nothing again, and
/// another job's lines file nothing.
#[tokio::test]
async fn the_security_jobs_findings_outside_the_change_are_filed_once() -> TestResult {
    let f = fixture_with(false, |_| {
        "[[ci.jobs]]\nname = \"security\"\ncommand = \"cat findings.txt\"\n\n\
         [[ci.jobs]]\nname = \"other\"\ncommand = \"cat other.txt\"\n"
            .to_owned()
    })
    .await?;
    let lines = finding_line("medium", "README.md", "8464a81d5a585dd2")
        + &finding_line("low", "yulius.toml", "f4778e0b243b2c7b")
        + &finding_line("high", "src/c.rs", "1111111111111111");
    f.push_feature("findings.txt", Some(&lines))?;
    let sha = f.push_feature(
        "other.txt",
        Some(&finding_line("low", "src/d.rs", "2222222222222222")),
    )?;
    f.h.app.ci_sync_sandbox(&f.sandbox).await?;
    f.settle().await?;
    let filed = |f: &Fixture| -> BoxResult<Vec<(String, Vec<String>)>> {
        Ok(f.h
            .github
            .issues
            .lock()
            .map_err(|_| "issues")?
            .iter()
            .map(|(_, issue)| (issue.title.clone(), issue.labels.clone()))
            .collect())
    };
    let outputs = job_outputs(&f, &sha).await?;
    let security = output_of(&outputs, "security")?;
    let issues = filed(&f)?;
    assert_eq!(
        issues.iter().map(|(t, _)| t.as_str()).collect::<Vec<_>>(),
        [
            "security: Weak check in README.md",
            "security: Weak check in yulius.toml"
        ],
        "one issue per valid finding of the security job only: {security}"
    );
    assert_eq!(
        issues[0].1,
        [
            "type: bug",
            "priority: medium",
            "area: security",
            "source: vidoc"
        ]
    );
    assert!(
        security.contains("[yulius] filed issue #101 for the medium finding 8464a81d5a585dd2"),
        "{security}"
    );
    // The next head reports the same findings: nothing is filed again.
    let next = f.push_feature("more.txt", Some("x\n"))?;
    f.h.app.ci_sync_sandbox(&f.sandbox).await?;
    f.settle().await?;
    assert_eq!(filed(&f)?.len(), 2, "filed once");
    let outputs = job_outputs(&f, &next).await?;
    let security = output_of(&outputs, "security")?;
    assert!(
        security.contains("2 finding(s) outside the change were filed before"),
        "{security}"
    );
    Ok(())
}

/// SPEC-273 NOI-008: findings outside the change wait for the tests. A run whose other job failed
/// files no issue and says why in the security job's output; once that job passes, the same
/// findings are filed.
#[tokio::test]
async fn findings_are_filed_only_from_a_run_whose_other_jobs_passed() -> TestResult {
    let f = fixture_with(false, |_| {
        "[[ci.jobs]]\nname = \"security\"\ncommand = \"cat findings.txt\"\n\n\
         [[ci.jobs]]\nname = \"quality\"\ncommand = \"if [ -f fail-quality ]; then exit 1; fi\"\n"
            .to_owned()
    })
    .await?;
    f.push_feature(
        "findings.txt",
        Some(&finding_line("medium", "README.md", "8464a81d5a585dd2")),
    )?;
    let failing = f.push_feature("fail-quality", Some("x\n"))?;
    f.h.app.ci_sync_sandbox(&f.sandbox).await?;
    f.settle().await?;
    let filed = |f: &Fixture| -> BoxResult<usize> {
        Ok(f.h.github.issues.lock().map_err(|_| "issues")?.len())
    };
    assert_eq!(filed(&f)?, 0, "nothing filed while quality fails");
    let outputs = job_outputs(&f, &failing).await?;
    let security = output_of(&outputs, "security")?;
    assert!(
        security.contains(
            "[yulius] 1 finding(s) outside the change not filed or reopened as issues yet: job \
             quality did not pass"
        ),
        "{security}"
    );
    let passing = f.push_feature("fail-quality", None)?;
    f.h.app.ci_sync_sandbox(&f.sandbox).await?;
    f.settle().await?;
    assert_eq!(filed(&f)?, 1, "filed once the tests pass");
    let outputs = job_outputs(&f, &passing).await?;
    let security = output_of(&outputs, "security")?;
    assert!(
        security.contains("[yulius] filed issue #101 for the medium finding 8464a81d5a585dd2"),
        "{security}"
    );
    Ok(())
}

/// SPEC-195 VSC-001 (review round 1): inside the containment too, a job finds `yulius` (bound
/// read-only) and runs the embedded scan.
#[tokio::test]
async fn a_contained_ci_job_runs_the_embedded_security_scan() -> TestResult {
    let probe = std::process::Command::new("bwrap")
        .args([
            "--die-with-parent",
            "--unshare-pid",
            "--ro-bind",
            "/",
            "/",
            "--proc",
            "/proc",
            "--",
            "/bin/true",
        ])
        .output();
    if !probe.is_ok_and(|out| out.status.success()) {
        eprintln!(
            "SKIPPED a_contained_ci_job_runs_the_embedded_security_scan: bubblewrap cannot run here"
        );
        return Ok(());
    }
    let f = fixture_with(true, |_| {
        "[[ci.jobs]]\nname = \"security\"\ncommand = \"yulius security-scan --ci\"\n".to_owned()
    })
    .await?;
    let pr_sha = git(&f.origin, &["rev-parse", "feat/one"])?;
    f.h.app.ci_sync_sandbox(&f.sandbox).await?;
    f.settle().await?;
    let outputs = job_outputs(&f, &pr_sha).await?;
    let security = output_of(&outputs, "security")?;
    assert!(
        security.contains("[yulius] security scan skipped"),
        "{security}"
    );
    Ok(())
}

/// A Vidoc connector whose tests always pass (the CI tests need only the key in use).
struct AnyVidoc;

#[async_trait::async_trait]
impl yulius_app::ports::vidoc::VidocConnector for AnyVidoc {
    async fn check(
        &self,
        _key: &yulius_app::model::Secret,
    ) -> yulius_app::ports::vidoc::VidocCheck {
        yulius_app::ports::vidoc::VidocCheck::Ok
    }
}

/// SPEC-195 VDK-005: a job granted `VIDOC_API_KEY` gets the instance key's placeholder when the
/// sandbox has no such agent secret of its own, its output says where the key comes from, and the
/// value never reaches it.
#[tokio::test]
async fn a_granted_job_gets_the_instance_vidoc_key_without_a_project_secret() -> TestResult {
    const INSTANCE_KEY: &str = "vidoc-instance-key-0123456789";
    let f = fixture_with(false, |_| {
        "[[ci.jobs]]\nname = \"security\"\nsecrets = \"VIDOC_API_KEY\"\ncommand = 'echo \"key=${VIDOC_API_KEY:-none} proxy=${HTTPS_PROXY:+set} base=${YULIUS_INTEGRATION_BRANCH:-none}\"'\n\n[[ci.jobs]]\nname = \"plain\"\ncommand = 'echo \"key=${VIDOC_API_KEY:-none}\"'\n".to_owned()
    })
    .await?;
    f.h.app.set_vidoc(yulius_app::vidoc_key::VidocServices::new(
        Arc::new(AnyVidoc),
        Ok(Some(yulius_app::model::Secret(INSTANCE_KEY.into()))),
    ));
    f.h.app.load_vidoc_key().await;
    f.h.app.enable_egress(Some(
        "-----BEGIN CERTIFICATE-----\ntest\n-----END CERTIFICATE-----\n".into(),
    ));
    let pr_sha = git(&f.origin, &["rev-parse", "feat/one"])?;
    f.h.app.ci_sync_sandbox(&f.sandbox).await?;
    f.settle().await?;
    let outputs = job_outputs(&f, &pr_sha).await?;
    let security = output_of(&outputs, "security")?;
    assert!(
        security.contains("key=yulius-secret-VIDOC_API_KEY proxy=set base=dev"),
        "{security}"
    );
    assert!(
        security.contains("[yulius] agent secrets: VIDOC_API_KEY (the instance's Vidoc key)"),
        "{security}"
    );
    assert!(!security.contains(INSTANCE_KEY), "{security}");
    let plain = output_of(&outputs, "plain")?;
    assert!(
        plain.contains("key=none"),
        "an ungranted job gets nothing: {plain}"
    );
    Ok(())
}

/// SPEC-217 VDP-001: with the instance's Vidoc key, a project whose CI does not scan gets the
/// built-in `security` job, granted the key; without a key it gets none.
#[tokio::test]
async fn every_project_with_a_vidoc_key_runs_the_built_in_security_job() -> TestResult {
    let f = fixture_with(false, |_| {
        "[[ci.jobs]]\nname = \"plain\"\ncommand = \"echo plain\"\n".to_owned()
    })
    .await?;
    let pr_sha = git(&f.origin, &["rev-parse", "feat/one"])?;
    f.h.app.ci_sync_sandbox(&f.sandbox).await?;
    f.settle().await?;
    let outputs = job_outputs(&f, &pr_sha).await?;
    assert_eq!(
        outputs.iter().map(|(n, _)| n.as_str()).collect::<Vec<_>>(),
        ["plain"],
        "no key, no scan"
    );
    f.h.app.set_vidoc(yulius_app::vidoc_key::VidocServices::new(
        Arc::new(AnyVidoc),
        Ok(Some(yulius_app::model::Secret(
            "vidoc-instance-key-0123456789".into(),
        ))),
    ));
    f.h.app.load_vidoc_key().await;
    f.h.app.enable_egress(Some(
        "-----BEGIN CERTIFICATE-----\ntest\n-----END CERTIFICATE-----\n".into(),
    ));
    let next = f.push_feature("notes.txt", Some("two\n"))?;
    f.h.app.ci_sync_sandbox(&f.sandbox).await?;
    f.settle().await?;
    let outputs = job_outputs(&f, &next).await?;
    let security = output_of(&outputs, "security")?;
    assert!(
        security.contains("[yulius] agent secrets: VIDOC_API_KEY (the instance's Vidoc key)"),
        "{security}"
    );
    // The change holds only prose: the scanner checks nothing and the job ends skipped.
    assert!(
        security.contains("[yulius] security scan skipped"),
        "{security}"
    );
    Ok(())
}

/// SPEC-356 VIO-002: with Vidoc switched off for the instance, a project whose own switch is on
/// gets no built-in security job, and its own scanning job, with its own key, ends skipped.
#[tokio::test]
async fn no_project_scans_in_ci_while_vidoc_is_off_for_the_instance() -> TestResult {
    let f = fixture_with(false, |_| {
        "[[ci.jobs]]\nname = \"plain\"\ncommand = \"echo plain\"\n".to_owned()
    })
    .await?;
    f.h.app.set_vidoc(yulius_app::vidoc_key::VidocServices::new(
        Arc::new(AnyVidoc),
        Ok(Some(yulius_app::model::Secret(
            "vidoc-instance-key-0123456789".into(),
        ))),
    ));
    {
        use yulius_app::ports::store_ext::UpdateStore as _;
        f.h.store
            .set_vidoc_off(
                Some("superadmin"),
                yulius_domain::time::Timestamp(1_800_000_000),
            )
            .await?;
    }
    f.h.app.load_vidoc_key().await;
    f.h.app.enable_egress(Some(
        "-----BEGIN CERTIFICATE-----\ntest\n-----END CERTIFICATE-----\n".into(),
    ));
    let created = f
        .post(
            "/api/sandboxes/s1/agent-secrets",
            Some(serde_json::json!({
                "name": "VIDOC_API_KEY",
                "hosts": ["model.vidocsecurity.com"],
                "value": "vidoc-own-key-0123456789",
            })),
        )
        .await?;
    assert_eq!(created.status, StatusCode::CREATED, "{}", created.body);
    let sha = f.push_feature("notes.txt", Some("two\n"))?;
    f.h.app.ci_sync_sandbox(&f.sandbox).await?;
    f.settle().await?;
    let outputs = job_outputs(&f, &sha).await?;
    assert_eq!(
        outputs.iter().map(|(n, _)| n.as_str()).collect::<Vec<_>>(),
        ["plain"],
        "instance off: no built-in scan"
    );
    // The integration branch's own scanning job (a pull request's own jobs never run, SPEC-504
    // CFG-001), at the pull request's next head (the scanner runs on pull requests only).
    f.push_dev(
        "yulius.toml",
        Some(
            "[[ci.jobs]]\nname = \"plain\"\ncommand = \"echo plain\"\n\n\
             [[ci.jobs]]\nname = \"security\"\nsecrets = \"VIDOC_API_KEY\"\ncommand = \"yulius security-scan --ci\"\n",
        ),
    )?;
    let own = f.push_feature("notes.txt", Some("three\n"))?;
    f.h.app.ci_sync_sandbox(&f.sandbox).await?;
    f.settle().await?;
    let outputs = job_outputs(&f, &own).await?;
    let security = output_of(&outputs, "security")?;
    assert!(security.contains("Security scan disabled"), "{security}");
    assert!(
        security.contains("[yulius] security scan skipped"),
        "{security}"
    );
    Ok(())
}

/// SPEC-338 VOF-002: a project whose Vidoc switch is off gets no built-in security job even with
/// the instance's key and its own, and its own scanning job says "disabled" and ends skipped, never passed.
#[tokio::test]
async fn a_switched_off_project_scans_nothing_in_ci() -> TestResult {
    let f = fixture_with(false, |_| {
        "[[ci.jobs]]\nname = \"plain\"\ncommand = \"echo plain\"\n".to_owned()
    })
    .await?;
    f.h.app.set_vidoc(yulius_app::vidoc_key::VidocServices::new(
        Arc::new(AnyVidoc),
        Ok(Some(yulius_app::model::Secret(
            "vidoc-instance-key-0123456789".into(),
        ))),
    ));
    f.h.app.load_vidoc_key().await;
    // The project's own key too: the switch, not the key, decides.
    let created = f
        .post(
            "/api/sandboxes/s1/agent-secrets",
            Some(serde_json::json!({
                "name": "VIDOC_API_KEY",
                "hosts": ["model.vidocsecurity.com"],
                "value": "vidoc-own-key-0123456789",
            })),
        )
        .await?;
    assert_eq!(created.status, StatusCode::CREATED, "{}", created.body);
    {
        use yulius_app::ports::store::SandboxStore as _;
        f.h.store
            .set_profile(&f.sandbox.id, r#"{"vidoc":false}"#)
            .await?;
    }
    let sandbox = {
        use yulius_app::ports::store::SandboxStore as _;
        f.h.store.sandbox(&f.sandbox.id).await?.ok_or("sandbox")?
    };
    let sha = f.push_feature("notes.txt", Some("two\n"))?;
    f.h.app.ci_sync_sandbox(&sandbox).await?;
    f.settle().await?;
    let outputs = job_outputs(&f, &sha).await?;
    assert_eq!(
        outputs.iter().map(|(n, _)| n.as_str()).collect::<Vec<_>>(),
        ["plain"],
        "switched off: no built-in scan"
    );

    // Its own job runs the scanner, which stops at once.
    // The integration branch's own scanning job (a pull request's own jobs never run, SPEC-504
    // CFG-001), at the pull request's next head (the scanner runs on pull requests only).
    f.push_dev(
        "yulius.toml",
        Some(
            "[[ci.jobs]]\nname = \"plain\"\ncommand = \"echo plain\"\n\n\
             [[ci.jobs]]\nname = \"security\"\ncommand = \"yulius security-scan --ci\"\n",
        ),
    )?;
    let own = f.push_feature("notes.txt", Some("three\n"))?;
    f.h.app.ci_sync_sandbox(&sandbox).await?;
    f.settle().await?;
    let outputs = job_outputs(&f, &own).await?;
    let security = output_of(&outputs, "security")?;
    assert!(
        security.contains("Vidoc is turned off in Yulius, for this project or the whole instance"),
        "{security}"
    );
    assert!(
        security.contains("[yulius] security scan skipped"),
        "{security}"
    );
    // Skipped, never passed (review round 1).
    let overview = f.get("/api/sandboxes/s1/ci").await?;
    let run = overview["runs"]
        .as_array()
        .and_then(|runs| runs.iter().find(|r| r["sha"] == own.as_str()))
        .ok_or("run")?;
    let detail = f
        .get(&format!(
            "/api/sandboxes/s1/ci/runs/{}",
            run["id"].as_str().ok_or("id")?
        ))
        .await?;
    let job = detail["jobs"]
        .as_array()
        .and_then(|jobs| jobs.iter().find(|j| j["name"] == "security"))
        .ok_or("security job")?;
    assert_eq!(job["state"], "skipped", "{detail}");
    Ok(())
}

/// SPEC-217 VDP-001 (the branch's own CI scan, medium): a pull request tested with its own jobs
/// (since SPEC-504 CFG-001 only one that brings CI to a branch without any) keeps the built-in
/// security job; its own job named `security` does not stand in for it.
#[tokio::test]
async fn a_pull_request_with_its_own_jobs_cannot_drop_the_security_job() -> TestResult {
    let f = fixture_with(false, |_| String::new()).await?;
    f.h.app.set_vidoc(yulius_app::vidoc_key::VidocServices::new(
        Arc::new(AnyVidoc),
        Ok(Some(yulius_app::model::Secret(
            "vidoc-instance-key-0123456789".into(),
        ))),
    ));
    f.h.app.load_vidoc_key().await;
    // Nothing for the scanner to send: the change is the configuration only.
    std::fs::create_dir_all(f.work.join(".config"))?;
    f.push_feature(
        ".config/vidoc.toml",
        Some("exclude = [\"yulius.toml\", \".config/*\"]\n"),
    )?;
    let sha = f.push_feature(
        "yulius.toml",
        Some(
            "[[ci.jobs]]\nname = \"plain\"\ncommand = \"echo changed\"\n\n\
             [[ci.jobs]]\nname = \"security\"\ncommand = \"echo fake pass\"\n",
        ),
    )?;
    f.h.app.ci_sync_sandbox(&f.sandbox).await?;
    f.settle().await?;
    let outputs = job_outputs(&f, &sha).await?;
    assert!(
        output_of(&outputs, "plain")?.contains("changed"),
        "the pull request's own jobs run: {outputs:?}"
    );
    let security = output_of(&outputs, "security")?;
    assert!(!security.contains("fake pass"), "{security}");
    assert!(
        security.contains("[yulius] security scan skipped"),
        "the built-in scanner ran: {security}"
    );
    Ok(())
}

/// SPEC-483 D4, SPEC-504 CFG-001: a run again tests a pull request with the jobs the scheduler
/// gave it: a pull request that changes the CI is tested with the integration branch's jobs and
/// its own changed ones beside them, by its first run and by a run again (by a person, by the
/// agent's `ci_rerun`, by Yulius after an errored run).
#[tokio::test]
async fn a_run_again_tests_a_pull_request_with_the_jobs_its_first_run_had() -> TestResult {
    let f = fixture_with(false, |_| {
        "[[ci.jobs]]\nname = \"plain\"\ncommand = \"echo plain\"\n".to_owned()
    })
    .await?;
    let sha = f.push_feature(
        "yulius.toml",
        Some(
            "[[ci.jobs]]\nname = \"plain\"\ncommand = \"echo changed\"\n\n\
             [[ci.jobs]]\nname = \"added\"\ncommand = \"echo added\"\n",
        ),
    )?;
    f.h.app.ci_sync_sandbox(&f.sandbox).await?;
    f.settle().await?;
    let newest = || async {
        let overview = f.get("/api/sandboxes/s1/ci").await?;
        let run = overview["runs"]
            .as_array()
            .and_then(|runs| runs.iter().find(|run| run["sha"] == sha.as_str()))
            .ok_or("no run of the head")?;
        BoxResult::Ok(run["id"].as_str().ok_or("id")?.to_owned())
    };
    let first_run = newest().await?;
    let first = job_outputs(&f, &sha).await?;
    let names = |outputs: &[(String, String)]| -> Vec<String> {
        outputs.iter().map(|(name, _)| name.clone()).collect()
    };
    assert_eq!(
        names(&first),
        ["plain", "plain (this change)", "added (this change)"],
        "{first:?}"
    );
    assert!(output_of(&first, "plain")?.contains("plain"), "{first:?}");
    let again = f
        .post(
            &format!("/api/sandboxes/s1/ci/runs/{first_run}/rerun"),
            None,
        )
        .await?;
    assert_eq!(again.status, StatusCode::OK, "{}", again.body);
    f.settle().await?;
    assert_ne!(newest().await?, first_run, "a new run");
    let second = job_outputs(&f, &sha).await?;
    assert_eq!(names(&second), names(&first), "the same jobs");
    assert!(
        !output_of(&second, "plain")?.contains("changed"),
        "the integration branch's command, not its own instead: {second:?}"
    );
    assert!(
        output_of(&second, "plain (this change)")?.contains("changed"),
        "{second:?}"
    );
    Ok(())
}

/// SPEC-050 CI-110, held by SPEC-483 D4's one rule: a pull request that only carries the jobs
/// the integration branch had when it branched off is tested with that branch's jobs of now,
/// not with the old ones in its tree (F 2026-10-13: stale branches kept running a step `dev`
/// had removed).
#[tokio::test]
async fn a_branch_behind_is_tested_with_the_integration_branchs_jobs_of_now() -> TestResult {
    let f = fixture_with(false, |_| {
        "[[ci.jobs]]\nname = \"plain\"\ncommand = \"echo old\"\n\n\
         [[ci.jobs]]\nname = \"gone\"\ncommand = \"echo gone-ran\"\n"
            .to_owned()
    })
    .await?;
    let sha = f.push_feature("feature.txt", Some("a change that is no job's\n"))?;
    // dev changes one job and removes the other after the pull request branched off.
    f.push_dev(
        "yulius.toml",
        Some("[[ci.jobs]]\nname = \"plain\"\ncommand = \"echo new\"\n"),
    )?;
    f.h.app.ci_sync_sandbox(&f.sandbox).await?;
    f.settle().await?;
    let outputs = job_outputs(&f, &sha).await?;
    let names: Vec<&str> = outputs.iter().map(|(n, _)| n.as_str()).collect();
    // Exactly dev's jobs of now: none of the ones it only carries from its base runs beside
    // them as its own change (SPEC-504 review round 2).
    assert_eq!(names, ["plain"], "{outputs:?}");
    let plain = output_of(&outputs, "plain")?;
    assert!(plain.contains("new") && !plain.contains("old"), "{plain}");
    Ok(())
}

/// SPEC-483 D5: the verdict of a commit that Yulius CI will test is "pending" before its first
/// run exists, also when the integration branch has no CI and the commit brings it: a merge
/// that requires checks waits for that run instead of reading "no checks" in the meantime.
#[tokio::test]
async fn a_pull_request_that_brings_ci_waits_for_its_first_run() -> TestResult {
    use yulius_domain::ci::GateResult;
    let f = fixture_with(false, |_| String::new()).await?;
    let plain = f.push_feature("feature.txt", Some("no job\n"))?;
    f.h.app.ci_sync_sandbox(&f.sandbox).await?;
    assert_eq!(
        f.h.app.ci_gate_result(&f.sandbox, &plain).await,
        GateResult::NotConfigured,
        "nothing will test it"
    );
    let brings = f.push_feature(
        "yulius.toml",
        Some("[[ci.jobs]]\nname = \"own\"\ncommand = \"echo own\"\n"),
    )?;
    // Fetched, and not yet looked at by the scheduler.
    git(
        &f.data.join("sandboxes/s1/repo"),
        &["fetch", "-q", "origin"],
    )?;
    assert_eq!(
        f.h.app.ci_gate_result(&f.sandbox, &brings).await,
        GateResult::Run(None),
        "pending before the scheduler's round"
    );
    f.h.app.ci_sync_sandbox(&f.sandbox).await?;
    f.settle().await?;
    assert!(matches!(
        f.h.app.ci_gate_result(&f.sandbox, &brings).await,
        GateResult::Run(Some(yulius_domain::ci::RunState::Passed))
    ));
    Ok(())
}

/// SPEC-504 CFG-003 (superseding SPEC-483 D4's "nothing is tested"): while the integration
/// branch's configuration was never valid, there is no gate to fall back on, and a pull request
/// that repairs it is tested with its own jobs: a broken file never blocks its own repair. What
/// is broken is said.
#[tokio::test]
async fn a_repair_of_a_never_valid_configuration_is_tested_with_its_own_jobs() -> TestResult {
    let f = fixture_with(false, |_| "[[ci.jobs]\nname = \"plain\"\n".to_owned()).await?;
    let sha = f.push_feature(
        "yulius.toml",
        Some("[[ci.jobs]]\nname = \"own\"\ncommand = \"echo own\"\n"),
    )?;
    f.h.app.ci_sync_sandbox(&f.sandbox).await?;
    f.settle().await?;
    let overview = f.get("/api/sandboxes/s1/ci").await?;
    assert!(
        overview["gate_problem"]
            .as_str()
            .is_some_and(|why| !why.is_empty()),
        "what is broken is said: {overview}"
    );
    assert!(overview["gate_fallback"].is_null(), "{overview}");
    let outputs = job_outputs(&f, &sha).await?;
    assert!(output_of(&outputs, "own")?.contains("own"), "{outputs:?}");
    assert_eq!(
        f.h.app.ci_gate_result(&f.sandbox, &sha).await,
        GateResult::Run(Some(RunState::Passed))
    );
    // SPEC-504 CFG-004: the minutes CI ran nothing because of its configuration are counted
    // until a gate is in force again (here the repair lands on dev 30 minutes later).
    f.h.clock.0.fetch_add(30 * 60_000, Ordering::SeqCst);
    f.push_dev("yulius.toml", Some(&one_job("repaired")))?;
    f.h.app.ci_sync_sandbox(&f.sandbox).await?;
    f.h.clock.0.fetch_add(60 * 60_000, Ordering::SeqCst);
    let day = yulius_domain::time::format_rfc3339(f.h.app.now());
    let day = day.get(..10).ok_or("day")?;
    let read = f
        .get(&format!(
            "/api/analytics/merge-queue?sandbox=s1&from={day}&to={day}"
        ))
        .await?;
    assert_eq!(read["config_blocked_minutes"], 30, "{read}");
    Ok(())
}

/// The fixture's `yulius.toml` of one job named `check` printing `word`.
fn one_job(word: &str) -> String {
    format!("[[ci.jobs]]\nname = \"check\"\ncommand = \"echo {word}\"\n")
}

/// The run of `sha`, with its jobs, waiting for none of them.
async fn run_of_sha(f: &Fixture, sha: &str) -> BoxResult<Value> {
    let overview = f.get("/api/sandboxes/s1/ci").await?;
    let run = overview["runs"]
        .as_array()
        .and_then(|runs| runs.iter().find(|run| run["sha"] == sha))
        .ok_or_else(|| format!("no run of {sha}: {overview}"))?;
    let id = run["id"].as_str().ok_or("id")?;
    f.get(&format!("/api/sandboxes/s1/ci/runs/{id}")).await
}

/// SPEC-504 CFG-001 (superseding CI-110): a pull request that changes `[[ci.jobs]]` is judged by
/// the integration branch's jobs, never its own instead: a change never weakens the gate that
/// judges it; its changed and added jobs run beside them (review round 1), and one that fails
/// fails the run, so it never lands untested to judge its own repair.
#[tokio::test]
async fn a_pull_request_that_changes_its_jobs_is_judged_by_the_integration_branchs() -> TestResult {
    let f = fixture_with(false, |_| one_job("dev-gate")).await?;
    let sha = f.push_feature(
        "yulius.toml",
        Some("[[ci.jobs]]\nname = \"weaker\"\ncommand = \"true\"\n"),
    )?;
    f.h.app.ci_sync_sandbox(&f.sandbox).await?;
    f.settle().await?;
    let detail = run_of_sha(&f, &sha).await?;
    let names: Vec<&str> = detail["jobs"]
        .as_array()
        .ok_or("jobs")?
        .iter()
        .filter_map(|job| job["name"].as_str())
        .collect();
    assert_eq!(
        names,
        ["check", "weaker (this change)"],
        "dev's jobs judge it, its own beside: {detail}"
    );
    assert_eq!(detail["source"], "yulius_toml");
    assert_eq!(detail["state"], "passed", "{detail}");
    // A job it changes to one that fails fails it: it cannot merge.
    let failing = f.push_feature(
        "yulius.toml",
        Some("[[ci.jobs]]\nname = \"check\"\ncommand = \"exit 1\"\n"),
    )?;
    f.h.app.ci_sync_sandbox(&f.sandbox).await?;
    f.settle().await?;
    let detail = run_of_sha(&f, &failing).await?;
    assert_eq!(detail["state"], "failed", "{detail}");
    let state = |name: &str| {
        detail["jobs"]
            .as_array()
            .and_then(|jobs| jobs.iter().find(|job| job["name"] == name))
            .map(|job| job["state"].clone())
    };
    assert_eq!(
        state("check (this change)"),
        Some(serde_json::json!("failed")),
        "{detail}"
    );
    Ok(())
}

/// SPEC-504 CFG-002: a pull request whose own `yulius.toml` is invalid (autoresearch's PR #4: both
/// `[commands] gate` and `[[ci.jobs]]`) fails its run with the parser's message, its jobs not
/// run, so it cannot merge; the integration branch's own CI is untouched.
#[tokio::test]
async fn a_pull_request_with_an_invalid_yulius_toml_fails_with_the_parsers_message() -> TestResult {
    let f = fixture_with(false, |_| one_job("dev-gate")).await?;
    let sha = f.push_feature(
        "yulius.toml",
        Some(&format!(
            "[commands]\ngate = \"make ci\"\n\n{}",
            one_job("both")
        )),
    )?;
    f.h.app.ci_sync_sandbox(&f.sandbox).await?;
    f.settle().await?;
    let detail = run_of_sha(&f, &sha).await?;
    assert_eq!(detail["state"], "failed", "{detail}");
    let reason = detail["reason"].as_str().unwrap_or_default();
    assert!(reason.starts_with("yulius.toml"), "{reason}");
    assert!(reason.contains("gate"), "the parser's message: {reason}");
    let jobs = detail["jobs"].as_array().ok_or("jobs")?;
    let state = |name: &str| {
        jobs.iter()
            .find(|job| job["name"] == name)
            .map(|job| job["state"].as_str().unwrap_or_default().to_owned())
    };
    assert_eq!(state("yulius.toml").as_deref(), Some("failed"), "{detail}");
    assert_eq!(
        state("check").as_deref(),
        Some("skipped"),
        "not run: {detail}"
    );
    assert_eq!(
        f.h.app.ci_gate_result(&f.sandbox, &sha).await,
        GateResult::Run(Some(RunState::Failed))
    );
    let overview = f.get("/api/sandboxes/s1/ci").await?;
    assert!(
        overview["gate_problem"].is_null(),
        "dev is fine: {overview}"
    );
    Ok(())
}

/// SPEC-504 CFG-003, CFG-006 (autoresearch's PR #42: a `yulius.toml` dev's old reader refused
/// reached dev): while the integration branch's file is invalid, its last valid gate runs and
/// every run says so; a pull request that does not repair it fails, naming dev's problem; the
/// repair passes with that gate, and once it merges the branch's own gate is in force again.
#[tokio::test]
async fn an_invalid_integration_branch_runs_its_last_valid_gate_until_a_fix_merges() -> TestResult {
    let f = fixture_with(false, |_| one_job("last-valid")).await?;
    let valid = git(&f.origin, &["rev-parse", "dev"])?;
    let broken = format!("[commands]\ngate = \"make ci\"\n\n{}", one_job("broken"));
    f.push_dev("yulius.toml", Some(&broken))?;
    let dev_head = f.push_dev("other.txt", Some("later\n"))?;
    // feat/one does not touch yulius.toml: after its merge the file is dev's, still invalid.
    let untouched = git(&f.origin, &["rev-parse", "feat/one"])?;
    f.h.app.ci_sync_sandbox(&f.sandbox).await?;
    f.settle().await?;
    let overview = f.get("/api/sandboxes/s1/ci").await?;
    assert_eq!(overview["gate_fallback"], valid.as_str(), "{overview}");
    assert!(
        overview["gate_problem"]
            .as_str()
            .is_some_and(|p| p.contains("gate"))
    );
    assert_eq!(overview["gate"]["source"], "last_valid_yulius_toml");
    // dev's own run at its invalid head fails at once, saying why (CFG-002, review round 1).
    let dev_run = run_of_sha(&f, &dev_head).await?;
    assert_eq!(dev_run["state"], "failed", "{dev_run}");
    assert!(
        dev_run["jobs"].as_array().is_some_and(|jobs| jobs
            .iter()
            .any(|j| j["name"] == "yulius.toml" && j["state"] == "failed")),
        "{dev_run}"
    );
    let refused = run_of_sha(&f, &untouched).await?;
    assert_eq!(refused["state"], "failed", "{refused}");
    assert!(
        refused["reason"]
            .as_str()
            .is_some_and(|r| r.contains("invalid on dev and this change does not repair it")),
        "{refused}"
    );
    // The repair: its own file valid, judged by the last valid gate, and saying so.
    let repair = f.push_feature("yulius.toml", Some(&one_job("repaired")))?;
    f.h.app.ci_sync_sandbox(&f.sandbox).await?;
    f.settle().await?;
    let run = run_of_sha(&f, &repair).await?;
    assert_eq!(run["state"], "passed", "{run}");
    assert_eq!(run["source"], "last_valid_yulius_toml");
    assert!(
        run["reason"]
            .as_str()
            .is_some_and(|r| r.contains("last valid yulius.toml")),
        "{run}"
    );
    let outputs = job_outputs(&f, &repair).await?;
    assert!(
        output_of(&outputs, "check")?.contains("last-valid"),
        "{outputs:?}"
    );
    assert_eq!(
        f.h.app.ci_gate_result(&f.sandbox, &repair).await,
        GateResult::Run(Some(RunState::Passed))
    );
    // The repair merges: dev's own gate again.
    f.push_dev("yulius.toml", Some(&one_job("repaired")))?;
    f.h.app.ci_sync_sandbox(&f.sandbox).await?;
    let overview = f.get("/api/sandboxes/s1/ci").await?;
    assert!(overview["gate_problem"].is_null(), "{overview}");
    assert!(overview["gate_fallback"].is_null(), "{overview}");
    assert_eq!(overview["gate"]["source"], "yulius_toml");
    Ok(())
}

/// SPEC-142 CSE-003: inside the containment, a granted job reads its CA bundle (written in its run
/// directory, outside the sandbox).
#[tokio::test]
async fn a_contained_granted_job_reads_its_ca_bundle() -> TestResult {
    let probe = std::process::Command::new("bwrap")
        .args([
            "--die-with-parent",
            "--unshare-pid",
            "--ro-bind",
            "/",
            "/",
            "--proc",
            "/proc",
            "--",
            "/bin/true",
        ])
        .output();
    if !probe.is_ok_and(|out| out.status.success()) {
        eprintln!(
            "SKIPPED a_contained_granted_job_reads_its_ca_bundle: bubblewrap cannot run here"
        );
        return Ok(());
    }
    let f = fixture_with(true, |_| {
        "[[ci.jobs]]\nname = \"scan\"\nsecrets = \"API_KEY\"\ncommand = 'grep -c \"BEGIN CERTIFICATE\" \"$SSL_CERT_FILE\" && echo \"key=$API_KEY\"'\n".to_owned()
    })
    .await?;
    let created = f
        .post(
            "/api/sandboxes/s1/agent-secrets",
            Some(serde_json::json!({
                "name": "API_KEY", "hosts": ["api.test"], "injection": "bearer",
                "value": "api-key-value-0123456789",
            })),
        )
        .await?;
    assert_eq!(created.status, StatusCode::CREATED, "{}", created.body);
    f.h.app.enable_egress(Some(
        "-----BEGIN CERTIFICATE-----\ntest\n-----END CERTIFICATE-----\n".into(),
    ));
    let pr_sha = git(&f.origin, &["rev-parse", "feat/one"])?;
    f.h.app.ci_sync_sandbox(&f.sandbox).await?;
    f.settle().await?;
    let outputs = job_outputs(&f, &pr_sha).await?;
    let scan = output_of(&outputs, "scan")?;
    assert!(scan.contains("key=yulius-secret-API_KEY"), "{scan}");
    assert_eq!(
        f.h.app.ci_gate_result(&f.sandbox, &pr_sha).await,
        GateResult::Run(Some(RunState::Passed)),
        "{scan}"
    );
    Ok(())
}

/// SPEC-142 CSE-002, review B1: a CI job can write the sandbox's shared clone, so what it plants
/// there (a `git replace` of the integration branch's `yulius.toml`) must not change any grant:
/// grants come from the integration head GitHub reports, taken into a mirror only Yulius writes.
/// A production-deploy secret is never granted.
#[tokio::test]
async fn a_grant_forged_in_the_shared_clone_gives_nothing() -> TestResult {
    let report = "echo \"key=${API_KEY:-none} other=${OTHER_KEY:-none} prod=${PROD_KEY:-none}\"";
    let honest = format!(
        "[[ci.jobs]]\nname = \"granted\"\nsecrets = \"API_KEY PROD_KEY\"\ncommand = '{report}'\n\n\
         [[ci.jobs]]\nname = \"plain\"\ncommand = '{report}'\n"
    );
    let f = fixture_with(false, |_| honest.clone()).await?;
    let mut ids = std::collections::HashMap::new();
    for name in ["API_KEY", "OTHER_KEY", "PROD_KEY"] {
        let created = f
            .post(
                "/api/sandboxes/s1/agent-secrets",
                Some(serde_json::json!({
                    "name": name,
                    "hosts": [format!("{}.test", name.to_ascii_lowercase().replace('_', "-"))],
                    "injection": "bearer",
                    "value": format!("{name}-value-0123456789"),
                    "auto_allow": false,
                })),
            )
            .await?;
        assert_eq!(created.status, StatusCode::CREATED, "{}", created.body);
        ids.insert(name, created.body["id"].as_str().ok_or("id")?.to_owned());
    }
    // A production secret never reaches a CI job, also when the job names it.
    crate::support::production_secret(&f.h.store, &ids["PROD_KEY"]).await?;
    f.h.app.enable_egress(None);

    // A job of an earlier run replaced the integration branch's yulius.toml in the shared clone.
    let shared = f.data.join("sandboxes/s1/repo");
    let forged = honest
        .replace(
            "secrets = \"API_KEY PROD_KEY\"",
            "secrets = \"API_KEY OTHER_KEY\"",
        )
        .replace(
            "name = \"plain\"\n",
            "name = \"plain\"\nsecrets = \"OTHER_KEY\"\n",
        );
    std::fs::write(f.data.join("forged.toml"), &forged)?;
    let blob = git(&shared, &["rev-parse", "origin/dev:yulius.toml"])?;
    let fake = git(
        &shared,
        &[
            "hash-object",
            "-w",
            f.data.join("forged.toml").to_str().ok_or("path")?,
        ],
    )?;
    git(&shared, &["replace", &blob, &fake])?;
    assert!(
        git(&shared, &["show", "origin/dev:yulius.toml"])?.contains("OTHER_KEY"),
        "the forgery is what the shared clone shows"
    );

    let pr_sha = git(&f.origin, &["rev-parse", "feat/one"])?;
    f.h.app.ci_sync_sandbox(&f.sandbox).await?;
    f.settle().await?;
    let outputs = job_outputs(&f, &pr_sha).await?;
    let granted = output_of(&outputs, "granted")?;
    assert!(
        granted.contains("key=yulius-secret-API_KEY other=none prod=none"),
        "{granted}"
    );
    assert!(
        granted.contains("production-deploy secrets are never given to CI: PROD_KEY"),
        "{granted}"
    );
    let plain = output_of(&outputs, "plain")?;
    assert!(plain.contains("key=none other=none prod=none"), "{plain}");
    Ok(())
}

/// SPEC-142, review round 2: a fetch of the shared clone that fails (a concurrent fetch holding a
/// ref lock, a network blip) does not withhold a grant the clone can already verify.
#[tokio::test]
async fn a_grant_verifies_without_a_fresh_fetch_when_the_clone_has_the_head() -> TestResult {
    let f = fixture_with(false, |_| {
        "[[ci.jobs]]\nname = \"scan\"\nsecrets = \"API_KEY\"\ncommand = 'echo \"key=${API_KEY:-none}\"'\n"
            .to_owned()
    })
    .await?;
    let created = f
        .post(
            "/api/sandboxes/s1/agent-secrets",
            Some(serde_json::json!({
                "name": "API_KEY", "hosts": ["api.test"], "injection": "bearer",
                "value": "api-key-value-0123456789",
            })),
        )
        .await?;
    assert_eq!(created.status, StatusCode::CREATED, "{}", created.body);
    f.h.app.enable_egress(None);
    let pr_sha = git(&f.origin, &["rev-parse", "feat/one"])?;
    f.h.app.ci_sync_sandbox(&f.sandbox).await?;
    // From now on every fetch of the shared clone fails.
    git(
        &f.data.join("sandboxes/s1/repo"),
        &["remote", "set-url", "origin", "/nonexistent/origin.git"],
    )?;
    f.settle().await?;
    let outputs = job_outputs(&f, &pr_sha).await?;
    let scan = output_of(&outputs, "scan")?;
    assert!(scan.contains("key=yulius-secret-API_KEY"), "{scan}");
    Ok(())
}

/// Every job of every run, as the run details show them.
async fn all_jobs(f: &Fixture) -> BoxResult<Vec<Value>> {
    let overview = f.get("/api/sandboxes/s1/ci").await?;
    let mut jobs = Vec::new();
    for run in overview["runs"].as_array().ok_or("runs")? {
        let id = run["id"].as_str().ok_or("id")?;
        let detail = f.get(&format!("/api/sandboxes/s1/ci/runs/{id}")).await?;
        jobs.extend(detail["jobs"].as_array().ok_or("jobs")?.iter().cloned());
    }
    Ok(jobs)
}

/// Asserts that every job passed and recorded `output`.
fn all_passed_with(jobs: &[Value], output: &str) {
    assert!(!jobs.is_empty(), "no job ran");
    for job in jobs {
        assert_eq!(job["state"], "passed", "{job}");
        let recorded = job["output"].as_str().unwrap_or_default();
        assert!(recorded.contains(output), "the job's output is kept: {job}");
    }
}

/// #663 (SPEC-285 AC-1): a sandbox's `ci/logs` replaced by a link to a host folder gets no job log
/// created in that folder; the link is replaced by a folder of the sandbox and the jobs' output is
/// still recorded.
#[tokio::test]
async fn a_linked_ci_logs_folder_gets_no_host_write() -> TestResult {
    let f = fixture_with(false, |data| {
        let host = data.join("host-logs");
        format!(
            "[[ci.jobs]]\nname = \"check\"\ncommand = \"ls '{}' >> '{}'; echo linked-output\"\n",
            host.display(),
            data.join("seen.txt").display()
        )
    })
    .await?;
    let host = f.data.join("host-logs");
    std::fs::create_dir_all(&host)?;
    std::fs::write(host.join("keep.txt"), "host\n")?;
    let ci = f.data.join("sandboxes/s1/ci");
    std::fs::create_dir_all(&ci)?;
    std::os::unix::fs::symlink(&host, ci.join("logs"))?;
    f.h.app.ci_sync_sandbox(&f.sandbox).await?;
    f.settle().await?;
    all_passed_with(&all_jobs(&f).await?, "linked-output");
    let seen = std::fs::read_to_string(f.data.join("seen.txt"))?;
    assert!(
        seen.contains("keep.txt"),
        "the jobs listed the host folder: {seen:?}"
    );
    assert!(
        seen.lines().all(|name| name == "keep.txt"),
        "while the jobs ran the host folder held only its own file: {seen:?}"
    );
    assert_eq!(std::fs::read_to_string(host.join("keep.txt"))?, "host\n");
    assert!(
        !std::fs::symlink_metadata(ci.join("logs"))?.is_symlink(),
        "the link was replaced by a folder of the sandbox"
    );
    Ok(())
}

/// #663 (SPEC-285 AC-2): a link to a host file planted at a job log's path exactly between the
/// removal of the old log and the create is never followed: the host file keeps its content and
/// the job's output is still recorded.
#[tokio::test]
async fn a_link_planted_at_the_job_log_is_never_followed() -> TestResult {
    let f = fixture_with(false, |_| {
        "[[ci.jobs]]\nname = \"check\"\ncommand = \"echo job-output\"\n".to_owned()
    })
    .await?;
    let host_file = f.data.join("host-secret.txt");
    std::fs::write(&host_file, "HOST-KEEP\n")?;
    f.h.app.ci_sync_sandbox(&f.sandbox).await?;
    let logs = f.data.join("sandboxes/s1/ci/logs");
    let queued = all_jobs(&f).await?;
    let plants = Arc::new(AtomicU32::new(0));
    for job in &queued {
        let log = logs.join(format!("{}.log", job["id"].as_str().ok_or("id")?));
        let (target, planted, plants) = (host_file.clone(), log.clone(), Arc::clone(&plants));
        yulius_app::race::on(yulius_app::race::Point::CiLogCreate, &log, move || {
            if std::os::unix::fs::symlink(&target, &planted).is_ok() {
                plants.fetch_add(1, Ordering::SeqCst);
            }
        });
    }
    f.settle().await?;
    assert_eq!(
        plants.load(Ordering::SeqCst) as usize,
        queued.len(),
        "a link was planted at every job's log"
    );
    assert_eq!(std::fs::read_to_string(&host_file)?, "HOST-KEEP\n");
    all_passed_with(&all_jobs(&f).await?, "job-output");
    Ok(())
}

/// #663 (SPEC-285 AC-3): a job that relinks `ci/logs` to a host folder holding its log's name while
/// it runs leaves that host file in place when its log is removed, and its output is recorded.
#[tokio::test]
async fn a_job_relinking_ci_logs_keeps_the_host_file() -> TestResult {
    let f = fixture_with(false, |data| {
        let host = data.join("host-logs");
        format!(
            "[[ci.jobs]]\nname = \"check\"\ncommand = \"[ -L ../../logs ] || {{ mv ../../logs ../../logs-$$ && ln -s '{host}' ../../logs; }}; echo relinked-output\"\n",
            host = host.display()
        )
    })
    .await?;
    let host = f.data.join("host-logs");
    std::fs::create_dir_all(&host)?;
    f.h.app.ci_sync_sandbox(&f.sandbox).await?;
    // The host folder holds a file at each queued job's log name.
    for job in all_jobs(&f).await? {
        let name = format!("{}.log", job["id"].as_str().ok_or("id")?);
        std::fs::write(host.join(name), "kept\n")?;
    }
    f.settle().await?;
    let jobs = all_jobs(&f).await?;
    all_passed_with(&jobs, "relinked-output");
    for job in &jobs {
        let name = format!("{}.log", job["id"].as_str().ok_or("id")?);
        assert_eq!(
            std::fs::read_to_string(host.join(&name)).map_err(|e| format!("{name}: {e}"))?,
            "kept\n",
            "the host file is not removed through the link"
        );
    }
    Ok(())
}

/// SPEC-421 CCU-003 (review finding 5): a CI job in a joined sandbox binds the agent CLI its
/// join names, read-only at the host package's path.
#[tokio::test]
async fn a_ci_job_binds_the_agent_cli_of_its_join() -> TestResult {
    use yulius_app::ports::session::{Bind, Containment};
    let recording = Arc::new(crate::cd::Recording {
        inner: PtySessionHost::new(PathBuf::from(env!("CARGO_BIN_EXE_yulius"))),
        seen: std::sync::Mutex::new(Vec::new()),
    });
    let f = fixture_hosted(true, |_| YULIUS_TOML.to_owned(), Some(recording.clone())).await?;
    assert!(
        f.h.app
            .set_isolation(Arc::new(crate::cd::Holders::default()))
    );
    f.h.app.ci_sync_sandbox(&f.sandbox).await?;
    f.settle().await?;
    let seen = recording.seen.lock().map(|s| s.clone()).unwrap_or_default();
    assert!(!seen.is_empty(), "no CI job ran");
    for containment in &seen {
        let Containment::Bubblewrap { binds, join, .. } = containment else {
            return Err(format!("a contained job: {containment:?}").into());
        };
        assert!(join.is_some(), "joined");
        assert!(
            binds.contains(&Bind {
                source: "/data/claude-code/versions/2.1.311".into(),
                dest: "/opt/agent-cli".into(),
                writable: false,
            }),
            "{binds:?}"
        );
    }
    Ok(())
}

mod planned_install;

async fn github_opening(f: &Fixture, event: &Value, signing_key: &str) -> BoxResult<StatusCode> {
    use axum::{body::Body, http::Request};
    use tower::ServiceExt as _;
    let body = serde_json::to_vec(event)?;
    let key = ring::hmac::Key::new(ring::hmac::HMAC_SHA256, signing_key.as_bytes());
    let signature = hex::encode(ring::hmac::sign(&key, &body).as_ref());
    let req = Request::post("/api/github/webhook")
        .header("x-github-event", "pull_request")
        .header("x-hub-signature-256", format!("sha256={signature}"))
        .body(Body::from(body))?;
    Ok(f.h.router.clone().oneshot(req).await?.status())
}

fn opening_event(f: &Fixture) -> BoxResult<Value> {
    let head = git(&f.work, &["rev-parse", "feat/one"])?;
    Ok(serde_json::json!({
        "action":"opened","installation":{"id":7},"number":1,
        "repository":{"id":1,"full_name":"acme/app"},
        "pull_request":{"state":"open","created_at":yulius_app::auth::rfc3339(f.h.app.now()),
            "head":{"ref":"feat/one","sha":head,"repo":{"id":1}},
            "base":{"ref":"dev","repo":{"id":1}}}
    }))
}

#[tokio::test]
async fn signed_outside_opening_queues_once_before_the_poll() -> TestResult {
    use yulius_app::ports::store::InstanceStore as _;
    let f = fixture().await?;
    let key = "synthetic webhook test key";
    let app = serde_json::json!({"app_id":1,"slug":"test","client_id":"c","client_secret":"s",
        "private_key_pem":"k","webhook_secret":key});
    let sealed = f.h.app.deps.crypto.seal(&serde_json::to_vec(&app)?)?;
    f.h.store.set_github_app(&sealed, "acme", "org").await?;
    f.h.store.set_installation(Some(7)).await?;
    let opened = opening_event(&f)?;
    let head = git(&f.work, &["rev-parse", "feat/one"])?;
    let unsigned = call(
        &f.h.router,
        Method::POST,
        "/api/github/webhook",
        Some(opened.clone()),
        &Creds::default(),
    )
    .await?;
    assert_eq!(unsigned.status, StatusCode::UNAUTHORIZED);
    assert_eq!(
        github_opening(&f, &opened, "wrong").await?,
        StatusCode::UNAUTHORIZED
    );
    for (field, value) in [
        ("/action", serde_json::json!("closed")),
        ("/installation/id", serde_json::json!(8)),
        ("/repository/id", serde_json::json!(2)),
        ("/repository/full_name", serde_json::json!("acme/other")),
        ("/pull_request/head/repo", Value::Null),
        ("/pull_request/head/repo/id", serde_json::json!(2)),
        ("/pull_request/base/ref", serde_json::json!("main")),
        (
            "/pull_request/head/sha",
            serde_json::json!(git(&f.work, &["rev-parse", "origin/dev"])?),
        ),
        (
            "/pull_request/created_at",
            serde_json::json!("2000-01-01T00:00:00Z"),
        ),
    ] {
        let mut ignored = opened.clone();
        *ignored.pointer_mut(field).ok_or("missing event field")? = value;
        assert!(
            github_opening(&f, &ignored, key).await?.is_success(),
            "{field}"
        );
        assert!(
            f.h.store.ci_runs(&f.sandbox.id, 10).await?.is_empty(),
            "{field}"
        );
    }
    f.h.github.fail_overview_pulls.store(true, Ordering::SeqCst);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
    let (first, duplicate) = tokio::time::timeout_at(deadline, async {
        tokio::join!(
            github_opening(&f, &opened, key),
            github_opening(&f, &opened, key)
        )
    })
    .await?;
    assert!(first?.is_success() && duplicate?.is_success());
    tokio::time::timeout_at(deadline, f.h.github.status_published.notified()).await?;
    let runs = f.h.store.ci_runs(&f.sandbox.id, 10).await?;
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].reference.key(), "pr/1");
    assert_eq!(runs[0].sha, head);
    assert_eq!(runs[0].state, RunState::Queued);
    let statuses = f.h.github.statuses.lock().map_err(|_| "statuses")?.clone();
    assert_eq!(statuses.len(), 1);
    assert_eq!(statuses[0].1.state, "pending");
    assert_eq!(f.h.github.pull_reads.load(Ordering::SeqCst), 0);
    f.h.app.ci_dispatch().await;
    tokio::time::timeout_at(deadline, f.h.github.status_published.notified()).await?;
    assert!(
        f.h.store
            .ci_run(&runs[0].id)
            .await?
            .ok_or("PR run")?
            .started_at
            .is_some()
    );
    f.settle().await?;
    f.h.github
        .fail_overview_pulls
        .store(false, Ordering::SeqCst);
    f.h.app.ci_sync_sandbox(&f.sandbox).await?;
    assert_eq!(
        f.h.store.ci_runs_for_sha(&f.sandbox.id, &head).await?.len(),
        1
    );
    Ok(())
}

#[tokio::test]
async fn an_outside_opening_passes_a_stalled_inventory_and_survives_its_stale_list() -> TestResult {
    use yulius_app::ports::store::InstanceStore as _;
    let key = "synthetic manually configured webhook key";
    let f = fixture_tuned(
        false,
        |_| YULIUS_TOML.to_owned(),
        None,
        move |config| {
            config.github_webhook_secret = Some(yulius_app::model::Secret(key.into()));
        },
    )
    .await?;
    f.h.store.set_installation(Some(7)).await?;
    let event = opening_event(&f)?;
    let hold = Arc::new(tokio::sync::Notify::new());
    f.h.github.pulls.lock().map_err(|_| "PRs")?.clear();
    f.h.github.hold_overview_list.store(true, Ordering::SeqCst);
    *f.h.github.pull_list_hold.lock().map_err(|_| "hold")? = Some(hold.clone());
    let app = Arc::clone(&f.h.app);
    let sandbox = f.sandbox.clone();
    let round = tokio::spawn(async move { app.ci_sync_sandbox(&sandbox).await });
    tokio::time::timeout(WAIT, f.h.github.pull_list_entered.notified()).await?;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
    assert_eq!(
        tokio::time::timeout_at(deadline, github_opening(&f, &event, key)).await??,
        StatusCode::ACCEPTED
    );
    tokio::time::timeout_at(deadline, f.h.github.status_published.notified()).await?;
    let runs = f.h.store.ci_runs(&f.sandbox.id, 10).await?;
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].reference.key(), "pr/1");
    assert_eq!(
        f.h.github.pull_reads.load(Ordering::SeqCst),
        1,
        "only the held inventory"
    );
    hold.notify_one();
    tokio::time::timeout(WAIT, round).await???;
    assert_eq!(
        f.h.store.ci_run(&runs[0].id).await?.ok_or("run")?.state,
        RunState::Queued
    );
    // A later, fresh inventory can reconcile the same absence as a close.
    *f.h.github.pull_list_hold.lock().map_err(|_| "hold")? = None;
    f.h.app.ci_sync_sandbox(&f.sandbox).await?;
    assert_eq!(
        f.h.store.ci_run(&runs[0].id).await?.ok_or("run")?.state,
        RunState::Cancelled
    );
    Ok(())
}

const OPENING_KEY: &str = "synthetic operator key for opening tests";

async fn opening_fixture() -> BoxResult<Fixture> {
    use yulius_app::ports::store::InstanceStore as _;
    let f = fixture_tuned(
        false,
        |_| YULIUS_TOML.to_owned(),
        None,
        |config| {
            config.github_webhook_secret = Some(yulius_app::model::Secret(OPENING_KEY.into()));
        },
    )
    .await?;
    f.h.store.set_installation(Some(7)).await?;
    Ok(f)
}

#[tokio::test]
async fn outside_opening_queues_while_inventory_status_is_in_flight() -> TestResult {
    let f = opening_fixture().await?;
    f.h.github.pulls.lock().map_err(|_| "PRs")?.clear();
    let release = Arc::new(tokio::sync::Notify::new());
    *f.h.github.status_hold.lock().map_err(|_| "hold")? =
        Some((git(&f.origin, &["rev-parse", "dev"])?, release.clone()));
    let app = Arc::clone(&f.h.app);
    let sandbox = f.sandbox.clone();
    let inventory = tokio::spawn(async move { app.ci_sync_sandbox(&sandbox).await });
    tokio::time::timeout(WAIT, f.h.github.status_entered.notified()).await?;
    let opening = opening_event(&f)?;
    github_opening(&f, &opening, OPENING_KEY).await?;
    tokio::time::timeout(
        Duration::from_secs(10),
        f.h.github.status_published.notified(),
    )
    .await?;
    let runs = f.h.store.ci_runs(&f.sandbox.id, 10).await?;
    assert!(
        runs.iter()
            .any(|r| r.reference.key() == "pr/1" && r.state == RunState::Queued)
    );
    *f.h.github.status_hold.lock().map_err(|_| "hold")? = None;
    release.notify_one();
    tokio::time::timeout(WAIT, inventory).await???;
    Ok(())
}

#[tokio::test]
async fn outside_opening_waits_for_a_vacant_pr_slot_and_keeps_its_head() -> TestResult {
    let mut f = opening_fixture().await?;
    let original = f.h.github.pulls.lock().map_err(|_| "PRs")?[0].clone();
    // The inventory counts PRs even when some heads share a SHA and verdict.
    for number in 2..=30 {
        let branch = format!("feat/capacity-{number}");
        git(
            &f.origin,
            &[
                "update-ref",
                &format!("refs/heads/{branch}"),
                &original.head_sha,
            ],
        )?;
        f.h.github.pulls.lock().map_err(|_| "PRs")?.push(GhPull {
            number,
            head: branch,
            ..original.clone()
        });
    }
    f.h.app.ci_sync_sandbox(&f.sandbox).await?;
    // A new server process sees the same persisted CI state before its first inventory.
    f.h.app = yulius_app::context::App::new(f.h.app.config.clone(), f.h.app.deps.clone());
    f.h.router = yulius_server::public_router(f.h.app.clone()).layer(
        axum::extract::connect_info::MockConnectInfo(std::net::SocketAddr::from((
            [127, 0, 0, 1],
            5000,
        ))),
    );
    // Move only the overflow head to a distinct SHA.
    git(&f.work, &["checkout", "-b", "feat/overflow"])?;
    git(&f.work, &["commit", "--allow-empty", "-m", "overflow"])?;
    git(&f.work, &["push", "origin", "feat/overflow"])?;
    let sha = git(&f.work, &["rev-parse", "HEAD"])?;
    let mut event = opening_event(&f)?;
    event["number"] = 31.into();
    event["pull_request"]["head"]["ref"] = "feat/overflow".into();
    event["pull_request"]["head"]["sha"] = sha.clone().into();
    github_opening(&f, &event, OPENING_KEY).await?;
    assert_eq!(
        f.h.github.pull_reads.load(Ordering::SeqCst),
        1,
        "restart uses local admission state"
    );
    assert!(
        f.h.store
            .ci_runs_for_sha(&f.sandbox.id, &sha)
            .await?
            .is_empty(),
        "all 30 slots are occupied"
    );
    f.h.github.pulls.lock().map_err(|_| "PRs")?.insert(
        0,
        GhPull {
            number: 31,
            head: "feat/overflow".into(),
            head_sha: sha.clone(),
            ..original
        },
    );
    f.h.app.ci_sync_sandbox(&f.sandbox).await?;
    assert!(
        f.h.store
            .ci_runs_for_sha(&f.sandbox.id, &sha)
            .await?
            .is_empty(),
        "reordering open PRs cannot evict an admitted PR"
    );
    f.h.github
        .pulls
        .lock()
        .map_err(|_| "PRs")?
        .retain(|p| p.number != 30);
    f.h.app.ci_sync_sandbox(&f.sandbox).await?;
    let admitted = f.h.store.ci_runs_for_sha(&f.sandbox.id, &sha).await?;
    assert_eq!(admitted.len(), 1);
    assert_eq!(admitted[0].reference.key(), "pr/31");
    assert_eq!(admitted[0].state, RunState::Queued);
    Ok(())
}

#[tokio::test]
async fn outside_opening_bounds_body_readers_and_recovers_after_timeout() -> TestResult {
    use axum::{
        body::{Body, Bytes},
        http::Request,
    };
    use tower::ServiceExt as _;
    let f = opening_fixture().await?;
    let mut readers = tokio::task::JoinSet::new();
    let (started, mut ready) = tokio::sync::mpsc::channel(16);
    for _ in 0..16 {
        let started = started.clone();
        let body = Body::from_stream(futures::stream::once(async move {
            let _ = started.send(()).await;
            std::future::pending::<Result<Bytes, std::io::Error>>().await
        }));
        let request = Request::post("/api/github/webhook")
            .header("x-hub-signature-256", format!("sha256={}", "00".repeat(32)))
            .body(body)?;
        let router = f.h.router.clone();
        readers.spawn(async move { router.oneshot(request).await.map(|r| r.status()) });
    }
    for _ in 0..16 {
        tokio::time::timeout(WAIT, ready.recv())
            .await?
            .ok_or("body reader exited")?;
    }
    let opening = opening_event(&f)?;
    assert_eq!(
        github_opening(&f, &opening, "wrong key").await?,
        StatusCode::TOO_MANY_REQUESTS
    );
    while let Some(response) =
        tokio::time::timeout(Duration::from_secs(10), readers.join_next()).await?
    {
        assert_eq!(response??, StatusCode::REQUEST_TIMEOUT);
    }
    assert_eq!(
        github_opening(&f, &opening, OPENING_KEY).await?,
        StatusCode::ACCEPTED
    );
    Ok(())
}
