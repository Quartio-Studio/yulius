//! The file format of default workflows (SPEC-002 §7.10 "Workflow catalog format", SPEC-001
//! AUT-004/AUT-006).
//!
//! Yulius ships its default workflows as one Markdown file per workflow,
//! `catalog/workflows/<id>.md`, embedded at build time:
//!
//! ```text
//! ---
//! name: Feature
//! description: New or changed behaviour, from spec to production.
//! when_to_use: The task adds or changes behaviour that users or other code rely on.
//! spec: required
//! tests: required
//! order: 10
//! ---
//!
//! ## Steps
//!
//! 1. Write the spec
//! 2. Implement
//!
//! ## Instructions
//!
//! Free Markdown given to the agent (### subsections allowed).
//! ```
//!
//! The frontmatter holds one `key: value` per line (plain text, optionally quoted, never
//! multi-line); unknown or repeated keys are errors so a typo cannot pass silently. The body has
//! exactly the two level-2 sections `Steps` (a list, one step title per top-level item) and
//! `Instructions` (everything below it), in that order. [`parse_workflow`] reports the first
//! problem with its line. Callers read the files; this module performs no I/O.

use crate::spec_format::markdown::{heading, normalize_title, scan};
use crate::steps::MAX_STEPS;

/// Longest workflow id (the file stem).
pub const MAX_ID_LEN: usize = 40;
/// Longest name, in characters.
pub const MAX_NAME_CHARS: usize = 80;
/// Longest `description` or `when_to_use`, in characters.
pub const MAX_LINE_CHARS: usize = 300;
/// Longest step title, in characters.
pub const MAX_STEP_CHARS: usize = 200;
/// Largest instructions text, in bytes.
pub const MAX_INSTRUCTIONS_BYTES: usize = 16 * 1024;
/// Largest workflow file, in bytes.
pub const MAX_FILE_BYTES: usize = 64 * 1024;
/// List order when the file sets none.
pub const DEFAULT_ORDER: u16 = 100;
/// Largest `order`.
pub const MAX_ORDER: u16 = 999;

/// A parsed default workflow.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkflowDefinition {
    /// The file stem (`feature` for `feature.md`).
    pub id: String,
    /// Display name.
    pub name: String,
    /// What the workflow is, in one line.
    pub description: String,
    /// When to pick it, in one line (also what intake reads when the workflow is chosen
    /// automatically).
    pub when_to_use: String,
    /// `spec: required`: a valid task spec before delivery.
    pub require_spec: bool,
    /// `tests: required`: the project's tests before delivery.
    pub require_tests: bool,
    /// List order (smaller first, then by name).
    pub order: u16,
    /// Initial step titles, in order.
    pub steps: Vec<String>,
    /// The instructions given to the agent (Markdown, trimmed).
    pub instructions: String,
}

/// The first problem of a workflow file.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("line {line}: {message}")]
pub struct WorkflowFormatError {
    /// 1-based line (1 for whole-file problems).
    pub line: usize,
    /// What is wrong, in words.
    pub message: String,
}

fn problem<T>(line: usize, message: impl Into<String>) -> Result<T, WorkflowFormatError> {
    Err(WorkflowFormatError {
        line,
        message: message.into(),
    })
}

/// Whether `id` is a valid workflow id: 1–[`MAX_ID_LEN`] lowercase ASCII letters, digits and
/// `-`, starting with a letter or digit.
#[must_use]
pub fn valid_workflow_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= MAX_ID_LEN
        && !id.starts_with('-')
        && id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// Removes one pair of matching surrounding quotes.
pub(crate) fn unquote(value: &str) -> &str {
    for quote in ['"', '\''] {
        if let Some(inner) = value
            .strip_prefix(quote)
            .and_then(|v| v.strip_suffix(quote))
        {
            return inner;
        }
    }
    value
}

fn requirement(line: usize, key: &str, value: &str) -> Result<bool, WorkflowFormatError> {
    match value {
        "required" => Ok(true),
        "optional" => Ok(false),
        _ => problem(line, format!("{key} must be `required` or `optional`")),
    }
}

fn one_line(
    line: usize,
    key: &str,
    value: Option<String>,
    max: usize,
) -> Result<String, WorkflowFormatError> {
    let Some(value) = value.filter(|v| !v.is_empty()) else {
        return problem(line, format!("the frontmatter needs `{key}`"));
    };
    if value.chars().count() > max {
        return problem(line, format!("`{key}` is longer than {max} characters"));
    }
    Ok(value)
}

/// A list item's text: `- x`, `* x`, `+ x` or `1. x` / `1) x` at the start of the line.
fn list_item(text: &str) -> Option<&str> {
    if text.starts_with([' ', '\t']) {
        return None;
    }
    let after = if let Some(rest) = text.strip_prefix(['-', '*', '+']) {
        rest
    } else {
        let digits = text.bytes().take_while(u8::is_ascii_digit).count();
        if digits == 0 || digits > 9 {
            return None;
        }
        text[digits..].strip_prefix(['.', ')'])?
    };
    after
        .strip_prefix([' ', '\t'])
        .map(str::trim)
        .filter(|t| !t.is_empty())
}

/// Parses one workflow file. `id` is its file stem.
///
/// # Errors
/// [`WorkflowFormatError`] with the line of the first problem: an invalid id, a missing or
/// unterminated frontmatter, an unknown, repeated or missing key, a value out of bounds, a
/// missing or extra level-2 section, or a step list that is empty of items but has other text.
pub fn parse_workflow(id: &str, text: &str) -> Result<WorkflowDefinition, WorkflowFormatError> {
    if !valid_workflow_id(id) {
        return problem(
            1,
            format!("the file name must be 1 to {MAX_ID_LEN} lowercase letters, digits or -"),
        );
    }
    if text.len() > MAX_FILE_BYTES {
        return problem(1, "the file is larger than 64 KiB");
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

    let (mut name, mut description, mut when_to_use) = (None, None, None);
    let (mut spec, mut tests, mut order) = (None, None, None);
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
            "when_to_use" => &mut when_to_use,
            "spec" => &mut spec,
            "tests" => &mut tests,
            "order" => &mut order,
            _ => {
                return problem(
                    line.number,
                    format!(
                        "unknown key `{key}` (name, description, when_to_use, spec, tests, order)"
                    ),
                );
            }
        };
        if slot.is_some() {
            return problem(line.number, format!("`{key}` is set twice"));
        }
        *slot = Some((line.number, value));
    }
    let at = |field: &Option<(usize, String)>| field.as_ref().map_or(1, |(n, _)| *n);
    let value = |field: &Option<(usize, String)>| field.as_ref().map(|(_, v)| v.clone());
    let name_text = one_line(at(&name), "name", value(&name), MAX_NAME_CHARS)?;
    let description_text = one_line(
        at(&description),
        "description",
        value(&description),
        MAX_LINE_CHARS,
    )?;
    let when_text = one_line(
        at(&when_to_use),
        "when_to_use",
        value(&when_to_use),
        MAX_LINE_CHARS,
    )?;
    let require_spec = match &spec {
        Some((n, v)) => requirement(*n, "spec", v)?,
        None => false,
    };
    let require_tests = match &tests {
        Some((n, v)) => requirement(*n, "tests", v)?,
        None => false,
    };
    let order = match &order {
        Some((n, v)) => match v.parse::<u16>() {
            Ok(o) if o <= MAX_ORDER => o,
            _ => {
                return problem(
                    *n,
                    format!("order must be a whole number from 0 to {MAX_ORDER}"),
                );
            }
        },
        None => DEFAULT_ORDER,
    };

    // Body: `## Steps`, then `## Instructions` (deeper headings only inside Instructions).
    let body = &lines[close + 1..];
    let mut sections: Vec<(usize, String, usize)> = Vec::new(); // (line, title, index in body)
    for (index, line) in body.iter().enumerate() {
        if line.code {
            continue;
        }
        if let Some((level, title)) = heading(line.content.trim_end()) {
            match level {
                1 => return problem(line.number, "use `##` sections, not a `#` title"),
                2 => sections.push((line.number, normalize_title(title), index)),
                _ if sections.is_empty() => {
                    return problem(line.number, "text before `## Steps`");
                }
                _ => {}
            }
        } else if sections.is_empty() && !line.content.trim().is_empty() {
            return problem(line.number, "text before `## Steps`");
        }
    }
    const SECTIONS: [&str; 2] = ["steps", "instructions"];
    if let Some(wrong) = (0..sections.len().max(SECTIONS.len()))
        .find(|&i| sections.get(i).map(|(_, t, _)| t.as_str()) != SECTIONS.get(i).copied())
    {
        let line = sections
            .get(wrong)
            .map_or(lines.len().max(1), |(n, _, _)| *n);
        return problem(
            line,
            "the body needs exactly `## Steps` then `## Instructions`",
        );
    }
    let (steps_at, instructions_at) = (sections[0].2, sections[1].2);

    let mut steps = Vec::new();
    for line in &body[steps_at + 1..instructions_at] {
        if line.code {
            return problem(line.number, "steps are a plain list, without code blocks");
        }
        let content = line.content.trim_end();
        if content.trim().is_empty() {
            continue;
        }
        let Some(item) = list_item(content) else {
            return problem(
                line.number,
                "each step is a list item (`1. Step` or `- Step`)",
            );
        };
        if item.chars().count() > MAX_STEP_CHARS {
            return problem(
                line.number,
                format!("a step is longer than {MAX_STEP_CHARS} characters"),
            );
        }
        steps.push(item.to_owned());
    }
    if steps.len() > MAX_STEPS {
        return problem(
            sections[0].0,
            format!("a workflow has at most {MAX_STEPS} steps"),
        );
    }

    let instructions = body[instructions_at + 1..]
        .iter()
        .map(|l| l.text)
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_owned();
    if instructions.len() > MAX_INSTRUCTIONS_BYTES {
        return problem(sections[1].0, "the instructions are larger than 16 KiB");
    }

    Ok(WorkflowDefinition {
        id: id.to_owned(),
        name: name_text,
        description: description_text,
        when_to_use: when_text,
        require_spec,
        require_tests,
        order,
        steps,
        instructions,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const FEATURE: &str = "---\nname: Feature\ndescription: \"New behaviour: spec to production.\"\n\
        when_to_use: The task changes behaviour.\nspec: required\ntests: required\norder: 10\n---\n\n\
        ## Steps\n\n1. Write the spec\n2. Implement\n- Review the diff\n\n\
        ## Instructions\n\nKeep the spec current.\n\n### Tests\n\n```sh\n## not a section\n```\n";

    fn parse(text: &str) -> Result<WorkflowDefinition, WorkflowFormatError> {
        parse_workflow("feature", text)
    }

    #[test]
    fn a_complete_file_parses() -> Result<(), WorkflowFormatError> {
        let wf = parse(FEATURE)?;
        assert_eq!(wf.id, "feature");
        assert_eq!(wf.name, "Feature");
        assert_eq!(wf.description, "New behaviour: spec to production.");
        assert_eq!(wf.when_to_use, "The task changes behaviour.");
        assert!(wf.require_spec && wf.require_tests);
        assert_eq!(wf.order, 10);
        assert_eq!(wf.steps, ["Write the spec", "Implement", "Review the diff"]);
        assert!(wf.instructions.starts_with("Keep the spec current."));
        assert!(wf.instructions.contains("### Tests"));
        assert!(wf.instructions.ends_with("```"), "{}", wf.instructions);
        Ok(())
    }

    #[test]
    fn defaults_and_empty_steps() -> Result<(), WorkflowFormatError> {
        let wf = parse(
            "---\nname: Research\ndescription: d\nwhen_to_use: w\n---\n## Steps\n\n## Instructions\n",
        )?;
        assert!(!wf.require_spec && !wf.require_tests);
        assert_eq!(wf.order, DEFAULT_ORDER);
        assert!(wf.steps.is_empty() && wf.instructions.is_empty());
        Ok(())
    }

    fn line_of(text: &str) -> usize {
        parse(text).err().map_or(0, |e| e.line)
    }

    #[test]
    fn problems_name_their_line() {
        let base = "---\nname: N\ndescription: d\nwhen_to_use: w\n---\n## Steps\n- a\n## Instructions\nx\n";
        assert!(parse(base).is_ok());
        assert_eq!(line_of(&base.replace("name: N", "nmae: N")), 2);
        assert_eq!(
            line_of(&base.replace("description: d", "description: d\nname: M")),
            4
        );
        assert_eq!(line_of(&base.replace("when_to_use: w\n", "")), 1);
        assert_eq!(
            line_of(&base.replace("when_to_use: w", "when_to_use: w\nspec: maybe")),
            5
        );
        assert_eq!(
            line_of(&base.replace("when_to_use: w", "when_to_use: w\norder: 1000")),
            5
        );
        assert_eq!(line_of(&base.replace("- a", "a")), 7);
        assert_eq!(
            line_of(&base.replace("## Instructions", "## Notes\n## Instructions")),
            8
        );
        assert_eq!(line_of(&base.replace("## Steps", "Intro\n## Steps")), 6);
        assert_eq!(line_of(&base.replace("## Steps", "# Title\n## Steps")), 6);
        assert!(parse(&base.replace("## Steps\n- a\n## Instructions", "## Instructions")).is_err());
        assert!(parse("name: N\n").is_err());
        assert!(parse("---\nname: N\n").is_err());
        let long = base.replace("- a", &format!("- {}", "x".repeat(MAX_STEP_CHARS + 1)));
        assert_eq!(line_of(&long), 7);
    }

    #[test]
    fn ids_are_file_stems() {
        assert!(valid_workflow_id("infra-bootstrap"));
        assert!(valid_workflow_id("fix2"));
        for bad in [
            "",
            "-x",
            "Feature",
            "a_b",
            "a.b",
            &"x".repeat(MAX_ID_LEN + 1),
        ] {
            assert!(!valid_workflow_id(bad), "{bad}");
        }
        assert!(parse_workflow("Bad", FEATURE).is_err());
    }
}
