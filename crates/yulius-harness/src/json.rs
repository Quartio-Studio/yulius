//! Small helpers for reading untrusted provider JSON.

use serde_json::{Map, Value};
use yulius_domain::time::Timestamp;
use yulius_domain::transcript::truncate_utf8;

/// Maximum bytes of a tool input or result summary kept while reading a transcript.
pub(crate) const SUMMARY_READ_BYTES: usize = 16 * 1024;

/// Parses one JSONL line that must be a JSON object.
pub(crate) fn parse_object(line: &str) -> Option<Map<String, Value>> {
    match serde_json::from_str::<Value>(line.trim()) {
        Ok(Value::Object(map)) => Some(map),
        _ => None,
    }
}

pub(crate) fn str_of<'a>(map: &'a Map<String, Value>, key: &str) -> Option<&'a str> {
    map.get(key).and_then(Value::as_str)
}

pub(crate) fn obj_of<'a>(map: &'a Map<String, Value>, key: &str) -> Option<&'a Map<String, Value>> {
    map.get(key).and_then(Value::as_object)
}

pub(crate) fn u64_of(map: &Map<String, Value>, key: &str) -> Option<u64> {
    map.get(key).and_then(Value::as_u64)
}

pub(crate) fn bool_of(map: &Map<String, Value>, key: &str) -> bool {
    map.get(key).and_then(Value::as_bool).unwrap_or(false)
}

pub(crate) fn owned_str(map: &Map<String, Value>, key: &str) -> Option<String> {
    str_of(map, key).map(str::to_owned)
}

pub(crate) fn timestamp_of(map: &Map<String, Value>, key: &str) -> Option<Timestamp> {
    str_of(map, key).and_then(crate::time::parse_rfc3339)
}

/// `text` cut to `max` bytes on a character boundary, with an ellipsis when cut.
pub(crate) fn shorten(text: &str, max: usize) -> String {
    let kept = truncate_utf8(text, max);
    if kept.len() == text.len() {
        text.to_owned()
    } else {
        format!("{kept}…")
    }
}

/// Compact JSON text of a value, shortened.
pub(crate) fn compact(value: &Value, max: usize) -> String {
    let text = match value {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    };
    shorten(&text, max)
}

/// TOML basic string literal (for Codex `-c key=value` overrides).
pub(crate) fn toml_string(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for c in text.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            '\r' => out.push_str("\\r"),
            c if c.is_control() => out.push_str(&format!("\\u{:04X}", u32::from(c))),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// TOML array of basic strings.
pub(crate) fn toml_string_array<S: AsRef<str>>(items: &[S]) -> String {
    let inner: Vec<String> = items.iter().map(|s| toml_string(s.as_ref())).collect();
    format!("[{}]", inner.join(","))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn toml_escaping() {
        assert_eq!(toml_string("plain"), "\"plain\"");
        assert_eq!(toml_string("a\"b\\c\nd\u{1}"), "\"a\\\"b\\\\c\\nd\\u0001\"");
        assert_eq!(toml_string_array(&["-y", "x y"]), "[\"-y\",\"x y\"]");
        assert_eq!(toml_string_array::<&str>(&[]), "[]");
    }

    #[test]
    fn shorten_respects_utf8() {
        assert_eq!(shorten("żółw", 3), "ż…");
        assert_eq!(shorten("abc", 3), "abc");
        assert_eq!(compact(&serde_json::json!({"a":1}), 100), "{\"a\":1}");
    }

    #[test]
    fn parse_object_rejects_non_objects() {
        assert!(parse_object("[1]").is_none());
        assert!(parse_object("{").is_none());
        assert!(parse_object(" {\"a\":1} ").is_some());
    }
}
