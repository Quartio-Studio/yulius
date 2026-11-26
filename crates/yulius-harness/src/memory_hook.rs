//! `yulius hook <claude|codex> MemoryGuard`, in the sandbox (SPEC-652 MEM-006): tells the agent
//! in one sentence why a write of its harness's memory or user-level instructions fails and where
//! lasting knowledge goes. The namespace refuses the write whatever this says; this only explains
//! it. It never asks Yulius, and a failure lets the agent go on.
//!
//! - `PreToolUse` of an edit tool (`yulius_contracts::hooks::EDIT_TOOLS`) on a read-only path: denied with the sentence,
//!   before the tool runs.
//! - `PostToolUse` or `PostToolUseFailure` (a failed tool, Claude Code 2.1.332) whose result says
//!   "Read-only file system" or "Device or resource busy" and names a read-only path (in the
//!   result or the tool's input): the sentence as added context.
//!
//! The read-only paths come from [`READONLY_PATHS_ENV`], one per line, which the launch sets.

use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};

use serde_json::{Value, json};
use yulius_contracts::hooks::{MEMORY_READ_ONLY_SENTENCE, READONLY_PATHS_ENV, edited_file_field};

/// Largest hook input read.
const MAX_INPUT_BYTES: u64 = 4 * 1024 * 1024;

/// What a refused write's error says in a tool result.
const REFUSED: [&str; 3] = [
    "Read-only file system",
    "Device or resource busy",
    "os error 30",
];

/// `path` made absolute against `cwd` with `.` and `..` resolved lexically.
fn absolute(path: &str, cwd: Option<&str>) -> Option<PathBuf> {
    let joined = match cwd {
        Some(cwd) if !Path::new(path).is_absolute() => Path::new(cwd).join(path),
        _ => PathBuf::from(path),
    };
    let mut out = PathBuf::new();
    for component in joined.components() {
        match component {
            Component::RootDir => out.push("/"),
            Component::Normal(part) => out.push(part),
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            Component::Prefix(_) => return None,
        }
    }
    out.is_absolute().then_some(out)
}

/// The answer to the hook input `event` for a session whose read-only paths are `guarded`;
/// `None` for nothing to say.
#[must_use]
pub fn answer(event: &Value, guarded: &[PathBuf]) -> Option<Value> {
    if guarded.is_empty() {
        return None;
    }
    let tool = event["tool_name"].as_str()?;
    match event["hook_event_name"].as_str()? {
        "PreToolUse" => {
            let field = edited_file_field(tool)?;
            let path = absolute(event["tool_input"][field].as_str()?, event["cwd"].as_str())?;
            guarded.iter().any(|g| path.starts_with(g)).then(|| {
                json!({ "hookSpecificOutput": {
                        "hookEventName": "PreToolUse",
                        "permissionDecision": "deny",
                        "permissionDecisionReason": MEMORY_READ_ONLY_SENTENCE,
                    } })
            })
        }
        name @ ("PostToolUse" | "PostToolUseFailure") => {
            // A failed tool's text is its `error` (Claude Code's `PostToolUseFailure`, 2.1.332).
            let result = format!("{}{}", event["tool_response"], event["error"]);
            let input = event["tool_input"].to_string();
            let refused = REFUSED.iter().any(|r| result.contains(r));
            // Named in the result or the input, or the tool ran inside a read-only folder
            // (a relative path; review of part 2).
            let cwd = event["cwd"].as_str().and_then(|cwd| absolute(cwd, None));
            let named = guarded.iter().any(|g| {
                let text = g.to_string_lossy();
                result.contains(text.as_ref())
                    || input.contains(text.as_ref())
                    || cwd.as_ref().is_some_and(|cwd| cwd.starts_with(g))
            });
            (refused && named).then(|| {
                json!({ "hookSpecificOutput": {
                    "hookEventName": name,
                    "additionalContext": MEMORY_READ_ONLY_SENTENCE,
                } })
            })
        }
        _ => None,
    }
}

/// The read-only paths of [`READONLY_PATHS_ENV`].
#[must_use]
pub fn guarded_paths(env: &dyn Fn(&str) -> Option<String>) -> Vec<PathBuf> {
    env(READONLY_PATHS_ENV)
        .unwrap_or_default()
        .lines()
        .map(str::trim)
        .filter(|line| Path::new(line).is_absolute())
        .map(PathBuf::from)
        .collect()
}

/// The hook: reads the input, prints the answer when there is one. Always exits 0.
pub fn run(
    input: &mut dyn Read,
    output: &mut dyn Write,
    env: &dyn Fn(&str) -> Option<String>,
) -> i32 {
    let mut body = Vec::new();
    if input.take(MAX_INPUT_BYTES).read_to_end(&mut body).is_err() {
        return 0;
    }
    let Ok(event) = serde_json::from_slice::<Value>(&body) else {
        return 0;
    };
    if let Some(reply) = answer(&event, &guarded_paths(env)) {
        let _ignored = serde_json::to_writer(&mut *output, &reply)
            .and_then(|()| output.flush().map_err(serde_json::Error::io));
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    const MEMORY: &str = "/s/home/.claude/projects/-s-repo/memory";

    fn guarded() -> Vec<PathBuf> {
        vec![
            PathBuf::from(MEMORY),
            PathBuf::from("/s/home/.claude/CLAUDE.md"),
        ]
    }

    /// MEM-006: an edit tool's write to a read-only path, by absolute or relative path, is denied
    /// with the sentence; one elsewhere, a read and a session without read-only paths get nothing.
    #[test]
    fn an_edit_of_memory_is_denied_with_the_sentence() {
        for (path, cwd) in [
            (format!("{MEMORY}/MEMORY.md"), "/s/worktrees/t1"),
            ("../../home/.claude/CLAUDE.md".to_owned(), "/s/worktrees/t1"),
            (format!("{MEMORY}/../memory/new.md"), "/"),
        ] {
            let event = json!({
                "hook_event_name": "PreToolUse", "tool_name": "Write", "cwd": cwd,
                "tool_input": { "file_path": path, "content": "a note" },
            });
            let reply = answer(&event, &guarded()).unwrap_or_default();
            assert_eq!(
                reply["hookSpecificOutput"]["permissionDecision"], "deny",
                "{path}"
            );
            assert_eq!(
                reply["hookSpecificOutput"]["permissionDecisionReason"],
                MEMORY_READ_ONLY_SENTENCE
            );
        }
        let elsewhere = json!({
            "hook_event_name": "PreToolUse", "tool_name": "Edit", "cwd": "/s/worktrees/t1",
            "tool_input": { "file_path": "/s/worktrees/t1/CLAUDE.md" },
        });
        assert_eq!(answer(&elsewhere, &guarded()), None);
        let read = json!({
            "hook_event_name": "PreToolUse", "tool_name": "Read",
            "tool_input": { "file_path": format!("{MEMORY}/MEMORY.md") },
        });
        assert_eq!(answer(&read, &guarded()), None);
        let write = json!({
            "hook_event_name": "PreToolUse", "tool_name": "Write",
            "tool_input": { "file_path": format!("{MEMORY}/MEMORY.md") },
        });
        assert_eq!(answer(&write, &[]), None);
    }

    /// MEM-006: a shell result refused on a read-only path gets the sentence as added context
    /// (Claude Code's `Bash`, Codex's shell); other failures and other paths get nothing.
    #[test]
    fn a_refused_shell_write_gets_the_sentence() {
        let refused = json!({
            "hook_event_name": "PostToolUse", "tool_name": "Bash",
            "tool_input": { "command": format!("echo note >> {MEMORY}/MEMORY.md") },
            "tool_response": { "stdout": "", "stderr":
                format!("bash: {MEMORY}/MEMORY.md: Read-only file system") },
        });
        let reply = answer(&refused, &guarded()).unwrap_or_default();
        assert_eq!(
            reply["hookSpecificOutput"]["additionalContext"],
            MEMORY_READ_ONLY_SENTENCE
        );
        assert_eq!(reply["hookSpecificOutput"]["hookEventName"], "PostToolUse");
        // A rename refused as a mount point, named only in the command.
        let busy = json!({
            "hook_event_name": "PostToolUse", "tool_name": "Bash",
            "tool_input": { "command": format!("mv {MEMORY} /tmp/m") },
            "tool_response": "mv: cannot move: Device or resource busy",
        });
        assert!(answer(&busy, &guarded()).is_some());
        // A failed command is Claude Code's `PostToolUseFailure` (2.1.332), its text in `error`.
        let failed = json!({
            "hook_event_name": "PostToolUseFailure", "tool_name": "Bash",
            "tool_input": { "command": format!("echo note >> {MEMORY}/MEMORY.md") },
            "error": format!("Exit code 1\n/bin/bash: {MEMORY}/MEMORY.md: Read-only file system"),
        });
        let reply = answer(&failed, &guarded()).unwrap_or_default();
        assert_eq!(
            reply["hookSpecificOutput"]["hookEventName"],
            "PostToolUseFailure"
        );
        assert_eq!(
            reply["hookSpecificOutput"]["additionalContext"],
            MEMORY_READ_ONLY_SENTENCE
        );
        // A relative path from a working folder inside the memory folder (review of part 2).
        let relative = json!({
            "hook_event_name": "PostToolUseFailure", "tool_name": "Bash", "cwd": MEMORY,
            "tool_input": { "command": "echo x >> MEMORY.md" },
            "error": "Exit code 1\n/bin/bash: MEMORY.md: Read-only file system",
        });
        assert!(answer(&relative, &guarded()).is_some());
        let other_path = json!({
            "hook_event_name": "PostToolUse", "tool_name": "Bash",
            "tool_input": { "command": "touch /usr/x" },
            "tool_response": { "stderr": "touch: cannot touch '/usr/x': Read-only file system" },
        });
        assert_eq!(answer(&other_path, &guarded()), None);
        let fine = json!({
            "hook_event_name": "PostToolUse", "tool_name": "Bash",
            "tool_input": { "command": format!("cat {MEMORY}/MEMORY.md") },
            "tool_response": { "stdout": "a note" },
        });
        assert_eq!(answer(&fine, &guarded()), None);
    }

    #[test]
    fn the_paths_come_from_the_run_variable() {
        let env = |name: &str| {
            (name == READONLY_PATHS_ENV)
                .then(|| format!("{MEMORY}\nrelative\n\n/s/home/.claude/rules"))
        };
        assert_eq!(
            guarded_paths(&env),
            [
                PathBuf::from(MEMORY),
                PathBuf::from("/s/home/.claude/rules")
            ]
        );
        let mut out = Vec::new();
        let input = json!({
            "hook_event_name": "PreToolUse", "tool_name": "Write",
            "tool_input": { "file_path": format!("{MEMORY}/x.md") },
        })
        .to_string();
        assert_eq!(run(&mut input.as_bytes(), &mut out, &env), 0);
        let reply: Value = serde_json::from_slice(&out).unwrap_or_default();
        assert_eq!(reply["hookSpecificOutput"]["permissionDecision"], "deny");
        let mut quiet = Vec::new();
        run(&mut input.as_bytes(), &mut quiet, &|_| None);
        assert!(quiet.is_empty());
    }
}
