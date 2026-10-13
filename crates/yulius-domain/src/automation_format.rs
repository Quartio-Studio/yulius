//! The file format of ready-made automations (SPEC-042 AUT-007).
//!
//! Yulius ships them as one Markdown file per automation, `catalog/automations/<id>.md`,
//! embedded at build time like the default workflows ([`crate::workflow_format`]):
//!
//! ```text
//! ---
//! name: Keep the docs up to date
//! description: Every night, documentation brought in line with the day's changes.
//! cron: 0 3 * * *
//! utc_offset_minutes: 120
//! workflow: wf-docs
//! order: 10
//! ---
//!
//! The prompt of each task the automation creates (Markdown).
//! ```
//!
//! The frontmatter holds one `key: value` per line; unknown or repeated keys are errors. The body
//! is the prompt. [`parse_automation`] reports the first problem with its line and performs no
//! I/O.

use crate::cron::{MAX_UTC_OFFSET_MINUTES, Schedule};
use crate::spec_format::markdown::scan;
use crate::workflow_format::{unquote, valid_workflow_id};

/// Longest name, in characters (the automation name limit).
pub const MAX_NAME_CHARS: usize = 100;
/// Longest `description`, in characters.
pub const MAX_DESCRIPTION_CHARS: usize = 300;
/// Longest `cron` expression, in characters.
pub const MAX_CRON_CHARS: usize = 200;
/// Largest prompt, in bytes.
pub const MAX_PROMPT_BYTES: usize = 16 * 1024;
/// Largest file, in bytes.
pub const MAX_FILE_BYTES: usize = 32 * 1024;
/// List order when the file sets none.
pub const DEFAULT_ORDER: u16 = 100;
/// Largest `order`.
pub const MAX_ORDER: u16 = 999;

/// A parsed ready-made automation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AutomationDefinition {
    /// The file stem (`docs-up-to-date` for `docs-up-to-date.md`).
    pub id: String,
    /// Name of the automation and title of its tasks.
    pub name: String,
    /// What it does, in one line.
    pub description: String,
    /// Five-field cron expression, normalised by [`Schedule::parse`].
    pub cron: String,
    /// Fixed UTC offset the schedule is evaluated at.
    pub utc_offset_minutes: i32,
    /// Workflow id of the created tasks (`wf-docs`), if any.
    pub workflow: Option<String>,
    /// List order (smaller first, then by name).
    pub order: u16,
    /// Prompt of each created task (trimmed).
    pub prompt: String,
}

/// The first problem of an automation file.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("line {line}: {message}")]
pub struct AutomationFormatError {
    /// 1-based line (1 for whole-file problems).
    pub line: usize,
    /// What is wrong, in words.
    pub message: String,
}

fn problem<T>(line: usize, message: impl Into<String>) -> Result<T, AutomationFormatError> {
    Err(AutomationFormatError {
        line,
        message: message.into(),
    })
}

/// Parses one automation file. `id` is its file stem (same rule as workflow ids).
///
/// # Errors
/// [`AutomationFormatError`] with the line of the first problem: an invalid id, a missing or
/// unterminated frontmatter, an unknown, repeated or missing key, an invalid cron, offset,
/// workflow or order, or an empty or oversized prompt.
pub fn parse_automation(
    id: &str,
    text: &str,
) -> Result<AutomationDefinition, AutomationFormatError> {
    if !valid_workflow_id(id) {
        return problem(
            1,
            "the file name must be lowercase letters, digits or - (at most 40)",
        );
    }
    if text.len() > MAX_FILE_BYTES {
        return problem(1, "the file is larger than 32 KiB");
    }
    let lines = scan(text);
    if lines.first().map(|l| l.text.trim_end()) != Some("---") {
        return problem(1, "the file must start with a `---` frontmatter line");
    }
    let Some(close) = lines
        .iter()
        .skip(1)
        .position(|l| l.text.trim_end() == "---")
        .map(|i| i + 1)
    else {
        return problem(1, "the frontmatter is not closed with `---`");
    };

    let (mut name, mut description, mut cron) = (None, None, None);
    let (mut offset, mut workflow, mut order) = (None, None, None);
    for line in &lines[1..close] {
        let raw = line.text.trim();
        if raw.is_empty() || raw.starts_with('#') {
            continue;
        }
        let Some((key, value)) = raw.split_once(':') else {
            return problem(line.number, "expected `key: value`");
        };
        let (key, value) = (key.trim(), unquote(value.trim()).trim().to_owned());
        let slot = match key {
            "name" => &mut name,
            "description" => &mut description,
            "cron" => &mut cron,
            "utc_offset_minutes" => &mut offset,
            "workflow" => &mut workflow,
            "order" => &mut order,
            _ => {
                return problem(
                    line.number,
                    format!(
                        "unknown key `{key}` (name, description, cron, utc_offset_minutes, workflow, order)"
                    ),
                );
            }
        };
        if slot.is_some() {
            return problem(line.number, format!("`{key}` is set twice"));
        }
        *slot = Some((line.number, value));
    }
    let required = |field: Option<(usize, String)>, key: &str, max: usize| {
        let Some((line, value)) = field else {
            return problem(1, format!("the frontmatter needs `{key}`"));
        };
        if value.is_empty() {
            return problem(line, format!("`{key}` is empty"));
        }
        if value.chars().count() > max {
            return problem(line, format!("`{key}` is longer than {max} characters"));
        }
        Ok((line, value))
    };
    let (_, name) = required(name, "name", MAX_NAME_CHARS)?;
    let (_, description) = required(description, "description", MAX_DESCRIPTION_CHARS)?;
    let (cron_line, cron) = required(cron, "cron", MAX_CRON_CHARS)?;
    let cron = match Schedule::parse(&cron) {
        Ok(schedule) => schedule.as_str().to_owned(),
        Err(err) => return problem(cron_line, format!("cron: {err}")),
    };
    let utc_offset_minutes = match offset {
        None => 0,
        Some((line, value)) => match value.parse::<i32>() {
            Ok(m) if m.abs() <= MAX_UTC_OFFSET_MINUTES => m,
            _ => {
                return problem(
                    line,
                    format!(
                        "utc_offset_minutes must be a whole number from -{MAX_UTC_OFFSET_MINUTES} to {MAX_UTC_OFFSET_MINUTES}"
                    ),
                );
            }
        },
    };
    let workflow = match workflow {
        None => None,
        Some((line, value)) => {
            if !value.strip_prefix("wf-").is_some_and(valid_workflow_id) {
                return problem(
                    line,
                    "workflow must be a default workflow id like `wf-docs`",
                );
            }
            Some(value)
        }
    };
    let order = match order {
        None => DEFAULT_ORDER,
        Some((line, value)) => match value.parse::<u16>() {
            Ok(o) if o <= MAX_ORDER => o,
            _ => {
                return problem(
                    line,
                    format!("order must be a whole number from 0 to {MAX_ORDER}"),
                );
            }
        },
    };

    let prompt = lines[close + 1..]
        .iter()
        .map(|l| l.text)
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_owned();
    if prompt.is_empty() {
        return problem(
            close + 2,
            "the prompt (the text below the frontmatter) is empty",
        );
    }
    if prompt.len() > MAX_PROMPT_BYTES {
        return problem(close + 2, "the prompt is larger than 16 KiB");
    }

    Ok(AutomationDefinition {
        id: id.to_owned(),
        name,
        description,
        cron,
        utc_offset_minutes,
        workflow,
        order,
        prompt,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const DOCS: &str = "---\nname: Keep the docs up to date\ndescription: \"Nightly: docs match the code.\"\n\
        cron: 0 3 * * *\nutc_offset_minutes: 120\nworkflow: wf-docs\norder: 10\n---\n\n\
        Review yesterday's changes.\n\n## Steps\n\n- one\n";

    fn parse(text: &str) -> Result<AutomationDefinition, AutomationFormatError> {
        parse_automation("docs-up-to-date", text)
    }

    #[test]
    fn a_complete_file_parses() -> Result<(), AutomationFormatError> {
        let a = parse(DOCS)?;
        assert_eq!(a.id, "docs-up-to-date");
        assert_eq!(a.name, "Keep the docs up to date");
        assert_eq!(a.description, "Nightly: docs match the code.");
        assert_eq!(a.cron, "0 3 * * *");
        assert_eq!(a.utc_offset_minutes, 120);
        assert_eq!(a.workflow.as_deref(), Some("wf-docs"));
        assert_eq!(a.order, 10);
        assert!(a.prompt.starts_with("Review yesterday's changes."));
        assert!(a.prompt.ends_with("- one"));
        Ok(())
    }

    #[test]
    fn defaults() -> Result<(), AutomationFormatError> {
        let a = parse("---\nname: N\ndescription: d\ncron: 0 9 * * MON\n---\nDo it.\n")?;
        assert_eq!(a.utc_offset_minutes, 0);
        assert_eq!(a.workflow, None);
        assert_eq!(a.order, DEFAULT_ORDER);
        assert_eq!(a.prompt, "Do it.");
        Ok(())
    }

    fn line_of(text: &str) -> usize {
        parse(text).err().map_or(0, |e| e.line)
    }

    #[test]
    fn problems_name_their_line() {
        let base = "---\nname: N\ndescription: d\ncron: 0 3 * * *\n---\nPrompt\n";
        assert!(parse(base).is_ok());
        assert_eq!(line_of(&base.replace("name: N", "nmae: N")), 2);
        assert_eq!(line_of(&base.replace("name: N", "name: N\nname: M")), 3);
        assert_eq!(line_of(&base.replace("description: d\n", "")), 1);
        assert_eq!(line_of(&base.replace("description: d", "description:")), 3);
        assert_eq!(line_of(&base.replace("0 3 * * *", "61 3 * * *")), 4);
        assert_eq!(
            line_of(&base.replace(
                "cron: 0 3 * * *",
                "cron: 0 3 * * *\nutc_offset_minutes: 2000"
            )),
            5
        );
        assert_eq!(
            line_of(&base.replace("cron: 0 3 * * *", "cron: 0 3 * * *\nworkflow: docs")),
            5
        );
        assert_eq!(
            line_of(&base.replace("cron: 0 3 * * *", "cron: 0 3 * * *\norder: 1000")),
            5
        );
        assert_eq!(line_of(&base.replace("Prompt\n", "  \n")), 6);
        assert_eq!(line_of(&base.replace("name: N", "just text")), 2);
        assert!(parse("name: N\n").is_err());
        assert!(parse("---\nname: N\n").is_err());
        assert!(parse(&base.replace("Prompt", &"x".repeat(MAX_PROMPT_BYTES + 1))).is_err());
        assert!(parse_automation("Bad", base).is_err());
    }
}
