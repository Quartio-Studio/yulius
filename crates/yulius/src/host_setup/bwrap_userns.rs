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

use std::collections::{BTreeMap, BTreeSet};
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

/// The program Yulius's profile is for.
const BWRAP: &str = "/usr/bin/bwrap";
/// Stands for the values a variable gets in a file this one includes: no path.
const UNKNOWN: &str = "\0";
/// A file past these is not read: the values one variable may come to, the bytes one header
/// word may, the alternatives tried for it.
const MOST_VALUES: usize = 1024;
const LONGEST_WORD: usize = 16 * 1024;
const MOST_TRIES: u32 = 1024;

/// Whether a profile in `text` attaches to `/usr/bin/bwrap`: a header at the top level
/// (`profile name /path flags=(…) {`, or `/path {`) whose path, read as AppArmor reads it, is
/// bwrap's. The header's brace may open the profile on the same line, before a comment or on
/// the next line (with its `flags = (…)`); the path may use variables the file sets and
/// alternations (`@{bin}={/usr,}/bin`, `@{bin}/bwrap`). A rule that only names bwrap
/// (`/usr/bin/bwrap mr,`), a header's attributes and a profile inside another one are none.
fn attaches_bwrap(text: &str) -> bool {
    // Blocks open, and the words since the last rule or block at the top level.
    let mut depth = 0_usize;
    let mut header: Vec<&str> = Vec::new();
    let mut in_list = false;
    let mut vars = Vars::new();
    // The variable whose values go on after a line's closing backslash.
    let mut more: Option<&str> = None;
    for line in text.lines() {
        // A variable's line ends without a comma and is no header.
        let assigned = more.take().map(|name| (name, line)).or_else(|| {
            let (name, adds, values) = assignment(line)?;
            vars.entry(name).or_default().declared |= !adds;
            Some((name, values))
        });
        if let Some((name, values)) = assigned {
            let var = vars.entry(name).or_default();
            let mut values = words(values).peekable();
            while let Some(value) = values.next() {
                if value == "\\" && values.peek().is_none() {
                    more = Some(name);
                } else {
                    var.values.push(value.trim_matches('"'));
                }
            }
            continue;
        }
        // Nor is an `include`, which ends without a comma too.
        if words(line).next() == Some("include") {
            continue;
        }
        // A brace that starts a word opens a block (`{userns,}`); an alternation's
        // (`/{usr/,}bin`, `@{bin}`) is inside its word and closes there.
        let words = words(line)
            .flat_map(|w| w.strip_prefix('{').map_or([w, ""], |rest| ["{", rest]))
            .filter(|w| !w.is_empty());
        for word in words {
            let opens = word.matches('{').count();
            let closes = word.matches('}').count();
            if opens > closes {
                header.extend(Some(word.trim_end_matches('{')).filter(|w| !w.is_empty()));
                if depth == 0 && header_attaches(&header, &vars) {
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

/// A line's words before its comment; quotes keep a word with spaces whole.
fn words(line: &str) -> impl Iterator<Item = &str> {
    let mut rest = line;
    std::iter::from_fn(move || {
        rest = rest.trim_start();
        if rest.is_empty() || rest.starts_with('#') {
            return None;
        }
        let mut quoted = false;
        let end = rest.find(|c: char| {
            quoted ^= c == '"';
            !quoted && c.is_whitespace()
        });
        let (word, after) = rest.split_at(end.unwrap_or(rest.len()));
        rest = after;
        Some(word)
    })
}

/// A variable of the file: whether the file sets it (`=`; `+=` adds to what an included file
/// set) and the values it gives.
#[derive(Default)]
struct Var<'a> {
    declared: bool,
    values: Vec<&'a str>,
}

/// A file's variables by name (as used in `@{name}`).
type Vars<'a> = BTreeMap<&'a str, Var<'a>>;

/// A variable's line (`@{name} = values`, `@{name}+=values`, `@name = values`): its name,
/// whether it adds, and its values. A header that starts with a variable (`@{bin}/bwrap {`,
/// which AppArmor itself refuses) is none.
fn assignment(line: &str) -> Option<(&str, bool, &str)> {
    let line = line.trim_start().strip_prefix('@')?;
    let (name, rest) = line.split_at(line.find(|c: char| c.is_whitespace() || "+=".contains(c))?);
    let name = name
        .strip_prefix('{')
        .map_or(Some(name), |name| name.strip_suffix('}'))?;
    let rest = rest.trim_start();
    let adds = rest.starts_with("+=");
    let values = rest.strip_prefix("+=").or_else(|| rest.strip_prefix('='))?;
    let plain = !name.is_empty() && name.chars().all(|c| c.is_alphanumeric() || c == '_');
    plain.then_some((name, adds, values))
}

/// Whether a header's word starts its attributes (`flags=(…)`, `xattrs = (…)`, a bare
/// `(complain)`): what follows names no program. A profile may be called `flags`.
fn attribute(word: &str, next: Option<&str>) -> bool {
    let keyed = |key: &str| {
        word.strip_prefix(key).is_some_and(|rest| {
            rest.starts_with('=') || (rest.is_empty() && next.is_some_and(|n| n.starts_with('=')))
        })
    };
    word.starts_with('(') || keyed("flags") || keyed("xattrs")
}

/// Whether a top-level header gives bwrap as its profile's program: its attachment where it
/// has one (`profile name /path`, `/name /path`), else its name (`profile /path`, `/path`).
/// A profile in a policy namespace (`:ns:name`) confines nothing on the host itself. In a
/// layout not known here any word may be the program: a person's profile is rather seen
/// than missed.
fn header_attaches(header: &[&str], vars: &Vars) -> bool {
    let named = (0..header.len())
        .find(|&at| attribute(header[at], header.get(at + 1).copied()))
        .map_or(header, |at| &header[..at]);
    let keyword = named.iter().rposition(|w| *w == "profile");
    match keyword.map_or(named, |at| &named[at + 1..]) {
        [name] => is_bwrap(name, vars, true),
        [name, attachment] => !name.starts_with(':') && is_bwrap(attachment, vars, false),
        words => words.iter().any(|word| is_bwrap(word, vars, false)),
    }
}

/// Whether a header's word is bwrap's path once the file's variables are in it. In a name,
/// `//` parts a child from its parent: that is no path.
fn is_bwrap(word: &str, vars: &Vars, name: bool) -> bool {
    expanded(word.trim_matches('"'), vars).is_some_and(|text| {
        !(name && text.contains("//")) && spells(&one_slash(&text), BWRAP, &mut MOST_TRIES.clone())
    })
}

/// A header's word with each variable replaced as AppArmor does: by its value, or by an
/// alternation of its values (`{a,b}`). A value loses its slashes where the word has one of
/// its own: at its end before a slash, at its start after one that follows other text.
/// `None` where a variable cannot be read.
fn expanded(word: &str, vars: &Vars) -> Option<String> {
    let mut text = word.to_owned();
    let mut from = 0;
    while let Some((at, end)) = variable(&text, from) {
        let (before, after) = (&text[..at], &text[end + 1..]);
        let values = values(&text[at + 2..end], vars, &mut Vec::new())?;
        let leads = before.ends_with('/') && before.contains(|c| c != '/');
        let trimmed: Vec<&str> = values
            .iter()
            .map(|value| {
                if leads {
                    value.trim_start_matches('/')
                } else {
                    value
                }
            })
            .map(|value| {
                if after.starts_with('/') {
                    value.trim_end_matches('/')
                } else {
                    value
                }
            })
            .collect();
        let put = match trimmed.as_slice() {
            [one] => (*one).to_owned(),
            many => format!("{{{}}}", many.join(",")),
        };
        from = at + put.len();
        text = format!("{before}{put}{after}");
        if text.len() > LONGEST_WORD {
            return None;
        }
    }
    Some(text)
}

/// Where the next `@{name}` of `text` starts and ends, from `from` on.
fn variable(text: &str, from: usize) -> Option<(usize, usize)> {
    let at = text[from..].find("@{")? + from;
    Some((at, text[at..].find('}')? + at))
}

/// A variable's values as AppArmor holds them: a set, the variables inside a value replaced
/// by each of theirs in turn. One the file does not set comes from a file it includes:
/// `@{bin}` is then taken for bwrap's directory, any other for no path. `None` where a
/// variable names itself (`seen`: the ones being read) or has too many values.
fn values(name: &str, vars: &Vars, seen: &mut Vec<String>) -> Option<BTreeSet<String>> {
    if seen.iter().any(|s| s == name) {
        return None;
    }
    let var = vars.get(name);
    let mut raw: Vec<String> = var.map_or_else(Vec::new, |var| {
        var.values.iter().map(|value| (*value).to_owned()).collect()
    });
    if !var.is_some_and(|var| var.declared) {
        raw.push(if name == "bin" { "/usr/bin" } else { UNKNOWN }.to_owned());
    }
    seen.push(name.to_owned());
    let mut done = BTreeSet::new();
    while let Some(value) = raw.pop() {
        if done.len() + raw.len() >= MOST_VALUES || value.len() > LONGEST_WORD {
            return None;
        }
        let Some((at, end)) = variable(&value, 0) else {
            done.insert(value);
            continue;
        };
        for inner in values(&value[at + 2..end], vars, seen)? {
            raw.push(format!("{}{inner}{}", &value[..at], &value[end + 1..]));
        }
    }
    seen.pop();
    Some(done)
}

/// A path as AppArmor reads its slashes: several are one, except exactly two at the start.
fn one_slash(path: &str) -> String {
    let kept = if path.starts_with("//") && !path.starts_with("///") {
        2
    } else {
        0
    };
    let mut out = path[..kept].to_owned();
    for c in path[kept..].chars() {
        if c != '/' || !out.ends_with('/') || out.len() == kept {
            out.push(c);
        }
    }
    out
}

/// Whether `pattern` spells `path`, each of its alternations (`{a,b}`, also one inside
/// another) tried in turn, `tries` times at most. A `*`, `?` or `[` stands for itself: a
/// profile that matches by one gives way to a profile that names the path.
fn spells(pattern: &str, path: &str, tries: &mut u32) -> bool {
    let mut depth = 0_usize;
    let (mut open, mut start) = (0, 0);
    let mut alternatives = Vec::new();
    for (at, c) in pattern.char_indices() {
        match c {
            '{' => {
                depth += 1;
                if depth == 1 {
                    (open, start) = (at, at + 1);
                }
            }
            ',' if depth == 1 => {
                alternatives.push(&pattern[start..at]);
                start = at + 1;
            }
            '}' if depth == 1 => {
                alternatives.push(&pattern[start..at]);
                let Some(path) = path.strip_prefix(&pattern[..open]) else {
                    return false;
                };
                return alternatives.iter().any(|alternative| {
                    *tries = tries.saturating_sub(1);
                    *tries > 0
                        && spells(&format!("{alternative}{}", &pattern[at + 1..]), path, tries)
                });
            }
            '}' => depth = depth.saturating_sub(1),
            _ => {}
        }
    }
    depth == 0 && pattern == path
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
        if self.mode != Mode::Setup {
            // Setup's steps alone do not let bwrap in past a kernel limit.
            let limits: String = match first {
                Proof::Fails(_) => self.limits().iter().map(|l| format!("; {l}")).collect(),
                Proof::Works | Proof::NotRun(_) => String::new(),
            };
            let steps = plan.steps(false);
            if self.mode == Mode::DryRun {
                return self.changed(ITEM, "", &format!("{steps}{limits}"));
            }
            let state = match &first {
                Proof::Fails(why) => {
                    format!("bwrap cannot start a user namespace as {user} ({why})")
                }
                Proof::Works | Proof::NotRun(_) => {
                    "the AppArmor rules for bwrap are not what setup leaves".to_owned()
                }
            };
            let then = if limits.is_empty() { ":" } else { ";" };
            return self.needs_setup(ITEM, &format!("{state}{limits}{then} setup would {steps}"));
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
            "@{prog} = /usr/bin/x\n@{prog} += \"/usr/bin/bwrap\" # theirs\nprofile b @{prog} {\n}\n",
            "@{prog}=/usr/bin/x\n@{prog}+=/usr/bin/bwrap\nprofile b @{prog} {\n}\n",
            "profile bwrap /usr/bin/bwrap\nflags = (unconfined) {\n  userns,\n}\n",
            "profile bwrap /usr/bin/bwrap flags = (attach_disconnected, unconfined) {\n}\n",
            "@{bin}/bwrap flags=(unconfined) {\n  userns,\n}\n",
            "@prog = /usr/bin/bwrap\nprofile b @{prog} {\n  userns,\n}\n",
            "@{dir} = /usr/bin\nprofile b @{dir}/bwrap {\n}\n",
            "@{a} = /usr/bin/bwrap\n@{b} = @{a}\nprofile b @{b} {\n}\n",
            "@{dirs} = /opt /usr\nprofile b @{dirs}/bin/bwrap {\n}\n",
            "@{dir} = /usr/bin/\nprofile b @{dir}/bwrap {\n}\n",
            // The apparmor.d project's layout, its `@{bin}` set in a file it includes.
            "@{exec_path} = @{bin}/bwrap\nprofile bwrap @{exec_path} flags=(complain) {\n  \
             @{exec_path} mr,\n}\n",
            "profile flags /usr/bin/bwrap {\n}\n",
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
            "abi <abi/4.0>,\n@prog = /usr/bin/bwrap\nprofile x /usr/bin/x {\n  @{prog} mr,\n}\n",
            "@{dir} = /usr/bin\nprofile x @{dir}/x {\n}\n",
            "@{prog} = /usr/bin/y\n@{prog} += /usr/bin/bwrap\nprofile x /usr/bin/x {\n  @{prog} \
             mr,\n}\n",
            // The apparmor.d project's profile for Flatpak's bwrap: entered by a transition,
            // attached to nothing.
            "@{exec_path} = @{bin}/bwrap\nprofile fbwrap flags=(attach_disconnected) {\n  \
             @{exec_path} mr,\n}\n",
            "@{bin} = /opt/bin\nprofile x @{bin}/bwrap {\n}\n",
            "@{a} = @{a}/bwrap\nprofile x @{a} {\n}\n",
            "profile x /usr/bin/x xattrs = (user.t = /usr/bin/bwrap ) {\n}\n",
        ] {
            assert!(!attaches_bwrap(other), "{other}");
        }
    }

    /// A header's variables, alternations and slashes are read as AppArmor reads them: each
    /// sample is what `apparmor_parser` 5.0.2 makes of it (SPEC-456, Evidence).
    #[test]
    fn a_header_is_read_as_apparmor_reads_it() {
        for whole in [
            // Ubuntu 26.04's own spelling (its `hostname` profile).
            "@{bin}={/usr,}/bin\nprofile b @{bin}/bwrap {\n  userns,\n}\n",
            "@{bin}={/usr,}/bin\n@{exec}={bwrap,x}\nprofile b @{bin}/@{exec} {\n}\n",
            // Added to what an included file sets: that file's values are not seen here.
            "include <tunables/global>\n@{bin} += /usr/local/bin\nprofile b @{bin}/bwrap {\n}\n",
            "@{exec} = bwrap\nprofile b @{bin}/@{exec} {\n}\n",
            "@{prog} = /usr/bin/bwrap\n@{prog} += /usr/bin/x\nprofile b @{prog} {\n}\n",
            "@{b} = @{a}\n@{a} = /usr/bin/bwrap\nprofile b @{b} {\n}\n",
            "@{paths} = /usr/bin/x \\\n  /usr/bin/bwrap\nprofile b @{paths} {\n}\n",
            "@{d} = /bin/ /sbin/\nprofile b /usr/@{d}/bwrap {\n}\n",
            "@{d} = /opt /usr\n@{x} = @{d}/bin/bwrap\nprofile b @{x} {\n}\n",
            "profile b /{usr,}/bin/bwrap {\n}\n",
            "profile b /{,usr/}{,s}bin/bwrap {\n}\n",
            "profile b /usr/{bin,sbin}/bwrap {\n}\n",
            "profile b /{usr/{bin,sbin},opt}/bwrap {\n}\n",
            "profile b /usr/bin///bwrap {\n}\n",
            "profile b /usr/bin/bwrap (unconfined) {\n}\n",
            "/usr/bin/x /usr/bin/bwrap {\n}\n",
            "profile \"my bwrap\" /usr/bin/bwrap {\n}\n",
            "@{d} = /usr/bin\nprofile @{d}/bwrap {\n}\n",
        ] {
            assert!(attaches_bwrap(whole), "{whole}");
        }
        for other in [
            "@{prog} = /usr/bin/x # /usr/bin/bwrap\nprofile x @{prog} {\n}\n",
            "  @{prog} = /usr/bin/bwrap\nprofile x /usr/bin/x {\n  @{prog} mr,\n}\n",
            "@{paths} = /usr/bin/x \\\n  /usr/bin/bwrap\nprofile x /usr/bin/x {\n  @{paths} ix,\n}\n",
            "@{p} = \"/usr/bin/bwrap helper\" /usr/bin/x\nprofile x @{p} {\n}\n",
            // Two slashes at the start are kept, and so are those an alternation makes.
            "profile b //usr/bin/bwrap {\n}\n",
            "@{dir} = /usr/bin\nprofile b /@{dir}/bwrap {\n}\n",
            "@{dirs} = /opt /usr/bin\nprofile b /@{dirs}/bwrap {\n}\n",
            "profile b /{usr/,}/bin/bwrap {\n}\n",
            "profile b /usr/bin/bwrap/ {\n}\n",
            // In a name, `//` parts a child from its parent.
            "/usr/bin//bwrap {\n}\n",
            "include <tunables/global>\nprofile /usr/bin//bwrap (complain) {\n}\n",
            "@{p} = /usr/bin//bwrap\nprofile @{p} {\n}\n",
            // Beside an attachment the name is none, and a policy namespace is not the host's.
            "profile /usr/bin/bwrap /usr/bin/x {\n}\n",
            "profile :ns:b /usr/bin/bwrap {\n}\n",
            // A glob gives way to a profile that names the path.
            "profile b /usr/bin/bw* {\n}\n",
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
