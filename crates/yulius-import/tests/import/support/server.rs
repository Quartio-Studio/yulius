//! In-process fake Yulius server implementing the import and device login contract, with fault
//! injection. Runs on a loopback port chosen by the OS in its own thread and runtime; dropped
//! servers shut down and are joined.

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::net::TcpListener;
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::JoinHandle;

use axum::Json;
use axum::Router;
use axum::body::Bytes;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post, put};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use yulius_contracts::api::secrets::{AgentSecretInput, TeamSecretInput};
use yulius_contracts::import::{
    CHUNK_SHA256_HEADER, CreateImportRequest, CreateImportResponse, DevicePoll, DevicePollRequest,
    DeviceStart, DeviceStatus, FileProgress, FileRef, ImportManifest, ImportResult,
    MAX_CHUNK_BYTES, OnboardPlan, OnboardPlanRequest, OnboardSandbox, OnboardingStatus,
    SessionPlan,
};
use yulius_import::Url;

use super::TestResult;

pub const TOKEN: &str = "yul_test_token";
pub const DEVICE_CODE: &str = "device-code-1";
pub const USER_CODE: &str = "ABCD-EFGH";

/// One received chunk request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PutRecord {
    pub name: String,
    pub offset: u64,
    pub len: u64,
    pub status: u16,
}

/// Observable and configurable server state.
#[derive(Default)]
pub struct ServerState {
    /// Completed content by sha256 (persists across jobs, as for one sandbox).
    pub content: HashMap<String, Vec<u8>>,
    /// Partially received content by sha256.
    pub partial: HashMap<String, Vec<u8>>,
    pub jobs: HashMap<String, HashMap<String, FileRef>>,
    pub manifests: Vec<ImportManifest>,
    pub puts: Vec<PutRecord>,
    /// New bytes stored from chunks (excludes idempotent repeats).
    pub stored_bytes: u64,
    pub accepted_chunks: usize,
    /// After this many accepted chunks the server goes down (every PUT answers 503).
    pub down_after_chunks: Option<usize>,
    pub down: bool,
    /// After accepting this chunk number, answer 500 once although the data was stored.
    pub lose_response_after_chunk: Option<usize>,
    /// Answer the next chunk with 422.
    pub reject_next_chunk: bool,
    /// Sessions held: session id → digest of the main transcript.
    pub held_sessions: BTreeMap<String, String>,
    pub completed_jobs: usize,
    /// Whether the repository already has a sandbox (else the plan offers to create it).
    pub sandbox_exists: bool,
    /// A refusal the plan answers with (permissions, foreign owner).
    pub refusal: Option<String>,
    /// Onboarding is ready (else `pending`, waiting for Start onboarding in the browser).
    pub onboarding_ready: bool,
    pub plan_requests: Vec<OnboardPlanRequest>,
    pub device_script: VecDeque<DeviceStatus>,
    pub device_polls: usize,
    pub device_interval_s: u64,
    /// Sandboxes listed by `GET /api/sandboxes`: (id, slug, owner, name).
    pub sandboxes: Vec<(String, String, String, String)>,
    /// Team secret titles that exist.
    pub team_titles: Vec<String>,
    /// Agent secrets that exist: (name, scope).
    pub agent_names: Vec<(String, String)>,
    /// Team secrets created, as sent.
    pub team_created: Vec<TeamSecretInput>,
    /// Agent secrets created, as sent, with the sandbox of the path.
    pub agent_created: Vec<(String, AgentSecretInput)>,
    /// Create requests accepted past authentication (team or agent).
    pub creates: usize,
    /// Every create request received, refused or not.
    pub create_requests: usize,
    /// Names whose create answers 422.
    pub refuse_names: Vec<String>,
    /// After this many creates every request answers 401.
    pub expire_after_creates: Option<usize>,
}

type Shared = Arc<Mutex<ServerState>>;

pub struct FakeServer {
    pub url: Url,
    state: Shared,
    shutdown: Option<tokio::sync::oneshot::Sender<()>>,
    thread: Option<JoinHandle<std::io::Result<()>>>,
}

impl FakeServer {
    pub fn start() -> TestResult<Self> {
        let listener = TcpListener::bind("127.0.0.1:0")?;
        listener.set_nonblocking(true)?;
        let address = listener.local_addr()?;
        let state: Shared = Arc::new(Mutex::new(ServerState::default()));
        let app = Router::new()
            .route("/api/onboard/plan", post(plan))
            .route("/api/import", post(create_job))
            .route("/api/import/{job}/files/{name}", put(put_chunk))
            .route("/api/import/{job}/complete", post(complete))
            .route("/api/device/start", post(device_start))
            .route("/api/device/poll", post(device_poll))
            .route("/api/sandboxes", get(sandboxes))
            .route("/api/team-secrets", get(team_list).post(team_create))
            .route(
                "/api/sandboxes/{sid}/agent-secrets",
                get(agent_list).post(agent_create),
            )
            .with_state(state.clone());
        let (shutdown, signal) = tokio::sync::oneshot::channel::<()>();
        let thread = std::thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()?;
            runtime.block_on(async move {
                let listener = tokio::net::TcpListener::from_std(listener)?;
                axum::serve(listener, app)
                    .with_graceful_shutdown(async {
                        let _ = signal.await;
                    })
                    .await
            })
        });
        Ok(Self {
            url: Url::parse(&format!("http://{address}"))?,
            state,
            shutdown: Some(shutdown),
            thread: Some(thread),
        })
    }

    /// Runs `f` with exclusive access to the state.
    pub fn with<R>(&self, f: impl FnOnce(&mut ServerState) -> R) -> TestResult<R> {
        let mut guard = self.state.lock().map_err(|_| "server state poisoned")?;
        Ok(f(&mut guard))
    }
}

impl Drop for FakeServer {
    fn drop(&mut self) {
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn error(status: StatusCode, code: &str, message: &str) -> Response {
    (
        status,
        Json(serde_json::json!({"error": {"code": code, "message": message}})),
    )
        .into_response()
}

fn lock(state: &Shared) -> Option<MutexGuard<'_, ServerState>> {
    state.lock().ok()
}

fn poisoned() -> Response {
    error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "poisoned")
}

fn authorized(headers: &HeaderMap) -> bool {
    let expected = format!("Bearer {TOKEN}");
    headers
        .get("authorization")
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value == expected)
}

fn unauthorized() -> Response {
    error(StatusCode::UNAUTHORIZED, "unauthenticated", "bad token")
}

fn sha256_hex(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

async fn create_job(
    State(state): State<Shared>,
    headers: HeaderMap,
    Json(request): Json<CreateImportRequest>,
) -> Response {
    if !authorized(&headers) {
        return unauthorized();
    }
    if let Err(problem) = request.manifest.validate() {
        return error(
            StatusCode::UNPROCESSABLE_ENTITY,
            "invalid",
            &problem.to_string(),
        );
    }
    let Some(mut state) = lock(&state) else {
        return poisoned();
    };
    let job_id = format!("job-{}", state.jobs.len() + 1);
    let files: HashMap<String, FileRef> = request
        .manifest
        .files()
        .map(|file| (file.name.clone(), file.clone()))
        .collect();
    let mut missing: Vec<FileProgress> = files
        .values()
        .filter(|file| !state.content.contains_key(&file.sha256))
        .map(|file| FileProgress {
            name: file.name.clone(),
            received_bytes: state
                .partial
                .get(&file.sha256)
                .map_or(0, |partial| partial.len() as u64),
        })
        .collect();
    missing.sort_by(|a, b| a.name.cmp(&b.name));
    state.jobs.insert(job_id.clone(), files);
    state.manifests.push(request.manifest);
    Json(CreateImportResponse { job_id, missing }).into_response()
}

#[derive(Deserialize)]
struct OffsetQuery {
    offset: u64,
}

async fn put_chunk(
    State(state): State<Shared>,
    Path((job, name)): Path<(String, String)>,
    Query(query): Query<OffsetQuery>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if !authorized(&headers) {
        return unauthorized();
    }
    let Some(mut state) = lock(&state) else {
        return poisoned();
    };
    let offset = query.offset;
    let len = body.len() as u64;
    let record = |state: &mut ServerState, status: StatusCode| {
        state.puts.push(PutRecord {
            name: name.clone(),
            offset,
            len,
            status: status.as_u16(),
        });
    };
    if state.down {
        record(&mut state, StatusCode::SERVICE_UNAVAILABLE);
        return error(StatusCode::SERVICE_UNAVAILABLE, "capacity", "down");
    }
    let Some(file) = state
        .jobs
        .get(&job)
        .and_then(|files| files.get(&name))
        .cloned()
    else {
        record(&mut state, StatusCode::NOT_FOUND);
        return error(StatusCode::NOT_FOUND, "not_found", "unknown job or file");
    };
    let declared = headers
        .get(CHUNK_SHA256_HEADER)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_owned();
    if std::mem::take(&mut state.reject_next_chunk)
        || declared != sha256_hex(&body)
        || len > MAX_CHUNK_BYTES
    {
        record(&mut state, StatusCode::UNPROCESSABLE_ENTITY);
        return error(
            StatusCode::UNPROCESSABLE_ENTITY,
            "invalid",
            "chunk digest mismatch",
        );
    }
    if state.content.contains_key(&file.sha256) {
        record(&mut state, StatusCode::OK);
        return Json(FileProgress {
            name,
            received_bytes: file.size,
        })
        .into_response();
    }
    let received = state
        .partial
        .get(&file.sha256)
        .map_or(0, |partial| partial.len() as u64);
    if offset > received {
        record(&mut state, StatusCode::CONFLICT);
        return error(
            StatusCode::CONFLICT,
            "conflict",
            "offset beyond received bytes",
        );
    }
    if offset + len <= received && len > 0 {
        record(&mut state, StatusCode::OK);
        return Json(FileProgress {
            name,
            received_bytes: received,
        })
        .into_response();
    }
    let skip = usize::try_from(received - offset).unwrap_or(usize::MAX);
    let fresh = &body[skip.min(body.len())..];
    let total = {
        let partial = state.partial.entry(file.sha256.clone()).or_default();
        partial.extend_from_slice(fresh);
        partial.len() as u64
    };
    if total > file.size {
        state.partial.remove(&file.sha256);
        record(&mut state, StatusCode::UNPROCESSABLE_ENTITY);
        return error(
            StatusCode::UNPROCESSABLE_ENTITY,
            "invalid",
            "more bytes than declared",
        );
    }
    state.stored_bytes += fresh.len() as u64;
    state.accepted_chunks += 1;
    if total == file.size {
        let data = state.partial.remove(&file.sha256).unwrap_or_default();
        if sha256_hex(&data) != file.sha256 {
            record(&mut state, StatusCode::UNPROCESSABLE_ENTITY);
            return error(
                StatusCode::UNPROCESSABLE_ENTITY,
                "invalid",
                "file digest mismatch",
            );
        }
        state.content.insert(file.sha256.clone(), data);
    }
    if state
        .down_after_chunks
        .is_some_and(|limit| state.accepted_chunks >= limit)
    {
        state.down = true;
    }
    if state.lose_response_after_chunk == Some(state.accepted_chunks) {
        state.lose_response_after_chunk = None;
        record(&mut state, StatusCode::INTERNAL_SERVER_ERROR);
        return error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal",
            "response lost",
        );
    }
    record(&mut state, StatusCode::OK);
    Json(FileProgress {
        name,
        received_bytes: total,
    })
    .into_response()
}

async fn complete(
    State(state): State<Shared>,
    Path(job): Path<String>,
    headers: HeaderMap,
) -> Response {
    if !authorized(&headers) {
        return unauthorized();
    }
    let Some(mut state) = lock(&state) else {
        return poisoned();
    };
    let Some(files) = state.jobs.get(&job) else {
        return error(StatusCode::NOT_FOUND, "not_found", "unknown job");
    };
    if files
        .values()
        .any(|file| !state.content.contains_key(&file.sha256))
    {
        return error(StatusCode::CONFLICT, "conflict", "files incomplete");
    }
    let Some(manifest) = state.manifests.last().cloned() else {
        return error(StatusCode::INTERNAL_SERVER_ERROR, "internal", "no manifest");
    };
    let (mut imported, mut updated, mut unchanged) = (0, 0, 0);
    for session in &manifest.sessions {
        let digest = session.files[0].sha256.clone();
        match state
            .held_sessions
            .insert(session.session_id.clone(), digest.clone())
        {
            None => imported += 1,
            Some(previous) if previous == digest => unchanged += 1,
            Some(_) => updated += 1,
        }
    }
    state.completed_jobs += 1;
    state.sandbox_exists = true;
    let status = onboarding(&state);
    Json(ImportResult {
        sandbox_id: "sbx-demo".to_owned(),
        sandbox_url: "https://yulius.example/s/demo".to_owned(),
        tasks_created: 0,
        sessions_skipped_existing: unchanged,
        onboarding: status.state.clone(),
        sessions_imported: imported,
        sessions_updated: updated,
        sessions_unchanged: unchanged,
        sessions_url: Some("https://yulius.example/s/demo/sessions".to_owned()),
        onboarding_status: Some(status),
    })
    .into_response()
}

fn onboarding(state: &ServerState) -> OnboardingStatus {
    if state.onboarding_ready {
        OnboardingStatus {
            state: "ready".to_owned(),
            ready: true,
            summary: "Ready: the preparation task adapted the repository.".to_owned(),
            pending: Vec::new(),
            task: None,
            warnings: Vec::new(),
        }
    } else {
        // The upload never starts onboarding: a person does in the browser (SPEC-051).
        OnboardingStatus {
            state: "pending".to_owned(),
            ready: false,
            summary: "Not ready yet: task #1 waits for someone to start onboarding.".to_owned(),
            pending: vec![
                "someone clicks Start onboarding on the sandbox page: https://yulius.example/s/demo"
                    .to_owned(),
                "the repository passes the Yulius format check".to_owned(),
            ],
            task: None,
            warnings: vec![
                "The production branch `main` is not protected from agents: add a ruleset."
                    .to_owned(),
            ],
        }
    }
}

async fn plan(
    State(state): State<Shared>,
    headers: HeaderMap,
    Json(request): Json<OnboardPlanRequest>,
) -> Response {
    if !authorized(&headers) {
        return unauthorized();
    }
    let Some(mut state) = lock(&state) else {
        return poisoned();
    };
    let sessions = request
        .sessions
        .iter()
        .map(|offered| SessionPlan {
            session_id: offered.session_id.clone(),
            state: match state.held_sessions.get(&offered.session_id) {
                None => "new",
                Some(digest) if *digest == offered.sha256 => "unchanged",
                Some(_) => "changed",
            }
            .to_owned(),
        })
        .collect();
    let sandbox = state.sandbox_exists.then(|| OnboardSandbox {
        id: "sbx-demo".to_owned(),
        slug: "demo".to_owned(),
        url: "https://yulius.example/s/demo".to_owned(),
        sessions_url: "https://yulius.example/s/demo/sessions".to_owned(),
        onboarding: onboarding(&state),
    });
    let response = OnboardPlan {
        login: "alice".to_owned(),
        can_create: sandbox.is_none() && state.refusal.is_none(),
        sandbox,
        refusal: state.refusal.clone(),
        sessions,
    };
    state.plan_requests.push(request);
    Json(response).into_response()
}

async fn device_start(State(state): State<Shared>) -> Response {
    let Some(interval_s) = lock(&state).map(|state| state.device_interval_s) else {
        return poisoned();
    };
    Json(DeviceStart {
        device_code: DEVICE_CODE.to_owned(),
        user_code: USER_CODE.to_owned(),
        verification_url: "https://yulius.example/device".to_owned(),
        interval_s,
        expires_in_s: 600,
    })
    .into_response()
}

async fn device_poll(
    State(state): State<Shared>,
    Json(request): Json<DevicePollRequest>,
) -> Response {
    if request.device_code != DEVICE_CODE {
        return error(StatusCode::NOT_FOUND, "not_found", "unknown device code");
    }
    let Some(mut state) = lock(&state) else {
        return poisoned();
    };
    state.device_polls += 1;
    let status = state
        .device_script
        .pop_front()
        .unwrap_or(DeviceStatus::Pending);
    let token = (status == DeviceStatus::Approved).then(|| TOKEN.to_owned());
    Json(DevicePoll { status, token }).into_response()
}

/// Whether a vault call may go on: authorized and not past `expire_after_creates`.
fn vault_allowed(state: &ServerState, headers: &HeaderMap) -> bool {
    authorized(headers)
        && state
            .expire_after_creates
            .is_none_or(|limit| state.creates < limit)
}

async fn sandboxes(State(state): State<Shared>, headers: HeaderMap) -> Response {
    let Some(state) = lock(&state) else {
        return poisoned();
    };
    if !vault_allowed(&state, &headers) {
        return unauthorized();
    }
    let list: Vec<serde_json::Value> = state
        .sandboxes
        .iter()
        .map(|(id, slug, owner, name)| {
            serde_json::json!({
                "id": id, "slug": slug, "name": name, "access": "admin",
                "repo": {"owner": owner, "name": name, "url": "https://github.com/x/y"}
            })
        })
        .collect();
    Json(list).into_response()
}

async fn team_list(State(state): State<Shared>, headers: HeaderMap) -> Response {
    let Some(state) = lock(&state) else {
        return poisoned();
    };
    if !vault_allowed(&state, &headers) {
        return unauthorized();
    }
    let list: Vec<serde_json::Value> = state
        .team_titles
        .iter()
        .map(|title| serde_json::json!({"id": "t", "title": title, "has_value": true}))
        .collect();
    Json(list).into_response()
}

async fn agent_list(
    State(state): State<Shared>,
    headers: HeaderMap,
    Path(_sid): Path<String>,
) -> Response {
    let Some(state) = lock(&state) else {
        return poisoned();
    };
    if !vault_allowed(&state, &headers) {
        return unauthorized();
    }
    let list: Vec<serde_json::Value> = state
        .agent_names
        .iter()
        .map(|(name, scope)| serde_json::json!({"id": "a", "name": name, "scope": scope}))
        .collect();
    Json(list).into_response()
}

/// Counts a create; `Some(response)` when it is refused.
fn create_refusal(state: &mut ServerState, headers: &HeaderMap, name: &str) -> Option<Response> {
    state.create_requests += 1;
    if !vault_allowed(state, headers) {
        return Some(unauthorized());
    }
    state.creates += 1;
    state.refuse_names.iter().any(|n| n == name).then(|| {
        error(
            StatusCode::UNPROCESSABLE_ENTITY,
            "invalid",
            "the server refused it",
        )
    })
}

async fn team_create(
    State(state): State<Shared>,
    headers: HeaderMap,
    Json(input): Json<TeamSecretInput>,
) -> Response {
    let Some(mut state) = lock(&state) else {
        return poisoned();
    };
    if let Some(refused) = create_refusal(&mut state, &headers, &input.title) {
        return refused;
    }
    state.team_titles.push(input.title.clone());
    state.team_created.push(input);
    (StatusCode::CREATED, Json(serde_json::json!({"id": "new"}))).into_response()
}

async fn agent_create(
    State(state): State<Shared>,
    headers: HeaderMap,
    Path(sid): Path<String>,
    Json(input): Json<AgentSecretInput>,
) -> Response {
    let Some(mut state) = lock(&state) else {
        return poisoned();
    };
    if let Some(refused) = create_refusal(&mut state, &headers, &input.name) {
        return refused;
    }
    state.agent_created.push((sid, input));
    (StatusCode::CREATED, Json(serde_json::json!({"id": "new"}))).into_response()
}
