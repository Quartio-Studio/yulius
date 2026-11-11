//! The "bwrap namespaces" item (SPEC-456, ADR-0296): `/usr/bin/bwrap` can create the user
//! namespaces sandboxes run in.
//!
//! Ubuntu since 24.04 restricts unprivileged user namespaces through AppArmor
//! (`kernel.apparmor_restrict_unprivileged_userns`): bwrap needs a profile that allows `userns`.
//! Ubuntu 24.04 ships none (bwrap fails), Ubuntu 26.04 ships `bwrap-userns-restrict`, which
//! denies every capability to what bwrap starts (a sandbox cannot mount), Debian has no such
//! restriction. The item proves what the host does by starting bwrap as the service user, and
//! where that fails writes and loads a profile Yulius owns, disabling the shipped one by
//! AppArmor's `disable/` link. A profile a person wrote is never changed and never joined by
//! ours: two profiles for one program cancel each other.

use std::path::Path;

use yulius_sandbox::userns::{HOLDER_NAMESPACE, PROOF_COMMAND};

use super::{Cmd, Mode, Run};

const ITEM: &str = "bwrap namespaces";
/// AppArmor's profiles; the unit loads every file here at boot, except those linked in
/// `disable/`.
const DIR: &str = "/etc/apparmor.d";
/// Yulius's profile, by name and file.
const PROFILE_NAME: &str = "yulius-bwrap";
pub(super) const PROFILE_FILE: &str = "/etc/apparmor.d/yulius-bwrap";
/// Ubuntu 26.04's profile for bwrap (package `apparmor`).
const SHIPPED: &str = "bwrap-userns-restrict";
/// `Y` when the kernel enforces AppArmor.
const ENABLED: &str = "/sys/module/apparmor/parameters/enabled";
/// `1` when a program needs a `userns` rule to get a user namespace with capabilities.
const RESTRICTED: &str = "/proc/sys/kernel/apparmor_restrict_unprivileged_userns";
/// Absolute: the update helper's `PATH` has no `sbin`.
const RUNUSER: &str = "/usr/sbin/runuser";
const PARSER: &str = "/usr/sbin/apparmor_parser";

/// What Ubuntu ships for other programs that need user namespaces (`abi/4.0` is the first with
/// the `userns` rule; later parsers read it too).
pub(super) const PROFILE: &str = "\
# Written by `yulius host setup`: lets bubblewrap create the user namespaces Yulius
# sandboxes run in. Changes here are overwritten; add rules in local/yulius-bwrap.
abi <abi/4.0>,
include <tunables/global>

profile yulius-bwrap /usr/bin/bwrap flags=(unconfined) {
  userns,

  include if exists <local/yulius-bwrap>
}
";

/// Yulius's profile file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Ours {
    Missing,
    /// Not the text this version writes.
    Stale,
    Current,
}

/// What the files say.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Facts {
    /// The kernel enforces AppArmor.
    enabled: bool,
    /// User namespaces need a `userns` rule.
    restricted: bool,
    /// Ubuntu's profile for bwrap is there and not disabled.
    shipped: bool,
    ours: Ours,
    /// A link in `disable/` keeps ours from loading (a person's).
    ours_disabled: bool,
    /// Other enabled files with a profile for bwrap (a person's), by name.
    foreign: Vec<String>,
}

/// What starting bwrap said.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Proof {
    Works,
    Fails(String),
    /// Why it could not run.
    NotRun(&'static str),
}

/// What setup does, in this order.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct Plan {
    /// Write Yulius's profile.
    write: bool,
    /// Disable the shipped profile.
    disable: bool,
    /// A person's files to load again, as a boot does.
    reload: Vec<String>,
    /// Load Yulius's profile.
    load: bool,
}

/// Names AppArmor's unit skips in its directory (`apparmor_parser`'s own list).
fn skipped(name: &str) -> bool {
    const SUFFIXES: [&str; 12] = [
        "~",
        ".orig",
        ".rej",
        ".rpmnew",
        ".rpmsave",
        ".dpkg-new",
        ".dpkg-old",
        ".dpkg-dist",
        ".dpkg-bak",
        ".dpkg-remove",
        ".pacsave",
        ".pacnew",
    ];
    name.starts_with('.') || SUFFIXES.iter().any(|s| name.ends_with(s))
}

/// Whether a profile in `text` attaches to `/usr/bin/bwrap`: a header line (`profile name /path
/// flags=(…) {`, or `/path {`) whose path is bwrap's, plain or as the alternations Ubuntu's own
/// profiles spell it. A rule that only names bwrap (`/usr/bin/bwrap mr,`) is none.
fn attaches_bwrap(text: &str) -> bool {
    const PATHS: [&str; 4] = [
        "/usr/bin/bwrap",
        "/{usr/,}bin/bwrap",
        "/{,usr/}bin/bwrap",
        "@{bin}/bwrap",
    ];
    text.lines()
        .map(str::trim)
        .filter(|l| !l.starts_with('#') && l.ends_with('{'))
        .any(|l| {
            l.split_whitespace()
                .take_while(|t| !t.starts_with("flags=") && *t != "{")
                .any(|t| PATHS.contains(&t.trim_matches('"')))
        })
}

impl Plan {
    /// From the files and the first proof. Nothing when AppArmor is off or a person disabled
    /// Yulius's profile: no rule of ours can help there.
    fn of(facts: &Facts, first: &Proof) -> Self {
        if !facts.enabled {
            return Self::default();
        }
        let fails = matches!(first, Proof::Fails(_));
        let disable = facts.shipped;
        if !facts.foreign.is_empty() {
            let reload = if disable || fails {
                facts.foreign.clone()
            } else {
                Vec::new()
            };
            return Self {
                disable,
                reload,
                ..Self::default()
            };
        }
        if facts.ours_disabled {
            return Self {
                disable,
                ..Self::default()
            };
        }
        // The proof speaks for the host as it is: not once the shipped profile is gone, and
        // not when it could not run.
        let needed = facts.restricted && (fails || disable || matches!(first, Proof::NotRun(_)));
        let write = facts.ours != Ours::Current && (needed || facts.ours == Ours::Stale);
        let load = write || (needed && (fails || disable));
        Self {
            write,
            disable,
            reload: Vec::new(),
            load,
        }
    }

    fn is_empty(&self) -> bool {
        *self == Self::default()
    }

    /// The steps, as done (`done`) or as an order.
    fn steps(&self, done: bool) -> String {
        let mut steps = Vec::new();
        if self.disable {
            steps.push(format!(
                "{} Ubuntu's AppArmor profile {SHIPPED} (a link in {DIR}/disable; its file is \
                 untouched)",
                if done { "disabled" } else { "disable" }
            ));
        }
        for name in &self.reload {
            steps.push(format!(
                "{} {DIR}/{name} again",
                if done { "loaded" } else { "load" }
            ));
        }
        match (self.write, self.load, done) {
            (true, _, true) => steps.push(format!("wrote and loaded {PROFILE_FILE}")),
            (true, _, false) => steps.push(format!("write and load {PROFILE_FILE}")),
            (false, true, true) => steps.push(format!("loaded {PROFILE_FILE}")),
            (false, true, false) => steps.push(format!("load {PROFILE_FILE}")),
            (false, false, _) => {}
        }
        steps.join(", ")
    }
}

impl Facts {
    /// Which rule lets bwrap create its namespaces.
    fn rule(&self) -> String {
        if !self.enabled {
            "AppArmor is not active here".to_owned()
        } else if !self.foreign.is_empty() {
            format!(
                "allowed by {} (not written by Yulius, left as it is)",
                self.foreign
                    .iter()
                    .map(|name| format!("{DIR}/{name}"))
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        } else if !self.restricted {
            "AppArmor does not restrict user namespaces here".to_owned()
        } else if self.ours == Ours::Current && !self.ours_disabled {
            format!("allowed by {PROFILE_FILE}")
        } else {
            "allowed by a rule Yulius did not write".to_owned()
        }
    }
}

impl Run<'_> {
    /// bwrap can create a sandbox's user namespace: proven by starting one, fixed by Yulius's
    /// AppArmor profile where the host needs it (SPEC-456). A check and a dry run only report.
    pub(super) fn bwrap_namespaces(&mut self) {
        let facts = self.apparmor_facts();
        let first = self.prove_namespace();
        let plan = Plan::of(&facts, &first);
        let user = self.service_name();
        if plan.is_empty() {
            return match first {
                Proof::Works => self.ok(
                    ITEM,
                    format!("bwrap starts a user namespace as {user}: {}", facts.rule()),
                ),
                Proof::NotRun(why) => {
                    self.ok(ITEM, format!("{}; not proven: {why}", facts.rule()));
                }
                Proof::Fails(why) => {
                    let cause = self.cause(&facts);
                    self.failed(
                        ITEM,
                        format!("bwrap cannot start a user namespace as {user}: {why}{cause}"),
                    );
                }
            };
        }
        match self.mode {
            Mode::Setup => {}
            Mode::DryRun => return self.changed(ITEM, "", &plan.steps(false)),
            Mode::Check => {
                let state = match &first {
                    Proof::Fails(why) => {
                        format!("bwrap cannot start a user namespace as {user} ({why})")
                    }
                    Proof::Works | Proof::NotRun(_) => {
                        "the AppArmor rules for bwrap are not what setup leaves".to_owned()
                    }
                };
                return self
                    .needs_setup(ITEM, &format!("{state}: setup would {}", plan.steps(false)));
            }
        }
        if let Err(why) = self.apply(&plan) {
            return self.failed(ITEM, why);
        }
        let done = plan.steps(true);
        match self.prove_namespace() {
            Proof::Works => self.changed(
                ITEM,
                &format!("{done}: bwrap starts a user namespace as {user}"),
                "",
            ),
            Proof::NotRun(why) => self.changed(ITEM, &format!("{done}; not proven: {why}"), ""),
            Proof::Fails(why) if !facts.foreign.is_empty() => {
                let files: Vec<String> = facts
                    .foreign
                    .iter()
                    .map(|name| format!("{DIR}/{name}"))
                    .collect();
                let disable: Vec<String> = files
                    .iter()
                    .map(|file| {
                        format!("sudo ln -s {file} {DIR}/disable/ && sudo {PARSER} -R {file}")
                    })
                    .collect();
                self.failed(
                    ITEM,
                    format!(
                        "bwrap cannot start a user namespace as {user} with {} ({why}); Yulius \
                         does not change a profile it did not write: disable it (`{}`), then run \
                         `sudo yulius host setup` again",
                        files.join(", "),
                        disable.join(" && ")
                    ),
                );
            }
            Proof::Fails(why) => {
                let cause = self.cause(&facts);
                self.failed(
                    ITEM,
                    format!("{done}, but bwrap still cannot start a user namespace as {user}: {why}{cause}"),
                );
            }
        }
    }

    fn service_name(&self) -> String {
        self.service
            .as_ref()
            .map_or_else(|| self.opts.user.clone(), |s| s.name.clone())
    }

    fn apparmor_facts(&self) -> Facts {
        let host = &*self.host;
        let says = |path: &str, value: &str| {
            host.read(Path::new(path))
                .is_some_and(|text| text.trim() == value)
        };
        let disabled = |name: &str| {
            host.stat(Path::new(&format!("{DIR}/disable/{name}")))
                .is_some()
        };
        let ours = match host.read(Path::new(PROFILE_FILE)) {
            None => Ours::Missing,
            Some(text) if text == PROFILE => Ours::Current,
            Some(_) => Ours::Stale,
        };
        let foreign = host
            .list(Path::new(DIR))
            .into_iter()
            .filter(|name| name != PROFILE_NAME && name != SHIPPED)
            .filter(|name| !skipped(name) && !disabled(name))
            .filter(|name| {
                host.read(Path::new(&format!("{DIR}/{name}")))
                    .is_some_and(|text| attaches_bwrap(&text))
            })
            .collect();
        Facts {
            enabled: says(ENABLED, "Y"),
            restricted: says(RESTRICTED, "1"),
            shipped: host.is_file(Path::new(&format!("{DIR}/{SHIPPED}"))) && !disabled(SHIPPED),
            ours,
            ours_disabled: disabled(PROFILE_NAME),
            foreign,
        }
    }

    /// Starts bwrap as the service user in the namespace a sandbox's holder runs in, with a
    /// child that mounts. Root maps only itself here (no `newuidmap`, which the update helper's
    /// `NoNewPrivileges` forbids); the doctor proves the mapped ids as the service user.
    fn prove_namespace(&mut self) -> Proof {
        let Some(user) = self
            .service
            .as_ref()
            .filter(|s| s.uid.is_some())
            .map(|s| s.name.clone())
        else {
            return Proof::NotRun("the service user does not exist yet");
        };
        if !self.installed("bubblewrap") {
            return Proof::NotRun("bubblewrap is not installed yet");
        }
        let mut cmd = Cmd::new(&[
            RUNUSER,
            "-u",
            &user,
            "--",
            "bwrap",
            "--unshare-user",
            "--uid",
            "0",
            "--gid",
            "0",
        ]);
        for arg in HOLDER_NAMESPACE.iter().chain(&PROOF_COMMAND) {
            cmd = cmd.arg(*arg);
        }
        let out = self.run_cmd(&cmd);
        if out.ok() {
            Proof::Works
        } else {
            Proof::Fails(out.why())
        }
    }

    /// What else keeps bwrap from a namespace, where the host says so.
    fn cause(&self, facts: &Facts) -> String {
        let zero = |path: &str| {
            self.host
                .read(Path::new(path))
                .is_some_and(|text| text.trim() == "0")
        };
        let mut causes = Vec::new();
        if zero("/proc/sys/kernel/unprivileged_userns_clone") {
            causes.push(
                "kernel.unprivileged_userns_clone is 0 (unprivileged user namespaces are off: \
                 set it to 1 in /etc/sysctl.d)"
                    .to_owned(),
            );
        }
        if zero("/proc/sys/user/max_user_namespaces") {
            causes.push(
                "user.max_user_namespaces is 0 (no user namespaces at all: raise it in \
                 /etc/sysctl.d)"
                    .to_owned(),
            );
        }
        if facts.enabled && facts.ours_disabled {
            causes.push(format!(
                "{DIR}/disable/{PROFILE_NAME} keeps Yulius's profile from loading (remove that \
                 link, then run `sudo yulius host setup` again)"
            ));
        }
        if causes.is_empty() {
            String::new()
        } else {
            format!("; {}", causes.join("; "))
        }
    }

    fn must(&mut self, argv: &[&str]) -> Result<(), String> {
        let out = self.run_cmd(&Cmd::new(argv));
        if out.ok() {
            Ok(())
        } else {
            Err(format!("`{}` failed: {}", argv.join(" "), out.why()))
        }
    }

    /// Ours is written before anything is unloaded, so a failed write leaves the host as it
    /// was.
    fn apply(&mut self, plan: &Plan) -> Result<(), String> {
        if plan.write {
            self.host
                .write(Path::new(PROFILE_FILE), PROFILE, 0o644)
                .map_err(|why| format!("cannot write {PROFILE_FILE}: {why}"))?;
        }
        if plan.disable {
            let file = format!("{DIR}/{SHIPPED}");
            self.must(&["ln", "-s", &file, &format!("{DIR}/disable/{SHIPPED}")])?;
            // Exit 254 when it was not loaded; the proof judges what is.
            let _ = self.run_cmd(&Cmd::new(&[PARSER, "-R", &file]));
        }
        for name in &plan.reload {
            self.must(&[PARSER, "-r", &format!("{DIR}/{name}")])?;
        }
        if plan.load {
            let out = self.run_cmd(&Cmd::new(&[PARSER, "-r", PROFILE_FILE]));
            if !out.ok() {
                // A file AppArmor cannot read would fail its unit at every boot.
                let removed =
                    plan.write && self.run_cmd(&Cmd::new(&["rm", "-f", PROFILE_FILE])).ok();
                return Err(format!(
                    "unsupported: this host's AppArmor cannot load Yulius's profile ({}){}",
                    out.why(),
                    if removed {
                        format!("; {PROFILE_FILE} was removed again")
                    } else {
                        String::new()
                    }
                ));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{Facts, Ours, Plan, Proof, attaches_bwrap, skipped};

    #[test]
    fn a_profile_header_for_bwrap_is_found_and_a_rule_is_not() {
        for header in [
            "profile bwrap /usr/bin/bwrap flags=(unconfined) {",
            "  profile bwrap \"/usr/bin/bwrap\" {",
            "/usr/bin/bwrap {",
            "profile b /{usr/,}bin/bwrap flags=(attach_disconnected) {",
            "profile b @{bin}/bwrap {",
        ] {
            assert!(attaches_bwrap(&format!(
                "abi <abi/4.0>,\n{header}\n  userns,\n}}\n"
            )));
        }
        for other in [
            // Ubuntu 26.04's glycin profile: a rule and a transition, no attachment.
            "profile :glycin:bwrap flags=(attach_disconnected) {\n  /{,usr/}bin/bwrap mr,\n}\n",
            "# profile bwrap /usr/bin/bwrap {\n",
            "profile flatpak /usr/bin/flatpak flags=(unconfined) {\n  userns,\n}\n",
            "profile x /usr/bin/x {\n  /usr/bin/bwrap Px -> bwrap,\n}\n",
        ] {
            assert!(!attaches_bwrap(other), "{other}");
        }
    }

    #[test]
    fn backups_and_hidden_files_are_not_profiles() {
        assert!(skipped("bwrap.dpkg-old") && skipped("bwrap~") && skipped(".bwrap.swp"));
        assert!(!skipped("bwrap") && !skipped("usr.bin.bwrap"));
    }

    fn facts() -> Facts {
        Facts {
            enabled: true,
            restricted: true,
            shipped: false,
            ours: Ours::Missing,
            ours_disabled: false,
            foreign: Vec::new(),
        }
    }

    /// The plan's rules (SPEC-456, Design): a host that works is left alone unless a shipped
    /// profile is enabled; a person's profile is never joined by ours.
    #[test]
    fn the_plan_follows_the_proof() {
        let fails = Proof::Fails("denied".into());
        let write_and_load = Plan {
            write: true,
            load: true,
            ..Plan::default()
        };
        assert_eq!(Plan::of(&facts(), &fails), write_and_load);
        assert_eq!(Plan::of(&facts(), &Proof::NotRun("x")), write_and_load);
        assert!(Plan::of(&facts(), &Proof::Works).is_empty());
        let off = Facts {
            enabled: false,
            shipped: true,
            ..facts()
        };
        assert!(Plan::of(&off, &fails).is_empty());
        let unrestricted = Facts {
            restricted: false,
            ..facts()
        };
        assert!(Plan::of(&unrestricted, &fails).is_empty());
        let shipped = Facts {
            shipped: true,
            ours: Ours::Current,
            ..facts()
        };
        assert_eq!(
            Plan::of(&shipped, &Proof::Works),
            Plan {
                disable: true,
                load: true,
                ..Plan::default()
            }
        );
        let theirs = Facts {
            foreign: vec!["bwrap".into()],
            ..facts()
        };
        assert!(Plan::of(&theirs, &Proof::Works).is_empty());
        assert_eq!(
            Plan::of(&theirs, &fails),
            Plan {
                reload: vec!["bwrap".into()],
                ..Plan::default()
            }
        );
        let disabled = Facts {
            ours: Ours::Current,
            ours_disabled: true,
            ..facts()
        };
        assert!(Plan::of(&disabled, &fails).is_empty());
        let stale = Facts {
            ours: Ours::Stale,
            ..facts()
        };
        assert_eq!(Plan::of(&stale, &Proof::Works), write_and_load);
    }
}
