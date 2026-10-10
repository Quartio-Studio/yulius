//! H.264 video of a sandbox's virtual display (SPEC-002 ADR-15).
//!
//! One [`VideoHub`] per display fans the encoder's access units out to the viewers:
//!
//! - **On demand.** The encoder runs only while at least one subscriber exists (plus a short
//!   [`IDLE_GRACE`], so switching panels does not restart it) and restarts at a new size when the
//!   viewport changes. The encoder process is terminated and reaped when it stops.
//! - **GOP cache.** The hub keeps the newest keyframe and the frames after it (at most
//!   [`GOP_MAX_FRAMES`] and [`GOP_MAX_BYTES`]); a new subscriber gets the configuration and that
//!   group first ([`VideoSubscription::take_backlog`]), so its first picture shows at once. A
//!   group beyond the bounds is not cached: new subscribers then start at the next keyframe.
//! - **Backpressure.** Each subscriber has a bounded queue ([`SUBSCRIBER_QUEUE`]); the hub never
//!   waits for one. A subscriber whose queue is full misses frames until the next keyframe (a
//!   decoder cannot skip a delta frame), so a slow viewer never grows memory or slows others.
//! - **Failure.** An encoder that keeps failing at start ([`MAX_START_FAILURES`] times in a row)
//!   marks the hub failed: subscriptions end and new ones are refused with the reason, and the
//!   viewer falls back to JPEG frames.
//! - **Adaptation (F 2026-10-10).** The encoder's bitrate is capped (x264 VBV) at a [`Tier`] of
//!   [`TIERS`]. Viewer connections report what their network delivers ([`NetworkReport`]);
//!   [`RateControl`] steps down at once when any viewer's path queues up and probes one tier up
//!   after a quiet spell, so the stream fits the slowest viewer's link instead of piling up in
//!   TCP and proxy buffers. A tier change restarts the encoder.
//! - **Keyframes on demand.** There is no periodic keyframe (each one is a burst and, capped, a
//!   moment of blur); a viewer that joins without a small cached group, falls behind or whose
//!   decoder failed asks for one ([`VideoSubscription::request_keyframe`]): the encoder restarts,
//!   at most once per [`KEYFRAME_SPACING`].

use std::sync::{Arc, Mutex, MutexGuard, PoisonError, Weak};
use std::time::{Duration, Instant};

use tokio::sync::{mpsc, watch};
use tokio::task::JoinHandle;
use tracing::{debug, info, warn};

/// Frames kept in the GOP cache.
pub const GOP_MAX_FRAMES: usize = 600;
/// Bytes kept in the GOP cache.
pub const GOP_MAX_BYTES: usize = 6 << 20;
/// A new subscriber replays the cached group only up to this size; beyond it (a slow link would
/// take long to catch up) it asks for a fresh keyframe instead.
pub const JOIN_REPLAY_BYTES: usize = 512 << 10;
/// Shortest time between encoder restarts for a keyframe.
pub const KEYFRAME_SPACING: Duration = Duration::from_secs(1);
/// Packets queued per subscriber (more than a cached group plus its configuration).
pub const SUBSCRIBER_QUEUE: usize = GOP_MAX_FRAMES + 16;
/// How long the encoder keeps running after the last subscriber left.
pub const IDLE_GRACE: Duration = Duration::from_secs(5);
/// Consecutive failed starts before the hub gives up.
pub const MAX_START_FAILURES: u32 = 3;
/// An encoder that ran at least this long before exiting counts as a success.
const HEALTHY_RUN: Duration = Duration::from_secs(10);
/// How long a replacement encoder may take to its first keyframe.
const SUCCESSOR_TIMEOUT: Duration = Duration::from_secs(3);
/// Encoder events buffered between its reader and the hub.
pub(crate) const ENCODER_EVENTS: usize = 32;

/// Decoder configuration of a stream (WebCodecs `VideoDecoderConfig`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VideoConfig {
    /// Codec string, e.g. `avc1.42c028`.
    pub codec: String,
    /// AVCDecoderConfigurationRecord (`avcC`).
    pub description: Arc<[u8]>,
    /// Coded width in device pixels.
    pub width: u32,
    /// Coded height in device pixels.
    pub height: u32,
}

/// One access unit. Cloning is cheap (shared bytes).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VideoFrame {
    /// An IDR access unit: a decoder can start here.
    pub keyframe: bool,
    /// Presentation timestamp in microseconds.
    pub timestamp_us: u64,
    /// Length-prefixed NAL units.
    pub data: Arc<[u8]>,
}

/// What a subscriber receives, in order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VideoPacket {
    /// Configuration for the frames that follow.
    Config(Arc<VideoConfig>),
    /// An access unit.
    Frame(VideoFrame),
}

/// Output of a running encoder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum EncoderEvent {
    /// A (new) decoder configuration.
    Config(Arc<VideoConfig>),
    /// An access unit.
    Frame(VideoFrame),
}

/// A quality tier of the stream: the encoder's bitrate cap and frame rate (F 2026-10-10).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Tier {
    /// x264 VBV maximum rate in kbit/s (`-maxrate`); static pages use far less.
    pub max_kbps: u32,
    /// Frames per second, at most the configured rate.
    pub fps: u32,
}

/// The tiers, best first. Measured on the VM while scrolling a text-and-pictures page at
/// 1138×766: 6 Mbit/s keeps 55 fps with readable text, 3 Mbit/s keeps 50 fps but blurs while it
/// moves; uncapped, the same scroll made 80–180 KB frames and only 36 fps.
pub const TIERS: [Tier; 7] = [
    Tier {
        max_kbps: 12_000,
        fps: 60,
    },
    Tier {
        max_kbps: 8_000,
        fps: 60,
    },
    Tier {
        max_kbps: 5_000,
        fps: 60,
    },
    Tier {
        max_kbps: 3_500,
        fps: 60,
    },
    Tier {
        max_kbps: 2_500,
        fps: 45,
    },
    Tier {
        max_kbps: 1_500,
        fps: 30,
    },
    Tier {
        max_kbps: 800,
        fps: 30,
    },
];
/// The tier a new stream starts at.
pub const START_TIER: usize = 2;

/// What a viewer connection measured about its path over the last period (about a second).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct NetworkReport {
    /// Video the viewer acknowledged, in bits per second.
    pub delivered_bps: u64,
    /// Frames queued up on the way to the viewer (its round trip grew well beyond the base, or
    /// frames had to be dropped because it fell behind).
    pub congested: bool,
    /// The path, not the stream, limited what was delivered (video waited for it), so
    /// `delivered_bps` measures what the path carries.
    pub limited: bool,
}

/// Shortest time between two tier changes when stepping down (a report may still describe the
/// stream before the last change).
const DOWNSHIFT_SPACING: Duration = Duration::from_millis(1500);
/// Quiet time before the first probe one tier up; doubled after a failed probe.
const UPSHIFT_WAIT_MIN: Duration = Duration::from_secs(6);
/// Longest quiet time before a probe.
const UPSHIFT_WAIT_MAX: Duration = Duration::from_secs(60);
/// Congestion this soon after a step up means the probe failed.
const PROBE_WINDOW: Duration = Duration::from_secs(6);
/// A step down aims at this share of what the path delivered while it limited the stream.
const FIT_SHARE: f64 = 0.85;
/// A step up only when the stream uses at least this share of its cap (a static page proves
/// nothing about the link).
const UPSHIFT_UTILIZATION: f64 = 0.5;

/// The bitrate tier policy (pure; driven by viewers' reports).
#[derive(Debug, Clone)]
pub struct RateControl {
    tier: usize,
    since: Instant,
    last_change: Option<Instant>,
    last_congestion: Option<Instant>,
    last_upshift: Option<Instant>,
    upshift_wait: Duration,
}

impl RateControl {
    /// A policy starting at `tier` at `now`.
    #[must_use]
    pub fn new(tier: usize, now: Instant) -> Self {
        Self {
            tier: tier.min(TIERS.len() - 1),
            since: now,
            last_change: None,
            last_congestion: None,
            last_upshift: None,
            upshift_wait: UPSHIFT_WAIT_MIN,
        }
    }

    /// The current tier (an index into [`TIERS`]).
    #[must_use]
    pub fn tier(&self) -> usize {
        self.tier
    }

    /// Applies a viewer's report; returns the new tier when it changes.
    pub fn on_report(&mut self, now: Instant, report: NetworkReport) -> Option<usize> {
        let lowest = TIERS.len() - 1;
        if report.congested {
            self.last_congestion = Some(now);
            if let Some(up) = self.last_upshift.take()
                && now.saturating_duration_since(up) < PROBE_WINDOW
            {
                self.upshift_wait = (self.upshift_wait * 2).min(UPSHIFT_WAIT_MAX);
            }
            if self
                .last_change
                .is_some_and(|at| now.saturating_duration_since(at) < DOWNSHIFT_SPACING)
            {
                return None;
            }
            // One step; more when the path set the delivered rate (then it says what fits). A
            // stream that sent little (a still page) says nothing about the path.
            let mut target = (self.tier + 1).min(lowest);
            let fit_kbps = report.delivered_bps as f64 * FIT_SHARE / 1000.0;
            if report.limited && report.delivered_bps > 0 {
                while target < lowest && f64::from(TIERS[target].max_kbps) > fit_kbps {
                    target += 1;
                }
            }
            return self.change(target, now);
        }
        if self.tier == 0 {
            return None;
        }
        if let Some(up) = self.last_upshift
            && now.saturating_duration_since(up) >= PROBE_WINDOW
        {
            // The last probe held.
            self.last_upshift = None;
            self.upshift_wait = UPSHIFT_WAIT_MIN;
        }
        let quiet_since = [self.last_congestion, self.last_change]
            .into_iter()
            .flatten()
            .max()
            .unwrap_or(self.since);
        if now.saturating_duration_since(quiet_since) < self.upshift_wait {
            return None;
        }
        let used = report.delivered_bps as f64 / 1000.0;
        if used < f64::from(TIERS[self.tier].max_kbps) * UPSHIFT_UTILIZATION {
            return None;
        }
        let changed = self.change(self.tier - 1, now);
        self.last_upshift = Some(now);
        changed
    }

    fn change(&mut self, tier: usize, now: Instant) -> Option<usize> {
        if tier == self.tier {
            return None;
        }
        self.tier = tier;
        self.last_change = Some(now);
        Some(tier)
    }
}

/// Why video is not available.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("video is not available: {0}")]
pub struct VideoUnavailable(pub String);

struct Subscriber {
    id: u64,
    tx: mpsc::Sender<VideoPacket>,
    /// Missing frames until the next keyframe.
    waiting_key: bool,
    /// The configuration must precede the next keyframe.
    need_config: bool,
}

#[derive(Default)]
struct HubState {
    config: Option<Arc<VideoConfig>>,
    gop: Vec<VideoFrame>,
    gop_bytes: usize,
    /// `gop` holds a whole group from its keyframe.
    gop_complete: bool,
    subscribers: Vec<Subscriber>,
    next_id: u64,
    failed: Option<String>,
    dropped: u64,
    /// A subscriber waits for a keyframe the encoder will not send by itself.
    wants_keyframe: bool,
}

impl HubState {
    fn on_config(&mut self, config: Arc<VideoConfig>) {
        self.config = Some(config);
        self.gop.clear();
        self.gop_bytes = 0;
        self.gop_complete = false;
        for subscriber in &mut self.subscribers {
            subscriber.waiting_key = true;
            subscriber.need_config = true;
        }
    }

    fn on_frame(&mut self, frame: &VideoFrame) {
        if frame.keyframe {
            self.wants_keyframe = false;
            self.gop.clear();
            self.gop_bytes = 0;
            self.gop_complete = true;
        }
        if self.gop_complete {
            if self.gop.len() < GOP_MAX_FRAMES && self.gop_bytes + frame.data.len() <= GOP_MAX_BYTES
            {
                self.gop_bytes += frame.data.len();
                self.gop.push(frame.clone());
            } else {
                self.gop.clear();
                self.gop_bytes = 0;
                self.gop_complete = false;
            }
        }
        let config = self.config.clone();
        let mut dropped = 0;
        self.subscribers.retain_mut(|subscriber| {
            if subscriber.waiting_key {
                if !frame.keyframe {
                    dropped += 1;
                    return !subscriber.tx.is_closed();
                }
                if subscriber.need_config {
                    let Some(config) = &config else {
                        return true;
                    };
                    // Both must fit, or the subscriber keeps waiting for the next keyframe.
                    if subscriber.tx.capacity() < 2 {
                        dropped += 1;
                        return !subscriber.tx.is_closed();
                    }
                    if subscriber
                        .tx
                        .try_send(VideoPacket::Config(Arc::clone(config)))
                        .is_err()
                    {
                        return !subscriber.tx.is_closed();
                    }
                    subscriber.need_config = false;
                }
                subscriber.waiting_key = false;
            }
            match subscriber.tx.try_send(VideoPacket::Frame(frame.clone())) {
                Ok(()) => true,
                Err(mpsc::error::TrySendError::Full(_)) => {
                    subscriber.waiting_key = true;
                    dropped += 1;
                    true
                }
                Err(mpsc::error::TrySendError::Closed(_)) => false,
            }
        });
        self.dropped += dropped;
    }

    fn subscribe(&mut self) -> (u64, Vec<VideoPacket>, mpsc::Receiver<VideoPacket>) {
        let (tx, rx) = mpsc::channel(SUBSCRIBER_QUEUE);
        let id = self.next_id;
        self.next_id += 1;
        let mut backlog = Vec::new();
        // A large group would take a slow link long to catch up on: a fresh keyframe is quicker.
        let starts_now =
            self.gop_complete && !self.gop.is_empty() && self.gop_bytes <= JOIN_REPLAY_BYTES;
        if let (Some(config), true) = (&self.config, starts_now) {
            backlog.push(VideoPacket::Config(Arc::clone(config)));
            backlog.extend(self.gop.iter().cloned().map(VideoPacket::Frame));
        }
        let ready = !backlog.is_empty();
        if !ready {
            self.wants_keyframe = true;
        }
        self.subscribers.push(Subscriber {
            id,
            tx,
            waiting_key: !ready,
            need_config: !ready,
        });
        (id, backlog, rx)
    }
}

/// What the encoder controller is asked to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Demand {
    subscribers: usize,
    size: (u32, u32),
    /// Index into [`TIERS`].
    tier: usize,
    /// Keyframe requests so far (a change wakes the controller).
    keyframes: u64,
    shutdown: bool,
}

struct HubShared {
    state: Mutex<HubState>,
    rate: Mutex<RateControl>,
    demand: watch::Sender<Demand>,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

impl HubShared {
    fn request_keyframe(&self) {
        lock(&self.state).wants_keyframe = true;
        self.demand.send_modify(|demand| demand.keyframes += 1);
    }

    fn report(&self, report: NetworkReport) {
        let changed = lock(&self.rate).on_report(Instant::now(), report);
        if let Some(tier) = changed {
            info!(
                max_kbps = TIERS[tier].max_kbps,
                fps = TIERS[tier].fps,
                delivered_kbps = report.delivered_bps / 1000,
                congested = report.congested,
                "display video tier changed"
            );
            self.demand.send_modify(|demand| demand.tier = tier);
        }
    }

    fn unsubscribe(&self, id: u64) {
        let mut state = lock(&self.state);
        let before = state.subscribers.len();
        state.subscribers.retain(|subscriber| subscriber.id != id);
        if state.subscribers.len() != before {
            drop(state);
            self.demand
                .send_modify(|demand| demand.subscribers = demand.subscribers.saturating_sub(1));
        }
    }

    fn fail(&self, reason: String) {
        let mut state = lock(&self.state);
        warn!(%reason, "display video unavailable");
        state.failed = Some(reason);
        // Ending the subscriptions tells viewers to fall back.
        let ended = state.subscribers.len();
        state.subscribers.clear();
        state.config = None;
        state.gop.clear();
        state.gop_complete = false;
        drop(state);
        self.demand
            .send_modify(|demand| demand.subscribers = demand.subscribers.saturating_sub(ended));
    }
}

/// A subscriber's stream. Dropping it unsubscribes.
#[derive(Debug)]
pub struct VideoSubscription {
    id: u64,
    backlog: Vec<VideoPacket>,
    rx: mpsc::Receiver<VideoPacket>,
    hub: Weak<HubShared>,
}

impl std::fmt::Debug for HubShared {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HubShared").finish_non_exhaustive()
    }
}

impl VideoSubscription {
    /// The configuration and the cached group to send before live frames (empty when the
    /// subscriber starts at the next keyframe). Taking it leaves it empty.
    pub fn take_backlog(&mut self) -> Vec<VideoPacket> {
        std::mem::take(&mut self.backlog)
    }

    /// The next live packet; `None` once the stream ended (display stopped or video failed).
    pub async fn recv(&mut self) -> Option<VideoPacket> {
        self.rx.recv().await
    }

    /// A queued packet without waiting.
    pub fn try_recv(&mut self) -> Option<VideoPacket> {
        self.rx.try_recv().ok()
    }

    /// Asks for a keyframe (this viewer fell behind or its decoder failed): the encoder
    /// restarts, at most once per [`KEYFRAME_SPACING`]; this subscriber then resumes at it.
    pub fn request_keyframe(&self) {
        if let Some(hub) = self.hub.upgrade() {
            hub.request_keyframe();
        }
    }

    /// Reports this viewer's network ([`RateControl`] adapts the stream to it).
    pub fn report(&self, report: NetworkReport) {
        if let Some(hub) = self.hub.upgrade() {
            hub.report(report);
        }
    }
}

impl Drop for VideoSubscription {
    fn drop(&mut self) {
        if let Some(hub) = self.hub.upgrade() {
            hub.unsubscribe(self.id);
        }
    }
}

/// Starts encoders for the hub.
pub(crate) trait EncoderStart: Send + Sync + 'static {
    /// Starts an encoder for `size` (device pixels) at `tier`.
    fn start(
        &self,
        size: (u32, u32),
        tier: Tier,
    ) -> impl std::future::Future<Output = Result<RunningEncoder, String>> + Send;
}

/// A running encoder: its events and how to stop it.
pub(crate) struct RunningEncoder {
    pub(crate) events: mpsc::Receiver<EncoderEvent>,
    pub(crate) stop: Box<dyn EncoderStop>,
}

/// Stops an encoder and reaps its process.
pub(crate) trait EncoderStop: Send {
    /// Terminates the encoder; resolves once it is reaped (or reports why not).
    fn stop(
        self: Box<Self>,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Option<String>> + Send>>;
}

/// The video of one display.
#[derive(Debug)]
pub struct VideoHub {
    shared: Arc<HubShared>,
    task: Mutex<Option<JoinHandle<()>>>,
}

impl VideoHub {
    /// A hub whose encoder starts on the first subscriber, at `size` device pixels.
    pub(crate) fn new<S: EncoderStart>(starter: S, size: (u32, u32)) -> Self {
        let (demand, demand_rx) = watch::channel(Demand {
            subscribers: 0,
            size,
            tier: START_TIER,
            keyframes: 0,
            shutdown: false,
        });
        let shared = Arc::new(HubShared {
            state: Mutex::new(HubState::default()),
            rate: Mutex::new(RateControl::new(START_TIER, Instant::now())),
            demand,
        });
        let task = tokio::spawn(run(Arc::downgrade(&shared), starter, demand_rx));
        Self {
            shared,
            task: Mutex::new(Some(task)),
        }
    }

    /// Subscribes to the video, starting the encoder if needed.
    ///
    /// # Errors
    /// [`VideoUnavailable`] when the encoder failed for good.
    pub fn subscribe(&self) -> Result<VideoSubscription, VideoUnavailable> {
        let (id, backlog, rx) = {
            let mut state = lock(&self.shared.state);
            if let Some(reason) = &state.failed {
                return Err(VideoUnavailable(reason.clone()));
            }
            state.subscribe()
        };
        let wants_keyframe = backlog.is_empty();
        self.shared.demand.send_modify(|demand| {
            demand.subscribers += 1;
            if wants_keyframe {
                demand.keyframes += 1;
            }
        });
        Ok(VideoSubscription {
            id,
            backlog,
            rx,
            hub: Arc::downgrade(&self.shared),
        })
    }

    /// Live subscribers.
    #[must_use]
    pub fn subscribers(&self) -> usize {
        lock(&self.shared.state).subscribers.len()
    }

    /// Frames subscribers missed because their queue was full or they waited for a keyframe.
    #[must_use]
    pub fn frames_dropped(&self) -> u64 {
        lock(&self.shared.state).dropped
    }

    /// Why video failed for good, if it did.
    #[must_use]
    pub fn failure(&self) -> Option<String> {
        lock(&self.shared.state).failed.clone()
    }

    /// Restarts the encoder at `size` device pixels (when it runs).
    pub(crate) fn set_size(&self, size: (u32, u32)) {
        self.shared.demand.send_if_modified(|demand| {
            let changed = demand.size != size;
            demand.size = size;
            changed
        });
    }

    /// The encoder's current size in device pixels.
    #[must_use]
    pub fn size(&self) -> (u32, u32) {
        self.shared.demand.borrow().size
    }

    /// The current quality tier (an index into [`TIERS`]).
    #[must_use]
    pub fn tier(&self) -> usize {
        self.shared.demand.borrow().tier
    }

    /// Stops the encoder (reaping it) and ends every subscription.
    pub(crate) async fn shutdown(&self, timeout: Duration) {
        self.shared
            .demand
            .send_modify(|demand| demand.shutdown = true);
        let task = lock(&self.task).take();
        if let Some(mut task) = task
            && tokio::time::timeout(timeout, &mut task).await.is_err()
        {
            warn!("display video did not stop in time; aborting it");
            // Aborting drops the encoder handle, whose process group is killed on drop.
            task.abort();
        }
        let mut state = lock(&self.shared.state);
        state.subscribers.clear();
    }
}

impl Drop for VideoHub {
    fn drop(&mut self) {
        if let Some(task) = lock(&self.task).take() {
            task.abort();
        }
    }
}

/// Why an encoder run ended.
enum RunEnd {
    Shutdown,
    Idle,
    /// A new size or tier, or a keyframe was asked for.
    Restart,
    Exited,
}

/// The encoder controller: starts the encoder on demand, forwards its output, restarts it on a
/// resize or after a crash, and stops it when idle or on shutdown.
async fn run<S: EncoderStart>(
    hub: Weak<HubShared>,
    starter: S,
    mut demand: watch::Receiver<Demand>,
) {
    let mut failures = 0_u32;
    loop {
        // Wait for a subscriber.
        let wanted = demand
            .wait_for(|d| d.shutdown || (d.subscribers > 0))
            .await
            .map(|d| *d);
        let Ok(wanted) = wanted else { return };
        if wanted.shutdown {
            return;
        }
        if hub
            .upgrade()
            .is_none_or(|hub| lock(&hub.state).failed.is_some())
        {
            // Failed for good: wait for shutdown only.
            let _ = demand.wait_for(|d| d.shutdown).await;
            return;
        }
        let size = wanted.size;
        let mut tier = wanted.tier;
        let mut keyframes = wanted.keyframes;
        let mut started = tokio::time::Instant::now();
        let mut encoder = match starter.start(size, TIERS[tier]).await {
            Ok(encoder) => encoder,
            Err(reason) => {
                failures += 1;
                warn!(%reason, failures, "display encoder did not start");
                if give_up(&hub, failures, &reason) {
                    continue;
                }
                backoff(&mut demand, failures).await;
                continue;
            }
        };
        info!(
            width = size.0,
            height = size.1,
            max_kbps = TIERS[tier].max_kbps,
            fps = TIERS[tier].fps,
            "display encoder started"
        );
        let mut idle_since: Option<tokio::time::Instant> = None;
        // A keyframe request within KEYFRAME_SPACING of the start waits until then (a starting
        // encoder sends one anyway).
        let mut keyframe_at: Option<tokio::time::Instant> = None;
        // A new tier or a keyframe starts a replacement while the current encoder keeps
        // streaming (make before break): viewers switch at its first keyframe, without a gap.
        let mut successor: Option<Successor> = None;
        let end = loop {
            let idle_deadline = idle_since.map(|since| since + IDLE_GRACE);
            let successor_deadline = successor.as_ref().map(|next| next.deadline);
            tokio::select! {
                event = encoder.events.recv() => {
                    let Some(event) = event else { break RunEnd::Exited };
                    let Some(hub) = hub.upgrade() else { break RunEnd::Shutdown };
                    let mut state = lock(&hub.state);
                    match event {
                        EncoderEvent::Config(config) => state.on_config(config),
                        EncoderEvent::Frame(frame) => state.on_frame(&frame),
                    }
                }
                event = successor_event(&mut successor) => {
                    let Some(next) = successor.as_mut() else { continue };
                    match event {
                        None => {
                            warn!("a replacement display encoder ended before its first keyframe");
                            if let Some(next) = successor.take() {
                                retire(next.encoder);
                            }
                        }
                        Some(EncoderEvent::Config(config)) => next.config = Some(config),
                        Some(EncoderEvent::Frame(frame)) if frame.keyframe && next.config.is_some() => {
                            let Some(next) = successor.take() else { continue };
                            let Some(hub) = hub.upgrade() else { break RunEnd::Shutdown };
                            retire(std::mem::replace(&mut encoder, next.encoder));
                            tier = next.tier;
                            started = next.started;
                            let mut state = lock(&hub.state);
                            if let Some(config) = next.config {
                                state.on_config(config);
                            }
                            state.on_frame(&frame);
                            drop(state);
                            info!(
                                max_kbps = TIERS[tier].max_kbps,
                                fps = TIERS[tier].fps,
                                "display encoder switched to its replacement"
                            );
                        }
                        // Nothing before its first keyframe is of use.
                        Some(EncoderEvent::Frame(_)) => {}
                    }
                }
                () = sleep_until(successor_deadline) => {
                    warn!("a replacement display encoder sent no keyframe in time");
                    if let Some(next) = successor.take() {
                        retire(next.encoder);
                    }
                }
                changed = demand.changed() => {
                    if changed.is_err() {
                        break RunEnd::Shutdown;
                    }
                    let now = *demand.borrow_and_update();
                    if now.shutdown {
                        break RunEnd::Shutdown;
                    }
                    // A new size changed the screen under the current encoder: restart.
                    if now.size != size {
                        break RunEnd::Restart;
                    }
                    let target = successor.as_ref().map_or(tier, |next| next.tier);
                    if now.tier != target {
                        match start_successor(&starter, size, now.tier, &mut successor).await {
                            Ok(()) => {}
                            Err(()) => break RunEnd::Restart,
                        }
                    }
                    idle_since = if now.subscribers == 0 {
                        idle_since.or_else(|| Some(tokio::time::Instant::now()))
                    } else {
                        None
                    };
                    if now.keyframes != keyframes {
                        keyframes = now.keyframes;
                        keyframe_at = keyframe_at.or(Some(started + KEYFRAME_SPACING));
                    }
                }
                () = sleep_until(idle_deadline) => break RunEnd::Idle,
                () = sleep_until(keyframe_at) => {
                    keyframe_at = None;
                    let wanted = hub.upgrade().is_some_and(|hub| lock(&hub.state).wants_keyframe);
                    if wanted && successor.is_none() {
                        debug!("a replacement display encoder starts for a keyframe");
                        if start_successor(&starter, size, tier, &mut successor).await.is_err() {
                            break RunEnd::Restart;
                        }
                    }
                }
            }
        };
        if let Some(next) = successor.take() {
            retire(next.encoder);
        }
        let ran = started.elapsed();
        if let Some(problem) = encoder.stop.stop().await {
            warn!(%problem, "display encoder could not be reaped");
        }
        match end {
            RunEnd::Shutdown => return,
            RunEnd::Idle => {
                debug!("display encoder stopped: nobody watches");
                failures = 0;
            }
            RunEnd::Restart => failures = 0,
            RunEnd::Exited => {
                if ran >= HEALTHY_RUN {
                    failures = 0;
                } else {
                    failures += 1;
                }
                warn!(failures, "display encoder exited");
                if give_up(&hub, failures, "the encoder keeps exiting") {
                    continue;
                }
                backoff(&mut demand, failures).await;
            }
        }
    }
}

/// An encoder started to replace the running one, until its first keyframe.
struct Successor {
    encoder: RunningEncoder,
    tier: usize,
    config: Option<Arc<VideoConfig>>,
    started: tokio::time::Instant,
    deadline: tokio::time::Instant,
}

/// The replacement's next event; pending while there is none.
async fn successor_event(successor: &mut Option<Successor>) -> Option<EncoderEvent> {
    match successor {
        Some(next) => next.encoder.events.recv().await,
        None => std::future::pending().await,
    }
}

/// Starts a replacement at `tier` (one in flight at most: a newer one replaces it).
async fn start_successor<S: EncoderStart>(
    starter: &S,
    size: (u32, u32),
    tier: usize,
    successor: &mut Option<Successor>,
) -> Result<(), ()> {
    if let Some(old) = successor.take() {
        retire(old.encoder);
    }
    match starter.start(size, TIERS[tier]).await {
        Ok(encoder) => {
            let started = tokio::time::Instant::now();
            info!(
                max_kbps = TIERS[tier].max_kbps,
                fps = TIERS[tier].fps,
                "display encoder started (replacement)"
            );
            *successor = Some(Successor {
                encoder,
                tier,
                config: None,
                started,
                deadline: started + SUCCESSOR_TIMEOUT,
            });
            Ok(())
        }
        Err(reason) => {
            warn!(%reason, "a replacement display encoder did not start");
            Err(())
        }
    }
}

/// Stops an encoder in the background (terminated and reaped by its own task).
fn retire(encoder: RunningEncoder) {
    let RunningEncoder { events, stop } = encoder;
    drop(events);
    tokio::spawn(async move {
        if let Some(problem) = stop.stop().await {
            warn!(%problem, "display encoder could not be reaped");
        }
    });
}

/// Marks the hub failed after too many failures; returns whether it did.
fn give_up(hub: &Weak<HubShared>, failures: u32, reason: &str) -> bool {
    if failures < MAX_START_FAILURES {
        return false;
    }
    if let Some(hub) = hub.upgrade() {
        hub.fail(reason.to_owned());
    }
    true
}

/// Waits before a restart (bounded, cancelled by shutdown).
async fn backoff(demand: &mut watch::Receiver<Demand>, failures: u32) {
    let delay = Duration::from_millis(500) * failures.min(6);
    let _ = tokio::time::timeout(delay, demand.wait_for(|d| d.shutdown)).await;
}

async fn sleep_until(deadline: Option<tokio::time::Instant>) {
    match deadline {
        Some(deadline) => tokio::time::sleep_until(deadline).await,
        None => std::future::pending().await,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(keyframe: bool, n: u64, bytes: usize) -> VideoFrame {
        VideoFrame {
            keyframe,
            timestamp_us: n,
            data: vec![0_u8; bytes].into(),
        }
    }

    fn config(width: u32) -> Arc<VideoConfig> {
        Arc::new(VideoConfig {
            codec: "avc1.42c028".into(),
            description: vec![1, 0x42, 0xC0, 0x28].into(),
            width,
            height: 800,
        })
    }

    fn drain(rx: &mut mpsc::Receiver<VideoPacket>) -> Vec<VideoPacket> {
        let mut packets = Vec::new();
        while let Ok(packet) = rx.try_recv() {
            packets.push(packet);
        }
        packets
    }

    fn stamps(packets: &[VideoPacket]) -> Vec<String> {
        packets
            .iter()
            .map(|packet| match packet {
                VideoPacket::Config(config) => format!("C{}", config.width),
                VideoPacket::Frame(frame) if frame.keyframe => format!("K{}", frame.timestamp_us),
                VideoPacket::Frame(frame) => format!("D{}", frame.timestamp_us),
            })
            .collect()
    }

    #[test]
    fn a_new_subscriber_starts_with_the_config_and_the_cached_group() {
        let mut state = HubState::default();
        state.on_config(config(1280));
        state.on_frame(&frame(false, 0, 10)); // before any keyframe: not cached
        state.on_frame(&frame(true, 1, 10));
        state.on_frame(&frame(false, 2, 10));
        state.on_frame(&frame(true, 3, 10)); // a new group replaces the old one
        state.on_frame(&frame(false, 4, 10));
        let (_, backlog, mut rx) = state.subscribe();
        assert_eq!(stamps(&backlog), ["C1280", "K3", "D4"]);
        state.on_frame(&frame(false, 5, 10));
        assert_eq!(stamps(&drain(&mut rx)), ["D5"]);
    }

    #[test]
    fn without_a_cached_group_a_subscriber_starts_at_the_next_keyframe_with_the_config() {
        let mut state = HubState::default();
        let (_, backlog, mut rx) = state.subscribe();
        assert!(backlog.is_empty());
        state.on_config(config(640));
        state.on_frame(&frame(false, 1, 10));
        assert!(
            drain(&mut rx).is_empty(),
            "a delta frame cannot start a decoder"
        );
        state.on_frame(&frame(true, 2, 10));
        state.on_frame(&frame(false, 3, 10));
        assert_eq!(stamps(&drain(&mut rx)), ["C640", "K2", "D3"]);
    }

    #[test]
    fn the_cache_is_bounded_in_frames_and_bytes() {
        let mut state = HubState::default();
        state.on_config(config(1));
        state.on_frame(&frame(true, 0, 10));
        for n in 1..GOP_MAX_FRAMES as u64 {
            state.on_frame(&frame(false, n, 10));
        }
        assert_eq!(state.gop.len(), GOP_MAX_FRAMES);
        state.on_frame(&frame(false, 999, 10));
        assert!(!state.gop_complete && state.gop.is_empty(), "outgrown");
        let (_, backlog, _rx) = state.subscribe();
        assert!(backlog.is_empty(), "waits for the next keyframe");
        state.on_frame(&frame(true, 1000, GOP_MAX_BYTES - 5));
        assert!(state.gop_complete);
        state.on_frame(&frame(false, 1001, 10));
        assert!(!state.gop_complete, "byte bound");
    }

    #[test]
    fn a_slow_subscriber_misses_frames_until_the_next_keyframe_and_others_are_unaffected() {
        let mut state = HubState::default();
        state.on_config(config(1));
        state.on_frame(&frame(true, 0, 1));
        let (_, _, mut slow) = state.subscribe();
        let (_, _, mut fast) = state.subscribe();
        let mut fast_seen = Vec::new();
        // The slow subscriber never reads: its queue fills.
        for n in 1..=(SUBSCRIBER_QUEUE as u64 + 10) {
            state.on_frame(&frame(false, n, 1));
            fast_seen.extend(drain(&mut fast));
        }
        assert_eq!(
            fast_seen.len(),
            SUBSCRIBER_QUEUE + 10,
            "fast one got everything"
        );
        let queued = drain(&mut slow);
        assert_eq!(queued.len(), SUBSCRIBER_QUEUE);
        assert!(state.dropped >= 10);
        // Deltas are still skipped after it drained; the next keyframe resumes it.
        state.on_frame(&frame(false, 5000, 1));
        assert!(drain(&mut slow).is_empty());
        state.on_frame(&frame(true, 5001, 1));
        state.on_frame(&frame(false, 5002, 1));
        assert_eq!(stamps(&drain(&mut slow)), ["K5001", "D5002"]);
    }

    #[test]
    fn a_new_config_is_sent_before_the_next_keyframe_and_closed_subscribers_are_removed() {
        let mut state = HubState::default();
        state.on_config(config(1));
        state.on_frame(&frame(true, 0, 1));
        let (_, _, mut rx) = state.subscribe();
        let (_, _, closed) = state.subscribe();
        drop(closed);
        state.on_config(config(2));
        state.on_frame(&frame(false, 1, 1)); // stale delta of no group: skipped
        state.on_frame(&frame(true, 2, 1));
        assert_eq!(stamps(&drain(&mut rx)), ["C2", "K2"]);
        assert_eq!(state.subscribers.len(), 1);
    }

    struct FakeStart {
        sizes: Arc<Mutex<Vec<(u32, u32)>>>,
        tiers: Arc<Mutex<Vec<Tier>>>,
        stops: Arc<Mutex<u32>>,
        fail: bool,
        /// A keyframe every this many frames (`u64::MAX`: only the first).
        key_every: u64,
        /// Time an encoder takes to its configuration (ffmpeg's start).
        delay: Duration,
    }

    impl FakeStart {
        fn new(fail: bool) -> Self {
            Self {
                sizes: Arc::new(Mutex::new(Vec::new())),
                tiers: Arc::new(Mutex::new(Vec::new())),
                stops: Arc::new(Mutex::new(0)),
                fail,
                key_every: 4,
                delay: Duration::ZERO,
            }
        }
    }

    struct FakeStop(Arc<Mutex<u32>>, JoinHandle<()>);

    impl EncoderStop for FakeStop {
        fn stop(
            self: Box<Self>,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Option<String>> + Send>> {
            self.1.abort();
            *lock(&self.0) += 1;
            Box::pin(async { None })
        }
    }

    impl EncoderStart for FakeStart {
        async fn start(&self, size: (u32, u32), tier: Tier) -> Result<RunningEncoder, String> {
            lock(&self.sizes).push(size);
            lock(&self.tiers).push(tier);
            if self.fail {
                return Err("no encoder".into());
            }
            let (tx, events) = mpsc::channel(ENCODER_EVENTS);
            let key_every = self.key_every;
            let delay = self.delay;
            let task = tokio::spawn(async move {
                tokio::time::sleep(delay).await;
                let _ = tx.send(EncoderEvent::Config(config(size.0))).await;
                for n in 0_u64.. {
                    if tx
                        .send(EncoderEvent::Frame(frame(n % key_every == 0, n, 8)))
                        .await
                        .is_err()
                    {
                        return;
                    }
                    tokio::time::sleep(Duration::from_millis(5)).await;
                }
            });
            Ok(RunningEncoder {
                events,
                stop: Box::new(FakeStop(Arc::clone(&self.stops), task)),
            })
        }
    }

    async fn next_frame_of(sub: &mut VideoSubscription) -> Option<VideoPacket> {
        if let Some(first) = sub.take_backlog().into_iter().next() {
            return Some(first);
        }
        tokio::time::timeout(Duration::from_secs(5), sub.recv())
            .await
            .ok()
            .flatten()
    }

    #[tokio::test]
    async fn the_encoder_runs_on_demand_restarts_on_resize_and_stops_on_shutdown() {
        let start = FakeStart::new(false);
        let (sizes, stops) = (Arc::clone(&start.sizes), Arc::clone(&start.stops));
        let hub = VideoHub::new(start, (1280, 800));
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(lock(&sizes).is_empty(), "nobody watches: no encoder");
        let Ok(mut sub) = hub.subscribe() else {
            panic!("subscribe failed");
        };
        assert_eq!(
            next_frame_of(&mut sub).await,
            Some(VideoPacket::Config(config(1280)))
        );
        hub.set_size((640, 480));
        // Frames of the new size arrive after its config.
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        loop {
            let packet = tokio::time::timeout_at(deadline, sub.recv()).await;
            if let Ok(Some(VideoPacket::Config(config))) = packet {
                assert_eq!(config.width, 640);
                break;
            }
            assert!(packet.is_ok(), "no new config");
        }
        assert_eq!(*lock(&sizes), [(1280, 800), (640, 480)]);
        assert_eq!(*lock(&stops), 1, "the old encoder was stopped");
        hub.shutdown(Duration::from_secs(5)).await;
        assert_eq!(*lock(&stops), 2);
    }

    #[tokio::test(start_paused = true)]
    async fn the_encoder_stops_after_the_idle_grace() {
        let start = FakeStart::new(false);
        let stops = Arc::clone(&start.stops);
        let hub = VideoHub::new(start, (100, 100));
        let Ok(sub) = hub.subscribe() else {
            panic!("subscribe failed");
        };
        tokio::time::sleep(Duration::from_millis(100)).await;
        drop(sub);
        tokio::time::sleep(IDLE_GRACE - Duration::from_secs(1)).await;
        assert_eq!(*lock(&stops), 0, "still within the grace");
        tokio::time::sleep(Duration::from_secs(2)).await;
        assert_eq!(*lock(&stops), 1, "stopped when nobody watched");
        hub.shutdown(Duration::from_secs(1)).await;
    }

    #[tokio::test(start_paused = true)]
    async fn an_encoder_that_cannot_start_fails_the_hub_and_ends_subscriptions() {
        let start = FakeStart::new(true);
        let sizes = Arc::clone(&start.sizes);
        let hub = VideoHub::new(start, (100, 100));
        let Ok(mut sub) = hub.subscribe() else {
            panic!("subscribe failed");
        };
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(30), sub.recv()).await,
            Ok(None),
            "the subscription ends"
        );
        assert_eq!(lock(&sizes).len(), MAX_START_FAILURES as usize);
        assert_eq!(hub.failure().as_deref(), Some("no encoder"));
        assert!(hub.subscribe().is_err());
        hub.shutdown(Duration::from_secs(1)).await;
    }

    #[test]
    fn a_large_cached_group_is_not_replayed_and_asks_for_a_keyframe() {
        let mut state = HubState::default();
        state.on_config(config(1));
        state.on_frame(&frame(true, 0, JOIN_REPLAY_BYTES / 2));
        state.on_frame(&frame(false, 1, 100));
        let (_, backlog, _rx) = state.subscribe();
        assert_eq!(backlog.len(), 3, "a small group is replayed");
        assert!(!state.wants_keyframe);
        state.on_frame(&frame(false, 2, JOIN_REPLAY_BYTES));
        let (_, backlog, mut rx) = state.subscribe();
        assert!(backlog.is_empty(), "a large group is not replayed");
        assert!(state.wants_keyframe);
        state.on_frame(&frame(true, 3, 10));
        assert!(!state.wants_keyframe, "a keyframe satisfies the request");
        assert_eq!(stamps(&drain(&mut rx)), ["C1", "K3"]);
    }

    fn at(base: Instant, millis: u64) -> Instant {
        base + Duration::from_millis(millis)
    }

    fn report(delivered_kbps: u64, congested: bool) -> NetworkReport {
        NetworkReport {
            delivered_bps: delivered_kbps * 1000,
            congested,
            limited: congested,
        }
    }

    #[test]
    fn congestion_steps_down_to_fit_what_arrived() {
        let t0 = Instant::now();
        let mut rate = RateControl::new(START_TIER, t0);
        assert_eq!(TIERS[START_TIER].max_kbps, 5_000);
        // 2.5 Mbit/s arrived while the path queued up: the tier fitting 85 % of it (2.1 Mbit/s).
        assert_eq!(rate.on_report(at(t0, 1000), report(2_500, true)), Some(5));
        assert_eq!(TIERS[5].max_kbps, 1_500);
        // A report still describing the old stream does not step down again at once.
        assert_eq!(rate.on_report(at(t0, 1500), report(1_000, true)), None);
        // Later congestion without a measured rate: one step.
        assert_eq!(rate.on_report(at(t0, 3000), report(0, true)), Some(6));
        assert_eq!(
            rate.on_report(at(t0, 5000), report(0, true)),
            None,
            "lowest"
        );
        // A congested viewer that was not path-limited (the stream was small) steps down once.
        let mut rate = RateControl::new(START_TIER, t0);
        let small = NetworkReport {
            delivered_bps: 270_000,
            congested: true,
            limited: false,
        };
        assert_eq!(rate.on_report(at(t0, 1000), small), Some(START_TIER + 1));
    }

    #[test]
    fn a_quiet_link_used_near_the_cap_is_probed_one_tier_up() {
        let t0 = Instant::now();
        let mut rate = RateControl::new(3, t0);
        let cap = u64::from(TIERS[3].max_kbps);
        // A static page uses little: nothing to gain from more.
        assert_eq!(
            rate.on_report(at(t0, 10_000), report(cap / 10, false)),
            None
        );
        // Scrolling near the cap, quiet for the wait: one tier up.
        assert_eq!(
            rate.on_report(at(t0, 10_500), report(cap * 9 / 10, false)),
            Some(2)
        );
        // Too soon for another probe.
        let cap = u64::from(TIERS[2].max_kbps);
        assert_eq!(rate.on_report(at(t0, 12_000), report(cap, false)), None);
        // The probe failed (congestion within the probe window): back down, and the next probe
        // waits twice as long.
        assert_eq!(rate.on_report(at(t0, 13_000), report(5_000, true)), Some(3));
        let cap = u64::from(TIERS[3].max_kbps);
        assert_eq!(
            rate.on_report(at(t0, 13_000 + 7_000), report(cap, false)),
            None
        );
        assert_eq!(
            rate.on_report(at(t0, 13_000 + 12_500), report(cap, false)),
            Some(2)
        );
        // This probe holds; the wait is short again afterwards.
        let cap = u64::from(TIERS[2].max_kbps);
        assert_eq!(
            rate.on_report(at(t0, 13_000 + 12_500 + 6_100), report(cap, false)),
            Some(1)
        );
    }

    async fn next_packet(sub: &mut VideoSubscription) -> Option<VideoPacket> {
        tokio::time::timeout(Duration::from_secs(10), sub.recv())
            .await
            .ok()
            .flatten()
    }

    #[tokio::test(start_paused = true)]
    async fn a_keyframe_request_restarts_the_encoder_at_most_once_per_spacing() {
        let mut start = FakeStart::new(false);
        start.key_every = u64::MAX;
        let sizes = Arc::clone(&start.sizes);
        let hub = VideoHub::new(start, (100, 100));
        let Ok(mut sub) = hub.subscribe() else {
            panic!("subscribe failed");
        };
        assert!(matches!(
            next_packet(&mut sub).await,
            Some(VideoPacket::Config(_))
        ));
        assert!(matches!(next_packet(&mut sub).await, Some(VideoPacket::Frame(f)) if f.keyframe));
        tokio::time::sleep(KEYFRAME_SPACING * 2).await;
        assert_eq!(lock(&sizes).len(), 1, "no periodic restart");
        sub.request_keyframe();
        sub.request_keyframe();
        // The restart brings a configuration and a keyframe.
        loop {
            match next_packet(&mut sub).await {
                Some(VideoPacket::Config(_)) => break,
                Some(VideoPacket::Frame(_)) => {}
                None => panic!("the stream ended"),
            }
        }
        assert!(matches!(next_packet(&mut sub).await, Some(VideoPacket::Frame(f)) if f.keyframe));
        tokio::time::sleep(KEYFRAME_SPACING * 3).await;
        assert_eq!(lock(&sizes).len(), 2, "two requests, one restart");
        hub.shutdown(Duration::from_secs(1)).await;
    }

    #[tokio::test(start_paused = true)]
    async fn a_congested_viewer_restarts_the_encoder_at_a_lower_tier() {
        let start = FakeStart::new(false);
        let tiers = Arc::clone(&start.tiers);
        let hub = VideoHub::new(start, (100, 100));
        let Ok(mut sub) = hub.subscribe() else {
            panic!("subscribe failed");
        };
        assert!(next_packet(&mut sub).await.is_some());
        sub.report(report(2_000, true));
        tokio::time::sleep(Duration::from_millis(200)).await;
        let started: Vec<u32> = lock(&tiers).iter().map(|t| t.max_kbps).collect();
        // 85 % of 2 Mbit/s: the 1.5 Mbit/s tier.
        assert_eq!(started, [5_000, 1_500]);
        assert_eq!(hub.tier(), 5);
        hub.shutdown(Duration::from_secs(1)).await;
    }

    #[tokio::test(start_paused = true)]
    async fn the_stream_keeps_flowing_while_a_replacement_encoder_starts() {
        let mut start = FakeStart::new(false);
        start.key_every = u64::MAX;
        start.delay = Duration::from_millis(300);
        let stops = Arc::clone(&start.stops);
        let hub = VideoHub::new(start, (100, 100));
        let Ok(mut sub) = hub.subscribe() else {
            panic!("subscribe failed");
        };
        assert!(matches!(
            next_packet(&mut sub).await,
            Some(VideoPacket::Config(_))
        ));
        tokio::time::sleep(KEYFRAME_SPACING * 2).await;
        while sub.try_recv().is_some() {}
        sub.request_keyframe();
        // The old encoder keeps streaming for the 300 ms the replacement needs to start.
        let mut old_frames = 0;
        let switched = loop {
            match next_packet(&mut sub).await {
                Some(VideoPacket::Frame(frame)) if !frame.keyframe => old_frames += 1,
                Some(VideoPacket::Config(_)) => break true,
                Some(VideoPacket::Frame(_)) => {}
                None => break false,
            }
        };
        assert!(switched);
        assert!(
            old_frames >= 40,
            "no gap: {old_frames} frames during the switch"
        );
        assert!(matches!(next_packet(&mut sub).await, Some(VideoPacket::Frame(f)) if f.keyframe));
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert_eq!(
            *lock(&stops),
            1,
            "the old encoder was stopped after the switch"
        );
        hub.shutdown(Duration::from_secs(1)).await;
    }
}
