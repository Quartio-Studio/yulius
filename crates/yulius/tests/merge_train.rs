//! Merge trains end to end (SPEC-084): real repositories (a bare `origin` the fake GitHub merges
//! in, Yulius's clone and the tasks' worktrees), the production Git adapter, real Yulius CI runs
//! through the detached process host, and a fake GitHub that enforces a ruleset like Yulius's
//! own: the `yulius/ci` status at success and the base in the head ("up to date") for every
//! merge.

use crate::support;

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use axum::http::{Method, StatusCode};
use serde_json::{Value, json};
use support::{
    Creds, FakeRemote, HarnessOptions, call, configure_github_app, harness_with, perms,
    seed_github_user, seed_sandbox,
};
use yulius::wiring::git::GitAdapter;
use yulius::wiring::session::PtySessionHost;
use yulius_app::ci::CiRunRecord;
use yulius_app::context::App;
use yulius_app::merge_train::MERGE_TRAINS_FILE;
use yulius_app::model::{AttemptRecord, LaunchKind, Sealed, TaskOrigin, TaskRecord};
use yulius_app::ports::store::TaskStore;
use yulius_domain::ci::{CiRef, RunState};
use yulius_domain::delivery::DeliveryStage;
use yulius_domain::harness::Harness;
use yulius_domain::ids::{AttemptId, SandboxId, TaskId, UserId};
use yulius_domain::task::{Activity, AttemptState, TaskStatus};
use yulius_domain::time::Timestamp;

type BoxResult<T> = Result<T, Box<dyn std::error::Error>>;
type TestResult = BoxResult<()>;

const WAIT: Duration = Duration::from_secs(50);

/// One job: two pull requests that each add a `clash-*` file pass alone and fail together (the
/// kind of conflict no merge shows, F 2026-10-13: two records taking one number).
const YULIUS_TOML: &str = r#"[[ci.jobs]]
name = "check"
command = 'if [ -f clash-1 ] && [ -f clash-2 ]; then echo "clash-1 and clash-2 together"; exit 1; fi; echo ok'
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

/// GitHub's repository side: the bare `origin`. Merges land as GitHub lands them: the tree of
/// merging the head into the base, as one squash commit or a merge commit.
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

fn valid_spec(task: &str) -> String {
    format!(
        "---\nyulius-task: {task}\nstatus: in-progress\n---\n# Change {task}\n\n\
         ## Goal\nShip change {task}.\n\n\
         ## Context\nParallel tasks deliver at once.\n\n\
         ## Requirements\n- It lands.\n\n\
         ## Plan\n- [x] Write the spec\n- [x] Make the change\n\n\
         ## Acceptance criteria\n- It is merged.\n\n\
         ## Test plan\n- The merge train test.\n\n\
         ## Decisions\n- None yet.\n"
    )
}

struct Fixture {
    h: support::Harness,
    alice: Creds,
    sandbox: SandboxId,
    origin: PathBuf,
    repo: PathBuf,
}

async fn fixture() -> BoxResult<Fixture> {
    // A short base: job sockets live at <data>/runs/ci-<uuid>/pty.sock (Unix path limit).
    let dir = tempfile::Builder::new().prefix("ytr").tempdir_in("/tmp")?;
    let root = dir.path().canonicalize()?;
    let exe = PathBuf::from(env!("CARGO_BIN_EXE_yulius"));
    let _ = Command::new(&exe).arg("version").output();
    let h = harness_with(HarnessOptions {
        session_host: Some(Arc::new(PtySessionHost::new(exe))),
        git: Some(Arc::new(GitAdapter::new("git"))),
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

    // origin: main == dev with the CI, a union-merged changes log and a shared file.
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
    // The machine's own Git configuration must not sign or rename anything here.
    git(&repo, &["config", "commit.gpgsign", "false"])?;
    git(&repo, &["config", "user.name", "Alice"])?;
    git(&repo, &["config", "user.email", "alice@yulius.test"])?;
    Ok(Fixture {
        h,
        alice,
        sandbox: sandbox.id,
        origin,
        repo,
    })
}

impl Fixture {
    /// Task `t<n>`: a running, idle task of Alice's whose worktree commits `files`, a changes
    /// entry and its spec, with its pull request open (number `n`).
    async fn task(&self, n: u32, files: &[(&str, &str)]) -> BoxResult<PathBuf> {
        let id = format!("t{n}");
        let branch = format!("feat/{n}-train");
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
            std::fs::write(worktree.join(path), text)?;
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
                id: TaskId::parse(&id)?,
                sandbox_id: self.sandbox.clone(),
                number: 0,
                title: format!("Change {n}"),
                prompt: format!("Make change {n}"),
                harness: Harness::ClaudeCode,
                model: None,
                effort: None,
                workflow_id: None,
                status: TaskStatus::Running,
                activity: Some(Activity::Waiting),
                branch: Some(branch),
                worktree_path: Some(worktree.to_string_lossy().into_owned()),
                spec_path: None,
                origin: TaskOrigin::Ui,
                automation_run: None,
                created_by: UserId::parse("u-alice")?,
                owner: UserId::parse("u-alice")?,
                created_at: Timestamp(1),
                updated_at: Timestamp(1),
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
                harness: Harness::ClaudeCode,
                account_id: None,
                provider_session_id: None,
                launch_kind: LaunchKind::New,
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
                child_pid: None,
                started_at: Timestamp(1),
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
        assert_eq!(opened.body["pr"]["number"], n, "{}", opened.body);
        Ok(worktree)
    }

    fn agent(id: &str) -> Creds {
        Creds {
            bearer: Some(format!("tok-{id}")),
            ..Creds::default()
        }
    }

    /// The agent of `t<n>` arms the merge of its head (`delivery_merge` while its checks run);
    /// a second later on the clock than the arm before.
    async fn arm(&self, n: u32, worktree: &Path) -> TestResult {
        self.h.clock.0.fetch_add(1000, Ordering::SeqCst);
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

    /// `yulius wait` of `t<n>` at its head: the state now.
    async fn wait(&self, n: u32, worktree: &Path) -> BoxResult<Value> {
        let head = git(worktree, &["rev-parse", "HEAD"])?;
        let r = call(
            &self.h.internal,
            Method::POST,
            "/internal/agent/wait",
            Some(json!({"head": head, "since": Value::Null, "timeout_secs": 1})),
            &Self::agent(&format!("t{n}")),
        )
        .await?;
        assert_eq!(r.status, StatusCode::OK, "{}", r.body);
        Ok(r.body)
    }

    async fn runs(&self, app: &App) -> BoxResult<Vec<CiRunRecord>> {
        Ok(app.deps.store.ci_runs(&self.sandbox, 200).await?)
    }

    /// The runs of merge trains.
    async fn train_runs(&self, app: &App) -> BoxResult<Vec<CiRunRecord>> {
        Ok(self
            .runs(app)
            .await?
            .into_iter()
            .filter(|r| matches!(r.reference, CiRef::Train(_)))
            .collect())
    }

    /// Runs that really ran jobs (not a result reused or recorded from a train).
    async fn executed(&self, app: &App) -> BoxResult<Vec<CiRunRecord>> {
        Ok(self
            .runs(app)
            .await?
            .into_iter()
            .filter(|r| {
                r.started_at.is_some()
                    && !r
                        .reason
                        .as_deref()
                        .is_some_and(|reason| reason.starts_with("not run again"))
            })
            .collect())
    }

    /// Dispatches `app`'s CI until no run is queued or running and every ended run's commit
    /// status was published (or has a note why not).
    async fn settle(&self, app: &Arc<App>) -> TestResult {
        let deadline = tokio::time::Instant::now() + WAIT;
        loop {
            app.ci_dispatch().await;
            let runs = self.runs(app).await?;
            let busy = runs.iter().any(|r| {
                r.state.is_active() || (r.status_for != Some(r.state) && r.status_note.is_none())
            });
            if !busy {
                return Ok(());
            }
            if tokio::time::Instant::now() > deadline {
                return Err(format!("CI did not settle: {runs:?}").into());
            }
            tokio::time::sleep(Duration::from_millis(150)).await;
        }
    }

    /// Commits `file` on `dev` of the origin, as another merged pull request would.
    fn advance_dev(&self, file: &str) -> TestResult {
        let other = tempfile::tempdir()?;
        let clone = other.path().join("c");
        git(
            other.path(),
            &[
                "clone",
                "-q",
                "--branch",
                "dev",
                self.origin.to_str().ok_or("path")?,
                clone.to_str().ok_or("path")?,
            ],
        )?;
        std::fs::write(clone.join(file), "other\n")?;
        git(&clone, &["add", "-A"])?;
        git(&clone, &["commit", "-q", "-m", file])?;
        git(&clone, &["push", "-q", "origin", "HEAD:dev"])?;
        Ok(())
    }

    /// Records a passed run of pull request `n` at `head` with its `yulius/ci` status published,
    /// as if it had run (no run ends, so no pass starts by itself).
    async fn record_passed(&self, app: &App, n: u32, head: &str) -> TestResult {
        let now = app.now();
        app.deps
            .store
            .insert_ci_run(
                &CiRunRecord {
                    id: format!("run-{n}-{}", &head[..7]),
                    sandbox_id: self.sandbox.clone(),
                    reference: CiRef::PullRequest(u64::from(n)),
                    sha: head.to_owned(),
                    source: yulius_domain::ci::GateSource::ProjectConfig,
                    state: RunState::Passed,
                    reason: None,
                    requested_by: "yulius".into(),
                    status_for: Some(RunState::Passed),
                    status_note: None,
                    status_attempts: 1,
                    queued_at: now,
                    started_at: Some(now),
                    ended_at: Some(now),
                },
                &[],
            )
            .await?;
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

    async fn merged(&self, app: &App, n: u32) -> BoxResult<bool> {
        Ok(app
            .deps
            .store
            .delivery(&TaskId::parse(&format!("t{n}"))?)
            .await?
            .is_some_and(|record| record.stage != DeliveryStage::PrOpen))
    }

    /// Lets `app` decide armed merges (as the end of a CI run and the delivery loop do) until
    /// `done` holds; merges the end of a run started in the background may already be deciding.
    async fn decide_until<F, Fut>(&self, app: &Arc<App>, done: F) -> TestResult
    where
        F: Fn() -> Fut,
        Fut: std::future::Future<Output = BoxResult<bool>>,
    {
        let deadline = tokio::time::Instant::now() + WAIT;
        loop {
            app.merge_armed(&self.sandbox).await;
            if done().await? {
                return Ok(());
            }
            if tokio::time::Instant::now() > deadline {
                let runs: Vec<String> = self
                    .runs(app)
                    .await?
                    .iter()
                    .map(|r| {
                        format!(
                            "{} {} {:?} {:?}",
                            r.reference.key(),
                            &r.sha[..9],
                            r.state,
                            r.reason
                        )
                    })
                    .collect();
                return Err(format!(
                    "not decided: {}\nruns: {runs:#?}\nwrites: {:?}",
                    self.trains_file(app),
                    self.h.github.writes()
                )
                .into());
            }
            tokio::time::sleep(Duration::from_millis(150)).await;
        }
    }

    fn trains_file(&self, app: &App) -> String {
        std::fs::read_to_string(app.config.data_dir.join(MERGE_TRAINS_FILE)).unwrap_or_default()
    }

    fn origin_tree(&self, rev: &str) -> BoxResult<String> {
        git(&self.origin, &["rev-parse", &format!("{rev}^{{tree}}")])
    }

    fn merges(&self) -> Vec<String> {
        self.h
            .github
            .writes()
            .into_iter()
            .filter(|w| w.starts_with("merge_pull"))
            .collect()
    }

    fn status_of(&self, sha: &str) -> Option<String> {
        self.h
            .github
            .statuses()
            .into_iter()
            .rev()
            .find(|(s, _)| s == sha)
            .map(|(_, status)| status.state)
    }
}

/// A new server process over the same data directory, database and GitHub: nothing of the
/// last one's memory, armed merges put back from their file (DLV-205).
async fn restart(app: &Arc<App>) -> Arc<App> {
    let next = App::new(app.config.clone(), app.deps.clone());
    next.restore_armed_merges().await;
    next
}

/// TRN-001..005: three armed pull requests are tested together by one run and merged one after
/// another on it. Each is brought up to date with exactly the tree the train predicted (the
/// union-merged changes log included), the train's verdict is published on that head, so the
/// strict ruleset accepts every merge without another run, and the integration branch ends with
/// the tested tree, which its own CI then reuses.
#[tokio::test]
async fn armed_pull_requests_merge_together_on_one_run() -> TestResult {
    let f = fixture().await?;
    let app = f.h.app.clone();
    let mut worktrees = Vec::new();
    for (n, file) in [(1, "a.txt"), (2, "b.txt"), (3, "c.txt")] {
        worktrees.push(f.task(n, &[(file, "x\n")]).await?);
    }
    // The heads' own runs, as the CI loop queues them.
    app.ci_sync_sandbox(&app.deps.store.sandbox(&f.sandbox).await?.ok_or("s1")?)
        .await?;
    for (n, worktree) in (1..).zip(&worktrees) {
        f.arm(n, worktree).await?;
    }
    app.merge_armed(&f.sandbox).await;
    let trains = f.train_runs(&app).await?;
    assert_eq!(trains.len(), 1, "{trains:?}");
    let train = trains[0].clone();
    assert_eq!(train.reference.key(), "train/1+2+3");
    // It goes ahead of the queued runs: it decides all three merges.
    let queued = f.runs(&app).await?;
    assert!(
        queued
            .iter()
            .filter(|r| r.id != train.id)
            .all(|r| r.state == RunState::Queued && r.queued_at > train.queued_at),
        "{queued:?}"
    );
    let note = f.wait(2, &worktrees[1]).await?["merge_train"]
        .as_str()
        .unwrap_or_default()
        .to_owned();
    assert!(
        note.contains("#2 is number 2 of 3 in merge train #1, #2, #3"),
        "{note}"
    );
    f.settle(&app).await?;
    assert_eq!(
        app.deps.store.ci_run(&train.id).await?.ok_or("run")?.state,
        RunState::Passed
    );
    f.decide_until(&app, || async {
        Ok(f.merged(&app, 1).await? && f.merged(&app, 2).await? && f.merged(&app, 3).await?)
    })
    .await?;

    assert_eq!(
        f.merges(),
        [
            "merge_pull #1 squash",
            "merge_pull #2 squash",
            "merge_pull #3 squash"
        ]
    );
    // The integration branch has exactly the tested tree, union entries and all.
    assert_eq!(
        f.origin_tree("dev")?,
        git(&f.repo, &["rev-parse", &format!("{}^{{tree}}", train.sha)])?
    );
    let log = git(&f.origin, &["show", "dev:CHANGES.md"])?;
    for entry in ["- entry 1", "- entry 2", "- entry 3", "- first"] {
        assert!(log.contains(entry), "{log}");
    }
    // One run per head and one for the train; none for the heads brought up to date.
    let executed = f.executed(&app).await?;
    let prs = executed
        .iter()
        .filter(|r| matches!(r.reference, CiRef::PullRequest(_)))
        .count();
    let trains = executed
        .iter()
        .filter(|r| matches!(r.reference, CiRef::Train(_)))
        .count();
    assert_eq!((prs, trains), (3, 1), "{executed:?}");
    for (n, worktree) in [(2, &worktrees[1]), (3, &worktrees[2])] {
        let head = git(worktree, &["rev-parse", "HEAD"])?;
        let runs = app.deps.store.ci_runs_for_sha(&f.sandbox, &head).await?;
        let run = runs.first().ok_or("verdict")?;
        assert_eq!(run.state, RunState::Passed, "{run:?}");
        assert!(
            run.reason
                .as_deref()
                .is_some_and(|r| r.contains("merge train #1, #2, #3 passed")),
            "{run:?}"
        );
        assert_eq!(f.status_of(&head).as_deref(), Some("success"), "#{n}");
        let state = f.wait(n, worktree).await?;
        let outcome = state["merge_outcome"].as_str().unwrap_or_default();
        assert!(
            outcome.starts_with(&format!("Yulius merged pull request #{n} into `dev`"))
                && outcome.contains("in a merge train"),
            "{state} {}",
            f.trains_file(&app)
        );
    }
    // The integration branch's own CI reuses the result for its tree (the train's).
    app.ci_sync_sandbox(&app.deps.store.sandbox(&f.sandbox).await?.ok_or("s1")?)
        .await?;
    let dev = git(&f.origin, &["rev-parse", "dev"])?;
    let dev_run = app
        .deps
        .store
        .ci_runs_for_sha(&f.sandbox, &dev)
        .await?
        .into_iter()
        .next()
        .ok_or("dev run")?;
    assert!(
        dev_run.state == RunState::Passed
            && dev_run
                .reason
                .as_deref()
                .is_some_and(|r| r.starts_with("not run again")),
        "{dev_run:?}"
    );
    assert!(!f.trains_file(&app).contains("\"train\":{"), "train over");
    Ok(())
}

/// TRN-006: a train whose combined result fails changes nothing: every member merges one by
/// one as before (each tested on its own, behind ones brought up to date), and no new train
/// forms from them until their agents arm again.
#[tokio::test]
async fn a_red_train_falls_back_to_one_by_one_merges() -> TestResult {
    let f = fixture().await?;
    let app = f.h.app.clone();
    let mut worktrees = Vec::new();
    for (n, file) in [(1, "clash-1"), (2, "clash-2"), (3, "c.txt")] {
        worktrees.push(f.task(n, &[(file, "x\n")]).await?);
    }
    for (n, worktree) in (1..).zip(&worktrees) {
        f.arm(n, worktree).await?;
    }
    app.merge_armed(&f.sandbox).await;
    app.ci_sync_sandbox(&app.deps.store.sandbox(&f.sandbox).await?.ok_or("s1")?)
        .await?;
    f.settle(&app).await?;
    let train = f.train_runs(&app).await?.first().cloned().ok_or("train")?;
    assert_eq!(train.state, RunState::Failed, "{train:?}");

    // One by one, as before: the first green and up to date merges on its own run; the others
    // are brought up to date and run again, each on its own.
    let sandbox = app.deps.store.sandbox(&f.sandbox).await?.ok_or("s1")?;
    let deadline = tokio::time::Instant::now() + WAIT;
    let failed = |n: u32| {
        let f = &f;
        let worktree = worktrees[n as usize - 1].clone();
        async move {
            Ok::<_, Box<dyn std::error::Error>>(
                f.wait(n, &worktree).await?["merge_outcome"]
                    .as_str()
                    .unwrap_or_default()
                    .contains(&format!("the checks of pull request #{n} failed")),
            )
        }
    };
    let (first, second) = loop {
        app.merge_armed(&f.sandbox).await;
        app.ci_sync_sandbox(&sandbox).await?;
        f.settle(&app).await?;
        app.merge_armed(&f.sandbox).await;
        let (one, two) = (f.merged(&app, 1).await?, f.merged(&app, 2).await?);
        if f.merged(&app, 3).await? && one != two {
            let pair = if one { (1, 2) } else { (2, 1) };
            if failed(pair.1).await? {
                break pair;
            }
        }
        if tokio::time::Instant::now() > deadline {
            return Err(format!("{:?}: {}", f.merges(), f.trains_file(&app)).into());
        }
        tokio::time::sleep(Duration::from_millis(150)).await;
    };
    // The clashing pair: the one merged second fails on top of the first, alone; the third
    // passes. No second train was formed from them.
    assert_eq!(f.train_runs(&app).await?.len(), 1);
    assert!(!f.merged(&app, second).await?);
    let merges = f.merges();
    assert_eq!(
        merges
            .iter()
            .filter(|m| m.contains(&format!("#{first} ")))
            .count(),
        1,
        "{merges:?}"
    );
    assert_eq!(merges.len(), 2, "#{first} and #3 only: {merges:?}");
    Ok(())
}

/// TRN-003/007: an armed pull request that conflicts on top of the ones armed before it is left
/// out of their train and told so; it waits until they merged, then the one-by-one flow tells it
/// what to resolve. The others merge on the train's run.
#[tokio::test]
async fn a_pull_request_that_conflicts_in_the_train_is_left_out() -> TestResult {
    let f = fixture().await?;
    let app = f.h.app.clone();
    let mut worktrees = Vec::new();
    for (n, file, text) in [
        (1, "shared.txt", "one\n"),
        (2, "b.txt", "x\n"),
        (3, "shared.txt", "three\n"),
    ] {
        worktrees.push(f.task(n, &[(file, text)]).await?);
    }
    for (n, worktree) in (1..).zip(&worktrees) {
        f.arm(n, worktree).await?;
    }
    app.merge_armed(&f.sandbox).await;
    let train = f.train_runs(&app).await?.first().cloned().ok_or("train")?;
    assert_eq!(train.reference.key(), "train/1+2");
    let state = f.wait(3, &worktrees[2]).await?;
    let note = state["merge_train"].as_str().unwrap_or_default();
    assert!(
        note.contains("#3 was left out of merge train #1, #2") && note.contains("`shared.txt`"),
        "{state}"
    );
    assert_eq!(state["merge_armed"], true, "still armed: {state}");
    let third = git(&worktrees[2], &["rev-parse", "HEAD"])?;

    app.ci_sync_sandbox(&app.deps.store.sandbox(&f.sandbox).await?.ok_or("s1")?)
        .await?;
    f.settle(&app).await?;
    f.decide_until(&app, || async {
        Ok(f.merged(&app, 1).await?
            && f.merged(&app, 2).await?
            && f.wait(3, &worktrees[2]).await?["merge_armed"] == false)
    })
    .await?;
    // #3 passed alone and was up to date, yet #1 and #2 (armed first) went first.
    assert_eq!(f.merges(), ["merge_pull #1 squash", "merge_pull #2 squash"]);
    assert_eq!(
        f.origin_tree("dev")?,
        git(&f.repo, &["rev-parse", &format!("{}^{{tree}}", train.sha)])?
    );
    let outcome = f.wait(3, &worktrees[2]).await?["merge_outcome"]
        .as_str()
        .unwrap_or_default()
        .to_owned();
    assert!(
        outcome.contains("conflicts with `dev`") && outcome.contains("`shared.txt`"),
        "{outcome}"
    );
    assert_eq!(
        git(&worktrees[2], &["rev-parse", "HEAD"])?,
        third,
        "aborted"
    );
    Ok(())
}

/// TRN-010: passes that decide armed merges at the same time (two runs ending together) take
/// turns. Two green, up-to-date pull requests: one merges, the other is brought up to date. It
/// used to be merged by the other pass from a stale "up to date", refused by the strict ruleset
/// as behind, and its armed merge dropped.
#[tokio::test]
async fn passes_at_the_same_time_take_turns() -> TestResult {
    let f = fixture().await?;
    let app = f.h.app.clone();
    let mut worktrees = Vec::new();
    for (n, file) in [(1, "a.txt"), (2, "b.txt")] {
        worktrees.push(f.task(n, &[(file, "x\n")]).await?);
    }
    for (n, worktree) in (1..).zip(&worktrees) {
        f.arm(n, worktree).await?;
    }
    let heads: Vec<String> = worktrees
        .iter()
        .map(|w| git(w, &["rev-parse", "HEAD"]))
        .collect::<Result<_, _>>()?;
    // Both heads' runs passed and were published (recorded here: no run ends to start a pass).
    for (n, head) in (1..).zip(&heads) {
        f.record_passed(&app, n, head).await?;
    }
    tokio::join!(app.merge_armed(&f.sandbox), app.merge_armed(&f.sandbox));
    assert_eq!(f.merges().len(), 1, "{:?}", f.merges());
    let (merged, other) = if f.merged(&app, 1).await? {
        (1, 2)
    } else {
        (2, 1)
    };
    let state = f.wait(other, &worktrees[other as usize - 1]).await?;
    assert_eq!(state["merge_armed"], true, "not dropped: {state}");
    assert_ne!(
        git(&f.origin, &["rev-parse", &format!("feat/{other}-train")])?,
        heads[other as usize - 1],
        "#{other} brought up to date after #{merged}"
    );
    assert!(
        f.train_runs(&app).await?.is_empty(),
        "both could merge alone"
    );
    Ok(())
}

/// TRN-011: an armed pull request whose agent is busy while its base moved on is not brought up
/// to date (DLV-203 waits for an idle agent); it now waits too, instead of being merged, refused
/// by the ruleset as behind and dropped. Once the agent is idle it is brought up to date.
#[tokio::test]
async fn a_busy_agent_behind_its_base_waits_instead_of_being_refused() -> TestResult {
    let f = fixture().await?;
    let app = f.h.app.clone();
    let worktree = f.task(1, &[("a.txt", "x\n")]).await?;
    f.arm(1, &worktree).await?;
    let head = git(&worktree, &["rev-parse", "HEAD"])?;
    f.record_passed(&app, 1, &head).await?;
    // Another pull request merged meanwhile; Yulius's CI round fetched it.
    f.advance_dev("other.txt")?;
    app.ci_sync_sandbox(&app.deps.store.sandbox(&f.sandbox).await?.ok_or("s1")?)
        .await?;
    let task = TaskId::parse("t1")?;
    f.h.store
        .transition_task(
            &task,
            None,
            TaskStatus::Running,
            Some(Activity::Working),
            app.now(),
        )
        .await?;

    app.merge_armed(&f.sandbox).await;
    let state = f.wait(1, &worktree).await?;
    assert_eq!(state["merge_armed"], true, "waits: {state}");
    assert!(f.merges().is_empty(), "{:?}", f.merges());

    f.h.store
        .transition_task(
            &task,
            None,
            TaskStatus::Running,
            Some(Activity::Waiting),
            app.now(),
        )
        .await?;
    app.merge_armed(&f.sandbox).await;
    assert_ne!(
        git(&f.origin, &["rev-parse", "feat/1-train"])?,
        head,
        "brought up to date once idle"
    );
    assert_eq!(f.wait(1, &worktree).await?["merge_armed"], true);
    Ok(())
}

/// TRN-012: an agent that merges the integration branch into its armed branch itself, cleanly,
/// keeps its armed merge: Git's own clean merges keep the review (SPEC-083 REV-001), so the
/// merge follows the new head instead of being dropped as "the branch moved on".
#[tokio::test]
async fn an_armed_merge_follows_the_agents_own_clean_merge_of_dev() -> TestResult {
    let f = fixture().await?;
    let app = f.h.app.clone();
    let worktree = f.task(1, &[("a.txt", "x\n")]).await?;
    f.arm(1, &worktree).await?;
    let armed = git(&worktree, &["rev-parse", "HEAD"])?;
    f.advance_dev("other.txt")?;
    git(&worktree, &["fetch", "-q", "origin"])?;
    git(&worktree, &["merge", "-q", "--no-edit", "origin/dev"])?;
    let merged = git(&worktree, &["rev-parse", "HEAD"])?;
    assert_ne!(merged, armed);

    app.merge_armed(&f.sandbox).await;
    let state = f.wait(1, &worktree).await?;
    assert_eq!(state["merge_armed"], true, "kept: {state}");
    assert_eq!(state["merge_outcome"], Value::Null, "{state}");
    let saved = std::fs::read_to_string(
        app.config
            .data_dir
            .join(yulius_app::agent_wait::ARMED_MERGES_FILE),
    )?;
    assert!(saved.contains(&merged), "follows the new head: {saved}");
    Ok(())
}

/// TRN-008: a train survives restarts, before its run and halfway through its merges: the next
/// process runs it, then picks up the merges where they stopped (GitHub had not shown a pushed
/// head yet) from the train's file and the armed merges' (DLV-205), without another run.
#[tokio::test]
async fn a_merge_train_survives_a_restart() -> TestResult {
    let f = fixture().await?;
    let mut worktrees = Vec::new();
    for (n, file) in [(1, "a.txt"), (2, "b.txt")] {
        worktrees.push(f.task(n, &[(file, "x\n")]).await?);
    }
    for (n, worktree) in (1..).zip(&worktrees) {
        f.arm(n, worktree).await?;
    }
    f.h.app.merge_armed(&f.sandbox).await;
    let train = f
        .train_runs(&f.h.app)
        .await?
        .first()
        .cloned()
        .ok_or("train")?;
    assert_eq!(train.state, RunState::Queued);

    // Restart before the run.
    let second = restart(&f.h.app).await;
    second
        .ci_sync_sandbox(&second.deps.store.sandbox(&f.sandbox).await?.ok_or("s1")?)
        .await?;
    // GitHub will not show #2's new head before the next restart.
    *f.h.github.stale_reads.lock().map_err(|_| "lock")? = u32::MAX;
    let armed_head = git(&worktrees[1], &["rev-parse", "HEAD"])?;
    f.settle(&second).await?;
    // #1 merged; #2 brought up to date and pushed, its merge waiting for GitHub.
    f.decide_until(&second, || async {
        Ok(f.merged(&second, 1).await?
            && git(&f.origin, &["rev-parse", "feat/2-train"])? != armed_head
            && f.trains_file(&second).contains("\"waiting_since\":1"))
    })
    .await?;
    assert!(!f.merged(&second, 2).await?);
    let moved = git(&worktrees[1], &["rev-parse", "HEAD"])?;
    assert_eq!(
        git(&f.origin, &["rev-parse", "feat/2-train"])?,
        moved,
        "brought up to date and pushed"
    );

    // Restart halfway through the merges; GitHub catches up.
    let third = restart(&second).await;
    *f.h.github.stale_reads.lock().map_err(|_| "lock")? = 0;
    f.decide_until(&third, || async { f.merged(&third, 2).await })
        .await?;
    // A last pass ends the train (the one before the restart may still have been polling).
    third.merge_armed(&f.sandbox).await;
    assert_eq!(f.merges(), ["merge_pull #1 squash", "merge_pull #2 squash"]);
    assert_eq!(
        f.origin_tree("dev")?,
        git(&f.repo, &["rev-parse", &format!("{}^{{tree}}", train.sha)])?
    );
    let executed = f.executed(&third).await?;
    assert_eq!(
        executed
            .iter()
            .filter(|r| matches!(r.reference, CiRef::Train(_)))
            .count(),
        1,
        "{executed:?}"
    );
    assert!(
        executed.iter().all(|r| r.sha != moved),
        "no run at the head brought up to date: {executed:?}"
    );
    assert!(!f.trains_file(&third).contains("\"train\":{"), "train over");
    Ok(())
}
