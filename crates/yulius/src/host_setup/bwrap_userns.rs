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

pub(super) const ITEM: &str = "bwrap namespaces";
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
    /// It is there and disabled by its link, which alone does not unload it.
    shipped_off: bool,
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
    /// Unload the shipped profile a link already disables.
    unload: bool,
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

/// Whether a profile in `text` attaches to `/usr/bin/bwrap`: a header at the top level
/// (`profile name /path flags=(…) {`, or `/path {`) whose path is bwrap's, plain or as the
/// alternations Ubuntu's own profiles spell it. The header's brace may open the profile on the
/// same line, before a comment or on the next line (with its `flags = (…)`), and the path may
/// be a variable set to bwrap's (`@{prog} = /usr/bin/bwrap`, with or without the spaces). A
/// rule that only names bwrap (`/usr/bin/bwrap mr,`) and a profile inside another one are none.
fn attaches_bwrap(text: &str) -> bool {
    const PATHS: [&str; 4] = [
        "/usr/bin/bwrap",
        "/{usr/,}bin/bwrap",
        "/{,usr/}bin/bwrap",
        "@{bin}/bwrap",
    ];
    // Blocks open, and the words since the last rule or block at the top level.
    let mut depth = 0_usize;
    let mut header: Vec<&str> = Vec::new();
    let mut in_list = false;
    // The variables that stand for bwrap's path.
    let mut vars: Vec<&str> = Vec::new();
    for line in text.lines() {
        let words: Vec<&str> = line
            .split_whitespace()
            .take_while(|w| !w.starts_with('#'))
            .collect();
        // A variable's line ends without a comma and is no header.
        if let Some((var, values)) = assignment(line).filter(|_| depth == 0) {
            let mut values = values
                .split_whitespace()
                .take_while(|w| !w.starts_with('#'));
            if values.any(|w| PATHS.contains(&w.trim_matches('"'))) {
                vars.push(var);
            }
            continue;
        }
        // A brace that starts a word opens a block (`{userns,}`); an alternation's
        // (`/{usr/,}bin`, `@{bin}`) is inside its word and closes there.
        let words = words
            .into_iter()
            .flat_map(|w| w.strip_prefix('{').map_or([w, ""], |rest| ["{", rest]))
            .filter(|w| !w.is_empty());
        for word in words {
            let opens = word.matches('{').count();
            let closes = word.matches('}').count();
            if opens > closes {
                header.push(word.trim_end_matches('{'));
                let attaches = header
                    .iter()
                    .take_while(|w| !w.starts_with("flags="))
                    .map(|w| w.trim_matches('"'))
                    .any(|w| PATHS.contains(&w) || vars.contains(&w));
                if depth == 0 && attaches {
                    return true;
                }
                depth += 1;
                header.clear();
            } else if closes > opens {
                depth = depth.saturating_sub(1);
                header.clear();
            } else if depth == 0 {
                // `flags=(a, b)`: a comma inside a list ends no rule.
                in_list = (in_list || word.contains('(')) && !word.contains(')');
                if word.ends_with(',') && !in_list {
                    header.clear();
                } else {
                    header.push(word);
                }
            }
        }
    }
    false
}

/// A variable's line (`@{name} = values`, `@{name}+=values`, `${flag} = true`): its name and
/// values. A header that starts with a variable (`@{bin}/bwrap flags=(…) {`) is none.
fn assignment(line: &str) -> Option<(&str, &str)> {
    let line = line.trim_start();
    if !line.starts_with(['@', '$']) {
        return None;
    }
    let (name, rest) = line.split_at(line.find('}')? + 1);
    let rest = rest.trim_start();
    let values = rest.strip_prefix("+=").or_else(|| rest.strip_prefix('='))?;
    Some((name, values))
}

/// What a person does about a profile of theirs that keeps bwrap from working.
fn theirs(files: &[String]) -> String {
    let disable: Vec<String> = files
        .iter()
        .map(|file| format!("sudo ln -s {file} {DIR}/disable/ && sudo {PARSER} -R {file}"))
        .collect();
    format!(
        "Yulius does not change a profile it did not write: disable it (`{}`), then run `sudo \
         yulius host setup` again",
        disable.join(" && ")
    )
}

impl Plan {
    /// From the files and the first proof. Nothing when AppArmor is off or a person disabled
    /// Yulius's profile: no rule of ours can help there, and disabling the shipped profile for
    /// it would only change how bwrap fails.
    fn of(facts: &Facts, first: &Proof) -> Self {
        if !facts.enabled {
            return Self::default();
        }
        let fails = matches!(first, Proof::Fails(_));
        let disable = facts.shipped;
        // Whoever made the link may not have unloaded it.
        let unload = facts.shipped_off && fails;
        if !facts.foreign.is_empty() {
            let reload = if disable || fails {
                facts.foreign.clone()
            } else {
                Vec::new()
            };
            return Self {
                disable,
                unload,
                reload,
                ..Self::default()
            };
        }
        if facts.ours_disabled {
            return Self::default();
        }
        // The proof speaks for the host as it is: not once the shipped profile is gone, and
        // not when it could not run.
        let needed = facts.restricted && (fails || disable || matches!(first, Proof::NotRun(_)));
        let write = facts.ours != Ours::Current && (needed || facts.ours == Ours::Stale);
        let load = write || (needed && (fails || disable));
        Self {
            write,
            disable,
            unload,
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
        if self.unload {
            steps.push(format!(
                "{} sure Ubuntu's disabled AppArmor profile {SHIPPED} is not loaded",
                if done { "made" } else { "make" }
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
    /// Which rule lets bwrap create its namespaces (`proven`: it does).
    fn rule(&self, proven: bool) -> String {
        if !self.enabled {
            "AppArmor is not active here".to_owned()
        } else if !self.foreign.is_empty() {
            format!(
                "allowed by {} (not written by Yulius, left as it is)",
                self.foreign_files().join(", ")
            )
        } else if !self.restricted {
            "AppArmor does not restrict user namespaces here".to_owned()
        } else if self.ours == Ours::Current && !self.ours_disabled {
            format!("allowed by {PROFILE_FILE}")
        } else if self.ours_disabled && !proven {
            format!("{DIR}/disable/{PROFILE_NAME} keeps Yulius's profile from loading")
        } else {
            "allowed by a rule Yulius did not write".to_owned()
        }
    }

    /// The files of a person's profiles for bwrap.
    fn foreign_files(&self) -> Vec<String> {
        self.foreign
            .iter()
            .map(|name| format!("{DIR}/{name}"))
            .collect()
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
                    format!(
                        "bwrap starts a user namespace as {user}: {}",
                        facts.rule(true)
                    ),
                ),
                Proof::NotRun(why) => {
                    self.ok(ITEM, format!("{}; not proven: {why}", facts.rule(false)));
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
                    // Setup's steps alone do not let bwrap in past a kernel limit.
                    Proof::Fails(why) => {
                        let limits: String =
                            self.limits().iter().map(|l| format!("; {l}")).collect();
                        format!("bwrap cannot start a user namespace as {user} ({why}{limits})")
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
            // A limit keeps bwrap out whatever the profile says: theirs is not to blame.
            Proof::Fails(why) if !facts.foreign.is_empty() && self.limits().is_empty() => {
                let files = facts.foreign_files();
                self.failed(
                    ITEM,
                    format!(
                        "{done}, but bwrap cannot start a user namespace as {user} with {} \
                         ({why}); {}",
                        files.join(", "),
                        theirs(&files)
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
        let shipped = host.is_file(Path::new(&format!("{DIR}/{SHIPPED}")));
        Facts {
            enabled: says(ENABLED, "Y"),
            restricted: says(RESTRICTED, "1"),
            shipped: shipped && !disabled(SHIPPED),
            shipped_off: shipped && disabled(SHIPPED),
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
            return Proof::NotRun("the service user is not set up yet");
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
        match self.host.run(&cmd) {
            Ok(out) if out.ok() => Proof::Works,
            Ok(out) => Proof::Fails(out.why()),
            Err(_) => Proof::NotRun("/usr/sbin/runuser cannot be started"),
        }
    }

    /// What else keeps bwrap from a namespace, where the host says so.
    fn cause(&self, facts: &Facts) -> String {
        let mut causes = self.limits();
        // Beside a person's profile Yulius's is never loaded: its link is no cause then.
        if facts.enabled && facts.ours_disabled && facts.foreign.is_empty() {
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

    /// The kernel's own limits on user namespaces, where one allows none.
    fn limits(&self) -> Vec<String> {
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
        causes
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
    /// was; a later step that fails is undone and the line says so.
    fn apply(&mut self, plan: &Plan) -> Result<(), String> {
        let earlier = self.host.read(Path::new(PROFILE_FILE));
        if plan.write {
            self.host
                .write(Path::new(PROFILE_FILE), PROFILE, 0o644)
                .map_err(|why| format!("cannot write {PROFILE_FILE}: {why}"))?;
        }
        let mut linked = false;
        self.apply_loaded(plan, &mut linked).map_err(|why| {
            let mut undone = String::new();
            // A file AppArmor does not hold, or cannot read, would change or fail its next
            // boot.
            if plan.write {
                let (back, done, not) = match &earlier {
                    // AppArmor still holds what this text loaded.
                    Some(text) => (
                        self.host
                            .write(Path::new(PROFILE_FILE), text, 0o644)
                            .is_ok(),
                        "has its earlier text again",
                        "keeps the text AppArmor did not load",
                    ),
                    None => (
                        self.run_cmd(&Cmd::new(&["rm", "-f", PROFILE_FILE])).ok(),
                        "was removed again",
                        "could not be removed again",
                    ),
                };
                undone += &format!("; {PROFILE_FILE} {}", if back { done } else { not });
            }
            if linked {
                let link = format!("{DIR}/disable/{SHIPPED}");
                let back = self.run_cmd(&Cmd::new(&["rm", "-f", &link])).ok()
                    && self
                        .run_cmd(&Cmd::new(&[PARSER, "-r", &format!("{DIR}/{SHIPPED}")]))
                        .ok();
                undone += &if back {
                    format!("; Ubuntu's AppArmor profile {SHIPPED} is enabled again")
                } else {
                    format!("; Ubuntu's AppArmor profile {SHIPPED} stays disabled by {link}")
                };
            }
            format!("{why}{undone}")
        })
    }

    /// What changes the profiles AppArmor holds; `linked` once the shipped profile's link is
    /// made.
    fn apply_loaded(&mut self, plan: &Plan, linked: &mut bool) -> Result<(), String> {
        let shipped = format!("{DIR}/{SHIPPED}");
        if plan.disable {
            self.must(&["ln", "-s", &shipped, &format!("{DIR}/disable/{SHIPPED}")])?;
            *linked = true;
        }
        if plan.disable || plan.unload {
            let argv = [PARSER, "-R", shipped.as_str()];
            let out = self.run_cmd(&Cmd::new(&argv));
            // Exit 254: it was not loaded.
            if !out.ok() && out.code != 254 {
                return Err(format!("`{}` failed: {}", argv.join(" "), out.why()));
            }
        }
        for name in &plan.reload {
            let file = format!("{DIR}/{name}");
            // A file the parser refuses cannot be unloaded by its file either (`-R` reads it).
            self.must(&[PARSER, "-r", &file]).map_err(|why| {
                format!(
                    "{why}; Yulius does not change a profile it did not write: correct {file}, \
                     then run `sudo yulius host setup` again"
                )
            })?;
        }
        if plan.load {
            let out = self.run_cmd(&Cmd::new(&[PARSER, "-r", PROFILE_FILE]));
            if !out.ok() {
                return Err(format!(
                    "unsupported: this host's AppArmor cannot load Yulius's profile ({})",
                    out.why()
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

    /// However a person lays the profile out, it is theirs; a profile inside another one
    /// attaches to nothing by itself.
    #[test]
    fn a_profile_header_is_found_in_any_layout() {
        for whole in [
            "profile bwrap /usr/bin/bwrap flags=(unconfined) { userns, }\n",
            "profile bwrap /usr/bin/bwrap flags=(unconfined) { # by hand\n  userns,\n}\n",
            "include <tunables/global>\nprofile bwrap /usr/bin/bwrap flags=(unconfined)\n{\n  \
             userns,\n}\n",
            "profile bwrap /usr/bin/bwrap flags=(unconfined, attach_disconnected) {\n}\n",
            "/usr/bin/bwrap{\n  userns,\n}\n",
            "profile x /usr/bin/x {\n  userns,\n}\nprofile b /usr/bin/bwrap {\n}\n",
            "profile bwrap /usr/bin/bwrap {userns,}\n",
            "/usr/bin/bwrap {}\n",
            "@{prog} = /usr/bin/bwrap\nprofile b @{prog} {\n  userns,\n}\n",
            "@{prog}=/usr/bin/bwrap\nprofile b @{prog} {\n  userns,\n}\n",
            "@{prog} += \"/usr/bin/bwrap\" # theirs\nprofile b @{prog} {\n}\n",
            "profile bwrap /usr/bin/bwrap\nflags = (unconfined) {\n  userns,\n}\n",
            "profile bwrap /usr/bin/bwrap flags = (attach_disconnected, unconfined) {\n}\n",
            "@{bin}/bwrap flags=(unconfined) {\n  userns,\n}\n",
        ] {
            assert!(attaches_bwrap(whole), "{whole}");
        }
        for other in [
            "profile outer /usr/bin/outer {\n  profile inner /usr/bin/bwrap {\n    userns,\n  \
             }\n}\n",
            "profile x /usr/bin/x { /usr/bin/bwrap Px -> bwrap, }\n",
            "profile x /usr/bin/x flags=(unconfined) /usr/bin/bwrap {\n}\n",
            "profile x /usr/bin/x { # /usr/bin/bwrap {\n}\n",
            "@{prog} = /usr/bin/bwrap\nprofile x /usr/bin/x {\n  userns,\n}\n",
            "profile x /usr/bin/x {userns,}\n/usr/bin/bwrap mr,\n",
            "@{prog}=/usr/bin/x\nprofile x @{prog} {\n  userns,\n}\n",
            "${flag} = true\nprofile x /usr/bin/x flags = (unconfined) {\n}\n",
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
            shipped_off: false,
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
        let disabled_beside_shipped = Facts {
            shipped: true,
            ..disabled
        };
        assert!(Plan::of(&disabled_beside_shipped, &fails).is_empty());
        let linked = Facts {
            shipped_off: true,
            ours: Ours::Current,
            ..facts()
        };
        assert!(Plan::of(&linked, &Proof::Works).is_empty());
        assert_eq!(
            Plan::of(&linked, &fails),
            Plan {
                unload: true,
                load: true,
                ..Plan::default()
            }
        );
        let stale = Facts {
            ours: Ours::Stale,
            ..facts()
        };
        assert_eq!(Plan::of(&stale, &Proof::Works), write_and_load);
    }
}
