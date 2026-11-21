//! SPEC-580: Antigravity armed green PRs parked with background `yulius wait`,
//! fresh PR conflict reporting and immediate arm drop on conflict, and bounded
//! check-in -> Needs You escalation for stalled armed green PRs.

use crate::support;

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use axum::http::{Method, StatusCode};
use serde_json::{Value, json};
use support::{
    Creds, FakeRemote, HarnessOptions, call, configure_github_app, harness_with, perms,
    seed_github_user, seed_sandbox,
};
use yulius::wiring::session::PtySessionHost;
use yulius_app::ci::{CiJobRecord, CiRunRecord};
use yulius_app::context::App;
use yulius_app::model::{AttemptRecord, LaunchKind, Sealed, TaskOrigin, TaskRecord};
use yulius_app::ports::session::{
    AttachedRun, BackgroundProbe, LaunchPlan, RunInspection, SessionHost, SpawnedRun, StrayProcess,
};
use yulius_app::ports::store::TaskStore;
use yulius_app::pr_watch::WatchBounds;
use yulius_domain::ci::{CiRef, GateSource, RunState};
use yulius_domain::harness::Harness;
use yulius_domain::ids::{AttemptId, SandboxId, TaskId, UserId};
use yulius_domain::resources::Need;
use yulius_domain::task::{Activity, AttemptState, TaskStatus};
use yulius_domain::time::Timestamp;

type BoxResult<T> = Result<T, Box<dyn std::error::Error>>;
type TestResult = BoxResult<()>;

const BUDGET: Duration = Duration::from_secs(90);

const YULIUS_TOML: &str = r#"[[ci.jobs]]
name = "check"
command = 'echo ok'
"#;

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

fn git_ok(dir: &Path, args: &[&str]) -> Option<String> {
    git(dir, args).ok()
}

struct Origin(PathBuf);

impl FakeRemote for Origin {
    fn branch_sha(&self, branch: &str) -> Option<String> {
        git_ok(
            &self.0,
            &[
                "rev-parse",
                "--verify",
                "--quiet",
                &format!("refs/heads/{branch}"),
            ],
        )
    }

    fn merge(&self, base: &str, head: &str, method: &str, message: &str) -> Result<String, String> {
        let base_sha = self.branch_sha(base).ok_or("base branch not found")?;
        let tree = git(&self.0, &["merge-tree", "--write-tree", &base_sha, head])
            .map_err(|e| e.to_string())?;
        let mut args = vec!["commit-tree", tree.as_str(), "-p", base_sha.as_str()];
        if method == "merge" {
            args.extend(["-p", head]);
        }
        args.extend(["-m", message]);
        let commit = git(&self.0, &args).map_err(|e| e.to_string())?;
        git(
            &self.0,
            &[
                "update-ref",
                &format!("refs/heads/{base}"),
                &commit,
                &base_sha,
            ],
        )
        .map_err(|e| e.to_string())?;
        Ok(commit)
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

    fn contains(&self, ancestor: &str, descendant: &str) -> Option<bool> {
        let status = Command::new("git")
            .args(["merge-base", "--is-ancestor", ancestor, descendant])
            .current_dir(&self.0)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .status()
            .ok()?;
        status.code().map(|code| code == 0)
    }
}

/// Real `PtySessionHost` wrapper whose background probe can be controlled for a synthetic live
/// task attempt PID without spawning an actual `bwrap` container.
struct ProbedHost {
    inner: PtySessionHost,
    shells: Arc<AtomicU32>,
    waits: Arc<AtomicU32>,
}

#[async_trait::async_trait]
impl SessionHost for ProbedHost {
    async fn spawn(
        &self,
        run_dir: &Path,
        attempt_id: &str,
        task_id: &str,
        plan: &LaunchPlan,
        token: &[u8; 32],
        size: (u16, u16),
    ) -> yulius_app::error::AppResult<SpawnedRun> {
        self.inner
            .spawn(run_dir, attempt_id, task_id, plan, token, size)
            .await
    }

    async fn attach(
        &self,
        run_dir: &Path,
        token: &[u8; 32],
    ) -> yulius_app::error::AppResult<AttachedRun> {
        self.inner.attach(run_dir, token).await
    }

    fn inspect(&self, run_dir: &Path) -> RunInspection {
        self.inner.inspect(run_dir)
    }

    fn terminate(&self, run_dir: &Path) -> yulius_app::error::AppResult<()> {
        self.inner.terminate(run_dir)
    }

    fn background_shells(&self, child_pid: u32) -> Option<u32> {
        if child_pid == 42_424 {
            return Some(self.shells.load(Ordering::SeqCst));
        }
        self.inner.background_shells(child_pid)
    }

    fn background_probe(&self, child_pid: u32) -> Option<BackgroundProbe> {
        if child_pid == 42_424 {
            return Some(BackgroundProbe {
                shells: self.shells.load(Ordering::SeqCst),
                waits: self.waits.load(Ordering::SeqCst),
                strays: Vec::new(),
            });
        }
        self.inner.background_probe(child_pid)
    }

    fn reap_strays(&self, strays: &[StrayProcess]) -> usize {
        self.inner.reap_strays(strays)
    }
}

fn valid_spec(task: &str) -> String {
    format!(
        "---\nyulius-task: {task}\nstatus: done\n---\n# Change {task}\n\n\
         ## Goal\nShip change {task}.\n\n\
         ## Context\nSpec 580 test fixture.\n\n\
         ## Requirements\n- It lands.\n\n\
         ## Plan\n- [x] Write the spec\n- [x] Make the change\n\n\
         ## Acceptance criteria\n- It is merged.\n\n\
         ## Test plan\n- Integration test.\n\n\
         ## Decisions\n- None.\n\n\
         ## Evidence\n- Passed.\n"
    )
}

struct Fixture {
    h: support::Harness,
    alice: Creds,
    sandbox: SandboxId,
    origin: PathBuf,
    repo: PathBuf,
    shells: Arc<AtomicU32>,
    waits: Arc<AtomicU32>,
    deadline: tokio::time::Instant,
}

async fn fixture() -> BoxResult<Fixture> {
    let deadline = tokio::time::Instant::now() + BUDGET;
    let dir = tempfile::Builder::new().prefix("yag").tempdir_in("/tmp")?;
    let root = dir.path().canonicalize()?;
    let exe = PathBuf::from(env!("CARGO_BIN_EXE_yulius"));
    let shells = Arc::new(AtomicU32::new(0));
    let waits = Arc::new(AtomicU32::new(0));
    let host = Arc::new(ProbedHost {
        inner: PtySessionHost::new(exe),
        shells: Arc::clone(&shells),
        waits: Arc::clone(&waits),
    });
    let h = harness_with(HarnessOptions {
        session_host: Some(host),
        git: Some(Arc::new(crate::support::confined_git())),
        dir: Some(dir),
        configure: Some(Box::new(|config| {
            config.ci.job_timeout = Duration::from_secs(60);
            config.ci.max_jobs = 3;
        })),
        ..HarnessOptions::default()
    })?;
    support::setup_superadmin_creds(&h).await?;
    configure_github_app(&h).await?;
    let alice = seed_github_user(&h, "u-alice", "alice").await?;
    h.github.set_perm("alice", "acme/app", perms("write"));
    let sandbox = seed_sandbox(&h, "s1", "app", "u-alice").await?;

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
    std::fs::write(work.join(".gitattributes"), "CHANGES.md merge=union\n")?;
    std::fs::write(work.join("CHANGES.md"), "# Changes\n\n- first\n")?;
    std::fs::write(work.join("shared.txt"), "base\n")?;
    git(&work, &["add", "-A"])?;
    git(&work, &["commit", "-q", "-m", "init"])?;
    git(&work, &["push", "-q", "origin", "HEAD:main", "HEAD:dev"])?;
    h.github.set_remote(Arc::new(Origin(origin.clone())));
    h.github.strict.store(true, Ordering::SeqCst);

    let data = h.app.config.data_dir.clone();
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
    git(&repo, &["config", "commit.gpgsign", "false"])?;
    git(&repo, &["config", "user.name", "Alice"])?;
    git(&repo, &["config", "user.email", "alice@yulius.test"])?;
    Ok(Fixture {
        h,
        alice,
        sandbox: sandbox.id,
        origin,
        repo,
        shells,
        waits,
        deadline,
    })
}

impl Fixture {
    async fn task_with(
        &self,
        n: u32,
        harness: Harness,
        activity: Activity,
        files: &[(&str, &str)],
    ) -> BoxResult<PathBuf> {
        let id = format!("t{n}");
        let branch = format!("feat/{n}-agy");
        let worktree = self
            .h
            .app
            .config
            .data_dir
            .join(format!("sandboxes/s1/worktrees/{id}"));
        git(&self.repo, &["fetch", "-q", "origin"])?;
        git(
            &self.repo,
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                &branch,
                worktree.to_str().ok_or("path")?,
                "origin/dev",
            ],
        )?;
        for (path, text) in files {
            let file = worktree.join(path);
            if let Some(parent) = file.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(file, text)?;
        }
        std::fs::write(
            worktree.join("CHANGES.md"),
            format!("# Changes\n\n- entry {n}\n- first\n"),
        )?;
        std::fs::create_dir_all(worktree.join("docs/specs"))?;
        std::fs::write(
            worktree.join(format!("docs/specs/{id}.md")),
            valid_spec(&id),
        )?;
        git(&worktree, &["add", "-A"])?;
        git(&worktree, &["commit", "-q", "-m", &format!("change {n}")])?;
        self.h
            .store
            .insert_task(&TaskRecord {
                parked_attempt_id: None,
                id: TaskId::parse(&id)?,
                sandbox_id: self.sandbox.clone(),
                number: n,
                title: format!("Change {n}"),
                prompt: format!("Make change {n}"),
                harness,
                model: None,
                effort: None,
                workflow_id: None,
                status: TaskStatus::Running,
                activity: Some(activity),
                branch: Some(branch),
                worktree_path: Some(worktree.to_string_lossy().into_owned()),
                spec_path: None,
                origin: TaskOrigin::Ui,
                automation_run: None,
                created_by: UserId::parse("u-alice")?,
                owner: UserId::parse("u-alice")?,
                created_at: Timestamp(1_000),
                updated_at: Timestamp(1_000),
                closed_at: None,
                archived_at: None,
                queued_at: None,
                gate_blocks: 0,
                wake_enabled: true,
                mode: yulius_app::model::TaskMode::Prompt,
                supertask: false,
            })
            .await?;
        self.h
            .store
            .insert_attempt(&AttemptRecord {
                id: AttemptId::parse(&format!("a{n}"))?,
                task_id: TaskId::parse(&id)?,
                seq: 1,
                harness,
                account_id: None,
                provider_session_id: None,
                launch_kind: LaunchKind::New,
                model: None,
                state: AttemptState::Live,
                run_dir: "/tmp/none".into(),
                token_hash: self
                    .h
                    .app
                    .deps
                    .crypto
                    .sha256_hex(format!("tok-{id}").as_bytes()),
                host_token: Sealed(vec![0]),
                ptyhost_pid: None,
                child_pid: Some(42_424),
                started_at: Timestamp(1_000),
                ended_at: None,
                exit_code: None,
                exit_reason: None,
            })
            .await?;
        let opened = call(
            &self.h.router,
            Method::POST,
            &format!("/api/tasks/{id}/delivery/pr"),
            None,
            &self.alice,
        )
        .await?;
        assert_eq!(opened.status, StatusCode::OK, "{}", opened.body);
        Ok(worktree)
    }

    async fn arm(&self, n: u32, worktree: &Path) -> TestResult {
        self.h.clock.0.fetch_add(1_000, Ordering::SeqCst);
        let head = git(worktree, &["rev-parse", "HEAD"])?;
        let result = support::mcp_tool(
            &self.h,
            &format!("tok-t{n}"),
            "delivery_merge",
            json!({"reviewed_head": head, "review_summary": "approve"}),
        )
        .await?;
        assert_eq!(support::tool_text(&result)["armed"], true, "{result}");
        Ok(())
    }

    async fn record_passed(&self, app: &App, pr: u64, head: &str) -> TestResult {
        let now = app.now();
        let id = format!("run-own-{pr}-{}", &head[..8]);
        let run = CiRunRecord {
            id: id.clone(),
            sandbox_id: self.sandbox.clone(),
            reference: CiRef::PullRequest(pr),
            sha: head.to_owned(),
            source: GateSource::ProjectConfig,
            state: RunState::Passed,
            reason: None,
            requested_by: "test".into(),
            status_for: Some(RunState::Passed),
            status_note: None,
            status_attempts: 1,
            queued_at: now,
            started_at: Some(now),
            ended_at: Some(now),
        };
        let job = CiJobRecord {
            id: format!("{id}-check"),
            run_id: id,
            ordinal: 0,
            name: "check".into(),
            key: "check".into(),
            command: "echo ok".into(),
            matrix: None,
            state: RunState::Passed,
            reason: None,
            exit_code: Some(0),
            output: "ok\n".into(),
            failures: None,
            started_at: Some(now),
            ended_at: Some(now),
            need: Need::default(),
            timeout_minutes: None,
            exclusive: false,
            waited_ms: None,
        };
        app.deps.store.insert_ci_run(&run, &[job]).await?;
        self.h.github.statuses.lock().map_err(|_| "lock")?.push((
            head.to_owned(),
            yulius_app::ports::github::NewCommitStatus {
                state: "success".into(),
                context: yulius_domain::ci::STATUS_CONTEXT.into(),
                description: "Passed".into(),
                target_url: None,
            },
        ));
        Ok(())
    }

    /// Advances `dev` directly on `origin` (optionally without fetching `self.repo`, to simulate a
    /// stale sandbox clone when another PR lands on GitHub).
    fn advance_origin_dev(
        &self,
        file: &str,
        contents: &str,
        fetch_local: bool,
    ) -> BoxResult<String> {
        let scratch = tempfile::tempdir()?;
        let clone = scratch.path().join("c");
        git(
            scratch.path(),
            &[
                "clone",
                "-q",
                "--branch",
                "dev",
                self.origin.to_str().ok_or("path")?,
                clone.to_str().ok_or("path")?,
            ],
        )?;
        std::fs::write(clone.join(file), contents)?;
        git(&clone, &["add", file])?;
        git(
            &clone,
            &["commit", "-q", "-m", &format!("dev update {file}")],
        )?;
        git(&clone, &["push", "-q", "origin", "HEAD:dev"])?;
        let sha = git(&clone, &["rev-parse", "HEAD"])?;
        if fetch_local {
            git(&self.repo, &["fetch", "-q", "origin"])?;
        }
        Ok(sha)
    }

    async fn settle(&self, app: &Arc<App>) -> TestResult {
        loop {
            app.ci_dispatch().await;
            let runs = app.deps.store.ci_runs(&self.sandbox, 100).await?;
            let busy = runs.iter().any(|r| {
                r.state.is_active() || (r.status_for != Some(r.state) && r.status_note.is_none())
            });
            if !busy {
                return Ok(());
            }
            if tokio::time::Instant::now() > self.deadline {
                return Err(format!("CI did not settle: {runs:?}").into());
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    async fn merged(&self, app: &App, n: u32) -> BoxResult<bool> {
        let id = TaskId::parse(&format!("t{n}"))?;
        let stage = app.deps.store.delivery(&id).await?.map(|r| r.stage);
        Ok(stage == Some(yulius_domain::delivery::DeliveryStage::MergedIntegration))
    }

    async fn wait_state(&self, n: u32, worktree: &Path) -> BoxResult<Value> {
        let head = git(worktree, &["rev-parse", "HEAD"])?;
        let r = call(
            &self.h.internal,
            Method::POST,
            "/internal/agent/wait",
            Some(json!({"head": head, "since": Value::Null, "timeout_secs": 1})),
            &Creds {
                bearer: Some(format!("tok-t{n}")),
                ..Creds::default()
            },
        )
        .await?;
        assert_eq!(r.status, StatusCode::OK, "{}", r.body);
        Ok(r.body)
    }

    /// Runs the real `yulius_harness::hook_client::run_hook_with` adapter against a real loopback
    /// HTTP server serving `self.h.internal` for any harness (`antigravity`, `claude`, `codex`).
    /// Uses `HoldPolicy::NEVER` and a bounded 10 s timeout so a test can never hang on a `202` hold.
    async fn run_harness_hook(
        &self,
        harness: &str,
        token: &str,
        event: &str,
        payload: Value,
    ) -> BoxResult<Value> {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let addr = listener.local_addr()?;
        let router = self.h.internal.clone();
        let server = tokio::spawn(async move {
            let _ = axum::serve(listener, router).await;
        });
        let base_url = format!("http://{addr}");
        let harness = harness.to_owned();
        let token = token.to_owned();
        let event = event.to_owned();
        let body = serde_json::to_vec(&payload)?;
        let out = tokio::time::timeout(
            Duration::from_secs(10),
            tokio::task::spawn_blocking(move || {
                let mut input = body.as_slice();
                let mut output = Vec::new();
                let code = yulius_harness::hook_client::run_hook_with(
                    &harness,
                    &event,
                    &mut input,
                    &mut output,
                    &|key| match key {
                        "YULIUS_URL" => Some(base_url.clone()),
                        "YULIUS_TASK_TOKEN" => Some(token.clone()),
                        _ => None,
                    },
                    yulius_harness::hook_client::HoldPolicy::NEVER,
                );
                (code, output)
            }),
        )
        .await
        .map_err(|_| "run_harness_hook timed out after 10s")??;
        server.abort();
        assert_eq!(out.0, 0, "hook client must exit 0");
        if out.1.is_empty() {
            return Ok(Value::Null);
        }
        Ok(serde_json::from_slice(&out.1)?)
    }

    async fn run_antigravity_hook(
        &self,
        token: &str,
        event: &str,
        payload: Value,
    ) -> BoxResult<Value> {
        self.run_harness_hook("antigravity", token, event, payload)
            .await
    }
}

/// SPEC-580 AC-1 (Case 1: quartio-v2 #338 PR #1498):
/// An Antigravity task arms `delivery_merge` on a green PR behind moved `dev`, starts
/// `yulius wait` in the background, and stops at its prompt (`Stop` with `fullyIdle: false`
/// through the real `antigravity` hook adapter). Yulius must settle the turn (`activity = waiting`),
/// show `yulius_wait = true` and `background_shells = Some(0)` (never `activity = working`),
/// pass `decidable_in`, and merge the PR through a merge train.
#[tokio::test]
async fn antigravity_armed_green_pr_with_yulius_wait_merges_and_is_not_working() -> TestResult {
    let f = fixture().await?;
    let app = f.h.app.clone();
    let worktree = f
        .task_with(
            1,
            Harness::Antigravity,
            Activity::Working,
            &[("a.txt", "x\n")],
        )
        .await?;
    let head = git(&worktree, &["rev-parse", "HEAD"])?;
    f.arm(1, &worktree).await?;
    f.record_passed(&app, 1, &head).await?;
    // Integration branch moves on while the PR is green and armed.
    f.advance_origin_dev("other.txt", "moved\n", true)?;

    // Background `yulius wait` is alive under the Antigravity agent (no non-wait shell).
    f.shells.store(0, Ordering::SeqCst);
    f.waits.store(1, Ordering::SeqCst);

    // Real Antigravity 1.5.12 Stop payload when `yulius wait` is running in background.
    f.run_antigravity_hook(
        "tok-t1",
        "Stop",
        json!({
            "conversationId": "conv-338",
            "cwd": worktree,
            "modelName": "gemini-4-neon",
            "terminationReason": "NO_TOOL_CALL",
            "continuationActive": false,
            "fullyIdle": false,
        }),
    )
    .await?;

    let id = TaskId::parse("t1")?;
    let task = app.deps.store.task(&id).await?.ok_or("t1")?;
    assert_eq!(
        task.activity,
        Some(Activity::Waiting),
        "Antigravity Stop with only yulius wait must end the turn (not stay Working)"
    );
    let summary = app.task_summary(&task).await?;
    assert_eq!(summary.activity.as_deref(), Some("waiting"));
    assert_eq!(summary.background_shells, Some(0));
    assert!(
        summary.yulius_wait,
        "must show waiting on Yulius: {summary:?}"
    );

    let sandbox = app.deps.store.sandbox(&f.sandbox).await?.ok_or("s1")?;
    assert!(
        app.decidable_in(&sandbox).await,
        "armed green PR with idle Antigravity agent parked on yulius wait must be decidable"
    );

    // Queue forms a merge train and merges PR #1 into `dev`.
    while !f.merged(&app, 1).await? {
        app.merge_armed(&f.sandbox).await;
        f.settle(&app).await?;
        if tokio::time::Instant::now() > f.deadline {
            let delivery = call(
                &f.h.router,
                Method::GET,
                "/api/tasks/t1/delivery",
                None,
                &f.alice,
            )
            .await?;
            return Err(format!("PR #1 did not merge: {}", delivery.body).into());
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    Ok(())
}

/// SPEC-580 AC-2:
/// An Antigravity Stop with `fullyIdle: false` while a NON-wait background command/subagent runs
/// (no `yulius wait`) settles `activity = Waiting` (the agent is idle at its prompt) while keeping
/// `background_shells >= 1` (`shows = working`, preserving SPEC-528 so it never falsely shows
/// `needs_you` with open steps, and holding the merge queue until the background command ends).
#[tokio::test]
async fn antigravity_stop_with_non_wait_background_work_preserves_spec_528_and_waits_for_completion()
-> TestResult {
    let f = fixture().await?;
    let app = f.h.app.clone();
    let worktree = f
        .task_with(
            1,
            Harness::Antigravity,
            Activity::Working,
            &[("a.txt", "x\n")],
        )
        .await?;
    let head = git(&worktree, &["rev-parse", "HEAD"])?;
    f.arm(1, &worktree).await?;
    f.record_passed(&app, 1, &head).await?;
    f.advance_origin_dev("other.txt", "moved\n", true)?;

    // No `yulius wait` running — `fullyIdle: false` is due to real background work (e.g. subagent
    // or non-wait background command).
    f.shells.store(0, Ordering::SeqCst);
    f.waits.store(0, Ordering::SeqCst);

    f.run_antigravity_hook(
        "tok-t1",
        "Stop",
        json!({
            "conversationId": "conv-501",
            "cwd": worktree,
            "modelName": "gemini-4-neon",
            "terminationReason": "NO_TOOL_CALL",
            "continuationActive": false,
            "fullyIdle": false,
        }),
    )
    .await?;

    let id = TaskId::parse("t1")?;
    let task = app.deps.store.task(&id).await?.ok_or("t1")?;
    assert_eq!(
        task.activity,
        Some(Activity::Waiting),
        "turn ended even with non-wait background work"
    );
    let summary = app.task_summary(&task).await?;
    assert!(
        summary.background_shells.unwrap_or(0) >= 1,
        "non-wait background work must be tracked in background_shells: {summary:?}"
    );
    let sandbox = app.deps.store.sandbox(&f.sandbox).await?.ok_or("s1")?;
    assert!(
        !app.decidable_in(&sandbox).await,
        "merge queue must wait while a real non-wait background command runs"
    );

    // Once background work finishes and Antigravity sends fullyIdle: true, the PR merges.
    f.run_antigravity_hook(
        "tok-t1",
        "Stop",
        json!({
            "conversationId": "conv-501",
            "cwd": worktree,
            "modelName": "gemini-4-neon",
            "terminationReason": "NO_TOOL_CALL",
            "continuationActive": false,
            "fullyIdle": true,
        }),
    )
    .await?;
    assert!(app.decidable_in(&sandbox).await, "decidable once fullyIdle");
    Ok(())
}

/// SPEC-580 AC-3 (Case 2: yulius #563 PR #1568):
/// When `origin/dev` advances with a real conflict against an armed green PR while the local clone
/// is still stale (unfetched) and GitHub reports `mergeable = false`, `task_delivery` must report
/// `pr.conflicts = true` and `merge_armed` must drop the arm telling the agent to resolve the
/// conflict (rather than waiting with `"as soon as your turn ends"`). A union-driver collision on
/// `CHANGES.md` must still report `pr.conflicts = false` after refresh (SPEC-491 M1).
#[tokio::test]
async fn armed_green_pr_conflict_reports_conflicts_true_and_drops_arm() -> TestResult {
    let f = fixture().await?;
    let app = f.h.app.clone();
    // PR #1 edits `shared.txt`. Keep activity = Working to prove conflict drop doesn't wait for
    // the agent's turn to end.
    let worktree = f
        .task_with(
            1,
            Harness::Antigravity,
            Activity::Working,
            &[("shared.txt", "task-1 version\n")],
        )
        .await?;
    let head = git(&worktree, &["rev-parse", "HEAD"])?;
    f.arm(1, &worktree).await?;
    f.record_passed(&app, 1, &head).await?;

    // Another PR lands on `origin/dev` conflicting with `shared.txt`, WITHOUT fetching `f.repo`
    // locally yet; GitHub already reports `mergeable = false`.
    f.advance_origin_dev("shared.txt", "conflicting dev version\n", false)?;
    if let Ok(mut pulls) = f.h.github.pulls.lock()
        && let Some(pull) = pulls.iter_mut().find(|p| p.number == 1)
    {
        pull.mergeable = Some(false);
    }

    let delivery = call(
        &f.h.router,
        Method::GET,
        "/api/tasks/t1/delivery",
        None,
        &f.alice,
    )
    .await?;
    assert_eq!(delivery.status, StatusCode::OK, "{}", delivery.body);
    assert_eq!(
        delivery.body["pr"]["conflicts"], true,
        "task_delivery must report pr.conflicts=true when origin/dev conflicts: {}",
        delivery.body
    );

    // `merge_armed` must drop the arm with actionable conflict instructions rather than waiting.
    app.merge_armed(&f.sandbox).await;
    let wait = f.wait_state(1, &worktree).await?;
    assert_eq!(
        wait["merge_armed"], false,
        "conflicting armed green PR must drop its arm instead of waiting: {wait}"
    );
    let outcome = wait["merge_outcome"].as_str().unwrap_or_default();
    assert!(
        outcome.contains("conflicts with `dev`") && outcome.contains("`shared.txt`"),
        "conflict outcome must instruct agent and name conflicting file: {outcome}"
    );
    Ok(())
}

/// SPEC-580 AC-4:
/// An armed green PR that makes no progress past the stated bound must send a check-in to the
/// agent, and if that does not move it, escalate to Needs You via SPEC-578 (`needs_you_notice`,
/// `tasks_list` / `task_get`, `wait_history`), and auto-clear when the PR later moves/merges.
#[tokio::test]
async fn stalled_armed_green_pr_checks_in_then_escalates_to_needs_you_and_auto_clears() -> TestResult
{
    let f = fixture().await?;
    let app = f.h.app.clone();
    let worktree = f
        .task_with(
            1,
            Harness::Antigravity,
            Activity::Working,
            &[("a.txt", "x\n")],
        )
        .await?;
    let head = git(&worktree, &["rev-parse", "HEAD"])?;
    f.arm(1, &worktree).await?;
    f.record_passed(&app, 1, &head).await?;
    f.advance_origin_dev("other.txt", "moved\n", true)?;

    // Dirty the worktree so the PR genuinely cannot enter a merge train or merge.
    std::fs::write(worktree.join("uncommitted.txt"), "wip\n")?;
    let id = TaskId::parse("t1")?;

    // Baseline pass starts the armed-green wait clock.
    let bounds = WatchBounds::default();
    let _ = app.watch_prs(&bounds).await;

    // Advance clock past check-in bound (10 min): check-in is triggered, not Needs You yet.
    f.h.clock.0.fetch_add(11 * 60 * 1000, Ordering::SeqCst);
    let _ = app.watch_prs(&bounds).await;
    let task = app.deps.store.task(&id).await?.ok_or("t1")?;
    let summary_before = app.task_summary(&task).await?;
    assert!(
        summary_before.needs_you.is_none(),
        "must check in before escalating to Needs You: {summary_before:?}"
    );

    // Advance clock past escalation bound (+10 min more, 21 min total): escalates to Needs You
    // using the SPEC-578 mechanism.
    f.h.clock.0.fetch_add(11 * 60 * 1000, Ordering::SeqCst);
    let _ = app.watch_prs(&bounds).await;

    let task = app.deps.store.task(&id).await?.ok_or("t1")?;
    let summary = app.task_summary(&task).await?;
    let notice = summary
        .needs_you_notice
        .as_ref()
        .ok_or("expected SPEC-578 needs_you_notice after armed-PR stall bound")?;
    assert!(
        notice.happened.contains("#1"),
        "notice must name the stalled PR: {notice:?}"
    );
    assert!(
        !notice.action.is_empty() && !notice.link.is_empty(),
        "notice must include concrete action and link: {notice:?}"
    );

    // Verify tasks_list and task_get parity (SPEC-578).
    let caller = yulius_app::coordination::ProjectCaller {
        sandbox: f.sandbox.clone(),
        user: task.owner.clone(),
        task: None,
        channel: yulius_app::coordination::Channel::Superagent,
        actor: "test viewer".into(),
        detail: json!({}),
    };
    let listed = app
        .tool_tasks_list(&caller, &json!({"scope": "needs_you"}))
        .await?;
    assert_eq!(
        listed["tasks"].as_array().map(Vec::len),
        Some(1),
        "{listed}"
    );
    let got = app.tool_task_get(&caller, &json!({"number": 1})).await?;
    assert_eq!(got["shows"], "needs_you", "{got}");
    assert!(
        got["wait_history"]
            .as_array()
            .is_some_and(|h| !h.is_empty()),
        "wait_history must record the stalled-PR Needs You episode: {got}"
    );

    // Once the PR merges (or leaves armed-green waiting), the Yulius-opened Needs You auto-clears.
    std::fs::remove_file(worktree.join("uncommitted.txt"))?;
    f.h.store
        .transition_task(
            &id,
            None,
            TaskStatus::Running,
            Some(Activity::Waiting),
            app.now(),
        )
        .await?;
    while !f.merged(&app, 1).await? {
        app.merge_armed(&f.sandbox).await;
        f.settle(&app).await?;
        if tokio::time::Instant::now() > f.deadline {
            return Err("PR #1 did not merge after cleanup".into());
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let _ = app.watch_prs(&bounds).await;
    let task_after = app.deps.store.task(&id).await?.ok_or("t1")?;
    let summary_after = app.task_summary(&task_after).await?;
    assert!(
        summary_after.needs_you.is_none() && summary_after.needs_you_notice.is_none(),
        "auto-opened armed-stall Needs You must clear once PR merges: {summary_after:?}"
    );
    Ok(())
}

/// Cross-harness parity check: Claude Code and Codex tasks with a background `yulius wait` and an
/// armed green PR behind moved `dev` settle their turn (`activity = waiting`, `background_shells = 0`,
/// `yulius_wait = true`), close `turn_in_progress`, and merge through the merge train.
#[tokio::test]
async fn claude_and_codex_armed_green_pr_with_yulius_wait_parity() -> TestResult {
    for (n, harness, hook_name) in [
        (1_u32, Harness::ClaudeCode, "claude"),
        (2_u32, Harness::Codex, "codex"),
    ] {
        let f = fixture().await?;
        let app = f.h.app.clone();
        let worktree = f
            .task_with(n, harness, Activity::Working, &[("a.txt", "x\n")])
            .await?;
        let head = git(&worktree, &["rev-parse", "HEAD"])?;
        f.arm(n, &worktree).await?;
        f.record_passed(&app, 1, &head).await?;
        f.advance_origin_dev("other.txt", "moved\n", true)?;

        f.shells.store(0, Ordering::SeqCst);
        f.waits.store(1, Ordering::SeqCst);
        let id = TaskId::parse(&format!("t{n}"))?;

        f.run_harness_hook(
            hook_name,
            &format!("tok-t{n}"),
            "Stop",
            json!({
                "session_id": format!("sess-{n}"),
                "cwd": worktree,
            }),
        )
        .await?;

        let task = app.deps.store.task(&id).await?.ok_or("task")?;
        assert_eq!(
            task.activity,
            Some(Activity::Waiting),
            "{hook_name} Stop with yulius wait must settle to Waiting"
        );
        let summary = app.task_summary(&task).await?;
        assert_eq!(summary.activity.as_deref(), Some("waiting"));
        assert_eq!(summary.background_shells, Some(0));
        assert!(
            summary.yulius_wait,
            "{hook_name} must show yulius_wait: {summary:?}"
        );

        let sandbox = app.deps.store.sandbox(&f.sandbox).await?.ok_or("s1")?;
        assert!(
            app.decidable_in(&sandbox).await,
            "{hook_name} armed green PR parked on yulius wait must be decidable"
        );

        while !f.merged(&app, n).await? {
            app.merge_armed(&f.sandbox).await;
            f.settle(&app).await?;
            if tokio::time::Instant::now() > f.deadline {
                return Err(format!("{hook_name} PR did not merge").into());
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }
    Ok(())
}

/// SPEC-580 AC-1/AC-2 combined edge case:
/// When Antigravity stops (`fullyIdle: false`) while BOTH a non-wait background shell AND
/// `yulius wait` are running (`shells == 1 && waits == 1`), Yulius holds the merge queue and
/// records `background_shells >= 1` while settling `activity = Waiting` and closing
/// `turn_in_progress`. Once the real shell exits (`shells == 0 && waits == 1`), a subsequent
/// `Stop { fullyIdle: false }` (or UserPromptSubmit) releases the hold and makes the PR decidable.
#[tokio::test]
async fn antigravity_stop_with_both_shell_and_wait_holds_until_shell_exits() -> TestResult {
    let f = fixture().await?;
    let app = f.h.app.clone();
    let worktree = f
        .task_with(
            1,
            Harness::Antigravity,
            Activity::Working,
            &[("a.txt", "x\n")],
        )
        .await?;
    let head = git(&worktree, &["rev-parse", "HEAD"])?;
    f.arm(1, &worktree).await?;
    f.record_passed(&app, 1, &head).await?;
    f.advance_origin_dev("other.txt", "moved\n", true)?;

    let id = TaskId::parse("t1")?;

    // Both a real background shell AND `yulius wait` are running.
    f.shells.store(1, Ordering::SeqCst);
    f.waits.store(1, Ordering::SeqCst);
    f.run_antigravity_hook(
        "tok-t1",
        "Stop",
        json!({
            "conversationId": "conv-both",
            "cwd": worktree,
            "modelName": "gemini-4-neon",
            "terminationReason": "NO_TOOL_CALL",
            "continuationActive": false,
            "fullyIdle": false,
        }),
    )
    .await?;

    let task = app.deps.store.task(&id).await?.ok_or("t1")?;
    assert_eq!(task.activity, Some(Activity::Waiting));
    let summary = app.task_summary(&task).await?;
    assert!(
        summary.background_shells.unwrap_or(0) >= 1,
        "must not drop real background shell just because yulius wait also runs: {summary:?}"
    );
    let sandbox = app.deps.store.sandbox(&f.sandbox).await?.ok_or("s1")?;
    assert!(
        !app.decidable_in(&sandbox).await,
        "must hold merge queue while non-wait shell still runs alongside yulius wait"
    );

    // Real shell exits; only `yulius wait` remains. `probe_idle_shells` alone (without another
    // Stop hook) must clear `background_shells` to 0 and unblock `decidable_in` (SPEC-580 F2).
    f.shells.store(0, Ordering::SeqCst);
    f.waits.store(1, Ordering::SeqCst);
    app.probe_idle_shells(&id).await;

    let task = app.deps.store.task(&id).await?.ok_or("t1")?;
    let summary = app.task_summary(&task).await?;
    assert_eq!(summary.background_shells, Some(0), "{summary:?}");
    assert!(summary.yulius_wait, "{summary:?}");
    assert!(
        app.decidable_in(&sandbox).await,
        "decidable once probe_idle_shells observes only yulius wait remains"
    );
    Ok(())
}

/// SPEC-580 AC-3 / SPEC-491 M1 preservation (plus review F3):
/// 1. When `origin/dev` advances modifying `CHANGES.md` (`merge=union`) without fetching `f.repo`
///    locally and GitHub reports `mergeable = false`, `task_delivery` refreshes the clone and
///    reports `pr.conflicts = false`.
/// 2. Once local `origin/dev` already matches `pull.base_sha`, subsequent reads (and `merge_armed`)
///    skip `dlv_refresh_clone` and keep `pr.conflicts = false` and the arm intact even if the
///    remote is temporarily unreachable.
#[tokio::test]
async fn union_driver_collision_with_stale_clone_reports_conflicts_false() -> TestResult {
    let f = fixture().await?;
    let app = f.h.app.clone();
    let worktree = f
        .task_with(
            1,
            Harness::Antigravity,
            Activity::Waiting,
            &[("a.txt", "x\n")],
        )
        .await?;
    let head = git(&worktree, &["rev-parse", "HEAD"])?;
    f.arm(1, &worktree).await?;
    f.record_passed(&app, 1, &head).await?;

    // Advance `origin/dev` on `CHANGES.md` only, WITHOUT fetching `f.repo`, and mark GitHub's
    // `mergeable = false` (GitHub ignores `.gitattributes` `merge=union`).
    let new_dev =
        f.advance_origin_dev("CHANGES.md", "# Changes\n\n- other PR\n- first\n", false)?;
    if let Ok(mut pulls) = f.h.github.pulls.lock()
        && let Some(pull) = pulls.iter_mut().find(|p| p.number == 1)
    {
        pull.mergeable = Some(false);
        pull.base_sha = new_dev;
    }

    let delivery = call(
        &f.h.router,
        Method::GET,
        "/api/tasks/t1/delivery",
        None,
        &f.alice,
    )
    .await?;
    assert_eq!(delivery.status, StatusCode::OK, "{}", delivery.body);
    assert_eq!(
        delivery.body["pr"]["conflicts"], false,
        "SPEC-491 M1 union-driver collision must still report conflicts=false after clone refresh: {}",
        delivery.body
    );

    // Local `origin/dev` now matches `pull.base_sha`: even if `origin` becomes temporarily
    // unreachable (fetch fails), `pull_conflicts` must skip fetching, keep `conflicts = false`,
    // and never drop the armed merge on a transient fetch failure (SPEC-580 review F3).
    git(
        &f.repo,
        &["remote", "set-url", "origin", "/nonexistent/origin.git"],
    )?;
    let delivery2 = call(
        &f.h.router,
        Method::GET,
        "/api/tasks/t1/delivery",
        None,
        &f.alice,
    )
    .await?;
    assert_eq!(delivery2.status, StatusCode::OK, "{}", delivery2.body);
    assert_eq!(
        delivery2.body["pr"]["conflicts"], false,
        "matching base_sha must not re-fetch or flip union-driver PR to conflicts=true: {}",
        delivery2.body
    );
    app.merge_armed(&f.sandbox).await;
    let wait = f.wait_state(1, &worktree).await?;
    assert_eq!(
        wait["merge_armed"], true,
        "transient fetch error must not drop an armed clean union-driver PR: {wait}"
    );
    Ok(())
}

/// SPEC-580 AC-4 live PTY delivery:
/// With a live `fake-agent` PTY session, an armed green PR that stays stuck past
/// `armed_check_in_after_ms` delivers the check-in message into the session's PTY first (visible on
/// screen), and if the PR still does not move past `armed_needs_you_after_ms`, escalates via
/// SPEC-578 `needs_you_notice` mentioning `"even after a check-in to the agent"`.
#[test]
fn stalled_armed_green_pr_delivers_live_pty_check_in_then_escalates() -> TestResult {
    use crate::pool_supervision::{
        ScriptedUsage, build_served_tuned, new_task, runtime, screen, seed, stop, temp, wait_for,
    };
    use yulius_app::delivery::DeliveryRecord;
    use yulius_app::model::Secret;
    use yulius_app::ports::github::{GitHubPort, NewPull};
    use yulius_domain::delivery::DeliveryStage;
    use yulius_ptyhost::test_support::locate_fake_agent;

    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let (_dir, data) = temp()?;
    let github = Arc::new(crate::support::FakeGitHub::default());
    github.set_remote(Arc::new(Origin(data.join("origin.git"))));
    github.add_user("code-admin", 1, "admin");
    github.set_perm("admin", "acme/pool", perms("write"));
    let agent = crate::support::agents::FakeAgentOptions {
        transcript: true,
        answers_first_message: true,
        ..crate::support::agents::FakeAgentOptions::default()
    };
    let rt = runtime()?;
    rt.block_on(async {
        let app = build_served_tuned(
            &data,
            &fake,
            Arc::new(ScriptedUsage::default()),
            agent,
            github.clone(),
            "http://127.0.0.1:9",
            |config| config.goal_ready_ms = 500,
        )?;
        let (sandbox, user) = seed(&app, &data, &["a-only"]).await?;
        app.deps
            .store
            .init_instance("https://yulius.test", "hash")
            .await?;
        let cfg = json!({"app_id": 1, "slug": "yulius-test", "client_id": "c",
            "client_secret": "s", "private_key_pem": "k", "webhook_secret": null});
        let sealed = app.deps.crypto.seal(cfg.to_string().as_bytes())?;
        app.deps
            .store
            .set_github_app(&sealed, "acme", "org")
            .await?;
        let seed_dir = data.join("seed");
        std::fs::write(seed_dir.join("yulius.toml"), YULIUS_TOML)?;
        git(&seed_dir, &["add", "yulius.toml"])?;
        git(&seed_dir, &["commit", "-q", "-m", "ci config"])?;
        git(&seed_dir, &["push", "-q", "origin", "HEAD:dev"])?;
        let repo = data.join("sandboxes").join(sandbox.as_str()).join("repo");
        git(&repo, &["fetch", "-q", "origin"])?;

        let id = new_task(&app, &sandbox, &user, "ArmedStall").await?;
        wait_for("first answer", Duration::from_secs(30), || {
            let shown = screen(&app, &id);
            async move { shown.contains("ECHO: ArmedStall: answer with an echo") }
        })
        .await?;

        let task = app.deps.store.task(&id).await?.ok_or("task")?;
        let branch = task.branch.clone().ok_or("branch")?;
        let worktree = PathBuf::from(task.worktree_path.as_deref().ok_or("worktree")?);
        std::fs::write(worktree.join("README.md"), "# armed stall line\n")?;
        git(&worktree, &["commit", "-q", "-am", "task"])?;
        git(&worktree, &["push", "-q", "origin", &branch])?;
        let head = git(&worktree, &["rev-parse", "HEAD"])?;
        let pull = github
            .create_pull(
                &Secret("token-admin".into()),
                "acme",
                "pool",
                &NewPull {
                    title: "feat: armed stall".into(),
                    body: "test".into(),
                    head: branch,
                    base: "dev".into(),
                },
            )
            .await?;
        let pr = pull.number;
        // Dirty worktree so the armed green PR cannot merge.
        std::fs::write(worktree.join("wip.txt"), "uncommitted\n")?;

        let open = DeliveryRecord {
            task_id: id.clone(),
            stage: DeliveryStage::PrOpen,
            pr_number: Some(pr),
            updated_at: app.now(),
        };
        app.deps
            .store
            .transition_delivery(&DeliveryStage::None, &open)
            .await?;

        // Record green CI run at `head` and arm `delivery_merge`.
        let at = app.now();
        let run = CiRunRecord {
            id: "r-green".into(),
            sandbox_id: sandbox.clone(),
            reference: CiRef::PullRequest(pr),
            sha: head.clone(),
            source: GateSource::ProjectConfig,
            state: RunState::Passed,
            reason: None,
            requested_by: "test".into(),
            status_for: Some(RunState::Passed),
            status_note: None,
            status_attempts: 1,
            queued_at: at,
            started_at: Some(at),
            ended_at: Some(at),
        };
        let job = CiJobRecord {
            id: "r-green-check".into(),
            run_id: "r-green".into(),
            ordinal: 0,
            name: "check".into(),
            key: "check".into(),
            command: "echo ok".into(),
            matrix: None,
            state: RunState::Passed,
            reason: None,
            exit_code: Some(0),
            output: "ok\n".into(),
            failures: None,
            started_at: Some(at),
            ended_at: Some(at),
            need: Need::default(),
            timeout_minutes: None,
            exclusive: false,
            waited_ms: None,
        };
        app.deps.store.insert_ci_run(&run, &[job]).await?;
        let armed_path = app.config.data_dir.join("armed-merges.json");
        std::fs::write(
            &armed_path,
            serde_json::to_vec(&json!([{
                "task": id.as_str(),
                "head": head,
                "summary": "approve",
                "armed_at": at.0,
            }]))?,
        )?;
        app.restore_armed_merges().await;

        // Use a generous escalation bound during the check-in loop so heavy CI load cannot skip
        // past `CheckIn` straight to `Escalate` before the PTY quiet window opens (SPEC-580 F6).
        let check_in_bounds = WatchBounds {
            check_in_after_ms: 60 * 60 * 1000,
            stall_after_ms: 60 * 60 * 1000,
            stall_quiet_ms: 5 * 60 * 1000,
            armed_check_in_after_ms: 300,
            armed_needs_you_after_ms: 60_000,
        };

        // Pass 1 starts the armed-green clock (no message yet).
        let typed1 = app.watch_prs(&check_in_bounds).await;
        assert!(typed1.is_empty(), "first pass starts clock: {typed1:?}");

        // Wait past `armed_check_in_after_ms` and until the live PTY is quiet/ready for input:
        // check-in is typed into the live PTY session.
        let check_in_deadline = tokio::time::Instant::now() + Duration::from_secs(15);
        let sent = loop {
            tokio::time::sleep(Duration::from_millis(150)).await;
            let typed = app.watch_prs(&check_in_bounds).await;
            if let Some((_, text)) = typed.into_iter().find(|(t, _)| *t == id) {
                break text;
            }
            if tokio::time::Instant::now() > check_in_deadline {
                let notices = std::fs::read_to_string(app.config.data_dir.join("pr-notices.json"))
                    .unwrap_or_default();
                let t = app.deps.store.task(&id).await?;
                let s = match &t {
                    Some(t) => Some(app.task_summary(t).await?),
                    None => None,
                };
                return Err(format!(
                    "expected armed-PR check-in to be typed into live session within 15s (notices={notices}, summary={s:?})"
                )
                .into());
            }
        };
        assert!(
            sent.contains("has been green for") && sent.contains("Your armed merge"),
            "unexpected check-in text: {sent}"
        );
        wait_for("check-in echoed on screen", Duration::from_secs(15), || {
            let shown = screen(&app, &id);
            async move { shown.contains("Your armed merge") }
        })
        .await?;
        let task_mid = app.deps.store.task(&id).await?.ok_or("task")?;
        let summary_mid = app.task_summary(&task_mid).await?;
        assert!(
            summary_mid.needs_you_notice.is_none(),
            "must not escalate on the check-in pass: {summary_mid:?}"
        );

        // Now tighten `armed_needs_you_after_ms` and wait past it without resolving: escalates
        // cleanly to Needs You.
        let escalate_bounds = WatchBounds {
            armed_needs_you_after_ms: 400,
            ..check_in_bounds
        };
        tokio::time::sleep(Duration::from_millis(500)).await;
        let typed3 = app.watch_prs(&escalate_bounds).await;
        assert!(
            typed3.is_empty(),
            "escalation pass must not send an extra generic check-in: {typed3:?}"
        );
        let task_end = app.deps.store.task(&id).await?.ok_or("task")?;
        let summary_end = app.task_summary(&task_end).await?;
        let notice = summary_end
            .needs_you_notice
            .as_ref()
            .ok_or("expected Needs You escalation after unanswered check-in")?;
        assert!(
            notice.happened.contains(&format!("#{pr}"))
                && notice
                    .happened
                    .contains("even after a check-in to the agent"),
            "notice must record the unanswered check-in: {notice:?}"
        );

        let _ = stop(&app, &id).await;
        Ok(())
    })
}

/// SPEC-580 review F1:
/// An active `/internal/agent/wait` long-poll alone (even when `host.background_probe` reports
/// `waits = 0, shells = 0`, e.g. on platforms or wrapper trees without `/proc` visibility) lets
/// Antigravity `Stop { fullyIdle: false }` settle cleanly (`activity = Waiting`, `yulius_wait =
/// true`, `decidable_in = true`), and drops `yulius_wait` back to `false` once the long-poll
/// finishes.
#[tokio::test]
async fn antigravity_stop_with_active_agent_wait_rpc_settles_without_probe_waits() -> TestResult {
    let f = fixture().await?;
    let app = f.h.app.clone();
    let worktree = f
        .task_with(
            1,
            Harness::Antigravity,
            Activity::Working,
            &[("a.txt", "x\n")],
        )
        .await?;
    let head = git(&worktree, &["rev-parse", "HEAD"])?;
    f.arm(1, &worktree).await?;
    f.record_passed(&app, 1, &head).await?;
    f.advance_origin_dev("other.txt", "moved\n", true)?;

    // Process probe sees neither shells nor waits: only the live `/internal/agent/wait` RPC proves
    // `yulius wait` is active.
    f.shells.store(0, Ordering::SeqCst);
    f.waits.store(0, Ordering::SeqCst);

    let initial = f.wait_state(1, &worktree).await?;
    let internal = f.h.internal.clone();
    let head_poll = head.clone();
    let waiter = tokio::spawn(async move {
        call(
            &internal,
            Method::POST,
            "/internal/agent/wait",
            Some(json!({"head": head_poll, "since": initial, "timeout_secs": 3})),
            &Creds {
                bearer: Some("tok-t1".into()),
                ..Creds::default()
            },
        )
        .await
        .map_err(|e| e.to_string())
    });

    // Wait (bounded) until the long-poll enters `agent_wait` and registers `active_waits`.
    let id = TaskId::parse("t1")?;
    let entered_deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        if app.active_waits_of(&id) > 0 {
            break;
        }
        if tokio::time::Instant::now() > entered_deadline {
            return Err("agent_wait long-poll did not register active_waits within 5s".into());
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }

    f.run_antigravity_hook(
        "tok-t1",
        "Stop",
        json!({
            "conversationId": "conv-rpc-wait",
            "cwd": worktree,
            "modelName": "gemini-4-neon",
            "terminationReason": "NO_TOOL_CALL",
            "continuationActive": false,
            "fullyIdle": false,
        }),
    )
    .await?;

    let task = app.deps.store.task(&id).await?.ok_or("t1")?;
    assert_eq!(task.activity, Some(Activity::Waiting));
    let summary = app.task_summary(&task).await?;
    assert_eq!(summary.background_shells, Some(0), "{summary:?}");
    assert!(summary.yulius_wait, "{summary:?}");
    let sandbox = app.deps.store.sandbox(&f.sandbox).await?.ok_or("s1")?;
    assert!(
        app.decidable_in(&sandbox).await,
        "decidable while active_waits is registered on Stop"
    );

    // Wait (bounded) for the 3s long-poll to return and verify `ActiveWaitGuard` drops cleanly
    // back to 0 (never underflowing).
    let res = tokio::time::timeout(Duration::from_secs(10), waiter)
        .await
        .map_err(|_| "agent_wait long-poll did not return within 10s")???;
    assert_eq!(res.status, StatusCode::OK, "{}", res.body);
    assert_eq!(
        app.active_waits_of(&id),
        0,
        "active_waits must return to 0 once the long-poll finishes"
    );
    Ok(())
}

/// SPEC-580 review F4:
/// When a dirty-worktree armed green PR is merged directly on GitHub while local delivery is still
/// `PrOpen`, `watch_prs` checks GitHub before escalating and must NOT falsely open a Needs You
/// notice for an already-merged PR.
#[tokio::test]
async fn stalled_armed_green_pr_already_merged_on_github_does_not_escalate() -> TestResult {
    let f = fixture().await?;
    let app = f.h.app.clone();
    let worktree = f
        .task_with(
            1,
            Harness::Antigravity,
            Activity::Working,
            &[("a.txt", "x\n")],
        )
        .await?;
    let head = git(&worktree, &["rev-parse", "HEAD"])?;
    f.arm(1, &worktree).await?;
    f.record_passed(&app, 1, &head).await?;
    f.advance_origin_dev("other.txt", "moved\n", true)?;

    // Dirty worktree prevents automatic local merge, so local delivery remains `PrOpen`.
    std::fs::write(worktree.join("wip.txt"), "uncommitted\n")?;
    let id = TaskId::parse("t1")?;
    let bounds = WatchBounds::default();
    let _ = app.watch_prs(&bounds).await;

    // A person merges the PR directly on GitHub.
    if let Ok(mut pulls) = f.h.github.pulls.lock()
        && let Some(pull) = pulls.iter_mut().find(|p| p.number == 1)
    {
        pull.state = "closed".into();
        pull.merged = true;
    }

    // Advance past both check-in (10m) and escalation (20m) bounds.
    f.h.clock.0.fetch_add(25 * 60 * 1000, Ordering::SeqCst);
    let _ = app.watch_prs(&bounds).await;

    let task = app.deps.store.task(&id).await?.ok_or("t1")?;
    let summary = app.task_summary(&task).await?;
    assert!(
        summary.needs_you.is_none() && summary.needs_you_notice.is_none(),
        "already-merged PR on GitHub must not trigger armed-stall Needs You: {summary:?}"
    );
    Ok(())
}

/// SPEC-580 merge review F2:
/// When a stalled armed green PR disarms and the unarmed `watch_prs` pass defers via
/// `observed_only()` (e.g. GitHub installation lookup unavailable while checking a dropped
/// singleton), all four armed-stall fields (`armed_green_key`, `armed_green_since_ms`,
/// `armed_check_in_ms`, `armed_needs_you_at`) must clear together so re-arming the same head
/// starts a fresh 10+10 min cycle rather than immediately re-escalating to Needs You.
#[tokio::test]
async fn disarmed_pr_on_deferred_pass_resets_stall_clock_before_rearm() -> TestResult {
    let f = fixture().await?;
    let app = f.h.app.clone();
    let worktree = f
        .task_with(
            1,
            Harness::Antigravity,
            Activity::Working,
            &[("a.txt", "x\n")],
        )
        .await?;
    let head = git(&worktree, &["rev-parse", "HEAD"])?;
    f.arm(1, &worktree).await?;
    f.record_passed(&app, 1, &head).await?;
    f.advance_origin_dev("other.txt", "moved\n", true)?;

    // Dirty worktree holds the PR outside a merge train; advance past 20m so it escalates.
    std::fs::write(worktree.join("wip.txt"), "uncommitted\n")?;
    let id = TaskId::parse("t1")?;
    let bounds = WatchBounds::default();
    let _ = app.watch_prs(&bounds).await;
    f.h.clock.0.fetch_add(25 * 60 * 1000, Ordering::SeqCst);
    let _ = app.watch_prs(&bounds).await;
    let task = app.deps.store.task(&id).await?.ok_or("t1")?;
    assert!(
        app.task_summary(&task).await?.needs_you_notice.is_some(),
        "precondition: stalled armed PR escalated to Needs You"
    );

    // Disarm the PR via a real `merge_armed` drop (wrong base branch), then restore `base = dev`.
    if let Ok(mut pulls) = f.h.github.pulls.lock()
        && let Some(pull) = pulls.iter_mut().find(|p| p.number == 1)
    {
        pull.base = "main".into();
    }
    app.merge_armed(&f.sandbox).await;
    if let Ok(mut pulls) = f.h.github.pulls.lock()
        && let Some(pull) = pulls.iter_mut().find(|p| p.number == 1)
    {
        pull.base = "dev".into();
    }
    let wait = f.wait_state(1, &worktree).await?;
    assert_eq!(wait["merge_armed"], false, "PR must be disarmed: {wait}");
    let outcome = wait["merge_outcome"]
        .as_str()
        .ok_or("expected merge_outcome after disarm")?
        .to_owned();

    // Record a valid dropped-singleton `("wait", "merge_armed")` span matching the dropped head
    // and outcome so `SingletonDecision::from_span` returns `Ok(Some(decision))` and the unarmed
    // pass enters `hosted_owner_pull` -> `return observed_only()` when GitHub is unavailable.
    f.h.clock.0.fetch_add(1_000, Ordering::SeqCst);
    let now = app.now();
    app.deps
        .store
        .insert_span(&yulius_app::stages::NewSpan {
            sandbox_id: f.sandbox.clone(),
            subject_kind: yulius_domain::stages::SubjectKind::Task,
            subject_id: id.as_str().to_owned(),
            key: format!("merge_armed:{}", now.0),
            parent_key: None,
            kind: "wait".into(),
            name: "merge_armed".into(),
            environment: None,
            job: None,
            started_at: now,
            ended_at: Some(now),
            outcome: Some("dropped".into()),
            detail: Some(
                json!({
                    "head": head,
                    "message": outcome,
                    "singleton": {
                        "pr": 1,
                        "base": "base-sha",
                        "tree": "tree-sha",
                        "run": "run-singleton-1"
                    }
                })
                .to_string(),
            ),
        })
        .await?;

    // Force `if !armed` to defer via `observed_only()` (GitHub unavailable during `hosted_owner_pull`).
    f.h.github.no_installation.store(true, Ordering::SeqCst);
    let deferred = app.watch_prs(&bounds).await;
    f.h.github.no_installation.store(false, Ordering::SeqCst);
    assert!(
        deferred.is_empty(),
        "deferred unarmed pass must not type a message: {deferred:?}"
    );

    let notices: Value = serde_json::from_str(&std::fs::read_to_string(
        app.config.data_dir.join("pr-notices.json"),
    )?)?;
    assert!(
        notices["t1"]["progress"]
            .as_str()
            .is_some_and(|p| p.ends_with("|armed")),
        "precondition: unarmed pass must have deferred via observed_only() (keeping prior `|armed` progress): {notices}"
    );
    assert!(
        notices["t1"]["keys"].get("outcome").is_none(),
        "precondition: unarmed pass must have deferred via observed_only() without recording outcome: {notices}"
    );
    assert_eq!(
        notices["t1"]["armed_green_key"], "",
        "deferred unarmed pass must clear armed_green_key: {notices}"
    );
    assert_eq!(
        notices["t1"]["armed_green_since_ms"], 0,
        "deferred unarmed pass must clear armed_green_since_ms: {notices}"
    );
    assert_eq!(
        notices["t1"]["armed_check_in_ms"], 0,
        "deferred unarmed pass must clear armed_check_in_ms: {notices}"
    );
    assert_eq!(
        notices["t1"]["armed_needs_you_at"], 0,
        "deferred unarmed pass must clear armed_needs_you_at: {notices}"
    );

    // Re-arm the exact same head and advance 1 minute: must start a fresh clock, NOT immediately
    // re-escalate to Needs You.
    f.arm(1, &worktree).await?;
    let _ = app.watch_prs(&bounds).await;
    f.h.clock.0.fetch_add(60 * 1000, Ordering::SeqCst);
    let _ = app.watch_prs(&bounds).await;
    let task_after = app.deps.store.task(&id).await?.ok_or("t1")?;
    let summary_after = app.task_summary(&task_after).await?;
    assert!(
        summary_after.needs_you.is_none() && summary_after.needs_you_notice.is_none(),
        "re-arming the same head after a deferred disarm must not immediately re-escalate: {summary_after:?}"
    );
    Ok(())
}
