//! Local answer to a contained Claude task's variable-path removal prompt (SPEC-596).
//! A permission decision covers the whole Bash script. Return a checked retry to the
//! calling agent, which can then run the other commands under their usual permissions.

use std::io::{Read, Write};

use conch_parser::ast::{
    AndOr, Command, ComplexWord, CompoundCommandKind, DefaultPipeableCommand, DefaultSimpleWord,
    GuardBodyPair, ListableCommand, Parameter, PipeableCommand, RedirectOrCmdWord, SimpleWord,
    TopLevelCommand, TopLevelWord, Word,
};
use conch_parser::{lexer::Lexer, parse::DefaultParser};
use serde_json::{Value, json};

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
    let commands = DefaultParser::new(Lexer::new(command.chars()))
        .into_iter()
        .collect::<Result<Vec<_>, _>>()
        .ok()?;
    let defines_rm = commands.iter().any(|command| {
        find_command(command, &|node| {
            matches!(node, PipeableCommand::FunctionDef(name, _) if name == "rm").then_some(())
        })
        .is_some()
    });
    let retry = commands
        .iter()
        .find_map(|command| find_command(command, &|node| checked_removal(node, defines_rm)))?;
    Some(format!(
        "The Bash script has not run. Resubmit it, replacing this removal in place with `{retry}`. \
         Preserve the conditions, loop, variable bindings and other commands. Check the cleanup \
         result before rebuilding a scratch copy."
    ))
}

fn find_command<T>(
    command: &TopLevelCommand<String>,
    visit: &impl Fn(&DefaultPipeableCommand) -> Option<T>,
) -> Option<T> {
    // The POSIX parser also represents Bash's combined output redirect as a job boundary.
    let Command::List(list) = &command.0 else {
        return None;
    };
    std::iter::once(&list.first)
        .chain(list.rest.iter().map(|op| match op {
            AndOr::And(c) | AndOr::Or(c) => c,
        }))
        .find_map(|item| match item {
            ListableCommand::Single(cmd) => find_pipe(cmd, visit),
            ListableCommand::Pipe(_, cmds) => cmds.iter().find_map(|cmd| find_pipe(cmd, visit)),
        })
}

fn find_pipe<T>(
    cmd: &DefaultPipeableCommand,
    visit: &impl Fn(&DefaultPipeableCommand) -> Option<T>,
) -> Option<T> {
    let PipeableCommand::Compound(compound) = cmd else {
        return visit(cmd);
    };
    let search = |command: &TopLevelCommand<String>| find_command(command, visit);
    let guard = |pair: &GuardBodyPair<TopLevelCommand<String>>| {
        pair.guard.iter().chain(&pair.body).find_map(search)
    };
    match &compound.kind {
        CompoundCommandKind::For { body, .. }
        | CompoundCommandKind::Brace(body)
        | CompoundCommandKind::Subshell(body) => body.iter().find_map(search),
        CompoundCommandKind::While(pair) | CompoundCommandKind::Until(pair) => guard(pair),
        CompoundCommandKind::If {
            conditionals,
            else_branch,
        } => conditionals
            .iter()
            .find_map(guard)
            .or_else(|| else_branch.as_ref()?.iter().find_map(search)),
        CompoundCommandKind::Case { arms, .. } => {
            arms.iter().flat_map(|arm| &arm.body).find_map(search)
        }
    }
}

fn checked_removal(cmd: &DefaultPipeableCommand, defines_rm: bool) -> Option<String> {
    let PipeableCommand::Simple(simple) = cmd else {
        return None;
    };
    // A replacement made only of the command and its operands would lose these effects.
    if !simple.redirects_or_env_vars.is_empty() {
        return None;
    }
    let mut words = simple
        .redirects_or_cmd_words
        .iter()
        .map(|item| match item {
            RedirectOrCmdWord::CmdWord(word) => Some(word),
            RedirectOrCmdWord::Redirect(_) => None,
        })
        .collect::<Option<Vec<_>>>()?
        .into_iter()
        .peekable();
    let program = literal(words.next()?)?;
    if !matches!(program, "rm" | "/bin/rm" | "/usr/bin/rm") || (program == "rm" && defines_rm) {
        return None;
    }
    let mut recursive = false;
    let mut force = false;
    let mut end_options = false;
    while let Some(flag) = words
        .peek()
        .and_then(|word| literal(word))
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
        let (path, has_variable) = checked_path(word)?;
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

fn literal(word: &TopLevelWord<String>) -> Option<&str> {
    match &word.0 {
        ComplexWord::Single(Word::Simple(SimpleWord::Literal(text))) => Some(text),
        _ => None,
    }
}

// Render only ordinary pathname fragments and named variables. Substitutions, globs and other
// shell expressions remain Claude's decision. Literal text must stay literal inside the quotes.
fn checked_path(word: &TopLevelWord<String>) -> Option<(String, bool)> {
    let parts = match &word.0 {
        ComplexWord::Single(part) => std::slice::from_ref(part),
        ComplexWord::Concat(parts) => parts.as_slice(),
    };
    let mut path = String::new();
    let mut has_variable = false;
    let literal_part = |text: &str, path: &mut String| -> Option<()> {
        if !text
            .chars()
            .all(|c| c.is_alphanumeric() || "/._- +".contains(c))
        {
            return None;
        }
        path.push_str(text);
        Some(())
    };
    for part in parts {
        let simple_parts = match part {
            Word::Simple(part) => std::slice::from_ref(part),
            Word::DoubleQuoted(parts) => parts.as_slice(),
            Word::SingleQuoted(text) => {
                literal_part(text, &mut path)?;
                continue;
            }
        };
        for part in simple_parts {
            match part {
                SimpleWord::Literal(text) | SimpleWord::Escaped(text) => {
                    literal_part(text, &mut path)?
                }
                DefaultSimpleWord::Param(Parameter::Var(name)) => {
                    has_variable = true;
                    path.push_str(&format!("${{{name}:?}}"));
                }
                _ => return None,
            }
        }
    }
    Some((path, has_variable))
}
