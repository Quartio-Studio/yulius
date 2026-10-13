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
//!   (CKP-004, CKP-006).

use std::path::PathBuf;
use std::sync::Arc;

use serde_json::json;

use yulius_domain::harness::Provider;
use yulius_domain::ids::{SandboxId, UserId};
use yulius_domain::permission::Action;
use yulius_domain::time::Timestamp;

use crate::auth::AuthContext;
use crate::context::App;
use crate::coordination::ProjectCaller;
use crate::error::{AppError, AppResult};
use crate::model::SandboxRecord;
use crate::ports::harness::CockpitLaunch;
use crate::shells::{COCKPIT_TOKEN_PREFIX, ShellInfo, ShellKind, ShellRuntime};

/// The Cockpit's display name.
pub const COCKPIT_NAME: &str = "Cockpit";
/// A resumed Cockpit that fails within this time could not resume its conversation.
const QUICK_EXIT_MS: i64 = 10_000;

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
        self.shells.cockpit_start.lock().map_or(0, |starts| starts.len())
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
        let ctx = ctx.clone();
        Self::run_detached(async move { app.start_cockpit(&ctx, &record).await }).await
    }

    /// Starts the caller's Cockpit unless another start of theirs in that sandbox just did.
    async fn start_cockpit(
        self: &Arc<Self>,
        ctx: &AuthContext,
        record: &SandboxRecord,
    ) -> AppResult<ShellInfo> {
        let _starting = self.cockpit_start_slot(&ctx.user.id, &record.id).await?;
        if let Some(running) = self.running_cockpit(&ctx.user.id, &record.id) {
            return Ok(running.info());
        }
        let repo = self.sandbox_repo(&record.id);
        if !repo.is_dir() {
            return Err(AppError::Conflict(
                "the sandbox repository is not cloned yet".into(),
            ));
        }
        // The Cockpit runs the default model, Opus 5.5: its weekly Opus cap binds (SPEC-023).
        let account = self
            .pool_account(
                Provider::Anthropic,
                yulius_domain::usage::ModelFamily::of(Some(
                    yulius_domain::harness::DEFAULT_CLAUDE_MODEL,
                )),
            )
            .await?;
        let credential = self.open_credential(&account)?;
        let id = self.new_shell_id();
        let runtime = self.new_shell_runtime(
            &id,
            ShellKind::Cockpit,
            &record.id,
            &ctx.user.id,
            None,
            &repo,
            COCKPIT_NAME.to_owned(),
        );
        match self
            .launch_cockpit(ctx, record, &runtime, credential)
            .await
        {
            Ok(reader) => match self.register_shell(&runtime) {
                Ok(()) => {
                    self.start_pump(&runtime, reader);
                    tracing::info!(shell = %id, sandbox = %record.slug, user = %ctx.user.id, account = %account.id, "cockpit started");
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
        ctx: &AuthContext,
        record: &SandboxRecord,
        runtime: &ShellRuntime,
        credential: crate::model::Secret,
    ) -> AppResult<Box<dyn crate::ports::session::RunReader>> {
        let (join, lease) = self.isolation_join(record).await?.unzip();
        let space = self.user_space(&ctx.user, record, join)?;
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
                tracing::warn!(error = %err, user = %ctx.user.id, "cockpit conversations unreadable; starting a fresh one");
                None
            }
        };
        let unresumable = self
            .shells
            .cockpit_unresumable
            .lock()
            .ok()
            .and_then(|failed| {
                failed
                    .get(&(ctx.user.id.clone(), record.id.clone()))
                    .cloned()
            });
        let resume = newest.filter(|id| Some(id) != unresumable.as_ref());
        let mut plan = self.deps.harness.plan_cockpit(&CockpitLaunch {
            cwd: folder,
            config_dir: space.home.join(".claude"),
            config_dir_host: space.home_host.join(".claude"),
            resume: resume.clone(),
            credential,
            yulius: Some((
                self.config.internal_url.clone(),
                crate::model::Secret(runtime.cockpit_token(self.deps.crypto.as_ref())),
            )),
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
        let mut env = self.shell_env(record, &space, &self.shell_program());
        env.retain(|(key, _)| !plan.env.iter().any(|(k, _)| k == key));
        env.append(&mut plan.env);
        plan.env = env;
        plan.containment = space.containment.clone();
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
