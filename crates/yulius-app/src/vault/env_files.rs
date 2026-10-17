//! Agent secrets copied into a project's `.env` files (SPEC-129, ADR-0105). A project keeps its
//! keys in a git-ignored `.env`, named without values in the tracked `.env.example`. When the
//! project's own code needs a secret's real value (it runs outside the egress proxy), the agent
//! asks Yulius to copy it there: the agent only ever sees `[secret:NAME]`, so it cannot write the
//! value itself. A value in a file leaves the secret's bound hosts, so a person who manages the
//! secret allows each secret once, as they allow its hosts (ADR-0098).
//!
//! Yulius runs outside the agent's containment, and the agent can rename any folder of its
//! sandbox at any moment. So the target's folder is opened once, one component at a time without
//! following links, its real path is checked to be inside the worktree, and every read and write
//! goes through that handle (`openat`, `renameat`): a folder swapped for a link afterwards is
//! never followed.

use std::io::{Read as _, Write as _};
use std::os::fd::OwnedFd;
use std::path::{Path, PathBuf};

use nix::errno::Errno;
use nix::fcntl::{OFlag, openat, renameat};
use nix::sys::stat::Mode;
use serde::Deserialize;
use serde_json::{Value, json};
use yulius_contracts::api::secrets::{AgentSecretView, EnvFilesAnswer};
use yulius_domain::redact::is_env_file;
use yulius_domain::vault::dotenv;
use zeroize::Zeroizing;

use super::requests::{reason, requester};
use super::{AgentPayload, SecretUser, invalid};
use crate::auth::AuthContext;
use crate::context::App;
use crate::coordination::AgentContext;
use crate::error::{AppError, AppResult};
use crate::files::{inside, relative_components};
use crate::ports::vault::{EnvFiles, VaultSecretRecord};

/// Most secrets copied in one call.
const MAX_SECRETS: usize = 16;
/// Largest dotenv file Yulius rewrites (it is read whole and written back whole).
const MAX_ENV_BYTES: u64 = 256 * 1024;
/// The tracked template next to every dotenv file.
const EXAMPLE: &str = ".env.example";
/// Flags of every folder opened on the way to the target.
const DIR_FLAGS: OFlag = OFlag::O_RDONLY
    .union(OFlag::O_DIRECTORY)
    .union(OFlag::O_NOFOLLOW)
    .union(OFlag::O_CLOEXEC);

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

/// A folder of the worktree, opened once: its handle and its real path below the worktree
/// (`""` for the worktree itself).
struct Folder {
    fd: OwnedFd,
    relative: String,
}

impl Folder {
    /// The worktree-relative path of `name` in this folder.
    fn path_of(&self, name: &str) -> String {
        if self.relative.is_empty() {
            name.to_owned()
        } else {
            format!("{}/{name}", self.relative)
        }
    }
}

fn os_error(err: Errno) -> AppError {
    crate::files::io_error(&std::io::Error::from(err))
}

/// Where an open folder really is.
fn real_path(fd: &OwnedFd) -> AppResult<PathBuf> {
    #[cfg(target_os = "linux")]
    {
        use std::os::fd::AsRawFd as _;
        std::fs::read_link(format!("/proc/self/fd/{}", fd.as_raw_fd()))
            .map_err(|e| crate::files::io_error(&e))
    }
    #[cfg(any(target_os = "macos", target_os = "ios"))]
    {
        let mut path = PathBuf::new();
        nix::fcntl::fcntl(fd, nix::fcntl::FcntlArg::F_GETPATH(&mut path)).map_err(os_error)?;
        Ok(path)
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "ios")))]
    {
        let _ = fd;
        Err(AppError::Forbidden(
            "copying into .env files needs Linux or macOS".into(),
        ))
    }
}

/// Opens `folders` below `worktree` one component at a time without following links, and checks
/// that the folder reached is inside the worktree and out of `.git`.
fn open_folder(worktree: &Path, folders: &[String]) -> AppResult<Folder> {
    let shown = || folders.join("/");
    let refused = |err: Errno| match err {
        Errno::ELOOP | Errno::ENOTDIR => AppError::Forbidden(format!(
            "a folder on the way to {} is a link or not a folder: Yulius follows no links",
            shown()
        )),
        Errno::ENOENT => AppError::Invalid(format!(
            "the folder {} does not exist in the folder you work in",
            shown()
        )),
        other => os_error(other),
    };
    let mut fd = nix::fcntl::open(worktree, DIR_FLAGS, Mode::empty()).map_err(refused)?;
    for part in folders {
        fd = openat(&fd, part.as_str(), DIR_FLAGS, Mode::empty()).map_err(refused)?;
    }
    let real = real_path(&fd)?;
    if !inside(worktree, &real) {
        return Err(AppError::Forbidden(format!(
            "{} is outside the folder you work in",
            shown()
        )));
    }
    let relative = real
        .strip_prefix(worktree)
        .map_err(|_| AppError::Forbidden("outside the folder you work in".into()))?
        .components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join("/");
    Ok(Folder { fd, relative })
}

/// The text of `name` in `folder`, `None` when it does not exist; refused when it is a link or
/// not a regular file, or too large to rewrite. Opened without blocking, so a FIFO cannot hang.
fn read_in(folder: &Folder, name: &str) -> AppResult<Option<Zeroizing<String>>> {
    let shown = folder.path_of(name);
    let flags = OFlag::O_RDONLY | OFlag::O_NOFOLLOW | OFlag::O_NONBLOCK | OFlag::O_CLOEXEC;
    let fd = match openat(&folder.fd, name, flags, Mode::empty()) {
        Ok(fd) => fd,
        Err(Errno::ENOENT) => return Ok(None),
        Err(Errno::ELOOP) => {
            return Err(AppError::Forbidden(format!(
                "{shown} is a link: Yulius writes only regular files"
            )));
        }
        Err(err) => return Err(os_error(err)),
    };
    let file = std::fs::File::from(fd);
    let meta = file.metadata().map_err(|e| crate::files::io_error(&e))?;
    if !meta.is_file() {
        return Err(AppError::Forbidden(format!(
            "{shown} is not a regular file: Yulius writes only regular files"
        )));
    }
    if meta.len() > MAX_ENV_BYTES {
        return Err(AppError::Invalid(format!(
            "{shown} is larger than {} KiB",
            MAX_ENV_BYTES / 1024
        )));
    }
    // Reserved up front, so reading never leaves a reallocated copy of the old keys behind.
    let mut text = Zeroizing::new(String::with_capacity(
        usize::try_from(meta.len()).unwrap_or(0) + 1,
    ));
    file.take(MAX_ENV_BYTES)
        .read_to_string(&mut text)
        .map_err(|_| AppError::Invalid(format!("{shown} is not UTF-8 text")))?;
    Ok(Some(text))
}

/// How a file is written: through a temporary file Git ignores, renamed into place, or (when Git
/// would not ignore any temporary name, so a crash could leave a committable copy) in place.
enum Writing {
    Replace(String),
    InPlace,
}

/// Writes `text` to `name` in `folder` with `mode`, never following a link.
fn write_in(folder: &Folder, name: &str, text: &str, mode: u32, how: &Writing) -> AppResult<()> {
    let mode = Mode::from_bits_truncate(mode);
    let flags = OFlag::O_WRONLY | OFlag::O_NOFOLLOW | OFlag::O_CLOEXEC;
    let write = |fd: OwnedFd| -> AppResult<()> {
        let mut file = std::fs::File::from(fd);
        let meta = file.metadata().map_err(|e| crate::files::io_error(&e))?;
        if !meta.is_file() {
            return Err(AppError::Forbidden(format!(
                "{} is not a regular file",
                folder.path_of(name)
            )));
        }
        nix::sys::stat::fchmod(&file, mode).map_err(os_error)?;
        file.set_len(0)
            .and_then(|()| file.write_all(text.as_bytes()))
            .and_then(|()| file.sync_all())
            .map_err(|e| crate::files::io_error(&e))
    };
    match how {
        Writing::Replace(tmp) => {
            let fd = openat(
                &folder.fd,
                tmp.as_str(),
                flags | OFlag::O_CREAT | OFlag::O_EXCL,
                mode,
            )
            .map_err(os_error)?;
            let done = write(fd).and_then(|()| {
                renameat(&folder.fd, tmp.as_str(), &folder.fd, name).map_err(os_error)
            });
            if done.is_err() {
                let _ = nix::unistd::unlinkat(
                    &folder.fd,
                    tmp.as_str(),
                    nix::unistd::UnlinkatFlags::NoRemoveDir,
                );
            }
            done
        }
        Writing::InPlace => {
            let fd = openat(
                &folder.fd,
                name,
                flags | OFlag::O_CREAT | OFlag::O_NONBLOCK,
                mode,
            )
            .map_err(|err| match err {
                Errno::ELOOP => AppError::Forbidden(format!("{} is a link", folder.path_of(name))),
                other => os_error(other),
            })?;
            write(fd)
        }
    }
}

impl App {
    /// `POST /api/agent-secrets/{id}/env`: a person who manages the secret allows agents to copy
    /// it into `.env` files (answering their request), dismisses the request, or revokes the
    /// allowance. The answer names the state and askers the person saw: it is refused unless they
    /// still hold, and it changes them in one compare-and-set step.
    ///
    /// # Errors
    /// `NotFound` for invisible secrets, `Forbidden` for people who cannot manage it,
    /// `Conflict` when nothing waits to be allowed or dismissed or it changed since the person
    /// looked, `Invalid` for an unknown state.
    pub async fn answer_env_files(
        &self,
        ctx: &AuthContext,
        id: &str,
        answer: &EnvFilesAnswer,
    ) -> AppResult<AgentSecretView> {
        let mut record = self.agent_record(ctx, id).await?;
        self.require_manage_agent(ctx, record.sandbox_id.as_ref())
            .await?;
        let seen = EnvFiles::parse(&answer.env_files)
            .ok_or_else(|| AppError::Invalid("env_files is no, asked or allowed".into()))?;
        let seen_by = answer.env_requested_by.as_deref();
        if (seen, seen_by) != (record.env_files, record.env_requested_by.as_deref()) {
            return Err(AppError::Conflict(
                "the secret changed since you looked: reload and answer again".into(),
            ));
        }
        if answer.allow && record.cd_prod {
            return Err(AppError::Conflict(super::PRODUCTION_SECRET.into()));
        }
        let action = match (seen, answer.allow) {
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
        if !self
            .deps
            .store
            .vault_set_env_files(id, (seen, seen_by), (to, None))
            .await?
        {
            return Err(AppError::Conflict(
                "the secret changed since you looked: reload and answer again".into(),
            ));
        }
        record.env_files = to;
        let by = record.env_requested_by.take();
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

    /// `POST /api/agent-secrets/{id}/cd-prod`: a person who manages the secret allows production
    /// CD runs to get its value, or revokes it (SPEC-134). Apart from `env_files`: an allowance
    /// for `.env` files lets any agent of the project use the value, which production must not.
    ///
    /// # Errors
    /// `NotFound` for invisible secrets, `Forbidden` for people who cannot manage it.
    pub async fn answer_cd_prod(
        &self,
        ctx: &AuthContext,
        id: &str,
        allow: bool,
    ) -> AppResult<AgentSecretView> {
        let mut record = self.agent_record(ctx, id).await?;
        self.require_manage_agent(ctx, record.sandbox_id.as_ref())
            .await?;
        // A secret agents can use (its `.env` copies, its hosts) is no production secret
        // (round 2, M-B).
        if allow && (record.env_files == EnvFiles::Allowed || !record.hosts.is_empty()) {
            return Err(AppError::Conflict(
                "agents can use this secret (copies into .env files or hosts the proxy sends it \
                 to): revoke those, or add a separate secret for production deploys"
                    .into(),
            ));
        }
        if record.cd_prod != allow {
            if !self
                .deps
                .store
                .vault_set_cd_prod(id, record.cd_prod, allow)
                .await?
            {
                return Err(AppError::Conflict(
                    "the secret changed since you looked: reload and answer again".into(),
                ));
            }
            record.cd_prod = allow;
            self.vault_audit(
                &Self::vault_actor(ctx),
                if allow {
                    "vault.agent.cd_prod_allow"
                } else {
                    "vault.agent.cd_prod_revoke"
                },
                Some(id),
                &format!("{} → production CD runs", record.name),
            )
            .await?;
        }
        self.agent_view(&record, true).await
    }

    /// `secret_copy_to_env`: copies the values of agent secrets of the caller's project into a
    /// dotenv file that Git ignores (default `.env`) of a task's worktree or of the sandbox
    /// repository a Superagent runs in (SPEC-186 SAV-002), and their names into the
    /// `.env.example` beside it. All or nothing: everything that would refuse the copy is checked
    /// first; then a secret no person has allowed is asked for instead (a task needs a person)
    /// and nothing is written.
    pub(crate) async fn tool_secret_copy_to_env(
        &self,
        user: SecretUser<'_>,
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
        let all = store.vault_agent(Some(user.sandbox())).await?;
        let mut records: Vec<VaultSecretRecord> = Vec::with_capacity(names.len());
        for name in &names {
            let record = all.iter().find(|r| &r.name == name).ok_or_else(|| {
                AppError::Invalid(format!(
                    "no agent secret named {name:?} applies to this project (see secrets_list)"
                ))
            })?;
            if record.cd_prod {
                return Err(AppError::Forbidden(format!(
                    "{name}: {}",
                    super::PRODUCTION_SECRET
                )));
            }
            records.push(record.clone());
        }

        // The target: a dotenv file (no template) in a folder of the worktree.
        let relative = args.path.as_deref().map_or(".env", str::trim);
        let parts = relative_components(relative)?;
        let (file_name, folders) = parts
            .split_last()
            .ok_or_else(|| AppError::Invalid("name the dotenv file, like .env".into()))?;
        if !is_env_file(file_name) {
            return Err(AppError::Invalid(format!(
                "{relative} is not a dotenv file: copy into .env (or .env.local, prod.env); \
                 templates like {EXAMPLE} never hold values"
            )));
        }
        let sandbox = store
            .sandbox(user.sandbox())
            .await?
            .ok_or(AppError::NotFound)?;
        let worktree = self.secret_user_folder(user, &sandbox).await?;
        let folder = open_folder(&worktree, folders)?;
        let target = folder.path_of(file_name);
        let existing = read_in(&folder, file_name)?;
        if !self.deps.git.is_disposable(&worktree, &target).await? {
            return Err(AppError::Forbidden(format!(
                "Git does not ignore {target} (or tracks it), so its values could be committed: \
                 add .env and .env.* to .gitignore with !.env.example, commit that, and copy again"
            )));
        }
        let mut entries = Zeroizing::new(Vec::with_capacity(records.len()));
        for record in &records {
            let payload: AgentPayload = self.open_payload(record)?;
            let line = dotenv::line(&record.name, &payload.value).map_err(invalid)?;
            entries.push((record.name.clone(), line));
        }
        let text = Zeroizing::new(
            dotenv::upsert(existing.as_ref().map_or("", |t| t.as_str()), &entries)
                .map_err(invalid)?,
        );
        drop(entries);

        // Only then: a secret no person allowed yet is asked for, and nothing is written.
        let waiting: Vec<&VaultSecretRecord> = records
            .iter()
            .filter(|r| r.env_files != EnvFiles::Allowed)
            .collect();
        if !waiting.is_empty() {
            let why = reason(args.reason.as_deref());
            for record in &waiting {
                self.ask_env_files(user, record, &sandbox.slug, &why)
                    .await?;
            }
            self.flag_secret_wait(user, "a .env request").await?;
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

        drop(existing);
        let how = self.writing(&worktree, &folder, file_name).await;
        write_in(&folder, file_name, &text, 0o600, &how)?;
        drop(text);

        let (example_added, warning) = self.add_example_names(&worktree, &folder, &names).await;
        for record in &records {
            self.note_agent_use(
                (&user.run(), &user.actor()),
                record,
                "vault.agent.env",
                &target,
            )
            .await;
        }
        let example = folder.path_of(EXAMPLE);
        let mut result = json!({
            "copied": names,
            "file": target,
            "example": example,
            "example_added": example_added,
            "next": format!(
                "{target} holds the real values; Git ignores it. Never edit those lines yourself: you only see [secret:NAME], and writing that back replaces the value. Commit {example} (names, no values). After a value is rotated, copy it again."
            ),
        });
        if let Some(warning) = warning {
            result["warning"] = Value::String(warning);
        }
        Ok(result)
    }

    /// How to write `name` in `folder`: through a temporary name Git ignores (so a crash leaves
    /// nothing committable), else in place.
    async fn writing(&self, worktree: &Path, folder: &Folder, name: &str) -> Writing {
        let nonce = crate::auth::hex_token(&self.deps.crypto.random_bytes()[..8]);
        for tmp in [
            format!("{name}.yulius-{nonce}"),
            format!(".yulius-{nonce}.{name}"),
        ] {
            if self
                .deps
                .git
                .is_disposable(worktree, &folder.path_of(&tmp))
                .await
                .unwrap_or(false)
            {
                return Writing::Replace(tmp);
            }
        }
        Writing::InPlace
    }

    /// Records that `user` (a task, a Superagent) asks to copy `record` into `.env` files
    /// (compare and set; a repeated ask by the same asker changes nothing).
    pub(crate) async fn ask_env_files(
        &self,
        user: SecretUser<'_>,
        record: &VaultSecretRecord,
        slug: &str,
        why: &str,
    ) -> AppResult<()> {
        let asked = record.env_files == EnvFiles::Asked;
        let by = requester(
            record.env_requested_by.as_deref(),
            asked,
            &user.asker(),
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
        self.announce_request(record);
        self.vault_audit(
            &user.actor(),
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

    /// Adds `NAME=` to the `.env.example` in `folder` for each name it lacks (created when
    /// missing, left alone when it is not a regular file); the names added and a warning when it
    /// cannot be written or Git ignores it. Never fails the copy that already happened.
    async fn add_example_names(
        &self,
        worktree: &Path,
        folder: &Folder,
        names: &[String],
    ) -> (Vec<String>, Option<String>) {
        let relative = folder.path_of(EXAMPLE);
        let existing = match read_in(folder, EXAMPLE) {
            Ok(existing) => existing,
            Err(err) => {
                return (
                    Vec::new(),
                    Some(format!("{relative} was not changed: {err}")),
                );
            }
        };
        let (text, added) = dotenv::example(existing.as_ref().map_or("", |t| t.as_str()), names);
        let ignored = self
            .deps
            .git
            .is_disposable(worktree, &relative)
            .await
            .unwrap_or(false);
        if !added.is_empty() {
            // The template keeps the mode it was committed with; a new one is 0644.
            #[allow(
                clippy::useless_conversion,
                reason = "mode_t is u32 on Linux but u16 on macOS"
            )]
            let mode = nix::sys::stat::fstatat(
                &folder.fd,
                EXAMPLE,
                nix::fcntl::AtFlags::AT_SYMLINK_NOFOLLOW,
            )
            .map_or(0o644, |st| u32::from(st.st_mode) & 0o777);
            let how = self.writing(worktree, folder, EXAMPLE).await;
            if let Err(err) = write_in(folder, EXAMPLE, &text, mode, &how) {
                return (
                    Vec::new(),
                    Some(format!("{relative} was not changed: {err}")),
                );
            }
        }
        let warning = ignored.then(|| {
            format!("Git ignores {relative}: add !.env.example to .gitignore so it is committed")
        });
        (added, warning)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    fn worktree() -> Result<(tempfile::TempDir, PathBuf, PathBuf), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let root = std::fs::canonicalize(dir.path())?;
        let wt = root.join("wt");
        let outside = root.join("outside");
        std::fs::create_dir_all(wt.join("deploy"))?;
        std::fs::create_dir_all(&outside)?;
        Ok((dir, wt, outside))
    }

    /// Review of SPEC-129, H1: a folder swapped for a link after it was opened is never
    /// followed: the read and the write stay in the folder that was checked.
    #[test]
    fn a_folder_swapped_for_a_link_after_the_check_is_not_followed() -> TestResult {
        let (_dir, wt, outside) = worktree()?;
        std::fs::write(outside.join(".env"), "OTHER_PROJECT_KEY=theirs\n")?;
        let folder = open_folder(&wt, &["deploy".to_owned()])?;
        assert_eq!(folder.relative, "deploy");
        std::fs::rename(wt.join("deploy"), wt.join("moved"))?;
        std::os::unix::fs::symlink(&outside, wt.join("deploy"))?;
        assert_eq!(read_in(&folder, ".env")?.map(|t| t.to_string()), None);
        write_in(&folder, ".env", "K=mine\n", 0o600, &Writing::InPlace)?;
        let tmp = Writing::Replace(".env.yulius-1".into());
        write_in(&folder, ".env.local", "K=mine\n", 0o600, &tmp)?;
        assert_eq!(
            std::fs::read_to_string(outside.join(".env"))?,
            "OTHER_PROJECT_KEY=theirs\n"
        );
        assert!(!outside.join(".env.local").exists());
        assert_eq!(std::fs::read_to_string(wt.join("moved/.env"))?, "K=mine\n");
        assert_eq!(
            std::fs::read_to_string(wt.join("moved/.env.local"))?,
            "K=mine\n"
        );
        assert!(!wt.join("moved/.env.yulius-1").exists());
        Ok(())
    }

    /// A link on the way, or a folder whose real path leaves the worktree, is refused.
    #[test]
    fn links_on_the_way_are_refused() -> TestResult {
        let (_dir, wt, outside) = worktree()?;
        std::os::unix::fs::symlink(&outside, wt.join("link"))?;
        std::os::unix::fs::symlink(wt.join("deploy"), wt.join("inner"))?;
        for folders in [vec!["link".to_owned()], vec!["inner".to_owned()]] {
            assert!(
                matches!(open_folder(&wt, &folders), Err(AppError::Forbidden(_))),
                "{folders:?}"
            );
        }
        assert!(matches!(
            open_folder(&wt, &["missing".to_owned()]),
            Err(AppError::Invalid(_))
        ));
        std::fs::write(wt.join("deploy/target"), "x")?;
        std::os::unix::fs::symlink(wt.join("deploy/target"), wt.join("deploy/.env"))?;
        let folder = open_folder(&wt, &["deploy".to_owned()])?;
        assert!(matches!(
            read_in(&folder, ".env"),
            Err(AppError::Forbidden(_))
        ));
        assert!(write_in(&folder, ".env", "K=v\n", 0o600, &Writing::InPlace).is_err());
        assert_eq!(std::fs::read_to_string(wt.join("deploy/target"))?, "x");
        Ok(())
    }
}

/// Where a secret a CD environment names stands (SPEC-134 CDR-002).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CdSecretState {
    /// A person allowed it for this environment: a CD run gets its value.
    Ready,
    /// It exists; no person allowed it for this environment yet.
    NotAllowed,
    /// No agent secret of the project has this name.
    Missing,
    /// A production secret, named by dev: dev never gets it.
    Production,
}

impl CdSecretState {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Ready => "ready",
            Self::NotAllowed => "not_allowed",
            Self::Missing => "missing",
            Self::Production => "production",
        }
    }
}

/// Whether a run of `env` may get `record`'s value (SPEC-134, ADR-0109): dev runs the secrets a
/// person allowed outside the proxy (`env_files`, ADR-0105: dev is the agents' own ground, as
/// `.env` is), production only those a person allowed for production deploys (`cd_prod`), which
/// no agent can use otherwise.
fn allowed_for(record: &VaultSecretRecord, env: yulius_domain::cd::CdEnv) -> bool {
    match env {
        yulius_domain::cd::CdEnv::Dev => record.env_files == EnvFiles::Allowed,
        yulius_domain::cd::CdEnv::Prod => record.cd_prod,
    }
}

impl App {
    /// The state of each secret `names` for runs of `env` (SPEC-134 CDT-001).
    pub(crate) async fn cd_secret_states(
        &self,
        sandbox: &yulius_domain::ids::SandboxId,
        env: yulius_domain::cd::CdEnv,
        names: &[String],
    ) -> AppResult<Vec<(String, CdSecretState)>> {
        let all = self.deps.store.vault_agent(Some(sandbox)).await?;
        Ok(names
            .iter()
            .map(|name| {
                let state = match all.iter().find(|r| &r.name == name) {
                    None => CdSecretState::Missing,
                    Some(r) if r.cd_prod && env == yulius_domain::cd::CdEnv::Dev => {
                        CdSecretState::Production
                    }
                    Some(r) if allowed_for(r, env) => CdSecretState::Ready,
                    Some(_) => CdSecretState::NotAllowed,
                };
                (name.clone(), state)
            })
            .collect())
    }

    /// The values of the secrets a CD run of `env` needs (SPEC-134 CDR-002): each must be an
    /// agent secret of the project allowed for `env` ([`allowed_for`]). Otherwise nothing is
    /// returned: for dev, a secret that exists is asked for when an agent (`asker`) wants the run
    /// (as `secret_copy_to_env` does); the refusal says what a person must do.
    ///
    /// # Errors
    /// `Conflict` naming the missing or not-allowed secrets; store or decryption failures.
    pub(crate) async fn cd_secret_values(
        &self,
        sandbox: &crate::model::SandboxRecord,
        env: yulius_domain::cd::CdEnv,
        names: &[String],
        asker: Option<&AgentContext>,
        why: &str,
    ) -> AppResult<Vec<(String, String)>> {
        let all = self.deps.store.vault_agent(Some(&sandbox.id)).await?;
        let link = self.agent_secrets_link(&sandbox.slug);
        let missing: Vec<&str> = names
            .iter()
            .filter(|name| !all.iter().any(|r| &r.name == *name))
            .map(String::as_str)
            .collect();
        if !missing.is_empty() {
            return Err(AppError::Conflict(format!(
                "nothing ran: {} {} not an agent secret of this project. A person adds {} on the \
                 Secrets page ({link}); an agent asks one with needs_you",
                missing.join(", "),
                if missing.len() == 1 { "is" } else { "are" },
                if missing.len() == 1 { "it" } else { "them" },
            )));
        }
        let records: Vec<&VaultSecretRecord> = names
            .iter()
            .filter_map(|name| all.iter().find(|r| &r.name == name))
            .collect();
        if env == yulius_domain::cd::CdEnv::Dev
            && let Some(production) = records.iter().find(|r| r.cd_prod)
        {
            return Err(AppError::Conflict(format!(
                "nothing ran: {} is a production secret, which dev deploys (agents' ground) never \
                 get; dev names a secret of its own",
                production.name
            )));
        }
        let waiting: Vec<&VaultSecretRecord> = records
            .iter()
            .copied()
            .filter(|r| !allowed_for(r, env))
            .collect();
        if !waiting.is_empty() {
            let names: Vec<&str> = waiting.iter().map(|r| r.name.as_str()).collect();
            if env == yulius_domain::cd::CdEnv::Prod {
                return Err(AppError::Conflict(format!(
                    "nothing ran: a person who manages {} allows {} for production deploys on \
                     the Secrets page: {link}",
                    names.join(", "),
                    if names.len() == 1 { "it" } else { "them" },
                )));
            }
            if let Some(ctx) = asker {
                let user = SecretUser::Task(ctx);
                for record in &waiting {
                    self.ask_env_files(user, record, &sandbox.slug, why).await?;
                }
                self.flag_secret_wait(user, "a CD secret request").await?;
            }
            return Err(AppError::Conflict(format!(
                "nothing ran: a person who manages {} allows using {} outside the proxy (the \
                 project's .env and its dev deploys) on the Secrets page: {link}",
                names.join(", "),
                if names.len() == 1 { "it" } else { "them" },
            )));
        }
        let mut values = Vec::with_capacity(records.len());
        for record in records {
            let payload: AgentPayload = self.open_payload(record)?;
            values.push((record.name.clone(), payload.value.clone()));
        }
        Ok(values)
    }
}
