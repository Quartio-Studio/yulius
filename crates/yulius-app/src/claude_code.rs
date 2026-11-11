//! Settings → Instance, "Claude Code" (SPEC-461, ADR-0293): which Claude Code sandboxes get and
//! whether it knows the models Yulius offers, the release channel and the checks.
//!
//! The channel a superadmin chooses is stored in the `instance` row and wins over the server's
//! configuration (`YULIUS_CLAUDE_UPDATES`); the adapter ([`AgentCli`]) follows it at once: the
//! next run binds as it says and a check runs now. Without an adapter (sandboxes are not
//! isolated here) runs use the host's Claude Code as it is and nothing is kept current.

use std::sync::Arc;

use yulius_contracts::api::updates::{ClaudeCodeRefused, ClaudeCodeView};
use yulius_domain::agent_cli::{
    Channel, Version, default_model_fix, default_model_problem, gap_text, lacks_default,
};
use yulius_domain::harness::DEFAULT_CLAUDE_MODEL;
use yulius_domain::time::Timestamp;

use crate::auth::{AuthContext, rfc3339};
use crate::context::App;
use crate::error::{AppError, AppResult};
use crate::ports::agent_cli::{AgentCli, AgentCliStatus, CheckResult, CliSource};

/// The audit target of channel changes (`claude_code.channel`).
pub const CLAUDE_CODE_TARGET: &str = "instance:claude-code";

/// Why nothing is kept current on a server that does not isolate sandboxes.
pub const NOT_ISOLATED: &str = "sandboxes are not isolated on this server, so they run the \
    host's Claude Code as it is and Yulius keeps nothing current: run Yulius contained \
    (YULIUS_CONTAINED=true in yulius.service) with the isolation prerequisites, which `sudo \
    yulius host setup` installs";

/// The channel a superadmin chose in Settings, as the store keeps it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChosenChannel {
    /// `latest`, `stable` or `off`.
    pub channel: String,
    /// When it was chosen.
    pub set_at: Timestamp,
    /// Who chose it (login).
    pub set_by: Option<String>,
}

/// The Claude Code adapter of an instance and the channel its configuration names.
pub struct AgentCliServices {
    adapter: Arc<dyn AgentCli>,
    /// `YULIUS_CLAUDE_UPDATES`: followed while nothing is chosen in Settings.
    config_channel: Channel,
    /// Serializes channel changes, so the stored channel and the adapter's agree.
    changing: tokio::sync::Mutex<()>,
}

/// The sentence about what the last check did.
fn result_text(result: &CheckResult) -> (&'static str, String) {
    match result {
        CheckResult::Installed(version) => (
            "installed",
            format!("Installed {version}: new runs use it."),
        ),
        CheckResult::UpToDate(version) => {
            ("up_to_date", format!("Up to date: runs get {version}."))
        }
        CheckResult::Refused(refused) => {
            let named: Vec<String> = refused
                .iter()
                .map(|(version, missing)| gap_text(&version.to_string(), missing))
                .collect();
            (
                "refused",
                format!("Refused, nothing was installed: {}.", named.join("; ")),
            )
        }
        CheckResult::Failed(why) => ("failed", format!("The check failed: {why}.")),
    }
}

impl App {
    /// The channel the server starts with: the one chosen in Settings (`true`), else `config`
    /// (SPEC-461 CCM-009). A stored channel that cannot be read is logged and `config` followed.
    pub async fn claude_channel_at_start(&self, config: Channel) -> (Channel, bool) {
        match self.deps.store.claude_channel().await {
            Ok(Some(chosen)) => match Channel::parse(&chosen.channel) {
                Some(channel) => (channel, true),
                None => {
                    tracing::warn!(
                        channel = %chosen.channel,
                        "the stored Claude Code channel is unknown: following the configuration"
                    );
                    (config, false)
                }
            },
            Ok(None) => (config, false),
            Err(error) => {
                tracing::warn!(%error, "could not read the stored Claude Code channel");
                (config, false)
            }
        }
    }

    /// Installs the Claude Code adapter (once) with the channel of the server's configuration.
    /// Returns whether it was set.
    pub fn set_agent_cli(&self, adapter: Arc<dyn AgentCli>, config_channel: Channel) -> bool {
        self.agent_cli
            .set(AgentCliServices {
                adapter,
                config_channel,
                changing: tokio::sync::Mutex::new(()),
            })
            .is_ok()
    }

    fn agent_cli_services(&self) -> AppResult<&AgentCliServices> {
        self.agent_cli
            .get()
            .ok_or_else(|| AppError::Conflict(NOT_ISOLATED.into()))
    }

    /// The Claude Code version a run starting now gets; `None` when unknown, or without an
    /// adapter.
    #[must_use]
    pub fn claude_code_for_new_runs(&self) -> Option<Version> {
        self.agent_cli.get()?.adapter.version_for_new_runs()
    }

    /// `GET /api/admin/claude-code` (superadmin).
    ///
    /// # Errors
    /// `Forbidden` for others; store failures.
    pub async fn claude_code_view(&self, ctx: &AuthContext) -> AppResult<ClaudeCodeView> {
        self.require_superadmin(ctx)?;
        self.claude_code_state().await
    }

    async fn claude_code_state(&self) -> AppResult<ClaudeCodeView> {
        let Some(services) = self.agent_cli.get() else {
            let default = Channel::default().as_str().to_owned();
            return Ok(ClaudeCodeView {
                managed: false,
                unmanaged: Some(NOT_ISOLATED.into()),
                channel: default.clone(),
                channel_source: "config".into(),
                config_channel: default,
                default_model: DEFAULT_CLAUDE_MODEL.into(),
                ..ClaudeCodeView::default()
            });
        };
        let status = services.adapter.status().await;
        let chosen = self.deps.store.claude_channel().await?;
        Ok(view_of(&status, services.config_channel, chosen.as_ref()))
    }

    /// `PUT /api/admin/claude-code/channel` (superadmin, SPEC-461 CCM-009): follows `channel`
    /// (`latest`, `stable` or `off`) from now on, stored; `None` goes back to the server's
    /// configuration. Applies at once: the next run binds as it says and a check runs now.
    ///
    /// # Errors
    /// `Forbidden` for others, `Invalid` for an unknown channel, `Conflict` without an adapter.
    pub async fn set_claude_channel(
        &self,
        ctx: &AuthContext,
        channel: Option<&str>,
    ) -> AppResult<ClaudeCodeView> {
        self.require_superadmin(ctx)?;
        let services = self.agent_cli_services()?;
        let chosen = match channel.map(str::trim) {
            None => None,
            Some(text) => Some(Channel::parse(text).ok_or_else(|| {
                AppError::Invalid("the channel must be latest, stable or off".into())
            })?),
        };
        let _changing = services.changing.lock().await;
        let stored = self
            .deps
            .store
            .claude_channel()
            .await?
            .and_then(|stored| Channel::parse(&stored.channel));
        if stored != chosen {
            let login = self.login_of(&ctx.user.id).await;
            let now = self.now();
            self.deps
                .store
                .set_claude_channel(chosen.map(|c| (c.as_str(), login.as_str())), now)
                .await?;
            services
                .adapter
                .set_channel(chosen.unwrap_or(services.config_channel), chosen.is_some());
            let detail = chosen.map_or("configuration", Channel::as_str);
            self.deps
                .store
                .audit(
                    now,
                    &login,
                    "claude_code.channel",
                    Some(CLAUDE_CODE_TARGET),
                    Some(detail),
                )
                .await?;
        }
        self.claude_code_state().await
    }

    /// `POST /api/admin/claude-code/check` (superadmin, SPEC-461 CCM-010): starts a check now;
    /// one that runs already is the check. The view says it runs.
    ///
    /// # Errors
    /// `Forbidden` for others, `Conflict` without an adapter.
    pub async fn check_claude_code_now(&self, ctx: &AuthContext) -> AppResult<ClaudeCodeView> {
        self.require_superadmin(ctx)?;
        self.agent_cli_services()?.adapter.check_now();
        self.claude_code_state().await
    }
}

/// The view of `status`, with the channel's origin.
fn view_of(
    status: &AgentCliStatus,
    config_channel: Channel,
    chosen: Option<&ChosenChannel>,
) -> ClaudeCodeView {
    let version = status.version.map(|v| v.to_string());
    let (result, result_text) = status
        .result
        .as_ref()
        .map(result_text)
        .map_or((None, None), |(kind, text)| {
            (Some(kind.to_owned()), Some(text))
        });
    let refused: Vec<ClaudeCodeRefused> = match &status.result {
        Some(CheckResult::Refused(refused)) => refused
            .iter()
            .map(|(version, missing)| ClaudeCodeRefused {
                version: version.to_string(),
                missing: missing.clone(),
            })
            .collect(),
        _ => Vec::new(),
    };
    // The newest release itself was refused: no release knows the default model yet.
    let newest_refused = match &status.result {
        Some(CheckResult::Refused(refused)) => refused
            .last()
            .filter(|(version, _)| Some(*version) == status.latest)
            .map(|(version, _)| version.to_string()),
        _ => None,
    };
    let unknown = status.missing.clone().unwrap_or_default();
    let (problem, fix) = if lacks_default(&unknown) {
        let named = version.as_deref().unwrap_or("in use");
        (
            Some(format!("{}.", default_model_problem(named))),
            Some(format!(
                "{}.",
                default_model_fix(status.channel, newest_refused.as_deref())
            )),
        )
    } else {
        (None, None)
    };
    ClaudeCodeView {
        managed: status.unmanaged.is_none(),
        unmanaged: status.unmanaged.clone(),
        channel: status.channel.as_str().to_owned(),
        channel_source: if chosen.is_some() {
            "settings"
        } else {
            "config"
        }
        .to_owned(),
        config_channel: config_channel.as_str().to_owned(),
        channel_set_by: chosen.and_then(|c| c.set_by.clone()),
        channel_set_at: chosen.map(|c| rfc3339(c.set_at)),
        version,
        source: status.source.map(|source| {
            match source {
                CliSource::Yulius => "yulius",
                CliSource::Host => "host",
            }
            .to_owned()
        }),
        host_version: status.host_version.map(|v| v.to_string()),
        latest: status.latest.map(|v| v.to_string()),
        stable: status.stable.map(|v| v.to_string()),
        checked_at: status.checked_at.map(rfc3339),
        checking: status.checking,
        result,
        result_text,
        default_model: DEFAULT_CLAUDE_MODEL.into(),
        models_read: status.missing.is_some(),
        unknown_models: unknown,
        problem,
        fix,
        refused,
    }
}
