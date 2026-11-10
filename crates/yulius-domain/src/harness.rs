//! Supported coding-agent harnesses and providers (SPEC-001 TSK-001..003).

use std::fmt;

/// The Claude model of a task that names none: Opus 5.5 by its full id, not Claude Code's `opus`
/// alias, which moves with each release (F 2026-10-12: "it should be opus-5.5"; task #21: "the
/// default model is always opus 5.5"). A subscription's own default would be Sonnet.
pub const DEFAULT_CLAUDE_MODEL: &str = "claude-opus-5-5";

/// The effort of a task that names none (task #21), sent so Claude Code never picks its own.
pub const DEFAULT_CLAUDE_EFFORT: &str = "high";

/// The effort a task runs at: its own, else [`DEFAULT_CLAUDE_EFFORT`], except for a Haiku model,
/// which takes no effort level.
#[must_use]
pub fn claude_effort<'a>(model: &str, effort: Option<&'a str>) -> Option<&'a str> {
    effort.or_else(|| {
        (!model.to_ascii_lowercase().contains("haiku")).then_some(DEFAULT_CLAUDE_EFFORT)
    })
}

/// Whether `text` is Claude Code's compaction command as typed: `/compact`, alone or with what to
/// keep (`/compact keep the plan`). Claude Code compacts the conversation and runs no turn: no
/// model work, no Stop hook. So the command starts no work, and the task stays as it was
/// (SPEC-085 STT-005).
#[must_use]
pub fn is_compact_command(text: &str) -> bool {
    text.trim()
        .strip_prefix("/compact")
        .is_some_and(|rest| rest.is_empty() || rest.starts_with(char::is_whitespace))
}

/// A coding-agent CLI Yulius can run as a task (SPEC-001 TSK-003). Claude Code, and Codex
/// again (SPEC-451, ADR-0290; proposed to reverse F 2026-10-08). A new harness needs a variant
/// here and an adapter.
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
    /// OpenAI (ChatGPT subscriptions or API keys, used by Codex).
    OpenAi,
}

/// Unknown or unsupported harness or provider text.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("unsupported harness or provider: {0} (Yulius runs Claude Code and Codex)")]
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

    /// Human-readable product name.
    #[must_use]
    pub fn display_name(self) -> &'static str {
        match self {
            Self::ClaudeCode => "Claude Code",
            Self::Codex => "Codex",
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

/// `true` for provider session ids Yulius accepts in paths and argv: 1–128 ASCII letters,
/// digits, `-` or `_`, not starting with `-`.
#[must_use]
pub fn valid_session_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && !id.starts_with('-')
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

#[cfg(test)]
mod tests {
    use super::*;

    /// SPEC-085 STT-005: the compaction command as typed, not a prompt that mentions it.
    #[test]
    fn the_compaction_command_is_known_as_typed() {
        for typed in [
            "/compact",
            "/compact keep the plan",
            "  /compact\n",
            "/compact\tx",
        ] {
            assert!(is_compact_command(typed), "{typed:?}");
        }
        for prompt in [
            "/compactor is broken",
            "please /compact",
            "/compaction",
            "compact it",
            "/goal x",
            "",
        ] {
            assert!(!is_compact_command(prompt), "{prompt:?}");
        }
    }

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
    }

    /// SPEC-451 CDX-001: Codex and its provider are accepted again; others stay unsupported.
    #[test]
    fn codex_is_a_harness_and_other_harnesses_are_unsupported() {
        assert_eq!(Harness::parse("codex"), Ok(Harness::Codex));
        assert_eq!(Provider::parse("openai"), Ok(Provider::OpenAi));
        assert_eq!(Harness::Codex.provider(), Provider::OpenAi);
        assert_eq!(Harness::Codex.display_name(), "Codex");
        assert!(Harness::parse("opencode").is_err());
        assert!(Provider::parse("google").is_err());
    }
}
