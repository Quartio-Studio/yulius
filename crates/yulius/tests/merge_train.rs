//! Merge trains end to end (SPEC-084): real repositories (a bare `origin` the fake GitHub merges
//! in, Yulius's clone and the tasks' worktrees), the production Git adapter, real Yulius CI runs
//! through the detached process host, and a fake GitHub that enforces a ruleset like Yulius's
//! own: the `yulius/ci` status at success and the base in the head ("up to date") for every
//! merge.

use crate::support;
use crate::support::git;

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
use yulius::wiring::session::PtySessionHost;
use yulius_app::ci::CiRunRecord;
use yulius_app::context::App;
use yulius_app::merge_train::MERGE_TRAINS_FILE;
use yulius_app::model::{AttemptRecord, LaunchKind, Sealed, TaskOrigin, TaskRecord};
use yulius_app::ports::store::{TaskStore, UserStore};
use yulius_domain::ci::{CiRef, RunState};
use yulius_domain::delivery::DeliveryStage;
use yulius_domain::harness::Harness;
use yulius_domain::ids::{AttemptId, SandboxId, TaskId, UserId};
use yulius_domain::task::{Activity, AttemptState, TaskStatus};
use yulius_domain::time::Timestamp;

type BoxResult<T> = Result<T, Box<dyn std::error::Error>>;
type TestResult = BoxResult<()>;

/// The usual fixture shares one deadline from its start (SPEC-140 TWT-002): nextest kills it
/// at 120 s (`slow-timeout` 30 s × `terminate-after` 4, `.config/nextest.toml`), and a test that
/// misses its deadline 20 s before reports what it saw. The red train test takes 9 to 10 s alone
/// and 37 to 41 s in 8 copies at once beside 24 busy loops (load 34 to 79).
const BUDGET: Duration = Duration::from_secs(100);
// The two four/five-head scenarios exceeded 100 s on a shard slowed from 4 to 11.7 minutes.
// Keep one deadline for all their conditions and 30 s to report before nextest intervenes.
const MULTI_HEAD_BUDGET: Duration = Duration::from_secs(300);

/// One job: two pull requests that each add a `clash-*` file pass alone and fail together (the
/// kind of conflict no merge shows, F 2026-10-13: two records taking one number).
const YULIUS_TOML: &str = r#"[[ci.jobs]]
name = "check"
command = 'if [ -f clash-1 ] && [ -f clash-2 ]; then echo "clash-1 and clash-2 together"; exit 1; fi; echo ok'
"#;

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

/// The spec of a task that finished its work: `done`, every Plan item ticked, Evidence (a task
/// whose spec does not say done is not Done, SPEC-491 DON-004).
fn valid_spec(task: &str) -> String {
    format!(
        "---\nyulius-task: {task}\nstatus: done\n---\n# Change {task}\n\n\
         ## Goal\nShip change {task}.\n\n\
         ## Context\nParallel tasks deliver at once.\n\n\
         ## Requirements\n- It lands.\n\n\
         ## Plan\n- [x] Write the spec\n- [x] Make the change\n\n\
         ## Acceptance criteria\n- It is merged.\n\n\
         ## Test plan\n- The merge train test.\n\n\
         ## Decisions\n- None yet.\n\n\
         ## Evidence\n- The gate: passed.\n"
    )
}

/// The project a fixture sets up (SPEC-441 MQ-010: nothing assumes one).
#[derive(Clone, Copy, PartialEq, Eq)]
enum Project {
    /// `dev` with squash merges, a union-merged changes log and a job two pull requests break.
    Standard,
    /// `trunk` with merge commits, no changes log and one job that always passes.
    Minimal,
    /// [`Project::Standard`] with a second job that runs only for changes under `never/`
    /// (SPEC-497 MQW-001: a run that skipped it is no source for a candidate's verdict).
    Paths,
    /// [`Project::Standard`] whose `[merge_queue]` holds changes under `gen/` apart (SPEC-497
    /// MQW-003).
    Apart,
}

struct Fixture {
    h: support::Harness,
    /// The integration branch.
    integration: &'static str,
    project: Project,
    /// When the test's waits give up ([`BUDGET`] from its start).
    deadline: tokio::time::Instant,
    alice: Creds,
    sandbox: SandboxId,
    origin: PathBuf,
    repo: PathBuf,
}

async fn fixture() -> BoxResult<Fixture> {
    fixture_with(Project::Standard, None, BUDGET).await
}

/// A fixture of `project`, its CI with `lanes` lanes per sandbox when given.
async fn fixture_with(
    project: Project,
    lanes: Option<u32>,
    budget: Duration,
) -> BoxResult<Fixture> {
    let deadline = tokio::time::Instant::now() + budget;
    // A short base: job sockets live at <data>/runs/ci-<uuid>/pty.sock (Unix path limit).
    let dir = tempfile::Builder::new().prefix("ytr").tempdir_in("/tmp")?;
    let root = dir.path().canonicalize()?;
    let exe = PathBuf::from(env!("CARGO_BIN_EXE_yulius"));
    let _ = Command::new(&exe).arg("version").output();
    let h = harness_with(HarnessOptions {
        session_host: Some(Arc::new(PtySessionHost::new(exe))),
        git: Some(Arc::new(crate::support::confined_git())),
        dir: Some(dir),
        configure: Some(Box::new(move |config| {
            config.ci.job_timeout = if budget == MULTI_HEAD_BUDGET {
                budget
            } else {
                Duration::from_secs(60)
            };
            config.ci.max_jobs = 3;
            if let Some(lanes) = lanes {
                config.ci.lanes = lanes;
            }
        })),
        ..HarnessOptions::default()
    })?;
    support::setup_superadmin_creds(&h).await?;
    configure_github_app(&h).await?;
    let alice = seed_github_user(&h, "u-alice", "alice").await?;
    h.github.set_perm("alice", "acme/app", perms("write"));
    let sandbox = seed_sandbox(&h, "s1", "app", "u-alice").await?;
    let integration = match project {
        Project::Standard | Project::Paths | Project::Apart => "dev",
        Project::Minimal => {
            let mut profile = yulius_domain::delivery::DeliveryProfile::standard();
            profile.integration_branch = "trunk".into();
            profile.merge = yulius_domain::delivery::MergeMethod::Merge;
            use yulius_app::ports::store::SandboxStore;
            h.store
                .set_profile(
                    &sandbox.id,
                    &serde_json::to_string(&yulius_app::delivery::profile_to_contract(&profile))?,
                )
                .await?;
            "trunk"
        }
    };

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
    match project {
        Project::Standard | Project::Paths | Project::Apart => {
            let toml = if project == Project::Apart {
                format!("{YULIUS_TOML}\n[merge_queue]\napart = [\"gen/\"]\n")
            } else if project == Project::Paths {
                format!(
                    "{YULIUS_TOML}\n[[ci.jobs]]\nname = \"never\"\ncommand = \"echo never\"\n\
                     paths = [\"never/**\"]\n"
                )
            } else {
                YULIUS_TOML.to_owned()
            };
            std::fs::write(work.join("yulius.toml"), toml)?;
            std::fs::write(work.join(".gitattributes"), "CHANGES.md merge=union\n")?;
            std::fs::write(work.join("CHANGES.md"), "# Changes\n\n- first\n")?;
            std::fs::write(work.join("shared.txt"), "base\n")?;
        }
        Project::Minimal => {
            std::fs::write(
                work.join("yulius.toml"),
                "[[ci.jobs]]\nname = \"test\"\ncommand = \"echo ok\"\n",
            )?;
        }
    }
    git(&work, &["add", "-A"])?;
    git(&work, &["commit", "-q", "-m", "init"])?;
    git(
        &work,
        &[
            "push",
            "-q",
            "origin",
            "HEAD:main",
            &format!("HEAD:{integration}"),
        ],
    )?;
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
        integration,
        project,
        deadline,
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
                &format!("origin/{}", self.integration),
            ],
        )?;
        for (path, text) in files {
            let file = worktree.join(path);
            if let Some(parent) = file.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(file, text)?;
        }
        if self.project != Project::Minimal {
            std::fs::write(
                worktree.join("CHANGES.md"),
                format!("# Changes\n\n- entry {n}\n- first\n"),
            )?;
        }
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
                resume_attempt_id: None,
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

    /// Runs that really ran jobs (not a result reused or recorded from a train, nor one
    /// [`Self::record_passed`] recorded).
    async fn executed(&self, app: &App) -> BoxResult<Vec<CiRunRecord>> {
        Ok(self
            .runs(app)
            .await?
            .into_iter()
            .filter(|r| {
                r.started_at.is_some()
                    && !r.id.starts_with("run-")
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
        loop {
            app.ci_dispatch().await;
            let runs = self.runs(app).await?;
            let busy = runs.iter().any(|r| {
                r.state.is_active() || (r.status_for != Some(r.state) && r.status_note.is_none())
            });
            if !busy {
                return Ok(());
            }
            if tokio::time::Instant::now() > self.deadline {
                return Err(format!("CI did not settle: {runs:?}").into());
            }
            tokio::time::sleep(Duration::from_millis(150)).await;
        }
    }

    /// [`Self::settle`], leaving run `held` running (a test holds it).
    async fn settle_but(&self, app: &Arc<App>, held: &str) -> TestResult {
        loop {
            app.ci_dispatch().await;
            let runs = self.runs(app).await?;
            let busy = runs.iter().filter(|r| r.id != held).any(|r| {
                r.state.is_active() || (r.status_for != Some(r.state) && r.status_note.is_none())
            });
            if !busy {
                return Ok(());
            }
            if tokio::time::Instant::now() > self.deadline {
                return Err(format!("CI did not settle: {runs:?}").into());
            }
            tokio::time::sleep(Duration::from_millis(150)).await;
        }
    }

    /// Commits `file` on the integration branch of the origin, as another merged pull request
    /// would.
    fn advance_dev(&self, file: &str) -> TestResult {
        self.advance_dev_with(file, "other\n")
    }

    /// [`Self::advance_dev`] with `contents` in `file`.
    fn advance_dev_with(&self, file: &str, contents: &str) -> TestResult {
        let other = tempfile::tempdir()?;
        let clone = other.path().join("c");
        git(
            other.path(),
            &[
                "clone",
                "-q",
                "--branch",
                self.integration,
                self.origin.to_str().ok_or("path")?,
                clone.to_str().ok_or("path")?,
            ],
        )?;
        std::fs::write(clone.join(file), contents)?;
        git(&clone, &["add", "-A"])?;
        git(&clone, &["commit", "-q", "-m", file])?;
        git(
            &clone,
            &[
                "push",
                "-q",
                "origin",
                &format!("HEAD:{}", self.integration),
            ],
        )?;
        Ok(())
    }

    /// Records a passed run of pull request `n` at `head` with its `yulius/ci` status published,
    /// as if it had run (no run ends, so no pass starts by itself).
    async fn record_passed(&self, app: &App, n: u32, head: &str) -> TestResult {
        self.record_run(app, n, head, RunState::Passed).await
    }

    /// As [`Self::record_passed`], ended in `state`.
    async fn record_run(&self, app: &App, n: u32, head: &str, state: RunState) -> TestResult {
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
                    state,
                    reason: None,
                    requested_by: "yulius".into(),
                    status_for: Some(state),
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
                state: if state == RunState::Passed {
                    "success"
                } else {
                    "error"
                }
                .into(),
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
        loop {
            app.merge_armed(&self.sandbox).await;
            if done().await? {
                return Ok(());
            }
            if tokio::time::Instant::now() > self.deadline {
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

/// TRN-001..005, SPEC-441 MQ-003: three armed pull requests whose own runs passed, and whose
/// base moved: the one still up to date merges at once, and the other two are tested together by
/// one run and merged one after another on it. Each is brought up to date with exactly the tree
/// the train predicted (the union-merged changes log included), the train's verdict is published
/// on that head, so the strict ruleset accepts every merge without another run, and the
/// integration branch ends with the tested tree, which its own CI then reuses.
#[tokio::test]
async fn armed_pull_requests_merge_together_on_one_run() -> TestResult {
    let f = fixture().await?;
    let app = f.h.app.clone();
    let mut worktrees = Vec::new();
    for (n, file) in [(1, "a.txt"), (2, "b.txt"), (3, "c.txt")] {
        worktrees.push(f.task(n, &[(file, "x\n")]).await?);
    }
    for (n, worktree) in (1..).zip(&worktrees) {
        f.arm(n, worktree).await?;
    }
    // #3 delivers a part of its plan: one slice is still ahead (SPEC-484 PLN-003).
    {
        use yulius_app::model::StepRecord;
        use yulius_domain::ids::StepId;
        let task = TaskId::parse("t3")?;
        let steps = [("This slice", "done"), ("Later slice", "pending")]
            .iter()
            .zip(1..)
            .map(|((title, status), ordinal)| {
                Ok(StepRecord {
                    id: StepId::parse(&format!("t3-step-{ordinal}"))?,
                    task_id: task.clone(),
                    ordinal,
                    title: (*title).to_owned(),
                    status: (*status).to_owned(),
                    note: None,
                    updated_by: None,
                    updated_at: None,
                    effort: 1,
                })
            })
            .collect::<BoxResult<Vec<_>>>()?;
        app.deps
            .store
            .replace_steps(&task, &steps, "test", Timestamp(2))
            .await?;
    }
    // The heads' own runs passed (recorded: no run ends to start a pass).
    for (n, worktree) in (1..).zip(&worktrees) {
        f.record_passed(&app, n, &git(worktree, &["rev-parse", "HEAD"])?)
            .await?;
    }
    // #1 is up to date: it merges on its own, first.
    app.merge_armed(&f.sandbox).await;
    assert_eq!(f.merges(), ["merge_pull #1 squash"]);
    assert!(f.train_runs(&app).await?.is_empty());
    let first_landing = git(&f.origin, &["rev-parse", "dev"])?;
    let first_head = git(&worktrees[0], &["rev-parse", "HEAD"])?;
    let first = f.wait(1, &worktrees[0]).await?;
    let outcome = first["merge_outcome"].as_str().ok_or("merge outcome")?;
    assert!(
        outcome.contains(&format!("into `dev` at {}", &first_landing[..9])),
        "{outcome}"
    );
    assert!(
        outcome.contains(&format!("PR head {}", &first_head[..9])),
        "{outcome}"
    );
    app.merge_armed(&f.sandbox).await;
    let trains = f.train_runs(&app).await?;
    assert_eq!(trains.len(), 1, "{trains:?}");
    let train = trains[0].clone();
    assert_eq!(train.reference.key(), "train/2+3");
    let note = f.wait(3, &worktrees[2]).await?["merge_train"]
        .as_str()
        .unwrap_or_default()
        .to_owned();
    assert!(
        note.contains("#3 is number 2 of 2 in merge train #2, #3"),
        "{note}"
    );
    f.settle(&app).await?;
    assert_eq!(
        app.deps.store.ci_run(&train.id).await?.ok_or("run")?.state,
        RunState::Passed
    );
    f.decide_until(&app, || async {
        Ok(f.merged(&app, 2).await? && f.merged(&app, 3).await?)
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
    // One run for the train; none for the heads brought up to date.
    let executed = f.executed(&app).await?;
    let prs = executed
        .iter()
        .filter(|r| matches!(r.reference, CiRef::PullRequest(_)))
        .count();
    let trains = executed
        .iter()
        .filter(|r| matches!(r.reference, CiRef::Train(_)))
        .count();
    assert_eq!((prs, trains), (0, 1), "{executed:?}");
    for (n, worktree) in [(2, &worktrees[1]), (3, &worktrees[2])] {
        let head = git(worktree, &["rev-parse", "HEAD"])?;
        let runs = app.deps.store.ci_runs_for_sha(&f.sandbox, &head).await?;
        let run = runs.first().ok_or("verdict")?;
        assert_eq!(run.state, RunState::Passed, "{run:?}");
        assert!(
            run.reason
                .as_deref()
                .is_some_and(|r| r.contains("merge train #2, #3 passed")),
            "{run:?}"
        );
        assert_eq!(f.status_of(&head).as_deref(), Some("success"), "#{n}");
        let state = f.wait(n, worktree).await?;
        let outcome = state["merge_outcome"].as_str().unwrap_or_default();
        let landed = git(
            &f.origin,
            &["rev-parse", if n == 2 { "dev^" } else { "dev" }],
        )?;
        let peer = if n == 2 { 3 } else { 2 };
        assert!(
            outcome.contains(&format!("into `dev` at {}", &landed[..9])),
            "{outcome}"
        );
        assert!(
            outcome.contains(&format!("PR head {}", &head[..9])),
            "{outcome}"
        );
        assert!(
            outcome.contains(&format!("in a merge train with #{peer}.")),
            "{outcome}"
        );
        assert!(
            outcome.contains(&format!("candidate at {}", &train.sha[..9])),
            "{outcome}"
        );
        // A train's merge of a part is not the task's end either; a whole plan's is.
        let goes_on = outcome
            .contains("1 step(s) of your plan are still ahead (next: \"Later slice\")")
            && outcome.contains("then go on with the next step");
        let ends = outcome.contains("then end with your summary");
        assert_eq!((goes_on, ends), (n == 3, n == 2), "#{n}: {outcome}");
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

/// A Vidoc connector whose tests always pass (only the key's presence matters here).
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

#[tokio::test]
async fn a_singleton_configuration_refusal_tells_the_owner_what_the_parser_rejected() -> TestResult
{
    let f = fixture().await?;
    let app = &f.h.app;
    let invalid = format!("[commands]\ngate = \"make ci\"\n\n{YULIUS_TOML}");
    let worktree = f.task(1, &[("yulius.toml", invalid.as_str())]).await?;
    f.arm(1, &worktree).await?;
    f.record_passed(app, 1, &git(&worktree, &["rev-parse", "HEAD"])?)
        .await?;
    f.advance_dev("other.txt")?;
    app.merge_armed(&f.sandbox).await;
    let runs = f.train_runs(app).await?;
    let refused = runs
        .iter()
        .find(|run| run.state == RunState::Failed)
        .ok_or("refused candidate")?;
    let jobs = app.deps.store.ci_jobs(&refused.id, true).await?;
    let job = jobs
        .iter()
        .find(|job| job.name == "yulius.toml")
        .ok_or("config job")?;
    let reason = job.reason.as_deref().ok_or("parser diagnostic")?;
    assert!(reason.contains("gate"));
    app.merge_armed(&f.sandbox).await;
    let view = f.wait(1, &worktree).await?;
    let outcome = view["merge_outcome"]
        .as_str()
        .ok_or("the owner's refusal")?;
    assert!(
        outcome.contains("`yulius.toml` exited 1") && outcome.contains(reason),
        "{outcome}; expected {reason}"
    );
    Ok(())
}

/// SPEC-504 CFG-002 (review round 1): a merge candidate is checked as what the branch would be:
/// a member whose `yulius.toml` is invalid (its own run recorded passed, as a behind pull
/// request's could be on an older base) makes the candidate's run fail at once, with a failed
/// job `yulius.toml` naming the parser's message and no job run.
#[tokio::test]
async fn a_candidate_with_an_invalid_yulius_toml_fails_with_the_parsers_message() -> TestResult {
    use yulius_domain::ci::RunState;
    let f = fixture().await?;
    let app = f.h.app.clone();
    let invalid = format!("[commands]\ngate = \"make ci\"\n\n{YULIUS_TOML}");
    let worktrees = [
        f.task(1, &[("yulius.toml", invalid.as_str())]).await?,
        f.task(2, &[("b.txt", "x\n")]).await?,
    ];
    for (n, worktree) in (1..).zip(&worktrees) {
        f.arm(n, worktree).await?;
        f.record_passed(&app, n, &git(worktree, &["rev-parse", "HEAD"])?)
            .await?;
    }
    f.advance_dev("other.txt")?;
    app.merge_armed(&f.sandbox).await;
    let trains = f.train_runs(&app).await?;
    let refused = trains
        .iter()
        .find(|run| run.state == RunState::Failed)
        .ok_or_else(|| format!("no refused candidate: {trains:?}"))?;
    assert!(
        refused
            .reason
            .as_deref()
            .is_some_and(|r| r.contains("gate")),
        "{refused:?}"
    );
    let jobs = app.deps.store.ci_jobs(&refused.id, false).await?;
    let config = jobs
        .iter()
        .find(|job| job.name == "yulius.toml")
        .ok_or("no yulius.toml job")?;
    assert_eq!(config.state, RunState::Failed);
    assert!(
        jobs.iter()
            .filter(|job| job.name != "yulius.toml")
            .all(|job| job.state == RunState::Skipped),
        "{jobs:?}"
    );
    Ok(())
}

/// SPEC-217 VDP-001 (review round 6): a train whose members changed the CI runs their jobs with
/// the trusted security job; a member's own job named `security` does not stand in for it.
#[tokio::test]
async fn a_train_with_its_own_jobs_keeps_the_security_job() -> TestResult {
    let f = fixture().await?;
    let app = f.h.app.clone();
    app.set_vidoc(yulius_app::vidoc_key::VidocServices::new(
        Arc::new(AnyVidoc),
        Ok(Some(yulius_app::model::Secret(
            "vidoc-instance-key-0123456789".into(),
        ))),
    ));
    app.load_vidoc_key().await;
    let own =
        format!("{YULIUS_TOML}\n[[ci.jobs]]\nname = \"security\"\ncommand = \"echo fake pass\"\n");
    let worktrees = [
        f.task(1, &[("yulius.toml", own.as_str())]).await?,
        f.task(2, &[("b.txt", "x\n")]).await?,
    ];
    for (n, worktree) in (1..).zip(&worktrees) {
        f.arm(n, worktree).await?;
        f.record_passed(&app, n, &git(worktree, &["rev-parse", "HEAD"])?)
            .await?;
    }
    // Both are behind their base once another pull request merged.
    f.advance_dev("other.txt")?;
    app.merge_armed(&f.sandbox).await;
    let trains = f.train_runs(&app).await?;
    assert_eq!(trains.len(), 1, "{trains:?}");
    let jobs = app.deps.store.ci_jobs(&trains[0].id, false).await?;
    let commands: Vec<(&str, &str)> = jobs
        .iter()
        .map(|job| (job.key.as_str(), job.command.as_str()))
        .collect();
    assert!(
        commands.contains(&("security", "yulius security-scan --ci")),
        "{commands:?}"
    );
    assert!(
        !commands
            .iter()
            .any(|(_, command)| command.contains("fake pass")),
        "{commands:?}"
    );
    Ok(())
}

/// SPEC-441 MQ-004 (was TRN-006's one-by-one fallback): a candidate whose combined result fails
/// is bisected. Of three green pull requests, #1 and #3 pass alone but break together: the
/// candidate of three fails, its first two pass and merge, and #3, which the first two passed
/// without on the same base, is told why at once, with no candidate of its own (SPEC-497
/// MQW-003). No pull request's own run is repeated.
#[tokio::test]
async fn a_red_candidate_is_bisected_and_the_breaking_pull_request_told() -> TestResult {
    let f = fixture().await?;
    let app = f.h.app.clone();
    let sandbox = app.deps.store.sandbox(&f.sandbox).await?.ok_or("s1")?;
    let mut worktrees = Vec::new();
    for (n, file) in [(1, "clash-1"), (2, "c.txt"), (3, "clash-2")] {
        worktrees.push(f.task(n, &[(file, "x\n")]).await?);
    }
    for (n, worktree) in (1..).zip(&worktrees) {
        f.arm(n, worktree).await?;
        f.record_passed(&app, n, &git(worktree, &["rev-parse", "HEAD"])?)
            .await?;
    }
    f.advance_dev("other.txt")?;
    let dropped = || async {
        Ok::<_, Box<dyn std::error::Error>>(
            f.wait(3, &worktrees[2]).await?["merge_outcome"]
                .as_str()
                .unwrap_or_default()
                .to_owned(),
        )
    };
    f.decide_until(&app, || async {
        app.ci_sync_sandbox(&sandbox).await?;
        f.settle(&app).await?;
        Ok(f.merged(&app, 1).await? && f.merged(&app, 2).await? && !dropped().await?.is_empty())
    })
    .await?;
    assert_eq!(f.merges(), ["merge_pull #1 squash", "merge_pull #2 squash"]);
    assert!(!f.merged(&app, 3).await?);
    let outcome = dropped().await?;
    // #1 and #2 passed without it on the same base: #3 is told at once, with no candidate of its
    // own (SPEC-497 MQW-003).
    assert!(
        outcome.contains(
            "The merge you armed was dropped: pull request #3 merged into the current `dev` \
             together with #1, #2 failed (merge candidate run"
        ) && outcome.contains("and #1, #2 passed without it on the same `dev`")
            && outcome.contains(yulius_domain::review::KEEP_APPROVAL)
            && outcome.contains("its own run at")
            && outcome.contains("passed."),
        "{outcome}"
    );
    assert_eq!(f.wait(3, &worktrees[2]).await?["merge_armed"], false);
    let mut trains: Vec<String> = f
        .train_runs(&app)
        .await?
        .iter()
        .map(|r| format!("{} {}", r.reference.key(), r.state.as_str()))
        .collect();
    trains.sort();
    assert_eq!(
        trains,
        ["train/1+2 passed", "train/1+2+3 failed"],
        "bisected, and #3 told without a run of its own"
    );
    let executed = f.executed(&app).await?;
    assert!(
        executed
            .iter()
            .all(|r| !matches!(r.reference, CiRef::PullRequest(_))),
        "no pre-check again: {executed:?}"
    );
    Ok(())
}

/// When the first half lands, the last two recreate the four-member tree that failed.
#[tokio::test]
async fn a_bisection_executes_the_failed_tree_only_once() -> TestResult {
    let f = fixture_with(Project::Standard, None, MULTI_HEAD_BUDGET).await?;
    let app = f.h.app.clone();
    let mut worktrees = Vec::new();
    for (n, file) in [(1, "a.txt"), (2, "b.txt"), (3, "clash-1"), (4, "clash-2")] {
        worktrees.push(f.task(n, &[(file, "x\n")]).await?);
    }
    for (n, worktree) in (1..).zip(&worktrees) {
        f.arm(n, worktree).await?;
        f.record_passed(&app, n, &git(worktree, &["rev-parse", "HEAD"])?)
            .await?;
    }
    f.advance_dev("other.txt")?;
    app.merge_armed(&f.sandbox).await;
    let red = f
        .train_runs(&app)
        .await?
        .into_iter()
        .find(|r| r.reference == CiRef::Train(vec![1, 2, 3, 4]))
        .ok_or("missing four-member candidate")?;
    assert_eq!(run_candidate(&f, &app, &red.id).await?, RunState::Failed);
    let red_tree = git(&f.repo, &["rev-parse", &format!("{}^{{tree}}", red.sha)])?;

    // A later green branch run on this tree could lend a pass. The original candidate failure
    // must win even though the green run has every job the candidate's gate requires.
    let pass = CiRunRecord {
        id: "same-tree-pass".into(),
        reference: CiRef::Branch("topic".into()),
        state: RunState::Passed,
        started_at: Some(app.now()),
        ended_at: Some(app.now()),
        status_for: Some(RunState::Passed),
        ..red.clone()
    };
    let mut jobs = app.deps.store.ci_jobs(&red.id, false).await?;
    assert_eq!(jobs.len(), 1);
    for job in &mut jobs {
        job.id = format!("passed-{}", job.id);
        job.run_id.clone_from(&pass.id);
        job.state = RunState::Passed;
        job.exit_code = Some(0);
        job.failures = None;
    }
    app.deps.store.insert_ci_run(&pass, &jobs).await?;

    let pair = format!("merge train #3, #4 (run {})", red.id);
    f.decide_until(&app, || async {
        for run in f.train_runs(&app).await?.iter().filter(|r| r.id != red.id) {
            let tree = git(&f.repo, &["rev-parse", &format!("{}^{{tree}}", run.sha)])?;
            assert_ne!(
                tree,
                red_tree,
                "{} got a second run on the failed tree",
                run.reference.key()
            );
        }
        if dissolved(&app)
            .await?
            .iter()
            .any(|detail| detail.starts_with(&pair) && detail.contains("(why: bisected)"))
        {
            return Ok(true);
        }
        f.settle(&app).await?;
        Ok(false)
    })
    .await?;
    let merges = f.merges();
    assert_eq!(
        merges.iter().take(2).collect::<Vec<_>>(),
        ["merge_pull #1 squash", "merge_pull #2 squash"]
    );
    Ok(())
}

/// The scheduler executes a shared commit once. Each open PR can use that result.
#[tokio::test]
async fn the_other_pr_on_a_scheduled_head_can_wait_and_enter_the_queue() -> TestResult {
    let f = fixture().await?;
    let app = &f.h.app;
    let (one, two, head) = shared_pull_heads(&f).await?;
    let sandbox = app.deps.store.sandbox(&f.sandbox).await?.ok_or("sandbox")?;
    app.ci_sync_sandbox(&sandbox).await?;
    let runs = app.deps.store.ci_runs_for_sha(&f.sandbox, &head).await?;
    let [shared] = runs.as_slice() else {
        return Err(format!("one scheduled run expected: {runs:?}").into());
    };
    let (pr, worktree) = match shared.reference {
        CiRef::PullRequest(1) => (2, &two),
        CiRef::PullRequest(2) => (1, &one),
        _ => return Err(format!("unexpected shared run: {shared:?}").into()),
    };
    assert_eq!(f.wait(pr, worktree).await?["checks"], "queued");
    f.advance_dev("moved-base.txt")?;
    f.arm(pr, worktree).await?;
    f.settle(app).await?;
    let wait = app
        .agent_wait_state(&format!("tok-t{pr}"), Some(&head))
        .await?;
    assert_eq!(wait.checks, "passed");
    f.decide_until(app, || async {
        f.settle(app).await?;
        f.merged(app, pr).await
    })
    .await?;
    assert_eq!(
        app.deps
            .store
            .ci_runs_for_sha(&f.sandbox, &head)
            .await?
            .len(),
        1
    );
    Ok(())
}

async fn shared_pull_heads(f: &Fixture) -> BoxResult<(PathBuf, PathBuf, String)> {
    let one = f.task(1, &[("a.txt", "a\n")]).await?;
    let two = f.task(2, &[("b.txt", "b\n")]).await?;
    let first = git(&one, &["rev-parse", "HEAD"])?;
    git(&two, &["merge", "-q", "--no-edit", &first])?;
    let head = git(&two, &["rev-parse", "HEAD"])?;
    git(&one, &["merge", "--ff-only", &head])?;
    for worktree in [&one, &two] {
        git(worktree, &["push", "-q", "origin", "HEAD"])?;
    }
    Ok((one, two, head))
}

/// An explicit owner rerun takes precedence over a later peer verdict on the same commit.
#[tokio::test]
async fn an_owner_failure_keeps_a_later_green_peer_from_merging_it() -> TestResult {
    shared_owner_rerun(RunState::Failed, RunState::Passed).await
}

#[tokio::test]
async fn an_owner_pass_survives_a_later_red_peer_in_the_merge_queue() -> TestResult {
    shared_owner_rerun(RunState::Passed, RunState::Failed).await
}

/// Private merge and playground runs cannot substitute for ordinary scheduler CI.
#[tokio::test]
async fn the_shared_head_fallback_skips_private_runs_and_cancelled_peers() -> TestResult {
    let f = fixture().await?;
    let app = &f.h.app;
    let (one, _two, head) = shared_pull_heads(&f).await?;
    f.record_passed(app, 2, &head).await?;
    let mut run = app
        .deps
        .store
        .ci_runs_for_sha(&f.sandbox, &head)
        .await?
        .remove(0);
    for (reference, state) in [
        (CiRef::PullRequest(2), RunState::Cancelled),
        (CiRef::Train(vec![1]), RunState::Failed),
        (CiRef::Task(1), RunState::Error),
    ] {
        run.id = format!("new-{}", reference.key());
        run.reference = reference;
        run.state = state;
        app.deps.store.insert_ci_run(&run, &[]).await?;
        assert_eq!(f.wait(1, &one).await?["checks"], "passed", "{}", run.id);
    }
    f.record_run(app, 1, &head, RunState::Cancelled).await?;
    assert_eq!(f.wait(1, &one).await?["checks"], "cancelled");
    Ok(())
}

/// The final merge checks must agree with the queue when the base has not moved.
#[tokio::test]
async fn a_green_shared_head_owner_merges_with_its_published_green_status() -> TestResult {
    shared_protected_merge(true).await
}

#[tokio::test]
async fn the_host_can_refuse_a_green_owner_when_the_shared_status_is_red() -> TestResult {
    shared_protected_merge(false).await
}

async fn shared_protected_merge(owner_publishes_last: bool) -> TestResult {
    let f = fixture().await?;
    let app = &f.h.app;
    let (one, _two, head) = shared_pull_heads(&f).await?;
    f.arm(1, &one).await?;
    f.record_run(app, 1, &head, RunState::Passed).await?;
    f.h.clock.0.fetch_add(1, Ordering::SeqCst);
    f.record_run(app, 2, &head, RunState::Failed).await?;
    if owner_publishes_last {
        // A status publication retry can reach GitHub after the newer peer’s status.
        let green =
            f.h.github
                .statuses()
                .into_iter()
                .find(|(sha, status)| sha == &head && status.state == "success")
                .ok_or("owner's published green status")?;
        f.h.github.statuses.lock().map_err(|_| "lock")?.push(green);
    }
    app.merge_armed(&f.sandbox).await;
    let wait = f.wait(1, &one).await?;
    assert_eq!(f.merged(app, 1).await?, owner_publishes_last, "{wait}");
    if !owner_publishes_last {
        assert!(
            wait["merge_outcome"]
                .as_str()
                .is_some_and(|s| s.contains("Required status check")),
            "{wait}"
        );
    }
    assert_eq!(wait["checks"], "passed", "{wait}");
    Ok(())
}

#[tokio::test]
async fn a_shared_head_infrastructure_error_retries_the_owner_once() -> TestResult {
    let f = fixture().await?;
    let app = &f.h.app;
    let (_, _, head) = shared_pull_heads(&f).await?;
    f.record_run(app, 1, &head, RunState::Error).await?;
    f.record_passed(app, 2, &head).await?;
    f.record_run(app, 3, &head, RunState::Error).await?;
    app.reconcile_deliveries().await;
    let runs = app.deps.store.ci_runs_for_sha(&f.sandbox, &head).await?;
    let retry = runs
        .iter()
        .find(|r| r.reference == CiRef::PullRequest(1) && r.state == RunState::Queued)
        .ok_or_else(|| format!("owner retry missing: {runs:?}"))?;
    app.deps
        .store
        .transition_ci_run(
            &retry.id,
            &[RunState::Queued],
            RunState::Error,
            &yulius_app::ci::CiRunUpdate::default(),
        )
        .await?;
    app.reconcile_deliveries().await;
    assert_eq!(
        app.deps
            .store
            .ci_runs_for_sha(&f.sandbox, &head)
            .await?
            .len(),
        runs.len()
    );
    Ok(())
}

#[tokio::test]
async fn a_new_shared_run_replaces_a_refusal_for_the_old_integration_configuration() -> TestResult {
    let f = fixture().await?;
    let app = &f.h.app;
    let (one, two, head) = shared_pull_heads(&f).await?;
    f.record_run(app, 1, &head, RunState::Running).await?;
    let owner = app
        .deps
        .store
        .ci_runs_for_sha(&f.sandbox, &head)
        .await?
        .remove(0);
    let refused = yulius_app::ci::CiRunUpdate {
        reason: Some("yulius.toml is invalid on dev and this change does not repair it".into()),
        ended_at: Some(app.now()),
        ..Default::default()
    };
    app.deps
        .store
        .transition_ci_run(&owner.id, &[RunState::Running], RunState::Failed, &refused)
        .await?;
    assert_eq!(f.wait(1, &one).await?["checks"], "failed");
    // CI scheduling transfers a superseded run’s queue place to the replacement.
    // This new row was admitted after the refusal, with a place before it.
    let mut replacement = owner.clone();
    replacement.id = "scheduled-replacement".into();
    replacement.reference = CiRef::PullRequest(2);
    replacement.queued_at.0 -= 1;
    replacement.state = RunState::Queued;
    app.deps.store.insert_ci_run(&replacement, &[]).await?;
    for prefix in [head.as_str(), &head[..9]] {
        assert_eq!(
            app.agent_wait_state("tok-t1", Some(prefix)).await?.checks,
            "queued"
        );
    }
    app.deps
        .store
        .transition_ci_run(
            &replacement.id,
            &[RunState::Queued],
            RunState::Passed,
            &yulius_app::ci::CiRunUpdate::default(),
        )
        .await?;
    for prefix in [head.as_str(), &head[..9]] {
        assert_eq!(
            app.agent_wait_state("tok-t1", Some(prefix)).await?.checks,
            "passed"
        );
    }
    // If a later ordinary run observes another invalid configuration, its new
    // refusal supersedes the intervening pass.
    f.record_run(app, 3, &head, RunState::Running).await?;
    let newer = app
        .deps
        .store
        .ci_runs_for_sha(&f.sandbox, &head)
        .await?
        .remove(0);
    app.deps
        .store
        .transition_ci_run(&newer.id, &[RunState::Running], RunState::Failed, &refused)
        .await?;
    assert_eq!(f.wait(1, &one).await?["checks"], "failed");
    // PR 2 had a real pass before PR 3 encountered the configuration refusal.
    // That pass is obsolete for both owners once CI admits a replacement.
    let sandbox = app.deps.store.sandbox(&f.sandbox).await?.ok_or("sandbox")?;
    app.ci_sync_sandbox(&sandbox).await?;
    assert_eq!(f.wait(1, &one).await?["checks"], "queued");
    assert_eq!(f.wait(2, &two).await?["checks"], "queued");
    app.ci_sync_sandbox(&sandbox).await?;
    // Past refusals are synthetic. Dispatch and wait for the real replacement.
    let pending = app
        .deps
        .store
        .ci_runs_for_sha(&f.sandbox, &head)
        .await?
        .into_iter()
        .filter(|r| r.state == RunState::Queued)
        .collect::<Vec<_>>();
    let [real_replacement] = pending.as_slice() else {
        return Err(format!("expected one repair run: {pending:?}").into());
    };
    assert_eq!(
        run_candidate(&f, app, &real_replacement.id).await?,
        RunState::Passed
    );
    assert_eq!(f.wait(1, &one).await?["checks"], "passed");
    Ok(())
}

#[tokio::test]
async fn a_failed_shared_repair_reaches_the_owner_of_an_invalidated_pass() -> TestResult {
    let f = fixture().await?;
    let app = &f.h.app;
    let (one, _, head) = shared_pull_heads(&f).await?;
    f.arm(1, &one).await?;
    f.record_passed(app, 1, &head).await?;
    f.record_run(app, 2, &head, RunState::Running).await?;
    let runs = app.deps.store.ci_runs_for_sha(&f.sandbox, &head).await?;
    let refusal = runs
        .iter()
        .find(|r| r.reference == CiRef::PullRequest(2))
        .ok_or("peer run")?;
    let failed = yulius_app::ci::CiRunUpdate {
        reason: Some("yulius.toml is invalid on dev".into()),
        ..Default::default()
    };
    app.deps
        .store
        .transition_ci_run(&refusal.id, &[RunState::Running], RunState::Failed, &failed)
        .await?;
    f.record_run(app, 3, &head, RunState::Queued).await?;
    let runs = app.deps.store.ci_runs_for_sha(&f.sandbox, &head).await?;
    let replacement = runs
        .iter()
        .find(|r| r.reference == CiRef::PullRequest(3))
        .ok_or("repair run")?;
    assert_eq!(f.wait(1, &one).await?["checks"], "queued");
    let reason = "the repaired configuration ran quality and quality failed";
    app.deps
        .store
        .transition_ci_run(
            &replacement.id,
            &[RunState::Queued],
            RunState::Failed,
            &yulius_app::ci::CiRunUpdate {
                reason: Some(reason.into()),
                ..Default::default()
            },
        )
        .await?;
    app.merge_armed(&f.sandbox).await;
    let wait = f.wait(1, &one).await?;
    assert_eq!(wait["checks"], "failed", "{wait}");
    assert_eq!(wait["checks_detail"], reason, "{wait}");
    assert_eq!(wait["merge_armed"], false, "{wait}");
    Ok(())
}

#[tokio::test]
async fn a_short_shared_head_finds_its_owner_beyond_recent_runs() -> TestResult {
    let f = fixture().await?;
    let app = &f.h.app;
    let (_, _, head) = shared_pull_heads(&f).await?;
    f.record_run(app, 1, &head, RunState::Failed).await?;
    for n in 10..65 {
        f.record_passed(app, n, &format!("{n:040x}")).await?;
    }
    f.record_passed(app, 2, &head).await?;
    let short = &head[..9];
    assert_eq!(
        app.agent_wait_state("tok-t1", Some(short)).await?.checks,
        "failed"
    );
    let other = format!(
        "{short}{}",
        if head.ends_with('0') {
            "1".repeat(31)
        } else {
            "0".repeat(31)
        }
    );
    f.record_passed(app, 3, &other).await?;
    assert_eq!(
        app.agent_wait_state("tok-t1", Some(short)).await?.checks,
        "none"
    );
    Ok(())
}

#[tokio::test]
async fn a_hosted_pull_head_requires_its_exact_commit_in_the_queue() -> TestResult {
    let f = fixture().await?;
    let one = f.task(1, &[("one.txt", "one\n")]).await?;
    f.arm(1, &one).await?;
    let head = git(&one, &["rev-parse", "HEAD"])?;
    f.record_run(
        &f.h.app,
        1,
        &format!("{head}{}", "b".repeat(24)),
        RunState::Failed,
    )
    .await?;
    f.h.app.merge_armed(&f.sandbox).await;
    let wait = f.wait(1, &one).await?;
    assert_eq!(wait["merge_armed"], true, "{wait}");
    Ok(())
}

#[tokio::test]
async fn a_forty_digit_wait_head_can_abbreviate_a_sha256_commit() -> TestResult {
    let f = fixture().await?;
    let app = &f.h.app;
    let _one = f.task(1, &[("one.txt", "one\n")]).await?;
    let prefix = "a".repeat(40);
    let full = format!("{prefix}{}", "b".repeat(24));
    f.record_run(app, 1, &full, RunState::Failed).await?;
    assert_eq!(
        app.agent_wait_state("tok-t1", Some(&prefix)).await?.checks,
        "failed"
    );
    f.record_passed(app, 2, &format!("{prefix}{}", "c".repeat(24)))
        .await?;
    assert_eq!(
        app.agent_wait_state("tok-t1", Some(&prefix)).await?.checks,
        "none"
    );
    // A full SHA-1 wins even when another available hash begins with all its digits.
    f.record_passed(app, 3, &prefix).await?;
    assert_eq!(
        app.agent_wait_state("tok-t1", Some(&prefix)).await?.checks,
        "passed"
    );
    Ok(())
}

async fn shared_owner_rerun(owner_state: RunState, peer_state: RunState) -> TestResult {
    let f = fixture().await?;
    let app = &f.h.app;
    let (one, _two, head) = shared_pull_heads(&f).await?;
    f.arm(1, &one).await?;
    f.record_run(app, 1, &head, owner_state).await?;
    f.h.clock.0.fetch_add(1, Ordering::SeqCst);
    f.record_run(app, 2, &head, peer_state).await?;
    f.advance_dev("moved-base.txt")?;
    let wait = f.wait(1, &one).await?;
    assert_eq!(wait["checks"], owner_state.as_str(), "{wait}");
    let sandbox = app.deps.store.sandbox(&f.sandbox).await?.ok_or("sandbox")?;
    assert_eq!(
        app.decidable_in(&sandbox).await,
        owner_state == RunState::Passed
    );

    app.merge_armed(&f.sandbox).await;
    if owner_state == RunState::Failed {
        let wait = f.wait(1, &one).await?;
        assert_eq!(wait["merge_armed"], false, "{wait}");
        assert!(
            wait["merge_outcome"]
                .as_str()
                .is_some_and(|s| s.contains("checks of pull request #1 failed")),
            "{wait}"
        );
        assert!(!f.merged(app, 1).await?);
    } else {
        assert!(
            f.train_runs(app)
                .await?
                .iter()
                .any(|r| r.reference == CiRef::Train(vec![1]))
        );
    }
    Ok(())
}

/// The owner of the singleton gets the verdict even when CI was executed for another train.
#[tokio::test]
async fn a_reused_failed_candidate_tells_the_singleton_owner_which_job_failed() -> TestResult {
    Box::pin(reused_singleton(SingletonChange::None)).await
}

#[tokio::test]
async fn a_reused_failure_on_an_obsolete_base_keeps_the_owner_armed() -> TestResult {
    Box::pin(reused_singleton(SingletonChange::Base)).await
}

#[tokio::test]
async fn a_reused_failure_does_not_blame_a_retargeted_owner_pr() -> TestResult {
    Box::pin(reused_singleton(SingletonChange::Retarget)).await
}

#[tokio::test]
async fn a_reused_failure_does_not_blame_the_owners_new_head() -> TestResult {
    Box::pin(reused_singleton(SingletonChange::Head)).await
}

#[tokio::test]
async fn the_owners_ordinary_failure_wins_during_singleton_verification() -> TestResult {
    Box::pin(reused_singleton(SingletonChange::Ordinary)).await
}

#[tokio::test]
async fn a_base_advance_during_singleton_verification_keeps_the_arm_and_no_drop_span() -> TestResult
{
    Box::pin(reused_singleton(SingletonChange::BaseDuringRead)).await
}

#[tokio::test]
async fn another_pr_failing_at_the_same_sha_keeps_the_singleton_failure_with_its_owner()
-> TestResult {
    Box::pin(reused_singleton(SingletonChange::PeerFailed)).await
}

#[tokio::test]
async fn an_owner_rerun_started_during_singleton_verification_keeps_its_bounded_wait() -> TestResult
{
    Box::pin(reused_singleton(SingletonChange::RerunStarted)).await
}

#[tokio::test]
async fn a_singleton_wait_deadline_survives_repeated_reads_and_reports_the_tested_failure()
-> TestResult {
    Box::pin(reused_singleton(SingletonChange::RerunExpired)).await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_singleton_decides_the_arm_before_recording_its_dissolution() -> TestResult {
    Box::pin(reused_singleton(SingletonChange::Bookkeeping)).await
}

#[tokio::test]
async fn a_singleton_with_unavailable_inputs_releases_its_place_after_the_limit() -> TestResult {
    Box::pin(reused_singleton(SingletonChange::Unavailable)).await
}

#[tokio::test]
async fn a_singleton_waits_for_its_failing_job_to_be_readable() -> TestResult {
    Box::pin(reused_singleton(SingletonChange::JobRead)).await
}

#[tokio::test]
async fn a_singleton_waits_for_safe_redaction_before_telling_its_owner() -> TestResult {
    Box::pin(reused_singleton(SingletonChange::MatcherRead)).await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_reused_failure_pruned_during_the_singleton_decision_keeps_the_original_arm() -> TestResult
{
    Box::pin(reused_singleton(SingletonChange::Pruned)).await
}

#[tokio::test]
async fn a_reused_failure_cannot_drop_a_later_arm_of_the_same_head_after_restart() -> TestResult {
    Box::pin(reused_singleton(SingletonChange::Rearmed)).await
}

#[tokio::test]
async fn a_reused_failure_cannot_blame_the_tasks_replacement_pr_at_the_same_head() -> TestResult {
    Box::pin(reused_singleton(SingletonChange::ReplacementPr)).await
}

#[tokio::test]
async fn a_singleton_decision_recovers_an_interrupted_history_write() -> TestResult {
    Box::pin(reused_singleton(SingletonChange::HistoryWrite)).await
}

#[tokio::test]
async fn a_singleton_keeps_its_train_until_the_saved_owner_can_be_restored() -> TestResult {
    Box::pin(reused_singleton(SingletonChange::HistoryRead)).await
}

#[tokio::test]
async fn an_unreadable_arm_file_keeps_the_singleton_until_the_file_recovers() -> TestResult {
    Box::pin(reused_singleton(SingletonChange::ArmFileRead)).await
}

#[tokio::test]
async fn a_damaged_singleton_receipt_keeps_the_consumed_arm_in_recovery() -> TestResult {
    Box::pin(reused_singleton(SingletonChange::DamagedReceipt)).await
}

#[tokio::test]
async fn a_singleton_decision_survives_span_pruning_before_arm_cleanup() -> TestResult {
    Box::pin(reused_singleton(SingletonChange::PrunedReceipt)).await
}

#[tokio::test]
async fn consuming_a_replayed_singleton_requires_its_original_activation_and_receipt() -> TestResult
{
    Box::pin(reused_singleton(SingletonChange::ReplayedWrite)).await
}

#[tokio::test]
async fn a_stale_saved_head_cannot_retire_the_singletons_newer_decision() -> TestResult {
    Box::pin(reused_singleton(SingletonChange::StaleSavedHead)).await
}

#[tokio::test]
async fn a_merged_retry_retires_the_saved_replacement_before_clearing_it() -> TestResult {
    Box::pin(reused_singleton(SingletonChange::MergedRetry)).await
}

#[tokio::test]
async fn a_merged_retry_keeps_the_replacement_until_its_retirement_is_writable() -> TestResult {
    Box::pin(reused_singleton(SingletonChange::MergedRetryOffline)).await
}

#[tokio::test]
async fn a_writable_replacement_singleton_arm_succeeds_on_its_first_call() -> TestResult {
    Box::pin(reused_singleton(SingletonChange::WritableRearm)).await
}

#[tokio::test]
async fn retrying_a_restored_arm_does_not_import_an_unrelated_stale_arm_file_entry() -> TestResult {
    let f = fixture().await?;
    let owner = f.task(1, &[("one.txt", "one")]).await?;
    let peer = f.task(2, &[("two.txt", "two")]).await?;
    f.arm(1, &owner).await?;
    let app = restart(&f.h.app).await;
    let path = app
        .config
        .data_dir
        .join(yulius_app::agent_wait::ARMED_MERGES_FILE);
    let head = git(&owner, &["rev-parse", "HEAD"])?;
    let peer_head = git(&peer, &["rev-parse", "HEAD"])?;
    let original = saved_armed(&app, 1)?;
    let mut entries: Vec<Value> = serde_json::from_slice(&std::fs::read(&path)?)?;
    let mut stale = entries.first().ok_or("saved arm")?.clone();
    stale["task"] = json!("t2");
    stale["head"] = json!(peer_head);
    entries.push(stale);
    std::fs::write(&path, serde_json::to_vec(&entries)?)?;
    let ctx = app.agent_context("tok-t1").await?;
    let retry = Box::pin(app.tool_delivery_merge(
        &ctx,
        &json!({"reviewed_head":head,"review_summary":"approve"}),
    ))
    .await?;
    assert_eq!(retry["armed"], true, "{retry}");
    assert_eq!(saved_armed(&app, 1)?, original);
    assert!(
        !app.agent_wait_state("tok-t2", Some(&peer_head))
            .await?
            .merge_armed
    );
    Ok(())
}

#[tokio::test]
async fn a_person_merging_an_armed_head_tells_the_agent_which_revision_to_stage() -> TestResult {
    let f = fixture().await?;
    f.advance_dev_with("yulius.toml", &format!(
        "{YULIUS_TOML}\n[cd.dev]\n[cd.prod]\n[cd.staging]\nstage = \"dev\"\nmerge_first = true\ndeploy = \"d\"\nverify = \"v\"\nrollback = \"r\"\nmigrations = \"none: stateless\"\n"
    ))?;
    let tree = f.task(1, &[("owner.txt", "change")]).await?;
    let head = git(&tree, &["rev-parse", "HEAD"])?;
    f.arm(1, &tree).await?;
    f.record_passed(&f.h.app, 1, &head).await?;
    let response = call(
        &f.h.router,
        Method::POST,
        "/api/tasks/t1/delivery/merge",
        None,
        &f.alice,
    )
    .await?;
    assert_eq!(response.status, StatusCode::OK, "{}", response.body);
    let landed = git(&f.origin, &["rev-parse", f.integration])?;
    let receipt =
        f.h.app
            .agent_wait_state("tok-t1", Some(&head))
            .await?
            .merge_outcome
            .ok_or("receipt")?;
    assert!(
        receipt.contains(&format!("your merged revision {}", &landed[..12]))
            && receipt.contains("cd_deploy {\"environment\":\"staging\"}"),
        "{receipt}"
    );
    let repeated = support::mcp_tool(
        &f.h,
        "tok-t1",
        "delivery_merge",
        json!({"reviewed_head": head, "review_summary": "approve"}),
    )
    .await?;
    let repeated = support::tool_text(&repeated);
    let next = repeated["next"].as_str().ok_or("repeated merge receipt")?;
    assert!(
        next.contains("it counts as merged on green Yulius CI")
            && next.contains(&format!("your merged revision {}", &landed[..12]))
            && next.contains("cd_deploy {\"environment\":\"staging\"}")
            && !next.contains("Finish what the delivery profile still asks"),
        "{next}"
    );
    Ok(())
}

#[tokio::test]
async fn the_person_http_merge_keeps_its_receipt_when_the_closed_pr_read_fails() -> TestResult {
    let f = fixture().await?;
    let tree = f.task(1, &[("owner.txt", "change")]).await?;
    let head = git(&tree, &["rev-parse", "HEAD"])?;
    f.arm(1, &tree).await?;
    f.record_passed(&f.h.app, 1, &head).await?;
    f.h.github.fail_merged_reads.store(true, Ordering::SeqCst);
    let response = call(
        &f.h.router,
        Method::POST,
        "/api/tasks/t1/delivery/merge",
        None,
        &f.alice,
    )
    .await?;
    assert_eq!(response.status, StatusCode::OK, "{}", response.body);
    let landed = git(&f.origin, &["rev-parse", f.integration])?;
    let wait = f.h.app.agent_wait_state("tok-t1", Some(&head)).await?;
    assert_eq!(wait.merge_outcome_head.as_deref(), Some(head.as_str()));
    let receipt = wait.merge_outcome.ok_or("accepted merge receipt")?;
    assert!(
        receipt.contains(&format!(" at {} from PR head {}", &landed[..9], &head[..9])),
        "{receipt}"
    );
    assert!(receipt.contains("own Yulius CI run passed"), "{receipt}");
    assert!(
        !receipt.contains("into `dev`"),
        "unconfirmed landing branch: {receipt}"
    );
    Ok(())
}

#[tokio::test]
async fn the_person_http_merge_reconciles_the_replacement_singleton_arm() -> TestResult {
    use yulius_domain::stages::SubjectKind;

    let f = fixture().await?;
    let app = &f.h.app;
    let tree = f.task(1, &[("a.txt", "owner change\n")]).await?;
    let head = git(&tree, &["rev-parse", "HEAD"])?;
    let id = TaskId::parse("t1")?;
    let old = yulius_app::stages::NewSpan {
        sandbox_id: f.sandbox.clone(), subject_kind: SubjectKind::Task, subject_id: id.as_str().into(),
        key: "wait@1".into(), parent_key: None, kind: "wait".into(), name: "merge_armed".into(),
        environment: None, job: None, started_at: Timestamp(1), ended_at: Some(Timestamp(2)),
        outcome: Some("dropped".into()),
        detail: Some(json!({"head":head,"message":"shard 2 failed","singleton":{"pr":1,
            "base":git(&tree,&["rev-parse","origin/dev"])?,"tree":git(&tree,&["rev-parse","HEAD^{tree}"])?,
            "run":"prior-train"}}).to_string()),
    };
    assert!(app.deps.store.insert_span(&old).await?);
    f.h.store.execute_for_tests("CREATE TRIGGER refuse_retirement BEFORE UPDATE OF detail ON spans WHEN OLD.name='merge_armed' BEGIN SELECT RAISE(ABORT,'unavailable'); END").await?;
    let ctx = app.agent_context("tok-t1").await?;
    let args = json!({"reviewed_head":head,"review_summary":"approve"});
    assert!(
        Box::pin(app.tool_delivery_merge(&ctx, &args))
            .await
            .is_err()
    );
    assert!(saved_armed(app, 1)?.is_some());
    f.h.store
        .execute_for_tests("DROP TRIGGER refuse_retirement")
        .await?;
    f.record_passed(app, 1, &head).await?;
    let result = call(
        &f.h.router,
        Method::POST,
        "/api/tasks/t1/delivery/merge",
        None,
        &f.alice,
    )
    .await?;
    assert_eq!(result.status, StatusCode::OK, "{}", result.body);
    assert!(f.merged(app, 1).await?);
    let wait = app.agent_wait_state("tok-t1", Some(&head)).await?;
    assert!(
        wait.merge_outcome
            .as_deref()
            .is_some_and(|message| message.contains("own Yulius CI run passed")),
        "{wait:?}"
    );
    let history = app
        .deps
        .store
        .subject_spans(&f.sandbox, SubjectKind::Task, id.as_str(), 100)
        .await?;
    let accepted = history
        .iter()
        .find(|span| span.key == old.key)
        .ok_or("earlier singleton receipt")?;
    let detail: Value = serde_json::from_str(accepted.detail.as_deref().ok_or("receipt")?)?;
    assert!(
        detail["superseded_at"].is_i64(),
        "person's merged response left the previous decision current"
    );
    assert!(
        saved_armed(app, 1)?.is_none(),
        "person's merged response left its saved arm"
    );
    let finished = app
        .deps
        .store
        .latest_merge_wait(&id)
        .await?
        .ok_or("finished replacement")?;
    assert!(
        finished.started_at > accepted.started_at,
        "replacement wait must retain its own identity"
    );
    assert_eq!(finished.outcome.as_deref(), Some("merged"));
    Ok(())
}

enum SingletonChange {
    WritableRearm,
    ReplayedWrite,
    MergedRetryOffline,
    MergedRetry,
    StaleSavedHead,
    DamagedReceipt,
    PrunedReceipt,
    ArmFileRead,
    HistoryRead,
    HistoryWrite,
    ReplacementPr,
    Rearmed,
    Pruned,
    JobRead,
    MatcherRead,
    Unavailable,
    Bookkeeping,
    RerunExpired,
    PeerFailed,
    RerunStarted,
    BaseDuringRead,
    Ordinary,
    None,
    Base,
    Retarget,
    Head,
}

async fn check_recovered_singleton_arm(
    f: &Fixture,
    change: &SingletonChange,
    cold: &Arc<App>,
    arms: &Path,
    original_arms: &[u8],
    head: &str,
) -> TestResult {
    // A process exit after the SQL commit can leave the original arm on disk.
    std::fs::write(arms, original_arms)?;
    let recovered = restart(cold).await;
    assert!(
        saved_armed(&recovered, 1)?.is_none(),
        "committed arm restored twice"
    );
    let ctx = recovered.agent_context("tok-t1").await?;
    let args = json!({"reviewed_head":head,"review_summary":"approve"});
    let tmp = arms.with_extension("json.tmp");
    std::fs::create_dir(&tmp)?;
    let refused = Box::pin(recovered.tool_delivery_merge(&ctx, &args)).await;
    std::fs::remove_dir(&tmp)?;
    assert!(
        refused.is_err(),
        "an unsaved replacement was acknowledged: {refused:?}"
    );
    let latest = recovered
        .deps
        .store
        .latest_merge_wait(&TaskId::parse("t1")?)
        .await?
        .ok_or("receipt after refused arm")?;
    let old: Value = serde_json::from_str(latest.detail.as_deref().ok_or("decision")?)?;
    assert!(
        old["superseded_at"].is_null(),
        "a refused arm retired its owner: {old}"
    );
    if matches!(change, SingletonChange::WritableRearm) {
        let response = Box::pin(recovered.tool_delivery_merge(&ctx, &args)).await?;
        assert_eq!(response["armed"], true, "{response}");
        let (saved_head, stamp) = saved_armed(&recovered, 1)?.ok_or("saved arm")?;
        assert_eq!(saved_head, head);
        assert!(stamp > latest.started_at.0);
        let decision = recovered
            .deps
            .store
            .latest_merge_wait(&ctx.task.id)
            .await?
            .ok_or("prior wait")?;
        assert_eq!(decision.key, latest.key);
        let detail: Value = serde_json::from_str(decision.detail.as_deref().ok_or("receipt")?)?;
        assert!(detail["superseded_at"].is_i64());
        return Ok(());
    }
    // The replacement file is durable before retirement in SQL. While that write is
    // refused the queue must preserve the later activation, including across restart.
    f.h.store.execute_for_tests(
        "CREATE TRIGGER refuse_retirement BEFORE UPDATE OF detail ON spans WHEN OLD.name='merge_armed' BEGIN SELECT RAISE(ABORT, 'unavailable'); END",
    ).await?;
    assert!(
        Box::pin(recovered.tool_delivery_merge(&ctx, &args))
            .await
            .is_err()
    );
    let (saved_head, stamp) =
        saved_armed(&recovered, 1)?.ok_or("requested replacement is durable")?;
    assert_eq!(saved_head, head);
    assert!(stamp > latest.started_at.0);
    let restart_during_rearm = matches!(
        change,
        SingletonChange::HistoryRead | SingletonChange::MergedRetry
    );
    let live = if restart_during_rearm {
        restart(&recovered).await
    } else {
        recovered
    };
    Box::pin(live.merge_armed(&f.sandbox)).await;
    assert_eq!(saved_armed(&live, 1)?, Some((head.to_owned(), stamp)));
    let offline = matches!(change, SingletonChange::MergedRetryOffline);
    if !offline {
        f.h.store
            .execute_for_tests("DROP TRIGGER refuse_retirement")
            .await?;
    }
    if matches!(
        change,
        SingletonChange::MergedRetry | SingletonChange::MergedRetryOffline
    ) {
        let landed =
            Origin(f.origin.clone()).merge(f.integration, head, "squash", "external merge")?;
        {
            let mut pulls = f.h.github.pulls.lock().map_err(|_| "pulls")?;
            let pull = pulls
                .iter_mut()
                .find(|pr| pr.number == 1)
                .ok_or("owner PR")?;
            pull.state = "closed".into();
            pull.merged = true;
            pull.merge_commit_sha = Some(landed);
        }
        let owner = TaskId::parse("t1")?;
        let record = live.deps.store.delivery(&owner).await?.ok_or("delivery")?;
        assert!(
            live.deps
                .store
                .transition_delivery(
                    &record.stage,
                    &yulius_app::delivery::DeliveryRecord {
                        stage: DeliveryStage::MergedIntegration,
                        ..record.clone()
                    }
                )
                .await?
        );
        let retry_ctx = live.agent_context("tok-t1").await?;
        let response = Box::pin(live.tool_delivery_merge(&retry_ctx, &args)).await?;
        assert_eq!(response["merged"], true, "{response}");
        if offline {
            assert_eq!(
                saved_armed(&live, 1)?,
                Some((head.to_owned(), stamp)),
                "a merged PR lost the saved replacement during the outage"
            );
            f.h.store
                .execute_for_tests("DROP TRIGGER refuse_retirement")
                .await?;
        }
        if !offline {
            let decision = live
                .deps
                .store
                .latest_merge_wait(&owner)
                .await?
                .ok_or("decision after retry")?;
            let detail: Value = serde_json::from_str(decision.detail.as_deref().ok_or("receipt")?)?;
            assert!(
                detail["superseded_at"].is_i64(),
                "successful retry left the consumed decision current"
            );
            assert!(
                saved_armed(&live, 1)?.is_none(),
                "successful retry left its replacement on disk"
            );
        }
        let next = restart(&live).await;
        let decision = next
            .deps
            .store
            .latest_merge_wait(&owner)
            .await?
            .ok_or("accepted decision")?;
        let detail: Value =
            serde_json::from_str(decision.detail.as_deref().ok_or("owner receipt")?)?;
        assert!(
            detail["superseded_at"].is_i64(),
            "merged retry lost pending retirement: {detail}"
        );
        return Ok(());
    }
    if restart_during_rearm {
        f.h.clock.0.fetch_add(500, Ordering::SeqCst);
        let retry_ctx = live.agent_context("tok-t1").await?;
        let retry = Box::pin(live.tool_delivery_merge(&retry_ctx, &args)).await?;
        assert_eq!(retry["armed"], true, "{retry}");
        assert_eq!(
            saved_armed(&live, 1)?,
            Some((head.to_owned(), stamp)),
            "a retry before the next queue pass changed the saved replacement's place"
        );
    } else {
        f.h.store.execute_for_tests("CREATE TEMP TABLE pending_detail AS SELECT id, detail FROM spans WHERE name='merge_armed' AND subject_id='t1' AND outcome='dropped'").await?;
        for damage in [
            "UPDATE spans SET detail='{}' WHERE id IN (SELECT id FROM pending_detail)",
            "UPDATE spans SET detail=json_remove(detail,'$.singleton.tree') WHERE id IN (SELECT id FROM pending_detail)",
        ] {
            f.h.store.execute_for_tests(damage).await?;
            let retry_ctx = live.agent_context("tok-t1").await?;
            let retry = Box::pin(live.tool_delivery_merge(&retry_ctx, &args)).await;
            assert!(
                retry.is_err(),
                "acknowledged an unreadable pending retirement: {retry:?}"
            );
            Box::pin(live.merge_armed(&f.sandbox)).await;
            assert_eq!(saved_armed(&live, 1)?, Some((head.to_owned(), stamp)));
            f.h.store.execute_for_tests("UPDATE spans SET detail=(SELECT detail FROM pending_detail WHERE pending_detail.id=spans.id) WHERE id IN (SELECT id FROM pending_detail)").await?;
        }
        Box::pin(live.merge_armed(&f.sandbox)).await;
    }
    let old = live
        .deps
        .store
        .subject_spans(
            &f.sandbox,
            yulius_domain::stages::SubjectKind::Task,
            "t1",
            100,
        )
        .await?
        .into_iter()
        .find(|span| span.key == latest.key)
        .ok_or("original receipt")?;
    let retired: Value = serde_json::from_str(old.detail.as_deref().ok_or("decision")?)?;
    assert!(
        retired["superseded_at"].is_i64(),
        "restored replacement must finish retirement: {retired}"
    );
    Ok(())
}

async fn check_singleton_history(
    f: &Fixture,
    change: &SingletonChange,
    head: &str,
    base: &str,
    tree: &str,
    run_id: &str,
    secret: &str,
) -> TestResult {
    use yulius_domain::stages::SubjectKind;
    let app = &f.h.app;
    let arms = app
        .config
        .data_dir
        .join(yulius_app::agent_wait::ARMED_MERGES_FILE);
    let original_arms = std::fs::read(&arms)?;
    if matches!(change, SingletonChange::ReplayedWrite) {
        // Replay with a row already present at the unique span key. Only a durable
        // copy of the owner's exact decision can acknowledge this activation.
        let saved = saved_armed(app, 1)?.ok_or("original arm")?;
        let trigger = "CREATE TRIGGER replay_merge_wait BEFORE INSERT ON spans WHEN NEW.name='merge_armed' BEGIN
            INSERT OR IGNORE INTO spans (sandbox_id,subject_kind,subject_id,key,kind,name,started_at,ended_at,outcome,detail)
            VALUES (NEW.sandbox_id,NEW.subject_kind,NEW.subject_id,NEW.key,NEW.kind,NEW.name,NEW.started_at,NEW.ended_at,NEW.outcome,NEW.detail);
            SELECT RAISE(IGNORE); END";
        for (stamp, detail) in [
            ("NEW.started_at + 1", "NEW.detail"),
            (
                "NEW.started_at",
                "json_set(NEW.detail, '$.singleton.run', 'different-run')",
            ),
        ] {
            let interrupted = trigger
                .replace(
                    "NEW.started_at,NEW.ended_at",
                    &format!("{stamp},NEW.ended_at"),
                )
                .replace("NEW.outcome,NEW.detail", &format!("NEW.outcome,{detail}"));
            f.h.store.execute_for_tests(&interrupted).await?;
            app.merge_armed(&f.sandbox).await;
            assert_eq!(
                saved_armed(app, 1)?,
                Some(saved.clone()),
                "a mismatched replay consumed the arm"
            );
            f.h.store.execute_for_tests("DROP TRIGGER replay_merge_wait; DELETE FROM spans WHERE subject_id='t1' AND name='merge_armed'").await?;
        }
        f.h.store.execute_for_tests(trigger).await?;
        app.merge_armed(&f.sandbox).await;
        assert!(
            saved_armed(app, 1)?.is_none(),
            "the original persisted receipt must finish the replay"
        );
        let span = app
            .deps
            .store
            .latest_merge_wait(&TaskId::parse("t1")?)
            .await?
            .ok_or("saved singleton")?;
        assert_eq!(span.started_at.0, saved.1);
        let written: Value = serde_json::from_str(span.detail.as_deref().ok_or("receipt")?)?;
        assert_eq!(written["singleton"]["run"], run_id);
        return Ok(());
    }
    f.h.store.execute_for_tests(
            "CREATE TRIGGER refuse_merge_span BEFORE INSERT ON spans WHEN NEW.name = 'merge_armed' BEGIN SELECT RAISE(ABORT, 'temporarily unavailable'); END",
        ).await?;
    app.merge_armed(&f.sandbox).await;
    f.h.store
        .execute_for_tests("DROP TRIGGER refuse_merge_span")
        .await?;
    if matches!(change, SingletonChange::HistoryRead) {
        f.h.store.execute_for_tests(
                "INSERT INTO spans(sandbox_id,subject_kind,subject_id,key,kind,name,started_at,ended_at,outcome,detail) VALUES ('s1','task','t1','wait@1','wait','merge_armed',1,2,'merged',x'FF')",
            ).await?;
    }
    if matches!(change, SingletonChange::ArmFileRead) {
        std::fs::write(&arms, b"[")?;
    }
    let cold = restart(app).await;
    if matches!(change, SingletonChange::ArmFileRead) {
        let expected = f.trains_file(app);
        Box::pin(cold.merge_decidable_everywhere()).await;
        cold.merge_armed(&f.sandbox).await;
        assert_eq!(
            f.trains_file(&cold),
            expected,
            "arm read failure dissolved the original singleton"
        );
        std::fs::write(&arms, &original_arms)?;
        cold.merge_armed(&f.sandbox).await;
    }
    if matches!(change, SingletonChange::HistoryRead) {
        let expected = f.trains_file(app);
        Box::pin(cold.merge_decidable_everywhere()).await;
        cold.merge_armed(&f.sandbox).await;
        assert_eq!(
            f.trains_file(&cold),
            expected,
            "keep the original train and deadline until its owner is restored"
        );
        f.h.store
            .execute_for_tests(
                "UPDATE spans SET detail='{}' WHERE key='wait@1' AND subject_id='t1'",
            )
            .await?;
        Box::pin(cold.resume_armed_merges()).await;
    }
    cold.merge_armed(&f.sandbox).await;
    cold.merge_armed(&f.sandbox).await;
    let spans = cold
        .deps
        .store
        .subject_spans(&f.sandbox, SubjectKind::Task, "t1", 100)
        .await?;
    let recorded: Vec<Value> = spans
        .iter()
        .filter(|s| s.name == "merge_armed" && s.outcome.as_deref() == Some("dropped"))
        .filter_map(|s| serde_json::from_str(s.detail.as_deref()?).ok())
        .collect();
    assert_eq!(recorded.len(), 1, "one accepted decision");
    assert_eq!(recorded[0]["head"], head);
    assert!(
        recorded[0]["message"]
            .as_str()
            .is_some_and(|message| message.contains("292.1 ms") && !message.contains(secret))
    );
    assert_eq!(
        recorded[0]["singleton"],
        json!({"pr":1,"base":base,"tree":tree,"run":run_id}),
        "the accepted singleton decision must survive the failed write and restart"
    );
    if matches!(change, SingletonChange::StaleSavedHead) {
        let mut entries: Value = serde_json::from_slice(&original_arms)?;
        let stamp = entries[0]["armed_at"]
            .as_i64()
            .ok_or("original activation")?;
        // A reviewed move keeps its stamp even when its best-effort file update fails.
        // An older saved activation also cannot displace a later accepted decision.
        for saved_stamp in [stamp, stamp - 1] {
            entries[0]["head"] = json!(base);
            entries[0]["armed_at"] = json!(saved_stamp);
            std::fs::write(&arms, serde_json::to_vec(&entries)?)?;
            let recovered = restart(&cold).await;
            assert!(
                saved_armed(&recovered, 1)?.is_none(),
                "an old head of activation {saved_stamp} was rearmed"
            );
            let span = recovered
                .deps
                .store
                .latest_merge_wait(&TaskId::parse("t1")?)
                .await?
                .ok_or("accepted receipt")?;
            let detail: Value = serde_json::from_str(span.detail.as_deref().ok_or("decision")?)?;
            assert_eq!(detail["head"], head);
            assert!(
                detail["superseded_at"].is_null(),
                "the old file retired the accepted decision: {detail}"
            );
        }
        return Ok(());
    }
    if matches!(change, SingletonChange::DamagedReceipt) {
        f.h.store.execute_for_tests("CREATE TEMP TABLE accepted_detail AS SELECT id, detail FROM spans WHERE name='merge_armed' AND subject_id='t1' AND outcome='dropped'").await?;
        for damage in [
            "UPDATE spans SET detail='{' WHERE id IN (SELECT id FROM accepted_detail)",
            "UPDATE spans SET detail=json_remove(detail,'$.singleton.tree') WHERE id IN (SELECT id FROM accepted_detail)",
        ] {
            std::fs::write(&arms, &original_arms)?;
            f.h.store.execute_for_tests(damage).await?;
            let recovering = restart(&cold).await;
            assert!(
                saved_armed(&recovering, 1)?.is_some(),
                "keep the saved identity until the receipt is readable"
            );
            f.h.store.execute_for_tests("UPDATE spans SET detail=(SELECT detail FROM accepted_detail WHERE accepted_detail.id=spans.id) WHERE id IN (SELECT id FROM accepted_detail)").await?;
            recovering.restore_armed_merges().await;
            assert!(
                saved_armed(&recovering, 1)?.is_none(),
                "unreadable accepted receipt reactivated a consumed arm: {damage}"
            );
        }
        return Ok(());
    }
    if matches!(change, SingletonChange::PrunedReceipt) {
        let entries: Value = serde_json::from_slice(&original_arms)?;
        let stamp = entries[0]["armed_at"].as_i64().ok_or("original stamp")?;
        cold.deps
            .store
            .prune_spans(yulius_domain::time::Timestamp(stamp + 1))
            .await?;
        std::fs::write(&arms, &original_arms)?;
        let recovering = restart(&cold).await;
        assert!(
            saved_armed(&recovering, 1)?.is_none(),
            "telemetry pruning reactivated the consumed arm"
        );
        return Ok(());
    }
    Box::pin(check_recovered_singleton_arm(
        f,
        change,
        &cold,
        &arms,
        &original_arms,
        head,
    ))
    .await?;
    Ok(())
}

async fn reused_singleton(change: SingletonChange) -> TestResult {
    use yulius_domain::stages::SubjectKind;
    let f = fixture().await?;
    let app = &f.h.app;
    let worktree = f.task(1, &[("a.txt", "x\n")]).await?;
    f.arm(1, &worktree).await?;
    let head = git(&worktree, &["rev-parse", "HEAD"])?;
    f.record_passed(app, 1, &head).await?;
    f.advance_dev("other.txt")?;
    git(&f.repo, &["fetch", "-q", "origin"])?;
    let base = git(&f.repo, &["rev-parse", "origin/dev"])?;
    let tree = git(&f.repo, &["merge-tree", "--write-tree", &base, &head])?;
    let sha = git(&f.repo, &["commit-tree", &tree, "-m", "previous train"])?;
    let secret = "synthetic-candidate-secret";
    let created = call(
        &f.h.router,
        Method::POST,
        "/api/sandboxes/s1/agent-secrets",
        Some(json!({"name":"CHECK_KEY", "value":secret})),
        &f.alice,
    )
    .await?;
    assert_eq!(created.status, StatusCode::CREATED, "{}", created.body);
    let failure = format!(
        "FAIL [0.292s] (1/1) search latency{}\n  stderr ───\n    thread panicked at search.rs\n    search took 292.1 ms, limit 200 ms ({secret}), synthetic-\u{0007}candidate-secret\n    {} beyond-limit",
        "s".repeat(600),
        "x".repeat(512)
    );
    // Race hooks are process-wide, so each held decision uses its own source run.
    let run_id = match change {
        SingletonChange::Bookkeeping => "prior-train-bookkeeping",
        SingletonChange::Pruned => "prior-train-pruned",
        _ => "prior-train",
    };
    let (mut red, mut job) =
        crate::pr_watch::run(&f.sandbox, run_id, &sha, RunState::Failed, None, 1);
    red.reference = CiRef::Train(vec![8, 9]);
    job.name = "shard 2".into();
    job.state = RunState::Running;
    let job_id = job.id.clone();
    let mut sibling = job.clone();
    sibling.id = "prior-train-sibling".into();
    sibling.name = "shard 1".into();
    let mut no_excerpt = job.clone();
    no_excerpt.id = "prior-train-without-log".into();
    no_excerpt.name = "job without a retained failure".into();
    no_excerpt.ordinal = 1;
    no_excerpt.state = RunState::Failed;
    job.ordinal = 2;
    app.deps
        .store
        .insert_ci_run(&red, &[sibling, no_excerpt, job])
        .await?;
    assert!(
        app.deps
            .store
            .transition_ci_job(
                "prior-train-sibling",
                &[RunState::Running],
                RunState::Cancelled,
                &yulius_app::ci::CiJobUpdate {
                    reason: Some("stopped early: job shard 2 failed".into()),
                    failures: Some(format!("FAIL [0.002s] (1/1) partial {}\n", "p".repeat(600))),
                    ..Default::default()
                }
            )
            .await?
    );
    assert!(
        app.deps
            .store
            .transition_ci_job(
                &job_id,
                &[RunState::Running],
                RunState::Failed,
                &yulius_app::ci::CiJobUpdate {
                    exit_code: Some(100),
                    failures: Some(failure),
                    output: Some("unrelated raw CI output".into()),
                    ended_at: Some(app.now()),
                    ..Default::default()
                }
            )
            .await?
    );
    app.merge_armed(&f.sandbox).await;
    if matches!(
        change,
        SingletonChange::HistoryWrite
            | SingletonChange::HistoryRead
            | SingletonChange::ArmFileRead
            | SingletonChange::DamagedReceipt
            | SingletonChange::PrunedReceipt
            | SingletonChange::ReplayedWrite
            | SingletonChange::WritableRearm
            | SingletonChange::MergedRetryOffline
            | SingletonChange::MergedRetry
            | SingletonChange::StaleSavedHead
    ) {
        return Box::pin(check_singleton_history(
            &f, &change, &head, &base, &tree, run_id, secret,
        ))
        .await;
    }
    if matches!(change, SingletonChange::Rearmed) {
        // A later arm is persisted before restart, while the previous train still names the old arm.
        let path = app
            .config
            .data_dir
            .join(yulius_app::agent_wait::ARMED_MERGES_FILE);
        let mut saved: Vec<Value> = serde_json::from_slice(&std::fs::read(&path)?)?;
        let entry = saved
            .iter_mut()
            .find(|entry| entry["task"] == "t1")
            .ok_or("owner arm")?;
        let later = entry["armed_at"].as_i64().ok_or("original arm time")? + 1;
        entry["armed_at"] = json!(later);
        std::fs::write(path, serde_json::to_vec(&saved)?)?;
        let cold = restart(app).await;
        cold.merge_armed(&f.sandbox).await;
        assert_eq!(
            saved_armed(&cold, 1)?.map(|(_, at)| at),
            Some(later),
            "a previous train cannot consume the newer arm"
        );
        return Ok(());
    }
    if matches!(change, SingletonChange::Pruned) {
        let arm = saved_armed(app, 1)?.ok_or("original arm")?;
        let (at, go) = support::hold_point(yulius_app::race::Point::FailedTrainDeciding, run_id);
        let pass = {
            let app = Arc::clone(app);
            let sandbox = f.sandbox.clone();
            tokio::spawn(async move { app.merge_armed(&sandbox).await })
        };
        support::point_reached(at).await?;
        let (mut newer, job) = crate::pr_watch::run(
            &f.sandbox,
            "new-train-verdict",
            "another-tree",
            RunState::Passed,
            None,
            app.now().0,
        );
        newer.reference = red.reference.clone();
        app.deps.store.insert_ci_run(&newer, &[job]).await?;
        app.deps.store.prune_ci_runs(0, app.now()).await?;
        assert!(app.deps.store.ci_run(run_id).await?.is_none());
        assert!(app.deps.store.ci_jobs(run_id, true).await?.is_empty());
        go.send(())?;
        pass.await?;
        assert_eq!(
            saved_armed(app, 1)?,
            Some(arm),
            "a pruned source cannot blame its former singleton"
        );
        assert!(f.wait(1, &worktree).await?["merge_outcome"].is_null());
        return Ok(());
    }
    if matches!(
        change,
        SingletonChange::JobRead | SingletonChange::MatcherRead
    ) {
        let arm = saved_armed(app, 1)?.ok_or("original arm")?;
        if matches!(change, SingletonChange::JobRead) {
            f.h.store.execute_for_tests("UPDATE ci_jobs SET state='unreadable' WHERE run_id='prior-train' AND ordinal=2").await?;
            assert!(app.deps.store.ci_jobs(run_id, true).await.is_err());
        } else {
            f.h.store
                .execute_for_tests(
                    "UPDATE vault_secrets SET hosts='unreadable' WHERE name='CHECK_KEY'",
                )
                .await?;
            assert!(app.agent_secret_matcher(&f.sandbox).await.is_err());
        }
        app.merge_armed(&f.sandbox).await;
        assert_eq!(
            saved_armed(app, 1)?,
            Some(arm),
            "an unavailable diagnostic keeps the original arm"
        );
        assert!(f.wait(1, &worktree).await?["merge_outcome"].is_null());
        f.h.store
            .execute_for_tests(
                "UPDATE ci_jobs SET state='failed' WHERE run_id='prior-train' AND ordinal=2",
            )
            .await?;
        f.h.store
            .execute_for_tests("UPDATE vault_secrets SET hosts='[]' WHERE name='CHECK_KEY'")
            .await?;
    }
    if matches!(change, SingletonChange::Unavailable) {
        let arm = saved_armed(app, 1)?.ok_or("arm")?;
        // A partially published hosted observation is also unavailable.
        let remote = f.h.github.remote.lock().map_err(|_| "remote")?.take();
        {
            let mut pulls = f.h.github.pulls.lock().map_err(|_| "pull")?;
            pulls[0].base_sha.clear();
        }
        app.merge_armed(&f.sandbox).await;
        let partial: Value = serde_json::from_str(&f.trains_file(app))?;
        assert_eq!(
            partial[f.sandbox.as_str()]["train"]["run"],
            "prior-train",
            "{partial}"
        );
        *f.h.github.remote.lock().map_err(|_| "remote")? = remote;
        *f.h.github
            .merged_on_direct_read
            .lock()
            .map_err(|_| "read")? = Some(1);
        f.h.github.fail_merged_reads.store(true, Ordering::SeqCst);
        app.merge_armed(&f.sandbox).await;
        let waiting: Value = serde_json::from_str(&f.trains_file(app))?;
        assert_eq!(
            waiting[f.sandbox.as_str()]["train"]["run"],
            "prior-train",
            "{waiting}"
        );
        f.h.clock
            .0
            .fetch_add(yulius_app::merge_train::WAIT_LIMIT_MS + 1, Ordering::SeqCst);
        app.merge_armed(&f.sandbox).await;
        let released: Value = serde_json::from_str(&f.trains_file(app))?;
        assert!(
            released[f.sandbox.as_str()]["train"].is_null(),
            "{released}"
        );
        assert_eq!(
            released[f.sandbox.as_str()]["held_back"],
            json!([["t1", arm.1]]),
            "{released}"
        );
        assert_eq!(saved_armed(app, 1)?, Some(arm));
        return Ok(());
    }
    if matches!(change, SingletonChange::Bookkeeping) {
        let (at, go) = support::hold_point(yulius_app::race::Point::TrainDissolving, run_id);
        let pass = {
            let app = Arc::clone(app);
            let sandbox = f.sandbox.clone();
            tokio::spawn(async move { app.merge_armed(&sandbox).await })
        };
        support::point_reached(at).await?;
        let (mut rerun, job) = crate::pr_watch::run(
            &f.sandbox,
            "later-rerun",
            &head,
            RunState::Running,
            None,
            app.now().0 + 1,
        );
        rerun.reference = CiRef::PullRequest(1);
        app.deps.store.insert_ci_run(&rerun, &[job]).await?;
        let view = f.wait(1, &worktree).await?;
        go.send(())?;
        pass.await?;
        assert_eq!(view["checks"], "running", "{view}");
        assert_eq!(
            view["merge_armed"], false,
            "decision before bookkeeping: {view}"
        );
        assert!(
            view["merge_outcome"]
                .as_str()
                .is_some_and(|text| text.contains("prior-train")),
            "{view}"
        );
        return Ok(());
    }
    if matches!(
        change,
        SingletonChange::Ordinary
            | SingletonChange::BaseDuringRead
            | SingletonChange::PeerFailed
            | SingletonChange::RerunStarted
            | SingletonChange::RerunExpired
    ) {
        let hold = Arc::new(tokio::sync::Notify::new());
        *f.h.github.pull_read_hold.lock().map_err(|_| "hold")? = Some(hold.clone());
        let change_run = async {
            tokio::time::timeout(
                Duration::from_secs(5),
                f.h.github.pull_read_entered.notified(),
            )
            .await?;
            let written = async {
                if matches!(change, SingletonChange::BaseDuringRead) {
                    f.advance_dev("late-base.txt")?;
                } else {
                    let state = match change {
                        SingletonChange::PeerFailed => RunState::Passed,
                        SingletonChange::RerunStarted | SingletonChange::RerunExpired => {
                            RunState::Running
                        }
                        _ => RunState::Failed,
                    };
                    let (mut own, job) = crate::pr_watch::run(
                        &f.sandbox,
                        "ordinary-rerun",
                        &head,
                        state,
                        None,
                        app.now().0 + 1,
                    );
                    own.reference = CiRef::PullRequest(1);
                    app.deps.store.insert_ci_run(&own, &[job]).await?;
                    let peer_state = if matches!(change, SingletonChange::PeerFailed) {
                        RunState::Failed
                    } else {
                        RunState::Passed
                    };
                    let (mut peer, job) = crate::pr_watch::run(
                        &f.sandbox,
                        "peer",
                        &head,
                        peer_state,
                        None,
                        app.now().0 + 2,
                    );
                    peer.reference = CiRef::PullRequest(2);
                    app.deps.store.insert_ci_run(&peer, &[job]).await?;
                }
                Ok::<_, Box<dyn std::error::Error>>(())
            }
            .await;
            *f.h.github.pull_read_hold.lock().map_err(|_| "hold")? = None;
            hold.notify_one();
            written?;
            Ok::<_, Box<dyn std::error::Error>>(())
        };
        let (changed, ()) = tokio::time::timeout(Duration::from_secs(12), async {
            tokio::join!(change_run, app.merge_armed(&f.sandbox))
        })
        .await?;
        changed?;
        let state = f.wait(1, &worktree).await?;
        if matches!(change, SingletonChange::BaseDuringRead) {
            assert_eq!(state["merge_armed"], true, "{state}");
            let spans = app
                .deps
                .store
                .subject_spans(&f.sandbox, SubjectKind::Task, "t1", 100)
                .await?;
            assert!(
                !spans
                    .iter()
                    .any(|s| s.name == "merge_armed" && s.outcome.as_deref() == Some("dropped"))
            );
            return Ok(());
        }
        if matches!(
            change,
            SingletonChange::RerunStarted | SingletonChange::RerunExpired
        ) {
            assert_eq!(state["merge_armed"], true, "{state}");
            assert_eq!(state["checks"], "running", "{state}");
            assert!(state["merge_outcome"].is_null(), "{state}");
            assert!(
                state["merge_train"].as_str().is_some_and(
                    |note| note.contains("waits") && note.contains("own Yulius CI run")
                ),
                "{state}"
            );
            let spans = app
                .deps
                .store
                .subject_spans(&f.sandbox, SubjectKind::Task, "t1", 100)
                .await?;
            assert!(
                !spans
                    .iter()
                    .any(|s| s.name == "merge_armed" && s.outcome.as_deref() == Some("dropped"))
            );
            if matches!(change, SingletonChange::RerunExpired) {
                let saved = saved_armed(app, 1)?.ok_or("armed before CI read error")?;
                f.h.store
                    .execute_for_tests(
                        "UPDATE ci_runs SET reference = 'pr/0' WHERE id = 'ordinary-rerun'",
                    )
                    .await?;
                app.merge_armed(&f.sandbox).await;
                assert_eq!(
                    saved_armed(app, 1)?,
                    Some(saved),
                    "an unreadable owner run cannot drop the arm"
                );
                f.h.store
                    .execute_for_tests(
                        "UPDATE ci_runs SET reference = 'pr/1' WHERE id = 'ordinary-rerun'",
                    )
                    .await?;
                app.merge_armed(&f.sandbox).await;
                assert_eq!(f.wait(1, &worktree).await?["checks"], "running");
                f.h.clock
                    .0
                    .fetch_add(yulius_app::merge_train::WAIT_LIMIT_MS, Ordering::SeqCst);
                // Verification goes offline at the original deadline. Keep this
                // candidate's wait budget when the same hosted PR answers again.
                *f.h.github
                    .merged_on_direct_read
                    .lock()
                    .map_err(|_| "read")? = Some(1);
                f.h.github.fail_merged_reads.store(true, Ordering::SeqCst);
                app.merge_armed(&f.sandbox).await;
                assert_eq!(f.wait(1, &worktree).await?["merge_armed"], true);
                f.h.github.fail_merged_reads.store(false, Ordering::SeqCst);
                *f.h.github
                    .merged_on_direct_read
                    .lock()
                    .map_err(|_| "read")? = None;
                f.h.clock.0.fetch_add(1, Ordering::SeqCst);
                app.merge_armed(&f.sandbox).await;
                let state = f.wait(1, &worktree).await?;
                assert_eq!(state["merge_armed"], false);
                let text = state["merge_outcome"]
                    .as_str()
                    .ok_or("the expired singleton verdict")?;
                assert!(
                    text.contains("prior-train") && !text.contains("passed its own run"),
                    "{text}"
                );
                let receipt = app
                    .deps
                    .store
                    .latest_merge_wait(&TaskId::parse("t1")?)
                    .await?
                    .ok_or("expired singleton receipt")?;
                let detail: Value =
                    serde_json::from_str(receipt.detail.as_deref().ok_or("decision")?)?;
                assert_eq!(detail["expired_ordinary_run"], "ordinary-rerun", "{detail}");
                return Ok(());
            }
            end_own_run(app, "ordinary-rerun", RunState::Failed).await?;
            app.merge_armed(&f.sandbox).await;
            let state = f.wait(1, &worktree).await?;
            assert!(
                state["merge_outcome"].as_str().is_some_and(
                    |outcome| outcome.contains("checks") && !outcome.contains("prior-train")
                ),
                "{state}"
            );
            return Ok(());
        }
        if matches!(change, SingletonChange::PeerFailed) {
            assert!(
                state["merge_outcome"]
                    .as_str()
                    .is_some_and(
                        |outcome| outcome.contains("prior-train") && outcome.contains("shard 2")
                    ),
                "{state}"
            );
            assert_eq!(state["checks"], "passed", "{state}");
            return Ok(());
        }
        let outcome = state["merge_outcome"].as_str().ok_or("ordinary outcome")?;
        assert!(
            outcome.contains("checks") && !outcome.contains("prior-train"),
            "{state}"
        );
        assert_eq!(state["checks"], "failed", "{state}");
        return Ok(());
    }
    if !matches!(
        change,
        SingletonChange::None | SingletonChange::JobRead | SingletonChange::MatcherRead
    ) {
        match change {
            SingletonChange::Base => f.advance_dev("new-base.txt")?,
            SingletonChange::Retarget => {
                // A different integration branch can point at exactly the tested base.
                git(
                    &f.origin,
                    &["update-ref", "refs/heads/other-integration", &base],
                )?;
                f.h.github.pulls.lock().map_err(|_| "pulls")?[0].base = "other-integration".into();
            }
            SingletonChange::ReplacementPr => {
                let task = TaskId::parse("t1")?;
                let mut delivery = app
                    .deps
                    .store
                    .delivery(&task)
                    .await?
                    .ok_or("open delivery")?;
                delivery.pr_number = Some(2);
                assert!(
                    app.deps
                        .store
                        .transition_delivery(&delivery.stage, &delivery)
                        .await?
                );
            }
            SingletonChange::Head => {
                git(&worktree, &["commit", "--allow-empty", "-qm", "later head"])?;
                git(&worktree, &["push", "-q", "origin", "HEAD"])?;
            }
            SingletonChange::HistoryWrite
            | SingletonChange::HistoryRead
            | SingletonChange::ArmFileRead
            | SingletonChange::DamagedReceipt
            | SingletonChange::PrunedReceipt
            | SingletonChange::ReplayedWrite
            | SingletonChange::WritableRearm
            | SingletonChange::MergedRetryOffline
            | SingletonChange::MergedRetry
            | SingletonChange::StaleSavedHead
            | SingletonChange::Rearmed
            | SingletonChange::Pruned
            | SingletonChange::JobRead
            | SingletonChange::MatcherRead
            | SingletonChange::Unavailable
            | SingletonChange::Bookkeeping
            | SingletonChange::None
            | SingletonChange::Ordinary
            | SingletonChange::BaseDuringRead
            | SingletonChange::PeerFailed
            | SingletonChange::RerunStarted
            | SingletonChange::RerunExpired => {
                unreachable!()
            }
        }
        app.merge_armed(&f.sandbox).await;
        let state = f.wait(1, &worktree).await?;
        if matches!(change, SingletonChange::Base) {
            assert_eq!(state["merge_armed"], true, "{state}");
        }
        assert!(
            !state["merge_outcome"]
                .as_str()
                .unwrap_or_default()
                .contains("prior-train"),
            "{state}"
        );
        return Ok(());
    }
    f.decide_until(app, || async {
        Ok(f.wait(1, &worktree).await?["merge_armed"] == false)
    })
    .await?;
    let state = f.wait(1, &worktree).await?;
    let outcome = state["merge_outcome"].as_str().ok_or("singleton outcome")?;
    assert!(
        outcome.contains("prior-train")
            && outcome.contains("shard 2")
            && outcome.contains("search took 292.1 ms, limit 200 ms"),
        "{outcome}"
    );
    assert!(outcome.contains("[secret:CHECK_KEY]"), "{outcome}");
    assert!(
        !outcome.contains(secret)
            && !outcome.contains("beyond-limit")
            && !outcome.contains("unrelated raw CI output")
            && !outcome.contains(char::is_control),
        "{outcome}"
    );
    assert_eq!(f.train_runs(app).await?, [red]);
    if matches!(change, SingletonChange::None) {
        // Both the live slot and its durable decision still name this failure. A new
        // ordinary rerun must enter its bounded wait even before the daemon restarts.
        f.h.clock.0.fetch_add(1, Ordering::SeqCst);
        let (mut rerun, job) = crate::pr_watch::run(
            &f.sandbox,
            "recheck-owner",
            &head,
            RunState::Running,
            None,
            app.now().0,
        );
        rerun.reference = CiRef::PullRequest(1);
        app.deps.store.insert_ci_run(&rerun, &[job]).await?;
        let notices = app
            .config
            .data_dir
            .join(yulius_app::pr_watch::PR_NOTICES_FILE);
        let bounds = yulius_app::pr_watch::WatchBounds::default();
        // Retaining an older receipt with the same head and different diagnosis must
        // not replace the newer in-memory decision or start a wait on its behalf.
        f.h.store.execute_for_tests("CREATE TABLE prior_decision AS SELECT id, detail FROM spans WHERE subject_id='t1' AND outcome='dropped' AND name='merge_armed'").await?;
        f.h.store.execute_for_tests("UPDATE spans SET detail=json_set(detail, '$.message', 'an earlier diagnosis') WHERE id IN (SELECT id FROM prior_decision)").await?;
        Box::pin(app.watch_prs(&bounds)).await;
        if notices.exists() {
            let saved: Value = serde_json::from_slice(&std::fs::read(&notices)?)?;
            assert!(saved["t1"]["singleton_wait"].is_null(), "{saved}");
        }
        f.h.store.execute_for_tests("UPDATE spans SET detail=(SELECT detail FROM prior_decision WHERE prior_decision.id=spans.id) WHERE id IN (SELECT id FROM prior_decision)").await?;
        assert!(Box::pin(app.watch_prs(&bounds)).await.is_empty());
        let saved: Value = serde_json::from_slice(&std::fs::read(notices)?)?;
        assert_eq!(saved["t1"]["singleton_wait"]["run"], rerun.id, "{saved}");
    }
    let cold = restart(app).await;
    let spans = cold
        .deps
        .store
        .subject_spans(&f.sandbox, SubjectKind::Task, "t1", 100)
        .await?;
    let waits: Vec<_> = spans
        .iter()
        .filter(|s| s.name == "merge_armed" && s.outcome.as_deref() == Some("dropped"))
        .collect();
    assert_eq!(waits.len(), 1);
    Ok(())
}

/// A previous candidate may have lost its worker or been cancelled while testing this tree.
/// A pull request's own failure also belongs to a different reference.
#[tokio::test]
async fn inconclusive_candidate_runs_leave_the_same_tree_eligible_to_run() -> TestResult {
    let f = fixture().await?;
    let app = f.h.app.clone();
    let worktree = f.task(1, &[("a.txt", "x\n")]).await?;
    f.arm(1, &worktree).await?;
    let head = git(&worktree, &["rev-parse", "HEAD"])?;
    f.record_passed(&app, 1, &head).await?;
    f.advance_dev("other.txt")?;
    git(&f.repo, &["fetch", "-q", "origin"])?;
    let tree = git(
        &f.repo,
        &["merge-tree", "--write-tree", "origin/dev", &head],
    )?;
    let sha = git(&f.repo, &["commit-tree", &tree, "-m", "previous candidate"])?;
    let template = f
        .runs(&app)
        .await?
        .into_iter()
        .find(|r| r.sha == head)
        .ok_or("own run")?;
    for (reference, state) in [
        (CiRef::Train(vec![5]), RunState::Error),
        (CiRef::Train(vec![6]), RunState::Cancelled),
        (CiRef::PullRequest(7), RunState::Failed),
    ] {
        app.deps
            .store
            .insert_ci_run(
                &CiRunRecord {
                    id: format!("earlier-{}", state.as_str()),
                    reference,
                    sha: sha.clone(),
                    state,
                    status_for: Some(state),
                    ..template.clone()
                },
                &[],
            )
            .await?;
    }
    app.merge_armed(&f.sandbox).await;
    let run = f
        .train_runs(&app)
        .await?
        .into_iter()
        .find(|r| r.reference == CiRef::Train(vec![1]))
        .ok_or("the candidate must get its own run")?;
    assert_eq!(
        git(&f.repo, &["rev-parse", &format!("{}^{{tree}}", run.sha)])?,
        tree
    );
    assert_eq!(run.state, RunState::Queued);
    assert_eq!(run_candidate(&f, &app, &run.id).await?, RunState::Passed);
    Ok(())
}

/// SPEC-497 MQW-003: the member a bisection left out is told only when the others passed without
/// it. Here #1 fails on the new base by itself: the candidate of two fails, #1 alone fails and is
/// told, and #2, blamed for nothing, merges on a candidate of its own.
#[tokio::test]
async fn a_member_left_out_is_blamed_only_when_the_others_pass_without_it() -> TestResult {
    let f = fixture().await?;
    let app = f.h.app.clone();
    let sandbox = app.deps.store.sandbox(&f.sandbox).await?.ok_or("s1")?;
    let one = f.task(1, &[("clash-1", "x\n")]).await?;
    let two = f.task(2, &[("c.txt", "x\n")]).await?;
    for (n, worktree) in [(1, &one), (2, &two)] {
        f.arm(n, worktree).await?;
        f.record_passed(&app, n, &git(worktree, &["rev-parse", "HEAD"])?)
            .await?;
    }
    f.advance_dev("clash-2")?;
    let outcome = |n: u32| {
        let worktree = if n == 1 { &one } else { &two };
        let f = &f;
        async move {
            Ok::<_, Box<dyn std::error::Error>>(
                f.wait(n, worktree).await?["merge_outcome"]
                    .as_str()
                    .unwrap_or_default()
                    .to_owned(),
            )
        }
    };
    f.decide_until(&app, || async {
        app.ci_sync_sandbox(&sandbox).await?;
        f.settle(&app).await?;
        Ok(f.merged(&app, 2).await? && !outcome(1).await?.is_empty())
    })
    .await?;
    assert!(!f.merged(&app, 1).await?);
    assert!(
        outcome(1)
            .await?
            .contains("merged into the current `dev` it fails")
            && outcome(1)
                .await?
                .contains(yulius_domain::review::KEEP_APPROVAL),
        "{}",
        outcome(1).await?
    );
    assert!(
        !outcome(2).await?.contains("dropped"),
        "{}",
        outcome(2).await?
    );
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
        f.record_passed(&app, n, &git(worktree, &["rev-parse", "HEAD"])?)
            .await?;
    }
    f.advance_dev("other.txt")?;
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
    let registry = app.config.data_dir.join(MERGE_TRAINS_FILE);
    let saved: Value = serde_json::from_str(&f.trains_file(&app))?;
    let (_, activation) = saved_armed(&app, 3)?.ok_or("excluded arm")?;
    let original = &saved[f.sandbox.as_str()]["train"]["left_out"][0];
    assert_eq!(original["head"], third);
    assert_eq!(original["armed_at"], activation);
    for (excluded_head, excluded_arm) in [
        (json!("earlier"), json!(activation)),
        (json!(third), json!(activation - 1)),
        (Value::Null, Value::Null),
    ] {
        let mut older = saved.clone();
        let exclusion = &mut older[f.sandbox.as_str()]["train"]["left_out"][0];
        exclusion["head"] = excluded_head;
        exclusion["armed_at"] = excluded_arm;
        std::fs::write(&registry, serde_json::to_vec(&older)?)?;
        let wait = f.wait(3, &worktrees[2]).await?;
        assert!(
            wait["merge_train"]
                .as_str()
                .is_some_and(|note| note.contains("#3 waits behind merge train #1, #2")
                    && !note.contains("shared.txt")),
            "{wait}"
        );
    }
    std::fs::write(&registry, serde_json::to_vec(&saved)?)?;

    app.ci_sync_sandbox(&app.deps.store.sandbox(&f.sandbox).await?.ok_or("s1")?)
        .await?;
    f.settle(&app).await?;
    f.decide_until(&app, || async {
        Ok(f.merged(&app, 1).await?
            && f.merged(&app, 2).await?
            && f.wait(3, &worktrees[2]).await?["merge_armed"] == false)
    })
    .await?;
    // #3 passed alone, yet #1 and #2 (armed first) went first.
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
    // Behind #{merged} now, it is tested on top of it (SPEC-441 MQ-003), its head untouched.
    let trains = f.train_runs(&app).await?;
    assert_eq!(
        trains.iter().map(|r| r.reference.key()).collect::<Vec<_>>(),
        [format!("train/{other}")],
        "#{other} after #{merged}"
    );
    assert_eq!(
        git(&f.origin, &["rev-parse", &format!("feat/{other}-train")])?,
        heads[other as usize - 1]
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
    // Once idle it is tested on top of the moved base (SPEC-441 MQ-003), and merges on green.
    let trains = f.train_runs(&app).await?;
    assert_eq!(trains.len(), 1, "{trains:?}");
    assert_eq!(f.wait(1, &worktree).await?["merge_armed"], true);
    f.settle(&app).await?;
    f.decide_until(&app, || async { f.merged(&app, 1).await })
        .await?;
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
    // Armed again with the head its review covered, before any pass: the place is kept and the
    // merge follows the new head (SPEC-441 MQ-006).
    let (_, armed_at) = saved_armed(&app, 1)?.ok_or("armed")?;
    f.h.clock.0.fetch_add(1000, Ordering::SeqCst);
    // Its plan is recorded, so Yulius pushes the new head.
    support::mcp_tool(
        &f.h,
        "tok-t1",
        "steps_set",
        json!({"steps": [{"title": "Make the change", "status": "done"}]}),
    )
    .await?;
    let again = support::mcp_tool(
        &f.h,
        "tok-t1",
        "delivery_merge",
        json!({"reviewed_head": armed, "review_summary": "approve"}),
    )
    .await?;
    assert_eq!(support::tool_text(&again)["armed"], true, "{again}");
    assert_eq!(saved_armed(&app, 1)?, Some((merged.clone(), armed_at)));

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
        f.record_passed(&f.h.app, n, &git(worktree, &["rev-parse", "HEAD"])?)
            .await?;
    }
    f.advance_dev("other.txt")?;
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
    // GitHub will not show #1's new head before the next restart.
    *f.h.github.stale_reads.lock().map_err(|_| "lock")? = u32::MAX;
    let armed_head = git(&worktrees[0], &["rev-parse", "HEAD"])?;
    f.settle(&second).await?;
    // #1 brought up to date and pushed, its merge waiting for GitHub.
    f.decide_until(&second, || async {
        Ok(
            git(&f.origin, &["rev-parse", "feat/1-train"])? != armed_head
                && f.trains_file(&second).contains("\"waiting_since\":1"),
        )
    })
    .await?;
    assert!(!f.merged(&second, 1).await?);
    let moved = git(&worktrees[0], &["rev-parse", "HEAD"])?;
    assert_eq!(
        git(&f.origin, &["rev-parse", "feat/1-train"])?,
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

/// SPEC-441 MQ-001: the integration branch moving while an armed pull request's own run (its
/// pre-check) is queued or running changes nothing: Yulius does not merge the base into it, so
/// the run is not superseded. Its merge candidate runs beside it (SPEC-493 MQC-001), and once
/// both passed the pull request lands on top of the moved base.
#[tokio::test]
async fn a_moving_base_never_restarts_a_pre_check() -> TestResult {
    let f = fixture().await?;
    let app = f.h.app.clone();
    let worktree = f.task(1, &[("a.txt", "x\n")]).await?;
    let sandbox = app.deps.store.sandbox(&f.sandbox).await?.ok_or("s1")?;
    app.ci_sync_sandbox(&sandbox).await?;
    f.arm(1, &worktree).await?;
    let head = git(&worktree, &["rev-parse", "HEAD"])?;
    // Another pull request merged meanwhile; Yulius's CI round fetched it.
    f.advance_dev("other.txt")?;
    app.ci_sync_sandbox(&sandbox).await?;
    app.merge_armed(&f.sandbox).await;
    app.ci_sync_sandbox(&sandbox).await?;
    assert_eq!(git(&worktree, &["rev-parse", "HEAD"])?, head, "untouched");
    assert_eq!(git(&f.origin, &["rev-parse", "feat/1-train"])?, head);
    let runs = f.runs(&app).await?;
    let pre_check: Vec<&CiRunRecord> = runs
        .iter()
        .filter(|r| r.reference == CiRef::PullRequest(1))
        .collect();
    assert_eq!(pre_check.len(), 1, "{runs:?}");
    assert_eq!(pre_check[0].sha, head);
    assert!(pre_check[0].state.is_active(), "{runs:?}");
    // Its candidate goes beside the pre-check (SPEC-493 MQC-001); nothing is merged into it.
    assert_eq!(f.train_runs(&app).await?.len(), 1, "{runs:?}");
    let state = f.wait(1, &worktree).await?;
    assert_eq!(state["merge_armed"], true);

    // The pre-check finishes, then the pull request lands on top of the moved base.
    f.settle(&app).await?;
    f.decide_until(&app, || async {
        app.ci_sync_sandbox(&sandbox).await?;
        f.settle(&app).await?;
        f.merged(&app, 1).await
    })
    .await?;
    let runs = f.runs(&app).await?;
    assert!(
        runs.iter()
            .filter(|r| matches!(r.reference, CiRef::PullRequest(_) | CiRef::Train(_)))
            .all(|r| r.state != RunState::Cancelled),
        "no pull request's run cancelled: {runs:?}"
    );
    let first = runs
        .iter()
        .find(|r| r.reference == CiRef::PullRequest(1) && r.sha == head)
        .ok_or("pre-check")?;
    assert_eq!(first.state, RunState::Passed, "{first:?}");
    let dev = git(&f.origin, &["show", "dev:other.txt"])?;
    assert_eq!(dev, "other");
    Ok(())
}

/// SPEC-493 MQC-001 (AC-13): two armed pull requests behind their base whose own runs are still
/// queued: their merge candidate forms at once and its run goes beside their own runs, nothing is
/// merged into them meanwhile, and both merge once the candidate and their own runs passed: one CI
/// cycle, not an own run and then a candidate.
#[tokio::test]
async fn a_candidate_runs_beside_its_members_own_runs() -> TestResult {
    let f = fixture().await?;
    let app = f.h.app.clone();
    let sandbox = app.deps.store.sandbox(&f.sandbox).await?.ok_or("s1")?;
    let one = f.task(1, &[("a.txt", "x\n")]).await?;
    let two = f.task(2, &[("b.txt", "x\n")]).await?;
    app.ci_sync_sandbox(&sandbox).await?;
    f.arm(1, &one).await?;
    f.arm(2, &two).await?;
    let heads = [
        git(&one, &["rev-parse", "HEAD"])?,
        git(&two, &["rev-parse", "HEAD"])?,
    ];
    f.advance_dev("other.txt")?;
    app.ci_sync_sandbox(&sandbox).await?;
    app.merge_armed(&f.sandbox).await;
    let trains = f.train_runs(&app).await?;
    assert_eq!(
        trains.len(),
        1,
        "a candidate beside the own runs: {trains:?}"
    );
    let runs = f.runs(&app).await?;
    for (n, head) in (1..).zip(&heads) {
        let own: Vec<&CiRunRecord> = runs
            .iter()
            .filter(|r| r.reference == CiRef::PullRequest(n))
            .collect();
        assert_eq!(own.len(), 1, "{runs:?}");
        assert!(own[0].state.is_active() && &own[0].sha == head, "{own:?}");
    }
    assert_eq!(
        git(&one, &["rev-parse", "HEAD"])?,
        heads[0],
        "nothing merged into it"
    );
    f.decide_until(&app, || async {
        app.ci_sync_sandbox(&sandbox).await?;
        f.settle(&app).await?;
        Ok(f.merged(&app, 1).await? && f.merged(&app, 2).await?)
    })
    .await?;
    let executed = f.executed(&app).await?;
    let own = executed
        .iter()
        .filter(|r| matches!(r.reference, CiRef::PullRequest(_)))
        .count();
    let candidates = executed
        .iter()
        .filter(|r| matches!(r.reference, CiRef::Train(_)))
        .count();
    assert_eq!((own, candidates), (2, 1), "{executed:#?}");
    // Each merged only once its own run had passed too.
    let runs = f.runs(&app).await?;
    for (n, head) in (1..).zip(&heads) {
        assert!(
            runs.iter().any(|r| r.reference == CiRef::PullRequest(n)
                && &r.sha == head
                && r.state == RunState::Passed),
            "#{n}'s own run: {runs:#?}"
        );
    }
    Ok(())
}

/// SPEC-497 MQW-002: a candidate that passed merges its members at once, even while a member's
/// own run still runs: the candidate's run tested the exact merged result. The own run is held
/// running (no process takes it), so only the candidate's run executes.
#[tokio::test]
async fn a_passed_candidate_merges_while_a_members_own_run_goes() -> TestResult {
    let f = fixture().await?;
    let app = f.h.app.clone();
    let sandbox = app.deps.store.sandbox(&f.sandbox).await?.ok_or("s1")?;
    let one = f.task(1, &[("a.txt", "x\n")]).await?;
    app.ci_sync_sandbox(&sandbox).await?;
    f.arm(1, &one).await?;
    let own = hold_own_run(&f, &app, 1).await?;
    f.advance_dev("other.txt")?;
    app.ci_sync_sandbox(&sandbox).await?;
    app.merge_armed(&f.sandbox).await;
    let train = f
        .train_runs(&app)
        .await?
        .into_iter()
        .next()
        .ok_or("no candidate")?;
    assert_eq!(run_candidate(&f, &app, &train.id).await?, RunState::Passed);
    f.decide_until(&app, || async {
        app.ci_sync_sandbox(&sandbox).await?;
        f.settle_but(&app, &own).await?;
        f.merged(&app, 1).await
    })
    .await?;
    let own = app.deps.store.ci_run(&own).await?.ok_or("own run")?;
    assert_ne!(
        own.state,
        RunState::Passed,
        "merged on the candidate's run alone"
    );
    // Quartio-v2 #331 / PR #1481: the checked head and the landed squash have the same
    // tree, but the checked head is not part of dev's ancestry.
    let checked = git(&one, &["rev-parse", "HEAD"])?;
    let landed = git(&f.origin, &["rev-parse", "dev"])?;
    assert_eq!(f.origin_tree(&checked)?, f.origin_tree(&landed)?);
    assert_eq!(
        Origin(f.origin.clone()).contains(&checked, &landed),
        Some(false)
    );
    let ctx = owner_ctx(&f, 1).await?;
    let merge = app.merge_state_for_person(&ctx, "t1").await?;
    let text = merge["last"]["text"].as_str().ok_or("last train verdict")?;
    assert!(
        text.contains(&format!("into `dev` at {}", &landed[..9])),
        "{text}"
    );
    assert!(
        text.contains(&format!("PR head {}", &checked[..9])),
        "{text}"
    );
    assert!(text.contains("in a merge train."), "{text}");
    assert!(
        text.contains(&format!("candidate at {}", &train.sha[..9])),
        "{text}"
    );
    let delivery = app.task_delivery(&ctx, "t1").await?;
    assert!(
        delivery.notes.iter().any(
            |note| note.contains(&format!("into `dev` at {}", &landed[..9]))
                && note.contains(&format!("PR head {}", &checked[..9]))
        ),
        "{:?}",
        delivery.notes
    );
    Ok(())
}

/// Holds pull request `n`'s own run running, as if a job of it took long: no process takes it.
async fn hold_own_run(f: &Fixture, app: &App, n: u64) -> BoxResult<String> {
    use yulius_app::ci::CiRunUpdate;
    let own = f
        .runs(app)
        .await?
        .into_iter()
        .find(|r| r.reference == CiRef::PullRequest(n))
        .ok_or("own run")?;
    let started = CiRunUpdate {
        started_at: Some(app.now()),
        ..CiRunUpdate::default()
    };
    assert!(
        app.deps
            .store
            .transition_ci_run(&own.id, &[RunState::Queued], RunState::Running, &started)
            .await?
    );
    Ok(own.id)
}

/// Ends a held own run in `state`, with its status published.
async fn end_own_run(app: &App, id: &str, state: RunState) -> TestResult {
    use yulius_app::ci::CiRunUpdate;
    let ended = CiRunUpdate {
        ended_at: Some(app.now()),
        ..CiRunUpdate::default()
    };
    assert!(
        app.deps
            .store
            .transition_ci_run(id, &[RunState::Running], state, &ended)
            .await?
    );
    app.ci_retry_statuses().await;
    Ok(())
}

/// Dispatches CI until the merge candidate's run `id` ended, and returns how.
async fn run_candidate(f: &Fixture, app: &Arc<App>, id: &str) -> BoxResult<RunState> {
    loop {
        app.ci_dispatch().await;
        let run = app.deps.store.ci_run(id).await?.ok_or("run")?;
        if !run.state.is_active() {
            return Ok(run.state);
        }
        if tokio::time::Instant::now() > f.deadline {
            return Err(format!(
                "candidate {id} at {} is still {}",
                run.sha,
                run.state.as_str()
            )
            .into());
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

async fn dissolved(app: &App) -> BoxResult<Vec<String>> {
    Ok(yulius_app::ports::store::AuditStore::audit_details(
        app.deps.store.as_ref(),
        "app",
        "delivery.train.dissolved",
        20,
    )
    .await?)
}

/// SPEC-497 MQW-002 (was review round 1 of SPEC-493 fix 4): a member whose own run is stuck holds
/// nothing back: once the candidate passed, both members merge, with no time limit to wait out
/// and nothing dissolved.
#[tokio::test]
async fn a_stuck_own_run_holds_nothing_back() -> TestResult {
    let f = fixture().await?;
    let app = f.h.app.clone();
    let sandbox = app.deps.store.sandbox(&f.sandbox).await?.ok_or("s1")?;
    let one = f.task(1, &[("a.txt", "x\n")]).await?;
    let two = f.task(2, &[("b.txt", "x\n")]).await?;
    app.ci_sync_sandbox(&sandbox).await?;
    f.arm(1, &one).await?;
    f.arm(2, &two).await?;
    let stuck = hold_own_run(&f, &app, 1).await?;
    f.advance_dev("other.txt")?;
    app.ci_sync_sandbox(&sandbox).await?;
    app.merge_armed(&f.sandbox).await;
    let train = f
        .train_runs(&app)
        .await?
        .into_iter()
        .next()
        .ok_or("no candidate")?;
    assert_eq!(run_candidate(&f, &app, &train.id).await?, RunState::Passed);
    f.decide_until(&app, || async {
        app.ci_sync_sandbox(&sandbox).await?;
        f.settle_but(&app, &stuck).await?;
        Ok(f.merged(&app, 1).await? && f.merged(&app, 2).await?)
    })
    .await?;
    assert!(
        dissolved(&app).await?.is_empty(),
        "{:?}",
        dissolved(&app).await?
    );
    assert_eq!(f.train_runs(&app).await?.len(), 1, "one candidate run");
    Ok(())
}

/// SPEC-497 MQW-001: a member whose own run is cancelled or errors (the infrastructure twice, a
/// time limit) stays in the candidate: that says nothing about its change, and the candidate's
/// run, which goes on, decides it. Both merge on it.
#[tokio::test]
async fn a_member_whose_own_run_is_cancelled_or_errors_stays_in_the_candidate() -> TestResult {
    let f = fixture().await?;
    let app = f.h.app.clone();
    let sandbox = app.deps.store.sandbox(&f.sandbox).await?.ok_or("s1")?;
    let one = f.task(1, &[("a.txt", "x\n")]).await?;
    let two = f.task(2, &[("b.txt", "x\n")]).await?;
    app.ci_sync_sandbox(&sandbox).await?;
    f.arm(1, &one).await?;
    f.arm(2, &two).await?;
    let held_one = hold_own_run(&f, &app, 1).await?;
    let held_two = hold_own_run(&f, &app, 2).await?;
    f.advance_dev("other.txt")?;
    app.ci_sync_sandbox(&sandbox).await?;
    app.merge_armed(&f.sandbox).await;
    let train = f
        .train_runs(&app)
        .await?
        .into_iter()
        .next()
        .ok_or("no candidate")?;
    end_own_run(&app, &held_one, RunState::Cancelled).await?;
    end_own_run(&app, &held_two, RunState::Error).await?;
    app.merge_armed(&f.sandbox).await;
    assert!(
        dissolved(&app).await?.is_empty(),
        "{:?}",
        dissolved(&app).await?
    );
    let run = app.deps.store.ci_run(&train.id).await?.ok_or("run")?;
    assert!(run.state.is_active(), "not cancelled: {run:?}");
    assert_eq!(run_candidate(&f, &app, &train.id).await?, RunState::Passed);
    f.decide_until(&app, || async {
        app.ci_sync_sandbox(&sandbox).await?;
        f.settle(&app).await?;
        Ok(f.merged(&app, 1).await? && f.merged(&app, 2).await?)
    })
    .await?;
    Ok(())
}

/// SPEC-497 MQW-001: a candidate formed again with the same members on the same base (here after
/// a passed one waited too long for a busy member) reuses the passed run of its tree: no second
/// run is made, and it merges once the member is idle.
#[tokio::test]
async fn a_candidate_formed_again_reuses_its_trees_passed_run() -> TestResult {
    formed_again(Project::Standard).await
}

/// SPEC-497 MQW-001 (review round 1): the first candidate's run skipped a job for its `paths`, so
/// it is no source: the same candidate formed again gets a run of its own.
#[tokio::test]
async fn a_run_that_skipped_a_paths_job_is_no_source_for_a_candidate() -> TestResult {
    formed_again(Project::Paths).await
}

/// The candidate of one pull request passes, waits too long for its busy member and is formed
/// again; `reused`: whether the second takes the first's passed run.
async fn formed_again(project: Project) -> TestResult {
    let reused = project != Project::Paths;
    let f = fixture_with(project, None, BUDGET).await?;
    let app = f.h.app.clone();
    let worktree = f.task(1, &[("f1.txt", "x\n")]).await?;
    f.arm(1, &worktree).await?;
    f.record_passed(&app, 1, &git(&worktree, &["rev-parse", "HEAD"])?)
        .await?;
    f.advance_dev("other-1.txt")?;
    app.merge_armed(&f.sandbox).await;
    let id = TaskId::parse("t1")?;
    f.h.store
        .transition_task(
            &id,
            None,
            TaskStatus::Running,
            Some(Activity::Working),
            app.now(),
        )
        .await?;
    f.settle(&app).await?;
    app.merge_armed(&f.sandbox).await;
    assert!(!f.merged(&app, 1).await?);
    let first = f.executed(&app).await?;
    let first: Vec<&CiRunRecord> = first
        .iter()
        .filter(|r| matches!(r.reference, CiRef::Train(_)))
        .collect();
    assert_eq!(first.len(), 1, "{first:?}");
    // Past the wait limit the candidate ends; once the agent is idle it is formed again.
    f.h.clock.0.fetch_add(
        yulius_app::merge_train::WAIT_LIMIT_MS + 1000,
        Ordering::SeqCst,
    );
    app.merge_armed(&f.sandbox).await;
    assert!(!f.trains_file(&app).contains("\"train\":{"), "ended");
    f.h.store
        .transition_task(
            &id,
            None,
            TaskStatus::Running,
            Some(Activity::Waiting),
            app.now(),
        )
        .await?;
    f.decide_until(&app, || async {
        f.settle(&app).await?;
        f.merged(&app, 1).await
    })
    .await?;
    let trains = f.train_runs(&app).await?;
    assert_eq!(trains.len(), 2, "formed again: {trains:?}");
    let again = trains
        .iter()
        .find(|r| r.id != first[0].id)
        .ok_or("the second candidate")?;
    assert_eq!(again.state, RunState::Passed, "{again:?}");
    let jobs = app.deps.store.ci_jobs(&again.id, true).await?;
    let copied = !jobs.is_empty() && jobs.iter().all(|j| j.output.contains("not run again"));
    assert_eq!(copied, reused, "{jobs:?}");
    let executed = f.executed(&app).await?;
    assert_eq!(
        executed
            .iter()
            .filter(|r| matches!(r.reference, CiRef::Train(_)))
            .count(),
        if reused { 1 } else { 2 },
        "candidate runs executed"
    );
    Ok(())
}

/// SPEC-497 MQW-003: two armed pull requests that both change files under a pattern the
/// project's `[merge_queue] apart` names never share a candidate: the first goes into one, the
/// other waits and merges on the next, on top of it. A third that changes nothing there joins the
/// first. No candidate fails on their combination.
#[tokio::test]
async fn derived_files_are_changed_one_candidate_at_a_time() -> TestResult {
    let f = fixture_with(Project::Apart, None, BUDGET).await?;
    let app = f.h.app.clone();
    let mut worktrees = Vec::new();
    for (n, file) in [(1u32, "gen/a.txt"), (2, "gen/b.txt"), (3, "c.txt")] {
        worktrees.push(f.task(n, &[(file, "x\n")]).await?);
    }
    for (n, worktree) in (1u32..).zip(&worktrees) {
        f.arm(n, worktree).await?;
        f.record_passed(&app, n, &git(worktree, &["rev-parse", "HEAD"])?)
            .await?;
    }
    f.advance_dev("other.txt")?;
    let sandbox = app.deps.store.sandbox(&f.sandbox).await?.ok_or("s1")?;
    f.decide_until(&app, || async {
        app.ci_sync_sandbox(&sandbox).await?;
        f.settle(&app).await?;
        Ok(f.merged(&app, 1).await? && f.merged(&app, 2).await? && f.merged(&app, 3).await?)
    })
    .await?;
    let mut trains: Vec<String> = f
        .train_runs(&app)
        .await?
        .iter()
        .map(|r| r.reference.key())
        .collect();
    trains.sort();
    assert_eq!(
        trains,
        ["train/1+3", "train/2"],
        "held apart, then on its own"
    );
    let formed = yulius_app::ports::store::AuditStore::audit_details(
        app.deps.store.as_ref(),
        "app",
        yulius_domain::merge_queue::FORMED,
        10,
    )
    .await?;
    assert!(
        formed.iter().any(|d| d
            .contains("held apart for the next candidate (derived files): #2 (with #1, gen/**)")),
        "{formed:?}"
    );
    Ok(())
}

/// SPEC-497 SHR-002 (slice D review round 1): a merge candidate's run takes a lane its members'
/// own runs took, and its job's output says so.
#[tokio::test]
async fn a_candidates_run_takes_a_lane_of_its_members() -> TestResult {
    let f = fixture_with(Project::Standard, Some(2), BUDGET).await?;
    let app = f.h.app.clone();
    let sandbox = app.deps.store.sandbox(&f.sandbox).await?.ok_or("s1")?;
    let one = f.task(1, &[("a.txt", "x\n")]).await?;
    let two = f.task(2, &[("b.txt", "x\n")]).await?;
    // `dev` moves first, so nothing merges on its own; the members' own runs and `dev`'s all run
    // and end before anything is armed (review round 2: a candidate formed by a run's end while
    // `dev`'s run took a member's lane). Of two lanes, one still names a member's run.
    f.advance_dev("other.txt")?;
    app.ci_sync_sandbox(&sandbox).await?;
    f.settle(&app).await?;
    f.arm(1, &one).await?;
    f.arm(2, &two).await?;
    app.merge_armed(&f.sandbox).await;
    let train = f
        .train_runs(&app)
        .await?
        .into_iter()
        .next()
        .ok_or("no candidate")?;
    run_candidate(&f, &app, &train.id).await?;
    let jobs = app.deps.store.ci_jobs(&train.id, true).await?;
    let output = jobs.first().map(|j| j.output.clone()).unwrap_or_default();
    let lane = output
        .lines()
        .find(|l| l.starts_with("[yulius] lane "))
        .unwrap_or_default();
    assert!(
        (lane.contains("last taken by pr/1,") || lane.contains("last taken by pr/2,"))
            && lane.ends_with("which this run builds on"),
        "{output}"
    );
    Ok(())
}

/// SPEC-497 MQW-002 (slice C review round 2): a member's own run that fails while the passed
/// candidate merges the members before it is read again before its own merge: #1 merges, #2's
/// own run fails exactly then (the pass held at its pause point), and #2 is not merged.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_member_whose_own_run_fails_during_the_merges_is_not_merged() -> TestResult {
    let f = fixture().await?;
    let app = f.h.app.clone();
    let sandbox = app.deps.store.sandbox(&f.sandbox).await?.ok_or("s1")?;
    let one = f.task(1, &[("a.txt", "x\n")]).await?;
    let two = f.task(2, &[("b.txt", "x\n")]).await?;
    app.ci_sync_sandbox(&sandbox).await?;
    f.arm(1, &one).await?;
    f.arm(2, &two).await?;
    let held = hold_own_run(&f, &app, 2).await?;
    let (at, go) = support::hold_point(yulius_app::race::Point::TrainMemberMerging, "t2");
    f.advance_dev("other.txt")?;
    app.ci_sync_sandbox(&sandbox).await?;
    app.merge_armed(&f.sandbox).await;
    let train = f
        .train_runs(&app)
        .await?
        .into_iter()
        .next()
        .ok_or("no candidate")?;
    assert_eq!(run_candidate(&f, &app, &train.id).await?, RunState::Passed);
    // A pass merges #1 and stops before #2; #2's own run fails there.
    let pass = {
        let app = Arc::clone(&app);
        let sandbox = f.sandbox.clone();
        tokio::spawn(async move { app.merge_armed(&sandbox).await })
    };
    support::point_reached(at).await?;
    assert!(f.merged(&app, 1).await?, "#1 merged first");
    end_own_run(&app, &held, RunState::Failed).await?;
    go.send(())?;
    pass.await?;
    assert!(!f.merged(&app, 2).await?, "merged on the candidate");
    let gone = dissolved(&app).await?;
    assert!(
        gone.iter()
            .any(|d| d.contains("#2's own run") && d.contains("(why: member_failed)")),
        "{gone:?}"
    );
    Ok(())
}

/// SPEC-497 MQW-001, MQW-002 (review round 1): a member whose own run fails after its candidate
/// passed is not merged on the candidate: it leaves with `member_failed`, its armed merge ends,
/// and the other merges on a candidate formed again.
#[tokio::test]
async fn a_failed_own_run_beats_a_passed_candidate() -> TestResult {
    let f = fixture().await?;
    let app = f.h.app.clone();
    let sandbox = app.deps.store.sandbox(&f.sandbox).await?.ok_or("s1")?;
    let one = f.task(1, &[("a.txt", "x\n")]).await?;
    let two = f.task(2, &[("b.txt", "x\n")]).await?;
    app.ci_sync_sandbox(&sandbox).await?;
    f.arm(1, &one).await?;
    f.arm(2, &two).await?;
    let held = hold_own_run(&f, &app, 2).await?;
    f.advance_dev("other.txt")?;
    app.ci_sync_sandbox(&sandbox).await?;
    app.merge_armed(&f.sandbox).await;
    let train = f
        .train_runs(&app)
        .await?
        .into_iter()
        .next()
        .ok_or("no candidate")?;
    assert_eq!(run_candidate(&f, &app, &train.id).await?, RunState::Passed);
    end_own_run(&app, &held, RunState::Failed).await?;
    app.merge_armed(&f.sandbox).await;
    assert!(!f.merged(&app, 2).await?, "merged on the candidate");
    let gone = dissolved(&app).await?;
    assert!(
        gone.iter()
            .any(|d| d.contains("#2's own run") && d.contains("(why: member_failed)")),
        "{gone:?}"
    );
    f.decide_until(&app, || async {
        app.ci_sync_sandbox(&sandbox).await?;
        f.settle(&app).await?;
        Ok(f.merged(&app, 1).await? && f.wait(2, &two).await?["merge_armed"] == false)
    })
    .await?;
    assert!(!f.merged(&app, 2).await?);
    Ok(())
}

/// Review round 1 of SPEC-493 fix 4: a failed candidate waits for a member's own run that still
/// goes (it may say why) for at most the wait limit, then it is bisected as before.
#[tokio::test]
async fn a_failed_candidate_waits_for_own_runs_within_the_wait_limit() -> TestResult {
    let f = fixture().await?;
    let app = f.h.app.clone();
    let sandbox = app.deps.store.sandbox(&f.sandbox).await?.ok_or("s1")?;
    // Each passes alone; together they fail the check.
    let one = f.task(1, &[("clash-1", "x\n")]).await?;
    let two = f.task(2, &[("clash-2", "x\n")]).await?;
    app.ci_sync_sandbox(&sandbox).await?;
    f.arm(1, &one).await?;
    f.arm(2, &two).await?;
    let held = hold_own_run(&f, &app, 2).await?;
    f.advance_dev("other.txt")?;
    app.ci_sync_sandbox(&sandbox).await?;
    app.merge_armed(&f.sandbox).await;
    let train = f
        .train_runs(&app)
        .await?
        .into_iter()
        .next()
        .ok_or("no candidate")?;
    assert_eq!(run_candidate(&f, &app, &train.id).await?, RunState::Failed);
    f.settle_but(&app, &held).await?;
    app.merge_armed(&f.sandbox).await;
    assert!(
        dissolved(&app).await?.is_empty(),
        "it waits for #2's own run"
    );
    assert!(!app.decidable_in(&sandbox).await);
    f.h.clock.0.fetch_add(
        yulius_app::merge_train::WAIT_LIMIT_MS + 1000,
        Ordering::SeqCst,
    );
    app.merge_armed(&f.sandbox).await;
    let gone = dissolved(&app).await?;
    assert!(
        gone.iter().any(|d| d.contains("(why: bisected)")),
        "{gone:?}"
    );
    end_own_run(&app, &held, RunState::Passed).await?;
    Ok(())
}

/// Review round 3 of SPEC-493 fix 5: a run that passed when it was queued (every job skipped
/// for its `paths`) never reaches the executor, whose run end starts the merge pass: the
/// dispatcher starts it, so a merge armed before the run merges at once, with no other pass.
#[tokio::test]
async fn a_merge_armed_on_a_run_of_skipped_jobs_happens_at_once() -> TestResult {
    let f = fixture().await?;
    let app = f.h.app.clone();
    let sandbox = app.deps.store.sandbox(&f.sandbox).await?.ok_or("s1")?;
    f.advance_dev_with(
        "yulius.toml",
        "[[ci.jobs]]\nname = \"check\"\ncommand = \"true\"\npaths = [\"never/**\"]\n",
    )?;
    // The integration branch's own run ends first: no other run's end starts a pass below.
    app.ci_sync_sandbox(&sandbox).await?;
    f.settle(&app).await?;
    let one = f.task(1, &[("a.txt", "x\n")]).await?;
    f.arm(1, &one).await?;
    app.ci_sync_sandbox(&sandbox).await?;
    let head = git(&one, &["rev-parse", "HEAD"])?;
    let runs = f.runs(&app).await?;
    assert!(
        runs.iter()
            .any(|r| r.sha == head && r.state == RunState::Passed),
        "passed when queued: {runs:?}"
    );
    assert!(
        !runs.iter().any(|r| r.state.is_active()),
        "no other run whose end would start a pass: {runs:?}"
    );
    // The dispatcher alone: no merge pass is called by the test.
    app.ci_dispatch().await;
    let mut merged = false;
    for _ in 0..200 {
        if f.merged(&app, 1).await? {
            merged = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(merged, "the armed merge waited for another pass");
    Ok(())
}

/// SPEC-493 MQC-002 (AC-13): a member whose own run fails leaves the candidate: the candidate is
/// formed again from the other, which merges, and the failed member's armed merge ends with its
/// run named, as a failed own run ends it outside a candidate.
#[tokio::test]
async fn a_member_whose_own_run_fails_leaves_the_candidate() -> TestResult {
    failed_member_leaves(false).await
}

/// Issue #1398: the member's run fails during the decision about a red candidate.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_member_failing_while_a_red_candidate_is_decided_leaves_the_candidate() -> TestResult {
    failed_member_leaves(true).await
}

async fn failed_member_leaves(fail_during_decision: bool) -> TestResult {
    let f = fixture().await?;
    let app = f.h.app.clone();
    let sandbox = app.deps.store.sandbox(&f.sandbox).await?.ok_or("s1")?;
    let one = f.task(1, &[("a.txt", "x\n")]).await?;
    // Its own change fails the check alone.
    let two = f.task(2, &[("clash-1", "x\n"), ("clash-2", "x\n")]).await?;
    app.ci_sync_sandbox(&sandbox).await?;
    f.arm(1, &one).await?;
    f.arm(2, &two).await?;
    let own = if fail_during_decision {
        Some(hold_own_run(&f, &app, 2).await?)
    } else {
        None
    };
    f.advance_dev("other.txt")?;
    app.ci_sync_sandbox(&sandbox).await?;
    app.merge_armed(&f.sandbox).await;
    let trains = f.train_runs(&app).await?;
    assert_eq!(trains.len(), 1);
    if let Some(own) = own {
        let candidate = &trains[0].id;
        f.settle_but(&app, &own).await?;
        assert_eq!(
            app.deps
                .store
                .ci_run(candidate)
                .await?
                .ok_or("candidate")?
                .state,
            RunState::Failed
        );

        // The pass sees the member running. Its failure arrives before the pass decides
        // whether to bisect the candidate, the order that produced the quarantine's audit.
        let (at, go) = support::hold_point(yulius_app::race::Point::FailedTrainDeciding, candidate);
        let pass = {
            let app = Arc::clone(&app);
            let sandbox = f.sandbox.clone();
            tokio::spawn(async move { app.merge_armed(&sandbox).await })
        };
        support::point_reached(at).await?;
        end_own_run(&app, &own, RunState::Failed).await?;
        go.send(())?;
        pass.await?;
    }
    f.decide_until(&app, || async {
        app.ci_sync_sandbox(&sandbox).await?;
        f.settle(&app).await?;
        Ok(f.merged(&app, 1).await? && f.wait(2, &two).await?["merge_armed"] == false)
    })
    .await?;
    assert!(!f.merged(&app, 2).await?);
    let outcome = f.wait(2, &two).await?["merge_outcome"]
        .as_str()
        .unwrap_or_default()
        .to_owned();
    assert!(outcome.contains("failed"), "{outcome}");
    let dissolved = yulius_app::ports::store::AuditStore::audit_details(
        app.deps.store.as_ref(),
        "app",
        "delivery.train.dissolved",
        10,
    )
    .await?;
    assert!(
        dissolved
            .iter()
            .any(|d| d.contains("#2's own run") && d.contains("(why: member_failed)")),
        "{dissolved:?}"
    );
    Ok(())
}

/// SPEC-441 MQ-001, MQ-003, MQ-010: three pull requests from one base turn green one after
/// another (one CI lane), and each merge moves the base. No pull request's run and no merge
/// candidate is cancelled, and all three land in order after their 3 pre-checks and at most 2
/// candidate runs.
async fn three_green_in_sequence(project: Project) -> TestResult {
    let f = fixture_with(project, Some(1), BUDGET).await?;
    let app = f.h.app.clone();
    let sandbox = app.deps.store.sandbox(&f.sandbox).await?.ok_or("s1")?;
    let mut worktrees = Vec::new();
    // Opened one after another: their pre-checks queue (and, in one lane, pass) in that order.
    for (n, file) in [(1, "a.txt"), (2, "b.txt"), (3, "c.txt")] {
        worktrees.push(f.task(n, &[(file, "x\n")]).await?);
        f.h.clock.0.fetch_add(1000, Ordering::SeqCst);
        app.ci_sync_sandbox(&sandbox).await?;
    }
    for (n, worktree) in (1..).zip(&worktrees) {
        f.arm(n, worktree).await?;
    }
    f.decide_until(&app, || async {
        app.ci_sync_sandbox(&sandbox).await?;
        f.settle(&app).await?;
        Ok(f.merged(&app, 1).await? && f.merged(&app, 2).await? && f.merged(&app, 3).await?)
    })
    .await?;
    let method = match project {
        Project::Standard | Project::Paths | Project::Apart => "squash",
        Project::Minimal => "merge",
    };
    assert_eq!(
        f.merges(),
        (1..=3)
            .map(|n| format!("merge_pull #{n} {method}"))
            .collect::<Vec<_>>()
    );
    let runs = f.runs(&app).await?;
    let decides = |r: &&CiRunRecord| matches!(r.reference, CiRef::PullRequest(_) | CiRef::Train(_));
    assert!(
        runs.iter()
            .filter(decides)
            .all(|r| r.state != RunState::Cancelled),
        "no pre-check or candidate cancelled: {runs:#?}"
    );
    let executed = f.executed(&app).await?;
    let pre_checks = executed
        .iter()
        .filter(|r| matches!(r.reference, CiRef::PullRequest(_)))
        .count();
    let candidates = executed
        .iter()
        .filter(|r| matches!(r.reference, CiRef::Train(_)))
        .count();
    assert_eq!(pre_checks, 3, "{executed:#?}");
    assert!((1..=2).contains(&candidates), "{executed:#?}");
    for file in ["a.txt", "b.txt", "c.txt"] {
        git(&f.origin, &["show", &format!("{}:{file}", f.integration)])?;
    }
    Ok(())
}

#[tokio::test]
async fn three_green_pull_requests_land_in_order_without_a_restart() -> TestResult {
    three_green_in_sequence(Project::Standard).await
}

#[tokio::test]
async fn three_green_pull_requests_land_the_same_in_a_minimal_project() -> TestResult {
    three_green_in_sequence(Project::Minimal).await
}

/// The saved armed merge of `t<n>` (DLV-205's file): its head and when it was armed.
fn saved_armed(app: &App, n: u32) -> BoxResult<Option<(String, i64)>> {
    let text = std::fs::read_to_string(
        app.config
            .data_dir
            .join(yulius_app::agent_wait::ARMED_MERGES_FILE),
    )
    .unwrap_or_else(|_| "[]".into());
    let saved: Vec<Value> = serde_json::from_str(&text)?;
    Ok(saved
        .iter()
        .find(|entry| entry["task"] == format!("t{n}"))
        .map(|entry| {
            (
                entry["head"].as_str().unwrap_or_default().to_owned(),
                entry["armed_at"].as_i64().unwrap_or_default(),
            )
        }))
}

/// SPEC-441 MQ-006: when Yulius merges the integration branch into an armed pull request (its
/// merge candidate passed), the merge stays armed on the new head, with its place, while GitHub
/// catches up, and merges once it has.
#[tokio::test]
async fn a_yulius_base_merge_keeps_the_merge_armed() -> TestResult {
    verified_head_during_github_wait(false).await
}

#[tokio::test]
async fn a_transient_pr_title_failure_keeps_the_trains_verified_head() -> TestResult {
    verified_head_during_github_wait(true).await
}

async fn verified_head_during_github_wait(retitle_fails: bool) -> TestResult {
    let f = fixture().await?;
    let app = f.h.app.clone();
    let worktree = f.task(1, &[("a.txt", "x\n")]).await?;
    if retitle_fails {
        // The PR opened with the fallback title. A later conventional commit requires a retitle.
        std::fs::write(worktree.join("fix.txt"), "reviewed repair")?;
        git(&worktree, &["add", "fix.txt"])?;
        git(
            &worktree,
            &["commit", "-qm", "fix(queue): finish the repair"],
        )?;
        git(&worktree, &["push", "-q", "origin", "HEAD:feat/1-train"])?;
    }
    f.arm(1, &worktree).await?;
    let reviewed = git(&worktree, &["rev-parse", "HEAD"])?;
    f.record_passed(&app, 1, &reviewed).await?;
    let (_, armed_at) = saved_armed(&app, 1)?.ok_or("armed")?;
    f.advance_dev("other.txt")?;
    app.merge_armed(&f.sandbox).await;
    assert_eq!(f.train_runs(&app).await?.len(), 1);
    if retitle_fails {
        // GitHub sees the pushed head and refuses title updates until the fixture releases it.
        f.h.github
            .retitle_errors
            .lock()
            .map_err(|_| "retitle errors")?
            .extend((0..8).map(|_| {
                yulius_app::error::AppError::Upstream("GitHub upstream error (HTTP 502)".into())
            }));
    } else {
        *f.h.github.stale_reads.lock().map_err(|_| "stale reads")? = u32::MAX;
    }
    f.settle(&app).await?;
    f.decide_until(&app, || async {
        Ok(git(&f.origin, &["rev-parse", "feat/1-train"])? != reviewed)
    })
    .await?;
    let moved = git(&worktree, &["rev-parse", "HEAD"])?;
    if retitle_fails {
        let outstanding =
            f.h.github
                .retitle_errors
                .lock()
                .map_err(|_| "retitle errors")?
                .len();
        assert!(
            (1..8).contains(&outstanding),
            "the title update must have reached GitHub"
        );
    }
    let state = f.wait(1, &worktree).await?;
    assert_eq!(state["merge_armed"], true, "kept: {state}");
    assert_eq!(state["merge_outcome"], Value::Null, "{state}");
    assert_eq!(saved_armed(&app, 1)?, Some((moved.clone(), armed_at)));
    let train: Value = serde_json::from_str(&f.trains_file(&app))?;
    let held = &train[f.sandbox.as_str()]["train"]["members"][0];
    assert_eq!(held["head"], reviewed, "admission head");
    assert_eq!(
        held["verified_head"], moved,
        "the watcher can identify the moved arm"
    );
    // The saved arm can be ahead of the train checkpoint, including across an upgrade.
    let registry = app.config.data_dir.join(MERGE_TRAINS_FILE);
    let mut legacy = train.clone();
    legacy[f.sandbox.as_str()]["train"]["members"][0]
        .as_object_mut()
        .ok_or("saved member")?
        .remove("verified_head");
    std::fs::write(&registry, serde_json::to_vec(&legacy)?)?;
    let interrupted = f.wait(1, &worktree).await?;
    assert!(
        interrupted["merge_train"]
            .as_str()
            .is_some_and(|note| note.contains("verifies its current head")),
        "{interrupted}"
    );
    std::fs::write(&registry, serde_json::to_vec(&train)?)?;
    let restored = f.wait(1, &worktree).await?;
    assert!(
        restored["merge_train"].as_str().is_some_and(
            |note| note.contains("number 1 of 1") && note.contains("combined result passed")
        ),
        "{restored}"
    );
    // An unrelated PR verdict arrives while GitHub catches up. Keep the already
    // published owner verdict at this head and finish the candidate that passed.
    let mut peer = app
        .deps
        .store
        .ci_runs_for_sha(&f.sandbox, &moved)
        .await?
        .remove(0);
    peer.id = "unpublished-red-peer".into();
    peer.reference = CiRef::PullRequest(82);
    peer.state = RunState::Failed;
    app.deps.store.insert_ci_run(&peer, &[]).await?;
    f.h.github
        .retitle_errors
        .lock()
        .map_err(|_| "retitle errors")?
        .clear();
    *f.h.github.stale_reads.lock().map_err(|_| "lock")? = 0;
    f.decide_until(&app, || async { f.merged(&app, 1).await })
        .await?;
    let ended = f.wait(1, &worktree).await?;
    assert!(
        ended["merge_outcome"]
            .as_str()
            .is_some_and(|s| s.contains("in a merge train")),
        "{ended}"
    );
    Ok(())
}

/// SPEC-441 MQ-006: `delivery_merge` called again for a head that is armed and in a merge
/// candidate keeps its place: the candidate is not dissolved and its run goes on.
#[tokio::test]
async fn arming_the_same_head_again_keeps_its_place() -> TestResult {
    let f = fixture().await?;
    let app = f.h.app.clone();
    let mut worktrees = Vec::new();
    for (n, file) in [(1, "a.txt"), (2, "b.txt")] {
        worktrees.push(f.task(n, &[(file, "x\n")]).await?);
    }
    for (n, worktree) in (1..).zip(&worktrees) {
        f.arm(n, worktree).await?;
        f.record_passed(&app, n, &git(worktree, &["rev-parse", "HEAD"])?)
            .await?;
    }
    f.advance_dev("other.txt")?;
    app.merge_armed(&f.sandbox).await;
    let train = f.train_runs(&app).await?.first().cloned().ok_or("train")?;
    assert_eq!(train.reference.key(), "train/1+2");
    let before = saved_armed(&app, 2)?;
    f.h.clock.0.fetch_add(1000, Ordering::SeqCst);
    let head = git(&worktrees[1], &["rev-parse", "HEAD"])?;
    let again = support::mcp_tool(
        &f.h,
        "tok-t2",
        "delivery_merge",
        json!({"reviewed_head": head, "review_summary": "approve"}),
    )
    .await?;
    let again = support::tool_text(&again);
    assert_eq!(again["armed"], true, "{again}");
    assert!(
        again["next"]
            .as_str()
            .is_some_and(|n| n.contains("keeps its place")),
        "{again}"
    );
    assert_eq!(saved_armed(&app, 2)?, before);
    app.merge_armed(&f.sandbox).await;
    let run = app.deps.store.ci_run(&train.id).await?.ok_or("run")?;
    assert!(run.state.is_active(), "not dissolved: {run:?}");
    f.settle(&app).await?;
    f.decide_until(&app, || async {
        Ok(f.merged(&app, 1).await? && f.merged(&app, 2).await?)
    })
    .await?;
    Ok(())
}

/// Four green heads arriving during a running candidate share the first candidate after it.
#[tokio::test]
async fn four_heads_that_pass_during_a_candidate_join_the_next_one_in_arm_order() -> TestResult {
    let f = fixture_with(Project::Standard, None, MULTI_HEAD_BUDGET).await?;
    let app = &f.h.app;
    let mut worktrees = Vec::new();
    for n in 1..=5 {
        worktrees.push(f.task(n, &[(&format!("file-{n}"), "change\n")]).await?);
    }
    f.arm(1, &worktrees[0]).await?;
    f.record_passed(app, 1, &git(&worktrees[0], &["rev-parse", "HEAD"])?)
        .await?;
    let release = f.h._dir.path().join("release-candidate");
    f.advance_dev_with("yulius.toml", &format!(
        "[[ci.jobs]]\nname = \"check\"\ncommand = 'while [ ! -f \"{}\" ]; do sleep 0.05; done'\n",
        release.display()
    ))?;
    app.merge_armed(&f.sandbox).await;
    let first = f.train_runs(app).await?.pop().ok_or("first candidate")?;
    assert_eq!(first.reference, CiRef::Train(vec![1]));
    app.ci_dispatch().await;
    f.decide_until(app, || async {
        Ok(app
            .deps
            .store
            .ci_run(&first.id)
            .await?
            .ok_or("first run")?
            .state
            == RunState::Running)
    })
    .await?;
    for n in [4, 2, 5, 3] {
        let worktree = &worktrees[n as usize - 1];
        f.arm(n, worktree).await?;
        f.record_passed(app, n, &git(worktree, &["rev-parse", "HEAD"])?)
            .await?;
        app.merge_armed(&f.sandbox).await;
    }
    let active = f.train_runs(app).await?;
    assert_eq!(active.len(), 1, "{active:?}");
    assert_eq!(active[0].state, RunState::Running);
    assert!(f.merges().is_empty());
    std::fs::write(release, "")?;
    assert_eq!(run_candidate(&f, app, &first.id).await?, RunState::Passed);
    app.merge_armed(&f.sandbox).await;
    assert_eq!(f.merges(), ["merge_pull #1 squash"]);
    app.merge_armed(&f.sandbox).await;
    let runs = f.train_runs(app).await?;
    assert_eq!(runs.len(), 2, "{runs:?}");
    let next = runs
        .iter()
        .find(|r| r.id != first.id)
        .ok_or("next candidate")?;
    assert_eq!(next.reference, CiRef::Train(vec![4, 2, 5, 3]));
    assert_eq!(run_candidate(&f, app, &next.id).await?, RunState::Passed);
    f.decide_until(app, || async { f.merged(app, 3).await })
        .await?;
    let state = f.wait(2, &worktrees[1]).await?;
    let text = state["merge_outcome"]
        .as_str()
        .ok_or("multi-member verdict")?;
    assert!(text.contains("in a merge train with #4, #5, #3."), "{text}");
    Ok(())
}

/// SPEC-441 MQ-007: every armed pull request says where it stands, in `yulius wait` and in
/// `delivery_status`: one in the merge candidate under test, one green behind it waiting for the
/// next candidate, and one whose agent is busy.
#[tokio::test]
async fn every_armed_pull_request_says_where_it_stands() -> TestResult {
    let f = fixture().await?;
    let app = f.h.app.clone();
    let mut worktrees = Vec::new();
    for (n, file) in [(1, "a.txt"), (2, "b.txt"), (3, "c.txt")] {
        worktrees.push(f.task(n, &[(file, "x\n")]).await?);
    }
    f.arm(1, &worktrees[0]).await?;
    f.record_passed(&app, 1, &git(&worktrees[0], &["rev-parse", "HEAD"])?)
        .await?;
    f.advance_dev("other.txt")?;
    app.merge_armed(&f.sandbox).await;
    let train = f.train_runs(&app).await?.first().cloned().ok_or("train")?;
    assert_eq!(train.reference.key(), "train/1");
    for n in [2, 3] {
        f.arm(n, &worktrees[n as usize - 1]).await?;
        f.record_passed(
            &app,
            n,
            &git(&worktrees[n as usize - 1], &["rev-parse", "HEAD"])?,
        )
        .await?;
    }
    f.h.store
        .transition_task(
            &TaskId::parse("t3")?,
            None,
            TaskStatus::Running,
            Some(Activity::Working),
            app.now(),
        )
        .await?;
    app.merge_armed(&f.sandbox).await;
    let note = |n: u32| {
        let f = &f;
        let worktree = worktrees[n as usize - 1].clone();
        async move {
            Ok::<_, Box<dyn std::error::Error>>(
                f.wait(n, &worktree).await?["merge_train"]
                    .as_str()
                    .unwrap_or_default()
                    .to_owned(),
            )
        }
    };
    let first = note(1).await?;
    assert!(
        first.contains("#1 is number 1 of 1 in merge train #1"),
        "{first}"
    );
    let second = note(2).await?;
    assert!(
        second.starts_with("Pull request #2 passed its own run at")
            && second.contains(&format!(
                "it goes into the next merge candidate, after the one Yulius tests now (merge \
                 candidate #1 (run {}))",
                train.id
            )),
        "{second}"
    );
    let third = note(3).await?;
    assert!(
        third.contains("as soon as your turn ends with everything committed"),
        "{third}"
    );
    for note in [&first, &second, &third] {
        assert!(
            note.contains("Do not merge the integration branch yourself"),
            "{note}"
        );
    }
    // Nothing here can be decided before the candidate's run ends: the fast pass leaves the
    // sandbox alone (review M1: it fetched and asked GitHub every 20 seconds for an hour).
    let sandbox = app.deps.store.sandbox(&f.sandbox).await?.ok_or("s1")?;
    assert!(!app.decidable_in(&sandbox).await);
    let status = support::mcp_tool(&f.h, "tok-t2", "delivery_status", json!({})).await?;
    let status = support::tool_text(&status);
    assert!(
        status["notes"]
            .as_array()
            .is_some_and(|notes| notes.iter().any(|n| n.as_str() == Some(second.as_str()))),
        "{status}"
    );
    Ok(())
}

/// SPEC-441 MQ-002, MQ-008: an armed pull request that is green and up to date merges on the
/// fast pass (every 20 seconds, with no CI run ending), even while a merge candidate of another
/// is under test; that candidate's base moved, so it is formed again on the new base. A pass over
/// a sandbox with nothing decidable does nothing.
#[tokio::test]
async fn a_green_current_pull_request_merges_on_the_next_pass() -> TestResult {
    let f = fixture().await?;
    let app = f.h.app.clone();
    let first = f.task(1, &[("a.txt", "x\n")]).await?;
    f.arm(1, &first).await?;
    f.record_passed(&app, 1, &git(&first, &["rev-parse", "HEAD"])?)
        .await?;
    f.advance_dev("other.txt")?;
    app.merge_armed(&f.sandbox).await;
    let stale = f.train_runs(&app).await?.first().cloned().ok_or("train")?;
    // #2 starts from the moved base; its run passes while #1's candidate is under test.
    let second = f.task(2, &[("b.txt", "x\n")]).await?;
    f.arm(2, &second).await?;
    // Nothing decidable yet (#2's run has not passed): the fast pass does nothing.
    let writes = f.h.github.writes().len();
    app.merge_decidable_everywhere().await;
    assert_eq!(f.h.github.writes().len(), writes);
    f.record_passed(&app, 2, &git(&second, &["rev-parse", "HEAD"])?)
        .await?;
    // The protected merge returns a commit even if the read confirming its base fails.
    f.h.github.fail_merged_reads.store(true, Ordering::SeqCst);
    app.merge_decidable_everywhere().await;
    f.h.github.fail_merged_reads.store(false, Ordering::SeqCst);
    assert_eq!(f.merges(), ["merge_pull #2 squash"]);
    let sha = git(&f.origin, &["rev-parse", "dev"])?;
    let state = f.wait(2, &second).await?;
    let text = state["merge_outcome"].as_str().ok_or("merge outcome")?;
    assert!(text.contains(&format!("at {}", &sha[..9])), "{text}");
    assert!(!text.contains("into `dev`"), "unconfirmed base: {text}");
    // The stale candidate makes way for one on the new base, and #1 lands on it.
    f.decide_until(&app, || async {
        f.settle(&app).await?;
        f.merged(&app, 1).await
    })
    .await?;
    let ended = app.deps.store.ci_run(&stale.id).await?.ok_or("run")?;
    assert_eq!(ended.state, RunState::Cancelled, "{ended:?}");
    assert_eq!(f.merges(), ["merge_pull #2 squash", "merge_pull #1 squash"]);
    Ok(())
}

/// SPEC-441 MQ-009: a merge made by hand, pinned to a head whose own Yulius CI run passed
/// (`gh pr merge --match-head-commit`), ends the armed merge with an outcome that says it counts
/// as merged on green Yulius CI, and `delivery_merge` and `delivery_status` say so too. A hand
/// merge at a head that never passed is told that it does not count.
#[tokio::test]
async fn a_pinned_hand_merge_on_a_green_head_counts() -> TestResult {
    let f = fixture().await?;
    let app = f.h.app.clone();
    let person = yulius_app::model::Secret("person-token".into());
    let (worktree, _peer, _) = shared_pull_heads(&f).await?;
    f.arm(1, &worktree).await?;
    let head = git(&worktree, &["rev-parse", "HEAD"])?;
    f.record_passed(&app, 1, &head).await?;
    let landed = app
        .deps
        .github
        .merge_pull(&person, "acme", "app", 1, "squash", &head)
        .await?;
    // Another PR finishes red at this same SHA after the person has merged the green owner.
    f.record_run(&app, 2, &head, RunState::Failed).await?;
    app.merge_armed(&f.sandbox).await;
    let state = f.wait(1, &worktree).await?;
    assert_eq!(state["merge_armed"], false, "{state}");
    let outcome = state["merge_outcome"].as_str().unwrap_or_default();
    assert!(
        outcome.contains(&format!("into `dev` at {}", &landed[..9]))
            && outcome.contains(&format!("PR head {}", &head[..9]))
            && outcome.contains("it counts as merged on green Yulius CI, whoever merged it."),
        "{outcome}"
    );
    let again = support::mcp_tool(
        &f.h,
        "tok-t1",
        "delivery_merge",
        json!({"reviewed_head": head, "review_summary": "approve"}),
    )
    .await?;
    let again = support::tool_text(&again);
    assert!(
        again["next"]
            .as_str()
            .is_some_and(|n| n.contains("it counts as merged on green Yulius CI")),
        "{again}"
    );
    let status = support::mcp_tool(&f.h, "tok-t1", "delivery_status", json!({})).await?;
    let status = support::tool_text(&status);
    assert!(
        status["notes"]
            .as_array()
            .is_some_and(|notes| notes.iter().any(|n| n
                .as_str()
                .is_some_and(|n| n.contains("counts as merged on green")))),
        "{status}"
    );
    // A stale read of an open PR or a different closed PR cannot identify this landing.
    for (merged, misnumbered) in [(false, false), (true, true)] {
        f.h.github
            .misnumber_merged_reads
            .store(misnumbered, Ordering::SeqCst);
        f.h.github
            .pulls
            .lock()
            .map_err(|_| "pulls")?
            .iter_mut()
            .find(|pull| pull.number == 1)
            .ok_or("merged PR")?
            .merged = merged;
        let again = support::mcp_tool(
            &f.h,
            "tok-t1",
            "delivery_merge",
            json!({"reviewed_head": head, "review_summary": "approve"}),
        )
        .await?;
        let again = support::tool_text(&again);
        assert!(
            again["next"]
                .as_str()
                .is_some_and(|note| note.contains("could not read its merged head")
                    && !note.contains("into `dev`")
                    && !note.contains(&landed[..9])),
            "{again}"
        );
    }
    f.h.github
        .misnumber_merged_reads
        .store(false, Ordering::SeqCst);

    // Without the ruleset, a head that never passed is merged by hand: it does not count.
    f.h.github.strict.store(false, Ordering::SeqCst);
    let untested = f.task(3, &[("b.txt", "x\n")]).await?;
    f.arm(3, &untested).await?;
    let red = git(&untested, &["rev-parse", "HEAD"])?;
    app.deps
        .github
        .merge_pull(&person, "acme", "app", 3, "squash", &red)
        .await?;
    f.h.github
        .pulls
        .lock()
        .map_err(|_| "pulls")?
        .iter_mut()
        .find(|pull| pull.number == 3)
        .ok_or("merged PR")?
        .merge_commit_sha = None;
    app.merge_armed(&f.sandbox).await;
    let outcome = f.wait(3, &untested).await?["merge_outcome"]
        .as_str()
        .unwrap_or_default()
        .to_owned();
    assert!(
        outcome.contains("but Yulius CI did not pass at that head (no run): it does not count"),
        "{outcome}"
    );
    assert!(!outcome.contains("into `dev` at"), "{outcome}");
    assert!(
        outcome.contains(&format!("PR head {}", &red[..9])),
        "{outcome}"
    );
    Ok(())
}

/// Review M2 (SPEC-441 MQ-004/MQ-005): a candidate whose run was cancelled (or errored) says
/// nothing about its members, so they go one by one: each brought up to date after its own run
/// passed, that head's run decides, and it merges. No second candidate forms from them.
#[tokio::test]
async fn a_cancelled_candidate_sends_its_members_one_by_one() -> TestResult {
    let f = fixture().await?;
    let app = f.h.app.clone();
    let sandbox = app.deps.store.sandbox(&f.sandbox).await?.ok_or("s1")?;
    let mut worktrees = Vec::new();
    for (n, file) in [(1, "a.txt"), (2, "b.txt")] {
        worktrees.push(f.task(n, &[(file, "x\n")]).await?);
    }
    for (n, worktree) in (1..).zip(&worktrees) {
        f.arm(n, worktree).await?;
        f.record_passed(&app, n, &git(worktree, &["rev-parse", "HEAD"])?)
            .await?;
    }
    f.advance_dev("other.txt")?;
    app.merge_armed(&f.sandbox).await;
    let train = f.train_runs(&app).await?.first().cloned().ok_or("train")?;
    let cancelled = call(
        &f.h.router,
        Method::POST,
        &format!("/api/sandboxes/s1/ci/runs/{}/cancel", train.id),
        None,
        &f.alice,
    )
    .await?;
    assert_eq!(cancelled.status, StatusCode::OK, "{}", cancelled.body);
    f.decide_until(&app, || async {
        app.ci_sync_sandbox(&sandbox).await?;
        f.settle(&app).await?;
        Ok(f.merged(&app, 1).await? && f.merged(&app, 2).await?)
    })
    .await?;
    assert_eq!(f.train_runs(&app).await?.len(), 1, "no second candidate");
    let executed = f.executed(&app).await?;
    let own = executed
        .iter()
        .filter(|r| matches!(r.reference, CiRef::PullRequest(_)))
        .count();
    assert!(
        own >= 1,
        "brought up to date, its own run decided: {executed:?}"
    );
    Ok(())
}

/// Review M2 (SPEC-441 MQ-003): a member whose agent is busy when a passed candidate reaches it
/// is waited for, not dropped from the candidate; it merges once its agent is idle. A member
/// busy beyond the bound (`WAIT_LIMIT_MS`) ends the candidate and stays armed.
#[tokio::test]
async fn a_busy_member_of_a_passed_candidate_is_waited_for() -> TestResult {
    let f = fixture().await?;
    let app = f.h.app.clone();
    let busy = |n: u32, activity: Activity| {
        let f = &f;
        async move {
            f.h.store
                .transition_task(
                    &TaskId::parse(&format!("t{n}"))?,
                    None,
                    TaskStatus::Running,
                    Some(activity),
                    f.h.app.now(),
                )
                .await?;
            Ok::<_, Box<dyn std::error::Error>>(())
        }
    };
    for n in [1u32, 2] {
        let worktree = f.task(n, &[(&format!("f{n}.txt"), "x\n")]).await?;
        f.arm(n, &worktree).await?;
        f.record_passed(&app, n, &git(&worktree, &["rev-parse", "HEAD"])?)
            .await?;
        f.advance_dev(&format!("other-{n}.txt"))?;
        app.merge_armed(&f.sandbox).await;
        assert_eq!(f.train_runs(&app).await?.len(), n as usize);
        busy(n, Activity::Working).await?;
        f.settle(&app).await?;
        app.merge_armed(&f.sandbox).await;
        assert!(!f.merged(&app, n).await?);
        assert!(
            f.trains_file(&app).contains("\"waiting_since\""),
            "waits: {}",
            f.trains_file(&app)
        );
        if n == 1 {
            busy(1, Activity::Waiting).await?;
            f.decide_until(&app, || async { f.merged(&app, 1).await })
                .await?;
        }
    }
    // #2's agent stays busy beyond the bound: the candidate ends, the merge stays armed.
    f.h.clock.0.fetch_add(
        yulius_app::merge_train::WAIT_LIMIT_MS + 1000,
        Ordering::SeqCst,
    );
    app.merge_armed(&f.sandbox).await;
    assert!(!f.trains_file(&app).contains("\"train\":{"), "ended");
    assert!(!f.merged(&app, 2).await?);
    let state = f
        .wait(2, &app.config.data_dir.join("sandboxes/s1/worktrees/t2"))
        .await?;
    assert_eq!(state["merge_armed"], true, "{state}");
    Ok(())
}

/// Review minor 1 (SPEC-441 MQ-005): a member that cannot merge at its turn for a reason of its
/// own (here its owner can no longer act) goes one by one, where it is told why, instead of
/// the same candidate being formed and run again.
#[tokio::test]
async fn a_member_that_cannot_merge_is_told_instead_of_tested_again() -> TestResult {
    let f = fixture().await?;
    let app = f.h.app.clone();
    let worktree = f.task(1, &[("a.txt", "x\n")]).await?;
    f.arm(1, &worktree).await?;
    let (_, armed_at) = saved_armed(&app, 1)?.ok_or("armed")?;
    f.record_passed(&app, 1, &git(&worktree, &["rev-parse", "HEAD"])?)
        .await?;
    f.advance_dev("other.txt")?;
    app.merge_armed(&f.sandbox).await;
    let trains = f.train_runs(&app).await?;
    let [train] = trains.as_slice() else {
        return Err(format!("expected one candidate: {trains:?}").into());
    };
    f.h.store
        .update_user_flags(&UserId::parse("u-alice")?, Some(Some(app.now())), None)
        .await?;
    // Record the passed run without its background merge pass. Inspect the train's decision
    // after exactly one pass, before a later pass prunes the now-disarmed member.
    let ended = yulius_app::ci::CiRunUpdate {
        started_at: Some(app.now()),
        ended_at: Some(app.now()),
        ..Default::default()
    };
    assert!(
        app.deps
            .store
            .transition_ci_run(&train.id, &[RunState::Queued], RunState::Passed, &ended)
            .await?
    );
    app.merge_armed(&f.sandbox).await;
    let reasons = dissolved(&app).await?;
    assert!(
        reasons
            .iter()
            .any(|r| r.contains("the owner of #1 can no longer act")),
        "{reasons:?}"
    );
    let trains: Value = serde_json::from_str(&f.trains_file(&app))?;
    assert_eq!(trains["s1"]["held_back"], json!([["t1", armed_at]]));
    assert!(!f.merged(&app, 1).await?);
    let state = f.wait(1, &worktree).await?;
    assert_eq!(state["merge_armed"], false, "{state}");
    let outcome = state["merge_outcome"].as_str().unwrap_or_default();
    assert!(outcome.contains("owner can no longer act"), "{outcome}");
    app.merge_armed(&f.sandbox).await;
    assert_eq!(f.train_runs(&app).await?.len(), 1, "not tested again");
    Ok(())
}

/// Review minor 3 (SPEC-441 MQ-007): an armed head whose own run errored says so and how to go
/// on, instead of "waits for its own run", which would never come.
#[tokio::test]
async fn an_errored_run_says_to_rerun_it() -> TestResult {
    let f = fixture().await?;
    let app = f.h.app.clone();
    let worktree = f.task(1, &[("a.txt", "x\n")]).await?;
    f.arm(1, &worktree).await?;
    f.record_run(
        &app,
        1,
        &git(&worktree, &["rev-parse", "HEAD"])?,
        RunState::Error,
    )
    .await?;
    app.merge_armed(&f.sandbox).await;
    let state = f.wait(1, &worktree).await?;
    assert_eq!(state["merge_armed"], true, "{state}");
    let note = state["merge_train"].as_str().unwrap_or_default();
    assert!(
        note.contains("ended in error") && note.contains("`ci_rerun`"),
        "{note}"
    );
    Ok(())
}

/// Review round 2, M3 (SPEC-441 MQ-005): a member whose branch cannot be brought up to date at
/// its turn (here a stale `index.lock` in its worktree) ends the candidate and goes one by one,
/// still armed and told where it stands; the same candidate is not formed and run again.
#[tokio::test]
async fn a_member_that_cannot_be_updated_goes_one_by_one() -> TestResult {
    let f = fixture().await?;
    let app = f.h.app.clone();
    let worktree = f.task(1, &[("a.txt", "x\n")]).await?;
    f.arm(1, &worktree).await?;
    f.record_passed(&app, 1, &git(&worktree, &["rev-parse", "HEAD"])?)
        .await?;
    f.advance_dev("other.txt")?;
    app.merge_armed(&f.sandbox).await;
    assert_eq!(f.train_runs(&app).await?.len(), 1);
    let lock = PathBuf::from(git(
        &worktree,
        &[
            "rev-parse",
            "--path-format=absolute",
            "--git-path",
            "index.lock",
        ],
    )?);
    std::fs::write(&lock, "")?;
    f.settle(&app).await?;
    for _ in 0..3 {
        app.merge_armed(&f.sandbox).await;
        f.settle(&app).await?;
    }
    assert_eq!(f.train_runs(&app).await?.len(), 1, "not tested again");
    assert!(!f.trains_file(&app).contains("\"train\":{"), "ended");
    let state = f.wait(1, &worktree).await?;
    assert_eq!(state["merge_armed"], true, "{state}");
    assert!(
        state["merge_train"]
            .as_str()
            .is_some_and(|n| n.contains("is behind `dev`")
                && n.contains("Do not merge the integration branch yourself")),
        "{state}"
    );
    std::fs::remove_file(&lock)?;
    Ok(())
}

/// Review round 2, minor 1 (SPEC-441 MQ-008): an idle agent with uncommitted changes cannot be
/// put in a candidate, so its green head behind the base gives the fast pass nothing to do until
/// it commits.
#[tokio::test]
async fn uncommitted_changes_leave_nothing_to_decide() -> TestResult {
    let f = fixture().await?;
    let app = f.h.app.clone();
    let sandbox = app.deps.store.sandbox(&f.sandbox).await?.ok_or("s1")?;
    let worktree = f.task(1, &[("a.txt", "x\n")]).await?;
    f.arm(1, &worktree).await?;
    f.record_passed(&app, 1, &git(&worktree, &["rev-parse", "HEAD"])?)
        .await?;
    f.advance_dev("other.txt")?;
    app.ci_sync_sandbox(&sandbox).await?;
    std::fs::write(worktree.join("draft.txt"), "wip\n")?;
    assert!(!app.decidable_in(&sandbox).await, "uncommitted");
    std::fs::remove_file(worktree.join("draft.txt"))?;
    assert!(app.decidable_in(&sandbox).await, "ready for a candidate");
    Ok(())
}

/// SPEC-487 CHN-001 (13 November 2026: #469 was started "after #465" at 19:57:02, while #465's
/// green pull request #1257 waited, armed, in the merge queue): a task chained after one whose
/// pull request is green and armed stays waiting, since Yulius will merge it and tell its agent,
/// and the task list reads the same; it starts once that pull request is merged.
#[tokio::test]
async fn a_task_chained_after_an_armed_green_pull_request_waits_for_its_merge() -> TestResult {
    let f = fixture().await?;
    let app = f.h.app.clone();
    let worktree = f.task(1, &[("a.txt", "x\n")]).await?;
    let first = TaskId::parse("t1")?;
    let next = chained_after_t1(&f).await?;
    let status = || status_of_task(&f, &next);

    f.arm(1, &worktree).await?;
    let head = git(&worktree, &["rev-parse", "HEAD"])?;
    f.record_passed(&app, 1, &head).await?;
    // Its turn ended, its plan is complete and its pull request is tested: what read Done.
    app.advance_chains(&first).await;
    assert_eq!(
        status().await,
        Some(TaskStatus::Chained),
        "the merge is still to come"
    );
    let shown = call(&f.h.router, Method::GET, "/api/tasks/t1", None, &f.alice).await?;
    assert_eq!(
        shown.body["yulius_wait"], true,
        "it goes on by itself: {}",
        shown.body
    );
    // SPEC-487 TCT-004 (review of slice 3, R2-1): its owner cannot close it out meanwhile.
    // A close would end the task with its pull request tested and its plan complete, which
    // reads Done for a closed task, and the chained task would start before the merge.
    {
        let record = TaskStore::task(&f.h.store, &first).await?.ok_or("t1")?;
        let owner = yulius_app::ports::store::UserStore::user(&f.h.store, &record.owner)
            .await?
            .ok_or("owner")?;
        let ctx = yulius_app::auth::AuthContext {
            user: owner,
            csrf_token: None,
            session_hash: None,
            token: None,
        };
        let refused = app.close_task_for_person(&ctx, "t1").await.err();
        assert!(
            matches!(&refused, Some(yulius_app::error::AppError::Conflict(why))
                if why.contains("goes on by itself") && why.contains("chained after it wait")),
            "{refused:?}"
        );
        assert_eq!(
            TaskStore::task(&f.h.store, &first).await?.map(|t| t.status),
            Some(TaskStatus::Running)
        );
        app.advance_chains(&first).await;
        assert_eq!(status().await, Some(TaskStatus::Chained));
    }

    f.decide_until(&app, || async { f.merged(&app, 1).await })
        .await?;
    app.advance_chains(&first).await;
    assert_ne!(
        status().await,
        Some(TaskStatus::Chained),
        "merged: it starts"
    );
    Ok(())
}

/// SPEC-491 DON-001, DON-003 (14 November 2026: #474 was started "after #465" at 18:20:13, while
/// #465's pull request #1287 had passed at its head and, since `dev` moved at 18:16:56,
/// conflicted with it): a pull request whose head conflicts with the integration branch, as
/// Yulius's own clone has it, is not tested, whatever its run said. Its task is not Done, the
/// task chained after it waits, and the Git overview and the counts say so; once its agent
/// merged the integration branch in and the new head passed, the chained task starts.
#[tokio::test]
async fn a_task_chained_after_a_green_pull_request_that_conflicts_waits() -> TestResult {
    let f = fixture().await?;
    let app = f.h.app.clone();
    let worktree = f.task(1, &[("shared.txt", "mine\n")]).await?;
    let first = TaskId::parse("t1")?;
    let next = chained_after_t1(&f).await?;
    // A task that waits on a start rule naming it asks the same question.
    let number = TaskStore::task(&f.h.store, &first)
        .await?
        .ok_or("t1")?
        .number;
    let ruled = call(
        &f.h.router,
        Method::POST,
        "/api/sandboxes/s1/tasks",
        Some(json!({"prompt": "after change 1", "start_rule": format!("#{number} done")})),
        &f.alice,
    )
    .await?;
    assert_eq!(ruled.body["status"], "chained", "{}", ruled.body);
    let ruled = TaskId::parse(ruled.body["id"].as_str().ok_or("id")?)?;
    let head = git(&worktree, &["rev-parse", "HEAD"])?;
    f.record_passed(&app, 1, &head).await?;
    // `dev` moves in the same file, and Yulius's clone learns of it.
    f.advance_dev("shared.txt")?;
    git(&f.repo, &["fetch", "-q", "origin"])?;

    app.advance_chains(&first).await;
    app.advance_all_chains().await;
    assert_eq!(
        status_of_task(&f, &next).await,
        Some(TaskStatus::Chained),
        "its pull request cannot merge"
    );
    assert_eq!(
        status_of_task(&f, &ruled).await,
        Some(TaskStatus::Chained),
        "`#1 done` is false"
    );
    let overview = call(
        &f.h.router,
        Method::GET,
        "/api/sandboxes/s1/git",
        None,
        &f.alice,
    )
    .await?;
    let pr = overview.body["tasks"]
        .as_array()
        .and_then(|tasks| tasks.iter().find(|task| task["task_id"] == "t1"))
        .map(|task| &task["pr"])
        .ok_or("t1 in the overview")?;
    assert_eq!(
        (&pr["checks"], &pr["conflicts"]),
        (&json!("success"), &json!(true)),
        "{}",
        overview.body
    );
    let sandbox = call(
        &f.h.router,
        Method::GET,
        "/api/sandboxes/s1",
        None,
        &f.alice,
    )
    .await?;
    assert_eq!(
        sandbox.body["counts"]["running"], 1,
        "in review, not Done: {}",
        sandbox.body
    );

    // Its agent merges `dev` in, Yulius pushes at the turn's end, and the new head passes.
    assert!(git_ok(&worktree, &["merge", "-q", "origin/dev"]).is_none());
    std::fs::write(worktree.join("shared.txt"), "mine\nother\n")?;
    git(&worktree, &["add", "-A"])?;
    git(&worktree, &["commit", "-q", "--no-edit"])?;
    let task = TaskStore::task(&f.h.store, &first).await?.ok_or("t1")?;
    app.sync_steps_from_spec(&task).await?;
    app.auto_open_pr(&first).await;
    let merged_head = git(&worktree, &["rev-parse", "HEAD"])?;
    assert_eq!(
        git(&f.origin, &["rev-parse", "feat/1-train"])?,
        merged_head,
        "pushed: {}",
        f.wait(1, &worktree).await?
    );
    f.record_passed(&app, 1, &merged_head).await?;
    app.advance_chains(&first).await;
    app.advance_all_chains().await;
    for waiting in [&next, &ruled] {
        assert_ne!(
            status_of_task(&f, waiting).await,
            Some(TaskStatus::Chained),
            "it merges cleanly and is tested: it starts"
        );
    }
    Ok(())
}

/// SPEC-491 DON-001 (review of SPEC-491, M1): GitHub knows nothing of the union rule Yulius gives
/// the changes log, so two pull requests that each add an entry at its top read "not mergeable"
/// there while Yulius's own merge resolves them. Where the clone answers, it decides: the pull
/// request is tested, the overview says no conflict, and the task chained after it starts.
#[tokio::test]
async fn github_s_flag_does_not_hold_a_pull_request_the_clone_merges() -> TestResult {
    let f = fixture().await?;
    let app = f.h.app.clone();
    let worktree = f.task(1, &[("a.txt", "x\n")]).await?;
    let first = TaskId::parse("t1")?;
    let next = chained_after_t1(&f).await?;
    let head = git(&worktree, &["rev-parse", "HEAD"])?;
    f.record_passed(&app, 1, &head).await?;
    // `dev` gains another entry at the top of the changes log: a union merge, not a conflict.
    let other = tempfile::tempdir()?;
    let clone = other.path().join("c");
    git(
        other.path(),
        &[
            "clone",
            "-q",
            "--branch",
            f.integration,
            f.origin.to_str().ok_or("path")?,
            clone.to_str().ok_or("path")?,
        ],
    )?;
    std::fs::write(
        clone.join("CHANGES.md"),
        "# Changes\n\n- another\n- first\n",
    )?;
    git(&clone, &["commit", "-q", "-am", "another entry"])?;
    git(
        &clone,
        &["push", "-q", "origin", &format!("HEAD:{}", f.integration)],
    )?;
    git(&f.repo, &["fetch", "-q", "origin"])?;
    // GitHub, which merges without that rule, calls it not mergeable.
    if let Ok(mut pulls) = f.h.github.pulls.lock()
        && let Some(pull) = pulls.iter_mut().find(|p| p.number == 1)
    {
        pull.mergeable = Some(false);
    }
    let overview = call(
        &f.h.router,
        Method::GET,
        "/api/sandboxes/s1/git",
        None,
        &f.alice,
    )
    .await?;
    let pr = overview.body["tasks"]
        .as_array()
        .and_then(|tasks| tasks.iter().find(|task| task["task_id"] == "t1"))
        .map(|task| &task["pr"])
        .ok_or("t1 in the overview")?;
    assert_eq!(pr["conflicts"], false, "{}", overview.body);
    app.advance_chains(&first).await;
    assert_ne!(
        status_of_task(&f, &next).await,
        Some(TaskStatus::Chained),
        "the clone merges it: tested, and the chained task starts"
    );
    Ok(())
}

/// SPEC-483 GRP-003 (finding D8): an agent that armed its merge and whose goal was then judged
/// achieved is not Done while the merge is armed: a task chained after it waits, a supertask
/// waits for it, Home counts it as working, and archiving it is refused (it would have ended the
/// armed merge, said to nobody); the merge stays armed.
#[tokio::test]
async fn an_achieved_goal_with_an_armed_merge_is_not_done() -> TestResult {
    let f = fixture().await?;
    let app = f.h.app.clone();
    let worktree = f.task(1, &[("a.txt", "x\n")]).await?;
    let first = TaskId::parse("t1")?;
    let next = chained_after_t1(&f).await?;
    f.arm(1, &worktree).await?;
    let head = git(&worktree, &["rev-parse", "HEAD"])?;
    f.record_passed(&app, 1, &head).await?;
    assert!(
        f.h.store
            .transition_task(
                &first,
                Some(TaskStatus::Running),
                TaskStatus::Running,
                Some(Activity::Done),
                Timestamp(10),
            )
            .await?
    );
    app.advance_chains(&first).await;
    assert_eq!(status_of_task(&f, &next).await, Some(TaskStatus::Chained));
    // A supertask waits for it too.
    let supertask = call(
        &f.h.router,
        Method::POST,
        "/api/sandboxes/s1/tasks",
        Some(json!({"prompt": "harden", "supertask": true})),
        &f.alice,
    )
    .await?;
    assert_eq!(supertask.body["status"], "queued", "{}", supertask.body);
    let held = call(
        &f.h.router,
        Method::GET,
        &format!("/api/tasks/{}", supertask.body["id"].as_str().ok_or("id")?),
        None,
        &f.alice,
    )
    .await?;
    assert_eq!(held.body["held_by"], json!([1]), "{}", held.body);
    let shown = call(&f.h.router, Method::GET, "/api/tasks/t1", None, &f.alice).await?;
    assert_eq!(shown.body["merge_armed"], true, "{}", shown.body);
    let sandbox = call(
        &f.h.router,
        Method::GET,
        "/api/sandboxes/s1",
        None,
        &f.alice,
    )
    .await?;
    assert_eq!(
        (
            sandbox.body["counts"]["running"].as_u64(),
            sandbox.body["counts"]["queued"].as_u64()
        ),
        (Some(1), Some(2)),
        "t1 working; the chained task and the supertask in line: {}",
        sandbox.body
    );
    let archived = call(
        &f.h.router,
        Method::POST,
        "/api/tasks/t1/archive",
        None,
        &f.alice,
    )
    .await?;
    assert_eq!(archived.status, StatusCode::CONFLICT, "{}", archived.body);
    assert_eq!(status_of_task(&f, &first).await, Some(TaskStatus::Running));
    let shown = call(&f.h.router, Method::GET, "/api/tasks/t1", None, &f.alice).await?;
    assert_eq!(
        shown.body["merge_armed"], true,
        "still armed: {}",
        shown.body
    );
    Ok(())
}

/// The owner of task `t<n>` (alice), as the functions of the pages take a person.
async fn owner_ctx(f: &Fixture, n: u32) -> BoxResult<yulius_app::auth::AuthContext> {
    let record = TaskStore::task(&f.h.store, &TaskId::parse(&format!("t{n}"))?)
        .await?
        .ok_or("task")?;
    let user = UserStore::user(&f.h.store, &record.owner)
        .await?
        .ok_or("owner")?;
    Ok(yulius_app::auth::AuthContext {
        user,
        csrf_token: None,
        session_hash: None,
        token: None,
    })
}

/// SPEC-487 MRG-001 to MRG-003 (wall 4: "arm/re-arm merge, why a green PR is not merging"): a
/// person merges their task's pull request through their agent as the page's Merge does; while
/// its checks run, the merge is armed for them at the branch's head and merges once they pass.
/// Meanwhile the task's delivery and the project's merge queue say where it stands; afterwards,
/// how it ended. A head whose checks failed is refused with that reason.
#[tokio::test]
async fn a_person_merges_their_tasks_pull_request_when_its_checks_pass() -> TestResult {
    let f = fixture().await?;
    let app = f.h.app.clone();
    let worktree = f.task(1, &[("a.txt", "x\n")]).await?;
    let head = git(&worktree, &["rev-parse", "HEAD"])?;
    let ctx = owner_ctx(&f, 1).await?;

    // Nothing armed yet.
    let queue = app.merge_queue_for_person(&ctx, &f.sandbox).await?;
    assert_eq!(queue["armed"], json!([]), "{queue}");
    let state = app.merge_state_for_person(&ctx, "t1").await?;
    assert_eq!(state["armed"], false, "{state}");

    // Its checks have not run: armed for the person, at the branch's head.
    let armed = app.merge_for_person(&ctx, "t1").await?;
    assert_eq!(
        (&armed["merged"], &armed["armed"]),
        (&json!(false), &json!(true)),
        "{armed}"
    );
    assert_eq!(armed["head"], head, "{armed}");
    let state = app.merge_state_for_person(&ctx, "t1").await?;
    assert_eq!(
        (&state["armed"], &state["head"]),
        (&json!(true), &json!(head)),
        "{state}"
    );
    let queue = app.merge_queue_for_person(&ctx, &f.sandbox).await?;
    let listed = queue["armed"].as_array().ok_or("armed")?;
    assert_eq!(listed.len(), 1, "{queue}");
    assert_eq!(
        (
            &listed[0]["number"],
            &listed[0]["head"],
            &listed[0]["owner"]
        ),
        (&json!(1), &json!(head), &json!("alice")),
        "{queue}"
    );
    // Asked again, it keeps its place.
    let again = app.merge_for_person(&ctx, "t1").await?;
    assert!(
        again["note"]
            .as_str()
            .is_some_and(|note| note.contains("keeps its place")),
        "{again}"
    );
    // Audited under the person's name.
    assert_eq!(
        yulius_app::ports::store::AuditStore::audit_details(
            &f.h.store,
            "t1",
            "delivery.person.merge_armed",
            10
        )
        .await?
        .len(),
        1
    );

    // Its checks pass: Yulius merges it, and its delivery says so.
    f.record_passed(&app, 1, &head).await?;
    f.decide_until(&app, || async { f.merged(&app, 1).await })
        .await?;
    let state = app.merge_state_for_person(&ctx, "t1").await?;
    assert_eq!(state["armed"], false, "{state}");
    assert!(
        state["last"]["text"]
            .as_str()
            .is_some_and(|text| text.contains("merged")),
        "{state}"
    );
    let queue = app.merge_queue_for_person(&ctx, &f.sandbox).await?;
    assert_eq!(queue["armed"], json!([]), "{queue}");
    // The merge is the person's, not a reviewed agent's: under their name, and said so
    // (review of slice 5, F2).
    let merged: Vec<(String, String, String)> =
        yulius_app::ports::store_ext::AutomationStore::automation_audit(&f.h.store, "app#1", 100)
            .await?
            .into_iter()
            .map(|entry| (entry.action, entry.actor, entry.detail.unwrap_or_default()))
            .collect();
    assert!(
        merged
            .iter()
            .any(|(action, actor, detail)| action == "delivery.person.merge"
                && actor == "alice"
                && detail.contains("armed by its owner")
                && !detail.contains("reviewed independently")),
        "{merged:?}"
    );
    assert!(
        !merged
            .iter()
            .any(|(action, _, _)| action == "delivery.agent.merge"),
        "{merged:?}"
    );
    assert_eq!(state["last"]["armed_by"], "alice", "{state}");

    // A head whose checks failed is not armed: the reason is said.
    let second = f.task(2, &[("b.txt", "y\n")]).await?;
    let failed = git(&second, &["rev-parse", "HEAD"])?;
    f.record_run(&app, 2, &failed, RunState::Failed).await?;
    let refused = app
        .merge_for_person(&owner_ctx(&f, 2).await?, "t2")
        .await
        .err();
    assert!(
        matches!(&refused, Some(yulius_app::error::AppError::Conflict(why)) if why.contains("failed")),
        "{refused:?}"
    );
    assert_eq!(
        app.merge_state_for_person(&ctx, "t2").await?["armed"],
        false
    );

    // A task whose run ended keeps no armed merge: it is not armed for nothing (F1).
    let third = f.task(3, &[("c.txt", "z\n")]).await?;
    let third_head = git(&third, &["rev-parse", "HEAD"])?;
    TaskStore::transition_task(
        &f.h.store,
        &TaskId::parse("t3")?,
        None,
        TaskStatus::Closed,
        None,
        app.now(),
    )
    .await?;
    let refused = app
        .merge_for_person(&owner_ctx(&f, 3).await?, "t3")
        .await
        .err();
    assert!(
        matches!(&refused, Some(yulius_app::error::AppError::Conflict(why))
            if why.contains("has ended") && why.contains("its checks pass")),
        "{refused:?}"
    );
    assert_eq!(
        app.merge_state_for_person(&ctx, "t3").await?["armed"],
        false
    );
    // Once its checks passed it merges at once, as the page's Merge does, and nothing is left
    // armed (F6).
    f.record_passed(&app, 3, &third_head).await?;
    let merged = app.merge_for_person(&owner_ctx(&f, 3).await?, "t3").await?;
    assert_eq!(
        (&merged["merged"], &merged["armed"]),
        (&json!(true), &json!(false)),
        "{merged}"
    );
    assert!(f.merged(&app, 3).await?);
    assert_eq!(
        app.merge_state_for_person(&ctx, "t3").await?["armed"],
        false
    );

    // A commit not pushed yet is not armed: what would merge is the branch on GitHub (F5).
    let fourth = f.task(4, &[("d.txt", "w\n")]).await?;
    std::fs::write(fourth.join("later.txt"), "later\n")?;
    git(&fourth, &["add", "-A"])?;
    git(&fourth, &["commit", "-q", "-m", "not pushed"])?;
    let refused = app
        .merge_for_person(&owner_ctx(&f, 4).await?, "t4")
        .await
        .err();
    assert!(
        matches!(&refused, Some(yulius_app::error::AppError::Conflict(why)) if why.contains("not pushed")),
        "{refused:?}"
    );
    Ok(())
}

/// A task chained after `t1`, created by Alice (who has a Claude account connected, SPEC-246).
async fn chained_after_t1(f: &Fixture) -> BoxResult<TaskId> {
    use yulius_app::model::{AccountRecord, AccountStatus};
    use yulius_app::ports::store::AccountStore;
    f.h.store
        .insert_account(&AccountRecord {
            id: yulius_domain::ids::AccountId::parse("acc-1")?,
            provider: yulius_domain::harness::Provider::Anthropic,
            label: "acc-1".into(),
            credential: Some(f.h.app.deps.crypto.seal(b"oauth-token")?),
            status: AccountStatus::Ok,
            usage_json: None,
            last_error: None,
            created_at: Timestamp(1),
            updated_at: Timestamp(1),
            email: None,
            email_checked_at: None,
            email_typed: false,
        })
        .await?;
    let created = call(
        &f.h.router,
        Method::POST,
        "/api/sandboxes/s1/tasks",
        Some(json!({"prompt": "build on change 1", "after_task_id": "t1"})),
        &f.alice,
    )
    .await?;
    assert_eq!(created.body["status"], "chained", "{}", created.body);
    Ok(TaskId::parse(created.body["id"].as_str().ok_or("id")?)?)
}

async fn status_of_task(f: &Fixture, id: &TaskId) -> Option<TaskStatus> {
    TaskStore::task(&f.h.store, id)
        .await
        .ok()
        .flatten()
        .map(|task| task.status)
}

/// SPEC-487 CHN-001 (review round 1, M1): a server that started again knows a merge is armed
/// before it put the armed merges back, so a chain pass in between (any task's turn end) does not
/// find the green pull request Done.
#[tokio::test]
async fn a_restarted_server_keeps_a_chained_task_waiting_for_an_armed_merge() -> TestResult {
    let f = fixture().await?;
    let app = f.h.app.clone();
    let worktree = f.task(1, &[("a.txt", "x\n")]).await?;
    let next = chained_after_t1(&f).await?;
    f.arm(1, &worktree).await?;
    f.record_passed(&app, 1, &git(&worktree, &["rev-parse", "HEAD"])?)
        .await?;
    // The server after a restart, before `restore_armed_merges` ran.
    let restarted = App::new(app.config.clone(), app.deps.clone());
    restarted.advance_chains(&TaskId::parse("t1")?).await;
    assert_eq!(status_of_task(&f, &next).await, Some(TaskStatus::Chained));
    // Once they are back, the slot answers as before.
    restarted.restore_armed_merges().await;
    restarted.advance_chains(&TaskId::parse("t1")?).await;
    assert_eq!(status_of_task(&f, &next).await, Some(TaskStatus::Chained));
    Ok(())
}

/// SPEC-487 CHN-002 (review round 1, M3): an armed merge that ended without merging leaves a
/// green, open pull request and an idle agent that has not heard how it ended. That is not
/// Done either: the task goes on by itself once the watch says so.
#[tokio::test]
async fn a_task_chained_after_a_dropped_armed_merge_waits_for_its_news() -> TestResult {
    let f = fixture().await?;
    let app = f.h.app.clone();
    let worktree = f.task(1, &[("a.txt", "x\n")]).await?;
    let next = chained_after_t1(&f).await?;
    f.arm(1, &worktree).await?;
    f.record_passed(&app, 1, &git(&worktree, &["rev-parse", "HEAD"])?)
        .await?;
    f.advance_dev("other.txt")?;
    app.merge_armed(&f.sandbox).await;
    // Its owner can no longer act: the train's end drops the merge and keeps why.
    f.h.store
        .update_user_flags(&UserId::parse("u-alice")?, Some(Some(app.now())), None)
        .await?;
    f.settle(&app).await?;
    f.decide_until(&app, || async {
        Ok(f.wait(1, &worktree).await?["merge_armed"] == false)
    })
    .await?;
    let first = TaskId::parse("t1")?;
    app.advance_chains(&first).await;
    assert_eq!(
        status_of_task(&f, &next).await,
        Some(TaskStatus::Chained),
        "its agent has not heard how the merge ended"
    );
    let task = TaskStore::task(&f.h.store, &first).await?.ok_or("task")?;
    assert!(app.task_summary(&task).await?.yulius_wait);
    Ok(())
}

/// SPEC-612 GF-006: once GitHub confirms a person's merge, the completed predecessor
/// releases its successor without waiting for a terminal notification.
#[tokio::test]
async fn a_merge_github_reports_first_releases_the_finished_tasks_successor() -> TestResult {
    let f = fixture().await?;
    let app = f.h.app.clone();
    let worktree = f.task(1, &[("a.txt", "x\n")]).await?;
    let next = chained_after_t1(&f).await?;
    let first = TaskId::parse("t1")?;
    let head = git(&worktree, &["rev-parse", "HEAD"])?;
    f.record_passed(&app, 1, &head).await?;
    // The watch sees the pull request open (reading the task makes this server know its run).
    let task = TaskStore::task(&f.h.store, &first).await?.ok_or("task")?;
    app.task_summary(&task).await?;
    app.watch_prs(&yulius_app::pr_watch::WatchBounds::default())
        .await;
    // A person merges it on GitHub; the record still says open.
    let person = yulius_app::model::Secret("person-token".into());
    app.deps
        .github
        .merge_pull(&person, "acme", "app", 1, "squash", &head)
        .await?;
    use yulius_app::ports::store_ext::DeliveryStore;
    assert_eq!(
        f.h.store.delivery(&first).await?.map(|record| record.stage),
        Some(DeliveryStage::PrOpen)
    );
    app.advance_chains(&first).await;
    assert_eq!(
        status_of_task(&f, &next).await,
        Some(TaskStatus::Queued),
        "a finished predecessor has no merge notification to wait for"
    );
    Ok(())
}

/// SPEC-575 MFS-002 (review round 1): with a merge-first environment a merge train skips the
/// full suite like its members' heads, and the integration branch's run after the train merged
/// runs it, never reusing the train's tree that skipped it.
#[tokio::test]
async fn a_train_skips_the_full_suite_and_the_merged_head_runs_it() -> TestResult {
    let f = fixture().await?;
    let app = f.h.app.clone();
    let sandbox = app.deps.store.sandbox(&f.sandbox).await?.ok_or("s1")?;
    f.advance_dev_with(
        "yulius.toml",
        &format!(
            "{YULIUS_TOML}\n[[ci.jobs]]\nname = \"full\"\ncommand = 'echo full suite ran'\n\
             after_merge = true\n\n[cd.dev]\n[cd.prod]\n\n[cd.staging]\nstage = \"dev\"\n\
             merge_first = true\ndeploy = \"d\"\nverify = \"v\"\nrollback = \"r\"\n\
             migrations = \"none: stateless\"\n"
        ),
    )?;
    app.ci_sync_sandbox(&sandbox).await?;
    f.settle(&app).await?;
    let mut worktrees = Vec::new();
    for (n, file) in [(1u32, "a.txt"), (2, "b.txt")] {
        worktrees.push(f.task(n, &[(file, "x\n")]).await?);
    }
    for (n, worktree) in (1u32..).zip(&worktrees) {
        f.arm(n, worktree).await?;
        f.record_passed(&app, n, &git(worktree, &["rev-parse", "HEAD"])?)
            .await?;
    }
    f.advance_dev("other.txt")?;
    f.decide_until(&app, || async {
        app.ci_sync_sandbox(&sandbox).await?;
        f.settle(&app).await?;
        Ok(f.merged(&app, 1).await? && f.merged(&app, 2).await?)
    })
    .await?;
    app.ci_sync_sandbox(&sandbox).await?;
    f.settle(&app).await?;
    let outcome = f.wait(2, &worktrees[1]).await?["merge_outcome"]
        .as_str()
        .ok_or("merge outcome")?
        .to_owned();
    let landed = git(&f.origin, &["rev-parse", f.integration])?;
    assert!(
        outcome.contains("cd_deploy {\"environment\":\"staging\"}")
            && outcome.contains(&format!("your merged revision {}", &landed[..12]))
            && outcome.contains("If your final step requires the outcome, leave it in progress"),
        "{outcome}"
    );
    let mut trains = 0;
    for run in f.runs(&app).await? {
        if !matches!(run.reference, CiRef::Train(_)) {
            continue;
        }
        trains += 1;
        let jobs = app.deps.store.ci_jobs(&run.id, true).await?;
        assert!(
            jobs.iter()
                .filter(|j| j.key == "full")
                .all(|j| j.state == RunState::Skipped && !j.output.contains("full suite ran")),
            "{jobs:?}"
        );
    }
    assert!(trains > 0, "a train judged the two");
    let dev = git(&f.origin, &["rev-parse", "dev"])?;
    let head_run = f
        .runs(&app)
        .await?
        .into_iter()
        .find(|r| r.sha == dev && matches!(r.reference, CiRef::Branch(_)))
        .ok_or("dev head run")?;
    let jobs = app.deps.store.ci_jobs(&head_run.id, true).await?;
    assert!(
        jobs.iter().any(|j| j.key == "full"
            && j.state == RunState::Passed
            && j.output.contains("full suite ran")),
        "the merged head ran the full suite: {head_run:?} {jobs:?}"
    );
    Ok(())
}

/// SPEC-575 MFS-002 (review round 2): a merge candidate formed again reuses its tree, and its
/// copied verdict keeps the full suite's skip, so the member's verdict built from it never
/// stands in for the full suite: the merged head runs it.
#[tokio::test]
async fn a_reused_candidate_still_leaves_the_full_suite_to_the_merged_head() -> TestResult {
    let f = fixture().await?;
    let app = f.h.app.clone();
    let sandbox = app.deps.store.sandbox(&f.sandbox).await?.ok_or("s1")?;
    f.advance_dev_with(
        "yulius.toml",
        &format!(
            "{YULIUS_TOML}\n[[ci.jobs]]\nname = \"full\"\ncommand = 'echo full suite ran'\n\
             after_merge = true\n\n[cd.dev]\n[cd.prod]\n\n[cd.staging]\nstage = \"dev\"\n\
             merge_first = true\ndeploy = \"d\"\nverify = \"v\"\nrollback = \"r\"\n\
             migrations = \"none: stateless\"\n"
        ),
    )?;
    app.ci_sync_sandbox(&sandbox).await?;
    f.settle(&app).await?;
    let worktree = f.task(1, &[("f1.txt", "x\n")]).await?;
    f.arm(1, &worktree).await?;
    f.record_passed(&app, 1, &git(&worktree, &["rev-parse", "HEAD"])?)
        .await?;
    f.advance_dev("other-1.txt")?;
    app.merge_armed(&f.sandbox).await;
    let id = TaskId::parse("t1")?;
    f.h.store
        .transition_task(
            &id,
            None,
            TaskStatus::Running,
            Some(Activity::Working),
            app.now(),
        )
        .await?;
    f.settle(&app).await?;
    app.merge_armed(&f.sandbox).await;
    assert!(!f.merged(&app, 1).await?);
    f.h.clock.0.fetch_add(
        yulius_app::merge_train::WAIT_LIMIT_MS + 1000,
        Ordering::SeqCst,
    );
    app.merge_armed(&f.sandbox).await;
    f.h.store
        .transition_task(
            &id,
            None,
            TaskStatus::Running,
            Some(Activity::Waiting),
            app.now(),
        )
        .await?;
    f.decide_until(&app, || async {
        f.settle(&app).await?;
        f.merged(&app, 1).await
    })
    .await?;
    app.ci_sync_sandbox(&sandbox).await?;
    f.settle(&app).await?;
    app.ci_sync_sandbox(&sandbox).await?;
    f.settle(&app).await?;
    let trains = f.train_runs(&app).await?;
    assert!(
        trains.iter().any(|r| r
            .reason
            .as_deref()
            .is_some_and(|x| x.starts_with("not run again"))),
        "a candidate reused its tree: {trains:?}"
    );
    let dev = git(&f.origin, &["rev-parse", "dev"])?;
    let head_run = f
        .runs(&app)
        .await?
        .into_iter()
        .find(|r| r.sha == dev && matches!(r.reference, CiRef::Branch(_)))
        .ok_or("dev head run")?;
    let jobs = app.deps.store.ci_jobs(&head_run.id, true).await?;
    assert!(
        jobs.iter().any(|j| j.key == "full"
            && j.state == RunState::Passed
            && j.output.contains("full suite ran")),
        "the merged head ran the full suite: {head_run:?} {jobs:?}"
    );
    Ok(())
}
