//! Repository fixtures for project and import tests: a `GitPort` over the real `git` CLI with
//! local bare repositories standing in for GitHub remotes, a GitHub fake extension for
//! installation repositories, repository creation and rulesets, and a harness built on both.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicI64, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use axum::body::Body;
use axum::extract::connect_info::MockConnectInfo;
use axum::http::{Method, Request, header};
use http_body_util::BodyExt;
use tower::ServiceExt;
use yulius_app::context::{App, AppConfig, Deps};
use yulius_app::error::{AppError, AppResult};
use yulius_app::model::{GitHubTokenRecord, Secret, UserRecord};
use yulius_app::ports::git::{CommitInfo, DiffStat, GitPort};
use yulius_app::ports::github::{
    ChecksState, GhPull, GhRepo, GhUser, GhUserToken, GitHubAppConfig, GitHubPort, NewPull,
    RepoAccess, RulesetRequest,
};
use yulius_app::ports::store::{InstanceStore, UserStore};
use yulius_domain::ids::UserId;
use yulius_domain::permission::GitHubPermissions;
use yulius_domain::time::Timestamp;
use yulius_store::SqliteStore;
use yulius_store::crypto::MasterKeyCrypto;

use super::{Creds, FakeGitHub, Harness, Resp, TestClock};

type BoxError = Box<dyn std::error::Error>;

/// Runs `git` with an isolated configuration; returns stdout.
pub fn git(dir: &Path, args: &[&str]) -> Result<String, String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args([
            "-c",
            "user.name=Fixture",
            "-c",
            "user.email=fixture@example.test",
            "-c",
            "init.defaultBranch=main",
            "-c",
            "commit.gpgsign=false",
            "-c",
            "core.hooksPath=/dev/null",
        ])
        .args(args)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
        .map_err(|err| err.to_string())?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    } else {
        Err(format!(
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        ))
    }
}

fn upstream(err: String) -> AppError {
    AppError::Upstream(err)
}

/// `GitPort` over the real `git` CLI. Clone URLs are mapped to local bare repositories.
#[derive(Default)]
pub struct LocalGit {
    /// Clone URL → bare repository.
    pub remotes: Mutex<HashMap<String, PathBuf>>,
    /// Operation log (`clone <url> token=<t>`, `push <local>-><remote> token=<t>`, …).
    pub calls: Mutex<Vec<String>>,
}

impl LocalGit {
    /// Maps a clone URL to a bare repository.
    pub fn add_remote(&self, url: &str, path: &Path) {
        if let Ok(mut remotes) = self.remotes.lock() {
            remotes.insert(url.to_owned(), path.to_path_buf());
        }
    }

    fn record(&self, entry: String) {
        if let Ok(mut calls) = self.calls.lock() {
            calls.push(entry);
        }
    }

    /// Logged operations.
    pub fn calls(&self) -> Vec<String> {
        self.calls.lock().map(|c| c.clone()).unwrap_or_default()
    }

    fn remote(&self, url: &str) -> AppResult<PathBuf> {
        self.remotes
            .lock()
            .map_err(AppError::internal)?
            .get(url)
            .cloned()
            .ok_or_else(|| AppError::Upstream(format!("unknown remote {url}")))
    }

    fn local_branch(repo: &Path, branch: &str) -> bool {
        git(
            repo,
            &[
                "rev-parse",
                "--verify",
                "--quiet",
                &format!("refs/heads/{branch}"),
            ],
        )
        .is_ok()
    }
}

#[async_trait]
impl GitPort for LocalGit {
    fn valid_branch(&self, name: &str) -> bool {
        !name.starts_with('-')
            && Command::new("git")
                .args(["check-ref-format", "--branch", name])
                // Outside any repository: `--branch` must not depend on the test's cwd.
                .current_dir(std::env::temp_dir())
                .output()
                .is_ok_and(|o| o.status.success())
    }

    async fn sync_clone(&self, url: &str, dest: &Path, token: &Secret) -> AppResult<()> {
        let remote = self.remote(url)?;
        self.record(format!("clone {url} token={}", token.expose()));
        if dest.join(".git").exists() {
            git(
                dest,
                &[
                    "fetch",
                    "--prune",
                    "origin",
                    "+refs/heads/*:refs/remotes/origin/*",
                ],
            )
            .map_err(upstream)?;
            return Ok(());
        }
        let parent = dest.parent().ok_or(AppError::NotFound)?;
        std::fs::create_dir_all(parent).map_err(AppError::internal)?;
        git(
            parent,
            &[
                "clone",
                "--quiet",
                &remote.to_string_lossy(),
                &dest.to_string_lossy(),
            ],
        )
        .map_err(upstream)?;
        Ok(())
    }

    async fn default_branch(&self, repo: &Path) -> AppResult<String> {
        let head = git(
            repo,
            &["symbolic-ref", "--short", "refs/remotes/origin/HEAD"],
        )
        .map_err(upstream)?;
        Ok(head.trim().trim_start_matches("origin/").to_owned())
    }

    async fn branch_exists(&self, repo: &Path, branch: &str) -> AppResult<bool> {
        Ok(Self::local_branch(repo, branch)
            || git(
                repo,
                &[
                    "rev-parse",
                    "--verify",
                    "--quiet",
                    &format!("refs/remotes/origin/{branch}"),
                ],
            )
            .is_ok())
    }

    async fn add_worktree(
        &self,
        repo: &Path,
        path: &Path,
        branch: &str,
        base: &str,
    ) -> AppResult<()> {
        let path_text = path.to_string_lossy();
        if Self::local_branch(repo, branch) {
            git(repo, &["worktree", "add", "--quiet", &path_text, branch]).map_err(upstream)?;
        } else {
            git(
                repo,
                &["worktree", "add", "--quiet", "-b", branch, &path_text, base],
            )
            .map_err(upstream)?;
        }
        self.record(format!("worktree {branch} from {base}"));
        Ok(())
    }

    async fn remove_worktree(
        &self,
        repo: &Path,
        path: &Path,
        owned_root: &Path,
    ) -> AppResult<bool> {
        if !path.starts_with(owned_root) || self.has_changes(path).await? {
            return Ok(false);
        }
        git(repo, &["worktree", "remove", &path.to_string_lossy()]).map_err(upstream)?;
        Ok(true)
    }

    async fn has_changes(&self, worktree: &Path) -> AppResult<bool> {
        Ok(!git(worktree, &["status", "--porcelain"])
            .map_err(upstream)?
            .trim()
            .is_empty())
    }

    async fn install_push_guard(&self, worktree: &Path, protected: &[String]) -> AppResult<()> {
        self.record(format!(
            "guard {} {}",
            worktree.display(),
            protected.join(",")
        ));
        Ok(())
    }

    async fn set_identity(&self, worktree: &Path, name: &str, email: &str) -> AppResult<()> {
        git(worktree, &["config", "user.name", name]).map_err(upstream)?;
        git(worktree, &["config", "user.email", email]).map_err(upstream)?;
        Ok(())
    }

    async fn diff_stat(&self, _: &Path, _: &str, _: &str) -> AppResult<DiffStat> {
        Err(AppError::Upstream(
            "diff_stat is not used by these tests".into(),
        ))
    }

    async fn review_changes(
        &self,
        _: &Path,
        _: &str,
    ) -> AppResult<yulius_app::ports::git::ReviewChanges> {
        Err(AppError::Upstream(
            "review_changes is not used by these tests".into(),
        ))
    }

    async fn review_commits(
        &self,
        _: &Path,
        _: &str,
        _: &str,
        _: u32,
    ) -> AppResult<Vec<yulius_contracts::api::git::TaskCommit>> {
        Err(AppError::Upstream(
            "review_commits is not used by these tests".into(),
        ))
    }

    async fn ahead_behind(&self, _: &Path, _: &str, _: &str) -> AppResult<(u32, u32)> {
        Err(AppError::Upstream(
            "ahead_behind is not used by these tests".into(),
        ))
    }

    async fn log(&self, _: &Path, _: &str, _: u32) -> AppResult<Vec<CommitInfo>> {
        Err(AppError::Upstream("log is not used by these tests".into()))
    }

    async fn commit_all(
        &self,
        worktree: &Path,
        message: &str,
        name: &str,
        email: &str,
    ) -> AppResult<bool> {
        git(worktree, &["add", "-A"]).map_err(upstream)?;
        if !self.has_changes(worktree).await? {
            return Ok(false);
        }
        git(
            worktree,
            &[
                "-c",
                &format!("user.name={name}"),
                "-c",
                &format!("user.email={email}"),
                "commit",
                "--quiet",
                "-m",
                message,
            ],
        )
        .map_err(upstream)?;
        self.record(format!("commit by {email}"));
        Ok(true)
    }

    async fn push(
        &self,
        repo: &Path,
        local_branch: &str,
        remote_branch: &str,
        token: &Secret,
    ) -> AppResult<()> {
        git(
            repo,
            &[
                "push",
                "--quiet",
                "origin",
                &format!("refs/heads/{local_branch}:refs/heads/{remote_branch}"),
            ],
        )
        .map_err(upstream)?;
        self.record(format!(
            "push {local_branch}->{remote_branch} token={}",
            token.expose()
        ));
        Ok(())
    }

    async fn import_bundle(
        &self,
        repo: &Path,
        bundle: &Path,
        prefix: &str,
    ) -> AppResult<Vec<String>> {
        let bundle_text = bundle.to_string_lossy();
        let heads = git(repo, &["bundle", "list-heads", &bundle_text]).map_err(upstream)?;
        let mut names = Vec::new();
        for line in heads.lines() {
            if let Some(branch) = line
                .split_whitespace()
                .nth(1)
                .and_then(|r| r.strip_prefix("refs/heads/"))
            {
                names.push(format!("{prefix}/{branch}"));
            }
        }
        git(
            repo,
            &[
                "fetch",
                "--quiet",
                &bundle_text,
                &format!("+refs/heads/*:refs/heads/{prefix}/*"),
            ],
        )
        .map_err(upstream)?;
        self.record(format!("bundle {prefix}"));
        Ok(names)
    }
}

/// A repository creation recorded by [`RepoGitHub`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreatedRepo {
    /// Owner.
    pub owner: String,
    /// Name.
    pub name: String,
    /// Organization owner.
    pub is_org: bool,
    /// Token used.
    pub token: String,
}

/// GitHub fake extension: installation repositories, repository creation (backed by local bare
/// repositories registered with [`LocalGit`]) and rulesets; everything else is delegated to
/// [`FakeGitHub`].
pub struct RepoGitHub {
    /// Users, OAuth and permissions.
    pub base: Arc<FakeGitHub>,
    /// Git fake receiving created remotes.
    pub git: Arc<LocalGit>,
    /// Directory of created bare repositories.
    pub remotes_dir: PathBuf,
    /// Repositories the App installation can access.
    pub installation_repos: Mutex<Vec<GhRepo>>,
    /// Created repositories.
    pub created: Mutex<Vec<CreatedRepo>>,
    /// Created rulesets: (repository, request, token).
    pub rulesets: Mutex<Vec<(String, RulesetRequest, String)>>,
    /// Installation lookups.
    pub installation_lookups: AtomicU32,
}

impl RepoGitHub {
    /// Adds an installation repository.
    pub fn add_installation_repo(&self, owner: &str, name: &str, private: bool) {
        if let Ok(mut repos) = self.installation_repos.lock() {
            repos.push(gh_repo(owner, name, private));
        }
    }

    /// Recorded creations.
    pub fn created(&self) -> Vec<CreatedRepo> {
        self.created.lock().map(|c| c.clone()).unwrap_or_default()
    }

    /// Recorded rulesets.
    pub fn rulesets(&self) -> Vec<(String, RulesetRequest, String)> {
        self.rulesets.lock().map(|r| r.clone()).unwrap_or_default()
    }
}

fn gh_repo(owner: &str, name: &str, private: bool) -> GhRepo {
    GhRepo {
        id: 100,
        owner: owner.to_owned(),
        name: name.to_owned(),
        private,
        default_branch: "main".to_owned(),
        html_url: format!("https://github.com/{owner}/{name}"),
        clone_url: format!("https://github.com/{owner}/{name}.git"),
    }
}

#[async_trait]
impl GitHubPort for RepoGitHub {
    fn authorize_url(
        &self,
        app: &GitHubAppConfig,
        state: &str,
        redirect: &str,
        select: bool,
    ) -> String {
        self.base.authorize_url(app, state, redirect, select)
    }
    async fn exchange_code(
        &self,
        app: &GitHubAppConfig,
        code: &str,
        redirect: &str,
    ) -> AppResult<GhUserToken> {
        self.base.exchange_code(app, code, redirect).await
    }
    async fn refresh_token(
        &self,
        app: &GitHubAppConfig,
        refresh: &Secret,
    ) -> AppResult<GhUserToken> {
        self.base.refresh_token(app, refresh).await
    }
    async fn current_user(&self, token: &Secret) -> AppResult<GhUser> {
        self.base.current_user(token).await
    }
    async fn repo_access(&self, token: &Secret, owner: &str, name: &str) -> AppResult<RepoAccess> {
        self.base.repo_access(token, owner, name).await
    }
    async fn find_installation(&self, _: &GitHubAppConfig, _: &str) -> AppResult<Option<u64>> {
        self.installation_lookups.fetch_add(1, Ordering::SeqCst);
        Ok(Some(7))
    }
    async fn installation_token(&self, _: &GitHubAppConfig, id: u64) -> AppResult<Secret> {
        Ok(Secret(format!("inst-{id}")))
    }
    async fn installation_repos(&self, _: &GitHubAppConfig, _: u64) -> AppResult<Vec<GhRepo>> {
        Ok(self
            .installation_repos
            .lock()
            .map_err(AppError::internal)?
            .clone())
    }
    async fn create_repo(
        &self,
        token: &Secret,
        owner: &str,
        owner_is_org: bool,
        name: &str,
        _description: &str,
    ) -> AppResult<GhRepo> {
        let path = self.remotes_dir.join(owner).join(format!("{name}.git"));
        std::fs::create_dir_all(&path).map_err(AppError::internal)?;
        git(&path, &["init", "--quiet", "--bare"]).map_err(AppError::Upstream)?;
        let repo = gh_repo(owner, name, true);
        self.git.add_remote(&repo.clone_url, &path);
        if let Ok(mut created) = self.created.lock() {
            created.push(CreatedRepo {
                owner: owner.to_owned(),
                name: name.to_owned(),
                is_org: owner_is_org,
                token: token.expose().to_owned(),
            });
        }
        Ok(repo)
    }
    async fn create_ruleset(
        &self,
        token: &Secret,
        owner: &str,
        name: &str,
        request: &RulesetRequest,
    ) -> AppResult<()> {
        if let Ok(mut rulesets) = self.rulesets.lock() {
            rulesets.push((
                format!("{owner}/{name}"),
                request.clone(),
                token.expose().to_owned(),
            ));
        }
        Ok(())
    }
    fn app_manifest(&self, public_url: &str, owner: &str) -> serde_json::Value {
        self.base.app_manifest(public_url, owner)
    }
    fn manifest_post_url(&self, owner: &str, is_org: bool) -> String {
        self.base.manifest_post_url(owner, is_org)
    }
    async fn convert_manifest(&self, code: &str) -> AppResult<GitHubAppConfig> {
        self.base.convert_manifest(code).await
    }
    async fn create_pull(&self, t: &Secret, o: &str, r: &str, p: &NewPull) -> AppResult<GhPull> {
        self.base.create_pull(t, o, r, p).await
    }
    async fn get_pull(&self, t: &Secret, o: &str, r: &str, n: u64) -> AppResult<GhPull> {
        self.base.get_pull(t, o, r, n).await
    }
    async fn list_pulls(
        &self,
        t: &Secret,
        o: &str,
        r: &str,
        head: Option<&str>,
    ) -> AppResult<Vec<GhPull>> {
        self.base.list_pulls(t, o, r, head).await
    }
    async fn checks(&self, t: &Secret, o: &str, r: &str, sha: &str) -> AppResult<ChecksState> {
        self.base.checks(t, o, r, sha).await
    }
    async fn merge_pull(
        &self,
        t: &Secret,
        o: &str,
        r: &str,
        n: u64,
        method: &str,
        head: &str,
    ) -> AppResult<String> {
        self.base.merge_pull(t, o, r, n, method, head).await
    }
    async fn fast_forward(
        &self,
        t: &Secret,
        o: &str,
        r: &str,
        b: &str,
        sha: &str,
    ) -> AppResult<()> {
        self.base.fast_forward(t, o, r, b, sha).await
    }
    async fn branch_sha(&self, t: &Secret, o: &str, r: &str, b: &str) -> AppResult<Option<String>> {
        self.base.branch_sha(t, o, r, b).await
    }
}

/// A harness with real Git over local remotes and the extended GitHub fake.
pub struct World {
    /// The harness (its `github` is the base fake for users and permissions).
    pub h: Harness,
    /// GitHub extension.
    pub github: Arc<RepoGitHub>,
    /// Git fake.
    pub git: Arc<LocalGit>,
    /// Holds remotes and local checkouts.
    pub scratch: tempfile::TempDir,
}

/// Owner of the test instance.
pub const OWNER: &str = "Quartio-Studio";

impl World {
    /// A configured instance owned by [`OWNER`] (`org` or `user`).
    pub async fn new(owner_kind: &str) -> Result<Self, BoxError> {
        let dir = tempfile::tempdir()?;
        let scratch = tempfile::tempdir()?;
        let store = SqliteStore::open_in_memory()?;
        let base = Arc::new(FakeGitHub::default());
        let git = Arc::new(LocalGit::default());
        let github = Arc::new(RepoGitHub {
            base: base.clone(),
            git: git.clone(),
            remotes_dir: scratch.path().join("remotes"),
            installation_repos: Mutex::new(Vec::new()),
            created: Mutex::new(Vec::new()),
            rulesets: Mutex::new(Vec::new()),
            installation_lookups: AtomicU32::new(0),
        });
        let clock = Arc::new(TestClock(AtomicI64::new(1_791_331_200_000)));
        let config = AppConfig::new(
            dir.path().to_path_buf(),
            "https://yulius.test",
            "http://127.0.0.1:1",
            "/bin/false".into(),
        );
        let deps = Deps {
            store: Arc::new(store.clone()),
            clock: clock.clone(),
            crypto: Arc::new(MasterKeyCrypto::from_key(&[9u8; 32])?),
            github: github.clone(),
            session_host: Arc::new(yulius::wiring::pending::UnavailableSessionHost),
            harness: Arc::new(yulius::wiring::harness::HarnessAdapter::new(
                "/bin/false".into(),
                "http://127.0.0.1:1",
            )),
            git: git.clone(),
            system: Arc::new(super::FixedSystem),
            browser: Arc::new(yulius::wiring::browser::UnavailableBrowser),
            usage: Arc::new(yulius_app::ports::usage::NoUsageProbe),
        };
        let app = App::new(config, deps);
        let router = yulius_server::public_router(app.clone()).layer(MockConnectInfo(
            std::net::SocketAddr::from(([127, 0, 0, 1], 5000)),
        ));
        let internal = yulius_server::internal_router(app.clone()).layer(MockConnectInfo(
            std::net::SocketAddr::from(([127, 0, 0, 1], 5001)),
        ));
        app.prepare_setup().await?;
        let app_cfg = serde_json::json!({"app_id": 1, "slug": "yulius-test", "client_id": "c",
            "client_secret": "s", "private_key_pem": "k", "webhook_secret": null});
        let sealed = app.deps.crypto.seal(app_cfg.to_string().as_bytes())?;
        store.set_github_app(&sealed, OWNER, owner_kind).await?;
        Ok(Self {
            h: Harness {
                app,
                internal,
                github: base,
                clock,
                store,
                router,
                _dir: dir,
            },
            github,
            git,
            scratch,
        })
    }

    /// The data directory.
    pub fn data_dir(&self) -> PathBuf {
        self.h.app.config.data_dir.clone()
    }

    /// A GitHub-linked user with a stored GitHub token and a CLI bearer token.
    pub async fn user(
        &self,
        login: &str,
        instance_admin: bool,
    ) -> Result<(UserRecord, Creds), BoxError> {
        let github_id = 1000 + u64::try_from(self.h.store.users().await?.len())?;
        let user = UserRecord {
            id: UserId::parse(&format!("u-{login}"))?,
            github_login: Some(login.to_owned()),
            github_id: Some(github_id),
            display_name: login.to_uppercase(),
            avatar_url: None,
            is_superadmin: false,
            is_instance_admin: instance_admin,
            password_hash: None,
            created_at: Timestamp(1),
            disabled_at: None,
        };
        self.h.store.insert_user(&user).await?;
        let sealed = self
            .h
            .app
            .deps
            .crypto
            .seal(format!("token-{login}").as_bytes())?;
        self.h
            .store
            .save_github_token(
                &GitHubTokenRecord {
                    user_id: user.id.clone(),
                    access: sealed,
                    refresh: None,
                    expires_at: None,
                    refresh_expires_at: None,
                },
                Timestamp(1),
            )
            .await?;
        let token = self.h.app.issue_api_token(&user, "test").await?;
        Ok((
            user,
            Creds {
                bearer: Some(token.expose().to_owned()),
                ..Creds::default()
            },
        ))
    }

    /// Grants GitHub permissions on `OWNER/name` (or a full `owner/name`).
    pub fn grant(&self, login: &str, repo: &str, level: &str) {
        let full = if repo.contains('/') {
            repo.to_owned()
        } else {
            format!("{OWNER}/{repo}")
        };
        let perms = GitHubPermissions {
            admin: level == "admin",
            maintain: level == "maintain",
            push: matches!(level, "admin" | "maintain" | "push"),
            triage: false,
            pull: level != "none",
        };
        self.h.github.set_perm(login, &full, perms);
    }

    /// A bare remote `owner/name` with `files` committed on `main` and extra branches pointing
    /// at the same commit; registered with the Git fake. Returns its path.
    pub fn remote(
        &self,
        owner: &str,
        name: &str,
        files: &[(&str, &str)],
        branches: &[&str],
    ) -> Result<PathBuf, BoxError> {
        let seed = self.scratch.path().join("seed").join(owner).join(name);
        std::fs::create_dir_all(&seed)?;
        git(&seed, &["init", "--quiet"])?;
        for (path, contents) in files {
            let target = seed.join(path);
            if let Some(parent) = target.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(target, contents)?;
        }
        git(&seed, &["add", "-A"])?;
        git(&seed, &["commit", "--quiet", "-m", "initial"])?;
        for branch in branches {
            git(&seed, &["branch", branch])?;
        }
        let bare = self
            .scratch
            .path()
            .join("remotes")
            .join(owner)
            .join(format!("{name}.git"));
        std::fs::create_dir_all(bare.parent().ok_or("parent")?)?;
        git(
            self.scratch.path(),
            &[
                "clone",
                "--quiet",
                "--bare",
                &seed.to_string_lossy(),
                &bare.to_string_lossy(),
            ],
        )?;
        self.git
            .add_remote(&format!("https://github.com/{owner}/{name}.git"), &bare);
        Ok(bare)
    }
}

/// Sends a raw body with extra headers.
pub async fn call_raw(
    router: &axum::Router,
    method: Method,
    path: &str,
    body: Vec<u8>,
    headers: &[(&str, &str)],
    creds: &Creds,
) -> Result<Resp, BoxError> {
    let mut req = Request::builder().method(method).uri(path);
    if let Some(token) = &creds.bearer {
        req = req.header(header::AUTHORIZATION, format!("Bearer {token}"));
    }
    if let Some(cookie) = &creds.cookie {
        req = req.header(header::COOKIE, format!("yulius_session={cookie}"));
    }
    if let Some(csrf) = &creds.csrf {
        req = req.header("x-yulius-csrf", csrf);
    }
    for (name, value) in headers {
        req = req.header(*name, *value);
    }
    let resp = router.clone().oneshot(req.body(Body::from(body))?).await?;
    let status = resp.status();
    let bytes = resp.into_body().collect().await?.to_bytes();
    Ok(Resp {
        status,
        body: serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null),
        set_cookie: None,
        location: None,
    })
}
