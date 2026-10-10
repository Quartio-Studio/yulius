//! In-process fake GitHub for tests (`fake` feature).
//!
//! [`FakeGitHub::start`] serves the subset of the GitHub REST API, OAuth and App endpoints
//! that [`crate::GitHubClient`] uses, on `127.0.0.1:0`. The API lives under `/api/` and the
//! web (OAuth) endpoints under `/web/` of the same server, which also exercises base URLs
//! with a path (as on GitHub Enterprise Server). Tests configure users, tokens,
//! installations, repositories, permissions, commits, branches, pull requests, check runs,
//! statuses and Actions workflow runs; withhold App permissions; inject faults; and inspect
//! every recorded request. All data is synthetic, including the App key in
//! `synthetic_app_key.pem`.
//!
//! App JWTs are verified for real (RS256 signature, issuer, `iat`/`exp` window) against the
//! fake App's key, so a client that signs incorrectly fails against the fake.
//!
//! The server stops when [`FakeGitHub::shutdown`] is awaited or the value is dropped.

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::net::SocketAddr;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use axum::Router;
use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, Method, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use reqwest::Url;
use ring::signature::RsaKeyPair;
use serde_json::{Value, json};
use tokio::sync::oneshot;
use tokio::task::JoinHandle;

use crate::config::{AppCredentials, GitHubConfig, OwnerKind, RetryPolicy};
use crate::jwt::{parse_rsa_key, verify};
use crate::secret::Secret;
use crate::time::format_rfc3339;
use crate::types::PullState;
use yulius_domain::permission::GitHubPermissions;

/// The synthetic RSA key of the fake App (test fixture, not a secret).
pub const FAKE_APP_KEY_PEM: &str = include_str!("synthetic_app_key.pem");
/// App id of the fake App.
pub const FAKE_APP_ID: u64 = 4242;
/// Client id of the fake App.
pub const FAKE_CLIENT_ID: &str = "Iv1.fakeyuliusapp";
/// Client secret of the fake App (synthetic).
pub const FAKE_CLIENT_SECRET: &str = "fake-client-secret-0123456789";
/// Slug of the fake App.
pub const FAKE_APP_SLUG: &str = "yulius-fake";
/// Lifetime of issued installation tokens.
pub const INSTALLATION_TOKEN_SECS: i64 = 3600;
/// Lifetime of issued user access tokens.
pub const USER_TOKEN_SECS: i64 = 8 * 3600;
/// Lifetime of issued refresh tokens.
pub const REFRESH_TOKEN_SECS: i64 = 184 * 24 * 3600;
/// Requests kept in the log (oldest dropped first).
pub const MAX_RECORDED_REQUESTS: usize = 2048;

/// One request as the fake received it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordedRequest {
    /// HTTP method.
    pub method: String,
    /// `true` for `/web/...` (OAuth) requests, `false` for `/api/...`.
    pub web: bool,
    /// Path relative to the API or web root, e.g. `/repos/acme/app`.
    pub path: String,
    /// Raw query string.
    pub query: Option<String>,
    /// Headers (lower-case names).
    pub headers: Vec<(String, String)>,
    /// Body as (lossy) text.
    pub body: String,
}

impl RecordedRequest {
    /// Value of header `name` (case-insensitive).
    #[must_use]
    pub fn header(&self, name: &str) -> Option<&str> {
        let name = name.to_ascii_lowercase();
        self.headers
            .iter()
            .find(|(key, _)| *key == name)
            .map(|(_, value)| value.as_str())
    }
}

/// A canned failure returned instead of the normal response.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FakeFault {
    /// Method to match (e.g. `GET`).
    pub method: String,
    /// Path relative to the API or web root (e.g. `/repos/acme/app`).
    pub path: String,
    /// Status to return.
    pub status: u16,
    /// How many matching requests receive the fault.
    pub times: u32,
    /// Extra response headers (e.g. `retry-after`).
    pub headers: Vec<(String, String)>,
    /// `message` field of the JSON body.
    pub message: String,
}

impl FakeFault {
    /// A fault answering `times` matching requests with `status`.
    #[must_use]
    pub fn new(method: &str, path: impl Into<String>, status: u16, times: u32) -> Self {
        Self {
            method: method.to_ascii_uppercase(),
            path: path.into(),
            status,
            times,
            headers: Vec::new(),
            message: format!("injected {status}"),
        }
    }

    /// Adds a response header.
    #[must_use]
    pub fn header(mut self, name: &str, value: &str) -> Self {
        self.headers.push((name.to_owned(), value.to_owned()));
        self
    }

    /// Sets the error message.
    #[must_use]
    pub fn message(mut self, message: impl Into<String>) -> Self {
        self.message = message.into();
        self
    }
}

/// A check run attached to a commit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FakeCheckRun {
    /// Name.
    pub name: String,
    /// `queued`, `in_progress` or `completed`.
    pub status: String,
    /// Conclusion when completed (`success`, `failure`, `neutral`, …).
    pub conclusion: Option<String>,
}

impl FakeCheckRun {
    /// A run with `status` and optional `conclusion`.
    #[must_use]
    pub fn new(name: &str, status: &str, conclusion: Option<&str>) -> Self {
        Self {
            name: name.to_owned(),
            status: status.to_owned(),
            conclusion: conclusion.map(str::to_owned),
        }
    }
}

/// A commit status attached to a commit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FakeStatus {
    /// Context name.
    pub context: String,
    /// `success`, `pending`, `failure` or `error`.
    pub state: String,
}

impl FakeStatus {
    /// A status for `context`.
    #[must_use]
    pub fn new(context: &str, state: &str) -> Self {
        Self {
            context: context.to_owned(),
            state: state.to_owned(),
        }
    }
}

#[derive(Debug, Clone)]
struct FakePull {
    number: u64,
    title: String,
    body: String,
    head: String,
    base: String,
    state: PullState,
    mergeable: bool,
    merged_head_sha: Option<String>,
    merged_base_sha: Option<String>,
    merge_commit_sha: Option<String>,
}

#[derive(Debug, Default)]
struct FakeRepo {
    id: u64,
    owner: String,
    name: String,
    private: bool,
    default_branch: String,
    permissions: HashMap<String, GitHubPermissions>,
    branches: BTreeMap<String, String>,
    commits: HashMap<String, Vec<String>>,
    pulls: Vec<FakePull>,
    check_runs: HashMap<String, Vec<FakeCheckRun>>,
    statuses: HashMap<String, Vec<FakeStatus>>,
    workflow_runs: HashMap<String, Vec<FakeCheckRun>>,
    rulesets: Vec<Value>,
}

#[derive(Debug, Clone)]
struct FakeUser {
    id: u64,
    login: String,
    name: Option<String>,
}

struct FakeState {
    base: String,
    /// Seconds added to the system clock (see [`FakeGitHub::advance_time`]).
    time_offset_secs: i64,
    app_key: RsaKeyPair,
    next_id: u64,
    users: HashMap<String, FakeUser>,
    user_tokens: HashMap<String, String>,
    refresh_tokens: HashMap<String, String>,
    oauth_codes: HashMap<String, String>,
    manifest_codes: HashSet<String>,
    installations: BTreeMap<u64, (String, OwnerKind)>,
    installation_tokens: HashMap<String, u64>,
    repos: HashMap<(String, String), FakeRepo>,
    /// App permissions the App does not have (`checks`, `statuses`, `actions`): their
    /// endpoints answer 403 "Resource not accessible by integration" to every token.
    withheld: HashSet<String>,
    faults: Vec<FakeFault>,
    requests: VecDeque<RecordedRequest>,
}

type Shared = Arc<Mutex<FakeState>>;

/// The running fake GitHub server.
pub struct FakeGitHub {
    addr: SocketAddr,
    api_base: Url,
    web_base: Url,
    state: Shared,
    shutdown: Option<oneshot::Sender<()>>,
    task: Option<JoinHandle<()>>,
}

impl std::fmt::Debug for FakeGitHub {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FakeGitHub")
            .field("addr", &self.addr)
            .finish()
    }
}

fn lock(state: &Shared) -> MutexGuard<'_, FakeState> {
    state
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn key(owner: &str, name: &str) -> (String, String) {
    (owner.to_ascii_lowercase(), name.to_ascii_lowercase())
}

fn now_secs(state: &FakeState) -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
        .unwrap_or(0)
        .saturating_add(state.time_offset_secs)
}

impl FakeGitHub {
    /// Starts the server on `127.0.0.1` with an OS-assigned port.
    ///
    /// # Errors
    /// Binding fails, or the embedded fixture key cannot be parsed.
    pub async fn start() -> std::io::Result<Self> {
        let app_key = parse_rsa_key(FAKE_APP_KEY_PEM)
            .map_err(|_| std::io::Error::other("fake App key fixture is invalid"))?;
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await?;
        let addr = listener.local_addr()?;
        let base_url = |path: &str| {
            Url::parse(&format!("http://{addr}/{path}/")).map_err(std::io::Error::other)
        };
        let (api_base, web_base) = (base_url("api")?, base_url("web")?);
        let state = Arc::new(Mutex::new(FakeState {
            base: format!("http://{addr}"),
            time_offset_secs: 0,
            app_key,
            next_id: 1000,
            users: HashMap::new(),
            user_tokens: HashMap::new(),
            refresh_tokens: HashMap::new(),
            oauth_codes: HashMap::new(),
            manifest_codes: HashSet::new(),
            installations: BTreeMap::new(),
            installation_tokens: HashMap::new(),
            repos: HashMap::new(),
            withheld: HashSet::new(),
            faults: Vec::new(),
            requests: VecDeque::new(),
        }));
        let router = Router::new()
            .fallback(handle)
            .with_state(Arc::clone(&state));
        let (tx, rx) = oneshot::channel::<()>();
        let task = tokio::spawn(async move {
            let _ = axum::serve(listener, router)
                .with_graceful_shutdown(async move {
                    let _ = rx.await;
                })
                .await;
        });
        Ok(Self {
            addr,
            api_base,
            web_base,
            state,
            shutdown: Some(tx),
            task: Some(task),
        })
    }

    /// Stops the server and waits for it to finish.
    pub async fn shutdown(mut self) {
        if let Some(tx) = self.shutdown.take() {
            let _ = tx.send(());
        }
        if let Some(task) = self.task.take() {
            let _ = tokio::time::timeout(Duration::from_secs(5), task).await;
        }
    }

    /// The socket address.
    #[must_use]
    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    /// API base URL (`http://127.0.0.1:<port>/api/`).
    #[must_use]
    pub fn api_base(&self) -> Url {
        self.api_base.clone()
    }

    /// Web base URL (`http://127.0.0.1:<port>/web/`).
    #[must_use]
    pub fn web_base(&self) -> Url {
        self.web_base.clone()
    }

    /// Credentials of the fake App.
    #[must_use]
    pub fn app_credentials(&self) -> AppCredentials {
        AppCredentials {
            app_id: FAKE_APP_ID,
            client_id: FAKE_CLIENT_ID.to_owned(),
            client_secret: Secret::new(FAKE_CLIENT_SECRET),
            private_key_pem: Secret::new(FAKE_APP_KEY_PEM),
            slug: FAKE_APP_SLUG.to_owned(),
            webhook_secret: None,
        }
    }

    /// A client configuration pointing at this server with the fake App and a fast retry
    /// policy (3 attempts, 10 ms base delay, 5 s budget).
    #[must_use]
    pub fn config(&self) -> GitHubConfig {
        GitHubConfig {
            api_base: self.api_base(),
            web_base: self.web_base(),
            app: Some(self.app_credentials()),
            retry: RetryPolicy {
                max_attempts: 3,
                max_total: Duration::from_secs(5),
                base_delay: Duration::from_millis(10),
            },
            request_timeout: Duration::from_secs(5),
        }
    }

    fn next_id(state: &mut FakeState) -> u64 {
        state.next_id += 1;
        state.next_id
    }

    /// Adds a user and returns its id.
    pub fn add_user(&self, login: &str, name: Option<&str>) -> u64 {
        let mut state = lock(&self.state);
        let id = Self::next_id(&mut state);
        state.users.insert(
            login.to_ascii_lowercase(),
            FakeUser {
                id,
                login: login.to_owned(),
                name: name.map(str::to_owned),
            },
        );
        id
    }

    /// Issues a user access token for `login` directly (the user must exist).
    pub fn issue_user_token(&self, login: &str) -> Secret {
        let mut state = lock(&self.state);
        let id = Self::next_id(&mut state);
        let token = format!("ghu_{id:036}");
        state
            .user_tokens
            .insert(token.clone(), login.to_ascii_lowercase());
        Secret::new(token)
    }

    /// Makes `code` redeemable once at the OAuth token endpoint for `login`.
    pub fn add_oauth_code(&self, code: &str, login: &str) {
        lock(&self.state)
            .oauth_codes
            .insert(code.to_owned(), login.to_ascii_lowercase());
    }

    /// Makes `code` convertible once into the fake App's credentials.
    pub fn add_manifest_code(&self, code: &str) {
        lock(&self.state).manifest_codes.insert(code.to_owned());
    }

    /// Installs the fake App on `account` and returns the installation id.
    pub fn add_installation(&self, account: &str, kind: OwnerKind) -> u64 {
        let mut state = lock(&self.state);
        let id = Self::next_id(&mut state);
        state.installations.insert(id, (account.to_owned(), kind));
        id
    }

    /// Adds a repository with one commit on `main`; returns that commit id.
    pub fn add_repo(&self, owner: &str, name: &str, private: bool) -> String {
        let mut state = lock(&self.state);
        let id = Self::next_id(&mut state);
        let sha = fake_sha(Self::next_id(&mut state));
        let mut repo = FakeRepo {
            id,
            owner: owner.to_owned(),
            name: name.to_owned(),
            private,
            default_branch: "main".to_owned(),
            ..FakeRepo::default()
        };
        repo.commits.insert(sha.clone(), Vec::new());
        repo.branches.insert("main".to_owned(), sha.clone());
        state.repos.insert(key(owner, name), repo);
        sha
    }

    /// Sets `login`'s permission flags on a repository.
    pub fn set_permission(
        &self,
        owner: &str,
        repo: &str,
        login: &str,
        permissions: GitHubPermissions,
    ) {
        if let Some(repo) = lock(&self.state).repos.get_mut(&key(owner, repo)) {
            repo.permissions
                .insert(login.to_ascii_lowercase(), permissions);
        }
    }

    /// Adds a commit with `parents` and returns its generated id.
    pub fn add_commit(&self, owner: &str, repo: &str, parents: &[&str]) -> String {
        let mut state = lock(&self.state);
        let sha = fake_sha(Self::next_id(&mut state));
        if let Some(repo) = state.repos.get_mut(&key(owner, repo)) {
            repo.commits.insert(
                sha.clone(),
                parents.iter().map(|p| (*p).to_owned()).collect(),
            );
        }
        sha
    }

    /// Points `branch` at `sha` (creating the branch when needed).
    pub fn set_branch(&self, owner: &str, repo: &str, branch: &str, sha: &str) {
        if let Some(repo) = lock(&self.state).repos.get_mut(&key(owner, repo)) {
            repo.branches.insert(branch.to_owned(), sha.to_owned());
        }
    }

    /// Current tip of `branch`.
    #[must_use]
    pub fn branch_sha(&self, owner: &str, repo: &str, branch: &str) -> Option<String> {
        lock(&self.state)
            .repos
            .get(&key(owner, repo))
            .and_then(|r| r.branches.get(branch).cloned())
    }

    /// Whether GitHub would allow merging pull request `number` (default true).
    pub fn set_pull_mergeable(&self, owner: &str, repo: &str, number: u64, mergeable: bool) {
        if let Some(repo) = lock(&self.state).repos.get_mut(&key(owner, repo))
            && let Some(pull) = repo.pulls.iter_mut().find(|p| p.number == number)
        {
            pull.mergeable = mergeable;
        }
    }

    /// Replaces the check runs of commit `sha`.
    pub fn set_check_runs(&self, owner: &str, repo: &str, sha: &str, runs: Vec<FakeCheckRun>) {
        if let Some(repo) = lock(&self.state).repos.get_mut(&key(owner, repo)) {
            repo.check_runs.insert(sha.to_owned(), runs);
        }
    }

    /// Replaces the GitHub Actions workflow runs whose head is commit `sha`.
    pub fn set_workflow_runs(&self, owner: &str, repo: &str, sha: &str, runs: Vec<FakeCheckRun>) {
        if let Some(repo) = lock(&self.state).repos.get_mut(&key(owner, repo)) {
            repo.workflow_runs.insert(sha.to_owned(), runs);
        }
    }

    /// Takes App permission `permission` (`checks`, `statuses` or `actions`) away from the App:
    /// its read endpoints (check runs, the combined status, workflow runs) then answer 403
    /// "Resource not accessible by integration", as GitHub does for installation and
    /// user-to-server tokens of an App without it.
    pub fn withhold_permission(&self, permission: &str) {
        lock(&self.state).withheld.insert(permission.to_owned());
    }

    /// Replaces the commit statuses of commit `sha`.
    pub fn set_statuses(&self, owner: &str, repo: &str, sha: &str, statuses: Vec<FakeStatus>) {
        if let Some(repo) = lock(&self.state).repos.get_mut(&key(owner, repo)) {
            repo.statuses.insert(sha.to_owned(), statuses);
        }
    }

    /// Ruleset bodies created on a repository.
    #[must_use]
    pub fn rulesets(&self, owner: &str, repo: &str) -> Vec<Value> {
        lock(&self.state)
            .repos
            .get(&key(owner, repo))
            .map(|r| r.rulesets.clone())
            .unwrap_or_default()
    }

    /// Whether the repository exists.
    #[must_use]
    pub fn has_repo(&self, owner: &str, repo: &str) -> bool {
        lock(&self.state).repos.contains_key(&key(owner, repo))
    }

    /// Moves the fake's clock forward by `by` (JWT validation and issued expiries), so tests
    /// can advance an injected client clock and the server together.
    pub fn advance_time(&self, by: Duration) {
        let secs = i64::try_from(by.as_secs()).unwrap_or(i64::MAX);
        let mut state = lock(&self.state);
        state.time_offset_secs = state.time_offset_secs.saturating_add(secs);
    }

    /// Queues a fault.
    pub fn inject_fault(&self, fault: FakeFault) {
        lock(&self.state).faults.push(fault);
    }

    /// Requests received so far (oldest first, at most [`MAX_RECORDED_REQUESTS`]).
    #[must_use]
    pub fn requests(&self) -> Vec<RecordedRequest> {
        lock(&self.state).requests.iter().cloned().collect()
    }

    /// Number of recorded requests with `method` and relative `path`.
    #[must_use]
    pub fn count(&self, method: &str, path: &str) -> usize {
        lock(&self.state)
            .requests
            .iter()
            .filter(|r| r.method == method && r.path == path)
            .count()
    }

    /// Clears the request log.
    pub fn clear_requests(&self) {
        lock(&self.state).requests.clear();
    }
}

impl Drop for FakeGitHub {
    fn drop(&mut self) {
        if let Some(tx) = self.shutdown.take() {
            let _ = tx.send(());
        }
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}

fn fake_sha(n: u64) -> String {
    format!("fa4e{n:036x}")
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn pct_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let (Some(high), Some(low)) = (hex_value(bytes[i + 1]), hex_value(bytes[i + 2]))
        {
            out.push(high * 16 + low);
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn reply(status: u16, body: Value) -> Response {
    let code = StatusCode::from_u16(status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    (code, axum::Json(body)).into_response()
}

fn error(status: u16, message: &str) -> Response {
    reply(
        status,
        json!({"message": message, "documentation_url": "https://docs.github.com/rest"}),
    )
}

fn query_pairs(query: Option<&str>) -> HashMap<String, String> {
    let Some(query) = query else {
        return HashMap::new();
    };
    Url::parse(&format!("http://fake/?{query}"))
        .map(|url| url.query_pairs().into_owned().collect())
        .unwrap_or_default()
}

fn paginate<T: Clone>(items: &[T], query: &HashMap<String, String>) -> Vec<T> {
    let per_page = query
        .get("per_page")
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(30)
        .clamp(1, 100);
    let page = query
        .get("page")
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(1)
        .max(1);
    items
        .iter()
        .skip((page - 1) * per_page)
        .take(per_page)
        .cloned()
        .collect()
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Identity {
    Anonymous,
    Invalid,
    App,
    Installation(u64),
    User(String),
}

fn identify(state: &FakeState, headers: &HeaderMap) -> Identity {
    let Some(value) = headers.get("authorization").and_then(|v| v.to_str().ok()) else {
        return Identity::Anonymous;
    };
    let token = value
        .strip_prefix("Bearer ")
        .or_else(|| value.strip_prefix("token "))
        .unwrap_or_default();
    if token.matches('.').count() == 2 {
        let Some(claims) = verify(token, &state.app_key) else {
            return Identity::Invalid;
        };
        let now = now_secs(state);
        let app_id = FAKE_APP_ID.to_string();
        let issuer = claims["iss"].as_str().unwrap_or_default();
        let issuer_ok = issuer == FAKE_CLIENT_ID
            || issuer == app_id
            || claims["iss"].as_u64() == Some(FAKE_APP_ID);
        let iat = claims["iat"].as_i64().unwrap_or(i64::MAX);
        let exp = claims["exp"].as_i64().unwrap_or(0);
        let window_ok = iat <= now + 60 && exp > now && exp - iat <= 600 && exp <= now + 600;
        return if issuer_ok && window_ok {
            Identity::App
        } else {
            Identity::Invalid
        };
    }
    if let Some(id) = state.installation_tokens.get(token) {
        return Identity::Installation(*id);
    }
    if let Some(login) = state.user_tokens.get(token) {
        return Identity::User(login.clone());
    }
    Identity::Invalid
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Need {
    Read,
    Write,
    Admin,
}

fn installation_covers(state: &FakeState, installation: u64, owner: &str) -> bool {
    state
        .installations
        .get(&installation)
        .is_some_and(|(account, _)| account.eq_ignore_ascii_case(owner))
}

/// `Ok(Some(perms))` for a user, `Ok(None)` for an installation, `Err((status, message))` otherwise.
fn authorize(
    state: &FakeState,
    identity: &Identity,
    repo: &FakeRepo,
    need: Need,
) -> Result<Option<GitHubPermissions>, (u16, &'static str)> {
    match identity {
        Identity::Anonymous | Identity::Invalid | Identity::App => Err((401, "Bad credentials")),
        Identity::Installation(id) => {
            if installation_covers(state, *id, &repo.owner) {
                Ok(None)
            } else {
                Err((404, "Not Found"))
            }
        }
        Identity::User(login) => {
            let perms = repo.permissions.get(login).copied().unwrap_or_default();
            let level = if perms.admin {
                Need::Admin
            } else if perms.maintain || perms.push {
                Need::Write
            } else if perms.triage || perms.pull {
                Need::Read
            } else {
                return Err((404, "Not Found"));
            };
            if level >= need {
                Ok(Some(perms))
            } else {
                Err((403, "Resource not accessible by integration"))
            }
        }
    }
}

fn owner_type(state: &FakeState, owner: &str) -> &'static str {
    let org = state.installations.values().any(|(account, kind)| {
        account.eq_ignore_ascii_case(owner) && *kind == OwnerKind::Organization
    });
    if org { "Organization" } else { "User" }
}

fn repo_json(state: &FakeState, repo: &FakeRepo, permissions: Option<GitHubPermissions>) -> Value {
    let mut value = json!({
        "id": repo.id,
        "name": repo.name,
        "full_name": format!("{}/{}", repo.owner, repo.name),
        "owner": {"login": repo.owner, "type": owner_type(state, &repo.owner)},
        "private": repo.private,
        "default_branch": repo.default_branch,
        "html_url": format!("{}/web/{}/{}", state.base, repo.owner, repo.name),
        "clone_url": format!("{}/web/{}/{}.git", state.base, repo.owner, repo.name),
    });
    if let Some(p) = permissions {
        value["permissions"] = json!({
            "admin": p.admin, "maintain": p.maintain, "push": p.push,
            "triage": p.triage, "pull": p.pull
        });
    }
    value
}

fn pull_json(state: &FakeState, repo: &FakeRepo, pull: &FakePull) -> Value {
    let (head_sha, base_sha) = match pull.state {
        PullState::Open => (
            repo.branches.get(&pull.head).cloned().unwrap_or_default(),
            repo.branches.get(&pull.base).cloned().unwrap_or_default(),
        ),
        _ => (
            pull.merged_head_sha.clone().unwrap_or_default(),
            pull.merged_base_sha.clone().unwrap_or_default(),
        ),
    };
    let merged = pull.state == PullState::Merged;
    json!({
        "number": pull.number,
        "html_url": format!("{}/web/{}/{}/pull/{}", state.base, repo.owner, repo.name, pull.number),
        "title": pull.title,
        "body": pull.body,
        "state": if pull.state == PullState::Open { "open" } else { "closed" },
        "draft": false,
        "merged": merged,
        "merged_at": if merged { json!("2026-10-07T12:00:00Z") } else { Value::Null },
        "mergeable": pull.state == PullState::Open && pull.mergeable,
        "merge_commit_sha": pull.merge_commit_sha,
        "head": {"ref": pull.head, "sha": head_sha},
        "base": {"ref": pull.base, "sha": base_sha},
    })
}

fn reachable(commits: &HashMap<String, Vec<String>>, from: &str) -> HashSet<String> {
    let mut seen = HashSet::new();
    let mut queue = vec![from.to_owned()];
    while let Some(sha) = queue.pop() {
        if !seen.insert(sha.clone()) {
            continue;
        }
        if let Some(parents) = commits.get(&sha) {
            queue.extend(parents.iter().cloned());
        }
    }
    seen
}

fn resolve(repo: &FakeRepo, rev: &str) -> Option<String> {
    if let Some(sha) = repo.branches.get(rev) {
        return Some(sha.clone());
    }
    repo.commits.contains_key(rev).then(|| rev.to_owned())
}

async fn handle(
    State(state): State<Shared>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let full_path = uri.path().to_owned();
    let (web, rest) = if let Some(rest) = full_path.strip_prefix("/api") {
        (false, rest.to_owned())
    } else if let Some(rest) = full_path.strip_prefix("/web") {
        (true, rest.to_owned())
    } else {
        return error(404, "Not Found");
    };
    let mut guard = lock(&state);
    let state = &mut *guard;
    state.requests.push_back(RecordedRequest {
        method: method.to_string(),
        web,
        path: rest.clone(),
        query: uri.query().map(str::to_owned),
        headers: headers
            .iter()
            .map(|(k, v)| {
                (
                    k.as_str().to_owned(),
                    String::from_utf8_lossy(v.as_bytes()).into_owned(),
                )
            })
            .collect(),
        body: String::from_utf8_lossy(&body).into_owned(),
    });
    while state.requests.len() > MAX_RECORDED_REQUESTS {
        state.requests.pop_front();
    }
    if let Some(index) = state
        .faults
        .iter()
        .position(|f| f.method == method.as_str() && f.path == rest && f.times > 0)
    {
        let fault = state.faults[index].clone();
        state.faults[index].times -= 1;
        if state.faults[index].times == 0 {
            state.faults.remove(index);
        }
        let mut response = error(fault.status, &fault.message);
        for (name, value) in &fault.headers {
            if let (Ok(name), Ok(value)) = (
                axum::http::HeaderName::from_bytes(name.as_bytes()),
                axum::http::HeaderValue::from_str(value),
            ) {
                response.headers_mut().insert(name, value);
            }
        }
        return response;
    }
    let query = query_pairs(uri.query());
    let segments: Vec<String> = rest
        .trim_start_matches('/')
        .split('/')
        .map(pct_decode)
        .collect();
    let segs: Vec<&str> = segments.iter().map(String::as_str).collect();
    let json_body: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
    if web {
        return match (method.as_str(), segs.as_slice()) {
            ("POST", ["login", "oauth", "access_token"]) => oauth_token(state, &body),
            _ => error(404, "Not Found"),
        };
    }
    let identity = identify(state, &headers);
    match (method.as_str(), segs.as_slice()) {
        ("POST", ["app", "installations", id, "access_tokens"]) => {
            installation_token(state, &identity, id)
        }
        ("GET", ["app", "installations"]) => {
            if identity != Identity::App {
                return error(401, "A JSON web token could not be decoded");
            }
            let all: Vec<Value> = state
                .installations
                .iter()
                .map(|(id, (account, kind))| {
                    let kind = if *kind == OwnerKind::Organization {
                        "Organization"
                    } else {
                        "User"
                    };
                    json!({"id": id, "account": {"login": account, "type": kind}})
                })
                .collect();
            reply(200, Value::Array(paginate(&all, &query)))
        }
        ("GET", ["installation", "repositories"]) => {
            let Identity::Installation(id) = identity else {
                return error(401, "Bad credentials");
            };
            let mut repos: Vec<&FakeRepo> = state
                .repos
                .values()
                .filter(|r| installation_covers(state, id, &r.owner))
                .collect();
            repos.sort_by_key(|r| r.id);
            let all: Vec<Value> = repos.iter().map(|r| repo_json(state, r, None)).collect();
            reply(
                200,
                json!({"total_count": all.len(), "repositories": paginate(&all, &query)}),
            )
        }
        ("POST", ["app-manifests", code, "conversions"]) => {
            if !state.manifest_codes.remove(*code) {
                return error(404, "Not Found");
            }
            reply(
                201,
                json!({
                    "id": FAKE_APP_ID,
                    "slug": FAKE_APP_SLUG,
                    "client_id": FAKE_CLIENT_ID,
                    "client_secret": FAKE_CLIENT_SECRET,
                    "webhook_secret": Value::Null,
                    "pem": FAKE_APP_KEY_PEM,
                    "owner": {"login": "fake-owner", "type": "Organization"},
                }),
            )
        }
        ("GET", ["user"]) => {
            let Identity::User(login) = identity else {
                return error(401, "Bad credentials");
            };
            match state.users.get(&login) {
                Some(user) => reply(
                    200,
                    json!({
                        "id": user.id,
                        "login": user.login,
                        "name": user.name,
                        "avatar_url": format!("{}/avatars/{}", state.base, user.login),
                    }),
                ),
                None => error(401, "Bad credentials"),
            }
        }
        ("POST", ["orgs", org, "repos"]) => {
            let Identity::Installation(id) = identity else {
                return error(403, "Resource not accessible by integration");
            };
            if !installation_covers(state, id, org) {
                return error(403, "Resource not accessible by integration");
            }
            create_repo(state, org, &json_body, None)
        }
        ("POST", ["user", "repos"]) => {
            let Identity::User(login) = identity else {
                return error(401, "Bad credentials");
            };
            let Some(owner) = state.users.get(&login).map(|u| u.login.clone()) else {
                return error(401, "Bad credentials");
            };
            create_repo(state, &owner, &json_body, Some(login))
        }
        (_, ["repos", owner, name, tail @ ..]) => {
            let request = RepoRequest {
                identity: &identity,
                method: &method,
                tail,
                query: &query,
                body: &json_body,
            };
            repo_route(state, owner, name, &request)
        }
        _ => error(404, "Not Found"),
    }
}

fn oauth_token(state: &mut FakeState, body: &[u8]) -> Response {
    let form = query_pairs(Some(&String::from_utf8_lossy(body)));
    let client_ok = form.get("client_id").map(String::as_str) == Some(FAKE_CLIENT_ID)
        && form.get("client_secret").map(String::as_str) == Some(FAKE_CLIENT_SECRET);
    if !client_ok {
        return reply(200, json!({"error": "incorrect_client_credentials"}));
    }
    let login = if form.get("grant_type").map(String::as_str) == Some("refresh_token") {
        match form
            .get("refresh_token")
            .and_then(|t| state.refresh_tokens.remove(t))
        {
            Some(login) => login,
            None => return reply(200, json!({"error": "bad_refresh_token"})),
        }
    } else {
        match form.get("code").and_then(|c| state.oauth_codes.remove(c)) {
            Some(login) => login,
            None => {
                return reply(
                    200,
                    json!({"error": "bad_verification_code", "error_description": "The code passed is incorrect or expired."}),
                );
            }
        }
    };
    let access_id = FakeGitHub::next_id(state);
    let refresh_id = FakeGitHub::next_id(state);
    let access = format!("ghu_{access_id:036}");
    let refresh = format!("ghr_{refresh_id:036}");
    state.user_tokens.insert(access.clone(), login.clone());
    state.refresh_tokens.insert(refresh.clone(), login);
    reply(
        200,
        json!({
            "access_token": access,
            "expires_in": USER_TOKEN_SECS,
            "refresh_token": refresh,
            "refresh_token_expires_in": REFRESH_TOKEN_SECS,
            "token_type": "bearer",
            "scope": "",
        }),
    )
}

fn installation_token(state: &mut FakeState, identity: &Identity, id: &str) -> Response {
    if *identity != Identity::App {
        return error(401, "A JSON web token could not be decoded");
    }
    let Ok(id) = id.parse::<u64>() else {
        return error(404, "Not Found");
    };
    if !state.installations.contains_key(&id) {
        return error(404, "Not Found");
    }
    let serial = FakeGitHub::next_id(state);
    let token = format!("ghs_{serial:036}");
    state.installation_tokens.insert(token.clone(), id);
    reply(
        201,
        json!({
            "token": token,
            "expires_at": format_rfc3339(now_secs(state) + INSTALLATION_TOKEN_SECS),
            "permissions": {"contents": "write", "pull_requests": "write"},
        }),
    )
}

fn create_repo(
    state: &mut FakeState,
    owner: &str,
    body: &Value,
    admin: Option<String>,
) -> Response {
    let Some(name) = body.get("name").and_then(Value::as_str) else {
        return error(422, "Validation Failed");
    };
    if state.repos.contains_key(&key(owner, name)) {
        return error(
            422,
            "Repository creation failed: name already exists on this account",
        );
    }
    let id = FakeGitHub::next_id(state);
    let mut repo = FakeRepo {
        id,
        owner: owner.to_owned(),
        name: name.to_owned(),
        private: body
            .get("private")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        default_branch: "main".to_owned(),
        ..FakeRepo::default()
    };
    if body
        .get("auto_init")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        let sha = fake_sha(FakeGitHub::next_id(state));
        repo.commits.insert(sha.clone(), Vec::new());
        repo.branches.insert("main".to_owned(), sha);
    }
    let permissions = admin.map(|login| {
        let perms = GitHubPermissions {
            admin: true,
            maintain: true,
            push: true,
            triage: true,
            pull: true,
        };
        repo.permissions.insert(login, perms);
        perms
    });
    let value = repo_json(state, &repo, permissions);
    state.repos.insert(key(owner, name), repo);
    reply(201, value)
}

/// A request routed below `/repos/{owner}/{name}`.
struct RepoRequest<'a> {
    identity: &'a Identity,
    method: &'a Method,
    tail: &'a [&'a str],
    query: &'a HashMap<String, String>,
    body: &'a Value,
}

fn repo_route(
    state: &mut FakeState,
    owner: &str,
    name: &str,
    request: &RepoRequest<'_>,
) -> Response {
    let repo_key = key(owner, name);
    let Some(mut repo) = state.repos.remove(&repo_key) else {
        return error(404, "Not Found");
    };
    let response = repo_action(state, &mut repo, request);
    state.repos.insert(repo_key, repo);
    response
}

fn repo_action(state: &mut FakeState, repo: &mut FakeRepo, request: &RepoRequest<'_>) -> Response {
    let RepoRequest {
        identity,
        method,
        tail,
        query,
        body,
    } = *request;
    let need = match (method.as_str(), tail) {
        ("GET", _) => Need::Read,
        ("POST", ["rulesets"]) => Need::Admin,
        _ => Need::Write,
    };
    let permissions = match authorize(state, identity, repo, need) {
        Ok(permissions) => permissions,
        Err((status, message)) => return error(status, message),
    };
    let app_permission = match (method.as_str(), tail) {
        ("GET", ["commits", _, "check-runs"]) => Some("checks"),
        ("GET", ["commits", _, "status" | "statuses"]) => Some("statuses"),
        ("GET", ["actions", ..]) => Some("actions"),
        _ => None,
    };
    if app_permission.is_some_and(|p| state.withheld.contains(p)) {
        return error(403, "Resource not accessible by integration");
    }
    match (method.as_str(), tail) {
        ("GET", []) => reply(200, repo_json(state, repo, permissions)),
        ("POST", ["rulesets"]) => {
            repo.rulesets.push(body.clone());
            let id = FakeGitHub::next_id(state);
            reply(
                201,
                json!({"id": id, "name": body["name"], "enforcement": body["enforcement"]}),
            )
        }
        ("POST", ["pulls"]) => {
            let head = body["head"].as_str().unwrap_or_default().to_owned();
            let base = body["base"].as_str().unwrap_or_default().to_owned();
            let (Some(head_sha), Some(base_sha)) = (
                repo.branches.get(&head).cloned(),
                repo.branches.get(&base).cloned(),
            ) else {
                return error(422, "Validation Failed: head or base branch does not exist");
            };
            if repo
                .pulls
                .iter()
                .any(|p| p.state == PullState::Open && p.head == head && p.base == base)
            {
                return error(
                    422,
                    &format!("A pull request already exists for {}:{head}.", repo.owner),
                );
            }
            if reachable(&repo.commits, &base_sha).contains(&head_sha) {
                return error(422, &format!("No commits between {base} and {head}"));
            }
            let number = repo.pulls.len() as u64 + 1;
            let pull = FakePull {
                number,
                title: body["title"].as_str().unwrap_or_default().to_owned(),
                body: body["body"].as_str().unwrap_or_default().to_owned(),
                head,
                base,
                state: PullState::Open,
                mergeable: true,
                merged_head_sha: None,
                merged_base_sha: None,
                merge_commit_sha: None,
            };
            let value = pull_json(state, repo, &pull);
            repo.pulls.push(pull);
            reply(201, value)
        }
        ("GET", ["pulls"]) => {
            let wanted = query.get("state").map_or("open", String::as_str);
            let head_filter = query.get("head").cloned();
            let matching: Vec<Value> = repo
                .pulls
                .iter()
                .filter(|p| match wanted {
                    "all" => true,
                    "closed" => p.state != PullState::Open,
                    _ => p.state == PullState::Open,
                })
                .filter(|p| {
                    head_filter
                        .as_ref()
                        .is_none_or(|h| *h == format!("{}:{}", repo.owner, p.head))
                })
                .map(|p| pull_json(state, repo, p))
                .collect();
            reply(200, Value::Array(paginate(&matching, query)))
        }
        ("GET", ["pulls", number]) => {
            match repo.pulls.iter().find(|p| p.number.to_string() == *number) {
                Some(pull) => reply(200, pull_json(state, repo, pull)),
                None => error(404, "Not Found"),
            }
        }
        ("PUT", ["pulls", number, "merge"]) => merge(state, repo, number, body),
        ("GET", ["commits", sha, "check-runs"]) => {
            let runs: Vec<Value> = repo
                .check_runs
                .get(*sha)
                .map(|runs| {
                    runs.iter()
                        .map(|r| json!({"name": r.name, "status": r.status, "conclusion": r.conclusion}))
                        .collect()
                })
                .unwrap_or_default();
            reply(
                200,
                json!({"total_count": runs.len(), "check_runs": paginate(&runs, query)}),
            )
        }
        ("GET", ["commits", sha, "status"]) => {
            let statuses = repo.statuses.get(*sha).cloned().unwrap_or_default();
            let combined = if statuses
                .iter()
                .any(|s| s.state == "failure" || s.state == "error")
            {
                "failure"
            } else if statuses.is_empty() || statuses.iter().any(|s| s.state == "pending") {
                "pending"
            } else {
                "success"
            };
            let list: Vec<Value> = statuses
                .iter()
                .map(|s| json!({"context": s.context, "state": s.state}))
                .collect();
            reply(
                200,
                json!({"state": combined, "total_count": list.len(), "statuses": list}),
            )
        }
        ("GET", ["actions", "runs"]) => {
            let runs: Vec<Value> = query
                .get("head_sha")
                .and_then(|sha| repo.workflow_runs.get(sha))
                .map(|runs| {
                    runs.iter()
                        .map(|r| json!({"name": r.name, "status": r.status, "conclusion": r.conclusion}))
                        .collect()
                })
                .unwrap_or_default();
            reply(
                200,
                json!({"total_count": runs.len(), "workflow_runs": paginate(&runs, query)}),
            )
        }
        ("PATCH", ["git", "refs", "heads", branch @ ..]) => {
            let branch = branch.join("/");
            let Some(current) = repo.branches.get(&branch).cloned() else {
                return error(422, "Reference does not exist");
            };
            let Some(target) = body["sha"].as_str().map(str::to_owned) else {
                return error(422, "Validation Failed");
            };
            if !repo.commits.contains_key(&target) {
                return error(422, "Object does not exist");
            }
            let force = body["force"].as_bool().unwrap_or(false);
            if !force && !reachable(&repo.commits, &target).contains(&current) {
                return error(422, "Update is not a fast forward");
            }
            repo.branches.insert(branch.clone(), target.clone());
            reply(
                200,
                json!({"ref": format!("refs/heads/{branch}"), "object": {"sha": target, "type": "commit"}}),
            )
        }
        ("GET", ["compare", basehead @ ..]) => {
            let joined = basehead.join("/");
            let Some((base, head)) = joined.split_once("...") else {
                return error(404, "Not Found");
            };
            let (Some(base_sha), Some(head_sha)) = (resolve(repo, base), resolve(repo, head))
            else {
                return error(404, "Not Found");
            };
            let from_base = reachable(&repo.commits, &base_sha);
            let from_head = reachable(&repo.commits, &head_sha);
            let ahead = from_head.difference(&from_base).count();
            let behind = from_base.difference(&from_head).count();
            let status = match (ahead, behind) {
                (0, 0) => "identical",
                (_, 0) => "ahead",
                (0, _) => "behind",
                _ => "diverged",
            };
            reply(
                200,
                json!({"status": status, "ahead_by": ahead, "behind_by": behind, "total_commits": ahead}),
            )
        }
        _ => error(404, "Not Found"),
    }
}

fn merge(state: &mut FakeState, repo: &mut FakeRepo, number: &str, body: &Value) -> Response {
    let Some(index) = repo
        .pulls
        .iter()
        .position(|p| p.number.to_string() == number)
    else {
        return error(404, "Not Found");
    };
    let pull = repo.pulls[index].clone();
    if pull.state != PullState::Open || !pull.mergeable {
        return error(405, "Pull Request is not mergeable");
    }
    let head_sha = repo.branches.get(&pull.head).cloned().unwrap_or_default();
    let base_sha = repo.branches.get(&pull.base).cloned().unwrap_or_default();
    if let Some(expected) = body["sha"].as_str()
        && expected != head_sha
    {
        return error(
            409,
            "Head branch was modified. Review and try the merge again.",
        );
    }
    let merge_sha = fake_sha(FakeGitHub::next_id(state));
    repo.commits
        .insert(merge_sha.clone(), vec![base_sha.clone(), head_sha.clone()]);
    repo.branches.insert(pull.base.clone(), merge_sha.clone());
    let stored = &mut repo.pulls[index];
    stored.state = PullState::Merged;
    stored.merged_head_sha = Some(head_sha);
    stored.merged_base_sha = Some(base_sha);
    stored.merge_commit_sha = Some(merge_sha.clone());
    reply(
        200,
        json!({"sha": merge_sha, "merged": true, "message": "Pull Request successfully merged"}),
    )
}
