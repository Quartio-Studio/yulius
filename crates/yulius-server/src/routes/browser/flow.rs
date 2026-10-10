//! Flow control of one viewer's video (SPEC-002 ADR-15, F 2026-10-10).
//!
//! Measured over the real network (Poland → the VM, through a tunnel): the server wrote frames as
//! fast as its socket took them, so whenever the stream outran the path the frames piled up in
//! the next hop's buffers (the SSH or Caddy socket's send queue reached 2 MB, about 4 s of video)
//! where the server could not see them, and scrolling showed seconds late.
//!
//! The viewer acknowledges every video frame as it arrives (`ack` with the count received so
//! far). [`Flow`] keeps the frames sent and not yet acknowledged and lets the next one go only
//! while the oldest of them was sent less than the base round trip plus [`SLACK`] ago: frames
//! never queue up on the path for longer than that, and the rest wait on the server, where the
//! viewer connection can drop them for a fresh keyframe. The base round trip is the smallest
//! acknowledgement delay of the last [`RTT_WINDOW`]. Every [`REPORT_PERIOD`] it reports what the
//! viewer received and whether the path queued up ([`FlowReport`]), which steers the stream's
//! bitrate tier. A viewer that never acknowledges (an older page) is not paced.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

/// Queueing on the path beyond the base round trip that closes the gate.
pub const SLACK: Duration = Duration::from_millis(120);
/// The base round trip is the smallest of this window.
pub const RTT_WINDOW: Duration = Duration::from_secs(10);
/// Frames in flight before the first round trip is known.
pub const INITIAL_IN_FLIGHT: usize = 30;
/// Frames in flight at most.
pub const MAX_IN_FLIGHT_FRAMES: usize = 240;
/// Bytes in flight at most.
pub const MAX_IN_FLIGHT_BYTES: usize = 4 << 20;
/// Frames remembered for a viewer that does not acknowledge (yet).
const MAX_TRACKED: usize = 4096;
/// How often the viewer's network is reported.
pub const REPORT_PERIOD: Duration = Duration::from_secs(1);
/// The path counts as queued up when the median acknowledgement delay of a period exceeds the
/// base by this much.
pub const CONGESTION_QUEUE: Duration = Duration::from_millis(50);

/// Round-trip samples before queueing is judged (the base must be known first).
const MIN_SAMPLES: u64 = 8;
/// Time after the first acknowledgement before queueing is judged: a stream's start (its first
/// keyframe, the cached group) queues briefly on any link.
const WARM_UP: Duration = Duration::from_secs(2);

/// What one period showed about the viewer's path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FlowReport {
    /// Video acknowledged during the period, in bits per second.
    pub delivered_bps: u64,
    /// The path queued up (median delay well above the base) or frames were dropped.
    pub congested: bool,
    /// The gate held video back during the period: the path, not the stream, set the delivered
    /// rate (so it measures what the path carries).
    pub limited: bool,
    /// Median acknowledgement delay of the period.
    pub rtt: Option<Duration>,
    /// The base round trip.
    pub base_rtt: Option<Duration>,
}

#[derive(Debug, Clone, Copy)]
struct Sent {
    seq: u64,
    bytes: usize,
    at: Instant,
}

/// The smallest value of a sliding window, kept in two halves.
#[derive(Debug, Clone, Copy)]
struct WindowedMin {
    current: Option<Duration>,
    previous: Option<Duration>,
    started: Instant,
}

impl WindowedMin {
    fn new(now: Instant) -> Self {
        Self {
            current: None,
            previous: None,
            started: now,
        }
    }

    fn rotate(&mut self, now: Instant) {
        if now.saturating_duration_since(self.started) >= RTT_WINDOW / 2 {
            self.previous = self.current;
            self.current = None;
            self.started = now;
        }
    }

    fn add(&mut self, sample: Duration, now: Instant) {
        self.rotate(now);
        self.current = Some(self.current.map_or(sample, |min| min.min(sample)));
    }

    fn get(&self) -> Option<Duration> {
        match (self.current, self.previous) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        }
    }
}

/// Flow control state of one connection (pure: time is passed in).
#[derive(Debug, Clone)]
pub struct Flow {
    unacked: VecDeque<Sent>,
    in_flight_bytes: usize,
    sent: u64,
    acked: u64,
    /// An acknowledgement arrived: the viewer takes part in flow control.
    active: bool,
    /// When the first acknowledgement arrived.
    since: Option<Instant>,
    base: WindowedMin,
    period_start: Instant,
    period_bytes: u64,
    period_rtts: Vec<Duration>,
    period_drops: u64,
    period_held: bool,
    samples: u64,
}

impl Flow {
    /// A connection's flow at `now`.
    #[must_use]
    pub fn new(now: Instant) -> Self {
        Self {
            unacked: VecDeque::new(),
            in_flight_bytes: 0,
            sent: 0,
            acked: 0,
            active: false,
            since: None,
            base: WindowedMin::new(now),
            period_start: now,
            period_bytes: 0,
            period_rtts: Vec::new(),
            period_drops: 0,
            period_held: false,
            samples: 0,
        }
    }

    /// Whether the viewer acknowledges frames.
    #[cfg(test)]
    #[must_use]
    pub fn active(&self) -> bool {
        self.active
    }

    /// Frames sent and not acknowledged.
    #[cfg(test)]
    #[must_use]
    pub fn in_flight(&self) -> usize {
        if self.active { self.unacked.len() } else { 0 }
    }

    /// The base round trip, once known.
    #[cfg(test)]
    #[must_use]
    pub fn base_rtt(&self) -> Option<Duration> {
        self.base.get()
    }

    /// Whether the next video frame may be written now. Only an acknowledgement reopens a
    /// closed gate (time only makes the oldest frame older).
    #[must_use]
    pub fn may_send(&self, now: Instant) -> bool {
        if !self.active {
            return true;
        }
        let Some(oldest) = self.unacked.front() else {
            return true;
        };
        if self.unacked.len() >= MAX_IN_FLIGHT_FRAMES || self.in_flight_bytes >= MAX_IN_FLIGHT_BYTES
        {
            return false;
        }
        match self.base.get() {
            None => self.unacked.len() < INITIAL_IN_FLIGHT,
            Some(base) => now.saturating_duration_since(oldest.at) < base + SLACK,
        }
    }

    /// Video was ready but the gate held it back.
    pub fn on_held(&mut self) {
        self.period_held = true;
    }

    /// A video frame of `bytes` was written.
    pub fn on_send(&mut self, bytes: usize, now: Instant) {
        self.sent += 1;
        self.unacked.push_back(Sent {
            seq: self.sent,
            bytes,
            at: now,
        });
        self.in_flight_bytes += bytes;
        if self.unacked.len() > MAX_TRACKED
            && let Some(old) = self.unacked.pop_front()
        {
            // A viewer that does not acknowledge: forget the oldest.
            self.in_flight_bytes -= old.bytes;
        }
    }

    /// The viewer received `frames` video frames so far.
    pub fn on_ack(&mut self, frames: u64, now: Instant) {
        self.active = true;
        self.since.get_or_insert(now);
        let frames = frames.min(self.sent);
        if frames <= self.acked {
            return;
        }
        self.acked = frames;
        let mut newest = None;
        while let Some(front) = self.unacked.front().copied() {
            if front.seq > frames {
                break;
            }
            self.unacked.pop_front();
            self.in_flight_bytes -= front.bytes;
            self.period_bytes += front.bytes as u64;
            newest = Some(front);
        }
        if let Some(sent) = newest {
            let rtt = now.saturating_duration_since(sent.at);
            self.base.add(rtt, now);
            self.samples += 1;
            if self.period_rtts.len() < 1024 {
                self.period_rtts.push(rtt);
            }
        }
    }

    /// `frames` video frames were dropped because the viewer fell behind.
    pub fn on_dropped(&mut self, frames: u64) {
        self.period_drops += frames;
    }

    /// The period's report once [`REPORT_PERIOD`] has passed (only for an acknowledging viewer);
    /// starts the next period.
    pub fn report(&mut self, now: Instant) -> Option<FlowReport> {
        let elapsed = now.saturating_duration_since(self.period_start);
        if elapsed < REPORT_PERIOD {
            return None;
        }
        let mut rtts = std::mem::take(&mut self.period_rtts);
        let bytes = std::mem::take(&mut self.period_bytes);
        let drops = std::mem::take(&mut self.period_drops);
        let held = std::mem::take(&mut self.period_held);
        self.period_start = now;
        if !self.active {
            return None;
        }
        rtts.sort_unstable();
        let rtt = rtts.get(rtts.len() / 2).copied();
        let base_rtt = self.base.get();
        // Frames still waiting for an acknowledgement longer than the slack also count: a path
        // that stopped delivering yields no samples at all.
        let stalled = self.unacked.front().is_some_and(|oldest| {
            now.saturating_duration_since(oldest.at) > base_rtt.unwrap_or_default() + SLACK * 2
        });
        let warm = self
            .since
            .is_some_and(|since| now.saturating_duration_since(since) >= WARM_UP);
        let queued = warm
            && self.samples >= MIN_SAMPLES
            && matches!((rtt, base_rtt), (Some(rtt), Some(base)) if rtt > base + CONGESTION_QUEUE);
        let secs = elapsed.as_secs_f64().max(0.001);
        Some(FlowReport {
            delivered_bps: (bytes as f64 * 8.0 / secs) as u64,
            congested: queued || stalled || drops > 0,
            limited: held,
            rtt,
            base_rtt,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ms(millis: u64) -> Duration {
        Duration::from_millis(millis)
    }

    #[test]
    fn a_viewer_that_never_acknowledges_is_not_paced() {
        let t0 = Instant::now();
        let mut flow = Flow::new(t0);
        for n in 0..(MAX_TRACKED as u64 + 10) {
            assert!(flow.may_send(t0 + ms(n)));
            flow.on_send(1000, t0 + ms(n));
        }
        assert_eq!(flow.in_flight(), 0);
        assert!(flow.unacked.len() <= MAX_TRACKED, "bounded memory");
        assert_eq!(flow.report(t0 + ms(5000)), None);
    }

    #[test]
    fn frames_go_out_while_the_oldest_unacknowledged_is_younger_than_rtt_plus_slack() {
        let t0 = Instant::now();
        let mut flow = Flow::new(t0);
        flow.on_send(5000, t0);
        flow.on_ack(1, t0 + ms(40)); // base round trip 40 ms
        assert_eq!(flow.base_rtt(), Some(ms(40)));
        // 60 fps over a path that stopped delivering: frames go out until the oldest
        // unacknowledged one is 40 + 120 ms old, then the gate closes.
        let mut sent = 0;
        for n in 0..40_u64 {
            let now = t0 + ms(50 + n * 17);
            if flow.may_send(now) {
                flow.on_send(5000, now);
                sent += 1;
            }
        }
        assert_eq!(sent, 10, "about RTT + slack worth of frames");
        assert!(!flow.may_send(t0 + ms(800)));
        // An acknowledgement of everything sent reopens it.
        flow.on_ack(11, t0 + ms(820));
        assert!(flow.may_send(t0 + ms(820)));
        assert_eq!(flow.in_flight(), 0);
    }

    #[test]
    fn before_the_first_sample_a_bounded_number_of_frames_is_in_flight() {
        let t0 = Instant::now();
        let mut flow = Flow::new(t0);
        flow.on_ack(0, t0); // the viewer takes part but nothing was sent yet
        for _ in 0..INITIAL_IN_FLIGHT {
            assert!(flow.may_send(t0));
            flow.on_send(100, t0);
        }
        assert!(!flow.may_send(t0));
    }

    #[test]
    fn reports_carry_the_delivered_rate_and_flag_queueing_drops_and_stalls() {
        let t0 = Instant::now();
        let mut flow = Flow::new(t0);
        // A clean second: 60 frames of 2 KB, each acknowledged 40 ms later.
        for n in 0..60_u64 {
            flow.on_send(2000, t0 + ms(n * 16));
            flow.on_ack(n + 1, t0 + ms(n * 16 + 40));
        }
        let report = flow.report(t0 + ms(1000));
        assert_eq!(
            report.map(|r| (r.delivered_bps, r.congested, r.limited)),
            Some((960_000, false, false))
        );
        // A queueing second: acknowledgements 200 ms late, the gate holding video back.
        flow.on_held();
        for n in 60..120_u64 {
            flow.on_send(2000, t0 + ms(n * 16));
            flow.on_ack(n + 1, t0 + ms(n * 16 + 200));
        }
        let report = flow.report(t0 + ms(2000 + 200));
        assert_eq!(report.map(|r| (r.congested, r.limited)), Some((true, true)));
        assert_eq!(report.and_then(|r| r.base_rtt), Some(ms(40)));
        // Too early for the next report.
        assert_eq!(flow.report(t0 + ms(2500)), None);
        // Drops count as congestion.
        flow.on_dropped(3);
        assert_eq!(flow.report(t0 + ms(3300)).map(|r| r.congested), Some(true));
        // A stalled path (no acknowledgement at all) too.
        flow.on_send(2000, t0 + ms(3400));
        assert_eq!(flow.report(t0 + ms(4400)).map(|r| r.congested), Some(true));
    }

    #[test]
    fn the_base_round_trip_follows_the_last_window() {
        let t0 = Instant::now();
        let mut flow = Flow::new(t0);
        flow.on_send(10, t0);
        flow.on_ack(1, t0 + ms(30));
        // Later the path is slower for good: after the window the old minimum is forgotten.
        let mut seq = 1;
        for k in 1..=12_u64 {
            let at = t0 + Duration::from_secs(k);
            flow.on_send(10, at);
            seq += 1;
            flow.on_ack(seq, at + ms(90));
        }
        assert_eq!(flow.base_rtt(), Some(ms(90)));
        assert!(flow.active());
    }

    #[test]
    fn stale_and_excessive_acknowledgements_are_ignored() {
        let t0 = Instant::now();
        let mut flow = Flow::new(t0);
        flow.on_send(10, t0);
        flow.on_send(10, t0);
        flow.on_ack(5, t0 + ms(10)); // more than sent: clamped
        assert_eq!(flow.in_flight(), 0);
        flow.on_ack(1, t0 + ms(20)); // older than the last
        assert_eq!(flow.in_flight(), 0);
        assert_eq!(flow.in_flight_bytes, 0);
    }

    #[test]
    fn queueing_is_judged_only_once_the_base_is_known() {
        let t0 = Instant::now();
        let mut flow = Flow::new(t0);
        // The first frames of a stream are slow (connection start, the first keyframe).
        for n in 0..3_u64 {
            flow.on_send(20_000, t0 + ms(n));
            let delay = if n == 0 { 50 } else { 400 };
            flow.on_ack(n + 1, t0 + ms(delay + n));
        }
        assert_eq!(flow.report(t0 + ms(1000)).map(|r| r.congested), Some(false));
    }
}
