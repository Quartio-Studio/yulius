//! Claude Code kept current by Yulius (SPEC-421, ADR-0266): which release channel, which
//! versions, which platform package, which installed versions to keep. [`codex`]: the Codex CLI
//! release Yulius provides to Codex runs (SPEC-451 CDX-012).
//!
//! Pure: the caller reads the registry, downloads, installs and removes.

use std::fmt;

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

/// Which releases an instance follows (`YULIUS_CLAUDE_UPDATES`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Channel {
    /// npm's `stable` tag: about a week behind `latest`, without releases that regressed.
    #[default]
    Stable,
    /// npm's `latest` tag.
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

/// The Codex CLI Yulius provides to Codex runs and sign-ins (SPEC-451 CDX-012, CLI-001): one
/// release pinned with the sha512 of each platform package, so what runs is what the harness was
/// checked against. Bumping it means checking the harness against the new release.
pub mod codex {
    /// The npm package; its platform packages are versions of it (`0.167.0-linux-x64`).
    pub const PACKAGE: &str = "@openai/codex";
    /// The pinned release.
    pub const VERSION: &str = "0.167.0";
    /// The native binary below an installed release's directory.
    pub const BINARY: &str = "bin/codex";

    /// A platform package of the pinned release.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct Platform {
        /// npm's suffix of the version (`linux-x64`).
        pub suffix: &'static str,
        /// Rust's target triple naming its `vendor/` directory.
        pub triple: &'static str,
        /// ELF `e_machine` of its binary.
        pub machine: u16,
        /// npm's `integrity` of its tarball, as published for [`VERSION`].
        pub integrity: &'static str,
    }

    impl Platform {
        /// Path of its tarball on the registry.
        #[must_use]
        pub fn tarball_path(&self) -> String {
            format!("/{PACKAGE}/-/codex-{VERSION}-{}.tgz", self.suffix)
        }

        /// The tarball's directory holding the release (`bin/codex`, `codex-path`, …).
        #[must_use]
        pub fn vendor_dir(&self) -> String {
            format!("package/vendor/{}/", self.triple)
        }

        /// Name of the installed release's directory.
        #[must_use]
        pub fn install_name(&self) -> String {
            format!("{VERSION}-{}", self.suffix)
        }
    }

    /// The platform package for Rust's `os` and `arch` names; `None` where Yulius provides none.
    /// Codex's Linux builds are static (musl), so the host's C library does not matter.
    #[must_use]
    pub fn platform(os: &str, arch: &str) -> Option<Platform> {
        match (os, arch) {
            ("linux", "x86_64") => Some(Platform {
                suffix: "linux-x64",
                triple: "x86_64-unknown-linux-musl",
                machine: 62,
                integrity: "sha512-FNc7danfUng9Sp6w+Fh7kofNHHZpA6VMyNKWsWrS3OwHly69gywTWeiuwrUPXpJSho8Df4xqRsHfHRPzj7zYQg==",
            }),
            ("linux", "aarch64") => Some(Platform {
                suffix: "linux-arm64",
                triple: "aarch64-unknown-linux-musl",
                machine: 183,
                integrity: "sha512-5cv+sybRWqpRJL6KCv3YSnIx1GHRmjek997C38n1Zfno4HomKDLkNSjWLDOOc2+ISlVNSRRnySWq4zKmzlY12g==",
            }),
            _ => None,
        }
    }
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
        assert_eq!(Channel::default(), Channel::Stable);
        assert_eq!(Channel::Stable.dist_tag(), Some("stable"));
        assert_eq!(Channel::Latest.dist_tag(), Some("latest"));
        assert_eq!(Channel::Off.dist_tag(), None);
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

    #[test]
    fn the_pinned_codex_release_has_linux_packages() {
        let x64 = codex::platform("linux", "x86_64").unwrap_or_else(|| panic!("x64"));
        assert_eq!(
            x64.tarball_path(),
            "/@openai/codex/-/codex-0.167.0-linux-x64.tgz"
        );
        assert_eq!(
            x64.vendor_dir(),
            "package/vendor/x86_64-unknown-linux-musl/"
        );
        assert_eq!(x64.install_name(), "0.167.0-linux-x64");
        assert_eq!(x64.machine, 62);
        let arm = codex::platform("linux", "aarch64").unwrap_or_else(|| panic!("arm64"));
        assert_eq!(
            arm.vendor_dir(),
            "package/vendor/aarch64-unknown-linux-musl/"
        );
        assert_eq!(arm.machine, 183);
        for platform in [x64, arm] {
            assert!(platform.integrity.starts_with("sha512-"), "{platform:?}");
        }
        assert_eq!(codex::platform("macos", "aarch64"), None);
        assert_eq!(codex::platform("linux", "riscv64"), None);
        assert!(Version::parse(codex::VERSION).is_some());
    }
}
