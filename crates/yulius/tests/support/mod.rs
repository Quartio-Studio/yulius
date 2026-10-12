//! Test harness: in-memory app with a fake GitHub and a controllable clock.
// Shared by the test modules; each uses a subset (and `isolation` is Linux-only), so unused
// items are expected here.
#![allow(dead_code)]

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use axum::Router;
use axum::body::Body;
use axum::extract::connect_info::MockConnectInfo;
use axum::http::{Method, Request, StatusCode, header};
use http_body_util::BodyExt;
use tower::ServiceExt;
use yulius_app::context::{App, AppConfig, Deps};
use yulius_app::error::{AppError, AppResult};
use yulius_app::model::Secret;
use yulius_app::ports::Clock;
use yulius_app::ports::github::{
    ChecksReport, ChecksState, GhIssue, GhPull, GhRepo, GhUser, GhUserToken, GitHubAppConfig,
    GitHubPort, NewCommitStatus, NewIssue, NewPull, RepoAccess, RulesetRequest,
    STATUS_PERMISSION_MISSING,
};
use yulius_domain::permission::GitHubPermissions;
use yulius_domain::time::Timestamp;
use yulius_store::SqliteStore;
use yulius_store::crypto::MasterKeyCrypto;

pub type TestResult = Result<(), Box<dyn std::error::Error>>;

/// Controllable clock.
pub struct TestClock(pub AtomicI64);

impl Clock for TestClock {
    fn now(&self) -> Timestamp {
        Timestamp(self.0.load(Ordering::SeqCst))
    }
}

/// Fake GitHub: OAuth codes map to users; permissions per (login, repo).
#[derive(Default)]
pub struct FakeGitHub {
    pub users_by_code: Mutex<HashMap<String, GhUser>>,
    pub perms: Mutex<HashMap<(String, String), GitHubPermissions>>,
    pub fail_permissions: std::sync::atomic::AtomicBool,
    /// Pull requests (delivery tests).
    pub pulls: Mutex<Vec<GhPull>>,
    /// Checks per commit SHA (default: `None`, every source read).
    pub check_states: Mutex<HashMap<String, ChecksReport>>,
    /// When set, reading checks fails with this error (as the adapter maps GitHub errors).
    pub checks_error: Mutex<Option<AppError>>,
    /// Repository side of PRs and refs; without it PR heads are unknown.
    pub remote: Mutex<Option<Arc<dyn FakeRemote>>>,
    /// Recorded state-changing calls (`create_pull …`, `merge_pull …`, `fast_forward …`).
    pub writes: Mutex<Vec<String>>,
    /// The same calls with who made them: the login whose token each carried, or `inst` for the
    /// App installation (F 2026-10-11).
    pub writes_by: Mutex<Vec<(String, String)>>,
    /// Who opened each pull request: the user login, or `inst` for the App installation.
    pub pull_authors: Mutex<HashMap<u64, String>>,
    /// Commit statuses published, in order: (sha, status).
    pub statuses: Mutex<Vec<(String, NewCommitStatus)>>,
    /// When set, publishing a commit status fails as without the App permission.
    pub statuses_forbidden: std::sync::atomic::AtomicBool,
    /// Issues opened (DLV-008).
    pub issues: Mutex<Vec<(String, NewIssue)>>,
    /// When set, reading checks waits for it (a slow GitHub); `checks_entered` says a read began.
    pub checks_hold: Mutex<Option<Arc<tokio::sync::Notify>>>,
    pub checks_entered: tokio::sync::Notify,
}

impl FakeGitHub {
    pub fn add_user(&self, code: &str, id: u64, login: &str) {
        if let Ok(mut m) = self.users_by_code.lock() {
            m.insert(
                code.into(),
                GhUser {
                    id,
                    login: login.into(),
                    name: Some(login.to_uppercase()),
                    avatar_url: None,
                },
            );
        }
    }
    pub fn set_perm(&self, login: &str, repo: &str, perms: GitHubPermissions) {
        if let Ok(mut m) = self.perms.lock() {
            m.insert((login.into(), repo.into()), perms);
        }
    }
}

fn login_of(token: &Secret) -> String {
    token.expose().trim_start_matches("token-").to_owned()
}

#[async_trait]
impl GitHubPort for FakeGitHub {
    fn authorize_url(&self, _: &GitHubAppConfig, state: &str, _: &str, select: bool) -> String {
        let prompt = if select { "&prompt=select_account" } else { "" };
        format!("https://github.test/login/oauth/authorize?state={state}{prompt}")
    }
    async fn exchange_code(
        &self,
        _: &GitHubAppConfig,
        code: &str,
        _: &str,
    ) -> AppResult<GhUserToken> {
        let user = self
            .users_by_code
            .lock()
            .map_err(AppError::internal)?
            .get(code)
            .cloned()
            .ok_or(AppError::Upstream("bad code".into()))?;
        Ok(GhUserToken {
            access: Secret(format!("token-{}", user.login)),
            refresh: None,
            expires_at: None,
            refresh_expires_at: None,
        })
    }
    async fn refresh_token(&self, _: &GitHubAppConfig, _: &Secret) -> AppResult<GhUserToken> {
        Err(AppError::Upstream("no refresh".into()))
    }
    async fn current_user(&self, token: &Secret) -> AppResult<GhUser> {
        let login = login_of(token);
        let users = self.users_by_code.lock().map_err(AppError::internal)?;
        users
            .values()
            .find(|u| u.login == login)
            .cloned()
            .ok_or(AppError::Unauthenticated)
    }
    async fn repo_access(&self, token: &Secret, owner: &str, name: &str) -> AppResult<RepoAccess> {
        if self.fail_permissions.load(Ordering::SeqCst) {
            return Err(AppError::Upstream("github down".into()));
        }
        let key = (login_of(token), format!("{owner}/{name}"));
        let perms = self
            .perms
            .lock()
            .map_err(AppError::internal)?
            .get(&key)
            .copied();
        Ok(match perms {
            None => RepoAccess::NoAccess,
            Some(permissions) => RepoAccess::Access {
                permissions,
                repo: GhRepo {
                    id: 1,
                    owner: owner.into(),
                    name: name.into(),
                    private: true,
                    default_branch: "main".into(),
                    html_url: String::new(),
                    clone_url: String::new(),
                },
            },
        })
    }
    async fn find_installation(&self, _: &GitHubAppConfig, _: &str) -> AppResult<Option<u64>> {
        Ok(Some(1))
    }
    async fn installation_token(&self, _: &GitHubAppConfig, _: u64) -> AppResult<Secret> {
        Ok(Secret("inst".into()))
    }
    async fn installation_repos(&self, _: &GitHubAppConfig, _: u64) -> AppResult<Vec<GhRepo>> {
        Ok(Vec::new())
    }
    async fn create_repo(
        &self,
        _: &Secret,
        _: &str,
        _: bool,
        _: &str,
        _: &str,
    ) -> AppResult<GhRepo> {
        Err(AppError::Upstream("n/a".into()))
    }
    async fn create_ruleset(
        &self,
        _: &Secret,
        _: &str,
        _: &str,
        _: &RulesetRequest,
    ) -> AppResult<()> {
        Ok(())
    }
    fn app_manifest(&self, _: &str, _: &str) -> serde_json::Value {
        serde_json::json!({})
    }
    fn manifest_post_url(&self, _: &str, _: bool) -> String {
        String::new()
    }
    async fn convert_manifest(&self, code: &str) -> AppResult<GitHubAppConfig> {
        if code != "manifest-code" {
            return Err(AppError::Upstream("unknown manifest code".into()));
        }
        Ok(GitHubAppConfig {
            app_id: 42,
            slug: "yulius-test-app".into(),
            client_id: "Iv1.test".into(),
            client_secret: "secret".into(),
            private_key_pem: "pem".into(),
            webhook_secret: None,
        })
    }
    async fn create_issue(
        &self,
        token: &Secret,
        owner: &str,
        repo: &str,
        issue: &NewIssue,
    ) -> AppResult<GhIssue> {
        let mut issues = self.issues.lock().map_err(AppError::internal)?;
        issues.push((login_of(token), issue.clone()));
        let number = u64::try_from(issues.len()).unwrap_or(0) + 100;
        drop(issues);
        self.record(token, format!("create_issue #{number} {}", issue.title));
        Ok(GhIssue {
            number,
            html_url: format!("https://github.test/{owner}/{repo}/issues/{number}"),
        })
    }
    async fn create_pull(
        &self,
        token: &Secret,
        owner: &str,
        repo: &str,
        pull: &NewPull,
    ) -> AppResult<GhPull> {
        let head_sha = self
            .remote()
            .and_then(|r| r.branch_sha(&pull.head))
            .ok_or_else(|| AppError::Upstream("head branch not found".into()))?;
        let mut pulls = self.pulls.lock().map_err(AppError::internal)?;
        if pulls
            .iter()
            .any(|p| p.state == "open" && p.head == pull.head && p.base == pull.base)
        {
            return Err(AppError::Upstream("a pull request already exists".into()));
        }
        let number = u64::try_from(pulls.len()).unwrap_or(0) + 1;
        let created = GhPull {
            number,
            html_url: format!("https://github.test/{owner}/{repo}/pull/{number}"),
            title: pull.title.clone(),
            state: "open".into(),
            merged: false,
            base: pull.base.clone(),
            head: pull.head.clone(),
            head_sha,
            mergeable: None,
            behind: false,
        };
        pulls.push(created.clone());
        if let Ok(mut authors) = self.pull_authors.lock() {
            authors.insert(number, login_of(token));
        }
        self.record(
            token,
            format!("create_pull #{number} {}<-{}", pull.base, pull.head),
        );
        Ok(created)
    }
    async fn get_pull(&self, _: &Secret, _: &str, _: &str, number: u64) -> AppResult<GhPull> {
        self.refresh_heads();
        self.pulls
            .lock()
            .map_err(AppError::internal)?
            .iter()
            .find(|p| p.number == number)
            .cloned()
            .ok_or(AppError::NotFound)
    }
    async fn list_pulls(
        &self,
        _: &Secret,
        _: &str,
        _: &str,
        head: Option<&str>,
    ) -> AppResult<Vec<GhPull>> {
        self.refresh_heads();
        Ok(self
            .pulls
            .lock()
            .map_err(AppError::internal)?
            .iter()
            .filter(|p| p.state == "open" && head.is_none_or(|h| p.head == h))
            .cloned()
            .collect())
    }
    async fn checks(&self, _: &Secret, _: &str, _: &str, sha: &str) -> AppResult<ChecksReport> {
        let hold = self.checks_hold.lock().ok().and_then(|h| h.clone());
        if let Some(hold) = hold {
            self.checks_entered.notify_one();
            hold.notified().await;
        }
        if let Some(err) = self
            .checks_error
            .lock()
            .map_err(AppError::internal)?
            .clone()
        {
            return Err(err);
        }
        Ok(self
            .check_states
            .lock()
            .map_err(AppError::internal)?
            .get(sha)
            .cloned()
            .unwrap_or_else(|| ChecksState::None.into()))
    }
    async fn merge_pull(
        &self,
        token: &Secret,
        _: &str,
        _: &str,
        number: u64,
        method: &str,
        expected_head: &str,
    ) -> AppResult<String> {
        self.refresh_heads();
        let pull = self
            .pulls
            .lock()
            .map_err(AppError::internal)?
            .iter()
            .find(|p| p.number == number)
            .cloned()
            .ok_or(AppError::NotFound)?;
        if pull.state != "open" {
            return Err(AppError::Conflict("pull request is not open".into()));
        }
        if pull.head_sha != expected_head {
            return Err(AppError::Conflict("head branch was modified".into()));
        }
        let remote = self
            .remote()
            .ok_or_else(|| AppError::Upstream("no remote".into()))?;
        let sha = remote
            .merge(
                &pull.base,
                &pull.head_sha,
                method,
                &format!("Merge pull request #{number}"),
            )
            .map_err(AppError::Upstream)?;
        if let Ok(mut pulls) = self.pulls.lock()
            && let Some(p) = pulls.iter_mut().find(|p| p.number == number)
        {
            p.state = "closed".into();
            p.merged = true;
        }
        self.record(token, format!("merge_pull #{number} {method}"));
        Ok(sha)
    }
    async fn fast_forward(
        &self,
        token: &Secret,
        _: &str,
        _: &str,
        branch: &str,
        sha: &str,
    ) -> AppResult<()> {
        let remote = self
            .remote()
            .ok_or_else(|| AppError::Upstream("no remote".into()))?;
        remote
            .fast_forward(branch, sha)
            .map_err(|_| AppError::Conflict("update is not a fast forward".into()))?;
        self.record(token, format!("fast_forward {branch} {sha}"));
        Ok(())
    }
    async fn create_commit_status(
        &self,
        _: &Secret,
        _: &str,
        _: &str,
        sha: &str,
        status: &NewCommitStatus,
    ) -> AppResult<()> {
        if self.statuses_forbidden.load(Ordering::SeqCst) {
            return Err(AppError::Forbidden(STATUS_PERMISSION_MISSING.to_owned()));
        }
        self.statuses
            .lock()
            .map_err(AppError::internal)?
            .push((sha.to_owned(), status.clone()));
        Ok(())
    }
    async fn delete_branch(
        &self,
        token: &Secret,
        _: &str,
        _: &str,
        branch: &str,
    ) -> AppResult<bool> {
        let remote = self
            .remote()
            .ok_or_else(|| AppError::Upstream("no remote".into()))?;
        let deleted = remote.delete_branch(branch).map_err(AppError::Upstream)?;
        self.record(token, format!("delete_branch {branch}"));
        Ok(deleted)
    }
    async fn branch_pulls(
        &self,
        _: &Secret,
        _: &str,
        _: &str,
        branch: &str,
    ) -> AppResult<Vec<GhPull>> {
        let mut pulls: Vec<GhPull> = self
            .pulls
            .lock()
            .map_err(AppError::internal)?
            .iter()
            .filter(|p| p.head == branch)
            .cloned()
            .collect();
        pulls.sort_by_key(|p| std::cmp::Reverse(p.number));
        Ok(pulls)
    }
    async fn branch_sha(
        &self,
        _: &Secret,
        _: &str,
        _: &str,
        branch: &str,
    ) -> AppResult<Option<String>> {
        Ok(self.remote().and_then(|r| r.branch_sha(branch)))
    }
}

/// A test instance.
pub struct Harness {
    pub app: Arc<App>,
    pub internal: Router,
    pub github: Arc<FakeGitHub>,
    pub clock: Arc<TestClock>,
    pub store: SqliteStore,
    pub router: Router,
    pub _dir: tempfile::TempDir,
}

pub fn harness() -> Result<Harness, Box<dyn std::error::Error>> {
    harness_with_git(Arc::new(yulius::wiring::pending::UnavailableGit))
}

/// Like [`harness`] with a specific Git adapter.
pub fn harness_with_git(
    git: Arc<dyn yulius_app::ports::git::GitPort>,
) -> Result<Harness, Box<dyn std::error::Error>> {
    build_harness(
        git,
        Arc::new(yulius::wiring::pending::UnavailableSessionHost),
    )
}

/// The shared builder: in-memory store, fake GitHub, fixed clock and system metrics.
pub fn build_harness(
    git: Arc<dyn yulius_app::ports::git::GitPort>,
    session_host: Arc<dyn yulius_app::ports::session::SessionHost>,
) -> Result<Harness, Box<dyn std::error::Error>> {
    harness_with(HarnessOptions {
        git: Some(git),
        session_host: Some(session_host),
        ..HarnessOptions::default()
    })
}

/// A response.
pub struct Resp {
    pub status: StatusCode,
    pub body: serde_json::Value,
    pub set_cookie: Option<String>,
    pub location: Option<String>,
}

/// Session credentials.
#[derive(Clone, Default)]
pub struct Creds {
    pub cookie: Option<String>,
    pub csrf: Option<String>,
    pub bearer: Option<String>,
}

pub async fn call(
    router: &Router,
    method: Method,
    path: &str,
    body: Option<serde_json::Value>,
    creds: &Creds,
) -> Result<Resp, Box<dyn std::error::Error>> {
    let mut req = Request::builder().method(method).uri(path);
    if let Some(cookie) = &creds.cookie {
        req = req.header(header::COOKIE, cookie_header(cookie));
    }
    if let Some(csrf) = &creds.csrf {
        req = req.header("x-yulius-csrf", csrf);
    }
    if let Some(token) = &creds.bearer {
        req = req.header(header::AUTHORIZATION, format!("Bearer {token}"));
    }
    let req = match body {
        Some(json) => req
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(json.to_string()))?,
        None => req.body(Body::empty())?,
    };
    let resp = router.clone().oneshot(req).await?;
    let status = resp.status();
    let set_cookie = resp
        .headers()
        .get(header::SET_COOKIE)
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    let location = resp
        .headers()
        .get(header::LOCATION)
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    let bytes = resp.into_body().collect().await?.to_bytes();
    let body = serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
    Ok(Resp {
        status,
        body,
        set_cookie,
        location,
    })
}

/// Sends `body` as a raw upload (`Content-Length` when `declared`).
pub async fn send(
    router: &Router,
    method: Method,
    path: &str,
    body: Body,
    declared: Option<usize>,
    creds: &Creds,
) -> Result<Resp, Box<dyn std::error::Error>> {
    let mut req = Request::builder().method(method).uri(path);
    if let Some(cookie) = &creds.cookie {
        req = req.header(header::COOKIE, cookie_header(cookie));
    }
    if let Some(csrf) = &creds.csrf {
        req = req.header("x-yulius-csrf", csrf);
    }
    if let Some(token) = &creds.bearer {
        req = req.header(header::AUTHORIZATION, format!("Bearer {token}"));
    }
    if let Some(len) = declared {
        req = req.header(header::CONTENT_LENGTH, len);
    }
    req = req.header(header::CONTENT_TYPE, "application/octet-stream");
    let resp = router.clone().oneshot(req.body(body)?).await?;
    let status = resp.status();
    let bytes = resp.into_body().collect().await?.to_bytes();
    Ok(Resp {
        status,
        body: serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null),
        set_cookie: None,
        location: None,
    })
}

/// Extracts the session value from a Set-Cookie header (`__Host-yulius_session` over HTTPS,
/// `yulius_session` over HTTP).
pub fn session_from(set_cookie: &Option<String>) -> Option<String> {
    let raw = set_cookie.as_ref()?;
    let first = raw.split(';').next()?;
    first
        .strip_prefix("__Host-yulius_session=")
        .or_else(|| first.strip_prefix("yulius_session="))
        .map(str::to_owned)
}

/// A `Cookie` header carrying a session under both names (HTTPS and HTTP instances).
pub fn cookie_header(session: &str) -> String {
    format!("yulius_session={session}; __Host-yulius_session={session}")
}

/// Deterministic machine metrics for status tests (disk total unknown on purpose).
pub struct FixedSystem;

impl yulius_app::ports::system::SystemProbe for FixedSystem {
    fn snapshot(&self, _: &std::path::Path) -> yulius_app::ports::system::SystemSnapshot {
        yulius_app::ports::system::SystemSnapshot {
            cpu_percent: Some(12.5),
            cpu_count: Some(4),
            memory_used: Some(1_000),
            memory_total: Some(4_000),
            disk_used: None,
            disk_total: None,
            load: Some([0.5, 0.25, 0.125]),
            uptime_s: Some(60),
        }
    }
}

/// Like [`harness`] but with a custom process host (account login tests).
pub fn harness_with_host(
    session_host: Arc<dyn yulius_app::ports::session::SessionHost>,
) -> Result<Harness, Box<dyn std::error::Error>> {
    build_harness(
        Arc::new(yulius::wiring::pending::UnavailableGit),
        session_host,
    )
}

/// Creates the superadmin through the setup flow and returns its cookie credentials.
pub async fn setup_superadmin_creds(h: &Harness) -> Result<Creds, Box<dyn std::error::Error>> {
    let token = h.app.prepare_setup().await?.ok_or("setup token")?;
    let r = call(
        &h.router,
        Method::POST,
        "/api/setup",
        Some(serde_json::json!({
            "token": token.expose(), "password": "a very long secret",
            "password_confirm": "a very long secret", "confirm_saved": true
        })),
        &Creds::default(),
    )
    .await?;
    if r.status != StatusCode::CREATED {
        return Err(format!("setup failed: {}", r.status).into());
    }
    Ok(Creds {
        cookie: session_from(&r.set_cookie),
        csrf: r.body["csrf_token"].as_str().map(str::to_owned),
        bearer: None,
    })
}

/// Bearer credentials for the superadmin (created by [`setup_superadmin_creds`]); WebSocket
/// tests connect with bearer tokens.
pub async fn superadmin_bearer(h: &Harness) -> Result<Creds, Box<dyn std::error::Error>> {
    use yulius_app::ports::store::UserStore;
    let user = h.store.superadmin().await?.ok_or("no superadmin")?;
    let token = h.app.issue_api_token(&user, "tests").await?;
    Ok(Creds {
        bearer: Some(token.expose().to_owned()),
        ..Creds::default()
    })
}

/// Stores a GitHub App so GitHub sign-in works against the fake.
pub async fn configure_github_app(h: &Harness) -> Result<(), Box<dyn std::error::Error>> {
    use yulius_app::ports::store::InstanceStore;
    let cfg = serde_json::json!({"app_id": 1, "slug": "yulius-test", "client_id": "c",
        "client_secret": "s", "private_key_pem": "k", "webhook_secret": null});
    let sealed = h.app.deps.crypto.seal(cfg.to_string().as_bytes())?;
    h.store
        .set_github_app(&sealed, "Quartio-Studio", "org")
        .await?;
    Ok(())
}

/// Admits `login` (as superadmin), signs it in through the fake GitHub and returns cookie
/// credentials with CSRF.
pub async fn admit_and_sign_in(
    h: &Harness,
    admin: &Creds,
    login: &str,
    github_id: u64,
    instance_admin: bool,
) -> Result<Creds, Box<dyn std::error::Error>> {
    let code = format!("code-{login}");
    h.github.add_user(&code, github_id, login);
    let admit = call(
        &h.router,
        Method::POST,
        "/api/admin/users",
        Some(serde_json::json!({"github_login": login, "is_instance_admin": instance_admin})),
        admin,
    )
    .await?;
    if admit.status != StatusCode::CREATED {
        return Err(format!("admit failed: {}", admit.status).into());
    }
    let start = call(
        &h.router,
        Method::GET,
        "/api/auth/github/start?next=/",
        None,
        &Creds::default(),
    )
    .await?;
    let location = start.location.ok_or("location")?;
    let state = location.split("state=").nth(1).ok_or("state")?.to_owned();
    let cb = call(
        &h.router,
        Method::GET,
        &format!("/api/auth/github/callback?code={code}&state={state}"),
        None,
        &Creds::default(),
    )
    .await?;
    let cookie = session_from(&cb.set_cookie).ok_or("no session cookie")?;
    let creds = Creds {
        cookie: Some(cookie),
        csrf: None,
        bearer: None,
    };
    let me = call(&h.router, Method::GET, "/api/me", None, &creds).await?;
    Ok(Creds {
        csrf: me.body["csrf_token"].as_str().map(str::to_owned),
        ..creds
    })
}

/// Inserts an active sandbox `id` for repository `Quartio-Studio/<slug>`.
pub async fn insert_sandbox(
    h: &Harness,
    id: &str,
    slug: &str,
    port: u16,
) -> Result<(), Box<dyn std::error::Error>> {
    use yulius_app::ports::store::SandboxStore;
    h.store
        .insert_sandbox(&yulius_app::model::SandboxRecord {
            id: yulius_domain::ids::SandboxId::parse(id)?,
            slug: slug.into(),
            repo_owner: "Quartio-Studio".into(),
            repo_name: slug.into(),
            github_repo_id: None,
            default_branch: "main".into(),
            profile_json: "{}".into(),
            port_base: port,
            port_count: 20,
            onboarding_state: "not_needed".into(),
            onboarding_task_id: None,
            created_by: yulius_domain::ids::UserId::parse("u")?,
            created_at: Timestamp(1),
            removed_at: None,
        })
        .await?;
    Ok(())
}

/// The real harness planning with the fake agent executable.
pub mod agents;
/// Repository fixtures (real Git over local remotes) for project and import tests.
pub mod repos;
/// A fake `main` source and update helper for instance update tests (SPEC-002 ADR-18).
pub mod updates;

/// Repository side of the fake GitHub's pull request and ref endpoints (delivery tests back it
/// with a real bare repository).
pub trait FakeRemote: Send + Sync {
    /// Head SHA of a branch.
    fn branch_sha(&self, branch: &str) -> Option<String>;
    /// Merges `head_sha` into `base` with `method` (`merge`, `squash`, `rebase`); returns the
    /// new head of `base`.
    fn merge(
        &self,
        base: &str,
        head_sha: &str,
        method: &str,
        message: &str,
    ) -> Result<String, String>;
    /// Moves `branch` to `sha` only as a fast-forward.
    fn fast_forward(&self, branch: &str, sha: &str) -> Result<(), String>;
    /// Deletes `branch`; `false` when it did not exist.
    fn delete_branch(&self, branch: &str) -> Result<bool, String> {
        let _ = branch;
        Ok(false)
    }
}

impl FakeGitHub {
    pub fn set_remote(&self, remote: Arc<dyn FakeRemote>) {
        if let Ok(mut slot) = self.remote.lock() {
            *slot = Some(remote);
        }
    }
    fn remote(&self) -> Option<Arc<dyn FakeRemote>> {
        self.remote.lock().ok().and_then(|r| r.clone())
    }
    fn record(&self, token: &Secret, call: String) {
        if let Ok(mut by) = self.writes_by.lock() {
            by.push((call.clone(), login_of(token)));
        }
        if let Ok(mut writes) = self.writes.lock() {
            writes.push(call);
        }
    }
    /// Who made each state-changing call starting with `prefix`, in order: the login whose
    /// token it carried, or `inst` for the App installation.
    pub fn actors_of(&self, prefix: &str) -> Vec<String> {
        self.writes_by
            .lock()
            .map(|by| {
                by.iter()
                    .filter(|(call, _)| call.starts_with(prefix))
                    .map(|(_, actor)| actor.clone())
                    .collect()
            })
            .unwrap_or_default()
    }
    /// Open pull requests follow their head branch, as on GitHub.
    fn refresh_heads(&self) {
        let Some(remote) = self.remote() else { return };
        if let Ok(mut pulls) = self.pulls.lock() {
            for pull in pulls.iter_mut().filter(|p| p.state == "open") {
                if let Some(sha) = remote.branch_sha(&pull.head) {
                    pull.head_sha = sha;
                }
            }
        }
    }
    pub fn set_checks(&self, sha: &str, state: ChecksState) {
        self.set_checks_report(sha, state.into());
    }
    /// Checks of `sha` with sources the App may not read (#111).
    pub fn set_checks_report(&self, sha: &str, report: ChecksReport) {
        if let Ok(mut m) = self.check_states.lock() {
            m.insert(sha.into(), report);
        }
    }
    /// Holds every checks read until the returned handle is released with [`Self::release_checks`].
    pub fn hold_checks(&self) -> Arc<tokio::sync::Notify> {
        let hold = Arc::new(tokio::sync::Notify::new());
        if let Ok(mut h) = self.checks_hold.lock() {
            *h = Some(Arc::clone(&hold));
        }
        hold
    }
    /// Lets the held checks read (and every later one) go on.
    pub fn release_checks(&self, hold: &tokio::sync::Notify) {
        if let Ok(mut h) = self.checks_hold.lock() {
            *h = None;
        }
        hold.notify_waiters();
        hold.notify_one();
    }
    /// Commit statuses published so far: (sha, status).
    pub fn statuses(&self) -> Vec<(String, NewCommitStatus)> {
        self.statuses.lock().map(|s| s.clone()).unwrap_or_default()
    }
    pub fn writes(&self) -> Vec<String> {
        self.writes.lock().map(|w| w.clone()).unwrap_or_default()
    }
    /// The login whose token opened pull request `number` (`inst`: the App installation).
    pub fn pull_author(&self, number: u64) -> Option<String> {
        self.pull_authors
            .lock()
            .ok()
            .and_then(|a| a.get(&number).cloned())
    }
    pub fn pull(&self, number: u64) -> Option<GhPull> {
        self.pulls
            .lock()
            .ok()
            .and_then(|p| p.iter().find(|p| p.number == number).cloned())
    }
}

/// Like [`harness`] but lets a test adjust the configuration first (for example
/// `max_live_sessions = 0`, which keeps created tasks queued without a process host).
pub fn harness_configured(
    configure: impl FnOnce(&mut AppConfig) + 'static,
) -> Result<Harness, Box<dyn std::error::Error>> {
    harness_with(HarnessOptions {
        configure: Some(Box::new(configure)),
        ..HarnessOptions::default()
    })
}

/// Inserts a GitHub-linked user (fake GitHub token `token-<login>`) with a CLI token and
/// returns bearer credentials for it.
pub async fn user_with_token(
    h: &Harness,
    id: &str,
    login: &str,
    instance_admin: bool,
) -> Result<Creds, Box<dyn std::error::Error>> {
    use yulius_app::model::{ApiTokenRecord, GitHubTokenRecord, UserRecord};
    use yulius_app::ports::store::{SessionStore, UserStore};
    use yulius_domain::ids::UserId;
    let user_id = UserId::parse(id)?;
    h.store
        .insert_user(&UserRecord {
            id: user_id.clone(),
            github_login: Some(login.into()),
            github_id: None,
            display_name: login.into(),
            avatar_url: None,
            is_superadmin: false,
            is_instance_admin: instance_admin,
            password_hash: None,
            created_at: Timestamp(1),
            disabled_at: None,
        })
        .await?;
    let access = h
        .app
        .deps
        .crypto
        .seal(format!("token-{login}").as_bytes())?;
    h.store
        .save_github_token(
            &GitHubTokenRecord {
                user_id: user_id.clone(),
                access,
                refresh: None,
                expires_at: None,
                refresh_expires_at: None,
            },
            Timestamp(1),
        )
        .await?;
    let token = format!("yul_test_{login}");
    h.store
        .insert_api_token(&ApiTokenRecord {
            id: format!("tok-{id}"),
            user_id,
            token_hash: h.app.deps.crypto.sha256_hex(token.as_bytes()),
            label: "test".into(),
            created_at: Timestamp(1),
            revoked_at: None,
        })
        .await?;
    Ok(Creds {
        bearer: Some(token),
        ..Creds::default()
    })
}

/// Inserts an active sandbox for repository `Quartio-Studio/<slug>`.
pub async fn add_sandbox(
    h: &Harness,
    id: &str,
    slug: &str,
    port_base: u16,
) -> Result<(), Box<dyn std::error::Error>> {
    use yulius_app::model::SandboxRecord;
    use yulius_app::ports::store::SandboxStore;
    use yulius_domain::ids::{SandboxId, UserId};
    h.store
        .insert_sandbox(&SandboxRecord {
            id: SandboxId::parse(id)?,
            slug: slug.into(),
            repo_owner: "Quartio-Studio".into(),
            repo_name: slug.into(),
            github_repo_id: None,
            default_branch: "main".into(),
            profile_json: r#"{"preset":"studio","integration_branch":"quartio","production_branch":"main","merge":"squash","promote":"fast_forward","auto_promote":true,"environments":[]}"#.into(),
            port_base,
            port_count: 20,
            onboarding_state: "not_needed".into(),
            onboarding_task_id: None,
            created_by: UserId::parse("u-owner")?,
            created_at: Timestamp(1),
            removed_at: None,
        })
        .await?;
    Ok(())
}

/// GitHub permissions for an access level name (`read`, `write`, `admin`).
pub fn perms(level: &str) -> GitHubPermissions {
    GitHubPermissions {
        admin: level == "admin",
        maintain: false,
        push: level == "write",
        triage: false,
        pull: true,
    }
}

/// A configuration adjustment applied before the app is built.
pub type ConfigureFn = Box<dyn FnOnce(&mut AppConfig)>;

/// Adapters and data directory for [`harness_with`]; unset fields keep the [`harness`]
/// defaults (browser "unavailable", no process host, no git).
#[derive(Default)]
pub struct HarnessOptions {
    pub git: Option<Arc<dyn yulius_app::ports::git::GitPort>>,
    pub session_host: Option<Arc<dyn yulius_app::ports::session::SessionHost>>,
    pub browser: Option<Arc<dyn yulius_app::ports::browser::BrowserPort>>,
    /// Data directory (default: a new temporary directory). Unix socket paths are short, so
    /// tests with real session hosts pass a directory under `/tmp`.
    pub dir: Option<tempfile::TempDir>,
    /// Adjusts the configuration before the app is built.
    pub configure: Option<ConfigureFn>,
    /// Harness port (default: the real adapter with `/bin/false` hooks).
    pub harness: Option<Arc<dyn yulius_app::ports::harness::HarnessPort>>,
    /// Provider usage probe (default: unavailable).
    pub usage: Option<Arc<dyn yulius_app::ports::usage::UsageProbe>>,
    /// Store (default: a new in-memory database).
    pub store: Option<SqliteStore>,
}

/// Like [`harness`] with the given adapters. The data directory path is canonical.
pub fn harness_with(options: HarnessOptions) -> Result<Harness, Box<dyn std::error::Error>> {
    let dir = match options.dir {
        Some(dir) => dir,
        None => tempfile::tempdir()?,
    };
    let data = dir.path().canonicalize()?;
    let store = match options.store {
        Some(store) => store,
        None => SqliteStore::open_in_memory()?,
    };
    let github = Arc::new(FakeGitHub::default());
    let clock = Arc::new(TestClock(AtomicI64::new(1_791_331_200_000)));
    let mut config = AppConfig::new(
        data,
        "https://yulius.test",
        "http://127.0.0.1:1",
        "/bin/false".into(),
    );
    if let Some(configure) = options.configure {
        configure(&mut config);
    }
    let deps = Deps {
        store: Arc::new(store.clone()),
        clock: clock.clone(),
        crypto: Arc::new(MasterKeyCrypto::from_key(&[9u8; 32])?),
        github: github.clone(),
        session_host: options
            .session_host
            .unwrap_or_else(|| Arc::new(yulius::wiring::pending::UnavailableSessionHost)),
        harness: options.harness.unwrap_or_else(|| {
            Arc::new(yulius::wiring::harness::HarnessAdapter::new(
                "/bin/false".into(),
                "http://127.0.0.1:1",
            ))
        }),
        git: options
            .git
            .unwrap_or_else(|| Arc::new(yulius::wiring::pending::UnavailableGit)),
        browser: options
            .browser
            .unwrap_or_else(|| Arc::new(yulius::wiring::browser::UnavailableBrowser)),
        usage: options
            .usage
            .unwrap_or_else(|| Arc::new(yulius_app::ports::usage::NoUsageProbe)),
        system: Arc::new(FixedSystem),
    };
    let app = App::new(config, deps);
    let router = yulius_server::public_router(app.clone())
        .layer(MockConnectInfo(SocketAddr::from(([127, 0, 0, 1], 5000))));
    let internal = yulius_server::internal_router(app.clone())
        .layer(MockConnectInfo(SocketAddr::from(([127, 0, 0, 1], 5001))));
    Ok(Harness {
        app,
        internal,
        github,
        clock,
        store,
        router,
        _dir: dir,
    })
}

/// Seeds a GitHub-linked user (its stored GitHub token maps to `login` in [`FakeGitHub`]) and
/// returns bearer credentials for the public API. Grant repository access with
/// [`FakeGitHub::set_perm`].
pub async fn seed_github_user(
    h: &Harness,
    id: &str,
    login: &str,
) -> Result<Creds, Box<dyn std::error::Error>> {
    use yulius_app::model::{GitHubTokenRecord, UserRecord};
    use yulius_app::ports::store::UserStore;
    use yulius_domain::ids::UserId;
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
    h.store
        .save_github_token(
            &GitHubTokenRecord {
                user_id: user.id.clone(),
                access: h
                    .app
                    .deps
                    .crypto
                    .seal(format!("token-{login}").as_bytes())?,
                refresh: None,
                expires_at: None,
                refresh_expires_at: None,
            },
            h.app.now(),
        )
        .await?;
    let token = h.app.issue_api_token(&user, "tests").await?;
    Ok(Creds {
        bearer: Some(token.expose().to_owned()),
        ..Creds::default()
    })
}

/// Seeds a sandbox for the repository `acme/<slug>` (created by `created_by`).
pub async fn seed_sandbox(
    h: &Harness,
    id: &str,
    slug: &str,
    created_by: &str,
) -> Result<yulius_app::model::SandboxRecord, Box<dyn std::error::Error>> {
    use yulius_app::ports::store::SandboxStore;
    use yulius_domain::ids::{SandboxId, UserId};
    let port_base = h.store.next_port_base(43000, 20).await?;
    let sandbox = yulius_app::model::SandboxRecord {
        id: SandboxId::parse(id)?,
        slug: slug.into(),
        repo_owner: "acme".into(),
        repo_name: slug.into(),
        github_repo_id: Some(1),
        default_branch: "main".into(),
        profile_json: "{}".into(),
        port_base,
        port_count: 20,
        onboarding_state: "not_needed".into(),
        onboarding_task_id: None,
        created_by: UserId::parse(created_by)?,
        created_at: Timestamp(1),
        removed_at: None,
    };
    h.store.insert_sandbox(&sandbox).await?;
    Ok(sandbox)
}

/// A real listener on `127.0.0.1:<OS port>` serving `router`; aborted on drop.
pub struct Served {
    pub addr: SocketAddr,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for Served {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// Serves `router` on an OS-assigned loopback port.
pub async fn serve(router: Router) -> Result<Served, Box<dyn std::error::Error>> {
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await?;
    let addr = listener.local_addr()?;
    let task = tokio::spawn(async move {
        let _ = axum::serve(listener, router).await;
    });
    Ok(Served { addr, task })
}

/// Opens a WebSocket to `path` with bearer credentials and an `Origin` header.
pub async fn ws_connect(
    addr: SocketAddr,
    path: &str,
    creds: &Creds,
    origin: &str,
) -> Result<
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>,
    tokio_tungstenite::tungstenite::Error,
> {
    use tokio_tungstenite::tungstenite::client::IntoClientRequest;
    use tokio_tungstenite::tungstenite::http::HeaderValue;
    let mut request = format!("ws://{addr}{path}").into_client_request()?;
    if let Some(token) = &creds.bearer
        && let Ok(value) = HeaderValue::from_str(&format!("Bearer {token}"))
    {
        request.headers_mut().insert("authorization", value);
    }
    if let Ok(value) = HeaderValue::from_str(origin) {
        request.headers_mut().insert("origin", value);
    }
    let (socket, _) = tokio_tungstenite::connect_async(request).await?;
    Ok(socket)
}

/// HTTP status of a refused WebSocket handshake.
pub fn ws_refusal_status(error: &tokio_tungstenite::tungstenite::Error) -> Option<u16> {
    match error {
        tokio_tungstenite::tungstenite::Error::Http(response) => Some(response.status().as_u16()),
        _ => None,
    }
}

/// Seeds a task of `sandbox` created by `created_by` (optionally with a worktree path).
pub async fn seed_task(
    h: &Harness,
    id: &str,
    sandbox: &str,
    created_by: &str,
    status: yulius_domain::task::TaskStatus,
    worktree: Option<&std::path::Path>,
) -> Result<yulius_app::model::TaskRecord, Box<dyn std::error::Error>> {
    use yulius_app::ports::store::TaskStore;
    use yulius_domain::ids::{SandboxId, TaskId, UserId};
    let now = h.app.now();
    let task = yulius_app::model::TaskRecord {
        id: TaskId::parse(id)?,
        sandbox_id: SandboxId::parse(sandbox)?,
        number: 0,
        title: format!("task {id}"),
        prompt: "do things".into(),
        harness: yulius_domain::harness::Harness::ClaudeCode,
        model: None,
        effort: None,
        workflow_id: None,
        status,
        activity: None,
        branch: Some(format!("feat/{id}")),
        worktree_path: worktree.map(|p| p.to_string_lossy().into_owned()),
        spec_path: None,
        origin: yulius_app::model::TaskOrigin::Ui,
        automation_run: None,
        created_by: UserId::parse(created_by)?,
        created_at: now,
        updated_at: now,
        closed_at: None,
        archived_at: None,
        queued_at: None,
        gate_blocks: 0,
        wake_enabled: true,
        mode: yulius_app::model::TaskMode::Prompt,
    };
    Ok(h.store.insert_task(&task).await?)
}

/// Seeds a live attempt for `task` whose MCP task token is `token`.
pub async fn seed_live_attempt(
    h: &Harness,
    task: &str,
    token: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    use yulius_app::ports::store::TaskStore;
    use yulius_domain::ids::{AttemptId, TaskId};
    h.store
        .insert_attempt(&yulius_app::model::AttemptRecord {
            id: AttemptId::parse(&format!("a-{task}"))?,
            task_id: TaskId::parse(task)?,
            seq: 1,
            harness: yulius_domain::harness::Harness::ClaudeCode,
            account_id: None,
            provider_session_id: None,
            launch_kind: yulius_app::model::LaunchKind::New,
            state: yulius_domain::task::AttemptState::Live,
            run_dir: "/nonexistent/run".into(),
            token_hash: h.app.deps.crypto.sha256_hex(token.as_bytes()),
            host_token: h.app.deps.crypto.seal(&[1u8; 32])?,
            ptyhost_pid: None,
            child_pid: None,
            started_at: h.app.now(),
            ended_at: None,
            exit_code: None,
            exit_reason: None,
        })
        .await?;
    Ok(())
}

/// Calls an MCP tool with a task token; returns the `tools/call` result object.
pub async fn mcp_tool(
    h: &Harness,
    token: &str,
    name: &str,
    args: serde_json::Value,
) -> Result<serde_json::Value, Box<dyn std::error::Error>> {
    let r = call(
        &h.internal,
        Method::POST,
        "/mcp",
        Some(
            serde_json::json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call",
            "params": {"name": name, "arguments": args}}),
        ),
        &Creds {
            bearer: Some(token.into()),
            ..Creds::default()
        },
    )
    .await?;
    if r.status != StatusCode::OK {
        return Err(format!("MCP HTTP status {}", r.status).into());
    }
    Ok(r.body["result"].clone())
}

/// The first text item of a tool result parsed as JSON (or as a string when it is not JSON).
pub fn tool_text(result: &serde_json::Value) -> serde_json::Value {
    let text = result["content"][0]["text"].as_str().unwrap_or_default();
    serde_json::from_str(text).unwrap_or_else(|_| serde_json::Value::String(text.to_owned()))
}
