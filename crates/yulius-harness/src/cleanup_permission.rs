//! Local answer to a contained Claude task's variable-path removal prompt (SPEC-596).
//! A permission decision covers the whole Bash script. Return a checked retry to the
//! calling agent, which can then run the other commands under their usual permissions.

use std::io::{Read, Write};

use serde_json::{Value, json};
use tree_sitter::{Node, Parser};

use crate::hook_client::MAX_INPUT_BYTES;

pub(crate) fn answer(input: &mut dyn Read, output: &mut dyn Write) -> i32 {
    let mut body = Vec::new();
    if input
        .take(MAX_INPUT_BYTES as u64 + 1)
        .read_to_end(&mut body)
        .is_err()
        || body.len() > MAX_INPUT_BYTES
    {
        return 0;
    }
    let Some(retry) = retry_message(&body) else {
        return 0;
    };
    let reply = json!({"hookSpecificOutput": {
        "hookEventName": "PermissionRequest",
        "decision": {"behavior": "deny", "interrupt": false, "message": retry},
    }});
    let _ = serde_json::to_writer(&mut *output, &reply)
        .and_then(|()| output.flush().map_err(serde_json::Error::io));
    0
}

fn retry_message(body: &[u8]) -> Option<String> {
    let event: Value = serde_json::from_slice(body).ok()?;
    if event["hook_event_name"] != "PermissionRequest"
        || event["permission_mode"] != "bypassPermissions"
        || event["tool_name"] != "Bash"
        || !event["permission_suggestions"].as_array()?.is_empty()
    {
        return None;
    }
    let command = event["tool_input"]["command"].as_str()?;
    let mut parser = Parser::new();
    parser
        .set_language(&tree_sitter_bash::LANGUAGE.into())
        .ok()?;
    let tree = parser.parse(command, None)?;
    let root = tree.root_node();
    if root.has_error() {
        return None;
    }
    let defines_rm = statements(root).any(|node| {
        node.kind() == "function_definition"
            && node
                .child_by_field_name("name")
                .is_some_and(|name| &command[name.byte_range()] == "rm")
    });
    let retry = statements(root).find_map(|node| checked_removal(node, command, defines_rm))?;
    Some(format!(
        "The Bash script has not run. Resubmit it, replacing this removal in place with `{retry}`. \
         Preserve the conditions, loop, variable bindings and other commands. Check the cleanup \
         result before rebuilding a scratch copy."
    ))
}

// Visit executable statements in source order. Words, expansions, definitions and here-doc
// bodies are not executable statement containers. Bash marks an asynchronous list with "&".
fn statements(root: Node<'_>) -> impl Iterator<Item = Node<'_>> {
    let mut pending = vec![root];
    std::iter::from_fn(move || {
        let node = pending.pop()?;
        match node.kind() {
            "program"
            | "list"
            | "pipeline"
            | "for_statement"
            | "c_style_for_statement"
            | "while_statement"
            | "if_statement"
            | "elif_clause"
            | "else_clause"
            | "case_statement"
            | "case_item"
            | "do_group"
            | "compound_statement"
            | "subshell"
            | "negated_command" => {
                for index in (0..node.named_child_count()).rev() {
                    if let Some(child) = node.named_child(index)
                        && !child.next_sibling().is_some_and(|next| next.kind() == "&")
                    {
                        pending.push(child);
                    }
                }
            }
            "redirected_statement" => pending.extend(node.child_by_field_name("body")),
            _ => {}
        }
        Some(node)
    })
}

// Tree-sitter attaches a trailing redirect to the entire list or pipeline. In Bash it
// belongs to the last command of that list. A redirect on an enclosing group stays in place.
fn has_trailing_redirect(mut node: Node<'_>) -> bool {
    while let Some(parent) = node.parent() {
        if parent.kind() == "redirected_statement" {
            return parent.child_by_field_name("body") == Some(node);
        }
        if !matches!(parent.kind(), "list" | "pipeline" | "negated_command")
            || node.next_named_sibling().is_some()
        {
            break;
        }
        node = parent;
    }
    false
}

fn checked_removal(node: Node<'_>, source: &str, defines_rm: bool) -> Option<String> {
    if node.kind() != "command" || has_trailing_redirect(node) {
        return None;
    }
    let name = node.child_by_field_name("name")?;
    let program = &source[name.byte_range()];
    if !matches!(program, "rm" | "/bin/rm" | "/usr/bin/rm") || (program == "rm" && defines_rm) {
        return None;
    }
    let mut cursor = node.walk();
    // Only the executable and its operands may be replaced, never a redirect or assignment.
    if node.child_count() != 1 + node.children_by_field_name("argument", &mut cursor).count() {
        return None;
    }
    let mut words = node
        .children_by_field_name("argument", &mut cursor)
        .peekable();
    let mut recursive = false;
    let mut force = false;
    let mut end_options = false;
    while let Some(flag) = words
        .peek()
        .filter(|word| word.kind() == "word")
        .map(|word| &source[word.byte_range()])
        .filter(|s| s.starts_with('-'))
    {
        words.next();
        match flag {
            "--" => {
                end_options = true;
                break;
            }
            "--recursive" => recursive = true,
            "--force" => force = true,
            short if short.len() > 1 => {
                for c in short[1..].chars() {
                    match c {
                        'r' | 'R' => recursive = true,
                        'f' => force = true,
                        _ => return None,
                    }
                }
            }
            _ => return None,
        }
    }
    let mut retry = String::from("yulius remove --preserve-trailing-slash");
    if !end_options {
        retry.push_str(" --reject-option-operands");
    }
    if !recursive {
        retry.push_str(" --file-only");
    }
    if !force {
        retry.push_str(" --require-existing");
    }
    retry.push_str(" --");
    let mut relevant = false;
    for word in words {
        let (path, has_variable) = checked_path(word, source)?;
        if !end_options && path.starts_with('-') {
            return None;
        }
        relevant |= has_variable
            || path
                .strip_prefix('/')
                .is_some_and(|rest| !rest.trim_matches('/').contains('/'));
        retry.push_str(" \"");
        retry.push_str(&path);
        retry.push('"');
    }
    // A sourced function, alias or PATH entry is visible only in the actual shell.
    relevant.then(|| format!(
        r#"{{ [[ "$(builtin type -t {program})" = file && "$(builtin command -v {program})" -ef /bin/rm ]] && {retry}; }}"#
    ))
}

// Retain only ordinary path fragments and simple named variables. Never replay shell code
// from an expansion, glob pattern or quoted literal in the suggested replacement.
fn checked_path(node: Node<'_>, source: &str) -> Option<(String, bool)> {
    let text = &source[node.byte_range()];
    let literal = |text: &str| {
        text.chars()
            .all(|c| c.is_alphanumeric() || "/._- +".contains(c))
            .then(|| (text.to_owned(), false))
    };
    match node.kind() {
        "word" | "number" => {
            // A backslash in an unquoted Bash word quotes its next character.
            let mut word = String::new();
            let mut chars = text.chars();
            while let Some(c) = chars.next() {
                word.push(if c == '\\' { chars.next()? } else { c });
            }
            literal(&word)
        }
        "string_content" => literal(text),
        "raw_string" => literal(text.strip_prefix('\'')?.strip_suffix('\'')?),
        "simple_expansion" | "expansion" => {
            let name = node.named_child(0)?;
            if name.kind() != "variable_name" {
                return None;
            }
            let name = &source[name.byte_range()];
            if !crate::launch::shell_identifier(name) {
                return None;
            }
            if text != format!("${name}") && text != format!("${{{name}}}") {
                return None;
            }
            Some((format!("${{{name}:?}}"), true))
        }
        "string" | "concatenation" => {
            let mut path = String::new();
            let mut variable = false;
            let mut cursor = node.walk();
            // Some literal punctuation is anonymous in the grammar. Every input byte
            // between the quotes must belong to a fragment we actually rendered.
            let quotes = usize::from(node.kind() == "string");
            let mut offset = node.start_byte() + quotes;
            for part in node.named_children(&mut cursor) {
                if part.start_byte() != offset {
                    return None;
                }
                let (fragment, has_variable) = checked_path(part, source)?;
                path.push_str(&fragment);
                variable |= has_variable;
                offset = part.end_byte();
            }
            (offset == node.end_byte() - quotes).then_some((path, variable))
        }
        _ => None,
    }
}
