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

/// A person's own Anthropic keys, such as `ANTHROPIC_API_KEY_QUARTIO` for the app a project builds:
/// `ANTHROPIC_` stays reserved, but the agent CLI reads no name that extends these, and a `_FILE`
/// name would by convention point it at a credential file (ADR-0292).
const OWN_ANTHROPIC_KEYS: [&str; 2] = ["ANTHROPIC_API_KEY_", "ANTHROPIC_AUTH_TOKEN_"];

/// The reserved prefix `text` starts with, if it is not one of [`OWN_ANTHROPIC_KEYS`].
fn reserved_prefix(text: &str) -> Option<&'static str> {
    let own = OWN_ANTHROPIC_KEYS
        .iter()
        .any(|key| text.len() > key.len() && text.starts_with(key))
        && !text.ends_with("_FILE");
    if own {
        return None;
    }
    RESERVED_PREFIXES
        .iter()
        .copied()
        .find(|p| text.starts_with(p))
}

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
        // Name what is reserved and a name that works, so nobody guesses another of the same start.
        let instead = format!("APP_{text}");
        let instead = if instead.len() <= MAX_NAME_LEN {
            format!(", such as {instead}")
        } else {
            String::new()
        };
        if RESERVED.contains(&text) {
            return Err(VaultError::Name(format!(
                "{text} is reserved for the agent's environment; choose another name{instead}"
            )));
        }
        if let Some(prefix) = reserved_prefix(text) {
            let own = if prefix == "ANTHROPIC_" {
                ", except your own ANTHROPIC_API_KEY_<NAME> or ANTHROPIC_AUTH_TOKEN_<NAME>"
            } else {
                ""
            };
            return Err(VaultError::Name(format!(
                "{text} is reserved for the agent's environment, as is every name starting with {prefix}{own}; choose another name{instead}"
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
    fn own_anthropic_keys_are_accepted_and_every_other_anthropic_name_is_reserved() {
        for own in [
            "ANTHROPIC_API_KEY_QUARTIO",
            "ANTHROPIC_AUTH_TOKEN_PROD",
            "ANTHROPIC_API_KEY_2",
        ] {
            assert!(SecretName::parse(own).is_ok(), "{own}");
        }
        for read in CLI_READS.iter().chain(&[
            "ANTHROPIC_API_KEY_",
            "ANTHROPIC_API_KEY_FILE",
            "ANTHROPIC_AUTH_TOKEN_FILE",
            "ANTHROPIC_KEY_PROD",
        ]) {
            let refused = SecretName::parse(read);
            assert!(
                matches!(&refused, Err(VaultError::Name(m)) if m.contains("reserved")),
                "{read}: {refused:?}"
            );
        }
    }

    #[test]
    fn a_refusal_names_the_reserved_prefix_and_a_name_that_works() {
        let Err(VaultError::Name(message)) = SecretName::parse("ANTHROPIC_KEY_PROD") else {
            panic!("ANTHROPIC_KEY_PROD is reserved");
        };
        assert!(message.contains("starting with ANTHROPIC_"), "{message}");
        assert!(message.contains("ANTHROPIC_API_KEY_<NAME>"), "{message}");
        assert!(message.contains("APP_ANTHROPIC_KEY_PROD"), "{message}");
        let Err(VaultError::Name(message)) = SecretName::parse("GIT_TOKEN") else {
            panic!("GIT_TOKEN is reserved");
        };
        assert!(message.contains("starting with GIT_"), "{message}");
        assert!(!message.contains("ANTHROPIC"), "{message}");
        for reserved in ["PATH", "GIT_TOKEN", "ANTHROPIC_KEY_PROD"] {
            let suggested = format!("APP_{reserved}");
            assert!(SecretName::parse(&suggested).is_ok(), "{suggested}");
        }
        let Err(VaultError::Name(message)) = SecretName::parse("ANTHROPIC_API_KEY_FILE") else {
            panic!("ANTHROPIC_API_KEY_FILE is reserved");
        };
        assert!(message.contains("not ending in _FILE"), "{message}");
        let at_limit = format!("GIT_{}", "A".repeat(MAX_NAME_LEN - 8));
        let Err(VaultError::Name(message)) = SecretName::parse(&at_limit) else {
            panic!("{at_limit} is reserved");
        };
        assert!(message.contains(&format!("such as APP_{at_limit}")), "{message}");
        assert!(SecretName::parse(&format!("APP_{at_limit}")).is_ok());
        let long = format!("GIT_{}", "A".repeat(MAX_NAME_LEN - 4));
        let Err(VaultError::Name(message)) = SecretName::parse(&long) else {
            panic!("{long} is reserved");
        };
        assert!(!message.contains("such as"), "{message}");
    }

    /// The `ANTHROPIC_` names in the agent CLI's strings (Claude Code 2.1.292, ADR-0292).
    const CLI_READS: &[&str] = &[
        "ANTHROPIC_API_HOST",
        "ANTHROPIC_API_KEY",
        "ANTHROPIC_ASSETS_HOST",
        "ANTHROPIC_AUTH_TOKEN",
        "ANTHROPIC_AWS",
        "ANTHROPIC_AWS_API_KEY",
        "ANTHROPIC_AWS_AUTH",
        "ANTHROPIC_AWS_BASE_URL",
        "ANTHROPIC_AWS_WORKSPACE_ID",
        "ANTHROPIC_BASE_URL",
        "ANTHROPIC_BEDROCK_BASE_URL",
        "ANTHROPIC_BEDROCK_MANTLE_BASE_URL",
        "ANTHROPIC_BEDROCK_REGION_PREFIX",
        "ANTHROPIC_BEDROCK_SERVICE_TIER",
        "ANTHROPIC_BETAS",
        "ANTHROPIC_CONFIG_DIR",
        "ANTHROPIC_CONTROL_PLANE_5XX",
        "ANTHROPIC_CUSTOM_HEADERS",
        "ANTHROPIC_CUSTOM_MODEL_OPTION",
        "ANTHROPIC_CUSTOM_MODEL_OPTION_DESCRIPTION",
        "ANTHROPIC_CUSTOM_MODEL_OPTION_NAME",
        "ANTHROPIC_CUSTOM_MODEL_OPTION_SUPPORTED_CAPABILITIES",
        "ANTHROPIC_DEFAULT_",
        "ANTHROPIC_DEFAULT_FABLE_MODEL",
        "ANTHROPIC_DEFAULT_FABLE_MODEL_DESCRIPTION",
        "ANTHROPIC_DEFAULT_FABLE_MODEL_NAME",
        "ANTHROPIC_DEFAULT_FABLE_MODEL_SUPPORTED_CAPABILITIES",
        "ANTHROPIC_DEFAULT_FABLE_MODEL_SUPPORTED_CAPABILITIES3",
        "ANTHROPIC_DEFAULT_HAIKU_MODEL",
        "ANTHROPIC_DEFAULT_HAIKU_MODEL_DESCRIPTION",
        "ANTHROPIC_DEFAULT_HAIKU_MODEL_NAME",
        "ANTHROPIC_DEFAULT_HAIKU_MODEL_SUPPORTED_CAPABILITIES",
        "ANTHROPIC_DEFAULT_HAIKU_MODEL_SUPPORTED_CAPABILITIES4",
        "ANTHROPIC_DEFAULT_MODEL",
        "ANTHROPIC_DEFAULT_OPUS_MODEL",
        "ANTHROPIC_DEFAULT_OPUS_MODEL_DESCRIPTION",
        "ANTHROPIC_DEFAULT_OPUS_MODEL_NAME",
        "ANTHROPIC_DEFAULT_OPUS_MODEL_SUPPORTED_CAPABILITIES",
        "ANTHROPIC_DEFAULT_SONNET_MODEL",
        "ANTHROPIC_DEFAULT_SONNET_MODEL_DESCRIPTION",
        "ANTHROPIC_DEFAULT_SONNET_MODEL_NAME",
        "ANTHROPIC_DEFAULT_SONNET_MODEL_SUPPORTED_CAPABILITIES",
        "ANTHROPIC_ENVIRONMENT_ID",
        "ANTHROPIC_ENVIRONMENT_KEY",
        "ANTHROPIC_FEDERATION_RULE_ID",
        "ANTHROPIC_FOUNDRY_API_KEY",
        "ANTHROPIC_FOUNDRY_AUTH_TOKEN",
        "ANTHROPIC_FOUNDRY_BASE_URL",
        "ANTHROPIC_FOUNDRY_RESOURCE",
        "ANTHROPIC_GOOGLE_CLOUD",
        "ANTHROPIC_GOOGLE_CLOUD_AUTH",
        "ANTHROPIC_GOOGLE_CLOUD_BASE_URL",
        "ANTHROPIC_GOOGLE_CLOUD_LOCATION",
        "ANTHROPIC_GOOGLE_CLOUD_PROJECT",
        "ANTHROPIC_GOOGLE_CLOUD_WORKSPACE_ID",
        "ANTHROPIC_IDENTITY_TOKEN",
        "ANTHROPIC_IDENTITY_TOKEN_FILE",
        "ANTHROPIC_LOG",
        "ANTHROPIC_MODEL",
        "ANTHROPIC_ORGANIZATION_ID",
        "ANTHROPIC_PROFILE",
        "ANTHROPIC_SCOPE",
        "ANTHROPIC_SECRET_PLACEHOLDER_",
        "ANTHROPIC_SERVICE_ACCOUNT_ID",
        "ANTHROPIC_SESSION_ID",
        "ANTHROPIC_SMALL_FAST_MODEL",
        "ANTHROPIC_SMALL_FAST_MODEL_AWS_REGION",
        "ANTHROPIC_TELEMETRY_GROUP",
        "ANTHROPIC_TIER_NAMES",
        "ANTHROPIC_UNIX_SOCKET",
        "ANTHROPIC_VERTEX_BASE_URL",
        "ANTHROPIC_VERTEX_PROJECT_ID",
        "ANTHROPIC_WEBHOOK_SIGNING_KEY",
        "ANTHROPIC_WORKSPACE_ID",
        "ANTHROPIC_WORK_ID",
        "ANTHROPIC_WORK_SECRET",
    ];
}
