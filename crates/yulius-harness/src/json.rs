//! Small helpers for reading untrusted provider JSON.

use serde_json::{Map, Value};

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

/// `text` cut to `max` bytes on a character boundary, with an ellipsis when cut.
pub(crate) fn shorten(text: &str, max: usize) -> String {
    let kept = truncate_utf8(text, max);
    if kept.len() == text.len() {
        text.to_owned()
    } else {
        format!("{kept}…")
    }
}

/// Longest prefix of `text` that is at most `max_bytes` long and ends on a character boundary.
pub(crate) fn truncate_utf8(text: &str, max_bytes: usize) -> &str {
    if text.len() <= max_bytes {
        return text;
    }
    let mut end = max_bytes;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shorten_respects_utf8() {
        assert_eq!(shorten("żółw", 3), "ż…");
        assert_eq!(shorten("abc", 3), "abc");
    }

    #[test]
    fn parse_object_rejects_non_objects() {
        assert!(parse_object("[1]").is_none());
        assert!(parse_object("{").is_none());
        assert!(parse_object(" {\"a\":1} ").is_some());
    }
}
