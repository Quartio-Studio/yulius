//! The connection to a virtual display's X server (SPEC-002 ADR-15): real input through XTest,
//! the screen size through RandR, and the browser's windows kept at the viewport.
//!
//! One OS thread per display owns the connection (x11rb's pure-Rust, blocking client). Callers
//! hand it [`DisplayInput`]s through a bounded queue and never wait for the X server; the thread
//! applies them in order, then the X events it watches, and ends when the queue's sender is
//! dropped or the server goes away. It tracks the keys and buttons it pressed so a viewer that
//! leaves never leaves one held ([`DisplayInput::ReleaseAll`]).
//!
//! There is no window manager. The thread plays the small part of one the browser needs: every
//! top-level window that maps (the browser's window, a popup, a dialog) is moved to the top
//! left at the viewport size and receives the keyboard focus, a press focuses the window under
//! the pointer, and a window closing hands the focus to the topmost remaining one. Menus and
//! tooltips (override-redirect windows) are left alone; they stay inside the screen because the
//! screen is the viewport (RandR).

use std::collections::{BTreeSet, HashMap};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, SyncSender, TrySendError};
use std::time::{Duration, Instant};

use tracing::{debug, warn};

use crate::display::DisplayInput;
use x11rb::connection::{Connection as _, RequestConnection as _};
use x11rb::protocol::Event;
use x11rb::protocol::randr::{ConnectionExt as _, ModeFlag, ModeInfo, Rotation};
use x11rb::protocol::xproto::{
    self, ChangeWindowAttributesAux, ConfigureWindowAux, ConnectionExt as _, EventMask, InputFocus,
    Window,
};
use x11rb::protocol::xtest::ConnectionExt as _;
use x11rb::rust_connection::{DefaultStream, RustConnection};

/// Directory of the X servers' filesystem sockets.
pub(crate) const X11_SOCKET_DIR: &str = "/tmp/.X11-unix";
/// Commands queued for the thread; input beyond it is refused, never buffered without bound.
pub(crate) const COMMAND_QUEUE: usize = 512;
/// How often the thread looks at X events when no command arrives.
const EVENT_POLL: Duration = Duration::from_millis(15);
/// Re-fits of one window per second at most (a page resizing its popup in a loop).
const MAX_REFITS_PER_SECOND: u32 = 10;
/// Authorization protocol of the cookie.
pub(crate) const COOKIE_PROTOCOL: &[u8] = b"MIT-MAGIC-COOKIE-1";

/// The socket path of display `number`.
pub(crate) fn socket_path(number: u32) -> PathBuf {
    Path::new(X11_SOCKET_DIR).join(format!("X{number}"))
}

/// Connects to display `number` with its cookie.
///
/// # Errors
/// When the socket is absent, refuses the connection or rejects the cookie.
pub(crate) fn connect(number: u32, cookie: &[u8]) -> Result<RustConnection, String> {
    let stream = UnixStream::connect(socket_path(number)).map_err(|error| error.to_string())?;
    let (stream, _) = DefaultStream::from_unix_stream(stream).map_err(|error| error.to_string())?;
    RustConnection::connect_to_stream_with_auth_info(
        stream,
        0,
        COOKIE_PROTOCOL.to_vec(),
        cookie.to_vec(),
    )
    .map_err(|error| error.to_string())
}

/// The X thread's handle. Dropping it ends the thread (after releasing held input).
#[derive(Debug)]
pub(crate) struct XController {
    tx: SyncSender<DisplayInput>,
    alive: Arc<AtomicBool>,
}

/// Why a command was not queued.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum XSendError {
    /// The queue is full (input arrives faster than the display takes it).
    Busy,
    /// The thread ended (the X server is gone).
    Gone,
}

impl XController {
    /// Takes over `conn` (connected to the display) on a new thread: watches the root window's
    /// children, requires XTest and sizes the screen to `size`.
    ///
    /// # Errors
    /// When XTest is missing or the root window cannot be watched.
    pub(crate) fn start(conn: RustConnection, size: (u16, u16)) -> Result<Self, String> {
        let root = conn
            .setup()
            .roots
            .first()
            .map(|screen| screen.root)
            .ok_or("no screen")?;
        let xtest = conn
            .extension_information(x11rb::protocol::xtest::X11_EXTENSION_NAME)
            .map_err(|error| error.to_string())?;
        if xtest.is_none() {
            return Err("the X server has no XTEST extension".into());
        }
        conn.change_window_attributes(
            root,
            &ChangeWindowAttributesAux::new().event_mask(EventMask::SUBSTRUCTURE_NOTIFY),
        )
        .map_err(|error| error.to_string())?
        .check()
        .map_err(|error| error.to_string())?;
        let (tx, rx) = std::sync::mpsc::sync_channel(COMMAND_QUEUE);
        let alive = Arc::new(AtomicBool::new(true));
        let thread_alive = Arc::clone(&alive);
        let screen = conn.setup().roots.first().map_or(size, |screen| {
            (screen.width_in_pixels, screen.height_in_pixels)
        });
        let mut state = XState {
            conn,
            root,
            screen,
            size,
            mapped: Vec::new(),
            keys: BTreeSet::new(),
            buttons: BTreeSet::new(),
            refits: HashMap::new(),
            mode: None,
        };
        state.resize(size.0, size.1);
        std::thread::Builder::new()
            .name("yulius-display".into())
            .spawn(move || {
                state.run(&rx);
                thread_alive.store(false, Ordering::Relaxed);
            })
            .map_err(|error| error.to_string())?;
        Ok(Self { tx, alive })
    }

    /// Queues a command without waiting.
    ///
    /// # Errors
    /// [`XSendError::Busy`] when the queue is full, [`XSendError::Gone`] when the thread ended.
    pub(crate) fn send(&self, command: DisplayInput) -> Result<(), XSendError> {
        if !self.alive.load(Ordering::Relaxed) {
            return Err(XSendError::Gone);
        }
        self.tx.try_send(command).map_err(|error| match error {
            TrySendError::Full(_) => XSendError::Busy,
            TrySendError::Disconnected(_) => XSendError::Gone,
        })
    }

    /// Whether the thread still runs (the X server answers).
    pub(crate) fn is_alive(&self) -> bool {
        self.alive.load(Ordering::Relaxed)
    }
}

struct XState {
    conn: RustConnection,
    root: Window,
    /// The screen's current size.
    screen: (u16, u16),
    /// The viewport: the size every top-level window gets.
    size: (u16, u16),
    /// Mapped top-level windows (not override-redirect), in map order.
    mapped: Vec<Window>,
    keys: BTreeSet<u8>,
    buttons: BTreeSet<u8>,
    refits: HashMap<Window, (Instant, u32)>,
    /// The RandR mode this thread created and uses.
    mode: Option<u32>,
}

impl XState {
    fn run(&mut self, rx: &Receiver<DisplayInput>) {
        loop {
            match rx.recv_timeout(EVENT_POLL) {
                Ok(command) => {
                    self.apply(command);
                    while let Ok(command) = rx.try_recv() {
                        self.apply(command);
                    }
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => {
                    self.release_all();
                    let _ = self.conn.flush();
                    return;
                }
            }
            loop {
                match self.conn.poll_for_event() {
                    Ok(Some(event)) => self.on_event(&event),
                    Ok(None) => break,
                    Err(error) => {
                        debug!(%error, "display connection closed");
                        return;
                    }
                }
            }
            if self.conn.flush().is_err() {
                return;
            }
        }
    }

    fn fake(&self, kind: u8, detail: u8, x: i16, y: i16) {
        if let Err(error) =
            self.conn
                .xtest_fake_input(kind, detail, x11rb::CURRENT_TIME, self.root, x, y, 0)
        {
            debug!(%error, "XTest input failed");
        }
    }

    fn apply(&mut self, command: DisplayInput) {
        match command {
            DisplayInput::Motion { x, y } => {
                let x = x.clamp(0, signed(self.size.0) - 1);
                let y = y.clamp(0, signed(self.size.1) - 1);
                self.fake(xproto::MOTION_NOTIFY_EVENT, 0, x, y);
            }
            DisplayInput::Button { button, press } => {
                if press {
                    self.focus_under_pointer();
                    self.buttons.insert(button);
                    self.fake(xproto::BUTTON_PRESS_EVENT, button, 0, 0);
                } else if self.buttons.remove(&button) {
                    self.fake(xproto::BUTTON_RELEASE_EVENT, button, 0, 0);
                }
            }
            DisplayInput::Key { keycode, press } => {
                if press {
                    self.keys.insert(keycode);
                    self.fake(xproto::KEY_PRESS_EVENT, keycode, 0, 0);
                } else if self.keys.remove(&keycode) {
                    self.fake(xproto::KEY_RELEASE_EVENT, keycode, 0, 0);
                }
            }
            DisplayInput::ReleaseAll => self.release_all(),
            DisplayInput::Resize { width, height } => self.resize(width, height),
        }
    }

    fn release_all(&mut self) {
        for keycode in std::mem::take(&mut self.keys) {
            self.fake(xproto::KEY_RELEASE_EVENT, keycode, 0, 0);
        }
        for button in std::mem::take(&mut self.buttons) {
            self.fake(xproto::BUTTON_RELEASE_EVENT, button, 0, 0);
        }
    }

    /// Click to focus: the top-level window under the pointer gets the keyboard.
    fn focus_under_pointer(&self) {
        let child = self
            .conn
            .query_pointer(self.root)
            .ok()
            .and_then(|cookie| cookie.reply().ok())
            .map(|reply| reply.child);
        if let Some(child) = child.filter(|child| *child != x11rb::NONE) {
            self.focus(child);
        }
    }

    fn focus(&self, window: Window) {
        if let Err(error) =
            self.conn
                .set_input_focus(InputFocus::PARENT, window, x11rb::CURRENT_TIME)
        {
            debug!(%error, "focusing a window failed");
        }
    }

    fn fit(&mut self, window: Window) {
        let (width, height) = self.size;
        let aux = ConfigureWindowAux::new()
            .x(0)
            .y(0)
            .width(u32::from(width))
            .height(u32::from(height))
            .border_width(0);
        if let Err(error) = self.conn.configure_window(window, &aux) {
            debug!(%error, "fitting a window failed");
        }
    }

    fn on_event(&mut self, event: &Event) {
        match event {
            Event::MapNotify(map) if map.event == self.root && !map.override_redirect => {
                if !self.mapped.contains(&map.window) {
                    self.mapped.push(map.window);
                }
                self.fit(map.window);
                // A new window (the browser, a popup, a dialog) is on top: it takes the keys.
                self.focus(map.window);
            }
            Event::UnmapNotify(unmap) if unmap.event == self.root => {
                self.forget(unmap.window);
            }
            Event::DestroyNotify(destroy) if destroy.event == self.root => {
                self.forget(destroy.window);
            }
            Event::ConfigureNotify(configure)
                if configure.event == self.root
                    && !configure.override_redirect
                    && self.mapped.contains(&configure.window) =>
            {
                let fitted = configure.x == 0
                    && configure.y == 0
                    && (configure.width, configure.height) == self.size;
                if !fitted && self.may_refit(configure.window) {
                    self.fit(configure.window);
                }
            }
            _ => {}
        }
    }

    fn may_refit(&mut self, window: Window) -> bool {
        let now = Instant::now();
        let entry = self.refits.entry(window).or_insert((now, 0));
        if now.duration_since(entry.0) >= Duration::from_secs(1) {
            *entry = (now, 0);
        }
        entry.1 += 1;
        entry.1 <= MAX_REFITS_PER_SECOND
    }

    fn forget(&mut self, window: Window) {
        let was_mapped = self.mapped.contains(&window);
        self.mapped.retain(|other| *other != window);
        self.refits.remove(&window);
        if was_mapped && let Some(top) = self.topmost() {
            self.focus(top);
        }
    }

    /// The topmost mapped top-level window (stacking order from the server).
    fn topmost(&self) -> Option<Window> {
        let tree = self.conn.query_tree(self.root).ok()?.reply().ok()?;
        tree.children
            .iter()
            .rev()
            .find(|child| self.mapped.contains(child))
            .copied()
    }

    /// Sets the screen to `width`×`height` with a RandR mode of that size, so pages and the
    /// browser's menus see the viewport as the screen, then fits every window.
    fn resize(&mut self, width: u16, height: u16) {
        self.size = (width, height);
        if let Err(error) = self.set_screen(width, height) {
            warn!(%error, width, height, "resizing the display's screen failed; fitting windows only");
        }
        for window in self.mapped.clone() {
            self.fit(window);
        }
        let _ = self.conn.flush();
    }

    fn set_screen(&mut self, width: u16, height: u16) -> Result<(), String> {
        let text = |error: &dyn std::fmt::Display| error.to_string();
        let resources = self
            .conn
            .randr_get_screen_resources(self.root)
            .map_err(|e| text(&e))?
            .reply()
            .map_err(|e| text(&e))?;
        let (Some(&crtc), Some(&output)) = (resources.crtcs.first(), resources.outputs.first())
        else {
            return Err("no RandR output".into());
        };
        let existing = resources
            .modes
            .iter()
            .find(|mode| mode.width == width && mode.height == height)
            .map(|mode| mode.id);
        let mode = match existing {
            Some(mode) => mode,
            None => {
                let name = format!("yulius-{width}x{height}");
                let info = ModeInfo {
                    id: 0,
                    width,
                    height,
                    dot_clock: u32::from(width) * u32::from(height) * 60,
                    hsync_start: width,
                    hsync_end: width,
                    htotal: width,
                    hskew: 0,
                    vsync_start: height,
                    vsync_end: height,
                    vtotal: height,
                    name_len: u16::try_from(name.len()).unwrap_or(0),
                    mode_flags: ModeFlag::default(),
                };
                let mode = self
                    .conn
                    .randr_create_mode(self.root, info, name.as_bytes())
                    .map_err(|e| text(&e))?
                    .reply()
                    .map_err(|e| text(&e))?
                    .mode;
                self.conn
                    .randr_add_output_mode(output, mode)
                    .map_err(|e| text(&e))?
                    .check()
                    .map_err(|e| text(&e))?;
                mode
            }
        };
        let current = self.screen;
        let mm = |pixels: u16| (u32::from(pixels) * 254 / 960).max(1);
        // Grow the screen before the mode and shrink it after, as xrandr does.
        let (grow_w, grow_h) = (width.max(current.0), height.max(current.1));
        self.conn
            .randr_set_screen_size(self.root, grow_w, grow_h, mm(grow_w), mm(grow_h))
            .map_err(|e| text(&e))?
            .check()
            .map_err(|e| text(&e))?;
        self.conn
            .randr_set_crtc_config(
                crtc,
                x11rb::CURRENT_TIME,
                resources.config_timestamp,
                0,
                0,
                mode,
                Rotation::ROTATE0,
                &[output],
            )
            .map_err(|e| text(&e))?
            .reply()
            .map_err(|e| text(&e))?;
        self.conn
            .randr_set_screen_size(self.root, width, height, mm(width), mm(height))
            .map_err(|e| text(&e))?
            .check()
            .map_err(|e| text(&e))?;
        self.screen = (width, height);
        // The previous size's mode is no longer needed (modes stay bounded).
        if existing.is_none()
            && let Some(old) = self.mode.replace(mode)
            && old != mode
        {
            let _ = self.conn.randr_delete_output_mode(output, old);
            let _ = self.conn.randr_destroy_mode(old);
        }
        Ok(())
    }
}

fn signed(value: u16) -> i16 {
    i16::try_from(value).unwrap_or(i16::MAX)
}
