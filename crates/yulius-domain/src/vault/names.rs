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
    // What the agent CLI and the Anthropic SDKs read; other `ANTHROPIC_` names are a person's own,
    // such as a second key for the app a project builds (ADR-0292).
    "ANTHROPIC_API_KEY",
    "ANTHROPIC_AUTH_TOKEN",
    "ANTHROPIC_BASE_URL",
    "ANTHROPIC_MODEL",
    "ANTHROPIC_BETAS",
    "ANTHROPIC_LOG",
    "ANTHROPIC_UNIX_SOCKET",
];

/// Prefixes owned by Yulius, Claude Code (and its Anthropic families), Git, TLS libraries, the dynamic linker and shells.
const RESERVED_PREFIXES: &[&str] = &[
    "YULIUS_",
    "CLAUDE_",
    "ANTHROPIC_CUSTOM_",
    "ANTHROPIC_DEFAULT_",
    "ANTHROPIC_SMALL_FAST_MODEL",
    "ANTHROPIC_BEDROCK_",
    "ANTHROPIC_VERTEX_",
    "ANTHROPIC_FOUNDRY_",
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
        for good in [
            "STRIPE_API_KEY",
            "A",
            "OPENAI_KEY_2",
            "NPM_TOKEN",
            "GH_TOKEN",
        ] {
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

    #[test]
    fn own_anthropic_names_are_accepted_and_the_cli_s_are_reserved() {
        for own in [
            "ANTHROPIC_API_KEY_QUARTIO",
            "ANTHROPIC_KEY_PROD",
            "ANTHROPIC_TOKEN",
            "ANTHROPIC_MODEL_FOR_APP",
        ] {
            assert!(SecretName::parse(own).is_ok(), "{own}");
        }
        for read in [
            "ANTHROPIC_API_KEY",
            "ANTHROPIC_AUTH_TOKEN",
            "ANTHROPIC_BASE_URL",
            "ANTHROPIC_MODEL",
            "ANTHROPIC_BETAS",
            "ANTHROPIC_LOG",
            "ANTHROPIC_UNIX_SOCKET",
            "ANTHROPIC_CUSTOM_HEADERS",
            "ANTHROPIC_DEFAULT_OPUS_MODEL",
            "ANTHROPIC_SMALL_FAST_MODEL",
            "ANTHROPIC_SMALL_FAST_MODEL_AWS_REGION",
            "ANTHROPIC_BEDROCK_BASE_URL",
            "ANTHROPIC_VERTEX_PROJECT_ID",
            "ANTHROPIC_FOUNDRY_API_KEY",
        ] {
            let refused = SecretName::parse(read);
            assert!(
                matches!(&refused, Err(VaultError::Name(m)) if m.contains("reserved")),
                "{read}: {refused:?}"
            );
        }
    }
}
