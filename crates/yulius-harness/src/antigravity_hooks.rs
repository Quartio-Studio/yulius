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
//!   refusal ends the turn showing its reason. A first call after a system message reports
//!   activity without a prompt. Later calls ask nothing.
//! - `Stop` → `Stop` (`continuationActive` as `stop_hook_active`, `fullyIdle` as `fully_idle`); a block becomes
//!   `{"decision":"continue"}`. A turn the model API ended with an error (`terminationReason`
//!   `ERROR`) is sent with its `error`, which Yulius settles without a gate, and nothing continues
//!   it; a turn a refused prompt ended (`TERMINAL_CUSTOM_HOOK`) asks nothing (Yulius ended it
//!   itself; the transcript's refusal step ends it there).
//! - `PreToolUse` of `run_command` → Claude Code's `Bash` for the guard, checked in the sandbox.
//!   The CLI denies a tool whose hook prints nothing, so every other outcome is `allow`.
//!
//! Like the rest of the hook client, a failure lets the agent go on.

use std::io::{Read, Write};

use serde_json::{Map, Value, json};
use yulius_contracts::hooks::events;

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

/// The hook for the step preceding a turn's first invocation. An explicit user input
/// submits a prompt. A system message wakes the agent. An unknown step reports neither.
fn invocation_event(payload: &Map<String, Value>) -> Option<&'static str> {
    use std::io::{BufRead as _, BufReader, Seek as _, SeekFrom};
    if payload.get("invocationNum")?.as_u64()? != 0 {
        return None;
    }
    let index = payload.get("initialNumSteps")?.as_u64()?.checked_sub(1)?;
    let path = compact_transcript(payload.get("transcriptPath")?.as_str()?);
    let mut file = crate::tail::open_regular(std::path::Path::new(&path)).ok()?;
    let length = file.metadata().ok()?.len();
    file.seek(SeekFrom::Start(
        length.saturating_sub(TRANSCRIPT_TAIL_BYTES),
    ))
    .ok()?;
    let mut found = None;
    for line in BufReader::new(file.take(TRANSCRIPT_TAIL_BYTES)).split(b'\n') {
        let line = line.ok()?;
        let Ok(Value::Object(step)) = serde_json::from_slice::<Value>(&line) else {
            continue;
        };
        if step.get("step_index").and_then(Value::as_u64) == Some(index) {
            found = match (
                step.get("source").and_then(Value::as_str),
                step.get("type").and_then(Value::as_str),
            ) {
                (Some("USER_EXPLICIT"), Some("USER_INPUT")) => Some(events::USER_PROMPT_SUBMIT),
                (Some("SYSTEM"), Some("SYSTEM_MESSAGE")) => Some(events::PRE_INVOCATION),
                _ => None,
            };
        }
    }
    found
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
    let readonly = !crate::memory_hook::guarded_paths(env).is_empty();
    let answer = with_read_only_notice(event, answer, readonly);
    let _ignored = output
        .write_all(answer.to_string().as_bytes())
        .and_then(|()| output.flush());
    0
}

/// `answer` to `event`, with the sentence that the session's rules are read-only and where
/// lasting knowledge goes injected at its start when it has `readonly` paths (SPEC-652 MEM-006):
/// the CLI has no hook after a tool, so a refused write is explained up front.
fn with_read_only_notice(event: &str, answer: Value, readonly: bool) -> Value {
    if event != "SessionStart" || !readonly {
        return answer;
    }
    let notice = json!({ "systemMessage": {
        "systemMessage": yulius_contracts::hooks::MEMORY_READ_ONLY_SENTENCE
    } });
    let mut answer = match answer {
        Value::Object(map) => map,
        _ => Map::new(),
    };
    match answer.entry("injectSteps").or_insert_with(|| json!([])) {
        Value::Array(steps) => steps.push(notice),
        other => *other = json!([notice]),
    }
    Value::Object(answer)
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
    // A turn its model API failed is never continued: agy would only try the model again.
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
        "PreInvocation" => invocation_event(payload)?,
        // A turn a refused prompt ended asks nothing: Yulius ended it itself.
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
        events::PRE_INVOCATION => {}
        "Stop" => {
            if let Some(idle) = payload.get("fullyIdle").and_then(Value::as_bool) {
                request.insert("fully_idle".into(), json!(idle));
            }
            let active = payload
                .get("continuationActive")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            request.insert("stop_hook_active".into(), json!(active));
            // A turn its model API failed: Yulius settles it without a gate (AGY-006).
            if payload.get("terminationReason").and_then(Value::as_str) == Some("ERROR") {
                let error = payload
                    .get("error")
                    .and_then(Value::as_str)
                    .filter(|e| !e.is_empty())
                    .unwrap_or("the model API failed");
                request.insert(
                    "error".into(),
                    json!(crate::json::truncate_utf8(error, 1024)),
                );
            }
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

    /// SPEC-652 MEM-006: an Antigravity session with read-only rules is told the sentence at its
    /// start, after what Yulius says or alone; one without, and other events, are not.
    #[test]
    fn a_session_with_read_only_rules_is_told_why_at_its_start() {
        let sentence = yulius_contracts::hooks::MEMORY_READ_ONLY_SENTENCE;
        let said = json!({ "injectSteps": [{ "systemMessage": { "systemMessage": "briefing" } }] });
        assert_eq!(
            with_read_only_notice("SessionStart", said.clone(), true),
            json!({ "injectSteps": [
                { "systemMessage": { "systemMessage": "briefing" } },
                { "systemMessage": { "systemMessage": sentence } },
            ] })
        );
        assert_eq!(
            with_read_only_notice("SessionStart", json!({}), true),
            json!({ "injectSteps": [{ "systemMessage": { "systemMessage": sentence } }] })
        );
        assert_eq!(
            with_read_only_notice("SessionStart", said.clone(), false),
            said
        );
        assert_eq!(with_read_only_notice("Stop", json!({}), true), json!({}));
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
