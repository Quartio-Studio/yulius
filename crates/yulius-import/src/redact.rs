//! Redacting Claude Code transcripts before they leave the computer (SPEC-001 IMP-008).
//!
//! Every JSON string of every line goes through [`yulius_domain::redact`]; secret-named keys
//! (`{"password": "…"}`) lose their literal values; whatever a tool read from or wrote to a dotenv
//! file (`.env`, `.env.local`, …; the tool call's path or command decides) loses every value; and
//! images (base64 screenshots, often with secrets on them) are replaced by a text block, which
//! keeps the transcript resumable. Lines without findings are copied byte for byte, so the
//! output is deterministic: the same session redacts to the same bytes and digest (IMP-006).

use std::collections::HashSet;
use std::fs::File;
use std::io::{self, BufRead, BufReader, BufWriter, Write};
use std::path::Path;

use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use yulius_contracts::import::FileRef;
use yulius_domain::redact::{
    RedactionTally, is_env_file, mentions_env_file, redact, redact_env, redact_named,
};

/// Longest transcript line kept; longer lines are left out and counted.
pub const MAX_STAGED_LINE_BYTES: usize = 16 * 1024 * 1024;
/// Text that replaces an image block.
pub const IMAGE_PLACEHOLDER: &str = "[image left out by yulius onboard]";

/// What redacting one transcript did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RedactionReport {
    /// Redacted secrets by rule and label.
    pub tally: RedactionTally,
    /// Image blocks replaced by text.
    pub images: u64,
    /// Lines over [`MAX_STAGED_LINE_BYTES`] left out.
    pub long_lines: u64,
}

impl RedactionReport {
    /// Adds another report.
    pub fn merge(&mut self, other: &Self) {
        self.tally.merge(&other.tally);
        self.images += other.images;
        self.long_lines += other.long_lines;
    }
}

/// Tool calls of a transcript that touch dotenv files (their results are dotenv content).
#[derive(Debug, Default)]
struct EnvCalls(HashSet<String>);

fn touches_env(name: &str, input: &Value) -> bool {
    let field = |key: &str| input.get(key).and_then(Value::as_str);
    match name {
        "Bash" => field("command").is_some_and(mentions_env_file),
        _ => ["file_path", "path", "notebook_path"]
            .iter()
            .filter_map(|key| field(key))
            .any(is_env_file),
    }
}

/// Redacts one string; `env` treats it as dotenv content.
fn redact_string(text: &mut String, env: bool, report: &mut RedactionReport) -> bool {
    let found = if env { redact_env(text) } else { redact(text) };
    match found {
        Some(redacted) => {
            report.tally.add(&redacted.findings);
            *text = redacted.text;
            true
        }
        None => false,
    }
}

/// Walks a JSON value: strings are redacted, secret-named keys lose literal values.
fn walk(value: &mut Value, env: bool, report: &mut RedactionReport) -> bool {
    match value {
        Value::String(text) => redact_string(text, env, report),
        Value::Array(items) => items
            .iter_mut()
            .fold(false, |changed, item| walk(item, env, report) | changed),
        Value::Object(map) => walk_object(map, env, report),
        _ => false,
    }
}

fn walk_object(map: &mut Map<String, Value>, env: bool, report: &mut RedactionReport) -> bool {
    let mut changed = false;
    for (key, item) in map.iter_mut() {
        if let Value::String(text) = item
            && let Some(redacted) = redact_named(key, text)
        {
            report.tally.add(&redacted.findings);
            *text = redacted.text;
            changed = true;
            continue;
        }
        changed |= walk(item, env, report);
    }
    changed
}

/// Replaces image blocks (anywhere in the line) by a text block.
fn strip_images(value: &mut Value, report: &mut RedactionReport) -> bool {
    match value {
        Value::Array(items) => items
            .iter_mut()
            .fold(false, |changed, item| strip_images(item, report) | changed),
        Value::Object(map) => {
            let image = map.get("type").and_then(Value::as_str) == Some("image")
                && map.get("source").is_some();
            if image {
                map.clear();
                map.insert("type".into(), Value::String("text".into()));
                map.insert("text".into(), Value::String(IMAGE_PLACEHOLDER.into()));
                report.images += 1;
                return true;
            }
            map.values_mut()
                .fold(false, |changed, item| strip_images(item, report) | changed)
        }
        _ => false,
    }
}

/// The content blocks of a Claude line (`message.content` when it is an array).
fn blocks(record: &Value) -> &[Value] {
    record
        .get("message")
        .and_then(|m| m.get("content"))
        .and_then(Value::as_array)
        .map_or(&[], Vec::as_slice)
}

/// Redacts one parsed transcript line in place; returns whether it changed.
fn redact_line(record: &mut Value, env_calls: &mut EnvCalls, report: &mut RedactionReport) -> bool {
    // Tool calls touching dotenv files: their inputs and later results are dotenv content.
    let mut env_line = false;
    for block in blocks(record) {
        let kind = block.get("type").and_then(Value::as_str);
        if kind == Some("tool_use") {
            let name = block.get("name").and_then(Value::as_str).unwrap_or("");
            let input = block.get("input").unwrap_or(&Value::Null);
            if touches_env(name, input) {
                if let Some(id) = block.get("id").and_then(Value::as_str) {
                    env_calls.0.insert(id.to_owned());
                }
                env_line |= name != "Bash";
            }
        } else if kind == Some("tool_result")
            && block
                .get("tool_use_id")
                .and_then(Value::as_str)
                .is_some_and(|id| env_calls.0.contains(id))
        {
            env_line = true;
        }
    }
    let mut changed = strip_images(record, report);
    changed |= walk(record, env_line, report);
    changed
}

/// Reads one `\n`-terminated line (a final line without newline is unfinished and not returned).
/// `Ok(Some(true))` = a line in `buf`, `Some(false)` = a line over `max` was skipped.
fn next_line(reader: &mut impl BufRead, buf: &mut Vec<u8>, max: usize) -> io::Result<Option<bool>> {
    buf.clear();
    let mut too_long = false;
    loop {
        let available = reader.fill_buf()?;
        if available.is_empty() {
            return Ok(None);
        }
        let (chunk, end) = match available.iter().position(|b| *b == b'\n') {
            Some(index) => (&available[..=index], true),
            None => (available, false),
        };
        if !too_long {
            if buf.len() + chunk.len() > max {
                too_long = true;
                buf.clear();
            } else {
                buf.extend_from_slice(chunk);
            }
        }
        let used = chunk.len();
        reader.consume(used);
        if end {
            return Ok(Some(!too_long));
        }
    }
}

/// Copies the complete lines of `source` to `dest` with secrets redacted and images left out,
/// stopping before `budget` bytes would be exceeded. Returns the staged file's reference (named
/// `name`) and what was redacted.
///
/// # Errors
/// I/O failures; [`io::ErrorKind::FileTooLarge`] when the output would exceed `budget`.
pub fn stage_redacted(
    source: &Path,
    dest: &Path,
    name: &str,
    budget: u64,
) -> io::Result<(FileRef, RedactionReport)> {
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut reader = BufReader::with_capacity(256 * 1024, File::open(source)?);
    let mut writer = BufWriter::with_capacity(256 * 1024, File::create(dest)?);
    let mut hasher = Sha256::new();
    let mut report = RedactionReport::default();
    let mut env_calls = EnvCalls::default();
    let mut buf = Vec::new();
    let mut size = 0_u64;
    while let Some(complete) = next_line(&mut reader, &mut buf, MAX_STAGED_LINE_BYTES)? {
        if !complete {
            report.long_lines += 1;
            continue;
        }
        let mut emit = |bytes: &[u8]| -> io::Result<()> {
            size = size.saturating_add(bytes.len() as u64);
            if size > budget {
                return Err(io::Error::new(
                    io::ErrorKind::FileTooLarge,
                    "the session exceeds the upload bound",
                ));
            }
            hasher.update(bytes);
            writer.write_all(bytes)
        };
        match serde_json::from_slice::<Value>(&buf) {
            Ok(mut record) => {
                if redact_line(&mut record, &mut env_calls, &mut report) {
                    let mut line = serde_json::to_vec(&record).map_err(io::Error::other)?;
                    line.push(b'\n');
                    emit(&line)?;
                } else {
                    emit(&buf)?;
                }
            }
            Err(_) => {
                // Not JSON: redact as text (keeps the line, never leaks it).
                let text = String::from_utf8_lossy(&buf).into_owned();
                match redact(&text) {
                    Some(redacted) => {
                        report.tally.add(&redacted.findings);
                        emit(redacted.text.as_bytes())?;
                    }
                    None => emit(&buf)?,
                }
            }
        }
    }
    writer.flush()?;
    writer.get_ref().sync_all()?;
    Ok((
        FileRef {
            name: name.to_owned(),
            size,
            sha256: hex::encode(hasher.finalize()),
        },
        report,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    fn token() -> String {
        format!("ghp_{}", "Kq8Lm".repeat(8))
    }

    fn transcript() -> String {
        [
            json!({"type": "user", "message": {"role": "user", "content": format!("deploy with {}", token())}}),
            json!({"type": "assistant", "message": {"role": "assistant", "content": [
                {"type": "tool_use", "id": "t1", "name": "Read", "input": {"file_path": "/repo/.env"}},
                {"type": "tool_use", "id": "t2", "name": "mcp__db__connect", "input": {"password": "s3cret-pass"}}
            ]}}),
            json!({"type": "user", "message": {"role": "user", "content": [
                {"type": "tool_result", "tool_use_id": "t1", "content": "     1→API_URL=https://api.internal\n     2→LOG_LEVEL=debug\n"},
                {"type": "image", "source": {"type": "base64", "media_type": "image/png", "data": "iVBORw0KGgo"}}
            ]}, "toolUseResult": {"file": {"filePath": "/repo/.env", "content": "API_URL=https://api.internal\n"}}}),
            json!({"type": "assistant", "message": {"role": "assistant", "content": [{"type": "text", "text": "Nothing secret here."}]}}),
        ]
        .iter()
        .map(|line| format!("{line}\n"))
        .collect::<String>()
            + "{\"type\":\"user\",\"unfinished"
    }

    #[test]
    fn transcripts_are_redacted_deterministically_and_stay_valid_jsonl() -> TestResult {
        let dir = tempfile::tempdir()?;
        let source = dir.path().join("in.jsonl");
        std::fs::write(&source, transcript())?;
        let (first, report) = stage_redacted(&source, &dir.path().join("a.jsonl"), "x", u64::MAX)?;
        let (second, _) = stage_redacted(&source, &dir.path().join("b.jsonl"), "x", u64::MAX)?;
        assert_eq!(first, second, "same input, same bytes and digest");
        let out = std::fs::read_to_string(dir.path().join("a.jsonl"))?;
        assert!(!out.contains(&token()));
        assert!(!out.contains("s3cret-pass"));
        assert!(!out.contains("api.internal"), "dotenv values from Read results go: {out}");
        assert!(!out.contains("iVBORw0KGgo"), "images are left out");
        assert!(out.contains(IMAGE_PLACEHOLDER));
        assert!(out.contains("Nothing secret here."));
        assert!(!out.contains("unfinished"), "the unfinished last line waits for the next run");
        for line in out.lines() {
            let _: Value = serde_json::from_str(line)?;
        }
        assert_eq!(report.images, 1);
        assert_eq!(report.tally.by_rule.get("github-token"), Some(&1));
        assert_eq!(report.tally.by_rule.get("secret-assignment"), Some(&1));
        assert_eq!(report.tally.by_rule.get("env-value"), Some(&3));
        assert!(
            report
                .tally
                .by_label
                .keys()
                .any(|(rule, label)| *rule == "env-value" && label == "API_URL")
        );
        assert_eq!(first.size, out.len() as u64);
        Ok(())
    }

    #[test]
    fn clean_lines_are_copied_byte_for_byte_and_the_budget_is_enforced() -> TestResult {
        let dir = tempfile::tempdir()?;
        let source = dir.path().join("in.jsonl");
        let clean = "{\"b\":1,  \"a\":\"plain\"}\n";
        std::fs::write(&source, clean.repeat(3))?;
        let (staged, report) = stage_redacted(&source, &dir.path().join("o.jsonl"), "x", u64::MAX)?;
        assert_eq!(std::fs::read_to_string(dir.path().join("o.jsonl"))?, clean.repeat(3));
        assert!(report.tally.is_empty());
        assert_eq!(staged.size, (clean.len() * 3) as u64);
        let refused = stage_redacted(&source, &dir.path().join("p.jsonl"), "x", 30);
        assert_eq!(
            refused.err().map(|e| e.kind()),
            Some(io::ErrorKind::FileTooLarge)
        );
        Ok(())
    }

    #[test]
    fn long_lines_are_left_out() -> TestResult {
        let mut reader = io::Cursor::new(b"abcdef\nxy\n".to_vec());
        let mut buf = Vec::new();
        assert_eq!(next_line(&mut reader, &mut buf, 4)?, Some(false));
        assert_eq!(next_line(&mut reader, &mut buf, 4)?, Some(true));
        assert_eq!(buf, b"xy\n");
        assert_eq!(next_line(&mut reader, &mut buf, 4)?, None);
        Ok(())
    }
}
