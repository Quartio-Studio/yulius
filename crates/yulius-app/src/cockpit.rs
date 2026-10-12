//! The Cockpit: every user's permanent Claude Code session in a sandbox, for quick questions and
//! work that should not become a task (SPEC-021, ADR-0019).
//!
//! Contract:
//! - one Cockpit per user and sandbox, for users with write access; [`App::cockpit_open`] returns
//!   the running one or starts it, and concurrent calls start one (CKP-001, CKP-005);
//! - it is plain Claude Code: no Yulius hooks, MCP server, settings, briefing or gates, the
//!   token of a pool subscription in its environment, its config in the user's own home, run in
//!   the sandbox repository as the user's sandbox identity (CKP-002);
//! - a Cockpit started again resumes the user's newest conversation there, so `/clear`,
//!   `/exit`, an idle stop and a lost process never lose it (CKP-003);
//! - it is a shell of kind [`ShellKind::Cockpit`]: private, adopted after a restart and ended
//!   after a day without viewers like a terminal, but never listed, renamed or closed as one
//!   (CKP-004, CKP-006).

use std::path::PathBuf;
use std::sync::Arc;

use yulius_domain::harness::Provider;
use yulius_domain::ids::{SandboxId, UserId};
use yulius_domain::permission::Action;
use yulius_domain::time::Timestamp;

use crate::auth::AuthContext;
use crate::context::App;
use crate::error::{AppError, AppResult};
use crate::model::SandboxRecord;
use crate::ports::harness::CockpitLaunch;
use crate::shells::{ShellInfo, ShellKind, ShellRuntime};

/// The Cockpit's display name.
pub const COCKPIT_NAME: &str = "Cockpit";

impl App {
    /// Configures the Cockpit's program instead of `claude` (tests); the first call wins.
    /// Returns whether it was applied.
    pub fn set_cockpit_program(&self, program: PathBuf) -> bool {
        self.shells.cockpit_program.set(program).is_ok()
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
        let _starting = self.shells.cockpit_start.lock().await;
        if let Some(running) = self.running_cockpit(&ctx.user.id, &record.id) {
            return Ok(running.info());
        }
        let repo = self.sandbox_repo(&record.id);
        if !repo.is_dir() {
            return Err(AppError::Conflict(
                "the sandbox repository is not cloned yet".into(),
            ));
        }
        let account = self.pool_account(Provider::Anthropic).await?;
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
            .launch_cockpit(ctx, &record, &runtime, credential)
            .await
        {
            Ok(reader) => {
                self.register_shell(&runtime)?;
                self.start_pump(&runtime, reader);
                tracing::info!(shell = %id, sandbox = %record.slug, user = %ctx.user.id, account = %account.id, "cockpit started");
                Ok(runtime.info())
            }
            Err(err) => {
                self.discard_shell(&runtime);
                Err(err)
            }
        }
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
        // The newest conversation of this user here: after `/clear`, the cleared one's successor.
        let resume = self
            .deps
            .harness
            .newest_claude_session(&space.home_host, &folder, Timestamp(0))
            .ok()
            .flatten()
            .map(|(id, _)| id);
        let mut plan = self.deps.harness.plan_cockpit(&CockpitLaunch {
            cwd: folder,
            config_dir: space.home.join(".claude"),
            config_dir_host: space.home_host.join(".claude"),
            resume,
            credential,
        })?;
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
