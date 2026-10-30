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
use yulius::wiring::git::GitAdapter;
use yulius::wiring::session::PtySessionHost;
use yulius_app::error::{AppError, AppResult};
use yulius_app::model::SandboxRecord;
use yulius_app::ports::github::GhPull;
use yulius_app::ports::isolation::{
    DiskUsage, SandboxIsolation, SandboxJoin, SandboxLimits, SandboxRef,
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
        session_host: Some(Arc::new(PtySessionHost::new(exe))),
        git: Some(Arc::new(GitAdapter::new("git"))),
        dir: Some(dir),
        configure: Some(Box::new(move |config| {
            config.ci.job_timeout = Duration::from_secs(8);
            config.ci.max_jobs = 3;
            config.contained = contained;
            config.yulius_exe = yulius_exe;
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
            head: "feat/one".into(),
            head_sha: head,
            mergeable: None,
            behind: false,
            from_fork: false,
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
            head: "feat/closed".into(),
            head_sha: git(&f.origin, &["rev-parse", "feat/closed"])?,
            mergeable: None,
            behind: false,
            from_fork: false,
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
            head: head.into(),
            head_sha: head_sha.trim().to_owned(),
            mergeable: None,
            behind: false,
            from_fork: false,
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
            head: "feat/later".into(),
            head_sha: closed_at.trim().to_owned(),
            mergeable: None,
            behind: false,
            from_fork: false,
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

    let scan = f.post("/api/sandboxes/s1/hygiene/scan", None).await?;
    assert_eq!(scan.status, StatusCode::OK, "{}", scan.body);
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
/// in dev whose pull requests the bound left unread are named in a note.
#[tokio::test]
async fn the_lookup_bound_counts_only_lookups_and_says_what_it_left() -> TestResult {
    let f = fixture().await?;
    let bound = yulius_app::ci::hygiene::MAX_PULL_LOOKUPS;
    git(&f.work, &["fetch", "-q", "origin"])?;
    // `bound` recent unmerged branches without a pull request (one commit), after feat/one,
    // whose pull request is open, and one release branch at dev's commit.
    git(&f.work, &["checkout", "-q", "-B", "wip", "origin/dev"])?;
    std::fs::write(f.work.join("wip.txt"), "x\n")?;
    git(&f.work, &["add", "-A"])?;
    git(&f.work, &["commit", "-q", "-m", "wip"])?;
    let refspecs: Vec<String> = (0..bound)
        .map(|n| format!("wip:refs/heads/zz/{n:02}"))
        .chain(std::iter::once(
            "origin/dev:refs/heads/release/1".to_owned(),
        ))
        .collect();
    let mut push = vec!["push", "-q", "origin"];
    push.extend(refspecs.iter().map(String::as_str));
    git(&f.work, &push)?;

    let scan = f.post("/api/sandboxes/s1/hygiene/scan", None).await?;
    assert_eq!(scan.status, StatusCode::OK, "{}", scan.body);
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
            .any(|n| n.contains("1 branch already in the integration branch")),
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
    let f = fixture_with(false, |data| {
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
    })
    .await?;
    let release = f.data.join("release");
    // The scan fails at once; quality and e2e are still running when it does.
    let first = f.push_feature("fail-security", Some("x\n"))?;
    f.h.app.ci_sync_sandbox(&f.sandbox).await?;
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
    refusals: AtomicU32,
}

const GB: u64 = 1_000_000_000;

impl Disk {
    fn new(used_gb: u64, refusals: u32) -> Arc<Self> {
        Arc::new(Self {
            used: AtomicU64::new(used_gb * GB),
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
    std::fs::write(f.data.join("go"), "")?;
    f.settle().await?;
    f.all_passed(2).await?;
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

/// SPEC-351 CCH-001, CCH-002 (pomerado-rust #460): two runs in the sandbox's two lanes never share
/// `$YULIUS_CI_CACHE`. Each job leaves a `live-<run>` marker in its cache and waits until both runs
/// are inside the job; with one cache shared by the lanes each found the other's marker (a Cargo
/// target directory kept there was relinked under the other run). A later run of the same
/// checkout finds its earlier run's `kept-<run>` marker: the cache still persists.
#[tokio::test]
async fn concurrent_runs_never_share_a_ci_cache() -> TestResult {
    let f = fixture_with(false, |data| {
        format!(
            "[[ci.jobs]]\nname = \"build\"\ncommand = 'c=\"$YULIUS_CI_CACHE\"; echo \"cache $c\"; \
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
    f.h.app.ci_sync_sandbox(&f.sandbox).await?;
    f.settle().await?;
    let overview = f.get("/api/sandboxes/s1/ci").await?;
    let runs = overview["runs"].as_array().ok_or("runs")?;
    assert_eq!(runs.len(), 2, "main/dev and the pull request: {overview}");
    let mut seen = Vec::new();
    for run in runs {
        let id = run["id"].as_str().ok_or("id")?;
        let detail = f.get(&format!("/api/sandboxes/s1/ci/runs/{id}")).await?;
        let output = detail["jobs"][0]["output"].as_str().unwrap_or_default();
        assert!(!output.contains("shared"), "one cache for two lanes: {output}");
        assert_eq!(run["state"], "passed", "{detail}");
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
    assert!(output.contains("kept-"), "the lane's cache persists: {output}");
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
    // The integration branch's command of the granted job, which a pull request may keep.
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
    let trusted = trusted
        .get()
        .ok_or("the fixture wrote no configuration")?
        .clone();
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

    // The pull request grants secrets in its own yulius.toml and reuses the granted job's name
    // for its own command (SPEC-264 CSG-001, CSG-002, #659): its own jobs run, and none gets a
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
    f.settle().await?;
    let outputs = job_outputs(&f, &own_sha).await?;
    let names: Vec<&str> = outputs.iter().map(|(n, _)| n.as_str()).collect();
    assert_eq!(names, ["granted", "plain", "sneaky"], "its own jobs ran");
    for name in ["granted", "plain", "sneaky"] {
        assert!(
            output_of(&outputs, name)?.contains("own key=none other=none proxy= ca=none"),
            "{outputs:?}"
        );
    }
    assert!(
        output_of(&outputs, "granted")?.contains(
            "[yulius] agent secrets: none; this job's command is not the one the integration \
             branch grants them to"
        ),
        "{outputs:?}"
    );

    // A pull request that keeps the granted job's name and command and changes another job
    // (SPEC-264 CSG-003): the trusted command keeps its grant, and only its grant.
    let kept = format!(
        "[[ci.jobs]]\nname = \"granted\"\nsecrets = \"API_KEY OTHER_KEY\"\ncommand = '{trusted}'\n\n\
         [[ci.jobs]]\nname = \"plain\"\ncommand = '{plain}'\n",
        plain = report("own "),
    );
    let kept_sha = f.push_feature("yulius.toml", Some(&kept))?;
    f.h.app.ci_sync_sandbox(&f.sandbox).await?;
    f.settle().await?;
    let outputs = job_outputs(&f, &kept_sha).await?;
    let names: Vec<&str> = outputs.iter().map(|(n, _)| n.as_str()).collect();
    assert_eq!(names, ["granted", "plain"], "its own jobs ran");
    let granted = output_of(&outputs, "granted")?;
    assert!(
        granted.contains("key=yulius-secret-API_KEY other=none proxy=set ca=/"),
        "{granted}"
    );
    assert!(
        granted.contains("[yulius] agent secrets: API_KEY"),
        "{granted}"
    );
    assert!(
        output_of(&outputs, "plain")?.contains("own key=none other=none proxy= ca=none"),
        "{outputs:?}"
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
    let own = f.push_feature(
        "yulius.toml",
        Some(
            "[[ci.jobs]]\nname = \"plain\"\ncommand = \"echo plain\"\n\n\
             [[ci.jobs]]\nname = \"security\"\ncommand = \"yulius security-scan --ci\"\n",
        ),
    )?;
    f.h.app.ci_sync_sandbox(&sandbox).await?;
    f.settle().await?;
    let outputs = job_outputs(&f, &own).await?;
    let security = output_of(&outputs, "security")?;
    assert!(
        security.contains("Vidoc is turned off for this project in Yulius"),
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
/// keeps the built-in security job; its own job named `security` does not stand in for it.
#[tokio::test]
async fn a_pull_request_with_its_own_jobs_cannot_drop_the_security_job() -> TestResult {
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
    // Production secrets are bound to no host; one from before that rule still never reaches a
    // CI job.
    crate::support::legacy_production_secret(&f.h.store, &ids["PROD_KEY"]).await?;
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
