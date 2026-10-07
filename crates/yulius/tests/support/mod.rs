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
        session_host,
        harness: Arc::new(yulius::wiring::harness::HarnessAdapter::new(
            "/bin/false".into(),
            "http://127.0.0.1:1",
        )),
        git,
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

/// Like [`harness`] but lets a test adjust the configuration first (for example
/// `max_live_sessions = 0`, which keeps created tasks queued without a process host).
pub fn harness_with(
    configure: impl FnOnce(&mut AppConfig),
) -> Result<Harness, Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let store = SqliteStore::open_in_memory()?;
    let github = Arc::new(FakeGitHub::default());
    let clock = Arc::new(TestClock(AtomicI64::new(1_791_331_200_000)));
    let mut config = AppConfig::new(
        dir.path().to_path_buf(),
        "https://yulius.test",
        "http://127.0.0.1:1",
        "/bin/false".into(),
    );
    configure(&mut config);
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
        git: Arc::new(yulius::wiring::pending::UnavailableGit),
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
