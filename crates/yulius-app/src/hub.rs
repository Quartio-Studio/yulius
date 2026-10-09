//! Per-task terminal hub: bounded fan-out of PTY output to viewers plus a server-side screen
//! model for snapshots on (re)connect (SPEC-002 §7.7, NFR-002/004).
//!
//! The hub outlives individual runs: account or provider switches swap the run underneath while
//! viewers stay connected. Publishing never blocks; viewers that fall behind lag out and
//! reconnect with a fresh snapshot.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::sync::broadcast;
use tokio::sync::broadcast::error::RecvError;

/// Per-viewer buffered frames before a viewer is considered lagging.
pub const VIEWER_QUEUE_FRAMES: usize = 512;
/// Largest output chunk forwarded as one frame.
pub const MAX_FRAME_BYTES: usize = 64 * 1024;
/// A redraw counts as finished after this long without output ([`Hub::settle`]).
pub const SETTLE_QUIET: Duration = Duration::from_millis(40);

/// A frame delivered to viewers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HubFrame {
    /// PTY output.
    Output {
        /// Sequence number.
        seq: u64,
        /// Bytes.
        bytes: Arc<[u8]>,
    },
    /// Clear and redraw.
    Snapshot {
        /// Sequence of the last included output.
        seq: u64,
        /// Formatted screen.
        bytes: Arc<[u8]>,
    },
    /// Informational line.
    Notice(String),
    /// Session facts changed (status, harness, size).
    State,
    /// The run ended.
    Exit {
        /// Task status after the exit.
        status: String,
        /// Exit code.
        code: Option<i32>,
    },
}

struct Screen {
    parser: vt100::Parser,
    seq: u64,
}

/// Terminal fan-out for one task.
pub struct Hub {
    tx: broadcast::Sender<HubFrame>,
    screen: Mutex<Screen>,
    viewers: AtomicU64,
    /// Viewer ids handed out to attached viewers.
    next_viewer: AtomicU64,
    /// The viewer that controls input focus and PTY size (0 = nobody yet; TSK-006).
    controller: AtomicU64,
}

/// A viewer subscription with its starting snapshot.
pub struct Subscription {
    /// Snapshot sequence.
    pub seq: u64,
    /// Formatted screen to draw first.
    pub snapshot: Vec<u8>,
    /// Current size.
    pub size: (u16, u16),
    /// Subsequent frames.
    pub rx: broadcast::Receiver<HubFrame>,
}

impl Hub {
    /// A hub with an empty screen of `cols`×`rows`.
    #[must_use]
    pub fn new(cols: u16, rows: u16) -> Self {
        let (tx, _) = broadcast::channel(VIEWER_QUEUE_FRAMES);
        Self {
            tx,
            screen: Mutex::new(Screen {
                parser: vt100::Parser::new(rows.max(1), cols.max(1), 0),
                seq: 0,
            }),
            viewers: AtomicU64::new(0),
            next_viewer: AtomicU64::new(1),
            controller: AtomicU64::new(0),
        }
    }

    /// Feeds PTY output (sequence from the host) and fans it out.
    pub fn output(&self, seq: u64, bytes: &[u8]) {
        if let Ok(mut screen) = self.screen.lock() {
            screen.parser.process(bytes);
            screen.seq = seq;
        }
        for chunk in bytes.chunks(MAX_FRAME_BYTES) {
            let _ = self.tx.send(HubFrame::Output {
                seq,
                bytes: Arc::from(chunk),
            });
        }
    }

    /// Replaces the screen with a host snapshot (attach/reattach) and tells viewers to redraw.
    pub fn reset(&self, seq: u64, snapshot: &[u8], cols: u16, rows: u16) {
        let formatted = if let Ok(mut screen) = self.screen.lock() {
            screen.parser = vt100::Parser::new(rows.max(1), cols.max(1), 0);
            screen.parser.process(snapshot);
            screen.seq = seq;
            viewer_snapshot(screen.parser.screen())
        } else {
            snapshot.to_vec()
        };
        let _ = self.tx.send(HubFrame::Snapshot {
            seq,
            bytes: Arc::from(formatted),
        });
    }

    /// Resizes the screen model and tells viewers; the same size again changes nothing and
    /// sends nothing (every viewer's hello repeats its size).
    pub fn resize(&self, cols: u16, rows: u16) {
        let (cols, rows) = (cols.max(1), rows.max(1));
        if let Ok(mut screen) = self.screen.lock() {
            if screen.parser.screen().size() == (rows, cols) {
                return;
            }
            screen.parser.screen_mut().set_size(rows, cols);
        }
        let _ = self.tx.send(HubFrame::State);
    }

    /// Current size (cols, rows).
    #[must_use]
    pub fn size(&self) -> (u16, u16) {
        self.screen
            .lock()
            .map(|s| {
                let (rows, cols) = s.parser.screen().size();
                (cols, rows)
            })
            .unwrap_or((80, 24))
    }

    /// The screen as escape sequences that redraw it, with its size (cols, rows).
    #[must_use]
    pub fn formatted_screen(&self) -> (Vec<u8>, (u16, u16)) {
        self.screen
            .lock()
            .map(|s| {
                let (rows, cols) = s.parser.screen().size();
                (s.parser.screen().contents_formatted(), (cols, rows))
            })
            .unwrap_or_default()
    }

    /// Sequence number of the latest output (0 before any).
    #[must_use]
    pub fn output_seq(&self) -> u64 {
        self.screen.lock().map(|s| s.seq).unwrap_or(0)
    }

    /// Whether any output reached this hub since it was created or reset.
    #[must_use]
    pub fn has_output(&self) -> bool {
        self.screen.lock().map(|s| s.seq > 0).unwrap_or(false)
    }

    /// Visible screen text (for previews and tests).
    #[must_use]
    pub fn screen_text(&self) -> String {
        self.screen
            .lock()
            .map(|s| s.parser.screen().contents())
            .unwrap_or_default()
    }

    /// Sends a notice line to viewers.
    pub fn notice(&self, text: impl Into<String>) {
        let _ = self.tx.send(HubFrame::Notice(text.into()));
    }

    /// Signals a state change (status/harness) to viewers.
    pub fn state_changed(&self) {
        let _ = self.tx.send(HubFrame::State);
    }

    /// Signals the end of a run.
    pub fn exit(&self, status: &str, code: Option<i32>) {
        let _ = self.tx.send(HubFrame::Exit {
            status: status.to_owned(),
            code,
        });
    }

    /// A receiver of the frames from now on, for [`Self::settle`] (not a viewer).
    #[must_use]
    pub fn watch(&self) -> broadcast::Receiver<HubFrame> {
        self.tx.subscribe()
    }

    /// Waits, at most `limit`, for the redraw that a resize triggers: output newer than `since`
    /// on `watch` (taken before the resize), then [`SETTLE_QUIET`] without more. A program that
    /// does not redraw costs the whole `limit`; a closed hub returns at once.
    pub async fn settle(
        &self,
        mut watch: broadcast::Receiver<HubFrame>,
        since: u64,
        limit: Duration,
    ) {
        let deadline = tokio::time::Instant::now() + limit;
        let mut redrawn = self.output_seq() > since;
        loop {
            let until = if redrawn {
                (tokio::time::Instant::now() + SETTLE_QUIET).min(deadline)
            } else {
                deadline
            };
            match tokio::time::timeout_at(until, watch.recv()).await {
                Ok(Ok(HubFrame::Output { .. }) | Err(RecvError::Lagged(_))) => redrawn = true,
                Ok(Ok(_)) => {}
                Ok(Err(RecvError::Closed)) | Err(_) => return,
            }
        }
    }

    /// Subscribes a viewer: snapshot and receiver are taken atomically so no output is lost or
    /// duplicated between them.
    #[must_use]
    pub fn subscribe(&self) -> Subscription {
        let screen = self.screen.lock();
        let rx = self.tx.subscribe();
        self.viewers.fetch_add(1, Ordering::SeqCst);
        match screen {
            Ok(s) => {
                let (rows, cols) = s.parser.screen().size();
                Subscription {
                    seq: s.seq,
                    snapshot: viewer_snapshot(s.parser.screen()),
                    size: (cols, rows),
                    rx,
                }
            }
            Err(_) => Subscription {
                seq: 0,
                snapshot: Vec::new(),
                size: (80, 24),
                rx,
            },
        }
    }

    /// Marks a viewer gone.
    pub fn unsubscribe(&self) {
        let _ = self
            .viewers
            .try_update(Ordering::SeqCst, Ordering::SeqCst, |v| {
                Some(v.saturating_sub(1))
            });
    }

    /// Connected viewers.
    #[must_use]
    pub fn viewer_count(&self) -> u64 {
        self.viewers.load(Ordering::SeqCst)
    }

    /// A new viewer id (never 0).
    pub fn new_viewer_id(&self) -> u64 {
        self.next_viewer.fetch_add(1, Ordering::SeqCst)
    }

    /// The controlling viewer (0 = nobody).
    #[must_use]
    pub fn controller(&self) -> u64 {
        self.controller.load(Ordering::SeqCst)
    }

    /// Makes `viewer` the controller; tells every viewer when control changed.
    pub fn take_control(&self, viewer: u64) {
        if self.controller.swap(viewer, Ordering::SeqCst) != viewer {
            self.state_changed();
        }
    }

    /// Whether `viewer` may resize: it controls, or nobody does yet (it then takes control).
    pub fn may_resize(&self, viewer: u64) -> bool {
        match self
            .controller
            .compare_exchange(0, viewer, Ordering::SeqCst, Ordering::SeqCst)
        {
            Ok(_) => {
                self.state_changed();
                true
            }
            Err(current) => current == viewer,
        }
    }

    /// Releases control when `viewer` leaves.
    pub fn release(&self, viewer: u64) {
        if self
            .controller
            .compare_exchange(viewer, 0, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
        {
            self.state_changed();
        }
    }
}

/// What a viewer needs to redraw the terminal as the app left it: the alternate screen when it
/// is on, the contents, the input modes (mouse reporting and its encoding, application keys,
/// bracketed paste), the cursor and the attributes — as the process host's own snapshot does.
/// Without the screen and mouse modes, a viewer who opens the task after a full-screen app
/// started could never scroll it with the wheel (#78).
fn viewer_snapshot(screen: &vt100::Screen) -> Vec<u8> {
    let mut bytes = Vec::new();
    if screen.alternate_screen() {
        bytes.extend_from_slice(b"\x1b[?1049h");
    }
    bytes.extend(screen.contents_formatted());
    bytes.extend(screen.input_mode_formatted());
    bytes.extend(screen.cursor_state_formatted());
    bytes.extend(screen.attributes_formatted());
    bytes
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn control_follows_the_last_writer_and_is_released_on_leave() {
        let hub = Hub::new(20, 5);
        let (a, b) = (hub.new_viewer_id(), hub.new_viewer_id());
        assert_ne!(a, b);
        assert_eq!(hub.controller(), 0);
        assert!(hub.may_resize(a), "the first resizer takes control");
        assert!(!hub.may_resize(b), "others keep watching at that size");
        hub.take_control(b);
        assert_eq!(hub.controller(), b);
        assert!(!hub.may_resize(a));
        hub.release(a);
        assert_eq!(hub.controller(), b, "only the controller's leave releases");
        hub.release(b);
        assert_eq!(hub.controller(), 0);
    }

    /// #78: a viewer who opens or reloads the task while a full-screen app runs gets the
    /// alternate screen and the app's mouse modes, so the wheel reaches the app (#34) for every
    /// viewer, not only those present when the app started.
    #[test]
    fn late_viewers_get_the_alternate_screen_and_mouse_modes() {
        let hub = Hub::new(40, 10);
        hub.output(1, b"\x1b[?1049h\x1b[?1000h\x1b[?1006hfull screen");
        let text = String::from_utf8_lossy(&hub.subscribe().snapshot).into_owned();
        for mode in ["\x1b[?1049h", "\x1b[?1000h", "\x1b[?1006h"] {
            assert!(text.contains(mode), "{mode:?} missing from {text:?}");
        }
        assert!(text.contains("full screen"));
        hub.reset(2, b"\x1b[?1049h\x1b[?1002h\x1b[?1006hagain", 40, 10);
        let again = String::from_utf8_lossy(&hub.subscribe().snapshot).into_owned();
        assert!(
            again.contains("\x1b[?1049h") && again.contains("\x1b[?1002h"),
            "after a host reset: {again:?}"
        );
    }

    /// Every viewer's hello repeats its size: the same size again changes nothing and tells
    /// nobody (F 2026-10-09: no state frames, no redraws for nothing).
    #[test]
    fn resizing_to_the_same_size_sends_nothing() {
        let hub = Hub::new(20, 5);
        let mut sub = hub.subscribe();
        hub.resize(20, 5);
        assert!(sub.rx.try_recv().is_err(), "no frame for the same size");
        hub.resize(30, 6);
        assert!(matches!(sub.rx.try_recv(), Ok(HubFrame::State)));
        assert_eq!(hub.size(), (30, 6));
    }

    /// The first screen after a resize waits for the program's redraw, then for a pause, and
    /// never longer than its limit (F 2026-10-09).
    #[tokio::test]
    async fn settle_waits_for_the_redraw_and_a_pause_within_its_limit() {
        let hub = Arc::new(Hub::new(20, 5));
        // A program that redraws in two chunks after the resize.
        let since = hub.output_seq();
        let watch = hub.watch();
        let drawer = Arc::clone(&hub);
        let redraw = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(30)).await;
            drawer.output(1, b"redrawn ");
            tokio::time::sleep(Duration::from_millis(10)).await;
            drawer.output(2, b"at the new size");
        });
        let started = tokio::time::Instant::now();
        hub.settle(watch, since, Duration::from_secs(5)).await;
        let waited = started.elapsed();
        redraw.await.ok();
        assert!(
            hub.screen_text().contains("at the new size"),
            "settled after the redraw"
        );
        assert!(
            waited >= Duration::from_millis(40) + SETTLE_QUIET,
            "{waited:?}"
        );
        assert!(
            waited < Duration::from_secs(2),
            "a pause ends the wait: {waited:?}"
        );

        // A program that does not redraw costs the limit, no more.
        let started = tokio::time::Instant::now();
        hub.settle(hub.watch(), hub.output_seq(), Duration::from_millis(120))
            .await;
        let waited = started.elapsed();
        assert!(
            waited >= Duration::from_millis(120) && waited < Duration::from_secs(2),
            "{waited:?}"
        );
    }

    #[test]
    fn snapshot_then_frames_without_gap() {
        let hub = Hub::new(20, 5);
        hub.output(1, b"hello");
        let mut sub = hub.subscribe();
        assert_eq!(sub.seq, 1);
        assert!(String::from_utf8_lossy(&sub.snapshot).contains("hello"));
        hub.output(2, b" world");
        match sub.rx.try_recv() {
            Ok(HubFrame::Output { seq, bytes }) => {
                assert_eq!(seq, 2);
                assert_eq!(&bytes[..], b" world");
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn lagging_viewer_does_not_block_publisher() {
        let hub = Hub::new(80, 24);
        let mut sub = hub.subscribe();
        for i in 0..(VIEWER_QUEUE_FRAMES as u64 * 2) {
            hub.output(i, b"x");
        }
        assert!(matches!(
            sub.rx.try_recv(),
            Err(broadcast::error::TryRecvError::Lagged(_))
        ));
        // A new subscription still gets the full current screen.
        let fresh = hub.subscribe();
        assert_eq!(fresh.seq, VIEWER_QUEUE_FRAMES as u64 * 2 - 1);
    }

    #[test]
    fn large_output_is_chunked() {
        let hub = Hub::new(80, 24);
        let mut sub = hub.subscribe();
        hub.output(1, &vec![b'a'; MAX_FRAME_BYTES * 2 + 10]);
        let mut frames = 0;
        while let Ok(HubFrame::Output { bytes, .. }) = sub.rx.try_recv() {
            assert!(bytes.len() <= MAX_FRAME_BYTES);
            frames += 1;
        }
        assert_eq!(frames, 3);
    }
}
