//! The Claude Code sandboxes get, as the running server knows it (SPEC-461, ADR-0293): the
//! channel in effect, which a superadmin changes without a restart; the checks (2 minutes after
//! a start, every hour, at a channel change and on demand, one at a time); what the last check
//! did; and which offered models the version in use knows.
//!
//! Requests for a check are a count the loop watches (`asked`), with the count its last check
//! answered beside it (`answered`): one made while a check runs gets one more check after it,
//! and "checking" is true from a request until the check that answers it ended.
//!
//! The state is also written to `<data>/claude-code/status.json` after every check and channel
//! change, for `yulius doctor`, which opens no database ([`doctor_line`]). The file is derived:
//! the channel a person chose lives in the database.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use yulius_app::ports::agent_cli::{AgentCli, AgentCliStatus, CheckResult, CliSource};
use yulius_app::ports::session::Bind;
use yulius_domain::agent_cli::{
    Channel, NewestFacts, RegistryRead, Version, default_model_fix, default_model_problem,
    gap_text, lacks_default, newest_lacking_default,
};
use yulius_domain::time::Timestamp;

use crate::wiring::claude_cli::{
    BINARY, CHECK_EVERY, ClaudeUpdater, FIRST_CHECK_AFTER, Outcome, agent_cli_binds,
    current_version, describe, host_version, source_for, unique_suffix,
};
use crate::wiring::claude_models::missing_in;

/// The status file's name under `<data>/claude-code`.
pub const STATUS_FILE: &str = "status.json";

/// A release the last check refused.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RefusedRelease {
    /// The release.
    pub version: String,
    /// The offered models it does not know, the default first.
    pub missing: Vec<String>,
}

/// `<data>/claude-code/status.json` (SPEC-461 CCM-006): what the server last knew.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct StatusFile {
    /// The channel in effect (`latest`, `stable`, `off`).
    #[serde(default)]
    pub channel: Option<String>,
    /// It was chosen in Settings, Instance (else it is the server's configuration).
    #[serde(default)]
    pub chosen: bool,
    /// When the registry was last asked (ms since the epoch).
    #[serde(default)]
    pub checked_at_ms: Option<i64>,
    /// npm's `latest` then.
    #[serde(default)]
    pub latest: Option<String>,
    /// npm's `stable` then.
    #[serde(default)]
    pub stable: Option<String>,
    /// What the last check did: `installed`, `up_to_date`, `refused` or `failed`.
    #[serde(default)]
    pub result: Option<String>,
    /// The version it installed or found up to date.
    #[serde(default)]
    pub version: Option<String>,
    /// Why it failed.
    #[serde(default)]
    pub error: Option<String>,
    /// The releases it refused, in the order tried.
    #[serde(default)]
    pub refused: Vec<RefusedRelease>,
    /// The version runs get.
    #[serde(default)]
    pub in_use: Option<String>,
    /// The offered models that version does not know.
    #[serde(default)]
    pub in_use_missing: Vec<String>,
}

impl StatusFile {
    /// The file under `root`, when there is a readable one.
    #[must_use]
    pub fn read(root: &Path) -> Option<Self> {
        let text = std::fs::read(root.join(STATUS_FILE)).ok()?;
        serde_json::from_slice(&text).ok()
    }

    /// Writes it under `root` by an atomic rename.
    fn write(&self, root: &Path) -> std::io::Result<()> {
        std::fs::create_dir_all(root)?;
        let text = serde_json::to_vec_pretty(self).map_err(std::io::Error::other)?;
        let temporary = root.join(format!(".status-{}", unique_suffix()));
        std::fs::write(&temporary, text)?;
        std::fs::rename(&temporary, root.join(STATUS_FILE)).inspect_err(|_| {
            let _ = std::fs::remove_file(&temporary);
        })
    }

    /// What the recorded check showed about npm's releases.
    fn registry_read(&self) -> RegistryRead {
        match self.result.as_deref() {
            Some("installed" | "up_to_date") => RegistryRead::Read,
            Some("refused") => self
                .refused
                .last()
                .and_then(|release| Version::parse(&release.version))
                .map_or(RegistryRead::Unknown, RegistryRead::Refused),
            _ => RegistryRead::Unknown,
        }
    }

    fn result(&self) -> Option<CheckResult> {
        let version = || self.version.as_deref().and_then(Version::parse);
        match self.result.as_deref()? {
            "installed" => version().map(CheckResult::Installed),
            "up_to_date" => version().map(CheckResult::UpToDate),
            "refused" => Some(CheckResult::Refused(
                self.refused
                    .iter()
                    .filter_map(|r| Some((Version::parse(&r.version)?, r.missing.clone())))
                    .collect(),
            )),
            "failed" => Some(CheckResult::Failed(self.error.clone().unwrap_or_default())),
            _ => None,
        }
    }
}

/// What the last check did and saw.
#[derive(Debug, Clone, Default)]
struct Last {
    checked_at_ms: Option<i64>,
    latest: Option<Version>,
    stable: Option<Version>,
    result: Option<CheckResult>,
}

/// The Claude Code sandboxes get, kept current: the [`AgentCli`] of a running server.
pub struct ClaudeCode {
    updater: ClaudeUpdater,
    /// The channel in effect, and whether it was chosen in Settings.
    channel: RwLock<(Channel, bool)>,
    last: Mutex<Last>,
    /// One check at a time.
    one: tokio::sync::Mutex<()>,
    /// A check runs.
    running: AtomicBool,
    /// How many checks were asked for (`check_now`, a channel change). The loop watches the
    /// count, so a request made while a check runs is neither lost nor answered twice.
    asked: tokio::sync::watch::Sender<u64>,
    /// The count the loop's last check answered: while it differs from `asked`, a check is
    /// about to run.
    answered: AtomicU64,
}

impl ClaudeCode {
    /// The service over `updater`, following `channel` (`chosen`: set in Settings). It shows
    /// the last check a status file of an earlier run recorded.
    #[must_use]
    pub fn new(updater: ClaudeUpdater, channel: Channel, chosen: bool) -> Arc<Self> {
        let last = StatusFile::read(updater.root()).map_or_else(Last::default, |file| Last {
            checked_at_ms: file.checked_at_ms,
            latest: file.latest.as_deref().and_then(Version::parse),
            stable: file.stable.as_deref().and_then(Version::parse),
            result: file.result(),
        });
        Arc::new(Self {
            updater,
            channel: RwLock::new((channel, chosen)),
            last: Mutex::new(last),
            one: tokio::sync::Mutex::new(()),
            running: AtomicBool::new(false),
            asked: tokio::sync::watch::Sender::new(0),
            answered: AtomicU64::new(0),
        })
    }

    /// The channel in effect, and whether it was chosen in Settings.
    #[must_use]
    pub fn channel(&self) -> (Channel, bool) {
        self.channel
            .read()
            .map_or((Channel::default(), false), |channel| *channel)
    }

    /// Whether a check runs, or was asked for and is about to.
    #[must_use]
    pub fn is_checking(&self) -> bool {
        self.running.load(Ordering::SeqCst) || self.pending()
    }

    /// Whether a check was asked for that the loop has not answered yet.
    fn pending(&self) -> bool {
        *self.asked.borrow() != self.answered.load(Ordering::SeqCst)
    }

    /// Asks the loop for a check now.
    fn ask(&self) {
        self.asked.send_modify(|asked| *asked += 1);
    }

    /// The read-only bind of the agent CLI for a run starting now, at the host package's path
    /// `host_dir`: as the channel in effect says (`off`: the host's own).
    #[must_use]
    pub fn binds(&self, host_dir: Option<&Path>) -> Vec<Bind> {
        let root = (self.channel().0 != Channel::Off).then(|| self.updater.root());
        agent_cli_binds(host_dir, root)
    }

    /// Checks [`FIRST_CHECK_AFTER`] a start, every [`CHECK_EVERY`] after, and whenever asked
    /// ([`AgentCli::check_now`], a channel change). The loop lives as long as the server.
    pub fn start(self: &Arc<Self>) -> tokio::task::JoinHandle<()> {
        self.start_every(FIRST_CHECK_AFTER, CHECK_EVERY)
    }

    /// [`Self::start`] with other periods (tests).
    pub fn start_every(
        self: &Arc<Self>,
        first: Duration,
        every: Duration,
    ) -> tokio::task::JoinHandle<()> {
        let service = Arc::clone(self);
        // Watched from before the first wait: a request made meanwhile is seen.
        let mut asked = self.asked.subscribe();
        tokio::spawn(async move {
            // What runs get now is known and said before the first check (CCM-005).
            service.report().await;
            let mut wait = first;
            loop {
                if !service.pending() {
                    tokio::select! {
                        () = tokio::time::sleep(wait) => {}
                        // The service holds the sender for as long as this loop holds it.
                        _ = asked.changed() => {}
                    }
                }
                // Each request and the download have their own deadline. A check asked for
                // while one ran (a channel change) is answered by one more check, here.
                loop {
                    let answers = *asked.borrow_and_update();
                    service.check().await;
                    service.answered.store(answers, Ordering::SeqCst);
                    if *asked.borrow() == answers {
                        break;
                    }
                }
                wait = every;
            }
        })
    }

    /// One check of the channel in effect, recorded and written to the status file.
    pub async fn check(&self) -> Option<CheckResult> {
        let _one = self.one.lock().await;
        self.running.store(true, Ordering::SeqCst);
        let (channel, _) = self.channel();
        let checked = self.updater.check_tagged(channel).await;
        let result = match &checked.result {
            Ok(Outcome::Installed(version)) => {
                tracing::info!(%version, "Claude Code updated: new runs use it");
                Some(CheckResult::Installed(*version))
            }
            Ok(Outcome::UpToDate(version)) => Some(CheckResult::UpToDate(*version)),
            // Off, or nothing to keep current on this host: nothing was asked.
            Ok(outcome) => {
                tracing::debug!(?outcome, "Claude Code update check");
                None
            }
            Err(error) if !error.refused().is_empty() => {
                tracing::warn!(%error, "a Claude Code release was refused");
                Some(CheckResult::Refused(
                    error
                        .refused()
                        .iter()
                        .map(|refusal| (refusal.version, refusal.missing.clone()))
                        .collect(),
                ))
            }
            Err(error) => {
                tracing::warn!(%error, "Claude Code was not updated");
                Some(CheckResult::Failed(error.to_string()))
            }
        };
        if let Ok(mut last) = self.last.lock() {
            if result.is_some() {
                last.checked_at_ms = Some(now_ms());
            }
            last.latest = checked.latest.or(last.latest);
            last.stable = checked.stable.or(last.stable);
            last.result.clone_from(&result);
        }
        self.report().await;
        // A check asked for meanwhile follows at once (the loop runs it): until that one
        // ended, the request itself keeps the page saying that it checks.
        self.running.store(false, Ordering::SeqCst);
        result
    }

    /// Reads what the version in use knows, says so when it lacks the default model, and writes
    /// the status file.
    async fn report(&self) {
        let (channel, chosen) = self.channel();
        let version = self
            .updater
            .in_use(channel)
            .and_then(|(version, _)| version);
        let missing = self.updater.missing(channel).await.unwrap_or_default();
        if lacks_default(&missing) {
            let version = version.map_or_else(|| "in use".to_owned(), |v| v.to_string());
            tracing::error!(
                fix = %default_model_fix(channel, None),
                "{}",
                default_model_problem(&version)
            );
        }
        let last = self
            .last
            .lock()
            .map(|last| last.clone())
            .unwrap_or_default();
        let mut file = StatusFile {
            channel: Some(channel.as_str().to_owned()),
            chosen,
            checked_at_ms: last.checked_at_ms,
            latest: last.latest.map(|v| v.to_string()),
            stable: last.stable.map(|v| v.to_string()),
            in_use: version.map(|v| v.to_string()),
            in_use_missing: missing,
            ..StatusFile::default()
        };
        match &last.result {
            Some(CheckResult::Installed(version)) => {
                file.result = Some("installed".into());
                file.version = Some(version.to_string());
            }
            Some(CheckResult::UpToDate(version)) => {
                file.result = Some("up_to_date".into());
                file.version = Some(version.to_string());
            }
            Some(CheckResult::Refused(refused)) => {
                file.result = Some("refused".into());
                file.refused = refused
                    .iter()
                    .map(|(version, missing)| RefusedRelease {
                        version: version.to_string(),
                        missing: missing.clone(),
                    })
                    .collect();
            }
            Some(CheckResult::Failed(error)) => {
                file.result = Some("failed".into());
                file.error = Some(error.clone());
            }
            None => {}
        }
        let root = self.updater.root().to_path_buf();
        let written = tokio::task::spawn_blocking(move || file.write(&root)).await;
        if let Ok(Err(error)) = written {
            tracing::warn!(%error, "the Claude Code status file was not written");
        }
    }
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .and_then(|d| i64::try_from(d.as_millis()).ok())
        .unwrap_or(0)
}

#[async_trait]
impl AgentCli for ClaudeCode {
    async fn status(&self) -> AgentCliStatus {
        let (channel, _) = self.channel();
        let in_use = self.updater.in_use(channel);
        let missing = self.updater.missing(channel).await;
        let last = self
            .last
            .lock()
            .map(|last| last.clone())
            .unwrap_or_default();
        AgentCliStatus {
            unmanaged: self.updater.unmanaged().map(str::to_owned),
            channel,
            version: in_use.and_then(|(version, _)| version),
            source: in_use.map(|(_, host)| {
                if host {
                    CliSource::Host
                } else {
                    CliSource::Yulius
                }
            }),
            host_version: self.updater.host(),
            missing,
            latest: last.latest,
            stable: last.stable,
            checked_at: last.checked_at_ms.map(Timestamp),
            checking: self.is_checking(),
            result: last.result,
        }
    }

    fn check_now(&self) {
        // A check that runs, or is about to, is the check asked for.
        if !self.is_checking() {
            self.ask();
        }
    }

    fn set_channel(&self, channel: Channel, chosen: bool) {
        if let Ok(mut now) = self.channel.write() {
            *now = (channel, chosen);
        }
        tracing::info!(channel = channel.as_str(), chosen, "Claude Code channel");
        // Checked on the new channel at once: a check that runs now read the old one.
        self.ask();
    }

    fn version_for_new_runs(&self) -> Option<Version> {
        self.updater
            .in_use(self.channel().0)
            .and_then(|(version, _)| version)
    }
}

/// How bad the doctor's "Claude Code in sandboxes" line is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Health {
    /// Runs get a Claude Code that knows every offered model.
    Ok,
    /// It lacks an offered model other than the default, or the last release was refused.
    Warn,
    /// It does not know the default model (SPEC-461 CCM-007).
    Fail,
}

/// `yulius doctor`'s "Claude Code in sandboxes" (SPEC-421 CCU-006, SPEC-461 CCM-007): which
/// Claude Code isolated runs get, where it comes from, and whether it knows the models Yulius
/// offers; with the fix when it does not know the default model. The channel is the one the
/// server recorded in its status file (chosen in Settings, or its configuration), else
/// `configured`. Reads the binary runs get (about 340 MB).
#[must_use]
pub fn doctor_line(root: &Path, host_dir: Option<&Path>, configured: Channel) -> (Health, String) {
    let file = StatusFile::read(root);
    let recorded = file
        .as_ref()
        .and_then(|file| Some((Channel::parse(file.channel.as_deref()?)?, file.chosen)));
    let (channel, chosen) = recorded.unwrap_or((configured, false));
    let base = describe(root, host_dir, channel, chosen);
    let Some(host_dir) = host_dir else {
        return (Health::Ok, base);
    };
    let source: PathBuf = source_for(channel, root, host_dir);
    let Some(missing) = missing_in(&source.join(BINARY)) else {
        return (
            Health::Warn,
            format!("{base}; its binary could not be read for the models it knows"),
        );
    };
    let version = if source == host_dir {
        host_version(host_dir)
    } else {
        current_version(root)
    };
    let version = version.map_or_else(|| "of an unknown version".to_owned(), |v| v.to_string());
    let version = version.as_str();
    let runs = Version::parse(version);
    // The releases the server's last check refused that are newer than what runs get now (an
    // install on the host since then makes an older refusal history).
    let refused: Vec<&RefusedRelease> = file
        .iter()
        .filter(|file| file.result.as_deref() == Some("refused"))
        .flat_map(|file| &file.refused)
        .filter(|release| Version::parse(&release.version) > runs)
        .collect();
    if lacks_default(&missing) {
        // Whether npm's newest release does not know the default model either: only a fresh
        // read of the registry by the server says so (`newest_lacking_default`).
        let newest = file
            .as_ref()
            .and_then(|file| {
                newest_lacking_default(&NewestFacts {
                    channel,
                    read: file.registry_read(),
                    read_ms_ago: file.checked_at_ms.map(|at| now_ms().saturating_sub(at)),
                    latest: file.latest.as_deref().and_then(Version::parse),
                    runs,
                })
            })
            .map(|newest| newest.to_string());
        return (
            Health::Fail,
            format!(
                "{base}. {}. Fix: {}.",
                default_model_problem(version),
                default_model_fix(channel, newest.as_deref())
            ),
        );
    }
    let mut notes = Vec::new();
    if !missing.is_empty() {
        notes.push(format!(
            "{}, which the composer offers",
            gap_text(version, &missing)
        ));
    }
    for release in &refused {
        notes.push(format!(
            "the last check refused a release: {}",
            gap_text(&release.version, &release.missing)
        ));
    }
    if notes.is_empty() {
        (Health::Ok, base)
    } else {
        (Health::Warn, format!("{base}; {}", notes.join("; ")))
    }
}
