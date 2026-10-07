//! Per-task terminal hub: bounded fan-out of PTY output to viewers plus a server-side screen
//! model for snapshots on (re)connect (SPEC-002 §7.7, NFR-002/004).
//!
//! The hub outlives individual runs: account or provider switches swap the run underneath while
//! viewers stay connected. Publishing never blocks; viewers that fall behind lag out and
//! reconnect with a fresh snapshot.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use tokio::sync::broadcast;

/// Per-viewer buffered frames before a viewer is considered lagging.
pub const VIEWER_QUEUE_FRAMES: usize = 512;
/// Largest output chunk forwarded as one frame.
pub const MAX_FRAME_BYTES: usize = 64 * 1024;

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
            screen.parser.screen().contents_formatted()
        } else {
            snapshot.to_vec()
        };
        let _ = self.tx.send(HubFrame::Snapshot {
            seq,
            bytes: Arc::from(formatted),
        });
    }

    /// Resizes the screen model.
    pub fn resize(&self, cols: u16, rows: u16) {
        if let Ok(mut screen) = self.screen.lock() {
            screen
                .parser
                .screen_mut()
                .set_size(rows.max(1), cols.max(1));
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
                    snapshot: s.parser.screen().contents_formatted(),
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
}

#[cfg(test)]
mod tests {
    use super::*;

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
