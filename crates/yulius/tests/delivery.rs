//! Git page and delivery flows end to end (AC-DLV-1..3, AC-GIT-1/2, AC-DSC-2): the HTTP API
//! over the application with a fake GitHub whose pull requests and refs are backed by a real
//! bare repository, and a test-local `GitPort` over the real `git` command line.

mod support;

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use axum::extract::connect_info::MockConnectInfo;
use axum::http::{Method, StatusCode};
use serde_json::{Value, json};
use support::{Creds, FakeRemote, Resp, TestResult, call, harness_with_git};
use yulius_app::delivery::profile_to_contract;
use yulius_app::delivery::runner::{CommandEnd, CommandSpec, OutputRing, run_bounded};
use yulius_app::error::{AppError, AppResult};
use yulius_app::model::{
    AttemptRecord, GitHubTokenRecord, LaunchKind, SandboxRecord, Sealed, Secret, StepRecord,
    TaskOrigin, TaskRecord, UserRecord,
};
use yulius_app::ports::git::{CommitInfo, DiffStat, GitPort};
use yulius_app::ports::github::{ChecksReport, ChecksSource, ChecksState};
use yulius_app::ports::store::{
    CoordinationStore, InstanceStore, SandboxStore, TaskStore, UserStore,
};
use yulius_app::ports::store_ext::DeliveryStore;
use yulius_domain::delivery::{DeliveryProfile, DeliveryStage};
use yulius_domain::harness::Harness;
use yulius_domain::ids::{AttemptId, SandboxId, StepId, TaskId, UserId};
use yulius_domain::permission::GitHubPermissions;
use yulius_domain::task::{AttemptState, TaskStatus};
use yulius_domain::time::Timestamp;

type BoxResult<T> = Result<T, Box<dyn std::error::Error>>;

// ---------------------------------------------------------------- real git helpers

const GIT_ENV: [(&str, &str); 3] = [
    ("GIT_CONFIG_GLOBAL", "/dev/null"),
    ("GIT_CONFIG_NOSYSTEM", "1"),
    ("GIT_TERMINAL_PROMPT", "0"),
];
/// Identity for commits; no fsync (throwaway repositories: keeps tests fast on busy disks).
const IDENTITY: [&str; 8] = [
    "-c",
    "user.name=Yulius Test",
    "-c",
    "user.email=test@yulius.test",
    "-c",
    "commit.gpgsign=false",
    "-c",
    "core.fsync=none",
];

fn git_sync(dir: &Path, args: &[&str]) -> Result<String, String> {
    let output = Command::new("git")
        .args(IDENTITY)
        .args(args)
        .current_dir(dir)
        .envs(GIT_ENV)
        .output()
        .map_err(|e| e.to_string())?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
    } else {
        Err(format!(
            "git {args:?}: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ))
    }
}

fn git(dir: &Path, args: &[&str]) -> BoxResult<String> {
    Ok(git_sync(dir, args)?)
}

/// Test-local Git port over the real `git` command line (no network: "origin" is a local bare
/// repository; URLs are ignored, and pushes record the token they carried).
struct RealGit {
    origin: PathBuf,
    /// Pushes, in order: `<local>-><remote> token=<token>`.
    pushes: std::sync::Mutex<Vec<String>>,
}

impl RealGit {
    async fn run(&self, dir: &Path, args: &[&str]) -> AppResult<String> {
        let output = tokio::process::Command::new("git")
            .args(IDENTITY)
            .args(args)
            .current_dir(dir)
            .envs(GIT_ENV)
            .output()
            .await
            .map_err(AppError::internal)?;
        if output.status.success() {
            Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
        } else {
            Err(AppError::Upstream(format!(
                "git {}: {}",
                args.first().copied().unwrap_or(""),
                String::from_utf8_lossy(&output.stderr).trim()
            )))
        }
    }
}

#[async_trait]
impl GitPort for RealGit {
    fn valid_branch(&self, name: &str) -> bool {
        !name.is_empty()
            && !name.starts_with('-')
            && !name.contains("..")
            && !name.chars().any(|c| c.is_whitespace() || c.is_control())
    }
    async fn sync_clone(&self, _url: &str, dest: &Path, _token: &Secret) -> AppResult<()> {
        if dest.join(".git").exists() {
            self.run(dest, &["fetch", "--prune", "--quiet", "origin"])
                .await?;
        } else {
            let origin = self.origin.to_string_lossy().into_owned();
            let target = dest.to_string_lossy().into_owned();
            self.run(Path::new("/"), &["clone", "--quiet", &origin, &target])
                .await?;
        }
        Ok(())
    }
    async fn default_branch(&self, repo: &Path) -> AppResult<String> {
        let head = self
            .run(
                repo,
                &["symbolic-ref", "--short", "refs/remotes/origin/HEAD"],
            )
            .await?;
        Ok(head.trim_start_matches("origin/").to_owned())
    }
    async fn branch_exists(&self, repo: &Path, branch: &str) -> AppResult<bool> {
        for candidate in [
            format!("refs/heads/{branch}"),
            format!("refs/remotes/origin/{branch}"),
        ] {
            if self
                .run(repo, &["rev-parse", "--verify", "--quiet", &candidate])
                .await
                .is_ok()
            {
                return Ok(true);
            }
        }
        Ok(false)
    }
    async fn add_worktree(
        &self,
        repo: &Path,
        path: &Path,
        branch: &str,
        base: &str,
    ) -> AppResult<()> {
        let path = path.to_string_lossy().into_owned();
        self.run(
            repo,
            &["worktree", "add", "--quiet", "-b", branch, &path, base],
        )
        .await
        .map(drop)
    }
    async fn remove_worktree(&self, _: &Path, _: &Path, _: &Path) -> AppResult<bool> {
        Ok(false)
    }
    async fn has_changes(&self, worktree: &Path) -> AppResult<bool> {
        Ok(!self
            .run(worktree, &["status", "--porcelain"])
            .await?
            .is_empty())
    }
    async fn install_push_guard(&self, _: &Path, _: &[String]) -> AppResult<()> {
        Ok(())
    }
    async fn set_identity(&self, worktree: &Path, name: &str, email: &str) -> AppResult<()> {
        self.run(worktree, &["config", "user.name", name]).await?;
        self.run(worktree, &["config", "user.email", email])
            .await
            .map(drop)
    }
    /// The production adapter: the review reads real Git output.
    async fn review_changes(
        &self,
        worktree: &Path,
        base: &str,
    ) -> AppResult<yulius_app::ports::git::ReviewChanges> {
        yulius::wiring::git::GitAdapter::new("git")
            .review_changes(worktree, base)
            .await
    }

    async fn review_commits(
        &self,
        repo: &Path,
        base: &str,
        head: &str,
        limit: u32,
    ) -> AppResult<Vec<yulius_contracts::api::git::TaskCommit>> {
        yulius::wiring::git::GitAdapter::new("git")
            .review_commits(repo, base, head, limit)
            .await
    }

    async fn diff_stat(&self, repo: &Path, base: &str, head: &str) -> AppResult<DiffStat> {
        let text = self
            .run(repo, &["diff", "--shortstat", &format!("{base}...{head}")])
            .await?;
        let mut stat = DiffStat::default();
        for part in text.split(',') {
            let mut words = part.split_whitespace();
            let number = words
                .next()
                .and_then(|n| n.parse::<u32>().ok())
                .unwrap_or(0);
            match words.next() {
                Some(w) if w.starts_with("file") => stat.files = number,
                Some(w) if w.starts_with("insertion") => stat.insertions = number,
                Some(w) if w.starts_with("deletion") => stat.deletions = number,
                _ => {}
            }
        }
        Ok(stat)
    }
    async fn ahead_behind(&self, repo: &Path, base: &str, head: &str) -> AppResult<(u32, u32)> {
        let text = self
            .run(
                repo,
                &[
                    "rev-list",
                    "--left-right",
                    "--count",
                    &format!("{base}...{head}"),
                ],
            )
            .await?;
        let mut counts = text
            .split_whitespace()
            .map(|n| n.parse::<u32>().unwrap_or(0));
        let behind = counts.next().unwrap_or(0);
        let ahead = counts.next().unwrap_or(0);
        Ok((ahead, behind))
    }
    async fn log(&self, repo: &Path, rev: &str, limit: u32) -> AppResult<Vec<CommitInfo>> {
        let text = self
            .run(
                repo,
                &[
                    "log",
                    &format!("-n{limit}"),
                    "--format=%H%x1f%s%x1f%an%x1f%ct",
                    rev,
                    "--",
                ],
            )
            .await?;
        Ok(text
            .lines()
            .filter_map(|line| {
                let mut fields = line.split('\u{1f}');
                Some(CommitInfo {
                    sha: fields.next()?.to_owned(),
                    subject: fields.next()?.to_owned(),
                    author: fields.next()?.to_owned(),
                    at_ms: fields.next()?.parse::<i64>().ok()? * 1000,
                })
            })
            .collect())
    }
    async fn commit_all(
        &self,
        worktree: &Path,
        message: &str,
        _: &str,
        _: &str,
    ) -> AppResult<bool> {
        self.run(worktree, &["add", "-A"]).await?;
        if self
            .run(worktree, &["diff", "--cached", "--quiet"])
            .await
            .is_ok()
        {
            return Ok(false);
        }
        self.run(worktree, &["commit", "--quiet", "-m", message])
            .await
            .map(|_| true)
    }
    async fn push(
        &self,
        repo: &Path,
        local_branch: &str,
        remote_branch: &str,
        token: &Secret,
    ) -> AppResult<()> {
        if let Ok(mut pushes) = self.pushes.lock() {
            pushes.push(format!(
                "{local_branch}->{remote_branch} token={}",
                token.expose()
            ));
        }
        self.run(
            repo,
            &[
                "push",
                "--quiet",
                "origin",
                &format!("refs/heads/{local_branch}:refs/heads/{remote_branch}"),
            ],
        )
        .await
        .map(drop)
    }
    async fn import_bundle(&self, _: &Path, _: &Path, _: &str) -> AppResult<Vec<String>> {
        Err(AppError::Upstream("not used".into()))
    }
}

/// The fake GitHub's repository: merges and fast-forwards happen directly in the bare origin
/// (`merge-tree` + `commit-tree` + `update-ref`, no working tree).
struct BareRemote {
    origin: PathBuf,
}

impl FakeRemote for BareRemote {
    fn branch_sha(&self, branch: &str) -> Option<String> {
        // Pushed refs are loose files in the bare repository; fall back to git for packed ones.
        if let Ok(text) = std::fs::read_to_string(self.origin.join("refs/heads").join(branch)) {
            return Some(text.trim().to_owned());
        }
        git_sync(
            &self.origin,
            &[
                "rev-parse",
                "--verify",
                "--quiet",
                &format!("refs/heads/{branch}"),
            ],
        )
        .ok()
    }
    fn merge(
        &self,
        base: &str,
        head_sha: &str,
        method: &str,
        message: &str,
    ) -> Result<String, String> {
        let base_sha = self.branch_sha(base).ok_or("base branch not found")?;
        let tree = git_sync(
            &self.origin,
            &["merge-tree", "--write-tree", &base_sha, head_sha],
        )?;
        let commit = match method {
            "merge" => git_sync(
                &self.origin,
                &[
                    "commit-tree",
                    &tree,
                    "-p",
                    &base_sha,
                    "-p",
                    head_sha,
                    "-m",
                    message,
                ],
            )?,
            "squash" => git_sync(
                &self.origin,
                &["commit-tree", &tree, "-p", &base_sha, "-m", message],
            )?,
            other => return Err(format!("unsupported merge method {other}")),
        };
        git_sync(
            &self.origin,
            &[
                "update-ref",
                &format!("refs/heads/{base}"),
                &commit,
                &base_sha,
            ],
        )?;
        Ok(commit)
    }
    fn fast_forward(&self, branch: &str, sha: &str) -> Result<(), String> {
        let reference = format!("refs/heads/{branch}");
        let current = self.branch_sha(branch);
        if let Some(current) = &current {
            git_sync(&self.origin, &["merge-base", "--is-ancestor", current, sha])
                .map_err(|_| "not a fast-forward".to_owned())?;
        }
        let mut args = vec!["update-ref", reference.as_str(), sha];
        if let Some(current) = &current {
            args.push(current);
        }
        git_sync(&self.origin, &args).map(drop)
    }
    fn delete_branch(&self, branch: &str) -> Result<bool, String> {
        if self.branch_sha(branch).is_none() {
            return Ok(false);
        }
        git_sync(
            &self.origin,
            &["update-ref", "-d", &format!("refs/heads/{branch}")],
        )?;
        Ok(true)
    }
}

// ---------------------------------------------------------------- fixture

const SPEC_PATH: &str = "docs/specs/2026-10-07-checkout-api.md";

fn valid_spec(task: &str) -> String {
    format!(
        "---\nyulius-task: {task}\nstatus: in-progress\n---\n# Checkout API\n\n\
         ## Goal\nCustomers can pay for their cart.\n\n\
         ## Context\nThe shop has a cart but no checkout.\n\n\
         ## Requirements\n- POST /checkout creates an order.\n\n\
         ## Plan\n- [x] Write the spec\n- [ ] Implement the endpoint\n\n\
         ## Acceptance criteria\n- An order is created.\n\n\
         ## Test plan\n- Integration test for POST /checkout.\n\n\
         ## Decisions\n- None yet.\n"
    )
}

struct Fixture {
    h: support::Harness,
    git: Arc<RealGit>,
    _root: tempfile::TempDir,
    origin: PathBuf,
    repo: PathBuf,
    /// Admin on the repository.
    alice: Creds,
    /// Write access.
    bob: Creds,
    /// Read access.
    rita: Creds,
}

async fn add_user(
    h: &support::Harness,
    id: &str,
    login: &str,
    perms: GitHubPermissions,
) -> BoxResult<Creds> {
    let user = UserRecord {
        id: UserId::parse(id)?,
        github_login: Some(login.into()),
        github_id: None,
        display_name: login.to_uppercase(),
        avatar_url: None,
        is_superadmin: false,
        is_instance_admin: false,
        password_hash: None,
        created_at: Timestamp(1),
        disabled_at: None,
    };
    h.store.insert_user(&user).await?;
    let access = h
        .app
        .deps
        .crypto
        .seal(format!("token-{login}").as_bytes())?;
    h.store
        .save_github_token(
            &GitHubTokenRecord {
                user_id: user.id.clone(),
                access,
                refresh: None,
                expires_at: None,
                refresh_expires_at: None,
            },
            Timestamp(1),
        )
        .await?;
    h.github.set_perm(login, "Quartio-Studio/shop", perms);
    let token = h.app.issue_api_token(&user, "test").await?;
    Ok(Creds {
        bearer: Some(token.expose().to_owned()),
        ..Creds::default()
    })
}

async fn fixture(profile: &DeliveryProfile) -> BoxResult<Fixture> {
    let root = tempfile::tempdir()?;
    let origin = root.path().join("origin.git");
    git(
        root.path(),
        &[
            "init",
            "--quiet",
            "--bare",
            "-b",
            "main",
            &origin.to_string_lossy(),
        ],
    )?;
    let real_git = Arc::new(RealGit {
        origin: origin.clone(),
        pushes: std::sync::Mutex::default(),
    });
    let h = harness_with_git(real_git.clone())?;
    h.github.set_remote(Arc::new(BareRemote {
        origin: origin.clone(),
    }));
    h.store.init_instance("https://yulius.test", "x").await?;
    let app_cfg = json!({"app_id": 1, "slug": "yulius-test", "client_id": "c", "client_secret": "s", "private_key_pem": "k", "webhook_secret": null});
    let sealed = h.app.deps.crypto.seal(app_cfg.to_string().as_bytes())?;
    h.store
        .set_github_app(&sealed, "Quartio-Studio", "org")
        .await?;
    h.store
        .insert_sandbox(&SandboxRecord {
            id: SandboxId::parse("s1")?,
            slug: "shop".into(),
            repo_owner: "Quartio-Studio".into(),
            repo_name: "shop".into(),
            github_repo_id: None,
            default_branch: "main".into(),
            profile_json: serde_json::to_string(&profile_to_contract(profile))?,
            port_base: 20000,
            port_count: 20,
            onboarding_state: "not_needed".into(),
            onboarding_task_id: None,
            created_by: UserId::parse("u-alice")?,
            created_at: Timestamp(1),
            removed_at: None,
        })
        .await?;
    let repo = h.app.config.data_dir.join("sandboxes/s1/repo");
    std::fs::create_dir_all(repo.parent().ok_or("parent")?)?;
    git(
        root.path(),
        &[
            "clone",
            "--quiet",
            &origin.to_string_lossy(),
            &repo.to_string_lossy(),
        ],
    )?;
    git(
        &repo,
        &["commit", "--quiet", "--allow-empty", "-m", "Initial commit"],
    )?;
    git(
        &repo,
        &[
            "push",
            "--quiet",
            "origin",
            "HEAD:refs/heads/main",
            &format!("HEAD:refs/heads/{}", profile.integration_branch),
        ],
    )?;
    let alice = add_user(
        &h,
        "u-alice",
        "alice",
        GitHubPermissions {
            admin: true,
            push: true,
            pull: true,
            ..Default::default()
        },
    )
    .await?;
    let bob = add_user(
        &h,
        "u-bob",
        "bob",
        GitHubPermissions {
            push: true,
            pull: true,
            ..Default::default()
        },
    )
    .await?;
    let rita = add_user(
        &h,
        "u-rita",
        "rita",
        GitHubPermissions {
            pull: true,
            ..Default::default()
        },
    )
    .await?;
    Ok(Fixture {
        h,
        git: real_git,
        _root: root,
        origin,
        repo,
        alice,
        bob,
        rita,
    })
}

impl Fixture {
    /// A task of Bob's with a worktree on its own branch from the integration branch.
    async fn add_task(
        &self,
        id: &str,
        title: &str,
        branch: &str,
        base: &str,
    ) -> BoxResult<PathBuf> {
        self.add_task_by(id, title, branch, base, "u-bob").await
    }

    /// [`Fixture::add_task`] created by the user `creator`.
    async fn add_task_by(
        &self,
        id: &str,
        title: &str,
        branch: &str,
        base: &str,
        creator: &str,
    ) -> BoxResult<PathBuf> {
        let worktree = self
            .h
            .app
            .config
            .data_dir
            .join(format!("sandboxes/s1/worktrees/{id}"));
        git(&self.repo, &["fetch", "--quiet", "origin"])?;
        git(
            &self.repo,
            &[
                "worktree",
                "add",
                "--quiet",
                "-b",
                branch,
                &worktree.to_string_lossy(),
                &format!("origin/{base}"),
            ],
        )?;
        self.h
            .store
            .insert_task(&TaskRecord {
                id: TaskId::parse(id)?,
                sandbox_id: SandboxId::parse("s1")?,
                number: 0,
                title: title.into(),
                prompt: format!("Build: {title}"),
                harness: Harness::ClaudeCode,
                model: None,
                effort: None,
                workflow_id: None,
                status: TaskStatus::Closed,
                activity: None,
                branch: Some(branch.into()),
                worktree_path: Some(worktree.to_string_lossy().into_owned()),
                spec_path: None,
                origin: TaskOrigin::Ui,
                automation_run: None,
                created_by: UserId::parse(creator)?,
                created_at: Timestamp(1),
                updated_at: Timestamp(1),
                closed_at: None,
                archived_at: None,
                queued_at: None,
                gate_blocks: 0,
                wake_enabled: true,
                mode: yulius_app::model::TaskMode::Prompt,
            })
            .await?;
        Ok(worktree)
    }

    async fn req(
        &self,
        method: Method,
        path: &str,
        body: Option<Value>,
        who: &Creds,
    ) -> BoxResult<Resp> {
        call(&self.h.router, method, path, body, who).await
    }

    async fn delivery(&self, task: &str) -> BoxResult<Value> {
        let r = self
            .req(
                Method::GET,
                &format!("/api/tasks/{task}/delivery"),
                None,
                &self.bob,
            )
            .await?;
        assert_eq!(r.status, StatusCode::OK, "{}", r.body);
        Ok(r.body)
    }

    /// Pushes so far (`<local>-><remote> token=<token>`).
    fn pushes(&self) -> Vec<String> {
        self.git
            .pushes
            .lock()
            .map(|p| p.clone())
            .unwrap_or_default()
    }

    fn origin_sha(&self, branch: &str) -> BoxResult<String> {
        git(
            &self.origin,
            &["rev-parse", &format!("refs/heads/{branch}")],
        )
    }

    /// Polls an environment run until it leaves `running` (bounded).
    async fn wait_run(&self, run_id: &str) -> BoxResult<Value> {
        for _ in 0..400 {
            let r = self
                .req(
                    Method::GET,
                    &format!("/api/sandboxes/s1/delivery/runs/{run_id}"),
                    None,
                    &self.rita,
                )
                .await?;
            assert_eq!(r.status, StatusCode::OK, "{}", r.body);
            if r.body["state"] != "running" {
                return Ok(r.body);
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        Err("environment run did not finish".into())
    }
}

fn commit_file(worktree: &Path, path: &str, contents: &str, message: &str) -> BoxResult<()> {
    let full = worktree.join(path);
    std::fs::create_dir_all(full.parent().ok_or("parent")?)?;
    std::fs::write(full, contents)?;
    git(worktree, &["add", "-A"])?;
    git(worktree, &["commit", "--quiet", "-m", message])?;
    Ok(())
}

fn actions(delivery: &Value) -> Vec<String> {
    delivery["next_actions"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default()
}

fn error_message(r: &Resp) -> String {
    r.body["error"]["message"].as_str().unwrap_or("").to_owned()
}

fn product_with_deploy() -> Value {
    json!({"profile": {
        "preset": "product", "integration_branch": "quartio", "production_branch": "main",
        "merge": "merge", "promote": "fast_forward", "auto_promote": false,
        "environments": [
            {"name": "dev-vm", "after": "integration", "verify": "command",
             "command": "printf 'deployed %s on %s\\n' \"$(cat feature.txt)\" \"$YULIUS_ENVIRONMENT\""},
            {"name": "rollout", "after": "production", "verify": "manual", "command": null}
        ]
    }})
}

/// Replaces a task's steps with one step per status.
async fn set_steps(f: &Fixture, task: &str, statuses: &[&str]) -> BoxResult<()> {
    let id = TaskId::parse(task)?;
    let steps = statuses
        .iter()
        .enumerate()
        .map(|(i, status)| {
            Ok(StepRecord {
                id: StepId::parse(&format!("{task}-step-{i}"))?,
                task_id: id.clone(),
                ordinal: u32::try_from(i)?,
                title: format!("Step {i}"),
                status: (*status).to_owned(),
                note: None,
                updated_by: None,
                updated_at: None,
                effort: 1,
            })
        })
        .collect::<BoxResult<Vec<_>>>()?;
    f.h.store
        .replace_steps(&id, &steps, "test", Timestamp(2))
        .await?;
    Ok(())
}

// ---------------------------------------------------------------- tests

/// AC-DSC-2, AC-GIT-1: the PR is refused until the task has a valid committed spec, which is
/// discovered and registered; the Git page shows the branch facts, the PR and its checks.
#[tokio::test]
async fn pull_request_requires_a_valid_committed_spec() -> TestResult {
    let f = fixture(&DeliveryProfile::standard()).await?;
    let wt = f
        .add_task("t1", "Checkout API", "feat/1-checkout-api", "dev")
        .await?;
    commit_file(&wt, "feature.txt", "hello", "Add feature")?;

    // No spec: delivery explains it and refuses the pull request.
    let d = f.delivery("t1").await?;
    assert_eq!(d["stage"], "none");
    assert_eq!(d["spec_valid"], false);
    assert!(actions(&d).is_empty());
    assert_eq!(d["blocked"][0]["action"], "open_pr");
    let refused = f
        .req(Method::POST, "/api/tasks/t1/delivery/pr", None, &f.bob)
        .await?;
    assert_eq!(refused.status, StatusCode::CONFLICT);
    assert!(error_message(&refused).contains("spec"), "{}", refused.body);
    assert!(f.h.github.writes().is_empty(), "nothing reached GitHub");
    assert!(
        f.origin_sha("feat/1-checkout-api").is_err(),
        "nothing was pushed"
    );

    // An invalid spec is discovered, registered and its problems are reported.
    commit_file(
        &wt,
        SPEC_PATH,
        "---\nyulius-task: t1\nstatus: draft\n---\n## Goal\nPay.\n",
        "Draft spec",
    )?;
    let refused = f
        .req(
            Method::POST,
            "/api/tasks/t1/delivery/pr",
            Some(json!({})),
            &f.bob,
        )
        .await?;
    assert_eq!(refused.status, StatusCode::CONFLICT);
    assert!(
        error_message(&refused).contains("Missing section '## Context'"),
        "{}",
        refused.body
    );

    // A valid but uncommitted spec is refused too: the PR must contain what was validated.
    std::fs::write(wt.join(SPEC_PATH), valid_spec("t1"))?;
    let refused = f
        .req(Method::POST, "/api/tasks/t1/delivery/pr", None, &f.bob)
        .await?;
    assert_eq!(refused.status, StatusCode::CONFLICT);
    assert!(
        error_message(&refused).contains("uncommitted"),
        "{}",
        refused.body
    );
    git(&wt, &["commit", "--quiet", "-am", "Complete spec"])?;

    let d = f.delivery("t1").await?;
    assert_eq!(d["spec_valid"], true, "{d}");
    assert_eq!(d["spec_path"], SPEC_PATH);
    assert_eq!(actions(&d), ["open_pr"]);
    let detail = f.req(Method::GET, "/api/tasks/t1", None, &f.rita).await?;
    assert_eq!(
        detail.body["spec_path"], SPEC_PATH,
        "discovered spec is registered"
    );
    assert_eq!(detail.body["delivery"]["spec_valid"], true);

    // Read access cannot open the PR; write access can.
    let forbidden = f
        .req(Method::POST, "/api/tasks/t1/delivery/pr", None, &f.rita)
        .await?;
    assert_eq!(forbidden.status, StatusCode::FORBIDDEN);
    let opened = f
        .req(Method::POST, "/api/tasks/t1/delivery/pr", None, &f.bob)
        .await?;
    assert_eq!(opened.status, StatusCode::OK, "{}", opened.body);
    assert_eq!(opened.body["stage"], "pr_open");
    assert_eq!(opened.body["pr"]["number"], 1);
    assert_eq!(opened.body["pr"]["base"], "dev");
    assert_eq!(opened.body["pr"]["head"], "feat/1-checkout-api");
    let head = git(&wt, &["rev-parse", "HEAD"])?;
    assert_eq!(
        f.origin_sha("feat/1-checkout-api")?,
        head,
        "branch pushed first"
    );
    let pull = f.h.github.pull(1).ok_or("pull")?;
    assert_eq!(pull.title, "Checkout API");
    assert_eq!(
        f.h.github.writes(),
        ["create_pull #1 dev<-feat/1-checkout-api"]
    );

    // Git page (read access): branch facts, PR with checks, integration commits.
    let page = f
        .req(Method::GET, "/api/sandboxes/s1/git", None, &f.rita)
        .await?;
    assert_eq!(page.status, StatusCode::OK, "{}", page.body);
    assert_eq!(page.body["integration_branch"], "dev");
    assert_eq!(page.body["production_branch"], "main");
    let task = &page.body["tasks"][0];
    assert_eq!(task["branch"], "feat/1-checkout-api");
    assert_eq!(task["ahead"], 3);
    assert_eq!(task["behind"], 0);
    assert_eq!(task["files_changed"], 2);
    assert_eq!(task["pr"]["number"], 1);
    // The standard profile requires checks: none reported yet reads as pending (#68).
    assert_eq!(task["pr"]["checks"], "pending");
    assert_eq!(page.body["pulls"].as_array().map(Vec::len), Some(1));
    assert_eq!(page.body["recent_commits"][0]["subject"], "Initial commit");
    assert_eq!(page.body["notes"], json!([]));

    Ok(())
}

/// AC-DLV-1: standard profile — merging waits for green checks; the dev checkpoint gates the
/// promotion, which opens a promotion PR dev → main and merges it on a later request.
#[tokio::test]
async fn standard_flow_merges_on_green_checks_and_promotes_by_pr() -> TestResult {
    let f = fixture(&DeliveryProfile::standard()).await?;
    let wt = f
        .add_task("t1", "Checkout API", "feat/1-checkout-api", "dev")
        .await?;
    commit_file(&wt, "feature.txt", "hello", "Add feature")?;
    commit_file(&wt, SPEC_PATH, &valid_spec("t1"), "Spec")?;
    let opened = f
        .req(Method::POST, "/api/tasks/t1/delivery/pr", None, &f.bob)
        .await?;
    assert_eq!(opened.status, StatusCode::OK, "{}", opened.body);
    let head = git(&wt, &["rev-parse", "HEAD"])?;

    // Checks gate the merge (standard requires them).
    f.h.github.set_checks(&head, ChecksState::Pending);
    let pending = f
        .req(Method::POST, "/api/tasks/t1/delivery/merge", None, &f.bob)
        .await?;
    assert_eq!(pending.status, StatusCode::CONFLICT);
    assert!(error_message(&pending).contains("still running"));
    f.h.github.set_checks(&head, ChecksState::Failure);
    let failing = f
        .req(Method::POST, "/api/tasks/t1/delivery/merge", None, &f.bob)
        .await?;
    assert!(error_message(&failing).contains("checks failed"));
    let d = f.delivery("t1").await?;
    assert_eq!(d["pr"]["checks"], "failure");
    assert_eq!(d["blocked"][0]["action"], "merge");
    let forbidden = f
        .req(Method::POST, "/api/tasks/t1/delivery/merge", None, &f.rita)
        .await?;
    assert_eq!(forbidden.status, StatusCode::FORBIDDEN);
    f.h.github.set_checks(&head, ChecksState::Success);
    let merged = f
        .req(Method::POST, "/api/tasks/t1/delivery/merge", None, &f.bob)
        .await?;
    assert_eq!(merged.status, StatusCode::OK, "{}", merged.body);
    assert_eq!(merged.body["stage"], "merged_integration");
    assert_eq!(merged.body["pr"]["state"], "merged");
    assert_eq!(actions(&merged.body), ["confirm_env:dev"]);
    assert!(
        f.h.github
            .writes()
            .contains(&"merge_pull #1 squash".to_owned())
    );
    assert_eq!(git(&f.origin, &["show", "dev:feature.txt"])?, "hello");
    // Hygiene (DLV-007): the merged head branch is deleted on GitHub.
    assert!(
        f.h.github
            .writes()
            .contains(&"delete_branch feat/1-checkout-api".to_owned()),
        "{:?}",
        f.h.github.writes()
    );
    assert!(
        git(
            &f.origin,
            &[
                "rev-parse",
                "--verify",
                "--quiet",
                "refs/heads/feat/1-checkout-api"
            ]
        )
        .is_err()
    );

    // The dev checkpoint comes first and needs an explicit confirmation.
    let early = f
        .req(
            Method::POST,
            "/api/sandboxes/s1/delivery/promote",
            None,
            &f.bob,
        )
        .await?;
    assert_eq!(early.status, StatusCode::CONFLICT);
    assert!(
        error_message(&early).contains("environment 'dev' is not done"),
        "{}",
        early.body
    );
    let unconfirmed = f
        .req(
            Method::POST,
            "/api/sandboxes/s1/delivery/environments/dev/runs",
            Some(json!({})),
            &f.bob,
        )
        .await?;
    assert_eq!(unconfirmed.status, StatusCode::UNPROCESSABLE_ENTITY);
    let confirmed = f
        .req(
            Method::POST,
            "/api/sandboxes/s1/delivery/environments/dev/runs",
            Some(json!({"confirm": true})),
            &f.bob,
        )
        .await?;
    assert_eq!(confirmed.status, StatusCode::OK, "{}", confirmed.body);
    assert_eq!(confirmed.body["kind"], "manual");
    assert_eq!(confirmed.body["state"], "succeeded");
    assert_eq!(confirmed.body["requested_by"], "bob");
    assert!(
        confirmed.body["output"]
            .as_str()
            .unwrap_or("")
            .contains("Tasks advanced: #1")
    );
    let d = f.delivery("t1").await?;
    assert_eq!(d["stage"], "env_done");
    assert_eq!(d["environment"], "dev");
    assert_eq!(actions(&d), ["promote"]);

    // Promotion: first a promotion PR, merged by a later request once its checks allow.
    let main_before = f.origin_sha("main")?;
    let opened = f
        .req(
            Method::POST,
            "/api/sandboxes/s1/delivery/promote",
            None,
            &f.bob,
        )
        .await?;
    assert_eq!(opened.status, StatusCode::OK, "{}", opened.body);
    assert_eq!(opened.body["result"], "pr_opened");
    assert!(
        opened.body["sha"]
            .as_str()
            .unwrap_or("")
            .ends_with("/pull/2")
    );
    assert!(
        f.h.github
            .writes()
            .contains(&"create_pull #2 main<-dev".to_owned())
    );
    assert_eq!(
        f.origin_sha("main")?,
        main_before,
        "opening the PR changes nothing"
    );
    assert_eq!(f.delivery("t1").await?["stage"], "env_done");
    let promotion_head = f.h.github.pull(2).ok_or("pull 2")?.head_sha;
    f.h.github.set_checks(&promotion_head, ChecksState::Pending);
    let waiting = f
        .req(
            Method::POST,
            "/api/sandboxes/s1/delivery/promote",
            None,
            &f.bob,
        )
        .await?;
    assert_eq!(waiting.status, StatusCode::CONFLICT);
    // Checks that can't be read never count as passed for the release either (#111).
    f.h.github.set_checks_report(
        &promotion_head,
        ChecksReport {
            state: ChecksState::Unknown,
            unreadable: vec![ChecksSource::CheckRuns, ChecksSource::CommitStatuses],
        },
    );
    let unknown = f
        .req(
            Method::POST,
            "/api/sandboxes/s1/delivery/promote",
            None,
            &f.bob,
        )
        .await?;
    assert_eq!(unknown.status, StatusCode::CONFLICT);
    assert!(
        error_message(&unknown).contains("can't be read"),
        "{}",
        unknown.body
    );
    f.h.github.set_checks(&promotion_head, ChecksState::Success);
    let promoted = f
        .req(
            Method::POST,
            "/api/sandboxes/s1/delivery/promote",
            None,
            &f.bob,
        )
        .await?;
    assert_eq!(promoted.status, StatusCode::OK, "{}", promoted.body);
    assert_eq!(promoted.body["result"], "promoted");
    assert!(
        f.h.github
            .writes()
            .contains(&"merge_pull #2 merge".to_owned())
    );
    assert_eq!(git(&f.origin, &["show", "main:feature.txt"])?, "hello");
    let d = f.delivery("t1").await?;
    assert_eq!(d["stage"], "production_done");
    assert!(actions(&d).is_empty());
    Ok(())
}

/// AC-DLV-2: product profile — PR into quartio, the dev-VM deployment runs only on an explicit
/// confirmed request and records its output, then main fast-forwards and the rollout step is
/// confirmed. Only repository admins change the profile.
#[tokio::test]
async fn product_flow_deploys_on_confirmation_then_fast_forwards_and_rolls_out() -> TestResult {
    let f = fixture(&DeliveryProfile::product()).await?;
    let denied = f
        .req(
            Method::PATCH,
            "/api/sandboxes/s1",
            Some(product_with_deploy()),
            &f.bob,
        )
        .await?;
    assert_eq!(
        denied.status,
        StatusCode::FORBIDDEN,
        "write access cannot change the profile"
    );
    let patched = f
        .req(
            Method::PATCH,
            "/api/sandboxes/s1",
            Some(product_with_deploy()),
            &f.alice,
        )
        .await?;
    assert_eq!(patched.status, StatusCode::OK, "{}", patched.body);
    assert_eq!(
        patched.body["profile"]["preset"], "custom",
        "a deploy command differs from the preset"
    );
    assert_eq!(patched.body["profile"]["require_checks"], true);
    let invalid = f
        .req(
            Method::PATCH,
            "/api/sandboxes/s1",
            Some(json!({"profile": {"preset": "product", "integration_branch": "main", "production_branch": "main", "merge": "merge", "promote": "fast_forward", "auto_promote": false, "environments": []}})),
            &f.alice,
        )
        .await?;
    assert_eq!(invalid.status, StatusCode::UNPROCESSABLE_ENTITY);

    let wt = f
        .add_task("t1", "Checkout API", "feat/1-checkout-api", "quartio")
        .await?;
    commit_file(&wt, "feature.txt", "hello", "Add feature")?;
    commit_file(&wt, SPEC_PATH, &valid_spec("t1"), "Spec")?;
    let opened = f
        .req(
            Method::POST,
            "/api/tasks/t1/delivery/pr",
            Some(json!({"title": "Checkout"})),
            &f.bob,
        )
        .await?;
    assert_eq!(opened.status, StatusCode::OK, "{}", opened.body);
    assert_eq!(opened.body["pr"]["base"], "quartio");
    assert_eq!(f.h.github.pull(1).ok_or("pull")?.title, "Checkout");
    let merged = f
        .req(Method::POST, "/api/tasks/t1/delivery/merge", None, &f.bob)
        .await?;
    assert_eq!(merged.status, StatusCode::OK, "{}", merged.body);
    assert!(
        f.h.github
            .writes()
            .contains(&"merge_pull #1 merge".to_owned())
    );
    assert_eq!(actions(&merged.body), ["run_env:dev-vm"]);

    // No deployment without the explicit confirmation; nothing is recorded.
    let unconfirmed = f
        .req(
            Method::POST,
            "/api/sandboxes/s1/delivery/environments/dev-vm/runs",
            None,
            &f.bob,
        )
        .await?;
    assert_eq!(unconfirmed.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(error_message(&unconfirmed).contains("explicit confirmation"));
    let runs = f
        .req(
            Method::GET,
            "/api/sandboxes/s1/delivery/runs",
            None,
            &f.rita,
        )
        .await?;
    assert_eq!(runs.body, json!([]));
    let read_only = f
        .req(
            Method::POST,
            "/api/sandboxes/s1/delivery/environments/dev-vm/runs",
            Some(json!({"confirm": true})),
            &f.rita,
        )
        .await?;
    assert_eq!(read_only.status, StatusCode::FORBIDDEN);
    let out_of_order = f
        .req(
            Method::POST,
            "/api/sandboxes/s1/delivery/environments/rollout/runs",
            Some(json!({"confirm": true})),
            &f.bob,
        )
        .await?;
    assert_eq!(
        out_of_order.status,
        StatusCode::OK,
        "rollout is standalone: no task is in production yet"
    );
    let early = f
        .req(
            Method::POST,
            "/api/sandboxes/s1/delivery/promote",
            None,
            &f.bob,
        )
        .await?;
    assert!(
        error_message(&early).contains("environment 'dev-vm' is not done"),
        "{}",
        early.body
    );

    // The confirmed run checks out the integration head and records the command output. A hook
    // an agent planted in the shared clone's `.git` is not executed by the server's checkout.
    let marker = f.repo.join("hook-ran");
    let hook = f.repo.join(".git/hooks/post-checkout");
    std::fs::create_dir_all(hook.parent().ok_or("hooks dir")?)?;
    std::fs::write(&hook, format!("#!/bin/sh\ntouch '{}'\n", marker.display()))?;
    std::fs::set_permissions(&hook, std::os::unix::fs::PermissionsExt::from_mode(0o755))?;
    let started = f
        .req(
            Method::POST,
            "/api/sandboxes/s1/delivery/environments/dev-vm/runs",
            Some(json!({"confirm": true})),
            &f.bob,
        )
        .await?;
    assert_eq!(started.status, StatusCode::OK, "{}", started.body);
    assert_eq!(started.body["kind"], "command");
    assert!(
        started.body["command"]
            .as_str()
            .unwrap_or("")
            .starts_with("printf")
    );
    let run_id = started.body["run_id"].as_str().ok_or("run id")?.to_owned();
    let run = f.wait_run(&run_id).await?;
    assert_eq!(run["state"], "succeeded", "{run}");
    assert_eq!(run["exit_code"], 0);
    assert!(
        run["output"]
            .as_str()
            .unwrap_or("")
            .contains("deployed hello on dev-vm"),
        "{run}"
    );
    assert_eq!(run["commit"], f.origin_sha("quartio")?.as_str());
    assert!(!marker.exists(), "planted post-checkout hook must not run");
    let d = f.delivery("t1").await?;
    assert_eq!(d["stage"], "env_done");
    assert_eq!(d["environment"], "dev-vm");
    assert_eq!(actions(&d), ["promote"]);

    // A redeploy is allowed (still confirmed) and changes no stage.
    let again = f
        .req(
            Method::POST,
            "/api/sandboxes/s1/delivery/environments/dev-vm/runs",
            Some(json!({"confirm": true})),
            &f.bob,
        )
        .await?;
    assert_eq!(again.status, StatusCode::OK, "{}", again.body);
    let rerun = f
        .wait_run(again.body["run_id"].as_str().ok_or("run id")?)
        .await?;
    assert_eq!(rerun["state"], "succeeded", "{rerun}");
    assert_eq!(f.delivery("t1").await?["environment"], "dev-vm");

    // Fast-forward main to the integration head, then confirm the rollout step.
    let quartio = f.origin_sha("quartio")?;
    let promoted = f
        .req(
            Method::POST,
            "/api/sandboxes/s1/delivery/promote",
            None,
            &f.bob,
        )
        .await?;
    assert_eq!(promoted.status, StatusCode::OK, "{}", promoted.body);
    assert_eq!(promoted.body["result"], "promoted");
    assert_eq!(promoted.body["sha"], quartio.as_str());
    assert!(
        f.h.github
            .writes()
            .contains(&format!("fast_forward main {quartio}"))
    );
    assert_eq!(f.origin_sha("main")?, quartio);
    let d = f.delivery("t1").await?;
    assert_eq!(d["stage"], "promoted");
    assert_eq!(actions(&d), ["confirm_env:rollout"]);
    let again = f
        .req(
            Method::POST,
            "/api/sandboxes/s1/delivery/promote",
            None,
            &f.bob,
        )
        .await?;
    assert_eq!(
        again.status,
        StatusCode::CONFLICT,
        "nothing left to promote"
    );
    let rollout = f
        .req(
            Method::POST,
            "/api/sandboxes/s1/delivery/environments/rollout/confirm",
            Some(json!({"confirm": true})),
            &f.bob,
        )
        .await?;
    assert_eq!(rollout.status, StatusCode::OK, "{}", rollout.body);
    assert_eq!(f.delivery("t1").await?["stage"], "production_done");
    let runs = f
        .req(
            Method::GET,
            "/api/sandboxes/s1/delivery/runs",
            None,
            &f.rita,
        )
        .await?;
    assert_eq!(runs.body.as_array().map(Vec::len), Some(4));
    assert_eq!(runs.body[0]["kind"], "manual", "newest first");
    Ok(())
}

/// AC-DLV-3: studio — after a merge into quartio, main fast-forwards automatically even with
/// failing checks (none are required); switching the profile to gated disables it.
#[tokio::test]
async fn studio_auto_promotes_until_the_profile_is_gated() -> TestResult {
    let f = fixture(&DeliveryProfile::studio()).await?;
    let wt = f
        .add_task("t1", "Landing page", "feat/1-landing", "quartio")
        .await?;
    commit_file(&wt, "landing.txt", "v1", "Landing")?;
    commit_file(&wt, SPEC_PATH, &valid_spec("t1"), "Spec")?;
    let opened = f
        .req(Method::POST, "/api/tasks/t1/delivery/pr", None, &f.bob)
        .await?;
    assert_eq!(opened.status, StatusCode::OK, "{}", opened.body);
    f.h.github
        .set_checks(&git(&wt, &["rev-parse", "HEAD"])?, ChecksState::Failure);
    let merged = f
        .req(Method::POST, "/api/tasks/t1/delivery/merge", None, &f.bob)
        .await?;
    assert_eq!(merged.status, StatusCode::OK, "{}", merged.body);
    assert_eq!(merged.body["stage"], "production_done", "{}", merged.body);
    let quartio = f.origin_sha("quartio")?;
    assert_eq!(
        f.origin_sha("main")?,
        quartio,
        "main fast-forwarded immediately"
    );
    assert!(
        f.h.github
            .writes()
            .contains(&format!("fast_forward main {quartio}"))
    );
    assert!(
        merged.body["notes"]
            .to_string()
            .contains("Automatic promotion"),
        "{}",
        merged.body
    );

    // Gate the profile: one change (auto_promote off) relabels it and stops auto-promotion.
    let mut gated = serde_json::to_value(profile_to_contract(&DeliveryProfile::studio()))?;
    gated["auto_promote"] = json!(false);
    let patched = f
        .req(
            Method::PATCH,
            "/api/sandboxes/s1",
            Some(json!({"profile": gated})),
            &f.alice,
        )
        .await?;
    assert_eq!(patched.status, StatusCode::OK, "{}", patched.body);
    assert_eq!(patched.body["profile"]["preset"], "custom");
    assert_eq!(patched.body["profile"]["auto_promote"], false);

    let wt2 = f
        .add_task("t2", "Pricing page", "feat/2-pricing", "quartio")
        .await?;
    commit_file(&wt2, "pricing.txt", "v1", "Pricing")?;
    commit_file(
        &wt2,
        "docs/specs/2026-10-07-pricing.md",
        &valid_spec("t2"),
        "Spec",
    )?;
    let opened = f
        .req(Method::POST, "/api/tasks/t2/delivery/pr", None, &f.bob)
        .await?;
    assert_eq!(opened.status, StatusCode::OK, "{}", opened.body);
    let writes_before = f.h.github.writes().len();
    let merged = f
        .req(Method::POST, "/api/tasks/t2/delivery/merge", None, &f.bob)
        .await?;
    assert_eq!(merged.status, StatusCode::OK, "{}", merged.body);
    assert_eq!(merged.body["stage"], "merged_integration");
    assert_eq!(actions(&merged.body), ["promote"]);
    // Only the merge and the merged branch's deletion (DLV-007), no promotion.
    let after: Vec<String> = f.h.github.writes()[writes_before..].to_vec();
    assert_eq!(after.len(), 2, "{after:?}");
    assert!(after[0].starts_with("merge_pull #"), "{after:?}");
    assert_eq!(after[1], "delete_branch feat/2-pricing");
    assert_eq!(f.origin_sha("main")?, quartio, "main did not move");

    let promoted = f
        .req(
            Method::POST,
            "/api/sandboxes/s1/delivery/promote",
            None,
            &f.bob,
        )
        .await?;
    assert_eq!(promoted.status, StatusCode::OK, "{}", promoted.body);
    assert_eq!(f.origin_sha("main")?, f.origin_sha("quartio")?);
    assert_eq!(f.delivery("t2").await?["stage"], "production_done");
    Ok(())
}

/// GIT-002: a pull request head that differs from the head that was checked is never merged.
#[tokio::test]
async fn stale_heads_are_refused() -> TestResult {
    let f = fixture(&DeliveryProfile::standard()).await?;
    let wt = f
        .add_task("t1", "Checkout API", "feat/1-checkout-api", "dev")
        .await?;
    commit_file(&wt, "feature.txt", "hello", "Add feature")?;
    commit_file(&wt, SPEC_PATH, &valid_spec("t1"), "Spec")?;
    let opened = f
        .req(Method::POST, "/api/tasks/t1/delivery/pr", None, &f.bob)
        .await?;
    assert_eq!(opened.status, StatusCode::OK, "{}", opened.body);

    // The agent committed more after the PR was opened: the PR head is stale.
    commit_file(&wt, "feature.txt", "hello again", "Change after review")?;
    let stale = f
        .req(Method::POST, "/api/tasks/t1/delivery/merge", None, &f.bob)
        .await?;
    assert_eq!(stale.status, StatusCode::CONFLICT);
    assert!(
        error_message(&stale).contains("head changed"),
        "{}",
        stale.body
    );
    let d = f.delivery("t1").await?;
    assert_eq!(d["blocked"][0]["action"], "merge");

    // Someone pushed to the PR branch outside Yulius: still not the checked head.
    git(
        &wt,
        &[
            "push",
            "--quiet",
            "origin",
            "HEAD:refs/heads/feat/1-checkout-api",
        ],
    )?;
    git(&wt, &["reset", "--quiet", "--hard", "HEAD~1"])?;
    let stale = f
        .req(Method::POST, "/api/tasks/t1/delivery/merge", None, &f.bob)
        .await?;
    assert_eq!(stale.status, StatusCode::CONFLICT);
    assert!(
        !f.h.github
            .writes()
            .iter()
            .any(|w| w.starts_with("merge_pull"))
    );

    // Heads agree again: the merge goes through at exactly that head.
    git(
        &wt,
        &[
            "pull",
            "--quiet",
            "--ff-only",
            "origin",
            "feat/1-checkout-api",
        ],
    )?;
    let merged = f
        .req(Method::POST, "/api/tasks/t1/delivery/merge", None, &f.bob)
        .await?;
    assert_eq!(merged.status, StatusCode::OK, "{}", merged.body);
    assert_eq!(git(&f.origin, &["show", "dev:feature.txt"])?, "hello again");
    Ok(())
}

/// DLV-003 bound: an environment command that outlives its timeout is killed together with the
/// processes it started (the whole process group), and the shell is reaped.
#[tokio::test]
async fn environment_timeout_kills_the_process_tree() -> TestResult {
    let dir = tempfile::tempdir()?;
    let spec = CommandSpec {
        argv: vec![
            "/bin/sh".into(),
            "-c".into(),
            "sleep 60 & echo $! > child.pid; echo started; wait".into(),
        ],
        cwd: dir.path().to_path_buf(),
        env: vec![("PATH".into(), "/usr/bin:/bin".into())],
        timeout: Duration::from_millis(800),
    };
    let mut ring = OutputRing::new(4096);
    let started = std::time::Instant::now();
    let end = run_bounded(&spec, &mut ring, None).await;
    assert_eq!(end, CommandEnd::TimedOut);
    assert!(
        started.elapsed() < Duration::from_secs(15),
        "returned promptly"
    );
    assert!(ring.text().contains("started") && ring.text().contains("timed out"));
    let pid = std::fs::read_to_string(dir.path().join("child.pid"))?
        .trim()
        .to_owned();
    assert!(!pid.is_empty());
    let alive = |pid: &str| {
        Command::new("ps")
            .args(["-o", "stat=", "-p", pid])
            .output()
            .map(|o| {
                let stat = String::from_utf8_lossy(&o.stdout).trim().to_owned();
                o.status.success() && !stat.is_empty() && !stat.starts_with('Z')
            })
            .unwrap_or(false)
    };
    let mut gone = false;
    for _ in 0..100 {
        if !alive(&pid) {
            gone = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(
        gone,
        "the background child {pid} of the timed-out command was killed"
    );
    Ok(())
}

/// AC-GIT-2, AC-DLV-8 (#139): agents deliver their own work only through the gated delivery
/// tools — a merge needs passed checks and an independent review of the exact head — while reads,
/// task tokens on the REST API and read-only users never merge, promote or deploy.
#[tokio::test]
async fn agents_merge_only_through_gated_tools() -> TestResult {
    let f = fixture(&DeliveryProfile::studio()).await?;
    let wt = f
        .add_task("t1", "Landing page", "feat/1-landing", "quartio")
        .await?;
    commit_file(&wt, "landing.txt", "v1", "Landing")?;
    commit_file(&wt, SPEC_PATH, &valid_spec("t1"), "Spec")?;
    let opened = f
        .req(Method::POST, "/api/tasks/t1/delivery/pr", None, &f.bob)
        .await?;
    assert_eq!(opened.status, StatusCode::OK, "{}", opened.body);
    let head = git(&wt, &["rev-parse", "HEAD"])?;
    f.h.github.set_checks(&head, ChecksState::Pending);
    let main_before = f.origin_sha("main")?;

    // An agent run of the task.
    f.h.store
        .insert_attempt(&AttemptRecord {
            id: AttemptId::parse("a1")?,
            task_id: TaskId::parse("t1")?,
            seq: 1,
            harness: Harness::ClaudeCode,
            account_id: None,
            provider_session_id: None,
            launch_kind: LaunchKind::New,
            state: AttemptState::Live,
            run_dir: "/tmp/none".into(),
            token_hash: f.h.app.deps.crypto.sha256_hex(b"tok-agent"),
            host_token: Sealed(vec![0]),
            ptyhost_pid: None,
            child_pid: None,
            started_at: Timestamp(1),
            ended_at: None,
            exit_code: None,
            exit_reason: None,
        })
        .await?;
    let internal = yulius_server::internal_router(f.h.app.clone())
        .layer(MockConnectInfo(SocketAddr::from(([127, 0, 0, 1], 1))));
    let agent = Creds {
        bearer: Some("tok-agent".into()),
        ..Creds::default()
    };
    let rpc = |method: &str, params: Value| {
        call(
            &internal,
            Method::POST,
            "/mcp",
            Some(json!({"jsonrpc": "2.0", "id": 1, "method": method, "params": params})),
            &agent,
        )
    };
    let tools = rpc("tools/list", json!({})).await?;
    let names: Vec<String> = tools.body["result"]["tools"]
        .as_array()
        .ok_or("tools")?
        .iter()
        .filter_map(|t| t["name"].as_str().map(str::to_owned))
        .collect();
    for tool in [
        "delivery_status",
        "delivery_merge",
        "delivery_deploy",
        "delivery_promote",
    ] {
        assert!(names.contains(&tool.to_owned()), "{tool}");
    }
    let delivery_tools = names
        .iter()
        .filter(|n| {
            ["merge", "promote", "deploy", "environment"]
                .iter()
                .any(|w| n.contains(w))
        })
        .count();
    assert_eq!(
        delivery_tools, 3,
        "no other way to merge, deploy or promote: {names:?}"
    );
    let status = rpc(
        "tools/call",
        json!({"name": "delivery_status", "arguments": {}}),
    )
    .await?;
    let text = status.body["result"]["content"][0]["text"]
        .as_str()
        .ok_or("text")?;
    let status: Value = serde_json::from_str(text)?;
    assert_eq!(status["stage"], "pr_open", "{status}");
    assert_eq!(status["pull_request"]["number"], 1);
    assert_eq!(status["profile"]["preset"], "studio");
    assert_eq!(status["next_actions_for_people"], json!(["merge"]));
    let tool_text = |r: &Resp| {
        r.body["result"]["content"][0]["text"]
            .as_str()
            .unwrap_or("")
            .to_owned()
    };
    // No review, a review of another head, or checks still running: refused.
    let attempt = rpc(
        "tools/call",
        json!({"name": "delivery_merge", "arguments": {}}),
    )
    .await?;
    assert_eq!(attempt.body["result"]["isError"], true);
    assert!(
        tool_text(&attempt).contains("reviewed_head"),
        "{}",
        attempt.body
    );
    let stale = rpc(
        "tools/call",
        json!({"name": "delivery_merge", "arguments": {"reviewed_head": "0000000"}}),
    )
    .await?;
    assert_eq!(stale.body["result"]["isError"], true);
    assert!(
        tool_text(&stale).contains("review the new head"),
        "{}",
        stale.body
    );
    let pending = rpc(
        "tools/call",
        json!({"name": "delivery_merge", "arguments": {"reviewed_head": head}}),
    )
    .await?;
    assert_eq!(pending.body["result"]["isError"], true);
    assert!(
        tool_text(&pending).contains("merge on green only"),
        "{}",
        pending.body
    );
    // A person-verified or unknown environment is never run by an agent.
    let deploy = rpc(
        "tools/call",
        json!({"name": "delivery_deploy", "arguments": {"environment": "rollout"}}),
    )
    .await?;
    assert_eq!(deploy.body["result"]["isError"], true);
    let direct = call(
        &f.h.router,
        Method::POST,
        "/api/tasks/t1/delivery/merge",
        None,
        &agent,
    )
    .await?;
    assert_eq!(
        direct.status,
        StatusCode::UNAUTHORIZED,
        "task tokens are not API credentials"
    );

    // Every read path, repeatedly, by every reader.
    for who in [&f.rita, &f.bob, &f.alice] {
        for path in [
            "/api/tasks/t1/delivery",
            "/api/tasks/t1",
            "/api/sandboxes/s1/git",
            "/api/sandboxes/s1/delivery/runs",
            "/api/sandboxes/s1",
        ] {
            let r = f.req(Method::GET, path, None, who).await?;
            assert_eq!(r.status, StatusCode::OK, "{path}: {}", r.body);
        }
    }
    // Read-only users cannot act.
    for path in [
        "/api/tasks/t1/delivery/merge",
        "/api/sandboxes/s1/delivery/promote",
        "/api/sandboxes/s1/delivery/environments/x/runs",
    ] {
        let r = f
            .req(Method::POST, path, Some(json!({"confirm": true})), &f.rita)
            .await?;
        assert_eq!(r.status, StatusCode::FORBIDDEN, "{path}");
    }

    assert_eq!(
        f.h.github.writes(),
        ["create_pull #1 quartio<-feat/1-landing"]
    );
    assert_eq!(f.origin_sha("main")?, main_before);
    assert_eq!(f.delivery("t1").await?["stage"], "pr_open");

    // On green, with the reviewed head, the agent merges with its creator's authority (and then
    // the explicit profile policy promotes).
    f.h.github.set_checks(&head, ChecksState::Success);
    let merged = rpc(
        "tools/call",
        json!({"name": "delivery_merge", "arguments": {"reviewed_head": &head[..12], "review_summary": "approve: code and tests checked"}}),
    )
    .await?;
    assert_eq!(merged.body["result"]["isError"], false, "{}", merged.body);
    assert_eq!(f.delivery("t1").await?["pr"]["state"], "merged");
    assert_eq!(f.origin_sha("main")?, f.origin_sha("quartio")?);
    // F 2026-10-11 (AC-GIT-3): on GitHub the creator merged and promoted, not the App.
    assert_eq!(f.h.github.actors_of("merge_pull"), ["bob"]);
    assert_eq!(f.h.github.actors_of("fast_forward"), ["bob"]);

    // Follow-up work becomes a ready issue (Definition of Ready), never a vague one.
    let vague = rpc(
        "tools/call",
        json!({"name": "issue_create", "arguments": {"title": "Fix things", "body": "Stuff.", "labels": ["type: bug"]}}),
    )
    .await?;
    assert_eq!(vague.body["result"]["isError"], true);
    assert!(tool_text(&vague).contains("not ready"), "{}", vague.body);
    let ready = rpc(
        "tools/call",
        json!({"name": "issue_create", "arguments": {
            "title": "Flaky landing test",
            "body": "Fails 1 in 20 runs.\n\n## Acceptance criteria\n- [ ] Passes 100 runs under load",
            "labels": ["type: bug", "priority: medium", "area: frontend"]
        }}),
    )
    .await?;
    assert_eq!(ready.body["result"]["isError"], false, "{}", ready.body);
    assert!(
        f.h.github
            .writes()
            .iter()
            .any(|w| w == "create_issue #101 Flaky landing test"),
        "{:?}",
        f.h.github.writes()
    );
    assert_eq!(
        f.h.github.actors_of("create_issue"),
        ["bob"],
        "the issue and its labels are the creator's"
    );
    Ok(())
}

/// Reconciliation: GitHub outages never change the recorded stage; a pull request merged
/// outside Yulius moves the task forward on read; a promotion merged on GitHub is recorded by
/// the next promotion request; a broken clone degrades the Git page to notes.
#[tokio::test]
async fn reads_reconcile_with_github_and_degrade_gracefully() -> TestResult {
    let f = fixture(&DeliveryProfile::standard()).await?;
    let wt = f
        .add_task("t1", "Checkout API", "feat/1-checkout-api", "dev")
        .await?;
    commit_file(&wt, "feature.txt", "hello", "Add feature")?;
    commit_file(&wt, SPEC_PATH, &valid_spec("t1"), "Spec")?;
    let opened = f
        .req(Method::POST, "/api/tasks/t1/delivery/pr", None, &f.bob)
        .await?;
    assert_eq!(opened.status, StatusCode::OK, "{}", opened.body);

    // Merged on GitHub directly, outside Yulius.
    let head = git(&wt, &["rev-parse", "HEAD"])?;
    yulius_app::ports::github::GitHubPort::merge_pull(
        f.h.github.as_ref(),
        &Secret("token-carol".into()),
        "Quartio-Studio",
        "shop",
        1,
        "squash",
        &head,
    )
    .await?;

    // GitHub unreachable (expired user token, broken App credentials): the page answers with
    // a note and the stage stays as recorded.
    let crypto = &f.h.app.deps.crypto;
    let expired = |access: &str| -> BoxResult<GitHubTokenRecord> {
        Ok(GitHubTokenRecord {
            user_id: UserId::parse("u-bob")?,
            access: crypto.seal(access.as_bytes())?,
            refresh: None,
            expires_at: Some(Timestamp(1)),
            refresh_expires_at: None,
        })
    };
    f.h.store
        .save_github_token(&expired("token-bob")?, Timestamp(2))
        .await?;
    f.h.store
        .set_github_app(&Sealed(vec![1, 2, 3]), "Quartio-Studio", "org")
        .await?;
    let d = f.delivery("t1").await?;
    assert_eq!(d["stage"], "pr_open", "{d}");
    assert!(
        d["notes"].to_string().contains("GitHub is not available"),
        "{d}"
    );
    let app_cfg = json!({"app_id": 1, "slug": "yulius-test", "client_id": "c", "client_secret": "s", "private_key_pem": "k", "webhook_secret": null});
    f.h.store
        .set_github_app(
            &crypto.seal(app_cfg.to_string().as_bytes())?,
            "Quartio-Studio",
            "org",
        )
        .await?;
    f.h.store
        .save_github_token(
            &GitHubTokenRecord {
                expires_at: None,
                ..expired("token-bob")?
            },
            Timestamp(3),
        )
        .await?;

    let d = f.delivery("t1").await?;
    assert_eq!(d["stage"], "merged_integration", "{d}");
    assert_eq!(actions(&d), ["confirm_env:dev"]);

    // The promotion pull request is merged on GitHub; the next promote records it.
    let confirmed = f
        .req(
            Method::POST,
            "/api/sandboxes/s1/delivery/environments/dev/confirm",
            Some(json!({"confirm": true})),
            &f.bob,
        )
        .await?;
    assert_eq!(confirmed.status, StatusCode::OK, "{}", confirmed.body);
    let opened = f
        .req(
            Method::POST,
            "/api/sandboxes/s1/delivery/promote",
            None,
            &f.bob,
        )
        .await?;
    assert_eq!(opened.body["result"], "pr_opened", "{}", opened.body);
    let head = f.h.github.pull(2).ok_or("pull 2")?.head_sha;
    yulius_app::ports::github::GitHubPort::merge_pull(
        f.h.github.as_ref(),
        &Secret("token-carol".into()),
        "Quartio-Studio",
        "shop",
        2,
        "merge",
        &head,
    )
    .await?;
    let recorded = f
        .req(
            Method::POST,
            "/api/sandboxes/s1/delivery/promote",
            None,
            &f.bob,
        )
        .await?;
    assert_eq!(recorded.status, StatusCode::OK, "{}", recorded.body);
    assert_eq!(recorded.body["result"], "promoted");
    assert_eq!(recorded.body["sha"], f.origin_sha("main")?.as_str());
    assert_eq!(f.delivery("t1").await?["stage"], "production_done");
    let nothing = f
        .req(
            Method::POST,
            "/api/sandboxes/s1/delivery/promote",
            None,
            &f.bob,
        )
        .await?;
    assert_eq!(nothing.status, StatusCode::CONFLICT);
    assert!(error_message(&nothing).contains("nothing to promote"));

    // The sandbox clone is missing: the Git page still answers, with notes.
    std::fs::remove_dir_all(&f.repo)?;
    let page = f
        .req(Method::GET, "/api/sandboxes/s1/git", None, &f.rita)
        .await?;
    assert_eq!(page.status, StatusCode::OK, "{}", page.body);
    assert_eq!(page.body["recent_commits"], json!([]));
    assert!(
        page.body["notes"].to_string().contains("not cloned"),
        "{}",
        page.body
    );
    Ok(())
}

/// #35: a task ready for delivery opens its pull request by itself — with its creator's GitHub
/// account, after the branch is pushed — while open steps or uncommitted work keep it waiting;
/// a merge on GitHub then moves the stage in the background without anyone reading the task.
#[tokio::test]
async fn a_ready_task_opens_its_pull_request_by_itself() -> TestResult {
    let f = fixture(&DeliveryProfile::standard()).await?;
    let wt = f
        .add_task("t1", "Checkout API", "feat/1-checkout-api", "dev")
        .await?;
    commit_file(&wt, "feature.txt", "hello", "Add feature")?;
    commit_file(&wt, SPEC_PATH, &valid_spec("t1"), "Spec")?;
    let t1 = TaskId::parse("t1")?;
    let pulls_opened = || {
        f.h.github
            .writes()
            .iter()
            .filter(|w| w.starts_with("create_pull"))
            .count()
    };

    set_steps(&f, "t1", &["done", "in_progress"]).await?;
    f.h.app.auto_open_pr(&t1).await;
    assert_eq!(pulls_opened(), 0, "a step is still open");

    set_steps(&f, "t1", &["done", "done"]).await?;
    std::fs::write(wt.join("scratch.txt"), "work in progress")?;
    f.h.app.auto_open_pr(&t1).await;
    assert_eq!(pulls_opened(), 0, "uncommitted work");
    std::fs::remove_file(wt.join("scratch.txt"))?;

    f.h.app.auto_open_pr(&t1).await;
    assert_eq!(pulls_opened(), 1, "{:?}", f.h.github.writes());
    let d = f.delivery("t1").await?;
    assert_eq!(d["stage"], "pr_open");
    assert_eq!(d["pr"]["base"], "dev");
    assert_eq!(d["pr"]["head"], "feat/1-checkout-api");
    let head = git(&wt, &["rev-parse", "HEAD"])?;
    assert_eq!(f.origin_sha("feat/1-checkout-api")?, head, "pushed first");
    // F 2026-10-11 (AC-GIT-3): the task's creator pushes and opens it, never the App.
    assert_eq!(
        f.pushes(),
        ["feat/1-checkout-api->feat/1-checkout-api token=token-bob"]
    );
    assert_eq!(f.h.github.actors_of("create_pull"), ["bob"]);
    // F 2026-10-11: the agent's turn ended, its work did not; Yulius tells it to deliver.
    let told = f.h.store.inbox(&t1, true, 10).await?;
    assert!(
        told.iter().any(
            |m| m.message.subject == "Your pull request is open: deliver it"
                && m.message.kind == "request"
                && m.message.body.contains("delivery_merge")
                && m.message.body.contains("without asking anyone")
        ),
        "{told:?}"
    );
    f.h.app.auto_open_pr(&t1).await;
    assert_eq!(pulls_opened(), 1, "one pull request per task");

    // #68: commits made after the pull request opened are pushed to it at the next clean Stop.
    commit_file(&wt, "follow-up.txt", "more", "Follow-up")?;
    let follow_up = git(&wt, &["rev-parse", "HEAD"])?;
    f.h.app.auto_open_pr(&t1).await;
    assert_eq!(
        f.origin_sha("feat/1-checkout-api")?,
        follow_up,
        "the open pull request gets the new commits"
    );
    assert_eq!(pulls_opened(), 1, "no second pull request");
    assert_eq!(
        f.pushes().last().map(String::as_str),
        Some("feat/1-checkout-api->feat/1-checkout-api token=token-bob"),
        "the creator pushes the follow-up too"
    );

    // Merged on GitHub: the background round follows it; nobody read the task meanwhile.
    yulius_app::ports::github::GitHubPort::merge_pull(
        f.h.github.as_ref(),
        &Secret("token-carol".into()),
        "Quartio-Studio",
        "shop",
        1,
        "squash",
        &follow_up,
    )
    .await?;
    f.h.app.reconcile_deliveries().await;
    let record = f.h.store.delivery(&t1).await?.ok_or("delivery row")?;
    assert_eq!(record.stage, DeliveryStage::MergedIntegration);
    // #68: the merged pull request stays on its task, so the task stays "Done · merged".
    let page = f
        .req(Method::GET, "/api/sandboxes/s1/git", None, &f.rita)
        .await?;
    let row = &page.body["tasks"][0];
    assert_eq!(row["pr"]["number"], 1, "{}", page.body);
    assert_eq!(row["pr"]["state"], "merged");
    Ok(())
}

/// DSC-013 (#139): features and fixes need a spec, documentation does not. A task without a
/// workflow that only changed documents opens its pull request with no spec and no plan; once it
/// changes code, it needs the spec again.
#[tokio::test]
async fn a_docs_only_task_needs_no_spec_to_deliver() -> TestResult {
    let f = fixture(&DeliveryProfile::standard()).await?;
    let wt = f
        .add_task("t1", "Fix the README", "docs/1-readme", "dev")
        .await?;
    let t1 = TaskId::parse("t1")?;
    let pulls_opened = || {
        f.h.github
            .writes()
            .iter()
            .filter(|w| w.starts_with("create_pull"))
            .count()
    };
    f.h.app.auto_open_pr(&t1).await;
    assert_eq!(pulls_opened(), 0, "nothing changed yet");

    commit_file(
        &wt,
        "docs/setup.md",
        "# Setup\n",
        "docs: describe the setup",
    )?;
    let task = f.h.store.task(&t1).await?.ok_or("task")?;
    assert!(!f.h.app.spec_required(&task).await?, "documentation only");
    f.h.app.auto_open_pr(&t1).await;
    assert_eq!(pulls_opened(), 1, "{:?}", f.h.github.writes());

    let t2_wt = f
        .add_task("t2", "Checkout API", "feat/2-checkout-api", "dev")
        .await?;
    commit_file(&t2_wt, "docs/checkout.md", "# Checkout\n", "docs")?;
    commit_file(
        &t2_wt,
        "src/checkout.rs",
        "fn main() {}\n",
        "feat: checkout",
    )?;
    let t2 = TaskId::parse("t2")?;
    let task = f.h.store.task(&t2).await?.ok_or("task")?;
    assert!(f.h.app.spec_required(&task).await?, "code changed");
    f.h.app.auto_open_pr(&t2).await;
    assert_eq!(pulls_opened(), 1, "a code change without a spec waits");
    Ok(())
}

/// #28 (UI-031): the task review reads the worktree's committed, uncommitted and untracked
/// changes against the merge base with `origin/dev`, and the branch's commits, with read access;
/// a symbolic link is listed but never followed.
#[tokio::test]
async fn task_review_shows_changes_and_commits() -> TestResult {
    let f = fixture(&DeliveryProfile::standard()).await?;
    let wt = f
        .add_task("t1", "Checkout API", "feat/1-checkout-api", "dev")
        .await?;
    commit_file(&wt, "feature.txt", "hello\nworld\n", "Add feature")?;
    std::fs::write(wt.join("feature.txt"), "hello\nthere\nworld\n")?;
    std::fs::write(wt.join("notes.md"), "draft\n")?;
    std::os::unix::fs::symlink("/etc/passwd", wt.join("outside"))?;

    let changes = f
        .req(Method::GET, "/api/tasks/t1/changes", None, &f.rita)
        .await?;
    assert_eq!(changes.status, StatusCode::OK, "{}", changes.body);
    let c = &changes.body;
    assert_eq!(c["base"], "dev");
    assert_eq!(c["head"], "feat/1-checkout-api");
    assert_eq!(c["ahead"], 1);
    assert_eq!(c["behind"], 0);
    assert_eq!(c["uncommitted"], 3, "{c}");
    assert_eq!(c["truncated"], false);
    let files = c["files"].as_array().ok_or("files")?;
    let file = |path: &str| files.iter().find(|f| f["path"] == path).cloned();
    let feature = file("feature.txt").ok_or("feature.txt")?;
    assert_eq!(feature["status"], "added", "new since the merge base");
    let added: Vec<&str> = feature["hunks"][0]["lines"]
        .as_array()
        .ok_or("lines")?
        .iter()
        .filter(|l| l["kind"] == "add")
        .filter_map(|l| l["text"].as_str())
        .collect();
    assert_eq!(
        added,
        ["hello", "there", "world"],
        "uncommitted work included"
    );
    let notes = file("notes.md").ok_or("notes.md")?;
    assert_eq!(notes["status"], "added");
    assert_eq!(notes["hunks"][0]["lines"][0]["text"], "draft");
    let link = file("outside").ok_or("outside")?;
    assert!(
        link["hunks"].as_array().is_some_and(Vec::is_empty),
        "a symlink is not followed"
    );
    assert!(
        !changes.body.to_string().contains("root:"),
        "nothing outside the worktree"
    );

    let commits = f
        .req(Method::GET, "/api/tasks/t1/commits", None, &f.rita)
        .await?;
    assert_eq!(commits.status, StatusCode::OK, "{}", commits.body);
    let list = commits.body.as_array().ok_or("commits")?;
    assert_eq!(list.len(), 1);
    assert_eq!(list[0]["subject"], "Add feature");
    assert_eq!(list[0]["files"], 1);
    assert_eq!(list[0]["insertions"], 2);
    assert!(list[0]["at"].as_str().is_some_and(|t| t.ends_with('Z')));

    let anonymous = f
        .req(
            Method::GET,
            "/api/tasks/t1/changes",
            None,
            &Creds::default(),
        )
        .await?;
    assert_eq!(anonymous.status, StatusCode::UNAUTHORIZED);
    Ok(())
}

/// #111 (`F 2026-10-10`): the GitHub App may not read every check source. What it can read
/// counts; checks that can't be read never count as passed; the missing permission is said once,
/// in plain words, with the App's permission page for instance administrators only; GitHub
/// errors are not prefixed "GitHub:" twice.
#[tokio::test]
async fn unreadable_check_sources_are_said_once_and_never_count_as_passed() -> TestResult {
    let f = fixture(&DeliveryProfile::standard()).await?;
    let owner = superadmin(&f.h).await?;
    let wt = f
        .add_task("t1", "Checkout API", "feat/1-checkout-api", "dev")
        .await?;
    commit_file(&wt, "feature.txt", "hello", "Add feature")?;
    commit_file(&wt, SPEC_PATH, &valid_spec("t1"), "Spec")?;
    let opened = f
        .req(Method::POST, "/api/tasks/t1/delivery/pr", None, &f.bob)
        .await?;
    assert_eq!(opened.status, StatusCode::OK, "{}", opened.body);
    let head = git(&wt, &["rev-parse", "HEAD"])?;

    // The installation of #111: Checks but no Commit statuses. The check runs decide.
    f.h.github.set_checks_report(
        &head,
        ChecksReport {
            state: ChecksState::Success,
            unreadable: vec![ChecksSource::CommitStatuses],
        },
    );
    let expected = "Yulius can't read commit statuses for Quartio-Studio/shop: the GitHub App \
                    needs the 'Commit statuses: read' permission. Results come from check runs only.";
    let page = f
        .req(Method::GET, "/api/sandboxes/s1/git", None, &f.rita)
        .await?;
    assert_eq!(page.status, StatusCode::OK, "{}", page.body);
    assert_eq!(page.body["pulls"][0]["checks"], "success");
    assert_eq!(page.body["notes"], json!([]), "no warning note");
    let access = &page.body["checks_access"];
    assert_eq!(access["repo"], "Quartio-Studio/shop");
    assert_eq!(access["missing"], json!(["Commit statuses"]));
    assert_eq!(access["read_from"], json!(["check runs"]));
    assert_eq!(access["message"], expected);
    assert_eq!(access["settings_url"], Value::Null, "not an administrator");
    let page = f
        .req(Method::GET, "/api/sandboxes/s1/git", None, &owner)
        .await?;
    assert_eq!(
        page.body["checks_access"]["settings_url"],
        "https://github.com/organizations/Quartio-Studio/settings/apps/yulius-test/permissions"
    );
    let d = f.delivery("t1").await?;
    assert_eq!(d["pr"]["checks"], "success");
    assert_eq!(d["checks_access"]["message"], expected);
    assert_eq!(actions(&d), ["merge"]);

    // Every source read: nothing to say.
    f.h.github.set_checks(&head, ChecksState::Success);
    let page = f
        .req(Method::GET, "/api/sandboxes/s1/git", None, &owner)
        .await?;
    assert_eq!(page.body["checks_access"], Value::Null);

    // Nothing readable: unknown, never green. The merge waits and says why.
    f.h.github.set_checks_report(
        &head,
        ChecksReport {
            state: ChecksState::Unknown,
            unreadable: vec![
                ChecksSource::CheckRuns,
                ChecksSource::CommitStatuses,
                ChecksSource::WorkflowRuns,
            ],
        },
    );
    let d = f.delivery("t1").await?;
    assert_eq!(d["pr"]["checks"], "unknown");
    assert!(actions(&d).is_empty(), "{d}");
    assert_eq!(d["blocked"][0]["action"], "merge");
    assert!(
        d["blocked"][0]["reason"]
            .as_str()
            .is_some_and(|r| r.contains("can't be read") && r.contains("don't count as passed")),
        "{d}"
    );
    assert_eq!(
        d["checks_access"]["missing"],
        json!(["Checks", "Commit statuses"])
    );
    assert_eq!(d["checks_access"]["read_from"], json!([]));
    assert_eq!(
        d["checks_access"]["message"],
        "Yulius can't read check results for Quartio-Studio/shop: the GitHub App needs the \
         'Checks: read' and 'Commit statuses: read' permissions. Checks show as unknown, and \
         merges that require them wait."
    );
    let refused = f
        .req(Method::POST, "/api/tasks/t1/delivery/merge", None, &f.bob)
        .await?;
    assert_eq!(refused.status, StatusCode::CONFLICT);
    assert!(
        error_message(&refused).contains("can't be read"),
        "{}",
        refused.body
    );
    assert!(
        !f.h.github
            .writes()
            .iter()
            .any(|w| w.starts_with("merge_pull")),
        "nothing merged"
    );

    // Any other GitHub failure: unknown too (not "running"), one note naming GitHub once.
    if let Ok(mut slot) = f.h.github.checks_error.lock() {
        *slot = Some(AppError::Upstream(
            "GitHub: GitHub refused the request (HTTP 403): Resource protected by organization \
             SAML enforcement."
                .into(),
        ));
    }
    let d = f.delivery("t1").await?;
    assert_eq!(d["pr"]["checks"], "unknown");
    assert_eq!(
        d["notes"],
        json!([
            "Checks can't be read: GitHub refused the request (HTTP 403): Resource protected by \
             organization SAML enforcement."
        ])
    );
    assert_eq!(d["checks_access"], Value::Null);
    let page = f
        .req(Method::GET, "/api/sandboxes/s1/git", None, &f.rita)
        .await?;
    assert_eq!(page.body["pulls"][0]["checks"], "unknown");
    let notes = page.body["notes"].to_string();
    assert!(
        notes.contains("Checks can't be read: GitHub refused"),
        "{notes}"
    );
    assert!(!notes.contains("GitHub: GitHub"), "{notes}");
    Ok(())
}

/// The password superadmin: no GitHub identity, admin on every sandbox (ACC-012).
async fn superadmin(h: &support::Harness) -> BoxResult<Creds> {
    let user = UserRecord {
        id: UserId::parse("u-root")?,
        github_login: None,
        github_id: None,
        display_name: "Superadmin".into(),
        avatar_url: None,
        is_superadmin: true,
        is_instance_admin: true,
        password_hash: Some("$argon2id$v=19$m=19456,t=2,p=1$synthetic$synthetic".into()),
        created_at: Timestamp(1),
        disabled_at: None,
    };
    h.store.insert_user(&user).await?;
    let token = h.app.issue_api_token(&user, "test").await?;
    Ok(Creds {
        bearer: Some(token.expose().to_owned()),
        ..Creds::default()
    })
}

/// ACC-012 (F 2026-10-09): the superadmin without a GitHub identity reads a task's delivery and
/// opens its pull request through the App, as Yulius does for a ready task; merging and
/// releasing act as a person on GitHub, so those two actions are refused with a message saying
/// so (403) — never as a signed-out session (401), and the rest of the page keeps working.
#[tokio::test]
async fn the_superadmin_delivers_through_the_app_and_is_told_what_needs_github() -> TestResult {
    let f = fixture(&DeliveryProfile::standard()).await?;
    let owner = superadmin(&f.h).await?;
    let wt = f
        .add_task("t1", "Checkout API", "feat/1-checkout-api", "dev")
        .await?;
    commit_file(&wt, "feature.txt", "hello", "Add feature")?;
    commit_file(&wt, SPEC_PATH, &valid_spec("t1"), "Spec")?;

    let d = f
        .req(Method::GET, "/api/tasks/t1/delivery", None, &owner)
        .await?;
    assert_eq!(d.status, StatusCode::OK, "{}", d.body);
    assert_eq!(actions(&d.body), ["open_pr"]);
    let opened = f
        .req(Method::POST, "/api/tasks/t1/delivery/pr", None, &owner)
        .await?;
    assert_eq!(opened.status, StatusCode::OK, "{}", opened.body);
    assert_eq!(opened.body["stage"], "pr_open");
    assert_eq!(
        f.h.github.pull_author(1).as_deref(),
        Some("inst"),
        "the App installation opened the pull request"
    );
    let head = git(&wt, &["rev-parse", "HEAD"])?;
    assert_eq!(f.origin_sha("feat/1-checkout-api")?, head);
    f.h.github.set_checks(&head, ChecksState::Success);

    let merge = f
        .req(Method::POST, "/api/tasks/t1/delivery/merge", None, &owner)
        .await?;
    assert_eq!(merge.status, StatusCode::FORBIDDEN, "{}", merge.body);
    assert!(
        error_message(&merge).contains("Merging a pull request acts as a person on GitHub"),
        "{}",
        merge.body
    );
    let release = f
        .req(
            Method::POST,
            "/api/sandboxes/s1/delivery/promote",
            Some(json!({})),
            &owner,
        )
        .await?;
    assert_eq!(release.status, StatusCode::FORBIDDEN, "{}", release.body);
    assert!(
        error_message(&release).contains("Releasing to main acts as a person on GitHub"),
        "{}",
        release.body
    );
    assert!(
        !f.h.github
            .writes()
            .iter()
            .any(|w| w.starts_with("merge_pull")),
        "nothing was merged"
    );

    // Still signed in, and the Git page answers.
    let page = f
        .req(Method::GET, "/api/sandboxes/s1/git", None, &owner)
        .await?;
    assert_eq!(page.status, StatusCode::OK, "{}", page.body);
    assert_eq!(page.body["tasks"][0]["pr"]["number"], 1);

    // A GitHub user merges the App's pull request as before.
    let merged = f
        .req(Method::POST, "/api/tasks/t1/delivery/merge", None, &f.bob)
        .await?;
    assert_eq!(merged.status, StatusCode::OK, "{}", merged.body);
    assert_eq!(merged.body["stage"], "merged_integration");
    Ok(())
}

/// F 2026-10-11 (AC-GIT-3): a task whose creator has no usable GitHub token — here the
/// superadmin, with no GitHub identity — still delivers: Yulius pushes and opens its pull request
/// as the App, and its agent merges as the App instead of being refused; the task's terminal
/// says once why the App appears.
#[tokio::test]
async fn a_task_without_a_github_creator_delivers_as_the_app() -> TestResult {
    let f = fixture(&DeliveryProfile::standard()).await?;
    superadmin(&f.h).await?;
    let wt = f
        .add_task_by("t1", "Checkout API", "feat/1-checkout-api", "dev", "u-root")
        .await?;
    commit_file(&wt, "feature.txt", "hello", "Add feature")?;
    commit_file(&wt, SPEC_PATH, &valid_spec("t1"), "Spec")?;
    set_steps(&f, "t1", &["done", "done"]).await?;
    let t1 = TaskId::parse("t1")?;
    let mut watch = f.h.app.runtime(&t1).hub.watch();

    f.h.app.auto_open_pr(&t1).await;
    assert_eq!(f.delivery("t1").await?["stage"], "pr_open");
    assert_eq!(
        f.pushes(),
        ["feat/1-checkout-api->feat/1-checkout-api token=inst"]
    );
    assert_eq!(f.h.github.actors_of("create_pull"), ["inst"]);

    // Its agent merges on green, as the App.
    let head = git(&wt, &["rev-parse", "HEAD"])?;
    f.h.github.set_checks(&head, ChecksState::Success);
    f.h.store
        .insert_attempt(&AttemptRecord {
            id: AttemptId::parse("a1")?,
            task_id: t1.clone(),
            seq: 1,
            harness: Harness::ClaudeCode,
            account_id: None,
            provider_session_id: None,
            launch_kind: LaunchKind::New,
            state: AttemptState::Live,
            run_dir: "/tmp/none".into(),
            token_hash: f.h.app.deps.crypto.sha256_hex(b"tok-agent"),
            host_token: Sealed(vec![0]),
            ptyhost_pid: None,
            child_pid: None,
            started_at: Timestamp(1),
            ended_at: None,
            exit_code: None,
            exit_reason: None,
        })
        .await?;
    let internal = yulius_server::internal_router(f.h.app.clone())
        .layer(MockConnectInfo(SocketAddr::from(([127, 0, 0, 1], 1))));
    let merged = call(
        &internal,
        Method::POST,
        "/mcp",
        Some(
            json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {
                "name": "delivery_merge",
                "arguments": {"reviewed_head": head, "review_summary": "approve"}
            }}),
        ),
        &Creds {
            bearer: Some("tok-agent".into()),
            ..Creds::default()
        },
    )
    .await?;
    assert_eq!(merged.body["result"]["isError"], false, "{}", merged.body);
    assert_eq!(f.h.github.actors_of("merge_pull"), ["inst"]);
    assert_eq!(f.delivery("t1").await?["stage"], "merged_integration");

    let mut said = Vec::new();
    while let Ok(frame) = watch.try_recv() {
        if let yulius_app::hub::HubFrame::Notice(text) = frame
            && text.contains("GitHub App")
        {
            said.push(text);
        }
    }
    assert_eq!(said.len(), 1, "said once: {said:?}");
    assert!(said[0].contains("no linked GitHub account"), "{said:?}");
    Ok(())
}

/// #147: Home's cards and the sandbox tabs count "needs you" with the sidebar's rule (`taskInfo`
/// in the web UI): Your turn, held prompts, blocked chains and failing checks count; a paused or
/// ended task does not; a tested pull request answers for a finished task once its plan is
/// complete (#139). The pull request facts are what the Git overview last read.
#[tokio::test]
async fn needs_you_counts_follow_the_sidebar_rule() -> TestResult {
    use yulius_app::intake::{ChainRecord, HoldRecord, hold_state};
    use yulius_app::ports::store_ext::IntakeStore;
    use yulius_domain::intake::PromptClass;
    use yulius_domain::task::Activity;

    let f = fixture(&DeliveryProfile::standard()).await?;
    let now = Timestamp(5);
    let set = async |id: &str, status: TaskStatus, activity: Option<Activity>| -> BoxResult<()> {
        f.h.store
            .transition_task(&TaskId::parse(id)?, None, status, activity, now)
            .await?;
        Ok(())
    };
    let hold = |id: &str| -> BoxResult<HoldRecord> {
        Ok(HoldRecord {
            id: format!("hold-{id}"),
            task_id: TaskId::parse(id)?,
            class: PromptClass::NewScope,
            prompt: "Also add dark mode.".into(),
            prompt_sha256: "0".repeat(64),
            percent: 90,
            typed_by: None,
            state: hold_state::PENDING.into(),
            created_at: now,
            resolved_by: None,
            resolved_at: None,
            new_task_id: None,
        })
    };
    let needs = async || -> BoxResult<u64> {
        let r = f.req(Method::GET, "/api/sandboxes", None, &f.bob).await?;
        assert_eq!(r.status, StatusCode::OK, "{}", r.body);
        let sandbox = r
            .body
            .as_array()
            .and_then(|list| list.iter().find(|s| s["id"] == "s1"))
            .ok_or("s1")?;
        sandbox["counts"]["needs_attention"]
            .as_u64()
            .ok_or_else(|| format!("{sandbox}").into())
    };

    // t1 finished its turn with an open pull request and a complete plan.
    let wt = f
        .add_task("t1", "Checkout API", "feat/1-checkout-api", "dev")
        .await?;
    commit_file(&wt, "feature.txt", "hello", "Add feature")?;
    commit_file(&wt, SPEC_PATH, &valid_spec("t1"), "Spec")?;
    let opened = f
        .req(Method::POST, "/api/tasks/t1/delivery/pr", None, &f.bob)
        .await?;
    assert_eq!(opened.status, StatusCode::OK, "{}", opened.body);
    let head = git(&wt, &["rev-parse", "HEAD"])?;
    set_steps(&f, "t1", &["done", "done"]).await?;
    set("t1", TaskStatus::Running, Some(Activity::Waiting)).await?;
    // Your turn; a held prompt; a chain whose predecessor ended otherwise.
    f.add_task("t2", "Your turn", "feat/2-your-turn", "dev")
        .await?;
    set("t2", TaskStatus::Running, Some(Activity::Waiting)).await?;
    f.add_task("t3", "Held prompt", "feat/3-held", "dev")
        .await?;
    set("t3", TaskStatus::Running, Some(Activity::Working)).await?;
    f.h.store.insert_hold(&hold("t3")?).await?;
    f.add_task("t4", "Blocked chain", "feat/4-chain", "dev")
        .await?;
    set("t4", TaskStatus::Chained, None).await?;
    f.h.store
        .insert_chain(&ChainRecord {
            task_id: TaskId::parse("t4")?,
            after_task_id: Some(TaskId::parse("t2")?),
            after_number: 2,
            blocked_reason: Some("#2 stopped".into()),
            created_at: now,
        })
        .await?;
    // Nobody needs to act: paused for capacity, or ended (even with a held prompt).
    f.add_task("t5", "Paused", "feat/5-paused", "dev").await?;
    set(
        "t5",
        TaskStatus::WaitingCapacity,
        Some(Activity::NeedsAttention),
    )
    .await?;
    f.add_task("t6", "Closed", "feat/6-closed", "dev").await?;
    f.h.store.insert_hold(&hold("t6")?).await?;

    // Before any Git overview read, t1's pull request is not known: Your turn.
    assert_eq!(needs().await?, 4);

    // Green checks, read by the Git overview: t1 is Done.
    f.h.github.set_checks(&head, ChecksState::Success);
    let page = f
        .req(Method::GET, "/api/sandboxes/s1/git", None, &f.rita)
        .await?;
    assert_eq!(page.status, StatusCode::OK, "{}", page.body);
    assert_eq!(needs().await?, 3);

    // A step opened again keeps it out of Done: its own state (Your turn) shows.
    set_steps(&f, "t1", &["done", "pending"]).await?;
    assert_eq!(needs().await?, 4);

    // Failing checks need a person whatever the plan says.
    set_steps(&f, "t1", &["done", "done"]).await?;
    f.h.github.set_checks(&head, ChecksState::Failure);
    f.req(Method::GET, "/api/sandboxes/s1/git", None, &f.rita)
        .await?;
    assert_eq!(needs().await?, 4);

    // Working again: the pull request no longer speaks for it.
    set("t1", TaskStatus::Running, Some(Activity::Working)).await?;
    assert_eq!(needs().await?, 3);
    Ok(())
}
