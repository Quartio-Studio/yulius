//! Agent-secret names: environment-variable style, never one Yulius or the agent's tools rely on.

use super::VaultError;

/// Longest secret name.
pub const MAX_NAME_LEN: usize = 64;

/// Names that the agent's environment, shell or Yulius itself defines.
const RESERVED: &[&str] = &[
    "PATH",
    "HOME",
    "USER",
    "LOGNAME",
    "SHELL",
    "TERM",
    "COLORTERM",
    "LANG",
    "LANGUAGE",
    "PWD",
    "OLDPWD",
    "TMPDIR",
    "TMP",
    "TEMP",
    "PORT",
    "HOSTNAME",
    "EDITOR",
    "VISUAL",
    "PAGER",
    "IFS",
    "PS1",
    "PS2",
    "PROMPT_COMMAND",
    "ENV",
    "BASH_ENV",
    "CDPATH",
    "HTTP_PROXY",
    "HTTPS_PROXY",
    "ALL_PROXY",
    "NO_PROXY",
    "FTP_PROXY",
    "REQUESTS_CA_BUNDLE",
    "CURL_CA_BUNDLE",
    "DISABLE_AUTOUPDATER",
];

/// Prefixes owned by Yulius, Claude Code, Git, TLS libraries, the dynamic linker and shells.
const RESERVED_PREFIXES: &[&str] = &[
    "YULIUS_",
    "CLAUDE_",
    "ANTHROPIC_",
    "GIT_",
    "SSL_",
    "NODE_",
    "NPM_CONFIG_",
    "LD_",
    "DYLD_",
    "XDG_",
    "LC_",
    "BASH_",
    "ZSH_",
];

/// A validated agent-secret name (`[A-Z][A-Z0-9_]*`, at most 64 characters, not reserved).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SecretName(String);

impl SecretName {
    /// Validates `text`.
    ///
    /// # Errors
    /// [`VaultError::Name`] for a malformed or reserved name.
    pub fn parse(text: &str) -> Result<Self, VaultError> {
        let text = text.trim();
        let mut bytes = text.bytes();
        let shape = text.len() <= MAX_NAME_LEN
            && bytes.next().is_some_and(|b| b.is_ascii_uppercase())
            && bytes.all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_');
        if !shape {
            return Err(VaultError::Name(format!(
                "invalid name {text:?}: capital letters, digits and _, starting with a letter (like STRIPE_API_KEY), at most {MAX_NAME_LEN} characters"
            )));
        }
        if RESERVED.contains(&text) || RESERVED_PREFIXES.iter().any(|p| text.starts_with(p)) {
            return Err(VaultError::Name(format!(
                "{text} is reserved for the agent's environment; choose another name"
            )));
        }
        Ok(Self(text.to_owned()))
    }

    /// The name.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for SecretName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_environment_style_and_not_reserved() {
        for good in ["STRIPE_API_KEY", "A", "OPENAI_KEY_2", "NPM_TOKEN", "GH_TOKEN"] {
            assert!(SecretName::parse(good).is_ok(), "{good}");
        }
        for bad in [
            "",
            "stripe",
            "1KEY",
            "_KEY",
            "KEY-1",
            "KEY 1",
            "PATH",
            "HTTPS_PROXY",
            "YULIUS_TASK_TOKEN",
            "CLAUDE_CODE_OAUTH_TOKEN",
            "LD_PRELOAD",
            "GIT_ASKPASS",
            "NODE_OPTIONS",
        ] {
            assert!(SecretName::parse(bad).is_err(), "{bad}");
        }
        assert!(SecretName::parse(&"A".repeat(MAX_NAME_LEN + 1)).is_err());
        assert_eq!(
            SecretName::parse("  KEY ").map(|n| n.to_string()),
            Ok("KEY".into())
        );
    }
}
