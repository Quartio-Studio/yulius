//! Yulius-run CI end to end (SPEC-001 DLV-006/007, AC-DLV-4/5; SPEC-002 ADR-18): a real
//! repository with `[[ci.jobs]]` on the integration branch, the real Git adapter, the real
//! detached ptyhost running the jobs, and the fake GitHub recording commit statuses and branch
//! deletions.

mod support;

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::time::Duration;

use axum::http::{Method, StatusCode};
use serde_json::Value;
use support::{
    Creds, FakeRemote, HarnessOptions, call, configure_github_app, harness_with, perms,
    seed_github_user, seed_sandbox,
};
use yulius::wiring::git::GitAdapter;
use yulius::wiring::session::PtySessionHost;
use yulius_app::model::SandboxRecord;
use yulius_app::ports::github::GhPull;
use yulius_domain::ci::{GateResult, RunState};

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
    // A short base: job sockets live at <data>/runs/ci-<uuid>/pty.sock (Unix path limit).
    let dir = tempfile::Builder::new().prefix("yci").tempdir_in("/tmp")?;
    let root = dir.path().canonicalize()?;
    let exe = PathBuf::from(env!("CARGO_BIN_EXE_yulius"));
    let h = harness_with(HarnessOptions {
        session_host: Some(Arc::new(PtySessionHost::new(exe))),
        git: Some(Arc::new(GitAdapter::new("git"))),
        dir: Some(dir),
        configure: Some(Box::new(|config| {
            config.ci.job_timeout = Duration::from_secs(8);
            config.ci.max_jobs = 3;
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
    std::fs::write(work.join("yulius.toml"), YULIUS_TOML)?;
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

    /// Dispatches until no run is queued or running (the background loops do not run here).
    async fn settle(&self) -> BoxResult<()> {
        let deadline = tokio::time::Instant::now() + WAIT;
        loop {
            self.h.app.ci_dispatch().await;
            let overview = self.get("/api/sandboxes/s1/ci").await?;
            let active = overview["runs"]
                .as_array()
                .ok_or("runs")?
                .iter()
                .any(|r| r["state"] == "queued" || r["state"] == "running");
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

#[tokio::test]
async fn hygiene_lists_and_cleans_up_stale_branches() -> TestResult {
    let f = fixture().await?;
    // feat/old is merged into dev; feat/closed had a pull request closed without merging;
    // feat/one has an open pull request; feat/wip is recent work without a pull request.
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
        });
    }
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
