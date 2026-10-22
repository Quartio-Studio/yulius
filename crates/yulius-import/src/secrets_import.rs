//! `yulius secrets import` (SPEC-205): team or agent secrets created over the vault API from a
//! dotenv file, key files or hidden prompts, after the person has read the names and typed `yes`,
//! or unattended with the operate token made for their agent (SPEC-256, ADR-0194). Entries that
//! are not secrets (too short, a plain word, a URL or an e-mail address without credentials) go to
//! the project's variables instead, when offered (SPEC-275, ADR-0203).
//!
//! Values never reach output: [`SecretsIo::say`] gets names, classes and server messages only,
//! values stay in [`Secret`] (wiped on drop), and nothing is written to disk. A create is not
//! idempotent, so it is sent once, never retried.

use std::io;
use std::path::Path;

use reqwest::Url;
use serde::Deserialize;
use serde::de::IgnoredAny;
use yulius_contracts::api::secrets::{AgentSecretInput, TeamSecretInput};
use yulius_contracts::api::variables::{
    ProjectVariableInput, ProjectVariables, ProjectVariablesSet, ProjectVariablesSetResult,
};
use yulius_domain::vault::names::SecretName;
use yulius_domain::vault::plain::{NotASecret, not_a_secret};
use yulius_domain::vault::{MAX_FIELD_BYTES, check_agent_value, dotenv, ordinary_text};
use zeroize::Zeroize;

use crate::dotenv::{DotenvEntry, MAX_DOTENV_ENTRIES};
use crate::git::{checkout_of, discover_repo};
use crate::http::{ApiClient, ApiError, JSON_TIMEOUT, RetryPolicy};
use crate::repo_values::find_in_checkout;
use crate::secret::Secret;

/// Longest team secret title (the server's limit).
const MAX_TITLE_CHARS: usize = 200;

/// Where the secrets go.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SecretsTarget {
    /// Team secrets, for people, titled by the name: a sandbox's (SPEC-216), or team-wide.
    Team {
        /// `--sandbox`: an id, slug or `owner/name` (`None`: the checkout's GitHub repository).
        sandbox: Option<String>,
        /// `--instance`: team-wide, for every sandbox (no sandbox is resolved).
        instance: bool,
    },
    /// Agent secrets of a sandbox.
    Agent {
        /// `--sandbox`: an id, slug or `owner/name` (`None`: the checkout's GitHub repository).
        sandbox: Option<String>,
        /// `--instance`: global secrets, switched on for this sandbox (SPEC-245); an administrator
        /// switches them on for others.
        instance: bool,
        /// `--confirm-hosts`: a person allows each host and production deploys; otherwise agents
        /// set them up (the server's default, SPEC-216).
        confirm_hosts: bool,
    },
}

/// What one run imports and where.
#[derive(Debug, Clone)]
pub struct SecretsImportOptions {
    /// The target.
    pub target: SecretsTarget,
    /// The checkout whose GitHub remote names the sandbox when `--sandbox` is absent.
    pub checkout: std::path::PathBuf,
    /// The file the values came from, for the reminder that it still holds them.
    pub source_file: Option<String>,
}

/// The terminal of a run: everything said holds no value.
pub trait SecretsIo {
    /// Shows a line.
    fn say(&mut self, line: &str);
    /// Asks a question at the terminal; an error means there is no terminal to ask.
    ///
    /// # Errors
    /// No terminal, or reading failed.
    fn ask(&mut self, prompt: &str) -> io::Result<String>;
    /// Asks for a value without echoing it.
    ///
    /// # Errors
    /// No terminal, or reading failed.
    fn ask_hidden(&mut self, prompt: &str) -> io::Result<Secret>;
}

/// How a run ended.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SecretsSummary {
    /// Created.
    pub imported: Vec<String>,
    /// Sent as project variables (SPEC-275).
    pub variables: Vec<String>,
    /// Skipped: a secret (or a project variable) with that name exists.
    pub existing: Vec<String>,
    /// Skipped: refused before sending.
    pub invalid: Vec<String>,
    /// Skipped: the person answered no about a value the repository holds (SPEC-274 PLW-003).
    pub declined: Vec<String>,
    /// Refused by the server.
    pub failed: Vec<String>,
    /// Not sent because the run stopped (expired sign-in, unreachable server).
    pub unsent: Vec<String>,
}

impl SecretsSummary {
    /// Whether nothing was invalid, refused or left unsent.
    #[must_use]
    pub fn ok(&self) -> bool {
        self.invalid.is_empty() && self.failed.is_empty() && self.unsent.is_empty()
    }
}

/// A run that imported nothing.
#[derive(Debug, thiserror::Error)]
pub enum SecretsImportError {
    /// The person did not type `yes`, or there was no terminal to ask.
    #[error("cancelled; nothing was imported")]
    Cancelled,
    /// No terminal for the prompts.
    #[error("{0}")]
    Terminal(String),
    /// The sandbox could not be chosen.
    #[error("{0}")]
    Sandbox(String),
    /// More secrets than one run takes.
    #[error("at most {MAX_DOTENV_ENTRIES} secrets per run")]
    TooMany,
    /// The server refused or could not be reached before anything was sent.
    #[error(transparent)]
    Api(#[from] ApiError),
}

#[derive(Deserialize)]
struct SandboxRef {
    id: String,
    #[serde(default)]
    slug: String,
    #[serde(default)]
    name: String,
    repo: RepoRef,
}

#[derive(Deserialize)]
struct RepoRef {
    owner: String,
    name: String,
}

impl SandboxRef {
    fn full_name(&self) -> String {
        format!("{}/{}", self.repo.owner, self.repo.name)
    }
}

#[derive(Deserialize)]
struct TeamName {
    title: String,
    #[serde(default)]
    scope: Option<String>,
}

#[derive(Deserialize)]
struct AgentName {
    name: String,
    scope: String,
}

/// The vault calls of a run, with the CLI's bearer token.
pub struct VaultClient {
    api: ApiClient,
}

impl VaultClient {
    /// A client of `server` (https, or http on loopback).
    ///
    /// # Errors
    /// An unusable server URL or HTTP client.
    pub fn new(server: Url, token: Secret) -> Result<Self, ApiError> {
        let mut api = ApiClient::new(server, Some(token))?;
        api.set_retry(RetryPolicy::immediate(1));
        Ok(Self { api })
    }

    /// The server.
    #[must_use]
    pub fn server(&self) -> &Url {
        self.api.base()
    }

    /// The API client (agent states, SPEC-271).
    pub(crate) fn api(&self) -> &ApiClient {
        &self.api
    }

    fn get<T: serde::de::DeserializeOwned>(&self, segments: &[&str]) -> Result<T, ApiError> {
        let url = self.api.endpoint(segments)?;
        self.api
            .execute(&|client| client.get(url.clone()).timeout(JSON_TIMEOUT))
    }

    fn put<B: serde::Serialize, T: serde::de::DeserializeOwned>(
        &self,
        segments: &[&str],
        body: &B,
    ) -> Result<T, ApiError> {
        let url = self.api.endpoint(segments)?;
        self.api
            .execute(&|client| client.put(url.clone()).timeout(JSON_TIMEOUT).json(body))
    }

    fn post<B: serde::Serialize>(&self, segments: &[&str], body: &B) -> Result<(), ApiError> {
        let url = self.api.endpoint(segments)?;
        let _: IgnoredAny = self
            .api
            .execute(&|client| client.post(url.clone()).timeout(JSON_TIMEOUT).json(body))?;
        Ok(())
    }
}

/// Asks for secrets one by one: a name (shown), its value (hidden) and an optional description,
/// until an empty name.
///
/// # Errors
/// [`SecretsImportError::Terminal`] without a terminal; [`SecretsImportError::TooMany`].
pub fn prompt_secrets(io: &mut dyn SecretsIo) -> Result<Vec<DotenvEntry>, SecretsImportError> {
    let terminal = |e: io::Error| SecretsImportError::Terminal(format!("no terminal to ask: {e}"));
    io.say(
        "Enter each secret's name and value; the value is not shown. An empty name finishes. \
         A key that spans lines (PEM, OpenSSH) goes in a file instead.",
    );
    let mut entries: Vec<DotenvEntry> = Vec::new();
    loop {
        let name = io.ask("Name: ").map_err(terminal)?.trim().to_owned();
        if name.is_empty() {
            return Ok(entries);
        }
        if entries.iter().any(|e| e.name == name) {
            io.say(&format!("{name} was already entered."));
            continue;
        }
        if entries.len() == MAX_DOTENV_ENTRIES {
            return Err(SecretsImportError::TooMany);
        }
        let value = io
            .ask_hidden(&format!("Value of {name} (hidden): "))
            .map_err(terminal)?;
        if cut_key(value.expose()) {
            io.say(CUT_KEY);
            continue;
        }
        let comment = io
            .ask("Description (optional): ")
            .map_err(terminal)?
            .trim()
            .to_owned();
        entries.push(DotenvEntry {
            line: entries.len() + 1,
            name,
            value,
            comment,
        });
    }
}

/// Why a key's first line alone is refused.
const CUT_KEY: &str =
    "that is only the first line of a key: import a key from a file, where it spans lines";

/// Whether `value` opens a PEM or OpenSSH key that never closes: a key pasted on one line lost
/// everything after its first line.
fn cut_key(value: &str) -> bool {
    let value = value.trim();
    value.starts_with("-----BEGIN ") && !value.contains("-----END ")
}

/// Where a run's secrets go, checked before anything is read or typed.
pub struct PreparedTarget {
    heading: String,
    team: bool,
    sandbox: Option<String>,
    instance: bool,
    confirm_hosts: bool,
    existing: Vec<String>,
    source_file: Option<String>,
    unattended: bool,
    env_files: bool,
    /// Agent secrets: the project (`owner/name`) and its checkout the run is in, if it is in one
    /// (SPEC-274 PLW-003).
    project: Option<(String, Option<std::path::PathBuf>)>,
    variables: bool,
}

impl PreparedTarget {
    /// Sends without the typed `yes` (SPEC-256 LOB-002, ADR-0194): only for a run whose token
    /// the instance confirmed is an `operate` token made for the person's agent, which the
    /// caller checks. The plan is still shown.
    #[must_use]
    pub fn unattended(mut self) -> Self {
        self.unattended = true;
        self
    }

    /// Agent secrets are created already allowed into the project's `.env` files (SPEC-256
    /// LOB-004), so code outside the proxy (ssh, deploy scripts) gets them without a person.
    #[must_use]
    pub fn env_files_allowed(mut self) -> Self {
        self.env_files = true;
        self
    }

    /// Entries that are not secrets go to the project's variables (SPEC-275 VAR-007): only for
    /// one project's agent secrets.
    #[must_use]
    pub fn with_variables(mut self) -> Self {
        self.variables = !self.team && !self.instance;
        self
    }

    /// What the run is told it imports into.
    pub(crate) fn heading(&self) -> &str {
        &self.heading
    }

    /// The project's sandbox id, for agent secrets of one (agent states go only there).
    pub(crate) fn project_sandbox(&self) -> Option<&str> {
        (!self.team && !self.instance)
            .then_some(self.sandbox.as_deref())
            .flatten()
    }

    /// Whether it sends without the typed `yes`.
    pub(crate) fn is_unattended(&self) -> bool {
        self.unattended
    }
}

/// Largest key file `--from-file` reads (SPEC-256 LOB-003); the vault's own limits apply after.
pub const MAX_SECRET_FILE_BYTES: usize = 64 * 1024;

/// The secret `name` holding the whole file at `path` (`--from-file NAME=PATH`, SPEC-256
/// LOB-003), described by the file's name: the agent that found a key passes its path and never
/// reads it. Its value stays in a wiping wrapper; errors name the path, never the content.
///
/// # Errors
/// The file cannot be read, is not UTF-8 text or is larger than [`MAX_SECRET_FILE_BYTES`].
pub fn secret_file_entry(name: &str, path: &Path) -> Result<DotenvEntry, String> {
    use std::io::Read as _;
    let shown = path.display();
    let file = std::fs::File::open(path).map_err(|e| format!("cannot read {shown}: {e}"))?;
    let mut bytes = Vec::new();
    let read = file
        .take(MAX_SECRET_FILE_BYTES as u64 + 1)
        .read_to_end(&mut bytes);
    // A failed or oversized read is said as such, whatever its bytes decode to.
    let refused = match read {
        Err(e) => Some(format!("cannot read {shown}: {e}")),
        Ok(_) if bytes.len() > MAX_SECRET_FILE_BYTES => {
            Some(format!("{shown} is larger than 64 KiB"))
        }
        Ok(_) => None,
    };
    if let Some(refused) = refused {
        bytes.zeroize();
        return Err(refused);
    }
    let text = String::from_utf8(bytes).map_err(|e| {
        let mut bytes = e.into_bytes();
        bytes.zeroize();
        format!("{shown} is not UTF-8 text")
    });
    let value = Secret::new(text?);
    let file_name = path
        .file_name()
        .map_or_else(|| shown.to_string(), |n| n.to_string_lossy().into_owned());
    Ok(DotenvEntry {
        name: name.trim().to_owned(),
        value,
        comment: format!("From the file {file_name}"),
        line: 0,
    })
}

/// Resolves the sandbox (agent secrets) and lists the names that exist there.
///
/// # Errors
/// [`SecretsImportError::Sandbox`] when no single sandbox matches; an API failure.
pub fn prepare_target(
    options: &SecretsImportOptions,
    client: &VaultClient,
) -> Result<PreparedTarget, SecretsImportError> {
    let confirm_hosts = matches!(
        options.target,
        SecretsTarget::Agent {
            confirm_hosts: true,
            ..
        }
    );
    let mut project = None;
    let (heading, team, sandbox, instance) = match &options.target {
        SecretsTarget::Team {
            instance: true,
            sandbox: _,
        } => (
            "the team-wide team secrets (for people)".to_owned(),
            true,
            None,
            true,
        ),
        SecretsTarget::Team {
            sandbox,
            instance: false,
        } => {
            // Team-wide is what `--to team` did before SPEC-216: name it beside --sandbox.
            let found = resolve_sandbox(client, sandbox.as_deref(), &options.checkout).map_err(
                |e| match e {
                    SecretsImportError::Sandbox(message) => SecretsImportError::Sandbox(format!(
                        "{message}; or --instance for team-wide team secrets"
                    )),
                    other => other,
                },
            )?;
            (
                format!(
                    "the team secrets of {} (this sandbox only, for people)",
                    found.full_name()
                ),
                true,
                Some(found.id),
                false,
            )
        }
        SecretsTarget::Agent {
            sandbox, instance, ..
        } => {
            let found = resolve_sandbox(client, sandbox.as_deref(), &options.checkout)?;
            let full_name = found.full_name();
            let checkout = checkout_of(&options.checkout)
                .filter(|(_, remote)| {
                    format!("{}/{}", remote.owner, remote.name).eq_ignore_ascii_case(&full_name)
                })
                .map(|(root, _)| root);
            project = Some((full_name, checkout));
            let reach = if *instance {
                "global, switched on for this sandbox"
            } else {
                "this sandbox only"
            };
            (
                format!("the agent secrets of {} ({reach})", found.full_name()),
                false,
                Some(found.id),
                *instance,
            )
        }
    };
    let existing: Vec<String> = match (team, &sandbox) {
        (true, None) => client
            .get::<Vec<TeamName>>(&["api", "team-secrets"])?
            .into_iter()
            .map(|t| t.title.trim().to_owned())
            .collect(),
        (true, Some(id)) => client
            .get::<Vec<TeamName>>(&["api", "sandboxes", id, "team-secrets"])?
            .into_iter()
            .filter(|t| t.scope.as_deref() == Some("sandbox"))
            .map(|t| t.title.trim().to_owned())
            .collect(),
        (false, None) => Vec::new(),
        (false, Some(id)) => {
            let scope = if instance { "instance" } else { "sandbox" };
            client
                .get::<Vec<AgentName>>(&["api", "sandboxes", id, "agent-secrets"])?
                .into_iter()
                .filter(|a| a.scope == scope)
                .map(|a| a.name)
                .collect()
        }
    };
    Ok(PreparedTarget {
        heading,
        team,
        sandbox,
        instance,
        confirm_hosts,
        existing,
        source_file: options.source_file.clone(),
        unattended: false,
        env_files: false,
        project,
        variables: false,
    })
}

/// One secret's fate before anything is sent.
enum Class {
    New(Secret),
    /// New, but a tracked file of the project's checkout holds it: where (SPEC-274 PLW-003).
    InRepo(Secret, String),
    Exists,
    Invalid(String),
    /// Not a secret: a project variable (SPEC-275).
    Variable(Secret, NotASecret),
    /// Not a secret, and a project variable of that name exists.
    VariableExists,
}

/// The project variables that exist, or `None` when the server has none (older than SPEC-275).
fn variable_names(
    client: &VaultClient,
    sandbox: &str,
    io: &mut dyn SecretsIo,
) -> Result<Option<Vec<String>>, ApiError> {
    match client.get::<ProjectVariables>(&["api", "sandboxes", sandbox, "variables"]) {
        Ok(found) => Ok(Some(found.variables.into_iter().map(|v| v.name).collect())),
        Err(ApiError::NotFound(_)) => {
            io.say(
                "This server has no project variables: values that are not secrets are checked \
                 as secrets.",
            );
            Ok(None)
        }
        Err(err) => Err(err),
    }
}

/// `count` things, singular or plural.
fn counted(count: usize, thing: &str) -> String {
    format!("{count} {thing}{}", if count == 1 { "" } else { "s" })
}

/// Why a value found in the repository is not imported unattended.
const IN_REPO: &str = "redaction would mask it there";

/// Where ordinary text belongs, and where a credential that only looks like it goes: never into
/// the hand-over, which tasks read (review of SPEC-274).
const BELONGS: &str = "it belongs in configuration, a project variable (`yulius vars set`) or \
                       the hand-over; if it is a credential after \
                       all, a person adds it on the Secrets page";

/// Imports `entries`: classifies each, shows the plan, asks for `yes` and creates the new ones.
///
/// # Errors
/// [`SecretsImportError::Cancelled`] without `yes`. Failures while creating are in the summary.
pub fn import_secrets(
    entries: Vec<DotenvEntry>,
    target: &PreparedTarget,
    client: &VaultClient,
    io: &mut dyn SecretsIo,
) -> Result<SecretsSummary, SecretsImportError> {
    if entries.len() > MAX_DOTENV_ENTRIES {
        return Err(SecretsImportError::TooMany);
    }
    let PreparedTarget {
        heading,
        team,
        sandbox,
        instance,
        confirm_hosts,
        existing,
        source_file,
        unattended,
        env_files,
        variables,
        project,
    } = target;
    let instance = *instance;
    let variables = match (variables, target.project_sandbox()) {
        (true, Some(id)) => variable_names(client, id, io)?,
        _ => None,
    };
    let mut classes: Vec<(DotenvEntry, Class)> = entries
        .into_iter()
        .map(|entry| {
            let plain = variables
                .as_ref()
                .filter(|_| SecretName::parse(&entry.name).is_ok())
                .and_then(|names| {
                    not_a_secret(&entry.name, entry.value.expose())
                        .map(|why| (names.contains(&entry.name), why))
                });
            let class = if existing.contains(&entry.name) {
                Class::Exists
            } else if *team {
                team_value(&entry)
            } else if let Some((exists, why)) = plain {
                if exists {
                    Class::VariableExists
                } else {
                    Class::Variable(entry.value.clone(), why)
                }
            } else {
                agent_value(&entry)
            };
            (entry, class)
        })
        .collect();

    let mut summary = SecretsSummary::default();
    io.say(&format!("Import into {heading} on {}:", client.server()));
    if let Some((name, checkout)) = project {
        search_repository(&mut classes, name, checkout.as_deref(), *unattended, io);
    }
    let width = classes
        .iter()
        .map(|(e, _)| e.name.chars().count())
        .max()
        .unwrap_or(0);
    for (entry, class) in &classes {
        let (mark, what) = match class {
            // A comment goes along in plain view of agents (or as sealed notes): say so.
            Class::New(_) if entry.comment.is_empty() => ('+', "new".to_owned()),
            Class::New(_) if !team => ('+', "new, with description (agents read it)".to_owned()),
            Class::New(_) => ('+', "new, with notes".to_owned()),
            Class::InRepo(_, found) => ('?', format!("{found}: asked below")),
            Class::Exists => ('=', "exists: skipped, never overwritten".to_owned()),
            Class::Invalid(why) => ('!', format!("skipped: {why}")),
            Class::Variable(_, why) => (
                '~',
                format!("not a secret ({}): a project variable", why.describe()),
            ),
            Class::VariableExists => (
                '=',
                "a project variable already: skipped, never overwritten".to_owned(),
            ),
        };
        io.say(&format!("  {mark} {:<width$}  {what}", entry.name));
        match class {
            Class::New(_) | Class::InRepo(..) | Class::Variable(..) => {}
            Class::Exists | Class::VariableExists => summary.existing.push(entry.name.clone()),
            Class::Invalid(_) => summary.invalid.push(entry.name.clone()),
        }
    }
    for (entry, class) in &mut classes {
        let Class::InRepo(value, found) = class else {
            continue;
        };
        let answer = io
            .ask(&format!(
                "{} {found}: redaction would mask it there. Import it anyway? Type yes: ",
                entry.name
            ))
            .map_err(|_| SecretsImportError::Cancelled)?;
        *class = if answer.trim().eq_ignore_ascii_case("yes") {
            Class::New(value.clone())
        } else {
            io.say(&format!("  skipped {}", entry.name));
            summary.declined.push(entry.name.clone());
            Class::Invalid(found.clone())
        };
    }
    let new = classes
        .iter()
        .filter(|(_, c)| matches!(c, Class::New(_)))
        .count();
    let plain = classes
        .iter()
        .filter(|(_, c)| matches!(c, Class::Variable(..)))
        .count();
    if new == 0 && plain == 0 {
        io.say("Nothing new to import; nothing was sent.");
        return Ok(summary);
    }
    let mut sent = Vec::new();
    if new > 0 || plain == 0 {
        sent.push(counted(new, "new secret"));
    }
    if plain > 0 {
        sent.push(counted(plain, "project variable"));
    }
    io.say(&format!(
        "{} will be sent. Values are not shown.",
        sent.join(" and ")
    ));
    if plain > 0 {
        io.say(
            "Project variables are not secrets: every task of the project has them in its \
             environment, and anyone who sees the project reads them.",
        );
    }
    if !team && new > 0 {
        io.say(if *confirm_hosts {
            "Agents ask for their hosts: a person allows each host and production deploys on the \
             Secrets page."
        } else {
            "Agents set them up: they bind the hosts they need and production deploys get them, \
             without asking (--confirm-hosts to allow each yourself)."
        });
        if *env_files {
            io.say("They start allowed into the project's .env files (--env-files).");
        }
    }
    if *unattended {
        io.say("Sending them without asking: --yes with the operate token made for your agent.");
    } else {
        let answer = io
            .ask("Type yes to import them: ")
            .map_err(|_| SecretsImportError::Cancelled)?;
        if !answer.trim().eq_ignore_ascii_case("yes") {
            return Err(SecretsImportError::Cancelled);
        }
    }

    let mut stopped: Option<String> = None;
    let mut plain_values: Vec<ProjectVariableInput> = Vec::new();
    for (entry, class) in classes {
        let value = match class {
            Class::New(value) => value,
            Class::Variable(value, _) => {
                plain_values.push(ProjectVariableInput {
                    name: entry.name,
                    value: value.expose().to_owned(),
                    description: (!entry.comment.is_empty()).then(|| entry.comment.clone()),
                });
                continue;
            }
            _ => continue,
        };
        if stopped.is_some() {
            summary.unsent.push(entry.name);
            continue;
        }
        let sent = match (team, &sandbox) {
            (true, sandbox) => send_team(client, sandbox.as_deref(), &entry, &value),
            (false, Some(id)) => send_agent(
                client,
                id,
                (instance, *confirm_hosts, *env_files),
                &entry,
                &value,
            ),
            (false, None) => Err(ApiError::NotFound("no sandbox".into())),
        };
        match sent {
            Ok(()) => {
                io.say(&format!("  imported {}", entry.name));
                summary.imported.push(entry.name);
            }
            // One secret refused; a 403 (no right to manage them) would refuse every other.
            Err(
                err @ (ApiError::NotFound(_) | ApiError::Conflict(_) | ApiError::Rejected { .. }),
            ) => {
                io.say(&format!("  failed {}: {err}", entry.name));
                summary.failed.push(entry.name);
            }
            Err(err) => {
                io.say(&format!("  stopped at {}: {err}", entry.name));
                stopped = Some(err.to_string());
                summary.unsent.push(entry.name);
            }
        }
    }
    if !plain_values.is_empty() {
        send_variables(
            client,
            sandbox.as_deref(),
            plain_values,
            stopped.is_some(),
            &mut summary,
            io,
        );
    }
    io.say(&format!(
        "Imported {}{}, skipped {} existing, {} invalid and {} declined, {} failed{}.",
        summary.imported.len(),
        if summary.variables.is_empty() {
            String::new()
        } else {
            format!(", {} as project variables", summary.variables.len())
        },
        summary.existing.len(),
        summary.invalid.len(),
        summary.declined.len(),
        summary.failed.len(),
        if summary.unsent.is_empty() {
            String::new()
        } else {
            format!(", {} not sent", summary.unsent.len())
        }
    ));
    if let Some(file) = source_file {
        io.say(&format!(
            "{file} still holds the values: delete it once you no longer need it."
        ));
    }
    Ok(summary)
}

/// Looks for the new values in the tracked files of the project's checkout (SPEC-274 PLW-003):
/// a value found there is skipped unattended, else asked about. Outside a checkout of `project`
/// nothing is searched, and the run says so.
fn search_repository(
    classes: &mut [(DotenvEntry, Class)],
    project: &str,
    checkout: Option<&Path>,
    unattended: bool,
    io: &mut dyn SecretsIo,
) {
    let Some(root) = checkout else {
        io.say(&format!(
            "Values are not looked for in the repository: run it in a checkout of {project} to \
             find those that would be masked there."
        ));
        return;
    };
    let values: Vec<&str> = classes
        .iter()
        .map(|(_, class)| match class {
            Class::New(value) => value.expose(),
            _ => "",
        })
        .collect();
    let search = match find_in_checkout(root, &values) {
        Ok(search) => search,
        Err(err) => {
            io.say(&format!(
                "Values are not looked for in the repository: {err}"
            ));
            return;
        }
    };
    if search.partial {
        io.say("The repository was looked through only in part (256 MiB of its files).");
    }
    for ((_, class), found) in classes.iter_mut().zip(search.found) {
        let (Class::New(value), Some(found)) = (&*class, found) else {
            continue;
        };
        *class = if unattended {
            Class::Invalid(format!("{}: {IN_REPO}; {BELONGS}", found.describe()))
        } else {
            Class::InRepo(value.clone(), found.describe())
        };
    }
}

/// The sandbox named by `wanted` (id, slug or `owner/name`), else by the checkout's remote.
fn resolve_sandbox(
    client: &VaultClient,
    wanted: Option<&str>,
    checkout: &Path,
) -> Result<SandboxRef, SecretsImportError> {
    let wanted = match wanted.map(str::trim).filter(|w| !w.is_empty()) {
        Some(wanted) => wanted.to_owned(),
        None => {
            let repo = discover_repo(checkout).map_err(|e| {
                SecretsImportError::Sandbox(format!(
                    "pass --sandbox <owner/name> (this directory names no sandbox: {e})"
                ))
            })?;
            format!("{}/{}", repo.owner, repo.name)
        }
    };
    let sandboxes: Vec<SandboxRef> = client.get(&["api", "sandboxes"])?;
    let mut matches = sandboxes.into_iter().filter(|s| {
        s.id == wanted
            || s.slug.eq_ignore_ascii_case(&wanted)
            || s.full_name().eq_ignore_ascii_case(&wanted)
            || (!s.name.is_empty() && s.name.eq_ignore_ascii_case(&wanted))
    });
    match (matches.next(), matches.next()) {
        (Some(found), None) => Ok(found),
        (None, _) => Err(SecretsImportError::Sandbox(format!(
            "no sandbox of yours is {wanted}; pass --sandbox <owner/name>"
        ))),
        (Some(_), Some(_)) => Err(SecretsImportError::Sandbox(format!(
            "several sandboxes match {wanted}; pass its id with --sandbox"
        ))),
    }
}

fn team_value(entry: &DotenvEntry) -> Class {
    let title = entry.name.trim();
    let value = entry.value.expose();
    if cut_key(value) {
        Class::Invalid(CUT_KEY.into())
    } else if title.is_empty() || title.chars().count() > MAX_TITLE_CHARS {
        Class::Invalid(format!(
            "a title of 1–{MAX_TITLE_CHARS} characters is required"
        ))
    } else if value.is_empty() {
        Class::Invalid("the value is empty".into())
    } else if value.len() > MAX_FIELD_BYTES || entry.comment.len() > MAX_FIELD_BYTES {
        Class::Invalid("the value or its comment is too long".into())
    } else if value.contains('\0') {
        Class::Invalid("the value holds a NUL character".into())
    } else {
        Class::New(entry.value.clone())
    }
}

fn agent_value(entry: &DotenvEntry) -> Class {
    if let Err(e) = SecretName::parse(&entry.name) {
        return Class::Invalid(e.to_string());
    }
    let raw = entry.value.expose();
    if cut_key(raw) {
        return Class::Invalid(CUT_KEY.into());
    }
    let value = if raw.contains('\n') {
        // A key file is stored as the Secrets page stores it: its line breaks as literal `\n`.
        match dotenv::agent_value_from_file(raw) {
            Ok(value) => Secret::new(value),
            Err(e) => return Class::Invalid(e.to_string()),
        }
    } else {
        entry.value.clone()
    };
    if let Err(e) = check_agent_value(value.expose()) {
        return Class::Invalid(e.to_string());
    }
    // Redaction would mask it wherever it appears (SPEC-274 PLW-002); the value is never said.
    match ordinary_text(value.expose()) {
        Some(kind) => Class::Invalid(format!(
            "{}: redaction would mask it everywhere it appears; {BELONGS}",
            kind.describe()
        )),
        None => Class::New(value),
    }
}

/// The project variables of a run (SPEC-275 VAR-007), after its secrets, only created, never
/// overwriting one made meanwhile: in one set, or one by one when the set is refused, so a bad
/// entry (a global secret's name, a long description) fails alone. Not sent when the run stopped.
fn send_variables(
    client: &VaultClient,
    sandbox: Option<&str>,
    variables: Vec<ProjectVariableInput>,
    stopped: bool,
    summary: &mut SecretsSummary,
    io: &mut dyn SecretsIo,
) {
    let Some(sandbox) = sandbox.filter(|_| !stopped) else {
        summary.unsent.extend(variables.into_iter().map(|v| v.name));
        return;
    };
    let batches: Vec<Vec<ProjectVariableInput>> = match put_variables(client, sandbox, &variables) {
        Err(ApiError::Conflict(_) | ApiError::Rejected { .. }) if variables.len() > 1 => {
            variables.into_iter().map(|v| vec![v]).collect()
        }
        first => {
            report_variables(first, variables, summary, io);
            return;
        }
    };
    let mut stop: Option<String> = None;
    for batch in batches {
        if stop.is_some() {
            summary.unsent.extend(batch.into_iter().map(|v| v.name));
            continue;
        }
        let sent = put_variables(client, sandbox, &batch);
        if let Err(err) = &sent
            && !matches!(
                err,
                ApiError::NotFound(_) | ApiError::Conflict(_) | ApiError::Rejected { .. }
            )
        {
            stop = Some(err.to_string());
        }
        report_variables(sent, batch, summary, io);
    }
}

fn put_variables(
    client: &VaultClient,
    sandbox: &str,
    variables: &[ProjectVariableInput],
) -> Result<ProjectVariablesSetResult, ApiError> {
    let body = ProjectVariablesSet {
        variables: variables.to_vec(),
        only_new: true,
    };
    client.put(&["api", "sandboxes", sandbox, "variables"], &body)
}

/// Says and counts what one set of variables did.
fn report_variables(
    sent: Result<ProjectVariablesSetResult, ApiError>,
    variables: Vec<ProjectVariableInput>,
    summary: &mut SecretsSummary,
    io: &mut dyn SecretsIo,
) {
    let names: Vec<String> = variables.into_iter().map(|v| v.name).collect();
    match sent {
        Ok(done) => {
            let set: Vec<String> = done.created.into_iter().chain(done.changed).collect();
            if !set.is_empty() {
                io.say(&format!("  set {} as project variables", set.join(", ")));
            }
            for name in &done.existing {
                io.say(&format!(
                    "  = {name}  a project variable already: skipped, never overwritten"
                ));
            }
            summary.variables.extend(set);
            summary.existing.extend(done.existing);
        }
        Err(err @ (ApiError::NotFound(_) | ApiError::Conflict(_) | ApiError::Rejected { .. })) => {
            io.say(&format!("  failed {}: {err}", names.join(", ")));
            summary.failed.extend(names);
        }
        Err(err) => {
            io.say(&format!("  stopped at the project variables: {err}"));
            summary.unsent.extend(names);
        }
    }
}

/// A team secret of `sandbox` (SPEC-216), or team-wide without one.
fn send_team(
    client: &VaultClient,
    sandbox: Option<&str>,
    entry: &DotenvEntry,
    value: &Secret,
) -> Result<(), ApiError> {
    let mut input = TeamSecretInput {
        // Team-wide is the plain route's own default: unsaid, an older server takes it.
        scope: sandbox.map(|_| "sandbox".to_owned()),
        title: entry.name.clone(),
        value: Some(value.expose().to_owned()),
        notes: (!entry.comment.is_empty()).then(|| entry.comment.clone()),
    };
    let sent = match sandbox {
        Some(id) => client.post(&["api", "sandboxes", id, "team-secrets"], &input),
        None => client.post(&["api", "team-secrets"], &input),
    };
    if let Some(value) = input.value.as_mut() {
        value.zeroize();
    }
    sent
}

/// An agent secret of `sandbox`; `(instance, confirm_hosts, env_files)` as the run was asked.
fn send_agent(
    client: &VaultClient,
    sandbox: &str,
    (instance, confirm_hosts, env_files): (bool, bool, bool),
    entry: &DotenvEntry,
    value: &Secret,
) -> Result<(), ApiError> {
    let mut input = AgentSecretInput {
        scope: Some(if instance { "instance" } else { "sandbox" }.to_owned()),
        // Absent: the server's default, agents set it up (SPEC-216).
        auto_allow: confirm_hosts.then_some(false),
        // Absent unless asked for, so older servers take the body (SPEC-256 LOB-004).
        env_files: env_files.then_some(true),
        name: entry.name.clone(),
        description: (!entry.comment.is_empty()).then(|| entry.comment.clone()),
        value: value.expose().to_owned(),
        ..AgentSecretInput::default()
    };
    let sent = client.post(&["api", "sandboxes", sandbox, "agent-secrets"], &input);
    input.value.zeroize();
    sent
}
