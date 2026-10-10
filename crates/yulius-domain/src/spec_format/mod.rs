//! Enforced documentation format and task spec validation (SPEC-001 §7.2, DSC-001..006,
//! PRJ-005, PRJ-008).
//!
//! Every Yulius project keeps, at configurable paths (`yulius.toml` `[docs]`):
//!
//! ```text
//! AGENTS.md                         agent entry point (rules, commands, layout)
//! CLAUDE.md                         pointer/mirror of AGENTS.md
//! yulius.toml                       docs paths, commands, delivery profile
//! docs/product.md                   project summary
//! docs/architecture.md              architecture with at least one Mermaid diagram
//! docs/decisions/NNNN-<slug>.md     ADRs: Status, Context, Decision, Alternatives, Consequences
//! docs/specs/YYYY-MM-DD-<slug>.md   one spec per task
//! docs/changes.md                   reverse-chronological summary of delivered changes
//! ```
//!
//! A task spec has frontmatter (`yulius-task`, `status`) and the level-2 sections Goal,
//! Context, Requirements, Plan (a checklist), Acceptance criteria, Test plan and Decisions;
//! Evidence is required once `status: done`. [`validate_spec`] and [`validate_project`]
//! report precise, stable problems whose text is shown to the agent as the fix-it prompt.
//! Callers read the files (this module performs no I/O) and pass their contents.

mod config;
pub(crate) mod markdown;
mod project;
mod templates;

use std::fmt;

pub use config::{
    ConfigProblem, FormatError, MAX_COMMAND_BYTES, MAX_CONFIG_BYTES, MAX_PATH_BYTES,
    parse_yulius_toml, parse_yulius_toml_docs, render_yulius_toml,
};
pub use project::{
    DecisionFiles, NamedFile, ProjectFiles, ProjectProblem, ProjectProblemKind, ProjectReport,
    decision_titles, project_layout, validate_project,
};
pub use templates::{TemplateKind, scaffold_files, spec_template};

use crate::cron::CivilDate;
use crate::delivery::DeliveryProfile;
use crate::ids::TaskId;
use markdown::{Line, checklist_item, heading, is_meaningful, normalize_title};

/// Longest slug produced by [`spec_slug`].
pub const MAX_SLUG_LEN: usize = 50;

/// Where architecture decisions live.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DecisionsLocation {
    /// One ADR file per decision (`NNNN-<slug>.md`) in this directory.
    Directory(String),
    /// A single Markdown file with one heading per decision (for example Studio's
    /// `knowledge/decisions.md`).
    File(String),
}

/// Repository-relative paths of the enforced documents (SPEC-001 §7.2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocsLayout {
    /// Project summary.
    pub product: String,
    /// Architecture document with a Mermaid diagram.
    pub architecture: String,
    /// Decision records.
    pub decisions: DecisionsLocation,
    /// Directory of task specs.
    pub specs_dir: String,
    /// Delivered-changes summary.
    pub changes: String,
    /// Agent entry point.
    pub agents: String,
}

impl Default for DocsLayout {
    fn default() -> Self {
        Self {
            product: "docs/product.md".to_owned(),
            architecture: "docs/architecture.md".to_owned(),
            decisions: DecisionsLocation::Directory("docs/decisions".to_owned()),
            specs_dir: "docs/specs".to_owned(),
            changes: "docs/changes.md".to_owned(),
            agents: "AGENTS.md".to_owned(),
        }
    }
}

/// Project verification commands (`[commands]`), opaque strings run by the caller inside the
/// sandbox.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Commands {
    /// Test command.
    pub test: Option<String>,
    /// Lint command.
    pub lint: Option<String>,
    /// Build command.
    pub build: Option<String>,
}

/// The parsed `yulius.toml` project contract.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectConfig {
    /// Document paths.
    pub docs: DocsLayout,
    /// Verification commands.
    pub commands: Commands,
    /// Delivery profile.
    pub delivery: DeliveryProfile,
}

impl Default for ProjectConfig {
    fn default() -> Self {
        Self {
            docs: DocsLayout::default(),
            commands: Commands::default(),
            delivery: DeliveryProfile::standard(),
        }
    }
}

/// Spec lifecycle status from the frontmatter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SpecStatus {
    /// Being written.
    Draft,
    /// Agreed; implementation may start.
    Approved,
    /// Being implemented.
    InProgress,
    /// Complete: Evidence present and every Plan item checked.
    Done,
    /// Dropped.
    Abandoned,
}

impl SpecStatus {
    /// Frontmatter spelling.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Draft => "draft",
            Self::Approved => "approved",
            Self::InProgress => "in-progress",
            Self::Done => "done",
            Self::Abandoned => "abandoned",
        }
    }

    /// Parses the frontmatter spelling.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "draft" => Some(Self::Draft),
            "approved" => Some(Self::Approved),
            "in-progress" => Some(Self::InProgress),
            "done" => Some(Self::Done),
            "abandoned" => Some(Self::Abandoned),
            _ => None,
        }
    }
}

/// One checklist item of the spec's Plan section.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanItem {
    /// Item text (trimmed, inline Markdown kept).
    pub text: String,
    /// `[x]`.
    pub checked: bool,
    /// Nesting depth (0 = top level).
    pub depth: usize,
    /// 1-based line number.
    pub line: usize,
}

/// A spec format problem. The `Display` text is stable and written as an instruction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SpecProblem {
    /// No `---` frontmatter block at the top.
    MissingFrontmatter,
    /// The frontmatter has no closing `---`.
    UnterminatedFrontmatter,
    /// A required frontmatter field is absent or empty.
    MissingField(&'static str),
    /// `status` has an unknown value.
    InvalidStatus(String),
    /// A required section is absent.
    MissingSection(&'static str),
    /// A required section appears twice.
    DuplicateSection {
        /// Section name.
        name: &'static str,
        /// Line of the second occurrence.
        line: usize,
    },
    /// A required section has no content.
    EmptySection {
        /// Section name.
        name: &'static str,
        /// Line of the heading.
        line: usize,
    },
    /// The Plan has no checklist item.
    NoPlanItems {
        /// Line of the Plan heading.
        line: usize,
    },
    /// A checklist item without text.
    EmptyPlanItem {
        /// Line of the item.
        line: usize,
    },
    /// `status: done` without an Evidence section.
    MissingEvidence,
    /// `status: done` with unchecked Plan items.
    UncheckedPlanItems {
        /// Number of unchecked items.
        count: usize,
        /// Line of the first unchecked item.
        first_line: usize,
    },
    /// The frontmatter names another task.
    TaskMismatch {
        /// This task's id.
        expected: String,
        /// The id in the spec.
        found: String,
    },
}

impl fmt::Display for SpecProblem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingFrontmatter => f.write_str(
                "Missing frontmatter: start the spec with a '---' block containing 'yulius-task: <task id>' and 'status: draft', closed by '---'.",
            ),
            Self::UnterminatedFrontmatter => {
                f.write_str("Frontmatter is not closed: add a '---' line after the fields.")
            }
            Self::MissingField(field) => write!(f, "Frontmatter is missing '{field}'."),
            Self::InvalidStatus(value) => write!(
                f,
                "Frontmatter status '{value}' is not one of: draft, approved, in-progress, done, abandoned."
            ),
            Self::MissingSection(name) => write!(f, "Missing section '## {name}'."),
            Self::DuplicateSection { name, line } => write!(
                f,
                "Section '## {name}' appears more than once (again at line {line}); keep one."
            ),
            Self::EmptySection { name, line } => {
                write!(f, "Section '## {name}' (line {line}) is empty.")
            }
            Self::NoPlanItems { line } => write!(
                f,
                "Section '## Plan' (line {line}) has no checklist item; list the steps as '- [ ] step'."
            ),
            Self::EmptyPlanItem { line } => write!(f, "Plan item at line {line} has no text."),
            Self::MissingEvidence => f.write_str(
                "Status is 'done' but section '## Evidence' is missing; record the commands run and their results.",
            ),
            Self::UncheckedPlanItems { count, first_line } => write!(
                f,
                "Status is 'done' but {count} Plan item(s) are unchecked (first at line {first_line}); check them or change the status."
            ),
            Self::TaskMismatch { expected, found } => write!(
                f,
                "Frontmatter 'yulius-task' is '{found}' but this task is '{expected}'."
            ),
        }
    }
}

/// Result of [`validate_spec`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpecReport {
    /// Problems in a stable order: frontmatter, sections in required order, completion.
    pub problems: Vec<SpecProblem>,
    /// Plan checklist items in document order (nested items included).
    pub plan: Vec<PlanItem>,
    /// `yulius-task` value, if present.
    pub task_id: Option<String>,
    /// `status` value, if valid.
    pub status: Option<SpecStatus>,
}

impl SpecReport {
    /// No problems.
    #[must_use]
    pub fn is_valid(&self) -> bool {
        self.problems.is_empty()
    }

    /// The fix-it message shown to the agent (for example as a Stop-hook block reason).
    #[must_use]
    pub fn agent_message(&self, spec_path: &str) -> String {
        if self.is_valid() {
            return format!("The task spec `{spec_path}` satisfies the Yulius format.");
        }
        let mut message =
            format!("The task spec `{spec_path}` does not satisfy the Yulius format:\n");
        for problem in &self.problems {
            message.push_str("- ");
            message.push_str(&problem.to_string());
            message.push('\n');
        }
        message.push_str("Fix the spec, then continue.");
        message
    }
}

const REQUIRED_SECTIONS: [&str; 7] = [
    "Goal",
    "Context",
    "Requirements",
    "Plan",
    "Acceptance criteria",
    "Test plan",
    "Decisions",
];

/// A level-2 section: its normalized title, heading line and body lines.
pub(crate) struct Section<'a> {
    pub name: String,
    pub line: usize,
    pub body: Vec<&'a Line<'a>>,
}

/// Level-2 sections of `lines` (level-1 headings end a section without starting one).
pub(crate) fn sections<'a>(lines: &'a [Line<'a>]) -> Vec<Section<'a>> {
    let mut sections: Vec<Section<'a>> = Vec::new();
    let mut open = false;
    for line in lines {
        if !line.code
            && let Some((level, title)) = heading(&line.content)
            && level <= 2
        {
            open = level == 2;
            if open {
                sections.push(Section {
                    name: normalize_title(title),
                    line: line.number,
                    body: Vec::new(),
                });
            }
            continue;
        }
        if open && let Some(section) = sections.last_mut() {
            section.body.push(line);
        }
    }
    sections
}

fn clip(text: &str, max: usize) -> String {
    let mut end = text.len().min(max);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text[..end].to_owned()
}

fn frontmatter_value(raw: &str) -> &str {
    let value = raw.trim();
    for quote in ['"', '\''] {
        if let Some(rest) = value.strip_prefix(quote) {
            return rest.find(quote).map_or(rest, |end| &rest[..end]);
        }
    }
    value
        .find(" #")
        .map_or(value, |comment| value[..comment].trim_end())
}

/// Validates a task spec (DSC-002, DSC-003).
///
/// # Examples
/// ```
/// use yulius_domain::spec_format::validate_spec;
/// let report = validate_spec("---\nyulius-task: t1\nstatus: draft\n---\n## Goal\nShip it.\n");
/// assert!(!report.is_valid());
/// assert_eq!(report.problems[0].to_string(), "Missing section '## Context'.");
/// ```
#[must_use]
pub fn validate_spec(markdown: &str) -> SpecReport {
    let lines = markdown::scan(markdown);
    let mut problems = Vec::new();
    let mut task_id = None;
    let mut status = None;
    let mut body_start = 0;
    if lines
        .first()
        .is_some_and(|line| line.text.trim_end() == "---")
    {
        match lines[1..]
            .iter()
            .position(|line| matches!(line.text.trim_end(), "---" | "..."))
        {
            None => {
                problems.push(SpecProblem::UnterminatedFrontmatter);
                body_start = 1;
            }
            Some(offset) => {
                let end = offset + 1;
                let mut raw_status = None;
                for line in &lines[1..end] {
                    if let Some((key, value)) = line.text.split_once(':') {
                        let value = frontmatter_value(value);
                        match key.trim() {
                            "yulius-task" if !value.is_empty() => {
                                task_id = Some(value.to_owned());
                            }
                            "status" if !value.is_empty() => raw_status = Some(value.to_owned()),
                            _ => {}
                        }
                    }
                }
                if task_id.is_none() {
                    problems.push(SpecProblem::MissingField("yulius-task"));
                }
                match raw_status {
                    None => problems.push(SpecProblem::MissingField("status")),
                    Some(value) => match SpecStatus::parse(&value) {
                        Some(parsed) => status = Some(parsed),
                        None => problems.push(SpecProblem::InvalidStatus(clip(&value, 64))),
                    },
                }
                body_start = end + 1;
            }
        }
    } else {
        problems.push(SpecProblem::MissingFrontmatter);
    }

    let found = sections(lines.get(body_start..).unwrap_or_default());
    let mut plan = Vec::new();
    for name in REQUIRED_SECTIONS {
        let normalized = normalize_title(name);
        let mut matching = found.iter().filter(|section| section.name == normalized);
        let Some(section) = matching.next() else {
            problems.push(SpecProblem::MissingSection(name));
            continue;
        };
        if let Some(again) = matching.next() {
            problems.push(SpecProblem::DuplicateSection {
                name,
                line: again.line,
            });
        }
        if name == "Plan" {
            let mut any_item = false;
            for line in section.body.iter().filter(|line| !line.code) {
                if let Some((depth, checked, text)) = checklist_item(&line.content) {
                    any_item = true;
                    if text.is_empty() {
                        problems.push(SpecProblem::EmptyPlanItem { line: line.number });
                    } else {
                        plan.push(PlanItem {
                            text: text.to_owned(),
                            checked,
                            depth,
                            line: line.number,
                        });
                    }
                }
            }
            if !any_item {
                problems.push(SpecProblem::NoPlanItems { line: section.line });
            }
        } else if !section.body.iter().any(|line| is_meaningful(line)) {
            problems.push(SpecProblem::EmptySection {
                name,
                line: section.line,
            });
        }
    }

    if status == Some(SpecStatus::Done) {
        let mut evidence = found.iter().filter(|section| section.name == "evidence");
        match evidence.next() {
            None => problems.push(SpecProblem::MissingEvidence),
            Some(section) => {
                if let Some(again) = evidence.next() {
                    problems.push(SpecProblem::DuplicateSection {
                        name: "Evidence",
                        line: again.line,
                    });
                }
                if !section.body.iter().any(|line| is_meaningful(line)) {
                    problems.push(SpecProblem::EmptySection {
                        name: "Evidence",
                        line: section.line,
                    });
                }
            }
        }
        let unchecked: Vec<&PlanItem> = plan.iter().filter(|item| !item.checked).collect();
        if let Some(first) = unchecked.first() {
            problems.push(SpecProblem::UncheckedPlanItems {
                count: unchecked.len(),
                first_line: first.line,
            });
        }
    }

    SpecReport {
        problems,
        plan,
        task_id,
        status,
    }
}

/// [`validate_spec`] plus a check that the frontmatter names `expected`.
#[must_use]
pub fn validate_spec_for(markdown: &str, expected: &TaskId) -> SpecReport {
    let mut report = validate_spec(markdown);
    if let Some(found) = &report.task_id
        && found != expected.as_str()
    {
        report.problems.push(SpecProblem::TaskMismatch {
            expected: expected.to_string(),
            found: clip(found, 64),
        });
    }
    report
}

fn transliterate(c: char) -> Option<&'static str> {
    Some(match c {
        'à' | 'á' | 'â' | 'ã' | 'ä' | 'å' | 'ā' | 'ą' | 'ă' => "a",
        'æ' => "ae",
        'ç' | 'ć' | 'č' | 'ĉ' | 'ċ' => "c",
        'ď' | 'đ' | 'ð' => "d",
        'è' | 'é' | 'ê' | 'ë' | 'ē' | 'ę' | 'ě' | 'ė' => "e",
        'ğ' | 'ģ' => "g",
        'ì' | 'í' | 'î' | 'ï' | 'ī' | 'į' | 'ı' => "i",
        'ķ' => "k",
        'ł' | 'ľ' | 'ĺ' | 'ļ' => "l",
        'ñ' | 'ń' | 'ň' | 'ņ' => "n",
        'ò' | 'ó' | 'ô' | 'õ' | 'ö' | 'ø' | 'ō' | 'ő' => "o",
        'œ' => "oe",
        'ř' | 'ŕ' => "r",
        'ś' | 'š' | 'ş' | 'ș' => "s",
        'ß' => "ss",
        'ť' | 'ţ' | 'ț' => "t",
        'þ' => "th",
        'ù' | 'ú' | 'û' | 'ü' | 'ū' | 'ů' | 'ű' | 'ų' => "u",
        'ý' | 'ÿ' => "y",
        'ź' | 'ż' | 'ž' => "z",
        _ => return None,
    })
}

/// ASCII kebab-case slug of a title, at most [`MAX_SLUG_LEN`] characters, cut at a word
/// boundary when possible; common Latin diacritics are transliterated (`Żółw` → `zolw`).
/// Returns `task` when nothing usable remains.
#[must_use]
pub fn spec_slug(title: &str) -> String {
    slug_or(title, "task")
}

/// [`spec_slug`] with a custom fallback.
pub(crate) fn slug_or(title: &str, fallback: &str) -> String {
    let mut slug = String::new();
    let mut dash = false;
    for c in title.chars().flat_map(char::to_lowercase) {
        if slug.len() > MAX_SLUG_LEN {
            break;
        }
        let piece = if c.is_ascii_alphanumeric() {
            Some(c.encode_utf8(&mut [0; 4]).to_owned())
        } else {
            transliterate(c).map(str::to_owned)
        };
        match piece {
            Some(piece) => {
                if dash && !slug.is_empty() {
                    slug.push('-');
                }
                dash = false;
                slug.push_str(&piece);
            }
            None => dash = true,
        }
    }
    if slug.len() > MAX_SLUG_LEN {
        let cut_mid_word = slug.as_bytes()[MAX_SLUG_LEN] != b'-';
        slug.truncate(MAX_SLUG_LEN);
        if cut_mid_word && let Some(boundary) = slug.rfind('-') {
            slug.truncate(boundary);
        }
    }
    let slug = slug.trim_matches('-');
    if slug.is_empty() {
        fallback.to_owned()
    } else {
        slug.to_owned()
    }
}

/// `<specs_dir>/<YYYY-MM-DD>-<slug>.md`.
#[must_use]
pub fn spec_path(layout: &DocsLayout, date: CivilDate, slug: &str) -> String {
    format!(
        "{}/{date}-{slug}.md",
        layout.specs_dir.trim_end_matches('/')
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Deterministic xorshift generator for randomized tests (no extra dependency).
    struct Rng(u64);

    impl Rng {
        fn next(&mut self) -> u64 {
            let mut x = self.0;
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            self.0 = x;
            x
        }

        fn below(&mut self, bound: usize) -> usize {
            usize::try_from(self.next() % u64::try_from(bound.max(1)).unwrap_or(1)).unwrap_or(0)
        }

        /// Text made of `pieces`, random bytes (lossy UTF-8) and multi-byte characters.
        fn text(&mut self, pieces: &[&str], max_parts: usize) -> String {
            let mut out = String::new();
            for _ in 0..self.below(max_parts + 1) {
                match self.below(10) {
                    0 => {
                        let bytes: Vec<u8> = (0..self.below(8))
                            .map(|_| (self.next() & 0xff) as u8)
                            .collect();
                        out.push_str(&String::from_utf8_lossy(&bytes));
                    }
                    1 => out
                        .push_str(["é", "😀", "\u{0}", "\r", "\t", "ﬀ", "\u{200b}"][self.below(7)]),
                    _ => out.push_str(pieces[self.below(pieces.len())]),
                }
            }
            out
        }
    }

    /// Randomized specs (#76): validation and the fix-it message never panic.
    #[test]
    fn random_specs_never_panic() -> TestResult {
        let pieces = [
            "---\n",
            "yulius-task: task-1\n",
            "status: draft\n",
            "status: done\n",
            "status: x\n",
            "# Title\n",
            "## Goal\n",
            "## Context\n",
            "## Requirements\n",
            "## Plan\n",
            "## Acceptance criteria\n",
            "## Test plan\n",
            "## Decisions\n",
            "- [ ] step\n",
            "- [x] done\n",
            "  - [ ] nested\n",
            "- [",
            "```\n",
            "text\n",
            "\r\n",
            "\n",
            " ",
            "#",
        ];
        let task = TaskId::parse("task-1")?;
        let mut rng = Rng(0x2545_f491_4f6c_dd1d);
        for _ in 0..4_000 {
            let text = rng.text(&pieces, 40);
            let report = validate_spec(&text);
            let _ = report.agent_message("docs/specs/x.md");
            let _ = validate_spec_for(&text, &task);
        }
        Ok(())
    }

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    const VALID: &str = "---\nyulius-task: task-1\nstatus: in-progress\n---\n\
        # Checkout API\n\n\
        ## Goal\n\nCustomers can pay.\n\n\
        ## Context\n\nThe shop has no checkout.\n\n\
        ## Requirements\n\n- Card payments.\n\n\
        ## Plan\n\n- [x] Write spec\n- [ ] Implement API\n  - [ ] Validate input\n\n\
        ## Acceptance Criteria\n\nA test order succeeds.\n\n\
        ## Test plan\n\n```sh\nnode --test\n```\n\n\
        ## Decisions\n\n- Use the provider's hosted page.\n";

    fn texts(report: &SpecReport) -> Vec<String> {
        report.problems.iter().map(ToString::to_string).collect()
    }

    #[test]
    fn valid_spec_has_no_problems_and_exposes_plan() {
        let report = validate_spec(VALID);
        assert_eq!(texts(&report), Vec::<String>::new());
        assert_eq!(report.task_id.as_deref(), Some("task-1"));
        assert_eq!(report.status, Some(SpecStatus::InProgress));
        let plan: Vec<(&str, bool, usize, usize)> = report
            .plan
            .iter()
            .map(|item| (item.text.as_str(), item.checked, item.depth, item.line))
            .collect();
        assert_eq!(
            plan,
            vec![
                ("Write spec", true, 0, 21),
                ("Implement API", false, 0, 22),
                ("Validate input", false, 1, 23)
            ]
        );
    }

    #[test]
    fn missing_frontmatter_and_sections_are_listed_in_order() {
        let report = validate_spec("# Title\n\n## goal\n\nDo it.\n\n## Plan\n\nJust prose.\n");
        assert_eq!(
            texts(&report),
            vec![
                "Missing frontmatter: start the spec with a '---' block containing 'yulius-task: <task id>' and 'status: draft', closed by '---'.",
                "Missing section '## Context'.",
                "Missing section '## Requirements'.",
                "Section '## Plan' (line 7) has no checklist item; list the steps as '- [ ] step'.",
                "Missing section '## Acceptance criteria'.",
                "Missing section '## Test plan'.",
                "Missing section '## Decisions'.",
            ]
        );
    }

    #[test]
    fn frontmatter_problems() {
        let unterminated = validate_spec("---\nyulius-task: t\n## Goal\n");
        assert_eq!(
            unterminated.problems[0],
            SpecProblem::UnterminatedFrontmatter
        );
        let fields = validate_spec("---\ntitle: x\nstatus: finished\n---\n");
        assert_eq!(fields.problems[0], SpecProblem::MissingField("yulius-task"));
        assert_eq!(
            fields.problems[1],
            SpecProblem::InvalidStatus("finished".to_owned())
        );
        let quoted = validate_spec("---\nyulius-task: \"t-9\" \nstatus: 'done' # finished\n---\n");
        assert_eq!(quoted.task_id.as_deref(), Some("t-9"));
        assert_eq!(quoted.status, Some(SpecStatus::Done));
        let missing_status = validate_spec("---\nyulius-task: t\n---\n");
        assert_eq!(
            missing_status.problems[0],
            SpecProblem::MissingField("status")
        );
    }

    #[test]
    fn empty_sections_comments_and_code_fences() {
        let text = VALID
            .replace("Customers can pay.", "<!-- one sentence -->")
            .replace(
                "- [x] Write spec\n- [ ] Implement API\n  - [ ] Validate input",
                "<!-- - [ ] example -->\n```\n- [ ] in code\n```",
            )
            .replace("```sh\nnode --test\n```", "### Unit\n");
        let report = validate_spec(&text);
        assert_eq!(
            texts(&report),
            vec![
                "Section '## Goal' (line 7) is empty.",
                "Section '## Plan' (line 19) has no checklist item; list the steps as '- [ ] step'.",
                "Section '## Test plan' (line 30) is empty.",
            ]
        );
        // Headings inside code fences do not start sections.
        let fenced = VALID.replace("## Decisions\n", "```\n## Decisions\n```\n");
        assert!(
            texts(&validate_spec(&fenced)).contains(&"Missing section '## Decisions'.".to_owned())
        );
    }

    #[test]
    fn duplicates_and_empty_items() {
        let text = VALID
            .replace("## Decisions", "## Goal\n\nAgain.\n\n## Decisions")
            .replace("- [ ] Implement API", "- [ ]");
        assert_eq!(
            texts(&validate_spec(&text)),
            vec![
                "Section '## Goal' appears more than once (again at line 35); keep one.",
                "Plan item at line 22 has no text.",
            ]
        );
    }

    #[test]
    fn done_requires_evidence_and_checked_plan() {
        let done = VALID.replace("status: in-progress", "status: done");
        assert_eq!(
            texts(&validate_spec(&done)),
            vec![
                "Status is 'done' but section '## Evidence' is missing; record the commands run and their results.",
                "Status is 'done' but 2 Plan item(s) are unchecked (first at line 22); check them or change the status.",
            ]
        );
        let empty_evidence = format!("{done}\n## Evidence\n\n<!-- todo -->\n");
        assert!(
            texts(&validate_spec(&empty_evidence))
                .contains(&"Section '## Evidence' (line 39) is empty.".to_owned())
        );
        let complete = format!(
            "{}\n## Evidence\n\n`node --test`: 3 passed.\n",
            done.replace("- [ ]", "- [x]")
        );
        assert!(validate_spec(&complete).is_valid());
        // Abandoned specs need neither.
        assert!(validate_spec(&VALID.replace("in-progress", "abandoned")).is_valid());
    }

    #[test]
    fn task_mismatch_and_agent_message() -> TestResult {
        let report = validate_spec_for(VALID, &TaskId::parse("task-2")?);
        assert_eq!(
            texts(&report),
            vec!["Frontmatter 'yulius-task' is 'task-1' but this task is 'task-2'."]
        );
        assert_eq!(
            report.agent_message("docs/specs/x.md"),
            "The task spec `docs/specs/x.md` does not satisfy the Yulius format:\n- Frontmatter 'yulius-task' is 'task-1' but this task is 'task-2'.\nFix the spec, then continue."
        );
        assert!(validate_spec_for(VALID, &TaskId::parse("task-1")?).is_valid());
        Ok(())
    }

    #[test]
    fn crlf_and_bom_are_tolerated() {
        let text = format!("\u{feff}{}", VALID.replace('\n', "\r\n"));
        assert!(validate_spec(&text).is_valid());
    }

    #[test]
    fn slugs() {
        assert_eq!(spec_slug("Checkout API"), "checkout-api");
        assert_eq!(
            spec_slug("  Fix: login (OAuth) -- again!  "),
            "fix-login-oauth-again"
        );
        assert_eq!(
            spec_slug("Zrób sklep internetowy z płatnościami"),
            "zrob-sklep-internetowy-z-platnosciami"
        );
        assert_eq!(spec_slug("Straße & Café"), "strasse-cafe");
        assert_eq!(spec_slug("🚀🚀"), "task");
        assert_eq!(spec_slug(""), "task");
        let long = spec_slug(
            "implement the complete customer checkout flow with card payments and receipts",
        );
        assert_eq!(long, "implement-the-complete-customer-checkout-flow-with");
        assert!(long.len() <= MAX_SLUG_LEN);
        let unbroken = spec_slug(&"a".repeat(80));
        assert_eq!(unbroken.len(), MAX_SLUG_LEN);
    }

    #[test]
    fn spec_paths() -> TestResult {
        let date = CivilDate::new(2026, 10, 7).ok_or("date")?;
        assert_eq!(
            spec_path(&DocsLayout::default(), date, "checkout-api"),
            "docs/specs/2026-10-07-checkout-api.md"
        );
        let custom = DocsLayout {
            specs_dir: "knowledge/specs/".to_owned(),
            ..DocsLayout::default()
        };
        assert_eq!(
            spec_path(&custom, date, "x"),
            "knowledge/specs/2026-10-07-x.md"
        );
        Ok(())
    }
}
