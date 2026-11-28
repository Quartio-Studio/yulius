//! A sandbox browser's virtual display (SPEC-002 ADR-15).
//!
//! Yulius starts one `Xvfb` per running sandbox browser, as the service user, and runs a
//! headful Chrome on it. Users see the display as H.264 video ([`crate::VideoHub`]) and drive it
//! with real input through XTest ([`crate::xserver`]); agents keep CDP.
//!
//! Isolation (SPEC-002 §8):
//!
//! - **No network or abstract socket.** `-nolisten tcp -nolisten local`: the only listener is
//!   the filesystem socket `/tmp/.X11-unix/X<n>`. Contained processes have a private `/tmp`, and
//!   an abstract socket would be reachable from them because they share the network namespace.
//! - **A cookie per display.** `-auth` with a random 16-byte MIT-MAGIC-COOKIE-1 in
//!   `<runtime root>/<sandbox>/Xauthority` (0600 in a 0700 directory) — outside every sandbox
//!   directory, which containment binds into the sandbox. Without the cookie the server refuses
//!   a connection even if its socket were reachable.
//! - **Per sandbox.** Each sandbox's browser has its own display number, cookie, profile and
//!   encoder; nothing is shared between displays.
//!
//! Sizes: the viewport is in CSS pixels; the display, the windows and the video are in device
//! pixels (CSS × the configured scale, rounded down to even numbers for 4:2:0 video). The
//! framebuffer is allocated at the configured maximum and RandR sets the screen to the viewport.

use std::ffi::OsString;
use std::os::unix::ffi::OsStrExt as _;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use tracing::{info, warn};
use yulius_contracts::browser::MouseButton;
use yulius_domain::ids::SandboxId;

use crate::encoder::FfmpegStart;
use crate::error::BrowserError;
use crate::process::{NotReaped, StopTimeouts, Supervised, passed_env};
use crate::video::{VideoHub, VideoSubscription, VideoUnavailable};
use crate::xserver::{self, XController, XSendError};

/// Name of the cookie file in a display's runtime directory.
pub const XAUTHORITY_FILE: &str = "Xauthority";
/// Poll interval while waiting for a new X server to accept connections.
const READY_POLL: Duration = Duration::from_millis(50);
/// How long the display's video may take to stop.
const VIDEO_STOP: Duration = Duration::from_secs(10);

/// One input event for a display, in device pixels (also what the fake display records).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DisplayInput {
    /// Move the pointer to an absolute position.
    Motion {
        /// Horizontal position.
        x: i16,
        /// Vertical position.
        y: i16,
    },
    /// Press or release a pointer button (1 left, 2 middle, 3 right, 8 back, 9 forward).
    Button {
        /// X button number.
        button: u8,
        /// Pressed (or released).
        press: bool,
    },
    /// Press or release a key.
    Key {
        /// X keycode.
        keycode: u8,
        /// Pressed (or released).
        press: bool,
    },
    /// Type a character as a real key press and release (a US-layout character, Return and Tab
    /// by their keys, any other through a spare keycode mapped to its keysym).
    Type(char),
    /// Release every pressed key and button.
    ReleaseAll,
    /// Resize the screen and the browser's windows.
    Resize {
        /// Width.
        width: u16,
        /// Height.
        height: u16,
    },
}

/// The longest a queued paste's outcome is waited for (a display thread that stopped answering).
const PASTE_ANSWER_MAX: Duration = Duration::from_secs(15);

/// How a queued paste ends ([`VirtualDisplay::queue_paste`]).
pub(crate) type PasteWait =
    std::pin::Pin<Box<dyn std::future::Future<Output = Result<PasteOutcome, BrowserError>> + Send>>;

/// How a paste ended on the display (SPEC-639).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PasteOutcome {
    /// The text became the CLIPBOARD, the paste key was pressed and a client (the page) asked for it.
    Read,
    /// The text became the CLIPBOARD and the paste key was pressed, but no client asked for it.
    Unread,
    /// The CLIPBOARD could not be taken: the text was typed as real key presses instead.
    Typed,
}

impl DisplayInput {
    /// A new press or typed character supersedes the pending clipboard offer.
    pub(crate) fn supersedes_copy(self) -> bool {
        matches!(
            self,
            Self::Button { press: true, .. } | Self::Key { press: true, .. } | Self::Type(_)
        )
    }
}

/// Settings of the virtual displays.
#[derive(Debug, Clone, PartialEq)]
pub struct DisplayConfig {
    /// The `Xvfb` executable (a path, or a name searched on `PATH`).
    pub xvfb: PathBuf,
    /// The `ffmpeg` executable (needs libx264 and x11grab).
    pub ffmpeg: PathBuf,
    /// Root of the displays' runtime directories (cookies), outside every sandbox directory.
    pub runtime_root: PathBuf,
    /// First display number tried; numbers in use (a socket or lock file exists) are skipped.
    pub first_display: u32,
    /// Display numbers tried from `first_display`.
    pub display_count: u32,
    /// Largest viewport in CSS pixels; larger panels show the page scaled down.
    pub max_size: (u32, u32),
    /// Device pixels per CSS pixel (Chrome's `--force-device-scale-factor`): 2 makes text crisp
    /// on HiDPI screens at about three times the encoding cost.
    pub scale: f64,
    /// Highest frames per second of the video (the stream's tier may use fewer).
    pub fps: u32,
    /// Time between periodic keyframes; `None` (the default): keyframes only when the encoder
    /// starts, which the video hub does when a viewer needs one (F 2026-10-10).
    pub keyframe_interval: Option<Duration>,
    /// Maximum wait for the X server.
    pub startup_timeout: Duration,
}

impl DisplayConfig {
    /// Defaults: `Xvfb` and `ffmpeg` from `PATH`, displays from :100, at most 1920×1200 CSS
    /// pixels, scale 1, up to 60 frames per second, keyframes on demand, 10 s startup.
    #[must_use]
    pub fn new(runtime_root: PathBuf) -> Self {
        Self {
            xvfb: "Xvfb".into(),
            ffmpeg: "ffmpeg".into(),
            runtime_root,
            first_display: 100,
            display_count: 100,
            max_size: (1920, 1200),
            scale: 1.0,
            fps: 60,
            keyframe_interval: None,
            startup_timeout: Duration::from_secs(10),
        }
    }

    /// Device pixels of a CSS size: scaled, clamped to the maximum, even.
    #[must_use]
    pub fn device_size(&self, (width, height): (u32, u32)) -> (u32, u32) {
        let scale = sane_scale(self.scale);
        let (max_w, max_h) = self.max_size;
        let device = |css: u32, max: u32| {
            let pixels = (f64::from(css.clamp(64, max.max(64))) * scale).floor();
            // Saturating float-to-int conversion; the clamp above bounds it anyway.
            let pixels = pixels as u32;
            (pixels & !1).max(64)
        };
        (device(width, max_w), device(height, max_h))
    }

    /// The CSS size a device size shows (the inverse of [`Self::device_size`], rounded).
    #[must_use]
    pub fn css_size(&self, (width, height): (u32, u32)) -> (u32, u32) {
        let scale = sane_scale(self.scale);
        let css = |device: u32| (f64::from(device) / scale).round() as u32;
        (css(width), css(height))
    }

    /// Chrome's arguments for a display of this configuration.
    #[must_use]
    pub fn chromium_args(&self, device: (u32, u32)) -> Vec<OsString> {
        let mut args: Vec<OsString> = vec![
            // No browser UI: the panel draws tabs and the address bar from CDP.
            "--kiosk".into(),
            // A pre-policy profile may already contain Docs Offline. Loading it would
            // show a native administrator dialog when the host's blocklist arrives.
            "--disable-extensions".into(),
            "--window-position=0,0".into(),
            format!("--window-size={},{}", device.0, device.1).into(),
            // Remote debugging makes `navigator.webdriver` true; captchas read it.
            "--disable-blink-features=AutomationControlled".into(),
            "--noerrdialogs".into(),
            "--disable-session-crashed-bubble".into(),
        ];
        let scale = sane_scale(self.scale);
        if (scale - 1.0).abs() > f64::EPSILON {
            args.push(format!("--force-device-scale-factor={scale}").into());
        }
        args
    }
}

fn sane_scale(scale: f64) -> f64 {
    if scale.is_finite() {
        scale.clamp(1.0, 3.0)
    } else {
        1.0
    }
}

/// How sandbox browsers render.
#[derive(Debug, Clone)]
pub enum DisplayMode {
    /// `--headless=new`; viewers get JPEG screencast frames and CDP input (development hosts and
    /// CI without Xvfb or ffmpeg).
    Headless,
    /// A headful Chrome on a virtual display: H.264 video and XTest input.
    Virtual(DisplayConfig),
    /// A fake display for tests: records input, streams synthetic frames.
    #[cfg(feature = "fake")]
    Fake(crate::fake::FakeDisplay),
}

impl DisplayMode {
    /// Whether browsers get a display.
    #[must_use]
    pub fn is_virtual(&self) -> bool {
        !matches!(self, Self::Headless)
    }
}

enum Backend {
    Xvfb {
        process: Supervised,
        x: XController,
        number: u32,
    },
    #[cfg(feature = "fake")]
    Fake(crate::fake::FakeDisplay),
}

/// A running virtual display with its input and video.
pub struct VirtualDisplay {
    config: DisplayConfig,
    dir: Option<PathBuf>,
    xauthority: Option<PathBuf>,
    backend: Backend,
    video: VideoHub,
    /// Device pixels.
    size: Mutex<(u32, u32)>,
}

impl std::fmt::Debug for VirtualDisplay {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VirtualDisplay")
            .field("name", &self.name())
            .field("size", &*lock(&self.size))
            .finish_non_exhaustive()
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

fn display_error(detail: impl Into<String>) -> BrowserError {
    BrowserError::Display(detail.into())
}

fn input_error(error: XSendError) -> BrowserError {
    match error {
        XSendError::Busy => BrowserError::InputBacklog,
        XSendError::Gone => display_error("the display stopped"),
    }
}

/// Creates `dir` (and missing parents) owner-only.
fn private_dir(dir: &Path) -> Result<(), BrowserError> {
    use std::os::unix::fs::{DirBuilderExt as _, PermissionsExt as _};
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)
        .map_err(|error| display_error(format!("cannot create {}: {error}", dir.display())))?;
    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))
        .map_err(|error| display_error(format!("cannot protect {}: {error}", dir.display())))
}

/// An Xauthority file with one entry for every display (family Wild, empty address and
/// number): `MIT-MAGIC-COOKIE-1` and `cookie`. Each display has its own file.
#[must_use]
pub fn xauthority_entry(cookie: &[u8]) -> Vec<u8> {
    let mut entry = Vec::with_capacity(64);
    entry.extend_from_slice(&0xFFFF_u16.to_be_bytes()); // FamilyWild
    for field in [&b""[..], &b""[..], xserver::COOKIE_PROTOCOL, cookie] {
        entry.extend_from_slice(&u16::try_from(field.len()).unwrap_or(0).to_be_bytes());
        entry.extend_from_slice(field);
    }
    entry
}

/// Writes the cookie file (0600, replacing an old one).
fn write_cookie(path: &Path, cookie: &[u8]) -> Result<(), BrowserError> {
    use std::io::Write as _;
    use std::os::unix::fs::OpenOptionsExt as _;
    match std::fs::remove_file(path) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(display_error(format!("cannot replace the cookie: {error}"))),
    }
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .map_err(|error| display_error(format!("cannot write the cookie: {error}")))?;
    file.write_all(&xauthority_entry(cookie))
        .and_then(|()| file.sync_all())
        .map_err(|error| display_error(format!("cannot write the cookie: {error}")))
}

/// Whether display `number` is taken: its lock file names a live process, or (without a lock)
/// something answers on its socket. A stale lock or socket left by a killed server is not taken:
/// Xvfb removes them itself when it starts on that number.
fn display_taken(number: u32) -> bool {
    match std::fs::read_to_string(format!("/tmp/.X{number}-lock")) {
        Ok(text) => text.trim().parse::<i32>().ok().is_none_or(process_alive),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let socket = xserver::socket_path(number);
            socket.exists() && std::os::unix::net::UnixStream::connect(&socket).is_ok()
        }
        Err(_) => true,
    }
}

fn process_alive(pid: i32) -> bool {
    use nix::errno::Errno;
    match nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid), None) {
        Ok(()) | Err(Errno::EPERM) => true,
        Err(_) => false,
    }
}

/// Stops X servers a previous Yulius process started and left behind (it was killed before it
/// could stop them): every `Xvfb` whose `-auth` cookie lies under `runtime_root`. Their browser
/// and encoder exit with them. Returns how many were stopped. Linux only (reads `/proc`).
pub fn stop_orphaned_displays(runtime_root: &Path) -> usize {
    let Ok(entries) = std::fs::read_dir("/proc") else {
        return 0;
    };
    let mut stopped = 0;
    for entry in entries.flatten() {
        let Some(pid) = entry
            .file_name()
            .to_str()
            .and_then(|n| n.parse::<i32>().ok())
        else {
            continue;
        };
        let Ok(cmdline) = std::fs::read(entry.path().join("cmdline")) else {
            continue;
        };
        let args: Vec<&[u8]> = cmdline.split(|b| *b == 0).collect();
        let xvfb = args
            .first()
            .is_some_and(|program| program.ends_with(b"Xvfb"));
        let ours = args.windows(2).any(|pair| {
            pair[0] == b"-auth"
                && Path::new(std::ffi::OsStr::from_bytes(pair[1])).starts_with(runtime_root)
        });
        if xvfb && ours {
            let target = nix::unistd::Pid::from_raw(pid);
            let _ = nix::sys::signal::kill(target, nix::sys::signal::Signal::SIGTERM);
            for _ in 0..40 {
                if !process_alive(pid) {
                    break;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            if process_alive(pid) {
                let _ = nix::sys::signal::kill(target, nix::sys::signal::Signal::SIGKILL);
            }
            warn!(pid, "stopped a virtual display left by a previous run");
            stopped += 1;
        }
    }
    stopped
}

/// A display start that failed ([`VirtualDisplay::start`]): why, and the X server it started
/// when that could not be stopped either (SPEC-616).
#[derive(Debug)]
pub(crate) struct DisplayStartFailure {
    /// What the caller reports.
    pub(crate) error: BrowserError,
    /// The X server that survived its stop, still owned: the caller stops it again before the
    /// sandbox gets another display.
    pub(crate) stranded: Option<Box<StrandedXvfb>>,
}

impl From<BrowserError> for DisplayStartFailure {
    fn from(error: BrowserError) -> Self {
        Self {
            error,
            stranded: None,
        }
    }
}

impl std::fmt::Display for DisplayStartFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.error.fmt(f)
    }
}

impl std::error::Error for DisplayStartFailure {}

/// An Xvfb whose display failed to start and that then survived SIGKILL (SPEC-616 XS-001): its
/// process handle, cookie and runtime directory are kept until a later stop reaps it.
#[derive(Debug)]
pub(crate) struct StrandedXvfb {
    process: Supervised,
    number: u32,
    dir: PathBuf,
    xauthority: PathBuf,
    /// Why the display did not start.
    cause: String,
}

impl StrandedXvfb {
    fn error(&self) -> BrowserError {
        BrowserError::DisplayNotStopped {
            display: format!(":{}", self.number),
            pid: self.process.pid(),
            cause: self.cause.clone(),
        }
    }

    /// Stops the X server again; once it is reaped, removes its cookie and runtime directory.
    ///
    /// # Errors
    /// [`BrowserError::DisplayNotStopped`] while the server survives SIGKILL.
    pub(crate) async fn stop(&self, timeouts: StopTimeouts) -> Result<(), BrowserError> {
        let timeouts = StopTimeouts {
            graceful: Duration::ZERO,
            ..timeouts
        };
        stop_xvfb(&self.process, &self.dir, timeouts)
            .await
            .map_err(|NotReaped| self.error())?;
        let _ = std::fs::remove_file(&self.xauthority);
        let _ = std::fs::remove_dir(&self.dir);
        info!(display = self.number, "stranded virtual display stopped");
        Ok(())
    }
}

/// Where a display start that was dropped (its request cancelled) leaves the X server it had
/// started, with its cookie and runtime directory, for its sandbox's slot to stop before the next
/// display (#1688).
pub(crate) type CancelledStart = Arc<Mutex<Option<Box<StrandedXvfb>>>>;

/// The X server of a display being started, owned from its spawn on: dropped before the start
/// handed it on (the start's future was dropped, during readiness or a failed start's stop), it
/// goes to the sandbox's [`CancelledStart`] instead of being killed and forgotten (#1688).
struct StartGuard<'a> {
    xvfb: Option<Box<StrandedXvfb>>,
    sink: &'a CancelledStart,
}

impl StartGuard<'_> {
    /// The X server, while the guard still owns it.
    fn process(&self) -> Option<&Supervised> {
        self.xvfb.as_ref().map(|xvfb| &xvfb.process)
    }
}

impl Drop for StartGuard<'_> {
    fn drop(&mut self) {
        let Some(xvfb) = self.xvfb.take() else {
            return;
        };
        let mut slot = lock(self.sink);
        warn!(
            display = xvfb.number,
            pid = xvfb.process.pid(),
            "a display start was cancelled; its X server is kept to be stopped"
        );
        // One start runs per slot at a time; an older server left there is kept first.
        if slot.is_none() {
            *slot = Some(xvfb);
        }
    }
}

/// Stops a display's X server (the one place a test can make it survive, [`stop_faults`]).
async fn stop_xvfb(
    process: &Supervised,
    dir: &Path,
    timeouts: StopTimeouts,
) -> Result<(), NotReaped> {
    #[cfg(test)]
    stop_faults::entered(dir);
    #[cfg(test)]
    if stop_faults::held(dir) {
        return Err(NotReaped);
    }
    #[cfg(not(test))]
    let _ = dir;
    process.terminate(timeouts).await.map(|_| ())
}

/// Stops the X server of a display that failed to start with `error` and removes its cookie;
/// keeps the server, cookie and directory when it survives (SPEC-616 XS-001, XS-004).
async fn abandon(mut guard: StartGuard<'_>, error: BrowserError) -> DisplayStartFailure {
    // The guard owns the server through the stop: a start dropped meanwhile keeps it (#1688).
    let stopped = match guard.xvfb.as_deref() {
        Some(xvfb) => stop_xvfb(&xvfb.process, &xvfb.dir, quick_stop())
            .await
            .is_ok(),
        None => true,
    };
    let Some(mut stranded) = guard.xvfb.take() else {
        return error.into();
    };
    if stopped {
        let _ = std::fs::remove_file(&stranded.xauthority);
        return error.into();
    }
    stranded.cause = error.to_string();
    let error = stranded.error();
    warn!(%error, "a virtual display that failed to start could not be stopped");
    DisplayStartFailure {
        error,
        stranded: Some(stranded),
    }
}

/// Test-only fault injection: a display's X server that survives SIGKILL (a process in
/// uninterruptible sleep), which an unprivileged test cannot produce. While a display's runtime
/// directory is held, [`stop_xvfb`] reports its server not reaped and sends no signal, so the
/// server really keeps running.
#[cfg(test)]
pub(crate) mod stop_faults {
    use std::path::{Path, PathBuf};
    use std::sync::Mutex;

    static HELD: Mutex<Vec<PathBuf>> = Mutex::new(Vec::new());

    fn held_dirs() -> std::sync::MutexGuard<'static, Vec<PathBuf>> {
        HELD.lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Makes the stops of the display in `dir` fail.
    pub(crate) fn hold(dir: &Path) {
        held_dirs().push(dir.to_owned());
    }

    /// Lets them succeed again.
    pub(crate) fn release(dir: &Path) {
        held_dirs().retain(|held| held != dir);
    }

    pub(super) fn held(dir: &Path) -> bool {
        held_dirs().iter().any(|held| held == dir)
    }

    static ENTERED: Mutex<Vec<PathBuf>> = Mutex::new(Vec::new());

    /// A stop of the display in `dir` began.
    pub(super) fn entered(dir: &Path) {
        ENTERED
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(dir.to_owned());
    }

    /// Whether a stop of the display in `dir` began.
    pub(crate) fn stop_began(dir: &Path) -> bool {
        ENTERED
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .iter()
            .any(|entered| entered == dir)
    }

    /// An `Xvfb` stand-in that records each start's pid in `starts` and acts as the `mode`
    /// file says: `silent` never accepts connections (`stubborn` also ignores SIGTERM, so its stop
    /// waits for the kill), `no-xtest` runs the real Xvfb without
    /// XTEST (the X controller's setup fails), `real` runs the real Xvfb.
    pub(crate) struct FakeXvfb {
        pub(crate) program: PathBuf,
        dir: PathBuf,
    }

    impl FakeXvfb {
        /// Writes the script into `dir`, starting in `mode`; `real` is the real Xvfb, if any.
        pub(crate) fn new(dir: &Path, real: Option<&Path>, mode: &str) -> std::io::Result<Self> {
            use std::os::unix::fs::PermissionsExt as _;
            let real = real.map_or_else(|| "/nonexistent/Xvfb".into(), |p| p.display().to_string());
            let dir = dir.to_owned();
            let program = dir.join("Xvfb");
            let script = format!(
                "#!/bin/sh\necho $$ >> '{d}/starts'\ncase \"$(cat '{d}/mode')\" in\n  silent) exec /bin/sleep 600 ;;\n  stubborn) trap '' TERM; exec /bin/sleep 600 ;;\n  no-xtest) exec '{real}' \"$@\" -tst ;;\n  *) exec '{real}' \"$@\" ;;\nesac\n",
                d = dir.display()
            );
            std::fs::write(&program, script)?;
            std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755))?;
            let fake = Self { program, dir };
            fake.set_mode(mode)?;
            Ok(fake)
        }

        pub(crate) fn set_mode(&self, mode: &str) -> std::io::Result<()> {
            std::fs::write(self.dir.join("mode"), mode)
        }

        /// The pids of the servers started so far.
        pub(crate) fn starts(&self) -> Vec<i32> {
            std::fs::read_to_string(self.dir.join("starts"))
                .unwrap_or_default()
                .lines()
                .filter_map(|line| line.trim().parse().ok())
                .collect()
        }
    }

    /// Display numbers of their own for one test (`slot` tells a process's tests apart): above
    /// the default `:100..:199` other tests use, and per process, so that no other X server
    /// starts on them meanwhile and every start of the test is its own.
    pub(crate) fn private_displays(config: &mut super::DisplayConfig, slot: u32) {
        config.first_display = 1000 + (std::process::id() % 4000) * 8 + slot * 4;
        config.display_count = 4;
    }

    /// Whether `pid` is a live process (a zombie counts until reaped).
    pub(crate) fn running(pid: i32) -> bool {
        super::process_alive(pid)
    }
}

impl VirtualDisplay {
    /// Starts a display for `sandbox` sized for `css` (CSS pixels).
    ///
    /// # Errors
    /// [`BrowserError::Display`] when Xvfb is missing, no display number is free, the server
    /// does not come up or lacks XTest. A failed start leaves no process behind, except an X
    /// server that survived its stop: that one is returned in the failure, with
    /// [`BrowserError::DisplayNotStopped`], for the caller to stop again (SPEC-616).
    #[cfg(test)]
    pub(crate) async fn start(
        mode: &DisplayMode,
        sandbox: &SandboxId,
        css: (u32, u32),
    ) -> Result<Option<Self>, DisplayStartFailure> {
        Self::start_into(mode, sandbox, css, &CancelledStart::default()).await
    }

    /// As [`Self::start`]; a start dropped before it ended leaves its X server in `cancelled`
    /// (#1688).
    pub(crate) async fn start_into(
        mode: &DisplayMode,
        sandbox: &SandboxId,
        css: (u32, u32),
        cancelled: &CancelledStart,
    ) -> Result<Option<Self>, DisplayStartFailure> {
        match mode {
            DisplayMode::Headless => Ok(None),
            DisplayMode::Virtual(config) => Self::start_xvfb(config, sandbox, css, cancelled)
                .await
                .map(Some),
            #[cfg(feature = "fake")]
            DisplayMode::Fake(fake) => Ok(Some(Self::start_fake(fake, css))),
        }
    }

    #[cfg(feature = "fake")]
    fn start_fake(fake: &crate::fake::FakeDisplay, css: (u32, u32)) -> Self {
        let config = fake.config();
        let size = config.device_size(css);
        fake.started();
        Self {
            video: VideoHub::new(fake.encoder(), size),
            backend: Backend::Fake(fake.clone()),
            dir: None,
            xauthority: None,
            size: Mutex::new(size),
            config,
        }
    }

    async fn start_xvfb(
        config: &DisplayConfig,
        sandbox: &SandboxId,
        css: (u32, u32),
        cancelled: &CancelledStart,
    ) -> Result<Self, DisplayStartFailure> {
        use rand::RngExt as _;
        let xvfb = crate::discovery::resolve_executable(&config.xvfb)
            .map_err(|_| display_error(format!("{} not found", config.xvfb.display())))?;
        private_dir(&config.runtime_root)?;
        let dir = config.runtime_root.join(sandbox.as_str());
        private_dir(&dir)?;
        let xauthority = dir.join(XAUTHORITY_FILE);
        let mut cookie = [0_u8; 16];
        rand::rng().fill(&mut cookie[..]);
        write_cookie(&xauthority, &cookie)?;
        let max = config.device_size(config.max_size);
        let size = config.device_size(css);
        let last = config.first_display.saturating_add(config.display_count);
        for number in config.first_display..last {
            if display_taken(number) {
                continue;
            }
            let args: Vec<OsString> = vec![
                format!(":{number}").into(),
                "-screen".into(),
                "0".into(),
                format!("{}x{}x24", max.0, max.1).into(),
                "-nolisten".into(),
                "tcp".into(),
                "-nolisten".into(),
                "local".into(),
                "-auth".into(),
                xauthority.clone().into_os_string(),
                "-noreset".into(),
                "-dpi".into(),
                "96".into(),
            ];
            let process =
                Supervised::spawn(&xvfb, &args, &passed_env()).map_err(BrowserError::Spawn)?;
            let mut guard = StartGuard {
                xvfb: Some(Box::new(StrandedXvfb {
                    process,
                    number,
                    dir: dir.clone(),
                    xauthority: xauthority.clone(),
                    cause: "its start was cancelled".into(),
                })),
                sink: cancelled,
            };
            let ready = match guard.process() {
                Some(process) => wait_ready(process, number, &cookie, config.startup_timeout).await,
                None => Ready::Failed(display_error("the X server was let go")),
            };
            match ready {
                Ready::Up(conn) => {
                    let width = u16::try_from(size.0).unwrap_or(u16::MAX);
                    let height = u16::try_from(size.1).unwrap_or(u16::MAX);
                    let started = tokio::task::spawn_blocking(move || {
                        XController::start(*conn, (width, height))
                    })
                    .await
                    .map_err(|error| error.to_string())
                    .and_then(|result| result);
                    let x = match started {
                        Ok(x) => x,
                        Err(problem) => {
                            let error = display_error(problem);
                            return Err(abandon(guard, error).await);
                        }
                    };
                    let Some(owned) = guard.xvfb.take() else {
                        return Err(display_error("the X server was let go").into());
                    };
                    let process = owned.process;
                    info!(sandbox = %sandbox, display = number, pid = process.pid(), "virtual display started");
                    let encoder = FfmpegStart {
                        program: config.ffmpeg.clone(),
                        display: format!(":{number}"),
                        xauthority: xauthority.clone(),
                        fps: config.fps.clamp(5, 60),
                        keyframe_seconds: config.keyframe_interval.map(|i| i.as_secs_f64()),
                    };
                    return Ok(Self {
                        video: VideoHub::new(encoder, size),
                        backend: Backend::Xvfb { process, x, number },
                        dir: Some(dir),
                        xauthority: Some(xauthority),
                        size: Mutex::new(size),
                        config: config.clone(),
                    });
                }
                Ready::Taken => {
                    // The server already exited and was reaped (that is how a taken number
                    // shows): this stop only clears its process group and cannot fail.
                    if let Some(process) = guard.process() {
                        let _ = process.terminate(quick_stop()).await;
                    }
                    drop(guard.xvfb.take());
                }
                Ready::Failed(error) => {
                    return Err(abandon(guard, error).await);
                }
            }
        }
        let _ = std::fs::remove_file(&xauthority);
        Err(DisplayStartFailure::from(display_error(format!(
            "no free display number in :{}..:{}",
            config.first_display,
            last.saturating_sub(1)
        ))))
    }

    /// The display name, e.g. `:101` (`fake` for the test double).
    #[must_use]
    pub fn name(&self) -> String {
        match &self.backend {
            Backend::Xvfb { number, .. } => format!(":{number}"),
            #[cfg(feature = "fake")]
            Backend::Fake(_) => "fake".into(),
        }
    }

    /// The environment Chrome needs to use the display.
    #[must_use]
    pub fn env(&self) -> Vec<(OsString, OsString)> {
        let mut env = vec![(OsString::from("DISPLAY"), OsString::from(self.name()))];
        if let Some(path) = &self.xauthority {
            env.push(("XAUTHORITY".into(), path.clone().into_os_string()));
        }
        env
    }

    /// The configuration.
    #[must_use]
    pub fn config(&self) -> &DisplayConfig {
        &self.config
    }

    /// The display size in device pixels.
    #[must_use]
    pub fn size(&self) -> (u32, u32) {
        *lock(&self.size)
    }

    /// Whether the X server still answers.
    #[must_use]
    pub fn is_alive(&self) -> bool {
        match &self.backend {
            Backend::Xvfb { process, x, .. } => !process.has_exited() && x.is_alive(),
            #[cfg(feature = "fake")]
            Backend::Fake(_) => true,
        }
    }

    /// The video.
    #[must_use]
    pub fn video(&self) -> &VideoHub {
        &self.video
    }

    /// Subscribes to the video.
    ///
    /// # Errors
    /// [`VideoUnavailable`] when the encoder failed for good.
    pub fn subscribe_video(&self) -> Result<VideoSubscription, VideoUnavailable> {
        self.video.subscribe()
    }

    fn send(&self, input: DisplayInput) -> Result<(), BrowserError> {
        match &self.backend {
            Backend::Xvfb { x, .. } => x.send(input).map_err(input_error),
            #[cfg(feature = "fake")]
            Backend::Fake(fake) => {
                fake.record(input);
                Ok(())
            }
        }
    }

    /// Resizes the display and the browser's windows for a viewport of `css` CSS pixels and
    /// restarts the video at the new size. Returns the CSS size applied (after clamping).
    ///
    /// # Errors
    /// When the display stopped.
    pub fn resize(&self, css: (u32, u32)) -> Result<(u32, u32), BrowserError> {
        let size = self.config.device_size(css);
        let changed = {
            let mut current = lock(&self.size);
            let changed = *current != size;
            *current = size;
            changed
        };
        if changed {
            self.send(DisplayInput::Resize {
                width: u16::try_from(size.0).unwrap_or(u16::MAX),
                height: u16::try_from(size.1).unwrap_or(u16::MAX),
            })?;
            self.video.set_size(size);
        }
        Ok(self.config.css_size(size))
    }

    /// Moves the pointer to `(x, y)` CSS pixels of the viewport. Moves are coalesced: only the
    /// newest position not yet applied is kept, so this never fails for load.
    ///
    /// # Errors
    /// When the display stopped.
    pub fn pointer(&self, x: f64, y: f64) -> Result<(), BrowserError> {
        let scale = sane_scale(self.config.scale);
        let (width, height) = self.size();
        let device = |css: f64, max: u32| {
            let pixels = (css * scale)
                .round()
                .clamp(0.0, f64::from(max.saturating_sub(1)));
            i16::try_from(pixels as i64).unwrap_or(i16::MAX)
        };
        self.send(DisplayInput::Motion {
            x: device(x, width),
            y: device(y, height),
        })
    }

    pub(crate) async fn accept_copy(&self, offer: u32) -> Option<String> {
        match &self.backend {
            Backend::Xvfb { x, .. } => {
                let (reply, text) = tokio::sync::oneshot::channel();
                x.queue(xserver::XCommand::AcceptCopy(offer, reply)).ok()?;
                tokio::time::timeout(Duration::from_secs(3), text)
                    .await
                    .ok()?
                    .ok()
            }
            #[cfg(feature = "fake")]
            Backend::Fake(fake) => fake.accept_copy(offer),
        }
    }

    /// Queues `text` as a person's paste: it becomes the display's CLIPBOARD and Shift+Insert is
    /// pressed, in order with the input queued before and after it; typed as key presses when
    /// the CLIPBOARD cannot be taken. The returned future says how it ended, once a client
    /// fetched the text or at most the paste window after it (and the wait for the page to stop
    /// reading the paste before).
    ///
    /// # Errors
    /// When the input queue is full or the display stopped.
    pub(crate) fn queue_paste(
        &self,
        text: zeroize::Zeroizing<String>,
    ) -> Result<PasteWait, BrowserError> {
        match &self.backend {
            Backend::Xvfb { x, .. } => {
                let (outcome, answer) = tokio::sync::oneshot::channel();
                x.queue(xserver::XCommand::Paste(text, outcome))
                    .map_err(input_error)?;
                // The X thread answers each paste by its own window's end, counted from its
                // paste key (pastes queued before it may wait first); this bound is only for a
                // thread that stopped answering.
                let bound = PASTE_ANSWER_MAX;
                Ok(Box::pin(async move {
                    match tokio::time::timeout(bound, answer).await {
                        Ok(Ok(outcome)) => Ok(outcome),
                        Ok(Err(_)) => Err(display_error("the display stopped")),
                        Err(_) => Ok(PasteOutcome::Unread),
                    }
                }))
            }
            #[cfg(feature = "fake")]
            Backend::Fake(fake) => Ok(fake.paste(&text)),
        }
    }

    /// [`Self::queue_paste`] and its outcome (the Linux display tests).
    #[cfg(all(test, target_os = "linux"))]
    pub(crate) async fn paste(
        &self,
        text: zeroize::Zeroizing<String>,
    ) -> Result<PasteOutcome, BrowserError> {
        self.queue_paste(text)?.await
    }

    /// Queues a primary press together with a one-shot recipient for a fresh clipboard write.
    pub(crate) fn copy_press(
        &self,
        result: tokio::sync::oneshot::Sender<u32>,
    ) -> Result<(), BrowserError> {
        match &self.backend {
            Backend::Xvfb { x, .. } => x
                .queue(xserver::XCommand::CopyPress(result))
                .map_err(input_error),
            #[cfg(feature = "fake")]
            Backend::Fake(fake) => {
                fake.copy_press(result);
                Ok(())
            }
        }
    }

    /// Presses or releases a pointer button (`None` does nothing).
    ///
    /// # Errors
    /// When the input queue is full or the display stopped.
    pub fn button(&self, button: MouseButton, press: bool) -> Result<(), BrowserError> {
        let button = match button {
            MouseButton::None => return Ok(()),
            MouseButton::Left => 1,
            MouseButton::Middle => 2,
            MouseButton::Right => 3,
            MouseButton::Back => 8,
            MouseButton::Forward => 9,
        };
        self.send(DisplayInput::Button { button, press })
    }

    /// Presses or releases a key.
    ///
    /// # Errors
    /// When the input queue is full or the display stopped.
    pub fn key(&self, keycode: u8, press: bool) -> Result<(), BrowserError> {
        self.send(DisplayInput::Key { keycode, press })
    }

    /// Types a character as a real key press and release, in order with other keys.
    ///
    /// # Errors
    /// When the input queue is full or the display stopped.
    pub fn type_char(&self, c: char) -> Result<(), BrowserError> {
        self.send(DisplayInput::Type(c))
    }

    /// Waits (at most a second) until the display has taken every input sent before, so input
    /// that takes another path (CDP) does not overtake it.
    pub async fn settle(&self) {
        match &self.backend {
            Backend::Xvfb { x, .. } => {
                let _ = tokio::time::timeout(Duration::from_secs(1), x.sync()).await;
                // The browser reads the events the server delivered on its next turn.
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
            #[cfg(feature = "fake")]
            Backend::Fake(_) => {}
        }
    }

    /// Releases every key and button the display holds (a viewer left).
    pub fn release_all(&self) {
        if let Err(error) = self.send(DisplayInput::ReleaseAll) {
            warn!(%error, "releasing the display's input failed");
        }
    }

    /// Stops the video (reaping the encoder) and the X server (reaping it), and removes the
    /// cookie.
    ///
    /// # Errors
    /// [`BrowserError::NotReaped`] when the X server survived SIGKILL.
    pub(crate) async fn stop(&self, timeouts: StopTimeouts) -> Result<(), BrowserError> {
        self.video.shutdown(VIDEO_STOP).await;
        let outcome = match &self.backend {
            Backend::Xvfb {
                process, number, ..
            } => {
                let stop = StopTimeouts {
                    graceful: Duration::ZERO,
                    ..timeouts
                };
                let result = process
                    .terminate(stop)
                    .await
                    .map(|_| ())
                    .map_err(|_| BrowserError::NotReaped { pid: process.pid() });
                info!(display = number, "virtual display stopped");
                result
            }
            #[cfg(feature = "fake")]
            Backend::Fake(fake) => fake.stopped(),
        };
        if let Some(path) = &self.xauthority {
            let _ = std::fs::remove_file(path);
        }
        if let Some(dir) = &self.dir {
            let _ = std::fs::remove_dir(dir);
        }
        outcome
    }
}

fn quick_stop() -> StopTimeouts {
    StopTimeouts {
        graceful: Duration::ZERO,
        term: Duration::from_secs(2),
        kill: Duration::from_secs(5),
    }
}

enum Ready {
    Up(Box<x11rb::rust_connection::RustConnection>),
    /// Another server holds the number.
    Taken,
    Failed(BrowserError),
}

async fn wait_ready(
    process: &Supervised,
    number: u32,
    cookie: &[u8; 16],
    timeout: Duration,
) -> Ready {
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        if let Some(status) = process.exit_status() {
            let stderr = process.stderr_tail();
            if stderr.contains("already active")
                || stderr.contains("Cannot establish any listening")
            {
                return Ready::Taken;
            }
            return Ready::Failed(BrowserError::ExitedEarly { status, stderr });
        }
        let cookie = *cookie;
        let attempt = tokio::task::spawn_blocking(move || xserver::connect(number, &cookie)).await;
        let last = match attempt {
            Ok(Ok(conn)) => return Ready::Up(Box::new(conn)),
            Ok(Err(problem)) => problem,
            Err(error) => error.to_string(),
        };
        if tokio::time::Instant::now() >= deadline {
            return Ready::Failed(display_error(format!(
                "Xvfb :{number} did not accept connections within {timeout:?}: {last}"
            )));
        }
        tokio::time::sleep(READY_POLL).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn device_sizes_are_scaled_clamped_and_even() {
        let mut config = DisplayConfig::new("/run".into());
        assert_eq!(config.device_size((1280, 800)), (1280, 800));
        assert_eq!(
            config.device_size((1281, 801)),
            (1280, 800),
            "even for 4:2:0"
        );
        assert_eq!(
            config.device_size((5000, 5000)),
            (1920, 1200),
            "the maximum"
        );
        assert_eq!(config.device_size((10, 10)), (64, 64), "the minimum");
        config.scale = 2.0;
        assert_eq!(config.device_size((1000, 700)), (2000, 1400));
        assert_eq!(config.css_size((2000, 1400)), (1000, 700));
        config.scale = 1.5;
        assert_eq!(config.device_size((333, 333)), (498, 498));
        config.scale = f64::NAN;
        assert_eq!(
            config.device_size((100, 100)),
            (100, 100),
            "nonsense scale is 1"
        );
    }

    #[test]
    fn chrome_runs_in_kiosk_mode_without_the_automation_flag() {
        let mut config = DisplayConfig::new("/run".into());
        let args: Vec<String> = config
            .chromium_args((1280, 800))
            .iter()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();
        for expected in [
            "--kiosk",
            "--window-position=0,0",
            "--window-size=1280,800",
            "--disable-blink-features=AutomationControlled",
        ] {
            assert!(
                args.iter().any(|arg| arg == expected),
                "{expected} in {args:?}"
            );
        }
        assert!(
            !args
                .iter()
                .any(|arg| arg.contains("automation") && !arg.contains("AutomationControlled"))
        );
        assert!(!args.iter().any(|arg| arg.contains("scale-factor")));
        config.scale = 2.0;
        assert!(
            config
                .chromium_args((100, 100))
                .iter()
                .any(|arg| arg == "--force-device-scale-factor=2")
        );
    }

    #[test]
    fn the_cookie_file_is_one_wildcard_entry() {
        let cookie = [7_u8; 16];
        let entry = xauthority_entry(&cookie);
        assert_eq!(&entry[..2], &[0xFF, 0xFF], "FamilyWild");
        assert_eq!(&entry[2..6], &[0, 0, 0, 0], "empty address and number");
        assert_eq!(&entry[6..8], &[0, 18]);
        assert_eq!(&entry[8..26], b"MIT-MAGIC-COOKIE-1");
        assert_eq!(&entry[26..28], &[0, 16]);
        assert_eq!(&entry[28..], &cookie);
    }

    #[test]
    fn the_cookie_is_owner_only_in_an_owner_only_directory()
    -> Result<(), Box<dyn std::error::Error>> {
        use std::os::unix::fs::PermissionsExt as _;
        let root = tempfile::tempdir()?;
        let dir = root.path().join("displays/s1");
        private_dir(&dir)?;
        let path = dir.join(XAUTHORITY_FILE);
        write_cookie(&path, &[1; 16])?;
        write_cookie(&path, &[2; 16])?; // replaced, not appended
        assert_eq!(
            std::fs::metadata(&path)?.permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(std::fs::metadata(&dir)?.permissions().mode() & 0o777, 0o700);
        assert_eq!(std::fs::read(&path)?, xauthority_entry(&[2; 16]));
        Ok(())
    }

    #[test]
    fn dead_processes_are_told_from_live_ones() {
        assert!(process_alive(
            i32::try_from(std::process::id()).unwrap_or(1)
        ));
        // Pids are below 2^22 on Linux and 99999 on macOS.
        assert!(!process_alive(i32::MAX - 7));
    }

    #[test]
    fn orphan_sweeps_ignore_foreign_displays() {
        // Nothing of ours runs under a fresh root.
        assert_eq!(
            stop_orphaned_displays(Path::new("/nonexistent/yulius-displays")),
            0
        );
    }

    /// SPEC-616 XS-001, XS-004, XS-005 (#1519): a display whose start fails, once in readiness
    /// and once in the X controller's setup, keeps the Xvfb it could not stop, with its cookie,
    /// and reports the failed stop; once a stop succeeds the cookie goes and a new start works.
    #[tokio::test]
    async fn a_failed_display_start_keeps_the_xvfb_it_could_not_stop()
    -> Result<(), Box<dyn std::error::Error>> {
        let real = crate::resolve_executable(Path::new("Xvfb")).ok();
        if real.is_none() {
            eprintln!("Xvfb unavailable; only the readiness failure is checked");
        }
        let modes: &[(&str, &str)] = if real.is_some() {
            &[
                ("silent", "did not accept connections"),
                ("no-xtest", "XTEST"),
            ]
        } else {
            &[("silent", "did not accept connections")]
        };
        let quick = StopTimeouts {
            graceful: Duration::ZERO,
            term: Duration::from_secs(2),
            kill: Duration::from_secs(5),
        };
        for &(mode, cause) in modes {
            let root = tempfile::tempdir()?;
            let fake = stop_faults::FakeXvfb::new(root.path(), real.as_deref(), mode)?;
            let sandbox = SandboxId::parse("stranded-display")?;
            let mut config = DisplayConfig::new(root.path().join("displays"));
            stop_faults::private_displays(&mut config, 0);
            config.xvfb = fake.program.clone();
            config.max_size = (320, 240);
            let real_start = DisplayMode::Virtual(config.clone());
            if mode == "silent" {
                config.startup_timeout = Duration::from_millis(300);
            }
            let mode_config = DisplayMode::Virtual(config);
            let dir = root.path().join("displays").join(sandbox.as_str());
            let cookie = dir.join(XAUTHORITY_FILE);

            // Without a fault, a failed start leaves nothing behind.
            let failure = VirtualDisplay::start(&mode_config, &sandbox, (320, 240))
                .await
                .err()
                .ok_or("the display started")?;
            assert!(failure.stranded.is_none(), "{mode}: {failure}");
            assert!(
                failure.error.to_string().contains(cause),
                "{mode}: {failure}"
            );
            assert!(
                !fake.starts().into_iter().any(stop_faults::running),
                "{mode}"
            );
            assert!(!cookie.exists(), "{mode}");

            stop_faults::hold(&dir);
            let failure = VirtualDisplay::start(&mode_config, &sandbox, (320, 240))
                .await
                .err()
                .ok_or("the display started")?;
            let pid = *fake.starts().last().ok_or("no Xvfb started")?;
            let reported = |error: &BrowserError| {
                matches!(error, BrowserError::DisplayNotStopped { pid: p, cause: c, .. }
                    if i32::try_from(*p) == Ok(pid) && c.contains(cause))
            };
            assert!(reported(&failure.error), "{mode}: {:?}", failure.error);
            assert_eq!(
                failure.error.viewer_code(),
                yulius_contracts::browser::ViewerErrorCode::Internal
            );
            let stranded = failure.stranded.ok_or("the Xvfb was let go")?;
            assert!(
                stop_faults::running(pid),
                "{mode}: the Xvfb is still running"
            );
            assert!(cookie.exists(), "{mode}: its cookie is kept while it runs");
            let again = stranded.stop(quick).await;
            assert!(again.as_ref().is_err_and(reported), "{mode}: {again:?}");
            assert!(stop_faults::running(pid) && cookie.exists(), "{mode}");

            stop_faults::release(&dir);
            stranded.stop(quick).await?;
            assert!(!stop_faults::running(pid), "{mode}: stopped once it may");
            assert!(!cookie.exists(), "{mode}: its cookie goes with it");
            assert!(!dir.exists(), "{mode}: and its runtime directory");
            if real.is_some() {
                fake.set_mode("real")?;
                let display = VirtualDisplay::start(&real_start, &sandbox, (320, 240))
                    .await?
                    .ok_or("no display")?;
                assert!(display.is_alive(), "{mode}");
                display.stop(quick).await?;
            }
        }
        Ok(())
    }

    #[test]
    fn keyframes_are_on_demand_by_default() {
        assert_eq!(DisplayConfig::new("/run".into()).keyframe_interval, None);
    }
}

#[cfg(test)]
mod copy_input_tests {
    use super::DisplayInput;

    /// The X selection owner is a fixture on a display created just for this test.
    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn a_private_display_requests_fresh_text_only_after_acceptance()
    -> Result<(), Box<dyn std::error::Error>> {
        use super::*;
        use x11rb::connection::Connection as _;
        use x11rb::protocol::Event;
        use x11rb::protocol::xproto::{
            ConnectionExt as _, EventMask, PropMode, SELECTION_NOTIFY_EVENT, SelectionNotifyEvent,
        };
        use x11rb::wrapper::ConnectionExt as _;

        let dir = tempfile::tempdir()?;
        let sandbox = SandboxId::parse("clipboard-test")?;
        let mut config = DisplayConfig::new(dir.path().to_owned());
        if crate::resolve_executable(&config.xvfb).is_err() {
            eprintln!("Xvfb unavailable; private-display clipboard check did not run");
            return Ok(());
        }
        config.max_size = (640, 480);
        let display = VirtualDisplay::start(&DisplayMode::Virtual(config), &sandbox, (640, 480))
            .await?
            .ok_or("no test display")?;
        let result = async {
            let auth = std::fs::read(dir.path().join(sandbox.as_str()).join(XAUTHORITY_FILE))?;
            let number = display.name().trim_start_matches(':').parse()?;
            let owner = xserver::connect(number, &auth[auth.len() - 16..])?;
            let window = owner.setup().roots[0].root;
            let selection = owner.intern_atom(false, b"CLIPBOARD")?.reply()?.atom;
            let utf8 = owner.intern_atom(false, b"UTF8_STRING")?.reply()?.atom;
            let write = || -> Result<(), Box<dyn std::error::Error>> {
                owner
                    .set_selection_owner(window, selection, x11rb::CURRENT_TIME)?
                    .check()?;
                Ok(())
            };
            write()?;
            let (send, mut pending) = tokio::sync::oneshot::channel();
            display.copy_press(send)?;
            display.settle().await;
            // The selection already had an owner before the click.
            assert!(matches!(
                pending.try_recv(),
                Err(tokio::sync::oneshot::error::TryRecvError::Empty)
            ));
            write()?;
            let offer = tokio::time::timeout(Duration::from_secs(2), pending).await??;
            // Order the X thread's flushed requests and their delivery to the fixture owner
            // before checking for a premature read, including one rejected by its token.
            let no_request = || async {
                display.settle().await;
                owner.get_input_focus()?.reply()?;
                while let Some(event) = owner.poll_for_event()? {
                    assert!(
                        !matches!(event, Event::SelectionRequest(_)),
                        "selection requested before acceptance"
                    );
                }
                Ok::<_, Box<dyn std::error::Error>>(())
            };
            no_request().await?;
            assert!(display.accept_copy(offer.wrapping_add(1)).await.is_none());
            no_request().await?;
            let respond = || async {
                loop {
                    if let Some(Event::SelectionRequest(req)) = owner.poll_for_event()? {
                        assert_eq!((req.target, req.selection), (utf8, selection));
                        owner.change_property8(
                            PropMode::REPLACE,
                            req.requestor,
                            req.property,
                            utf8,
                            b"fixture copy",
                        )?;
                        owner.send_event(
                            false,
                            req.requestor,
                            EventMask::NO_EVENT,
                            SelectionNotifyEvent {
                                response_type: SELECTION_NOTIFY_EVENT,
                                sequence: 0,
                                time: req.time,
                                requestor: req.requestor,
                                selection,
                                target: utf8,
                                property: req.property,
                            },
                        )?;
                        owner.flush()?;
                        return Ok::<_, Box<dyn std::error::Error>>(());
                    }
                    tokio::time::sleep(Duration::from_millis(5)).await;
                }
            };
            let (text, sent) = tokio::join!(
                display.accept_copy(offer),
                tokio::time::timeout(Duration::from_secs(2), respond())
            );
            sent??;
            assert_eq!(text.as_deref(), Some("fixture copy"));
            assert!(display.accept_copy(offer).await.is_none());
            // A second write invalidates the first offer before acceptance.
            let (send, pending) = tokio::sync::oneshot::channel();
            display.copy_press(send)?;
            display.settle().await;
            write()?;
            let stale = tokio::time::timeout(Duration::from_secs(2), pending).await??;
            write()?;
            let acceptance = display.accept_copy(stale);
            tokio::pin!(acceptance);
            let text = tokio::select! {
                text = &mut acceptance => text,
                sent = respond() => { sent?; acceptance.await }
            };
            assert!(
                text.is_none(),
                "the old offer delivered the replacement text"
            );
            Ok::<_, Box<dyn std::error::Error>>(())
        }
        .await;
        display.stop(crate::StopTimeouts::default()).await?;
        result
    }

    /// SPEC-639 AC-2: a paste makes its text the display's CLIPBOARD and presses Shift+Insert (the paste key), in
    /// order with the keys around it. A client on the display stands in for Chrome: it reads
    /// the selection when the paste key arrives, as Chrome does.
    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn a_paste_owns_the_clipboard_and_presses_shift_insert_on_a_private_display()
    -> Result<(), Box<dyn std::error::Error>> {
        use super::*;
        use std::time::Instant;
        use x11rb::protocol::xproto::ConnectionExt as _;

        let dir = tempfile::tempdir()?;
        let sandbox = SandboxId::parse("paste-test")?;
        let mut config = DisplayConfig::new(dir.path().to_owned());
        if crate::resolve_executable(&config.xvfb).is_err() {
            eprintln!("Xvfb unavailable; private-display paste check did not run");
            return Ok(());
        }
        config.max_size = (320, 240);
        let display = VirtualDisplay::start(&DisplayMode::Virtual(config), &sandbox, (320, 240))
            .await?
            .ok_or("no test display")?;
        let result = async {
            let (page, atoms) = paste_page(&display, dir.path(), &sandbox).await?;
            let watch = |millis: u64| {
                let page = std::sync::Arc::clone(&page);
                tokio::task::spawn_blocking(move || {
                    observe_paste(&page, atoms, Instant::now() + Duration::from_millis(millis))
                })
            };
            for length in [100, 5_000] {
                let text: String = "ab€😀".chars().cycle().take(length).collect();
                let seen = watch(1_500);
                let outcome = display.paste(zeroize::Zeroizing::new(text.clone())).await?;
                assert_eq!(outcome, PasteOutcome::Read, "{length} characters");
                // Shift+Insert: a paste in Chrome, and one xterm.js leaves to the browser (it
                // makes Ctrl+V the ^V character).
                let (keys, read) = seen.await??;
                assert_eq!(keys, ["+50", "+118", "-118", "-50"]);
                assert_eq!(read, [text]);
            }
            // With Control held by the viewer (its Ctrl+V) Control is lifted around the
            // shortcut (Ctrl+Shift+Insert is no paste) and pressed again after it.
            display.type_char('x')?;
            display.key(37, true)?;
            let seen = watch(1_500);
            let outcome = display
                .paste(zeroize::Zeroizing::new("held".into()))
                .await?;
            assert_eq!(outcome, PasteOutcome::Read);
            let (keys, read) = seen.await??;
            assert_eq!(
                keys,
                [
                    "+53", "-53", "+37", "-37", "+50", "+118", "-118", "-50", "+37"
                ]
            );
            assert_eq!(read, ["held"]);
            display.key(37, false)?;
            // Nobody asks for the text: the paste was not read.
            page.unmap_window(atoms.window)?.check()?;
            let unread = display
                .paste(zeroize::Zeroizing::new("lost".into()))
                .await?;
            assert_eq!(unread, PasteOutcome::Unread);
            Ok::<_, Box<dyn std::error::Error>>(())
        }
        .await;
        display.stop(crate::StopTimeouts::default()).await?;
        result
    }

    /// SPEC-639 PST-006 (review round 1): a newer paste waits while the page still reads the one
    /// before (Chrome asks for the targets again after the text), also when it was queued before
    /// the page asked at all, so each paste key reads its own text; and the wait is bounded for a
    /// client that never stops asking.
    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn a_newer_paste_waits_until_the_page_stopped_reading_the_one_before()
    -> Result<(), Box<dyn std::error::Error>> {
        use super::*;
        use crate::xclipboard::{PASTE_QUIET, PASTE_QUIET_MAX};
        use std::time::Instant;

        let dir = tempfile::tempdir()?;
        let sandbox = SandboxId::parse("paste-quiet")?;
        let mut config = DisplayConfig::new(dir.path().to_owned());
        if crate::resolve_executable(&config.xvfb).is_err() {
            eprintln!("Xvfb unavailable; paste quiet check did not run");
            return Ok(());
        }
        config.max_size = (320, 240);
        let display = VirtualDisplay::start(&DisplayMode::Virtual(config), &sandbox, (320, 240))
            .await?
            .ok_or("no test display")?;
        let result = async {
            let (page, atoms) = paste_page(&display, dir.path(), &sandbox).await?;
            let watch = |presses: usize, reread: Duration, asking: Duration, late: Duration| {
                let page = std::sync::Arc::clone(&page);
                tokio::task::spawn_blocking(move || {
                    observe_rereads(&page, atoms, presses, reread, asking, late)
                })
            };
            // Two pastes queued at once: the second waits for the page to read the first.
            let seen = watch(2, Duration::from_millis(30), Duration::ZERO, Duration::ZERO);
            let (first, second) = tokio::join!(
                display.paste(zeroize::Zeroizing::new("first".into())),
                display.paste(zeroize::Zeroizing::new("second".into()))
            );
            assert_eq!((first?, second?), (PasteOutcome::Read, PasteOutcome::Read));
            let seen = seen.await??;
            let names: Vec<&str> = seen.iter().map(|(_, name)| name.as_str()).collect();
            assert_eq!(
                names,
                [
                    "key",
                    "text first",
                    "targets",
                    "key",
                    "text second",
                    "targets"
                ]
            );
            let gap = seen[3].0.duration_since(seen[2].0);
            assert!(
                gap >= PASTE_QUIET,
                "the second paste came {gap:?} after the last read"
            );

            // A client that keeps asking holds a newer paste at most the bound.
            // A busy page asks late (review round 3 measured 360 ms): the paste queued after it
            // still waits, so each gets its own text.
            let seen = watch(
                2,
                Duration::ZERO,
                Duration::ZERO,
                Duration::from_millis(350),
            );
            let (first, second) = tokio::join!(
                display.paste(zeroize::Zeroizing::new("busy first".into())),
                display.paste(zeroize::Zeroizing::new("busy second".into()))
            );
            assert_eq!((first?, second?), (PasteOutcome::Read, PasteOutcome::Read));
            let names: Vec<String> = seen.await??.into_iter().map(|(_, name)| name).collect();
            assert_eq!(
                names,
                [
                    "key",
                    "text busy first",
                    "targets",
                    "key",
                    "text busy second",
                    "targets"
                ]
            );

            let seen = watch(2, Duration::ZERO, PASTE_QUIET_MAX * 3, Duration::ZERO);
            display
                .paste(zeroize::Zeroizing::new("third".into()))
                .await?;
            let started = Instant::now();
            let outcome = display
                .paste(zeroize::Zeroizing::new("fourth".into()))
                .await?;
            assert_eq!(outcome, PasteOutcome::Read);
            let seen = seen.await??;
            let second_key = seen
                .iter()
                .filter(|(_, name)| name == "key")
                .nth(1)
                .ok_or("no second paste key")?;
            let held = second_key.0.duration_since(started);
            assert!(
                held < PASTE_QUIET_MAX * 2,
                "a client that kept asking held the paste {held:?}"
            );
            Ok::<_, Box<dyn std::error::Error>>(())
        }
        .await;
        display.stop(crate::StopTimeouts::default()).await?;
        result
    }

    /// SPEC-639 (review round 2): pastes nobody reads (the focus is not in a field, the person
    /// pastes again) hold the display's input only a short grace each, not a second each.
    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn unread_pastes_hold_the_input_after_them_only_briefly()
    -> Result<(), Box<dyn std::error::Error>> {
        use super::*;
        use std::time::Instant;

        let dir = tempfile::tempdir()?;
        let sandbox = SandboxId::parse("paste-unread")?;
        let mut config = DisplayConfig::new(dir.path().to_owned());
        if crate::resolve_executable(&config.xvfb).is_err() {
            eprintln!("Xvfb unavailable; unread paste check did not run");
            return Ok(());
        }
        config.max_size = (320, 240);
        let display = VirtualDisplay::start(&DisplayMode::Virtual(config), &sandbox, (320, 240))
            .await?
            .ok_or("no test display")?;
        let result = async {
            let (page, atoms) = paste_page(&display, dir.path(), &sandbox).await?;
            let started = Instant::now();
            let mut outcomes = Vec::new();
            for n in 0..5 {
                outcomes.push(display.queue_paste(zeroize::Zeroizing::new(format!("unread {n}")))?);
            }
            display.type_char('z')?;
            // The page never asks; it only waits for the key after the pastes.
            let arrived = {
                let page = std::sync::Arc::clone(&page);
                tokio::task::spawn_blocking(move || -> Result<Option<Instant>, String> {
                    use x11rb::connection::Connection as _;
                    use x11rb::protocol::Event;
                    let until = Instant::now() + Duration::from_secs(8);
                    while Instant::now() < until {
                        match page.poll_for_event().map_err(|e| e.to_string())? {
                            Some(Event::KeyPress(key)) if key.detail == 52 => {
                                return Ok(Some(Instant::now()));
                            }
                            Some(_) => {}
                            None => std::thread::sleep(Duration::from_millis(2)),
                        }
                    }
                    Ok(None)
                })
            };
            let at = arrived
                .await??
                .ok_or("the key after the pastes never came")?;
            let held = at.duration_since(started);
            assert!(
                held < Duration::from_millis(2_500),
                "five unread pastes held the input {held:?}"
            );
            let _ = atoms;
            for outcome in outcomes {
                assert_eq!(outcome.await?, PasteOutcome::Unread);
            }
            Ok::<_, Box<dyn std::error::Error>>(())
        }
        .await;
        display.stop(crate::StopTimeouts::default()).await?;
        result
    }

    /// SPEC-639 (review round 2), ADR-0423: a paste right after a click is no page's copy: the
    /// click's watch for a CLIPBOARD write ends, so the pasted text is never offered back.
    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn a_paste_after_a_click_is_not_offered_as_the_pages_copy()
    -> Result<(), Box<dyn std::error::Error>> {
        use super::*;

        let dir = tempfile::tempdir()?;
        let sandbox = SandboxId::parse("paste-after-click")?;
        let mut config = DisplayConfig::new(dir.path().to_owned());
        if crate::resolve_executable(&config.xvfb).is_err() {
            eprintln!("Xvfb unavailable; paste after click check did not run");
            return Ok(());
        }
        config.max_size = (320, 240);
        let display = VirtualDisplay::start(&DisplayMode::Virtual(config), &sandbox, (320, 240))
            .await?
            .ok_or("no test display")?;
        let result = async {
            let (send, mut offer) = tokio::sync::oneshot::channel();
            display.copy_press(send)?;
            let outcome = display
                .paste(zeroize::Zeroizing::new("pasted by A".into()))
                .await?;
            assert_eq!(outcome, PasteOutcome::Unread, "no page read it");
            display.settle().await;
            assert!(
                matches!(
                    offer.try_recv(),
                    Err(tokio::sync::oneshot::error::TryRecvError::Closed)
                ),
                "the paste was offered as the page's copy"
            );
            Ok::<_, Box<dyn std::error::Error>>(())
        }
        .await;
        display.stop(crate::StopTimeouts::default()).await?;
        result
    }

    /// A page that reads like Chrome: on each Shift+Insert (`late` after it, a busy page) its
    /// targets, then the text, then (after `reread`) its targets again; with `asking`, it goes on
    /// asking for the targets every 10 ms for that long after the first paste key. Returns when and what it saw (`key`,
    /// `text <text>`, `targets`) once it saw `presses` paste keys and read after the last.
    #[cfg(target_os = "linux")]
    fn observe_rereads(
        page: &x11rb::rust_connection::RustConnection,
        atoms: PasteAtoms,
        presses: usize,
        reread: std::time::Duration,
        asking: std::time::Duration,
        late: std::time::Duration,
    ) -> Result<Vec<(std::time::Instant, String)>, String> {
        use std::time::{Duration, Instant};
        use x11rb::connection::Connection as _;
        use x11rb::protocol::Event;
        use x11rb::protocol::xproto::{AtomEnum, ConnectionExt as _, KeyButMask};
        let text = |e: &dyn std::fmt::Display| e.to_string();
        // Events that arrive while a read waits for its answer, for the loop below.
        let later = std::cell::RefCell::new(std::collections::VecDeque::new());
        let convert = |target| -> Result<Option<Vec<u8>>, String> {
            page.convert_selection(
                atoms.window,
                atoms.selection,
                target,
                atoms.into,
                x11rb::CURRENT_TIME,
            )
            .map_err(|e| text(&e))?;
            page.flush().map_err(|e| text(&e))?;
            loop {
                let event = page.wait_for_event().map_err(|e| text(&e))?;
                let Event::SelectionNotify(answer) = event else {
                    later.borrow_mut().push_back(event);
                    continue;
                };
                if answer.requestor == atoms.window {
                    if answer.property == x11rb::NONE {
                        return Ok(None);
                    }
                    let value = page
                        .get_property(true, atoms.window, atoms.into, AtomEnum::ANY, 0, 1 << 20)
                        .map_err(|e| text(&e))?
                        .reply()
                        .map_err(|e| text(&e))?
                        .value;
                    return Ok(Some(value));
                }
            }
        };
        let until = Instant::now() + Duration::from_secs(10);
        let mut seen = Vec::new();
        let mut asking_until: Option<Instant> = None;
        while Instant::now() < until {
            let keys = seen
                .iter()
                .filter(|(_, n): &&(Instant, String)| n == "key")
                .count();
            if keys == presses && seen.last().is_some_and(|(_, n)| n == "targets") {
                return Ok(seen);
            }
            if asking_until.is_some_and(|end| Instant::now() < end) {
                convert(atoms.targets)?;
                std::thread::sleep(Duration::from_millis(10));
            }
            let queued = later.borrow_mut().pop_front();
            let next = match queued {
                Some(event) => Some(event),
                None => page.poll_for_event().map_err(|e| text(&e))?,
            };
            let Some(event) = next else {
                std::thread::sleep(Duration::from_millis(1));
                continue;
            };
            if let Event::KeyPress(press) = &event
                && press.detail == 118
                && press.state.contains(KeyButMask::SHIFT)
            {
                seen.push((Instant::now(), "key".to_owned()));
                if asking_until.is_none() && !asking.is_zero() {
                    asking_until = Some(Instant::now() + asking);
                }
                // A busy page handles the key, and asks, only once its task ended.
                std::thread::sleep(late);
                convert(atoms.targets)?.ok_or("the targets were refused")?;
                let pasted = convert(atoms.utf8)?.ok_or("the text was refused")?;
                let pasted = String::from_utf8(pasted).map_err(|e| text(&e))?;
                seen.push((Instant::now(), format!("text {pasted}")));
                std::thread::sleep(reread);
                convert(atoms.targets)?.ok_or("the targets were refused")?;
                seen.push((Instant::now(), "targets".to_owned()));
            }
        }
        Err(format!("the page saw {seen:?}"))
    }

    /// SPEC-639 PST-003 (review round 1): where the CLIPBOARD cannot be taken a paste is typed as
    /// real key presses, the viewer's held Control lifted meanwhile (it would make shortcuts of
    /// the text) and pressed again after.
    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn a_paste_without_the_clipboard_is_typed_as_key_presses()
    -> Result<(), Box<dyn std::error::Error>> {
        use super::*;
        use std::time::Instant;

        let dir = tempfile::tempdir()?;
        let sandbox = SandboxId::parse("paste-typed")?;
        let mut config = DisplayConfig::new(dir.path().to_owned());
        if crate::resolve_executable(&config.xvfb).is_err() {
            eprintln!("Xvfb unavailable; typed paste check did not run");
            return Ok(());
        }
        config.max_size = (320, 240);
        let display = VirtualDisplay::start(&DisplayMode::Virtual(config), &sandbox, (320, 240))
            .await?
            .ok_or("no test display")?;
        let result = async {
            let (page, atoms) = paste_page(&display, dir.path(), &sandbox).await?;
            let auth = std::fs::read(dir.path().join(sandbox.as_str()).join(XAUTHORITY_FILE))?;
            let number = display.name().trim_start_matches(':').parse()?;
            let conn = xserver::connect(number, &auth[auth.len() - 16..])?;
            let typing = xserver::XController::start_with(conn, (320, 240), false)?;
            let seen = {
                let page = std::sync::Arc::clone(&page);
                tokio::task::spawn_blocking(move || {
                    observe_paste(&page, atoms, Instant::now() + Duration::from_millis(1_500))
                })
            };
            typing
                .send(DisplayInput::Key {
                    keycode: 37,
                    press: true,
                })
                .map_err(|e| format!("{e:?}"))?;
            let (outcome, answer) = tokio::sync::oneshot::channel();
            typing
                .queue(xserver::XCommand::Paste(
                    zeroize::Zeroizing::new("aB\r\n".into()),
                    outcome,
                ))
                .map_err(|e| format!("{e:?}"))?;
            assert_eq!(answer.await?, PasteOutcome::Typed);
            let (keys, read) = seen.await??;
            assert_eq!(
                keys,
                [
                    "+37", "-37", "+38", "-38", "+50", "+56", "-56", "-50", "+36", "-36", "+37"
                ]
            );
            assert!(read.is_empty(), "nothing was pasted: {read:?}");
            Ok::<_, Box<dyn std::error::Error>>(())
        }
        .await;
        display.stop(crate::StopTimeouts::default()).await?;
        result
    }

    /// A window on the test display that takes the keyboard, as a page does, and the atoms a
    /// paste uses.
    #[cfg(target_os = "linux")]
    async fn paste_page(
        display: &super::VirtualDisplay,
        runtime: &std::path::Path,
        sandbox: &yulius_domain::ids::SandboxId,
    ) -> Result<
        (
            std::sync::Arc<x11rb::rust_connection::RustConnection>,
            PasteAtoms,
        ),
        Box<dyn std::error::Error>,
    > {
        use super::*;
        use std::time::Instant;
        use x11rb::connection::Connection as _;
        use x11rb::protocol::xproto::{
            ConnectionExt as _, CreateWindowAux, EventMask, WindowClass,
        };
        let auth = std::fs::read(runtime.join(sandbox.as_str()).join(XAUTHORITY_FILE))?;
        let number = display.name().trim_start_matches(':').parse()?;
        let page = xserver::connect(number, &auth[auth.len() - 16..])?;
        let root = page.setup().roots[0].root;
        let atom = |name: &[u8]| -> Result<u32, Box<dyn std::error::Error>> {
            Ok(page.intern_atom(false, name)?.reply()?.atom)
        };
        let (selection, targets, utf8, into) = (
            atom(b"CLIPBOARD")?,
            atom(b"TARGETS")?,
            atom(b"UTF8_STRING")?,
            atom(b"YULIUS_TEST_PASTE")?,
        );
        let window = page.generate_id()?;
        page.create_window(
            0,
            window,
            root,
            0,
            0,
            100,
            100,
            0,
            WindowClass::INPUT_OUTPUT,
            0,
            &CreateWindowAux::new().event_mask(EventMask::KEY_PRESS | EventMask::KEY_RELEASE),
        )?
        .check()?;
        page.map_window(window)?.check()?;
        // The display's thread focuses a window that maps (it plays the window manager).
        let deadline = Instant::now() + Duration::from_secs(5);
        while page.get_input_focus()?.reply()?.focus != window {
            assert!(
                Instant::now() < deadline,
                "the test window never got the focus"
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        let atoms = PasteAtoms {
            window,
            selection,
            targets,
            utf8,
            into,
        };
        Ok((std::sync::Arc::new(page), atoms))
    }

    #[cfg(target_os = "linux")]
    #[derive(Clone, Copy)]
    struct PasteAtoms {
        window: u32,
        selection: u32,
        targets: u32,
        utf8: u32,
        into: u32,
    }

    /// What a page on the display sees until `until`: its key presses (`+50`) and releases
    /// (`-118`), and the texts it read on each Shift+Insert as Chrome does (TARGETS first, then
    /// UTF8_STRING).
    #[cfg(target_os = "linux")]
    fn observe_paste(
        page: &x11rb::rust_connection::RustConnection,
        atoms: PasteAtoms,
        until: std::time::Instant,
    ) -> Result<(Vec<String>, Vec<String>), String> {
        use x11rb::connection::Connection as _;
        use x11rb::protocol::Event;
        use x11rb::protocol::xproto::{AtomEnum, ConnectionExt as _, KeyButMask};
        let text = |e: &dyn std::fmt::Display| e.to_string();
        let (mut keys, mut read) = (Vec::new(), Vec::new());
        let key = |event: &Event, keys: &mut Vec<String>| match event {
            Event::KeyPress(key) => keys.push(format!("+{}", key.detail)),
            Event::KeyRelease(key) => keys.push(format!("-{}", key.detail)),
            _ => {}
        };
        let convert = |target, keys: &mut Vec<String>| -> Result<Option<Vec<u8>>, String> {
            page.convert_selection(
                atoms.window,
                atoms.selection,
                target,
                atoms.into,
                x11rb::CURRENT_TIME,
            )
            .map_err(|e| text(&e))?;
            page.flush().map_err(|e| text(&e))?;
            loop {
                match page.wait_for_event().map_err(|e| text(&e))? {
                    Event::SelectionNotify(answer) if answer.requestor == atoms.window => {
                        if answer.property == x11rb::NONE {
                            return Ok(None);
                        }
                        let value = page
                            .get_property(true, atoms.window, atoms.into, AtomEnum::ANY, 0, 1 << 20)
                            .map_err(|e| text(&e))?
                            .reply()
                            .map_err(|e| text(&e))?
                            .value;
                        return Ok(Some(value));
                    }
                    other => key(&other, keys),
                }
            }
        };
        while std::time::Instant::now() < until {
            let Some(event) = page.poll_for_event().map_err(|e| text(&e))? else {
                std::thread::sleep(std::time::Duration::from_millis(5));
                continue;
            };
            key(&event, &mut keys);
            if let Event::KeyPress(press) = &event
                && press.detail == 118
                && press.state.contains(KeyButMask::SHIFT)
            {
                let offered = convert(atoms.targets, &mut keys)?.unwrap_or_default();
                let offered: Vec<u32> = offered
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .map(|b| u32::from_ne_bytes(*b))
                    .collect();
                if !offered.contains(&atoms.utf8) {
                    return Err(format!("targets {offered:?}"));
                }
                let pasted = convert(atoms.utf8, &mut keys)?.ok_or("the text was refused")?;
                read.push(String::from_utf8(pasted).map_err(|e| text(&e))?);
            }
        }
        Ok((keys, read))
    }

    #[test]
    fn cleanup_when_an_observer_leaves_preserves_the_offered_copy() {
        assert!(!DisplayInput::ReleaseAll.supersedes_copy());
        assert!(DisplayInput::Type('x').supersedes_copy());
        for press in [false, true] {
            assert_eq!(
                DisplayInput::Key { keycode: 1, press }.supersedes_copy(),
                press
            );
            assert_eq!(
                DisplayInput::Button { button: 1, press }.supersedes_copy(),
                press
            );
        }
    }
}
