//! `yulius hook claude PreToolUse`, inside the sandbox: refuses a Bash command that can never
//! work as meant, before it runs (F 2026-10-12, complaint 1: dead monitors and shells). The edit
//! tools' calls ([`EDIT_TOOLS`]) are not judged here: the hook client sends them to Yulius's
//! edit gate (SPEC-426).
//!
//! Claude Code runs a Bash command as `bash -c '<command>'`, so `pgrep -f P` and `pkill -f P` in
//! it see their own shell, whose command line contains `P`: a wait on `pgrep -f` never ends (the
//! task looks busy for hours) and `pkill -f` kills the agent's own shell. Every task's transcript
//! of 2026-10-11/12 had such loops. A pattern anchored with `^` or using the bracket trick
//! (`[s]cripts/dev ci`) does not match itself and passes.
//!
//! A merge by hand (`gh pr merge`, `gh api -X PUT …/pulls/N/merge`) is refused too, with why
//! (SPEC-444 PRN-011): Yulius merges through `delivery_merge` and its queue.

use std::io::{Read, Write};

use serde_json::Value;
pub use yulius_contracts::hooks::EDIT_TOOLS;
use yulius_contracts::hooks::edited_file_field;

/// Largest hook input read.
const MAX_INPUT_BYTES: u64 = 1024 * 1024;

/// The request Yulius's edit gate gets for the `PreToolUse` input `body` of an edit tool
/// ([`EDIT_TOOLS`]): only the event, the tool, the agent's folder and the edited path, never the
/// file's content (SPEC-426 EDG-001). `None` for any other tool or input.
#[must_use]
pub fn edit_request(body: &[u8]) -> Option<Vec<u8>> {
    let event = serde_json::from_slice::<Value>(body).ok()?;
    let tool = event["tool_name"].as_str()?;
    let field = edited_file_field(tool)?;
    let mut request = serde_json::Map::new();
    for key in ["hook_event_name", "tool_name", "cwd"] {
        if let Some(value) = event.get(key).filter(|v| v.is_string()) {
            request.insert(key.to_owned(), value.clone());
        }
    }
    if let Some(path) = event["tool_input"].get(field).filter(|v| v.is_string()) {
        request.insert(
            "tool_input".to_owned(),
            serde_json::json!({ field: path.clone() }),
        );
    }
    serde_json::to_vec(&Value::Object(request)).ok()
}

/// Why `command` would match itself: the first `pgrep -f`/`pkill -f` pattern that its own
/// `bash -c` command line contains, or `None`.
#[must_use]
pub fn self_matching_pattern(command: &str) -> Option<(String, String)> {
    let words = split_words(command);
    for (index, word) in words.iter().enumerate() {
        let tool = word.rsplit('/').next().unwrap_or(word);
        if tool != "pgrep" && tool != "pkill" {
            continue;
        }
        let mut full = false;
        let mut pattern = None;
        for arg in words.iter().skip(index + 1) {
            if is_separator(arg) {
                break;
            }
            if let Some(flags) = arg.strip_prefix('-').filter(|f| !f.starts_with('-')) {
                // Short options; options with a value (-u name) are rare in waits and harmless here.
                full |= flags.contains('f');
                continue;
            }
            if arg == "--full" {
                full = true;
                continue;
            }
            if arg.starts_with("--") {
                continue;
            }
            pattern = Some(arg.clone());
            break;
        }
        if let (true, Some(pattern)) = (full, pattern)
            && !pattern.starts_with('^')
            && !pattern.contains('[')
            && !pattern.is_empty()
        {
            return Some((tool.to_owned(), pattern));
        }
    }
    None
}

/// Whether `command` merges a pull request by hand (SPEC-444 PRN-011): `gh pr merge`, or
/// `gh api` with method `PUT` on a pull request's `/merge`. Yulius merges through
/// `delivery_merge` and its queue; a hand merge does not end the task's delivery.
#[must_use]
pub fn hand_merge(command: &str) -> bool {
    let words = split_words(command);
    words.iter().enumerate().any(|(index, word)| {
        if word.rsplit('/').next() != Some("gh") {
            return false;
        }
        let args: Vec<&str> = words
            .iter()
            .skip(index + 1)
            .take_while(|arg| !is_separator(arg))
            .map(String::as_str)
            .collect();
        // The subcommand words, without options and their values (`-R owner/repo`).
        let mut plain = Vec::new();
        let mut skip_value = false;
        for arg in &args {
            if std::mem::take(&mut skip_value) {
                continue;
            }
            if arg.starts_with('-') {
                skip_value = matches!(*arg, "-R" | "--repo" | "-X" | "--method");
                continue;
            }
            plain.push(*arg);
        }
        match plain.as_slice() {
            ["pr", "merge", ..] => true,
            ["api", path, ..] => {
                let merge_path =
                    path.trim_end_matches('/').ends_with("/merge") && path.contains("pulls/");
                let put = args.windows(2).any(|pair| {
                    matches!(pair[0], "-X" | "--method") && pair[1].eq_ignore_ascii_case("PUT")
                }) || args.iter().any(|arg| {
                    arg.strip_prefix("--method=")
                        .or_else(|| arg.strip_prefix("-X"))
                        .is_some_and(|method| method.eq_ignore_ascii_case("PUT"))
                });
                merge_path && put
            }
            _ => false,
        }
    })
}

fn is_separator(word: &str) -> bool {
    matches!(
        word,
        ";" | "&&" | "||" | "|" | "&" | ")" | "do" | "then" | "done" | "fi"
    )
}

/// Shell-ish words: quotes group, `;`, `&&`, `||`, `|`, `&` and `)` separate (good enough to
/// find a `pgrep` and its pattern; nothing is executed).
fn split_words(command: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut word = String::new();
    let mut quote: Option<char> = None;
    let mut chars = command.chars().peekable();
    let push = |word: &mut String, words: &mut Vec<String>| {
        if !word.is_empty() {
            words.push(std::mem::take(word));
        }
    };
    while let Some(c) = chars.next() {
        match quote {
            Some(q) if c == q => quote = None,
            Some(_) => word.push(c),
            None => match c {
                '\'' | '"' => quote = Some(c),
                ' ' | '\t' | '\n' => push(&mut word, &mut words),
                ';' | '|' | '&' | ')' | '(' => {
                    push(&mut word, &mut words);
                    let mut op = c.to_string();
                    if (c == '|' || c == '&') && chars.peek() == Some(&c) {
                        op.push(c);
                        chars.next();
                    }
                    if c != '(' {
                        words.push(op);
                    }
                }
                _ => word.push(c),
            },
        }
    }
    push(&mut word, &mut words);
    words
}

/// The hook: denies a Bash command whose `pgrep -f`/`pkill -f` matches itself, with the way to
/// wait instead, and a merge by hand (SPEC-444 PRN-011), with why; prints nothing otherwise.
/// Always exits 0.
pub fn pre_tool_use(input: &mut dyn Read, output: &mut dyn Write) -> i32 {
    let mut body = Vec::new();
    if input.take(MAX_INPUT_BYTES).read_to_end(&mut body).is_err() {
        return 0;
    }
    let Ok(event) = serde_json::from_slice::<Value>(&body) else {
        return 0;
    };
    if event["tool_name"].as_str() != Some("Bash") {
        return 0;
    }
    let Some(command) = event["tool_input"]["command"].as_str() else {
        return 0;
    };
    let reason = if hand_merge(command) {
        "Yulius refused this command: Merging by hand is refused. Yulius merges your pull request \
         through `delivery_merge` and its merge queue, in order, on green Yulius CI; a hand merge \
         skips that and does not end your goal's delivery. If a green pull request is not \
         merging, `delivery_status` says why, and Yulius tells this session when it moves: \
         arm it once with `delivery_merge` and end your turn."
            .to_owned()
    } else if let Some((tool, pattern)) = self_matching_pattern(command) {
        format!(
            "Yulius refused this command: `{tool} -f '{pattern}'` runs inside `bash -c`, whose \
             own command line contains '{pattern}', so it always finds itself (a wait on it \
             never ends, and pkill kills this shell). Wait on a PID instead (`cmd & pid=$!; wait \
             $pid`), anchor the pattern (`^…`) or use the bracket trick (`[x]yz`); for checks or \
             a merge, start `yulius wait` with run_in_background and end your turn."
        )
    } else {
        return 0;
    };
    let answer = serde_json::json!({
        "hookSpecificOutput": {
            "hookEventName": "PreToolUse",
            "permissionDecision": "deny",
            "permissionDecisionReason": reason,
        }
    });
    let _ignored = serde_json::to_writer(&mut *output, &answer)
        .and_then(|()| output.flush().map_err(serde_json::Error::io));
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn waits_and_kills_that_match_themselves_are_found() {
        for (command, pattern) in [
            (
                r#"until ! pgrep -f "scripts/dev ci"; do sleep 10; done"#,
                "scripts/dev ci",
            ),
            (
                "while pgrep -f 'xtask ci' >/dev/null; do sleep 5; done",
                "xtask ci",
            ),
            ("pkill -f cargo-nextest", "cargo-nextest"),
            (
                "sleep 2 && /usr/bin/pgrep -af 'e2e-web install'",
                "e2e-web install",
            ),
            ("pgrep --full 'npm ci'", "npm ci"),
        ] {
            assert_eq!(
                self_matching_pattern(command).map(|(_, p)| p),
                Some(pattern.to_owned()),
                "{command}"
            );
        }
    }

    #[test]
    fn safe_process_checks_pass() {
        for command in [
            "pgrep cargo",
            "pgrep -x cargo",
            "pgrep -f '[s]cripts/dev ci'",
            "pgrep -f '^/usr/bin/cargo test'",
            "cargo test -p pgrep-like",
            "echo 'pgrep -f x'",
            "ps aux | grep cargo",
        ] {
            assert_eq!(self_matching_pattern(command), None, "{command}");
        }
    }

    /// SPEC-444 PRN-011 (quartio-v2 #116, #96 planned it; #111's goal got stuck for good): a
    /// merge by hand is refused with why, other `gh` calls are not.
    #[test]
    fn hand_merges_are_refused() {
        for command in [
            "gh pr merge 12 --squash",
            "gh pr merge --squash --match-head-commit abc 12",
            "cd /w && /usr/bin/gh -R acme/app pr merge 12",
            "gh api -X PUT repos/acme/app/pulls/12/merge -f merge_method=squash",
            "gh api --method=PUT /repos/acme/app/pulls/12/merge",
        ] {
            assert!(hand_merge(command), "{command}");
        }
        for command in [
            "gh pr view 12 --json mergeable",
            "gh pr checks 12",
            "echo 'gh pr merge 12'",
            "gh api repos/acme/app/pulls/12/merge",
            "git merge --no-edit origin/dev",
            "gh pr list --search merge",
        ] {
            assert!(!hand_merge(command), "{command}");
        }
        let mut out = Vec::new();
        let event = serde_json::json!({
            "tool_name": "Bash",
            "tool_input": {"command": "gh pr merge 12 --squash"}
        });
        pre_tool_use(&mut event.to_string().as_bytes(), &mut out);
        let denied = String::from_utf8(out).unwrap_or_default();
        assert!(
            denied.contains("\"permissionDecision\":\"deny\"")
                && denied.contains("Merging by hand is refused")
                && denied.contains("delivery_merge")
                && denied.contains("delivery_status"),
            "{denied}"
        );
    }

    #[test]
    fn the_hook_denies_only_such_bash_commands() {
        let run = |event: Value| {
            let mut out = Vec::new();
            pre_tool_use(&mut event.to_string().as_bytes(), &mut out);
            String::from_utf8(out).unwrap_or_default()
        };
        let denied = run(serde_json::json!({
            "tool_name": "Bash",
            "tool_input": {"command": "until ! pgrep -f 'scripts/dev ci'; do sleep 10; done"}
        }));
        assert!(
            denied.contains("\"permissionDecision\":\"deny\""),
            "{denied}"
        );
        assert!(denied.contains("yulius wait"), "{denied}");
        assert!(
            run(serde_json::json!({"tool_name": "Bash", "tool_input": {"command": "cargo test"}}))
                .is_empty()
        );
        assert!(
            run(serde_json::json!({"tool_name": "Read", "tool_input": {"file_path": "/x"}}))
                .is_empty()
        );
        assert!(run(serde_json::json!("not an event")).is_empty());
    }
}
