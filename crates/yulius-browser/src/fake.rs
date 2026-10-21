//! Test doubles (feature `fake`): an in-process fake CDP server, a fake Chromium launcher and a
//! fake virtual display.
//!
//! [`FakeCdpServer`] speaks JSON-RPC over WebSocket on `127.0.0.1:<OS-assigned port>`, and as
//! NUL-terminated JSON (the framing of Chromium's `--remote-debugging-pipe`) on a second port,
//! and answers the methods this adapter uses with canned results. It records every request,
//! emits target events, emits synthetic `Page.screencastFrame` events gated on acknowledgements
//! (at most two in flight, like Chromium), and can be told to stay silent on a method (timeout
//! tests), fail a method, or drop all connections (crash tests).
//!
//! [`write_fake_chromium`] writes a POSIX shell script that behaves like Chromium's startup
//! contract: it reads `--user-data-dir=`, records its argv, relays its DevTools pipe (fds 3 and
//! 4) to the fake server's pipe port through `python3` (a repository prerequisite), forks a
//! descendant (to prove process-group cleanup) and stays alive until killed.
//!
//! [`FakeDisplay`] stands in for Xvfb, XTest and ffmpeg ([`DisplayMode::Fake`]): it records the
//! input the display would receive and streams synthetic H.264-shaped access units (a
//! configuration, then frames with a keyframe every few frames) through the real video hub.
//! Synthetic data only; nothing here talks to a real browser or X server.

use std::collections::{HashMap, HashSet};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use base64::Engine as _;
use futures::{Sink, Stream};
use futures::{SinkExt as _, StreamExt as _};
use serde_json::{Value, json};
use std::sync::atomic::AtomicUsize;
use tokio::io::{AsyncBufReadExt as _, AsyncWriteExt as _};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{Notify, mpsc, watch};
use tokio::task::JoinHandle;
use tokio_tungstenite::tungstenite::Message;

use crate::display::{DisplayConfig, DisplayInput, DisplayMode};
use crate::video::{
    ENCODER_EVENTS, EncoderEvent, EncoderStart, EncoderStop, RunningEncoder, Tier, VideoConfig,
    VideoFrame,
};

/// Maximum recorded requests; older entries are discarded.
const MAX_RECORDED: usize = 10_000;
/// Screencast frames Chromium keeps unacknowledged.
const FRAMES_IN_FLIGHT: u64 = 2;

/// One request received by the fake.
#[derive(Debug, Clone, PartialEq)]
pub struct RecordedCall {
    /// Method name.
    pub method: String,
    /// Parameters.
    pub params: Value,
    /// Flattened session, if any.
    pub session_id: Option<String>,
}

#[derive(Debug, Clone)]
struct FakeTarget {
    id: String,
    url: String,
    title: String,
}

struct Connection {
    outgoing: mpsc::Sender<String>,
    in_flight: Arc<(AtomicU64, Notify)>,
}

struct FakeState {
    targets: Vec<FakeTarget>,
    next_target: u64,
    next_session: u64,
    sessions: HashMap<String, String>,
    casting: HashMap<String, Arc<AtomicBool>>,
    calls: Vec<RecordedCall>,
    muted: HashSet<String>,
    failing: HashMap<String, (i64, String)>,
    frames_per_screencast: u64,
    frame_bytes: usize,
    screenshot_base64_len: usize,
    ax_nodes: Value,
    emit_load_events: bool,
    selection: String,
    connections: Vec<Arc<Connection>>,
}

struct Shared {
    state: Mutex<FakeState>,
    calls_seen: watch::Sender<u64>,
    acks: watch::Sender<u64>,
    frames_sent: AtomicU64,
    disconnect: watch::Sender<u64>,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// A 32-hex-character target id like Chromium's.
#[must_use]
pub fn fake_target_id(n: u64) -> String {
    format!("{n:032X}")
}

/// The canned accessibility tree returned by `Accessibility.getFullAXTree`.
#[must_use]
pub fn default_ax_nodes() -> Value {
    json!([
        {"nodeId": "1", "ignored": false, "role": {"type": "role", "value": "RootWebArea"},
         "name": {"type": "computedString", "value": "Fake page"}, "childIds": ["2", "3", "4"]},
        {"nodeId": "2", "parentId": "1", "ignored": false, "role": {"type": "role", "value": "heading"},
         "name": {"type": "computedString", "value": "Welcome"}, "childIds": [],
         "properties": [{"name": "level", "value": {"type": "integer", "value": 1}}]},
        {"nodeId": "3", "parentId": "1", "ignored": false, "role": {"type": "role", "value": "link"},
         "name": {"type": "computedString", "value": "Docs"}, "childIds": []},
        {"nodeId": "4", "parentId": "1", "ignored": false, "role": {"type": "role", "value": "button"},
         "name": {"type": "computedString", "value": "Sign in"}, "childIds": []}
    ])
}

/// An in-process fake Chrome DevTools Protocol endpoint.
pub struct FakeCdpServer {
    shared: Arc<Shared>,
    port: u16,
    pipe_port: u16,
    accept: JoinHandle<()>,
}

impl std::fmt::Debug for FakeCdpServer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FakeCdpServer")
            .field("port", &self.port)
            .finish_non_exhaustive()
    }
}

impl Drop for FakeCdpServer {
    fn drop(&mut self) {
        self.accept.abort();
        self.disconnect_all();
    }
}

impl FakeCdpServer {
    /// Starts the fake with one `about:blank` page target.
    ///
    /// # Errors
    /// When binding the loopback listener fails.
    pub async fn start() -> std::io::Result<Self> {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await?;
        let port = listener.local_addr()?.port();
        let pipe_listener = TcpListener::bind(("127.0.0.1", 0)).await?;
        let pipe_port = pipe_listener.local_addr()?.port();
        let shared = Arc::new(Shared {
            state: Mutex::new(FakeState {
                targets: vec![FakeTarget {
                    id: fake_target_id(1),
                    url: "about:blank".into(),
                    title: "about:blank".into(),
                }],
                next_target: 2,
                next_session: 1,
                sessions: HashMap::new(),
                casting: HashMap::new(),
                calls: Vec::new(),
                muted: HashSet::new(),
                failing: HashMap::new(),
                frames_per_screencast: 3,
                frame_bytes: 64,
                screenshot_base64_len: 0,
                ax_nodes: default_ax_nodes(),
                emit_load_events: true,
                selection: String::new(),
                connections: Vec::new(),
            }),
            calls_seen: watch::channel(0).0,
            acks: watch::channel(0).0,
            frames_sent: AtomicU64::new(0),
            disconnect: watch::channel(0).0,
        });
        let accept_shared = Arc::clone(&shared);
        let accept = tokio::spawn(async move {
            loop {
                let shared = Arc::clone(&accept_shared);
                tokio::select! {
                    accepted = listener.accept() => match accepted {
                        Ok((stream, _)) => { tokio::spawn(serve_websocket(shared, stream)); }
                        Err(_) => return,
                    },
                    accepted = pipe_listener.accept() => match accepted {
                        Ok((stream, _)) => { tokio::spawn(serve_pipe(shared, stream)); }
                        Err(_) => return,
                    },
                }
            }
        });
        Ok(Self {
            shared,
            port,
            pipe_port,
            accept,
        })
    }

    /// The WebSocket listening port.
    #[must_use]
    pub fn port(&self) -> u16 {
        self.port
    }

    /// The port speaking NUL-terminated JSON, which [`FakeChromium::Ready`] relays its DevTools
    /// pipe to.
    #[must_use]
    pub fn pipe_port(&self) -> u16 {
        self.pipe_port
    }

    /// Browser WebSocket URL (`ws://127.0.0.1:<port>/devtools/browser/fake`).
    #[must_use]
    pub fn ws_url(&self) -> String {
        format!("ws://127.0.0.1:{}/devtools/browser/fake", self.port)
    }

    /// All recorded requests in arrival order (at most the last 10 000).
    #[must_use]
    pub fn calls(&self) -> Vec<RecordedCall> {
        lock(&self.shared.state).calls.clone()
    }

    /// Recorded requests of one method.
    #[must_use]
    pub fn calls_for(&self, method: &str) -> Vec<RecordedCall> {
        lock(&self.shared.state)
            .calls
            .iter()
            .filter(|call| call.method == method)
            .cloned()
            .collect()
    }

    /// Waits until at least `count` requests of `method` were recorded.
    pub async fn wait_for_calls(&self, method: &str, count: usize, timeout: Duration) -> bool {
        let mut seen = self.shared.calls_seen.subscribe();
        let waiting = async {
            loop {
                if self.calls_for(method).len() >= count {
                    return true;
                }
                if seen.changed().await.is_err() {
                    return false;
                }
            }
        };
        tokio::time::timeout(timeout, waiting)
            .await
            .unwrap_or(false)
    }

    /// `Page.screencastFrameAck` requests received.
    #[must_use]
    pub fn ack_count(&self) -> u64 {
        *self.shared.acks.borrow()
    }

    /// Screencast frames emitted.
    #[must_use]
    pub fn frames_sent(&self) -> u64 {
        self.shared.frames_sent.load(Ordering::Relaxed)
    }

    /// Waits until at least `count` acknowledgements arrived.
    pub async fn wait_for_acks(&self, count: u64, timeout: Duration) -> bool {
        let mut acks = self.shared.acks.subscribe();
        tokio::time::timeout(timeout, acks.wait_for(|n| *n >= count))
            .await
            .is_ok_and(|result| result.is_ok())
    }

    /// Never answer `method` (requests are still recorded).
    pub fn mute(&self, method: &str) {
        lock(&self.shared.state).muted.insert(method.to_owned());
    }

    /// Answer `method` with a JSON-RPC error.
    pub fn fail(&self, method: &str, code: i64, message: &str) {
        lock(&self.shared.state)
            .failing
            .insert(method.to_owned(), (code, message.to_owned()));
    }

    /// Frames emitted per `Page.startScreencast` (default 3).
    pub fn set_frames_per_screencast(&self, frames: u64) {
        lock(&self.shared.state).frames_per_screencast = frames;
    }

    /// Size of each synthetic JPEG payload in bytes (default 64).
    pub fn set_frame_bytes(&self, bytes: usize) {
        lock(&self.shared.state).frame_bytes = bytes.max(16);
    }

    /// Length of the base64 text returned by `Page.captureScreenshot` (0 = a tiny PNG).
    pub fn set_screenshot_base64_len(&self, len: usize) {
        lock(&self.shared.state).screenshot_base64_len = len;
    }

    /// Nodes returned by `Accessibility.getFullAXTree`.
    pub fn set_ax_nodes(&self, nodes: Value) {
        lock(&self.shared.state).ax_nodes = nodes;
    }

    /// Whether `Page.navigate` is followed by `Page.loadEventFired` (default true).
    pub fn set_emit_load_events(&self, emit: bool) {
        lock(&self.shared.state).emit_load_events = emit;
    }

    /// The text pages report as the user's selection (the copy expression's result).
    pub fn set_selection(&self, text: &str) {
        text.clone_into(&mut lock(&self.shared.state).selection);
    }

    /// A page opens a new page (a link to a new tab or `window.open`): a target with an
    /// `openerId` appears. Returns its id.
    pub async fn open_from_page(&self, opener: &str, url: &str) -> String {
        let (event, id, connections) = {
            let mut state = lock(&self.shared.state);
            let target = FakeTarget {
                id: fake_target_id(state.next_target),
                url: url.to_owned(),
                title: String::new(),
            };
            state.next_target += 1;
            let mut event = target_event("Target.targetCreated", &target);
            event["params"]["targetInfo"]["openerId"] = json!(opener);
            let id = target.id.clone();
            state.targets.push(target);
            (event, id, state.connections.clone())
        };
        for connection in connections {
            let _ = connection.outgoing.send(event.to_string()).await;
        }
        id
    }

    /// Current page target ids.
    #[must_use]
    pub fn target_ids(&self) -> Vec<String> {
        lock(&self.shared.state)
            .targets
            .iter()
            .map(|target| target.id.clone())
            .collect()
    }

    /// Changes a target's title and notifies connected clients (`Target.targetInfoChanged`).
    pub async fn set_target_title(&self, target_id: &str, title: &str) {
        let (event, connections) = {
            let mut state = lock(&self.shared.state);
            let Some(target) = state.targets.iter_mut().find(|t| t.id == target_id) else {
                return;
            };
            title.clone_into(&mut target.title);
            let event = target_event("Target.targetInfoChanged", target);
            (event, state.connections.clone())
        };
        for connection in connections {
            let _ = connection.outgoing.send(event.to_string()).await;
        }
    }

    /// Sends a browser-level `Fetch.requestPaused` for `url` (a page's request matched the
    /// client's interception patterns); the client answers with `Fetch.failRequest` or
    /// `Fetch.continueRequest` naming `request_id`.
    pub async fn pause_request(&self, request_id: &str, url: &str) {
        let connections = lock(&self.shared.state).connections.clone();
        let event = json!({"method": "Fetch.requestPaused", "params": {
            "requestId": request_id, "resourceType": "Document",
            "request": {"url": url, "method": "GET", "headers": {}}}});
        for connection in connections {
            let _ = connection.outgoing.send(event.to_string()).await;
        }
    }

    /// Drops every open connection (simulates a browser crash from the client's view).
    pub fn disconnect_all(&self) {
        self.shared
            .disconnect
            .send_modify(|generation| *generation += 1);
    }

    /// Open connections.
    #[must_use]
    pub fn connections(&self) -> usize {
        lock(&self.shared.state)
            .connections
            .iter()
            .filter(|connection| !connection.outgoing.is_closed())
            .count()
    }
}

fn target_info(target: &FakeTarget) -> Value {
    json!({"targetId": target.id, "type": "page", "title": target.title, "url": target.url,
           "attached": false, "canAccessOpener": false, "browserContextId": "fake-context"})
}

fn target_event(method: &str, target: &FakeTarget) -> Value {
    json!({"method": method, "params": {"targetInfo": target_info(target)}})
}

/// A tiny valid 1×1 PNG.
const TINY_PNG_BASE64: &str = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk+M9QDwADhgGAWjR9awAAAABJRU5ErkJggg==";

async fn serve_websocket(shared: Arc<Shared>, stream: TcpStream) {
    let Ok(socket) = tokio_tungstenite::accept_async(stream).await else {
        return;
    };
    let (sink, source) = socket.split();
    let sink = sink.with(|text: String| async move {
        Ok::<_, tokio_tungstenite::tungstenite::Error>(Message::text(text))
    });
    let source = futures::stream::unfold(source, |mut source| async move {
        loop {
            match source.next().await? {
                Ok(Message::Text(text)) => return Some((text.as_str().to_owned(), source)),
                Ok(Message::Close(_)) | Err(_) => return None,
                Ok(_) => {}
            }
        }
    });
    serve(shared, source, sink).await;
}

async fn serve_pipe(shared: Arc<Shared>, stream: TcpStream) {
    let (read, write) = stream.into_split();
    let source = futures::stream::unfold(tokio::io::BufReader::new(read), |mut read| async {
        let mut message = Vec::new();
        match read.read_until(0, &mut message).await {
            Ok(n) if n > 0 && message.pop() == Some(0) => {
                Some((String::from_utf8_lossy(&message).into_owned(), read))
            }
            _ => None,
        }
    });
    let sink = futures::sink::unfold(write, |mut write, text: String| async move {
        write.write_all(text.as_bytes()).await?;
        write.write_all(&[0]).await?;
        Ok::<_, std::io::Error>(write)
    });
    serve(shared, source, sink).await;
}

async fn serve<I, K>(shared: Arc<Shared>, source: I, sink: K)
where
    I: Stream<Item = String>,
    K: Sink<String> + Send + 'static,
{
    let mut source = std::pin::pin!(source);
    let mut sink = Box::pin(sink);
    let (outgoing, mut queue) = mpsc::channel::<String>(4096);
    let connection = Arc::new(Connection {
        outgoing,
        in_flight: Arc::new((AtomicU64::new(0), Notify::new())),
    });
    lock(&shared.state)
        .connections
        .push(Arc::clone(&connection));
    let mut disconnect = shared.disconnect.subscribe();
    disconnect.mark_unchanged();
    let writer = tokio::spawn(async move {
        while let Some(text) = queue.recv().await {
            if sink.send(text).await.is_err() {
                break;
            }
        }
        let _ = sink.close().await;
    });
    loop {
        tokio::select! {
            message = source.next() => match message {
                Some(text) => handle_request(&shared, &connection, &text).await,
                None => break,
            },
            _ = disconnect.changed() => break,
        }
    }
    writer.abort();
    lock(&shared.state)
        .connections
        .retain(|other| !Arc::ptr_eq(other, &connection));
}

fn record(shared: &Shared, call: RecordedCall) {
    let mut state = lock(&shared.state);
    if state.calls.len() >= MAX_RECORDED {
        state.calls.remove(0);
    }
    state.calls.push(call);
    drop(state);
    shared.calls_seen.send_modify(|n| *n += 1);
}

async fn send(connection: &Connection, value: &Value) {
    let _ = connection.outgoing.send(value.to_string()).await;
}

async fn handle_request(shared: &Arc<Shared>, connection: &Arc<Connection>, text: &str) {
    let Ok(request) = serde_json::from_str::<Value>(text) else {
        return;
    };
    let id = request.get("id").cloned().unwrap_or(Value::Null);
    let method = request
        .get("method")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    let params = request.get("params").cloned().unwrap_or(Value::Null);
    let session_id = request
        .get("sessionId")
        .and_then(Value::as_str)
        .map(str::to_owned);
    record(
        shared,
        RecordedCall {
            method: method.clone(),
            params: params.clone(),
            session_id: session_id.clone(),
        },
    );
    let (muted, failure) = {
        let state = lock(&shared.state);
        (
            state.muted.contains(&method),
            state.failing.get(&method).cloned(),
        )
    };
    if muted {
        return;
    }
    if let Some((code, message)) = failure {
        let mut reply = json!({"id": id, "error": {"code": code, "message": message}});
        if let Some(session) = &session_id {
            reply["sessionId"] = json!(session);
        }
        send(connection, &reply).await;
        return;
    }
    let mut after: Vec<Value> = Vec::new();
    let mut before: Vec<Value> = Vec::new();
    let result = respond(
        shared,
        connection,
        &method,
        &params,
        session_id.as_deref(),
        &mut before,
        &mut after,
    );
    for event in &before {
        send(connection, event).await;
    }
    let mut reply = match result {
        Ok(result) => json!({"id": id, "result": result}),
        Err((code, message)) => json!({"id": id, "error": {"code": code, "message": message}}),
    };
    if let Some(session) = &session_id {
        reply["sessionId"] = json!(session);
    }
    send(connection, &reply).await;
    for event in &after {
        send(connection, event).await;
    }
}

type Reply = Result<Value, (i64, String)>;

fn respond(
    shared: &Arc<Shared>,
    connection: &Arc<Connection>,
    method: &str,
    params: &Value,
    session_id: Option<&str>,
    before: &mut Vec<Value>,
    after: &mut Vec<Value>,
) -> Reply {
    let text = |name: &str| {
        params
            .get(name)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned()
    };
    match method {
        "Target.setDiscoverTargets" => {
            let state = lock(&shared.state);
            before.extend(
                state
                    .targets
                    .iter()
                    .map(|t| target_event("Target.targetCreated", t)),
            );
            Ok(json!({}))
        }
        "Target.getTargetInfo" => {
            let target_id = text("targetId");
            let state = lock(&shared.state);
            match state.targets.iter().find(|t| t.id == target_id) {
                Some(target) => Ok(json!({"targetInfo": target_info(target)})),
                None => Err((-32602, "No target with given id found".into())),
            }
        }
        "Target.getTargets" => {
            let state = lock(&shared.state);
            let infos: Vec<Value> = state.targets.iter().map(target_info).collect();
            Ok(json!({"targetInfos": infos}))
        }
        "Target.createTarget" => {
            let mut state = lock(&shared.state);
            let target = FakeTarget {
                id: fake_target_id(state.next_target),
                url: text("url"),
                title: String::new(),
            };
            state.next_target += 1;
            before.push(target_event("Target.targetCreated", &target));
            let id = target.id.clone();
            state.targets.push(target);
            Ok(json!({"targetId": id}))
        }
        "Target.closeTarget" | "Target.activateTarget" => {
            let target_id = text("targetId");
            let mut state = lock(&shared.state);
            if !state.targets.iter().any(|t| t.id == target_id) {
                return Err((-32602, "No target with given id found".into()));
            }
            if method == "Target.closeTarget" {
                state.targets.retain(|t| t.id != target_id);
                after.push(
                    json!({"method": "Target.targetDestroyed", "params": {"targetId": target_id}}),
                );
                return Ok(json!({"success": true}));
            }
            Ok(json!({}))
        }
        "Target.attachToTarget" => {
            let target_id = text("targetId");
            let mut state = lock(&shared.state);
            let Some(target) = state.targets.iter().find(|t| t.id == target_id).cloned() else {
                return Err((-32602, "No target with given id found".into()));
            };
            let session = format!("SESSION-{}", state.next_session);
            state.next_session += 1;
            state.sessions.insert(session.clone(), target_id);
            before.push(json!({"method": "Target.attachedToTarget", "params": {
                "sessionId": session, "targetInfo": target_info(&target), "waitingForDebugger": false}}));
            Ok(json!({"sessionId": session}))
        }
        "Page.navigate" => {
            let url = text("url");
            let mut state = lock(&shared.state);
            if url.contains("unresolvable.invalid") {
                return Ok(
                    json!({"frameId": "frame-1", "errorText": "net::ERR_NAME_NOT_RESOLVED"}),
                );
            }
            let target_id = session_id.and_then(|s| state.sessions.get(s)).cloned();
            if let Some(target) = state
                .targets
                .iter_mut()
                .find(|t| Some(&t.id) == target_id.as_ref())
            {
                target.url.clone_from(&url);
                target.title = format!("Title of {url}");
                after.push(target_event("Target.targetInfoChanged", target));
            }
            if state.emit_load_events {
                after.push(
                    json!({"method": "Page.loadEventFired", "params": {"timestamp": 1.0},
                                  "sessionId": session_id}),
                );
            }
            Ok(json!({"frameId": "frame-1", "loaderId": "loader-1"}))
        }
        "Page.getNavigationHistory" => {
            let state = lock(&shared.state);
            let url = session_id
                .and_then(|s| state.sessions.get(s))
                .and_then(|target| state.targets.iter().find(|t| &t.id == target))
                .map(|t| t.url.clone())
                .unwrap_or_default();
            Ok(json!({"currentIndex": 1, "entries": [
                {"id": 7, "url": "https://previous.example/", "title": "Previous"},
                {"id": 8, "url": url, "title": "Current"}]}))
        }
        "Page.startScreencast" => {
            let Some(session) = session_id else {
                return Err((-32601, "Page.startScreencast needs a session".into()));
            };
            let (frames, frame_bytes, flag) = {
                let mut state = lock(&shared.state);
                let flag = Arc::new(AtomicBool::new(true));
                if let Some(previous) = state.casting.insert(session.to_owned(), Arc::clone(&flag))
                {
                    previous.store(false, Ordering::Relaxed);
                }
                (state.frames_per_screencast, state.frame_bytes, flag)
            };
            tokio::spawn(emit_frames(
                Arc::clone(shared),
                Arc::clone(connection),
                session.to_owned(),
                frames,
                frame_bytes,
                flag,
            ));
            Ok(json!({}))
        }
        "Page.stopScreencast" => {
            if let Some(flag) = session_id.and_then(|s| lock(&shared.state).casting.remove(s)) {
                flag.store(false, Ordering::Relaxed);
            }
            Ok(json!({}))
        }
        "Page.screencastFrameAck" => {
            let (count, notify) = &*connection.in_flight;
            let _ = count.try_update(Ordering::SeqCst, Ordering::SeqCst, |n| n.checked_sub(1));
            notify.notify_waiters();
            shared.acks.send_modify(|n| *n += 1);
            Ok(json!({}))
        }
        "Page.captureScreenshot" => {
            let len = lock(&shared.state).screenshot_base64_len;
            let data = if len == 0 {
                TINY_PNG_BASE64.to_owned()
            } else {
                "A".repeat(scaled_screenshot_len(len, params))
            };
            Ok(json!({"data": data}))
        }
        "Accessibility.getFullAXTree" => Ok(json!({"nodes": lock(&shared.state).ax_nodes.clone()})),
        "Runtime.evaluate" => {
            let expression = text("expression");
            if expression.contains("getSelection") {
                let selection = lock(&shared.state).selection.clone();
                return Ok(json!({"result": {"type": "string", "value": selection}}));
            }
            if expression == "document" {
                Ok(
                    json!({"result": {"type": "object", "subtype": "node", "className": "HTMLDocument",
                                     "objectId": "document-object"}}),
                )
            } else {
                Ok(json!({"result": {"type": "string", "value": "Fake page text"}}))
            }
        }
        "Runtime.callFunctionOn" => {
            let selector = params
                .get("arguments")
                .and_then(|a| a.get(0))
                .and_then(|a| a.get("value"))
                .and_then(Value::as_str)
                .unwrap_or_default();
            if selector.starts_with("!!") {
                return Ok(json!({"result": {"type": "object"}, "exceptionDetails": {
                    "text": "Uncaught", "exception": {"description": "SyntaxError: not a valid selector"}}}));
            }
            let value = match selector {
                "#missing" => json!({"found": false}),
                "#hidden" => json!({"found": true, "x": 0, "y": 0, "width": 0, "height": 0}),
                _ => json!({"found": true, "x": 100.5, "y": 40.25, "width": 80, "height": 20}),
            };
            Ok(json!({"result": {"type": "object", "value": value}}))
        }
        "Browser.getVersion" => Ok(json!({"product": "FakeChrome/1.0", "protocolVersion": "1.3"})),
        "Fetch.enable" | "Fetch.disable" | "Fetch.continueRequest" | "Fetch.failRequest" => {
            Ok(json!({}))
        }
        "Page.enable"
        | "Runtime.enable"
        | "Runtime.releaseObject"
        | "Emulation.setDeviceMetricsOverride"
        | "Input.dispatchMouseEvent"
        | "Input.dispatchKeyEvent"
        | "Input.insertText"
        | "Page.navigateToHistoryEntry"
        | "Page.reload"
        | "Browser.close" => Ok(json!({})),
        other => Err((-32601, format!("'{other}' wasn't found"))),
    }
}

/// Synthetic size of a configured screenshot: PNG keeps `len`; JPEG shrinks with `quality`
/// (percent) and with the square of `clip.scale`, like a real encoder roughly does.
fn scaled_screenshot_len(len: usize, params: &Value) -> usize {
    if params.get("format").and_then(Value::as_str) != Some("jpeg") {
        return len;
    }
    let quality = params
        .get("quality")
        .and_then(Value::as_u64)
        .unwrap_or(100)
        .min(100);
    let per_mille = params
        .get("clip")
        .and_then(|clip| clip.get("scale"))
        .and_then(Value::as_f64)
        .map_or(1000, |scale| {
            (scale.clamp(0.0, 1.0) * 1000.0).round() as u64
        });
    let scaled = (len as u64)
        .saturating_mul(quality)
        .saturating_mul(per_mille)
        .saturating_mul(per_mille)
        / (100 * 1000 * 1000);
    usize::try_from(scaled).unwrap_or(len).max(1)
}

async fn emit_frames(
    shared: Arc<Shared>,
    connection: Arc<Connection>,
    session: String,
    frames: u64,
    frame_bytes: usize,
    active: Arc<AtomicBool>,
) {
    let (in_flight, notify) = &*connection.in_flight;
    for index in 0..frames {
        loop {
            let waiting = notify.notified();
            if !active.load(Ordering::Relaxed) || connection.outgoing.is_closed() {
                return;
            }
            if in_flight.load(Ordering::SeqCst) < FRAMES_IN_FLIGHT {
                break;
            }
            // Re-check periodically in case a notification raced the load above.
            let _ = tokio::time::timeout(Duration::from_millis(50), waiting).await;
        }
        let mut jpeg = vec![0xFF, 0xD8];
        jpeg.extend_from_slice(format!("frame-{index};").as_bytes());
        jpeg.resize(frame_bytes, b'.');
        let event = json!({
            "method": "Page.screencastFrame",
            "sessionId": session,
            "params": {
                "data": base64::engine::general_purpose::STANDARD.encode(&jpeg),
                "metadata": {"offsetTop": 0, "pageScaleFactor": 1, "deviceWidth": 1280,
                             "deviceHeight": 800, "scrollOffsetX": 0, "scrollOffsetY": 0,
                             "timestamp": index},
                "sessionId": 1,
            },
        });
        in_flight.fetch_add(1, Ordering::SeqCst);
        if connection.outgoing.send(event.to_string()).await.is_err() {
            return;
        }
        shared.frames_sent.fetch_add(1, Ordering::Relaxed);
    }
}

/// Startup behaviour of a fake Chromium script.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FakeChromium {
    /// Relay the DevTools pipe to the fake server's [`FakeCdpServer::pipe_port`] `port`, fork a
    /// descendant, stay alive.
    Ready {
        /// Fake CDP server pipe port.
        port: u16,
    },
    /// Like `Ready` but ignore SIGTERM (exercises SIGKILL escalation).
    IgnoresSigterm {
        /// Fake CDP server pipe port.
        port: u16,
    },
    /// Print to stderr and exit 1 without becoming ready.
    ExitsImmediately,
    /// Stay alive, keeping the DevTools pipe open, without ever answering on it.
    NeverReady,
}

/// Argv file the fake writes into the profile directory.
pub const FAKE_ARGV_FILE: &str = "fake-chromium-argv.txt";
/// File holding the fake's own pid (stable across its final `exec`).
pub const FAKE_PID_FILE: &str = "fake-chromium.pid";
/// File holding the fake's forked descendant pid.
pub const FAKE_CHILD_PID_FILE: &str = "fake-chromium-child.pid";

/// Relays fd 3 to a TCP connection to `127.0.0.1:<argv[1]>` and the connection back to fd 4,
/// exiting when either side ends. No single quotes (it is quoted for the shell).
const PIPE_RELAY: &str = r#"import os, socket, sys, threading
peer = socket.create_connection(("127.0.0.1", int(sys.argv[1])))
def requests():
    while True:
        data = os.read(3, 65536)
        if not data:
            break
        peer.sendall(data)
    peer.shutdown(socket.SHUT_WR)
threading.Thread(target=requests, daemon=True).start()
while True:
    data = peer.recv(65536)
    if not data:
        break
    while data:
        data = data[os.write(4, data):]
"#;

/// Writes an executable fake Chromium script named `name` into `dir`.
///
/// # Errors
/// File-system failures.
pub fn write_fake_chromium(
    dir: &Path,
    name: &str,
    behavior: FakeChromium,
) -> std::io::Result<PathBuf> {
    let parse_dir = r#"dir=""
for arg in "$@"; do
  case "$arg" in
    --user-data-dir=*) dir="${arg#--user-data-dir=}" ;;
  esac
done
if [ -z "$dir" ]; then echo "fake-chromium: missing --user-data-dir" >&2; exit 2; fi
printf '%s\n' "$@" > "$dir/fake-chromium-argv.txt"
printf '%s' "$$" > "$dir/fake-chromium.pid"
"#;
    // Only the relay keeps fds 3 and 4, so its end (the server dropped the connection) is the
    // end of the pipe for Yulius, as a crashed browser's would be.
    let ready = |port: u16, trap: &str| {
        format!(
            "{trap}{parse_dir}python3 -I -c '{PIPE_RELAY}' {port} &\nsleep 600 3<&- 4>&- &\nprintf '%s' \"$!\" > \"$dir/{FAKE_CHILD_PID_FILE}\"\nexec sleep 600 3<&- 4>&-\n"
        )
    };
    let body = match behavior {
        FakeChromium::Ready { port } => ready(port, ""),
        FakeChromium::IgnoresSigterm { port } => ready(port, "trap '' TERM\n"),
        FakeChromium::ExitsImmediately => {
            "echo 'fake-chromium: cannot open shared object file' >&2\nexit 1\n".to_owned()
        }
        FakeChromium::NeverReady => format!("{parse_dir}exec sleep 600\n"),
    };
    let path = dir.join(name);
    std::fs::write(&path, format!("#!/bin/sh\n{body}"))?;
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))?;
    Ok(path)
}

/// The `avcC` record of the fake display's stream (constrained baseline 3.1).
pub const FAKE_AVCC: [u8; 8] = [1, 0x42, 0xC0, 0x1F, 0xFF, 0xE1, 0, 0];

struct FakeDisplayInner {
    config: Mutex<DisplayConfig>,
    inputs: Mutex<Vec<DisplayInput>>,
    inputs_seen: watch::Sender<u64>,
    encoders: Mutex<Vec<(u32, u32)>>,
    tiers: Mutex<Vec<Tier>>,
    running: AtomicUsize,
    frame_interval_ms: AtomicU64,
    keyframe_every: AtomicU64,
    frame_bytes: AtomicUsize,
    fail_encoder: AtomicBool,
    started: AtomicU64,
    stopped: AtomicU64,
}

/// A fake virtual display ([`DisplayMode::Fake`]). Clones share the recording.
#[derive(Clone)]
pub struct FakeDisplay {
    inner: Arc<FakeDisplayInner>,
}

impl std::fmt::Debug for FakeDisplay {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FakeDisplay").finish_non_exhaustive()
    }
}

impl Default for FakeDisplay {
    fn default() -> Self {
        Self::new()
    }
}

impl FakeDisplay {
    /// A display with the default configuration (scale 1, at most 1920×1200) streaming a frame
    /// every 20 ms with a keyframe every 10 frames.
    #[must_use]
    pub fn new() -> Self {
        Self::with_config(DisplayConfig::new(
            "/nonexistent/yulius-fake-displays".into(),
        ))
    }

    /// A display with `config` (its scale and maximum size apply; programs are not used).
    #[must_use]
    pub fn with_config(config: DisplayConfig) -> Self {
        Self {
            inner: Arc::new(FakeDisplayInner {
                config: Mutex::new(config),
                inputs: Mutex::new(Vec::new()),
                inputs_seen: watch::channel(0).0,
                encoders: Mutex::new(Vec::new()),
                tiers: Mutex::new(Vec::new()),
                running: AtomicUsize::new(0),
                frame_interval_ms: AtomicU64::new(20),
                keyframe_every: AtomicU64::new(10),
                frame_bytes: AtomicUsize::new(64),
                fail_encoder: AtomicBool::new(false),
                started: AtomicU64::new(0),
                stopped: AtomicU64::new(0),
            }),
        }
    }

    /// The display mode that uses this fake.
    #[must_use]
    pub fn mode(&self) -> DisplayMode {
        DisplayMode::Fake(self.clone())
    }

    /// Every input the display received, in order.
    #[must_use]
    pub fn inputs(&self) -> Vec<DisplayInput> {
        lock(&self.inner.inputs).clone()
    }

    /// Waits until an input satisfying `matches` was received.
    pub async fn wait_for_input(
        &self,
        matches: impl Fn(&DisplayInput) -> bool,
        timeout: Duration,
    ) -> bool {
        let mut seen = self.inner.inputs_seen.subscribe();
        let waiting = async {
            loop {
                if self.inputs().iter().any(&matches) {
                    return true;
                }
                if seen.changed().await.is_err() {
                    return false;
                }
            }
        };
        tokio::time::timeout(timeout, waiting)
            .await
            .unwrap_or(false)
    }

    /// Sizes the encoder was started at (device pixels), in order.
    #[must_use]
    pub fn encoder_sizes(&self) -> Vec<(u32, u32)> {
        lock(&self.inner.encoders).clone()
    }

    /// Tiers the encoder was started at, in order.
    #[must_use]
    pub fn encoder_tiers(&self) -> Vec<Tier> {
        lock(&self.inner.tiers).clone()
    }

    /// Encoders running now.
    #[must_use]
    pub fn running_encoders(&self) -> usize {
        self.inner.running.load(Ordering::SeqCst)
    }

    /// Time between synthetic frames.
    pub fn set_frame_interval(&self, interval: Duration) {
        let millis = u64::try_from(interval.as_millis())
            .unwrap_or(u64::MAX)
            .max(1);
        self.inner
            .frame_interval_ms
            .store(millis, Ordering::Relaxed);
    }

    /// A keyframe every `frames` frames.
    pub fn set_keyframe_every(&self, frames: u64) {
        self.inner
            .keyframe_every
            .store(frames.max(1), Ordering::Relaxed);
    }

    /// Bytes of each synthetic access unit.
    pub fn set_frame_bytes(&self, bytes: usize) {
        self.inner
            .frame_bytes
            .store(bytes.max(16), Ordering::Relaxed);
    }

    /// Makes every encoder start fail (video unavailable).
    pub fn fail_encoder(&self) {
        self.inner.fail_encoder.store(true, Ordering::Relaxed);
    }

    /// Displays started and stopped so far.
    #[must_use]
    pub fn lifecycle(&self) -> (u64, u64) {
        (
            self.inner.started.load(Ordering::SeqCst),
            self.inner.stopped.load(Ordering::SeqCst),
        )
    }

    pub(crate) fn config(&self) -> DisplayConfig {
        lock(&self.inner.config).clone()
    }

    pub(crate) fn record(&self, input: DisplayInput) {
        lock(&self.inner.inputs).push(input);
        self.inner.inputs_seen.send_modify(|n| *n += 1);
    }

    pub(crate) fn started(&self) {
        self.inner.started.fetch_add(1, Ordering::SeqCst);
    }

    pub(crate) fn stopped(&self) {
        self.inner.stopped.fetch_add(1, Ordering::SeqCst);
    }

    pub(crate) fn encoder(&self) -> FakeEncoderStart {
        FakeEncoderStart(self.clone())
    }
}

/// Starts the fake display's synthetic encoder.
pub(crate) struct FakeEncoderStart(FakeDisplay);

struct FakeEncoderStop {
    display: FakeDisplay,
    task: JoinHandle<()>,
}

impl EncoderStop for FakeEncoderStop {
    fn stop(
        self: Box<Self>,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Option<String>> + Send>> {
        self.task.abort();
        self.display.inner.running.fetch_sub(1, Ordering::SeqCst);
        Box::pin(async { None })
    }
}

impl EncoderStart for FakeEncoderStart {
    async fn start(&self, size: (u32, u32), tier: Tier) -> Result<RunningEncoder, String> {
        let display = self.0.clone();
        lock(&display.inner.encoders).push(size);
        lock(&display.inner.tiers).push(tier);
        if display.inner.fail_encoder.load(Ordering::Relaxed) {
            return Err("the fake encoder is broken".into());
        }
        display.inner.running.fetch_add(1, Ordering::SeqCst);
        let (tx, events) = mpsc::channel(ENCODER_EVENTS);
        let inner = Arc::clone(&display.inner);
        let task = tokio::spawn(async move {
            let config = VideoConfig {
                codec: "avc1.42c01f".into(),
                description: FAKE_AVCC.to_vec().into(),
                width: size.0,
                height: size.1,
            };
            if tx
                .send(EncoderEvent::Config(Arc::new(config)))
                .await
                .is_err()
            {
                return;
            }
            for n in 0_u64.. {
                let every = inner.keyframe_every.load(Ordering::Relaxed).max(1);
                let bytes = inner.frame_bytes.load(Ordering::Relaxed);
                let mut data = format!("au-{n};").into_bytes();
                data.resize(bytes, b'.');
                let frame = VideoFrame {
                    keyframe: n % every == 0,
                    timestamp_us: n * 16_667,
                    data: data.into(),
                };
                if tx.send(EncoderEvent::Frame(frame)).await.is_err() {
                    return;
                }
                let interval = inner.frame_interval_ms.load(Ordering::Relaxed);
                tokio::time::sleep(Duration::from_millis(interval)).await;
            }
        });
        Ok(RunningEncoder {
            events,
            stop: Box::new(FakeEncoderStop { display, task }),
        })
    }
}
