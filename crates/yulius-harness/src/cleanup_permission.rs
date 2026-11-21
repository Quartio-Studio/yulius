//! Local answer to a contained Claude task's variable-path removal prompt (SPEC-596).
//! A permission decision covers the whole Bash script. Return a checked retry to the
//! calling agent, which can then run the other commands under their usual permissions.

use std::io::{Read, Write};

use conch_parser::ast::{
    AndOr, Command, ComplexWord, CompoundCommandKind, DefaultPipeableCommand, DefaultSimpleWord,
    ListableCommand, Parameter, PipeableCommand, RedirectOrCmdWord, SimpleWord, TopLevelCommand,
    TopLevelWord, Word,
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
    let (parent, child) = commands.iter().find_map(cleanup_in_command)?;
    Some(format!(
        "The command has not run. Retry the cleanup now using \
         `yulius remove \"${{{parent}:?}}/${{{child}:?}}\"` for this removal. \
         Check its result before rebuilding the scratch copy."
    ))
}

fn cleanup_in_command(command: &TopLevelCommand<String>) -> Option<(&str, &str)> {
    let (Command::List(list) | Command::Job(list)) = &command.0;
    std::iter::once(&list.first)
        .chain(list.rest.iter().map(|op| match op {
            AndOr::And(c) | AndOr::Or(c) => c,
        }))
        .find_map(|item| match item {
            ListableCommand::Single(cmd) => cleanup_in_pipe(cmd),
            ListableCommand::Pipe(_, cmds) => cmds.iter().find_map(cleanup_in_pipe),
        })
}

fn cleanup_in_pipe(cmd: &DefaultPipeableCommand) -> Option<(&str, &str)> {
    let simple = match cmd {
        PipeableCommand::Simple(simple) => simple,
        PipeableCommand::Compound(compound) => match &compound.kind {
            CompoundCommandKind::For { body, .. } => {
                return body.iter().find_map(cleanup_in_command);
            }
            _ => return None,
        },
        PipeableCommand::FunctionDef(_, _) => return None,
    };
    let mut words = simple
        .redirects_or_cmd_words
        .iter()
        .filter_map(|item| match item {
            RedirectOrCmdWord::CmdWord(word) => Some(word),
            RedirectOrCmdWord::Redirect(_) => None,
        });
    let name = literal(words.next()?)?;
    let flags = literal(words.next()?)?;
    if !matches!(name, "rm" | "/bin/rm" | "/usr/bin/rm") || !matches!(flags, "-rf" | "-fr") {
        return None;
    }
    let mut target = words.next()?;
    if literal(target) == Some("--") {
        target = words.next()?;
    }
    if words.next().is_some() {
        return None;
    }
    variable_path(target)
}

fn literal(word: &TopLevelWord<String>) -> Option<&str> {
    match &word.0 {
        ComplexWord::Single(Word::Simple(SimpleWord::Literal(text))) => Some(text),
        _ => None,
    }
}

fn variable_path(word: &TopLevelWord<String>) -> Option<(&str, &str)> {
    let (parent, separator, child) = match &word.0 {
        ComplexWord::Concat(parts) => {
            let [Word::Simple(a), Word::Simple(b), Word::Simple(c)] = parts.as_slice() else {
                return None;
            };
            (a, b, c)
        }
        ComplexWord::Single(Word::DoubleQuoted(parts)) => {
            let [a, b, c] = parts.as_slice() else {
                return None;
            };
            (a, b, c)
        }
        _ => return None,
    };
    let DefaultSimpleWord::Literal(slash) = separator else {
        return None;
    };
    let (SimpleWord::Param(Parameter::Var(parent)), SimpleWord::Param(Parameter::Var(child))) =
        (parent, child)
    else {
        return None;
    };
    (slash == "/").then_some((parent, child))
}
