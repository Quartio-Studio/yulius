//! Accessible text outline from `Accessibility.getFullAXTree` nodes, bounded in size.

use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;

use serde_json::Value;

/// Maximum nodes visited (cycle and size protection for hostile pages).
const MAX_NODES: usize = 100_000;
/// Maximum characters of a name or value on one line.
const MAX_FIELD_CHARS: usize = 200;
/// Indentation stops growing beyond this depth.
const MAX_INDENT_DEPTH: usize = 40;
/// Roles that carry no information on their own; their children are lifted.
const TRANSPARENT_ROLES: &[&str] = &["none", "generic", "InlineTextBox", "LineBreak", "Ignored"];
/// Boolean or level properties worth showing.
const SHOWN_PROPERTIES: &[&str] = &[
    "checked", "disabled", "expanded", "focused", "level", "pressed", "required", "selected",
];

/// Truncates `text` to at most `max` bytes on a character boundary.
pub(crate) fn truncate_utf8(text: &mut String, max: usize) -> bool {
    if text.len() <= max {
        return false;
    }
    let mut cut = max;
    while !text.is_char_boundary(cut) {
        cut -= 1;
    }
    text.truncate(cut);
    true
}

fn field(node: &Value, name: &str) -> String {
    let raw = node
        .get(name)
        .and_then(|v| v.get("value"))
        .map(|v| match v {
            Value::String(s) => s.clone(),
            Value::Null => String::new(),
            other => other.to_string(),
        })
        .unwrap_or_default();
    let mut flat: String = raw
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    if let Some((index, _)) = flat.char_indices().nth(MAX_FIELD_CHARS) {
        flat.truncate(index);
        flat.push('…');
    }
    flat
}

fn properties(node: &Value) -> String {
    let Some(list) = node.get("properties").and_then(Value::as_array) else {
        return String::new();
    };
    let mut shown = String::new();
    for property in list {
        let Some(name) = property.get("name").and_then(Value::as_str) else {
            continue;
        };
        if !SHOWN_PROPERTIES.contains(&name) {
            continue;
        }
        let value = property.get("value").and_then(|v| v.get("value"));
        match value {
            Some(Value::Bool(false)) | None => {}
            Some(Value::Bool(true)) => {
                let _ = write!(shown, " [{name}]");
            }
            Some(Value::String(s)) if s == "false" => {}
            Some(Value::String(s)) => {
                let _ = write!(shown, " [{name}={s}]");
            }
            Some(other) => {
                let _ = write!(shown, " [{name}={other}]");
            }
        }
    }
    shown
}

/// Renders an indented outline (`- role "name" value="…" [flags]`) of the AX tree, cut at
/// `max_bytes`. Returns the outline and whether it was truncated.
pub(crate) fn outline(nodes: &[Value], max_bytes: usize) -> (String, bool) {
    let by_id: HashMap<&str, &Value> = nodes
        .iter()
        .filter_map(|node| {
            node.get("nodeId")
                .and_then(Value::as_str)
                .map(|id| (id, node))
        })
        .collect();
    let root = nodes
        .iter()
        .find(|node| node.get("parentId").is_none())
        .or_else(|| nodes.first());
    let mut out = String::new();
    let Some(root) = root else {
        return (out, false);
    };
    let mut stack: Vec<(&Value, usize)> = vec![(root, 0)];
    let mut visited: HashSet<&str> = HashSet::new();
    let mut count = 0_usize;
    while let Some((node, depth)) = stack.pop() {
        count += 1;
        if count > MAX_NODES || out.len() > max_bytes {
            break;
        }
        if let Some(id) = node.get("nodeId").and_then(Value::as_str)
            && !visited.insert(id)
        {
            continue;
        }
        let ignored = node
            .get("ignored")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let role = field(node, "role");
        let name = field(node, "name");
        let transparent = ignored
            || role.is_empty()
            || (TRANSPARENT_ROLES.contains(&role.as_str()) && name.is_empty())
            || role == "InlineTextBox";
        let child_depth = if transparent {
            depth
        } else {
            let indent = "  ".repeat(depth.min(MAX_INDENT_DEPTH));
            let role = if role == "StaticText" {
                "text"
            } else {
                role.as_str()
            };
            let _ = write!(out, "{indent}- {role}");
            if !name.is_empty() {
                let _ = write!(out, " \"{name}\"");
            }
            let value = field(node, "value");
            if !value.is_empty() {
                let _ = write!(out, " value=\"{value}\"");
            }
            out.push_str(&properties(node));
            out.push('\n');
            depth + 1
        };
        if let Some(children) = node.get("childIds").and_then(Value::as_array) {
            for child in children.iter().rev() {
                if let Some(child) = child.as_str().and_then(|id| by_id.get(id)) {
                    stack.push((child, child_depth));
                }
            }
        }
    }
    let mut truncated = truncate_utf8(&mut out, max_bytes);
    if count > MAX_NODES {
        truncated = true;
    }
    (out, truncated)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn node(id: &str, parent: Option<&str>, role: &str, name: &str, children: &[&str]) -> Value {
        let mut value = json!({
            "nodeId": id, "ignored": false,
            "role": {"type": "role", "value": role},
            "name": {"type": "computedString", "value": name},
            "childIds": children,
        });
        if let Some(parent) = parent {
            value["parentId"] = json!(parent);
        }
        value
    }

    #[test]
    fn renders_roles_names_values_and_flags_with_lifted_generics() {
        let mut textbox = node("5", Some("3"), "textbox", "Email", &[]);
        textbox["value"] = json!({"type": "string", "value": "a@b.c"});
        textbox["properties"] = json!([
            {"name": "focused", "value": {"type": "boolean", "value": true}},
            {"name": "disabled", "value": {"type": "boolean", "value": false}},
        ]);
        let mut ignored = node("6", Some("3"), "generic", "", &["7"]);
        ignored["ignored"] = json!(true);
        let nodes = vec![
            node("1", None, "RootWebArea", "Login", &["2", "3"]),
            node("2", Some("1"), "heading", "Sign in\nnow", &[]),
            node("3", Some("1"), "generic", "", &["4", "5", "6"]),
            node("4", Some("3"), "button", "Go", &[]),
            textbox,
            ignored,
            node("7", Some("6"), "StaticText", "hello", &["8"]),
            node("8", Some("7"), "InlineTextBox", "hello", &[]),
        ];
        let (text, truncated) = outline(&nodes, 64 * 1024);
        assert!(!truncated);
        assert_eq!(
            text,
            "- RootWebArea \"Login\"\n  - heading \"Sign in now\"\n  - button \"Go\"\n  - textbox \"Email\" value=\"a@b.c\" [focused]\n  - text \"hello\"\n"
        );
    }

    #[test]
    fn truncates_on_char_boundary_and_survives_cycles() {
        let children: Vec<String> = (2..2000).map(|i| i.to_string()).collect();
        let child_refs: Vec<&str> = children.iter().map(String::as_str).collect();
        let mut nodes = vec![node("1", None, "RootWebArea", "ż", &child_refs)];
        for id in &children {
            nodes.push(node(id, Some("1"), "link", "żółć link text", &["1"]));
        }
        let (text, truncated) = outline(&nodes, 1000);
        assert!(truncated);
        assert!(text.len() <= 1000);
        assert!(text.starts_with("- RootWebArea \"ż\"\n  - link"));
    }

    #[test]
    fn empty_tree_gives_empty_outline() {
        assert_eq!(outline(&[], 10), (String::new(), false));
        let mut text = "ab".to_owned();
        assert!(!truncate_utf8(&mut text, 2));
        let mut text = "aż".to_owned();
        assert!(truncate_utf8(&mut text, 2));
        assert_eq!(text, "a");
    }
}
