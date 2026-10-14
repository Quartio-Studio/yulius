//! The `yulius.toml` project contract (SPEC-001 §7.2, DLV-005).
//!
//! Choice: the domain owns a tiny, strict TOML-subset parser instead of accepting a
//! pre-parsed map, so the domain stays dependency-free and every problem carries a line
//! number for the agent prompt. Supported:
//!
//! - `[docs]`, `[commands]`, `[delivery]`, `[disk]`, `[branch_policy]`, `[resources.env]`,
//!   `[cd.dev]` and `[cd.prod]` tables, `[[delivery.environments]]` and `[[ci.jobs]]` entries;
//! - `key = "basic string"` (escapes `\" \\ \n \t \r \b \f \uXXXX \UXXXXXXXX`),
//!   `key = 'literal string'`, `key = true|false`, whole numbers (`cpus = 6`) and
//!   `environments = []`;
//! - blank lines and `#` comments (also after a value).
//!
//! Keys in the known tables are strict (unknown keys are errors, catching typos). Other
//! tables and keys before the first table are ignored for forward compatibility; their lines
//! are skipped unparsed. Multi-line strings, signed or fractional numbers, non-empty arrays,
//! inline tables, dotted and quoted keys are rejected inside known tables.
//!
//! ```toml
//! [docs]
//! decisions = "knowledge/decisions.md"   # a `.md` path selects single-file decisions
//!
//! [commands]
//! test = "cargo test"
//! gate = "scripts/ci.sh"                 # what Yulius runs as the project's CI (DLV-006)
//!
//! [delivery]
//! preset = "studio"
//! auto_promote = false                   # one change: production becomes gated
//!
//! [[ci.jobs]]                           # instead of `gate`: jobs run in parallel
//! name = "test"
//! command = "cargo test"
//! matrix = "1/2 2/2"                     # one job per value, in $YULIUS_CI_MATRIX
//! cpus = 4                               # its CPU and memory budget (SPEC-082); without them
//! memory_gb = 8                          # the run's jobs share the sandbox equally
//!
//! [resources.env]                       # the budget in this repository's tools' terms
//! CARGO_BUILD_JOBS = "{cpus}"            # also {memory_mb}, {memory_gb}; with YULIUS_CPUS and
//!                                        # YULIUS_MEMORY_MB in agent sessions and CI jobs
//!
//! [branch_policy]                      # the merge gate's rules (SPEC-069)
//! preset = "standard"                    # or "none"; types = "feat fix …" replaces the types
//!
//! [cd.dev]                              # CD (deployment, SPEC-134): both may be empty
//! deploy = "scripts/cd/deploy.sh dev"    # run by Yulius from the integration branch head
//! verify = "scripts/cd/verify.sh dev"    # a smoke test, after a successful deploy
//! secrets = "DEV_SSH_KEY DEV_HOST"       # agent secrets a person allowed outside the proxy
//!
//! [cd.prod]
//! about = "the instances update themselves from main"
//! promote = true                         # a person's deploy promotes the integration branch first
//!
//! [[delivery.environments]]
//! name = "dev-vm"
//! after = "integration"
//! command = "node scripts/deploy.mjs dev"
//! verify = "manual"
//! ```

use crate::branch_policy::BranchPolicy;
use crate::cd::{CdConfig, CdEnv, CdTarget, MAX_ABOUT_CHARS, MAX_CD_SECRETS, valid_secret_name};
use crate::delivery::{
    DeliveryProfile, Environment, MergeMethod, Preset, ProfileError, PromoteMethod, Stage, Verify,
};

use super::{Commands, DecisionsLocation, DocsLayout, ProjectConfig};
use crate::ci::{CiConfigError, JobDef, parse_matrix, project_jobs};
use crate::resources::{
    EnvTemplate, EnvTemplateError, MAX_DECLARED_CPUS, MAX_DECLARED_MEMORY_GB, MAX_ENV_ENTRIES,
    Need, check_env_template,
};

/// Largest accepted `yulius.toml`.
pub const MAX_CONFIG_BYTES: usize = 64 * 1024;
/// Longest accepted path value.
pub const MAX_PATH_BYTES: usize = 512;
/// Longest accepted command value.
pub const MAX_COMMAND_BYTES: usize = 2_000;

/// A `yulius.toml` problem with its 1-based line number.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("yulius.toml line {line}: {problem}")]
pub struct FormatError {
    /// 1-based line (1 for whole-file problems).
    pub line: usize,
    /// What is wrong.
    pub problem: ConfigProblem,
}

/// What is wrong in `yulius.toml`. Messages are stable and agent-facing.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ConfigProblem {
    /// Larger than [`MAX_CONFIG_BYTES`].
    #[error("file is larger than 64 KiB")]
    TooLarge,
    /// Not a key/value pair, header or comment.
    #[error("expected 'key = value', a [table] header or a comment")]
    ExpectedKeyValue,
    /// Malformed header.
    #[error("invalid table header")]
    InvalidHeader,
    /// A known table twice.
    #[error("table [{0}] appears more than once")]
    DuplicateTable(String),
    /// `[[docs]]` and similar.
    #[error("[{0}] must be a single table, not [[{0}]]")]
    NotAnArrayTable(String),
    /// `[delivery.environments]` without double brackets.
    #[error("write environments as [[{0}]] entries")]
    ArrayTableRequired(String),
    /// A key twice in one table.
    #[error("key '{0}' appears more than once in this table")]
    DuplicateKey(String),
    /// Unknown key in a known table.
    #[error("unknown key '{key}' in [{table}]; known keys: {known}")]
    UnknownKey {
        /// Table name.
        table: String,
        /// The unknown key.
        key: String,
        /// Accepted keys.
        known: &'static str,
    },
    /// A value outside the subset.
    #[error(
        "unsupported value; use a \"quoted string\", a whole number, true/false or [] (signed or fractional numbers, multi-line strings, arrays and inline tables are not supported)"
    )]
    UnsupportedValue,
    /// A string without its closing quote.
    #[error("unterminated string")]
    UnterminatedString,
    /// An unknown escape sequence.
    #[error("invalid escape sequence in string")]
    InvalidEscape,
    /// A raw control character inside a string.
    #[error("control characters are not allowed in strings")]
    ControlCharacter,
    /// Text after a value that is not a comment.
    #[error("unexpected text after the value")]
    TrailingText,
    /// A key needing a string.
    #[error("'{0}' must be a quoted string")]
    ExpectedString(String),
    /// A key needing a boolean.
    #[error("'{0}' must be true or false")]
    ExpectedBool(String),
    /// A key needing a whole number in a range.
    #[error("'{key}' must be a whole number from 1 to {max}")]
    ExpectedNumber {
        /// The key.
        key: String,
        /// Largest accepted value.
        max: u64,
    },
    /// An invalid `[resources.env]` entry.
    #[error("[resources.env] '{key}': {problem}")]
    ResourceEnv {
        /// The variable (clipped).
        key: String,
        /// What is wrong.
        problem: EnvTemplateError,
    },
    /// A path that is absolute, escapes the repository or is malformed.
    #[error("'{key}' must be a repository-relative path: {reason}")]
    InvalidPath {
        /// The key.
        key: String,
        /// Why.
        reason: &'static str,
    },
    /// A value outside its allowed set.
    #[error("'{key}' is '{value}'; expected one of: {allowed}")]
    InvalidValue {
        /// The key.
        key: String,
        /// The value (clipped).
        value: String,
        /// Allowed values.
        allowed: &'static str,
    },
    /// Empty, multi-line or oversized command.
    #[error("'{0}' must be a non-empty single-line command of at most 2000 bytes")]
    InvalidCommand(String),
    /// A required environment key is missing.
    #[error("[[delivery.environments]] entry is missing '{0}'")]
    MissingKey(&'static str),
    /// `environments = []` together with environment entries.
    #[error("'environments = []' conflicts with [[delivery.environments]] entries")]
    EnvironmentsConflict,
    /// The resulting profile is invalid.
    #[error("invalid delivery profile: {0}")]
    Profile(ProfileError),
    /// Invalid CI jobs (`[[ci.jobs]]`, `[commands] gate`).
    #[error("{0}")]
    Ci(CiConfigError),
    /// A `[cd.<name>]` table other than dev and prod.
    #[error("CD has two environments, [cd.dev] and [cd.prod]; '[{0}]' is neither")]
    UnknownCdEnvironment(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Value {
    Str(String),
    Bool(bool),
    Int(u64),
    EmptyArray,
}

#[derive(Debug)]
struct Entry {
    key: String,
    value: Value,
    line: usize,
}

#[derive(Debug, Default)]
struct Table {
    line: usize,
    entries: Vec<Entry>,
}

#[derive(Debug, Default)]
struct Document {
    docs: Option<Table>,
    commands: Option<Table>,
    delivery: Option<Table>,
    disk: Option<Table>,
    branch_policy: Option<Table>,
    environments: Vec<Table>,
    ci_jobs: Vec<Table>,
    resources_env: Option<Table>,
    cd_dev: Option<Table>,
    cd_prod: Option<Table>,
}

enum Target {
    Ignored,
    Docs,
    Commands,
    Delivery,
    Disk,
    BranchPolicy,
    Environment,
    CiJob,
    ResourcesEnv,
    Cd(CdEnv),
}

const DOCS_KEYS: &str = "product, architecture, decisions, specs_dir, changes, agents";
const COMMAND_KEYS: &str = "test, lint, build, gate";
const DELIVERY_KEYS: &str = "preset, integration_branch, production_branch, merge, promote, auto_promote, require_checks, environments";
const DISK_KEYS: &str = "caches";
const BRANCH_POLICY_KEYS: &str = "preset, types";
const POLICY_TYPES: &str = "1-20 space-separated types of 1-20 lowercase letters including feat (Yulius's task branches), like \"feat fix docs\"";
const ENVIRONMENT_KEYS: &str = "name, after, command, verify";
const CI_JOB_KEYS: &str = "name, command, matrix, cpus, memory_gb";
const CD_DEV_KEYS: &str = "deploy, verify, secrets, about";
const CD_PROD_KEYS: &str = "deploy, verify, secrets, about, promote";
const CD_SECRETS: &str = "at most 16 space-separated agent-secret names like DEV_SSH_KEY";
const CD_ABOUT: &str = "one line of at most 300 characters";

fn err(line: usize, problem: ConfigProblem) -> FormatError {
    FormatError { line, problem }
}

fn is_key_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_' || c == '-'
}

/// Parses a header line. `Ok(None)` when the line is not a header attempt.
fn parse_header(text: &str, line: usize) -> Result<Option<(String, bool)>, FormatError> {
    let Some(after_open) = text.strip_prefix('[') else {
        return Ok(None);
    };
    let (array, inner) = match after_open.strip_prefix('[') {
        Some(inner) => (true, inner),
        None => (false, after_open),
    };
    if !inner.trim_start().starts_with(is_key_char) {
        return Ok(None);
    }
    let close = if array { "]]" } else { "]" };
    let Some(end) = inner.find(close) else {
        return Err(err(line, ConfigProblem::InvalidHeader));
    };
    let name = inner[..end].trim();
    let rest = inner[end + close.len()..].trim();
    let valid_name = name
        .split('.')
        .all(|part| !part.is_empty() && part.chars().all(is_key_char));
    if !valid_name || !(rest.is_empty() || rest.starts_with('#')) {
        return Err(err(line, ConfigProblem::InvalidHeader));
    }
    Ok(Some((name.to_owned(), array)))
}

fn parse_basic_string(text: &str) -> Result<(String, &str), ConfigProblem> {
    let mut value = String::new();
    let mut chars = text.char_indices();
    while let Some((offset, c)) = chars.next() {
        match c {
            '"' => return Ok((value, &text[offset + 1..])),
            '\\' => {
                let (_, escape) = chars.next().ok_or(ConfigProblem::UnterminatedString)?;
                let decoded = match escape {
                    '"' => '"',
                    '\\' => '\\',
                    'n' => '\n',
                    't' => '\t',
                    'r' => '\r',
                    'b' => '\u{8}',
                    'f' => '\u{c}',
                    'u' | 'U' => {
                        let digits = if escape == 'u' { 4 } else { 8 };
                        let mut code = 0_u32;
                        for _ in 0..digits {
                            let (_, digit) = chars.next().ok_or(ConfigProblem::InvalidEscape)?;
                            code = code * 16
                                + digit.to_digit(16).ok_or(ConfigProblem::InvalidEscape)?;
                        }
                        char::from_u32(code).ok_or(ConfigProblem::InvalidEscape)?
                    }
                    _ => return Err(ConfigProblem::InvalidEscape),
                };
                value.push(decoded);
            }
            c if c.is_control() && c != '\t' => return Err(ConfigProblem::ControlCharacter),
            c => value.push(c),
        }
    }
    Err(ConfigProblem::UnterminatedString)
}

fn parse_value(text: &str) -> Result<(Value, &str), ConfigProblem> {
    if text.starts_with("\"\"\"") || text.starts_with("'''") {
        return Err(ConfigProblem::UnsupportedValue);
    }
    if let Some(rest) = text.strip_prefix('"') {
        let (value, rest) = parse_basic_string(rest)?;
        return Ok((Value::Str(value), rest));
    }
    if let Some(rest) = text.strip_prefix('\'') {
        let end = rest.find('\'').ok_or(ConfigProblem::UnterminatedString)?;
        let value = &rest[..end];
        if value.chars().any(|c| c.is_control() && c != '\t') {
            return Err(ConfigProblem::ControlCharacter);
        }
        return Ok((Value::Str(value.to_owned()), &rest[end + 1..]));
    }
    if let Some(rest) = text.strip_prefix('[') {
        let rest = rest.trim_start();
        return match rest.strip_prefix(']') {
            Some(rest) => Ok((Value::EmptyArray, rest)),
            None => Err(ConfigProblem::UnsupportedValue),
        };
    }
    let digits = text
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(text.len());
    if digits > 0 {
        let rest = &text[digits..];
        if rest.starts_with(is_key_char) || rest.starts_with('.') {
            return Err(ConfigProblem::UnsupportedValue);
        }
        let value = text[..digits]
            .parse::<u64>()
            .map_err(|_| ConfigProblem::UnsupportedValue)?;
        return Ok((Value::Int(value), rest));
    }
    for (word, value) in [("true", true), ("false", false)] {
        if let Some(rest) = text.strip_prefix(word)
            && !rest.starts_with(is_key_char)
        {
            return Ok((Value::Bool(value), rest));
        }
    }
    Err(ConfigProblem::UnsupportedValue)
}

fn parse_entry(text: &str, line: usize) -> Result<Entry, FormatError> {
    let key_end = text.find(|c: char| !is_key_char(c)).unwrap_or(text.len());
    let key = &text[..key_end];
    let Some(after_equals) = text[key_end..].trim_start().strip_prefix('=') else {
        return Err(err(line, ConfigProblem::ExpectedKeyValue));
    };
    if key.is_empty() {
        return Err(err(line, ConfigProblem::ExpectedKeyValue));
    }
    let (value, rest) = parse_value(after_equals.trim_start()).map_err(|p| err(line, p))?;
    let rest = rest.trim();
    if !(rest.is_empty() || rest.starts_with('#')) {
        return Err(err(line, ConfigProblem::TrailingText));
    }
    Ok(Entry {
        key: key.to_owned(),
        value,
        line,
    })
}

fn parse_document(text: &str) -> Result<Document, FormatError> {
    if text.len() > MAX_CONFIG_BYTES {
        return Err(err(1, ConfigProblem::TooLarge));
    }
    let mut document = Document::default();
    let mut target = Target::Ignored;
    for (index, raw) in text.lines().enumerate() {
        let line = index + 1;
        let raw = if index == 0 {
            raw.strip_prefix('\u{feff}').unwrap_or(raw)
        } else {
            raw
        };
        let trimmed = raw.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        if let Some((name, array)) = parse_header(trimmed, line)? {
            let fresh = Table {
                line,
                entries: Vec::new(),
            };
            target = match (name.as_str(), array) {
                (
                    "docs" | "commands" | "delivery" | "disk" | "branch_policy" | "resources.env",
                    true,
                ) => {
                    return Err(err(line, ConfigProblem::NotAnArrayTable(name)));
                }
                ("cd.dev" | "cd.prod", true) => {
                    return Err(err(line, ConfigProblem::NotAnArrayTable(name)));
                }
                ("cd.dev" | "cd.prod", false) => {
                    let (slot, env) = if name == "cd.dev" {
                        (&mut document.cd_dev, CdEnv::Dev)
                    } else {
                        (&mut document.cd_prod, CdEnv::Prod)
                    };
                    if slot.is_some() {
                        return Err(err(line, ConfigProblem::DuplicateTable(name)));
                    }
                    *slot = Some(fresh);
                    Target::Cd(env)
                }
                (other, _) if other == "cd" || other.starts_with("cd.") => {
                    return Err(err(line, ConfigProblem::UnknownCdEnvironment(name)));
                }
                ("resources.env", false) => {
                    if document.resources_env.is_some() {
                        return Err(err(line, ConfigProblem::DuplicateTable(name)));
                    }
                    document.resources_env = Some(fresh);
                    Target::ResourcesEnv
                }
                ("delivery.environments", false) => {
                    return Err(err(line, ConfigProblem::ArrayTableRequired(name)));
                }
                ("delivery.environments", true) => {
                    document.environments.push(fresh);
                    Target::Environment
                }
                ("ci.jobs", false) => {
                    return Err(err(line, ConfigProblem::ArrayTableRequired(name)));
                }
                ("ci.jobs", true) => {
                    document.ci_jobs.push(fresh);
                    Target::CiJob
                }
                (known @ ("docs" | "commands" | "delivery" | "disk" | "branch_policy"), false) => {
                    let (slot, target) = match known {
                        "docs" => (&mut document.docs, Target::Docs),
                        "commands" => (&mut document.commands, Target::Commands),
                        "disk" => (&mut document.disk, Target::Disk),
                        "branch_policy" => (&mut document.branch_policy, Target::BranchPolicy),
                        _ => (&mut document.delivery, Target::Delivery),
                    };
                    if slot.is_some() {
                        return Err(err(line, ConfigProblem::DuplicateTable(name)));
                    }
                    *slot = Some(fresh);
                    target
                }
                _ => Target::Ignored,
            };
            continue;
        }
        let table = match target {
            Target::Ignored => continue,
            Target::Docs => document.docs.as_mut(),
            Target::Commands => document.commands.as_mut(),
            Target::Delivery => document.delivery.as_mut(),
            Target::Disk => document.disk.as_mut(),
            Target::BranchPolicy => document.branch_policy.as_mut(),
            Target::Environment => document.environments.last_mut(),
            Target::CiJob => document.ci_jobs.last_mut(),
            Target::ResourcesEnv => document.resources_env.as_mut(),
            Target::Cd(CdEnv::Dev) => document.cd_dev.as_mut(),
            Target::Cd(CdEnv::Prod) => document.cd_prod.as_mut(),
        };
        let Some(table) = table else { continue };
        let entry = parse_entry(trimmed, line)?;
        if table
            .entries
            .iter()
            .any(|existing| existing.key == entry.key)
        {
            return Err(err(line, ConfigProblem::DuplicateKey(entry.key)));
        }
        table.entries.push(entry);
    }
    Ok(document)
}

fn string_value(entry: &Entry) -> Result<&str, FormatError> {
    match &entry.value {
        Value::Str(text) => Ok(text),
        _ => Err(err(
            entry.line,
            ConfigProblem::ExpectedString(entry.key.clone()),
        )),
    }
}

fn bool_value(entry: &Entry) -> Result<bool, FormatError> {
    match entry.value {
        Value::Bool(value) => Ok(value),
        _ => Err(err(
            entry.line,
            ConfigProblem::ExpectedBool(entry.key.clone()),
        )),
    }
}

fn number_value(entry: &Entry, max: u64) -> Result<u64, FormatError> {
    match entry.value {
        Value::Int(value) if (1..=max).contains(&value) => Ok(value),
        _ => Err(err(
            entry.line,
            ConfigProblem::ExpectedNumber {
                key: entry.key.clone(),
                max,
            },
        )),
    }
}

fn clip(text: &str) -> String {
    let mut end = text.len().min(64);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text[..end].to_owned()
}

fn enum_value<T>(
    entry: &Entry,
    parse: fn(&str) -> Option<T>,
    allowed: &'static str,
) -> Result<T, FormatError> {
    let text = string_value(entry)?;
    parse(text).ok_or_else(|| {
        err(
            entry.line,
            ConfigProblem::InvalidValue {
                key: entry.key.clone(),
                value: clip(text),
                allowed,
            },
        )
    })
}

fn unknown(table: &str, entry: &Entry, known: &'static str) -> FormatError {
    err(
        entry.line,
        ConfigProblem::UnknownKey {
            table: table.to_owned(),
            key: entry.key.clone(),
            known,
        },
    )
}

/// Validates a repository-relative path and strips one trailing `/`.
pub(crate) fn check_repo_path(path: &str) -> Result<String, &'static str> {
    let trimmed = path.strip_suffix('/').unwrap_or(path);
    if trimmed.is_empty() {
        return Err("empty");
    }
    if trimmed.len() > MAX_PATH_BYTES {
        return Err("longer than 512 bytes");
    }
    if trimmed
        .chars()
        .any(|c| c.is_control() || matches!(c, '\\' | '*' | '?'))
    {
        return Err("contains a control character, '\\', '*' or '?'");
    }
    let bytes = trimmed.as_bytes();
    if trimmed.starts_with('/') || (bytes.len() >= 2 && bytes[1] == b':') {
        return Err("absolute paths are not allowed");
    }
    if trimmed
        .split('/')
        .any(|part| matches!(part, "" | "." | ".."))
    {
        return Err("empty, '.' or '..' segments are not allowed");
    }
    Ok(trimmed.to_owned())
}

fn docs_layout(table: Option<&Table>) -> Result<DocsLayout, FormatError> {
    let mut layout = DocsLayout::default();
    for entry in table.map(|t| t.entries.as_slice()).unwrap_or_default() {
        let path = check_repo_path(string_value(entry)?).map_err(|reason| {
            err(
                entry.line,
                ConfigProblem::InvalidPath {
                    key: entry.key.clone(),
                    reason,
                },
            )
        })?;
        match entry.key.as_str() {
            "product" => layout.product = path,
            "architecture" => layout.architecture = path,
            "decisions" if path.ends_with(".md") => {
                layout.decisions = DecisionsLocation::File(path)
            }
            "decisions" => layout.decisions = DecisionsLocation::Directory(path),
            "specs_dir" => layout.specs_dir = path,
            "changes" => layout.changes = path,
            "agents" => layout.agents = path,
            _ => return Err(unknown("docs", entry, DOCS_KEYS)),
        }
    }
    Ok(layout)
}

fn command(entry: &Entry) -> Result<String, FormatError> {
    let text = string_value(entry)?.trim();
    if text.is_empty() || text.len() > MAX_COMMAND_BYTES || text.contains(['\n', '\r', '\0']) {
        return Err(err(
            entry.line,
            ConfigProblem::InvalidCommand(entry.key.clone()),
        ));
    }
    Ok(text.to_owned())
}

fn commands(table: Option<&Table>) -> Result<Commands, FormatError> {
    let mut commands = Commands::default();
    for entry in table.map(|t| t.entries.as_slice()).unwrap_or_default() {
        let value = Some(command(entry)?);
        match entry.key.as_str() {
            "test" => commands.test = value,
            "lint" => commands.lint = value,
            "build" => commands.build = value,
            "gate" => commands.gate = value,
            _ => return Err(unknown("commands", entry, COMMAND_KEYS)),
        }
    }
    Ok(commands)
}

fn environment(table: &Table) -> Result<Environment, FormatError> {
    let (mut name, mut after, mut command_text, mut verify) = (None, None, None, None);
    for entry in &table.entries {
        match entry.key.as_str() {
            "name" => name = Some(string_value(entry)?.to_owned()),
            "after" => {
                after = Some(enum_value(entry, Stage::parse, "integration, production")?);
            }
            "command" => command_text = Some(command(entry)?),
            "verify" => verify = Some(enum_value(entry, Verify::parse, "manual, command")?),
            _ => return Err(unknown("[delivery.environments]", entry, ENVIRONMENT_KEYS)),
        }
    }
    Ok(Environment {
        name: name.ok_or_else(|| err(table.line, ConfigProblem::MissingKey("name")))?,
        after: after.ok_or_else(|| err(table.line, ConfigProblem::MissingKey("after")))?,
        command: command_text,
        verify: verify.unwrap_or(Verify::Manual),
    })
}

fn ci_job(table: &Table) -> Result<JobDef, FormatError> {
    let (mut name, mut command_text, mut matrix) = (None, None, Vec::new());
    let mut need = Need::default();
    for entry in &table.entries {
        match entry.key.as_str() {
            "name" => name = Some(string_value(entry)?.to_owned()),
            "command" => command_text = Some(command(entry)?),
            "matrix" => {
                let job = name.as_deref().unwrap_or("?");
                matrix = parse_matrix(job, string_value(entry)?)
                    .map_err(|e| err(entry.line, ConfigProblem::Ci(e)))?;
            }
            "cpus" => {
                let cpus = number_value(entry, MAX_DECLARED_CPUS)?;
                need.cpus = Some(u32::try_from(cpus).unwrap_or(u32::MAX));
            }
            "memory_gb" => {
                need.memory_mb = Some(number_value(entry, MAX_DECLARED_MEMORY_GB)? * 1024);
            }
            _ => return Err(unknown("[ci.jobs]", entry, CI_JOB_KEYS)),
        }
    }
    Ok(JobDef {
        name: name.ok_or_else(|| err(table.line, ConfigProblem::MissingKey("name")))?,
        command: command_text
            .ok_or_else(|| err(table.line, ConfigProblem::MissingKey("command")))?,
        matrix,
        need,
    })
}

fn resources_env(document: &Document) -> Result<Vec<EnvTemplate>, FormatError> {
    let entries = document
        .resources_env
        .as_ref()
        .map(|t| t.entries.as_slice())
        .unwrap_or_default();
    let mut templates = Vec::with_capacity(entries.len());
    for entry in entries {
        let template = string_value(entry)?;
        let problem = if templates.len() == MAX_ENV_ENTRIES {
            Err(EnvTemplateError::TooMany)
        } else {
            check_env_template(&entry.key, template)
        };
        problem.map_err(|problem| {
            err(
                entry.line,
                ConfigProblem::ResourceEnv {
                    key: clip(&entry.key),
                    problem,
                },
            )
        })?;
        templates.push(EnvTemplate {
            name: entry.key.clone(),
            template: template.to_owned(),
        });
    }
    Ok(templates)
}

fn ci_jobs(document: &Document, commands: &Commands) -> Result<Vec<JobDef>, FormatError> {
    let defs = document
        .ci_jobs
        .iter()
        .map(ci_job)
        .collect::<Result<Vec<_>, _>>()?;
    let line = document.ci_jobs.first().map_or_else(
        || document.commands.as_ref().map_or(1, |t| t.line),
        |t| t.line,
    );
    project_jobs(commands.gate.as_deref(), &defs).map_err(|e| err(line, ConfigProblem::Ci(e)))?;
    Ok(defs)
}

fn delivery(document: &Document) -> Result<DeliveryProfile, FormatError> {
    let entries = document
        .delivery
        .as_ref()
        .map(|t| t.entries.as_slice())
        .unwrap_or_default();
    let preset = match entries.iter().find(|entry| entry.key == "preset") {
        Some(entry) => enum_value(entry, Preset::parse, "standard, product, studio, custom")?,
        None => Preset::Standard,
    };
    let mut profile = DeliveryProfile::for_preset(preset);
    let mut cleared = None;
    for entry in entries {
        match entry.key.as_str() {
            "preset" => {}
            "integration_branch" => profile.integration_branch = string_value(entry)?.to_owned(),
            "production_branch" => profile.production_branch = string_value(entry)?.to_owned(),
            "merge" => {
                profile.merge = enum_value(entry, MergeMethod::parse, "squash, merge, rebase")?
            }
            "promote" => {
                profile.promote =
                    enum_value(entry, PromoteMethod::parse, "merge_commit, fast_forward")?;
            }
            "auto_promote" => profile.auto_promote = bool_value(entry)?,
            "require_checks" => profile.require_checks = bool_value(entry)?,
            "environments" => {
                if entry.value != Value::EmptyArray {
                    return Err(err(
                        entry.line,
                        ConfigProblem::InvalidValue {
                            key: entry.key.clone(),
                            value: match &entry.value {
                                Value::Str(text) => clip(text),
                                Value::Bool(value) => value.to_string(),
                                Value::Int(value) => value.to_string(),
                                Value::EmptyArray => String::new(),
                            },
                            allowed: "[] (define entries with [[delivery.environments]])",
                        },
                    ));
                }
                profile.environments.clear();
                cleared = Some(entry.line);
            }
            _ => return Err(unknown("delivery", entry, DELIVERY_KEYS)),
        }
    }
    if !document.environments.is_empty() {
        if let Some(line) = cleared {
            return Err(err(line, ConfigProblem::EnvironmentsConflict));
        }
        profile.environments = document
            .environments
            .iter()
            .map(environment)
            .collect::<Result<_, _>>()?;
    }
    // DLV-010: the manual `dev` checkpoint of the former standard preset is gone.
    if preset == Preset::Standard
        && matches!(profile.environments.as_slice(), [only] if DeliveryProfile::is_legacy_standard_checkpoint(only))
    {
        profile.environments.clear();
    }
    let line = document.delivery.as_ref().map_or_else(
        || document.environments.first().map_or(1, |t| t.line),
        |t| t.line,
    );
    profile
        .validate()
        .map_err(|e| err(line, ConfigProblem::Profile(e)))?;
    Ok(profile.relabeled())
}

/// Parses `yulius.toml` into the project contract. Missing tables take defaults: the §7.2
/// layout, no commands and the standard delivery profile. A `[delivery]` table starts from
/// its `preset` (standard when absent) and applies the other keys as overrides; a profile
/// that then differs from its preset is labelled [`Preset::Custom`].
///
/// # Errors
/// Returns the first [`FormatError`] with its line number.
///
/// # Examples
/// ```
/// use yulius_domain::delivery::Preset;
/// use yulius_domain::spec_format::parse_yulius_toml;
/// let config = parse_yulius_toml("[delivery]\npreset = \"studio\"\n")?;
/// assert_eq!(config.delivery.preset, Preset::Studio);
/// assert!(config.delivery.auto_promote);
/// # Ok::<(), yulius_domain::spec_format::FormatError>(())
/// ```
pub fn parse_yulius_toml(text: &str) -> Result<ProjectConfig, FormatError> {
    let document = parse_document(text)?;
    let commands = commands(document.commands.as_ref())?;
    let ci = ci_jobs(&document, &commands)?;
    branch_policy_of(&document)?;
    resources_env(&document)?;
    cd_of(&document)?;
    Ok(ProjectConfig {
        docs: docs_layout(document.docs.as_ref())?,
        commands,
        ci,
        delivery: delivery(&document)?,
    })
}

fn cd_target(table: &Table, env: CdEnv) -> Result<CdTarget, FormatError> {
    let (name, known) = match env {
        CdEnv::Dev => ("cd.dev", CD_DEV_KEYS),
        CdEnv::Prod => ("cd.prod", CD_PROD_KEYS),
    };
    let invalid = |entry: &Entry, value: &str, allowed| {
        err(
            entry.line,
            ConfigProblem::InvalidValue {
                key: entry.key.clone(),
                value: clip(value),
                allowed,
            },
        )
    };
    let mut target = CdTarget::default();
    for entry in &table.entries {
        match entry.key.as_str() {
            "deploy" => target.deploy = Some(command(entry)?),
            "verify" => target.verify = Some(command(entry)?),
            "secrets" => {
                let text = string_value(entry)?;
                let names: Vec<String> = text.split_whitespace().map(str::to_owned).collect();
                if names.len() > MAX_CD_SECRETS {
                    return Err(invalid(entry, text, CD_SECRETS));
                }
                for secret in &names {
                    if !valid_secret_name(secret) {
                        return Err(invalid(entry, secret, CD_SECRETS));
                    }
                }
                target.secrets = Vec::new();
                for secret in names {
                    if !target.secrets.contains(&secret) {
                        target.secrets.push(secret);
                    }
                }
            }
            "about" => {
                let text = string_value(entry)?.trim();
                if text.chars().count() > MAX_ABOUT_CHARS || text.contains(['\n', '\r']) {
                    return Err(invalid(entry, text, CD_ABOUT));
                }
                target.about = (!text.is_empty()).then(|| text.to_owned());
            }
            "promote" if env == CdEnv::Prod => target.promote = bool_value(entry)?,
            _ => return Err(unknown(name, entry, known)),
        }
    }
    Ok(target)
}

fn cd_of(document: &Document) -> Result<CdConfig, FormatError> {
    Ok(CdConfig {
        dev: document
            .cd_dev
            .as_ref()
            .map(|table| cd_target(table, CdEnv::Dev))
            .transpose()?,
        prod: document
            .cd_prod
            .as_ref()
            .map(|table| cd_target(table, CdEnv::Prod))
            .transpose()?,
    })
}

/// The project's CD (deployment, SPEC-134): `[cd.dev]` and `[cd.prod]`, each `None` when its
/// table is missing. Only these tables are checked, so a mistake elsewhere does not hide CD.
///
/// # Errors
/// A syntax error of the file or an invalid CD table.
///
/// # Examples
/// ```
/// use yulius_domain::cd::{CdEnv, CdState};
/// use yulius_domain::spec_format::parse_yulius_toml_cd;
/// let cd = parse_yulius_toml_cd("[cd.dev]\n[cd.prod]\npromote = true\n")?;
/// assert_eq!(cd.state(CdEnv::Dev), CdState::Empty);
/// assert_eq!(cd.state(CdEnv::Prod), CdState::Ready);
/// # Ok::<(), yulius_domain::spec_format::FormatError>(())
/// ```
pub fn parse_yulius_toml_cd(text: &str) -> Result<CdConfig, FormatError> {
    parse_document(text).and_then(|document| cd_of(&document))
}

/// Parses only the `[docs]` layout of `yulius.toml` (the whole file is still validated).
///
/// # Errors
/// Returns the first [`FormatError`] with its line number.
pub fn parse_yulius_toml_docs(text: &str) -> Result<DocsLayout, FormatError> {
    parse_yulius_toml(text).map(|config| config.docs)
}

fn branch_policy_of(document: &Document) -> Result<BranchPolicy, FormatError> {
    let entries = document
        .branch_policy
        .as_ref()
        .map(|t| t.entries.as_slice())
        .unwrap_or_default();
    let mut enforced = true;
    let mut types = None;
    for entry in entries {
        match entry.key.as_str() {
            "preset" => {
                enforced = enum_value(
                    entry,
                    |text| match text {
                        "standard" => Some(true),
                        "none" => Some(false),
                        _ => None,
                    },
                    "standard, none",
                )?;
            }
            "types" => types = Some(entry),
            _ => return Err(unknown("branch_policy", entry, BRANCH_POLICY_KEYS)),
        }
    }
    let Some(entry) = types else {
        return Ok(if enforced {
            BranchPolicy::standard()
        } else {
            BranchPolicy::none()
        });
    };
    let text = string_value(entry)?;
    let invalid = |allowed| {
        err(
            entry.line,
            ConfigProblem::InvalidValue {
                key: entry.key.clone(),
                value: clip(text),
                allowed,
            },
        )
    };
    if !enforced {
        return Err(invalid("no types with preset \"none\""));
    }
    let list: Vec<String> = text.split_whitespace().map(str::to_owned).collect();
    // Yulius names task branches `feat/<n>-<words>`: without `feat` it could never merge them.
    if !list.iter().any(|t| t == "feat") {
        return Err(invalid(POLICY_TYPES));
    }
    BranchPolicy::with_types(list).ok_or_else(|| invalid(POLICY_TYPES))
}

/// The branch policy `yulius.toml` configures (`[branch_policy]`, SPEC-069): the standard preset
/// without the table. The syntax of the whole file is still validated.
///
/// # Errors
/// The first [`FormatError`] of the file.
///
/// # Examples
/// ```
/// use yulius_domain::spec_format::parse_yulius_toml_branch_policy;
/// let policy = parse_yulius_toml_branch_policy("[branch_policy]\npreset = \"none\"\n")?;
/// assert!(!policy.is_enforced());
/// # Ok::<(), yulius_domain::spec_format::FormatError>(())
/// ```
pub fn parse_yulius_toml_branch_policy(text: &str) -> Result<BranchPolicy, FormatError> {
    parse_document(text).and_then(|document| branch_policy_of(&document))
}

/// The build-cache directories `yulius.toml` declares (`[disk] caches = "target build/cache"`,
/// space-separated, relative to the repository root; SPEC-008). A file that does not parse
/// declares none.
///
/// # Errors
/// The first [`FormatError`] of the file.
pub fn parse_yulius_toml_caches(text: &str) -> Result<Vec<String>, FormatError> {
    let document = parse_document(text)?;
    let mut caches = Vec::new();
    for entry in document
        .disk
        .as_ref()
        .map(|t| t.entries.as_slice())
        .unwrap_or_default()
    {
        if entry.key != "caches" {
            return Err(unknown("disk", entry, DISK_KEYS));
        }
        for path in string_value(entry)?.split_whitespace() {
            let path = check_repo_path(path).map_err(|reason| {
                err(
                    entry.line,
                    ConfigProblem::InvalidValue {
                        key: entry.key.clone(),
                        value: clip(path),
                        allowed: reason,
                    },
                )
            })?;
            caches.push(path);
        }
    }
    Ok(caches)
}

/// The repository's `[resources.env]` (SPEC-082 RES-004): the variables its tools read, as
/// templates of the budget Yulius gives each process. Only this table is checked, so one wrong
/// key elsewhere does not take the budget away from every session.
///
/// # Errors
/// A syntax error of the file or an invalid entry of the table.
///
/// # Examples
/// ```
/// use yulius_domain::spec_format::parse_yulius_toml_resources;
/// let env = parse_yulius_toml_resources("[resources.env]\nMAKEFLAGS = \"-j{cpus}\"\n")?;
/// assert_eq!(env[0].name, "MAKEFLAGS");
/// assert_eq!(env[0].template, "-j{cpus}");
/// # Ok::<(), yulius_domain::spec_format::FormatError>(())
/// ```
pub fn parse_yulius_toml_resources(text: &str) -> Result<Vec<EnvTemplate>, FormatError> {
    parse_document(text).and_then(|document| resources_env(&document))
}

fn quote(text: &str) -> String {
    let mut quoted = String::with_capacity(text.len() + 2);
    quoted.push('"');
    for c in text.chars() {
        match c {
            '"' => quoted.push_str("\\\""),
            '\\' => quoted.push_str("\\\\"),
            '\n' => quoted.push_str("\\n"),
            '\t' => quoted.push_str("\\t"),
            '\r' => quoted.push_str("\\r"),
            c if c.is_control() => quoted.push_str(&format!("\\u{:04X}", u32::from(c))),
            c => quoted.push(c),
        }
    }
    quoted.push('"');
    quoted
}

/// Renders a project contract as `yulius.toml` that [`parse_yulius_toml`] reads back to an
/// equal value. Delivery settings are written as the preset plus the keys that differ from
/// it.
#[must_use]
pub fn render_yulius_toml(config: &ProjectConfig) -> String {
    let docs = &config.docs;
    let decisions = match &docs.decisions {
        DecisionsLocation::Directory(path) | DecisionsLocation::File(path) => path,
    };
    let mut out = String::from(
        "# Yulius project contract: documentation paths, verification commands and delivery profile.\n\n[docs]\n",
    );
    for (key, value) in [
        ("product", &docs.product),
        ("architecture", &docs.architecture),
        ("decisions", decisions),
        ("specs_dir", &docs.specs_dir),
        ("changes", &docs.changes),
        ("agents", &docs.agents),
    ] {
        out.push_str(&format!("{key} = {}\n", quote(value)));
    }
    out.push_str("\n[commands]\n");
    for (key, value) in [
        ("test", &config.commands.test),
        ("lint", &config.commands.lint),
        ("build", &config.commands.build),
        ("gate", &config.commands.gate),
    ] {
        if let Some(command) = value {
            out.push_str(&format!("{key} = {}\n", quote(command)));
        }
    }
    for job in &config.ci {
        out.push_str(&format!(
            "\n[[ci.jobs]]\nname = {}\ncommand = {}\n",
            quote(&job.name),
            quote(&job.command)
        ));
        if !job.matrix.is_empty() {
            out.push_str(&format!("matrix = {}\n", quote(&job.matrix.join(" "))));
        }
        if let Some(cpus) = job.need.cpus {
            out.push_str(&format!("cpus = {cpus}\n"));
        }
        if let Some(memory_mb) = job.need.memory_mb {
            out.push_str(&format!("memory_gb = {}\n", memory_mb / 1024));
        }
    }
    let profile = &config.delivery;
    let base = DeliveryProfile::for_preset(profile.preset);
    out.push_str(&format!(
        "\n[delivery]\npreset = {}\n",
        quote(profile.preset.as_str())
    ));
    let overrides = [
        (
            "integration_branch",
            profile.integration_branch != base.integration_branch,
            quote(&profile.integration_branch),
        ),
        (
            "production_branch",
            profile.production_branch != base.production_branch,
            quote(&profile.production_branch),
        ),
        (
            "merge",
            profile.merge != base.merge,
            quote(profile.merge.as_str()),
        ),
        (
            "promote",
            profile.promote != base.promote,
            quote(profile.promote.as_str()),
        ),
        (
            "auto_promote",
            profile.auto_promote != base.auto_promote,
            profile.auto_promote.to_string(),
        ),
        (
            "require_checks",
            profile.require_checks != base.require_checks,
            profile.require_checks.to_string(),
        ),
    ];
    for (key, differs, value) in overrides {
        if differs {
            out.push_str(&format!("{key} = {value}\n"));
        }
    }
    if profile.environments != base.environments {
        if profile.environments.is_empty() {
            out.push_str("environments = []\n");
        }
        for environment in &profile.environments {
            out.push_str(&format!(
                "\n[[delivery.environments]]\nname = {}\nafter = {}\n",
                quote(&environment.name),
                quote(environment.after.as_str())
            ));
            if let Some(command) = &environment.command {
                out.push_str(&format!("command = {}\n", quote(command)));
            }
            out.push_str(&format!(
                "verify = {}\n",
                quote(environment.verify.as_str())
            ));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cd::CdState;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    fn problem(text: &str) -> Option<(usize, ConfigProblem)> {
        parse_yulius_toml(text).err().map(|e| (e.line, e.problem))
    }

    /// SPEC-134 CDC-001/002: both tables, an empty one, a missing one.
    #[test]
    fn cd_tables_define_dev_and_production() -> TestResult {
        let cd = parse_yulius_toml_cd(
            "[cd.dev]\nabout = \"no dev machine yet\"\n\n[cd.prod]\npromote = true\n\
             deploy = \"scripts/cd/deploy.sh prod\"\nverify = 'scripts/cd/verify.sh prod'\n\
             secrets = \"PROD_SSH_KEY  PROD_HOST\"\n",
        )?;
        let dev = cd.dev.as_ref().ok_or("dev")?;
        assert_eq!(dev.about.as_deref(), Some("no dev machine yet"));
        assert_eq!(cd.state(CdEnv::Dev), CdState::Empty);
        let prod = cd.prod.as_ref().ok_or("prod")?;
        assert!(prod.promote);
        assert_eq!(prod.deploy.as_deref(), Some("scripts/cd/deploy.sh prod"));
        assert_eq!(prod.verify.as_deref(), Some("scripts/cd/verify.sh prod"));
        assert_eq!(prod.secrets, ["PROD_SSH_KEY", "PROD_HOST"]);
        assert!(cd.is_defined());
        let none = parse_yulius_toml_cd("[commands]\ntest = \"x\"\n")?;
        assert_eq!(none, CdConfig::default());
        assert_eq!(none.state(CdEnv::Prod), CdState::NotDefined);
        // The whole file is validated with them.
        assert!(parse_yulius_toml("[cd.dev]\n[cd.prod]\n").is_ok());
        Ok(())
    }

    /// SPEC-134 CDC-001: every invalid form names its line.
    #[test]
    fn cd_tables_refuse_what_cannot_run() {
        let cases: [(&str, usize, &str); 9] = [
            (
                "[cd.dev]\npromote = true\n",
                2,
                "unknown key 'promote' in [cd.dev]",
            ),
            (
                "[cd.prod]\nrun = \"x\"\n",
                2,
                "unknown key 'run' in [cd.prod]",
            ),
            (
                "[cd.dev]\ndeploy = \"\"\n",
                2,
                "'deploy' must be a non-empty",
            ),
            (
                "[cd.dev]\ndeploy = \"a\\nb\"\n",
                2,
                "'deploy' must be a non-empty",
            ),
            ("[cd.dev]\nsecrets = \"ssh_key\"\n", 2, "'secrets'"),
            ("[cd.staging]\n", 1, "[cd.dev] and [cd.prod]"),
            ("[[cd.dev]]\n", 1, "[cd.dev] must be a single table"),
            ("[cd.dev]\n[cd.dev]\n", 2, "appears more than once"),
            (
                "[cd.prod]\npromote = \"yes\"\n",
                2,
                "'promote' must be true or false",
            ),
        ];
        for (text, line, message) in cases {
            let error = parse_yulius_toml_cd(text).err();
            assert_eq!(error.as_ref().map(|e| e.line), Some(line), "{text}");
            let shown = error.map(|e| e.to_string()).unwrap_or_default();
            assert!(shown.contains(message), "{text}: {shown}");
            assert!(parse_yulius_toml(text).is_err(), "{text}");
        }
        let many: Vec<String> = (0..17).map(|n| format!("KEY_{n}")).collect();
        let text = format!("[cd.dev]\nsecrets = \"{}\"\n", many.join(" "));
        assert!(parse_yulius_toml_cd(&text).is_err_and(|e| e.to_string().contains("'secrets'")));
        let text = format!("[cd.dev]\nabout = \"{}\"\n", "a".repeat(301));
        assert!(parse_yulius_toml_cd(&text).is_err_and(|e| e.to_string().contains("'about'")));
    }

    #[test]
    fn empty_file_gives_defaults() -> TestResult {
        let config = parse_yulius_toml("")?;
        assert_eq!(config, ProjectConfig::default());
        assert_eq!(
            config.docs.decisions,
            DecisionsLocation::Directory("docs/decisions".to_owned())
        );
        assert_eq!(config.delivery, DeliveryProfile::standard());
        Ok(())
    }

    #[test]
    fn full_file_with_comments_escapes_and_ignored_tables() -> TestResult {
        let text = "\u{feff}# top comment\nversion = 3 # root keys are ignored\n\n\
            [docs]\n\
            product = \"knowledge/product.md\"   # comment\n\
            decisions = 'knowledge/decisions.md'\n\
            specs_dir = \"docs/specs/\"\n\
            \n[tool.other]\nanything = [1, 2]\nlist = [\n  \"x\",\n]\n\
            [commands]\n\
            test = \"node scripts/test-all.mjs \\\"all\\\"\"\n\
            lint = \"eslint\\u0020.\"\n\
            gate = \"scripts/dev ci\"\n\
            [delivery]\n\
            preset = \"product\"\n";
        let config = parse_yulius_toml(text)?;
        assert_eq!(config.docs.product, "knowledge/product.md");
        assert_eq!(
            config.docs.decisions,
            DecisionsLocation::File("knowledge/decisions.md".to_owned())
        );
        assert_eq!(config.docs.specs_dir, "docs/specs");
        assert_eq!(config.docs.architecture, "docs/architecture.md");
        assert_eq!(
            config.commands.test.as_deref(),
            Some("node scripts/test-all.mjs \"all\"")
        );
        assert_eq!(config.commands.lint.as_deref(), Some("eslint ."));
        assert_eq!(config.commands.build, None);
        assert_eq!(config.commands.gate.as_deref(), Some("scripts/dev ci"));
        assert_eq!(config.delivery, DeliveryProfile::product());
        Ok(())
    }

    #[test]
    fn studio_gated_with_one_change() -> TestResult {
        let studio = parse_yulius_toml("[delivery]\npreset = \"studio\"\n")?;
        assert_eq!(studio.delivery, DeliveryProfile::studio());
        let gated = parse_yulius_toml("[delivery]\npreset = \"studio\"\nauto_promote = false\n")?;
        assert_eq!(gated.delivery.preset, Preset::Custom);
        assert!(!gated.delivery.auto_promote);
        assert_eq!(gated.delivery.integration_branch, "quartio");
        Ok(())
    }

    #[test]
    fn environments_entries_and_clearing() -> TestResult {
        let text = "[delivery]\npreset = \"product\"\n\n\
            [[delivery.environments]]\nname = \"dev-vm\"\nafter = \"integration\"\n\
            command = \"node scripts/deploy.mjs dev\"\n\n\
            [[delivery.environments]]\nname = \"rollout\"\nafter = \"production\"\n";
        let config = parse_yulius_toml(text)?;
        assert_eq!(config.delivery.preset, Preset::Custom);
        assert_eq!(config.delivery.environments.len(), 2);
        assert_eq!(
            config.delivery.environments[0].command.as_deref(),
            Some("node scripts/deploy.mjs dev")
        );
        assert_eq!(config.delivery.environments[1].verify, Verify::Manual);
        let cleared = parse_yulius_toml("[delivery]\nenvironments = []\n")?;
        assert!(cleared.delivery.environments.is_empty());
        assert_eq!(cleared.delivery.preset, Preset::Standard);
        assert_eq!(
            problem(
                "[delivery]\nenvironments = []\n[[delivery.environments]]\nname = \"x\"\nafter = \"integration\"\n"
            ),
            Some((2, ConfigProblem::EnvironmentsConflict))
        );
        assert_eq!(
            problem("[[delivery.environments]]\nafter = \"integration\"\n"),
            Some((1, ConfigProblem::MissingKey("name")))
        );
        Ok(())
    }

    #[test]
    fn a_declared_legacy_dev_checkpoint_reads_as_the_standard_profile() -> TestResult {
        let legacy = "[[delivery.environments]]\nname = \"dev\"\nafter = \"integration\"\n";
        let config = parse_yulius_toml(legacy)?;
        assert_eq!(config.delivery, DeliveryProfile::standard());
        let deliberate = format!("{legacy}command = \"make deploy-dev\"\nverify = \"command\"\n");
        let config = parse_yulius_toml(&deliberate)?;
        assert_eq!(config.delivery.preset, Preset::Custom);
        assert_eq!(config.delivery.environments.len(), 1);
        Ok(())
    }

    #[test]
    fn precise_errors_with_line_numbers() {
        assert_eq!(
            problem("[docs]\nproduct = docs/product.md\n"),
            Some((2, ConfigProblem::UnsupportedValue))
        );
        assert_eq!(
            problem("[docs]\nproduct = \"x.md\" extra\n"),
            Some((2, ConfigProblem::TrailingText))
        );
        assert_eq!(
            problem("[docs]\nproduct = \"x.md\n"),
            Some((2, ConfigProblem::UnterminatedString))
        );
        assert_eq!(
            problem("[docs]\nproduct = \"a\\qb\"\n"),
            Some((2, ConfigProblem::InvalidEscape))
        );
        assert_eq!(
            problem("[docs]\nproduct = \"\"\"x\"\"\"\n"),
            Some((2, ConfigProblem::UnsupportedValue))
        );
        assert_eq!(
            problem("[docs]\njust text\n"),
            Some((2, ConfigProblem::ExpectedKeyValue))
        );
        assert_eq!(
            problem("[docs]\na.b = \"x\"\n"),
            Some((2, ConfigProblem::ExpectedKeyValue))
        );
        assert_eq!(
            problem("[docs\nproduct = \"x\"\n"),
            Some((1, ConfigProblem::InvalidHeader))
        );
        assert_eq!(
            problem("[docs]\n[docs]\n"),
            Some((2, ConfigProblem::DuplicateTable("docs".to_owned())))
        );
        assert_eq!(
            problem("[[docs]]\n"),
            Some((1, ConfigProblem::NotAnArrayTable("docs".to_owned())))
        );
        assert_eq!(
            problem("[delivery.environments]\n"),
            Some((
                1,
                ConfigProblem::ArrayTableRequired("delivery.environments".to_owned())
            ))
        );
        assert_eq!(
            problem("[commands]\ntest = \"a\"\ntest = \"b\"\n"),
            Some((3, ConfigProblem::DuplicateKey("test".to_owned())))
        );
        assert!(matches!(
            problem("[docs]\nprodcut = \"x.md\"\n"),
            Some((2, ConfigProblem::UnknownKey { .. }))
        ));
        assert!(matches!(
            problem("[docs]\nproduct = \"/etc/passwd\"\n"),
            Some((2, ConfigProblem::InvalidPath { .. }))
        ));
        assert!(matches!(
            problem("[docs]\narchitecture = \"../outside.md\"\n"),
            Some((2, ConfigProblem::InvalidPath { .. }))
        ));
        assert_eq!(
            problem("[docs]\nproduct = true\n"),
            Some((2, ConfigProblem::ExpectedString("product".to_owned())))
        );
        assert_eq!(
            problem("[delivery]\nauto_promote = \"yes\"\n"),
            Some((2, ConfigProblem::ExpectedBool("auto_promote".to_owned())))
        );
        assert!(matches!(
            problem("[delivery]\nmerge = \"fast\"\n"),
            Some((2, ConfigProblem::InvalidValue { .. }))
        ));
        assert_eq!(
            problem("[commands]\ntest = \"  \"\n"),
            Some((2, ConfigProblem::InvalidCommand("test".to_owned())))
        );
        assert!(matches!(
            problem("[delivery]\nintegration_branch = \"main\"\n"),
            Some((1, ConfigProblem::Profile(ProfileError::SameBranches(_))))
        ));
        assert_eq!(
            problem(&"#".repeat(MAX_CONFIG_BYTES + 1)),
            Some((1, ConfigProblem::TooLarge))
        );
        let message = parse_yulius_toml("[docs]\nproduct = 1.5\n")
            .err()
            .map(|e| e.to_string());
        assert_eq!(
            message.as_deref(),
            Some(
                "yulius.toml line 2: unsupported value; use a \"quoted string\", a whole number, true/false or [] (signed or fractional numbers, multi-line strings, arrays and inline tables are not supported)"
            )
        );
        assert_eq!(
            problem("[docs]\nproduct = 1\n"),
            Some((2, ConfigProblem::ExpectedString("product".to_owned())))
        );
    }

    #[test]
    fn render_round_trips() -> TestResult {
        let mut custom = DeliveryProfile::product();
        custom.integration_branch = "develop".to_owned();
        custom.require_checks = false;
        custom.environments[0].command = Some("deploy \"dev\" \\ now".to_owned());
        custom.environments[0].verify = Verify::Command;
        let configs = [
            ProjectConfig::default(),
            ProjectConfig {
                docs: DocsLayout {
                    decisions: DecisionsLocation::File("knowledge/decisions.md".to_owned()),
                    ..DocsLayout::default()
                },
                commands: Commands {
                    test: Some("cargo test".to_owned()),
                    lint: None,
                    build: Some("cargo build --release".to_owned()),
                    gate: Some("cargo test && cargo build --release".to_owned()),
                },
                ci: Vec::new(),
                delivery: custom.relabeled(),
            },
            ProjectConfig {
                ci: vec![
                    JobDef {
                        name: "lint".to_owned(),
                        command: "cargo clippy -- -D warnings".to_owned(),
                        matrix: Vec::new(),
                        need: Need::default(),
                    },
                    JobDef {
                        name: "test".to_owned(),
                        command: "cargo test --partition \"hash:$YULIUS_CI_MATRIX\"".to_owned(),
                        matrix: vec!["1/2".to_owned(), "2/2".to_owned()],
                        need: Need {
                            cpus: Some(6),
                            memory_mb: Some(12 * 1024),
                        },
                    },
                ],
                ..ProjectConfig::default()
            },
            ProjectConfig {
                delivery: DeliveryProfile {
                    environments: Vec::new(),
                    ..DeliveryProfile::standard()
                }
                .relabeled(),
                ..ProjectConfig::default()
            },
            ProjectConfig {
                delivery: DeliveryProfile::studio(),
                ..ProjectConfig::default()
            },
        ];
        for config in configs {
            let text = render_yulius_toml(&config);
            assert_eq!(parse_yulius_toml(&text)?, config, "{text}");
        }
        Ok(())
    }

    #[test]
    fn ci_jobs_parse_with_matrices_and_report_problems() -> TestResult {
        let config = parse_yulius_toml(
            "[[ci.jobs]]\nname = \"lint\"\ncommand = \"make lint\"\n\n\
             [[ci.jobs]]\nname = \"test\"\ncommand = \"make test\"\nmatrix = \"a b\"\n",
        )?;
        let jobs = crate::ci::project_jobs(config.commands.gate.as_deref(), &config.ci)?;
        let names: Vec<&str> = jobs.iter().map(|j| j.name.as_str()).collect();
        assert_eq!(names, ["lint", "test (a)", "test (b)"]);
        assert_eq!(
            problem("[ci.jobs]\nname = \"x\"\n"),
            Some((1, ConfigProblem::ArrayTableRequired("ci.jobs".to_owned())))
        );
        assert_eq!(
            problem("[[ci.jobs]]\nname = \"x\"\n"),
            Some((1, ConfigProblem::MissingKey("command")))
        );
        assert!(matches!(
            problem("[[ci.jobs]]\nname = \"x\"\ncommand = \"y\"\nmatrix = \"a;b\"\n"),
            Some((4, ConfigProblem::Ci(CiConfigError::InvalidMatrix(_))))
        ));
        assert!(matches!(
            problem("[commands]\ngate = \"g\"\n[[ci.jobs]]\nname = \"x\"\ncommand = \"y\"\n"),
            Some((3, ConfigProblem::Ci(CiConfigError::GateAndJobs)))
        ));
        assert!(matches!(
            problem("[[ci.jobs]]\nname = \"X\"\ncommand = \"y\"\n"),
            Some((1, ConfigProblem::Ci(CiConfigError::InvalidJobName(_))))
        ));
        Ok(())
    }

    /// SPEC-082 RES-002, RES-004: a job declares its CPUs and memory; the repository maps the
    /// budget to its tools' variables.
    #[test]
    fn jobs_declare_their_needs_and_the_budget_maps_to_variables() -> TestResult {
        let text = "[[ci.jobs]]\nname = \"quality\"\ncommand = \"make ci\"\ncpus = 6 # half\n\
                    memory_gb = 16\n\n[[ci.jobs]]\nname = \"e2e\"\ncommand = \"make e2e\"\n\n\
                    [resources.env]\nCARGO_BUILD_JOBS = \"{cpus}\"\nMAKEFLAGS = \"-j{cpus}\"\n";
        let config = parse_yulius_toml(text)?;
        let jobs = crate::ci::project_jobs(None, &config.ci)?;
        assert_eq!(
            jobs[0].need,
            Need {
                cpus: Some(6),
                memory_mb: Some(16 * 1024)
            }
        );
        assert_eq!(jobs[1].need, Need::default());
        let env = parse_yulius_toml_resources(text)?;
        let names: Vec<&str> = env.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["CARGO_BUILD_JOBS", "MAKEFLAGS"]);
        assert_eq!(env[1].template, "-j{cpus}");
        assert!(parse_yulius_toml_resources("[docs]\n")?.is_empty());

        let job = "[[ci.jobs]]\nname = \"x\"\ncommand = \"y\"\n";
        for (extra, key, max) in [
            ("cpus = 0\n", "cpus", MAX_DECLARED_CPUS),
            ("cpus = 2000\n", "cpus", MAX_DECLARED_CPUS),
            ("cpus = \"4\"\n", "cpus", MAX_DECLARED_CPUS),
            ("memory_gb = 0\n", "memory_gb", MAX_DECLARED_MEMORY_GB),
        ] {
            assert_eq!(
                problem(&format!("{job}{extra}")),
                Some((
                    4,
                    ConfigProblem::ExpectedNumber {
                        key: key.to_owned(),
                        max
                    }
                )),
                "{extra}"
            );
        }
        for value in ["-1", "1.5", "4_0", "4x"] {
            assert_eq!(
                problem(&format!("{job}cpus = {value}\n")),
                Some((4, ConfigProblem::UnsupportedValue)),
                "{value}"
            );
        }
        assert_eq!(
            problem("[resources.env]\nPATH = \"/x\"\n"),
            Some((
                2,
                ConfigProblem::ResourceEnv {
                    key: "PATH".to_owned(),
                    problem: EnvTemplateError::Reserved
                }
            ))
        );
        assert_eq!(
            problem("[resources.env]\nJOBS = \"{threads}\"\n"),
            Some((
                2,
                ConfigProblem::ResourceEnv {
                    key: "JOBS".to_owned(),
                    problem: EnvTemplateError::InvalidTemplate
                }
            ))
        );
        assert_eq!(
            problem("[[resources.env]]\nJOBS = \"1\"\n"),
            Some((
                1,
                ConfigProblem::NotAnArrayTable("resources.env".to_owned())
            ))
        );
        let many: String = (0..=MAX_ENV_ENTRIES)
            .map(|i| format!("V{i} = \"{{cpus}}\"\n"))
            .collect();
        assert!(matches!(
            parse_yulius_toml_resources(&format!("[resources.env]\n{many}")),
            Err(FormatError {
                problem: ConfigProblem::ResourceEnv {
                    problem: EnvTemplateError::TooMany,
                    ..
                },
                ..
            })
        ));
        Ok(())
    }

    #[test]
    fn declared_caches_are_repository_relative_paths() -> TestResult {
        let caches = parse_yulius_toml_caches("[disk]\ncaches = \"target build/cache/\"\n")?;
        assert_eq!(caches, ["target", "build/cache"]);
        assert!(parse_yulius_toml_caches("[docs]\nproduct = \"a.md\"\n")?.is_empty());
        for bad in ["/etc", "../x", "a/../b"] {
            let text = format!("[disk]\ncaches = \"{bad}\"\n");
            assert!(parse_yulius_toml_caches(&text).is_err(), "{bad}");
        }
        assert!(parse_yulius_toml_caches("[disk]\nother = \"x\"\n").is_err());
        // The rest of the contract still parses with the table present.
        assert!(parse_yulius_toml("[disk]\ncaches = \"target\"\n").is_ok());
        Ok(())
    }

    #[test]
    fn branch_policy_table_selects_a_preset_and_types() -> TestResult {
        assert_eq!(
            parse_yulius_toml_branch_policy("")?,
            BranchPolicy::standard()
        );
        assert_eq!(
            parse_yulius_toml_branch_policy("[branch_policy]\npreset = \"standard\"\n")?,
            BranchPolicy::standard()
        );
        assert_eq!(
            parse_yulius_toml_branch_policy("[branch_policy]\npreset = \"none\"\n")?,
            BranchPolicy::none()
        );
        let custom =
            parse_yulius_toml_branch_policy("[branch_policy]\ntypes = \"feat  bug\" # ours\n")?;
        assert_eq!(custom.types(), ["feat", "bug"]);
        // Yulius names task branches `feat/<n>-<words>`: a list without `feat` would refuse them.
        assert!(matches!(
            problem("[branch_policy]\ntypes = \"story bug\"\n"),
            Some((2, ConfigProblem::InvalidValue { .. }))
        ));
        // The rest of the contract parses with the table present, and validates it.
        assert!(parse_yulius_toml("[branch_policy]\npreset = \"none\"\n").is_ok());
        assert!(matches!(
            problem("[branch_policy]\npreset = \"strict\"\n"),
            Some((2, ConfigProblem::InvalidValue { .. }))
        ));
        assert!(matches!(
            problem("[branch_policy]\nmax = \"3\"\n"),
            Some((2, ConfigProblem::UnknownKey { .. }))
        ));
        assert!(matches!(
            problem("[branch_policy]\ntypes = \"Feat\"\n"),
            Some((2, ConfigProblem::InvalidValue { .. }))
        ));
        assert!(matches!(
            problem("[branch_policy]\ntypes = \"\"\n"),
            Some((2, ConfigProblem::InvalidValue { .. }))
        ));
        assert!(matches!(
            problem("[branch_policy]\npreset = \"none\"\ntypes = \"feat\"\n"),
            Some((3, ConfigProblem::InvalidValue { .. }))
        ));
        assert!(matches!(
            problem("[[branch_policy]]\n"),
            Some((1, ConfigProblem::NotAnArrayTable(_)))
        ));
        assert!(parse_yulius_toml_branch_policy("[branch_policy]\npreset = true\n").is_err());
        Ok(())
    }
}
