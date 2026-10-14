//! Agent secrets copied into a project's `.env` files (SPEC-129, ADR-0105). A project keeps its
//! keys in a git-ignored `.env`, named without values in the tracked `.env.example`. When the
//! project's own code needs a secret's real value (it runs outside the egress proxy), the agent
//! asks Yulius to copy it there: the agent only ever sees `[secret:NAME]`, so it cannot write the
//! value itself. A value in a file leaves the secret's bound hosts, so a person who manages the
//! secret allows each secret once, as they allow its hosts (ADR-0098).

use std::path::{Path, PathBuf};

use serde::Deserialize;
use serde_json::{Value, json};
use yulius_contracts::api::secrets::{AgentSecretView, EnvFilesAnswer};
use yulius_domain::redact::is_env_file;
use yulius_domain::task::Activity;
use yulius_domain::vault::dotenv;
use zeroize::Zeroizing;

use super::requests::{reason, requester};
use super::{AgentPayload, invalid};
use crate::auth::AuthContext;
use crate::context::App;
use crate::coordination::AgentContext;
use crate::error::{AppError, AppResult};
use crate::files::{inside, io_error, relative_components};
use crate::ports::vault::{EnvFiles, VaultSecretRecord};

/// Most secrets copied in one call.
const MAX_SECRETS: usize = 16;
/// Largest dotenv file Yulius rewrites (it is read whole and written back whole).
const MAX_ENV_BYTES: u64 = 256 * 1024;
/// The tracked template next to every dotenv file.
const EXAMPLE: &str = ".env.example";

/// Arguments of `secret_copy_to_env`.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CopyArgs {
    secrets: Vec<String>,
    #[serde(default)]
    path: Option<String>,
    #[serde(default)]
    reason: Option<String>,
}

/// A dotenv file of the worktree: its folder (resolved, inside the worktree) and names.
struct EnvTarget {
    folder: PathBuf,
    file_name: String,
    relative: String,
    folder_relative: String,
}

impl EnvTarget {
    fn path(&self) -> PathBuf {
        self.folder.join(&self.file_name)
    }

    fn example_relative(&self) -> String {
        if self.folder_relative.is_empty() {
            EXAMPLE.to_owned()
        } else {
            format!("{}/{EXAMPLE}", self.folder_relative)
        }
    }
}

/// Resolves `relative` (a dotenv file name, no template) inside `worktree`, whose folder must
/// exist there.
fn env_target(worktree: &Path, relative: &str) -> AppResult<EnvTarget> {
    let parts = relative_components(relative)?;
    let (file_name, folders) = parts
        .split_last()
        .ok_or_else(|| AppError::Invalid("name the dotenv file, like .env".into()))?;
    if !is_env_file(file_name) {
        return Err(AppError::Invalid(format!(
            "{relative} is not a dotenv file: copy into .env (or .env.local, prod.env); templates \
             like {EXAMPLE} never hold values"
        )));
    }
    let folder_relative = folders.join("/");
    let folder = std::fs::canonicalize(worktree.join(&folder_relative)).map_err(|_| {
        AppError::Invalid(format!(
            "the folder of {relative} does not exist in the worktree"
        ))
    })?;
    if !inside(worktree, &folder) || !folder.is_dir() {
        return Err(AppError::Forbidden(format!(
            "{relative} is outside the task's worktree"
        )));
    }
    Ok(EnvTarget {
        folder,
        file_name: file_name.clone(),
        relative: parts.join("/"),
        folder_relative,
    })
}

/// The text of a file of the worktree, `None` when it does not exist; refused when it is a link
/// or not a regular file, or too large to rewrite.
fn read_existing(worktree: &Path, path: &Path, relative: &str) -> AppResult<Option<String>> {
    let meta = match std::fs::symlink_metadata(path) {
        Ok(meta) => meta,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(io_error(&err)),
    };
    if !meta.is_file() {
        return Err(AppError::Forbidden(format!(
            "{relative} is a link or not a regular file: Yulius writes only regular files"
        )));
    }
    if meta.len() > MAX_ENV_BYTES {
        return Err(AppError::Invalid(format!(
            "{relative} is larger than {} KiB",
            MAX_ENV_BYTES / 1024
        )));
    }
    crate::hooks::read_repo_doc(worktree, relative)
        .map(Some)
        .ok_or_else(|| AppError::Forbidden(format!("{relative} cannot be read as a regular file")))
}

/// Writes `text` to `folder/name` whole: a new file with `mode` beside it, renamed into place (a
/// link swapped in meanwhile is replaced, never followed).
fn write_whole(folder: &Path, name: &str, text: &str, mode: u32, nonce: &str) -> AppResult<()> {
    use std::io::Write as _;
    use std::os::unix::fs::OpenOptionsExt as _;
    let tmp = folder.join(format!(".{name}.yulius-{nonce}"));
    let written = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(mode)
        .custom_flags(nix::libc::O_NOFOLLOW)
        .open(&tmp)
        .and_then(|mut file| {
            file.write_all(text.as_bytes())?;
            file.sync_all()
        })
        .and_then(|()| std::fs::rename(&tmp, folder.join(name)));
    if let Err(err) = written {
        let _ = std::fs::remove_file(&tmp);
        return Err(io_error(&err));
    }
    Ok(())
}

impl App {
    /// `POST /api/agent-secrets/{id}/env`: a person who manages the secret allows agents to copy
    /// it into `.env` files (answering their request), dismisses the request, or revokes the
    /// allowance. Compare and set on what was read, so a concurrent answer is never undone.
    ///
    /// # Errors
    /// `NotFound` for invisible secrets, `Forbidden` for people who cannot manage it,
    /// `Conflict` when nothing waits to be allowed or dismissed (or it changed meanwhile).
    pub async fn answer_env_files(
        &self,
        ctx: &AuthContext,
        id: &str,
        answer: &EnvFilesAnswer,
    ) -> AppResult<AgentSecretView> {
        let mut record = self.agent_record(ctx, id).await?;
        self.require_manage_agent(ctx, record.sandbox_id.as_ref())
            .await?;
        let action = match (record.env_files, answer.allow) {
            (EnvFiles::Asked, true) => "vault.agent.env_allow",
            (EnvFiles::Asked, false) => "vault.agent.env_dismiss",
            (EnvFiles::Allowed, false) => "vault.agent.env_revoke",
            (EnvFiles::Allowed, true) => return self.agent_view(&record, true).await,
            (EnvFiles::No, _) => {
                return Err(AppError::Conflict(
                    "no agent asks to copy this secret into .env files".into(),
                ));
            }
        };
        let to = if answer.allow {
            EnvFiles::Allowed
        } else {
            EnvFiles::No
        };
        let by = record.env_requested_by.take();
        if !self
            .deps
            .store
            .vault_set_env_files(id, (record.env_files, by.as_deref()), (to, None))
            .await?
        {
            return Err(AppError::Conflict(
                "the secret changed meanwhile: reload and answer again".into(),
            ));
        }
        record.env_files = to;
        self.vault_audit(
            &Self::vault_actor(ctx),
            action,
            Some(id),
            &match by {
                Some(by) => format!("{} → .env files (asked by {by})", record.name),
                None => format!("{} → .env files", record.name),
            },
        )
        .await?;
        self.agent_view(&record, true).await
    }
}

impl App {
    /// `secret_copy_to_env`: copies the values of agent secrets of the task's project into a
    /// dotenv file of its worktree that Git ignores (default `.env`), and their names into the
    /// `.env.example` beside it. All or nothing: a secret no person has allowed is asked for
    /// instead (the task then needs a person) and nothing is written.
    pub(crate) async fn tool_secret_copy_to_env(
        &self,
        ctx: &AgentContext,
        args: &Value,
    ) -> AppResult<Value> {
        let args: CopyArgs = serde_json::from_value(args.clone())
            .map_err(|e| AppError::Invalid(format!("invalid arguments: {e}")))?;
        let mut names: Vec<String> = Vec::new();
        for name in &args.secrets {
            let name = name.trim().to_owned();
            if !names.contains(&name) {
                names.push(name);
            }
        }
        if names.is_empty() || names.len() > MAX_SECRETS {
            return Err(AppError::Invalid(format!(
                "name 1 to {MAX_SECRETS} secrets from secrets_list"
            )));
        }
        let store = &self.deps.store;
        let all = store.vault_agent(Some(&ctx.task.sandbox_id)).await?;
        let mut records: Vec<VaultSecretRecord> = Vec::with_capacity(names.len());
        for name in &names {
            let record = all.iter().find(|r| &r.name == name).ok_or_else(|| {
                AppError::Invalid(format!(
                    "no agent secret named {name:?} applies to this project (see secrets_list)"
                ))
            })?;
            records.push(record.clone());
        }
        let relative = args.path.as_deref().map_or(".env", str::trim);
        let sandbox = store
            .sandbox(&ctx.task.sandbox_id)
            .await?
            .ok_or(AppError::NotFound)?;
        let worktree = self
            .sandbox_worktree(&sandbox, ctx.task.id.as_str(), None)
            .await?;
        let target = env_target(&worktree, relative)?;

        let waiting: Vec<&VaultSecretRecord> = records
            .iter()
            .filter(|r| r.env_files != EnvFiles::Allowed)
            .collect();
        if !waiting.is_empty() {
            let why = reason(args.reason.as_deref());
            for record in &waiting {
                self.ask_env_files(ctx, record, &sandbox.slug, &why).await?;
            }
            if let Some(task) = store.task(&ctx.task.id).await?
                && let Err(err) = self
                    .set_task_activity(&task, Activity::NeedsAttention)
                    .await
            {
                tracing::warn!(task = %task.id, error = %err, "could not flag a .env request");
            }
            let waiting: Vec<&str> = waiting.iter().map(|r| r.name.as_str()).collect();
            return Ok(json!({
                "copied": [],
                "waiting": waiting,
                "link": self.agent_secrets_link(&sandbox.slug),
                "next": format!(
                    "Nothing was written. A person who manages {} allows copying it into .env files on the Secrets page: print the link so they can, go on with other work, and call secret_copy_to_env again once secrets_list shows env_files allowed.",
                    waiting.join(", ")
                ),
            }));
        }

        let path = target.path();
        let existing = read_existing(&worktree, &path, &target.relative)?;
        if !self
            .deps
            .git
            .is_disposable(&worktree, &target.relative)
            .await?
        {
            return Err(AppError::Forbidden(format!(
                "Git does not ignore {} (or tracks it), so its values could be committed: add \
                 .env and .env.* to .gitignore with !.env.example, commit that, and copy again",
                target.relative
            )));
        }
        let mut entries = Vec::with_capacity(records.len());
        for record in &records {
            let payload: AgentPayload = self.open_payload(record)?;
            let line = dotenv::line(&record.name, &payload.value).map_err(invalid)?;
            entries.push((record.name.clone(), line));
        }
        let text = Zeroizing::new(dotenv::upsert(
            existing.as_deref().unwrap_or_default(),
            &entries,
        ));
        drop(Zeroizing::new(entries));
        let nonce = crate::auth::hex_token(&self.deps.crypto.random_bytes()[..8]);
        write_whole(&target.folder, &target.file_name, &text, 0o600, &nonce)?;
        drop(text);

        let (example_added, warning) = self.add_example_names(&worktree, &target, &names).await;
        for record in &records {
            self.note_agent_use(
                ctx.attempt.id.as_str(),
                ctx.task.number,
                record,
                "vault.agent.env",
                &target.relative,
            )
            .await;
        }
        let mut result = json!({
            "copied": names,
            "file": target.relative,
            "example": target.example_relative(),
            "example_added": example_added,
            "next": format!(
                "{} holds the real values; Git ignores it. Never edit those lines yourself: you only see [secret:NAME], and writing that back replaces the value. Commit {} (names, no values). After a value is rotated, copy it again.",
                target.relative,
                target.example_relative()
            ),
        });
        if let Some(warning) = warning {
            result["warning"] = Value::String(warning);
        }
        Ok(result)
    }

    /// Records that `ctx`'s task asks to copy `record` into `.env` files (compare and set; a
    /// repeated ask by the same task changes nothing).
    async fn ask_env_files(
        &self,
        ctx: &AgentContext,
        record: &VaultSecretRecord,
        slug: &str,
        why: &str,
    ) -> AppResult<()> {
        let asked = record.env_files == EnvFiles::Asked;
        let by = requester(
            record.env_requested_by.as_deref(),
            asked,
            ctx.task.number,
            record.sandbox_id.is_none().then_some(slug),
        );
        if asked && record.env_requested_by.as_deref() == Some(by.as_str()) {
            return Ok(());
        }
        if !self
            .deps
            .store
            .vault_set_env_files(
                &record.id,
                (record.env_files, record.env_requested_by.as_deref()),
                (EnvFiles::Asked, Some(&by)),
            )
            .await?
        {
            return Err(AppError::Conflict(
                "the secret changed meanwhile: ask again".into(),
            ));
        }
        self.vault_audit(
            &format!("task:{}", ctx.task.number),
            "vault.agent.env_request",
            Some(&record.id),
            &if why.is_empty() {
                format!("{} → .env files", record.name)
            } else {
                format!("{} → .env files ({why})", record.name)
            },
        )
        .await
    }

    /// Adds `NAME=` to the `.env.example` beside `target` for each name it lacks (created when
    /// missing, left alone when it is not a regular file); the names added and a warning when it
    /// cannot be written or Git ignores it. Never fails the copy that already happened.
    async fn add_example_names(
        &self,
        worktree: &Path,
        target: &EnvTarget,
        names: &[String],
    ) -> (Vec<String>, Option<String>) {
        use std::os::unix::fs::PermissionsExt as _;
        let relative = target.example_relative();
        let path = target.folder.join(EXAMPLE);
        let existing = match read_existing(worktree, &path, &relative) {
            Ok(existing) => existing,
            Err(err) => return (Vec::new(), Some(format!("{relative} was not changed: {err}"))),
        };
        let (text, added) = dotenv::example(existing.as_deref().unwrap_or_default(), names);
        if !added.is_empty() {
            let mode = std::fs::metadata(&path).map_or(0o644, |m| m.permissions().mode() & 0o777);
            let nonce = crate::auth::hex_token(&self.deps.crypto.random_bytes()[..8]);
            if let Err(err) = write_whole(&target.folder, EXAMPLE, &text, mode, &nonce) {
                return (Vec::new(), Some(format!("{relative} was not changed: {err}")));
            }
        }
        let ignored = self
            .deps
            .git
            .is_disposable(worktree, &relative)
            .await
            .unwrap_or(false);
        let warning = ignored.then(|| {
            format!("Git ignores {relative}: add !.env.example to .gitignore so it is committed")
        });
        (added, warning)
    }
}
