//! The export's freeze (SPEC-329 EXP-001, ADR-0222): systemd's freezer on the whole service
//! unit, so the server and everything it started stop between two instructions while the data
//! directory is read, and the database's main file and WAL stand as after a power cut.

use std::path::Path;
use std::sync::atomic::{AtomicU8, Ordering};
use std::time::Duration;

/// How long systemd may take to report the unit frozen.
pub const FREEZE_WAIT: Duration = Duration::from_secs(60);
/// Pause between two looks at the freezer state.
const POLL: Duration = Duration::from_millis(250);

/// The host, as the freeze sees it.
pub trait Host {
    /// Runs `systemctl` with `args`: its exit code and standard output, or why it did not run.
    ///
    /// # Errors
    /// It could not be started.
    fn systemctl(&mut self, args: &[&str]) -> Result<(i32, String), String>;
    /// A file's text; `None` when it cannot be read.
    fn read(&self, path: &Path) -> Option<String>;
    /// Waits `d`.
    fn sleep(&mut self, d: Duration);
}

/// The unit's state, from `systemctl show`.
#[derive(Debug, Default)]
struct Unit {
    load: String,
    active: String,
    freezer: String,
    cgroup: String,
}

fn show(host: &mut dyn Host, unit: &str) -> Result<Unit, String> {
    let (code, out) = host.systemctl(&[
        "show",
        "--property=LoadState,ActiveState,FreezerState,ControlGroup",
        unit,
    ])?;
    if code != 0 {
        return Err(format!("`systemctl show {unit}` failed (exit {code})"));
    }
    let mut u = Unit::default();
    for line in out.lines() {
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let value = value.trim().to_owned();
        match key {
            "LoadState" => u.load = value,
            "ActiveState" => u.active = value,
            "FreezerState" => u.freezer = value,
            "ControlGroup" => u.cgroup = value,
            _ => {}
        }
    }
    Ok(u)
}

/// The cgroup v2 path of this process (`0::<path>`).
fn own_cgroup(host: &dyn Host) -> Option<String> {
    host.read(Path::new("/proc/self/cgroup"))?
        .lines()
        .find_map(|l| l.strip_prefix("0::").map(str::to_owned))
}

/// A held freeze: thawed when dropped unless it was released or kept for a move.
pub struct Freeze {
    host: Box<dyn Host>,
    unit: String,
    update_unit: String,
    /// The export froze the unit (it was running and not frozen already).
    ours: bool,
    /// The unit was stopped with nothing of it running.
    stopped: bool,
}

impl std::fmt::Debug for Freeze {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Freeze")
            .field("unit", &self.unit)
            .field("ours", &self.ours)
            .finish_non_exhaustive()
    }
}

/// Freezes `unit` for an export, or says why not; `Err` is a refusal, nothing was changed.
///
/// # Errors
/// The unit is unknown, the export runs inside it, the update helper `update_unit` runs, a
/// stopped unit still has processes, or systemd did not freeze it in [`FREEZE_WAIT`].
pub fn freeze(mut host: Box<dyn Host>, unit: &str, update_unit: &str) -> Result<Freeze, String> {
    let state = show(&mut *host, unit)?;
    if state.load != "loaded" {
        return Err(format!(
            "there is no unit {unit} ({}); name the server's unit with --service",
            if state.load.is_empty() {
                "unknown"
            } else {
                &state.load
            }
        ));
    }
    if !state.cgroup.is_empty()
        && let Some(own) = own_cgroup(&*host)
        && (own == state.cgroup || own.starts_with(&format!("{}/", state.cgroup)))
    {
        return Err(format!(
            "this command runs inside {unit} (a Yulius terminal?) and would freeze itself; run \
             it over SSH on the host"
        ));
    }
    let mut held = Freeze {
        host,
        unit: unit.to_owned(),
        update_unit: update_unit.to_owned(),
        ours: false,
        stopped: false,
    };
    held.helper_idle()?;
    if !matches!(state.active.as_str(), "active" | "activating" | "reloading") {
        let events = Path::new("/sys/fs/cgroup")
            .join(state.cgroup.trim_start_matches('/'))
            .join("cgroup.events");
        let populated = !state.cgroup.is_empty()
            && held
                .host
                .read(&events)
                .is_some_and(|e| e.lines().any(|l| l.trim() == "populated 1"));
        if populated {
            return Err(format!(
                "{unit} is stopped but its agents and terminals still run (KillMode=process); \
                 start it (`systemctl start {unit}`) so the export can freeze them"
            ));
        }
        held.stopped = true;
        return Ok(held);
    }
    if state.freezer == "frozen" {
        return Ok(held);
    }
    let (code, _) = held.host.systemctl(&["freeze", unit])?;
    held.ours = true;
    if code != 0 {
        held.thaw_now();
        return Err(format!(
            "`systemctl freeze {unit}` failed (exit {code}); the export needs systemd 246 or \
             later with cgroup v2"
        ));
    }
    let mut waited = Duration::ZERO;
    loop {
        if show(&mut *held.host, unit)?.freezer == "frozen" {
            // An update request written just before the freeze may have started the helper,
            // which would restart the unit under the export.
            if let Err(why) = held.helper_idle() {
                held.thaw_now();
                return Err(why);
            }
            return Ok(held);
        }
        if waited >= FREEZE_WAIT {
            held.thaw_now();
            return Err(format!(
                "{unit} did not freeze within {} s (a process may hang in the kernel); it was \
                 thawed and nothing was exported",
                FREEZE_WAIT.as_secs()
            ));
        }
        held.host.sleep(POLL);
        waited += POLL;
    }
}

impl Freeze {
    /// Whether the export froze the unit (so it thaws it).
    #[must_use]
    pub fn ours(&self) -> bool {
        self.ours
    }

    /// What the export holds, for the operator.
    #[must_use]
    pub fn describe(&self) -> String {
        let unit = &self.unit;
        if self.ours {
            format!(
                "Frozen {unit}: every Yulius process is paused until the export ends. If this \
                 command is killed with SIGKILL, thaw it with `sudo systemctl thaw {unit}`."
            )
        } else if self.stopped {
            format!("{unit} is stopped and nothing of it runs; nothing was frozen.")
        } else {
            format!("{unit} was frozen before the export; it stays frozen after it.")
        }
    }

    /// Refuses while the update helper runs: it would stop and start the unit.
    ///
    /// # Errors
    /// The helper is active, or `systemctl` did not run.
    pub fn helper_idle(&mut self) -> Result<(), String> {
        let (_, state) = self.host.systemctl(&["is-active", &self.update_unit])?;
        if matches!(state.trim(), "active" | "activating" | "reloading") {
            return Err(format!(
                "an update is running ({}); export once it has finished",
                self.update_unit
            ));
        }
        Ok(())
    }

    fn thaw_now(&mut self) -> Option<String> {
        if !std::mem::take(&mut self.ours) {
            return None;
        }
        match self.host.systemctl(&["thaw", &self.unit]) {
            Ok((0, _)) => None,
            Ok((code, _)) => Some(format!(
                "`systemctl thaw {}` failed (exit {code}); run it by hand",
                self.unit
            )),
            Err(e) => Some(format!(
                "`systemctl thaw {}` did not run ({e}); run it by hand",
                self.unit
            )),
        }
    }

    /// Thaws the unit if the export froze it.
    ///
    /// # Errors
    /// The thaw failed; the message says to run it by hand.
    pub fn release(mut self) -> Result<(), String> {
        self.thaw_now().map_or(Ok(()), Err)
    }

    /// For a move: leaves the unit as it is (frozen) and disables it, so a reboot does not start
    /// the old instance beside the new one.
    ///
    /// # Errors
    /// The disable failed (the unit stays frozen).
    pub fn keep_for_move(mut self) -> Result<(), String> {
        self.ours = false;
        match self.host.systemctl(&["disable", &self.unit])? {
            (0, _) => Ok(()),
            (code, _) => Err(format!(
                "`systemctl disable {}` failed (exit {code}); the instance stays frozen, \
                 disable it by hand",
                self.unit
            )),
        }
    }
}

impl Drop for Freeze {
    fn drop(&mut self) {
        if let Some(why) = self.thaw_now() {
            eprintln!("yulius instance export: {why}");
        }
    }
}

/// [`on_signal`]: the freeze is being taken.
pub const ARMING: u8 = 0;
/// [`on_signal`]: the export froze the unit and has not finished.
pub const THAW: u8 = 1;
/// [`on_signal`]: nothing to thaw (not frozen by the export, or the export has finished).
pub const LEAVE: u8 = 2;

/// What a SIGINT, SIGTERM or SIGHUP does during an export, on its own thread so it works while
/// the export is blocked writing to a stalled pipe: waits (bounded) while the freeze is being
/// taken, so a thaw never comes before it, then thaws the unit if `state` says the export froze
/// it. Returns why a thaw failed.
pub fn on_signal(state: &AtomicU8, host: &mut dyn Host, unit: &str) -> Option<String> {
    let mut waited = Duration::ZERO;
    while state.load(Ordering::SeqCst) == ARMING && waited < FREEZE_WAIT + FREEZE_WAIT {
        host.sleep(POLL);
        waited += POLL;
    }
    if state.load(Ordering::SeqCst) != THAW {
        return None;
    }
    match host.systemctl(&["thaw", unit]) {
        Ok((0, _)) => None,
        Ok((code, _)) => Some(format!(
            "`systemctl thaw {unit}` failed (exit {code}); run it by hand"
        )),
        Err(e) => Some(format!(
            "`systemctl thaw {unit}` did not run ({e}); run it by hand"
        )),
    }
}

/// The real host.
#[derive(Debug, Default)]
pub struct System;

impl Host for System {
    fn systemctl(&mut self, args: &[&str]) -> Result<(i32, String), String> {
        let out = std::process::Command::new("systemctl")
            .args(args)
            .stdin(std::process::Stdio::null())
            .stderr(std::process::Stdio::inherit())
            .output()
            .map_err(|e| format!("systemctl: {e}"))?;
        Ok((
            out.status.code().unwrap_or(-1),
            String::from_utf8_lossy(&out.stdout).into_owned(),
        ))
    }

    fn read(&self, path: &Path) -> Option<String> {
        std::fs::read_to_string(path).ok()
    }

    fn sleep(&mut self, d: Duration) {
        std::thread::sleep(d);
    }
}

#[cfg(test)]
mod tests;
