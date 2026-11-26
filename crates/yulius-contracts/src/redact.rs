//! The JSON walk every redactor shares: transcripts, imports, MCP results and the sandbox's
//! scrub client.

use serde_json::Value;

/// Calls `f` on every string inside `value`, with the key of the object entry that holds it
/// (`None` in an array or at the top). Returns whether any call reported a change.
pub fn redact_json_strings(
    value: &mut Value,
    f: &mut dyn FnMut(Option<&str>, &mut String) -> bool,
) -> bool {
    walk(value, None, f)
}

fn walk(
    value: &mut Value,
    key: Option<&str>,
    f: &mut dyn FnMut(Option<&str>, &mut String) -> bool,
) -> bool {
    match value {
        Value::String(text) => f(key, text),
        Value::Array(items) => items
            .iter_mut()
            .fold(false, |changed, item| walk(item, None, f) | changed),
        Value::Object(map) => map.iter_mut().fold(false, |changed, (key, item)| {
            walk(item, Some(key), f) | changed
        }),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::redact_json_strings;

    #[test]
    fn every_string_is_visited_once_with_its_key() {
        let mut value = json!({"a": "x", "b": ["y", {"c": "z"}], "n": 1, "t": true});
        let mut seen = Vec::new();
        let changed = redact_json_strings(&mut value, &mut |key, text| {
            seen.push((key.map(str::to_owned), text.clone()));
            text.make_ascii_uppercase();
            text == "Y"
        });
        seen.sort();
        assert_eq!(
            seen,
            [
                (None, "y".to_owned()),
                (Some("a".to_owned()), "x".to_owned()),
                (Some("c".to_owned()), "z".to_owned()),
            ]
        );
        assert!(changed);
        assert_eq!(
            value,
            json!({"a": "X", "b": ["Y", {"c": "Z"}], "n": 1, "t": true})
        );
    }

    /// A change early in an object or array does not stop the walk: every later sibling is still
    /// visited, in order.
    #[test]
    fn a_reported_change_never_cuts_the_walk_short() {
        let mut value = json!({"a": "x", "b": ["y", "v"], "d": "w"});
        let mut seen = Vec::new();
        let changed = redact_json_strings(&mut value, &mut |_, text| {
            seen.push(text.clone());
            text.make_ascii_uppercase();
            true
        });
        assert!(changed);
        assert_eq!(seen, ["x", "y", "v", "w"]);
        assert_eq!(value, json!({"a": "X", "b": ["Y", "V"], "d": "W"}));
    }

    #[test]
    fn nothing_changed_reports_false() {
        let mut value = json!(["a", {"b": "c"}]);
        assert!(!redact_json_strings(&mut value, &mut |_, _| false));
        let mut top = json!("a");
        assert!(redact_json_strings(&mut top, &mut |key, _| key.is_none()));
    }
}
