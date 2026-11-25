//! The `yulius.toml` project contract (SPEC-001 §7.2, DLV-005).
//!
//! The file is read by a TOML 1.1 parser (the `toml` crate's spanned document, SPEC-504
//! CFG-005, ADR-0360): anything valid TOML parses, whatever its layout (multi-line arrays,
//! trailing commas, comments inside arrays and after values, literal and multi-line strings,
//! inline tables, dotted keys), and invalid TOML is refused with the parser's message at its line.
//! Known tables:
//!
//! - `[docs]`, `[commands]`, `[delivery]`, `[disk]`, `[branch_policy]`, `[resources.env]`,
//!   `[review]`, `[merge_queue]`, `[cd.<name>]` tables, `[[delivery.environments]]` and
//!   `[[ci.jobs]]` entries.
//!
//! Their keys are strict (unknown keys are errors, catching typos), and their values are what
//! the checks read: strings, `true`/`false`, whole numbers of at least 0 and arrays of strings;
//! anything else is refused at its line. Other tables and keys are ignored for forward
//! compatibility. Nesting is bounded by the parser (a line of brackets is refused, never
//! recursed into: review of SPEC-278, B1).
//!
//! ```toml
//! [docs]
//! decisions = "knowledge/decisions.md"   # a `.md` path selects single-file decisions
//! changes = "docs/changes/"              # a trailing `/`: one change note per pull request
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
//! secrets = "VIDOC_API_KEY"              # agent secrets it uses through the egress proxy
//!                                        # (SPEC-142; the integration branch's list counts,
//!                                        # for its name and command, SPEC-264)
//! timeout_minutes = 120                  # its own time limit (1-360), counted from its start;
//!                                        # default: the instance's (SPEC-435)
//! exclusive = true                       # one run of it at a time in the sandbox; a later
//!                                        # run's job waits, outside its limit (SPEC-435)
//! paths = ["crates/web/**", "docs/"]     # the files it tests (1-50 globs): a pull request or
//!                                        # merge candidate touching none of them skips it,
//!                                        # with the reason; the integration and production
//!                                        # branches, a change to yulius.toml and an unknown
//!                                        # change run it (SPEC-493 CJP)
//! after_merge = true                     # part of the full suite: beside a merge-first
//!                                        # staging target it runs on the merged revision, not
//!                                        # on pull requests (SPEC-575 MFS-002, ADR-0410)
//!
//! [review]                             # what counts as product code in a follow-up after an
//! not_product = ["crates/gen/"]          # approval (SPEC-493 APR-002): `product` globs count
//!                                        # only these, `not_product` never these; default:
//!                                        # every file but tests and documents
//!
//! [merge_queue]                        # derived files (SPEC-497 MQW-003): two armed pull
//! apart = ["skills/defaults/"]           # requests that both change files under one of these
//!                                        # globs never share a merge candidate
//!
//! [resources.env]                       # the budget in this repository's tools' terms
//! CARGO_BUILD_JOBS = "{cpus}"            # also {memory_mb}, {memory_gb}; with YULIUS_CPUS and
//!                                        # YULIUS_MEMORY_MB in agent sessions and CI jobs;
//!                                        # without the table: the Rust defaults (SPEC-268)
//!
//! [branch_policy]                      # the merge gate's rules (SPEC-069)
//! preset = "standard"                    # or "none"; types = "feat fix …" replaces the types
//!
//! [cd.dev]                              # CD (deployment, SPEC-134): both may be empty
//! deploy = "scripts/cd/deploy.sh dev"    # run by Yulius from the integration branch head
//! verify = "scripts/cd/verify.sh dev"    # a smoke test, after a successful deploy
//! secrets = "DEV_SSH_KEY DEV_HOST"       # agent secrets a person allowed outside the proxy
//! states = ["OPERATOR"]                  # agent states restored before, saved after (SPEC-278)
//! variables = ["REGION"]                 # project variables production gets (dev gets them all)
//!
//! rollback = "scripts/cd/rollback.sh dev" # redeploys the kept release $YULIUS_CD_RELEASE, then
//!                                        # verify (SPEC-415); a production deploy needs it proven
//! migrations = "expand/contract: the previous release reads the new schema"  # with rollback
//! parts = ["engine", "infra"]            # parts released separately (default: one)
//!
//! [cd.prod]
//! about = "the instances update themselves from main"
//! promote = true                         # a person's deploy promotes the integration branch first
//! drill = "dev"                          # whose rollback drill proves this one (default dev)
//!
//! [cd.platform]                         # more environments (SPEC-257): any name with its stage,
//! stage = "prod"                         # "dev" or "prod" (fixed for [cd.dev] and [cd.prod])
//! deploy = "scripts/cd/deploy.sh platform"
//!
//! [cd.staging]                          # the shared staging target (SPEC-575 MFS-001): at most
//! stage = "dev"                          # one dev-stage table, with deploy, verify and
//! merge_first = true                     # rollback; its after_merge jobs leave pull requests
//! deploy = "scripts/cd/deploy.sh staging" # (ADR-0410)
//! verify = "scripts/cd/verify.sh staging"
//! rollback = "scripts/cd/rollback.sh staging"
//! migrations = "none: stateless"
//! serve_every_minutes = 60               # SPEC-650: Yulius asks for the head after 60 minutes
//! serve_after_merges = 5                 # behind it, or 5 merges ahead (defaults; 0 turns off)
//!
//! [local]                              # the task-local deployment (SPEC-575 MFS-010): it deploys
//! deploy = "scripts/cd/deploy.sh local"  # and tests the project in the task's own sandbox only
//! verify = "scripts/cd/verify.sh local"
//!
//! [[delivery.environments]]
//! name = "dev-vm"
//! after = "integration"
//! command = "node scripts/deploy.mjs dev"
//! verify = "manual"
//! ```

use crate::branch_policy::BranchPolicy;
use crate::cd::{
    CdConfig, CdStage, CdTarget, LocalDeploy, MAX_ABOUT_CHARS, MAX_CD_ENVIRONMENTS, MAX_CD_SECRETS,
    MAX_CD_STATES, MAX_CD_VARIABLES, fixed_stage, valid_env_name, valid_secret_name,
};
use crate::delivery::{
    DeliveryProfile, Environment, MergeMethod, Preset, ProfileError, PromoteMethod, Stage, Verify,
};
use crate::rollback::{MAX_PARTS, valid_part};

use super::{Commands, DecisionsLocation, DocsLayout, ProjectConfig};
use crate::ci::{
    CiConfigError, JobDef, MAX_JOB_TIMEOUT_MINUTES, parse_matrix, parse_secrets, project_jobs,
};
use crate::follow_up::ProductRule;
use crate::resources::{
    EnvTemplate, EnvTemplateError, MAX_DECLARED_CPUS, MAX_DECLARED_MEMORY_GB, MAX_ENV_ENTRIES,
    Need, check_env_template,
};
use crate::scope::Pattern;

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
    /// Not valid TOML: the parser's message (SPEC-504 CFG-005).
    #[error("{0}")]
    Syntax(String),
    /// A known table written as a plain value (`docs = "x"`).
    #[error("[{0}] must be a table")]
    NotATable(String),
    /// `[[docs]]` and similar.
    #[error("[{0}] must be a single table, not [[{0}]]")]
    NotAnArrayTable(String),
    /// `[delivery.environments]` without double brackets.
    #[error("write environments as [[{0}]] entries")]
    ArrayTableRequired(String),
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
    /// A table inside a known one (`[commands.extra]`, an inline table, a dotted key).
    #[error("[{table}.{key}] is not a table Yulius reads: [{table}] takes keys with values")]
    NestedTable {
        /// The known table.
        table: String,
        /// The key holding a table.
        key: String,
    },
    /// A value outside the subset.
    #[error(
        "unsupported value; use a \"quoted string\", a whole number, true/false or an array of \"quoted\", \"strings\" (signed or fractional numbers, dates and inline tables are not values Yulius reads here)"
    )]
    UnsupportedValue,
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
    /// `environments = []` together with environment entries (the TOML parser refuses that as a
    /// duplicate key first; kept as a guard of the profile's rule).
    #[error("'environments = []' conflicts with [[delivery.environments]] entries")]
    EnvironmentsConflict,
    /// The resulting profile is invalid.
    #[error("invalid delivery profile: {0}")]
    Profile(ProfileError),
    /// Invalid CI jobs (`[[ci.jobs]]`, `[commands] gate`).
    #[error("{0}")]
    Ci(CiConfigError),
    /// A `cd` table that cannot name an environment.
    #[error(
        "'[{0}]' is not a CD environment: use [cd.<name>] with a name of 1-32 lowercase letters, digits and '-', starting with a letter (like [cd.dev], [cd.prod] or [cd.platform]; production is [cd.prod])"
    )]
    UnknownCdEnvironment(String),
    /// More `[cd.<name>]` tables than accepted.
    #[error("CD has at most 16 environments")]
    TooManyCdEnvironments,
    /// A named environment without its stage.
    #[error(
        "[{0}] needs 'stage = \"dev\"' or 'stage = \"prod\"': what decides who may deploy it is never implied"
    )]
    MissingCdStage(String),
    /// A rollback command without its data-safety line (SPEC-415 RBK-002).
    #[error(
        "[{0}] has 'rollback' without 'migrations': say in one line how schema and data changes stay compatible with the previous release (or why there are none)"
    )]
    RollbackNeedsMigrations(String),
    /// A `drill` naming no dev-stage table of the file (SPEC-415 RBK-004).
    #[error(
        "[{0}]: 'drill = \"{1}\"' names no dev-stage table of yulius.toml; name a [cd.<name>] whose stage is \"dev\""
    )]
    UnknownDrill(String, String),
    /// A merge-first environment without its deploy, verify or rollback (SPEC-575 MFS-001).
    #[error(
        "[{0}] has 'merge_first = true': it needs 'deploy', 'verify' and 'rollback', since staging changes only to a verified revision and goes back to its last verified release after a failed deploy"
    )]
    MergeFirstNeedsScripts(String),
    /// A schedule of staging on a table that is not the merge-first target (SPEC-650 PST-006).
    #[error(
        "[{0}] has 'serve_every_minutes' or 'serve_after_merges', which only the merge-first staging target takes: add 'merge_first = true' or remove them"
    )]
    ServeNeedsMergeFirst(String),
    /// Every CI job is part of the full suite (SPEC-575 MFS-002).
    #[error(
        "every [[ci.jobs]] entry has 'after_merge = true': keep at least one quick check without it, since a pull request merges on those alone"
    )]
    NoQuickCheck,
    /// A second merge-first environment (SPEC-575 MFS-001).
    #[error(
        "[{0}] has 'merge_first = true', but only one [cd.<name>] may be the shared staging target"
    )]
    SecondMergeFirst(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Value {
    Str(String),
    Bool(bool),
    Int(u64),
    /// A one-line array of strings (`[]` included).
    Array(Vec<String>),
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
    review: Option<Table>,
    merge_queue: Option<Table>,
    local: Option<Table>,
    branch_policy: Option<Table>,
    environments: Vec<Table>,
    ci_jobs: Vec<Table>,
    resources_env: Option<Table>,
    /// `[cd.<name>]` tables in file order.
    cd: Vec<(String, Table)>,
}

const DOCS_KEYS: &str = "product, architecture, decisions, specs_dir, changes, agents";
const COMMAND_KEYS: &str = "test, lint, build, gate";
const DELIVERY_KEYS: &str = "preset, integration_branch, production_branch, merge, promote, auto_promote, require_checks, environments";
const DISK_KEYS: &str = "caches";
const REVIEW_KEYS: &str = "product, not_product";
const MERGE_QUEUE_KEYS: &str = "apart";
const LOCAL_KEYS: &str = "deploy, verify, about";
/// Most globs in one `[review]` key.
const MAX_REVIEW_GLOBS: usize = 50;
const BRANCH_POLICY_KEYS: &str = "preset, types";
const POLICY_TYPES: &str = "1-20 space-separated types of 1-20 lowercase letters including feat (Yulius's task branches), like \"feat fix docs\"";
const ENVIRONMENT_KEYS: &str = "name, after, command, verify";
const CI_JOB_KEYS: &str = "name, command, matrix, cpus, memory_gb, secrets, timeout_minutes, \
    exclusive, paths, after_merge";
const CD_DEV_KEYS: &str = "stage, deploy, verify, secrets, states, variables, about, rollback, \
    migrations, parts, merge_first, serve_every_minutes, serve_after_merges";
const CD_SERVE_EVERY: &str = "0 (never by time) to 10080 minutes (a week)";
const CD_SERVE_AFTER: &str = "0 (never by count) to 1000 merged revisions";
const CD_PROD_KEYS: &str = "stage, deploy, verify, secrets, states, variables, about, promote, \
    rollback, migrations, parts, drill";
const CD_PARTS: &str = "at most 8 part names like [\"engine\", \"infra\"] (an array, or space-separated) of 1-32 lowercase letters, digits and '-', starting with a letter";
const CD_DRILL: &str = "a dev-stage environment's name, like \"dev\"";
const CD_SECRETS: &str = "at most 16 space-separated agent-secret names like DEV_SSH_KEY";
const CD_STATES: &str =
    "at most 8 agent-state names like [\"QUARTIO_OPERATOR\"] (an array, or space-separated)";
const CD_VARIABLES: &str =
    "at most 32 project-variable names like [\"OVH_ENDPOINT\"] (an array, or space-separated)";
const CD_ABOUT: &str = "one line of at most 300 characters";

fn err(line: usize, problem: ConfigProblem) -> FormatError {
    FormatError { line, problem }
}

/// The 1-based line of byte `offset` in `text`.
fn line_at(text: &str, offset: usize) -> usize {
    text.as_bytes()
        .iter()
        .take(offset)
        .filter(|&&b| b == b'\n')
        .count()
        + 1
}

type Spanned<T> = toml::Spanned<T>;
type TomlTable<'i> = toml::de::DeTable<'i>;
type TomlValue<'i> = toml::de::DeValue<'i>;

/// The entries of `table` in the order of the file (the parser keeps them by key).
fn in_file_order<'a, 'i>(
    table: &'a TomlTable<'i>,
) -> Vec<(
    &'a Spanned<std::borrow::Cow<'i, str>>,
    &'a Spanned<TomlValue<'i>>,
)> {
    let mut entries: Vec<_> = table.iter().collect();
    entries.sort_by_key(|(key, _)| key.span().start);
    entries
}

/// A value of a known table, within the subset the checks read: a string, `true`/`false`, a
/// whole number of at least 0, or an array of strings. Anything else (a fractional or signed
/// number, a date, an inline table, an array of other things) is refused at its line.
fn value_of(value: &TomlValue<'_>) -> Result<Value, ConfigProblem> {
    match value {
        TomlValue::String(text) => Ok(Value::Str(text.to_string())),
        TomlValue::Boolean(flag) => Ok(Value::Bool(*flag)),
        TomlValue::Integer(number) => u64::from_str_radix(number.as_str(), number.radix())
            .map(Value::Int)
            .map_err(|_| ConfigProblem::UnsupportedValue),
        TomlValue::Array(items) => items
            .into_iter()
            .map(|item| match item.get_ref() {
                TomlValue::String(text) => Ok(text.to_string()),
                _ => Err(ConfigProblem::UnsupportedValue),
            })
            .collect::<Result<Vec<_>, _>>()
            .map(Value::Array),
        _ => Err(ConfigProblem::UnsupportedValue),
    }
}

/// The entry `key` = `value` of the known table `name` at its line: a table there
/// (`[name.key]`, an inline table, a dotted key) is no key Yulius reads (review round 1).
fn entry_of(
    text: &str,
    name: &str,
    key: &Spanned<std::borrow::Cow<'_, str>>,
    value: &Spanned<TomlValue<'_>>,
) -> Result<Entry, FormatError> {
    let line = line_at(text, key.span().start);
    let key = key.get_ref().to_string();
    if value.get_ref().is_table() {
        return Err(err(
            line,
            ConfigProblem::NestedTable {
                table: name.to_owned(),
                key,
            },
        ));
    }
    Ok(Entry {
        value: value_of(value.get_ref()).map_err(|problem| err(line, problem))?,
        key,
        line,
    })
}

/// The known table `name` at `line` with its entries in file order.
fn table_of(
    text: &str,
    name: &str,
    table: &TomlTable<'_>,
    line: usize,
) -> Result<Table, FormatError> {
    let entries = in_file_order(table)
        .into_iter()
        .map(|(key, value)| entry_of(text, name, key, value))
        .collect::<Result<_, _>>()?;
    Ok(Table { line, entries })
}

/// The single table `name` (`[name]`); `[[name]]` and a plain value are refused.
fn single_table(
    text: &str,
    name: &str,
    value: &Spanned<TomlValue<'_>>,
    line: usize,
) -> Result<Table, FormatError> {
    match value.get_ref() {
        TomlValue::Table(table) => table_of(text, name, table, line),
        TomlValue::Array(_) => Err(err(line, ConfigProblem::NotAnArrayTable(name.to_owned()))),
        _ => Err(err(line, ConfigProblem::NotATable(name.to_owned()))),
    }
}

/// The entries of the array of tables `name` (`[[name]]`, or an array of inline tables), each
/// at its own line; a single table or other values are refused.
fn array_tables(
    text: &str,
    name: &str,
    value: &Spanned<TomlValue<'_>>,
    line: usize,
) -> Result<Vec<Table>, FormatError> {
    let refused = || err(line, ConfigProblem::ArrayTableRequired(name.to_owned()));
    let TomlValue::Array(items) = value.get_ref() else {
        return Err(refused());
    };
    items
        .into_iter()
        .map(|item| match item.get_ref() {
            TomlValue::Table(table) => {
                table_of(text, name, table, line_at(text, item.span().start))
            }
            _ => Err(refused()),
        })
        .collect()
}

/// Reads `yulius.toml` with a TOML 1.1 parser (SPEC-504 CFG-005, ADR-0360): anything valid
/// TOML parses, whatever its layout (multi-line arrays, trailing commas, comments, inline
/// tables, dotted keys); invalid TOML is refused with the parser's message at its line. The
/// known tables then take only the values their checks read ([`value_of`]); other tables and
/// keys are ignored for forward compatibility.
fn parse_document(text: &str) -> Result<Document, FormatError> {
    if text.len() > MAX_CONFIG_BYTES {
        return Err(err(1, ConfigProblem::TooLarge));
    }
    // A byte-order mark adds no line.
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let root = TomlTable::parse(text).map_err(|error| {
        let line = error.span().map_or(1, |span| line_at(text, span.start));
        let message = error.message().lines().next().unwrap_or("invalid TOML");
        err(line, ConfigProblem::Syntax(message.trim().to_owned()))
    })?;
    let mut document = Document::default();
    for (key, value) in in_file_order(root.get_ref()) {
        let name = key.get_ref().as_ref();
        let line = line_at(text, key.span().start);
        match name {
            "docs" | "commands" | "disk" | "review" | "merge_queue" | "local" | "branch_policy" => {
                let table = Some(single_table(text, name, value, line)?);
                match name {
                    "docs" => document.docs = table,
                    "commands" => document.commands = table,
                    "disk" => document.disk = table,
                    "review" => document.review = table,
                    "merge_queue" => document.merge_queue = table,
                    "local" => document.local = table,
                    _ => document.branch_policy = table,
                }
            }
            "delivery" => {
                let TomlValue::Table(delivery) = value.get_ref() else {
                    return Err(single_table(text, name, value, line)
                        .err()
                        .unwrap_or_else(|| err(line, ConfigProblem::NotATable(name.to_owned()))));
                };
                // `[[delivery.environments]]` are tables of their own; `environments = []`
                // stays a key of `[delivery]`.
                let tables = delivery.iter().find(|(key, value)| {
                    key.get_ref() == "environments"
                        && !matches!(value.get_ref(), TomlValue::Array(items)
                            if items.into_iter().all(|item| !item.get_ref().is_table()))
                });
                let mut table = Table {
                    line,
                    entries: Vec::new(),
                };
                if let Some((key, environments)) = tables {
                    let at = line_at(text, key.span().start);
                    document.environments =
                        array_tables(text, "delivery.environments", environments, at)?;
                }
                for (key, value) in in_file_order(delivery) {
                    if tables.is_some() && key.get_ref() == "environments" {
                        continue;
                    }
                    table.entries.push(entry_of(text, name, key, value)?);
                }
                document.delivery = Some(table);
            }
            "ci" => {
                if let TomlValue::Table(ci) = value.get_ref()
                    && let Some((key, jobs)) = ci.iter().find(|(key, _)| key.get_ref() == "jobs")
                {
                    let at = line_at(text, key.span().start);
                    document.ci_jobs = array_tables(text, "ci.jobs", jobs, at)?;
                }
            }
            "resources" => {
                if let TomlValue::Table(resources) = value.get_ref()
                    && let Some((key, env)) =
                        resources.iter().find(|(key, _)| key.get_ref() == "env")
                {
                    let at = line_at(text, key.span().start);
                    document.resources_env = Some(single_table(text, "resources.env", env, at)?);
                }
            }
            "cd" => {
                let TomlValue::Table(cd) = value.get_ref() else {
                    return Err(err(
                        line,
                        ConfigProblem::UnknownCdEnvironment(name.to_owned()),
                    ));
                };
                if cd.is_empty() {
                    return Err(err(
                        line,
                        ConfigProblem::UnknownCdEnvironment(name.to_owned()),
                    ));
                }
                for (key, table) in in_file_order(cd) {
                    let env = key.get_ref().as_ref();
                    let full = format!("cd.{env}");
                    let at = line_at(text, key.span().start);
                    if !valid_env_name(env)
                        || !table.get_ref().is_table() && !table.get_ref().is_array()
                    {
                        return Err(err(at, ConfigProblem::UnknownCdEnvironment(full)));
                    }
                    if document.cd.len() == MAX_CD_ENVIRONMENTS {
                        return Err(err(at, ConfigProblem::TooManyCdEnvironments));
                    }
                    // `[cd.a.b]` names no environment: a table inside one is refused as such.
                    if let TomlValue::Table(inner) = table.get_ref()
                        && let Some((key, _)) =
                            inner.iter().find(|(_, value)| value.get_ref().is_table())
                    {
                        return Err(err(
                            line_at(text, key.span().start),
                            ConfigProblem::UnknownCdEnvironment(format!(
                                "{full}.{}",
                                key.get_ref()
                            )),
                        ));
                    }
                    let table = single_table(text, &full, table, at)?;
                    document.cd.push((env.to_owned(), table));
                }
            }
            _ => {}
        }
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
            // A trailing `/` names a folder of change notes (SPEC-363 CHN-001).
            "changes" if string_value(entry)?.ends_with('/') => layout.changes = path + "/",
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
    let (mut need, mut secrets) = (Need::default(), Vec::new());
    let (mut timeout_minutes, mut exclusive, mut after_merge) = (None, false, false);
    let mut paths_entry = None;
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
            "secrets" => {
                let job = name.as_deref().unwrap_or("?");
                secrets = parse_secrets(job, string_value(entry)?)
                    .map_err(|e| err(entry.line, ConfigProblem::Ci(e)))?;
            }
            "timeout_minutes" => {
                let minutes = number_value(entry, MAX_JOB_TIMEOUT_MINUTES)?;
                timeout_minutes = Some(u32::try_from(minutes).unwrap_or(u32::MAX));
            }
            "exclusive" => exclusive = bool_value(entry)?,
            "after_merge" => after_merge = bool_value(entry)?,
            // Read once the name is known, wherever it stands in the entry.
            "paths" => paths_entry = Some(entry),
            _ => return Err(unknown("[ci.jobs]", entry, CI_JOB_KEYS)),
        }
    }
    let paths = match paths_entry {
        Some(entry) => {
            let items: Vec<String> = match &entry.value {
                Value::Array(items) => items.clone(),
                Value::Str(text) => text.split_whitespace().map(str::to_owned).collect(),
                _ => Vec::new(),
            };
            crate::ci::parse_paths(name.as_deref().unwrap_or("?"), &items)
                .map_err(|e| err(entry.line, ConfigProblem::Ci(e)))?
        }
        None => Vec::new(),
    };
    Ok(JobDef {
        name: name.ok_or_else(|| err(table.line, ConfigProblem::MissingKey("name")))?,
        command: command_text
            .ok_or_else(|| err(table.line, ConfigProblem::MissingKey("command")))?,
        matrix,
        need,
        secrets,
        timeout_minutes,
        exclusive,
        paths,
        after_merge,
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
    // A pull request merges on its quick checks: some job must stay one (SPEC-575 MFS-002).
    if !defs.is_empty() && defs.iter().all(|def| def.after_merge) {
        return Err(err(line, ConfigProblem::NoQuickCheck));
    }
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
                if !matches!(&entry.value, Value::Array(items) if items.is_empty()) {
                    return Err(err(
                        entry.line,
                        ConfigProblem::InvalidValue {
                            key: entry.key.clone(),
                            value: match &entry.value {
                                Value::Str(text) => clip(text),
                                Value::Bool(value) => value.to_string(),
                                Value::Int(value) => value.to_string(),
                                Value::Array(items) => clip(&items.join(" ")),
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
    review_of(&document)?;
    merge_queue_of(&document)?;
    local_of(&document)?;
    resources_env(&document)?;
    cd_of(&document)?;
    Ok(ProjectConfig {
        docs: docs_layout(document.docs.as_ref())?,
        commands,
        ci,
        delivery: delivery(&document)?,
    })
}

/// `[cd.<env>]`'s stage (SPEC-257 CDN-002): fixed for `dev` and `prod`, required for the others.
fn cd_stage(env: &str, table: &Table) -> Result<CdStage, FormatError> {
    let fixed = fixed_stage(env);
    let Some(entry) = table.entries.iter().find(|entry| entry.key == "stage") else {
        return fixed.ok_or_else(|| {
            err(
                table.line,
                ConfigProblem::MissingCdStage(format!("cd.{env}")),
            )
        });
    };
    let text = string_value(entry)?;
    let stage = match text {
        "dev" => Some(CdStage::Dev),
        "prod" => Some(CdStage::Prod),
        _ => None,
    };
    match (stage, fixed) {
        (Some(stage), None) => Ok(stage),
        (Some(stage), Some(fixed)) if stage == fixed => Ok(stage),
        _ => Err(err(
            entry.line,
            ConfigProblem::InvalidValue {
                key: entry.key.clone(),
                value: clip(text),
                allowed: match fixed {
                    Some(CdStage::Dev) => "\"dev\" (the stage of [cd.dev])",
                    Some(CdStage::Prod) => "\"prod\" (the stage of [cd.prod])",
                    None => "\"dev\" or \"prod\"",
                },
            },
        )),
    }
}

fn cd_target(env: &str, table: &Table) -> Result<CdTarget, FormatError> {
    let stage = cd_stage(env, table)?;
    let name = format!("cd.{env}");
    let known = match stage {
        CdStage::Dev => CD_DEV_KEYS,
        CdStage::Prod => CD_PROD_KEYS,
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
    let mut target = CdTarget::new(env, stage);
    let mut serve_set = false;
    for entry in &table.entries {
        match entry.key.as_str() {
            "stage" => {}
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
            "states" => target.states = names_of(entry, MAX_CD_STATES, CD_STATES)?,
            "variables" => {
                target.variables = names_of(entry, MAX_CD_VARIABLES, CD_VARIABLES)?;
            }
            "about" => {
                let text = string_value(entry)?.trim();
                if text.chars().count() > MAX_ABOUT_CHARS || text.contains(['\n', '\r']) {
                    return Err(invalid(entry, text, CD_ABOUT));
                }
                target.about = (!text.is_empty()).then(|| text.to_owned());
            }
            "promote" if stage == CdStage::Prod => target.promote = bool_value(entry)?,
            "merge_first" if stage == CdStage::Dev => target.merge_first = bool_value(entry)?,
            // SPEC-650 PST-006: `0` turns either trigger off.
            "serve_every_minutes" | "serve_after_merges" if stage == CdStage::Dev => {
                let (max, allowed) = if entry.key == "serve_every_minutes" {
                    (10_080, CD_SERVE_EVERY)
                } else {
                    (1_000, CD_SERVE_AFTER)
                };
                let value = match entry.value {
                    Value::Int(value) if value <= max => u32::try_from(value).unwrap_or(0),
                    Value::Int(value) => return Err(invalid(entry, &value.to_string(), allowed)),
                    _ => return Err(invalid(entry, "", allowed)),
                };
                if entry.key == "serve_every_minutes" {
                    target.serve_every_minutes = value;
                } else {
                    target.serve_after_merges = value;
                }
                serve_set = true;
            }
            "rollback" => target.rollback = Some(command(entry)?),
            "migrations" => {
                let text = string_value(entry)?.trim();
                if text.is_empty()
                    || text.chars().count() > MAX_ABOUT_CHARS
                    || text.contains(['\n', '\r'])
                {
                    return Err(invalid(entry, text, CD_ABOUT));
                }
                target.migrations = Some(text.to_owned());
            }
            "parts" => {
                let parts: Vec<String> = match &entry.value {
                    Value::Array(items) => items.iter().map(|s| s.trim().to_owned()).collect(),
                    Value::Str(text) => text.split_whitespace().map(str::to_owned).collect(),
                    _ => return Err(invalid(entry, "", CD_PARTS)),
                };
                if parts.len() > MAX_PARTS {
                    return Err(invalid(entry, &parts.join(" "), CD_PARTS));
                }
                target.parts = Vec::new();
                for part in parts {
                    if !valid_part(&part) {
                        return Err(invalid(entry, &part, CD_PARTS));
                    }
                    if !target.parts.contains(&part) {
                        target.parts.push(part);
                    }
                }
            }
            "drill" if stage == CdStage::Prod => {
                let text = string_value(entry)?.trim();
                if !valid_env_name(text) || fixed_stage(text) == Some(CdStage::Prod) {
                    return Err(invalid(entry, text, CD_DRILL));
                }
                target.drill = Some(text.to_owned());
            }
            _ => return Err(unknown(&name, entry, known)),
        }
    }
    if target.rollback.is_some() && target.migrations.is_none() {
        return Err(err(
            table.line,
            ConfigProblem::RollbackNeedsMigrations(name),
        ));
    }
    // The shared staging target changes only to a verified revision and goes back to its last
    // verified release after a failed deploy (SPEC-575 MFS-001, MFS-007).
    if target.merge_first
        && (target.deploy.is_none() || target.verify.is_none() || target.rollback.is_none())
    {
        return Err(err(table.line, ConfigProblem::MergeFirstNeedsScripts(name)));
    }
    if serve_set && !target.merge_first {
        return Err(err(table.line, ConfigProblem::ServeNeedsMergeFirst(name)));
    }
    Ok(target)
}

/// The names an entry lists, as an array or a space-separated string: at most `max`, each an
/// agent secret's (or variable's) name, duplicates dropped.
fn names_of(entry: &Entry, max: usize, allowed: &'static str) -> Result<Vec<String>, FormatError> {
    let invalid = |value: &str| {
        err(
            entry.line,
            ConfigProblem::InvalidValue {
                key: entry.key.clone(),
                value: clip(value),
                allowed,
            },
        )
    };
    let names: Vec<String> = match &entry.value {
        Value::Array(items) => items.iter().map(|s| s.trim().to_owned()).collect(),
        Value::Str(text) => text.split_whitespace().map(str::to_owned).collect(),
        _ => return Err(invalid("")),
    };
    if names.len() > max {
        return Err(invalid(&names.join(" ")));
    }
    let mut out: Vec<String> = Vec::new();
    for name in names {
        if !valid_secret_name(&name) {
            return Err(invalid(&name));
        }
        if !out.contains(&name) {
            out.push(name);
        }
    }
    Ok(out)
}

fn cd_of(document: &Document) -> Result<CdConfig, FormatError> {
    let config = CdConfig {
        environments: document
            .cd
            .iter()
            .map(|(env, table)| cd_target(env, table))
            .collect::<Result<_, _>>()?,
    };
    // One shared staging target (SPEC-575 MFS-001).
    if let Some(((_, table), second)) = document
        .cd
        .iter()
        .zip(&config.environments)
        .filter(|(_, target)| target.merge_first)
        .nth(1)
    {
        return Err(err(
            table.line,
            ConfigProblem::SecondMergeFirst(format!("cd.{}", second.name)),
        ));
    }
    // A named drill environment is a dev-stage table of the same file (review of SPEC-415).
    for ((_, table), target) in document.cd.iter().zip(&config.environments) {
        if let Some(drill) = &target.drill
            && config.target_of(drill, CdStage::Dev).is_none()
        {
            return Err(err(
                table.line,
                ConfigProblem::UnknownDrill(format!("cd.{}", target.name), drill.clone()),
            ));
        }
    }
    Ok(config)
}

/// The project's CD (deployment, SPEC-134, SPEC-257): its `[cd.<name>]` tables, `[cd.dev]` and
/// `[cd.prod]` being the defaults. Only these tables are checked, so a mistake elsewhere does not
/// hide CD.
///
/// # Errors
/// A syntax error of the file or an invalid CD table.
///
/// # Examples
/// ```
/// use yulius_domain::cd::{CdStage, CdState};
/// use yulius_domain::spec_format::parse_yulius_toml_cd;
/// let cd = parse_yulius_toml_cd("[cd.dev]\n[cd.prod]\npromote = true\n")?;
/// assert_eq!(cd.state("dev", CdStage::Dev), CdState::Empty);
/// assert_eq!(cd.state("prod", CdStage::Prod), CdState::Ready);
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

/// `[review]`'s rule of what counts as product code (SPEC-493 APR-002).
fn review_of(document: &Document) -> Result<ProductRule, FormatError> {
    let mut rule = ProductRule::default();
    for entry in document
        .review
        .as_ref()
        .map(|t| t.entries.as_slice())
        .unwrap_or_default()
    {
        let slot = match entry.key.as_str() {
            "product" => &mut rule.product,
            "not_product" => &mut rule.not_product,
            _ => return Err(unknown("review", entry, REVIEW_KEYS)),
        };
        let items: Vec<String> = match &entry.value {
            Value::Array(items) => items.clone(),
            Value::Str(text) => text.split_whitespace().map(str::to_owned).collect(),
            _ => Vec::new(),
        };
        let invalid = |value: &str| {
            err(
                entry.line,
                ConfigProblem::InvalidValue {
                    key: entry.key.clone(),
                    value: clip(value),
                    allowed: "1 to 50 repository path globs, as scope claims take them",
                },
            )
        };
        if items.is_empty() || items.len() > MAX_REVIEW_GLOBS {
            return Err(invalid(&items.join(" ")));
        }
        for item in &items {
            slot.push(Pattern::parse(item.trim()).map_err(|_| invalid(item))?);
        }
    }
    Ok(rule)
}

/// What counts as product code in a follow-up after an approval: `[review]` of `yulius.toml`
/// (SPEC-493 APR-002), the default (every file but tests and documents) without it.
///
/// # Errors
/// A syntax error of the file or an invalid `[review]` entry.
///
/// # Examples
/// ```
/// use yulius_domain::spec_format::parse_yulius_toml_review;
/// let rule = parse_yulius_toml_review("[review]\nnot_product = [\"crates/gen/\"]\n")?;
/// assert!(!rule.counts("crates/gen/x.rs"));
/// assert!(rule.counts("crates/a/src/lib.rs"));
/// # Ok::<(), yulius_domain::spec_format::FormatError>(())
/// ```
pub fn parse_yulius_toml_review(text: &str) -> Result<ProductRule, FormatError> {
    parse_document(text).and_then(|document| review_of(&document))
}

/// `[merge_queue]`'s derived files (SPEC-497 MQW-003): `apart`, 1 to 50 repository path globs;
/// two armed pull requests that both change files under one of them never share a merge
/// candidate. None without the table.
///
/// # Errors
/// A syntax error of the file or an invalid `[merge_queue]` entry.
///
/// # Examples
/// ```
/// use yulius_domain::spec_format::parse_yulius_toml_merge_queue;
/// let apart = parse_yulius_toml_merge_queue("[merge_queue]\napart = [\"gen/\", \"Cargo.lock\"]\n")?;
/// assert_eq!(apart[0].as_str(), "gen/**");
/// assert!(parse_yulius_toml_merge_queue("[docs]\nproduct = \"README.md\"\n")?.is_empty());
/// assert!(parse_yulius_toml_merge_queue("[merge_queue]\nserial = [\"x\"]\n").is_err());
/// # Ok::<(), yulius_domain::spec_format::FormatError>(())
/// ```
pub fn parse_yulius_toml_merge_queue(text: &str) -> Result<Vec<Pattern>, FormatError> {
    parse_document(text).and_then(|document| merge_queue_of(&document))
}

/// The task-local deployment (SPEC-575 MFS-010): `[local]` with its `deploy` command and an
/// optional `verify` and `about`; `None` without the table.
///
/// # Errors
/// A syntax error of the file or an invalid `[local]` table.
///
/// # Examples
/// ```
/// use yulius_domain::spec_format::parse_yulius_toml_local;
/// let local = parse_yulius_toml_local("[local]\ndeploy = \"scripts/cd/deploy.sh local\"\n")?;
/// assert_eq!(local.map(|l| l.deploy).as_deref(), Some("scripts/cd/deploy.sh local"));
/// # Ok::<(), yulius_domain::spec_format::FormatError>(())
/// ```
pub fn parse_yulius_toml_local(text: &str) -> Result<Option<LocalDeploy>, FormatError> {
    parse_document(text).and_then(|document| local_of(&document))
}

fn local_of(document: &Document) -> Result<Option<LocalDeploy>, FormatError> {
    let Some(table) = &document.local else {
        return Ok(None);
    };
    let (mut deploy, mut verify, mut about) = (None, None, None);
    for entry in &table.entries {
        match entry.key.as_str() {
            "deploy" => deploy = Some(command(entry)?),
            "verify" => verify = Some(command(entry)?),
            "about" => {
                let text = string_value(entry)?.trim();
                if text.chars().count() > MAX_ABOUT_CHARS || text.contains(['\n', '\r']) {
                    return Err(err(
                        entry.line,
                        ConfigProblem::InvalidValue {
                            key: entry.key.clone(),
                            value: clip(text),
                            allowed: CD_ABOUT,
                        },
                    ));
                }
                about = (!text.is_empty()).then(|| text.to_owned());
            }
            _ => return Err(unknown("local", entry, LOCAL_KEYS)),
        }
    }
    let deploy = deploy.ok_or_else(|| err(table.line, ConfigProblem::MissingKey("deploy")))?;
    Ok(Some(LocalDeploy {
        deploy,
        verify,
        about,
    }))
}

/// `[merge_queue]`'s `apart` patterns, checked.
fn merge_queue_of(document: &Document) -> Result<Vec<Pattern>, FormatError> {
    let mut apart = Vec::new();
    for entry in document
        .merge_queue
        .as_ref()
        .map(|t| t.entries.as_slice())
        .unwrap_or_default()
    {
        if entry.key != "apart" {
            return Err(unknown("merge_queue", entry, MERGE_QUEUE_KEYS));
        }
        let items: Vec<String> = match &entry.value {
            Value::Array(items) => items.clone(),
            Value::Str(text) => text.split_whitespace().map(str::to_owned).collect(),
            _ => Vec::new(),
        };
        let invalid = |value: &str| {
            err(
                entry.line,
                ConfigProblem::InvalidValue {
                    key: entry.key.clone(),
                    value: clip(value),
                    allowed: "1 to 50 repository path globs, as scope claims take them",
                },
            )
        };
        if items.is_empty() || items.len() > MAX_REVIEW_GLOBS {
            return Err(invalid(&items.join(" ")));
        }
        for item in &items {
            apart.push(Pattern::parse(item.trim()).map_err(|_| invalid(item))?);
        }
    }
    Ok(apart)
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

/// As [`parse_yulius_toml_resources`], `None` when the file has no `[resources.env]` table at all
/// (an empty table is `Some` of nothing), so a repository that says nothing can get defaults
/// (SPEC-268 PID-007).
///
/// # Errors
/// As [`parse_yulius_toml_resources`].
pub fn parse_yulius_toml_resources_table(
    text: &str,
) -> Result<Option<Vec<EnvTemplate>>, FormatError> {
    let document = parse_document(text)?;
    if document.resources_env.is_none() {
        return Ok(None);
    }
    resources_env(&document).map(Some)
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
        if !job.secrets.is_empty() {
            out.push_str(&format!("secrets = {}\n", quote(&job.secrets.join(" "))));
        }
        if let Some(minutes) = job.timeout_minutes {
            out.push_str(&format!("timeout_minutes = {minutes}\n"));
        }
        if job.exclusive {
            out.push_str("exclusive = true\n");
        }
        if !job.paths.is_empty() {
            let items: Vec<String> = job.paths.iter().map(|p| quote(p)).collect();
            out.push_str(&format!("paths = [{}]\n", items.join(", ")));
        }
        if job.after_merge {
            out.push_str("after_merge = true\n");
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
        let dev = cd.target("dev").ok_or("dev")?;
        assert_eq!(dev.about.as_deref(), Some("no dev machine yet"));
        assert_eq!(dev.stage, CdStage::Dev);
        assert_eq!(cd.state("dev", CdStage::Dev), CdState::Empty);
        let prod = cd.target("prod").ok_or("prod")?;
        assert_eq!(prod.stage, CdStage::Prod);
        assert!(prod.promote);
        assert_eq!(prod.deploy.as_deref(), Some("scripts/cd/deploy.sh prod"));
        assert_eq!(prod.verify.as_deref(), Some("scripts/cd/verify.sh prod"));
        assert_eq!(prod.secrets, ["PROD_SSH_KEY", "PROD_HOST"]);
        assert!(cd.is_defined());
        let none = parse_yulius_toml_cd("[commands]\ntest = \"x\"\n")?;
        assert_eq!(none, CdConfig::default());
        assert_eq!(none.state("prod", CdStage::Prod), CdState::NotDefined);
        // The whole file is validated with them.
        assert!(parse_yulius_toml("[cd.dev]\n[cd.prod]\n").is_ok());
        Ok(())
    }

    /// SPEC-415 RBK-001..004: the rollback contract's keys, and what is refused.
    #[test]
    fn cd_tables_define_their_rollback() -> TestResult {
        let cd = parse_yulius_toml_cd(
            "[cd.dev]\ndeploy = \"scripts/cd/run deploy dev\"\n\
             rollback = \"scripts/cd/run rollback dev\"\n\
             migrations = \"expand/contract: the previous release reads the new schema\"\n\
             parts = [\"engine\", \"infra\", \"engine\"]\n\
             [cd.prod]\npromote = true\ndeploy = \"d\"\nrollback = \"r\"\nmigrations = \"none: stateless\"\n\
             parts = \"engine infra\"\n\
             [cd.prod-both]\nstage = \"prod\"\ndeploy = \"d\"\ndrill = \"dev-both\"\n\
             [cd.dev-both]\nstage = \"dev\"\n",
        )?;
        let dev = cd.target("dev").ok_or("dev")?;
        assert_eq!(dev.rollback.as_deref(), Some("scripts/cd/run rollback dev"));
        assert!(
            dev.migrations
                .as_deref()
                .is_some_and(|m| m.starts_with("expand"))
        );
        assert_eq!(dev.parts, ["engine", "infra"]);
        assert_eq!(dev.part_names(), ["engine", "infra"]);
        let prod = cd.target("prod").ok_or("prod")?;
        assert_eq!(
            prod.drill_env(),
            "dev",
            "production is proven by dev by default"
        );
        assert_eq!(prod.parts, ["engine", "infra"]);
        let both = cd.target("prod-both").ok_or("prod-both")?;
        assert_eq!(
            (both.drill_env(), both.rollback.as_deref()),
            ("dev-both", None)
        );
        assert_eq!(both.part_names(), [""], "one unnamed part");
        for (bad, says) in [
            ("[cd.dev]\nrollback = \"r\"\n", "without 'migrations'"),
            (
                "[cd.dev]\nrollback = \"\"\nmigrations = \"m\"\n",
                "rollback",
            ),
            (
                "[cd.dev]\nrollback = \"r\"\nmigrations = \"\"\n",
                "'migrations'",
            ),
            ("[cd.dev]\ndrill = \"dev\"\n", "drill"),
            ("[cd.prod]\ndrill = \"prod\"\n", "'drill'"),
            ("[cd.prod]\ndrill = \"Dev\"\n", "'drill'"),
            ("[cd.dev]\nparts = [\"Engine\"]\n", "'parts'"),
            ("[cd.dev]\nparts = \"a b c d e f g h i\"\n", "'parts'"),
            (
                "[cd.prod]\ndrill = \"staging\"\n",
                "'drill = \"staging\"' names no dev-stage table",
            ),
            (
                "[cd.prod]\ndrill = \"platform\"\n[cd.platform]\nstage = \"prod\"\n",
                "'drill = \"platform\"' names no dev-stage table",
            ),
        ] {
            let shown = parse_yulius_toml_cd(bad).err().map(|e| e.to_string());
            assert!(
                shown.as_deref().is_some_and(|e| e.contains(says)),
                "{bad}: {shown:?}"
            );
        }
        Ok(())
    }

    /// SPEC-278 CDS-001: a table names the agent states its run uses, as an array of names or a
    /// space-separated string, at most 8, duplicates dropped.
    #[test]
    fn cd_tables_name_their_states() -> TestResult {
        let cd = parse_yulius_toml_cd(
            "[cd.dev]\nstates = [\"QUARTIO_OPERATOR\", 'OTHER_STATE' , \"QUARTIO_OPERATOR\",]\n\
             [cd.prod]\nstates = \"QUARTIO_OPERATOR  OTHER_STATE\"\n\
             [cd.platform]\nstage = \"prod\"\nstates = []\n",
        )?;
        let dev = cd.target("dev").ok_or("dev")?;
        assert_eq!(dev.states, ["QUARTIO_OPERATOR", "OTHER_STATE"]);
        assert_eq!(
            cd.target("prod").ok_or("prod")?.states,
            ["QUARTIO_OPERATOR", "OTHER_STATE"]
        );
        assert!(cd.target("platform").ok_or("platform")?.states.is_empty());
        // Production names the project variables it gets (review of SPEC-278, M3).
        let cd = parse_yulius_toml_cd(
            "[cd.prod]\nvariables = [\"QUARTIO_CD_PROD_DEPLOYMENTS\", \"OVH_ENDPOINT\"]\n\
             [cd.dev]\nvariables = \"OVH_ENDPOINT\"\n",
        )?;
        assert_eq!(
            cd.target("prod").ok_or("prod")?.variables,
            ["QUARTIO_CD_PROD_DEPLOYMENTS", "OVH_ENDPOINT"]
        );
        assert_eq!(cd.target("dev").ok_or("dev")?.variables, ["OVH_ENDPOINT"]);
        for bad in [
            "[cd.prod]\nvariables = [\"path\"]\n",
            "[cd.prod]\nvariables = 3\n",
        ] {
            let shown = parse_yulius_toml_cd(bad).err().map(|e| e.to_string());
            assert!(
                shown.as_deref().is_some_and(|e| e.contains("'variables'")),
                "{bad}: {shown:?}"
            );
        }
        let many: Vec<String> = (0..9).map(|n| format!("\"STATE_{n}\"")).collect();
        let cases = [
            ("[cd.dev]\nstates = [\"ops\"]\n".to_owned(), "'states'"),
            (
                "[cd.dev]\nstates = [\"OPS\", 1]\n".to_owned(),
                "unsupported value",
            ),
            ("[cd.dev]\nstates = [\"OPS\"\n".to_owned(), "unclosed array"),
            (
                "[cd.dev]\nstates = [\"OPS\" \"B\"]\n".to_owned(),
                "missing comma",
            ),
            ("[cd.dev]\nstates = true\n".to_owned(), "'states'"),
            (
                format!("[cd.dev]\nstates = [{}]\n", many.join(", ")),
                "'states'",
            ),
            // Arrays hold strings only: nothing nests (review of SPEC-278, B1).
            (
                "[cd.dev]\nstates = [[\"OPS\"]]\n".to_owned(),
                "unsupported value",
            ),
            // Arrays of names are for states only.
            (
                "[cd.dev]\nsecrets = [\"OPS\"]\n".to_owned(),
                "'secrets' must be a quoted string",
            ),
        ];
        // A deeply nested line within the size bound is refused, never recursed into: the file
        // comes from branches and worktrees agents write (review of SPEC-278, B1).
        let deep = format!("[commands]\ntest = {}\n", "[".repeat(65_000));
        assert!(deep.len() < MAX_CONFIG_BYTES);
        let shown = parse_yulius_toml(&deep).err().map(|e| e.to_string());
        assert!(
            shown.as_deref().is_some_and(|e| e.contains("recurs")),
            "{shown:?}"
        );
        for (text, message) in cases {
            let shown = parse_yulius_toml_cd(&text)
                .err()
                .map(|e| e.to_string())
                .unwrap_or_default();
            assert!(shown.contains(message), "{text}: {shown}");
        }
        Ok(())
    }

    /// SPEC-134 CDK-004, SPEC-591: Yulius defines its own CD: dev its merge-first staging VM,
    /// production a promotion.
    #[test]
    fn the_repositorys_own_cd_is_defined() -> TestResult {
        let raw = include_str!("../../../../yulius.toml");
        let cd = parse_yulius_toml_cd(raw)?;
        assert!(cd.is_defined());
        // SPEC-591: `[cd.dev]` deploys Yulius to its staging VM with scripts/cd (ADR-0430),
        // with a rollback and the migrations line, and the key it needs.
        assert_eq!(cd.state("dev", CdStage::Dev), CdState::Ready);
        let dev = cd.target("dev").ok_or("dev")?;
        assert_eq!(dev.deploy.as_deref(), Some("scripts/cd/deploy.sh dev"));
        assert_eq!(dev.verify.as_deref(), Some("scripts/cd/verify.sh dev"));
        assert_eq!(dev.rollback.as_deref(), Some("scripts/cd/rollback.sh dev"));
        assert!(
            dev.migrations
                .as_deref()
                .is_some_and(|m| m.contains("store"))
        );
        assert_eq!(dev.secrets, ["STAGING_VM_SSH_KEY"]);
        let dev_about = dev.about.as_deref().unwrap_or_default();
        assert!(dev_about.contains("staging VM"), "{dev_about}");
        // SPEC-591 STG-007: the staging VM is the merge-first target, and the full suite (both
        // test partitions and e2e) runs on the merged revision. PRs run all quality passes
        // and security (SPEC-598).
        assert!(dev.merge_first);
        assert_eq!(cd.merge_first().map(|t| t.name.as_str()), Some("dev"));
        let config = parse_yulius_toml(raw)?;
        let jobs = crate::ci::project_jobs(config.commands.gate.as_deref(), &config.ci)?;
        let after: Vec<(&str, bool)> = jobs
            .iter()
            .map(|j| (j.name.as_str(), j.after_merge))
            .collect();
        assert_eq!(
            after,
            [
                ("quality", false),
                ("lint", false),
                ("macos", false),
                ("docs", false),
                ("deps", false),
                ("file-contracts", false),
                ("test (1/2)", true),
                ("test (2/2)", true),
                ("e2e", true),
                ("security", false)
            ],
            "{jobs:?}"
        );
        assert_eq!(parse_yulius_toml_local(raw)?, None);
        let prod = cd.target("prod").ok_or("prod")?;
        assert!(prod.promote && prod.deploy.is_none());
        // SPEC-418 AC4: no rollback command (nothing to redeploy); the line says how Yulius
        // rolls back instead.
        assert!(prod.rollback.is_none());
        let about = prod.about.as_deref().unwrap_or_default();
        assert!(
            about.contains("restores the previous binary and database")
                && about.contains("revert on dev and promote again"),
            "{about}"
        );
        assert_eq!(cd.environments.len(), 2, "only the defaults");
        Ok(())
    }

    /// SPEC-257 CDN-001/002: named environments with their stage, in file order, next to the
    /// defaults, whose stage `stage` may repeat.
    #[test]
    fn cd_tables_define_named_environments_with_a_stage() -> TestResult {
        let cd = parse_yulius_toml_cd(
            "[cd.dev]\nstage = \"dev\"\n\n[cd.selfserve]\nstage = \"prod\"\npromote = true\n\n\
             [cd.platform]\nstage = 'prod'\ndeploy = \"scripts/cd/deploy.sh platform\"\n\
             secrets = \"PLATFORM_SSH_KEY\"\n\n[cd.preview-2]\nstage = \"dev\"\n",
        )?;
        let names: Vec<(&str, CdStage)> = cd
            .environments
            .iter()
            .map(|t| (t.name.as_str(), t.stage))
            .collect();
        assert_eq!(
            names,
            [
                ("dev", CdStage::Dev),
                ("selfserve", CdStage::Prod),
                ("platform", CdStage::Prod),
                ("preview-2", CdStage::Dev),
            ]
        );
        let platform = cd.target("platform").ok_or("platform")?;
        assert_eq!(
            platform.deploy.as_deref(),
            Some("scripts/cd/deploy.sh platform")
        );
        assert_eq!(platform.secrets, ["PLATFORM_SSH_KEY"]);
        assert!(cd.target("selfserve").is_some_and(|t| t.promote));
        assert!(
            cd.is_defined(),
            "no [cd.prod], but production-stage environments"
        );
        assert!(parse_yulius_toml("[cd.staging]\nstage = \"dev\"\n[cd.prod]\n").is_ok());
        Ok(())
    }

    /// SPEC-142 CSE-007, SPEC-162 SKP-005: Yulius's own CI has its `security` job, granted
    /// `VIDOC_API_KEY` alone (the running instance knows `secrets` since the promotion of #517);
    /// no other job gets a secret.
    #[test]
    fn the_repositorys_own_security_job_alone_is_granted_the_vidoc_key() -> TestResult {
        let config = parse_yulius_toml(include_str!("../../../../yulius.toml"))?;
        let jobs = crate::ci::project_jobs(config.commands.gate.as_deref(), &config.ci)?;
        for job in &jobs {
            let expected: &[&str] = if job.name == "security" {
                &["VIDOC_API_KEY"]
            } else {
                &[]
            };
            assert_eq!(job.secrets, expected, "{jobs:?}");
        }
        assert!(jobs.iter().any(|j| j.name == "security"), "{jobs:?}");
        Ok(())
    }

    /// SPEC-134 CDC-001: every invalid form names its line.
    #[test]
    fn cd_tables_refuse_what_cannot_run() {
        let cases: [(&str, usize, &str); 17] = [
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
            ("[cd.staging]\n", 1, "needs 'stage = \"dev\"'"),
            (
                "[cd.staging]\nstage = \"production\"\n",
                2,
                "expected one of: \"dev\" or",
            ),
            ("[cd.dev]\nstage = \"prod\"\n", 2, "the stage of [cd.dev]"),
            ("[cd.prod]\nstage = \"dev\"\n", 2, "the stage of [cd.prod]"),
            (
                "[cd.staging]\nstage = \"dev\"\npromote = true\n",
                3,
                "unknown key 'promote' in [cd.staging]",
            ),
            ("[cd.Staging]\n", 1, "is not a CD environment"),
            ("[cd.a.b]\n", 1, "is not a CD environment"),
            ("[cd]\n", 1, "is not a CD environment"),
            (
                "[cd.production]\nstage = \"prod\"\n",
                1,
                "is not a CD environment",
            ),
            ("[[cd.dev]]\n", 1, "[cd.dev] must be a single table"),
            ("[cd.dev]\n[cd.dev]\n", 2, "duplicate key"),
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
        let environments: String = (0..17)
            .map(|n| format!("[cd.e{n}]\nstage = \"dev\"\n"))
            .collect();
        let error = parse_yulius_toml_cd(&environments).err();
        assert_eq!(error.as_ref().map(|e| e.line), Some(33), "the 17th table");
        assert!(error.is_some_and(|e| e.to_string().contains("at most 16 environments")));
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
            Some((3, ConfigProblem::Syntax("duplicate key".to_owned())))
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

    /// SPEC-504 CFG-005: valid TOML parses: a multi-line array with one entry per line (what
    /// autoresearch's PR #42 merged), trailing commas, comments inside arrays and after values,
    /// literal and multi-line strings, and a job written as an inline table; unknown tables of any
    /// shape are still ignored.
    #[test]
    fn valid_toml_parses_whatever_its_layout() -> TestResult {
        let text = r#"
[commands]
test = """cargo test"""   # a multi-line basic string on one line

[[ci.jobs]]
name = "web"
command = 'npm test'
paths = [
  "web/**",   # the site
  "e2e/**",
  # and its fixtures
  "fixtures/**",
]
cpus = 4_0

[future.thing]
anything = { a = [1, 2.5, { b = 1979-05-27 }] }
"#;
        let config = parse_yulius_toml(text)?;
        assert_eq!(config.commands.test.as_deref(), Some("cargo test"));
        assert_eq!(config.ci.len(), 1);
        assert_eq!(config.ci[0].paths, ["web/**", "e2e/**", "fixtures/**"]);
        assert_eq!(config.ci[0].need.cpus, Some(40));
        let inline = "ci.jobs = [ { name = \"t\", command = \"true\" } ]\n";
        assert_eq!(parse_yulius_toml(inline)?.ci[0].name, "t");
        Ok(())
    }

    #[test]
    fn precise_errors_with_line_numbers() {
        let syntax = |text: &str| match problem(text) {
            Some((line, ConfigProblem::Syntax(message))) => Some((line, message)),
            _ => None,
        };
        // Invalid TOML: the parser's message at its line (SPEC-504 CFG-005).
        for (text, line) in [
            ("[docs]\nproduct = docs/product.md\n", 2),
            ("[docs]\nproduct = \"x.md\" extra\n", 2),
            ("[docs]\nproduct = \"x.md\n", 2),
            ("[docs]\nproduct = \"a\\qb\"\n", 2),
            ("[docs]\njust text\n", 2),
            ("[docs\nproduct = \"x\"\n", 1),
            ("[docs]\n[docs]\n", 2),
            ("[commands]\ntest = \"a\"\ntest = \"b\"\n", 3),
            (
                "[[ci.jobs]]\nname = \"a\"\ncommand = \"b\"\npaths = [\n  \"x\",\n  oops\n]\n",
                6,
            ),
        ] {
            let found = syntax(text);
            assert_eq!(
                found.as_ref().map(|(at, _)| *at),
                Some(line),
                "{text:?}: {found:?}"
            );
            assert!(
                found.is_some_and(|(_, message)| !message.is_empty()),
                "{text:?}"
            );
        }
        // Valid TOML outside what a known table reads.
        assert_eq!(
            problem("[docs]\nproduct = 1.5\n"),
            Some((2, ConfigProblem::UnsupportedValue))
        );
        for text in [
            "[docs]\na.b = \"x\"\n",
            "[docs]\na = { b = \"x\" }\n",
            "[docs.a]\nb = \"x\"\n",
        ] {
            assert!(
                matches!(problem(text), Some((_, ConfigProblem::NestedTable { ref table, ref key })) if table == "docs" && key == "a"),
                "{text}"
            );
        }
        assert_eq!(
            problem("[[docs]]\n"),
            Some((1, ConfigProblem::NotAnArrayTable("docs".to_owned())))
        );
        assert_eq!(
            problem("docs = \"x\"\n"),
            Some((1, ConfigProblem::NotATable("docs".to_owned())))
        );
        assert_eq!(
            problem("[delivery.environments]\n"),
            Some((
                1,
                ConfigProblem::ArrayTableRequired("delivery.environments".to_owned())
            ))
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
                "yulius.toml line 2: unsupported value; use a \"quoted string\", a whole number, true/false or an array of \"quoted\", \"strings\" (signed or fractional numbers, dates and inline tables are not values Yulius reads here)"
            )
        );
        assert_eq!(
            problem("[docs]\nproduct = 1\n"),
            Some((2, ConfigProblem::ExpectedString("product".to_owned())))
        );
    }

    #[test]
    fn a_changes_path_with_a_trailing_slash_names_a_folder() -> TestResult {
        let folder = parse_yulius_toml("[docs]\nchanges = \"docs/changes/\"\n")?.docs;
        assert_eq!(folder.changes, "docs/changes/");
        assert_eq!(folder.changes_folder(), Some("docs/changes"));
        let file = parse_yulius_toml("[docs]\nchanges = \"CHANGELOG.md\"\n")?.docs;
        assert_eq!(file.changes, "CHANGELOG.md");
        assert_eq!(file.changes_folder(), None);
        assert_eq!(DocsLayout::default().changes_folder(), None);
        assert!(parse_yulius_toml("[docs]\nchanges = \"docs/changes//\"\n").is_err());
        let rendered = render_yulius_toml(&ProjectConfig {
            docs: folder.clone(),
            ..ProjectConfig::default()
        });
        assert!(
            rendered.contains("changes = \"docs/changes/\"\n"),
            "{rendered}"
        );
        assert_eq!(parse_yulius_toml(&rendered)?.docs, folder);
        Ok(())
    }

    #[test]
    fn an_existing_changes_directory_names_a_folder() {
        let mut layout = DocsLayout {
            changes: "docs/changes".to_owned(),
            ..DocsLayout::default()
        };
        layout.resolve_changes_folder(|path| path == "docs/changes");
        assert_eq!(layout.changes_folder(), Some("docs/changes"));
        // Already a folder, or a file: unchanged, and the checkout is not asked again.
        layout.resolve_changes_folder(|_| unreachable!("a folder is not resolved again"));
        assert_eq!(layout.changes, "docs/changes/");
        let mut file = DocsLayout::default();
        file.resolve_changes_folder(|_| false);
        assert_eq!(file, DocsLayout::default());
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
                        secrets: Vec::new(),
                        timeout_minutes: None,
                        exclusive: false,
                        paths: Vec::new(),
                        after_merge: false,
                    },
                    JobDef {
                        name: "test".to_owned(),
                        command: "cargo test --partition \"hash:$YULIUS_CI_MATRIX\"".to_owned(),
                        matrix: vec!["1/2".to_owned(), "2/2".to_owned()],
                        need: Need {
                            cpus: Some(6),
                            memory_mb: Some(12 * 1024),
                        },
                        secrets: vec!["VIDOC_API_KEY".to_owned()],
                        timeout_minutes: Some(120),
                        exclusive: true,
                        paths: Vec::new(),
                        after_merge: false,
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

    /// SPEC-435 JTL-001/003: a job sets its own time limit (1-360 minutes) and `exclusive`; its
    /// matrix jobs inherit both, and a job without them keeps the instance's limit, not exclusive.
    #[test]
    fn ci_jobs_set_their_own_time_limit_and_exclusive() -> TestResult {
        let config = parse_yulius_toml(
            "[[ci.jobs]]\nname = \"websites\"\ncommand = \"scripts/gate websites\"\n\
             timeout_minutes = 120\nexclusive = true\nmatrix = \"a b\"\n\n\
             [[ci.jobs]]\nname = \"lint\"\ncommand = \"make lint\"\n",
        )?;
        let jobs = crate::ci::project_jobs(None, &config.ci)?;
        let set: Vec<(&str, Option<u32>, bool)> = jobs
            .iter()
            .map(|j| (j.name.as_str(), j.timeout_minutes, j.exclusive))
            .collect();
        assert_eq!(
            set,
            [
                ("websites (a)", Some(120), true),
                ("websites (b)", Some(120), true),
                ("lint", None, false),
            ]
        );
        let job = |extra: &str| format!("[[ci.jobs]]\nname = \"x\"\ncommand = \"y\"\n{extra}\n");
        for (extra, key) in [
            ("timeout_minutes = 0", "timeout_minutes"),
            ("timeout_minutes = 361", "timeout_minutes"),
            ("timeout_minutes = \"2\"", "timeout_minutes"),
        ] {
            assert_eq!(
                problem(&job(extra)),
                Some((
                    4,
                    ConfigProblem::ExpectedNumber {
                        key: key.to_owned(),
                        max: 360
                    }
                )),
                "{extra}"
            );
        }
        assert_eq!(
            problem(&job("exclusive = \"yes\"")),
            Some((4, ConfigProblem::ExpectedBool("exclusive".to_owned())))
        );
        assert!(parse_yulius_toml(&job("timeout_minutes = 360\nexclusive = false")).is_ok());
        Ok(())
    }

    /// SPEC-575 MFS-002: a job of the full suite says so with `after_merge`; its matrix jobs
    /// inherit it, it renders back, and a value other than a boolean is refused.
    #[test]
    fn ci_jobs_mark_the_full_suite_after_merge() -> TestResult {
        let text = "[[ci.jobs]]\nname = \"e2e\"\ncommand = \"scripts/e2e\"\nmatrix = \"a b\"\n\
                    after_merge = true\n\n[[ci.jobs]]\nname = \"quick\"\ncommand = \"q\"\n";
        let config = parse_yulius_toml(text)?;
        assert_eq!(
            config
                .ci
                .iter()
                .map(|j| (j.name.as_str(), j.after_merge))
                .collect::<Vec<_>>(),
            [("e2e", true), ("quick", false)]
        );
        let jobs = crate::ci::project_jobs(None, &config.ci)?;
        assert_eq!(
            jobs.iter()
                .map(|j| (j.name.as_str(), j.after_merge))
                .collect::<Vec<_>>(),
            [("e2e (a)", true), ("e2e (b)", true), ("quick", false)]
        );
        let rendered = render_yulius_toml(&config);
        assert!(rendered.contains("after_merge = true\n"), "{rendered}");
        assert_eq!(parse_yulius_toml(&rendered)?.ci, config.ci);
        assert_eq!(
            problem("[[ci.jobs]]\nname = \"x\"\ncommand = \"y\"\nafter_merge = \"yes\"\n"),
            Some((4, ConfigProblem::ExpectedBool("after_merge".to_owned())))
        );
        // A pull request needs at least one quick check (review round 1).
        assert_eq!(
            problem("[[ci.jobs]]\nname = \"x\"\ncommand = \"y\"\nafter_merge = true\n"),
            Some((1, ConfigProblem::NoQuickCheck))
        );
        Ok(())
    }

    /// SPEC-575 MFS-010: `[local]` names the task-local deployment, a deploy command with an
    /// optional verify and about; anything else is refused at its line.
    #[test]
    fn local_names_the_task_local_deployment() -> TestResult {
        let local = parse_yulius_toml_local(
            "[local]\nabout = \"the app on this sandbox's ports\"\n\
             deploy = \"scripts/cd/deploy.sh local\"\nverify = \"scripts/cd/verify.sh local\"\n",
        )?
        .ok_or("local")?;
        assert_eq!(local.deploy, "scripts/cd/deploy.sh local");
        assert_eq!(local.verify.as_deref(), Some("scripts/cd/verify.sh local"));
        assert_eq!(
            local.about.as_deref(),
            Some("the app on this sandbox's ports")
        );
        assert_eq!(parse_yulius_toml_local("[commands]\ntest = \"t\"\n")?, None);
        for (bad, says) in [
            ("[local]\nverify = \"v\"\n", "deploy"),
            ("[local]\ndeploy = \"d\"\nhost = \"x\"\n", "host"),
            ("[local]\ndeploy = \"\"\n", "deploy"),
        ] {
            let error = parse_yulius_toml(bad).err().ok_or("refused")?;
            assert!(error.to_string().contains(says), "{bad}: {error}");
        }
        Ok(())
    }

    /// SPEC-650 PST-006: staging moves on by itself on a timer and after a number of merges,
    /// configurable on the merge-first target; `0` turns either off.
    #[test]
    fn the_merge_first_target_serves_on_a_schedule() -> TestResult {
        let staging = "[cd.staging]\nstage = \"dev\"\nmerge_first = true\ndeploy = \"d\"\n\
                       verify = \"v\"\nrollback = \"r\"\nmigrations = \"none: stateless\"\n";
        let target = |extra: &str| -> Result<CdTarget, FormatError> {
            let cd = parse_yulius_toml_cd(&format!("[cd.dev]\n[cd.prod]\n{staging}{extra}"))?;
            Ok(cd
                .target("staging")
                .cloned()
                .unwrap_or_else(|| CdTarget::new("x", CdStage::Dev)))
        };
        let plain = target("")?;
        assert_eq!(
            (plain.serve_every_minutes, plain.serve_after_merges),
            (60, 5)
        );
        let set = target("serve_every_minutes = 30\nserve_after_merges = 0\n")?;
        assert_eq!((set.serve_every_minutes, set.serve_after_merges), (30, 0));
        let most = target("serve_every_minutes = 10080\nserve_after_merges = 1000\n")?;
        assert_eq!(
            (most.serve_every_minutes, most.serve_after_merges),
            (10_080, 1_000)
        );
        for bad in [
            "serve_every_minutes = 10081\n",
            "serve_after_merges = 1001\n",
            "serve_every_minutes = \"soon\"\n",
        ] {
            let said = target(bad).err().map(|e| e.to_string()).unwrap_or_default();
            assert!(said.contains("serve_"), "{bad}: {said}");
        }
        let said = parse_yulius_toml_cd("[cd.dev]\nserve_every_minutes = 30\n")
            .err()
            .map(|e| e.to_string())
            .unwrap_or_default();
        assert!(said.contains("merge_first"), "{said}");
        assert!(parse_yulius_toml_cd("[cd.prod]\nserve_after_merges = 2\n").is_err());
        Ok(())
    }

    /// SPEC-575 MFS-001: one dev-stage environment may be the merge-first staging target; it
    /// deploys, verifies and rolls back, and is the only one.
    #[test]
    fn a_dev_stage_environment_is_the_merge_first_target() -> TestResult {
        let staging = "[cd.staging]\nstage = \"dev\"\nmerge_first = true\ndeploy = \"d\"\n\
                       verify = \"v\"\nrollback = \"r\"\nmigrations = \"none: stateless\"\n";
        let cd = parse_yulius_toml_cd(&format!("[cd.dev]\n[cd.prod]\n{staging}"))?;
        assert!(cd.target("staging").ok_or("staging")?.merge_first);
        assert!(!cd.target("dev").ok_or("dev")?.merge_first);
        assert_eq!(cd.merge_first().map(|t| t.name.as_str()), Some("staging"));
        assert_eq!(parse_yulius_toml_cd("[cd.dev]\n")?.merge_first(), None);
        // An empty `merge_first = false` table is no target.
        assert_eq!(
            parse_yulius_toml_cd("[cd.dev]\nmerge_first = false\n")?.merge_first(),
            None
        );
        let without = |key: &str| {
            staging
                .lines()
                .filter(|line| !line.starts_with(key))
                .map(|line| format!("{line}\n"))
                .collect::<String>()
        };
        for (bad, says) in [
            (without("deploy"), "merge_first"),
            (without("verify"), "merge_first"),
            (without("rollback"), "merge_first"),
            (
                format!(
                    "{staging}[cd.qa]\nstage = \"dev\"\nmerge_first = true\ndeploy = \"d\"\nverify = \"v\"\nrollback = \"r\"\nmigrations = \"m\"\n"
                ),
                "only one",
            ),
            ("[cd.prod]\nmerge_first = true\n".to_owned(), "merge_first"),
            (
                "[cd.dev]\nmerge_first = \"yes\"\n".to_owned(),
                "merge_first",
            ),
        ] {
            let error = parse_yulius_toml_cd(&bad).err().ok_or("refused")?;
            assert!(error.to_string().contains(says), "{bad}: {error}");
        }
        Ok(())
    }

    /// SPEC-493 CJP-001 (AC-16): a job names the files it tests, as scope-claim globs (a folder's
    /// trailing `/` normalized), its matrix jobs inherit them, they render back, and a bad list
    /// is a configuration error naming the job.
    #[test]
    fn ci_jobs_name_the_files_they_test() -> TestResult {
        let text = "[[ci.jobs]]\nname = \"websites\"\ncommand = \"scripts/gate websites\"\n\
                    matrix = \"a b\"\npaths = [\"crates/pomerado/\", \"connectors/**/*.rs\"]\n";
        let config = parse_yulius_toml(text)?;
        let jobs = crate::ci::project_jobs(None, &config.ci)?;
        assert!(
            jobs.iter()
                .all(|j| j.paths == ["crates/pomerado/**", "connectors/**/*.rs"]),
            "{jobs:?}"
        );
        let rendered = render_yulius_toml(&config);
        assert!(
            rendered.contains("paths = [\"crates/pomerado/**\", \"connectors/**/*.rs\"]"),
            "{rendered}"
        );
        assert_eq!(parse_yulius_toml(&rendered)?, config);
        let job = |extra: &str| format!("[[ci.jobs]]\nname = \"x\"\ncommand = \"y\"\n{extra}\n");
        for extra in [
            "paths = []",
            "paths = [\"../up\"]",
            "paths = [\"/abs\"]",
            "paths = [\"a/**b\"]",
            "paths = [\"a\", \"a\"]",
        ] {
            // Named even when `paths` comes before `name`.
            assert_eq!(
                problem(&format!(
                    "[[ci.jobs]]\n{extra}\nname = \"x\"\ncommand = \"y\"\n"
                )),
                Some((
                    2,
                    ConfigProblem::Ci(CiConfigError::InvalidPaths("x".to_owned()))
                )),
                "{extra} first"
            );
            assert_eq!(
                problem(&job(extra)),
                Some((
                    4,
                    ConfigProblem::Ci(CiConfigError::InvalidPaths("x".to_owned()))
                )),
                "{extra}"
            );
        }
        Ok(())
    }

    /// SPEC-142 CSE-001: a job names the agent secrets it may use, like `[cd.<env>] secrets`;
    /// its matrix jobs inherit them, and a bad list names the job.
    #[test]
    fn ci_jobs_name_their_agent_secrets() -> TestResult {
        let config = parse_yulius_toml(
            "[[ci.jobs]]\nname = \"scan\"\ncommand = \"make scan\"\nsecrets = \"VIDOC_API_KEY  OTHER\"\n\
             matrix = \"a b\"\n\n[[ci.jobs]]\nname = \"lint\"\ncommand = \"make lint\"\n",
        )?;
        assert_eq!(config.ci[0].secrets, ["VIDOC_API_KEY", "OTHER"]);
        assert!(config.ci[1].secrets.is_empty());
        let jobs = crate::ci::project_jobs(None, &config.ci)?;
        assert_eq!(
            jobs[1].secrets,
            ["VIDOC_API_KEY", "OTHER"],
            "matrix jobs inherit"
        );
        assert!(jobs[2].secrets.is_empty());
        assert!(
            render_yulius_toml(&config).contains("secrets = \"VIDOC_API_KEY OTHER\"\n"),
            "{}",
            render_yulius_toml(&config)
        );
        let many: Vec<String> = (0..=crate::ci::MAX_JOB_SECRETS)
            .map(|i| format!("S{i}"))
            .collect();
        for bad in [
            "\"lower\"".to_owned(),
            "\"A A\"".to_owned(),
            "\"  \"".to_owned(),
            "\"A $(B)\"".to_owned(),
            "true".to_owned(),
            format!("\"{}\"", many.join(" ")),
        ] {
            let text = format!("[[ci.jobs]]\nname = \"x\"\ncommand = \"y\"\nsecrets = {bad}\n");
            assert!(
                matches!(
                    problem(&text),
                    Some((4, ConfigProblem::Ci(CiConfigError::InvalidSecrets(ref job)))) if job == "x"
                ) || (bad == "true" && problem(&text).is_some_and(|(line, _)| line == 4)),
                "{bad}: {:?}",
                problem(&text)
            );
        }
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
        // SPEC-268 PID-007: a table not there differs from an empty one.
        assert_eq!(parse_yulius_toml_resources_table("[docs]\n")?, None);
        assert_eq!(
            parse_yulius_toml_resources_table("[resources.env]\n")?,
            Some(Vec::new())
        );

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
        for value in ["-1", "1.5"] {
            assert_eq!(
                problem(&format!("{job}cpus = {value}\n")),
                Some((4, ConfigProblem::UnsupportedValue)),
                "{value}"
            );
        }
        // TOML's own: `4_0` is 40, `4x` is no value at all (SPEC-504 CFG-005).
        assert_eq!(
            parse_yulius_toml(&format!("{job}cpus = 4_0\n"))?.ci[0]
                .need
                .cpus,
            Some(40)
        );
        assert!(matches!(
            problem(&format!("{job}cpus = 4x\n")),
            Some((4, ConfigProblem::Syntax(_)))
        ));
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

    /// SPEC-493 APR-002: `[review]` sets what counts as product code; its keys and globs are
    /// strict, and the whole contract checks them.
    #[test]
    fn the_review_table_sets_what_counts_as_product_code() -> TestResult {
        assert_eq!(parse_yulius_toml_review("")?, ProductRule::default());
        let rule = parse_yulius_toml_review(
            "[review]\nproduct = [\"crates/\", \"web/**\"]\nnot_product = \"crates/gen/\"\n",
        )?;
        assert!(rule.counts("crates/a/src/lib.rs"));
        assert!(rule.counts("web/app.js"));
        assert!(!rule.counts("crates/gen/x.rs"));
        assert!(!rule.counts("scripts/dev"));
        for bad in [
            "[review]\nproducts = [\"a\"]\n",
            "[review]\nproduct = []\n",
            "[review]\nproduct = [\"/etc\"]\n",
            "[review]\nnot_product = [\"a/../b\"]\n",
            "[[review]]\nproduct = [\"a\"]\n",
        ] {
            assert!(parse_yulius_toml_review(bad).is_err(), "{bad}");
            assert!(parse_yulius_toml(bad).is_err(), "{bad}");
        }
        Ok(())
    }

    /// SPEC-497 MQW-003 (review round 1): `[merge_queue] apart` takes 1 to 50 repository globs,
    /// as a list or a string; anything else is refused by its own parse and the whole file's.
    #[test]
    fn the_merge_queue_table_names_derived_files() -> TestResult {
        assert!(parse_yulius_toml_merge_queue("")?.is_empty());
        let apart = parse_yulius_toml_merge_queue("[merge_queue]\napart = \"gen/ Cargo.lock\"\n")?;
        let names: Vec<&str> = apart.iter().map(Pattern::as_str).collect();
        assert_eq!(names, ["gen/**", "Cargo.lock"]);
        parse_yulius_toml("[merge_queue]\napart = [\"gen/\"]\n")?;
        let many = (0..51)
            .map(|n| format!("\"d{n}/\""))
            .collect::<Vec<_>>()
            .join(", ");
        for bad in [
            "[merge_queue]\napart = []\n".to_owned(),
            format!("[merge_queue]\napart = [{many}]\n"),
            "[merge_queue]\napart = [\"/etc\"]\n".to_owned(),
            "[merge_queue]\napart = [\"a/../b\"]\n".to_owned(),
            "[merge_queue]\napart = 3\n".to_owned(),
            "[merge_queue]\nserial = [\"a\"]\n".to_owned(),
            "[[merge_queue]]\napart = [\"a\"]\n".to_owned(),
        ] {
            assert!(parse_yulius_toml_merge_queue(&bad).is_err(), "{bad}");
            assert!(parse_yulius_toml(&bad).is_err(), "{bad}");
        }
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
