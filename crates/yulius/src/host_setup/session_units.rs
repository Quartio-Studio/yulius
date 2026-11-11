//! The units that keep sessions out of the server's own group (ADR-0298, SPEC-458).
//!
//! systemd 258 and later start a unit by cloning their executor into the unit's own cgroup, which
//! the kernel refuses while that group passes controllers to children that hold processes: a
//! server whose sandboxes live in its own unit cannot start again while one session survives.
//! `yulius host setup` therefore writes
//!
//! - the sessions unit `<stem>-sandboxes.service`: one keeper process whose delegated group
//!   holds every sandbox, in a slice it shares with the server;
//! - the adopt unit `<stem>-adopt.service`: a root oneshot before every start of the server that
//!   moves what an earlier layout left in the server's unit (`yulius host adopt-sessions`);
//! - the server unit's second drop-in, which puts it in the slice and pulls both in.

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Duration;

use yulius_sandbox::{adopt, cgroup, units};

use super::{Cmd, Mode, Run, service_name_ok, unit_safe};
use crate::restart_safety::unit_groups;

/// The name of the server unit's second drop-in.
pub const DROP_IN_NAME: &str = "20-yulius-sessions.conf";
/// How long an ended Docker engine may take to leave (SPEC-458 LAY-005).
const ENGINE_WAIT: Duration = Duration::from_secs(10);
/// How long systemd may take to let go of the server unit's emptied old group.
const RELEASE_WAIT: Duration = Duration::from_secs(5);

/// The sessions unit of the server unit `service`, delegated to `user` in `slice`. It keeps the
/// default `KillMode`: a unit that kept processes after its stop could not start again on systemd
/// 258. Its keeper is root's (`+`), while `User=` still names who the group is delegated to: the
/// service user, whose kill would end every session with it, cannot signal it.
#[must_use]
pub fn sessions_unit_text(service: &str, user: &str, slice: &str) -> String {
    format!(
        "# Written by `yulius host setup` (docs/operations.md §3, ADR-0298): the group every \
         session and\n# sandbox of {service} lives in. Restarting {service} never touches it; \
         stopping this unit\n# ends every session. Its one process is root's, so nothing of \
         {user} can end it.\n\
         [Unit]\nDescription=Yulius sessions and sandboxes ({service})\n\n\
         [Service]\nSlice={slice}\nUser={user}\n\
         ExecStartPre=+/usr/bin/chown {user} /sys/fs/cgroup/{slice}/cgroup.procs\n\
         ExecStart=+/usr/bin/sleep infinity\nDelegate=yes\nDelegateSubgroup=keeper\n\
         OOMScoreAdjust=-1000\n"
    )
}

/// The adopt unit of `service`: `exe host adopt-sessions` as root, after the sessions unit.
#[must_use]
pub fn adopt_unit_text(service: &str, sessions: &str, exe: &Path) -> String {
    format!(
        "# Written by `yulius host setup` (docs/operations.md §3, ADR-0298): before {service} \
         starts, sessions\n# an earlier layout left in its own group move to {sessions}.\n\
         [Unit]\nDescription=Move Yulius sessions out of {service}\nRequires={sessions}\n\
         After={sessions}\n\n\
         [Service]\nType=oneshot\nExecStart={exe} host adopt-sessions --service {service}\n",
        exe = exe.display()
    )
}

/// The server unit's second drop-in.
#[must_use]
pub fn drop_in_text(sessions: &str, adopt: &str, slice: &str) -> String {
    format!(
        "# Written by `yulius host setup` (docs/operations.md §3, ADR-0298): sessions live in \
         {sessions},\n# so a restart of this unit keeps them on every systemd.\n\
         [Unit]\nWants={sessions} {adopt}\nAfter={sessions} {adopt}\n\n\
         [Service]\nSlice={slice}\n"
    )
}

impl Run<'_> {
    /// SPEC-458 HST-001: the sessions unit, the adopt unit and the drop-in. Unsafe: a check only
    /// reports it (the update helper cannot write unit files). A running server is not
    /// restarted: its next start moves the sessions.
    pub(super) fn session_units(&mut self) {
        const ITEM: &str = "sessions unit";
        let service = self.opts.service.clone();
        let (Some(sessions), Some(adopt), Some(slice)) = (
            units::sessions_unit(&service),
            units::adopt_unit(&service),
            units::slice(&service),
        ) else {
            return;
        };
        // Without the server's unit or its user their own items already said what is missing.
        let Some(user) = self.service.as_ref().map(|s| s.name.clone()) else {
            return;
        };
        if !self.unit.loaded {
            return;
        }
        let exe = self
            .unit
            .exec
            .clone()
            .unwrap_or_else(|| self.opts.exe.clone());
        if !unit_safe(&user) || !unit_safe(&exe.display().to_string()) {
            return self.failed(
                ITEM,
                "the user or the binary's path has characters a unit file cannot hold",
            );
        }
        let system = Path::new("/etc/systemd/system");
        let wanted: [(PathBuf, String); 3] = [
            (
                system.join(&sessions),
                sessions_unit_text(&service, &user, &slice),
            ),
            (
                system.join(&adopt),
                adopt_unit_text(&service, &sessions, &exe),
            ),
            (
                system.join(format!("{service}.d")).join(DROP_IN_NAME),
                drop_in_text(&sessions, &adopt, &slice),
            ),
        ];
        let stale: Vec<&(PathBuf, String)> = wanted
            .iter()
            .filter(|(path, text)| self.host.read(path).as_deref() != Some(text.as_str()))
            .collect();
        if stale.is_empty() {
            return self.ok(
                ITEM,
                format!("sessions live in {sessions}: a restart of {service} keeps them"),
            );
        }
        let files = stale
            .iter()
            .map(|(path, _)| path.display().to_string())
            .collect::<Vec<_>>()
            .join(", ");
        if self.mode == Mode::Check {
            // Files that are all there but differ are another build's or the operator's own: the
            // layout may well be in use, so only the difference is said.
            let missing = stale.iter().any(|(path, _)| self.host.read(path).is_none());
            let why = if missing {
                format!(
                    "sessions live in {service}'s own group, where systemd 258 and later cannot \
                     start the unit again while one runs"
                )
            } else {
                format!("{files} differs from what this version writes")
            };
            return self.needs_setup(ITEM, &why);
        }
        if self.may(false) {
            for (path, text) in stale {
                if let Err(why) = self.host.write(path, text, 0o644) {
                    return self.failed(ITEM, format!("cannot write {}: {why}", path.display()));
                }
            }
            for cmd in [
                Cmd::new(&["systemctl", "daemon-reload"]),
                Cmd::new(&["systemctl", "start", &sessions]),
            ] {
                let out = self.run_cmd(&cmd);
                if !out.ok() {
                    return self.failed(
                        ITEM,
                        format!("`{}` failed: {}", cmd.argv.join(" "), out.why()),
                    );
                }
            }
        }
        let when = if self.unit.active {
            format!("the next restart of {service} moves its sessions there and keeps them")
        } else {
            format!("{service} starts with its sessions there")
        };
        self.changed(
            ITEM,
            &format!("wrote {files} and started {sessions}; {when}"),
            &format!("write {files} and start {sessions}; {when}"),
        );
    }
}

/// What systemd names as the group of `service` (`ControlGroup`); `None` when it cannot be asked.
fn control_group(service: &str) -> Option<String> {
    let out = std::process::Command::new("systemctl")
        .args(["show", "--property=ControlGroup", "--value", "--", service])
        .stdin(std::process::Stdio::null())
        .output()
        .ok()?;
    Some(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// `yulius host adopt-sessions --service <unit>` (SPEC-458 LAY-004): as root, before the server
/// starts, moves the sessions an earlier layout left in the server's unit into its sessions unit.
/// Nothing to move is success; a group that keeps processes fails the unit, which the server's
/// `Wants=` survives.
#[must_use]
pub fn adopt_main(service: &str) -> ExitCode {
    let (true, Some(sessions), Some(slice)) = (
        service_name_ok(service),
        units::sessions_unit(service),
        units::slice(service),
    ) else {
        eprintln!("yulius host adopt-sessions: {service:?} is not a unit name");
        return ExitCode::from(2);
    };
    let root = Path::new(cgroup::CGROUP_ROOT);
    let sessions_dir = root.join(&slice).join(&sessions);
    let wanted = root.join(&slice).join(service);
    let old = unit_groups(root, service, control_group(service).as_deref());
    let shown = || control_group(service);
    let mut failed = false;
    for unit in old {
        match adopt::adopt(&unit, &sessions_dir, unit == wanted, ENGINE_WAIT) {
            Ok(done) if done.nothing() => {
                // What an earlier move left of the old group, once systemd names it no more.
                if unit != wanted {
                    let _ = adopt::let_go(&unit, root, &shown, Duration::ZERO);
                }
            }
            Ok(done) => {
                println!(
                    "moved {} processes in {} groups from {} to {} ({} Docker engines ended: \
                     they start again on demand)",
                    done.processes,
                    done.groups,
                    unit.display(),
                    sessions_dir.display(),
                    done.engines
                );
                for line in &done.left {
                    eprintln!("yulius host adopt-sessions: {line}");
                }
                failed |= !done.left.is_empty();
                // The server starts where systemd still holds its group: wait for it to let go.
                if unit != wanted && !adopt::let_go(&unit, root, &shown, RELEASE_WAIT) {
                    eprintln!(
                        "yulius host adopt-sessions: systemd still holds {} for {service}, which \
                         starts in it once more and moves at its next restart",
                        unit.display()
                    );
                    failed = true;
                }
            }
            Err(error) => {
                eprintln!(
                    "yulius host adopt-sessions: cannot move the sessions of {} to {}: {error}",
                    unit.display(),
                    sessions_dir.display()
                );
                failed = true;
            }
        }
    }
    if failed {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}
