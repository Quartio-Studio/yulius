//! Claude Code transcripts as read-only documents (SPEC-001 SES-002..005): one JSONL line →
//! one [`TranscriptEntry`] of messages, tool calls, results and diffs, redacted and bounded;
//! paged reading, index passages and the plain-text rendering agents read.
//!
//! Every text that leaves this module went through [`yulius_domain::redact`]: provider tokens,
//! keys and secret assignments everywhere, and every value of a dotenv file that a tool read or
//! wrote (the tool call's path decides) — so task transcripts, which are not redacted at the
//! source, are safe to show to people and to other tasks.

use std::collections::HashSet;
use std::fs::File;
use std::io::{self, BufRead, BufReader, Seek, SeekFrom};
use std::path::Path;

use serde_json::Value;
use yulius_contracts::api::sessions::{DiffHunk, TranscriptBlock, TranscriptDiff, TranscriptEntry};
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
#[derive(Debug, Default)]
pub struct EnvCalls {
    ids: HashSet<String>,
}

impl EnvCalls {
    fn touches_env(name: &str, input: &Value) -> bool {
        match name {
            "Bash" => str_field(input, "command").is_some_and(mentions_env_file),
            _ => ["file_path", "path", "notebook_path"]
                .iter()
                .filter_map(|key| str_field(input, key))
                .any(is_env_file),
        }
    }
}

/// One-line description of a tool call.
fn tool_summary(name: &str, input: &Value) -> String {
    let field = |key| str_field(input, key).map(one_line);
    let summary = match name {
        "Bash" => field("command"),
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

fn diff_of(name: &str, input: &Value) -> Option<TranscriptDiff> {
    let path = str_field(input, "file_path")?.to_owned();
    let hunk = |old: &str, new: &str| DiffHunk {
        old: bounded(old, MAX_HUNK_CHARS),
        new: bounded(new, MAX_HUNK_CHARS),
    };
    let hunks = match name {
        "Edit" => vec![hunk(
            str_field(input, "old_string").unwrap_or(""),
            str_field(input, "new_string").unwrap_or(""),
        )],
        "Write" => vec![hunk("", str_field(input, "content").unwrap_or(""))],
        "MultiEdit" => input
            .get("edits")
            .and_then(Value::as_array)?
            .iter()
            .take(MAX_HUNKS)
            .map(|edit| {
                hunk(
                    str_field(edit, "old_string").unwrap_or(""),
                    str_field(edit, "new_string").unwrap_or(""),
                )
            })
            .collect(),
        _ => return None,
    };
    Some(TranscriptDiff { path, hunks })
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
            let text = str_field(record, "summary")?;
            blocks.push(TranscriptBlock::Text {
                text: clean(&bounded(text, MAX_TEXT_CHARS), false),
                truncated: false,
            });
            "summary"
        }
        "system" => {
            let text = str_field(record, "content")?;
            blocks.push(TranscriptBlock::Meta {
                text: clean(&bounded(text, MAX_TEXT_CHARS), false),
            });
            "system"
        }
        "user" | "assistant" => {
            let content = record.get("message").and_then(|m| m.get("content"))?;
            match content {
                Value::String(text) => blocks.push(text_block(text, meta_line)),
                Value::Array(parts) => {
                    for part in parts {
                        if let Some(block) = content_block(part, meta_line, env_calls) {
                            blocks.push(block);
                        }
                    }
                }
                _ => {}
            }
            kind
        }
        _ => return None,
    };
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

fn text_block(text: &str, meta: bool) -> TranscriptBlock {
    if meta || is_meta_text(text) {
        return TranscriptBlock::Meta {
            text: clean(&bounded(text, MAX_RESULT_CHARS), false),
        };
    }
    let (text, truncated) = cut(text, MAX_TEXT_CHARS);
    TranscriptBlock::Text {
        text: clean(&text, false),
        truncated,
    }
}

fn content_block(part: &Value, meta: bool, env_calls: &mut EnvCalls) -> Option<TranscriptBlock> {
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
            let id = str_field(part, "id").unwrap_or("").to_owned();
            let name = str_field(part, "name").unwrap_or("tool").to_owned();
            let mut input = part.get("input").cloned().unwrap_or(Value::Null);
            let env = EnvCalls::touches_env(&name, &input);
            if env && !id.is_empty() {
                env_calls.ids.insert(id.clone());
            }
            redact_value(&mut input, env);
            let pretty = serde_json::to_string_pretty(&input).unwrap_or_default();
            Some(TranscriptBlock::ToolCall {
                summary: tool_summary(&name, &input),
                diff: diff_of(&name, &input),
                input: bounded(&pretty, MAX_INPUT_CHARS),
                id,
                name,
            })
        }
        "tool_result" => {
            let tool_use_id = str_field(part, "tool_use_id").unwrap_or("").to_owned();
            let env = env_calls.ids.contains(&tool_use_id);
            let raw = result_text(part.get("content"));
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
            | TranscriptBlock::Meta { text }
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

/// Reads entries from line `from`: at most `limit` entries (≤ [`MAX_PAGE_ENTRIES`]) and about
/// `max_chars` of text (≤ [`MAX_PAGE_CHARS`]). Tool calls on earlier lines are scanned too, so
/// a dotenv file read before the page is still redacted in it.
///
/// # Errors
/// I/O failures opening or reading the transcript.
pub fn read_page(path: &Path, from: u64, limit: usize, max_chars: usize) -> io::Result<Page> {
    let limit = limit.clamp(1, MAX_PAGE_ENTRIES);
    let max_chars = max_chars.clamp(1, MAX_PAGE_CHARS);
    let mut reader = BufReader::with_capacity(256 * 1024, File::open(path)?);
    let mut buf = Vec::new();
    let mut env_calls = EnvCalls::default();
    let mut page = Page::default();
    let mut chars = 0;
    let mut line = 0_u64;
    loop {
        let read = read_line(&mut reader, &mut buf, MAX_LINE_BYTES)?;
        let current = line;
        match read {
            LineRead::Eof => return Ok(page),
            LineRead::TooLong(_) => {
                line += 1;
                if current >= from {
                    page.skipped += 1;
                }
                continue;
            }
            LineRead::Line(_) => line += 1,
        }
        if current < from {
            // Only tool calls matter before the page (dotenv tracking); skip the parse otherwise.
            if memchr_tool_use(&buf)
                && let Ok(record) = serde_json::from_slice::<Value>(&buf)
            {
                let _ = entry_of(current, &record, &mut env_calls);
            }
            continue;
        }
        if page.entries.len() >= limit || chars >= max_chars {
            page.next = Some(current);
            return Ok(page);
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

fn memchr_tool_use(line: &[u8]) -> bool {
    line.windows(10).any(|window| window == b"\"tool_use\"")
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
    /// Characters of passage text.
    pub chars: usize,
    /// Whether the end of the transcript was reached.
    pub eof: bool,
}

/// Reads complete lines from byte `from_byte` (line number `from_line`) until `max_bytes` were
/// read or `max_chars` of passage text were produced.
///
/// # Errors
/// I/O failures.
pub fn read_for_index(
    path: &Path,
    from_byte: u64,
    from_line: u64,
    max_bytes: u64,
    max_chars: usize,
) -> io::Result<IndexBatch> {
    let mut file = File::open(path)?;
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
    while read_bytes < max_bytes && batch.chars < max_chars {
        let used = match read_line(&mut reader, &mut buf, MAX_LINE_BYTES)? {
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
        let page = read_page(&path, 0, 100, MAX_PAGE_CHARS)?;
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
        let first = read_page(&path, 0, 2, MAX_PAGE_CHARS)?;
        assert_eq!(first.entries.len(), 2);
        assert_eq!(first.next, Some(3));
        let second = read_page(&path, 3, 1, MAX_PAGE_CHARS)?;
        let TranscriptBlock::ToolResult { text, .. } = &second.entries[0].blocks[0] else {
            return Err("tool result".into());
        };
        assert!(text.contains("[REDACTED:"), "{text}");
        Ok(())
    }

    #[test]
    fn index_batches_resume_at_byte_offsets() -> TestResult {
        let dir = tempfile::tempdir()?;
        let path = write_transcript(dir.path())?;
        let first = read_for_index(&path, 0, 0, 1, usize::MAX)?;
        assert_eq!(first.end_line, 1, "one line per budget step");
        assert!(first.chunks.is_empty(), "the snapshot line has no passage");
        let rest = read_for_index(&path, first.end_byte, first.end_line, u64::MAX, usize::MAX)?;
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
    fn rendering_for_agents_is_bounded() -> TestResult {
        let dir = tempfile::tempdir()?;
        let path = write_transcript(dir.path())?;
        let page = read_page(&path, 0, 100, MAX_PAGE_CHARS)?;
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
}
