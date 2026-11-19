//! Supported coding-agent harnesses and providers (SPEC-001 TSK-001..003).

use std::fmt;

/// The Claude model of a task that names none: Opus 5.5 by its full id, not an alias that moves
/// with each release (the person's decision of 2026-11-13, SPEC-493: Opus 5.5 writes, Fable 5.2
/// reviews; Fable 5.2 from 2026-11-10 until then, Opus 5.5 before, task #21). A subscription's
/// own default would be Sonnet.
pub const DEFAULT_CLAUDE_MODEL: &str = "claude-opus-5-5";

/// Fable 5.2 by its full id: the model the person chose for the independent code review's
/// first two rounds (SPEC-493; the reviewer's own slice of that spec makes reviews run it).
/// Until then it is the Fable model the capacity view and the pages watch for the fallback of
/// tasks set to Fable. One of [`OFFERED_CLAUDE_MODELS`], so a Claude Code release is read for
/// it.
pub const REVIEW_CLAUDE_MODEL: &str = "claude-fable-5-2";

/// The Claude models the composer offers (SPEC-461 CCM-002), the default first. The web UI's
/// `models.js` lists the same ids, and a test keeps the two equal. A Claude Code release is read
/// for them before sandboxes get it.
pub const OFFERED_CLAUDE_MODELS: [&str; 5] = [
    DEFAULT_CLAUDE_MODEL,
    REVIEW_CLAUDE_MODEL,
    "claude-fable-5-1",
    "claude-sonnet-5-5",
    "claude-haiku-4-5",
];

/// The model a run falls back to while no subscription can run its Fable model (SPEC-482,
/// "add fallback for opus 5.5 if fable is done everywhere", 2026-11-12): Opus 5.5 by its full
/// id, which is the default model since SPEC-493. One of [`OFFERED_CLAUDE_MODELS`], so a Claude
/// Code release is read for it.
pub const FALLBACK_CLAUDE_MODEL: &str = "claude-opus-5-5";

/// The model a run of `model` is launched with instead while no subscription can run `model`:
/// [`FALLBACK_CLAUDE_MODEL`] for a Fable model, whose weekly allowance of usage credits can be
/// used up on every subscription while their other windows have room. `None` for any other
/// model, the default one included: its task waits.
#[must_use]
pub fn fallback_model(model: &str) -> Option<&'static str> {
    (crate::usage::ModelFamily::of(Some(model)) == crate::usage::ModelFamily::Fable)
        .then_some(FALLBACK_CLAUDE_MODEL)
}

/// A Claude model id as people say it: "Opus 5.5" for `claude-opus-5-5`.
#[must_use]
pub fn claude_model_name(id: &str) -> String {
    let rest = id.strip_prefix("claude-").unwrap_or(id);
    let (family, version) = rest.split_once('-').unwrap_or((rest, ""));
    let mut letters = family.chars();
    let family: String = letters
        .next()
        .map(|first| first.to_uppercase().chain(letters).collect())
        .unwrap_or_default();
    if version.is_empty() {
        family
    } else {
        format!("{family} {}", version.replace('-', "."))
    }
}

/// The effort of a task that names none, sent so Claude Code never picks its own: high (the
/// person's decision of 2026-11-11, SPEC-462: "Not max, not xhigh"; max for one day before it,
/// high from task #21 until then).
pub const DEFAULT_CLAUDE_EFFORT: &str = "high";

/// The model of an Antigravity task that names none: Gemini 4 Carbon (the person's request of
/// 2026-11-17, SPEC-505: "Use Gemini 4 Carbon high by default").
pub const DEFAULT_ANTIGRAVITY_MODEL: &str = "gemini-4-carbon";

/// The effort of an Antigravity task that names none: high (SPEC-505).
pub const DEFAULT_ANTIGRAVITY_EFFORT: &str = "high";

/// The model of a Codex task that names none: GPT-6.2 Astra (the person's request of 2026-11-19,
/// SPEC-511: "default model for codex should be astra 6.2 effort high"; the account's own default
/// before, SPEC-503). Codex's id for it, served to Codex CLI 0.166.0 and later.
pub const DEFAULT_CODEX_MODEL: &str = "gpt-6.2-astra";

/// The effort of a Codex task that names none: high (SPEC-511).
pub const DEFAULT_CODEX_EFFORT: &str = "high";

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

/// A coding-agent CLI Yulius can run as a task (SPEC-001 TSK-003). Claude Code, Codex again
/// (SPEC-503, ADR-0355: F 2026-11-16 reverses the Codex part of F 2026-10-08) and Google's
/// Antigravity CLI (SPEC-505, ADR-0361: F 2026-11-17). A new harness needs a variant here and an
/// adapter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Harness {
    /// Anthropic Claude Code CLI.
    ClaudeCode,
    /// OpenAI Codex CLI.
    Codex,
    /// Google Antigravity CLI (`agy`, SPEC-505).
    Antigravity,
}

/// The subscription provider behind a harness; accounts belong to providers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Provider {
    /// Anthropic (Claude subscriptions).
    Anthropic,
    /// OpenAI (ChatGPT subscriptions or API keys, used by Codex).
    OpenAi,
    /// Google (Google sign-ins or Gemini API keys, used by Antigravity; SPEC-505).
    Google,
}

/// Unknown or unsupported harness or provider text.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("unsupported harness or provider: {0} (Yulius runs Claude Code, Codex and Antigravity)")]
pub struct UnknownHarness(pub String);

impl Harness {
    /// All supported harnesses.
    pub const ALL: [Self; 3] = [Self::ClaudeCode, Self::Codex, Self::Antigravity];

    /// The agent a model given without one implies: Antigravity for a Gemini model, else Claude
    /// Code (Codex runs only when chosen; SPEC-505 RND-001).
    #[must_use]
    pub fn implied_by(model: &str) -> Self {
        if model.trim().to_ascii_lowercase().starts_with("gemini-") {
            Self::Antigravity
        } else {
            Self::ClaudeCode
        }
    }

    /// The agent of a task created without one (SPEC-514 PIN-001/002, ADR-0368): the one `model`
    /// implies when a model is given, else `caller`'s (the task whose agent created it), else
    /// Claude Code. It is never drawn, so it does not change from one task to the next.
    #[must_use]
    pub fn unnamed(model: Option<&str>, caller: Option<Self>) -> Self {
        match model.map(str::trim).filter(|m| !m.is_empty()) {
            Some(model) => Self::implied_by(model),
            None => caller.unwrap_or(Self::ClaudeCode),
        }
    }

    /// The provider whose accounts this harness uses.
    #[must_use]
    pub fn provider(self) -> Provider {
        match self {
            Self::ClaudeCode => Provider::Anthropic,
            Self::Codex => Provider::OpenAi,
            Self::Antigravity => Provider::Google,
        }
    }

    /// Stable wire name (`claude`, `codex` or `antigravity`).
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ClaudeCode => "claude",
            Self::Codex => "codex",
            Self::Antigravity => "antigravity",
        }
    }

    /// Human-readable product name.
    #[must_use]
    pub fn display_name(self) -> &'static str {
        match self {
            Self::ClaudeCode => "Claude Code",
            Self::Codex => "Codex",
            Self::Antigravity => "Antigravity",
        }
    }

    /// Parses a wire name.
    ///
    /// # Errors
    /// Returns [`UnknownHarness`] for anything other than `claude`, `codex` or `antigravity`.
    pub fn parse(text: &str) -> Result<Self, UnknownHarness> {
        match text {
            "claude" => Ok(Self::ClaudeCode),
            "codex" => Ok(Self::Codex),
            "antigravity" => Ok(Self::Antigravity),
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
            Self::Google => Harness::Antigravity,
        }
    }

    /// Stable wire name (`anthropic`, `openai` or `google`).
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Anthropic => "anthropic",
            Self::OpenAi => "openai",
            Self::Google => "google",
        }
    }

    /// Parses a wire name.
    ///
    /// # Errors
    /// Returns [`UnknownHarness`] for anything other than `anthropic`, `openai` or `google`.
    pub fn parse(text: &str) -> Result<Self, UnknownHarness> {
        match text {
            "anthropic" => Ok(Self::Anthropic),
            "openai" => Ok(Self::OpenAi),
            "google" => Ok(Self::Google),
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

    /// SPEC-514 PIN-001/002: a task without an agent takes its model's agent, else its caller's,
    /// else Claude Code; a model's agent is Antigravity for Gemini, else Claude Code.
    #[test]
    fn a_task_without_an_agent_takes_its_models_or_its_callers_or_claude_code() {
        assert_eq!(Harness::unnamed(None, None), Harness::ClaudeCode);
        assert_eq!(Harness::unnamed(Some("  "), None), Harness::ClaudeCode);
        assert_eq!(
            Harness::unnamed(None, Some(Harness::Antigravity)),
            Harness::Antigravity
        );
        assert_eq!(Harness::unnamed(None, Some(Harness::Codex)), Harness::Codex);
        assert_eq!(
            Harness::unnamed(Some("claude-sonnet-5-5"), Some(Harness::Antigravity)),
            Harness::ClaudeCode
        );
        assert_eq!(
            Harness::unnamed(Some("gemini-4-carbon"), Some(Harness::ClaudeCode)),
            Harness::Antigravity
        );
        assert_eq!(Harness::implied_by("gemini-4-carbon"), Harness::Antigravity);
        assert_eq!(Harness::implied_by(" Gemini-3.1-pro"), Harness::Antigravity);
        assert_eq!(Harness::implied_by("claude-opus-5-5"), Harness::ClaudeCode);
        assert_eq!(Harness::implied_by("opus"), Harness::ClaudeCode);
    }

    /// SPEC-493 MDL-001: a task that names no model runs Opus 5.5, the first model offered;
    /// Fable 5.2, chosen for the review, is offered too, so every Claude Code release is read
    /// for both.
    #[test]
    fn the_default_model_is_opus_5_5_and_the_review_model_fable_5_2() {
        assert_eq!(DEFAULT_CLAUDE_MODEL, "claude-opus-5-5");
        assert_eq!(REVIEW_CLAUDE_MODEL, "claude-fable-5-2");
        assert_eq!(OFFERED_CLAUDE_MODELS[0], DEFAULT_CLAUDE_MODEL);
        assert!(OFFERED_CLAUDE_MODELS.contains(&REVIEW_CLAUDE_MODEL));
        let mut unique = OFFERED_CLAUDE_MODELS.to_vec();
        unique.sort_unstable();
        unique.dedup();
        assert_eq!(unique.len(), OFFERED_CLAUDE_MODELS.len(), "an id twice");
    }

    /// SPEC-482 FBK-001: a Fable model falls back to Opus 5.5, which is offered (so every Claude
    /// Code release is read for it); no other model falls back, nor does the fallback itself,
    /// which is the default model (SPEC-493 MDL-002: a default task waits).
    #[test]
    fn a_fable_model_falls_back_to_opus_5_5() {
        assert_eq!(FALLBACK_CLAUDE_MODEL, "claude-opus-5-5");
        assert!(OFFERED_CLAUDE_MODELS.contains(&FALLBACK_CLAUDE_MODEL));
        assert_eq!(fallback_model(DEFAULT_CLAUDE_MODEL), None);
        for fable in [
            REVIEW_CLAUDE_MODEL,
            "claude-fable-5-1",
            "claude-fable-5-2[1m]",
        ] {
            assert_eq!(
                fallback_model(fable),
                Some(FALLBACK_CLAUDE_MODEL),
                "{fable}"
            );
        }
        for other in [
            FALLBACK_CLAUDE_MODEL,
            "claude-opus-4-8",
            "claude-sonnet-5-5",
            "claude-haiku-4-5",
            "",
        ] {
            assert_eq!(fallback_model(other), None, "{other}");
        }
    }

    #[test]
    fn a_model_id_as_people_say_it() {
        for (id, name) in [
            ("claude-fable-5-2", "Fable 5.2"),
            ("claude-fable-5-1", "Fable 5.1"),
            ("claude-opus-5-5", "Opus 5.5"),
            ("claude-haiku-4-5", "Haiku 4.5"),
            ("claude-opus-4", "Opus 4"),
            ("sonnet", "Sonnet"),
            ("", ""),
        ] {
            assert_eq!(claude_model_name(id), name, "{id}");
        }
    }

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

    /// SPEC-503 CDX-001: Codex and its provider are accepted again; others stay unsupported.
    #[test]
    fn codex_is_a_harness_and_other_harnesses_are_unsupported() {
        assert_eq!(Harness::parse("codex"), Ok(Harness::Codex));
        assert_eq!(Provider::parse("openai"), Ok(Provider::OpenAi));
        assert_eq!(Harness::Codex.provider(), Provider::OpenAi);
        assert_eq!(Harness::Codex.display_name(), "Codex");
        assert!(Harness::parse("opencode").is_err());
        assert!(Provider::parse("gemini").is_err());
    }

    /// SPEC-505 AGY-001: Antigravity runs on Google's accounts; other names stay unsupported.
    #[test]
    fn antigravity_is_a_harness_of_google() {
        assert_eq!(Harness::parse("antigravity"), Ok(Harness::Antigravity));
        assert_eq!(Provider::parse("google"), Ok(Provider::Google));
        assert_eq!(Harness::Antigravity.provider(), Provider::Google);
        assert_eq!(Provider::Google.harness(), Harness::Antigravity);
        assert_eq!(Harness::Antigravity.display_name(), "Antigravity");
        assert!(Harness::parse("agy").is_err());
        assert!(Harness::parse("gemini").is_err());
    }
}
