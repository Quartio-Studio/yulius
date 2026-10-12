//! Provider accounts (subscriptions): add, see usage, remove, and the native login flow
//! (SPEC-001 ROT-001/002/009, ACC-009; SPEC-002 ADR-05, ADR-17, §7.2 "Login", §8). There are no
//! priorities or switches to manage: the pool policy decides (F 2026-10-10).
//!
//! Each login runs the provider CLI's own sign-in (`claude setup-token`) in a server-owned PTY through the process host, with a temporary config
//! directory under `<data_dir>/logins/<login_id>/`. Viewers (instance administrators) watch it
//! through a [`Hub`]; every output chunk passes through the harness masker first, so a Claude
//! token never reaches a browser. A bounded copy of the raw output is kept in memory for token
//! extraction. On exit the credential is sealed into the account, the temporary directory is
//! overwritten and removed, and `account.updated` is published.
//!
//! Bounds: one active login per account (a new one replaces the old), at most
//! [`MAX_ACTIVE_LOGINS`] at once, [`LOGIN_RAW_MAX`] bytes of raw output, [`LOGIN_TIMEOUT`] per
//! login, [`LOGIN_VIEWERS_MAX`] viewers per login.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::sync::Notify;
use yulius_contracts::api::admin::{
    AccountLoginStarted, AccountUsageView, AccountView, CreateAccount, TokenTotalsView,
    UpdateAccount, UsageWindowView,
};
use yulius_contracts::events::UiEvent;
use yulius_contracts::terminal::TerminalServerMsg;
use yulius_domain::harness::Provider;
use yulius_domain::ids::AccountId;
use yulius_domain::time::Timestamp;
use yulius_domain::usage::{AccountUsage, UsageWindow};

use crate::auth::{AuthContext, rfc3339};
use crate::context::App;
use crate::error::{AppError, AppResult};
use crate::hub::{Hub, Subscription};
use crate::model::{AccountRecord, AccountStatus, AttemptRecord, Secret};
use crate::ports::harness::OutputMasker;
use crate::ports::session::{
    Containment, LaunchPlan, RunEvent, RunExit, RunInspection, RunReader, RunSignal, RunWriter,
};
use crate::ports::usage::{UsageFailure, UsageFailureKind};
use crate::usage_json;

/// Maximum duration of one login before its process is stopped.
pub const LOGIN_TIMEOUT: Duration = Duration::from_secs(15 * 60);
/// PTY width of login terminals: wide enough that tokens and URLs never wrap.
pub const LOGIN_COLS: u16 = 500;
/// PTY height of login terminals.
pub const LOGIN_ROWS: u16 = 40;
/// Raw output kept in memory for token extraction (the newest bytes win).
pub const LOGIN_RAW_MAX: usize = 64 * 1024;
/// Logins running at the same time (instance-wide).
pub const MAX_ACTIVE_LOGINS: usize = 8;
/// Viewers per login terminal.
pub const LOGIN_VIEWERS_MAX: u64 = 16;
/// How long a finished login stays visible to (re)connecting viewers.
pub const FINISHED_LOGIN_RETENTION_MS: i64 = 10 * 60 * 1000;
/// Logins kept in memory (running plus recently finished).
pub const MAX_RETAINED_LOGINS: usize = 32;
/// Largest keyboard input frame accepted from a viewer.
pub const LOGIN_INPUT_MAX: usize = 64 * 1024;
/// Longest account label.
pub const LABEL_MAX: usize = 80;

/// Wait for the process to exit after a stop request before killing its host.
const STOP_WAIT: Duration = Duration::from_secs(10);
/// Grace given to the process on a stop request before the host escalates.
const STOP_GRACE_MS: u32 = 2_000;
/// Secure-deletion walk bounds.
const WIPE_MAX_DEPTH: usize = 16;
const WIPE_MAX_ENTRIES: usize = 10_000;

/// Phase of a login.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoginPhase {
    /// The CLI is running.
    Running,
    /// A credential was sealed into the account.
    Succeeded,
    /// The login ended without a credential.
    Failed,
    /// Replaced by a newer login or the account was deleted.
    Cancelled,
}

impl LoginPhase {
    /// Wire name used in terminal `state`/`exit` frames.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Succeeded => "ok",
            Self::Failed => "error",
            Self::Cancelled => "cancelled",
        }
    }
}

/// Raw login output, bounded to the newest [`LOGIN_RAW_MAX`] bytes.
#[derive(Default)]
struct RawOutput {
    bytes: Vec<u8>,
}

impl RawOutput {
    fn push(&mut self, chunk: &[u8]) {
        self.bytes.extend_from_slice(chunk);
        if self.bytes.len() > LOGIN_RAW_MAX {
            let excess = self.bytes.len() - LOGIN_RAW_MAX;
            self.bytes.drain(..excess);
        }
    }

    fn take(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.bytes)
    }
}

/// One running or recently finished login.
pub struct LoginSession {
    id: String,
    account_id: AccountId,
    provider: Provider,
    dir: PathBuf,
    hub: Hub,
    raw: Mutex<RawOutput>,
    writer: tokio::sync::Mutex<Option<Box<dyn RunWriter>>>,
    phase: Mutex<(LoginPhase, Option<i32>)>,
    cancelled: AtomicBool,
    cancel: Notify,
    finished_at: AtomicI64,
}

impl LoginSession {
    fn phase(&self) -> (LoginPhase, Option<i32>) {
        self.phase
            .lock()
            .map(|p| *p)
            .unwrap_or((LoginPhase::Failed, None))
    }

    fn is_running(&self) -> bool {
        self.phase().0 == LoginPhase::Running
    }

    fn request_cancel(&self) {
        self.cancelled.store(true, Ordering::SeqCst);
        self.cancel.notify_one();
    }
}

/// In-memory registry of logins (owned by [`App`]).
#[derive(Default)]
pub struct LoginRegistry {
    sessions: Mutex<HashMap<String, Arc<LoginSession>>>,
    /// Serializes login starts so the per-account and global bounds hold under races.
    start: tokio::sync::Mutex<()>,
    timeout_ms: AtomicU64,
}

/// A viewer's handle on a login terminal (used by the WebSocket route).
pub struct LoginTerminal {
    session: Arc<LoginSession>,
}

impl LoginTerminal {
    /// Subscribes a viewer: masked snapshot plus subsequent masked frames.
    #[must_use]
    pub fn subscribe(&self) -> Subscription {
        self.session.hub.subscribe()
    }

    /// Marks a viewer gone.
    pub fn unsubscribe(&self) {
        self.session.hub.unsubscribe();
    }

    /// Current `state` frame.
    #[must_use]
    pub fn state(&self) -> TerminalServerMsg {
        let (phase, _) = self.session.phase();
        TerminalServerMsg::State {
            attempt_seq: None,
            harness: Some(self.session.provider.harness().as_str().to_owned()),
            cols: LOGIN_COLS,
            rows: LOGIN_ROWS,
            can_write: phase == LoginPhase::Running,
            controller: true,
            status: phase.as_str().to_owned(),
        }
    }

    /// The `exit` frame once the login has finished.
    #[must_use]
    pub fn exit(&self) -> Option<TerminalServerMsg> {
        let (phase, code) = self.session.phase();
        (phase != LoginPhase::Running).then(|| TerminalServerMsg::Exit {
            status: phase.as_str().to_owned(),
            code,
        })
    }

    /// Writes viewer keyboard input to the login process.
    ///
    /// # Errors
    /// `Invalid` for oversized input, `Conflict` once the login has ended.
    pub async fn input(&self, data: &[u8]) -> AppResult<()> {
        if data.len() > LOGIN_INPUT_MAX {
            return Err(AppError::Invalid("input frame too large".into()));
        }
        if !self.session.is_running() {
            return Err(AppError::Conflict("the sign-in has ended".into()));
        }
        let mut writer = self.session.writer.lock().await;
        match writer.as_mut() {
            Some(w) => w.input(data).await,
            None => Err(AppError::Conflict("the sign-in has ended".into())),
        }
    }
}

/// How the login process ended.
enum Ending {
    Exited(RunExit),
    TimedOut,
    Cancelled,
    Lost,
}

fn window_view(w: &UsageWindow) -> UsageWindowView {
    UsageWindowView {
        used_percent: w.known_percent(),
        resets_at: w.resets_at.map(rfc3339),
        window_minutes: w.window_minutes,
    }
}

/// API view of a usage snapshot; unknown windows stay `null`.
#[must_use]
pub fn usage_view(usage: &AccountUsage) -> AccountUsageView {
    AccountUsageView {
        primary: usage.primary.as_ref().map(window_view),
        secondary: usage.secondary.as_ref().map(window_view),
        weekly_opus: usage.weekly_opus.as_ref().map(window_view),
        weekly_sonnet: usage.weekly_sonnet.as_ref().map(window_view),
        observed_at: usage.observed_at.map(rfc3339),
    }
}

/// Parsed usage of an account record (absent or malformed → unknown).
#[must_use]
pub fn account_usage(account: &AccountRecord) -> AccountUsage {
    account
        .usage_json
        .as_deref()
        .map(usage_json::from_json)
        .unwrap_or_default()
}

/// Live attempts that use `account`.
#[must_use]
pub fn in_use_count(account: &AccountId, live: &[AttemptRecord]) -> u32 {
    u32::try_from(
        live.iter()
            .filter(|a| a.account_id.as_ref() == Some(account))
            .count(),
    )
    .unwrap_or(u32::MAX)
}

/// Full API view of an account.
#[must_use]
pub fn account_view(account: &AccountRecord, live: &[AttemptRecord]) -> AccountView {
    AccountView {
        id: account.id.to_string(),
        provider: account.provider.as_str().to_owned(),
        label: account.label.clone(),
        status: account.status.as_str().to_owned(),
        usage: usage_view(&account_usage(account)),
        last_error: account.last_error.clone(),
        in_use_by: in_use_count(&account.id, live),
        tokens_24h: None,
        tokens_7d: None,
    }
}

fn totals_view(tokens: Option<&crate::model::AccountTokens>) -> TokenTotalsView {
    tokens.map_or_else(TokenTotalsView::default, |t| TokenTotalsView {
        runs: t.runs,
        tasks: t.tasks,
        input: t.input,
        output: t.output,
        cache_read: t.cache_read,
        cache_write: t.cache_write,
    })
}

/// One day in milliseconds.
const DAY_MS: i64 = 24 * 60 * 60 * 1000;
/// Prefix of an account's `last_error` that is a note about reading its usage, not a broken
/// account (the UI shows it quietly, ROT-009).
pub const USAGE_NOTE_PREFIX: &str = "usage: ";
/// Back-off after a failed usage request that gave no `Retry-After` (ROT-009).
pub const USAGE_BACKOFF_MS: i64 = 30 * 60 * 1000;
/// A usage round skips a subscription read (by a session or the provider) this recently.
pub const USAGE_FRESH_MS: i64 = 90 * 1000;
/// Opening the subscriptions list asks for a usage round at most this often (F 2026-10-11:
/// "usage should be updated more often + every time someone loads its screen").
pub const USAGE_ON_DEMAND_MS: i64 = 30 * 1000;
/// Back-off after the usage endpoint refused a sign-in that may not read usage (a
/// `setup-token`, ADR-17): asked again once a day in case the sign-in changed.
pub const USAGE_NOT_SHARED_BACKOFF_MS: i64 = DAY_MS;
/// How soon after a successful reading an account is asked again (each reading of an idle
/// subscription is one real request).
pub const USAGE_READ_EVERY_MS: i64 = 5 * 60 * 1000;
/// Longest back-off taken from a provider's `Retry-After`.
pub const USAGE_BACKOFF_MAX_MS: i64 = 2 * 60 * 60 * 1000;

fn validate_label(label: &str) -> AppResult<String> {
    let label = label.trim();
    if label.is_empty() || label.chars().count() > LABEL_MAX || label.chars().any(char::is_control)
    {
        return Err(AppError::Invalid(format!(
            "label must be 1 to {LABEL_MAX} printable characters"
        )));
    }
    Ok(label.to_owned())
}

fn human_duration(d: Duration) -> String {
    let secs = d.as_secs();
    if secs >= 60 {
        format!("{} minutes", secs.div_ceil(60))
    } else {
        format!("{} ms", d.as_millis())
    }
}

fn actor(ctx: &AuthContext) -> String {
    ctx.user
        .github_login
        .clone()
        .unwrap_or_else(|| "superadmin".into())
}

fn mask_whole(masker: &mut dyn OutputMasker, bytes: &[u8]) -> Vec<u8> {
    let mut out = masker.push(bytes);
    out.extend(masker.finish());
    out
}

fn create_private_dir(path: &Path) -> AppResult<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::create_dir_all(path).map_err(AppError::internal)?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))
        .map_err(AppError::internal)
}

/// Overwrites every regular file under `dir` with zeros, syncs it, then removes the tree.
/// Symlinks are removed, never followed. Best effort: on copy-on-write or flash storage the old
/// blocks may survive; the plaintext never leaves the data directory either way.
pub fn secure_delete(dir: &Path) {
    fn wipe(path: &Path, depth: usize, budget: &mut usize) {
        if depth > WIPE_MAX_DEPTH || *budget == 0 {
            return;
        }
        let Ok(entries) = std::fs::read_dir(path) else {
            return;
        };
        for entry in entries.flatten() {
            if *budget == 0 {
                return;
            }
            *budget -= 1;
            let Ok(meta) = entry.file_type() else {
                continue;
            };
            let p = entry.path();
            if meta.is_dir() {
                wipe(&p, depth + 1, budget);
            } else if meta.is_file() {
                overwrite(&p);
            }
        }
    }
    fn overwrite(path: &Path) {
        use std::io::Write as _;
        let Ok(mut file) = std::fs::OpenOptions::new().write(true).open(path) else {
            return;
        };
        let len = file.metadata().map(|m| m.len()).unwrap_or(0);
        let zeros = [0u8; 8192];
        let mut left = len;
        while left > 0 {
            let n = usize::try_from(left.min(zeros.len() as u64)).unwrap_or(zeros.len());
            if file.write_all(&zeros[..n]).is_err() {
                break;
            }
            left -= n as u64;
        }
        let _ = file.sync_all();
    }
    if std::fs::symlink_metadata(dir).is_err() {
        return;
    }
    let mut budget = WIPE_MAX_ENTRIES;
    wipe(dir, 0, &mut budget);
    if let Err(err) = std::fs::remove_dir_all(dir) {
        tracing::warn!(error = %err, "could not remove a login directory");
    }
}

impl App {
    fn login_timeout(&self) -> Duration {
        match self.logins.timeout_ms.load(Ordering::SeqCst) {
            0 => LOGIN_TIMEOUT,
            ms => Duration::from_millis(ms),
        }
    }

    /// Overrides [`LOGIN_TIMEOUT`] for this instance (configuration and tests).
    pub fn set_account_login_timeout(&self, timeout: Duration) {
        let ms = u64::try_from(timeout.as_millis())
            .unwrap_or(u64::MAX)
            .max(1);
        self.logins.timeout_ms.store(ms, Ordering::SeqCst);
    }

    fn logins_dir(&self) -> PathBuf {
        self.config.data_dir.join("logins")
    }

    fn publish_account(&self, view: AccountView) {
        self.events
            .publish(None, true, UiEvent::AccountUpdated { account: view });
    }

    async fn publish_account_id(&self, id: &AccountId) {
        let store = &self.deps.store;
        if let (Ok(Some(account)), Ok(live)) =
            (store.account(id).await, store.live_attempts().await)
        {
            self.publish_account(account_view(&account, &live));
        }
    }

    async fn load_account(&self, id: &str) -> AppResult<AccountRecord> {
        let id = AccountId::parse(id).map_err(|_| AppError::NotFound)?;
        self.deps
            .store
            .account(&id)
            .await?
            .ok_or(AppError::NotFound)
    }

    /// Lists accounts with usage and live use (ROT-009).
    ///
    /// # Errors
    /// `Forbidden` for non-administrators.
    pub async fn list_accounts(self: &Arc<Self>, ctx: &AuthContext) -> AppResult<Vec<AccountView>> {
        self.require_instance_admin(ctx)?;
        let store = &self.deps.store;
        let live = store.live_attempts().await?;
        let now = self.now();
        // Opening the list reads usage again in the background (within each subscription's
        // back-off); the cards update as the readings arrive.
        let due = self
            .capacity
            .usage_asked
            .lock()
            .map(|mut last| {
                let due = last.is_none_or(|at| now.millis_since(at) >= USAGE_ON_DEMAND_MS);
                if due {
                    *last = Some(now);
                }
                due
            })
            .unwrap_or(false);
        if due {
            let app = Arc::clone(self);
            tokio::spawn(async move { app.refresh_usage_shared().await });
        }
        let day = store.account_tokens(now.plus_millis(-DAY_MS)).await?;
        let week = store.account_tokens(now.plus_millis(-7 * DAY_MS)).await?;
        let find = |list: &[crate::model::AccountTokens], id: &str| {
            list.iter().find(|t| t.account_id == id).cloned()
        };
        Ok(store
            .accounts()
            .await?
            .iter()
            .map(|a| {
                let mut view = account_view(a, &live);
                view.tokens_24h = Some(totals_view(find(&day, a.id.as_str()).as_ref()));
                view.tokens_7d = Some(totals_view(find(&week, a.id.as_str()).as_ref()));
                view
            })
            .collect())
    }

    /// Asks the provider for the account's current usage windows and stores them (ROT-009).
    /// A failure is recorded as the account's last error and returned.
    ///
    /// # Errors
    /// `Forbidden` for non-admins, `NotFound`, `Conflict` without a credential, `Upstream` when
    /// the provider does not answer usefully.
    pub async fn refresh_account_usage(
        &self,
        ctx: &AuthContext,
        id: &str,
    ) -> AppResult<AccountView> {
        self.require_instance_admin(ctx)?;
        let id = AccountId::parse(id).map_err(|_| AppError::NotFound)?;
        let store = &self.deps.store;
        let account = store.account(&id).await?.ok_or(AppError::NotFound)?;
        if account.credential.is_none() {
            return Err(AppError::Conflict(
                "the account is not signed in yet".into(),
            ));
        }
        // A sign-in that may not read usage is not a failure: its sessions report it (ADR-17).
        if let Err(failure) = self.fetch_and_record_usage(&account).await
            && failure.kind != UsageFailureKind::NotShared
        {
            return Err(AppError::Upstream(failure.message));
        }
        let updated = store.account(&id).await?.ok_or(AppError::NotFound)?;
        let mut view = account_view(&updated, &store.live_attempts().await?);
        let now = self.now();
        view.tokens_24h = Some(totals_view(
            store
                .account_tokens(now.plus_millis(-DAY_MS))
                .await?
                .iter()
                .find(|t| t.account_id == id.as_str()),
        ));
        view.tokens_7d = Some(totals_view(
            store
                .account_tokens(now.plus_millis(-7 * DAY_MS))
                .await?
                .iter()
                .find(|t| t.account_id == id.as_str()),
        ));
        Ok(view)
    }

    /// Asks the provider for one account's usage and stores it; a failure becomes the account's
    /// last error and keeps the last known usage (ROT-009).
    async fn fetch_and_record_usage(&self, account: &AccountRecord) -> Result<(), UsageFailure> {
        let sealed = account
            .credential
            .as_ref()
            .ok_or_else(|| UsageFailure::new("the account is not signed in yet"))?;
        let credential = self
            .deps
            .crypto
            .open(sealed)
            .ok()
            .and_then(|bytes| String::from_utf8(bytes).ok())
            .map(Secret)
            .ok_or_else(|| UsageFailure::new("the stored credential cannot be read"))?;
        match self.deps.usage.fetch(account.provider, &credential).await {
            Ok(mut usage) => {
                usage.observed_at = Some(self.now());
                self.record_account_usage(&account.id, &usage).await;
                self.record_usage_samples(&account.id, &usage, crate::model::USAGE_FROM_PROBE)
                    .await;
                Ok(())
            }
            Err(failure) if failure.kind == UsageFailureKind::NotShared => {
                // Not an error: the sessions on this subscription report its usage. An earlier
                // note about usage goes away; other errors stay.
                let _ = self
                    .deps
                    .store
                    .clear_usage_error(&account.id, self.now())
                    .await;
                Err(failure)
            }
            Err(failure) if failure.kind == UsageFailureKind::Throttled => {
                // Not an error: the provider asks to wait (the round backs off for its
                // Retry-After) and the last reading stays with its time. A note here sat on
                // every subscription at once (F 2026-10-11); an earlier one goes away.
                let _ = self
                    .deps
                    .store
                    .clear_usage_error(&account.id, self.now())
                    .await;
                Err(failure)
            }
            Err(failure) => {
                // Only the error: the status and usage may have changed since `account` was
                // read (a task's limit hit during the request), and must stay (#72). The
                // `usage:` prefix marks it as a note about usage, not a broken account; it never
                // replaces a sign-in or re-login error written meanwhile.
                let message = format!("{USAGE_NOTE_PREFIX}{}", failure.message);
                let _ = self
                    .deps
                    .store
                    .set_usage_error(&account.id, &message, self.now())
                    .await;
                Err(failure)
            }
        }
    }

    /// A usage round with the back-off shared by the background loop and on-demand reads; one
    /// round at a time.
    pub async fn refresh_usage_shared(&self) {
        let mut not_before = self.capacity.usage_not_before.lock().await;
        self.refresh_usage_round(&mut not_before).await;
    }

    /// One background usage round (ROT-009): asks the provider about every signed-in account
    /// whose back-off has passed, so routing and the account cards stay current without
    /// pressing Refresh. A failure backs that account off for the provider's `Retry-After`, or
    /// [`USAGE_BACKOFF_MS`] without one (at most [`USAGE_BACKOFF_MAX_MS`]).
    pub async fn refresh_usage_round(&self, not_before: &mut HashMap<AccountId, Timestamp>) {
        let Ok(accounts) = self.deps.store.accounts().await else {
            return;
        };
        for account in accounts.iter().filter(|a| {
            a.credential.is_some()
                && !matches!(
                    a.status,
                    AccountStatus::PendingLogin | AccountStatus::Relogin
                )
        }) {
            let now = self.now();
            if not_before.get(&account.id).is_some_and(|at| *at > now) {
                continue;
            }
            // A reading from moments ago needs no request. That covers a running session, which
            // reports as it works; a running task that is idle reports nothing, so its reading
            // ages and is asked for again (production, #4: a task in use is no reason to skip).
            if account_usage(account)
                .observed_at
                .is_some_and(|at| now.millis_since(at) < USAGE_FRESH_MS)
            {
                continue;
            }
            match self.fetch_and_record_usage(account).await {
                Ok(()) => {
                    // An idle subscription is read from a real request: ask again in minutes.
                    not_before.insert(account.id.clone(), now.plus_millis(USAGE_READ_EVERY_MS));
                }
                Err(failure) => {
                    let wait = match (failure.kind, failure.retry_after) {
                        (UsageFailureKind::NotShared, _) => USAGE_NOT_SHARED_BACKOFF_MS,
                        (_, Some(d)) => i64::try_from(d.as_millis())
                            .unwrap_or(USAGE_BACKOFF_MAX_MS)
                            .clamp(60_000, USAGE_BACKOFF_MAX_MS),
                        (_, None) => USAGE_BACKOFF_MS,
                    };
                    not_before.insert(account.id.clone(), now.plus_millis(wait));
                }
            }
        }
    }

    /// Creates an account in `pending_login` and starts its login terminal (ROT-001/002).
    ///
    /// The creation is undone when the login cannot be started.
    ///
    /// # Errors
    /// `Invalid` provider/label, `Forbidden`, `Capacity` when too many logins run, `Upstream`
    /// when the process host fails.
    pub async fn create_account(
        self: &Arc<Self>,
        ctx: &AuthContext,
        req: &CreateAccount,
    ) -> AppResult<AccountLoginStarted> {
        self.require_instance_admin(ctx)?;
        let provider = Provider::parse(req.provider.trim())
            .map_err(|_| AppError::Invalid("provider must be anthropic (Claude)".into()))?;
        let label = validate_label(&req.label)?;
        let store = &self.deps.store;
        let now = self.now();
        let account = AccountRecord {
            id: AccountId::parse(&self.deps.crypto.new_id()).map_err(AppError::internal)?,
            provider,
            label: label.clone(),
            credential: None,
            status: AccountStatus::PendingLogin,
            usage_json: None,
            last_error: None,
            created_at: now,
            updated_at: now,
        };
        store.insert_account(&account).await?;
        let session = match self.start_login(&account).await {
            Ok(s) => s,
            Err(err) => {
                store.delete_account(&account.id).await?;
                return Err(err);
            }
        };
        store
            .audit(
                now,
                &actor(ctx),
                "account.created",
                Some(account.id.as_str()),
                Some(&format!("provider={} label={label}", provider.as_str())),
            )
            .await?;
        let view = account_view(&account, &[]);
        self.publish_account(view.clone());
        Ok(AccountLoginStarted {
            account: view,
            login_id: session.id.clone(),
        })
    }

    /// Renames an account (the only thing to change: the pool has no priorities or switches).
    ///
    /// # Errors
    /// `NotFound`, `Invalid`, `Forbidden`.
    pub async fn update_account(
        &self,
        ctx: &AuthContext,
        id: &str,
        req: &UpdateAccount,
    ) -> AppResult<AccountView> {
        self.require_instance_admin(ctx)?;
        let account = self.load_account(id).await?;
        let label = validate_label(&req.label)?;
        let now = self.now();
        let store = &self.deps.store;
        store.update_account(&account.id, &label, now).await?;
        store
            .audit(
                now,
                &actor(ctx),
                "account.updated",
                Some(account.id.as_str()),
                Some(&format!("label={label:?}")),
            )
            .await?;
        let updated = store
            .account(&account.id)
            .await?
            .ok_or(AppError::NotFound)?;
        let view = account_view(&updated, &store.live_attempts().await?);
        self.publish_account(view.clone());
        Ok(view)
    }

    /// Deletes an account and its sealed credential; refused while a live task uses it.
    /// A running login of the account is cancelled.
    ///
    /// # Errors
    /// `NotFound`, `Conflict` while in use, `Forbidden`.
    pub async fn delete_account(&self, ctx: &AuthContext, id: &str) -> AppResult<()> {
        self.require_instance_admin(ctx)?;
        let account = self.load_account(id).await?;
        let store = &self.deps.store;
        let in_use = in_use_count(&account.id, &store.live_attempts().await?);
        if in_use > 0 {
            return Err(AppError::Conflict(format!(
                "{} is used by {in_use} running task(s); stop or switch them first",
                account.label
            )));
        }
        if let Some(active) = self.active_login(&account.id) {
            active.request_cancel();
        }
        store.delete_account(&account.id).await?;
        self.capacity_changed();
        store
            .audit(
                self.now(),
                &actor(ctx),
                "account.deleted",
                Some(account.id.as_str()),
                Some(&account.label),
            )
            .await
    }

    /// Starts a new login for an existing account (replacing a running one).
    ///
    /// # Errors
    /// `NotFound`, `Forbidden`, `Capacity`, `Upstream`.
    pub async fn relogin_account(
        self: &Arc<Self>,
        ctx: &AuthContext,
        id: &str,
    ) -> AppResult<AccountLoginStarted> {
        self.require_instance_admin(ctx)?;
        let account = self.load_account(id).await?;
        let session = self.start_login(&account).await?;
        let store = &self.deps.store;
        store
            .audit(
                self.now(),
                &actor(ctx),
                "account.relogin",
                Some(account.id.as_str()),
                None,
            )
            .await?;
        Ok(AccountLoginStarted {
            account: account_view(&account, &store.live_attempts().await?),
            login_id: session.id.clone(),
        })
    }

    /// Opens a login terminal for a viewer.
    ///
    /// # Errors
    /// `Forbidden` for non-administrators, `NotFound` for unknown or expired logins,
    /// `Capacity` when the viewer bound is reached.
    pub fn login_terminal(&self, ctx: &AuthContext, login_id: &str) -> AppResult<LoginTerminal> {
        self.require_instance_admin(ctx)?;
        self.prune_logins();
        let session = self
            .logins
            .sessions
            .lock()
            .map_err(AppError::internal)?
            .get(login_id)
            .cloned()
            .ok_or(AppError::NotFound)?;
        if session.hub.viewer_count() >= LOGIN_VIEWERS_MAX {
            return Err(AppError::Capacity(
                "too many viewers on this sign-in".into(),
            ));
        }
        Ok(LoginTerminal { session })
    }

    /// Removes login directories left by a previous server process (their hosts are stopped
    /// first). Call once at startup, before serving requests.
    pub async fn cleanup_stale_logins(&self) {
        let dir = self.logins_dir();
        let Ok(entries) = std::fs::read_dir(&dir) else {
            return;
        };
        let active: Vec<PathBuf> = self
            .logins
            .sessions
            .lock()
            .map(|m| m.values().map(|s| s.dir.clone()).collect())
            .unwrap_or_default();
        for entry in entries.flatten().take(WIPE_MAX_ENTRIES) {
            let path = entry.path();
            if active.contains(&path) {
                continue;
            }
            if matches!(
                self.deps.session_host.inspect(&path),
                RunInspection::Recorded {
                    host_alive: true,
                    ..
                }
            ) {
                let _ = self.deps.session_host.terminate(&path);
            }
            let _ = tokio::task::spawn_blocking(move || secure_delete(&path)).await;
        }
    }

    fn active_login(&self, account: &AccountId) -> Option<Arc<LoginSession>> {
        self.logins.sessions.lock().ok().and_then(|m| {
            m.values()
                .find(|s| &s.account_id == account && s.is_running())
                .cloned()
        })
    }

    /// Drops finished logins older than the retention period, and the oldest finished ones
    /// beyond [`MAX_RETAINED_LOGINS`].
    fn prune_logins(&self) {
        let now = self.now().0;
        if let Ok(mut map) = self.logins.sessions.lock() {
            map.retain(|_, s| {
                let finished = s.finished_at.load(Ordering::SeqCst);
                finished == 0 || now.saturating_sub(finished) < FINISHED_LOGIN_RETENTION_MS
            });
            let mut finished: Vec<(i64, String)> = map
                .iter()
                .map(|(id, s)| (s.finished_at.load(Ordering::SeqCst), id.clone()))
                .filter(|(at, _)| *at != 0)
                .collect();
            finished.sort();
            let excess = map.len().saturating_sub(MAX_RETAINED_LOGINS);
            for (_, id) in finished.into_iter().take(excess) {
                map.remove(&id);
            }
        }
    }

    /// Spawns the provider CLI login for `account` and starts its output pump.
    async fn start_login(
        self: &Arc<Self>,
        account: &AccountRecord,
    ) -> AppResult<Arc<LoginSession>> {
        let _serialized = self.logins.start.lock().await;
        self.prune_logins();
        let replaced = {
            let map = self.logins.sessions.lock().map_err(AppError::internal)?;
            let previous = map
                .values()
                .find(|s| s.account_id == account.id && s.is_running())
                .cloned();
            let running = map.values().filter(|s| s.is_running()).count();
            if running.saturating_sub(usize::from(previous.is_some())) >= MAX_ACTIVE_LOGINS {
                return Err(AppError::Capacity(
                    "too many sign-ins are running; finish or wait for one".into(),
                ));
            }
            previous
        };
        if let Some(previous) = replaced {
            previous.request_cancel();
        }

        let login_id = self.deps.crypto.new_id();
        let dir = self.logins_dir().join(&login_id);
        let home = dir.join("home");
        let config_dir = dir.join(match account.provider {
            Provider::Anthropic => "claude",
        });
        create_private_dir(&self.logins_dir())?;
        for d in [&dir, &home, &config_dir] {
            create_private_dir(d)?;
        }
        let plan = self.login_plan(account.provider, &home, &config_dir);
        let token = self.deps.crypto.random_bytes();
        let host = &self.deps.session_host;
        let started = async {
            host.spawn(
                &dir,
                &login_id,
                &format!("login-{}", account.id),
                &plan,
                &token,
                (LOGIN_COLS, LOGIN_ROWS),
            )
            .await?;
            host.attach(&dir, &token).await
        }
        .await;
        let attached = match started {
            Ok(a) => a,
            Err(err) => {
                let _ = host.terminate(&dir);
                let wipe_dir = dir.clone();
                let _ = tokio::task::spawn_blocking(move || secure_delete(&wipe_dir)).await;
                return Err(err);
            }
        };
        let session = Arc::new(LoginSession {
            id: login_id.clone(),
            account_id: account.id.clone(),
            provider: account.provider,
            dir,
            hub: Hub::new(LOGIN_COLS, LOGIN_ROWS),
            raw: Mutex::new(RawOutput::default()),
            writer: tokio::sync::Mutex::new(Some(attached.writer)),
            phase: Mutex::new((LoginPhase::Running, None)),
            cancelled: AtomicBool::new(false),
            cancel: Notify::new(),
            finished_at: AtomicI64::new(0),
        });
        let mut masker = self.deps.harness.masker();
        if let Some(snapshot) = &attached.welcome.snapshot {
            session.hub.reset(
                attached.welcome.seq,
                &mask_whole(masker.as_mut(), snapshot),
                LOGIN_COLS,
                LOGIN_ROWS,
            );
        }
        self.logins
            .sessions
            .lock()
            .map_err(AppError::internal)?
            .insert(login_id, session.clone());
        let app = self.clone();
        let pumped = session.clone();
        let reader = attached.reader;
        let seq = attached.welcome.seq;
        tokio::spawn(async move { app.run_login(pumped, reader, seq).await });
        Ok(session)
    }

    fn login_plan(&self, provider: Provider, home: &Path, config_dir: &Path) -> LaunchPlan {
        let path = std::env::var("PATH").unwrap_or_else(|_| "/usr/local/bin:/usr/bin:/bin".into());
        let mut env = vec![
            ("HOME".to_owned(), home.to_string_lossy().into_owned()),
            ("PATH".to_owned(), path),
            ("TERM".to_owned(), "xterm-256color".to_owned()),
            ("COLORTERM".to_owned(), "truecolor".to_owned()),
            ("LANG".to_owned(), "C.UTF-8".to_owned()),
            ("COLUMNS".to_owned(), LOGIN_COLS.to_string()),
            // The sign-in CLI must not try to update itself either (#45).
            ("DISABLE_AUTOUPDATER".to_owned(), "1".to_owned()),
        ];
        match provider {
            Provider::Anthropic => env.push((
                "CLAUDE_CONFIG_DIR".to_owned(),
                config_dir.to_string_lossy().into_owned(),
            )),
        }
        LaunchPlan {
            argv: self.deps.harness.login_argv(provider.harness()),
            cwd: home.to_path_buf(),
            env,
            secret_env: Vec::new(),
            files: Vec::new(),
            containment: Containment::None,
        }
    }

    /// Pumps masked output to viewers until the process ends, then stores the credential.
    async fn run_login(
        self: Arc<Self>,
        session: Arc<LoginSession>,
        mut reader: Box<dyn RunReader>,
        mut seq: u64,
    ) {
        let mut masker = self.deps.harness.masker();
        let deadline = tokio::time::Instant::now() + self.login_timeout();
        let ending = loop {
            tokio::select! {
                () = session.cancel.notified() => break Ending::Cancelled,
                event = tokio::time::timeout_at(deadline, reader.next()) => match event {
                    Err(_) => break Ending::TimedOut,
                    Ok(Ok(Some(event))) => {
                        if let Some(exit) = self.login_event(&session, masker.as_mut(), &mut seq, event) {
                            break Ending::Exited(exit);
                        }
                    }
                    Ok(Ok(None) | Err(_)) => break Ending::Lost,
                },
            }
        };
        if matches!(ending, Ending::TimedOut | Ending::Cancelled) {
            let stopped = self
                .stop_login(&session, masker.as_mut(), &mut seq, reader.as_mut())
                .await;
            tracing::info!(
                exited = stopped.is_some(),
                timed_out = matches!(ending, Ending::TimedOut),
                "login process stopped"
            );
        }
        let ending = match ending {
            Ending::Lost => match self.deps.session_host.inspect(&session.dir) {
                RunInspection::Exited(exit) => Ending::Exited(exit),
                _ => {
                    let _ = self.deps.session_host.terminate(&session.dir);
                    Ending::Lost
                }
            },
            other => other,
        };
        let tail = masker.finish();
        if !tail.is_empty() {
            session.hub.output(seq, &tail);
        }
        *session.writer.lock().await = None;
        self.finish_login(&session, ending).await;
        if session.phase().0 == LoginPhase::Succeeded {
            // A new or repaired subscription: paused tasks may resume on it at once (ROT-010),
            // and its usage is read right away (after viewers got the exit frame).
            self.capacity_changed();
            if let Ok(Some(account)) = self.deps.store.account(&session.account_id).await {
                let _ = self.fetch_and_record_usage(&account).await;
            }
        }
    }

    /// Handles one host event; returns the exit when the process ended.
    fn login_event(
        &self,
        session: &LoginSession,
        masker: &mut dyn OutputMasker,
        seq: &mut u64,
        event: RunEvent,
    ) -> Option<RunExit> {
        match event {
            RunEvent::Output { seq: s, bytes } => {
                *seq = s;
                if let Ok(mut raw) = session.raw.lock() {
                    raw.push(&bytes);
                }
                let masked = masker.push(&bytes);
                if !masked.is_empty() {
                    session.hub.output(s, &masked);
                }
                None
            }
            RunEvent::Snapshot { seq: s, bytes } => {
                *seq = s;
                // The screen already contains any held-back bytes; restart masking after it.
                let _ = masker.finish();
                let mut fresh = self.deps.harness.masker();
                session.hub.reset(
                    s,
                    &mask_whole(fresh.as_mut(), &bytes),
                    LOGIN_COLS,
                    LOGIN_ROWS,
                );
                None
            }
            RunEvent::Exit(exit) => Some(exit),
        }
    }

    /// Asks the process to stop and waits (bounded) for its exit; kills the host otherwise.
    async fn stop_login(
        &self,
        session: &LoginSession,
        masker: &mut dyn OutputMasker,
        seq: &mut u64,
        reader: &mut dyn RunReader,
    ) -> Option<RunExit> {
        if let Some(writer) = session.writer.lock().await.as_mut() {
            let _ = writer.signal(RunSignal::Terminate).await;
            let _ = writer.shutdown(STOP_GRACE_MS).await;
        }
        let deadline = tokio::time::Instant::now() + STOP_WAIT;
        while let Ok(Ok(Some(event))) = tokio::time::timeout_at(deadline, reader.next()).await {
            if let Some(exit) = self.login_event(session, masker, seq, event) {
                return Some(exit);
            }
        }
        if let RunInspection::Exited(exit) = self.deps.session_host.inspect(&session.dir) {
            return Some(exit);
        }
        let _ = self.deps.session_host.terminate(&session.dir);
        None
    }

    /// Extracts and seals the credential (or records the failure), wipes the temporary
    /// directory and tells viewers and the UI.
    async fn finish_login(&self, session: &LoginSession, ending: Ending) {
        let mut raw = session.raw.lock().map(|mut r| r.take()).unwrap_or_default();
        let exit_code = match &ending {
            Ending::Exited(exit) => exit.code,
            _ => None,
        };
        let cancelled =
            matches!(ending, Ending::Cancelled) || session.cancelled.load(Ordering::SeqCst);
        let result: Option<Result<Secret, String>> = if cancelled {
            None
        } else {
            let credential = match session.provider {
                Provider::Anthropic => self
                    .deps
                    .harness
                    .extract_claude_token(&String::from_utf8_lossy(&raw))
                    .map(Secret)
                    .ok_or_else(|| "the sign-in ended without producing a token".to_owned()),
            };
            Some(credential.map_err(|reason| match &ending {
                Ending::TimedOut => format!(
                    "the sign-in timed out after {}",
                    human_duration(self.login_timeout())
                ),
                Ending::Lost => format!("the sign-in process was lost; {reason}"),
                Ending::Exited(exit) if !exit.is_clean() => {
                    format!("{reason} (exit code {:?})", exit.code)
                }
                _ => reason,
            }))
        };
        // Best-effort scrub of the plaintext copy before the buffer is freed.
        raw.fill(0);
        std::hint::black_box(&raw);
        drop(raw);
        let phase = match result {
            None => LoginPhase::Cancelled,
            Some(Ok(secret)) => match self.store_credential(&session.account_id, &secret).await {
                Ok(()) => LoginPhase::Succeeded,
                Err(err) => {
                    tracing::error!(error = %err, "could not store an account credential");
                    self.record_login_failure(
                        &session.account_id,
                        "the credential could not be stored",
                    )
                    .await;
                    LoginPhase::Failed
                }
            },
            Some(Err(reason)) => {
                self.record_login_failure(&session.account_id, &reason)
                    .await;
                LoginPhase::Failed
            }
        };
        let dir = session.dir.clone();
        let _ = tokio::task::spawn_blocking(move || secure_delete(&dir)).await;
        if let Ok(mut p) = session.phase.lock() {
            *p = (phase, exit_code);
        }
        session.finished_at.store(self.now().0, Ordering::SeqCst);
        session.hub.state_changed();
        session.hub.exit(phase.as_str(), exit_code);
        if phase != LoginPhase::Cancelled {
            let _ = self
                .deps
                .store
                .audit(
                    self.now(),
                    "system",
                    if phase == LoginPhase::Succeeded {
                        "account.login_succeeded"
                    } else {
                        "account.login_failed"
                    },
                    Some(session.account_id.as_str()),
                    None,
                )
                .await;
            self.publish_account_id(&session.account_id).await;
        }
    }

    async fn store_credential(&self, account: &AccountId, secret: &Secret) -> AppResult<()> {
        let sealed = self.deps.crypto.seal(secret.expose().as_bytes())?;
        let store = &self.deps.store;
        if store.account(account).await?.is_none() {
            return Err(AppError::NotFound);
        }
        store
            .set_account_credential(account, &sealed, AccountStatus::Ok, self.now())
            .await
    }

    /// Records a failed login. An account that still holds a usable credential keeps its
    /// status (a failed re-login must not take a working account out of rotation).
    async fn record_login_failure(&self, account: &AccountId, reason: &str) {
        let store = &self.deps.store;
        let Ok(Some(record)) = store.account(account).await else {
            return;
        };
        let usable = record.credential.is_some()
            && matches!(record.status, AccountStatus::Ok | AccountStatus::Limited);
        let status = if usable {
            record.status
        } else {
            AccountStatus::Error
        };
        let message = if usable {
            format!("re-login failed: {reason}")
        } else {
            reason.to_owned()
        };
        if let Err(err) = store
            .set_account_state(account, status, None, Some(&message), self.now())
            .await
        {
            tracing::error!(error = %err, "could not record a login failure");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use yulius_domain::time::Timestamp;

    #[test]
    fn raw_output_keeps_the_newest_bytes() {
        let mut raw = RawOutput::default();
        raw.push(&vec![b'a'; LOGIN_RAW_MAX]);
        raw.push(b"tail");
        let bytes = raw.take();
        assert_eq!(bytes.len(), LOGIN_RAW_MAX);
        assert!(bytes.ends_with(b"tail"));
        assert!(raw.take().is_empty());
    }

    #[test]
    fn labels_are_validated() {
        assert_eq!(
            validate_label("  Claude #1 ").ok().as_deref(),
            Some("Claude #1")
        );
        assert!(validate_label("   ").is_err());
        assert!(validate_label("bad\nlabel").is_err());
        assert!(validate_label(&"x".repeat(LABEL_MAX + 1)).is_err());
    }

    #[test]
    fn unknown_usage_stays_null() {
        let empty = usage_view(&AccountUsage::default());
        assert_eq!(empty, AccountUsageView::default());
        let known = usage_view(&AccountUsage {
            primary: Some(UsageWindow {
                used_percent: Some(f64::NAN),
                window_minutes: Some(300),
                resets_at: Some(Timestamp(0)),
            }),
            ..AccountUsage::default()
        });
        let primary = known.primary.as_ref();
        assert_eq!(primary.and_then(|w| w.used_percent), None, "NaN is unknown");
        assert_eq!(
            primary.and_then(|w| w.resets_at.clone()).as_deref(),
            Some("1970-01-01T00:00:00Z")
        );
        assert!(known.secondary.is_none());
    }

    #[test]
    fn secure_delete_removes_files_and_does_not_follow_symlinks()
    -> Result<(), Box<dyn std::error::Error>> {
        let root = std::env::temp_dir().join(format!("yulius-wipe-{}", std::process::id()));
        let outside = root.with_extension("outside");
        std::fs::create_dir_all(root.join("nested"))?;
        std::fs::write(&outside, b"keep me")?;
        std::fs::write(root.join("nested").join("auth.json"), b"secret")?;
        std::os::unix::fs::symlink(&outside, root.join("link"))?;
        secure_delete(&root);
        assert!(!root.exists());
        assert_eq!(std::fs::read(&outside)?, b"keep me");
        std::fs::remove_file(&outside)?;
        Ok(())
    }
}
