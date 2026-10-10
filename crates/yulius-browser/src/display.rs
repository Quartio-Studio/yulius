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
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use tracing::{info, warn};
use yulius_contracts::browser::MouseButton;
use yulius_domain::ids::SandboxId;

use crate::encoder::FfmpegStart;
use crate::error::BrowserError;
use crate::process::{StopTimeouts, Supervised, passed_env};
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

impl VirtualDisplay {
    /// Starts a display for `sandbox` sized for `css` (CSS pixels).
    ///
    /// # Errors
    /// [`BrowserError::Display`] when Xvfb is missing, no display number is free, the server
    /// does not come up or lacks XTest. A failed start leaves no process behind.
    pub(crate) async fn start(
        mode: &DisplayMode,
        sandbox: &SandboxId,
        css: (u32, u32),
    ) -> Result<Option<Self>, BrowserError> {
        match mode {
            DisplayMode::Headless => Ok(None),
            DisplayMode::Virtual(config) => Self::start_xvfb(config, sandbox, css).await.map(Some),
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
    ) -> Result<Self, BrowserError> {
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
            match wait_ready(&process, number, &cookie, config.startup_timeout).await {
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
                            let _ = process.terminate(quick_stop()).await;
                            let _ = std::fs::remove_file(&xauthority);
                            return Err(display_error(problem));
                        }
                    };
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
                    let _ = process.terminate(quick_stop()).await;
                }
                Ready::Failed(error) => {
                    let _ = process.terminate(quick_stop()).await;
                    let _ = std::fs::remove_file(&xauthority);
                    return Err(error);
                }
            }
        }
        let _ = std::fs::remove_file(&xauthority);
        Err(display_error(format!(
            "no free display number in :{}..:{}",
            config.first_display,
            last.saturating_sub(1)
        )))
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
            Backend::Xvfb { x, .. } => x.send(input).map_err(|error| match error {
                // Only discrete input queues (moves are coalesced): the X server stopped
                // taking input.
                XSendError::Busy => BrowserError::InputBacklog,
                XSendError::Gone => display_error("the display stopped"),
            }),
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
            Backend::Fake(fake) => {
                fake.stopped();
                Ok(())
            }
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

    #[test]
    fn keyframes_are_on_demand_by_default() {
        assert_eq!(DisplayConfig::new("/run".into()).keyframe_interval, None);
    }
}
