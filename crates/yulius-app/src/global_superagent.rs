//! The Global Superagent (SPEC-427, ADR-0271): every person's one Superagent outside every
//! sandbox, which sees every sandbox they can see and keeps them in sync through Yulius.
//!
//! Contract:
//! - one per person, a Cockpit shell marked `global` ([`crate::cockpit::Seat::Global`]):
//!   [`App::global_superagent_open`] returns the running one or starts it, resuming its newest
//!   conversation; it shares the sandbox Superagent's start lock, usage-limit moves and update
//!   refreshes (GSA-001, GSA-005);
//! - it is hosted (registered) by a sandbox its person can work in, which only keeps the shell
//!   machinery (private terminal, idle end, removal); a lost or removed host ends it and the next
//!   open starts it under another, resuming the same conversation (GSA-005);
//! - it is plain Claude Code without built-in tools and with only the Yulius MCP, its folder and
//!   config in `users/<id>/superagent`, which no run binds; contained, in a bubblewrap root of
//!   that folder, without a sandbox holder, repository, agent secrets or egress proxy (GSA-002);
//! - its `gs_` bearer is served the remote MCP's tools for its person in every project, without
//!   [`WITHHELD_TOOLS`], checked per call as an operate token's (GSA-003).

use std::path::PathBuf;
use std::sync::Arc;

use yulius_contracts::mcp::ToolDescriptor;
use yulius_domain::ids::{SandboxId, UserId};
use yulius_domain::permission::{Action, TokenScope};
use yulius_domain::time::Timestamp;

use crate::auth::{AuthContext, TokenGrant};
use crate::cockpit::Seat;
use crate::context::App;
use crate::error::{AppError, AppResult};
use crate::model::{SandboxRecord, Secret, UserRecord};
use crate::ports::harness::CockpitLaunch;
use crate::ports::session::{Containment, RunReader};
use crate::remote::{RemoteCaller, remote_tool_action, remote_tool_descriptors};
use crate::shells::{GLOBAL_TOKEN_PREFIX, ShellInfo, ShellRuntime};

/// Bump when a Global Superagent's launch changes in a way a running one must get: running ones
/// then start again once idle (SPEC-186 SAV-006). Its instructions and tools are in the digest by
/// themselves.
pub(crate) const GLOBAL_LAUNCH_REVISION: u32 = 1;

/// What a person without a sandbox to host it is told (GSA-001).
const NO_HOST: &str = "the Global Superagent needs a sandbox you can work in (write access)";

/// What a person is told when its old session, under a host they lost, could not be ended.
const NOT_MOVED: &str = "The Global Superagent could not move to another sandbox: its process could not be ended, so it keeps running.";

/// The remote tools the Global Superagent is not served (GSA-003): the vault and variables,
/// deployments, and adding, onboarding or skipping projects are the person's decisions.
pub(crate) const WITHHELD_TOOLS: &[&str] = &[
    "secret_update",
    "secret_hosts_answer",
    "secret_delete",
    "variables_set",
    "variable_delete",
    "cd_deploy",
    "project_import",
    "onboarding_start",
    "onboarding_skip",
];

/// The Global Superagent's MCP instructions (GSA-004).
pub const GLOBAL_INSTRUCTIONS: &str = "You are the person's Global Superagent in Yulius: one Claude Code session outside every sandbox that sees every project (sandbox) the person can see and keeps them in sync. You have no shell or file tools: you act only through these Yulius tools, each project tool with `project` (owner/name as projects_list shows it), as the person. Say what runs where: projects_list, then tasks_list, task_get, task_delivery and ci_runs per project. Read across projects with project_files, project_file_read, sessions_search and session_read. Keep them in sync: when a change in one project needs others to follow (an API change and its clients, a shared configuration, the same fix in several repositories), start one task per project with task_create, saying in each prompt what changed where and what this project must do; make an exact small file edit with project_change (a pull request Yulius CI tests; change_merge once it passed); tell a task something with task_message. Work that needs judgment or code is a task in its project, never done here. Follow up with tasks_list and task_delivery and report back. Act only on what the person asks; delete, stop and merge only what they name. Deploying, agent secrets, variables and adding or onboarding projects are the person's, on their pages; quick work inside one sandbox is that sandbox's Superagent's, in its panel.";

/// Whether the Global Superagent is served the remote tool `name`.
pub(crate) fn serves(name: &str) -> bool {
    !WITHHELD_TOOLS.contains(&name)
}

/// The tools the Global Superagent is served (GSA-003): an operate token's, without
/// [`WITHHELD_TOOLS`].
#[must_use]
pub fn global_tool_descriptors() -> Vec<ToolDescriptor> {
    remote_tool_descriptors(TokenScope::Operate)
        .into_iter()
        .filter(|tool| serves(&tool.name))
        .collect()
}

/// Its tools that only read: pre-allowed on an uncontained server, where the others ask first
/// (ADR-0165).
fn global_read_tools() -> Vec<String> {
    global_tool_descriptors()
        .into_iter()
        .filter(|tool| remote_tool_action(&tool.name) == Some(Action::View))
        .map(|tool| tool.name)
        .collect()
}

impl App {
    /// `users/<id>/superagent`: the Global Superagent's own folder, its Claude Code config
    /// included (GSA-002). No run binds it, unlike the person's home.
    pub(crate) fn global_superagent_dir(&self, user: &UserId) -> PathBuf {
        self.users_dir().join(user.as_str()).join("superagent")
    }

    /// The folder it works in.
    pub(crate) fn global_superagent_work(&self, user: &UserId) -> PathBuf {
        self.global_superagent_dir(user).join("work")
    }

    /// [`Self::global_superagent_work`], made (owner-only) when missing.
    ///
    /// # Errors
    /// `Internal` when it cannot be made.
    pub(crate) fn global_superagent_folder(&self, user: &UserId) -> AppResult<PathBuf> {
        let work = self.global_superagent_work(user);
        crate::remote_files::private_dir(&self.global_superagent_dir(user))?;
        crate::remote_files::private_dir(&work)?;
        Ok(work)
    }

    /// The person's running Global Superagent.
    fn running_global_superagent(&self, user: &UserId) -> Option<Arc<ShellRuntime>> {
        self.live_cockpits()
            .into_iter()
            .find(|shell| shell.global && shell.creator() == user)
    }

    /// The Yulius MCP bearer of `user`'s running Global Superagent (tests, diagnostics).
    #[must_use]
    pub fn global_superagent_mcp_token(&self, user: &UserId) -> Option<String> {
        self.running_global_superagent(user)
            .map(|shell| shell.cockpit_token(self.deps.crypto.as_ref()))
    }

    /// The sandbox that hosts `owner`'s Global Superagent: `prefer` while they may work there,
    /// else the oldest live sandbox they can work in (GSA-005).
    ///
    /// # Errors
    /// `Conflict` when there is none.
    pub(crate) async fn global_host(
        &self,
        owner: &UserRecord,
        prefer: Option<&SandboxId>,
    ) -> AppResult<SandboxRecord> {
        if let Some(id) = prefer
            && let Ok((record, _)) = self.require(owner, id, Action::Work).await
        {
            return Ok(record);
        }
        self.visible_sandboxes(owner)
            .await?
            .into_iter()
            .filter(|(sandbox, level)| sandbox.removed_at.is_none() && level.allows(Action::Work))
            .map(|(sandbox, _)| sandbox)
            .min_by(|a, b| (a.created_at.0, a.id.as_str()).cmp(&(b.created_at.0, b.id.as_str())))
            .ok_or_else(|| AppError::Conflict(NO_HOST.into()))
    }

    /// `POST /api/superagent`: the caller's Global Superagent, started (resuming its newest
    /// conversation) when it is not running, or when its host is no longer a sandbox they can
    /// work in (GSA-001, GSA-005).
    ///
    /// # Errors
    /// `Conflict` without a sandbox they can work in or when the old session could not be
    /// ended, `AccountRequired` without a usable Claude account, `Capacity` while none has
    /// capacity, `Upstream` when the session cannot start.
    pub async fn global_superagent_open(
        self: &Arc<Self>,
        ctx: &AuthContext,
    ) -> AppResult<ShellInfo> {
        if let Some(running) = self.running_global_superagent(&ctx.user.id)
            && self
                .require(&ctx.user, running.sandbox(), Action::Work)
                .await
                .is_ok()
        {
            return Ok(running.info());
        }
        // Detached from the request, as the sandbox Superagent's start (SPEC-077 DRS-001).
        let app = Arc::clone(self);
        let user = ctx.user.clone();
        Self::run_detached(async move { app.start_requested_global(&user).await }).await
    }

    async fn start_requested_global(self: &Arc<Self>, user: &UserRecord) -> AppResult<ShellInfo> {
        let _starting = self.cockpit_start_slot(&user.id, Seat::Global).await?;
        if let Some(running) = self.running_global_superagent(&user.id) {
            if self
                .require(user, running.sandbox(), Action::Work)
                .await
                .is_ok()
            {
                return Ok(running.info());
            }
            // Its host is no longer one its person works in: it ends there and starts under
            // another, resuming its conversation.
            tracing::info!(shell = %running.id(), user = %user.id, "the Global Superagent moves to another host sandbox");
            if !self.end_shells(std::slice::from_ref(&running)).await {
                return Err(AppError::Conflict(NOT_MOVED.into()));
            }
        }
        let host = self.global_host(user, None).await?;
        self.start_on_account(user, &host, true).await
    }

    /// Launches `user`'s Global Superagent in `runtime` (GSA-002): only the Yulius MCP, no
    /// built-in tools, its own folder and config, contained in a root of its own.
    pub(crate) async fn launch_global_superagent(
        &self,
        user: &UserRecord,
        runtime: &ShellRuntime,
        credential: Secret,
        continue_interrupted: bool,
    ) -> AppResult<Box<dyn RunReader>> {
        let dir = self.global_superagent_dir(&user.id);
        let config = dir.join(".claude");
        let folder = PathBuf::from(runtime.info().folder);
        let newest = match self
            .deps
            .harness
            .newest_claude_session(&dir, &folder, Timestamp(0))
        {
            Ok(found) => found.map(|(id, _)| id),
            Err(err) => {
                tracing::warn!(error = %err, user = %user.id, "global superagent conversations unreadable; starting a fresh one");
                None
            }
        };
        let unresumable = self
            .shells
            .cockpit_unresumable
            .lock()
            .ok()
            .and_then(|failed| failed.get(&(user.id.clone(), Seat::Global)).cloned());
        let resume = newest.filter(|id| Some(id) != unresumable.as_ref());
        let bearer = runtime.cockpit_token(self.deps.crypto.as_ref());
        debug_assert!(bearer.starts_with(GLOBAL_TOKEN_PREFIX));
        let mut plan = self.deps.harness.plan_cockpit(&CockpitLaunch {
            cwd: folder.clone(),
            sandbox_dir: dir.clone(),
            config_dir: config.clone(),
            config_dir_host: config,
            resume: resume.clone(),
            continue_interrupted: continue_interrupted && resume.is_some(),
            credential,
            yulius: Some((self.config.internal_url.clone(), Secret(bearer))),
            project_mcp_servers: Vec::new(),
            mcp: Vec::new(),
            contained: self.config.contained,
            only_yulius: Some(global_read_tools()),
        })?;
        if let Some(id) = resume {
            let _ = runtime.resumed.set((id, self.now()));
        }
        if let (Some(program), Some(first)) =
            (self.shells.cockpit_program.get(), plan.argv.first_mut())
        {
            *first = program.to_string_lossy().into_owned();
        }
        let login = crate::identity::sandbox_login(user);
        let path = if self.config.contained {
            crate::isolation::SANDBOX_PATH.to_owned()
        } else {
            std::env::var("PATH").unwrap_or_else(|_| "/usr/local/bin:/usr/bin:/bin".into())
        };
        let mut env: Vec<(String, String)> = vec![
            ("HOME".into(), dir.to_string_lossy().into_owned()),
            ("USER".into(), login.clone()),
            ("LOGNAME".into(), login),
            ("PATH".into(), path),
            ("TERM".into(), "xterm-256color".into()),
            ("COLORTERM".into(), "truecolor".into()),
            ("LANG".into(), "C.UTF-8".into()),
        ];
        env.retain(|(key, _)| !plan.env.iter().any(|(k, _)| k == key));
        env.append(&mut plan.env);
        plan.env = env;
        plan.containment = self.global_containment(&dir);
        let reader = self.spawn_shell(runtime, &plan).await?;
        self.save_shell(runtime)?;
        Ok(reader)
    }

    /// Its containment on a contained server (GSA-002): a bubblewrap root of its own folder
    /// `dir`, without a sandbox holder, identity, repository or the person's home (and tests).
    #[must_use]
    pub fn global_containment(&self, dir: &std::path::Path) -> Containment {
        if !self.config.contained {
            return Containment::None;
        }
        Containment::Bubblewrap {
            sandbox_root: dir.to_path_buf(),
            home: dir.to_path_buf(),
            writable: Vec::new(),
            readonly: self.config.contained_readonly.clone(),
            binds: Vec::new(),
            identity: None,
            join: None,
        }
    }

    /// Resolves a Global Superagent's bearer to a caller of the remote tools acting for its
    /// person as an operate token would (GSA-003): the session must be live, its person enabled
    /// and still able to work in some sandbox, checked at every call.
    ///
    /// # Errors
    /// `Unauthenticated` otherwise.
    pub async fn global_superagent_caller(&self, token: &str) -> AppResult<RemoteCaller> {
        if !token.starts_with(GLOBAL_TOKEN_PREFIX) {
            return Err(AppError::Unauthenticated);
        }
        let crypto = self.deps.crypto.as_ref();
        // Compared as hashes, so the time taken says nothing about a token's characters.
        let wanted = crypto.sha256_hex(token.as_bytes());
        let shell = self
            .live_cockpits()
            .into_iter()
            .filter(|shell| shell.global)
            .find(|shell| crypto.sha256_hex(shell.cockpit_token(crypto).as_bytes()) == wanted)
            .ok_or(AppError::Unauthenticated)?;
        let user = self
            .deps
            .store
            .user(shell.creator())
            .await?
            .filter(|u| u.disabled_at.is_none())
            .ok_or(AppError::Unauthenticated)?;
        self.global_host(&user, Some(shell.sandbox()))
            .await
            .map_err(|_| AppError::Unauthenticated)?;
        let login = self.login_of(&user.id).await;
        let id = shell.id().to_owned();
        Ok(RemoteCaller {
            ctx: AuthContext {
                user,
                csrf_token: None,
                session_hash: None,
                token: Some(TokenGrant {
                    id: format!("superagent:{id}"),
                    scope: TokenScope::Operate,
                }),
            },
            token_id: format!("superagent:{id}"),
            scope: TokenScope::Operate,
            login,
            superagent: Some(id),
        })
    }
}
