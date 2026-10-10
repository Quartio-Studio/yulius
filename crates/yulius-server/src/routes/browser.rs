//! `WS /api/sandboxes/{sid}/browser`: the shared sandbox browser viewer (docs/api.md "Browser,
//! editor, shell"; SPEC-001 BRW-001..008, SPEC-002 ADR-15).
//!
//! Write access is required and re-checked every 60 s; the origin must be Yulius's. The browser
//! starts on first use. A connection receives the browser's display as H.264 video (default) or,
//! when the browser has no display or the viewer asks for it, JPEG screencast frames of the shown
//! tab; `stream` says which. Input (mouse, keys) is forwarded at a bounded rate and every key and
//! button the viewer held is released when it leaves.
//!
//! Queues: JSON messages go through a bounded queue and a client that stops draining them is
//! disconnected (4008). Media go through [`MediaQueue`], bounded in frames and bytes: JPEG frames
//! keep the newest few; video is paced by the viewer's acknowledgements ([`flow`]: about a round
//! trip of frames on the way, F 2026-10-10), and a viewer whose frames wait on the server longer
//! than [`VIDEO_QUEUE_LIMIT`] skips to a fresh keyframe, which the connection asks the encoder
//! for, instead of growing a backlog (a decoder cannot skip delta frames). Its latency stays
//! bounded, it never slows the encoder or other viewers, and its reports steer the stream's
//! bitrate tier.

mod flow;

use std::collections::VecDeque;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use axum::extract::ws::{CloseFrame, Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Path, State};
use axum::http::HeaderMap;
use axum::response::Response;
use base64::Engine as _;
use futures::{SinkExt as _, StreamExt as _};
use tokio::sync::{Notify, mpsc};
use yulius_app::auth::AuthContext;
use yulius_app::browser::BrowserGrant;
use yulius_app::ports::browser::{
    BrowserFrames, BrowserNotice, BrowserSession, BrowserVideo, VideoNetworkReport, VideoUpdate,
};
use yulius_contracts::browser::{
    MouseEventKind, TabId, TabInfo, ViewMode, ViewerClientMsg, ViewerErrorCode, ViewerServerMsg,
    encode_frame, encode_video_frame,
};

use super::events::check_origin;
use crate::AppState;
use crate::auth::Auth;
use crate::error::ApiResult;

/// Permission re-check interval for open viewers (ACC-008).
const RECHECK: Duration = Duration::from_secs(60);
/// JPEG frames queued per connection; older ones are dropped for newer ones.
pub const FRAME_QUEUE: usize = 4;
/// Video frames queued per connection at most.
pub const VIDEO_QUEUE_FRAMES: usize = 300;
/// Video bytes queued per connection at most.
pub const VIDEO_QUEUE_BYTES: usize = 8 << 20;
/// How long video may wait on the server (the viewer's path is full or its socket blocks)
/// before the viewer skips to a fresh keyframe.
pub const VIDEO_QUEUE_LIMIT: Duration = Duration::from_millis(250);
/// Shortest time between two keyframe requests of one connection.
const KEYFRAME_REQUEST_SPACING: Duration = Duration::from_secs(1);
/// JSON messages queued per connection.
const CONTROL_QUEUE: usize = 64;
/// How long a JSON message may wait for queue space before the viewer is disconnected.
const CONTROL_SEND_TIMEOUT: Duration = Duration::from_secs(10);
/// Largest accepted client message (a paste is at most 32 KiB of text).
const MAX_CLIENT_MESSAGE: usize = 64 * 1024;
/// Input events per second a connection may send on average.
const INPUT_RATE: f64 = 500.0;
/// Input events a connection may send in a burst.
const INPUT_BURST: f64 = 1000.0;
/// Budget moves and wheel turns leave for discrete input (a flood of moves never makes a click
/// or a key refused).
const INPUT_RESERVE: f64 = 100.0;
/// Close code: access revoked.
const CLOSE_REVOKED: u16 = 4003;
/// Close code: the viewer did not drain its messages.
const CLOSE_LAGGING: u16 = 4008;
/// Close code: the browser failed or stopped.
const CLOSE_BROWSER_FAILED: u16 = 1011;

pub async fn browser(
    State(app): State<AppState>,
    Auth(ctx): Auth,
    Path(sid): Path<String>,
    headers: HeaderMap,
    ws: WebSocketUpgrade,
) -> ApiResult<Response> {
    check_origin(&app, &headers)?;
    let grant = app.browser_grant(&ctx, &sid).await?;
    Ok(ws
        .max_message_size(MAX_CLIENT_MESSAGE)
        .on_upgrade(move |socket| run(app, ctx, grant, socket)))
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MediaKind {
    Jpeg,
    VideoConfig,
    VideoKey,
    VideoDelta,
}

impl MediaKind {
    fn is_video_frame(self) -> bool {
        matches!(self, Self::VideoKey | Self::VideoDelta)
    }
}

#[derive(Debug)]
struct Media {
    message: Message,
    bytes: usize,
    kind: MediaKind,
    /// When it was queued.
    at: Instant,
}

/// The media queue's state and policy (pure, so it is unit-tested; time is passed in).
#[derive(Debug)]
struct MediaState {
    items: VecDeque<Media>,
    video_frames: usize,
    video_bytes: usize,
    /// Video frames are dropped until the next keyframe.
    waiting_key: bool,
    /// The connection should ask the encoder for a keyframe.
    wants_keyframe: bool,
    dropped: u64,
    closed: bool,
    flow: flow::Flow,
}

impl MediaState {
    fn new(now: Instant) -> Self {
        Self {
            items: VecDeque::new(),
            video_frames: 0,
            video_bytes: 0,
            waiting_key: false,
            wants_keyframe: false,
            dropped: 0,
            closed: false,
            flow: flow::Flow::new(now),
        }
    }

    fn push_back(&mut self, media: Media) {
        if media.kind.is_video_frame() {
            self.video_frames += 1;
            self.video_bytes += media.bytes;
        }
        self.items.push_back(media);
    }

    fn pop_front(&mut self) -> Option<Media> {
        let media = self.items.pop_front()?;
        if media.kind.is_video_frame() {
            self.video_frames -= 1;
            self.video_bytes -= media.bytes;
        }
        Some(media)
    }

    /// A JPEG frame: the newest [`FRAME_QUEUE`] are kept.
    fn push_jpeg(&mut self, media: Media) {
        let jpegs = self
            .items
            .iter()
            .filter(|m| m.kind == MediaKind::Jpeg)
            .count();
        if jpegs >= FRAME_QUEUE
            && let Some(oldest) = self.items.iter().position(|m| m.kind == MediaKind::Jpeg)
        {
            self.items.remove(oldest);
            self.dropped += 1;
        }
        self.push_back(media);
    }

    /// Drops every queued video frame (configurations stay).
    fn drop_video(&mut self) -> u64 {
        let before = self.video_frames as u64;
        self.items.retain(|m| !m.kind.is_video_frame());
        self.video_frames = 0;
        self.video_bytes = 0;
        self.dropped += before;
        before
    }

    /// Whether the oldest queued video frame has waited longer than [`VIDEO_QUEUE_LIMIT`].
    fn lagging(&self, now: Instant) -> bool {
        self.items
            .iter()
            .find(|m| m.kind.is_video_frame())
            .is_some_and(|m| now.saturating_duration_since(m.at) > VIDEO_QUEUE_LIMIT)
    }

    /// A video packet at `now`.
    fn push_video(&mut self, media: Media, now: Instant) {
        match media.kind {
            MediaKind::VideoConfig => self.push_back(media),
            MediaKind::VideoKey => {
                self.waiting_key = false;
                self.wants_keyframe = false;
                // Everything still queued is superseded: the decoder can start here.
                self.drop_video();
                self.push_back(media);
            }
            MediaKind::VideoDelta => {
                let full = self.video_frames >= VIDEO_QUEUE_FRAMES
                    || self.video_bytes + media.bytes > VIDEO_QUEUE_BYTES;
                if !self.waiting_key && (full || self.lagging(now)) {
                    // Behind: catch up at a fresh keyframe instead of growing the delay.
                    self.waiting_key = true;
                    self.wants_keyframe = true;
                    let dropped = self.drop_video();
                    self.flow.on_dropped(dropped);
                }
                if self.waiting_key {
                    self.dropped += 1;
                    self.flow.on_dropped(1);
                } else {
                    self.push_back(media);
                }
            }
            MediaKind::Jpeg => self.push_jpeg(media),
        }
    }

    /// The next message the writer may send at `now`: video waits while [`flow::Flow`] holds it.
    fn pop_ready(&mut self, now: Instant) -> Option<Media> {
        let front = self.items.front()?;
        if front.kind.is_video_frame() {
            if !self.flow.may_send(now) {
                self.flow.on_held();
                return None;
            }
            let media = self.pop_front()?;
            self.flow.on_send(media.bytes, now);
            return Some(media);
        }
        self.pop_front()
    }

    fn clear(&mut self) {
        self.items.clear();
        self.video_frames = 0;
        self.video_bytes = 0;
        self.waiting_key = false;
        self.wants_keyframe = false;
    }
}

/// The media queue between the pump and the socket writer (one consumer).
#[derive(Debug)]
struct MediaQueue {
    state: Mutex<MediaState>,
    ready: Notify,
}

impl Default for MediaQueue {
    fn default() -> Self {
        Self {
            state: Mutex::new(MediaState::new(Instant::now())),
            ready: Notify::new(),
        }
    }
}

impl MediaQueue {
    fn jpeg(&self, tab: &TabId, jpeg: &[u8]) {
        let frame = encode_frame(tab, jpeg);
        let media = Media {
            bytes: frame.len(),
            message: Message::Binary(frame.into()),
            kind: MediaKind::Jpeg,
            at: Instant::now(),
        };
        lock(&self.state).push_jpeg(media);
        self.ready.notify_one();
    }

    /// Queues a video update; returns whether the connection should ask for a keyframe now.
    fn video(&self, update: VideoUpdate) -> bool {
        let now = Instant::now();
        let media = match update {
            VideoUpdate::Config {
                codec,
                description,
                width,
                height,
            } => {
                let message = ViewerServerMsg::VideoConfig {
                    codec,
                    description: base64::engine::general_purpose::STANDARD.encode(&description),
                    width,
                    height,
                };
                let Ok(text) = serde_json::to_string(&message) else {
                    return false;
                };
                Media {
                    bytes: text.len(),
                    message: Message::Text(text.into()),
                    kind: MediaKind::VideoConfig,
                    at: now,
                }
            }
            VideoUpdate::Frame {
                keyframe,
                timestamp_us,
                data,
            } => {
                let frame = encode_video_frame(keyframe, timestamp_us, &data);
                Media {
                    bytes: frame.len(),
                    message: Message::Binary(frame.into()),
                    kind: if keyframe {
                        MediaKind::VideoKey
                    } else {
                        MediaKind::VideoDelta
                    },
                    at: now,
                }
            }
        };
        let wants_keyframe = {
            let mut state = lock(&self.state);
            state.push_video(media, now);
            std::mem::take(&mut state.wants_keyframe)
        };
        self.ready.notify_one();
        wants_keyframe
    }

    /// The viewer received `frames` video frames so far.
    fn ack(&self, frames: u64) {
        lock(&self.state).flow.on_ack(frames, Instant::now());
        self.ready.notify_one();
    }

    /// Whether the path has room for video now (a keyframe asked for while it is full would
    /// only wait, and restart the encoder for nothing).
    fn can_send(&self) -> bool {
        lock(&self.state).flow.may_send(Instant::now())
    }

    /// The viewer's network over the last period, when one has passed.
    fn report(&self) -> Option<flow::FlowReport> {
        lock(&self.state).flow.report(Instant::now())
    }

    fn clear(&self) {
        lock(&self.state).clear();
    }

    fn close(&self) {
        lock(&self.state).closed = true;
        self.ready.notify_one();
    }

    fn dropped(&self) -> u64 {
        lock(&self.state).dropped
    }

    async fn next(&self) -> Option<Message> {
        loop {
            {
                let mut state = lock(&self.state);
                if let Some(media) = state.pop_ready(Instant::now()) {
                    return Some(media.message);
                }
                if state.closed {
                    return None;
                }
            }
            // `notify_one` stores a permit when nobody waits, so no push or acknowledgement is
            // missed; a closed flow gate opens only with an acknowledgement.
            self.ready.notified().await;
        }
    }
}

/// Outgoing queues of one connection.
struct Outbox {
    control: mpsc::Sender<Message>,
    media: Arc<MediaQueue>,
}

impl Outbox {
    /// Queues a JSON message; `false` when the client is gone or not draining.
    async fn json(&self, message: &ViewerServerMsg) -> bool {
        let Ok(text) = serde_json::to_string(message) else {
            return false;
        };
        self.control
            .send_timeout(Message::Text(text.into()), CONTROL_SEND_TIMEOUT)
            .await
            .is_ok()
    }

    async fn close(&self, code: u16, reason: &str) {
        let frame = CloseFrame {
            code,
            reason: reason.to_owned().into(),
        };
        let _ = self
            .control
            .send_timeout(Message::Close(Some(frame)), CONTROL_SEND_TIMEOUT)
            .await;
    }
}

/// Writes queued messages to the socket, JSON first.
async fn write_loop(
    mut sink: futures::stream::SplitSink<WebSocket, Message>,
    mut control: mpsc::Receiver<Message>,
    media: Arc<MediaQueue>,
) {
    loop {
        let message = tokio::select! {
            biased;
            message = control.recv() => message,
            message = media.next() => message,
        };
        let Some(message) = message else { break };
        let closing = matches!(message, Message::Close(_));
        if sink.send(message).await.is_err() || closing {
            break;
        }
    }
    let _ = sink.close().await;
}

fn active_tab(tabs: &[TabInfo]) -> Option<TabId> {
    tabs.iter()
        .find(|tab| tab.active)
        .or_else(|| tabs.first())
        .map(|tab| tab.id.clone())
}

/// What one connection streams.
#[derive(Default)]
enum Stream {
    #[default]
    None,
    Video(Box<dyn BrowserVideo>),
    Pixels(Box<dyn BrowserFrames>),
}

enum StreamEvent {
    Video(Option<VideoUpdate>),
    Jpeg(TabId, Option<Arc<[u8]>>),
}

async fn next_media(stream: &mut Stream) -> StreamEvent {
    match stream {
        Stream::None => std::future::pending().await,
        Stream::Video(video) => StreamEvent::Video(video.next().await),
        Stream::Pixels(frames) => {
            let tab = frames.tab_id().clone();
            StreamEvent::Jpeg(tab, frames.next().await)
        }
    }
}

/// The connection's view: the mode it asked for and what it streams.
struct View {
    mode: ViewMode,
    stream: Stream,
    /// The `stream` message last sent (mode and reason).
    announced: Option<(ViewMode, Option<String>)>,
    /// A keyframe is wanted but was asked for too recently.
    keyframe_due: bool,
    keyframe_requested: Option<Instant>,
}

impl View {
    fn new() -> Self {
        Self {
            mode: ViewMode::Video,
            stream: Stream::None,
            announced: None,
            keyframe_due: false,
            keyframe_requested: None,
        }
    }

    /// Streams `tab` (`None`: nothing) in the asked mode: the display's video, or the tab's JPEG
    /// frames when video is unavailable or not asked for. `false` when the client is gone.
    async fn watch(
        &mut self,
        session: &Arc<dyn BrowserSession>,
        tab: Option<&TabId>,
        out: &Outbox,
    ) -> bool {
        let Some(tab) = tab else {
            self.stream = Stream::None;
            out.media.clear();
            return true;
        };
        if self.mode == ViewMode::Video {
            // One display shows one tab: an existing video stream already shows it.
            if matches!(self.stream, Stream::Video(_)) {
                return true;
            }
            self.stream = Stream::None;
            out.media.clear();
            match session.video() {
                Ok(mut video) => {
                    for update in video.take_backlog() {
                        out.media.video(update);
                    }
                    self.stream = Stream::Video(video);
                    return self.announce(ViewMode::Video, None, out).await;
                }
                Err(reason) => {
                    if !self.announce(ViewMode::Pixels, Some(reason), out).await {
                        return false;
                    }
                }
            }
        } else {
            out.media.clear();
            if !self.announce(ViewMode::Pixels, None, out).await {
                return false;
            }
        }
        self.stream = Stream::None;
        match session.frames(tab).await {
            Ok(frames) => self.stream = Stream::Pixels(frames),
            Err(failure) => tracing::debug!(%failure, "screencast subscription failed"),
        }
        true
    }

    /// Asks the encoder for a keyframe when one is due and the path has room for it, at most
    /// once per [`KEYFRAME_REQUEST_SPACING`] (a later call retries: on acknowledgements and every
    /// report period).
    fn request_keyframe(&mut self, due: bool, out: &Outbox) {
        self.keyframe_due |= due;
        if !self.keyframe_due || !out.media.can_send() {
            return;
        }
        let now = Instant::now();
        if self
            .keyframe_requested
            .is_some_and(|at| now.saturating_duration_since(at) < KEYFRAME_REQUEST_SPACING)
        {
            return;
        }
        if let Stream::Video(video) = &mut self.stream {
            video.request_keyframe();
            self.keyframe_requested = Some(now);
        }
        self.keyframe_due = false;
    }

    /// Passes the viewer's network report to the video stream.
    fn report(&mut self, report: flow::FlowReport) {
        if let Stream::Video(video) = &mut self.stream {
            video.report(VideoNetworkReport {
                delivered_bps: report.delivered_bps,
                congested: report.congested,
                limited: report.limited,
            });
        }
        tracing::trace!(
            delivered_kbps = report.delivered_bps / 1000,
            congested = report.congested,
            limited = report.limited,
            rtt_ms = report.rtt.map(|d| d.as_millis()),
            base_ms = report.base_rtt.map(|d| d.as_millis()),
            "browser viewer network"
        );
    }

    async fn announce(&mut self, mode: ViewMode, reason: Option<String>, out: &Outbox) -> bool {
        let next = Some((mode, reason.clone()));
        if self.announced == next {
            return true;
        }
        self.announced = next;
        out.json(&ViewerServerMsg::Stream { mode, reason }).await
    }
}

/// A token bucket bounding a connection's input rate.
struct InputBudget {
    tokens: f64,
    at: Instant,
}

impl InputBudget {
    fn new() -> Self {
        Self {
            tokens: INPUT_BURST,
            at: Instant::now(),
        }
    }

    /// Takes one event's budget; a droppable event (a move, a wheel turn) only beyond the
    /// reserve kept for discrete input.
    fn take(&mut self, now: Instant, droppable: bool) -> bool {
        let elapsed = now.saturating_duration_since(self.at).as_secs_f64();
        self.at = now;
        self.tokens = (self.tokens + elapsed * INPUT_RATE).min(INPUT_BURST);
        let floor = if droppable { INPUT_RESERVE } else { 0.0 };
        if self.tokens >= 1.0 + floor {
            self.tokens -= 1.0;
            true
        } else {
            false
        }
    }
}

async fn run(app: AppState, ctx: AuthContext, grant: BrowserGrant, socket: WebSocket) {
    let (sink, mut stream) = socket.split();
    let (control_tx, control_rx) = mpsc::channel(CONTROL_QUEUE);
    let media = Arc::new(MediaQueue::default());
    let writer = tokio::spawn(write_loop(sink, control_rx, Arc::clone(&media)));
    let out = Outbox {
        control: control_tx,
        media: Arc::clone(&media),
    };
    match app.browser_session(&grant.sandbox_id).await {
        Ok(session) => {
            pump(&app, &ctx, &grant, &session, &mut stream, &out).await;
            // Nothing this viewer pressed stays held on the display.
            session.release_input();
        }
        Err(failure) => {
            let _ = out
                .json(&ViewerServerMsg::Error {
                    code: failure.viewer_code(),
                    message: failure.message.clone(),
                })
                .await;
            out.close(CLOSE_BROWSER_FAILED, "browser unavailable").await;
        }
    }
    if media.dropped() > 0 {
        tracing::debug!(dropped = media.dropped(), "browser viewer dropped frames");
    }
    drop(out);
    media.close();
    let _ = tokio::time::timeout(CONTROL_SEND_TIMEOUT, writer).await;
}

/// Whether an input message may be dropped silently when the budget is exhausted.
fn droppable(message: &ViewerClientMsg) -> bool {
    matches!(
        message,
        ViewerClientMsg::Mouse {
            event: MouseEventKind::Move | MouseEventKind::Wheel,
            ..
        }
    )
}

async fn pump(
    app: &AppState,
    ctx: &AuthContext,
    grant: &BrowserGrant,
    session: &Arc<dyn BrowserSession>,
    stream: &mut futures::stream::SplitStream<WebSocket>,
    out: &Outbox,
) {
    let mut tabs_rx = session.watch_tabs();
    let mut notices = session.notices();
    let tabs = tabs_rx.borrow_and_update().clone();
    if !out
        .json(&ViewerServerMsg::Tabs { tabs: tabs.clone() })
        .await
    {
        return;
    }
    let (width, height) = session.viewport();
    if !out.json(&ViewerServerMsg::Viewport { width, height }).await {
        return;
    }
    let mut watching = active_tab(&tabs);
    // `Some(choice)` once the viewer chose what to watch (`watch`); until then it follows the
    // shared active tab.
    let mut chosen: Option<Option<TabId>> = None;
    let mut view = View::new();
    if !view.watch(session, watching.as_ref(), out).await {
        return;
    }
    let mut budget = InputBudget::new();
    let mut recheck = tokio::time::interval(RECHECK);
    recheck.tick().await;
    let mut reports = tokio::time::interval(flow::REPORT_PERIOD);
    reports.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            changed = tabs_rx.changed() => {
                if changed.is_err() {
                    out.close(CLOSE_BROWSER_FAILED, "browser stopped").await;
                    return;
                }
                let tabs = tabs_rx.borrow_and_update().clone();
                // The display shows the active tab (a watched tab was brought to the front; when
                // it closes the next one is); `watch null` streams nothing.
                let next = match &chosen {
                    Some(None) => None,
                    _ => active_tab(&tabs),
                };
                if !out.json(&ViewerServerMsg::Tabs { tabs }).await {
                    out.close(CLOSE_LAGGING, "lagging; reconnect").await;
                    return;
                }
                let idle = matches!(view.stream, Stream::None);
                if next != watching || (idle && next.is_some()) {
                    // Pixels follow the shown tab; video keeps its stream.
                    if matches!(view.stream, Stream::Pixels(_)) {
                        view.stream = Stream::None;
                    }
                    if !view.watch(session, next.as_ref(), out).await {
                        return;
                    }
                    watching = next;
                }
            }
            notice = notices.next() => {
                match notice {
                    Some(BrowserNotice::ActionRequested { tab_id, reason, task_id }) => {
                        if !out.json(&ViewerServerMsg::ActionRequested { tab_id, reason, task_id }).await {
                            return;
                        }
                    }
                    Some(BrowserNotice::ActionCleared { .. }) => {}
                    Some(BrowserNotice::AgentActivity { tab_id, x, y, action, label }) => {
                        let message = ViewerServerMsg::AgentActivity { tab_id, x, y, action, label };
                        if !out.json(&message).await {
                            return;
                        }
                    }
                    Some(BrowserNotice::Viewport { width, height }) => {
                        if !out.json(&ViewerServerMsg::Viewport { width, height }).await {
                            return;
                        }
                    }
                    Some(BrowserNotice::Disconnected) | None => {
                        let _ = out.json(&ViewerServerMsg::Error {
                            code: ViewerErrorCode::Upstream,
                            message: "the sandbox browser stopped; reconnect to restart it".into(),
                        }).await;
                        out.close(CLOSE_BROWSER_FAILED, "browser stopped").await;
                        return;
                    }
                }
            }
            event = next_media(&mut view.stream) => {
                match event {
                    StreamEvent::Video(Some(update)) => {
                        let behind = out.media.video(update);
                        view.request_keyframe(behind, out);
                    }
                    StreamEvent::Jpeg(tab, Some(jpeg)) => out.media.jpeg(&tab, &jpeg),
                    // The video ended (its encoder failed): fall back to pixels.
                    StreamEvent::Video(None) => {
                        view.stream = Stream::None;
                        if !view.watch(session, watching.as_ref(), out).await {
                            return;
                        }
                    }
                    // The tab closed; a tab-list update selects the next one.
                    StreamEvent::Jpeg(_, None) => view.stream = Stream::None,
                }
            }
            incoming = stream.next() => {
                let Some(Ok(message)) = incoming else { return };
                let text = match message {
                    Message::Text(text) => text,
                    Message::Close(_) => return,
                    Message::Binary(_) => {
                        let error = ViewerServerMsg::Error {
                            code: ViewerErrorCode::Invalid,
                            message: "binary messages are not accepted".into(),
                        };
                        if !out.json(&error).await { return; }
                        continue;
                    }
                    _ => continue,
                };
                let result = match serde_json::from_str::<ViewerClientMsg>(&text) {
                    // Flow control and recovery of this connection's video.
                    Ok(ViewerClientMsg::Ack { frames }) => {
                        out.media.ack(frames);
                        view.request_keyframe(false, out);
                        Ok(())
                    }
                    Ok(ViewerClientMsg::Keyframe) => {
                        view.request_keyframe(true, out);
                        Ok(())
                    }
                    // Per-connection choices.
                    Ok(ViewerClientMsg::View { mode }) => {
                        view.mode = mode;
                        view.stream = Stream::None;
                        if !view.watch(session, watching.as_ref(), out).await {
                            return;
                        }
                        Ok(())
                    }
                    Ok(ViewerClientMsg::Watch { tab_id }) => {
                        chosen = Some(tab_id.clone());
                        // The display shows one tab: watching a tab brings it to the front.
                        let shown = match &tab_id {
                            Some(tab) => session
                                .apply_viewer(ViewerClientMsg::Activate { tab_id: tab.clone() })
                                .await
                                .map_err(|failure| (failure.viewer_code(), failure.message)),
                            None => Ok(()),
                        };
                        if matches!(view.stream, Stream::Pixels(_)) {
                            view.stream = Stream::None;
                        }
                        watching = tab_id;
                        if !view.watch(session, watching.as_ref(), out).await {
                            return;
                        }
                        shown
                    }
                    Ok(ViewerClientMsg::Copy { tab_id }) => {
                        match session.copy_selection(&tab_id).await {
                            Ok(text) => {
                                if !out.json(&ViewerServerMsg::Clipboard { text }).await {
                                    return;
                                }
                                Ok(())
                            }
                            Err(failure) => Err((failure.viewer_code(), failure.message)),
                        }
                    }
                    Ok(message @ (ViewerClientMsg::Mouse { .. } | ViewerClientMsg::Key { .. })) => {
                        let silent = droppable(&message);
                        if budget.take(Instant::now(), silent) {
                            match session.apply_viewer(message).await {
                                Ok(_) => Ok(()),
                                // A move or wheel that did not apply is superseded by the
                                // next one: never an error for the user (F 2026-10-10).
                                Err(failure) if silent => {
                                    tracing::debug!(%failure, "viewer move or wheel not applied");
                                    Ok(())
                                }
                                Err(failure) => Err((failure.viewer_code(), failure.message)),
                            }
                        } else if silent {
                            Ok(())
                        } else {
                            Err((ViewerErrorCode::Invalid, "input arrives too fast; slow down".into()))
                        }
                    }
                    Ok(message) => session.apply_viewer(message).await.map_err(|failure| {
                        (failure.viewer_code(), failure.message)
                    }),
                    Err(err) => Err((ViewerErrorCode::Invalid, format!("invalid message: {err}"))),
                };
                if let Err((code, message)) = result
                    && !out.json(&ViewerServerMsg::Error { code, message }).await
                {
                    return;
                }
            }
            _ = reports.tick() => {
                if let Some(report) = out.media.report() {
                    view.report(report);
                }
                view.request_keyframe(false, out);
            }
            _ = recheck.tick() => {
                if !app.still_signed_in(ctx).await {
                    out.close(CLOSE_REVOKED, "signed out").await;
                    return;
                }
                if !app.browser_still_allowed(ctx, grant).await {
                    out.close(CLOSE_REVOKED, "access revoked").await;
                    return;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn media_at(kind: MediaKind, bytes: usize, at: Instant) -> Media {
        Media {
            message: Message::Binary(vec![0_u8; bytes].into()),
            bytes,
            kind,
            at,
        }
    }

    fn kinds(state: &MediaState) -> Vec<MediaKind> {
        state.items.iter().map(|m| m.kind).collect()
    }

    fn ms(millis: u64) -> Duration {
        Duration::from_millis(millis)
    }

    #[test]
    fn a_joining_viewer_gets_its_whole_cached_group() {
        let t0 = Instant::now();
        let mut state = MediaState::new(t0);
        state.push_video(media_at(MediaKind::VideoConfig, 10, t0), t0);
        state.push_video(media_at(MediaKind::VideoKey, 1000, t0), t0);
        for _ in 0..119 {
            state.push_video(media_at(MediaKind::VideoDelta, 50, t0), t0);
        }
        assert_eq!(state.items.len(), 121);
        assert_eq!(state.dropped, 0);
    }

    #[test]
    fn video_waiting_too_long_skips_to_a_fresh_keyframe_and_asks_for_one() {
        let t0 = Instant::now();
        let mut state = MediaState::new(t0);
        state.push_video(media_at(MediaKind::VideoKey, 100, t0), t0);
        // The writer cannot send (the path is full): frames wait on the server.
        let mut now = t0;
        while now.saturating_duration_since(t0) <= VIDEO_QUEUE_LIMIT {
            now += ms(17);
            state.push_video(media_at(MediaKind::VideoDelta, 10, now), now);
        }
        // The oldest frame waited beyond the limit: everything queued goes, a keyframe is wanted.
        assert!(state.waiting_key);
        assert!(state.wants_keyframe);
        assert_eq!(state.video_frames, 0);
        // Deltas keep being dropped until a keyframe arrives…
        state.push_video(media_at(MediaKind::VideoDelta, 10, now), now);
        assert!(state.items.is_empty());
        // …which resumes the stream.
        state.push_video(media_at(MediaKind::VideoKey, 100, now), now);
        state.push_video(media_at(MediaKind::VideoDelta, 10, now), now);
        assert_eq!(kinds(&state), [MediaKind::VideoKey, MediaKind::VideoDelta]);
        assert!(!state.waiting_key && !state.wants_keyframe);
    }

    #[test]
    fn a_keyframe_supersedes_what_is_queued_but_keeps_configs() {
        let t0 = Instant::now();
        let mut state = MediaState::new(t0);
        state.push_video(media_at(MediaKind::VideoConfig, 10, t0), t0);
        state.push_video(media_at(MediaKind::VideoKey, 100, t0), t0);
        for _ in 0..100 {
            state.push_video(media_at(MediaKind::VideoDelta, 10, t0), t0);
        }
        state.push_video(media_at(MediaKind::VideoKey, 100, t0), t0);
        assert_eq!(kinds(&state), [MediaKind::VideoConfig, MediaKind::VideoKey]);
        assert_eq!((state.video_frames, state.video_bytes), (1, 100));
    }

    #[test]
    fn the_queue_is_bounded_in_bytes() {
        let t0 = Instant::now();
        let mut state = MediaState::new(t0);
        state.push_video(
            media_at(MediaKind::VideoKey, VIDEO_QUEUE_BYTES - 10, t0),
            t0,
        );
        state.push_video(media_at(MediaKind::VideoDelta, 20, t0), t0);
        assert!(state.waiting_key, "beyond the byte bound");
        assert!(state.wants_keyframe);
        assert_eq!(state.video_frames, 0);
    }

    #[test]
    fn jpeg_frames_keep_the_newest_few() {
        let t0 = Instant::now();
        let mut state = MediaState::new(t0);
        for bytes in 1..=(FRAME_QUEUE + 3) {
            state.push_jpeg(media_at(MediaKind::Jpeg, bytes, t0));
        }
        let sizes: Vec<usize> = state.items.iter().map(|m| m.bytes).collect();
        assert_eq!(sizes, (4..=FRAME_QUEUE + 3).collect::<Vec<_>>());
        assert_eq!(state.dropped, 3);
    }

    #[test]
    fn an_acknowledging_viewer_gets_about_a_round_trip_of_video_and_configs_always() {
        let t0 = Instant::now();
        let mut state = MediaState::new(t0);
        state.push_video(media_at(MediaKind::VideoConfig, 10, t0), t0);
        state.push_video(media_at(MediaKind::VideoKey, 1000, t0), t0);
        assert!(
            state
                .pop_ready(t0)
                .is_some_and(|m| m.kind == MediaKind::VideoConfig)
        );
        assert!(
            state
                .pop_ready(t0)
                .is_some_and(|m| m.kind == MediaKind::VideoKey)
        );
        state.flow.on_ack(1, t0 + ms(40)); // base round trip 40 ms
        // Frames keep coming; the viewer stops acknowledging (its path is full).
        let mut sent = 0;
        for n in 1..=20_u64 {
            let now = t0 + ms(40 + n * 17);
            state.push_video(media_at(MediaKind::VideoDelta, 500, now), now);
            while state.pop_ready(now).is_some() {
                sent += 1;
            }
        }
        assert!(
            (8..=11).contains(&sent),
            "about RTT + slack of frames went out: {sent}"
        );
        // A configuration is never held back by the gate.
        let now = t0 + ms(400);
        state
            .items
            .push_front(media_at(MediaKind::VideoConfig, 10, now));
        assert!(
            state
                .pop_ready(now)
                .is_some_and(|m| m.kind == MediaKind::VideoConfig)
        );
        assert!(state.pop_ready(now).is_none(), "video waits");
        // The acknowledgement reopens the gate.
        state.flow.on_ack(1 + sent, now);
        assert!(state.pop_ready(now).is_some());
    }

    #[test]
    fn the_input_budget_allows_bursts_and_refills() {
        let start = Instant::now();
        let mut budget = InputBudget::new();
        budget.at = start;
        let moves = (0..2000).filter(|_| budget.take(start, true)).count();
        assert_eq!(moves, (INPUT_BURST - INPUT_RESERVE) as usize);
        // The reserve still lets discrete input through after a flood of moves.
        let clicks = (0..2000).filter(|_| budget.take(start, false)).count();
        assert_eq!(clicks, INPUT_RESERVE as usize);
        assert!(!budget.take(start, false));
        assert!(
            budget.take(start + Duration::from_millis(10), false),
            "5 tokens later"
        );
    }

    #[tokio::test]
    async fn the_media_queue_wakes_its_writer_and_ends_when_closed() {
        let queue = Arc::new(MediaQueue::default());
        let reader = {
            let queue = Arc::clone(&queue);
            tokio::spawn(async move {
                let mut count = 0;
                while queue.next().await.is_some() {
                    count += 1;
                }
                count
            })
        };
        let tab = TabId::parse("T1").map_err(|_| "tab").ok();
        if let Some(tab) = tab {
            queue.jpeg(&tab, b"jpeg");
        }
        let _ = queue.video(VideoUpdate::Frame {
            keyframe: true,
            timestamp_us: 1,
            data: vec![1_u8].into(),
        });
        tokio::time::sleep(Duration::from_millis(50)).await;
        queue.close();
        let count = tokio::time::timeout(Duration::from_secs(5), reader).await;
        assert!(matches!(count, Ok(Ok(2))), "{count:?}");
    }
}
