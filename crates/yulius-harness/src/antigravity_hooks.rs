//! `yulius hook antigravity <event>`: the Antigravity CLI's hooks, translated to and from the
//! Claude-shaped contract Yulius serves (SPEC-505 AGY-006).
//!
//! The CLI (1.5.10) sends a camelCase payload (`conversationId`, `transcriptPath`, `cwd`,
//! `modelName`, `lastUserInput`, …) and reads an answer of its own shape. Translated here:
//!
//! - `SessionStart` → `SessionStart` (`source` `startup`); added context comes back as an
//!   injected system message.
//! - `PreInvocation` with `invocationNum` 0, the model call that opens a turn, whose step before
//!   it is a person's `USER_INPUT` → `UserPromptSubmit` with `lastUserInput` as the prompt; a
//!   refusal ends the turn showing its reason. Later model calls of the turn, and the turn the CLI
//!   opens by itself when a background command ends (a system message before it, the earlier
//!   prompt still in `lastUserInput`), ask nothing.
//! - `Stop` → `Stop` (`continuationActive` as `stop_hook_active`); a block becomes
//!   `{"decision":"continue"}`. A turn the model API ended with an error (`terminationReason`
//!   `ERROR`) is sent as its end, but never continued: agy would only try the failing model
//!   again. A turn a refused prompt ended (`TERMINAL_CUSTOM_HOOK`) asks nothing, so the refused
//!   prompt is never continued (Claude Code runs no turn for a refused prompt).
//! - `PreToolUse` of `run_command` → Claude Code's `Bash` for the guard, checked in the sandbox.
//!   The CLI denies a tool whose hook prints nothing, so every other outcome is `allow`.
//!
//! Like the rest of the hook client, a failure lets the agent go on.

use std::io::{Read, Write};

use serde_json::{Map, Value, json};

use crate::hook_client::{HOOK_TIMEOUT, HoldPolicy, MAX_INPUT_BYTES, MAX_TOOL_INPUT_BYTES};

/// The CLI's full transcript, which the hook names, and the one Yulius reads beside it.
const FULL_TRANSCRIPT: &str = "transcript_full.jsonl";
const TRANSCRIPT: &str = "transcript.jsonl";
/// How much of a transcript's end is read to find the step that opened a turn.
const TRANSCRIPT_TAIL_BYTES: u64 = 4 * 1024 * 1024;

/// The compact transcript beside the full one a hook names (both hold every step).
fn compact_transcript(full: &str) -> String {
    full.strip_suffix(FULL_TRANSCRIPT)
        .map_or_else(|| full.to_owned(), |dir| format!("{dir}{TRANSCRIPT}"))
}

/// Whether step `index` of the transcript at `path` is a prompt a person typed, read from the
/// file's last [`TRANSCRIPT_TAIL_BYTES`] without following a link or waiting on a FIFO. `false`
/// when it cannot be read or found.
fn typed_step(path: &str, index: u64) -> bool {
    use std::io::{BufRead as _, BufReader, Seek as _, SeekFrom};
    let Ok(mut file) = crate::tail::open_regular(std::path::Path::new(path)) else {
        return false;
    };
    let length = file.metadata().map(|m| m.len()).unwrap_or(0);
    if file
        .seek(SeekFrom::Start(
            length.saturating_sub(TRANSCRIPT_TAIL_BYTES),
        ))
        .is_err()
    {
        return false;
    }
    let mut found = None;
    for line in BufReader::new(file.take(TRANSCRIPT_TAIL_BYTES)).split(b'\n') {
        let Ok(line) = line else { return false };
        let Ok(Value::Object(step)) = serde_json::from_slice::<Value>(&line) else {
            continue;
        };
        if step.get("step_index").and_then(Value::as_u64) == Some(index) {
            found = Some(
                step.get("type").and_then(Value::as_str) == Some("USER_INPUT")
                    && step.get("source").and_then(Value::as_str) == Some("USER_EXPLICIT"),
            );
        }
    }
    found.unwrap_or(false)
}

/// Whether a `PreInvocation` opens a turn a person's prompt started: its first model call, the
/// step before it the prompt (SPEC-505 AGY-006).
fn opens_a_typed_turn(payload: &Map<String, Value>) -> bool {
    let first = payload.get("invocationNum").and_then(Value::as_u64) == Some(0);
    let Some(prompt_step) = payload
        .get("initialNumSteps")
        .and_then(Value::as_u64)
        .and_then(|n| n.checked_sub(1))
    else {
        return false;
    };
    let Some(full) = payload.get("transcriptPath").and_then(Value::as_str) else {
        return false;
    };
    first && typed_step(&compact_transcript(full), prompt_step)
}

/// Runs one Antigravity hook. Always returns exit code 0.
pub(crate) fn run(
    event: &str,
    input: &mut dyn Read,
    output: &mut dyn Write,
    env: &dyn Fn(&str) -> Option<String>,
    hold: HoldPolicy,
) -> i32 {
    let answer = answer(event, input, env, hold);
    let _ignored = output
        .write_all(answer.to_string().as_bytes())
        .and_then(|()| output.flush());
    0
}

/// What the CLI is told when Yulius has nothing to say: go on.
fn go_on(event: &str) -> Value {
    if event == "PreToolUse" {
        json!({ "decision": "allow" })
    } else {
        json!({})
    }
}

fn answer(
    event: &str,
    input: &mut dyn Read,
    env: &dyn Fn(&str) -> Option<String>,
    hold: HoldPolicy,
) -> Value {
    let bound = if event == "PreToolUse" {
        MAX_TOOL_INPUT_BYTES
    } else {
        MAX_INPUT_BYTES
    };
    let mut body = Vec::new();
    if input.take(bound as u64 + 1).read_to_end(&mut body).is_err() || body.len() > bound {
        return go_on(event);
    }
    let Ok(Value::Object(payload)) = serde_json::from_slice::<Value>(&body) else {
        return go_on(event);
    };
    let Some((claude_event, request)) = to_claude(event, &payload) else {
        return go_on(event);
    };
    let Ok(request) = serde_json::to_vec(&Value::Object(request)) else {
        return go_on(event);
    };
    let reply = if claude_event == "PreToolUse" {
        // The guard of Claude Code's `Bash`, in the sandbox, without a request to Yulius.
        let mut out = Vec::new();
        crate::guard_client::pre_tool_use(&mut request.as_slice(), &mut out);
        out
    } else {
        match crate::hook_client::forward(
            "antigravity",
            claude_event,
            &mut request.as_slice(),
            env,
            HOOK_TIMEOUT,
            hold,
        ) {
            Ok(reply) => reply,
            Err(_) => return go_on(event),
        }
    };
    // A turn the model API failed ends: Yulius hears of it, but no answer makes agy try the
    // failing model again.
    if payload.get("terminationReason").and_then(Value::as_str) == Some("ERROR") {
        return go_on(event);
    }
    from_claude(event, claude_event, &reply)
}

fn text(payload: &Map<String, Value>, key: &str) -> Option<Value> {
    payload
        .get(key)
        .and_then(Value::as_str)
        .map(|s| Value::String(s.to_owned()))
}

/// The Claude-shaped event and request of an Antigravity hook payload, `None` when Yulius has
/// nothing to be asked (a later model call of a turn, a tool the guard does not read).
fn to_claude(
    event: &str,
    payload: &Map<String, Value>,
) -> Option<(&'static str, Map<String, Value>)> {
    let claude_event = match event {
        "SessionStart" => "SessionStart",
        "PreInvocation" if opens_a_typed_turn(payload) => "UserPromptSubmit",
        // A turn a refused prompt ended: Yulius ended it itself.
        "Stop"
            if payload.get("terminationReason").and_then(Value::as_str)
                != Some("TERMINAL_CUSTOM_HOOK") =>
        {
            "Stop"
        }
        "PreToolUse" => "PreToolUse",
        _ => return None,
    };
    let mut request = Map::new();
    request.insert("hook_event_name".into(), json!(claude_event));
    if let Some(id) = text(payload, "conversationId") {
        request.insert("session_id".into(), id);
    }
    if let Some(cwd) = text(payload, "cwd") {
        request.insert("cwd".into(), cwd);
    }
    if let Some(model) = text(payload, "modelName") {
        request.insert("model".into(), model);
    }
    if let Some(path) = payload.get("transcriptPath").and_then(Value::as_str) {
        request.insert("transcript_path".into(), json!(compact_transcript(path)));
    }
    match claude_event {
        "SessionStart" => {
            request.insert("source".into(), json!("startup"));
        }
        "UserPromptSubmit" => {
            request.insert("prompt".into(), text(payload, "lastUserInput")?);
        }
        "Stop" => {
            let active = payload
                .get("continuationActive")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            request.insert("stop_hook_active".into(), json!(active));
        }
        _ => {
            let call = payload.get("toolCall")?;
            if call.get("name").and_then(Value::as_str) != Some("run_command") {
                return None;
            }
            let command = call.get("args")?.get("CommandLine")?.as_str()?;
            request.insert("tool_name".into(), json!("Bash"));
            request.insert("tool_input".into(), json!({ "command": command }));
        }
    }
    Some((claude_event, request))
}

/// The Antigravity answer of a Claude-shaped `reply` to `claude_event`.
fn from_claude(event: &str, claude_event: &str, reply: &[u8]) -> Value {
    let reply: Map<String, Value> = match serde_json::from_slice(reply) {
        Ok(Value::Object(map)) => map,
        _ => return go_on(event),
    };
    let blocked = reply.get("decision").and_then(Value::as_str) == Some("block");
    let reason = reply
        .get("reason")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    let specific = reply.get("hookSpecificOutput");
    let context = specific
        .and_then(|s| s.get("additionalContext"))
        .and_then(Value::as_str)
        .filter(|c| !c.is_empty());
    let inject = |text: &str| json!({ "systemMessage": { "systemMessage": text } });
    match claude_event {
        "Stop" if blocked => json!({ "decision": "continue", "reason": reason }),
        "UserPromptSubmit" if blocked => json!({
            "injectSteps": [{ "errorMessage": { "userMessage": reason } }],
            "terminationBehavior": "terminate"
        }),
        "SessionStart" | "UserPromptSubmit" => match context {
            Some(context) => json!({ "injectSteps": [inject(context)] }),
            None => json!({}),
        },
        "PreToolUse" => {
            let denied = specific
                .and_then(|s| s.get("permissionDecision"))
                .and_then(Value::as_str)
                == Some("deny");
            if denied {
                let reason = specific
                    .and_then(|s| s.get("permissionDecisionReason"))
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                json!({ "decision": "deny", "reason": reason })
            } else {
                go_on(event)
            }
        }
        _ => go_on(event),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A reply to a prompt that adds context injects it; an empty or broken reply says go on.
    #[test]
    fn replies_become_the_clis_answers() {
        assert_eq!(
            from_claude(
                "PreInvocation",
                "UserPromptSubmit",
                br#"{"hookSpecificOutput":{"hookEventName":"UserPromptSubmit","additionalContext":"note"}}"#
            ),
            json!({ "injectSteps": [{ "systemMessage": { "systemMessage": "note" } }] })
        );
        assert_eq!(from_claude("Stop", "Stop", b""), json!({}));
        assert_eq!(
            from_claude("PreToolUse", "PreToolUse", b"not json"),
            json!({ "decision": "allow" })
        );
        assert_eq!(
            from_claude("SessionStart", "SessionStart", b"{}"),
            json!({})
        );
    }

    /// A payload without a prompt is no turn's start; an unknown event asks nothing.
    #[test]
    fn what_is_not_translated() {
        let mut payload = Map::new();
        payload.insert("invocationNum".into(), json!(0));
        assert!(to_claude("PreInvocation", &payload).is_none());
        assert!(to_claude("PostInvocation", &payload).is_none());
        assert!(to_claude("PreToolUse", &payload).is_none());
    }
}
