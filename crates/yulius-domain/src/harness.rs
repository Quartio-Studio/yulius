//! Supported coding-agent harnesses and providers (SPEC-001 TSK-001..003).

use std::fmt;

/// A coding-agent CLI Yulius can run as a task. Only Claude Code and Codex are supported now;
/// new variants require an adapter (SPEC-001 TSK-003).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Harness {
    /// Anthropic Claude Code CLI.
    ClaudeCode,
    /// OpenAI Codex CLI.
    Codex,
}

/// The subscription provider behind a harness; accounts belong to providers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Provider {
    /// Anthropic (Claude subscriptions).
    Anthropic,
    /// OpenAI (ChatGPT/Codex subscriptions).
    OpenAi,
}

/// Unknown harness or provider text.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("unknown harness or provider: {0}")]
pub struct UnknownHarness(pub String);

impl Harness {
    /// All supported harnesses.
    pub const ALL: [Self; 2] = [Self::ClaudeCode, Self::Codex];

    /// The provider whose accounts this harness uses.
    #[must_use]
    pub fn provider(self) -> Provider {
        match self {
            Self::ClaudeCode => Provider::Anthropic,
            Self::Codex => Provider::OpenAi,
        }
    }

    /// Stable wire name (`claude` or `codex`).
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ClaudeCode => "claude",
            Self::Codex => "codex",
        }
    }

    /// Parses a wire name.
    ///
    /// # Errors
    /// Returns [`UnknownHarness`] for anything other than `claude` or `codex`.
    pub fn parse(text: &str) -> Result<Self, UnknownHarness> {
        match text {
            "claude" => Ok(Self::ClaudeCode),
            "codex" => Ok(Self::Codex),
            other => Err(UnknownHarness(other.to_owned())),
        }
    }
}

impl Provider {
    /// The harness that consumes this provider's accounts.
    #[must_use]
    pub fn harness(self) -> Harness {
        match self {
            Self::Anthropic => Harness::ClaudeCode,
            Self::OpenAi => Harness::Codex,
        }
    }

    /// Stable wire name (`anthropic` or `openai`).
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Anthropic => "anthropic",
            Self::OpenAi => "openai",
        }
    }

    /// Parses a wire name.
    ///
    /// # Errors
    /// Returns [`UnknownHarness`] for anything other than `anthropic` or `openai`.
    pub fn parse(text: &str) -> Result<Self, UnknownHarness> {
        match text {
            "anthropic" => Ok(Self::Anthropic),
            "openai" => Ok(Self::OpenAi),
            other => Err(UnknownHarness(other.to_owned())),
        }
    }
}

impl fmt::Display for Harness {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl fmt::Display for Provider {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn harness_provider_round_trip() {
        for harness in Harness::ALL {
            assert_eq!(harness.provider().harness(), harness);
            assert_eq!(Harness::parse(harness.as_str()), Ok(harness));
            assert_eq!(
                Provider::parse(harness.provider().as_str()),
                Ok(harness.provider())
            );
        }
        assert!(Harness::parse("opencode").is_err());
    }
}
