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
    AttemptRecord, GitHubTokenRecord, LaunchKind, SandboxRecord, Sealed, Secret, TaskOrigin,
    TaskRecord, UserRecord,
};
use yulius_app::ports::git::{CommitInfo, DiffStat, GitPort};
use yulius_app::ports::github::ChecksState;
use yulius_app::ports::store::{InstanceStore, SandboxStore, TaskStore, UserStore};
use yulius_domain::delivery::DeliveryProfile;
use yulius_domain::harness::Harness;
use yulius_domain::ids::{AttemptId, SandboxId, TaskId, UserId};
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
/// repository; tokens and URLs are ignored).
struct RealGit {
    origin: PathBuf,
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
        _token: &Secret,
    ) -> AppResult<()> {
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
    let h = harness_with_git(Arc::new(RealGit {
        origin: origin.clone(),
    }))?;
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
        _root: root,
        origin,
        repo,
        alice,
        bob,
        rita,
    })
}

impl Fixture {
    /// A task with a worktree on its own branch from the integration branch.
    async fn add_task(
        &self,
        id: &str,
        title: &str,
        branch: &str,
        base: &str,
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
                created_by: UserId::parse("u-bob")?,
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
    assert_eq!(task["pr"]["checks"], "none");
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
    assert_eq!(
        f.h.github.writes().len(),
        writes_before + 1,
        "only the merge happened"
    );
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

/// AC-GIT-2: reads, agents and read-only users never merge, promote or deploy — even with an
/// auto-promoting profile and green checks, only a person's merge request moves anything.
#[tokio::test]
async fn no_api_path_merges_or_promotes_without_a_person() -> TestResult {
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
        .set_checks(&git(&wt, &["rev-parse", "HEAD"])?, ChecksState::Success);
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
    assert!(names.contains(&"delivery_status".to_owned()));
    for name in &names {
        for forbidden in ["merge", "promote", "deploy", "environment", "run_env"] {
            assert!(
                !name.contains(forbidden),
                "agents get no {forbidden} tool ({name})"
            );
        }
    }
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
    let attempt = rpc(
        "tools/call",
        json!({"name": "delivery_merge", "arguments": {}}),
    )
    .await?;
    assert_eq!(attempt.body["result"]["isError"], true);
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

    // Only the person's merge request merges (and then the explicit profile policy promotes).
    let merged = f
        .req(Method::POST, "/api/tasks/t1/delivery/merge", None, &f.bob)
        .await?;
    assert_eq!(merged.status, StatusCode::OK, "{}", merged.body);
    assert_eq!(f.origin_sha("main")?, f.origin_sha("quartio")?);
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
