//! Claude Code transcripts as read-only documents (SPEC-001 SES-002..005): one JSONL line →
//! one [`TranscriptEntry`] of messages, tool calls, results and diffs, redacted and bounded;
//! paged reading, index passages and the plain-text rendering agents read. Codex rollouts and
//! Antigravity transcripts read into the same entries (SPEC-525): each line's shape tells its
//! format.
//!
//! Every text that leaves this module went through [`yulius_domain::redact`]: provider tokens,
//! keys and secret assignments everywhere, and every value of a dotenv file that a tool read or
//! wrote (the tool call's path decides) — so task transcripts, which are not redacted at the
//! source, are safe to show to people and to other tasks.

use std::collections::{HashSet, VecDeque};
use std::fs::File;
use std::io::{self, BufRead, BufReader, Read as _, Seek, SeekFrom};
use std::path::Path;

use serde_json::Value;
use yulius_contracts::api::sessions::{DiffHunk, TranscriptBlock, TranscriptDiff, TranscriptEntry};
use yulius_domain::harness::is_codex_injected_context;
use yulius_domain::redact::{
    is_env_file, is_secret_name, mentions_env_file, redact, redact_env, redact_named,
};

use crate::ports::session_docs::SessionChunk;

/// Longest transcript line read; longer lines are skipped and counted.
pub const MAX_LINE_BYTES: usize = 8 * 1024 * 1024;
/// Bound of one message text in a page.
pub const MAX_TEXT_CHARS: usize = 32 * 1024;
/// Bound of one thinking block.
pub const MAX_THINKING_CHARS: usize = 8 * 1024;
/// Bound of a tool call's pretty-printed input.
pub const MAX_INPUT_CHARS: usize = 8 * 1024;
/// Bound of a tool result.
pub const MAX_RESULT_CHARS: usize = 16 * 1024;
/// Bound of one side of a diff hunk.
pub const MAX_HUNK_CHARS: usize = 16 * 1024;
/// Most hunks shown for one edit.
pub const MAX_HUNKS: usize = 20;
/// Most entries in one page.
pub const MAX_PAGE_ENTRIES: usize = 500;
/// Approximate bound of one page's text.
pub const MAX_PAGE_CHARS: usize = 2 * 1024 * 1024;
/// Bound of one index passage.
pub const MAX_CHUNK_CHARS: usize = 8 * 1024;
/// Bound of a tool result's share of an index passage.
const MAX_CHUNK_RESULT_CHARS: usize = 1024;
/// Bound of a summary line (tool call summaries, previews).
const MAX_SUMMARY_CHARS: usize = 200;

/// Wrappers Claude Code puts around commands, reminders and interruptions.
const META_PREFIXES: &[&str] = &[
    "<command-",
    "<local-command-",
    "<system-reminder>",
    "<user-prompt-submit-hook>",
    "<bash-input>",
    "<bash-stdout>",
    "<bash-stderr>",
    "Caveat: The messages below were generated",
    "[Request interrupted",
];

/// Cuts `text` to at most `max` characters; returns whether it was cut.
#[must_use]
pub fn cut(text: &str, max: usize) -> (String, bool) {
    match text.char_indices().nth(max) {
        Some((index, _)) => (text[..index].to_owned(), true),
        None => (text.to_owned(), false),
    }
}

fn clean(text: &str, env: bool) -> String {
    let redacted = if env { redact_env(text) } else { redact(text) };
    redacted.map_or_else(|| text.to_owned(), |r| r.text)
}

fn one_line(text: &str) -> String {
    let collapsed = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let (short, cut_off) = cut(&collapsed, MAX_SUMMARY_CHARS);
    if cut_off {
        format!("{short}…")
    } else {
        short
    }
}

/// Redacts every string of a JSON value; secret-named keys lose their literal values.
fn redact_value(value: &mut Value, env: bool) {
    match value {
        Value::String(text) => *text = clean(text, env),
        Value::Array(items) => items.iter_mut().for_each(|item| redact_value(item, env)),
        Value::Object(map) => {
            for (key, item) in map.iter_mut() {
                if let Value::String(text) = item
                    && is_secret_name(key)
                    && let Some(redacted) = redact_named(key, text)
                {
                    *text = redacted.text;
                    continue;
                }
                redact_value(item, env);
            }
        }
        _ => {}
    }
}

fn str_field<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    value.get(key).and_then(Value::as_str)
}

/// Tracks tool calls that touch dotenv files, so their results are redacted as dotenv content.
#[derive(Debug, Default, Clone)]
pub struct EnvCalls {
    ids: HashSet<String>,
    /// Protected Claude Bash calls awaiting the result that may name a background shell.
    pending_bash: HashSet<String>,
    /// Background shells whose BashOutput and TaskOutput results need dotenv redaction.
    claude_shells: HashSet<String>,
    /// An Antigravity model response called a tool on a dotenv file: its steps name no call, so
    /// the tool steps up to the next response are redacted as dotenv content (SPEC-525).
    steps_touch_env: bool,
    /// An Antigravity command on a dotenv file ran: one that went on as a task is read by a later
    /// `manage_task`, whose steps are then redacted as dotenv content too.
    commands_touched_env: bool,
    /// What a tracked Codex call goes on as: `cell:<id>` (a code-mode script that yielded, read by
    /// `wait`) and `session:<id>` (a command still running, read by `write_stdin`).
    continuations: HashSet<String>,
}

impl EnvCalls {
    fn touches_env(name: &str, input: &Value) -> bool {
        match (name, input) {
            // A free-text call: a Codex code-mode `exec` script (`tools.exec_command({cmd:…})`),
            // a patch, or arguments that did not parse.
            (_, Value::String(script)) => script_mentions_env(script),
            ("apply_patch", _) => patch_files(input).iter().any(|path| is_env_file(path)),
            (SHELL_TOOLS!(), _) => {
                command_of(input).is_some_and(|command| mentions_env_file(&command))
            }
            // A file tool's path, also one level down (Antigravity's `call_mcp_tool` keeps its
            // tool's arguments in `Arguments`).
            _ => std::iter::once(input)
                .chain(input.as_object().into_iter().flat_map(|map| map.values()))
                .flat_map(|value| {
                    [
                        "file_path",
                        "path",
                        "notebook_path",
                        "Path",
                        "AbsolutePath",
                        "TargetFile",
                    ]
                    .iter()
                    .filter_map(move |key| str_field(value, key))
                })
                .any(is_env_file),
        }
    }
}

/// Whether a script names a dotenv file, its words split at quotes, backticks and brackets too
/// (`{cmd:`cat .env`}`).
fn script_mentions_env(script: &str) -> bool {
    let mut words = String::with_capacity(script.len());
    let mut chars = script.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            // An escape (`\n`, `\"`, `\\`) separates words as what it stands for does.
            chars.next();
            words.push(' ');
        } else if matches!(
            c,
            '`' | '"' | '\'' | '{' | '}' | '[' | ']' | ',' | ':' | '='
        ) {
            words.push(' ');
        } else {
            words.push(c);
        }
    }
    mentions_env_file(&words)
}

/// Whether a Codex call reads on from a tracked one: a `wait` for a script that yielded, or a
/// `write_stdin` (a function call, or in a code-mode script) to a command still running.
fn continues_tracked(name: &str, input: &Value, env_calls: &EnvCalls) -> bool {
    let id_of = |value: &Value| match value {
        Value::String(text) => Some(text.clone()),
        Value::Number(number) => Some(number.to_string()),
        _ => None,
    };
    let known = |key: String| env_calls.continuations.contains(&key);
    match (name, input) {
        ("wait", _) => input
            .get("cell_id")
            .and_then(id_of)
            .is_some_and(|id| known(format!("cell:{id}"))),
        ("write_stdin", _) => input
            .get("session_id")
            .and_then(id_of)
            .is_some_and(|id| known(format!("session:{id}"))),
        // A script names its sessions as it likes (`[7, 9].map(session_id => …)`, a variable):
        // any tracked session id in a script that writes to a session continues it.
        (_, Value::String(script)) if script.contains("write_stdin(") => script
            .split(|c: char| !c.is_ascii_alphanumeric())
            .any(|word| !word.is_empty() && known(format!("session:{word}"))),
        _ => false,
    }
}

/// Keeps what a tracked Codex call's output says it goes on as (see [`EnvCalls`]).
fn keep_continuations(parts: &[String], env_calls: &mut EnvCalls) {
    for part in parts {
        if let Some((_, rest)) = part.split_once("Script running with cell ID ") {
            let id: String = rest
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '-')
                .collect();
            if !id.is_empty() {
                env_calls.continuations.insert(format!("cell:{id}"));
            }
        }
        if let Ok(value) = serde_json::from_str::<Value>(part) {
            let mut stack = vec![&value];
            while let Some(value) = stack.pop() {
                match value {
                    Value::Object(map) => {
                        match map.get("session_id") {
                            Some(Value::Number(id)) => {
                                env_calls.continuations.insert(format!("session:{id}"));
                            }
                            Some(Value::String(id)) => {
                                env_calls.continuations.insert(format!("session:{id}"));
                            }
                            _ => {}
                        }
                        stack.extend(map.values());
                    }
                    Value::Array(items) => stack.extend(items),
                    _ => {}
                }
            }
        }
    }
}

/// The first command of a Codex code-mode script: the quoted `cmd` of its first
/// `tools.exec_command({cmd: …})`.
fn code_mode_command(script: &str) -> Option<&str> {
    let (_, rest) = script.split_once("exec_command(")?;
    let (_, rest) = rest.split_once("cmd:")?;
    let rest = rest.trim_start();
    let quote = rest
        .chars()
        .next()
        .filter(|c| matches!(c, '`' | '"' | '\''))?;
    let rest = &rest[quote.len_utf8()..];
    rest.split_once(quote).map(|(command, _)| command)
}

/// The shell tools of the three harnesses: Claude's `Bash`, Codex's `exec_command`, `shell` and
/// `shell_command`, Antigravity's `run_command`.
macro_rules! SHELL_TOOLS {
    () => {
        "Bash" | "exec_command" | "shell" | "shell_command" | "run_command"
    };
}
use SHELL_TOOLS;

/// The command line of a shell tool's input: Claude's `command`, Codex's `cmd` or `command` (an
/// argument list for `shell`), Antigravity's `CommandLine`.
fn command_of(input: &Value) -> Option<String> {
    ["command", "cmd", "CommandLine"]
        .iter()
        .find_map(|key| match input.get(*key)? {
            Value::String(text) => Some(text.clone()),
            Value::Array(parts) => Some(
                parts
                    .iter()
                    .filter_map(Value::as_str)
                    .collect::<Vec<_>>()
                    .join(" "),
            ),
            _ => None,
        })
}

/// The files a Codex `apply_patch` names (`*** Update File: <path>` and its kin).
fn patch_files(input: &Value) -> Vec<&str> {
    // A custom tool's text, or a function call's `{"input": …}`.
    let Some(patch) = input.as_str().or_else(|| str_field(input, "input")) else {
        return Vec::new();
    };
    patch
        .lines()
        .filter_map(|line| {
            ["*** Add File: ", "*** Update File: ", "*** Delete File: "]
                .iter()
                .find_map(|prefix| line.strip_prefix(prefix))
        })
        .map(str::trim)
        .collect()
}

/// One-line description of a tool call.
fn tool_summary(name: &str, input: &Value) -> String {
    let field = |key| str_field(input, key).map(one_line);
    let summary = match name {
        SHELL_TOOLS!() => command_of(input).map(|c| one_line(&c)),
        "apply_patch" => {
            Some(one_line(&patch_files(input).join(", "))).filter(|files| !files.is_empty())
        }
        "exec" => input.as_str().and_then(code_mode_command).map(one_line),
        "read_file" | "view_file" | "edit_file" | "write_file" => field("Path"),
        "Read" | "Write" | "Edit" | "MultiEdit" => field("file_path"),
        "NotebookEdit" => field("notebook_path"),
        "Glob" | "Grep" => field("pattern").map(|pattern| {
            field("path").map_or_else(|| pattern.clone(), |path| format!("{pattern} in {path}"))
        }),
        "WebFetch" => field("url"),
        "WebSearch" => field("query"),
        "Task" | "Agent" => field("description").or_else(|| field("prompt")),
        "TodoWrite" => input
            .get("todos")
            .and_then(Value::as_array)
            .map(|todos| format!("{} todo(s)", todos.len())),
        _ => None,
    };
    summary.unwrap_or_else(|| {
        if let Value::String(text) = input {
            return one_line(text);
        }
        let compact = serde_json::to_string(input).unwrap_or_default();
        if compact == "{}" {
            String::new()
        } else {
            one_line(&compact)
        }
    })
}

fn bounded(text: &str, max: usize) -> String {
    cut(text, max).0
}

/// The edit an edit tool made; `truncated` when a side was cut or hunks were dropped.
fn diff_of(name: &str, input: &Value) -> Option<TranscriptDiff> {
    let path = str_field(input, "file_path")?.to_owned();
    let mut truncated = false;
    let mut hunk = |old: &str, new: &str| {
        let (old, old_cut) = cut(old, MAX_HUNK_CHARS);
        let (new, new_cut) = cut(new, MAX_HUNK_CHARS);
        truncated |= old_cut || new_cut;
        DiffHunk { old, new }
    };
    let hunks: Vec<DiffHunk> = match name {
        "Edit" => vec![hunk(
            str_field(input, "old_string").unwrap_or(""),
            str_field(input, "new_string").unwrap_or(""),
        )],
        "Write" => vec![hunk("", str_field(input, "content").unwrap_or(""))],
        "MultiEdit" => {
            let edits = input.get("edits").and_then(Value::as_array)?;
            let hunks = edits
                .iter()
                .take(MAX_HUNKS)
                .map(|edit| {
                    hunk(
                        str_field(edit, "old_string").unwrap_or(""),
                        str_field(edit, "new_string").unwrap_or(""),
                    )
                })
                .collect();
            truncated |= edits.len() > MAX_HUNKS;
            hunks
        }
        _ => return None,
    };
    Some(TranscriptDiff {
        path,
        hunks,
        truncated,
    })
}

/// A meta block (harness bookkeeping) cut at `max`.
fn meta_block(text: &str, max: usize) -> TranscriptBlock {
    let (text, truncated) = cut(text, max);
    TranscriptBlock::Meta {
        text: clean(&text, false),
        truncated,
    }
}

fn result_text(content: Option<&Value>) -> String {
    match content {
        Some(Value::String(text)) => text.clone(),
        Some(Value::Array(parts)) => parts
            .iter()
            .filter_map(|part| match str_field(part, "type") {
                Some("text") => str_field(part, "text").map(str::to_owned),
                Some("image") => Some("[image]".to_owned()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

fn is_meta_text(text: &str) -> bool {
    let trimmed = text.trim_start();
    META_PREFIXES
        .iter()
        .any(|prefix| trimmed.starts_with(prefix))
}

/// Normalizes one parsed transcript line; `None` for bookkeeping lines (snapshots, queue
/// operations, progress) and lines without visible content.
#[must_use]
pub fn entry_of(line: u64, record: &Value, env_calls: &mut EnvCalls) -> Option<TranscriptEntry> {
    if record.get("step_index").is_some() && record.get("source").is_some() {
        return antigravity_entry(line, record, env_calls);
    }
    let kind = str_field(record, "type")?;
    let at = str_field(record, "timestamp").map(str::to_owned);
    let sidechain = record
        .get("isSidechain")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let meta_line = record.get("isMeta").and_then(Value::as_bool) == Some(true);
    let mut blocks = Vec::new();
    let role = match kind {
        "summary" => {
            let (text, truncated) = cut(str_field(record, "summary")?, MAX_TEXT_CHARS);
            blocks.push(TranscriptBlock::Text {
                text: clean(&text, false),
                truncated,
            });
            "summary"
        }
        "system" => {
            let text = str_field(record, "content")?;
            blocks.push(meta_block(text, MAX_TEXT_CHARS));
            "system"
        }
        "user" | "assistant" => {
            let content = record.get("message").and_then(|m| m.get("content"))?;
            match content {
                Value::String(text) => blocks.push(text_block(text, meta_line)),
                Value::Array(parts) => {
                    // Record-level metadata identifies the shell when only one result owns it.
                    let background_id = record
                        .get("toolUseResult")
                        .and_then(|r| str_field(r, "backgroundTaskId"))
                        .filter(|id| !id.is_empty())
                        .filter(|_| {
                            parts
                                .iter()
                                .filter(|p| str_field(p, "type") == Some("tool_result"))
                                .take(2)
                                .count()
                                == 1
                        });
                    for part in parts {
                        if let Some(block) =
                            content_block(part, meta_line, background_id, env_calls)
                        {
                            blocks.push(block);
                        }
                    }
                }
                _ => {}
            }
            kind
        }
        // A Codex rollout (SPEC-525): its items; `event_msg` lines repeat them or count.
        "response_item" => {
            let (role, item) = codex_item(record.get("payload")?, env_calls)?;
            blocks.push(item);
            role
        }
        "compacted" => {
            // Codex 0.169 leaves `message` empty (its summary is in `replacement_history`).
            let text = record
                .get("payload")
                .and_then(|p| str_field(p, "message"))
                .filter(|text| !text.trim().is_empty())?;
            let (text, truncated) = cut(text, MAX_TEXT_CHARS);
            blocks.push(TranscriptBlock::Text {
                text: clean(&text, false),
                truncated,
            });
            "summary"
        }
        _ => return None,
    };
    entry(line, role, at, sidechain, blocks)
}

fn entry(
    line: u64,
    role: &str,
    at: Option<String>,
    sidechain: bool,
    blocks: Vec<TranscriptBlock>,
) -> Option<TranscriptEntry> {
    if blocks.is_empty() {
        return None;
    }
    Some(TranscriptEntry {
        line,
        role: role.to_owned(),
        at,
        sidechain,
        blocks,
    })
}

/// One Codex `response_item` payload as a block and its role: a message (developer instructions
/// left out, context Codex added shown quietly), a reasoning summary, a tool call or its output.
fn codex_item(
    payload: &Value,
    env_calls: &mut EnvCalls,
) -> Option<(&'static str, TranscriptBlock)> {
    match str_field(payload, "type")? {
        "message" => {
            let role = match str_field(payload, "role")? {
                "user" => "user",
                "assistant" => "assistant",
                _ => return None,
            };
            let text = codex_texts(payload.get("content")).join("\n\n");
            if text.trim().is_empty() {
                return None;
            }
            let added = role == "user" && codex_added(payload, &text);
            Some((
                role,
                if added {
                    meta_block(&text, MAX_RESULT_CHARS)
                } else {
                    text_block(&text, false)
                },
            ))
        }
        "reasoning" => {
            let summary = payload.get("summary").and_then(Value::as_array)?;
            let text = summary
                .iter()
                .filter_map(|part| str_field(part, "text"))
                .collect::<Vec<_>>()
                .join("\n\n");
            if text.trim().is_empty() {
                return None;
            }
            let (text, truncated) = cut(&text, MAX_THINKING_CHARS);
            Some((
                "assistant",
                TranscriptBlock::Thinking {
                    text: clean(&text, false),
                    truncated,
                },
            ))
        }
        "function_call" | "custom_tool_call" | "local_shell_call" => {
            let id = str_field(payload, "call_id")
                .or_else(|| str_field(payload, "id"))
                .unwrap_or("");
            let name = str_field(payload, "name").unwrap_or("shell");
            let input = match payload.get("arguments") {
                Some(Value::String(text)) => {
                    serde_json::from_str(text).unwrap_or_else(|_| Value::String(text.clone()))
                }
                Some(value) => value.clone(),
                None => payload
                    .get("input")
                    .or_else(|| payload.get("action"))
                    .cloned()
                    .unwrap_or(Value::Null),
            };
            let continued = continues_tracked(name, &input, env_calls);
            let (block, env) = tool_call(id, name, input);
            if (env || continued) && !id.is_empty() {
                env_calls.ids.insert(id.to_owned());
            }
            Some(("assistant", block))
        }
        "function_call_output" | "custom_tool_call_output" | "local_shell_call_output" => {
            let tool_use_id = str_field(payload, "call_id").unwrap_or("").to_owned();
            let env = env_calls.ids.contains(&tool_use_id);
            let parts = match payload.get("output") {
                Some(Value::Object(output)) => ["content", "output"]
                    .iter()
                    .find_map(|key| output.get(*key))
                    .map_or_else(Vec::new, |value| codex_texts(Some(value))),
                other => codex_texts(other),
            };
            if env {
                keep_continuations(&parts, env_calls);
            }
            let raw = output_text(&parts, env);
            let (text, truncated) = cut(&raw, MAX_RESULT_CHARS);
            Some((
                "user",
                TranscriptBlock::ToolResult {
                    text: clean(&text, env),
                    is_error: false,
                    tool_use_id,
                    truncated,
                },
            ))
        }
        _ => None,
    }
}

/// A Codex tool output's parts as one text. A code-mode `exec` prints each command's result as
/// JSON (`{"chunk_id", "exit_code", "output", …}`), also nested (a `Promise.allSettled` list,
/// `{status, value}`, an MCP tool's `{content, isError}`): the texts in it are read out, so a
/// dotenv file printed there reads as lines. Other JSON is kept as it is, its strings redacted as
/// dotenv content when the call read a dotenv file (`env`).
fn output_text(parts: &[String], env: bool) -> String {
    let mut lines = Vec::new();
    for part in parts {
        match serde_json::from_str::<Value>(part) {
            Ok(value @ (Value::Object(_) | Value::Array(_))) => {
                let mut texts = Vec::new();
                if is_code_mode_result(&value) {
                    json_texts(&value, &mut texts);
                }
                if !texts.is_empty() {
                    lines.push(joined(texts.into_iter().map(str::to_owned)));
                } else if env {
                    let mut value = value;
                    redact_strings_as_env(&mut value);
                    lines.push(value.to_string());
                } else {
                    lines.push(part.clone());
                }
            }
            _ => lines.push(part.clone()),
        }
    }
    joined(lines.into_iter())
}

/// The texts a code-mode result holds: each `output`, an MCP result's `content[].text`, a
/// settled promise's `value`, `reason` or `error`, the strings of a list; other objects are
/// searched.
fn json_texts<'a>(value: &'a Value, texts: &mut Vec<&'a str>) {
    match value {
        Value::Object(map) => {
            if let Some(Value::String(output)) = map.get("output") {
                texts.push(output);
            } else if let (Some("text"), Some(text)) =
                (str_field(value, "type"), str_field(value, "text"))
            {
                // A content item (`{"type": "text", "text": …}`), also in a settled `value`.
                texts.push(text);
            } else if let Some(Value::Array(content)) = map.get("content") {
                texts.extend(content.iter().filter_map(|item| str_field(item, "text")));
            } else {
                for key in ["value", "reason", "error"] {
                    if let Some(Value::String(text)) = map.get(key) {
                        texts.push(text);
                    }
                }
                for item in map.values().filter(|v| v.is_object() || v.is_array()) {
                    json_texts(item, texts);
                }
            }
        }
        Value::Array(items) => {
            for item in items {
                match item {
                    Value::String(text) => texts.push(text),
                    other => json_texts(other, texts),
                }
            }
        }
        _ => {}
    }
}

/// Whether JSON is a code-mode result: it holds a command's result (`chunk_id`), a settled
/// promise (`status` `fulfilled` or `rejected`) or an MCP tool's result (`content` and
/// `isError`). Other JSON is shown as it is.
fn is_code_mode_result(value: &Value) -> bool {
    match value {
        Value::Object(map) => {
            map.contains_key("chunk_id")
                || matches!(
                    map.get("status").and_then(Value::as_str),
                    Some("fulfilled" | "rejected")
                )
                || (map.get("content").is_some_and(Value::is_array) && map.contains_key("isError"))
                || map.values().any(is_code_mode_result)
        }
        Value::Array(items) => items.iter().any(is_code_mode_result),
        _ => false,
    }
}

/// Every string of a JSON value redacted as dotenv content.
fn redact_strings_as_env(value: &mut Value) {
    match value {
        Value::String(text) => *text = clean(text, true),
        Value::Array(items) => items.iter_mut().for_each(redact_strings_as_env),
        Value::Object(map) => map.values_mut().for_each(redact_strings_as_env),
        _ => {}
    }
}

/// Texts one after another, each on its own line.
fn joined(texts: impl Iterator<Item = String>) -> String {
    let mut text = String::new();
    for part in texts {
        if !text.is_empty() && !text.ends_with('\n') {
            text.push('\n');
        }
        text.push_str(&part);
    }
    text
}

/// The texts of a Codex content value: a string, or the text parts of a list.
fn codex_texts(content: Option<&Value>) -> Vec<String> {
    match content {
        Some(Value::String(text)) => vec![text.clone()],
        Some(Value::Array(parts)) => parts
            .iter()
            .filter_map(|part| str_field(part, "text").map(str::to_owned))
            .collect(),
        _ => Vec::new(),
    }
}

/// Whether Codex added a user-role message (its environment context, a blocking hook's
/// continuation) rather than a person typing it: its provenance says so when Codex wrote one
/// (0.169: `harness` or `user`), else a text that opens as Codex's injected context does.
fn codex_added(payload: &Value, text: &str) -> bool {
    let kinds: Vec<&str> = payload
        .get("internal_chat_message_metadata_passthrough")
        .and_then(|meta| meta.get("content_item_metadata"))
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.get("provenance")?.get("type")?.as_str())
                .filter(|kind| *kind != "unknown")
                .collect()
        })
        .unwrap_or_default();
    if !kinds.is_empty() {
        return !kinds.contains(&"user");
    }
    is_codex_injected_context(text)
}

/// One Antigravity transcript step (SPEC-525): the person's request without the CLI's wrapper,
/// the model's answer and tool calls, a system message, an API error, a tool step.
fn antigravity_entry(line: u64, step: &Value, env_calls: &mut EnvCalls) -> Option<TranscriptEntry> {
    let at = str_field(step, "created_at").map(str::to_owned);
    let index = step
        .get("step_index")
        .and_then(Value::as_u64)
        .unwrap_or(line);
    let content = str_field(step, "content").filter(|text| !text.trim().is_empty());
    let error = str_field(step, "error").filter(|text| !text.trim().is_empty());
    let mut blocks = Vec::new();
    let role = match (str_field(step, "source"), str_field(step, "type")) {
        (_, Some("USER_INPUT")) => {
            let request = user_request(content?);
            if request.is_empty() {
                return None;
            }
            blocks.push(text_block(&request, false));
            "user"
        }
        (_, Some("PLANNER_RESPONSE")) => {
            env_calls.steps_touch_env = false;
            if let Some(text) = str_field(step, "thinking").filter(|t| !t.trim().is_empty()) {
                let (text, truncated) = cut(text, MAX_THINKING_CHARS);
                blocks.push(TranscriptBlock::Thinking {
                    text: clean(&text, false),
                    truncated,
                });
            }
            if let Some(text) = content {
                blocks.push(text_block(text, false));
            }
            let calls = step.get("tool_calls").and_then(Value::as_array);
            for (n, call) in calls.into_iter().flatten().enumerate() {
                let name = str_field(call, "name").unwrap_or("tool");
                let input = antigravity_args(call.get("args"));
                let (block, env) = tool_call(&format!("step-{index}-{n}"), name, input);
                if env && name == "run_command" {
                    env_calls.commands_touched_env = true;
                }
                // A command that went on as a task is read by `manage_task`.
                env_calls.steps_touch_env |=
                    env || (name == "manage_task" && env_calls.commands_touched_env);
                blocks.push(block);
            }
            "assistant"
        }
        (_, Some("SYSTEM_MESSAGE")) => {
            // A command that went on as a task reports its result as a system message
            // (`Task id "…" finished with result: …`): redacted as dotenv content once a command
            // on a dotenv file ran.
            let text = content?;
            // `finished with result`, `was canceled with result`.
            let env = env_calls.commands_touched_env && text.contains(" with result");
            let (text, truncated) = cut(text, MAX_RESULT_CHARS);
            blocks.push(TranscriptBlock::Meta {
                text: clean(&text, env),
                truncated,
            });
            "system"
        }
        (Some("MODEL"), _) | (_, Some("ERROR_MESSAGE")) => {
            let raw = error.or(content)?;
            let is_error = error.is_some() || str_field(step, "status") == Some("ERROR");
            let (text, truncated) = cut(raw, MAX_RESULT_CHARS);
            blocks.push(TranscriptBlock::ToolResult {
                tool_use_id: format!("step-{index}"),
                text: clean(&text, env_calls.steps_touch_env),
                is_error,
                truncated,
            });
            "user"
        }
        _ => {
            blocks.push(meta_block(content.or(error)?, MAX_RESULT_CHARS));
            "system"
        }
    };
    entry(line, role, at, false, blocks)
}

/// The person's words in an Antigravity `USER_INPUT`: the text inside `<USER_REQUEST>`, without
/// the metadata the CLI adds after it; the whole content when there is no such tag.
fn user_request(content: &str) -> String {
    content
        .split_once("<USER_REQUEST>")
        .and_then(|(_, rest)| rest.split_once("</USER_REQUEST>"))
        .map_or(content, |(request, _)| request)
        .trim()
        .to_owned()
}

/// An Antigravity tool call's arguments: each value is written as JSON text (`"\"ls\""`), read
/// back as the value it holds.
fn antigravity_args(args: Option<&Value>) -> Value {
    match args {
        Some(Value::Object(map)) => Value::Object(
            map.iter()
                .map(|(key, value)| {
                    let value = match value {
                        Value::String(text) => {
                            serde_json::from_str(text).unwrap_or_else(|_| value.clone())
                        }
                        other => other.clone(),
                    };
                    (key.clone(), value)
                })
                .collect(),
        ),
        Some(other) => other.clone(),
        None => Value::Null,
    }
}

/// A tool call block: its input redacted (as dotenv content when it touches a dotenv file, whose
/// result is then redacted the same way) and summarized; with whether it touches one.
fn tool_call(id: &str, name: &str, mut input: Value) -> (TranscriptBlock, bool) {
    let env = EnvCalls::touches_env(name, &input);
    // A script that writes a dotenv file in a quoted string (`"…\n+KEY=value\n…"`, with `\n`
    // escapes) has its lines apart, so each is redacted as a line.
    if let (true, Value::String(script)) = (env, &mut input)
        && script.contains("\\n")
    {
        *script = script.replace("\\n", "\n");
    }
    redact_value(&mut input, env);
    // A patch is text: shown as written, not as a JSON string.
    let pretty = match &input {
        Value::String(text) => text.clone(),
        other => serde_json::to_string_pretty(other).unwrap_or_default(),
    };
    let block = TranscriptBlock::ToolCall {
        summary: tool_summary(name, &input),
        diff: diff_of(name, &input),
        input: bounded(&pretty, MAX_INPUT_CHARS),
        id: id.to_owned(),
        name: name.to_owned(),
    };
    (block, env)
}

fn text_block(text: &str, meta: bool) -> TranscriptBlock {
    if meta || is_meta_text(text) {
        return meta_block(text, MAX_RESULT_CHARS);
    }
    let (text, truncated) = cut(text, MAX_TEXT_CHARS);
    TranscriptBlock::Text {
        text: clean(&text, false),
        truncated,
    }
}

/// Claude's rendered Bash launch notice, used when the transcript omits toolUseResult.
fn background_shell_id(text: &str) -> Option<&str> {
    text.lines().rev().find_map(|notice| {
        if let Some(rest) = notice
            .strip_prefix("Command running in background with ID: ")
            .or_else(|| notice.strip_prefix("Command was manually backgrounded by user with ID: "))
        {
            return rest
                .split_ascii_whitespace()
                .next()
                .map(|id| id.trim_end_matches('.'));
        }
        let (_, rest) = notice
            .strip_prefix("Command ")?
            .split_once("background (ID: ")?;
        rest.split_once(')').map(|(id, _)| id)
    })
}

fn content_block(
    part: &Value,
    meta: bool,
    background_id: Option<&str>,
    env_calls: &mut EnvCalls,
) -> Option<TranscriptBlock> {
    match str_field(part, "type")? {
        "text" => {
            let text = str_field(part, "text")?;
            (!text.trim().is_empty()).then(|| text_block(text, meta))
        }
        "thinking" => {
            let text = str_field(part, "thinking")?;
            if text.trim().is_empty() {
                return None;
            }
            let (text, truncated) = cut(text, MAX_THINKING_CHARS);
            Some(TranscriptBlock::Thinking {
                text: clean(&text, false),
                truncated,
            })
        }
        "tool_use" | "server_tool_use" => {
            let id = str_field(part, "id").unwrap_or("");
            let name = str_field(part, "name").unwrap_or("tool");
            let input = part.get("input").unwrap_or(&Value::Null);
            let shell = match name {
                "BashOutput" => str_field(input, "bash_id"),
                "TaskOutput" => str_field(input, "task_id"),
                _ => None,
            };
            let (block, env) = tool_call(id, name, input.clone());
            if !id.is_empty()
                && (env || shell.is_some_and(|id| env_calls.claude_shells.contains(id)))
            {
                env_calls.ids.insert(id.to_owned());
                if name == "Bash" {
                    env_calls.pending_bash.insert(id.to_owned());
                }
            }
            Some(block)
        }
        "tool_result" => {
            let tool_use_id = str_field(part, "tool_use_id").unwrap_or("").to_owned();
            let env = env_calls.ids.contains(&tool_use_id);
            let raw = result_text(part.get("content"));
            if env_calls.pending_bash.remove(&tool_use_id)
                && let Some(shell) = background_id.or_else(|| background_shell_id(&raw))
                && !shell.is_empty()
            {
                env_calls.claude_shells.insert(shell.to_owned());
            }
            let (text, truncated) = cut(&raw, MAX_RESULT_CHARS);
            Some(TranscriptBlock::ToolResult {
                text: clean(&text, env),
                is_error: part.get("is_error").and_then(Value::as_bool) == Some(true),
                tool_use_id,
                truncated,
            })
        }
        "image" => Some(TranscriptBlock::Image {
            media_type: part
                .get("source")
                .and_then(|s| str_field(s, "media_type"))
                .map(str::to_owned),
        }),
        _ => None,
    }
}

/// The searchable passage of an entry (bounded); `None` for bookkeeping only.
#[must_use]
pub fn chunk_of(entry: &TranscriptEntry) -> Option<SessionChunk> {
    let mut parts = Vec::new();
    let mut tool_only = true;
    for block in &entry.blocks {
        match block {
            TranscriptBlock::Text { text, .. } => {
                tool_only = false;
                parts.push(text.clone());
            }
            TranscriptBlock::Thinking { text, .. } => {
                tool_only = false;
                parts.push(bounded(text, 2 * MAX_CHUNK_RESULT_CHARS));
            }
            TranscriptBlock::ToolCall {
                name,
                summary,
                diff,
                ..
            } => {
                parts.push(format!("{name} {summary}"));
                if let Some(diff) = diff {
                    for hunk in &diff.hunks {
                        parts.push(bounded(&hunk.new, 2 * MAX_CHUNK_RESULT_CHARS));
                    }
                }
            }
            TranscriptBlock::ToolResult { text, .. } => {
                parts.push(bounded(text, MAX_CHUNK_RESULT_CHARS));
            }
            TranscriptBlock::Image { .. } | TranscriptBlock::Meta { .. } => {}
        }
    }
    let text = parts.join("\n");
    if text.trim().is_empty() {
        return None;
    }
    let role = match entry.role.as_str() {
        "summary" => "summary",
        _ if tool_only => "tool",
        "assistant" => "assistant",
        _ => "user",
    };
    Some(SessionChunk {
        line: entry.line,
        role: role.to_owned(),
        text: bounded(&text, MAX_CHUNK_CHARS),
    })
}

/// Result of reading one bounded line.
#[derive(Debug, PartialEq, Eq)]
enum LineRead {
    /// End of file (possibly with a partial last line, which is not returned).
    Eof,
    /// A complete line is in the buffer (without the newline); the byte count read.
    Line(usize),
    /// A complete line longer than the bound was skipped; the byte count read.
    TooLong(usize),
}

/// Reads one `\n`-terminated line into `buf` (at most `max` bytes kept). A final line without a
/// newline is treated as unfinished (the transcript may still be written) and not returned.
fn read_line(reader: &mut impl BufRead, buf: &mut Vec<u8>, max: usize) -> io::Result<LineRead> {
    buf.clear();
    let mut total = 0;
    let mut too_long = false;
    loop {
        let available = reader.fill_buf()?;
        if available.is_empty() {
            return Ok(LineRead::Eof);
        }
        let (chunk, done) = match available.iter().position(|b| *b == b'\n') {
            Some(index) => (&available[..index], Some(index + 1)),
            None => (available, None),
        };
        if !too_long {
            if buf.len() + chunk.len() > max {
                too_long = true;
                buf.clear();
            } else {
                buf.extend_from_slice(chunk);
            }
        }
        let used = done.unwrap_or(available.len());
        total += used;
        reader.consume(used);
        if done.is_some() {
            return Ok(if too_long {
                LineRead::TooLong(total)
            } else {
                LineRead::Line(total)
            });
        }
    }
}

/// A page of entries.
#[derive(Debug, Default)]
pub struct Page {
    /// Entries.
    pub entries: Vec<TranscriptEntry>,
    /// The line to continue from; `None` at the end.
    pub next: Option<u64>,
    /// The line after the last one read: where a reader of a growing transcript continues. A last
    /// line still being written (no newline yet) is not read.
    pub end: u64,
    /// Whether entries before the page were left out ([`read_tail`]).
    pub earlier: bool,
    /// Lines skipped (unreadable or too long).
    pub skipped: u32,
}

fn page_chars(entry: &TranscriptEntry) -> usize {
    entry
        .blocks
        .iter()
        .map(|block| match block {
            TranscriptBlock::Text { text, .. }
            | TranscriptBlock::Thinking { text, .. }
            | TranscriptBlock::Meta { text, .. }
            | TranscriptBlock::ToolResult { text, .. } => text.len(),
            TranscriptBlock::ToolCall { input, diff, .. } => {
                input.len()
                    + diff.as_ref().map_or(0, |d| {
                        d.hunks.iter().map(|h| h.old.len() + h.new.len()).sum()
                    })
            }
            TranscriptBlock::Image { .. } => 0,
        })
        .sum()
}

/// Opens the transcript at `path` for reading (ADR-0174, ADR-0315). A session's own transcript
/// is in its sandbox's home, which the agent writes: below `anchor` (the sandbox directory)
/// every folder on the way is opened from its parent without following a link. The file itself
/// is opened without following a link and without waiting (a FIFO nobody writes would hold the
/// thread for good), and must be a regular file. A path that is not below the anchor is a copy
/// the server keeps in its own folders, opened the same way at its name.
///
/// # Errors
/// `NotFound` when the file or a folder on the way is missing; `InvalidInput` for a link or
/// anything that is not a regular file ([`unreadable`] tells both); other I/O failures.
pub fn open(anchor: &Path, path: &Path) -> io::Result<File> {
    use nix::fcntl::OFlag;
    use nix::sys::stat::Mode;
    let flags = OFlag::O_RDONLY | OFlag::O_NOFOLLOW | OFlag::O_NONBLOCK | OFlag::O_CLOEXEC;
    let not_a_file = || io::Error::new(io::ErrorKind::InvalidInput, "not a regular file");
    let refused = |err: nix::errno::Errno| match err {
        nix::errno::Errno::ELOOP | nix::errno::Errno::ENOTDIR => not_a_file(),
        other => io::Error::from(other),
    };
    let fd = match path.strip_prefix(anchor) {
        Ok(below) => {
            if !below
                .components()
                .all(|part| matches!(part, std::path::Component::Normal(_)))
            {
                return Err(not_a_file());
            }
            let name = below.file_name().ok_or_else(not_a_file)?;
            let folders: Vec<&std::ffi::OsStr> = below
                .parent()
                .map(|folders| folders.iter().collect())
                .unwrap_or_default();
            let dir = crate::files::open_dirs_below(anchor, &folders, false).map_err(refused)?;
            nix::fcntl::openat(&dir, name, flags, Mode::empty()).map_err(refused)?
        }
        Err(_) => nix::fcntl::open(path, flags, Mode::empty()).map_err(refused)?,
    };
    let file = File::from(fd);
    if !file.metadata()?.is_file() {
        return Err(not_a_file());
    }
    Ok(file)
}

/// Whether `err` of [`open`] says there is no transcript to read: it is missing, or what is in
/// its place is no regular file (a link or a FIFO an agent put there). A reader shows that as
/// no transcript, not as a failure of the server.
#[must_use]
pub fn unreadable(err: &io::Error) -> bool {
    matches!(
        err.kind(),
        io::ErrorKind::NotFound | io::ErrorKind::InvalidInput
    )
}

/// Validates a pinned Codex CLI review child's first rollout record (SPEC-605).
pub(crate) struct CodexReviewStart {
    pub(crate) head: Option<String>,
    pub(crate) started_at: yulius_domain::time::Timestamp,
}

pub(crate) fn codex_review_start(
    file: File,
    parent: &str,
    child: &str,
) -> Option<CodexReviewStart> {
    let mut line = Vec::new();
    let mut reader = BufReader::new(file.take(4 << 20));
    reader.read_until(b'\n', &mut line).ok()?;
    if !line.ends_with(b"\n") {
        return None;
    }
    let row: Value = serde_json::from_slice(&line).ok()?;
    if str_field(&row, "type") != Some("session_meta") {
        return None;
    }
    let meta = &row["payload"];
    let spawn = &meta["source"]["subagent"]["thread_spawn"];
    let suffix = str_field(spawn, "agent_path")?.strip_prefix("/root/yulius_review_")?;
    if str_field(meta, "id") != Some(child)
        || str_field(spawn, "parent_thread_id") != Some(parent)
        || spawn["depth"].as_u64() != Some(1)
        || !suffix.bytes().all(|byte| byte.is_ascii_digit())
        || suffix.parse::<u32>().ok()? == 0
    {
        return None;
    }
    let head = str_field(&meta["git"], "commit_hash")
        .filter(|sha| matches!(sha.len(), 40 | 64) && sha.bytes().all(|b| b.is_ascii_hexdigit()))
        .map(str::to_owned);
    let started_at = str_field(meta, "timestamp")
        .or_else(|| str_field(&row, "timestamp"))
        .and_then(yulius_domain::time::parse_rfc3339)?;
    Some(CodexReviewStart { head, started_at })
}

/// Where a read of a transcript can go on without reading what came before (SPEC-483 slice 36):
/// the line number, the byte offset of its start, and the dotenv calls seen on the lines before
/// it, so a dotenv file read earlier is still redacted after it. Kept by the server, never sent
/// to a reader: a reader's own state could leave a dotenv read out.
#[derive(Debug, Clone, Default)]
pub struct ReadCursor {
    /// The line it is at.
    pub line: u64,
    /// The byte offset of that line's start.
    pub byte: u64,
    /// The file it is in (device, inode): a transcript replaced by another file (a step-back
    /// renames a rewritten copy over it) has none of the old file's points.
    file: Option<(u64, u64)>,
    env_calls: EnvCalls,
}

/// Reads entries from line `from`: at most `limit` entries (≤ [`MAX_PAGE_ENTRIES`]) and about
/// `max_chars` of text (≤ [`MAX_PAGE_CHARS`]). Tool calls on earlier lines are scanned too, so
/// a dotenv file read before the page is still redacted in it.
///
/// # Errors
/// I/O failures reading the transcript.
pub fn read_page(file: File, from: u64, limit: usize, max_chars: usize) -> io::Result<Page> {
    Ok(read_page_at(file, None, from, limit, max_chars)?.0)
}

impl ReadCursor {
    /// Whether this point should replace `kept` for the same transcript, now `len` bytes long:
    /// it is of another file, or as far on or farther (two watchers at different lines keep the
    /// farther point), or the file was shortened in place below the kept point.
    #[must_use]
    pub fn replaces(&self, kept: &Self, len: u64) -> bool {
        self.file != kept.file || self.line >= kept.line || kept.byte > len
    }
}

/// [`read_page`] that goes on from `cursor` when it can: when the cursor is at or before `from`
/// and still at a line's start (the byte before it a newline), the lines before it are not read
/// again. Also returns the cursor at the page's end, for the next read.
///
/// # Errors
/// I/O failures reading the transcript.
pub fn read_page_at(
    mut file: File,
    cursor: Option<&ReadCursor>,
    from: u64,
    limit: usize,
    max_chars: usize,
) -> io::Result<(Page, ReadCursor)> {
    use std::os::unix::fs::MetadataExt as _;
    let limit = limit.clamp(1, MAX_PAGE_ENTRIES);
    let max_chars = max_chars.clamp(1, MAX_PAGE_CHARS);
    let metadata = file.metadata()?;
    let identity = Some((metadata.dev(), metadata.ino()));
    let start = match cursor {
        Some(cursor)
            if cursor.file == identity
                && cursor.line <= from
                && at_line_start(&mut file, cursor.byte)? =>
        {
            cursor.clone()
        }
        _ => ReadCursor::default(),
    };
    file.seek(SeekFrom::Start(start.byte))?;
    let mut reader = BufReader::with_capacity(256 * 1024, file);
    let mut buf = Vec::new();
    let mut env_calls = start.env_calls;
    let mut page = Page::default();
    let mut chars = 0;
    let mut line = start.line;
    let mut byte = start.byte;
    loop {
        let at = ReadCursor {
            line,
            byte,
            file: identity,
            env_calls: EnvCalls::default(),
        };
        let read = read_line(&mut reader, &mut buf, MAX_LINE_BYTES)?;
        let current = line;
        match read {
            LineRead::Eof => {
                page.end = line;
                return Ok((page, ReadCursor { env_calls, ..at }));
            }
            LineRead::TooLong(used) => {
                line += 1;
                byte += used as u64;
                if current >= from {
                    page.skipped += 1;
                }
                continue;
            }
            LineRead::Line(used) => {
                line += 1;
                byte += used as u64;
            }
        }
        if current < from {
            // A pending Bash launch can acquire its shell ID on a result before this page.
            if (!env_calls.pending_bash.is_empty() || may_change_env_tracking(&buf))
                && let Ok(record) = serde_json::from_slice::<Value>(&buf)
            {
                let _ = entry_of(current, &record, &mut env_calls);
            }
            continue;
        }
        if page.entries.len() >= limit || chars >= max_chars {
            page.next = Some(current);
            page.end = current;
            return Ok((page, ReadCursor { env_calls, ..at }));
        }
        if buf.iter().all(u8::is_ascii_whitespace) {
            continue;
        }
        match serde_json::from_slice::<Value>(&buf) {
            Ok(record) => {
                if let Some(entry) = entry_of(current, &record, &mut env_calls) {
                    chars += page_chars(&entry);
                    page.entries.push(entry);
                }
            }
            Err(_) => page.skipped += 1,
        }
    }
}

/// Whether `byte` is at a line's start in `file`: the start of the file, or just after a
/// newline within it.
fn at_line_start(file: &mut File, byte: u64) -> io::Result<bool> {
    if byte == 0 {
        return Ok(true);
    }
    if file.metadata()?.len() < byte {
        return Ok(false);
    }
    file.seek(SeekFrom::Start(byte - 1))?;
    let mut previous = [0_u8; 1];
    Ok(file.read(&mut previous)? == 1 && previous[0] == b'\n')
}

/// Reads the last `limit` entries (≤ [`MAX_PAGE_ENTRIES`]) within about `max_chars` of text (≤
/// [`MAX_PAGE_CHARS`]; the last entry is kept whatever its size): the tail of a live session
/// (SPEC-179 WAT-001). The whole transcript is read, so a dotenv file read before the tail is still
/// redacted in it. `earlier` says entries were left out; `end` is where a reader continues.
///
/// # Errors
/// I/O failures reading the transcript.
pub fn read_tail(file: File, limit: usize, max_chars: usize) -> io::Result<Page> {
    let limit = limit.clamp(1, MAX_PAGE_ENTRIES);
    let max_chars = max_chars.clamp(1, MAX_PAGE_CHARS);
    let mut reader = BufReader::with_capacity(256 * 1024, file);
    let mut buf = Vec::new();
    let mut env_calls = EnvCalls::default();
    let mut tail: VecDeque<(usize, TranscriptEntry)> = VecDeque::new();
    let mut page = Page::default();
    let mut chars = 0;
    let mut line = 0_u64;
    loop {
        let current = line;
        match read_line(&mut reader, &mut buf, MAX_LINE_BYTES)? {
            LineRead::Eof => break,
            LineRead::TooLong(_) => {
                line += 1;
                page.skipped += 1;
                continue;
            }
            LineRead::Line(_) => line += 1,
        }
        if buf.iter().all(u8::is_ascii_whitespace) {
            continue;
        }
        match serde_json::from_slice::<Value>(&buf) {
            Ok(record) => {
                if let Some(entry) = entry_of(current, &record, &mut env_calls) {
                    let size = page_chars(&entry);
                    chars += size;
                    tail.push_back((size, entry));
                    while tail.len() > 1 && (tail.len() > limit || chars > max_chars) {
                        if let Some((size, _)) = tail.pop_front() {
                            chars -= size;
                            page.earlier = true;
                        }
                    }
                }
            }
            Err(_) => page.skipped += 1,
        }
    }
    page.entries = tail.into_iter().map(|(_, entry)| entry).collect();
    page.end = line;
    Ok(page)
}

/// Whether a line before a page can change the dotenv tracking: a tool call (Claude Code's
/// `tool_use` or `server_tool_use`, Codex's calls), an Antigravity model response (its `tool_calls` start the
/// tracking, and each response ends the one before), or a Codex output naming a continuation.
/// One pass over the line.
fn may_change_env_tracking(line: &[u8]) -> bool {
    const NEEDLES: [&[u8]; 6] = [
        b"\"tool_use\"",
        b"\"server_tool_use\"",
        b"\"function_call\"",
        b"\"custom_tool_call\"",
        b"\"local_shell_call\"",
        b"\"PLANNER_RESPONSE\"",
    ];
    // A tracked call's output that says what it goes on as (`keep_continuations`).
    const CONTINUATIONS: [&[u8]; 2] = [b"Script running with cell ID", b"session_id"];
    line.iter().enumerate().any(|(at, byte)| {
        (*byte == b'"' && NEEDLES.iter().any(|needle| line[at..].starts_with(needle)))
            || CONTINUATIONS
                .iter()
                .any(|needle| line[at..].starts_with(needle))
    })
}

/// Restore the context at the saved offsets, then return to the new record just read.
fn replay_index_prefix(
    reader: &mut BufReader<File>,
    byte: u64,
    lines: u64,
    env_calls: &mut EnvCalls,
) -> io::Result<()> {
    let resume = reader.stream_position()?;
    reader.rewind()?;
    let mut prefix = reader.by_ref().take(byte);
    let mut buf = Vec::new();
    let invalid = || {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "inconsistent session index offsets",
        )
    };
    for line in 0..lines {
        match read_line(&mut prefix, &mut buf, MAX_LINE_BYTES)? {
            LineRead::Eof => return Err(invalid()),
            LineRead::TooLong(_) => {}
            LineRead::Line(_) => {
                if let Ok(record) = serde_json::from_slice(&buf) {
                    let _ = entry_of(line, &record, env_calls);
                }
            }
        }
    }
    if prefix.limit() != 0 {
        return Err(invalid());
    }
    reader.seek(SeekFrom::Start(resume))?;
    Ok(())
}

/// Passages read for the index from a byte offset.
#[derive(Debug, Default)]
pub struct IndexBatch {
    /// Passages.
    pub chunks: Vec<SessionChunk>,
    /// Byte offset after the last complete line read.
    pub end_byte: u64,
    /// Line number after the last complete line read.
    pub end_line: u64,
    /// Bytes re-read to reconstruct redaction context, charged to the refresh budget.
    pub replayed_bytes: u64,
    /// Characters of passage text.
    pub chars: usize,
    /// Whether the end of the transcript was reached.
    pub eof: bool,
}

/// Reads complete lines from byte `from_byte` (line number `from_line`) until `max_bytes` were
/// read or `max_chars` of passage text were produced. A resumed batch reconstructs its
/// redaction context from the indexed prefix before emitting passages.
///
/// # Errors
/// I/O failures.
pub fn read_for_index(
    mut file: File,
    from_byte: u64,
    from_line: u64,
    max_bytes: u64,
    max_chars: usize,
) -> io::Result<IndexBatch> {
    file.seek(SeekFrom::Start(from_byte))?;
    let mut reader = BufReader::with_capacity(256 * 1024, file);
    let mut buf = Vec::new();
    let mut env_calls = EnvCalls::default();
    let mut batch = IndexBatch {
        end_byte: from_byte,
        end_line: from_line,
        ..IndexBatch::default()
    };
    let mut read_bytes = 0_u64;
    let mut needs_replay = from_byte != 0 || from_line != 0;
    while read_bytes < max_bytes && batch.chars < max_chars {
        let next = read_line(&mut reader, &mut buf, MAX_LINE_BYTES)?;
        if needs_replay && next != LineRead::Eof {
            replay_index_prefix(&mut reader, from_byte, from_line, &mut env_calls)?;
            batch.replayed_bytes = from_byte;
            needs_replay = false;
        }
        let used = match next {
            LineRead::Eof => {
                batch.eof = true;
                return Ok(batch);
            }
            LineRead::TooLong(used) => {
                batch.end_line += 1;
                used
            }
            LineRead::Line(used) => {
                let line = batch.end_line;
                batch.end_line += 1;
                if let Ok(record) = serde_json::from_slice::<Value>(&buf)
                    && let Some(entry) = entry_of(line, &record, &mut env_calls)
                    && let Some(chunk) = chunk_of(&entry)
                {
                    batch.chars += chunk.text.len();
                    batch.chunks.push(chunk);
                }
                used
            }
        };
        let used = u64::try_from(used).unwrap_or(u64::MAX);
        batch.end_byte += used;
        read_bytes += used;
    }
    Ok(batch)
}

/// Plain-text rendering of entries for agents (`session_read`), at most `max_chars`.
/// Returns the text and the line to continue from when it was cut.
#[must_use]
pub fn render_text(entries: &[TranscriptEntry], max_chars: usize) -> (String, Option<u64>) {
    let mut out = String::new();
    for entry in entries {
        let mut block_text = String::new();
        let header = format!(
            "[line {}] {}{}{}\n",
            entry.line,
            entry.role,
            if entry.sidechain { " (subagent)" } else { "" },
            entry
                .at
                .as_deref()
                .map(|at| format!(" · {at}"))
                .unwrap_or_default()
        );
        for block in &entry.blocks {
            match block {
                TranscriptBlock::Text { text, truncated } => {
                    block_text.push_str(text);
                    if *truncated {
                        block_text.push_str(" …(cut)");
                    }
                    block_text.push('\n');
                }
                TranscriptBlock::Thinking { text, .. } => {
                    block_text.push_str(&format!("(thinking) {}\n", bounded(text, 2000)));
                }
                TranscriptBlock::ToolCall {
                    name,
                    summary,
                    diff,
                    ..
                } => {
                    block_text.push_str(&format!("→ {name} {summary}\n"));
                    if let Some(diff) = diff {
                        for hunk in &diff.hunks {
                            for old in hunk.old.lines().take(40) {
                                block_text.push_str(&format!("  - {old}\n"));
                            }
                            for new in hunk.new.lines().take(80) {
                                block_text.push_str(&format!("  + {new}\n"));
                            }
                        }
                        if diff.truncated {
                            block_text.push_str("  …(cut: the change is longer)\n");
                        }
                    }
                }
                TranscriptBlock::ToolResult { text, is_error, .. } => {
                    let label = if *is_error { "← error" } else { "← result" };
                    let short = bounded(text, 2000);
                    block_text.push_str(&format!("{label}: {short}\n"));
                }
                TranscriptBlock::Image { .. } => block_text.push_str("[image]\n"),
                TranscriptBlock::Meta { .. } => {}
            }
        }
        if block_text.is_empty() {
            continue;
        }
        if out.len() + header.len() + block_text.len() > max_chars && !out.is_empty() {
            return (out, Some(entry.line));
        }
        out.push_str(&header);
        out.push_str(&bounded(&block_text, max_chars));
        out.push('\n');
    }
    (out, None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    /// ADR-0315: a session's transcript is in its sandbox's home, which the agent writes. It
    /// is opened from the sandbox directory without following a link at its name or on the way,
    /// a FIFO in its place is refused at once, and a copy outside the sandbox is the server's.
    #[test]
    fn a_transcript_is_opened_without_following_links_or_waiting() -> TestResult {
        use std::os::unix::fs::symlink;
        let dir = tempfile::tempdir()?;
        let sandbox = dir.path().join("sandbox");
        let folder = sandbox.join("home/.claude/projects/slug");
        std::fs::create_dir_all(&folder)?;
        let real = folder.join("s1.jsonl");
        std::fs::write(&real, "{}\n")?;
        assert_eq!(read_page(open(&sandbox, &real)?, 0, 10, 1000)?.end, 1);

        let host = dir.path().join("host.jsonl");
        std::fs::write(&host, "{}\n")?;
        let refused = |path: &Path| open(&sandbox, path).map(|_| ()).map_err(|e| e.kind());
        // A link at the name.
        let link = folder.join("link.jsonl");
        symlink(&host, &link)?;
        assert_eq!(refused(&link), Err(io::ErrorKind::InvalidInput));
        // A FIFO nobody writes.
        let fifo = folder.join("fifo.jsonl");
        assert!(
            std::process::Command::new("mkfifo")
                .arg(&fifo)
                .status()?
                .success()
        );
        let started = std::time::Instant::now();
        assert_eq!(refused(&fifo), Err(io::ErrorKind::InvalidInput));
        assert!(started.elapsed() < std::time::Duration::from_secs(5));
        // A folder on the way that is a link out of the sandbox.
        let outside = dir.path().join("outside");
        std::fs::create_dir_all(&outside)?;
        std::fs::write(outside.join("s2.jsonl"), "{}\n")?;
        symlink(&outside, sandbox.join("home/.claude/projects/linked"))?;
        assert_eq!(
            refused(&sandbox.join("home/.claude/projects/linked/s2.jsonl")),
            Err(io::ErrorKind::InvalidInput)
        );
        // Missing, and the server's own copy outside the sandbox.
        assert_eq!(
            refused(&folder.join("gone.jsonl")),
            Err(io::ErrorKind::NotFound)
        );
        assert_eq!(read_page(open(&sandbox, &host)?, 0, 10, 1000)?.end, 1);
        Ok(())
    }

    fn fake_token() -> String {
        format!("ghp_{}", "aB3dE".repeat(8))
    }

    fn lines() -> Vec<Value> {
        vec![
            json!({"type": "file-history-snapshot", "snapshot": {}}),
            json!({"type": "user", "timestamp": "2026-10-01T10:00:00.000Z", "message": {"role": "user", "content": "Add a cart"}}),
            json!({"type": "assistant", "message": {"role": "assistant", "content": [
                {"type": "thinking", "thinking": "Plan the cart"},
                {"type": "text", "text": "I'll read the env first."},
                {"type": "tool_use", "id": "t1", "name": "Read", "input": {"file_path": "/repo/.env"}},
            ]}}),
            json!({"type": "user", "message": {"role": "user", "content": [
                {"type": "tool_result", "tool_use_id": "t1", "content": "     1→STRIPE_SECRET=sk_live_abc123\n     2→PORT=3000\n"}
            ]}}),
            json!({"type": "assistant", "message": {"role": "assistant", "content": [
                {"type": "tool_use", "id": "t2", "name": "Edit", "input": {"file_path": "src/cart.ts", "old_string": "a", "new_string": format!("const t = '{}';", fake_token())}},
                {"type": "tool_use", "id": "t3", "name": "Bash", "input": {"command": "npm test", "description": "Run tests"}},
            ]}}),
            json!({"type": "user", "isMeta": true, "message": {"role": "user", "content": "<command-name>/clear</command-name>"}}),
            json!({"type": "summary", "summary": "Cart added"}),
        ]
    }

    fn write_transcript(dir: &Path) -> Result<std::path::PathBuf, Box<dyn std::error::Error>> {
        let path = dir.join("s.jsonl");
        let mut text = String::new();
        for line in lines() {
            text.push_str(&serde_json::to_string(&line)?);
            text.push('\n');
        }
        text.push_str("{\"type\":\"user\",\"message\":{\"content\":\"unfinished");
        std::fs::write(&path, text)?;
        Ok(path)
    }

    #[test]
    fn lines_become_entries_with_tool_calls_diffs_and_redactions() -> TestResult {
        let dir = tempfile::tempdir()?;
        let path = write_transcript(dir.path())?;
        let page = read_page(File::open(&path)?, 0, 100, MAX_PAGE_CHARS)?;
        assert_eq!(page.next, None, "an unfinished last line is not returned");
        let roles: Vec<(u64, &str)> = page
            .entries
            .iter()
            .map(|e| (e.line, e.role.as_str()))
            .collect();
        assert_eq!(
            roles,
            vec![
                (1, "user"),
                (2, "assistant"),
                (3, "user"),
                (4, "assistant"),
                (5, "user"),
                (6, "summary")
            ]
        );
        let TranscriptBlock::ToolResult { text, .. } = &page.entries[2].blocks[0] else {
            return Err("tool result".into());
        };
        assert!(
            text.contains("STRIPE_SECRET=[REDACTED:") && text.contains("PORT=3000"),
            "a dotenv file read by a tool is redacted: {text}"
        );
        let TranscriptBlock::ToolCall {
            name,
            summary,
            diff: Some(diff),
            ..
        } = &page.entries[3].blocks[0]
        else {
            return Err("edit".into());
        };
        assert_eq!((name.as_str(), summary.as_str()), ("Edit", "src/cart.ts"));
        assert!(diff.hunks[0].new.contains("[REDACTED:github-token]"));
        assert!(!diff.hunks[0].new.contains(&fake_token()));
        assert!(matches!(
            page.entries[4].blocks[0],
            TranscriptBlock::Meta { .. }
        ));
        let TranscriptBlock::ToolCall { summary, .. } = &page.entries[3].blocks[1] else {
            return Err("bash".into());
        };
        assert_eq!(summary, "npm test");
        Ok(())
    }

    #[test]
    fn pages_continue_where_they_stopped_and_keep_env_tracking() -> TestResult {
        let dir = tempfile::tempdir()?;
        let path = write_transcript(dir.path())?;
        let first = read_page(File::open(&path)?, 0, 2, MAX_PAGE_CHARS)?;
        assert_eq!(first.entries.len(), 2);
        assert_eq!(first.next, Some(3));
        let second = read_page(File::open(&path)?, 3, 1, MAX_PAGE_CHARS)?;
        let TranscriptBlock::ToolResult { text, .. } = &second.entries[0].blocks[0] else {
            return Err("tool result".into());
        };
        assert!(text.contains("[REDACTED:"), "{text}");
        Ok(())
    }

    #[test]
    fn a_page_ends_before_a_line_still_being_written_and_reads_it_once_complete() -> TestResult {
        let dir = tempfile::tempdir()?;
        let path = write_transcript(dir.path())?;
        let page = read_page(File::open(&path)?, 0, 100, MAX_PAGE_CHARS)?;
        assert_eq!(page.end, 7, "the unfinished line is left for later");
        let full = read_page(File::open(&path)?, 0, 2, MAX_PAGE_CHARS)?;
        assert_eq!(full.end, 3, "a full page ends where the next one starts");
        let mut text = std::fs::read_to_string(&path)?;
        text.push_str(" cart\"}}\n");
        std::fs::write(&path, text)?;
        let rest = read_page(File::open(&path)?, page.end, 100, MAX_PAGE_CHARS)?;
        assert_eq!(
            rest.entries.iter().map(|e| e.line).collect::<Vec<_>>(),
            vec![7]
        );
        assert_eq!((rest.end, rest.next), (8, None));
        Ok(())
    }

    /// SPEC-483 slice 36 (QB5): a read that continues from a resume point (the byte offset of a
    /// line and the dotenv calls seen before it) gives what a read from the start gives, the
    /// dotenv read before the point still redacted; a point past the asked line, or one that is
    /// not at a line's start any more, is not used.
    #[test]
    fn a_read_from_a_resume_point_gives_what_a_read_from_the_start_gives() -> TestResult {
        let dir = tempfile::tempdir()?;
        let path = write_transcript(dir.path())?;
        let shown = |page: &Page| format!("{:?}", page.entries);
        let whole = read_page(File::open(&path)?, 3, 100, MAX_PAGE_CHARS)?;
        let (head, point) = read_page_at(File::open(&path)?, None, 0, 2, MAX_PAGE_CHARS)?;
        assert_eq!((head.end, point.line), (3, 3));
        assert!(point.byte > 0);
        let (resumed, after) =
            read_page_at(File::open(&path)?, Some(&point), 3, 100, MAX_PAGE_CHARS)?;
        assert_eq!(shown(&resumed), shown(&whole));
        assert_eq!((resumed.end, resumed.next), (whole.end, whole.next));
        assert_eq!(after.line, 7, "the unfinished line is left for later");
        let TranscriptBlock::ToolResult { text, .. } = &resumed.entries[0].blocks[0] else {
            return Err("tool result".into());
        };
        assert!(
            text.contains("[REDACTED:"),
            "the .env read before the point: {text}"
        );
        // A point past the asked line, or off a line's start, is read past: the same answer.
        let (early, _) = read_page_at(File::open(&path)?, Some(&after), 3, 100, MAX_PAGE_CHARS)?;
        assert_eq!(shown(&early), shown(&whole));
        let off = ReadCursor {
            byte: point.byte - 1,
            ..point.clone()
        };
        let (shifted, _) = read_page_at(File::open(&path)?, Some(&off), 3, 100, MAX_PAGE_CHARS)?;
        assert_eq!(shown(&shifted), shown(&whole));
        // A value only the dotenv rule hides (a secret's name, a plain value): the point keeps
        // the dotenv read made before it, at a page's end and at the file's end alike.
        let dotenv = dir.path().join("dotenv.jsonl");
        let mut text = String::new();
        for line in [
            json!({"type": "assistant", "message": {"role": "assistant", "content": [
                {"type": "tool_use", "id": "e1", "name": "Read", "input": {"file_path": "/repo/.env"}}]}}),
            json!({"type": "user", "message": {"role": "user", "content": "Go on"}}),
        ] {
            text.push_str(&serde_json::to_string(&line)?);
            text.push('\n');
        }
        std::fs::write(&dotenv, &text)?;
        let (_, at_end) = read_page_at(File::open(&dotenv)?, None, 0, 100, MAX_PAGE_CHARS)?;
        let (_, at_page) = read_page_at(File::open(&dotenv)?, None, 0, 1, MAX_PAGE_CHARS)?;
        text.push_str(&serde_json::to_string(
            &json!({"type": "user", "message": {"role": "user", "content": [
            {"type": "tool_result", "tool_use_id": "e1", "content": "DB_PASSWORD=plainword\n"}]}}),
        )?);
        text.push('\n');
        std::fs::write(&dotenv, &text)?;
        for point in [&at_end, &at_page] {
            let (page, _) =
                read_page_at(File::open(&dotenv)?, Some(point), 2, 100, MAX_PAGE_CHARS)?;
            let shown = shown(&page);
            assert!(
                shown.contains("DB_PASSWORD") && !shown.contains("plainword"),
                "{shown}"
            );
        }
        // Replaced by another file (a step-back renames a rewritten copy over it) whose dotenv
        // read ends exactly where the old point is, its result after it: the old point is of the
        // other file and is not used, so the result is redacted as a read from the start does.
        let replaced = dir.path().join("dotenv.new");
        let call = |pad: usize| {
            serde_json::to_string(
                &json!({"type": "assistant", "message": {"role": "assistant", "content": [
                {"type": "tool_use", "id": "e2", "name": "Read",
                 "input": {"file_path": "/repo/.env", "pad": "x".repeat(pad)}}]}}),
            )
        };
        let short = call(0)?.len() + 1;
        let target = usize::try_from(at_end.byte)?;
        let mut other = call(target.saturating_sub(short))?;
        other.push('\n');
        assert_eq!(other.len(), target, "the dotenv read ends at the old point");
        other.push_str(&serde_json::to_string(
            &json!({"type": "user", "message": {"role": "user", "content": [
            {"type": "tool_result", "tool_use_id": "e2", "content": "DB_PASSWORD=plainword\n"}]}}),
        )?);
        other.push('\n');
        std::fs::write(&replaced, &other)?;
        std::fs::rename(&replaced, &dotenv)?;
        let from_start = read_page(File::open(&dotenv)?, 2, 100, MAX_PAGE_CHARS)?;
        let (on, _) = read_page_at(File::open(&dotenv)?, Some(&at_end), 2, 100, MAX_PAGE_CHARS)?;
        assert_eq!(shown(&on), shown(&from_start));
        assert!(!shown(&on).contains("plainword"), "{}", shown(&on));
        Ok(())
    }

    /// SPEC-483 slice 36 (review): two watchers of a transcript at different lines keep the
    /// farther point; a point of another file replaces any.
    #[test]
    fn the_farther_point_of_a_file_is_kept() -> TestResult {
        let dir = tempfile::tempdir()?;
        let path = write_transcript(dir.path())?;
        let (_, early) = read_page_at(File::open(&path)?, None, 0, 2, MAX_PAGE_CHARS)?;
        let (_, far) = read_page_at(File::open(&path)?, None, 0, 100, MAX_PAGE_CHARS)?;
        assert!(early.line < far.line);
        let len = far.byte;
        assert!(far.replaces(&early, len) && !early.replaces(&far, len) && far.replaces(&far, len));
        // A file shortened in place (same inode) below the kept point: the kept point goes.
        assert!(early.replaces(&far, far.byte - 1));
        let copy = dir.path().join("copy.jsonl");
        std::fs::copy(&path, &copy)?;
        let (_, other) = read_page_at(File::open(&copy)?, None, 0, 2, MAX_PAGE_CHARS)?;
        assert!(
            other.replaces(&far, len),
            "another file's point replaces a farther one"
        );
        Ok(())
    }

    /// SPEC-483 slice 36 (review): a line past the bound is skipped and its bytes counted, so
    /// the point after it is at the next line's start.
    #[test]
    fn a_point_after_an_overlong_line_is_at_the_next_line() -> TestResult {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("long.jsonl");
        let mut text = format!("{{\"pad\":\"{}\"}}\n", "x".repeat(MAX_LINE_BYTES));
        text.push_str(&serde_json::to_string(
            &json!({"type": "user", "message": {"role": "user", "content": "After"}}),
        )?);
        text.push('\n');
        std::fs::write(&path, &text)?;
        let (page, point) = read_page_at(File::open(&path)?, None, 0, 100, MAX_PAGE_CHARS)?;
        assert_eq!((page.skipped, page.entries.len(), point.line), (1, 1, 2));
        assert_eq!(point.byte, u64::try_from(text.len())?);
        let (head, early) = read_page_at(File::open(&path)?, None, 0, 100, 1)?;
        assert_eq!((head.skipped, early.line), (1, 2));
        let mut more = std::fs::OpenOptions::new().append(true).open(&path)?;
        std::io::Write::write_all(
            &mut more,
            b"{\"type\":\"user\",\"message\":{\"role\":\"user\",\"content\":\"Next\"}}\n",
        )?;
        let (next, _) = read_page_at(File::open(&path)?, Some(&point), 2, 100, MAX_PAGE_CHARS)?;
        assert_eq!(
            next.entries.iter().map(|e| e.line).collect::<Vec<_>>(),
            vec![2]
        );
        Ok(())
    }

    #[test]
    fn the_tail_holds_the_last_entries_with_env_tracking_from_the_start() -> TestResult {
        let dir = tempfile::tempdir()?;
        let path = write_transcript(dir.path())?;
        let tail = read_tail(File::open(&path)?, 4, MAX_PAGE_CHARS)?;
        let lines: Vec<u64> = tail.entries.iter().map(|e| e.line).collect();
        assert_eq!(lines, vec![3, 4, 5, 6]);
        assert!(tail.earlier, "lines 1 and 2 were left out");
        assert_eq!((tail.end, tail.next), (7, None));
        let TranscriptBlock::ToolResult { text, .. } = &tail.entries[0].blocks[0] else {
            return Err("tool result".into());
        };
        assert!(
            text.contains("[REDACTED:"),
            "the .env read before the tail: {text}"
        );
        let all = read_tail(File::open(&path)?, 100, MAX_PAGE_CHARS)?;
        assert_eq!(all.entries.len(), 6);
        assert!(!all.earlier);
        let bounded = read_tail(File::open(&path)?, 100, 1)?;
        assert_eq!(
            bounded.entries.len(),
            1,
            "the text bound keeps the last entry"
        );
        assert!(bounded.earlier);
        Ok(())
    }

    #[test]
    fn index_batches_resume_at_byte_offsets() -> TestResult {
        let dir = tempfile::tempdir()?;
        let path = write_transcript(dir.path())?;
        let first = read_for_index(File::open(&path)?, 0, 0, 1, usize::MAX)?;
        assert_eq!(first.end_line, 1, "one line per budget step");
        assert!(first.chunks.is_empty(), "the snapshot line has no passage");
        let rest = read_for_index(
            File::open(&path)?,
            first.end_byte,
            first.end_line,
            u64::MAX,
            usize::MAX,
        )?;
        assert!(rest.eof);
        assert_eq!(rest.end_line, 7, "the unfinished line is left for later");
        let roles: Vec<&str> = rest.chunks.iter().map(|c| c.role.as_str()).collect();
        assert_eq!(roles, vec!["user", "assistant", "tool", "tool", "summary"]);
        assert!(rest.chunks[3].text.contains("Edit src/cart.ts"));
        assert!(
            rest.chunks
                .iter()
                .all(|c| !c.text.contains("sk_live_abc123"))
        );
        Ok(())
    }

    #[test]
    fn resumed_index_redacts_dotenv_results_across_a_restart() -> TestResult {
        use std::io::Write as _;
        let dir = tempfile::tempdir()?;
        let call = json!({"type": "assistant", "message": {"content": [
            {"type": "tool_use", "id": "r1", "name": "Read", "input": {"file_path": ".env"}}
        ]}});
        let path = write_lines(dir.path(), &[call])?;
        let first = read_for_index(File::open(&path)?, 0, 0, u64::MAX, usize::MAX)?;
        assert_eq!(first.end_line, 1);
        let result = json!({"type": "user", "message": {"content": [
            {"type": "tool_result", "tool_use_id": "r1", "content": ENV_TWO},
            {"type": "tool_result", "tool_use_id": "other", "content": "MODE=ordinary"}
        ]}});
        let mut growing = std::fs::OpenOptions::new().append(true).open(&path)?;
        write!(growing, "{result}")?;
        let resume = || {
            read_for_index(
                File::open(&path)?,
                first.end_byte,
                first.end_line,
                u64::MAX,
                usize::MAX,
            )
        };
        let partial = resume()?;
        assert_eq!((partial.end_byte, partial.end_line), (first.end_byte, 1));
        assert!(partial.chunks.is_empty());
        assert_eq!(partial.replayed_bytes, 0);
        writeln!(growing)?;
        let second = resume()?;
        assert_eq!(second.replayed_bytes, first.end_byte);
        assert_eq!(second.chunks.len(), 1);
        let passage = &second.chunks[0];
        assert_eq!(passage.line, 1);
        assert!(passage.text.contains("SHOP_REGION=[REDACTED:"));
        assert!(passage.text.contains("MAIL_DOMAIN=[REDACTED:"));
        assert!(!passage.text.contains("eu-north-7b") && !passage.text.contains("mg.shop.example"));
        assert!(passage.text.contains("MODE=ordinary"));
        Ok(())
    }

    #[test]
    fn a_resumed_index_refuses_offsets_that_disagree_with_the_transcript() -> TestResult {
        let dir = tempfile::tempdir()?;
        let path = write_transcript(dir.path())?;
        let first = read_for_index(File::open(&path)?, 0, 0, 1, usize::MAX)?;
        for (byte, line) in [
            (0, first.end_line),
            (first.end_byte - 1, first.end_line),
            (first.end_byte, first.end_line - 1),
            (first.end_byte, first.end_line + 1),
        ] {
            let result = read_for_index(File::open(&path)?, byte, line, u64::MAX, usize::MAX);
            assert_eq!(
                result.err().map(|err| err.kind()),
                Some(io::ErrorKind::InvalidData)
            );
        }
        Ok(())
    }

    #[test]
    fn an_oversized_complete_suffix_validates_the_saved_index_offsets() -> TestResult {
        use std::io::Write as _;
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("rewritten.jsonl");
        let mut file = File::create(&path)?;
        file.seek(SeekFrom::Start(u64::try_from(MAX_LINE_BYTES)? + 2))?;
        writeln!(file)?;
        // The old line ended at byte 1. The rewritten file has no newline in that prefix.
        let result = read_for_index(File::open(path)?, 1, 1, u64::MAX, usize::MAX);
        assert_eq!(
            result.err().map(|err| err.kind()),
            Some(io::ErrorKind::InvalidData)
        );
        Ok(())
    }

    #[test]
    fn rendering_for_agents_is_bounded() -> TestResult {
        let dir = tempfile::tempdir()?;
        let path = write_transcript(dir.path())?;
        let page = read_page(File::open(&path)?, 0, 100, MAX_PAGE_CHARS)?;
        let (text, next) = render_text(&page.entries, 100_000);
        assert!(text.contains("[line 1] user · 2026-10-01T10:00:00.000Z\nAdd a cart"));
        assert!(text.contains("→ Read /repo/.env"));
        assert!(text.contains("  + const t = '[REDACTED:github-token]';"));
        assert_eq!(next, None);
        let (short, next) = render_text(&page.entries, 120);
        assert!(short.len() < 400);
        assert!(next.is_some());
        Ok(())
    }

    fn diff_in(line: &Value) -> Result<TranscriptDiff, Box<dyn std::error::Error>> {
        let entry = entry_of(0, line, &mut EnvCalls::default()).ok_or("no entry")?;
        match entry.blocks.into_iter().next() {
            Some(TranscriptBlock::ToolCall {
                diff: Some(diff), ..
            }) => Ok(diff),
            other => Err(format!("not an edit: {other:?}").into()),
        }
    }

    fn call(name: &str, input: Value) -> Value {
        json!({"type": "assistant", "message": {"role": "assistant", "content": [
            {"type": "tool_use", "id": "t1", "name": name, "input": input},
        ]}})
    }

    // #350: a diff the server cut says so, so the page claims no count.
    #[test]
    fn a_diff_cut_at_its_bounds_is_marked_truncated() -> TestResult {
        let small = diff_in(&call(
            "Edit",
            json!({"file_path": "a.rs", "old_string": "a", "new_string": "b"}),
        ))?;
        assert!(!small.truncated, "a small edit is whole");
        let at_bound = diff_in(&call(
            "Write",
            json!({"file_path": "a.rs", "content": "x".repeat(MAX_HUNK_CHARS)}),
        ))?;
        assert!(!at_bound.truncated, "a side at the bound is whole");
        let long = diff_in(&call(
            "Write",
            json!({"file_path": "a.rs", "content": "x".repeat(MAX_HUNK_CHARS + 1)}),
        ))?;
        assert!(long.truncated, "a side over the bound is cut");
        assert_eq!(long.hunks[0].new.chars().count(), MAX_HUNK_CHARS);
        let old_side = diff_in(&call(
            "Edit",
            json!({"file_path": "a.rs", "old_string": "y".repeat(MAX_HUNK_CHARS + 1), "new_string": ""}),
        ))?;
        assert!(old_side.truncated, "the removed side counts too");
        let edits = |n: usize| {
            (0..n)
                .map(|i| json!({"old_string": format!("a{i}"), "new_string": format!("b{i}")}))
                .collect::<Vec<_>>()
        };
        let all = diff_in(&call(
            "MultiEdit",
            json!({"file_path": "a.rs", "edits": edits(MAX_HUNKS)}),
        ))?;
        assert!(!all.truncated, "{MAX_HUNKS} hunks are all kept");
        let dropped = diff_in(&call(
            "MultiEdit",
            json!({"file_path": "a.rs", "edits": edits(MAX_HUNKS + 1)}),
        ))?;
        assert!(dropped.truncated, "hunks past {MAX_HUNKS} are dropped");
        assert_eq!(dropped.hunks.len(), MAX_HUNKS);
        // Agents reading the session are told too; the field is additive on the wire.
        let entry = entry_of(
            0,
            &call(
                "Write",
                json!({"file_path": "a.rs", "content": "x".repeat(MAX_HUNK_CHARS + 1)}),
            ),
            &mut EnvCalls::default(),
        )
        .ok_or("no entry")?;
        let (text, _) = render_text(&[entry], 100_000);
        assert!(
            text.ends_with("  …(cut: the change is longer)\n\n"),
            "{text}"
        );
        let old: TranscriptDiff = serde_json::from_value(json!({"path": "a.rs", "hunks": []}))?;
        assert!(!old.truncated, "a diff without the field is whole");
        Ok(())
    }

    // #380: a meta text the server cut says so, like text and tool results.
    #[test]
    fn a_meta_text_cut_at_its_bound_is_marked_truncated() -> TestResult {
        let meta = |line: &Value| -> Result<(String, bool), Box<dyn std::error::Error>> {
            let entry = entry_of(0, line, &mut EnvCalls::default()).ok_or("no entry")?;
            match entry.blocks.into_iter().next() {
                Some(TranscriptBlock::Meta { text, truncated }) => Ok((text, truncated)),
                other => Err(format!("not meta: {other:?}").into()),
            }
        };
        let user = |text: String| json!({"type": "user", "isMeta": true, "message": {"role": "user", "content": text}});
        let system = |text: String| json!({"type": "system", "content": text});
        assert_eq!(meta(&user("short".into()))?, ("short".to_owned(), false));
        let (text, cut_off) = meta(&user("s".repeat(MAX_RESULT_CHARS)))?;
        assert!(
            !cut_off && text.len() == MAX_RESULT_CHARS,
            "at the bound it is whole"
        );
        let (text, cut_off) = meta(&user("s".repeat(MAX_RESULT_CHARS + 1)))?;
        assert!(
            cut_off && text.len() == MAX_RESULT_CHARS,
            "over the bound it is cut"
        );
        let (_, cut_off) = meta(&system("s".repeat(MAX_TEXT_CHARS)))?;
        assert!(!cut_off, "a system line at its bound is whole");
        let (text, cut_off) = meta(&system("s".repeat(MAX_TEXT_CHARS + 1)))?;
        assert!(
            cut_off && text.len() == MAX_TEXT_CHARS,
            "a system line over it is cut"
        );
        // A summary line is cut at the message bound and says so too (review round 2).
        let summary = |text: String| -> Result<bool, Box<dyn std::error::Error>> {
            let line = json!({"type": "summary", "summary": text});
            match entry_of(0, &line, &mut EnvCalls::default())
                .and_then(|entry| entry.blocks.into_iter().next())
            {
                Some(TranscriptBlock::Text { truncated, .. }) => Ok(truncated),
                other => Err(format!("not text: {other:?}").into()),
            }
        };
        assert!(!summary("s".repeat(MAX_TEXT_CHARS))?);
        assert!(summary("s".repeat(MAX_TEXT_CHARS + 1))?);
        let old: TranscriptBlock = serde_json::from_value(json!({"type": "meta", "text": "x"}))?;
        assert_eq!(
            old,
            TranscriptBlock::Meta {
                text: "x".into(),
                truncated: false
            }
        );
        Ok(())
    }

    #[test]
    fn long_lines_are_skipped_and_counted() -> TestResult {
        let mut reader = io::Cursor::new(b"aaaa\nbb\n".to_vec());
        let mut buf = Vec::new();
        assert_eq!(read_line(&mut reader, &mut buf, 3)?, LineRead::TooLong(5));
        assert_eq!(read_line(&mut reader, &mut buf, 3)?, LineRead::Line(3));
        assert_eq!(buf, b"bb");
        assert_eq!(read_line(&mut reader, &mut buf, 3)?, LineRead::Eof);
        Ok(())
    }

    /// A harness fixture of `yulius-harness` (SPEC-525): the formats as the real CLIs wrote them.
    fn fixture(name: &str) -> Result<std::path::PathBuf, Box<dyn std::error::Error>> {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../yulius-harness/tests/fixtures")
            .join(name);
        if !path.is_file() {
            return Err(format!("missing fixture {}", path.display()).into());
        }
        Ok(path)
    }

    /// An entry as one line per block: `role kind text`, the text cut to 40 characters.
    fn shape(entries: &[TranscriptEntry]) -> Vec<String> {
        let short = |text: &str| {
            text.chars()
                .take(40)
                .collect::<String>()
                .trim_end()
                .to_owned()
        };
        entries
            .iter()
            .flat_map(|entry| {
                entry.blocks.iter().map(|block| {
                    let what = match block {
                        TranscriptBlock::Text { text, .. } => format!("text {}", short(text)),
                        TranscriptBlock::Thinking { text, .. } => {
                            format!("thinking {}", short(text))
                        }
                        TranscriptBlock::ToolCall {
                            id, name, summary, ..
                        } => {
                            format!("call {id} {name}: {}", short(summary))
                        }
                        TranscriptBlock::ToolResult {
                            tool_use_id,
                            text,
                            is_error,
                            ..
                        } => format!(
                            "{} {tool_use_id} {}",
                            if *is_error { "error" } else { "result" },
                            short(text)
                        ),
                        TranscriptBlock::Image { .. } => "image".to_owned(),
                        TranscriptBlock::Meta { text, .. } => format!("meta {}", short(text)),
                    };
                    format!("{} {what}", entry.role)
                })
            })
            .collect()
    }

    fn write_lines(
        dir: &Path,
        lines: &[Value],
    ) -> Result<std::path::PathBuf, Box<dyn std::error::Error>> {
        let path = dir.join("t.jsonl");
        let mut text = String::new();
        for line in lines {
            text.push_str(&serde_json::to_string(line)?);
            text.push('\n');
        }
        std::fs::write(&path, text)?;
        Ok(path)
    }

    /// SPEC-525 WCX-001: a Codex rollout reads as its prompts, answers, reasoning, tool calls and
    /// results (it read as nothing, so a watcher saw "Codex is starting…" for the whole run).
    /// Codex's own instructions are left out; context it added is shown quietly.
    #[test]
    fn a_codex_rollout_reads_as_prompts_answers_and_tool_calls() -> TestResult {
        let tail = read_tail(
            File::open(fixture("codex/rollout.jsonl")?)?,
            100,
            MAX_PAGE_CHARS,
        )?;
        assert_eq!(
            shape(&tail.entries),
            vec![
                "user meta <environment_context>\n  <cwd>/work/demo<",
                "user text Add a greeting function and test it.",
                "assistant call call_01 exec_command: cargo test",
                "user result call_01 test result: ok. 3 passed",
                "assistant text Tests pass. Added `greet`.",
                "user text Now make it greet in Polish: cześć.",
                "assistant call call_02 apply_patch: src/lib.rs",
                "user result call_02 Success. Updated the following files:\nM",
                "assistant text Done: `greet` now returns \"cześć\".",
                "summary text Synthetic compaction summary.",
            ]
        );
        let at = tail.entries[1].at.as_deref();
        assert_eq!(at, Some("2026-11-09T10:00:03.000Z"));
        let TranscriptBlock::ToolCall { input, .. } = &tail.entries[6].blocks[0] else {
            return Err("the patch".into());
        };
        assert!(
            input.starts_with("*** Begin Patch\n"),
            "a patch reads as written: {input}"
        );

        // Codex 0.169 marks what it added by provenance: its continuation after a blocking Stop
        // hook is shown quietly, the person's prompts as prompts.
        let blocked = read_tail(
            File::open(fixture("codex/exec-0.169.0-stop-block.jsonl")?)?,
            100,
            MAX_PAGE_CHARS,
        )?;
        let shapes = shape(&blocked.entries);
        assert_eq!(shapes[1], "user text Say hi");
        assert_eq!(shapes[2], "assistant text Synthetic answer 1.");
        assert!(
            shapes[3].starts_with("user meta <hook_prompt"),
            "{shapes:?}"
        );
        assert_eq!(shapes[4], "assistant text Synthetic answer 2.");
        assert_eq!(shapes.len(), 5, "{shapes:?}");

        // A reasoning summary is the model's visible thinking; a shell call reads its command.
        let dir = tempfile::tempdir()?;
        let path = write_lines(
            dir.path(),
            &[
                json!({"type": "response_item", "payload": {"type": "reasoning", "summary": [{"type": "summary_text", "text": "**Plan** the greeting"}], "encrypted_content": "eA=="}}),
                json!({"type": "response_item", "payload": {"type": "function_call", "name": "shell", "arguments": "{\"command\":[\"bash\",\"-lc\",\"ls src\"]}", "call_id": "c9"}}),
                json!({"type": "response_item", "payload": {"type": "function_call_output", "call_id": "c9", "output": {"content": "lib.rs", "success": true}}}),
            ],
        )?;
        let page = read_page(File::open(&path)?, 0, 100, MAX_PAGE_CHARS)?;
        assert_eq!(
            shape(&page.entries),
            vec![
                "assistant thinking **Plan** the greeting",
                "assistant call c9 shell: bash -lc ls src",
                "user result c9 lib.rs",
            ]
        );
        Ok(())
    }

    /// SPEC-525 WCX-002: an Antigravity transcript reads as the person's requests (without the
    /// CLI's wrapper), the model's answers and tool calls, its tool steps and errors.
    #[test]
    fn an_antigravity_transcript_reads_as_requests_answers_and_tool_steps() -> TestResult {
        let quota = read_tail(
            File::open(fixture("antigravity/quota-1.5.10.jsonl")?)?,
            100,
            MAX_PAGE_CHARS,
        )?;
        let shapes = shape(&quota.entries);
        assert_eq!(
            shapes[..4],
            [
                "user text Say hello",
                "assistant text Hello from the stub.",
                "user text Run the probe command",
                "system meta The following is a <SYSTEM_MESSAGE> not",
            ]
        );
        assert_eq!(
            shapes[7],
            "user error step-7 API error (attempt 1): Error 429, Messag"
        );
        assert_eq!(quota.entries[0].at.as_deref(), Some("2026-11-17T17:09:20Z"));

        let tool = read_tail(
            File::open(fixture("antigravity/tool-1.5.10.jsonl")?)?,
            100,
            MAX_PAGE_CHARS,
        )?;
        assert_eq!(
            shape(&tool.entries),
            vec![
                "user error step-5 invalid arguments:\n- missing property 't",
                "assistant text Hello from the stub.",
                "user text run a command",
                "system meta The following is a <SYSTEM_MESSAGE> not",
                "assistant call step-9-0 run_command: echo probe-ran",
                "user error step-10 tool call denied by pre-tool hook:",
                "assistant text Hello from the stub.",
            ]
        );
        Ok(())
    }

    /// SPEC-525 WCX-003: what a Codex or Antigravity command printed from a dotenv file is
    /// redacted as dotenv content, also on a page or a tail that starts after the call.
    #[test]
    fn a_dotenv_read_by_codex_or_antigravity_is_redacted_on_every_page() -> TestResult {
        let dir = tempfile::tempdir()?;
        let codex = write_lines(
            dir.path(),
            &[
                json!({"type": "response_item", "payload": {"type": "function_call", "name": "exec_command", "arguments": "{\"cmd\":\"cat .env\"}", "call_id": "e1"}}),
                json!({"type": "response_item", "payload": {"type": "function_call_output", "call_id": "e1", "output": "SHOP_PASSPHRASE=plainvalue42\nPORT=3000"}}),
                json!({"type": "response_item", "payload": {"type": "function_call", "name": "exec_command", "arguments": "{\"cmd\":\"echo SHOP_PASSPHRASE=plainvalue42\"}", "call_id": "e2"}}),
                json!({"type": "response_item", "payload": {"type": "function_call_output", "call_id": "e2", "output": "done"}}),
            ],
        )?;
        for page in [
            read_page(File::open(&codex)?, 0, 100, MAX_PAGE_CHARS)?,
            read_page(File::open(&codex)?, 1, 1, MAX_PAGE_CHARS)?,
            read_tail(File::open(&codex)?, 3, MAX_PAGE_CHARS)?,
        ] {
            let result = page
                .entries
                .iter()
                .find(|e| e.line == 1)
                .ok_or("the output")?;
            let TranscriptBlock::ToolResult { text, .. } = &result.blocks[0] else {
                return Err("tool result".into());
            };
            assert!(
                text.contains("SHOP_PASSPHRASE=[REDACTED:") && text.contains("PORT=3000"),
                "{text}"
            );
        }

        let agy = dir.path().join("agy");
        std::fs::create_dir_all(&agy)?;
        let agy = write_lines(
            &agy,
            &[
                json!({"step_index": 0, "source": "MODEL", "type": "PLANNER_RESPONSE", "status": "DONE", "tool_calls": [{"name": "run_command", "args": {"CommandLine": "\"cat .env\""}}]}),
                json!({"step_index": 1, "source": "MODEL", "type": "RUN_COMMAND", "status": "DONE", "content": "SHOP_PASSPHRASE=plainvalue42"}),
                json!({"step_index": 2, "source": "MODEL", "type": "PLANNER_RESPONSE", "status": "DONE", "content": "Read it."}),
                json!({"step_index": 3, "source": "MODEL", "type": "RUN_COMMAND", "status": "DONE", "content": "MODE=plain"}),
            ],
        )?;
        for page in [
            read_page(File::open(&agy)?, 0, 100, MAX_PAGE_CHARS)?,
            read_page(File::open(&agy)?, 1, 100, MAX_PAGE_CHARS)?,
            read_tail(File::open(&agy)?, 3, MAX_PAGE_CHARS)?,
        ] {
            let texts = shape(&page.entries);
            assert!(
                texts
                    .iter()
                    .any(|t| t.starts_with("user result step-1 SHOP_PASSPHRASE=[REDACTED:")),
                "{texts:?}"
            );
            assert!(
                texts.iter().any(|t| t == "user result step-3 MODE=plain"),
                "a later step's output is not dotenv content: {texts:?}"
            );
        }
        Ok(())
    }

    #[test]
    fn a_cold_page_skips_plain_text_before_the_requested_line() {
        let ordinary = br#"{"type":"user","message":{"content":"build finished"}}"#;
        assert!(!may_change_env_tracking(ordinary));
    }

    /// SPEC-536: the launch and each retrieval have different tool-use IDs. The shell
    /// association must survive a cursor between them, and a cold scan up to the results.
    #[test]
    fn claude_background_shell_output_is_redacted_on_every_read() -> TestResult {
        let call = |id: &str, name: &str, input: Value| json!({"type":"assistant","message":{"content":[{"type":"tool_use","id":id,"name":name,"input":input}]}});
        let result = |id: &str, content: &str| json!({"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":id,"content":content}]}});
        for launch in [
            result(
                "dotenv",
                "partial output\nCommand running in background with ID: shell_7",
            ),
            result(
                "dotenv",
                "Command running in background with ID: shell_7. Output is being written to: /tmp/shell_7.output",
            ),
            result(
                "dotenv",
                "Command was moved to the background (ID: shell_7) after running for 30s. Output is being written to: /tmp/shell_7.output",
            ),
            result(
                "dotenv",
                "Command was manually backgrounded by user with ID: shell_7. Output is being written to: /tmp/shell_7.output.\nYou will be notified when it completes.",
            ),
            {
                let mut record = result("dotenv", "Command moved to the background");
                record["message"]["content"]
                    .as_array_mut()
                    .ok_or("expected blocks")?
                    .extend([
                        json!({"type":"text","text":"launch completed"}),
                        json!({"type":"text","text":"shell is running"}),
                    ]);
                record["toolUseResult"] = json!({"backgroundTaskId":"shell_7"});
                record
            },
        ] {
            let dir = tempfile::tempdir()?;
            let path = write_lines(
                dir.path(),
                &[
                    call(
                        "dotenv",
                        "Bash",
                        json!({"command":"cat .env","run_in_background":true}),
                    ),
                    call(
                        "ordinary",
                        "Bash",
                        json!({"command":"pwd","run_in_background":true}),
                    ),
                    result(
                        "ordinary",
                        "Command running in background with ID: shell_70",
                    ),
                    launch,
                    call("old", "BashOutput", json!({"bash_id":"shell_7"})),
                    call("other", "TaskOutput", json!({"task_id":"shell_70"})),
                    {
                        let mut retrieval = call("new", "TaskOutput", json!({"task_id":"shell_7"}));
                        retrieval["message"]["content"][0]["type"] = json!("server_tool_use");
                        retrieval
                    },
                    result("old", &format!("<stdout>\n{ENV_TWO}\n</stdout>")),
                    result("other", "SHOP_REGION=public-region"),
                    result("new", &format!("<output>\n{ENV_TWO}\n</output>")),
                ],
            )?;
            let (_, kept) = read_page_at(File::open(&path)?, None, 0, 4, MAX_PAGE_CHARS)?;
            assert_eq!(kept.line, 4);
            for page in [
                read_page(File::open(&path)?, 0, 100, MAX_PAGE_CHARS)?,
                read_page(File::open(&path)?, 7, 100, MAX_PAGE_CHARS)?,
                read_page_at(File::open(&path)?, Some(&kept), 7, 100, MAX_PAGE_CHARS)?.0,
                read_tail(File::open(&path)?, 3, MAX_PAGE_CHARS)?,
            ] {
                let results = page
                    .entries
                    .iter()
                    .flat_map(|entry| &entry.blocks)
                    .filter_map(|block| {
                        if let TranscriptBlock::ToolResult {
                            tool_use_id, text, ..
                        } = block
                        {
                            Some((tool_use_id.as_str(), text.as_str()))
                        } else {
                            None
                        }
                    })
                    .collect::<std::collections::HashMap<_, _>>();
                for id in ["old", "new"] {
                    let output = results[id];
                    assert!(output.contains("SHOP_REGION=[REDACTED:"), "{id}: {output}");
                    assert!(output.contains("MAIL_DOMAIN=[REDACTED:"), "{id}: {output}");
                    assert!(!output.contains("eu-north-7b") && !output.contains("mg.shop.example"));
                }
                assert_eq!(results["other"], "SHOP_REGION=public-region");
            }
        }
        Ok(())
    }

    /// Two dotenv values no generic pattern catches: only dotenv redaction hides them.
    const ENV_TWO: &str = "SHOP_REGION=eu-north-7b\nMAIL_DOMAIN=mg.shop.example";

    /// Reads `lines` from the start, from a page after the call (line 1, without and with the
    /// cursor a first read kept) and as a tail, and checks the dotenv values never show.
    fn assert_env_hidden(name: &str, lines: &[Value]) -> TestResult {
        let dir = tempfile::tempdir()?;
        let path = write_lines(dir.path(), lines)?;
        let (_, kept) = read_page_at(File::open(&path)?, None, 0, 1, MAX_PAGE_CHARS)?;
        let last = u64::try_from(lines.len())? - 1;
        let reads = [
            (
                "start",
                read_page(File::open(&path)?, 0, 100, MAX_PAGE_CHARS)?,
            ),
            (
                "page",
                read_page(File::open(&path)?, 1, 100, MAX_PAGE_CHARS)?,
            ),
            (
                "kept cursor",
                read_page_at(File::open(&path)?, Some(&kept), 1, 100, MAX_PAGE_CHARS)?.0,
            ),
            ("tail", read_tail(File::open(&path)?, 1, MAX_PAGE_CHARS)?),
            (
                "last line",
                read_page(File::open(&path)?, last, 100, MAX_PAGE_CHARS)?,
            ),
        ];
        // Restart at every record boundary using only the offsets the index persists.
        let whole = read_for_index(File::open(&path)?, 0, 0, u64::MAX, usize::MAX)?;
        let mut chunks = Vec::new();
        let (mut byte, mut line) = (0, 0);
        while line <= last {
            let next = read_for_index(File::open(&path)?, byte, line, 1, usize::MAX)?;
            assert_eq!(next.end_line, line + 1);
            (byte, line) = (next.end_byte, next.end_line);
            chunks.extend(next.chunks);
        }
        assert_eq!(chunks, whole.chunks, "{name}: resumed index");
        for (how, page) in reads {
            assert!(!page.entries.is_empty(), "{name} ({how}): no entries");
            let text = serde_json::to_string(&page.entries)?;
            assert!(
                !text.contains("eu-north-7b") && !text.contains("mg.shop.example"),
                "{name} ({how}): {text}"
            );
        }
        Ok(())
    }

    /// SPEC-525 WCX-003, on the shapes the real CLIs write (review round 1): Codex 0.169 runs
    /// commands from a code-mode `exec` script whose output parts wrap the command's in JSON,
    /// `shell_command` is the bundled models' shell, a patch adds dotenv lines with `+`;
    /// Antigravity 1.5.10 names a file `Path` and numbers `read_file`'s lines `1: `.
    #[test]
    fn dotenv_reads_in_the_real_cli_shapes_are_redacted() -> TestResult {
        let wrapped = serde_json::to_string(
            &json!({"chunk_id": "ab12cd", "wall_time_seconds": 0.01, "exit_code": 0, "original_token_count": 9, "output": format!("{ENV_TWO}\n")}),
        )?;
        assert_env_hidden(
            "codex exec",
            &[
                json!({"type": "response_item", "payload": {"type": "custom_tool_call", "status": "completed", "call_id": "c1", "name": "exec", "input": "text(await tools.exec_command({cmd:`cat .env`}))\n"}}),
                json!({"type": "response_item", "payload": {"type": "custom_tool_call_output", "call_id": "c1", "output": [{"type": "input_text", "text": "Script completed\nWall time 0.1 seconds\nOutput:\n"}, {"type": "input_text", "text": wrapped}]}}),
            ],
        )?;
        assert_env_hidden(
            "codex shell_command",
            &[
                json!({"type": "response_item", "payload": {"type": "function_call", "name": "shell_command", "arguments": "{\"command\":\"cat .env\",\"workdir\":\"/w\"}", "call_id": "s1"}}),
                json!({"type": "response_item", "payload": {"type": "function_call_output", "call_id": "s1", "output": format!("Exit code: 0\nWall time: 0.1 seconds\nOutput:\n{ENV_TWO}\n")}}),
            ],
        )?;
        for patch in [
            "*** Begin Patch\n*** Add File: .env\n+SHOP_REGION=eu-north-7b\n+MAIL_DOMAIN=mg.shop.example\n*** End Patch",
            "*** Begin Patch\n*** Update File: apps/web/.env.local\n@@\n-SHOP_REGION=us-east\n+SHOP_REGION=eu-north-7b\n MAIL_DOMAIN=mg.shop.example\n*** End Patch",
        ] {
            assert_env_hidden(
                "codex apply_patch",
                &[
                    json!({"type": "response_item", "payload": {"type": "custom_tool_call", "status": "completed", "call_id": "p1", "name": "apply_patch", "input": patch}}),
                    json!({"type": "response_item", "payload": {"type": "custom_tool_call_output", "call_id": "p1", "output": "Success."}}),
                ],
            )?;
        }
        let stamps = "Created At: 2026-11-19T15:18:15Z\nCompleted At: 2026-11-19T15:18:15Z\n";
        assert_env_hidden(
            "agy read_file",
            &[
                json!({"step_index": 0, "source": "MODEL", "type": "PLANNER_RESPONSE", "status": "DONE", "tool_calls": [{"name": "read_file", "args": {"Path": "\"/w/.env\""}}]}),
                json!({"step_index": 1, "source": "MODEL", "type": "GENERIC", "status": "DONE", "content": format!("{stamps}Showing lines 1-2 of 2\n1: SHOP_REGION=eu-north-7b\n2: MAIL_DOMAIN=mg.shop.example")}),
            ],
        )?;
        assert_env_hidden(
            "agy write_file",
            &[
                json!({"step_index": 0, "source": "MODEL", "type": "PLANNER_RESPONSE", "status": "DONE", "tool_calls": [{"name": "write_file", "args": {"Path": "\"/w/.env\"", "Overwrite": "true", "Content": serde_json::to_string(&format!("{ENV_TWO}\n"))?}}]}),
                json!({"step_index": 1, "source": "MODEL", "type": "GENERIC", "status": "DONE", "content": format!("{stamps}Created file.")}),
            ],
        )?;
        assert_env_hidden(
            "agy edit_file",
            &[
                json!({"step_index": 0, "source": "MODEL", "type": "PLANNER_RESPONSE", "status": "DONE", "tool_calls": [{"name": "edit_file", "args": {"Path": "\"/w/.env\"", "Target": "\"SHOP_REGION=us-east\"", "Replacement": "\"SHOP_REGION=eu-north-7b\\nMAIL_DOMAIN=mg.shop.example\""}}]}),
                json!({"step_index": 1, "source": "MODEL", "type": "GENERIC", "status": "DONE", "content": format!("{stamps}Edited file.")}),
            ],
        )?;
        Ok(())
    }

    /// SPEC-525, review round 1: the real shapes read cleanly. A code-mode call's summary is its
    /// command and its result the command's output; Antigravity's thinking is shown and its file
    /// tools name their file; an empty compaction is no entry; a typed prompt that opens with a
    /// tag stays a prompt when the rollout has no provenance.
    #[test]
    fn the_real_cli_shapes_read_as_commands_outputs_and_thinking() -> TestResult {
        let dir = tempfile::tempdir()?;
        let wrapped = serde_json::to_string(
            &json!({"chunk_id": "ab12cd", "exit_code": 0, "output": "lib.rs\nmain.rs\n"}),
        )?;
        let path = write_lines(
            dir.path(),
            &[
                json!({"type": "response_item", "payload": {"type": "custom_tool_call", "call_id": "c1", "name": "exec", "input": "text(await tools.exec_command({cmd:`ls src`, yield_time_ms: 1000}))\n"}}),
                json!({"type": "response_item", "payload": {"type": "custom_tool_call_output", "call_id": "c1", "output": [{"type": "input_text", "text": "Script completed\nWall time 0.1 seconds\nOutput:\n"}, {"type": "input_text", "text": wrapped}]}}),
                json!({"type": "compacted", "payload": {"message": "", "replacement_history": []}}),
                json!({"type": "response_item", "payload": {"type": "message", "role": "user", "content": [{"type": "input_text", "text": "<div> breaks the layout"}]}}),
                json!({"type": "response_item", "payload": {"type": "custom_tool_call", "call_id": "c2", "name": "exec", "input": "const a = 1;\ntext(String(a))\n"}}),
            ],
        )?;
        let page = read_page(File::open(&path)?, 0, 100, MAX_PAGE_CHARS)?;
        assert_eq!(
            shape(&page.entries),
            vec![
                "assistant call c1 exec: ls src",
                "user result c1 Script completed\nWall time 0.1 seconds\nO",
                "user text <div> breaks the layout",
                "assistant call c2 exec: const a = 1; text(String(a))",
            ]
        );
        let TranscriptBlock::ToolResult { text, .. } = &page.entries[1].blocks[0] else {
            return Err("the output".into());
        };
        assert!(
            text.ends_with("Output:\nlib.rs\nmain.rs\n"),
            "the command's own output: {text:?}"
        );

        let agy = dir.path().join("agy");
        std::fs::create_dir_all(&agy)?;
        let path = write_lines(
            &agy,
            &[
                json!({"step_index": 0, "source": "MODEL", "type": "PLANNER_RESPONSE", "status": "DONE", "thinking": "Check the cart first.", "tool_calls": [{"name": "read_file", "args": {"Path": "\"/w/src/cart.ts\"", "StartLine": "1"}}]}),
            ],
        )?;
        let page = read_page(File::open(&path)?, 0, 100, MAX_PAGE_CHARS)?;
        assert_eq!(
            shape(&page.entries),
            vec![
                "assistant thinking Check the cart first.",
                "assistant call step-0-0 read_file: /w/src/cart.ts",
            ]
        );
        Ok(())
    }

    fn exec_call(id: &str, script: &str) -> Value {
        json!({"type": "response_item", "payload": {"type": "custom_tool_call", "status": "completed", "call_id": id, "name": "exec", "input": script}})
    }

    fn exec_output(id: &str, parts: &[Value]) -> Value {
        let mut list = vec![
            json!({"type": "input_text", "text": "Script completed\nWall time 0.1 seconds\nOutput:\n"}),
        ];
        for part in parts {
            let text = match part {
                Value::String(text) => text.clone(),
                other => other.to_string(),
            };
            list.push(json!({"type": "input_text", "text": text}));
        }
        json!({"type": "response_item", "payload": {"type": "custom_tool_call_output", "call_id": id, "output": list}})
    }

    fn chunk(output: &str) -> Value {
        json!({"chunk_id": "ab12cd", "wall_time_seconds": 0.01, "exit_code": 0, "original_token_count": 9, "output": output})
    }

    /// SPEC-525 WCX-003, review round 2: a code-mode script prints its results nested (a
    /// `Promise.allSettled` list, `{status, value}` one by one, a list of wrappers or of strings,
    /// an MCP tool's `{content, isError}`), quotes its command with escapes, or yields and is read
    /// on by a later `wait` or `write_stdin`; a patch can come as a function call; an
    /// Antigravity command can go on as a task read by `manage_task` after the next response, and
    /// an MCP tool keeps its path inside `Arguments`. Every one hides the dotenv values.
    #[test]
    fn dotenv_reads_nested_escaped_or_continued_are_redacted() -> TestResult {
        let env = format!("{ENV_TWO}\n");
        let all = "const r = await Promise.allSettled([tools.exec_command({cmd:`cat .env`}), tools.exec_command({cmd:`ls`})]);\ntext(r)\n";
        let cases: Vec<(&str, Vec<Value>)> = vec![
            (
                "allSettled list",
                vec![
                    exec_call("c1", all),
                    exec_output(
                        "c1",
                        &[
                            json!([{"status": "fulfilled", "value": chunk(&env)}, {"status": "fulfilled", "value": chunk("src\n")}]),
                        ],
                    ),
                ],
            ),
            (
                "allSettled one by one",
                vec![
                    exec_call("c1", all),
                    exec_output(
                        "c1",
                        &[
                            json!({"status": "fulfilled", "value": chunk(&env)}),
                            json!({"status": "fulfilled", "value": chunk("src\n")}),
                        ],
                    ),
                ],
            ),
            (
                "list of wrappers",
                vec![
                    exec_call("c1", all),
                    exec_output("c1", &[json!([chunk(&env), chunk("src\n")])]),
                ],
            ),
            (
                "list of strings",
                vec![exec_call("c1", all), exec_output("c1", &[json!([env])])],
            ),
            (
                "an unknown JSON shape",
                vec![
                    exec_call("c1", all),
                    exec_output("c1", &[json!({"results": {"first": {"text_of_it": env}}})]),
                ],
            ),
            (
                "MCP result",
                vec![
                    exec_call(
                        "c1",
                        "text(await tools.mcp__fs__read_file({path:`/w/.env`}))\n",
                    ),
                    exec_output(
                        "c1",
                        &[json!({"content": [{"type": "text", "text": env}], "isError": false})],
                    ),
                ],
            ),
            (
                "double quotes and a newline escape",
                vec![
                    exec_call(
                        "c1",
                        "text(await tools.exec_command({cmd:\"cat .env\\ncat package.json\"}))\n",
                    ),
                    exec_output("c1", &[chunk(&env)]),
                ],
            ),
            (
                "escaped quotes around the file",
                vec![
                    exec_call(
                        "c1",
                        "text(await tools.exec_command({cmd:\"cat \\\".env\\\"\"}))\n",
                    ),
                    exec_output("c1", &[chunk(&env)]),
                ],
            ),
            (
                "bash -lc with escaped quotes",
                vec![
                    exec_call(
                        "c1",
                        "text(await tools.exec_command({cmd:\"bash -lc \\\"cat .env\\\"\"}))\n",
                    ),
                    exec_output("c1", &[chunk(&env)]),
                ],
            ),
            (
                "write_stdin continuation",
                vec![
                    exec_call(
                        "c1",
                        "text(await tools.exec_command({cmd:`sleep 2; cat .env`, yield_time_ms: 1000}))\n",
                    ),
                    exec_output(
                        "c1",
                        &[
                            json!({"chunk_id": "a", "wall_time_seconds": 1.0, "session_id": 7, "original_token_count": 0, "output": ""}),
                        ],
                    ),
                    exec_call(
                        "c2",
                        "text(await tools.write_stdin({session_id: 7, chars: ``, yield_time_ms: 3000}))\n",
                    ),
                    exec_output("c2", &[chunk(&env)]),
                ],
            ),
            (
                "wait continuation",
                vec![
                    exec_call(
                        "c1",
                        "text(await tools.exec_command({cmd:`sleep 20; cat .env`}))\n",
                    ),
                    exec_output("c1", &[json!("Script running with cell ID 5")]),
                    json!({"type": "response_item", "payload": {"type": "function_call", "name": "wait", "arguments": "{\"cell_id\":\"5\",\"yield_time_ms\":10000}", "call_id": "w1"}}),
                    json!({"type": "response_item", "payload": {"type": "function_call_output", "call_id": "w1", "output": [{"type": "input_text", "text": "Script completed\nOutput:\n"}, {"type": "input_text", "text": chunk(&env).to_string()}]}}),
                ],
            ),
            (
                "patch heredoc in double quotes",
                vec![
                    exec_call(
                        "c1",
                        "text(await tools.exec_command({cmd:\"apply_patch <<'PATCH'\\n*** Begin Patch\\n*** Add File: .env\\n+SHOP_REGION=eu-north-7b\\n+MAIL_DOMAIN=mg.shop.example\\n*** End Patch\\nPATCH\"}))\n",
                    ),
                    exec_output("c1", &[chunk("Success.\n")]),
                ],
            ),
            (
                "function-style apply_patch",
                vec![
                    json!({"type": "response_item", "payload": {"type": "function_call", "name": "apply_patch", "arguments": json!({"input": "*** Begin Patch\n*** Add File: .env\n+SHOP_REGION=eu-north-7b\n+MAIL_DOMAIN=mg.shop.example\n*** End Patch"}).to_string(), "call_id": "p1"}}),
                    json!({"type": "response_item", "payload": {"type": "function_call_output", "call_id": "p1", "output": "Success."}}),
                ],
            ),
            (
                "unparsed shell arguments",
                vec![
                    json!({"type": "response_item", "payload": {"type": "function_call", "name": "shell_command", "arguments": "cat .env", "call_id": "u1"}}),
                    json!({"type": "response_item", "payload": {"type": "function_call_output", "call_id": "u1", "output": env.clone()}}),
                ],
            ),
            (
                "local_shell_call",
                vec![
                    json!({"type": "response_item", "payload": {"type": "local_shell_call", "call_id": "l1", "status": "completed", "action": {"type": "exec", "command": ["bash", "-lc", "cat .env"]}}}),
                    json!({"type": "response_item", "payload": {"type": "local_shell_call_output", "call_id": "l1", "output": env.clone()}}),
                ],
            ),
            (
                "agy async command read by manage_task",
                vec![
                    json!({"step_index": 0, "source": "MODEL", "type": "PLANNER_RESPONSE", "status": "DONE", "tool_calls": [{"name": "run_command", "args": {"CommandLine": "\"sleep 9; cat .env\"", "WaitMsBeforeAsync": "1000"}}]}),
                    json!({"step_index": 1, "source": "MODEL", "type": "GENERIC", "status": "RUNNING", "content": "Command is running as task t1."}),
                    json!({"step_index": 2, "source": "MODEL", "type": "PLANNER_RESPONSE", "status": "DONE", "tool_calls": [{"name": "manage_task", "args": {"Action": "\"status\"", "TaskId": "\"t1\""}}]}),
                    json!({"step_index": 3, "source": "MODEL", "type": "GENERIC", "status": "DONE", "content": format!("Command output:\n{ENV_TWO}")}),
                ],
            ),
            (
                "agy call_mcp_tool",
                vec![
                    json!({"step_index": 0, "source": "MODEL", "type": "PLANNER_RESPONSE", "status": "DONE", "tool_calls": [{"name": "call_mcp_tool", "args": {"ServerName": "\"fs\"", "ToolName": "\"read\"", "Arguments": "{\"path\":\"/w/.env\"}"}}]}),
                    json!({"step_index": 1, "source": "MODEL", "type": "GENERIC", "status": "DONE", "content": ENV_TWO}),
                ],
            ),
        ];
        let async_result = |body: &str| json!({"step_index": 3, "source": "SYSTEM", "type": "SYSTEM_MESSAGE", "status": "DONE", "content": format!("The following is a <SYSTEM_MESSAGE> not actually sent by the user. <SYSTEM_MESSAGE> [Message] timestamp=2026-11-19T10:00:00Z sender=abc/task-1 priority=MESSAGE_PRIORITY_HIGH content=Task id \"abc/task-1\" finished with result: {body}</SYSTEM_MESSAGE>")});
        let went_async = [
            json!({"step_index": 0, "source": "MODEL", "type": "PLANNER_RESPONSE", "status": "DONE", "tool_calls": [{"name": "run_command", "args": {"CommandLine": "\"sleep 9; cat .env\"", "WaitMsBeforeAsync": "1000"}}]}),
            json!({"step_index": 1, "source": "MODEL", "type": "GENERIC", "status": "RUNNING", "content": "Tool is running as a background task with task id: abc/task-1"}),
            json!({"step_index": 2, "source": "MODEL", "type": "PLANNER_RESPONSE", "status": "DONE", "content": "Waiting."}),
        ];
        let round_three: Vec<(&str, Vec<Value>)> = vec![
            (
                "agy async result as a system message",
                went_async
                    .iter()
                    .cloned()
                    .chain([async_result(&format!(
                        "Command exited with code 0\nOutput:\n{ENV_TWO}\n"
                    ))])
                    .collect(),
            ),
            (
                "agy async one-line result",
                went_async
                    .iter()
                    .cloned()
                    .chain([async_result(
                        "Command exited with code 0\nOutput:\nSHOP_REGION=eu-north-7b\n",
                    )])
                    .collect(),
            ),
            (
                "agy async numbered result",
                went_async
                    .iter()
                    .cloned()
                    .chain([async_result(
                        "Output:\n1: SHOP_REGION=eu-north-7b\n2: MAIL_DOMAIN=mg.shop.example\n",
                    )])
                    .collect(),
            ),
            (
                "write_stdin through a map shorthand",
                vec![
                    exec_call(
                        "c1",
                        "text(await tools.exec_command({cmd:`sleep 2; cat .env`, yield_time_ms: 1000}))\n",
                    ),
                    exec_output(
                        "c1",
                        &[
                            json!({"chunk_id": "a", "wall_time_seconds": 1.0, "session_id": 7, "original_token_count": 0, "output": ""}),
                        ],
                    ),
                    exec_call(
                        "c2",
                        "for (const r of await Promise.allSettled([7,9].map(session_id=>tools.write_stdin({session_id,chars:\"\",yield_time_ms:3000})))) text(r.value)\n",
                    ),
                    exec_output("c2", &[chunk(&env), chunk("ok\n")]),
                ],
            ),
            (
                "write_stdin with the id in a variable",
                vec![
                    exec_call(
                        "c1",
                        "text(await tools.exec_command({cmd:`sleep 2; cat .env`, yield_time_ms: 1000}))\n",
                    ),
                    exec_output(
                        "c1",
                        &[
                            json!({"chunk_id": "a", "wall_time_seconds": 1.0, "session_id": 7, "original_token_count": 0, "output": ""}),
                        ],
                    ),
                    exec_call(
                        "c2",
                        "const id = 7;\ntext(await tools.write_stdin({session_id: id, chars: ``}))\n",
                    ),
                    exec_output("c2", &[chunk(&env)]),
                ],
            ),
        ];
        for (name, lines) in cases.into_iter().chain(round_three) {
            assert_env_hidden(name, &lines)?;
        }
        Ok(())
    }

    /// SPEC-525, review round 2: nested results read as the commands' own output (not JSON), and
    /// an Antigravity request with nothing in it is no entry.
    #[test]
    fn nested_code_mode_results_read_as_their_outputs() -> TestResult {
        let dir = tempfile::tempdir()?;
        let path = write_lines(
            dir.path(),
            &[
                exec_call(
                    "c1",
                    "text(await Promise.allSettled([tools.exec_command({cmd:`ls`}), tools.view({path:`a`}), tools.exec_command({cmd:`false`})]))\n",
                ),
                exec_output(
                    "c1",
                    &[
                        json!([{"status": "fulfilled", "value": chunk("src\n")}, {"status": "fulfilled", "value": "file body"}, {"status": "rejected", "reason": "exit 1"}]),
                    ],
                ),
                json!({"step_index": 0, "source": "USER_EXPLICIT", "type": "USER_INPUT", "status": "DONE", "content": "<USER_REQUEST>\n</USER_REQUEST>"}),
                exec_call("c2", "text(await tools.mcp__x__list({}))\n"),
                exec_output(
                    "c2",
                    &[json!({"items": ["alpha", "beta"], "total": 2, "next": "cursor-3"})],
                ),
            ],
        )?;
        let page = read_page(File::open(&path)?, 0, 100, MAX_PAGE_CHARS)?;
        assert_eq!(page.entries.len(), 4, "{:?}", shape(&page.entries));
        let result = |n: usize| match &page.entries[n].blocks[0] {
            TranscriptBlock::ToolResult { text, .. } => Ok(text.clone()),
            _ => Err("the output"),
        };
        assert_eq!(
            result(1)?,
            "Script completed\nWall time 0.1 seconds\nOutput:\nsrc\nfile body\nexit 1"
        );
        // JSON that is no code-mode result is shown whole.
        assert_eq!(
            result(3)?,
            "Script completed\nWall time 0.1 seconds\nOutput:\n{\"items\":[\"alpha\",\"beta\"],\"next\":\"cursor-3\",\"total\":2}"
        );
        Ok(())
    }

    /// SPEC-525, review round 4: a settled promise whose `value` is a list of content items
    /// shows their texts; a background task cancelled with a result is redacted as one finished.
    #[test]
    fn settled_content_items_show_and_cancelled_results_are_redacted() -> TestResult {
        let dir = tempfile::tempdir()?;
        let path = write_lines(
            dir.path(),
            &[
                exec_call(
                    "c1",
                    "text(await Promise.allSettled([tools.exec_command({cmd:`ls`}), tools.mcp__x__read({})]))\n",
                ),
                exec_output(
                    "c1",
                    &[
                        json!([{"status": "fulfilled", "value": chunk("src\n")}, {"status": "fulfilled", "value": [{"type": "text", "text": "page one"}]}]),
                    ],
                ),
            ],
        )?;
        let page = read_page(File::open(&path)?, 0, 100, MAX_PAGE_CHARS)?;
        let TranscriptBlock::ToolResult { text, .. } = &page.entries[1].blocks[0] else {
            return Err("the output".into());
        };
        assert!(text.ends_with("Output:\nsrc\npage one"), "{text:?}");
        let cancelled = format!(
            "<SYSTEM_MESSAGE> content=Task id \"abc/task-1\" was canceled with result: \
             Output:\n{ENV_TWO}\n</SYSTEM_MESSAGE>"
        );
        assert_env_hidden(
            "agy async result of a cancelled task",
            &[
                json!({"step_index": 0, "source": "MODEL", "type": "PLANNER_RESPONSE", "status": "DONE", "tool_calls": [{"name": "run_command", "args": {"CommandLine": "\"sleep 9; cat .env\""}}]}),
                json!({
                    "step_index": 1,
                    "source": "SYSTEM",
                    "type": "SYSTEM_MESSAGE",
                    "status": "DONE",
                    "content": cancelled,
                }),
            ],
        )
    }
}
