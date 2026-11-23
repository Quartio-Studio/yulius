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
use yulius_domain::harness::Harness;

pub mod control;

pub use control::Change;
use control::SizeControl;

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
        /// Why it ended, where there is a reason to give: the limit the sandbox was at when a
        /// run ended (`pids`, `memory`; SPEC-268 PID-005), or why a sign-in failed.
        cause: Option<String>,
    },
}

#[derive(Clone, Copy)]
pub(crate) enum ScreenUpdate {
    /// Host snapshot attested at the dimensions used to restore it.
    Host {
        rendered: bool,
    },
    Park,
    /// Read from disk before any host attached or any other viewer restored the grid.
    Cold {
        parked: bool,
    },
}

struct Screen {
    initialized: bool,
    parked: bool,
    /// Resizing can crop a hint or a draft. A host snapshot can already contain that
    /// crop. Only fresh PTY output can reestablish an empty-box proof.
    redraw: yulius_contracts::ptyhost::Redraw,
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
    /// Writer viewers and the one that controls the PTY size (TSK-006, [`control`]).
    control: Mutex<SizeControl>,
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
                initialized: false,
                parked: false,
                redraw: Default::default(),
                parser: vt100::Parser::new(rows.max(1), cols.max(1), 0),
                seq: 0,
            }),
            viewers: AtomicU64::new(0),
            next_viewer: AtomicU64::new(1),
            control: Mutex::new(SizeControl::default()),
        }
    }

    /// Feeds PTY output (sequence from the host) and fans it out.
    pub fn output(&self, seq: u64, bytes: &[u8]) {
        let Ok(mut screen) = self.screen.lock() else {
            return;
        };
        if screen.parked {
            return;
        }
        screen.initialized = true;
        let Screen { redraw, parser, .. } = &mut *screen;
        redraw.process(parser, bytes);
        screen.seq = seq;
        for chunk in bytes.chunks(MAX_FRAME_BYTES) {
            let _ = self.tx.send(HubFrame::Output {
                seq,
                bytes: Arc::from(chunk),
            });
        }
    }

    /// Replaces the screen with a host snapshot (attach/reattach) and tells viewers to redraw.
    pub fn reset(&self, seq: u64, snapshot: &[u8], cols: u16, rows: u16) {
        self.restore(
            seq,
            snapshot,
            cols,
            rows,
            ScreenUpdate::Host { rendered: false },
        );
    }

    /// Replace the visible grid and announce a change between a parked capture and a host
    /// screen before sending its bytes. Only a new host screen releases a parked grid.
    pub(crate) fn restore(
        &self,
        seq: u64,
        snapshot: &[u8],
        cols: u16,
        rows: u16,
        update: ScreenUpdate,
    ) {
        let Ok(mut screen) = self.screen.lock() else {
            return;
        };
        let parked = match update {
            ScreenUpdate::Host { .. } => false,
            ScreenUpdate::Park => true,
            ScreenUpdate::Cold { .. } if screen.initialized => return,
            ScreenUpdate::Cold { parked } => parked,
        };
        let announce = parked || screen.parked;
        screen.initialized = true;
        screen.parked = parked;
        screen.redraw = yulius_contracts::ptyhost::Redraw::restored(matches!(
            update,
            ScreenUpdate::Host { rendered: true }
        ));
        screen.parser = vt100::Parser::new(rows.max(1), cols.max(1), 0);
        screen.parser.process(snapshot);
        screen.seq = seq;
        if announce {
            self.state_changed();
        }
        let _ = self.tx.send(HubFrame::Snapshot {
            seq,
            bytes: Arc::from(viewer_snapshot(screen.parser.screen())),
        });
    }

    /// Resizes the screen model and tells viewers; the same size again changes nothing and
    /// sends nothing (every viewer's hello repeats its size).
    pub fn resize(&self, cols: u16, rows: u16) {
        let (cols, rows) = (cols.max(1), rows.max(1));
        if let Ok(mut screen) = self.screen.lock() {
            if screen.parked || screen.parser.screen().size() == (rows, cols) {
                return;
            }
            screen.redraw.invalidate();
            screen.parser.screen_mut().set_size(rows, cols);
        }
        let _ = self.tx.send(HubFrame::State);
    }

    /// Current size (cols, rows).
    #[must_use]
    pub fn size(&self) -> (u16, u16) {
        self.grid_state().0
    }

    /// Grid dimensions and source, read from the same snapshot for a state frame.
    pub(crate) fn grid_state(&self) -> ((u16, u16), bool) {
        self.screen
            .lock()
            .map(|screen| {
                let (rows, cols) = screen.parser.screen().size();
                ((cols, rows), screen.parked)
            })
            .unwrap_or(((80, 24), false))
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

    /// True after the first host snapshot, including a host that started at sequence zero.
    pub(crate) fn initialized(&self) -> bool {
        self.screen.lock().is_ok_and(|screen| screen.initialized)
    }

    /// Whether any output reached this hub since it was created or reset.
    #[must_use]
    pub fn has_output(&self) -> bool {
        self.screen.lock().map(|s| s.seq > 0).unwrap_or(false)
    }

    /// Positive evidence from live renderer output at the current terminal geometry.
    pub(crate) fn prompt_box_shown_empty(&self, harness: Harness) -> bool {
        self.screen.lock().is_ok_and(|s| {
            s.redraw.rendered()
                && match harness {
                    Harness::ClaudeCode => crate::prompt_box::shown_empty(s.parser.screen()),
                    Harness::Codex => crate::prompt_box::codex_shown_empty(s.parser.screen()),
                    Harness::Antigravity => false,
                }
        })
    }

    /// Whether the live VT screen shows text in Claude Code's conversation input.
    pub(crate) fn shows_draft(&self) -> bool {
        self.screen
            .lock()
            .is_ok_and(|s| crate::visible_draft::shown(s.parser.screen()))
    }

    /// Whether the current grid shows Codex's rate-limit model picker. An
    /// alternate grid needs a repaint and a visible input cursor to rule it out.
    /// While a partial redraw shows only the title, the native picker still hides the cursor.
    pub(crate) fn codex_rate_limit_picker_shown(&self) -> Option<bool> {
        let screen = self.screen.lock().ok()?;
        let grid = screen.parser.screen();
        if crate::prompt_box::codex_rate_limit_picker_shown(grid) {
            return Some(true);
        }
        if grid.alternate_screen()
            && (!screen.redraw.rendered()
                || grid.hide_cursor()
                || grid.rows(0, grid.size().1).all(|row| row.trim().is_empty()))
        {
            return None;
        }
        Some(false)
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
        self.exit_at(status, code, None);
    }

    /// Signals the end of a run that ended while its sandbox was at `cause`, a limit (SPEC-268
    /// PID-005), or of a sign-in that failed for `cause`.
    pub fn exit_at(&self, status: &str, code: Option<i32>, cause: Option<&str>) {
        let _ = self.tx.send(HubFrame::Exit {
            status: status.to_owned(),
            code,
            cause: cause.map(str::to_owned),
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
        while tokio::time::Instant::now() < deadline {
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
        self.control.lock().map(|c| c.controller()).unwrap_or(0)
    }

    /// The size the controlling viewer's window fits, when it reported one.
    #[must_use]
    pub fn controller_size(&self) -> Option<(u16, u16)> {
        self.control.lock().ok().and_then(|c| c.controller_size())
    }

    /// Runs one size-control operation; tells every viewer when control moved.
    fn control(&self, operation: impl FnOnce(&mut SizeControl) -> Change) -> Change {
        let change = self
            .control
            .lock()
            .map(|mut control| operation(&mut control))
            .unwrap_or_default();
        if change.moved {
            self.state_changed();
        }
        change
    }

    /// A writer attached with the size its window fits ([`SizeControl::join`]).
    pub fn join_writer(&self, viewer: u64, size: Option<(u16, u16)>) -> Change {
        self.control(|c| c.join(viewer, size))
    }

    /// The size a writer's window fits now ([`SizeControl::report_size`]).
    pub fn report_size(&self, viewer: u64, size: (u16, u16)) -> Change {
        self.control(|c| c.report_size(viewer, size))
    }

    /// A writer is active ([`SizeControl::active`]).
    pub fn viewer_active(&self, viewer: u64) -> Change {
        self.control(|c| c.active(viewer))
    }

    /// A writer typed or clicked into the terminal ([`SizeControl::claim`]).
    pub fn claim(&self, viewer: u64) -> Change {
        self.control(|c| c.claim(viewer))
    }

    /// A writer went idle ([`SizeControl::idle`]).
    pub fn viewer_idle(&self, viewer: u64) -> Change {
        self.control(|c| c.idle(viewer))
    }

    /// A writer left or lost write access ([`SizeControl::leave`]); safe to call twice.
    pub fn leave(&self, viewer: u64) -> Change {
        self.control(|c| c.leave(viewer))
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
    use super::Harness;
    use super::*;

    #[test]
    fn a_parked_grid_ignores_late_output_and_viewer_sizes_until_the_next_host_snapshot() {
        let hub = Hub::new(80, 24);
        hub.restore(7, b"last answer", 80, 24, ScreenUpdate::Park);
        hub.resize(22, 5);
        hub.output(8, b"old provider exit");
        assert_eq!(hub.grid_state(), ((80, 24), true));
        assert_eq!(hub.output_seq(), 7);
        assert_eq!(hub.screen_text(), "last answer");
        let mut messages = hub.subscribe().rx;
        hub.reset(1, b"resumed", 22, 5);
        assert!(matches!(messages.try_recv(), Ok(HubFrame::State)));
        assert!(matches!(messages.try_recv(), Ok(HubFrame::Snapshot { .. })));
        hub.resize(30, 7);
        hub.output(2, b" answer");
        assert_eq!(hub.grid_state(), ((30, 7), false));
        assert_eq!(hub.screen_text(), "resumed answer");
    }

    #[test]
    fn a_cold_viewer_finishing_its_read_after_reattach_keeps_the_new_host() {
        let hub = Hub::new(80, 24);
        hub.restore(
            1,
            b"last answer",
            90,
            28,
            ScreenUpdate::Cold { parked: true },
        );
        assert_eq!(hub.grid_state(), ((90, 28), true));
        assert_eq!(hub.screen_text(), "last answer");
        hub.reset(0, b"new host", 40, 12);
        hub.restore(
            1,
            b"old capture",
            80,
            24,
            ScreenUpdate::Cold { parked: true },
        );
        assert_eq!(hub.grid_state(), ((40, 12), false));
        assert_eq!(hub.screen_text(), "new host");
    }

    #[test]
    fn control_moves_tell_every_viewer() {
        let hub = Hub::new(20, 5);
        let mut sub = hub.subscribe();
        let (a, b) = (hub.new_viewer_id(), hub.new_viewer_id());
        assert_ne!(a, b);
        assert_eq!(hub.controller(), 0);
        assert_eq!(hub.join_writer(a, Some((30, 6))).resize, Some((30, 6)));
        assert!(
            matches!(sub.rx.try_recv(), Ok(HubFrame::State)),
            "a controls"
        );
        assert!(!hub.join_writer(b, Some((40, 8))).moved, "b watches");
        assert!(sub.rx.try_recv().is_err(), "nothing moved, nothing sent");
        assert_eq!(hub.claim(b).resize, Some((40, 8)));
        assert_eq!(hub.controller(), b);
        assert!(matches!(sub.rx.try_recv(), Ok(HubFrame::State)));
        assert!(!hub.leave(a).moved, "a watcher leaving moves nothing");
        assert!(hub.leave(b).moved);
        assert!(!hub.leave(b).moved, "leaving twice is harmless");
        assert_eq!(hub.controller(), 0);
    }

    #[test]
    fn a_resize_cannot_crop_a_stash_into_proof_of_emptiness() {
        let hub = Hub::new(40, 12);
        let frame = |cols: usize| {
            let rule = "─".repeat(cols);
            format!("\x1b[?1049h\x1b[H\x1b[2J\x1b[8;1H{rule}\x1b[9;1H❯ \x1b[10;1H{rule}\x1b[9;3H")
        };
        let mut stashed = frame(40);
        stashed.push_str("\x1b[7;31H› stashed\x1b[9;3H");
        hub.output(1, stashed.as_bytes());
        assert!(!hub.prompt_box_shown_empty(Harness::ClaudeCode));
        hub.resize(20, 12);
        assert!(
            !hub.prompt_box_shown_empty(Harness::ClaudeCode),
            "a local crop is not a redraw"
        );
        let recovered = Hub::new(20, 12);
        recovered.reset(1, &hub.subscribe().snapshot, 20, 12);
        assert!(
            !recovered.prompt_box_shown_empty(Harness::ClaudeCode),
            "a host snapshot can carry the same crop"
        );
        recovered.output(2, b"\x1b]0;title\x1b[2J\x07");
        assert!(
            !recovered.prompt_box_shown_empty(Harness::ClaudeCode),
            "a title cannot certify a redraw"
        );
        hub.output(2, b"\x1b[11;1Hstatus\x1b[9;3H");
        assert!(
            !hub.prompt_box_shown_empty(Harness::ClaudeCode),
            "a status update leaves the crop unproven"
        );
        hub.output(
            3,
            format!("{}\x1b[7;10H› stashed\x1b[9;3H", frame(20)).as_bytes(),
        );
        assert!(!hub.prompt_box_shown_empty(Harness::ClaudeCode));
        hub.output(4, frame(20).as_bytes());
        assert!(
            hub.prompt_box_shown_empty(Harness::ClaudeCode),
            "Claude redrew an empty box"
        );
    }

    #[test]
    fn a_genuine_repaint_is_recognized_in_every_pty_partition() {
        let rule = "─".repeat(20);
        let frame = format!("\x1b[?1049h\x1b[2J\x1b[2;1H{rule}\x1b[3;1H❯ \x1b[4;1H{rule}\x1b[3;3H");
        for prefix in ["J", "\x1b]0;title[2J\x07"] {
            let output = format!("{prefix}{frame}");
            for cut in 0..=output.len() {
                let hub = Hub::new(20, 6);
                hub.reset(1, frame.as_bytes(), 20, 6);
                hub.output(2, &output.as_bytes()[..cut]);
                hub.output(3, &output.as_bytes()[cut..]);
                assert!(
                    hub.prompt_box_shown_empty(Harness::ClaudeCode),
                    "partition {cut} of {prefix:?}"
                );
            }
        }
    }

    /// SPEC-572 AC3: a hub rebuilt from a snapshot (a reattached run, as after a restart of the
    /// server) still reads the prompt box: empty, a dim suggestion, and a draft.
    #[test]
    fn a_snapshot_requires_a_live_redraw_before_releasing_typing() -> Result<(), std::io::Error> {
        let rule = "─".repeat(20);
        let framed = |row: &str| {
            format!(
                "\x1b[?1049h\x1b[H\x1b[2J\x1b[2;1H{rule}\x1b[3;1H{row}\x1b[0m\x1b[4;1H{rule}\x1b[5;1H  status\x1b[3;3H"
            )
        };
        for (row, empty) in [
            ("❯ ", true),
            ("❯ \x1b[7mr\x1b[0m\x1b[2mun the tests", true),
            ("❯ \x1b[2mrun the tests", true),
            ("❯ \x1b[7mh\x1b[0malf a thought", false),
            ("❯ half", false),
        ] {
            let hub = Hub::new(20, 6);
            hub.output(1, framed(row).as_bytes());
            assert_eq!(
                hub.prompt_box_shown_empty(Harness::ClaudeCode),
                empty,
                "{row:?}"
            );
            let snapshot = hub
                .screen
                .lock()
                .map(|s| viewer_snapshot(s.parser.screen()))
                .unwrap_or_default();
            let again = Hub::new(20, 6);
            again.restore(1, &snapshot, 20, 6, super::ScreenUpdate::Park);
            assert!(
                !again.prompt_box_shown_empty(Harness::ClaudeCode),
                "a parked snapshot cannot release typing"
            );
            again.reset(2, &snapshot, 20, 6);
            assert!(
                !again.prompt_box_shown_empty(Harness::ClaudeCode),
                "a snapshot may contain a host-side crop"
            );
            let repaint = framed(row);
            let (before_clear, after_clear) = repaint
                .split_once("2J")
                .ok_or_else(|| std::io::Error::other("fixture lacks ED 2"))?;
            again.output(3, before_clear.as_bytes());
            again.output(4, format!("2J{after_clear}").as_bytes());
            assert_eq!(
                again.prompt_box_shown_empty(Harness::ClaudeCode),
                empty,
                "after a renderer repaint: {row:?}"
            );
            again.restore(
                5,
                &snapshot,
                20,
                6,
                super::ScreenUpdate::Host { rendered: true },
            );
            assert_eq!(
                again.prompt_box_shown_empty(Harness::ClaudeCode),
                empty,
                "rendered host: {row:?}"
            );
        }
        Ok(())
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

    #[test]
    fn a_partial_codex_picker_repaint_cannot_take_automated_input() {
        let hub = Hub::new(120, 40);
        hub.output(
            1,
            b"\x1b[?1049h\x1b[?25l\x1b[34;1HApproaching rate limits\r\nSwitch to gpt-6-luna",
        );
        assert_eq!(hub.codex_rate_limit_picker_shown(), Some(true));
        hub.resize(120, 24);
        assert_eq!(hub.codex_rate_limit_picker_shown(), None);

        hub.output(2, b"\x1b[H\x1b[J\x1b[18;1HApproaching rate limits");
        assert_eq!(hub.codex_rate_limit_picker_shown(), None);
        hub.output(3, b"\r\nSwitch to gpt-6-luna");
        assert_eq!(hub.codex_rate_limit_picker_shown(), Some(true));

        hub.output(
            4,
            "\x1b[H\x1b[J\x1b[18;1H› Ask Codex to do anything\x1b[?25h".as_bytes(),
        );
        assert_eq!(hub.codex_rate_limit_picker_shown(), Some(false));
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

    /// SPEC-623 review round 1: a program that never pauses its output costs the limit, no
    /// more, so the repaint probe's bound holds under continuous output.
    #[tokio::test]
    async fn settle_ends_at_its_limit_under_continuous_output() {
        let hub = Arc::new(Hub::new(20, 5));
        let since = hub.output_seq();
        let watch = hub.watch();
        let drawer = Arc::clone(&hub);
        // Bursts keep a backlog on the watch, so a frame is always ready.
        let flood = tokio::spawn(async move {
            let mut seq = 0;
            loop {
                for _ in 0..64 {
                    seq += 1;
                    drawer.output(seq, b"x");
                }
                tokio::time::sleep(SETTLE_QUIET / 4).await;
            }
        });
        let started = tokio::time::Instant::now();
        let settled = tokio::time::timeout(
            Duration::from_secs(5),
            hub.settle(watch, since, Duration::from_millis(300)),
        )
        .await;
        let waited = started.elapsed();
        assert!(settled.is_ok(), "settle outlived its limit: {waited:?}");
        flood.abort();
        assert!(waited >= Duration::from_millis(300), "{waited:?}");
        assert!(
            waited < Duration::from_millis(300) + SETTLE_QUIET * 4,
            "{waited:?}"
        );
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
