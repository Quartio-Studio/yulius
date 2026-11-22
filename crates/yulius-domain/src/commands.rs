//! Brokered commands (SPEC-626, ADR-0459): the rules of a project's command definitions. A
//! command is a CLI that needs an agent secret as a file; agents run it by name and Yulius runs
//! it outside their sandbox. This module checks what an administrator defines.

/// Longest command name.
pub const MAX_NAME: usize = 40;
/// Longest binary path, in bytes.
pub const MAX_BINARY: usize = 512;
/// Most entries of a definition's list (`fixed`, `subcommands`, `refuse`, `send_flags`, `env`,
/// `tasks`).
pub const MAX_ENTRIES: usize = 64;
/// Longest entry of a definition's list, in bytes.
pub const MAX_ENTRY: usize = 4096;
/// The longest description, in bytes.
pub const MAX_DESCRIPTION: usize = 2000;
/// Approval minutes: 1 to this.
pub const MAX_APPROVAL_MINUTES: u32 = 60;
/// The approval minutes when none are given.
pub const DEFAULT_APPROVAL_MINUTES: u32 = 10;
/// A run's time limit: 1 second to this.
pub const MAX_TIMEOUT_SECONDS: u32 = 1800;
/// The time limit when none is given.
pub const DEFAULT_TIMEOUT_SECONDS: u32 = 600;

/// The run's key file.
pub const KEYFILE: &str = "{keyfile}";
/// The run's fresh home.
pub const HOME: &str = "{home}";
/// The key's value, in `env` only.
pub const SECRET: &str = "{secret}";

/// When a run waits for a person (BRK-013).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Approve {
    /// Every run.
    Always,
    /// A run the agent says changes something, or with one of the send flags.
    Send,
    /// No run.
    Never,
}

impl Approve {
    /// Parses `always`, `send` or `never`.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "always" => Some(Self::Always),
            "send" => Some(Self::Send),
            "never" => Some(Self::Never),
            _ => None,
        }
    }

    /// Its name.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Always => "always",
            Self::Send => "send",
            Self::Never => "never",
        }
    }
}

/// Why a command name is refused (BRK-001): 1 to 40 of `a-z`, `0-9` and `-`, not starting with
/// `-`.
#[must_use]
pub fn name_refusal(name: &str) -> Option<String> {
    let valid = !name.is_empty()
        && name.len() <= MAX_NAME
        && !name.starts_with('-')
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-');
    (!valid).then(|| {
        format!(
            "a command's name is 1 to {MAX_NAME} lowercase letters, digits and dashes, not \
             starting with a dash"
        )
    })
}

/// Why a binary path is refused (BRK-001): absolute and normalised, with no `.`, `..` or empty
/// component, at most [`MAX_BINARY`] bytes, no control character.
#[must_use]
pub fn binary_refusal(binary: &str) -> Option<String> {
    let Some(rest) = binary.strip_prefix('/') else {
        return Some("the binary is an absolute path, like /usr/local/bin/deploy".into());
    };
    if binary.len() > MAX_BINARY || binary.chars().any(char::is_control) {
        return Some(format!(
            "the binary is a path of at most {MAX_BINARY} bytes"
        ));
    }
    if rest.is_empty()
        || rest
            .split('/')
            .any(|c| c.is_empty() || c == "." || c == "..")
    {
        return Some("the binary's path has no empty, . or .. part".into());
    }
    None
}

/// Why an environment variable's name is refused (BRK-001): `[A-Za-z_][A-Za-z0-9_]*`, and not
/// one Yulius sets or one that changes how programs load (`PATH`, `HOME`, `TMPDIR`, `LD_*`,
/// `YULIUS_*`).
#[must_use]
pub fn env_name_refusal(name: &str) -> Option<String> {
    let mut bytes = name.bytes();
    let valid = bytes
        .next()
        .is_some_and(|b| b.is_ascii_alphabetic() || b == b'_')
        && bytes.all(|b| b.is_ascii_alphanumeric() || b == b'_')
        && name.len() <= 128;
    if !valid {
        return Some(format!("{} is not a variable name", shown(name)));
    }
    let upper = name.to_ascii_uppercase();
    if matches!(upper.as_str(), "PATH" | "HOME" | "TMPDIR")
        || upper.starts_with("LD_")
        || upper.starts_with("YULIUS_")
    {
        return Some(format!("{name} is set by Yulius"));
    }
    None
}

/// Why a fixed argument or env value is refused for its placeholders (BRK-001): `{keyfile}` and
/// `{home}` anywhere, `{secret}` only in env (`in_env`), no other `{…}` placeholder.
#[must_use]
pub fn placeholder_refusal(value: &str, in_env: bool) -> Option<String> {
    if value.len() > MAX_ENTRY || value.contains('\0') {
        return Some(format!("a value is at most {MAX_ENTRY} bytes, without NUL"));
    }
    let mut rest = value;
    while let Some(start) = rest.find('{') {
        let after = &rest[start..];
        let Some(end) = after.find('}') else {
            break;
        };
        let token = &after[..=end];
        let known = token == KEYFILE || token == HOME || (in_env && token == SECRET);
        let word = token[1..token.len() - 1]
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b':' || b == b'-');
        if !known && word && token.len() > 2 {
            return Some(if token == SECRET {
                "{secret} goes only in env: arguments show in a process list".into()
            } else {
                format!(
                    "{token} is not a placeholder: use {{keyfile}}, {{home}} or, in env, {{secret}}"
                )
            });
        }
        rest = &after[end + 1..];
    }
    None
}

/// Why a flag of `refuse` or `send_flags` is refused: it starts with `-` and has no space or
/// `=`.
#[must_use]
pub fn flag_refusal(flag: &str) -> Option<String> {
    let valid = flag.len() >= 2
        && flag.len() <= 128
        && flag.starts_with('-')
        && flag != "--"
        && !flag.contains(['=', ' '])
        && !flag.chars().any(char::is_control);
    (!valid).then(|| format!("{} is not a flag like -k or --key-file", shown(flag)))
}

/// Why a subcommand of the allowed list is refused: a word that does not start with `-`, has no
/// `/` and no space.
#[must_use]
pub fn subcommand_refusal(word: &str) -> Option<String> {
    let valid = !word.is_empty()
        && word.len() <= 128
        && !word.starts_with('-')
        && !word.contains(['/', ' ', '~'])
        && !word.chars().any(char::is_control);
    (!valid).then(|| format!("{} is not a subcommand", shown(word)))
}

/// A value shown in a refusal: at most 60 characters.
fn shown(text: &str) -> String {
    let cut: String = text.chars().take(60).collect();
    format!("{cut:?}")
}

/// `value` with `{keyfile}` and `{home}` replaced, and `{secret}` when `secret` is given (env
/// only, BRK-009).
#[must_use]
pub fn expand(value: &str, keyfile: &str, home: &str, secret: Option<&str>) -> String {
    let value = value.replace(KEYFILE, keyfile).replace(HOME, home);
    match secret {
        Some(secret) => value.replace(SECRET, secret),
        None => value,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names() {
        for ok in ["deploy", "gcloud-2", "a", "0x"] {
            assert_eq!(name_refusal(ok), None, "{ok}");
        }
        for bad in ["", "-x", "Deploy", "de ploy", "de/ploy", &"a".repeat(41)] {
            assert!(name_refusal(bad).is_some(), "{bad}");
        }
    }

    #[test]
    fn binaries() {
        assert_eq!(binary_refusal("/usr/local/bin/deploy"), None);
        for bad in [
            "deploy",
            "./deploy",
            "~/bin/deploy",
            "/",
            "/usr//bin/x",
            "/usr/../bin/sh",
            "/usr/./bin/x",
            "/usr/bin/",
            "/usr/bin/x\n",
        ] {
            assert!(binary_refusal(bad).is_some(), "{bad:?}");
        }
        assert!(binary_refusal(&format!("/{}", "a".repeat(600))).is_some());
    }

    #[test]
    fn env_names() {
        for ok in ["GOOGLE_APPLICATION_CREDENTIALS", "_X", "api_url"] {
            assert_eq!(env_name_refusal(ok), None, "{ok}");
        }
        for bad in [
            "",
            "1X",
            "A-B",
            "A B",
            "PATH",
            "home",
            "TMPDIR",
            "LD_PRELOAD",
            "ld_library_path",
            "YULIUS_TOKEN",
        ] {
            assert!(env_name_refusal(bad).is_some(), "{bad}");
        }
    }

    #[test]
    fn placeholders() {
        assert_eq!(placeholder_refusal("--key-file={keyfile}", false), None);
        assert_eq!(placeholder_refusal("{home}/.config", false), None);
        assert_eq!(placeholder_refusal("Bearer {secret}", true), None);
        // JSON and braces that are no placeholder stay as they are.
        assert_eq!(placeholder_refusal("{\"a\": 1}", false), None);
        assert_eq!(placeholder_refusal("{}", false), None);
        assert_eq!(placeholder_refusal("x{", false), None);
        let secret = placeholder_refusal("--token={secret}", false);
        assert!(secret.is_some_and(|r| r.contains("only in env")));
        assert!(placeholder_refusal("{url:api}", false).is_some());
        assert!(placeholder_refusal("{key}", true).is_some());
        assert!(placeholder_refusal("a\0b", false).is_some());
    }

    #[test]
    fn flags_and_subcommands() {
        for ok in ["-k", "--key-file", "--send"] {
            assert_eq!(flag_refusal(ok), None, "{ok}");
        }
        for bad in ["k", "-", "--", "--a=b", "--a b"] {
            assert!(flag_refusal(bad).is_some(), "{bad}");
        }
        assert_eq!(subcommand_refusal("push"), None);
        for bad in ["", "-x", "a/b", "a b", "~"] {
            assert!(subcommand_refusal(bad).is_some(), "{bad}");
        }
    }

    #[test]
    fn expansion() {
        assert_eq!(
            expand("--key={keyfile} {home} {secret}", "/k", "/h", None),
            "--key=/k /h {secret}"
        );
        assert_eq!(expand("T={secret}", "/k", "/h", Some("v")), "T=v");
    }

    #[test]
    fn approve_modes() {
        for mode in ["always", "send", "never"] {
            assert_eq!(Approve::parse(mode).map(Approve::as_str), Some(mode));
        }
        assert_eq!(Approve::parse("ask"), None);
    }
}
