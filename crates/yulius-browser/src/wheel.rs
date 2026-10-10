//! Wheel input without a backlog (SPEC-002 ADR-15, F 2026-10-10).
//!
//! Chrome answers a CDP `Input.dispatchMouseEvent` of type `mouseWheel` only once the renderer
//! handled it, about two frames later (33 ms measured on the VM). Dispatched one after the other,
//! at most about 30 wheel events per second got through while a trackpad sends 60–120, so the
//! viewer's scrolling queued up without bound (measured over the real network: the page scrolled
//! 2.5 s behind the hand after 8 s of scrolling, and the viewer socket's video waited behind each
//! dispatch).
//!
//! [`WheelDispatcher`] keeps at most [`MAX_IN_FLIGHT`] dispatches outstanding and never makes the
//! caller wait: deltas that arrive meanwhile are summed into one pending event (the newest
//! position wins) and dispatched as soon as an outstanding one completes. No delta is lost, the
//! scroll lags the input by at most one dispatch, and Chrome itself coalesces the events it
//! queues. Ownership: one task per outstanding dispatch, each ending when nothing is pending;
//! CDP calls are bounded by the client's call timeout.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use serde_json::json;
use tracing::debug;

use crate::cdp::CdpClient;

/// Wheel dispatches outstanding at once.
pub(crate) const MAX_IN_FLIGHT: usize = 2;
/// Distinct pending events (another target or modifier state) kept apart; beyond this they are
/// merged into the newest.
const MAX_PENDING: usize = 4;

/// One wheel event for a CDP target session, in CSS pixels.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct WheelEvent {
    /// The CDP session of the tab the wheel scrolls.
    pub(crate) session: String,
    /// Pointer position.
    pub(crate) x: f64,
    /// Pointer position.
    pub(crate) y: f64,
    /// Horizontal delta.
    pub(crate) dx: f64,
    /// Vertical delta.
    pub(crate) dy: f64,
    /// CDP modifier bits.
    pub(crate) modifiers: u8,
    /// CDP pressed-buttons mask (a headless browser's pointer state; 0 on a display).
    pub(crate) buttons: u32,
}

impl WheelEvent {
    fn absorb(&mut self, newer: Self) {
        self.dx += newer.dx;
        self.dy += newer.dy;
        self.x = newer.x;
        self.y = newer.y;
        self.session = newer.session;
        self.modifiers = newer.modifiers;
        self.buttons = newer.buttons;
    }

    fn compatible(&self, other: &Self) -> bool {
        self.session == other.session
            && self.modifiers == other.modifiers
            && self.buttons == other.buttons
    }
}

/// The coalescing policy (pure, unit-tested).
#[derive(Debug, Default)]
pub(crate) struct WheelQueue {
    pending: VecDeque<WheelEvent>,
    in_flight: usize,
    /// Events folded into a pending one.
    coalesced: u64,
}

impl WheelQueue {
    /// A new event; returns it when it may be dispatched now (the caller then dispatches it and
    /// reports [`Self::done`]).
    pub(crate) fn push(&mut self, event: WheelEvent) -> Option<WheelEvent> {
        if self.pending.is_empty() && self.in_flight < MAX_IN_FLIGHT {
            self.in_flight += 1;
            return Some(event);
        }
        self.coalesced += 1;
        let full = self.pending.len() >= MAX_PENDING;
        match self.pending.back_mut() {
            Some(last) if full || last.compatible(&event) => last.absorb(event),
            _ => self.pending.push_back(event),
        }
        None
    }

    /// An outstanding dispatch completed; returns the next event to dispatch, if any.
    pub(crate) fn done(&mut self) -> Option<WheelEvent> {
        self.in_flight = self.in_flight.saturating_sub(1);
        if self.in_flight < MAX_IN_FLIGHT
            && let Some(next) = self.pending.pop_front()
        {
            self.in_flight += 1;
            return Some(next);
        }
        None
    }

    #[cfg(test)]
    fn in_flight(&self) -> usize {
        self.in_flight
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Dispatches wheel events through CDP without a backlog and without blocking the caller.
#[derive(Debug, Clone)]
pub(crate) struct WheelDispatcher {
    queue: Arc<Mutex<WheelQueue>>,
    cdp: CdpClient,
}

impl WheelDispatcher {
    pub(crate) fn new(cdp: CdpClient) -> Self {
        Self {
            queue: Arc::new(Mutex::new(WheelQueue::default())),
            cdp,
        }
    }

    /// Queues `event`; returns at once.
    pub(crate) fn push(&self, event: WheelEvent) {
        let ready = lock(&self.queue).push(event);
        if let Some(event) = ready {
            let this = self.clone();
            tokio::spawn(async move { this.run(event).await });
        }
    }

    /// Wheel events folded into others so far.
    pub(crate) fn coalesced(&self) -> u64 {
        lock(&self.queue).coalesced
    }

    async fn run(self, first: WheelEvent) {
        let mut next = Some(first);
        while let Some(event) = next {
            let params = json!({
                "type": "mouseWheel",
                "x": event.x,
                "y": event.y,
                "deltaX": event.dx,
                "deltaY": event.dy,
                "button": "none",
                "buttons": event.buttons,
                "clickCount": 0,
                "modifiers": event.modifiers & 0x0F,
            });
            if let Err(error) = self
                .cdp
                .call_session(&event.session, "Input.dispatchMouseEvent", params)
                .await
            {
                debug!(%error, "wheel dispatch failed");
            }
            next = lock(&self.queue).done();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wheel(session: &str, dy: f64, x: f64) -> WheelEvent {
        WheelEvent {
            session: session.into(),
            x,
            y: 10.0,
            dx: 0.0,
            dy,
            modifiers: 0,
            buttons: 0,
        }
    }

    #[test]
    fn events_go_out_at_once_until_the_limit_then_are_summed() {
        let mut queue = WheelQueue::default();
        assert!(queue.push(wheel("s", 50.0, 1.0)).is_some());
        assert!(queue.push(wheel("s", 50.0, 2.0)).is_some());
        assert_eq!(queue.in_flight(), MAX_IN_FLIGHT);
        // A trackpad keeps sending while both dispatches are outstanding: one pending sum.
        for n in 0..10 {
            assert!(queue.push(wheel("s", 10.0, 3.0 + f64::from(n))).is_none());
        }
        let next = queue.done();
        assert_eq!(
            next,
            Some(wheel("s", 100.0, 12.0)),
            "summed, newest position"
        );
        assert_eq!(queue.in_flight(), MAX_IN_FLIGHT);
        assert_eq!(queue.done(), None);
        assert_eq!(queue.done(), None);
        assert_eq!(queue.in_flight(), 0);
        assert_eq!(queue.coalesced, 10);
    }

    #[test]
    fn nothing_is_lost_however_fast_the_input_is() {
        let mut queue = WheelQueue::default();
        let mut sent = 0.0;
        let mut outstanding = Vec::new();
        for n in 0..1000 {
            if let Some(event) = queue.push(wheel("s", 7.0, 1.0)) {
                outstanding.push(event);
            }
            // Dispatches complete at a third of the input rate.
            if n % 3 == 0 && !outstanding.is_empty() {
                sent += outstanding.remove(0).dy;
                if let Some(event) = queue.done() {
                    outstanding.push(event);
                }
            }
            assert!(outstanding.len() <= MAX_IN_FLIGHT);
        }
        while !outstanding.is_empty() {
            sent += outstanding.remove(0).dy;
            if let Some(event) = queue.done() {
                outstanding.push(event);
            }
        }
        assert!(
            (sent - 7000.0).abs() < 1e-6,
            "every delta dispatched: {sent}"
        );
    }

    #[test]
    fn another_target_or_modifier_state_is_kept_apart_and_bounded() {
        let mut queue = WheelQueue::default();
        let _ = queue.push(wheel("a", 1.0, 1.0));
        let _ = queue.push(wheel("a", 1.0, 1.0));
        let _ = queue.push(wheel("a", 5.0, 1.0));
        let _ = queue.push(wheel("b", 7.0, 1.0));
        let mut zoom = wheel("b", 3.0, 1.0);
        zoom.modifiers = 2;
        let _ = queue.push(zoom.clone());
        assert_eq!(queue.pending.len(), 3);
        for n in 0..20 {
            let _ = queue.push(wheel(&format!("t{n}"), 1.0, 1.0));
        }
        assert_eq!(queue.pending.len(), MAX_PENDING, "bounded");
        assert_eq!(
            queue.done().map(|e| (e.session, e.dy)),
            Some(("a".into(), 5.0))
        );
        assert_eq!(
            queue.done().map(|e| (e.session, e.dy)),
            Some(("b".into(), 7.0))
        );
        assert_eq!(queue.done().map(|e| e.modifiers), Some(2));
    }
}
