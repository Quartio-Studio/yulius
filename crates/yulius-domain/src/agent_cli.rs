//! Claude Code kept current by Yulius (SPEC-421, ADR-0266): which release channel, which
//! versions, which platform package, which installed versions to keep; and which models a build
//! must know before sandboxes get it (SPEC-461, ADR-0293).
//!
//! Pure: the caller reads the registry, downloads, reads the binary, installs and removes.

use std::fmt;

use crate::harness::{DEFAULT_CLAUDE_MODEL, OFFERED_CLAUDE_MODELS};

/// The npm package of the Claude Code CLI.
pub const PACKAGE: &str = "@anthropic-ai/claude-code";
/// The only registry versions and tarballs are read from.
pub const REGISTRY_HOST: &str = "registry.npmjs.org";
/// Installed versions kept: the one in use and the one before it (CCU-004).
pub const KEEP: usize = 2;

/// A released version: plain `X.Y.Z`, nothing else (it names a directory).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Version {
    major: u32,
    minor: u32,
    patch: u32,
}

impl Version {
    /// `X.Y.Z` of decimal numbers (no sign, pre-release or build part).
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        let mut parts = text.split('.');
        let mut number = || {
            let part = parts.next()?;
            let leading_zero = part.len() > 1 && part.starts_with('0');
            if part.is_empty()
                || part.len() > 9
                || leading_zero
                || !part.bytes().all(|b| b.is_ascii_digit())
            {
                return None;
            }
            part.parse().ok()
        };
        let version = Self {
            major: number()?,
            minor: number()?,
            patch: number()?,
        };
        parts.next().is_none().then_some(version)
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

/// Which releases an instance follows (`YULIUS_CLAUDE_UPDATES`, or the channel chosen in
/// Settings).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Channel {
    /// npm's `stable` tag: about a week behind `latest`, without releases that regressed.
    Stable,
    /// npm's `latest` tag: the default, so a new default model never outruns the Claude Code
    /// that runs it (SPEC-461 CCM-001; the incident of 2026-11-11).
    #[default]
    Latest,
    /// No checks: runs get the host's Claude Code.
    Off,
}

impl Channel {
    /// `stable`, `latest` or `off`.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "stable" => Some(Self::Stable),
            "latest" => Some(Self::Latest),
            "off" => Some(Self::Off),
            _ => None,
        }
    }

    /// The npm dist-tag read, `None` when off.
    #[must_use]
    pub fn dist_tag(self) -> Option<&'static str> {
        match self {
            Self::Stable => Some("stable"),
            Self::Latest => Some("latest"),
            Self::Off => None,
        }
    }

    /// `stable`, `latest` or `off`: what [`Self::parse`] reads.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Stable => "stable",
            Self::Latest => "latest",
            Self::Off => "off",
        }
    }

    /// The dist-tag taken instead when neither the version in use nor this channel's release
    /// knows the default model (SPEC-461 CCM-004): `latest` for `stable`, nothing newer for
    /// `latest`.
    #[must_use]
    pub fn falls_forward_to(self) -> Option<&'static str> {
        (self == Self::Stable).then_some("latest")
    }
}

/// What a turn's end knows about a session that may be restarted on a newer Claude Code
/// (SPEC-461 CCM-013).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MoveFacts {
    /// The Claude Code version the session's transcript names.
    pub runs: Version,
    /// The version a run starting now gets.
    pub target: Version,
    /// The version this task's session was sent to before.
    pub sent_to: Option<Version>,
    /// A version a session could not continue on.
    pub stopped_for: Option<Version>,
    /// How long ago a person typed into the session, when one did.
    pub typed_ms_ago: Option<i64>,
    /// A switch of this task, or an earlier restart for a newer Claude Code, was refused moments
    /// ago.
    pub switch_refused: bool,
    /// A person paused the task: a restart would end their pause.
    pub paused: bool,
    /// An update of the instance holds the runs: a restart would outrun it.
    pub update_holds: bool,
    /// Whether background shells (one that only waits for Yulius included), agents or monitors of
    /// the session run now; `None` when that could not be told.
    pub background: Option<bool>,
}

/// A person who typed into a session this recently owns its prompt box: it is not restarted
/// under them (their draft would be lost).
pub const TYPED_RECENTLY_MS: i64 = 60_000;

/// Whether the session is restarted on `target` now: it runs an older version, was not sent to
/// `target` before (a session that still names the old one afterwards is not restarted at every
/// turn's end), no session failed to continue on `target`, nobody typed into it in the last
/// minute or paused it, no update of the instance holds the runs, no switch was just refused,
/// and nothing of it runs in the background, which a restart would end (unknown counts as
/// running).
#[must_use]
pub fn moves_now(facts: &MoveFacts) -> bool {
    facts.runs < facts.target
        && facts.sent_to != Some(facts.target)
        && facts.stopped_for != Some(facts.target)
        && facts
            .typed_ms_ago
            .is_none_or(|ago| ago >= TYPED_RECENTLY_MS)
        && !facts.switch_refused
        && !facts.paused
        && !facts.update_holds
        && facts.background == Some(false)
}

/// The text after a model id that ends it: anything but a letter, a digit or a hyphen, or the end
/// of the file (SPEC-461 CCM-002). So `claude-fable-5-2` is not read in `claude-fable-5-20`, nor
/// `claude-fable-5` in `claude-fable-5-2`.
#[must_use]
pub fn ends_model_id(next: Option<u8>) -> bool {
    next.is_none_or(|byte| !byte.is_ascii_alphanumeric() && byte != b'-')
}

/// The offered models a build does not know, the default first, given the ids its binary names.
#[must_use]
pub fn missing_models(known: &[&str]) -> Vec<String> {
    OFFERED_CLAUDE_MODELS
        .iter()
        .filter(|model| !known.contains(model))
        .map(|model| (*model).to_owned())
        .collect()
}

/// Whether a build that lacks `missing` must never run in a sandbox: it does not know the
/// default model (SPEC-461 CCM-003). A gap in the other offered models is reported, not refused.
#[must_use]
pub fn lacks_default(missing: &[String]) -> bool {
    missing.iter().any(|model| model == DEFAULT_CLAUDE_MODEL)
}

/// How the host's Claude Code is updated by hand: runs get the newer of it and Yulius's copy.
pub const HOST_INSTALL: &str = "sudo npm install -g @anthropic-ai/claude-code@latest";

/// What is wrong with a Claude Code that does not know the default model, in one sentence
/// (SPEC-461 CCM-007, CCM-008; the incident of 2026-11-11).
#[must_use]
pub fn default_model_problem(version: &str) -> String {
    format!(
        "Claude Code {version} does not know {DEFAULT_CLAUDE_MODEL}, Yulius's default model: \
         tasks still ask for it by id, but what a session picks by itself (its model picker, a \
         model alias, its subagents) is an older model"
    )
}

/// What the last check of the registry showed about npm's releases.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegistryRead {
    /// None ran, or it failed: what npm offers now is not known.
    Unknown,
    /// It read npm's tags, and installed a release or found runs up to date.
    Read,
    /// It read npm's tags and refused this release, the last one it tried, for the default
    /// model.
    Refused(Version),
}

/// What is known about npm's newest release while runs get a Claude Code that does not know the
/// default model.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NewestFacts {
    /// The channel in effect.
    pub channel: Channel,
    /// What the last check showed.
    pub read: RegistryRead,
    /// How long ago that check was made, when one was.
    pub read_ms_ago: Option<i64>,
    /// npm's `latest` as that check read it.
    pub latest: Option<Version>,
    /// The version runs get.
    pub runs: Option<Version>,
}

/// How old a read of the registry may be and still say what npm's newest release is: two rounds
/// of the hourly check.
pub const NEWEST_KNOWN_FOR_MS: i64 = 2 * 60 * 60 * 1000;

/// npm's newest release, when the last check shows that it does not know the default model
/// either: that check refused it, or it is what runs get. Then no release knows the model yet,
/// and neither a check nor the host's install helps. `None` whenever that is not known: with
/// updates off (nothing reads the registry), after a failed check, or when the last read is
/// older than [`NEWEST_KNOWN_FOR_MS`]; a `latest` remembered from an earlier day says nothing
/// about today, and the channel's own fix is the one to give.
#[must_use]
pub fn newest_lacking_default(facts: &NewestFacts) -> Option<Version> {
    if facts.channel == Channel::Off {
        return None;
    }
    let fresh = facts
        .read_ms_ago
        .is_some_and(|ago| (0..=NEWEST_KNOWN_FOR_MS).contains(&ago));
    let latest = facts.latest.filter(|_| fresh)?;
    match facts.read {
        RegistryRead::Unknown => None,
        RegistryRead::Refused(refused) if refused == latest => Some(latest),
        RegistryRead::Refused(_) | RegistryRead::Read => facts.runs.filter(|runs| *runs >= latest),
    }
}

/// How a Claude Code that does not know the default model is fixed, in one sentence: what
/// Yulius does by itself on `channel`, and the host's way. `newest_lacks` is npm's newest
/// release when that does not know the model either ([`newest_lacking_default`]): then no
/// release knows it yet, and neither a check nor the host's install helps.
#[must_use]
pub fn default_model_fix(channel: Channel, newest_lacks: Option<&str>) -> String {
    if let Some(newest) = newest_lacks {
        return format!(
            "No Claude Code release knows it yet ({newest}, the newest, does not): the default \
             model is ahead of Claude Code, so choose a model it knows for tasks until a \
             release does"
        );
    }
    match channel {
        Channel::Off => format!(
            "Claude Code updates are off: choose Latest in Settings, Instance, Claude Code, or \
             install a newer one on the host: `{HOST_INSTALL}`"
        ),
        Channel::Stable | Channel::Latest => format!(
            "Yulius installs a release that knows it at its next check (every hour, or \"Check \
             now\" in Settings, Instance, Claude Code); to fix it at once, install one on the \
             host: `{HOST_INSTALL}`"
        ),
    }
}

/// "2.1.311 does not know claude-fable-5-2 (Yulius's default model) and claude-opus-5-5".
#[must_use]
pub fn gap_text(version: &str, missing: &[String]) -> String {
    let named: Vec<String> = missing
        .iter()
        .map(|model| {
            if model == DEFAULT_CLAUDE_MODEL {
                format!("{model} (Yulius's default model)")
            } else {
                model.clone()
            }
        })
        .collect();
    let list = match named.as_slice() {
        [] => return format!("{version} knows every model Yulius offers"),
        [one] => one.clone(),
        [first @ .., last] => format!("{} and {last}", first.join(", ")),
    };
    format!("{version} does not know {list}")
}

/// The platform package holding the native binary, for Rust's `os` and `arch` names and the C
/// library; `None` where none is published (or Yulius does not install one).
#[must_use]
pub fn platform_package(os: &str, arch: &str, musl: bool) -> Option<String> {
    let cpu = match arch {
        "x86_64" => "x64",
        "aarch64" => "arm64",
        _ => return None,
    };
    let libc = if musl { "-musl" } else { "" };
    (os == "linux").then(|| format!("{PACKAGE}-linux-{cpu}{libc}"))
}

/// ELF's `e_machine` of a Linux platform package's binary.
#[must_use]
pub fn elf_machine(package: &str) -> Option<u16> {
    let platform = package.strip_prefix(PACKAGE)?.trim_end_matches("-musl");
    match platform {
        "-linux-x64" => Some(62),
        "-linux-arm64" => Some(183),
        _ => None,
    }
}

/// Whether `offered` should be installed over the version runs get now.
#[must_use]
pub fn should_install(offered: Version, in_use: Option<Version>) -> bool {
    in_use.is_none_or(|in_use| offered > in_use)
}

/// Installed versions to remove once `current` is in use: all but `current` and the newest
/// one older than it.
#[must_use]
pub fn to_remove(installed: &[Version], current: Version) -> Vec<Version> {
    let previous = installed.iter().copied().filter(|&v| v < current).max();
    installed
        .iter()
        .copied()
        .filter(|&v| v != current && Some(v) != previous)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(text: &str) -> Version {
        Version::parse(text).unwrap_or_else(|| panic!("{text} parses"))
    }

    #[test]
    fn only_plain_versions_parse() {
        assert_eq!(v("2.1.311").to_string(), "2.1.311");
        assert_eq!(v("0.0.0").to_string(), "0.0.0");
        for bad in [
            "",
            "2.1",
            "2.1.3.4",
            "2.1.3-beta.1",
            "2.1.3+b",
            "v2.1.3",
            "+2.1.3",
            "2.-1.3",
            "2..3",
            " 2.1.3",
            "2.1.3 ",
            "../1.2",
            "1/2.3",
            "2.1.99999999999",
            "02.1.3",
            "2.01.3",
        ] {
            assert_eq!(Version::parse(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn versions_order_by_number_not_text() {
        assert!(v("2.1.311") > v("2.1.292"));
        assert!(v("2.10.0") > v("2.9.99"));
        assert!(v("3.0.0") > v("2.99.99"));
    }

    #[test]
    fn channels_name_their_dist_tags() {
        assert_eq!(Channel::parse("stable"), Some(Channel::Stable));
        assert_eq!(Channel::parse("latest"), Some(Channel::Latest));
        assert_eq!(Channel::parse("off"), Some(Channel::Off));
        assert_eq!(Channel::parse("next"), None);
        for channel in [Channel::Stable, Channel::Latest, Channel::Off] {
            assert_eq!(Channel::parse(channel.as_str()), Some(channel));
        }
        assert_eq!(Channel::Stable.dist_tag(), Some("stable"));
        assert_eq!(Channel::Latest.dist_tag(), Some("latest"));
        assert_eq!(Channel::Off.dist_tag(), None);
    }

    /// SPEC-461 AC2: an instance follows `latest` unless told otherwise.
    #[test]
    fn the_default_channel_is_latest() {
        assert_eq!(Channel::default(), Channel::Latest);
        assert_eq!(Channel::default().dist_tag(), Some("latest"));
    }

    /// SPEC-461 CCM-004: only `stable` has a newer tag to fall forward to.
    #[test]
    fn only_stable_falls_forward_to_latest() {
        assert_eq!(Channel::Stable.falls_forward_to(), Some("latest"));
        assert_eq!(Channel::Latest.falls_forward_to(), None);
        assert_eq!(Channel::Off.falls_forward_to(), None);
    }

    /// SPEC-461 CCM-002: an id ends where a longer id would go on.
    #[test]
    fn a_model_id_ends_before_anything_but_a_letter_digit_or_hyphen() {
        for ends in [b'"', b'\'', b'[', b']', b',', b' ', b'@', 0] {
            assert!(ends_model_id(Some(ends)), "{ends}");
        }
        assert!(ends_model_id(None), "the end of the file");
        for goes_on in *b"09aZ-" {
            assert!(!ends_model_id(Some(goes_on)), "{goes_on}");
        }
    }

    /// SPEC-461 CCM-003: the default model is the gate, the other offered models a reported gap.
    #[test]
    fn a_build_without_the_default_model_is_refused_and_another_gap_is_named() {
        assert_eq!(OFFERED_CLAUDE_MODELS[0], DEFAULT_CLAUDE_MODEL);
        assert!(missing_models(&OFFERED_CLAUDE_MODELS).is_empty());
        let without_default = missing_models(&OFFERED_CLAUDE_MODELS[1..]);
        assert_eq!(without_default, [DEFAULT_CLAUDE_MODEL]);
        assert!(lacks_default(&without_default));
        let without_haiku = missing_models(&OFFERED_CLAUDE_MODELS[..4]);
        assert_eq!(without_haiku, ["claude-haiku-4-5"]);
        assert!(!lacks_default(&without_haiku));
        assert!(!lacks_default(&[]));
        assert_eq!(
            gap_text("2.1.311", &without_default),
            "2.1.311 does not know claude-fable-5-2 (Yulius's default model)"
        );
        assert_eq!(
            gap_text(
                "2.1.311",
                &missing_models(&["claude-opus-5-5", "claude-sonnet-5-5"])
            ),
            "2.1.311 does not know claude-fable-5-2 (Yulius's default model), claude-fable-5-1 \
             and claude-haiku-4-5"
        );
        assert_eq!(
            gap_text("2.1.322", &[]),
            "2.1.322 knows every model Yulius offers"
        );
    }

    /// SPEC-461 CCM-007: the fix names what Yulius does and the host's command, per channel.
    #[test]
    fn the_fix_names_what_happens_next_and_the_host_command() {
        let problem = default_model_problem("2.1.311");
        assert!(
            problem.starts_with("Claude Code 2.1.311 does not know claude-fable-5-2"),
            "{problem}"
        );
        for channel in [Channel::Latest, Channel::Stable] {
            let fix = default_model_fix(channel, None);
            assert!(fix.contains("at its next check"), "{fix}");
            assert!(fix.contains("Check now"), "{fix}");
            assert!(fix.contains(HOST_INSTALL), "{fix}");
        }
        let off = default_model_fix(Channel::Off, None);
        assert!(
            off.contains("updates are off") && off.contains("choose Latest"),
            "{off}"
        );
        assert!(off.contains(HOST_INSTALL), "{off}");
        // The newest release itself was refused: installing it on the host would not help.
        let ahead = default_model_fix(Channel::Latest, Some("2.1.322"));
        assert!(ahead.contains("2.1.322, the newest, does not"), "{ahead}");
        assert!(!ahead.contains(HOST_INSTALL), "{ahead}");
    }

    /// SPEC-461 CCM-013: when a turn's end restarts a session on the newer Claude Code.
    #[test]
    fn a_session_moves_only_when_nothing_would_be_lost() {
        let due = MoveFacts {
            runs: v("2.1.311"),
            target: v("2.1.322"),
            sent_to: None,
            stopped_for: None,
            typed_ms_ago: None,
            switch_refused: false,
            paused: false,
            update_holds: false,
            background: Some(false),
        };
        assert!(moves_now(&due));
        let stays = |facts: MoveFacts, why: &str| assert!(!moves_now(&facts), "{why}");
        stays(
            MoveFacts {
                runs: v("2.1.322"),
                ..due
            },
            "it runs the version runs get",
        );
        stays(
            MoveFacts {
                runs: v("2.1.400"),
                ..due
            },
            "it runs a newer one",
        );
        stays(
            MoveFacts {
                sent_to: Some(v("2.1.322")),
                ..due
            },
            "it was sent to that version before",
        );
        stays(
            MoveFacts {
                stopped_for: Some(v("2.1.322")),
                ..due
            },
            "a session could not continue on that version",
        );
        stays(
            MoveFacts {
                typed_ms_ago: Some(59_999),
                ..due
            },
            "a person just typed",
        );
        stays(
            MoveFacts {
                switch_refused: true,
                ..due
            },
            "a switch was just refused",
        );
        stays(
            MoveFacts {
                paused: true,
                ..due
            },
            "a person paused the task",
        );
        stays(
            MoveFacts {
                update_holds: true,
                ..due
            },
            "an update of the instance holds the runs",
        );
        stays(
            MoveFacts {
                background: Some(true),
                ..due
            },
            "background work runs",
        );
        stays(
            MoveFacts {
                background: None,
                ..due
            },
            "background work could not be told",
        );
        // Sent to an older version before, or stopped for another: this one is new.
        assert!(moves_now(&MoveFacts {
            sent_to: Some(v("2.1.318")),
            stopped_for: Some(v("2.1.318")),
            typed_ms_ago: Some(TYPED_RECENTLY_MS),
            ..due
        }));
    }

    /// SPEC-461 CCM-007 (review round 2): npm's newest release is said not to know the default
    /// model only on a fresh, successful read of the registry by a channel that checks.
    #[test]
    fn the_newest_release_is_named_only_from_a_fresh_read() {
        let refused = NewestFacts {
            channel: Channel::Latest,
            read: RegistryRead::Refused(v("2.1.322")),
            read_ms_ago: Some(5 * 60_000),
            latest: Some(v("2.1.322")),
            runs: Some(v("2.1.311")),
        };
        // The newest release was refused for it.
        assert_eq!(newest_lacking_default(&refused), Some(v("2.1.322")));
        // Runs already get the newest release (or a newer one): that is the one named.
        let in_use = NewestFacts {
            read: RegistryRead::Read,
            runs: Some(v("2.1.322")),
            ..refused
        };
        assert_eq!(newest_lacking_default(&in_use), Some(v("2.1.322")));
        let beyond = NewestFacts {
            runs: Some(v("2.1.330")),
            ..in_use
        };
        assert_eq!(newest_lacking_default(&beyond), Some(v("2.1.330")));
        // On `stable`, stable's release was refused and `latest`'s too: the last one tried.
        let stable = NewestFacts {
            channel: Channel::Stable,
            ..refused
        };
        assert_eq!(newest_lacking_default(&stable), Some(v("2.1.322")));

        let unknown = |facts: NewestFacts, why: &str| {
            assert_eq!(newest_lacking_default(&facts), None, "{why}");
        };
        // A newer release than the one runs get was read and nothing refused: the next check
        // (or the host's install) takes it.
        unknown(
            NewestFacts {
                read: RegistryRead::Read,
                ..refused
            },
            "a newer release exists",
        );
        unknown(
            NewestFacts {
                read: RegistryRead::Refused(v("2.1.318")),
                ..refused
            },
            "an older release than the newest was refused",
        );
        // Nothing reads the registry with updates off: a remembered `latest` is not today's.
        for facts in [refused, in_use, beyond] {
            unknown(
                NewestFacts {
                    channel: Channel::Off,
                    ..facts
                },
                "updates are off",
            );
            unknown(
                NewestFacts {
                    read: RegistryRead::Unknown,
                    ..facts
                },
                "the last check failed, or none ran",
            );
            unknown(
                NewestFacts {
                    read_ms_ago: Some(NEWEST_KNOWN_FOR_MS + 1),
                    ..facts
                },
                "the last read is old",
            );
            unknown(
                NewestFacts {
                    read_ms_ago: None,
                    ..facts
                },
                "no check was made",
            );
            unknown(
                NewestFacts {
                    read_ms_ago: Some(-1),
                    ..facts
                },
                "the check is dated after now",
            );
            unknown(
                NewestFacts {
                    latest: None,
                    ..facts
                },
                "npm's latest was not read",
            );
        }
        unknown(
            NewestFacts {
                runs: None,
                ..in_use
            },
            "the version runs get is unknown",
        );
        assert_eq!(
            newest_lacking_default(&NewestFacts {
                read_ms_ago: Some(NEWEST_KNOWN_FOR_MS),
                ..refused
            }),
            Some(v("2.1.322")),
            "two rounds of the hourly check are still fresh"
        );
    }

    #[test]
    fn linux_hosts_have_a_platform_package() {
        let name = |os, arch, musl| platform_package(os, arch, musl);
        assert_eq!(
            name("linux", "x86_64", false).as_deref(),
            Some("@anthropic-ai/claude-code-linux-x64")
        );
        assert_eq!(
            name("linux", "aarch64", false).as_deref(),
            Some("@anthropic-ai/claude-code-linux-arm64")
        );
        assert_eq!(
            name("linux", "x86_64", true).as_deref(),
            Some("@anthropic-ai/claude-code-linux-x64-musl")
        );
        assert_eq!(name("macos", "aarch64", false), None);
        assert_eq!(name("linux", "riscv64", false), None);
    }

    #[test]
    fn platform_packages_name_their_elf_machine() {
        assert_eq!(elf_machine("@anthropic-ai/claude-code-linux-x64"), Some(62));
        assert_eq!(
            elf_machine("@anthropic-ai/claude-code-linux-x64-musl"),
            Some(62)
        );
        assert_eq!(
            elf_machine("@anthropic-ai/claude-code-linux-arm64"),
            Some(183)
        );
        assert_eq!(
            elf_machine("@anthropic-ai/claude-code-linux-arm64-musl"),
            Some(183)
        );
        assert_eq!(elf_machine("@anthropic-ai/claude-code-darwin-arm64"), None);
    }

    #[test]
    fn only_a_newer_version_is_installed() {
        assert!(should_install(v("2.1.311"), Some(v("2.1.292"))));
        assert!(should_install(v("2.1.311"), None));
        assert!(!should_install(v("2.1.311"), Some(v("2.1.311"))));
        assert!(!should_install(v("2.1.311"), Some(v("2.1.318"))));
    }

    #[test]
    fn the_current_and_the_previous_version_are_kept() {
        let installed = [v("2.1.300"), v("2.1.311"), v("2.1.292"), v("2.1.318")];
        let mut removed = to_remove(&installed, v("2.1.318"));
        removed.sort();
        assert_eq!(removed, vec![v("2.1.292"), v("2.1.300")]);
        // A version above the current one (left by an interrupted switch) goes too.
        assert_eq!(
            to_remove(&[v("2.1.311"), v("2.1.318")], v("2.1.311")),
            vec![v("2.1.318")]
        );
        assert!(to_remove(&[v("2.1.311")], v("2.1.311")).is_empty());
        assert_eq!(KEEP, 2);
    }
}
