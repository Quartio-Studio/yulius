//! Hook payloads exchanged with Claude Code hooks (SPEC-002 §7.5).
//!
//! The CLI sends a JSON object on the hook command's stdin and read a JSON object from its
//! stdout. The field names below are the CLIs' own (snake_case input, camelCase output).
//! Inputs tolerate missing and unknown fields because each CLI version adds fields; outputs
//! omit unset fields so a CLI never sees a value Yulius did not decide.

use serde::{Deserialize, Serialize};

/// Hook event names Yulius registers (SPEC-002 §7.2, §7.5).
pub mod events {
    /// Session started or resumed; output may carry the briefing as additional context.
    pub const SESSION_START: &str = "SessionStart";
    /// The user submitted a prompt; output may carry message digests.
    pub const USER_PROMPT_SUBMIT: &str = "UserPromptSubmit";
    /// The agent finished its turn; output may block with a reason (discipline gate).
    pub const STOP: &str = "Stop";
    /// Claude Code notification (for example a permission prompt).
    pub const NOTIFICATION: &str = "Notification";
}

/// The JSON a CLI sends to a hook command. Every field is optional and unknown fields are
/// ignored.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct HookInput {
    /// Provider session id.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    /// Path of the session transcript file.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub transcript_path: Option<String>,
    /// Working directory of the session.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    /// Event name, for example `Stop`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hook_event_name: Option<String>,
    /// Permission mode of the session (Claude Code).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub permission_mode: Option<String>,
    /// `SessionStart` source such as `startup`, `resume`, `clear` or `compact`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    /// Submitted prompt (`UserPromptSubmit`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prompt: Option<String>,
    /// `true` when the agent is already continuing because of a Stop hook block.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stop_hook_active: Option<bool>,
    /// Notification text (`Notification`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    /// Notification title (`Notification`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Tool name for tool and permission events.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_name: Option<String>,
    /// Tool input for tool and permission events, as sent.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_input: Option<serde_json::Value>,
    /// Model of the session, when the CLI reports it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
}

/// Event-specific output (`hookSpecificOutput`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct HookSpecificOutput {
    /// Event this output answers (`hookEventName`); must equal the input event.
    pub hook_event_name: String,
    /// Text added to the agent's context (`additionalContext`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub additional_context: Option<String>,
}

/// The JSON a hook command prints back to the CLI. Unset fields are omitted.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct HookOutput {
    /// `block` to stop the action (for `Stop`: keep the agent working).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub decision: Option<String>,
    /// Reason shown to the agent when blocking.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// Event-specific output (`hookSpecificOutput`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hook_specific_output: Option<HookSpecificOutput>,
    /// Message shown to the user (`systemMessage`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system_message: Option<String>,
    /// `false` stops the agent entirely (`continue`).
    #[serde(rename = "continue", skip_serializing_if = "Option::is_none")]
    pub continue_: Option<bool>,
    /// Reason shown when `continue` is `false` (`stopReason`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stop_reason: Option<String>,
    /// Hide the hook's stdout from the transcript view (`suppressOutput`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub suppress_output: Option<bool>,
}

/// Decision value that blocks the action.
pub const DECISION_BLOCK: &str = "block";

impl HookOutput {
    /// Output that blocks with `reason` (for `Stop`, the agent continues with the reason).
    #[must_use]
    pub fn block(reason: impl Into<String>) -> Self {
        Self {
            decision: Some(DECISION_BLOCK.to_owned()),
            reason: Some(reason.into()),
            ..Self::default()
        }
    }

    /// Output that adds `context` to the agent's context for `event`.
    #[must_use]
    pub fn additional_context(event: impl Into<String>, context: impl Into<String>) -> Self {
        Self {
            hook_specific_output: Some(HookSpecificOutput {
                hook_event_name: event.into(),
                additional_context: Some(context.into()),
            }),
            ..Self::default()
        }
    }

    /// `true` when nothing is set; such an output is equivalent to printing nothing.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};

    type TestResult = Result<(), serde_json::Error>;

    #[test]
    fn input_parses_claude_stop_payload_and_ignores_unknown_fields() -> TestResult {
        let raw = r#"{"session_id":"3f2b1c9e-aaaa-4bbb-8ccc-0123456789ab",
            "transcript_path":"/h/.claude/projects/-w/3f2b.jsonl","cwd":"/w",
            "permission_mode":"default","hook_event_name":"Stop","stop_hook_active":false,
            "future_field":{"nested":[1,2]}}"#;
        let input: HookInput = serde_json::from_str(raw)?;
        assert_eq!(input.hook_event_name.as_deref(), Some(events::STOP));
        assert_eq!(input.stop_hook_active, Some(false));
        assert_eq!(input.cwd.as_deref(), Some("/w"));
        assert_eq!(input.prompt, None);
        Ok(())
    }

    #[test]
    fn input_accepts_empty_object_and_tool_input() -> TestResult {
        assert_eq!(
            serde_json::from_str::<HookInput>("{}")?,
            HookInput::default()
        );
        let input: HookInput = serde_json::from_value(json!({
            "hook_event_name": "Notification",
            "tool_name": "Bash",
            "tool_input": {"command": "ls"},
            "unknown_field": 1
        }))?;
        assert_eq!(input.tool_input, Some(json!({"command": "ls"})));
        Ok(())
    }

    #[test]
    fn input_round_trips() -> TestResult {
        let input = HookInput {
            session_id: Some("s".into()),
            hook_event_name: Some(events::USER_PROMPT_SUBMIT.into()),
            prompt: Some("fix the bug".into()),
            ..HookInput::default()
        };
        let text = serde_json::to_string(&input)?;
        assert_eq!(
            serde_json::from_str::<Value>(&text)?,
            json!({"session_id":"s","hook_event_name":"UserPromptSubmit","prompt":"fix the bug"})
        );
        assert_eq!(serde_json::from_str::<HookInput>(&text)?, input);
        Ok(())
    }

    #[test]
    fn output_uses_cli_field_names() -> TestResult {
        let output = HookOutput {
            decision: Some(DECISION_BLOCK.into()),
            reason: Some("spec missing".into()),
            hook_specific_output: Some(HookSpecificOutput {
                hook_event_name: events::SESSION_START.into(),
                additional_context: Some("briefing".into()),
            }),
            system_message: Some("note".into()),
            continue_: Some(true),
            stop_reason: Some("why".into()),
            suppress_output: Some(false),
        };
        let value = serde_json::to_value(&output)?;
        assert_eq!(
            value,
            json!({
                "decision": "block",
                "reason": "spec missing",
                "hookSpecificOutput": {"hookEventName": "SessionStart", "additionalContext": "briefing"},
                "systemMessage": "note",
                "continue": true,
                "stopReason": "why",
                "suppressOutput": false
            })
        );
        assert_eq!(serde_json::from_value::<HookOutput>(value)?, output);
        Ok(())
    }

    #[test]
    fn output_helpers_omit_unset_fields() -> TestResult {
        assert_eq!(
            serde_json::to_value(HookOutput::block("write the spec"))?,
            json!({"decision": "block", "reason": "write the spec"})
        );
        assert_eq!(
            serde_json::to_value(HookOutput::additional_context(
                events::USER_PROMPT_SUBMIT,
                "2 messages"
            ))?,
            json!({"hookSpecificOutput": {"hookEventName": "UserPromptSubmit", "additionalContext": "2 messages"}})
        );
        assert_eq!(serde_json::to_string(&HookOutput::default())?, "{}");
        assert!(HookOutput::default().is_empty());
        assert!(!HookOutput::block("x").is_empty());
        Ok(())
    }
}
