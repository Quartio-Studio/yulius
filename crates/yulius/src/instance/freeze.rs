//! The export's freeze (SPEC-329 EXP-001, ADR-0222): systemd's freezer on the whole service
//! unit and on its sessions unit (SPEC-458 EXP-001, ADR-0298), so the server and everything it
//! started stop between two instructions while the data directory is read, and the database's
//! main file and WAL stand as after a power cut.

use std::path::Path;
use std::sync::Mutex;
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

fn runs(state: &Unit) -> bool {
    matches!(state.active.as_str(), "active" | "activating" | "reloading")
}

/// What the kernel says of a unit's cgroup and everything below it (its `cgroup.events`).
#[derive(Debug, Default, Clone, Copy)]
struct Group {
    /// A process lives in it. A delegated unit keeps its processes in subgroups, which this
    /// covers.
    populated: bool,
    /// All of it is frozen.
    frozen: bool,
}

/// The cgroup `cgroup` of a unit, as `systemctl show` names it. A unit without one (it has
/// stopped and left nothing, or never ran since the host started) holds nothing, and neither
/// does a group that is gone by the time it is read.
fn group(host: &dyn Host, cgroup: &str) -> Group {
    if cgroup.is_empty() {
        return Group::default();
    }
    let events = Path::new("/sys/fs/cgroup")
        .join(cgroup.trim_start_matches('/'))
        .join("cgroup.events");
    let said = host.read(&events).unwrap_or_default();
    let says = |line: &str| said.lines().any(|l| l.trim() == line);
    Group {
        populated: says("populated 1"),
        frozen: says("frozen 1"),
    }
}

/// A held freeze: thawed when dropped unless it was released or kept for a move.
pub struct Freeze {
    host: Box<dyn Host>,
    unit: String,
    update_unit: String,
    /// The running units the freeze covers: the server's and its sessions unit (SPEC-458).
    covered: Vec<String>,
    /// The units the export froze (they ran and were not frozen already), in that order.
    frozen: Vec<String>,
    /// The unit was stopped with nothing of it running.
    stopped: bool,
}

impl std::fmt::Debug for Freeze {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Freeze")
            .field("unit", &self.unit)
            .field("frozen", &self.frozen)
            .finish_non_exhaustive()
    }
}

/// Freezes `unit` and its sessions unit (SPEC-458 EXP-001) for an export, or says why not;
/// `Err` is a refusal, nothing stays changed.
///
/// # Errors
/// The unit is unknown, the export runs inside one of them, the update helper `update_unit`
/// runs, a stopped unit still has processes, or systemd did not freeze one in [`FREEZE_WAIT`].
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
    // Sessions live in a unit of their own beside the server's when the host has one.
    let mut sessions = None;
    if let Some(name) = yulius_sandbox::units::sessions_unit(unit) {
        let found = show(&mut *host, &name)?;
        if found.load == "loaded" && runs(&found) {
            sessions = Some((name, found));
        }
    }
    let own = own_cgroup(&*host);
    let places = std::iter::once((unit, &state.cgroup)).chain(
        sessions
            .iter()
            .map(|(name, found)| (name.as_str(), &found.cgroup)),
    );
    for (name, cgroup) in places {
        if !cgroup.is_empty()
            && let Some(own) = &own
            && (own == cgroup || own.starts_with(&format!("{cgroup}/")))
        {
            return Err(format!(
                "this command runs inside {name} (a Yulius terminal?) and would freeze itself; \
                 run it over SSH on the host"
            ));
        }
    }
    let mut held = Freeze {
        host,
        unit: unit.to_owned(),
        update_unit: update_unit.to_owned(),
        covered: Vec::new(),
        frozen: Vec::new(),
        stopped: false,
    };
    held.helper_idle()?;
    let mut targets = Vec::new();
    if runs(&state) {
        held.covered.push(unit.to_owned());
        if state.freezer != "frozen" {
            targets.push(unit.to_owned());
        }
    } else {
        if group(&*held.host, &state.cgroup).populated {
            return Err(format!(
                "{unit} is stopped but its agents and terminals still run (KillMode=process); \
                 start it (`systemctl start {unit}`) so the export can freeze them"
            ));
        }
        held.stopped = sessions.is_none();
    }
    if let Some((name, found)) = sessions {
        held.covered.push(name.clone());
        if found.freezer != "frozen" {
            targets.push(name);
        }
    }
    for target in &targets {
        held.freeze_one(target)?;
    }
    // An update request written just before the freeze may have started the helper, which
    // would restart the unit under the export.
    if !held.frozen.is_empty()
        && let Err(why) = held.helper_idle()
    {
        held.thaw_now();
        return Err(why);
    }
    Ok(held)
}

/// What a freeze to stay did ([`freeze_to_stay`]).
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Stayed {
    /// The units it froze, in that order.
    pub frozen: Vec<String>,
    /// The units that could not be frozen, each with why.
    pub failed: Vec<(String, String)>,
}

/// Freezes the instance of the server's unit `unit` to stay frozen: for a move whose new
/// instance has taken over, so nothing of the old one may run beside it (SPEC-467 LFT-002).
/// Every unit of it that may run unfrozen is sent the freeze: the server's and its sessions unit
/// (SPEC-458), also one systemd says nothing about. A unit that is frozen already is left
/// alone, and so is a server's unit that has stopped (`inactive`, `failed`) and a sessions unit
/// the host does not have or that does not run: systemd freezes none of those.
///
/// Unlike [`freeze`] nothing here ever thaws, and the update helper is not asked: a freeze
/// that fails, or is not complete within [`FREEZE_WAIT`], is left as it stands for the caller
/// to report. Every unit is sent its freeze before any is waited for, and what was frozen is
/// told beside what was not, because the more of the old instance is frozen, and the sooner,
/// the better.
pub fn freeze_to_stay(host: &mut dyn Host, unit: &str) -> Stayed {
    let mut targets = Vec::new();
    match show(host, unit) {
        Ok(state)
            if state.freezer == "frozen"
                || matches!(state.active.as_str(), "inactive" | "failed") => {}
        // It runs, is on its way up or down, or systemd does not say.
        _ => targets.push(unit.to_owned()),
    }
    if let Some(sessions) = yulius_sandbox::units::sessions_unit(unit) {
        match sessions_state(host, unit) {
            Ok(None | Some(true)) => {}
            // It runs unfrozen, or systemd does not say.
            Ok(Some(false)) | Err(_) => targets.push(sessions),
        }
    }
    let mut did = Stayed::default();
    // Sent to all of them first: a freeze that hangs on one unit does not keep the other
    // running for the length of the wait.
    let mut sent = Vec::new();
    for target in targets {
        match host.systemctl(&["freeze", &target]) {
            Ok((0, _)) => sent.push(target),
            // Refused: also when an earlier freeze is still on its way, or has landed since.
            Ok((_, _)) if show(host, &target).is_ok_and(|state| state.freezer == "frozen") => {
                did.frozen.push(target);
            }
            Ok((code, _)) => {
                let why = format!("`systemctl freeze {target}` failed (exit {code})");
                did.failed.push((target, why));
            }
            Err(why) => {
                let why = format!("`systemctl freeze {target}` did not run ({why})");
                did.failed.push((target, why));
            }
        }
    }
    for target in sent {
        match frozen_to_stay(host, &target) {
            Ok(()) => did.frozen.push(target),
            Err(why) => did.failed.push((target, why)),
        }
    }
    did
}

/// The units of the instance of the server's unit `unit` that systemd says do not run while
/// processes of them still do, unfrozen: the server's unit, which keeps the agents, terminals
/// and sandbox engines it started when it is stopped (`KillMode=process`), and its sessions
/// unit (SPEC-458) when a stop of it did not end everything. systemd freezes nothing of such a
/// unit ("Unit is not active"), so [`freeze_to_stay`] leaves it alone; `systemctl kill
/// --kill-whom=all` still reaches what is left (SPEC-478 RSL-001).
///
/// A unit that runs is not looked at: whether it is frozen is systemd's to say. Processes the
/// kernel holds frozen do not count: they do not run.
///
/// # Errors
/// systemd did not say what a unit does. That is not "nothing is left".
pub fn left_running(host: &mut dyn Host, unit: &str) -> Result<Vec<String>, String> {
    let mut left = Vec::new();
    let names = std::iter::once(unit.to_owned()).chain(yulius_sandbox::units::sessions_unit(unit));
    for name in names {
        let state = show(host, &name)?;
        let held = group(host, &state.cgroup);
        if !runs(&state) && held.populated && !held.frozen {
            left.push(name);
        }
    }
    Ok(left)
}

/// Waits until systemd says that `target`, which was sent the freeze, is frozen; never thaws.
fn frozen_to_stay(host: &mut dyn Host, target: &str) -> Result<(), String> {
    let mut waited = Duration::ZERO;
    loop {
        let state = show(host, target).map_err(|why| {
            format!(
                "{target} was sent the freeze, and systemd does not say whether it took ({why})"
            )
        })?;
        if state.freezer == "frozen" {
            return Ok(());
        }
        if waited >= FREEZE_WAIT {
            return Err(format!(
                "{target} was not frozen within {} s (a process may hang in the kernel); the \
                 freeze is left in place",
                FREEZE_WAIT.as_secs()
            ));
        }
        host.sleep(POLL);
        waited += POLL;
    }
}

impl Freeze {
    /// Whether the export froze a unit (so it thaws it).
    #[must_use]
    pub fn ours(&self) -> bool {
        !self.frozen.is_empty()
    }

    /// The units the export froze, in the order a thaw takes them.
    #[must_use]
    pub fn frozen(&self) -> Vec<String> {
        self.frozen.iter().rev().cloned().collect()
    }

    /// The running units the freeze covers (frozen by the export or before it).
    #[must_use]
    pub fn covered(&self) -> &[String] {
        &self.covered
    }

    /// What the export holds, for the operator.
    #[must_use]
    pub fn describe(&self) -> String {
        let unit = &self.unit;
        if self.ours() {
            format!(
                "Frozen {}: every Yulius process is paused until the export ends. If this \
                 command is killed with SIGKILL, thaw {} with `sudo systemctl thaw {}`.",
                self.frozen.join(" and "),
                if self.frozen.len() == 1 { "it" } else { "them" },
                self.frozen().join(" ")
            )
        } else if self.stopped {
            format!("{unit} is stopped and nothing of it runs; nothing was frozen.")
        } else if self.covered.len() > 1 {
            format!(
                "{} were frozen before the export; they stay frozen after it.",
                self.covered.join(" and ")
            )
        } else {
            // With a stopped server that is its sessions unit, not the server's.
            let frozen = self.covered.first().unwrap_or(unit);
            format!("{frozen} was frozen before the export; it stays frozen after it.")
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

    /// Freezes `target` and waits until systemd says so; on a failure everything the export
    /// froze is thawed.
    fn freeze_one(&mut self, target: &str) -> Result<(), String> {
        let (code, _) = self.host.systemctl(&["freeze", target])?;
        self.frozen.push(target.to_owned());
        if code != 0 {
            self.thaw_now();
            return Err(format!(
                "`systemctl freeze {target}` failed (exit {code}); the export needs systemd 246 \
                 or later with cgroup v2"
            ));
        }
        let mut waited = Duration::ZERO;
        loop {
            if show(&mut *self.host, target)?.freezer == "frozen" {
                return Ok(());
            }
            if waited >= FREEZE_WAIT {
                self.thaw_now();
                return Err(format!(
                    "{target} did not freeze within {} s (a process may hang in the kernel); it \
                     was thawed and nothing was exported",
                    FREEZE_WAIT.as_secs()
                ));
            }
            self.host.sleep(POLL);
            waited += POLL;
        }
    }

    fn thaw_now(&mut self) -> Option<String> {
        let units = std::mem::take(&mut self.frozen);
        thaw(&mut *self.host, units.iter().rev())
    }

    /// Thaws what the export froze.
    ///
    /// # Errors
    /// A thaw failed; the message says to run it by hand.
    pub fn release(mut self) -> Result<(), String> {
        self.thaw_now().map_or(Ok(()), Err)
    }

    /// Leaves the units frozen without disabling the server's: a move whose new instance may
    /// still run must not thaw the old one (SPEC-460 MIG-004).
    pub fn leave_frozen(mut self) {
        self.frozen.clear();
    }

    /// For a move: leaves the units as they are (frozen) and disables the server's, so a reboot
    /// does not start the old instance beside the new one (its sessions unit starts only with
    /// it).
    ///
    /// # Errors
    /// The disable failed (the units stay frozen).
    pub fn keep_for_move(mut self) -> Result<(), String> {
        self.frozen.clear();
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

/// The sessions unit of the server's unit `unit` (SPEC-458): `Some(frozen)` when the host has
/// one that runs, `None` when it has none or it does not run.
///
/// # Errors
/// systemd did not say. That is not "no sessions unit": nothing may be started on it.
pub fn sessions_state(host: &mut dyn Host, unit: &str) -> Result<Option<bool>, String> {
    let Some(name) = yulius_sandbox::units::sessions_unit(unit) else {
        return Ok(None);
    };
    let found = show(host, &name)?;
    Ok((found.load == "loaded" && runs(&found)).then_some(found.freezer == "frozen"))
}

/// The units to thaw to bring the instance of the server's unit `unit` back, in that order: its
/// sessions unit first, when the host has one that runs, then `unit` (SPEC-458 EXP-001). A
/// server's unit that systemd says does not run is left out: systemd refuses to thaw it, and
/// there is nothing of it to thaw. A unit systemd says nothing about stays in: a thaw that
/// reports a failure is better than a unit left frozen.
pub fn thaw_units(host: &mut dyn Host, unit: &str) -> Vec<String> {
    let mut units = Vec::new();
    if !matches!(sessions_state(host, unit), Ok(None))
        && let Some(name) = yulius_sandbox::units::sessions_unit(unit)
    {
        units.push(name);
    }
    if show(host, unit).map_or(true, |found| runs(&found)) {
        units.push(unit.to_owned());
    }
    units
}

/// Thaws the instance of the server's unit `unit`, whoever froze it: every unit of
/// [`thaw_units`] is tried.
///
/// # Errors
/// A thaw failed; the message names the command to run by hand.
pub fn thaw_instance(host: &mut dyn Host, unit: &str) -> Result<(), String> {
    let units = thaw_units(host, unit);
    match thaw(host, units.iter()) {
        None => Ok(()),
        Some(why) => Err(format!("{why} (`systemctl thaw {}`)", units.join(" "))),
    }
}

/// Thaws `units` in that order without asking systemd what there is: for a caller that knows
/// them (what a freeze covered). Every one is tried.
///
/// # Errors
/// A thaw failed; the message says which, to run by hand.
pub fn thaw_each(host: &mut dyn Host, units: &[String]) -> Result<(), String> {
    thaw(host, units.iter()).map_or(Ok(()), Err)
}

/// Thaws `units` in order; every one is tried, and the failures say to run it by hand.
fn thaw<'a>(host: &mut dyn Host, units: impl Iterator<Item = &'a String>) -> Option<String> {
    let mut failures = Vec::new();
    for unit in units {
        match host.systemctl(&["thaw", unit]) {
            Ok((0, _)) => {}
            Ok((code, _)) => failures.push(format!(
                "`systemctl thaw {unit}` failed (exit {code}); run it by hand"
            )),
            Err(e) => failures.push(format!(
                "`systemctl thaw {unit}` did not run ({e}); run it by hand"
            )),
        }
    }
    (!failures.is_empty()).then(|| failures.join("; "))
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
/// [`on_signal`]: nothing to thaw (not frozen by the export, or the export ends it itself).
pub const LEAVE: u8 = 2;
/// [`on_signal`]: the freeze was refused; the refusal ends the command (exit 2).
pub const REFUSED: u8 = 3;
/// [`on_signal`]: a signal took the end: it thaws and exits, the export must not.
pub const CLAIMED: u8 = 4;
/// [`on_signal`]: the export took its end (a thaw, a release or a move): it exits itself.
pub const ENDING: u8 = 5;

/// What the signal thread does after [`on_signal`].
#[derive(Debug, PartialEq, Eq)]
pub enum Signal {
    /// Exit with 1, saying why a thaw failed when it did.
    Exit(Option<String>),
    /// Do nothing: the refusal of the freeze ends the command.
    LeaveToMain,
}

/// What a SIGINT, SIGTERM or SIGHUP does during an export, on its own thread so it works while
/// the export is blocked writing to a stalled pipe: waits (bounded) while the freeze is being
/// taken, so a thaw never comes before it, then, if the export froze units and has not claimed
/// its end ([`claim_end`]), claims it and thaws `frozen` (the export's [`Freeze::frozen`], set
/// before the state leaves [`ARMING`]).
pub fn on_signal(state: &AtomicU8, host: &mut dyn Host, frozen: &Mutex<Vec<String>>) -> Signal {
    let mut waited = Duration::ZERO;
    // The server's unit and its sessions unit are frozen one after the other.
    while state.load(Ordering::SeqCst) == ARMING && waited < FREEZE_WAIT * 3 {
        host.sleep(POLL);
        waited += POLL;
    }
    if matches!(state.load(Ordering::SeqCst), REFUSED | ENDING) {
        return Signal::LeaveToMain;
    }
    match state.compare_exchange(THAW, CLAIMED, Ordering::SeqCst, Ordering::SeqCst) {
        Ok(_) => {}
        // The export took its end meanwhile: its own thaw, release or disable must run.
        Err(ENDING) => return Signal::LeaveToMain,
        // Not the export's freeze, and the export may be blocked writing: just stop.
        Err(_) => return Signal::Exit(None),
    }
    let units = frozen.lock().map(|units| units.clone()).unwrap_or_default();
    Signal::Exit(thaw(host, units.iter()))
}

/// The export takes its own end (a thaw, or keeping the freeze for a move): from then on a
/// signal leaves the exit to it, so the process never ends before that `systemctl` runs.
/// `false` when a signal already took it: the export must leave the end (and the exit) to the
/// signal.
pub fn claim_end(state: &AtomicU8) -> bool {
    state
        .try_update(Ordering::SeqCst, Ordering::SeqCst, |now| {
            (now != CLAIMED).then_some(ENDING)
        })
        .is_ok()
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
