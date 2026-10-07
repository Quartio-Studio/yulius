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

/// Adapters and data directory for [`harness_with`]; unset fields keep the [`harness`]
/// defaults (browser and editor "unavailable", no process host, no git).
#[derive(Default)]
pub struct HarnessOptions {
    pub git: Option<Arc<dyn yulius_app::ports::git::GitPort>>,
    pub session_host: Option<Arc<dyn yulius_app::ports::session::SessionHost>>,
    pub browser: Option<Arc<dyn yulius_app::ports::browser::BrowserPort>>,
    pub editor: Option<Arc<dyn yulius_app::ports::editor::EditorPort>>,
    /// Data directory (default: a new temporary directory). Unix socket paths are short, so
    /// tests with real session hosts pass a directory under `/tmp`.
    pub dir: Option<tempfile::TempDir>,
}

/// Like [`harness`] with the given adapters. The data directory path is canonical.
pub fn harness_with(options: HarnessOptions) -> Result<Harness, Box<dyn std::error::Error>> {
    let dir = match options.dir {
        Some(dir) => dir,
        None => tempfile::tempdir()?,
    };
    let data = dir.path().canonicalize()?;
    let store = SqliteStore::open_in_memory()?;
    let github = Arc::new(FakeGitHub::default());
    let clock = Arc::new(TestClock(AtomicI64::new(1_791_331_200_000)));
    let config = AppConfig::new(
        data,
        "https://yulius.test",
        "http://127.0.0.1:1",
        "/bin/false".into(),
    );
    let deps = Deps {
        store: Arc::new(store.clone()),
        clock: clock.clone(),
        crypto: Arc::new(MasterKeyCrypto::from_key(&[9u8; 32])?),
        github: github.clone(),
        session_host: options
            .session_host
            .unwrap_or_else(|| Arc::new(yulius::wiring::pending::UnavailableSessionHost)),
        harness: Arc::new(yulius::wiring::harness::HarnessAdapter {
            yulius_exe: "/bin/false".into(),
            yulius_url: "http://127.0.0.1:1".into(),
        }),
        git: options
            .git
            .unwrap_or_else(|| Arc::new(yulius::wiring::pending::UnavailableGit)),
        browser: options
            .browser
            .unwrap_or_else(|| Arc::new(yulius::wiring::browser::UnavailableBrowser)),
        editor: options
            .editor
            .unwrap_or_else(|| Arc::new(yulius::wiring::editor::UnavailableEditor::default())),
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

/// GitHub permissions for a level name (`read`, `write`, `admin`).
#[must_use]
pub fn perms(level: &str) -> GitHubPermissions {
    GitHubPermissions {
        admin: level == "admin",
        push: level == "write" || level == "admin",
        pull: true,
        ..GitHubPermissions::default()
    }
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
