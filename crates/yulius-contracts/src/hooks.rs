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
    /// Antigravity started a turn after a system message (no typed prompt).
    pub const PRE_INVOCATION: &str = "PreInvocation";
    /// The agent finished its turn; output may block with a reason (discipline gate).
    pub const STOP: &str = "Stop";
    /// Claude Code notification (for example a permission prompt).
    pub const NOTIFICATION: &str = "Notification";
    /// The session ends (`/exit`, Ctrl+C twice, `/clear`, a signal); `reason` says why.
    pub const SESSION_END: &str = "SessionEnd";
    /// A tool is about to run; output may deny it with a reason (the edit gate, SPEC-426), or
    /// change its input (the reviewer of a review round, SPEC-493 REV-002).
    pub const PRE_TOOL_USE: &str = "PreToolUse";
    /// A subagent started, or a cut-off one was resumed (`agent_id`, `agent_type`).
    pub const SUBAGENT_START: &str = "SubagentStart";
    /// A subagent ended with its report (`last_assistant_message`). Claude Code 2.1.325 sends
    /// none for a subagent a usage limit cut off.
    pub const SUBAGENT_STOP: &str = "SubagentStop";
    /// Not a hook: Claude Code's `statusLine` command, run by the same client. Its input is a
    /// [`super::StatusLineInput`]; the answer is the status line's text.
    pub const STATUS_LINE: &str = "StatusLine";
}

/// Claude Code's tools that write a file, with the `tool_input` field naming it: their
/// `PreToolUse` goes to Yulius's edit gate (SPEC-426 EDG-001).
pub const EDIT_TOOLS: [(&str, &str); 4] = [
    ("Edit", "file_path"),
    ("MultiEdit", "file_path"),
    ("Write", "file_path"),
    ("NotebookEdit", "notebook_path"),
];

/// The event of `yulius hook <harness> MemoryGuard`, answered in the sandbox without asking
/// Yulius (SPEC-652 MEM-006): an edit of a read-only memory or instruction path is denied with
/// [`MEMORY_READ_ONLY_SENTENCE`], and a tool result that a read-only path refused gets it as
/// added context.
pub const MEMORY_GUARD_EVENT: &str = "MemoryGuard";

/// The run variable naming the session's read-only memory and instruction paths, one per line.
pub const READONLY_PATHS_ENV: &str = "YULIUS_READONLY_PATHS";

/// The one sentence a refused write of harness memory is explained with (SPEC-652 MEM-006).
pub const MEMORY_READ_ONLY_SENTENCE: &str = "Yulius keeps this harness's memory and user-level \
     instructions read-only (the owner's decision of 2026-11-25): put lasting knowledge in the \
     repository through a pull request.";

/// The `tool_input` field naming the file an edit tool writes (see [`EDIT_TOOLS`]).
#[must_use]
pub fn edited_file_field(tool: &str) -> Option<&'static str> {
    EDIT_TOOLS
        .iter()
        .find(|(name, _)| *name == tool)
        .map(|(_, field)| *field)
}

/// The request header in which `yulius hook` says when it sent the hook (Unix ms): a hook Yulius
/// handles late, after its client gave up waiting, is ordered with the agent's later hooks by it
/// (SPEC-346 LPH-001).
pub const SENT_AT_HEADER: &str = "x-yulius-hook-sent-ms";

/// Client-generated identity shared by every HTTP exchange of one submit invocation.
pub const INVOCATION_HEADER: &str = "x-yulius-hook-invocation";

/// `SessionEnd` reasons (Claude Code 2.1.292): the person left the session (`/exit`, Ctrl+C
/// twice, Ctrl+D) — `prompt_input_exit`; `/clear` started a new conversation in the same
/// process; `logout`; `resume` (another session was resumed in the same process); anything else
/// (a signal, an error) is `other`.
pub mod session_end {
    /// `/exit`, Ctrl+C twice or Ctrl+D at the prompt.
    pub const PROMPT_INPUT_EXIT: &str = "prompt_input_exit";
    /// `/clear`: the process goes on with a new session.
    pub const CLEAR: &str = "clear";
    /// `/logout`.
    pub const LOGOUT: &str = "logout";
}

/// The JSON a CLI sends to a hook command. Every field is optional and unknown fields are
/// ignored.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct HookInput {
    /// Submit invocation from the transport header. The provider JSON cannot set it.
    #[serde(skip)]
    pub invocation: Option<u128>,
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
    /// `SessionStart` source such as `startup`, `resume`, `clear` or `compact`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    /// Claude prompt identifier shared with its transcript record.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prompt_id: Option<String>,
    /// Submitted prompt (`UserPromptSubmit`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prompt: Option<String>,
    /// `true` when the agent is already continuing because of a Stop hook block.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stop_hook_active: Option<bool>,
    /// Whether Antigravity had also finished its background work at Stop.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fully_idle: Option<bool>,
    /// Notification text (`Notification`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    /// Notification title (`Notification`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Kind of notification (Claude Code `Notification`): `permission_prompt`, `idle_prompt`,
    /// `elicitation_dialog`, `auth_success`, …
    #[serde(skip_serializing_if = "Option::is_none")]
    pub notification_type: Option<String>,
    /// Tool name for tool and permission events.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_name: Option<String>,
    /// Tool input for tool and permission events, as sent.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_input: Option<serde_json::Value>,
    /// Model of the session, when the CLI reports it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// The error that ended the turn (`Stop` of an Antigravity turn its model API failed,
    /// SPEC-505): such a turn ends and is never continued.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Why the session ends (`SessionEnd`, see [`session_end`]).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// The subagent of `SubagentStart` and `SubagentStop`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<String>,
    /// The subagent's type (its agent definition's name).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent_type: Option<String>,
    /// The subagent's last message (`SubagentStop`): its report.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_assistant_message: Option<String>,
}

/// One plan rate-limit window as Claude Code passes it to its status line command:
/// `used_percentage` (0–100) and `resets_at` in Unix seconds. Claude Code reads both from the
/// `anthropic-ratelimit-unified-*` headers of its own API responses, so they are known for every
/// Claude subscription sign-in, including `claude setup-token` ones that cannot read the usage
/// endpoint.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct RateLimitWindow {
    /// Percent of the window used.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub used_percentage: Option<f64>,
    /// When the window resets (Unix seconds).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resets_at: Option<i64>,
}

/// The plan rate limits in Claude Code's status line input (present once a response of the
/// session reported them).
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct RateLimits {
    /// The five-hour session window.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub five_hour: Option<RateLimitWindow>,
    /// The weekly window.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub seven_day: Option<RateLimitWindow>,
}

/// The JSON Claude Code sends to its `statusLine` command (Claude Code 2.1.292; only the fields
/// Yulius reads — the rest, such as `model` as an object, `workspace` and `cost`, is ignored).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct StatusLineInput {
    /// Provider session id.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    /// Plan rate limits; absent before the session's first response or without a plan.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rate_limits: Option<RateLimits>,
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
    /// `PreToolUse`: `deny` keeps the tool from running (`permissionDecision`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub permission_decision: Option<String>,
    /// `PreToolUse`: why, shown to the agent (`permissionDecisionReason`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub permission_decision_reason: Option<String>,
    /// `PreToolUse`: the tool's input to run instead of the one asked for (`updatedInput`,
    /// with `permissionDecision: allow`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub updated_input: Option<serde_json::Value>,
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
}

/// Decision value that blocks the action.
pub const DECISION_BLOCK: &str = "block";
/// `PreToolUse` permission decision that keeps the tool from running.
pub const PERMISSION_DENY: &str = "deny";
/// `PreToolUse` permission decision that lets the tool run (with an `updatedInput`).
pub const PERMISSION_ALLOW: &str = "allow";

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
                ..HookSpecificOutput::default()
            }),
            ..Self::default()
        }
    }

    /// `PreToolUse` output that keeps the tool from running, telling the agent `reason`.
    #[must_use]
    pub fn deny_tool(reason: impl Into<String>) -> Self {
        Self {
            hook_specific_output: Some(HookSpecificOutput {
                hook_event_name: events::PRE_TOOL_USE.into(),
                permission_decision: Some(PERMISSION_DENY.into()),
                permission_decision_reason: Some(reason.into()),
                ..HookSpecificOutput::default()
            }),
            ..Self::default()
        }
    }

    /// `PreToolUse` output that runs the tool with `input` instead, telling the agent `context`.
    #[must_use]
    pub fn run_tool_with(input: serde_json::Value, context: impl Into<String>) -> Self {
        Self {
            hook_specific_output: Some(HookSpecificOutput {
                hook_event_name: events::PRE_TOOL_USE.into(),
                permission_decision: Some(PERMISSION_ALLOW.into()),
                updated_input: Some(input),
                additional_context: Some(context.into()),
                ..HookSpecificOutput::default()
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

    /// SPEC-493 REV-002: the subagent hooks' fields as Claude Code 2.1.325 sends them
    /// (synthetic values), and the answer that runs the Agent tool with another input, as the
    /// real CLI takes it.
    #[test]
    fn subagent_hooks_and_an_updated_tool_input() -> TestResult {
        let stop: HookInput = serde_json::from_value(json!({
            "session_id": "s", "hook_event_name": "SubagentStop", "agent_id": "a73a9170080d69b92",
            "agent_type": "yulius-reviewer", "stop_hook_active": false,
            "agent_transcript_path": "/h/.claude/projects/-w/s/subagents/agent-a73a.jsonl",
            "last_assistant_message": "Findings…\nVerdict: approve",
            "background_tasks": [], "effort": {"level": "high"}
        }))?;
        assert_eq!(stop.agent_id.as_deref(), Some("a73a9170080d69b92"));
        assert_eq!(stop.agent_type.as_deref(), Some("yulius-reviewer"));
        assert_eq!(
            stop.last_assistant_message.as_deref(),
            Some("Findings…\nVerdict: approve")
        );
        let out = HookOutput::run_tool_with(
            json!({"subagent_type": "yulius-reviewer-opus", "prompt": "p"}),
            "round 3",
        );
        assert_eq!(
            serde_json::to_value(&out)?,
            json!({"hookSpecificOutput": {"hookEventName": "PreToolUse",
                "additionalContext": "round 3", "permissionDecision": "allow",
                "updatedInput": {"subagent_type": "yulius-reviewer-opus", "prompt": "p"}}})
        );
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

    /// The status line input of Claude Code 2.1.292 (synthetic values): `model` is an object
    /// there, so it has its own type and does not break [`HookInput`].
    #[test]
    fn status_line_input_reads_the_rate_limits() -> TestResult {
        let raw = json!({
            "session_id": "3f2b1c9e-aaaa-4bbb-8ccc-0123456789ab",
            "model": {"id": "claude-opus-4-1", "display_name": "Opus"},
            "workspace": {"current_dir": "/w", "project_dir": "/w"},
            "cost": {"total_cost_usd": 0.5},
            "rate_limits": {
                "five_hour": {"used_percentage": 42.5, "resets_at": 1_791_700_000},
                "seven_day": {"used_percentage": 18.0, "resets_at": 1_792_000_000}
            }
        });
        let input: StatusLineInput = serde_json::from_value(raw)?;
        let limits = input
            .rate_limits
            .ok_or_else(|| serde::de::Error::custom("limits"))?;
        assert_eq!(
            limits.five_hour,
            Some(RateLimitWindow {
                used_percentage: Some(42.5),
                resets_at: Some(1_791_700_000)
            })
        );
        assert_eq!(limits.seven_day.and_then(|w| w.used_percentage), Some(18.0));
        let before_first_response: StatusLineInput =
            serde_json::from_value(json!({"session_id": "s", "model": {"id": "m"}}))?;
        assert_eq!(before_first_response.rate_limits, None);
        let ended: HookInput = serde_json::from_value(
            json!({"hook_event_name": "SessionEnd", "reason": "prompt_input_exit"}),
        )?;
        assert_eq!(
            ended.reason.as_deref(),
            Some(session_end::PROMPT_INPUT_EXIT)
        );
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
                ..HookSpecificOutput::default()
            }),
        };
        let value = serde_json::to_value(&output)?;
        assert_eq!(
            value,
            json!({
                "decision": "block",
                "reason": "spec missing",
                "hookSpecificOutput": {"hookEventName": "SessionStart", "additionalContext": "briefing"}
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
        assert_eq!(
            serde_json::to_value(HookOutput::deny_tool("write the Plan"))?,
            json!({"hookSpecificOutput": {
                "hookEventName": "PreToolUse",
                "permissionDecision": "deny",
                "permissionDecisionReason": "write the Plan"
            }})
        );
        assert_eq!(serde_json::to_string(&HookOutput::default())?, "{}");
        assert!(HookOutput::default().is_empty());
        assert!(!HookOutput::block("x").is_empty());
        Ok(())
    }
}
