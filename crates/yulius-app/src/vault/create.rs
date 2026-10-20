//! Agent secrets an agent creates from a file (SPEC-211, ADR-0167): a key it generated, a token a
//! CLI wrote, a key in `~/.ssh`. Yulius reads the file itself, so the value never passes through
//! the model, its transcript or the screen; the secret starts with no hosts and no `.env`
//! allowance, so a person still decides every use.
//!
//! Yulius runs outside the agent's containment and with more rights than it, so the file is
//! reached as `secret_copy_to_env` reaches its target: from the folder the agent works in or its
//! home, one folder at a time without following links, the file itself opened without following
//! a link and without blocking.

use std::io::Read as _;
use std::path::PathBuf;

use nix::errno::Errno;
use nix::fcntl::{AtFlags, OFlag, openat};
use nix::sys::stat::{Mode, SFlag, fstatat};
use nix::unistd::{UnlinkatFlags, unlinkat};
use serde::Deserialize;
use serde_json::{Value, json};
use yulius_domain::permission::Action;
use yulius_domain::vault::dotenv::agent_value_from_file;
use yulius_domain::vault::names::SecretName;
use yulius_domain::vault::{Injection, MAX_FIELD_BYTES};
use zeroize::Zeroizing;

use super::agents::{Meta, NewAgentSecret, description};
use super::env_files::{Folder, Naming, WORK_FOLDER, open_folder, os_error};
use super::{AgentPayload, SecretUser, invalid};
use crate::context::App;
use crate::error::{AppError, AppResult};
use crate::files::relative_components;
use crate::model::SandboxRecord;

/// Arguments of `secret_create_from_file`.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CreateArgs {
    name: String,
    file: String,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    delete_file: bool,
}

/// How messages name the caller's home.
const HOME: &str = "your home";

/// Where a file named to `secret_create_from_file` lies.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum Root {
    /// The folder the caller works in (a task's worktree, the Superagent's repository).
    Work,
    /// The caller's home (`~/…`).
    Home,
}

impl Root {
    /// How messages name it.
    pub(super) fn named(&self) -> &'static str {
        match self {
            Self::Work => WORK_FOLDER,
            Self::Home => HOME,
        }
    }

    /// What its paths are written after, as the caller writes them.
    pub(super) fn prefix(&self) -> &'static str {
        match self {
            Self::Work => "",
            Self::Home => "~/",
        }
    }
}

/// The root and the components of `file`: relative to the folder the caller works in, or below
/// `~/` for its home. Absolute paths, `..`, `.git`, `~user` and a bare folder are refused.
pub(super) fn source(file: &str) -> AppResult<(Root, Vec<String>)> {
    let file = file.trim();
    let (root, rest) = match file.strip_prefix('~') {
        Some(rest) => match rest.strip_prefix('/') {
            Some(rest) => (Root::Home, rest),
            None => {
                return Err(AppError::Invalid(
                    "name a file in your home as ~/path (not ~ or ~user)".into(),
                ));
            }
        },
        None => (Root::Work, file),
    };
    if rest.starts_with('/') {
        return Err(AppError::Invalid(
            "name the file relative to the folder you work in, or as ~/path in your home; \
             copy a file from elsewhere there first, without printing it"
                .into(),
        ));
    }
    let parts = relative_components(rest)?;
    if parts.is_empty() {
        return Err(AppError::Invalid(
            "name the file that holds the secret".into(),
        ));
    }
    Ok((root, parts))
}

/// The text of `name` in `folder`: a regular file (not a link, a FIFO or a device) of at most
/// [`MAX_FIELD_BYTES`], in UTF-8, wiped from memory when dropped. Messages name the file as
/// `shown`, never its content.
pub(super) fn read_secret_file(
    folder: &Folder,
    name: &str,
    shown: &str,
) -> AppResult<Zeroizing<String>> {
    // Its type first, so a device or socket is never opened (a link is not followed).
    match fstatat(&folder.fd, name, AtFlags::AT_SYMLINK_NOFOLLOW) {
        Ok(stat) => {
            let kind = SFlag::from_bits_truncate(stat.st_mode) & SFlag::S_IFMT;
            if kind == SFlag::S_IFLNK {
                return Err(AppError::Forbidden(format!(
                    "{shown} is a link: Yulius reads only regular files and follows no links"
                )));
            }
            if kind != SFlag::S_IFREG {
                return Err(AppError::Forbidden(format!(
                    "{shown} is not a regular file: Yulius reads only regular files"
                )));
            }
        }
        Err(Errno::ENOENT) => {
            return Err(AppError::Invalid(format!("{shown} does not exist")));
        }
        Err(err) => return Err(os_error(err)),
    }
    let flags = OFlag::O_RDONLY | OFlag::O_NOFOLLOW | OFlag::O_NONBLOCK | OFlag::O_CLOEXEC;
    let fd = match openat(&folder.fd, name, flags, Mode::empty()) {
        Ok(fd) => fd,
        Err(Errno::ENOENT) => {
            return Err(AppError::Invalid(format!("{shown} does not exist")));
        }
        Err(Errno::ELOOP) => {
            return Err(AppError::Forbidden(format!(
                "{shown} is a link: Yulius reads only regular files and follows no links"
            )));
        }
        Err(err) => return Err(os_error(err)),
    };
    let file = std::fs::File::from(fd);
    let meta = file.metadata().map_err(|e| crate::files::io_error(&e))?;
    if !meta.is_file() {
        return Err(AppError::Forbidden(format!(
            "{shown} is not a regular file: Yulius reads only regular files"
        )));
    }
    let too_large = || {
        AppError::Invalid(format!(
            "{shown} is larger than {} KiB, more than an agent secret holds",
            MAX_FIELD_BYTES / 1024
        ))
    };
    let limit = u64::try_from(MAX_FIELD_BYTES).unwrap_or(u64::MAX);
    if meta.len() > limit {
        return Err(too_large());
    }
    // Reserved up front, so reading never leaves a reallocated copy behind; one byte more than
    // the limit tells a file that grew since its size was read.
    let mut bytes = Zeroizing::new(Vec::with_capacity(MAX_FIELD_BYTES + 1));
    file.take(limit + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| crate::files::io_error(&e))?;
    if bytes.len() > MAX_FIELD_BYTES {
        return Err(too_large());
    }
    let text = std::str::from_utf8(&bytes)
        .map_err(|_| AppError::Invalid(format!("{shown} is not UTF-8 text")))?;
    Ok(Zeroizing::new(text.to_owned()))
}

/// Removes `name` from `folder` (never a folder, never through a link).
pub(super) fn remove_file(folder: &Folder, name: &str) -> AppResult<()> {
    unlinkat(&folder.fd, name, UnlinkatFlags::NoRemoveDir).map_err(os_error)
}

/// Deletes a file whose secret is stored (SCF-006): whether it is gone, and otherwise the warning
/// that says the secret stays and the file must go by hand.
fn delete_stored(folder: &Folder, name: &str, shown: &str) -> (bool, Option<String>) {
    match remove_file(folder, name) {
        Ok(()) => (true, None),
        Err(err) => (
            false,
            Some(format!(
                "the secret is stored, but {shown} could not be deleted ({}): delete it yourself \
                 if the vault should hold the only copy",
                err.public_message()
            )),
        ),
    }
}

impl App {
    /// `secret_create_from_file` (SPEC-211): stores the text of a file the caller names (in the
    /// folder it works in, or `~/…` in its home) as an agent secret of its sandbox, without the
    /// value reaching the caller. The person it acts for must be able to manage the sandbox's agent
    /// secrets; the secret starts with no hosts and no `.env` allowance; an existing name is never
    /// replaced. With `delete_file`, the file is removed once the secret is stored.
    pub(crate) async fn tool_secret_create_from_file(
        &self,
        user: SecretUser<'_>,
        args: &Value,
    ) -> AppResult<Value> {
        let args: CreateArgs = serde_json::from_value(args.clone())
            .map_err(|e| AppError::Invalid(format!("invalid arguments: {e}")))?;
        let name = SecretName::parse(args.name.trim()).map_err(invalid)?;
        if name.as_str() == crate::vidoc_key::VIDOC_SECRET {
            // A project secret of this name replaces the instance's Vidoc key for every run and
            // CI job of the sandbox (ADR-0157): a person's choice, never an agent's.
            return Err(AppError::Forbidden(format!(
                "{VIDOC} is reserved: a project secret of that name replaces the instance's Vidoc \
                 key for this project, so only a person adds one, on the Secrets page",
                VIDOC = crate::vidoc_key::VIDOC_SECRET
            )));
        }
        let description = description(args.description.as_deref())?;
        let (root, parts) = source(&args.file)?;
        let (file_name, folders) = parts
            .split_last()
            .ok_or_else(|| AppError::Invalid("name the file that holds the secret".into()))?;

        // As on the Secrets page: only a person who manages the sandbox's agent secrets.
        let store = &self.deps.store;
        let person = match user {
            SecretUser::Task(ctx) => &ctx.task.owner,
            SecretUser::Superagent(caller) => &caller.user,
        };
        let person = store.user(person).await?.ok_or(AppError::NotFound)?;
        let (sandbox, _) = self
            .require(&person, user.sandbox(), Action::Configure)
            .await
            .map_err(|err| match err {
                AppError::Forbidden(_) => AppError::Forbidden(
                    "the person you work for cannot manage this project's agent secrets (it \
                     needs write access to the repository), so you cannot add one for them"
                        .into(),
                ),
                other => other,
            })?;
        // A project secret of a name that reaches the project already (a global or the person's
        // own) would replace it for the project's runs (SPEC-245 RES-001): never an agent's doing.
        if self
            .resolved_agent_secrets(user.sandbox(), user.holder())
            .await?
            .iter()
            .any(|r| r.name == name.as_str())
        {
            return Err(AppError::Conflict(format!(
                "an agent secret named {} already applies to this project (see secrets_list); it \
                 is never replaced: pick another name, or a person rotates it on the Secrets page",
                name.as_str()
            )));
        }

        // Anchored on a folder the agent cannot replace, and walked down without links: the
        // home or repository itself may be a link the agent planted (review of SPEC-211, 1).
        let (anchor, mut below) = self.secret_source_root(user, &sandbox, &root).await?;
        let naming = Naming {
            within: root.named(),
            hidden: below.len(),
            prefix: root.prefix(),
        };
        below.extend(folders.iter().cloned());
        let folder = open_folder(&anchor, &below, &naming)?;
        // Messages name the path as the caller gave it, never the anchor's folders.
        let shown = match root {
            Root::Work => parts.join("/"),
            Root::Home => format!("~/{}", parts.join("/")),
        };
        let text = read_secret_file(&folder, file_name, &shown)?;
        let value = Zeroizing::new(
            agent_value_from_file(&text)
                .map_err(|err| AppError::Invalid(format!("{shown}: {err}")))?,
        );
        drop(text);
        let payload = AgentPayload {
            username: None,
            value: value.to_string(),
            totp: None,
        };
        drop(value);
        let origin = format!("from a file, by {}", user.asker());
        let record = self
            .insert_agent_secret(NewAgentSecret {
                sandbox: &sandbox.id,
                scope: crate::ports::vault::AgentScope::Project(sandbox.id.clone()),
                meta: Meta {
                    name: name.as_str().to_owned(),
                    hosts: Vec::new(),
                    injection: Injection::None,
                },
                description,
                payload: &payload,
                by: person.id.as_str(),
                actor: &user.actor(),
                origin: Some(&origin),
                // An agent's secret waits for a person (SPEC-211), never set up by agents
                // (SPEC-216).
                auto_allow: false,
            })
            .await?;
        drop(payload);
        // Open Secrets pages show it without a reload (SPEC-209).
        self.announce_request(&record);

        let (deleted, warning) = if args.delete_file {
            delete_stored(&folder, file_name, &shown)
        } else {
            (false, None)
        };
        let keep = if deleted {
            format!("{shown} is deleted: the vault holds the value now.")
        } else {
            format!(
                "{shown} still holds the value: delete it (without printing it) if the vault \
                 should hold the only copy."
            )
        };
        let mut result = json!({
            "created": record.name,
            "file": shown,
            "deleted": deleted,
            "hosts": [],
            "link": self.agent_secrets_link(&sandbox.slug),
            "next": format!(
                "{name} is an agent secret of this project, with no hosts yet and not allowed in \
                 .env files. You never see its value. To send it to a service, request its hosts \
                 with secret_request_hosts; for code outside the proxy, secret_copy_to_env; a \
                 person allows each on the Secrets page. {keep}",
                name = record.name
            ),
        });
        if let Some(warning) = warning {
            result["warning"] = Value::String(warning);
        }
        Ok(result)
    }

    /// Where `root` lies: a folder the agent cannot replace (the sandbox folder, or the
    /// Superagent's person's folder, canonical) and the folders below it, to be opened one at a
    /// time without following links. The folder the caller works in is a task's worktree or the
    /// sandbox repository; the home is the sandbox home for a task, its person's home (shared by
    /// their projects) for the Superagent.
    async fn secret_source_root(
        &self,
        user: SecretUser<'_>,
        sandbox: &SandboxRecord,
        root: &Root,
    ) -> AppResult<(PathBuf, Vec<String>)> {
        let missing =
            || AppError::Invalid(format!("{} does not exist on this machine", root.named()));
        let canonical = |path: PathBuf| std::fs::canonicalize(path).map_err(|_| missing());
        let below = |name: &str| vec![name.to_owned()];
        match (root, user) {
            (Root::Work, SecretUser::Task(ctx)) => {
                let anchor = canonical(self.sandbox_dir(&sandbox.id))?;
                // Canonical and inside the sandbox folder (`sandbox_worktree` checks it); the
                // walk from the anchor checks it again without links.
                let worktree = self
                    .sandbox_worktree(sandbox, ctx.task.id.as_str(), None)
                    .await?;
                let parts = worktree
                    .strip_prefix(&anchor)
                    .map_err(|_| {
                        AppError::Forbidden(format!("{WORK_FOLDER} is outside the sandbox"))
                    })?
                    .components()
                    .map(|c| c.as_os_str().to_string_lossy().into_owned())
                    .collect();
                Ok((anchor, parts))
            }
            (Root::Work, SecretUser::Superagent(_)) => {
                Ok((canonical(self.sandbox_dir(&sandbox.id))?, below("repo")))
            }
            (Root::Home, SecretUser::Task(_)) => {
                Ok((canonical(self.sandbox_dir(&sandbox.id))?, below("home")))
            }
            (Root::Home, SecretUser::Superagent(caller)) => Ok((
                canonical(self.users_dir().join(caller.user.as_str()))?,
                below("home"),
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vault::env_files::WORK;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    /// SPEC-211 SCF-002: relative paths are the work folder's, `~/` the home's; anything that
    /// could leave them is refused before a file is opened.
    #[test]
    fn paths_name_the_work_folder_or_the_home() -> TestResult {
        assert_eq!(
            source("secrets/token.txt")?,
            (
                Root::Work,
                vec!["secrets".to_owned(), "token.txt".to_owned()]
            )
        );
        assert_eq!(
            source(" ~/.ssh/deploy_key ")?,
            (Root::Home, vec![".ssh".to_owned(), "deploy_key".to_owned()])
        );
        for refused in [
            "/etc/shadow",
            "~//etc/shadow",
            "../other/key",
            "~/../other",
            "~root/.ssh/id",
            "~",
            "",
            "./",
            ".git/config",
            "a\0b",
        ] {
            assert!(source(refused).is_err(), "{refused:?}");
        }
        Ok(())
    }

    /// SPEC-211 SCF-002 (AC-3): only a regular file, small enough, in UTF-8, read without
    /// following a link or blocking on a FIFO; errors never quote the content.
    #[test]
    fn only_a_small_regular_text_file_is_read() -> TestResult {
        let dir = tempfile::tempdir()?;
        let root = std::fs::canonicalize(dir.path())?;
        let outside = root.join("outside");
        let wt = root.join("wt");
        std::fs::create_dir_all(&wt)?;
        std::fs::write(&outside, "outside-value-never-read")?;
        std::fs::write(wt.join("token.txt"), "synthetic-token-1234\n")?;
        std::os::unix::fs::symlink(&outside, wt.join("link"))?;
        nix::unistd::mkfifo(&wt.join("fifo"), Mode::from_bits_truncate(0o600))?;
        std::fs::write(wt.join("big"), "x".repeat(MAX_FIELD_BYTES + 1))?;
        std::fs::write(wt.join("binary"), [0xff, 0xfe, 0x00, 0x41])?;
        std::fs::create_dir(wt.join("folder"))?;

        let folder = open_folder(&wt, &[], &WORK)?;
        assert_eq!(
            read_secret_file(&folder, "token.txt", "token.txt")?.as_str(),
            "synthetic-token-1234\n"
        );
        for (name, forbidden) in [
            ("link", true),
            ("fifo", true),
            ("folder", true),
            ("big", false),
            ("binary", false),
            ("missing", false),
        ] {
            let err = read_secret_file(&folder, name, name)
                .err()
                .ok_or(format!("{name} was read"))?;
            assert_eq!(
                matches!(err, AppError::Forbidden(_)),
                forbidden,
                "{name}: {err:?}"
            );
            assert!(
                !format!("{err:?}").contains("outside-value"),
                "{name}: {err:?}"
            );
        }

        remove_file(&folder, "token.txt")?;
        assert!(!wt.join("token.txt").exists());
        // Review of SPEC-211, finding 4: a file that cannot be deleted leaves the secret and says
        // so.
        let (deleted, warning) = delete_stored(&folder, "token.txt", "secrets/token.txt");
        assert!(!deleted);
        let warning = warning.ok_or("no warning")?;
        assert!(
            warning.starts_with("the secret is stored, but secrets/token.txt could not be deleted"),
            "{warning}"
        );
        std::fs::write(wt.join("again.txt"), "synthetic-token-5678\n")?;
        assert_eq!(
            delete_stored(&folder, "again.txt", "again.txt"),
            (true, None)
        );
        assert!(!wt.join("again.txt").exists());
        assert!(remove_file(&folder, "folder").is_err());
        assert!(wt.join("folder").is_dir());
        Ok(())
    }
}
