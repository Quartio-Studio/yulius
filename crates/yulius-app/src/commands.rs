//! Brokered commands (SPEC-626, ADR-0459): a project's CLIs that need an agent secret as a file.
//! Administrators define them next to the project's secrets; agents and the Superagent list
//! them, and task agents run them by name outside their sandbox.
//!
//! - BRK-001: [`validated`]; BRK-002: [`App::create_command`], [`App::update_command`],
//!   [`App::delete_command`], [`App::commands`]; BRK-003: [`App::command_secret`];
//! - BRK-004: [`App::tool_commands_list`].

use serde_json::{Value, json};
use yulius_contracts::api::commands::{CommandDefinition, CommandList, CommandView};
use yulius_contracts::events::UiEvent;
use yulius_domain::commands as dom;
use yulius_domain::ids::SandboxId;
use yulius_domain::permission::Action;

use crate::auth::{AuthContext, rfc3339};
use crate::context::App;
use crate::coordination::ProjectCaller;
use crate::error::{AppError, AppResult};
use crate::model::SandboxRecord;
use crate::ports::store_ext::CommandRecord;
use crate::ports::vault::VaultSecretRecord;
use crate::vault::Holder;

/// Most commands a project may have.
pub const MAX_COMMANDS: usize = 100;

/// A list's entries checked by `check` and bounded by [`dom::MAX_ENTRIES`] and
/// [`dom::MAX_ENTRY`].
fn each(list: &[String], what: &str, check: impl Fn(&str) -> Option<String>) -> AppResult<()> {
    if list.len() > dom::MAX_ENTRIES {
        return Err(AppError::Invalid(format!(
            "{what} has at most {} entries",
            dom::MAX_ENTRIES
        )));
    }
    for entry in list {
        if entry.len() > dom::MAX_ENTRY {
            return Err(AppError::Invalid(format!(
                "an entry of {what} is at most {} bytes",
                dom::MAX_ENTRY
            )));
        }
        if let Some(reason) = check(entry) {
            return Err(AppError::Invalid(format!("{what}: {reason}")));
        }
    }
    Ok(())
}

/// The definition, trimmed and checked (BRK-001).
///
/// # Errors
/// `Invalid` with the first reason it is refused.
pub fn validated(mut def: CommandDefinition) -> AppResult<CommandDefinition> {
    def.name = def.name.trim().to_owned();
    def.binary = def.binary.trim().to_owned();
    def.secret = def.secret.trim().to_owned();
    def.description = def.description.trim().to_owned();
    let refuse = |reason: Option<String>| reason.map_or(Ok(()), |r| Err(AppError::Invalid(r)));
    refuse(dom::name_refusal(&def.name))?;
    refuse(dom::binary_refusal(&def.binary))?;
    if def.description.len() > dom::MAX_DESCRIPTION || def.description.contains('\0') {
        return Err(AppError::Invalid(format!(
            "the description is at most {} bytes",
            dom::MAX_DESCRIPTION
        )));
    }
    if yulius_domain::vault::SecretName::parse(&def.secret).is_err() {
        return Err(AppError::Invalid(
            "the secret is the NAME of an agent secret of this project".into(),
        ));
    }
    if dom::Approve::parse(&def.approve).is_none() {
        return Err(AppError::Invalid("approve is always, send or never".into()));
    }
    if !(1..=dom::MAX_APPROVAL_MINUTES).contains(&def.approval_minutes) {
        return Err(AppError::Invalid(format!(
            "a run waits 1 to {} minutes for its approval",
            dom::MAX_APPROVAL_MINUTES
        )));
    }
    if !(1..=dom::MAX_TIMEOUT_SECONDS).contains(&def.timeout_seconds) {
        return Err(AppError::Invalid(format!(
            "a run's time limit is 1 to {} seconds",
            dom::MAX_TIMEOUT_SECONDS
        )));
    }
    if def.tasks.len() > dom::MAX_ENTRIES || def.tasks.contains(&0) {
        return Err(AppError::Invalid(format!(
            "tasks are at most {} task numbers",
            dom::MAX_ENTRIES
        )));
    }
    def.tasks.sort_unstable();
    def.tasks.dedup();
    each(&def.fixed, "fixed", |v| dom::placeholder_refusal(v, false))?;
    each(&def.subcommands, "subcommands", dom::subcommand_refusal)?;
    each(&def.refuse, "refuse", dom::flag_refusal)?;
    each(&def.send_flags, "send_flags", dom::flag_refusal)?;
    if def.env.len() > dom::MAX_ENTRIES {
        return Err(AppError::Invalid(format!(
            "env has at most {} variables",
            dom::MAX_ENTRIES
        )));
    }
    for (name, value) in &def.env {
        refuse(dom::env_name_refusal(name))?;
        if let Some(reason) = dom::placeholder_refusal(value, true) {
            return Err(AppError::Invalid(format!("env {name}: {reason}")));
        }
    }
    if def.approve == "send" && def.send_flags.is_empty() {
        // Without a flag the CLI acts on, only the agent's word would ask (BRK-013).
        return Err(AppError::Invalid(
            "approve send needs the flags that make the command change something (send_flags)"
                .into(),
        ));
    }
    Ok(def)
}

fn parsed(record: &CommandRecord) -> AppResult<CommandDefinition> {
    serde_json::from_str(&record.definition).map_err(AppError::internal)
}

impl App {
    /// The agent secret a command's runs get (BRK-003): resolved as for a run that holds nobody
    /// (the project's, else a global one switched on here; never a personal one), and never a
    /// production secret. `None` when there is none.
    ///
    /// # Errors
    /// Store failures.
    pub(crate) async fn command_secret(
        &self,
        sandbox: &SandboxId,
        name: &str,
    ) -> AppResult<Option<VaultSecretRecord>> {
        Ok(self
            .resolved_agent_secrets(sandbox, Holder::Nobody)
            .await?
            .into_iter()
            .find(|r| r.name == name && !r.cd_prod))
    }

    /// Refuses a definition naming a production secret of the project (BRK-003).
    async fn refuse_production_secret(
        &self,
        sandbox: &SandboxId,
        def: &CommandDefinition,
    ) -> AppResult<()> {
        let production = self
            .resolved_agent_secrets(sandbox, Holder::Nobody)
            .await?
            .iter()
            .any(|r| r.name == def.secret && r.cd_prod);
        if production {
            return Err(AppError::Invalid(format!(
                "{} is a production secret, which reaches no run: copy it into an agent secret \
                 for the command",
                def.secret
            )));
        }
        Ok(())
    }

    async fn command_view(&self, record: &CommandRecord, can_edit: bool) -> AppResult<CommandView> {
        let definition = parsed(record)?;
        let secret_ready = self
            .command_secret(&record.sandbox_id, &definition.secret)
            .await?
            .is_some();
        let created_by = self.login_of_id(&record.created_by).await;
        let updated_by = self.login_of_id(&record.updated_by).await;
        Ok(CommandView {
            id: record.id.clone(),
            definition,
            secret_ready,
            created_by,
            created_at: rfc3339(record.created_at),
            updated_by,
            updated_at: rfc3339(record.updated_at),
            can_edit,
        })
    }

    /// Whether `ctx` administers the project: it changes the project's commands (BRK-002).
    async fn commands_access(
        &self,
        ctx: &AuthContext,
        sid: &str,
        action: Action,
    ) -> AppResult<(SandboxRecord, bool)> {
        let sid = SandboxId::parse(sid).map_err(|_| AppError::NotFound)?;
        let (sandbox, level) = self.require_for(ctx, &sid, action).await?;
        let can_edit = level.allows(Action::Administer)
            && ctx
                .scope()
                .is_none_or(|scope| scope.allows(Action::Administer));
        Ok((sandbox, can_edit))
    }

    /// `GET /api/sandboxes/{sid}/commands` (BRK-002): for those who may view the project.
    ///
    /// # Errors
    /// Access errors.
    pub async fn commands(&self, ctx: &AuthContext, sid: &str) -> AppResult<CommandList> {
        let (sandbox, can_edit) = self.commands_access(ctx, sid, Action::View).await?;
        let mut items = Vec::new();
        for record in self.deps.store.commands_in(&sandbox.id).await? {
            items.push(self.command_view(&record, can_edit).await?);
        }
        Ok(CommandList { items, can_edit })
    }

    /// `POST /api/sandboxes/{sid}/commands` (BRK-002): an administrator adds a command; audited
    /// `command.define`.
    ///
    /// # Errors
    /// Access errors, `Invalid` for a refused definition, `Conflict` for a name taken.
    pub async fn create_command(
        &self,
        ctx: &AuthContext,
        sid: &str,
        def: CommandDefinition,
    ) -> AppResult<CommandView> {
        let (sandbox, _) = self.commands_access(ctx, sid, Action::Administer).await?;
        let def = validated(def)?;
        self.refuse_production_secret(&sandbox.id, &def).await?;
        if self.deps.store.commands_in(&sandbox.id).await?.len() >= MAX_COMMANDS {
            return Err(AppError::Conflict(format!(
                "a project has at most {MAX_COMMANDS} commands"
            )));
        }
        let now = self.now();
        let record = CommandRecord {
            id: self.new_command_id(),
            sandbox_id: sandbox.id.clone(),
            name: def.name.clone(),
            definition: serde_json::to_string(&def).map_err(AppError::internal)?,
            created_by: ctx.user.id.as_str().to_owned(),
            created_at: now,
            updated_by: ctx.user.id.as_str().to_owned(),
            updated_at: now,
        };
        self.deps.store.insert_command(&record).await?;
        self.audit_command(ctx, &sandbox, "command.define", &record.definition)
            .await;
        self.announce_commands(&sandbox.id);
        self.command_view(&record, true).await
    }

    /// `PUT /api/sandboxes/{sid}/commands/{id}` (BRK-002): an administrator replaces a
    /// command's definition; audited `command.update`.
    ///
    /// # Errors
    /// As [`App::create_command`]; `NotFound` for another project's or a removed command,
    /// `Conflict` when it changed meanwhile.
    pub async fn update_command(
        &self,
        ctx: &AuthContext,
        sid: &str,
        id: &str,
        def: CommandDefinition,
    ) -> AppResult<CommandView> {
        let (sandbox, _) = self.commands_access(ctx, sid, Action::Administer).await?;
        let def = validated(def)?;
        self.refuse_production_secret(&sandbox.id, &def).await?;
        let current = self
            .deps
            .store
            .command(&sandbox.id, id)
            .await?
            .ok_or(AppError::NotFound)?;
        let record = CommandRecord {
            name: def.name.clone(),
            definition: serde_json::to_string(&def).map_err(AppError::internal)?,
            updated_by: ctx.user.id.as_str().to_owned(),
            // Later than the version read, so the next conditional write tells them apart.
            updated_at: self.now().max(current.updated_at.plus_millis(1)),
            ..current.clone()
        };
        if !self
            .deps
            .store
            .update_command(&record, current.updated_at)
            .await?
        {
            return Err(AppError::Conflict(
                "the command changed meanwhile: reload and edit again".into(),
            ));
        }
        self.audit_command(ctx, &sandbox, "command.update", &record.definition)
            .await;
        self.announce_commands(&sandbox.id);
        self.command_view(&record, true).await
    }

    /// `DELETE /api/sandboxes/{sid}/commands/{id}` (BRK-002): audited `command.remove`.
    ///
    /// # Errors
    /// Access errors, `NotFound`.
    pub async fn delete_command(&self, ctx: &AuthContext, sid: &str, id: &str) -> AppResult<()> {
        let (sandbox, _) = self.commands_access(ctx, sid, Action::Administer).await?;
        let record = self
            .deps
            .store
            .command(&sandbox.id, id)
            .await?
            .ok_or(AppError::NotFound)?;
        if !self.deps.store.delete_command(&sandbox.id, id).await? {
            return Err(AppError::NotFound);
        }
        self.audit_command(ctx, &sandbox, "command.remove", &record.name)
            .await;
        self.announce_commands(&sandbox.id);
        Ok(())
    }

    fn new_command_id(&self) -> String {
        let hex: String = self
            .deps
            .crypto
            .random_bytes()
            .iter()
            .take(8)
            .map(|b| format!("{b:02x}"))
            .collect();
        format!("cmd-{hex}")
    }

    pub(crate) fn announce_commands(&self, sandbox: &SandboxId) {
        self.events.publish(
            Some(sandbox.clone()),
            false,
            UiEvent::CommandsUpdated {
                sandbox_id: sandbox.as_str().to_owned(),
            },
        );
    }

    /// Audits a change of the project's commands: a definition holds no value (BRK-002).
    async fn audit_command(
        &self,
        ctx: &AuthContext,
        sandbox: &SandboxRecord,
        action: &str,
        detail: &str,
    ) {
        let login = self.login_of(&ctx.user.id).await;
        if let Err(err) = self
            .deps
            .store
            .audit(
                self.now(),
                &login,
                action,
                Some(sandbox.id.as_str()),
                Some(detail),
            )
            .await
        {
            tracing::warn!(error = %err, action, "a command's audit entry was not written");
        }
    }

    /// `commands_list` (BRK-004), for a task's agent and the Superagent: the project's commands
    /// as defined (placeholders unexpanded, no value), and whether the caller's task may run
    /// each.
    ///
    /// # Errors
    /// Store failures.
    pub(crate) async fn tool_commands_list(&self, caller: &ProjectCaller) -> AppResult<Value> {
        let mut commands = Vec::new();
        for record in self.deps.store.commands_in(&caller.sandbox).await? {
            let def = parsed(&record)?;
            let ready = self
                .command_secret(&caller.sandbox, &def.secret)
                .await?
                .is_some();
            let may_run = caller
                .task
                .as_ref()
                .map(|task| ready && (def.tasks.is_empty() || def.tasks.contains(&task.number)));
            commands.push(json!({
                "name": def.name,
                "description": def.description,
                "binary": def.binary,
                "fixed": def.fixed,
                "subcommands": def.subcommands,
                "refuse": def.refuse,
                "slashes": def.slashes,
                "env": def.env.keys().collect::<Vec<_>>(),
                "approve": def.approve,
                "send_flags": def.send_flags,
                "approval_minutes": def.approval_minutes,
                "timeout_seconds": def.timeout_seconds,
                "secret": def.secret,
                "secret_ready": ready,
                "tasks": def.tasks,
                "you_may_run": may_run,
            }));
        }
        Ok(json!({
            "commands": commands,
            "note": "A command runs outside your sandbox with its secret as a file you never see. \
                     Task agents run one with command_run or `yulius run NAME ARGS...`; no \
                     argument may name a path (/ or ~).",
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn def() -> CommandDefinition {
        serde_json::from_value(json!({
            "name": "deploy",
            "binary": "/usr/local/bin/deploy",
            "secret": "DEPLOY_KEY",
        }))
        .unwrap_or_else(|e| panic!("{e}"))
    }

    #[test]
    fn defaults_and_a_valid_definition() {
        let d = def();
        assert_eq!(d.approve, "always");
        assert_eq!(d.approval_minutes, 10);
        assert_eq!(d.timeout_seconds, 600);
        assert!(validated(d).is_ok());
    }

    #[test]
    fn refused_definitions() {
        let cases: Vec<(Value, &str)> = vec![
            (json!({"binary": "deploy"}), "absolute"),
            (json!({"name": "De ploy"}), "name"),
            (json!({"secret": "not a name"}), "secret"),
            (json!({"approve": "maybe"}), "approve"),
            (json!({"approve": "send"}), "send_flags"),
            (json!({"approval_minutes": 0}), "minutes"),
            (json!({"approval_minutes": 61}), "minutes"),
            (json!({"timeout_seconds": 1801}), "time limit"),
            (json!({"fixed": ["--token={secret}"]}), "only in env"),
            (json!({"fixed": ["{url:api}"]}), "not a placeholder"),
            (json!({"env": {"LD_PRELOAD": "/x"}}), "set by Yulius"),
            (json!({"env": {"PATH": "/x"}}), "set by Yulius"),
            (json!({"refuse": ["key-file"]}), "not a flag"),
            (json!({"subcommands": ["a/b"]}), "not a subcommand"),
            (json!({"tasks": [0]}), "task numbers"),
        ];
        for (patch, says) in cases {
            let mut value = serde_json::to_value(def()).unwrap_or_default();
            if let (Some(obj), Some(p)) = (value.as_object_mut(), patch.as_object()) {
                for (k, v) in p {
                    obj.insert(k.clone(), v.clone());
                }
            }
            let d: CommandDefinition =
                serde_json::from_value(value).unwrap_or_else(|e| panic!("{e}"));
            let err = validated(d).err().map(|e| e.public_message());
            assert!(
                err.as_deref().is_some_and(|e| e.contains(says)),
                "{patch}: {err:?}"
            );
        }
    }

    #[test]
    fn unknown_fields_are_refused() {
        let parsed: Result<CommandDefinition, _> = serde_json::from_value(json!({
            "name": "x", "binary": "/x", "secret": "X", "key": "inline value",
        }));
        assert!(parsed.is_err());
    }
}
