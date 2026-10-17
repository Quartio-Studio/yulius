//! The Cockpit: every user's permanent Claude Code session in a sandbox, for quick questions and
//! work that should not become a task (SPEC-021, ADR-0020).
//!
//! Contract:
//! - one Cockpit per user and sandbox, for users with write access; [`App::cockpit_open`] returns
//!   the running one or starts it, and concurrent calls start one (CKP-001, CKP-005);
//! - it is plain Claude Code: no Yulius hooks, settings, briefing or gates, the token of a pool
//!   subscription in its environment, its config in the user's own home, run in the sandbox
//!   repository as the user's sandbox identity (CKP-002);
//! - it manages the project through the Yulius MCP server with its own bearer, served only the
//!   management tools and acting for its owner, checked at every call (SPEC-037 MGT-004..007);
//! - a Cockpit started again resumes the user's newest conversation there, so `/clear`,
//!   `/exit`, an idle stop and a lost process never lose it (CKP-003);
//! - it is a shell of kind [`ShellKind::Cockpit`]: private, adopted after a restart and ended
//!   after a day without viewers like a terminal, but never listed, renamed or closed as one
//!   (CKP-004, CKP-006);
//! - one adopted from a Yulius that served it something else (an update) starts again once its
//!   conversation is idle, resuming it, so it gets the new tools and environment (SPEC-186
//!   SAV-006).

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use serde_json::json;

use yulius_domain::harness::{Harness, Provider};
use yulius_domain::ids::{SandboxId, UserId};
use yulius_domain::permission::Action;
use yulius_domain::time::Timestamp;
use yulius_domain::usage::ModelFamily;

use crate::auth::AuthContext;
use crate::context::App;
use crate::coordination::ProjectCaller;
use crate::error::{AppError, AppResult};
use crate::model::{AccountRecord, SandboxRecord, UserRecord};
use crate::ports::harness::{CockpitLaunch, Observation, TranscriptWatch, WatchFrom};
use crate::shells::{COCKPIT_NOT_MOVED, COCKPIT_TOKEN_PREFIX, ShellInfo, ShellKind, ShellRuntime};

/// The Cockpit's display name.
pub const COCKPIT_NAME: &str = "Cockpit";
/// A resumed Cockpit that fails within this time could not resume its conversation.
const QUICK_EXIT_MS: i64 = 10_000;
/// How often a running Cockpit's conversation is read for usage limits (SPEC-074 SUP-002).
const WATCH_INTERVAL: Duration = Duration::from_secs(1);
/// What a Cockpit's viewers are told when a usage limit cut its turn and no other subscription
/// has capacity (SPEC-074 SUP-005).
pub const NO_CAPACITY_NOTICE: &str = "Usage limit reached. No other subscription has capacity right now, so Claude Code continues when the limit resets.";
/// What a Cockpit's viewers are told when it starts again to get what an updated Yulius serves
/// (SPEC-186 SAV-006).
pub const REFRESH_NOTICE: &str = "Yulius was updated: the Superagent restarts to get its new tools, and its conversation continues.";
/// Bump when a Cockpit's launch changes in a way a running one must get (its environment,
/// settings or the tools it runs without asking): running ones then start again (SPEC-186
/// SAV-006). Its MCP instructions and tools are part of the digest by themselves.
/// 2: contained ones bypass approvals (SPEC-208).
const COCKPIT_LAUNCH_REVISION: u32 = 2;
/// An adopted Cockpit that needs no new start.
pub(crate) const REFRESH_NONE: u8 = 0;
/// An adopted Cockpit that starts again once its conversation is idle.
pub(crate) const REFRESH_DUE: u8 = 1;
/// [`REFRESH_DUE`], and its conversation is in a turn: it waits for the turn to end.
pub(crate) const REFRESH_WAITING: u8 = 2;
/// How long a refresh that could not start (no capacity, access lost) waits before it is tried
/// again.
const REFRESH_RETRY_MS: i64 = 60_000;

/// How a refresh went ([`App::refresh_cockpit`]).
enum Refresh {
    /// The Cockpit has ended (started again, or it had ended already).
    Ended,
    /// Its conversation was written to since it was found idle: it stays, and is looked at again.
    Busy,
    /// It could not start again now: it stays, and is tried again later.
    Failed,
}

/// Whether a Cockpit's conversation is idle, from its transcript: no turn open and no background
/// work (a Monitor stream, a background agent) Claude Code waits for (SPEC-186 SAV-006).
#[derive(Default)]
struct TurnState {
    open: bool,
    /// Background work by id, with when a Monitor expires (ms; `None`: until it ends).
    background: HashMap<String, Option<i64>>,
}

/// The length of a conversation's transcript (0 when it cannot be read).
fn transcript_len(path: &std::path::Path) -> u64 {
    std::fs::metadata(path).map_or(0, |meta| meta.len())
}

/// Whether the conversation found idle with transcript `seen` (path and length) still is: nothing
/// was written to it since, such as a prompt sent meanwhile (review round 1). No conversation:
/// nothing to cut.
fn still_idle(seen: Option<(&std::path::Path, u64)>) -> bool {
    seen.is_none_or(|(path, len)| transcript_len(path) == len)
}

impl TurnState {
    fn observe(&mut self, observation: &Observation, now: Timestamp) {
        match observation {
            Observation::UserTurn { .. } | Observation::AgentWork { .. } => self.open = true,
            // As the task reducer reads them (`finish.rs`): a full context ends a turn (#213).
            Observation::TurnComplete { at: Some(_) }
            | Observation::TurnInterrupted { .. }
            | Observation::TurnEnded { .. }
            | Observation::ContextFull => self.open = false,
            Observation::BackgroundStarted {
                id,
                expires_in_ms,
                at,
                ..
            } => {
                let started = at.unwrap_or(now).0;
                let expires = expires_in_ms
                    .map(|ms| started.saturating_add(i64::try_from(ms).unwrap_or(i64::MAX)));
                self.background.insert(id.clone(), expires);
            }
            Observation::BackgroundEnded { id } => {
                self.background.remove(id);
            }
            _ => {}
        }
    }

    fn idle(&mut self, now: Timestamp) -> bool {
        self.background
            .retain(|_, expires| expires.is_none_or(|at| at > now.0));
        !self.open && self.background.is_empty()
    }
}

/// The model family the Cockpit runs: the default model, Opus 5.5, so its weekly Opus cap binds
/// (SPEC-023).
fn cockpit_family() -> ModelFamily {
    ModelFamily::of(Some(yulius_domain::harness::DEFAULT_CLAUDE_MODEL))
}

impl App {
    /// Configures the Cockpit's program instead of `claude` (tests); the first call wins.
    /// Returns whether it was applied.
    pub fn set_cockpit_program(&self, program: PathBuf) -> bool {
        self.shells.cockpit_program.set(program).is_ok()
    }

    /// Remembers a conversation a Cockpit could not resume: it exited with an error right after
    /// it started, so starting it the same way again would fail again.
    pub(crate) fn note_cockpit_exit(&self, runtime: &ShellRuntime, code: Option<i32>) {
        let Some((resumed, spawned)) = runtime.resumed.get() else {
            return;
        };
        // An error exit of Claude Code itself; a signal (the host ended) says nothing about it.
        let failed = code.is_some_and(|code| code != 0);
        let quick = self.now().millis_since(*spawned) < QUICK_EXIT_MS;
        if runtime.kind != ShellKind::Cockpit || !failed || !quick {
            return;
        }
        tracing::warn!(shell = %runtime.id(), conversation = %resumed, ?code, "the cockpit could not resume its conversation; the next start begins a fresh one");
        if let Ok(mut failed) = self.shells.cockpit_unresumable.lock() {
            failed.insert(
                (runtime.creator().clone(), runtime.sandbox().clone()),
                resumed.clone(),
            );
        }
    }

    /// How many (user, sandbox) pairs have a Cockpit start running or waiting (tests,
    /// diagnostics).
    #[must_use]
    pub fn cockpit_starts_pending(&self) -> usize {
        self.shells
            .cockpit_start
            .lock()
            .map_or(0, |starts| starts.len())
    }

    /// What a Cockpit this Yulius starts is served and started with, as a digest: its MCP
    /// instructions and tools, whether the egress proxy is wired and
    /// [`COCKPIT_LAUNCH_REVISION`] (SPEC-186 SAV-006).
    pub(crate) fn cockpit_launch_digest(&self) -> String {
        let view = json!({
            "revision": COCKPIT_LAUNCH_REVISION,
            // Started without the egress proxy, it gets it once the proxy is wired.
            "egress": self.egress_wired(),
            "instructions": crate::mcp::COCKPIT_INSTRUCTIONS,
            "tools": crate::mcp::cockpit_tool_descriptors(),
        });
        self.deps.crypto.sha256_hex(view.to_string().as_bytes())
    }

    /// Whether the live Cockpit `shell` is to start again to get what this Yulius serves:
    /// `none`, `due`, or `waiting` for its turn to end (tests, diagnostics; SPEC-186 SAV-006).
    #[must_use]
    pub fn cockpit_refresh_state(&self, shell: &str) -> &'static str {
        match self.shell(shell).map(|s| s.refresh.load(Ordering::SeqCst)) {
            Some(REFRESH_DUE) => "due",
            Some(REFRESH_WAITING) => "waiting",
            _ => "none",
        }
    }

    /// The Yulius MCP bearer of `user`'s running Cockpit in `sandbox` (SPEC-037 MGT-005).
    #[must_use]
    pub fn cockpit_mcp_token(&self, user: &UserId, sandbox: &SandboxId) -> Option<String> {
        self.running_cockpit(user, sandbox)
            .map(|cockpit| cockpit.cockpit_token(self.deps.crypto.as_ref()))
    }

    /// Resolves a Cockpit's bearer to a caller of the management tools acting for its owner
    /// (SPEC-037 MGT-005, MGT-007): the Cockpit must be live, its owner enabled and able to work
    /// in its project, checked at every call.
    ///
    /// # Errors
    /// `Unauthenticated` otherwise.
    pub async fn cockpit_caller(&self, token: &str) -> AppResult<ProjectCaller> {
        if !token.starts_with(COCKPIT_TOKEN_PREFIX) {
            return Err(AppError::Unauthenticated);
        }
        let crypto = self.deps.crypto.as_ref();
        // Compared as hashes, so the time taken says nothing about a token's characters.
        let wanted = crypto.sha256_hex(token.as_bytes());
        let cockpit = self
            .live_cockpits()
            .into_iter()
            .find(|cockpit| crypto.sha256_hex(cockpit.cockpit_token(crypto).as_bytes()) == wanted)
            .ok_or(AppError::Unauthenticated)?;
        let user = self
            .deps
            .store
            .user(cockpit.creator())
            .await?
            .filter(|u| u.disabled_at.is_none())
            .ok_or(AppError::Unauthenticated)?;
        self.require(&user, cockpit.sandbox(), Action::Work)
            .await
            .map_err(|_| AppError::Unauthenticated)?;
        let login = self.login_of(&user.id).await;
        Ok(ProjectCaller {
            sandbox: cockpit.sandbox().clone(),
            user: user.id,
            task: None,
            actor: format!("cockpit:{login}"),
            detail: json!({"via": "cockpit", "shell": cockpit.id()}),
        })
    }

    /// The caller's running Cockpit in `sandbox`.
    fn running_cockpit(&self, user: &UserId, sandbox: &SandboxId) -> Option<Arc<ShellRuntime>> {
        self.sandbox_shells(sandbox)
            .into_iter()
            .find(|shell| shell.kind == ShellKind::Cockpit && shell.creator() == user)
    }

    /// `POST /api/sandboxes/{sid}/cockpit`: the caller's Cockpit, started (resuming the newest
    /// conversation) when it is not running.
    ///
    /// # Errors
    /// `NotFound`/`Forbidden` without write access, `Conflict` before the repository is cloned
    /// or without a Claude subscription, `Capacity` while none has capacity, `Upstream` when the
    /// session cannot start.
    pub async fn cockpit_open(
        self: &Arc<Self>,
        ctx: &AuthContext,
        sandbox: &str,
    ) -> AppResult<ShellInfo> {
        let sandbox_id = SandboxId::parse(sandbox).map_err(|_| AppError::NotFound)?;
        let (record, _) = self.require(&ctx.user, &sandbox_id, Action::Work).await?;
        // A running Cockpit is returned without waiting for anybody's start.
        if let Some(running) = self.running_cockpit(&ctx.user.id, &record.id) {
            return Ok(running.info());
        }
        // Detached from the request: a client that goes away mid-start leaves its Cockpit
        // registered (or discarded), and a second open waits for that start (SPEC-077 DRS-001).
        let app = Arc::clone(self);
        let user = ctx.user.clone();
        Self::run_detached(async move { app.start_requested_cockpit(&user, &record).await }).await
    }

    /// Starts `user`'s Cockpit unless another start of theirs in that sandbox just did.
    async fn start_requested_cockpit(
        self: &Arc<Self>,
        user: &UserRecord,
        record: &SandboxRecord,
    ) -> AppResult<ShellInfo> {
        let _starting = self.cockpit_start_slot(&user.id, &record.id).await?;
        if let Some(running) = self.running_cockpit(&user.id, &record.id) {
            return Ok(running.info());
        }
        if !self.sandbox_repo(&record.id).is_dir() {
            return Err(AppError::Conflict(
                "the sandbox repository is not cloned yet".into(),
            ));
        }
        // The Cockpit runs the default model, Opus 5.5: its weekly Opus cap binds (SPEC-023).
        let account = self
            .pool_account(Provider::Anthropic, cockpit_family())
            .await?;
        // A Cockpit a usage limit ended, whose move failed, continues the cut turn now.
        let key = (user.id.clone(), record.id.clone());
        let pending = |remove: bool| {
            self.shells.cockpit_continue.lock().is_ok_and(|mut set| {
                if remove {
                    set.remove(&key)
                } else {
                    set.contains(&key)
                }
            })
        };
        let started = self
            .start_cockpit(user, record, &account, pending(false))
            .await?;
        pending(true);
        Ok(started)
    }

    /// Starts `user`'s Cockpit in `record` on `account`, resuming the newest conversation; with
    /// `continue_interrupted`, it redoes the turn a usage limit cut (SPEC-074 SUP-004). The caller
    /// holds the start lock.
    async fn start_cockpit(
        self: &Arc<Self>,
        user: &UserRecord,
        record: &SandboxRecord,
        account: &AccountRecord,
        continue_interrupted: bool,
    ) -> AppResult<ShellInfo> {
        let repo = self.sandbox_repo(&record.id);
        let credential = self.open_credential(account)?;
        let id = self.new_shell_id();
        let runtime = self.new_shell_runtime(
            &id,
            ShellKind::Cockpit,
            &record.id,
            &user.id,
            None,
            &repo,
            COCKPIT_NAME.to_owned(),
        );
        let _ = runtime.account.set(account.id.clone());
        let _ = runtime.launch.set(self.cockpit_launch_digest());
        match self
            .launch_cockpit(user, record, &runtime, credential, continue_interrupted)
            .await
        {
            Ok(reader) => match self.register_shell(&runtime) {
                Ok(()) => {
                    self.start_pump(&runtime, reader);
                    self.start_cockpit_watch(&runtime);
                    tracing::info!(shell = %id, sandbox = %record.slug, user = %user.id, account = %account.id, continue_interrupted, "cockpit started");
                    self.end_if_removed(&runtime);
                    Ok(runtime.info())
                }
                Err(err) => {
                    self.discard_shell(&runtime);
                    Err(err)
                }
            },
            Err(err) => {
                self.discard_shell(&runtime);
                Err(err)
            }
        }
    }

    /// Watches a running Cockpit's conversations for usage limits until it ends (SPEC-074
    /// SUP-002): every [`WATCH_INTERVAL`], the newest conversation in its folder, read from its
    /// start; a limit recorded before the Cockpit was created is history.
    pub(crate) fn start_cockpit_watch(self: &Arc<Self>, runtime: &Arc<ShellRuntime>) {
        let app = Arc::clone(self);
        let watched = Arc::clone(runtime);
        let task = tokio::spawn(async move { app.watch_cockpit(watched).await });
        if let Ok(mut watch) = runtime.watch.lock() {
            *watch = Some(task);
        }
    }

    async fn watch_cockpit(self: Arc<Self>, runtime: Arc<ShellRuntime>) {
        let home = self
            .users_dir()
            .join(runtime.creator().as_str())
            .join("home");
        let info = runtime.info();
        let folder = PathBuf::from(&info.folder);
        let started = info.created_at;
        let mut ended = runtime.ended_signal();
        let mut watched: Option<(PathBuf, Box<dyn TranscriptWatch>)> = None;
        // Whether the watched conversation's history has been read: a limit without a readable
        // time counts only after it.
        let mut history_read = false;
        let mut turn = TurnState::default();
        // A refresh that could not start is tried again from then on (ms).
        let mut refresh_from = 0_i64;
        let mut interval = tokio::time::interval(WATCH_INTERVAL);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                _ = interval.tick() => {}
                _ = ended.wait_for(|ended| *ended) => return,
            }
            if !runtime.is_live() {
                return;
            }
            let newest = match self
                .deps
                .harness
                .newest_claude_session(&home, &folder, Timestamp(0))
            {
                Ok(newest) => newest.map(|(_, path)| path),
                Err(err) => {
                    tracing::debug!(error = %err, shell = %runtime.id(), "cockpit conversations unreadable");
                    None
                }
            };
            if let Some(path) = newest
                && watched.as_ref().is_none_or(|(current, _)| *current != path)
            {
                match self
                    .deps
                    .harness
                    .watch(Harness::ClaudeCode, &path, WatchFrom::Start)
                {
                    Ok(watch) => {
                        watched = Some((path, watch));
                        history_read = false;
                        turn = TurnState::default();
                    }
                    Err(err) => {
                        tracing::debug!(error = %err, shell = %runtime.id(), "cockpit conversation not watchable");
                    }
                }
            }
            let Some((watched_path, watch)) = watched.as_mut() else {
                // No conversation yet: a new start cuts nothing (SPEC-186 SAV-006).
                if self
                    .cockpit_refresh_now(&runtime, &mut refresh_from, None)
                    .await
                {
                    return;
                }
                continue;
            };
            // Its length before the poll: everything up to it is read by the poll, so a prompt
            // written later shows as growth when the refresh checks again (review round 2).
            let polled_len = transcript_len(watched_path);
            let observations = match watch.poll() {
                Ok(observations) => observations,
                Err(err) => {
                    tracing::debug!(error = %err, shell = %runtime.id(), "cockpit conversation poll failed");
                    continue;
                }
            };
            let now = self.now();
            for observation in &observations {
                turn.observe(observation, now);
            }
            // New: recorded since the Cockpit was created and after the last limit it acted on,
            // and not reset already (SPEC-074 SUP-002).
            let handled = runtime.limit_handled_ms.load(Ordering::SeqCst);
            // Several read at once count as one, the newest (transcript order): its reset is the
            // subscription's, and the mark passes them all.
            let mut limit = observations.into_iter().filter_map(|o| match o {
                Observation::LimitReached {
                    resets_at,
                    window_minutes,
                    at,
                    ..
                } if at.map_or(history_read, |at| at.0 >= started.0 && at.0 > handled)
                    && resets_at.is_none_or(|resets| resets.0 > now.0) =>
                {
                    Some((resets_at, window_minutes, at))
                }
                _ => None,
            });
            let limit = limit.next_back();
            history_read = history_read || watch.caught_up();
            if let Some((resets_at, window_minutes, at)) = limit {
                runtime
                    .limit_handled_ms
                    .fetch_max(at.unwrap_or(now).0, Ordering::SeqCst);
                if let Err(err) = self.save_shell(&runtime) {
                    tracing::warn!(error = %err, shell = %runtime.id(), "could not keep the cockpit's handled limit");
                }
                if self
                    .cockpit_limit_reached(&runtime, resets_at, window_minutes)
                    .await
                {
                    return;
                }
            }
            // Once its history is read: idle, it starts again now; in a turn, it waits for the end.
            if history_read && runtime.refresh.load(Ordering::SeqCst) != REFRESH_NONE {
                if turn.idle(now) {
                    // What was read so far: a prompt written after it keeps the process.
                    let seen = (watched_path.as_path(), polled_len);
                    if self
                        .cockpit_refresh_now(&runtime, &mut refresh_from, Some(seen))
                        .await
                    {
                        return;
                    }
                } else {
                    runtime.refresh.store(REFRESH_WAITING, Ordering::SeqCst);
                }
            }
        }
    }

    /// Starts a Cockpit due for a refresh again now, unless a failed try asks to wait until
    /// `retry_from` (ms). Returns whether this Cockpit has ended.
    async fn cockpit_refresh_now(
        self: &Arc<Self>,
        runtime: &Arc<ShellRuntime>,
        retry_from: &mut i64,
        seen: Option<(&std::path::Path, u64)>,
    ) -> bool {
        let now = self.now().0;
        if runtime.refresh.load(Ordering::SeqCst) == REFRESH_NONE || now < *retry_from {
            return false;
        }
        runtime.refresh.store(REFRESH_DUE, Ordering::SeqCst);
        match self.refresh_cockpit(runtime, seen).await {
            Refresh::Ended => true,
            // The conversation moved on: the watch reads it and decides again.
            Refresh::Busy => false,
            Refresh::Failed => {
                *retry_from = now.saturating_add(REFRESH_RETRY_MS);
                false
            }
        }
    }

    /// Starts a Cockpit adopted from a Yulius that served it something else again, resuming its
    /// newest conversation, so it gets this Yulius' tools and environment (SPEC-186 SAV-006);
    /// its viewers are told why and follow it (`switched`). `seen` is the conversation's
    /// transcript as it was found idle: written to since, the Cockpit is busy again and stays.
    /// Otherwise the Cockpit stays running as it is when its owner lost access, no subscription
    /// has capacity or its process could not be ended (`Failed`).
    async fn refresh_cockpit(
        self: &Arc<Self>,
        runtime: &Arc<ShellRuntime>,
        seen: Option<(&std::path::Path, u64)>,
    ) -> Refresh {
        let Ok(_starting) = self
            .cockpit_start_slot(runtime.creator(), runtime.sandbox())
            .await
        else {
            return Refresh::Failed;
        };
        if !runtime.is_live() {
            return Refresh::Ended;
        }
        let owner = match self.deps.store.user(runtime.creator()).await {
            Ok(Some(user)) if user.disabled_at.is_none() => user,
            _ => return Refresh::Failed,
        };
        let Ok((record, _)) = self.require(&owner, runtime.sandbox(), Action::Work).await else {
            return Refresh::Failed;
        };
        let account = match self
            .pool_account(Provider::Anthropic, cockpit_family())
            .await
        {
            Ok(account) => account,
            Err(err) => {
                tracing::info!(error = %err, shell = %runtime.id(), "the cockpit waits for a subscription to restart on");
                return Refresh::Failed;
            }
        };
        // Checked last, right before the end: a prompt sent meanwhile is not cut.
        if !still_idle(seen) {
            return Refresh::Busy;
        }
        runtime.notice(REFRESH_NOTICE);
        runtime.switching.store(true, Ordering::SeqCst);
        if !self.end_shells(std::slice::from_ref(runtime)).await {
            // Its process may still run: a second one would write the same conversation.
            runtime.switching.store(false, Ordering::SeqCst);
            tracing::warn!(shell = %runtime.id(), "the cockpit is not restarted: its process could not be ended");
            return Refresh::Failed;
        }
        match self.start_cockpit(&owner, &record, &account, false).await {
            Ok(started) => {
                tracing::info!(from = %runtime.id(), to = %started.id, "cockpit restarted to get what this Yulius serves");
            }
            // The panel starts it again, resuming the conversation, when it asks for its Cockpit.
            Err(err) => {
                tracing::warn!(error = %err, shell = %runtime.id(), "the cockpit could not start again after an update");
            }
        }
        Refresh::Ended
    }

    /// A usage limit cut the Cockpit's turn (SPEC-074): records it on its subscription
    /// (SUP-003), then starts it again on one with capacity, continuing the turn (SUP-004), or
    /// tells its viewers there is none (SUP-005). Returns whether this Cockpit has ended.
    async fn cockpit_limit_reached(
        self: &Arc<Self>,
        runtime: &Arc<ShellRuntime>,
        resets_at: Option<Timestamp>,
        window_minutes: Option<u32>,
    ) -> bool {
        let current = runtime.account.get().cloned();
        tracing::info!(shell = %runtime.id(), account = ?current, "usage limit reached in the cockpit");
        if let Some(account) = &current {
            self.record_limit(account, resets_at, window_minutes).await;
        }
        let Ok(_starting) = self
            .cockpit_start_slot(runtime.creator(), runtime.sandbox())
            .await
        else {
            return false;
        };
        if !runtime.is_live() {
            return true;
        }
        // Its owner, still allowed to work here, as at any start.
        let owner = match self.deps.store.user(runtime.creator()).await {
            Ok(Some(user)) if user.disabled_at.is_none() => user,
            _ => return false,
        };
        let Ok((record, _)) = self.require(&owner, runtime.sandbox(), Action::Work).await else {
            return false;
        };
        let account = match self
            .pool_account(Provider::Anthropic, cockpit_family())
            .await
        {
            Ok(account) if Some(&account.id) != current.as_ref() => account,
            _ => {
                runtime.notice(NO_CAPACITY_NOTICE);
                return false;
            }
        };
        runtime.switching.store(true, Ordering::SeqCst);
        if !self.end_shells(std::slice::from_ref(runtime)).await {
            // Its process may still run: a second one would write the same conversation, whose
            // transcript the move replaces (SPEC-146 SHE-002). It stays here, live.
            runtime.switching.store(false, Ordering::SeqCst);
            tracing::warn!(shell = %runtime.id(), "the cockpit stays on its subscription: its process could not be ended");
            runtime.notice(COCKPIT_NOT_MOVED);
            return false;
        }
        match self.start_cockpit(&owner, &record, &account, true).await {
            Ok(started) => {
                tracing::info!(from = %runtime.id(), to = %started.id, account = %account.id, "cockpit moved to another subscription after a usage limit");
            }
            // The panel starts it again when it asks for its Cockpit, still continuing the turn.
            Err(err) => {
                tracing::warn!(error = %err, shell = %runtime.id(), "the cockpit could not start on another subscription");
                if let Ok(mut pending) = self.shells.cockpit_continue.lock() {
                    pending.insert((runtime.creator().clone(), runtime.sandbox().clone()));
                }
            }
        }
        true
    }

    /// Takes the start lock of `user`'s Cockpit in `sandbox`: one start at a time per user and
    /// sandbox, other users and sandboxes never wait (CKP-001, SPEC-077 DRS-004).
    async fn cockpit_start_slot(
        &self,
        user: &UserId,
        sandbox: &SandboxId,
    ) -> AppResult<CockpitStartSlot<'_>> {
        let key = (user.clone(), sandbox.clone());
        let lock = {
            let mut starts = self
                .shells
                .cockpit_start
                .lock()
                .map_err(AppError::internal)?;
            Arc::clone(starts.entry(key.clone()).or_default())
        };
        let mut slot = CockpitStartSlot {
            app: self,
            key,
            guard: None,
        };
        slot.guard = Some(lock.lock_owned().await);
        Ok(slot)
    }

    async fn launch_cockpit(
        &self,
        user: &UserRecord,
        record: &SandboxRecord,
        runtime: &ShellRuntime,
        credential: crate::model::Secret,
        continue_interrupted: bool,
    ) -> AppResult<Box<dyn crate::ports::session::RunReader>> {
        let (join, lease) = self.isolation_join(record).await?.unzip();
        let space = self.user_space(user, record, join)?;
        let folder = PathBuf::from(runtime.info().folder);
        // The newest conversation of this user here: after `/clear`, the cleared one's successor;
        // none when it could not be resumed last time.
        let newest = match self.deps.harness.newest_claude_session(
            &space.home_host,
            &folder,
            Timestamp(0),
        ) {
            Ok(found) => found.map(|(id, _)| id),
            Err(err) => {
                tracing::warn!(error = %err, user = %user.id, "cockpit conversations unreadable; starting a fresh one");
                None
            }
        };
        let unresumable = self
            .shells
            .cockpit_unresumable
            .lock()
            .ok()
            .and_then(|failed| failed.get(&(user.id.clone(), record.id.clone())).cloned());
        let resume = newest.filter(|id| Some(id) != unresumable.as_ref());
        // Yulius attaches the repository's `.mcp.json` servers; Claude Code rejects them here
        // instead of asking the person (SPEC-095 PMC-001, PMC-004).
        let project_mcp_servers = self.import_project_mcp(record).await;
        // The sandbox's MCP servers as a task run gets them, the file's included (SPEC-147).
        let mcp = self.mcp_attachments(record).await?;
        let bearer = runtime.cockpit_token(self.deps.crypto.as_ref());
        let mut plan = self.deps.harness.plan_cockpit(&CockpitLaunch {
            cwd: folder.clone(),
            config_dir: space.home.join(".claude"),
            config_dir_host: space.home_host.join(".claude"),
            resume: resume.clone(),
            continue_interrupted: continue_interrupted && resume.is_some(),
            credential,
            yulius: Some((
                self.config.internal_url.clone(),
                crate::model::Secret(bearer.clone()),
            )),
            project_mcp_servers,
            mcp,
            contained: self.config.contained,
        })?;
        if let Some(id) = resume {
            let _ = runtime.resumed.set((id, self.now()));
        }
        if let (Some(program), Some(first)) =
            (self.shells.cockpit_program.get(), plan.argv.first_mut())
        {
            *first = program.to_string_lossy().into_owned();
        }
        // The user's own environment (identity, Git as their GitHub account, ports), then the
        // session's; the shell its Bash tool uses stays the user's login shell.
        let mut env = self.shell_env(record, &space, &self.shell_program(), &folder);
        env.retain(|(key, _)| !plan.env.iter().any(|(k, _)| k == key));
        env.append(&mut plan.env);
        // Its Bash commands, hooks and stdio MCP servers run through the agent-secret redaction,
        // which loads its fingerprints with the person's workspace token (SPEC-147 SAM-002).
        if let Some(prefix) = self.scrub_shell_prefix() {
            env.retain(|(key, _)| *key != prefix.0);
            env.push(prefix);
        }
        plan.env = env;
        plan.containment = space.containment.clone();
        // Behind the egress proxy as a task run is, with its own bearer (SPEC-186 SAV-001).
        self.vault_superagent_setup(record, &runtime.run_dir, &bearer, &mut plan)
            .await?;
        let reader = self.spawn_shell(runtime, &plan).await;
        drop(lease);
        let reader = reader?;
        self.save_shell(runtime)?;
        Ok(reader)
    }
}

/// A held Cockpit start lock. Dropped, it forgets the lock when no other start holds or waits for
/// it, so the map keeps only pairs with a start running or waiting (SPEC-077 DRS-005).
struct CockpitStartSlot<'a> {
    app: &'a App,
    key: (UserId, SandboxId),
    guard: Option<tokio::sync::OwnedMutexGuard<()>>,
}

impl Drop for CockpitStartSlot<'_> {
    fn drop(&mut self) {
        drop(self.guard.take());
        // Under the map's lock nobody can take a new handle on this mutex: a count of one is the
        // map's own.
        if let Ok(mut starts) = self.app.shells.cockpit_start.lock()
            && starts
                .get(&self.key)
                .is_some_and(|lock| Arc::strong_count(lock) == 1)
        {
            starts.remove(&self.key);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Observation, Timestamp, TurnState};

    /// SPEC-186 SAV-006: a conversation is idle with no turn open and no background work Claude
    /// Code waits for; a Monitor stops counting once it expires.
    #[test]
    fn a_conversation_is_idle_without_a_turn_or_background_work() {
        let mut turn = TurnState::default();
        let now = Timestamp(1_000);
        assert!(turn.idle(now), "nothing happened yet");
        turn.observe(
            &Observation::UserTurn {
                at: None,
                wake: false,
            },
            now,
        );
        assert!(!turn.idle(now));
        turn.observe(&Observation::TurnComplete { at: None }, now);
        assert!(
            !turn.idle(now),
            "a sub-agent's turn ended, not the session's"
        );
        turn.observe(&Observation::TurnComplete { at: Some(now) }, now);
        assert!(turn.idle(now));

        let agent = Observation::BackgroundStarted {
            id: "agent-1".into(),
            agent: true,
            expires_in_ms: None,
            at: Some(now),
        };
        let monitor = Observation::BackgroundStarted {
            id: "monitor-1".into(),
            agent: false,
            expires_in_ms: Some(5_000),
            at: Some(now),
        };
        turn.observe(&agent, now);
        turn.observe(&monitor, now);
        assert!(!turn.idle(Timestamp(5_999)), "both run");
        turn.observe(
            &Observation::BackgroundEnded {
                id: "agent-1".into(),
            },
            now,
        );
        assert!(!turn.idle(Timestamp(5_999)), "the Monitor still runs");
        assert!(turn.idle(Timestamp(6_001)), "the Monitor expired");
        turn.observe(&agent, now);
        assert!(
            !turn.idle(Timestamp(100_000)),
            "an agent runs until it ends"
        );
        turn.observe(
            &Observation::BackgroundEnded {
                id: "agent-1".into(),
            },
            now,
        );
        turn.observe(&Observation::AgentWork { at: None }, now);
        assert!(!turn.idle(now), "woken by a notification");
        turn.observe(&Observation::TurnInterrupted { at: None }, now);
        assert!(turn.idle(now));
        // The API rejected a prompt for a full context: that turn is over too (#213).
        turn.observe(
            &Observation::UserTurn {
                at: None,
                wake: false,
            },
            now,
        );
        turn.observe(&Observation::ContextFull, now);
        assert!(turn.idle(now), "a full context ends the turn");
    }

    /// Review round 1: a conversation that grew after it was found idle (a prompt sent meanwhile)
    /// is not cut by the restart.
    #[test]
    fn a_transcript_that_grew_since_it_was_idle_is_not_cut()
    -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("conv-1.jsonl");
        std::fs::write(&path, "{}\n")?;
        let seen = super::transcript_len(&path);
        assert!(super::still_idle(Some((path.as_path(), seen))));
        assert!(super::still_idle(None), "no conversation: nothing to cut");
        std::fs::write(&path, "{}\n{\"type\":\"user\"}\n")?;
        assert!(!super::still_idle(Some((path.as_path(), seen))));
        Ok(())
    }
}
