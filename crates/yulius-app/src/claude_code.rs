//! Settings → Instance, "Claude Code" (SPEC-461, ADR-0293): which Claude Code sandboxes get and
//! whether it knows the models Yulius offers, the release channel and the checks.
//!
//! The channel a superadmin chooses is stored in the `instance` row and wins over the server's
//! configuration (`YULIUS_CLAUDE_UPDATES`); the adapter ([`AgentCli`]) follows it at once: the
//! next run binds as it says and a check runs now. Without an adapter (sandboxes are not
//! isolated here) runs use the host's Claude Code as it is and nothing is kept current.

use std::collections::HashMap;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};

use yulius_contracts::api::updates::{ClaudeCodeBehind, ClaudeCodeRefused, ClaudeCodeView};
use yulius_domain::agent_cli::{
    Channel, MoveFacts, Version, default_model_fix, default_model_problem, gap_text, lacks_default,
    moves_now,
};
use yulius_domain::harness::DEFAULT_CLAUDE_MODEL;
use yulius_domain::ids::{AccountId, AttemptId, TaskId};
use yulius_domain::task::{Activity, StopReason, TaskStatus};
use yulius_domain::time::Timestamp;

use crate::auth::{AuthContext, rfc3339};
use crate::context::App;
use crate::error::{AppError, AppResult};
use crate::model::TaskMode;
use crate::ports::agent_cli::{AgentCli, AgentCliStatus, CheckResult, CliSource};

/// The audit target of channel changes (`claude_code.channel`).
pub const CLAUDE_CODE_TARGET: &str = "instance:claude-code";

/// Why nothing is kept current on a server that does not isolate sandboxes.
pub const NOT_ISOLATED: &str = "sandboxes are not isolated on this server, so they run the \
    host's Claude Code as it is and Yulius keeps nothing current: run Yulius contained \
    (YULIUS_CONTAINED=true in yulius.service) with the isolation prerequisites, which `sudo \
    yulius host setup` installs";

/// The switch reason of a session restarted on a newer Claude Code (SPEC-461 CCM-013): the
/// subscription stays, so it is no subscription switch in a task's info.
pub const MOVE_REASON: &str = "claude_code";

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
    /// The version each task's session was last sent to: once per version per task, so a
    /// session that still names the old one afterwards is not restarted at every turn's end.
    moved: Mutex<HashMap<TaskId, Version>>,
    /// A version a session could not continue on, and what happened: no other session is sent
    /// to it (a newer version, or a restart of the server, lifts it).
    stopped: Mutex<Option<(Version, String)>>,
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
                moved: Mutex::default(),
                stopped: Mutex::default(),
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
        let mut view = view_of(&status, services.config_channel, chosen.as_ref());
        view.behind = self.tasks_behind(status.version).await?;
        view.moves_stopped = services.stopped.lock().ok().and_then(|stopped| {
            stopped.as_ref().map(|(version, why)| {
                format!("Running tasks are no longer moved to {version}: {why}.")
            })
        });
        Ok(view)
    }

    /// The tasks whose running session names an older Claude Code than `newest`, the one runs
    /// get now (SPEC-461 CCM-013): they move when their turn ends, or at their next start.
    async fn tasks_behind(&self, newest: Option<Version>) -> AppResult<Vec<ClaudeCodeBehind>> {
        let Some(newest) = newest else {
            return Ok(Vec::new());
        };
        let store = &self.deps.store;
        let mut behind = Vec::new();
        for (task_id, version) in store.live_cli_versions().await? {
            if Version::parse(&version).is_none_or(|runs| runs >= newest) {
                continue;
            }
            let Some(task) = store.task(&task_id).await? else {
                continue;
            };
            let Some(sandbox) = store.sandbox(&task.sandbox_id).await? else {
                continue;
            };
            behind.push(ClaudeCodeBehind {
                sandbox: sandbox.slug,
                number: task.number,
                title: task.title,
                version,
            });
        }
        Ok(behind)
    }

    /// The Claude Code version `attempt`'s transcript names, when it names a plain one.
    async fn run_cli_version(&self, task_id: &TaskId, attempt: &AttemptId) -> Option<Version> {
        let facts = self.deps.store.task_run_facts(task_id).await.ok()?;
        let named = facts.into_iter().find(|facts| facts.attempt == *attempt)?;
        Version::parse(named.cli_version.as_deref()?)
    }

    /// The newer Claude Code the session of `attempt` moves to now, at its turn's end (SPEC-461
    /// CCM-013); `None` when it stays: it runs the version runs get (or names none), it was
    /// sent to that version before, a session could not continue on that version, a person
    /// typed into it in the last minute, a switch was refused moments ago, or it has background
    /// shells, agents or monitors, which a restart would end.
    pub(crate) async fn claude_code_move_due(
        self: &Arc<Self>,
        task_id: &TaskId,
        attempt: &AttemptId,
    ) -> Option<Version> {
        let services = self.agent_cli.get()?;
        let target = services.adapter.version_for_new_runs()?;
        let runs = match self.run_cli_version(task_id, attempt).await {
            Some(runs) => runs,
            // The lines of this turn name it: they are read first.
            None => {
                self.await_transcript_read(task_id).await;
                self.run_cli_version(task_id, attempt).await?
            }
        };
        let typed = self.runtime(task_id).typed_at_ms.load(Ordering::SeqCst);
        let mut facts = MoveFacts {
            runs,
            target,
            sent_to: services.moved.lock().ok()?.get(task_id).copied(),
            stopped_for: services
                .stopped
                .lock()
                .ok()?
                .as_ref()
                .map(|(version, _)| *version),
            typed_ms_ago: (typed > 0).then(|| self.now().0.saturating_sub(typed)),
            switch_refused: self.switch_cooling(task_id),
            // Asked last: it reads the turn's lines and probes the session's processes.
            background: Some(false),
        };
        if !moves_now(&facts) {
            return None;
        }
        // Background work started in this turn is known from its lines (SPEC-079 BGW-004).
        self.await_transcript_read(task_id).await;
        facts.background = self.background_work_now(task_id).await;
        moves_now(&facts).then_some(target)
    }

    /// Restarts the session of `task_id` on Claude Code `version` in the same Claude session, on
    /// its subscription `account` (SPEC-461 CCM-013): the switch a subscription move makes, with
    /// `prompt` typed into the resumed session. A version a session could not continue on gets
    /// no further sessions.
    pub(crate) async fn move_to_claude_code(
        self: &Arc<Self>,
        task_id: &TaskId,
        account: Option<AccountId>,
        version: Version,
        prompt: Option<String>,
    ) {
        let Some(services) = self.agent_cli.get() else {
            return;
        };
        self.runtime(task_id).hub.notice(format!(
            "↻ Claude Code {version} is installed: this session restarts on it and continues…"
        ));
        let Err(err) = self
            .switch_task_now(task_id, account, MOVE_REASON, prompt)
            .await
        else {
            // Sent once: a session that still names the old version is not restarted again.
            if let Ok(mut moved) = services.moved.lock() {
                moved.insert(task_id.clone(), version);
            }
            return;
        };
        tracing::warn!(error = %err, task = %task_id, %version, "the session did not move to the new Claude Code");
        // The old run ended and the new one did not start: the version is not given another
        // session. A refusal before the old run ended, or a stop that overtook the move, says
        // nothing about the version.
        if let Ok(Some(task)) = self.deps.store.task(task_id).await
            && task.status == TaskStatus::Stopped(StopReason::SwitchFailed)
            && let Ok(mut stopped) = services.stopped.lock()
        {
            *stopped = Some((
                version,
                format!(
                    "task #{} could not continue on it ({})",
                    task.number,
                    err.public_message()
                ),
            ));
        }
    }

    /// An idle prompt-mode session whose turn just settled restarts on the Claude Code runs get
    /// now, with nothing typed (SPEC-461 CCM-013). A goal session is not restarted idle: its
    /// goal is typed again into a resumed session, which would start work nobody asked for; it
    /// moves at a boundary where it works on anyway.
    pub(crate) async fn move_idle_to_newer_claude_code(self: &Arc<Self>, task_id: &TaskId) {
        if self.agent_cli.get().is_none() {
            return;
        }
        // A session a person paused, or one an instance update is about to freeze, is left as
        // it is: the restart would end the pause (SPEC-191 PER-005) or outrun the update.
        if self.is_paused_by_person(task_id).await || self.update_holds_starts() {
            return;
        }
        let store = &self.deps.store;
        let idle = |task: &crate::model::TaskRecord| {
            task.mode == TaskMode::Prompt
                && task.status == TaskStatus::Running
                && task.activity == Some(Activity::Waiting)
        };
        if !matches!(store.task(task_id).await, Ok(Some(task)) if idle(&task)) {
            return;
        }
        let Ok(Some(attempt)) = store.live_attempt(task_id).await else {
            return;
        };
        let Some(version) = self.claude_code_move_due(task_id, &attempt.id).await else {
            return;
        };
        // Still idle: no prompt arrived while this looked.
        if !matches!(store.task(task_id).await, Ok(Some(task)) if idle(&task)) {
            return;
        }
        self.move_to_claude_code(task_id, attempt.account_id, version, None)
            .await;
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
        behind: Vec::new(),
        moves_stopped: None,
    }
}
