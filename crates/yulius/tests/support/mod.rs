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
    async fn create_pull(&self, _: &Secret, _: &str, _: &str, _: &NewPull) -> AppResult<GhPull> {
        Err(AppError::Upstream("n/a".into()))
    }
    async fn get_pull(&self, _: &Secret, _: &str, _: &str, _: u64) -> AppResult<GhPull> {
        Err(AppError::Upstream("n/a".into()))
    }
    async fn list_pulls(
        &self,
        _: &Secret,
        _: &str,
        _: &str,
        _: Option<&str>,
    ) -> AppResult<Vec<GhPull>> {
        Ok(Vec::new())
    }
    async fn checks(&self, _: &Secret, _: &str, _: &str, _: &str) -> AppResult<ChecksState> {
        Ok(ChecksState::None)
    }
    async fn merge_pull(
        &self,
        _: &Secret,
        _: &str,
        _: &str,
        _: u64,
        _: &str,
        _: &str,
    ) -> AppResult<String> {
        Err(AppError::Upstream("n/a".into()))
    }
    async fn fast_forward(&self, _: &Secret, _: &str, _: &str, _: &str, _: &str) -> AppResult<()> {
        Err(AppError::Upstream("n/a".into()))
    }
    async fn branch_sha(&self, _: &Secret, _: &str, _: &str, _: &str) -> AppResult<Option<String>> {
        Ok(None)
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
        browser: Arc::new(yulius::wiring::browser::UnavailableBrowser),
        editor: Arc::new(yulius::wiring::editor::UnavailableEditor::default()),
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
