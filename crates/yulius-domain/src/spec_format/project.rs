//! Project-level documentation validation (SPEC-001 DSC-001, DSC-004, DSC-005; AC-DSC-1,
//! AC-PRJ-3).

use std::fmt;

use super::markdown::{self, heading, is_meaningful};
use super::{
    DecisionsLocation, DocsLayout, FormatError, ProjectConfig, parse_yulius_toml, sections,
};

/// Decision records as read by the caller.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DecisionFiles<'a> {
    /// Nothing found at the configured location.
    Missing,
    /// Files directly inside the configured directory (file names, not paths).
    Directory(Vec<NamedFile<'a>>),
    /// The single decisions file.
    File(&'a str),
}

/// A file name and its contents.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NamedFile<'a> {
    /// File name without directories.
    pub name: &'a str,
    /// File contents.
    pub contents: &'a str,
}

/// Contents of the enforced files; `None` = absent. Read them at the paths of
/// [`project_layout`] for the repository's `yulius.toml`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectFiles<'a> {
    /// `AGENTS.md` (layout `agents`).
    pub agents: Option<&'a str>,
    /// `CLAUDE.md` at the repository root.
    pub claude: Option<&'a str>,
    /// `yulius.toml` at the repository root.
    pub yulius_toml: Option<&'a str>,
    /// Project summary (layout `product`).
    pub product: Option<&'a str>,
    /// Architecture document (layout `architecture`).
    pub architecture: Option<&'a str>,
    /// Decision records (layout `decisions`).
    pub decisions: DecisionFiles<'a>,
    /// Changes summary (layout `changes`).
    pub changes: Option<&'a str>,
}

impl<'a> ProjectFiles<'a> {
    /// Picks the enforced files out of a `(repository path, contents)` listing, using the
    /// layout from the listing's `yulius.toml` (defaults when absent or invalid). Useful to
    /// validate a scaffold before pushing it.
    #[must_use]
    pub fn from_files(files: &'a [(String, String)]) -> Self {
        let find = |path: &str| {
            files
                .iter()
                .find(|(candidate, _)| candidate == path)
                .map(|(_, contents)| contents.as_str())
        };
        let yulius_toml = find("yulius.toml");
        let layout = project_layout(yulius_toml);
        let decisions = match &layout.decisions {
            DecisionsLocation::File(path) => {
                find(path).map_or(DecisionFiles::Missing, DecisionFiles::File)
            }
            DecisionsLocation::Directory(directory) => {
                let prefix = format!("{directory}/");
                let entries: Vec<NamedFile<'a>> = files
                    .iter()
                    .filter_map(|(path, contents)| {
                        path.strip_prefix(&prefix)
                            .filter(|name| !name.is_empty() && !name.contains('/'))
                            .map(|name| NamedFile { name, contents })
                    })
                    .collect();
                if entries.is_empty() {
                    DecisionFiles::Missing
                } else {
                    DecisionFiles::Directory(entries)
                }
            }
        };
        Self {
            agents: find(&layout.agents),
            claude: find("CLAUDE.md"),
            yulius_toml,
            product: find(&layout.product),
            architecture: find(&layout.architecture),
            decisions,
            changes: find(&layout.changes),
        }
    }
}

/// The layout configured by `yulius.toml`, or the §7.2 defaults when it is absent or invalid.
#[must_use]
pub fn project_layout(yulius_toml: Option<&str>) -> DocsLayout {
    yulius_toml
        .and_then(|text| parse_yulius_toml(text).ok())
        .map(|config| config.docs)
        .unwrap_or_default()
}

/// What is wrong with one project file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProjectProblemKind {
    /// The file or directory does not exist.
    Missing,
    /// Only headings, comments or whitespace.
    Empty,
    /// `CLAUDE.md` neither mentions nor mirrors `AGENTS.md`.
    NotPointingToAgents,
    /// `yulius.toml` does not parse.
    InvalidConfig(FormatError),
    /// No Mermaid diagram in the architecture document.
    NoMermaidDiagram,
    /// The decisions directory has no `NNNN-<slug>.md` file.
    NoDecisions,
    /// An ADR lacks a section.
    AdrMissingSection(&'static str),
    /// An ADR section is empty.
    AdrEmptySection(&'static str),
    /// The single decisions file has no decision heading.
    NoDecisionHeadings,
    /// A directory was configured but a file was found.
    ExpectedDirectory,
    /// A file was configured but a directory was found.
    ExpectedFile,
}

impl fmt::Display for ProjectProblemKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Missing => f.write_str("is missing"),
            Self::Empty => f.write_str("has no content besides headings"),
            Self::NotPointingToAgents => {
                f.write_str("must point to AGENTS.md (mention it) or mirror its content")
            }
            Self::InvalidConfig(error) => write!(f, "line {}: {}", error.line, error.problem),
            Self::NoMermaidDiagram => f.write_str(
                "has no ```mermaid diagram; describe the architecture with at least one Mermaid diagram",
            ),
            Self::NoDecisions => f.write_str("has no ADR files named NNNN-<slug>.md"),
            Self::AdrMissingSection(section) => {
                write!(f, "ADR is missing section '## {section}'")
            }
            Self::AdrEmptySection(section) => write!(f, "ADR section '## {section}' is empty"),
            Self::NoDecisionHeadings => {
                f.write_str("has no decision headings; add one '## <decision>' per decision")
            }
            Self::ExpectedDirectory => f.write_str(
                "should be a directory of ADR files (see yulius.toml [docs] decisions)",
            ),
            Self::ExpectedFile => f.write_str(
                "should be a single decisions file (see yulius.toml [docs] decisions)",
            ),
        }
    }
}

/// A problem with a repository path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectProblem {
    /// Repository path of the file or directory.
    pub path: String,
    /// What is wrong.
    pub kind: ProjectProblemKind,
}

impl fmt::Display for ProjectProblem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.path, self.kind)
    }
}

/// Result of [`validate_project`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectReport {
    /// Problems in a stable order (yulius.toml, AGENTS.md, CLAUDE.md, product, architecture,
    /// decisions, changes).
    pub problems: Vec<ProjectProblem>,
    /// The effective layout (defaults when `yulius.toml` is missing or invalid).
    pub layout: DocsLayout,
    /// The parsed contract, when `yulius.toml` is present and valid.
    pub config: Option<ProjectConfig>,
}

impl ProjectReport {
    /// No problems.
    #[must_use]
    pub fn is_valid(&self) -> bool {
        self.problems.is_empty()
    }
}

const ADR_SECTIONS: [&str; 5] = [
    "Status",
    "Context",
    "Decision",
    "Alternatives",
    "Consequences",
];

fn has_content(text: &str) -> bool {
    markdown::scan(text).iter().any(is_meaningful)
}

fn has_mermaid(text: &str) -> bool {
    let lines = markdown::scan(text);
    let mut in_mermaid = false;
    for line in &lines {
        if let Some(info) = line.fence_info {
            in_mermaid = info
                .split_whitespace()
                .next()
                .is_some_and(|word| word.eq_ignore_ascii_case("mermaid"));
        } else if line.fence {
            in_mermaid = false;
        } else if in_mermaid && line.code && !line.text.trim().is_empty() {
            return true;
        }
    }
    false
}

fn is_adr_name(name: &str) -> bool {
    let digits = name.bytes().take_while(u8::is_ascii_digit).count();
    digits >= 4
        && name[digits..].starts_with('-')
        && name.ends_with(".md")
        && name.len() > digits + 4
}

fn adr_problems(contents: &str) -> Vec<ProjectProblemKind> {
    let lines = markdown::scan(contents);
    let found = sections(&lines);
    let mut problems = Vec::new();
    for required in ADR_SECTIONS {
        let keyword = required.to_lowercase();
        match found
            .iter()
            .find(|section| section.name.starts_with(&keyword))
        {
            None => problems.push(ProjectProblemKind::AdrMissingSection(required)),
            Some(section) => {
                let inline_status = required == "Status" && section.name != keyword;
                if !inline_status && !section.body.iter().any(|line| is_meaningful(line)) {
                    problems.push(ProjectProblemKind::AdrEmptySection(required));
                }
            }
        }
    }
    problems
}

fn headings_between(text: &str, levels: std::ops::RangeInclusive<usize>) -> Vec<String> {
    markdown::scan(text)
        .iter()
        .filter(|line| !line.code)
        .filter_map(|line| heading(&line.content))
        .filter(|(level, title)| levels.contains(level) && !title.is_empty())
        .map(|(_, title)| title.to_owned())
        .collect()
}

fn sorted_adrs<'f, 'a>(entries: &'f [NamedFile<'a>]) -> Vec<&'f NamedFile<'a>> {
    let mut adrs: Vec<&NamedFile<'a>> = entries.iter().filter(|f| is_adr_name(f.name)).collect();
    adrs.sort_by(|a, b| a.name.cmp(b.name));
    adrs
}

/// Decision titles for the briefing's decision index: each ADR's first `#` heading (or its
/// file name) in file-name order, or the `##`/`###` headings of a single decisions file.
#[must_use]
pub fn decision_titles(decisions: &DecisionFiles<'_>) -> Vec<String> {
    match decisions {
        DecisionFiles::Missing => Vec::new(),
        DecisionFiles::File(text) => headings_between(text, 2..=3),
        DecisionFiles::Directory(entries) => sorted_adrs(entries)
            .into_iter()
            .map(|adr| {
                headings_between(adr.contents, 1..=1)
                    .into_iter()
                    .next()
                    .unwrap_or_else(|| adr.name.trim_end_matches(".md").to_owned())
            })
            .collect(),
    }
}

/// Validates the enforced documentation format of a project (DSC-001, DSC-004, DSC-005).
#[must_use]
pub fn validate_project(files: &ProjectFiles<'_>) -> ProjectReport {
    let mut problems = Vec::new();
    let mut push = |path: &str, kind| {
        problems.push(ProjectProblem {
            path: path.to_owned(),
            kind,
        });
    };
    let config = match files.yulius_toml {
        None => {
            push("yulius.toml", ProjectProblemKind::Missing);
            None
        }
        Some(text) => match parse_yulius_toml(text) {
            Ok(config) => Some(config),
            Err(error) => {
                push("yulius.toml", ProjectProblemKind::InvalidConfig(error));
                None
            }
        },
    };
    let layout = config
        .as_ref()
        .map(|config| config.docs.clone())
        .unwrap_or_default();

    match files.agents {
        None => push(&layout.agents, ProjectProblemKind::Missing),
        Some(text) if !has_content(text) => push(&layout.agents, ProjectProblemKind::Empty),
        Some(_) => {}
    }
    match files.claude {
        None => push("CLAUDE.md", ProjectProblemKind::Missing),
        Some(text) => {
            let points = text.to_lowercase().contains("agents.md");
            let mirrors = files
                .agents
                .is_some_and(|agents| agents.trim() == text.trim());
            if !points && !mirrors {
                push("CLAUDE.md", ProjectProblemKind::NotPointingToAgents);
            }
        }
    }
    match files.product {
        None => push(&layout.product, ProjectProblemKind::Missing),
        Some(text) if !has_content(text) => push(&layout.product, ProjectProblemKind::Empty),
        Some(_) => {}
    }
    match files.architecture {
        None => push(&layout.architecture, ProjectProblemKind::Missing),
        Some(text) if !has_mermaid(text) => {
            push(&layout.architecture, ProjectProblemKind::NoMermaidDiagram);
        }
        Some(_) => {}
    }
    match (&layout.decisions, &files.decisions) {
        (
            DecisionsLocation::Directory(path) | DecisionsLocation::File(path),
            DecisionFiles::Missing,
        ) => {
            push(path, ProjectProblemKind::Missing);
        }
        (DecisionsLocation::Directory(directory), DecisionFiles::Directory(entries)) => {
            let adrs = sorted_adrs(entries);
            if adrs.is_empty() {
                push(directory, ProjectProblemKind::NoDecisions);
            }
            for adr in adrs {
                for kind in adr_problems(adr.contents) {
                    push(&format!("{directory}/{}", adr.name), kind);
                }
            }
        }
        (DecisionsLocation::File(path), DecisionFiles::File(text)) => {
            if headings_between(text, 2..=3).is_empty() {
                push(path, ProjectProblemKind::NoDecisionHeadings);
            }
        }
        (DecisionsLocation::Directory(path), DecisionFiles::File(_)) => {
            push(path, ProjectProblemKind::ExpectedDirectory);
        }
        (DecisionsLocation::File(path), DecisionFiles::Directory(_)) => {
            push(path, ProjectProblemKind::ExpectedFile);
        }
    }
    if files.changes.is_none() {
        push(&layout.changes, ProjectProblemKind::Missing);
    }
    ProjectReport {
        problems,
        layout,
        config,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ADR: &str = "# 1. Use SQLite\n\n## Status\n\nAccepted\n\n## Context\n\nOne VM.\n\n\
        ## Decision\n\nSQLite in WAL mode.\n\n## Alternatives\n\n- Postgres\n\n\
        ## Consequences\n\nSimple backups.\n";

    fn listing(extra: &[(&str, &str)], without: &[&str]) -> Vec<(String, String)> {
        let base = [
            ("AGENTS.md", "# Rules\n\nWrite a spec first.\n"),
            ("CLAUDE.md", "Read [AGENTS.md](AGENTS.md).\n"),
            ("yulius.toml", "[delivery]\npreset = \"standard\"\n"),
            ("docs/product.md", "# Shop\n\nSells things.\n"),
            (
                "docs/architecture.md",
                "# Architecture\n\n```mermaid\nflowchart LR\n  A --> B\n```\n",
            ),
            ("docs/decisions/0001-use-sqlite.md", ADR),
            ("docs/decisions/README.md", "Not an ADR.\n"),
            ("docs/changes.md", "# Changes\n"),
        ];
        base.iter()
            .filter(|(path, _)| !without.contains(path))
            .chain(extra)
            .map(|(path, contents)| ((*path).to_owned(), (*contents).to_owned()))
            .collect()
    }

    fn problems(files: &[(String, String)]) -> Vec<String> {
        validate_project(&ProjectFiles::from_files(files))
            .problems
            .iter()
            .map(ToString::to_string)
            .collect()
    }

    #[test]
    fn complete_project_is_valid() {
        let files = listing(&[], &[]);
        let report = validate_project(&ProjectFiles::from_files(&files));
        assert!(report.is_valid(), "{:?}", report.problems);
        assert!(report.config.is_some());
    }

    #[test]
    fn missing_files_are_reported_in_order() {
        let files = listing(
            &[],
            &[
                "AGENTS.md",
                "CLAUDE.md",
                "yulius.toml",
                "docs/product.md",
                "docs/architecture.md",
                "docs/decisions/0001-use-sqlite.md",
                "docs/decisions/README.md",
                "docs/changes.md",
            ],
        );
        assert_eq!(
            problems(&files),
            vec![
                "yulius.toml: is missing",
                "AGENTS.md: is missing",
                "CLAUDE.md: is missing",
                "docs/product.md: is missing",
                "docs/architecture.md: is missing",
                "docs/decisions: is missing",
                "docs/changes.md: is missing",
            ]
        );
    }

    #[test]
    fn content_problems() {
        let files = listing(
            &[
                ("AGENTS.md", "# Rules\n<!-- todo -->\n"),
                ("CLAUDE.md", "Claude instructions.\n"),
                ("docs/product.md", "# Shop\n"),
                (
                    "docs/architecture.md",
                    "# Architecture\n\n```text\nA -> B\n```\n```mermaid\n```\n",
                ),
                (
                    "docs/decisions/0002-bad.md",
                    "# 2. Bad\n\n## Status: proposed\n\n## Context\n\n## Decision\n\nX\n",
                ),
            ],
            &[
                "AGENTS.md",
                "CLAUDE.md",
                "docs/product.md",
                "docs/architecture.md",
            ],
        );
        assert_eq!(
            problems(&files),
            vec![
                "AGENTS.md: has no content besides headings",
                "CLAUDE.md: must point to AGENTS.md (mention it) or mirror its content",
                "docs/product.md: has no content besides headings",
                "docs/architecture.md: has no ```mermaid diagram; describe the architecture with at least one Mermaid diagram",
                "docs/decisions/0002-bad.md: ADR section '## Context' is empty",
                "docs/decisions/0002-bad.md: ADR is missing section '## Alternatives'",
                "docs/decisions/0002-bad.md: ADR is missing section '## Consequences'",
            ]
        );
    }

    #[test]
    fn claude_md_may_mirror_agents_md() {
        let files = listing(
            &[("CLAUDE.md", "# Rules\n\nWrite a spec first.\n")],
            &["CLAUDE.md"],
        );
        assert!(problems(&files).is_empty());
    }

    #[test]
    fn invalid_config_falls_back_to_default_layout() {
        let files = listing(
            &[("yulius.toml", "[docs]\nproduct = \"/abs.md\"\n")],
            &["yulius.toml"],
        );
        let report = validate_project(&ProjectFiles::from_files(&files));
        assert_eq!(report.layout, DocsLayout::default());
        assert_eq!(report.problems.len(), 1);
        assert_eq!(
            report.problems[0].to_string(),
            "yulius.toml: line 2: 'product' must be a repository-relative path: absolute paths are not allowed"
        );
    }

    #[test]
    fn single_file_decisions_studio_style() {
        let toml =
            "[docs]\nproduct = \"knowledge/product.md\"\ndecisions = \"knowledge/decisions.md\"\n";
        let files = listing(
            &[
                ("yulius.toml", toml),
                ("knowledge/product.md", "Studio is a knowledge base.\n"),
                (
                    "knowledge/decisions.md",
                    "# Decisions\n\n## 2026-09-01 Lock secrets with git-crypt\n\nBecause.\n\n### Use quartio as actor branch\n",
                ),
            ],
            &["yulius.toml", "docs/product.md"],
        );
        let report = validate_project(&ProjectFiles::from_files(&files));
        assert!(report.is_valid(), "{:?}", report.problems);
        assert_eq!(
            decision_titles(&ProjectFiles::from_files(&files).decisions),
            vec![
                "2026-09-01 Lock secrets with git-crypt",
                "Use quartio as actor branch"
            ]
        );
        let empty = listing(
            &[
                ("yulius.toml", toml),
                ("knowledge/product.md", "x\n"),
                ("knowledge/decisions.md", "# Decisions\n"),
            ],
            &["yulius.toml", "docs/product.md"],
        );
        assert_eq!(
            problems(&empty),
            vec![
                "knowledge/decisions.md: has no decision headings; add one '## <decision>' per decision"
            ]
        );
    }

    #[test]
    fn directory_without_adrs_and_titles() {
        let files = listing(&[], &["docs/decisions/0001-use-sqlite.md"]);
        assert_eq!(
            problems(&files),
            vec!["docs/decisions: has no ADR files named NNNN-<slug>.md"]
        );
        let titled = listing(&[("docs/decisions/0002-untitled.md", "## Status\n")], &[]);
        assert_eq!(
            decision_titles(&ProjectFiles::from_files(&titled).decisions),
            vec!["1. Use SQLite", "0002-untitled"]
        );
        assert!(!is_adr_name("0001.md") && !is_adr_name("01-x.md") && is_adr_name("0001-x.md"));
    }

    #[test]
    fn kind_mismatch() {
        let report = validate_project(&ProjectFiles {
            decisions: DecisionFiles::File("## A decision\n"),
            ..ProjectFiles::from_files(&listing(&[], &[]))
        });
        assert_eq!(
            report
                .problems
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>(),
            vec![
                "docs/decisions: should be a directory of ADR files (see yulius.toml [docs] decisions)"
            ]
        );
    }
}
