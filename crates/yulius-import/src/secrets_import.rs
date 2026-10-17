//! `yulius secrets import` (SPEC-205): team or agent secrets created over the vault API from a
//! dotenv file or hidden prompts, after the person has read the names and typed `yes`.
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
use yulius_domain::vault::names::SecretName;
use yulius_domain::vault::{MAX_FIELD_BYTES, check_agent_value, dotenv};
use zeroize::Zeroize;

use crate::dotenv::{DotenvEntry, MAX_DOTENV_ENTRIES};
use crate::git::discover_repo;
use crate::http::{ApiClient, ApiError, JSON_TIMEOUT, RetryPolicy};
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
        /// `--instance`: they apply to every sandbox.
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
    /// Skipped: one with that name exists.
    pub existing: Vec<String>,
    /// Skipped: refused before sending.
    pub invalid: Vec<String>,
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

    fn get<T: serde::de::DeserializeOwned>(&self, segments: &[&str]) -> Result<T, ApiError> {
        let url = self.api.endpoint(segments)?;
        self.api
            .execute(&|client| client.get(url.clone()).timeout(JSON_TIMEOUT))
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
            let found = resolve_sandbox(client, sandbox.as_deref(), &options.checkout)?;
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
            let reach = if *instance {
                "every sandbox (instance)"
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
    })
}

/// One secret's fate before anything is sent.
enum Class {
    New(Secret),
    Exists,
    Invalid(String),
}

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
    } = target;
    let instance = *instance;
    let classes: Vec<(DotenvEntry, Class)> = entries
        .into_iter()
        .map(|entry| {
            let class = if existing.contains(&entry.name) {
                Class::Exists
            } else if *team {
                team_value(&entry)
            } else {
                agent_value(&entry)
            };
            (entry, class)
        })
        .collect();

    let mut summary = SecretsSummary::default();
    io.say(&format!("Import into {heading} on {}:", client.server()));
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
            Class::Exists => ('=', "exists: skipped, never overwritten".to_owned()),
            Class::Invalid(why) => ('!', format!("skipped: {why}")),
        };
        io.say(&format!("  {mark} {:<width$}  {what}", entry.name));
        match class {
            Class::New(_) => {}
            Class::Exists => summary.existing.push(entry.name.clone()),
            Class::Invalid(_) => summary.invalid.push(entry.name.clone()),
        }
    }
    let new = classes
        .iter()
        .filter(|(_, c)| matches!(c, Class::New(_)))
        .count();
    if new == 0 {
        io.say("Nothing new to import; nothing was sent.");
        return Ok(summary);
    }
    io.say(&format!(
        "{new} new secret{} will be sent. Values are not shown.",
        if new == 1 { "" } else { "s" }
    ));
    if !team {
        io.say(if *confirm_hosts {
            "Agents ask for their hosts: a person allows each host and production deploys on the \
             Secrets page."
        } else {
            "Agents set them up: they bind the hosts they need and production deploys get them, \
             without asking (--confirm-hosts to allow each yourself)."
        });
    }
    let answer = io
        .ask("Type yes to import them: ")
        .map_err(|_| SecretsImportError::Cancelled)?;
    if !answer.trim().eq_ignore_ascii_case("yes") {
        return Err(SecretsImportError::Cancelled);
    }

    let mut stopped: Option<String> = None;
    for (entry, class) in classes {
        let Class::New(value) = class else { continue };
        if stopped.is_some() {
            summary.unsent.push(entry.name);
            continue;
        }
        let sent = match (team, &sandbox) {
            (true, sandbox) => send_team(client, sandbox.as_deref(), &entry, &value),
            (false, Some(id)) => send_agent(client, id, instance, *confirm_hosts, &entry, &value),
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
    io.say(&format!(
        "Imported {}, skipped {} existing and {} invalid, {} failed{}.",
        summary.imported.len(),
        summary.existing.len(),
        summary.invalid.len(),
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
        let normalized = Secret::new(raw.replace("\r\n", "\n"));
        let mut joined = normalized
            .expose()
            .trim_end_matches('\n')
            .replace('\n', "\\n");
        let key = dotenv::key_block(&joined).is_some();
        let value = Secret::new(joined.clone());
        joined.zeroize();
        if !key {
            return Class::Invalid(
                "an agent secret cannot span lines (a PEM or OpenSSH key can)".into(),
            );
        }
        value
    } else {
        entry.value.clone()
    };
    match check_agent_value(value.expose()) {
        Ok(()) => Class::New(value),
        Err(e) => Class::Invalid(e.to_string()),
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

fn send_agent(
    client: &VaultClient,
    sandbox: &str,
    instance: bool,
    confirm_hosts: bool,
    entry: &DotenvEntry,
    value: &Secret,
) -> Result<(), ApiError> {
    let mut input = AgentSecretInput {
        scope: Some(if instance { "instance" } else { "sandbox" }.to_owned()),
        // Absent: the server's default, agents set it up (SPEC-216).
        auto_allow: confirm_hosts.then_some(false),
        name: entry.name.clone(),
        description: (!entry.comment.is_empty()).then(|| entry.comment.clone()),
        value: value.expose().to_owned(),
        ..AgentSecretInput::default()
    };
    let sent = client.post(&["api", "sandboxes", sandbox, "agent-secrets"], &input);
    input.value.zeroize();
    sent
}
