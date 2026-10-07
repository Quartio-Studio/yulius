//! Test harness: in-memory app with a fake GitHub and a controllable clock.
// Shared by several test binaries; each uses a subset, so unused items are expected here.
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
    ChecksState, GhPull, GhRepo, GhUser, GhUserToken, GitHubAppConfig, GitHubPort, NewPull,
    RepoAccess, RulesetRequest,
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
    /// Check state per commit SHA (default `None`).
    pub check_states: Mutex<HashMap<String, ChecksState>>,
    /// Repository side of PRs and refs; without it PR heads are unknown.
    pub remote: Mutex<Option<Arc<dyn FakeRemote>>>,
    /// Recorded state-changing calls (`create_pull …`, `merge_pull …`, `fast_forward …`).
    pub writes: Mutex<Vec<String>>,
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
    fn authorize_url(&self, _: &GitHubAppConfig, state: &str, _: &str) -> String {
        format!("https://github.test/login/oauth/authorize?state={state}")
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
    async fn convert_manifest(&self, _: &str) -> AppResult<GitHubAppConfig> {
        Err(AppError::Upstream("n/a".into()))
    }
    async fn create_pull(
        &self,
        _: &Secret,
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
        };
        pulls.push(created.clone());
        self.record(format!(
            "create_pull #{number} {}<-{}",
            pull.base, pull.head
        ));
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
    async fn checks(&self, _: &Secret, _: &str, _: &str, sha: &str) -> AppResult<ChecksState> {
        Ok(self
            .check_states
            .lock()
            .map_err(AppError::internal)?
            .get(sha)
            .copied()
            .unwrap_or(ChecksState::None))
    }
    async fn merge_pull(
        &self,
        _: &Secret,
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
        self.record(format!("merge_pull #{number} {method}"));
        Ok(sha)
    }
    async fn fast_forward(
        &self,
        _: &Secret,
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
        self.record(format!("fast_forward {branch} {sha}"));
        Ok(())
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
    pub github: Arc<FakeGitHub>,
    pub clock: Arc<TestClock>,
    pub store: SqliteStore,
    pub router: Router,
    pub _dir: tempfile::TempDir,
}

pub fn harness() -> Result<Harness, Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let store = SqliteStore::open_in_memory()?;
    let github = Arc::new(FakeGitHub::default());
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
        harness: Arc::new(yulius::wiring::harness::HarnessAdapter {
            yulius_exe: "/bin/false".into(),
            yulius_url: "http://127.0.0.1:1".into(),
        }),
        git: Arc::new(yulius::wiring::pending::UnavailableGit),
    };
    let app = App::new(config, deps);
    let router = yulius_server::public_router(app.clone())
        .layer(MockConnectInfo(SocketAddr::from(([127, 0, 0, 1], 5000))));
    Ok(Harness {
        app,
        github,
        clock,
        store,
        router,
        _dir: dir,
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
        req = req.header(header::COOKIE, format!("yulius_session={cookie}"));
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

/// Extracts the session value from a Set-Cookie header.
pub fn session_from(set_cookie: &Option<String>) -> Option<String> {
    let raw = set_cookie.as_ref()?;
    let first = raw.split(';').next()?;
    first.strip_prefix("yulius_session=").map(str::to_owned)
}

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
    fn record(&self, call: String) {
        if let Ok(mut writes) = self.writes.lock() {
            writes.push(call);
        }
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
        if let Ok(mut m) = self.check_states.lock() {
            m.insert(sha.into(), state);
        }
    }
    pub fn writes(&self) -> Vec<String> {
        self.writes.lock().map(|w| w.clone()).unwrap_or_default()
    }
    pub fn pull(&self, number: u64) -> Option<GhPull> {
        self.pulls
            .lock()
            .ok()
            .and_then(|p| p.iter().find(|p| p.number == number).cloned())
    }
}

/// [`harness`] with a custom Git port (for example a test-local adapter over real `git`).
pub fn harness_with_git(
    git: Arc<dyn yulius_app::ports::git::GitPort>,
) -> Result<Harness, Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let store = SqliteStore::open_in_memory()?;
    let github = Arc::new(FakeGitHub::default());
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
        harness: Arc::new(yulius::wiring::harness::HarnessAdapter {
            yulius_exe: "/bin/false".into(),
            yulius_url: "http://127.0.0.1:1".into(),
        }),
        git,
    };
    let app = App::new(config, deps);
    let router = yulius_server::public_router(app.clone())
        .layer(MockConnectInfo(SocketAddr::from(([127, 0, 0, 1], 5000))));
    Ok(Harness {
        app,
        github,
        clock,
        store,
        router,
        _dir: dir,
    })
}
