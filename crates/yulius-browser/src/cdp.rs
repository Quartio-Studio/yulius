//! Chrome DevTools Protocol client over one WebSocket (flattened sessions).
//!
//! Bounds and ownership:
//!
//! - one reader task and one writer task per connection, aborted when the last [`CdpClient`]
//!   clone is dropped;
//! - requests carry increasing JSON-RPC ids; the pending map holds at most
//!   [`CdpConfig::max_pending`] entries and every call has a timeout ([`CdpConfig::call_timeout`],
//!   10 s by default); timed-out or cancelled calls remove their entry;
//! - the writer queue is bounded ([`CdpConfig::outgoing_queue`]); callers wait for space, the
//!   reader never does;
//! - ordinary events fan out on a bounded broadcast channel: a lagging subscriber loses the
//!   oldest events and is told so (`RecvError::Lagged`) and must resync;
//! - `Page.screencastFrame` events are acknowledged by the reader immediately (Chromium keeps
//!   at most two unacknowledged frames in flight) and delivered to the per-session frame sink,
//!   a bounded broadcast channel that drops the oldest frames for slow consumers. The socket
//!   reader never blocks on a consumer.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use base64::Engine as _;
use futures::stream::{SplitSink, SplitStream};
use futures::{SinkExt as _, StreamExt as _};
use serde_json::{Value, json};
use tokio::net::TcpStream;
use tokio::sync::{broadcast, mpsc, oneshot, watch};
use tokio::task::JoinHandle;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::protocol::WebSocketConfig;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};
use tracing::{debug, warn};

use crate::error::CdpError;

type Socket = WebSocketStream<MaybeTlsStream<TcpStream>>;

/// Limits of one CDP connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CdpConfig {
    /// Per-call response timeout.
    pub call_timeout: Duration,
    /// WebSocket handshake timeout.
    pub connect_timeout: Duration,
    /// Maximum concurrent requests awaiting a response.
    pub max_pending: usize,
    /// Capacity of the writer queue (messages).
    pub outgoing_queue: usize,
    /// Capacity of the event broadcast channel (events).
    pub event_buffer: usize,
    /// Largest accepted incoming message (bytes); screenshots of large pages need several MiB.
    pub max_message_bytes: usize,
}

impl Default for CdpConfig {
    fn default() -> Self {
        Self {
            call_timeout: Duration::from_secs(10),
            connect_timeout: Duration::from_secs(10),
            max_pending: 256,
            outgoing_queue: 1024,
            event_buffer: 1024,
            max_message_bytes: 64 << 20,
        }
    }
}

/// A CDP event other than `Page.screencastFrame`.
#[derive(Debug, Clone, PartialEq)]
pub struct CdpEvent {
    /// Method name, e.g. `Target.targetCreated`.
    pub method: String,
    /// Flattened session the event belongs to; `None` for browser-level events.
    pub session_id: Option<String>,
    /// Event parameters.
    pub params: Value,
}

/// Screencast frame metadata reported by Chromium.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct FrameMetadata {
    /// Device width in CSS pixels.
    pub device_width: f64,
    /// Device height in CSS pixels.
    pub device_height: f64,
    /// Top offset in CSS pixels.
    pub offset_top: f64,
    /// Page scale factor.
    pub page_scale_factor: f64,
    /// Frame timestamp (seconds), when present.
    pub timestamp: Option<f64>,
}

/// One decoded screencast frame. Cloning is cheap (shared bytes).
#[derive(Debug, Clone, PartialEq)]
pub struct ScreencastFrame {
    /// JPEG bytes.
    pub jpeg: Arc<[u8]>,
    /// Frame metadata.
    pub metadata: FrameMetadata,
}

/// Per-session frame fan-out: a bounded broadcast channel (oldest frames dropped for slow
/// subscribers) plus the latest frame, so a new subscriber sees the current screen at once.
#[derive(Debug)]
pub struct FrameSink {
    sender: broadcast::Sender<ScreencastFrame>,
    latest: Mutex<Option<ScreencastFrame>>,
}

impl FrameSink {
    /// A sink retaining at most `capacity` undelivered frames per subscriber.
    #[must_use]
    pub fn new(capacity: usize) -> Self {
        let (sender, _) = broadcast::channel(capacity.max(1));
        Self {
            sender,
            latest: Mutex::new(None),
        }
    }

    /// A new receiver of subsequent frames.
    #[must_use]
    pub fn subscribe(&self) -> broadcast::Receiver<ScreencastFrame> {
        self.sender.subscribe()
    }

    /// The most recent frame, if any arrived.
    #[must_use]
    pub fn latest(&self) -> Option<ScreencastFrame> {
        lock(&self.latest).clone()
    }

    /// Live receivers.
    #[must_use]
    pub fn receiver_count(&self) -> usize {
        self.sender.receiver_count()
    }

    fn publish(&self, frame: ScreencastFrame) {
        *lock(&self.latest) = Some(frame.clone());
        // No receivers is fine: the frame is only kept as the latest.
        let _ = self.sender.send(frame);
    }
}

struct RpcError {
    code: i64,
    message: String,
}

type Responder = oneshot::Sender<Result<Value, RpcError>>;

struct Shared {
    url: String,
    config: CdpConfig,
    outgoing: mpsc::Sender<Message>,
    pending: Mutex<HashMap<u64, Responder>>,
    next_id: AtomicU64,
    events: broadcast::Sender<CdpEvent>,
    frame_sinks: Mutex<HashMap<String, Arc<FrameSink>>>,
    closed: watch::Sender<bool>,
    dropped_acks: AtomicU64,
}

struct Tasks {
    reader: JoinHandle<()>,
    writer: JoinHandle<()>,
}

impl Drop for Tasks {
    fn drop(&mut self) {
        self.reader.abort();
        self.writer.abort();
    }
}

/// A connected CDP client. Clones share the connection.
#[derive(Clone)]
pub struct CdpClient {
    shared: Arc<Shared>,
    _tasks: Arc<Tasks>,
}

impl std::fmt::Debug for CdpClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CdpClient")
            .field("url", &self.shared.url)
            .field("closed", &self.is_closed())
            .finish_non_exhaustive()
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

impl CdpClient {
    /// Connects to a DevTools WebSocket URL (`ws://127.0.0.1:<port>/devtools/browser/<id>`).
    ///
    /// # Errors
    /// [`CdpError::Connect`] when the handshake fails or exceeds the connect timeout.
    pub async fn connect(url: &str, config: CdpConfig) -> Result<Self, CdpError> {
        let ws_config = WebSocketConfig::default()
            .max_message_size(Some(config.max_message_bytes))
            .max_frame_size(Some(config.max_message_bytes));
        let connecting = tokio_tungstenite::connect_async_with_config(url, Some(ws_config), false);
        let (socket, _response) = tokio::time::timeout(config.connect_timeout, connecting)
            .await
            .map_err(|_| {
                CdpError::Connect(format!("timed out after {:?}", config.connect_timeout))
            })?
            .map_err(|error| CdpError::Connect(error.to_string()))?;
        let (sink, stream) = socket.split();
        let (outgoing, outgoing_rx) = mpsc::channel(config.outgoing_queue.max(1));
        let (events, _) = broadcast::channel(config.event_buffer.max(1));
        let (closed, _) = watch::channel(false);
        let shared = Arc::new(Shared {
            url: url.to_owned(),
            config,
            outgoing,
            pending: Mutex::new(HashMap::new()),
            next_id: AtomicU64::new(1),
            events,
            frame_sinks: Mutex::new(HashMap::new()),
            closed,
            dropped_acks: AtomicU64::new(0),
        });
        let reader = tokio::spawn(read_loop(Arc::clone(&shared), stream));
        let writer = tokio::spawn(write_loop(Arc::clone(&shared), sink, outgoing_rx));
        Ok(Self {
            shared,
            _tasks: Arc::new(Tasks { reader, writer }),
        })
    }

    /// The URL this client connected to.
    #[must_use]
    pub fn url(&self) -> &str {
        &self.shared.url
    }

    /// Calls a browser-level method.
    ///
    /// # Errors
    /// [`CdpError::Timeout`], [`CdpError::Closed`], [`CdpError::Busy`] or
    /// [`CdpError::Protocol`].
    pub async fn call(&self, method: &str, params: Value) -> Result<Value, CdpError> {
        self.send_call(None, method, params).await
    }

    /// Calls a method on a flattened target session.
    ///
    /// # Errors
    /// As [`CdpClient::call`].
    pub async fn call_session(
        &self,
        session_id: &str,
        method: &str,
        params: Value,
    ) -> Result<Value, CdpError> {
        self.send_call(Some(session_id), method, params).await
    }

    async fn send_call(
        &self,
        session_id: Option<&str>,
        method: &str,
        params: Value,
    ) -> Result<Value, CdpError> {
        if self.is_closed() {
            return Err(CdpError::Closed);
        }
        let id = self.shared.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        {
            let mut pending = lock(&self.shared.pending);
            if pending.len() >= self.shared.config.max_pending {
                return Err(CdpError::Busy(self.shared.config.max_pending));
            }
            pending.insert(id, tx);
        }
        let _entry = PendingEntry {
            shared: &self.shared,
            id,
        };
        let mut message = json!({"id": id, "method": method, "params": params});
        if let Some(session_id) = session_id {
            message["sessionId"] = Value::String(session_id.to_owned());
        }
        let timeout = self.shared.config.call_timeout;
        let exchange = async {
            self.shared
                .outgoing
                .send(Message::text(message.to_string()))
                .await
                .map_err(|_| CdpError::Closed)?;
            match rx.await {
                Ok(Ok(result)) => Ok(result),
                Ok(Err(error)) => Err(CdpError::Protocol {
                    method: method.to_owned(),
                    code: error.code,
                    message: error.message,
                }),
                Err(_) => Err(CdpError::Closed),
            }
        };
        tokio::time::timeout(timeout, exchange)
            .await
            .unwrap_or_else(|_| {
                Err(CdpError::Timeout {
                    method: method.to_owned(),
                    timeout,
                })
            })
    }

    /// Subscribes to events (all sessions). Subscribe before triggering the event.
    #[must_use]
    pub fn subscribe_events(&self) -> broadcast::Receiver<CdpEvent> {
        self.shared.events.subscribe()
    }

    /// Routes `Page.screencastFrame` events of `session_id` to `sink`.
    pub fn register_frame_sink(&self, session_id: &str, sink: Arc<FrameSink>) {
        lock(&self.shared.frame_sinks).insert(session_id.to_owned(), sink);
    }

    /// Stops routing frames of `session_id` (frames are still acknowledged).
    pub fn remove_frame_sink(&self, session_id: &str) {
        lock(&self.shared.frame_sinks).remove(session_id);
    }

    /// Whether the connection is closed.
    #[must_use]
    pub fn is_closed(&self) -> bool {
        *self.shared.closed.borrow()
    }

    /// A watch that turns `true` when the connection closes.
    #[must_use]
    pub fn close_signal(&self) -> watch::Receiver<bool> {
        self.shared.closed.subscribe()
    }

    /// Resolves when the connection is closed.
    pub async fn closed(&self) {
        wait_closed(&mut self.close_signal()).await;
    }

    /// Requests currently awaiting a response (diagnostics and tests).
    #[must_use]
    pub fn pending_requests(&self) -> usize {
        lock(&self.shared.pending).len()
    }

    /// Screencast acknowledgements dropped because the writer queue was full.
    #[must_use]
    pub fn dropped_acks(&self) -> u64 {
        self.shared.dropped_acks.load(Ordering::Relaxed)
    }
}

/// Removes a pending entry when its call finishes, times out or is cancelled.
struct PendingEntry<'a> {
    shared: &'a Shared,
    id: u64,
}

impl Drop for PendingEntry<'_> {
    fn drop(&mut self) {
        lock(&self.shared.pending).remove(&self.id);
    }
}

impl Shared {
    fn close(&self) {
        self.closed.send_replace(true);
        lock(&self.pending).clear();
        lock(&self.frame_sinks).clear();
    }

    fn dispatch(&self, text: &str) {
        let mut message: Value = match serde_json::from_str(text) {
            Ok(value) => value,
            Err(error) => {
                warn!(%error, "ignoring malformed DevTools message");
                return;
            }
        };
        if let Some(id) = message.get("id").and_then(Value::as_u64) {
            let responder = lock(&self.pending).remove(&id);
            if let Some(responder) = responder {
                let outcome = match message.get("error") {
                    Some(error) => Err(RpcError {
                        code: error.get("code").and_then(Value::as_i64).unwrap_or(0),
                        message: error
                            .get("message")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_owned(),
                    }),
                    None => Ok(message
                        .get_mut("result")
                        .map(Value::take)
                        .unwrap_or(Value::Null)),
                };
                let _ = responder.send(outcome);
            }
            return;
        }
        let Some(method) = message
            .get("method")
            .and_then(Value::as_str)
            .map(str::to_owned)
        else {
            return;
        };
        let session_id = message
            .get("sessionId")
            .and_then(Value::as_str)
            .map(str::to_owned);
        let params = message
            .get_mut("params")
            .map(Value::take)
            .unwrap_or(Value::Null);
        if method == "Page.screencastFrame" {
            self.on_frame(session_id, &params);
            return;
        }
        let _ = self.events.send(CdpEvent {
            method,
            session_id,
            params,
        });
    }

    fn on_frame(&self, session_id: Option<String>, params: &Value) {
        let frame_session = params.get("sessionId").and_then(Value::as_i64).unwrap_or(0);
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let mut ack = json!({
            "id": id,
            "method": "Page.screencastFrameAck",
            "params": {"sessionId": frame_session},
        });
        if let Some(session_id) = &session_id {
            ack["sessionId"] = Value::String(session_id.clone());
        }
        if self
            .outgoing
            .try_send(Message::text(ack.to_string()))
            .is_err()
        {
            self.dropped_acks.fetch_add(1, Ordering::Relaxed);
            warn!("screencast acknowledgement dropped: DevTools writer queue full");
        }
        let sink =
            session_id.and_then(|session_id| lock(&self.frame_sinks).get(&session_id).cloned());
        let Some(sink) = sink else {
            return;
        };
        let Some(data) = params.get("data").and_then(Value::as_str) else {
            return;
        };
        match base64::engine::general_purpose::STANDARD.decode(data) {
            Ok(jpeg) => sink.publish(ScreencastFrame {
                jpeg: jpeg.into(),
                metadata: parse_metadata(params.get("metadata")),
            }),
            Err(error) => warn!(%error, "dropping screencast frame with invalid base64"),
        }
    }
}

fn parse_metadata(metadata: Option<&Value>) -> FrameMetadata {
    let number = |name: &str| metadata.and_then(|m| m.get(name)).and_then(Value::as_f64);
    FrameMetadata {
        device_width: number("deviceWidth").unwrap_or(0.0),
        device_height: number("deviceHeight").unwrap_or(0.0),
        offset_top: number("offsetTop").unwrap_or(0.0),
        page_scale_factor: number("pageScaleFactor").unwrap_or(1.0),
        timestamp: number("timestamp"),
    }
}

async fn read_loop(shared: Arc<Shared>, mut stream: SplitStream<Socket>) {
    while let Some(message) = stream.next().await {
        match message {
            Ok(Message::Text(text)) => shared.dispatch(text.as_str()),
            Ok(Message::Close(_)) => break,
            Ok(_) => {}
            Err(error) => {
                debug!(%error, "DevTools socket read failed");
                break;
            }
        }
    }
    shared.close();
}

/// Resolves once the watch reads `true` (or its sender is gone); holds no borrow afterwards.
pub(crate) async fn wait_closed(closed: &mut watch::Receiver<bool>) {
    let _ = closed.wait_for(|closed| *closed).await;
}

async fn write_loop(
    shared: Arc<Shared>,
    mut sink: SplitSink<Socket, Message>,
    mut queue: mpsc::Receiver<Message>,
) {
    let mut closed = shared.closed.subscribe();
    loop {
        tokio::select! {
            message = queue.recv() => {
                let Some(message) = message else { break };
                if let Err(error) = sink.send(message).await {
                    debug!(%error, "DevTools socket write failed");
                    break;
                }
            }
            () = wait_closed(&mut closed) => break,
        }
    }
    let _ = tokio::time::timeout(Duration::from_secs(1), sink.close()).await;
    shared.close();
}
