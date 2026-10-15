//! The user's own space for user processes (sandbox shells and editors): SPEC-001 UI-016 and
//! EDT-003, SPEC-002 ADR-12.
//!
//! Contract:
//! - every user has one persistent home, `<data>/users/<user id>/home` (0700), shared by all of
//!   their shells and editors in every sandbox and never bound into agent runs or other users'
//!   processes; it is seeded once from `/etc/skel` with a Git identity;
//! - contained user processes run in their own user namespace as uid/gid 0 with generated
//!   `/etc/passwd` and `/etc/group` that name uid 0 after the user's login, `HOME=/home/<login>`,
//!   the sandbox slug as host name and a `sudo` shim first on `PATH` (the user is root of the
//!   sandbox; host system directories stay read-only, U-12);
//! - uncontained (development) user processes keep the server's identity and use the home's
//!   host path;
//! - Git in user processes authenticates through the credential helper with a workspace token
//!   that the internal listener maps to the user's own GitHub token (the superadmin, who has no
//!   GitHub identity, gets the App's token for the sandbox's repository, as agents do, ACC-012)
//!   ([`App::workspace_git_credential`]); a grant is checked against the user's current access
//!   on every use;
//! - the GitHub CLI is logged in the same way: a `gh` shim first on `PATH` fetches that token per
//!   invocation (`yulius gh-token`) and runs the real `gh` with it ([`gh_shim`], SPEC-004); agent
//!   runs get the same shim, answered with their task token by the App's narrowed agent token,
//!   never a person's (`App::add_agent_gh`, ADR-0114).

use std::collections::HashMap;
use std::io::Write as _;
use std::os::unix::fs::{OpenOptionsExt as _, PermissionsExt as _};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use yulius_domain::ids::{SandboxId, UserId};
use yulius_domain::permission::Action;

use crate::auth::hex_token;
use crate::context::App;
use crate::error::{AppError, AppResult};
use crate::hooks::GitCredential;
use crate::model::{SandboxRecord, UserRecord};
use crate::ports::isolation::SandboxJoin;
use crate::ports::session::{Bind, Containment, Identity, LaunchPlan};

/// Directory of Yulius helper commands inside user processes.
pub const SANDBOX_BIN: &str = "/yulius/bin";
/// The host's sudo, replaced by the shim inside user processes.
const HOST_SUDO: &str = "/usr/bin/sudo";

/// The `sudo` shim: the user already is root (uid 0) inside the sandbox, so `sudo` only drops
/// its own options and runs the command (`sudo -i`/`-s` or no command start a shell).
pub const SUDO_SHIM: &str = r#"#!/bin/sh
# Yulius sandbox sudo: you are already root (uid 0) inside this sandbox.
shell=0
while [ $# -gt 0 ]; do
  case "$1" in
    -i|-s|--login|--shell) shell=1; shift ;;
    -u|-g|-p|-C|-h|-r|-t|-T|-U|-D) shift 2 ;;
    --) shift; break ;;
    -*) shift ;;
    *) break ;;
  esac
done
if [ $# -eq 0 ]; then
  if [ "$shell" = 1 ] || [ -t 0 ]; then exec "${SHELL:-/bin/sh}" -l; fi
  echo "usage: sudo command [args...]" >&2
  exit 1
fi
# `sudo NAME=value command` sets variables like sudo does.
exec env "$@"
"#;

/// The `gh` shim for the Yulius executable `exe`: runs the real `gh` (the first one on `PATH`
/// that is not a copy of the shim) logged in as the user. The user's token comes from
/// `yulius gh-token` for this one process only, so it is never stored, never reaches the shell's
/// other children and is checked against the user's current access at every use; a `GH_TOKEN`
/// or `GITHUB_TOKEN` the user set wins, and without a token `gh` simply stays logged out.
#[must_use]
pub fn gh_shim(exe: &Path) -> String {
    let exe = exe.to_string_lossy().replace('\'', "'\\''");
    format!(
        r#"#!/bin/sh
# Yulius gh shim: the GitHub CLI, logged in as the GitHub user who signed in to Yulius.
real=
set -f
old_ifs=$IFS
IFS=:
for dir in $PATH; do
  # Skip empty entries and every copy of this shim (whatever way its directory is spelled).
  if [ -n "$dir" ] && [ -x "$dir/gh" ] && ! grep -qs "^# Yulius gh shim" "$dir/gh"; then
    real="$dir/gh"
    break
  fi
done
IFS=$old_ifs
set +f
if [ -z "$real" ]; then
  echo "gh is not installed in this sandbox" >&2
  exit 127
fi
if [ -z "${{GH_TOKEN:-}}" ] && [ -z "${{GITHUB_TOKEN:-}}" ]; then
  if token=$('{exe}' gh-token 2>/dev/null) && [ -n "$token" ]; then
    GH_TOKEN=$token
    export GH_TOKEN
  fi
fi
exec "$real" "$@"
"#
    )
}

/// The `yulius` command of agent runs (SPEC-011): only `yulius wait`, which blocks until the
/// checks at the agent's head finish, its delivery moves or an armed merge is decided.
#[must_use]
pub fn agent_yulius_shim(exe: &Path) -> String {
    let exe = exe.to_string_lossy().replace('\'', "'\\''");
    format!(
        r#"#!/bin/sh
# Yulius agent command: `yulius wait [--minutes N]` (start it with run_in_background).
case "${{1:-}}" in
  wait) exec '{exe}' "$@" ;;
  *) echo "In a task's session yulius offers: yulius wait [--minutes N]" >&2; exit 2 ;;
esac
"#
    )
}

/// Registry of workspace tokens. A token is derived from a server secret kept in the data
/// directory, the user and the sandbox, so it stays valid across restarts: shells, the Cockpit
/// and editors survive a restart (they are adopted) and keep their environment, and their `gh`
/// and Git lost GitHub after every deploy while tokens were random per server lifetime
/// (F 2026-10-12, SPEC-050 IDN-101). The maps cache tokens already derived.
#[derive(Default)]
pub struct WorkspaceGrants {
    by_token: Mutex<HashMap<String, (UserId, SandboxId)>>,
    by_owner: Mutex<HashMap<(UserId, SandboxId), String>>,
    secret: std::sync::OnceLock<String>,
}

/// The file in the data directory that holds the workspace token secret (hex, mode 0600).
pub const WORKSPACE_SECRET_FILE: &str = "workspace-token.key";

/// Paths and settings of a user's process space in one sandbox.
#[derive(Debug, Clone)]
pub struct UserSpace {
    /// Login name inside sandboxes.
    pub login: String,
    /// Host path of the user's home.
    pub home_host: PathBuf,
    /// `HOME` as seen by the user's processes.
    pub home: PathBuf,
    /// Containment of the user's processes in this sandbox.
    pub containment: Containment,
    /// Environment shared by the user's shells and editors.
    pub env: Vec<(String, String)>,
}

impl UserSpace {
    /// A path inside the user's home as seen by the user's processes, and its host path.
    #[must_use]
    pub fn home_path(&self, relative: &str) -> (PathBuf, PathBuf) {
        (self.home.join(relative), self.home_host.join(relative))
    }
}

/// The user's login inside sandboxes: the GitHub login in lower case (letters, digits, `-`,
/// `_`), else `user-<id prefix>`; never `root`.
#[must_use]
pub fn sandbox_login(user: &UserRecord) -> String {
    let login: String = user
        .github_login
        .as_deref()
        .unwrap_or_default()
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
        .flat_map(char::to_lowercase)
        .take(32)
        .collect();
    let valid = login.chars().next().is_some_and(|c| c.is_ascii_lowercase()) && login != "root";
    if valid {
        login
    } else {
        let id: String = user
            .id
            .as_str()
            .chars()
            .filter(char::is_ascii_alphanumeric)
            .take(8)
            .collect();
        format!("user-{}", id.to_ascii_lowercase())
    }
}

/// The sandbox slug as a host name (letters, digits, `-`; at most 63 characters).
#[must_use]
pub fn sandbox_hostname(slug: &str) -> String {
    let name: String = slug
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .take(63)
        .collect();
    let name = name.trim_matches('-').to_owned();
    if name.is_empty() {
        "sandbox".into()
    } else {
        name
    }
}

fn private_dir(path: &Path) -> AppResult<()> {
    std::fs::create_dir_all(path).map_err(AppError::internal)?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))
        .map_err(AppError::internal)
}

/// Writes `bytes` atomically with `mode`.
fn write_file(path: &Path, bytes: &[u8], mode: u32) -> AppResult<()> {
    let temporary = path.with_extension("tmp");
    let _ = std::fs::remove_file(&temporary);
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(mode)
        .open(&temporary)
        .map_err(AppError::internal)?;
    file.write_all(bytes).map_err(AppError::internal)?;
    file.sync_all().map_err(AppError::internal)?;
    drop(file);
    std::fs::rename(&temporary, path).map_err(AppError::internal)
}

/// `passwd` and `group` naming uid/gid 0 after `login` (first, so it wins lookups by id).
fn identity_files(login: &str, display_name: &str, home: &Path) -> (String, String) {
    let gecos: String = display_name
        .chars()
        .filter(|c| !matches!(c, ':' | '\n' | '\r' | ','))
        .take(64)
        .collect();
    let shell = if Path::new("/bin/bash").exists() {
        "/bin/bash"
    } else {
        "/bin/sh"
    };
    let passwd = format!(
        "{login}:x:0:0:{gecos}:{home}:{shell}\nroot:x:0:0:root:/root:{shell}\nnobody:x:65534:65534:nobody:/nonexistent:/usr/sbin/nologin\n",
        home = home.display()
    );
    let group = format!("{login}:x:0:\nroot:x:0:\nnogroup:x:65534:\n");
    (passwd, group)
}

/// The identity files of an isolated sandbox (ADR-22): the user's line first, then the sandbox
/// system's own users and groups (system users of packages it installed included), so names
/// resolve as on the system while uid 0 is named after the user. Lines naming the login are
/// left out of the system part; at most 4096 lines of each are kept.
fn merged_identity(own: &str, system: &str, login: &str) -> String {
    let mut out = String::from(own.lines().next().unwrap_or_default());
    out.push('\n');
    for line in system.lines().take(4096) {
        let name = line.split(':').next().unwrap_or_default();
        if name.is_empty() || name == login || line.starts_with('#') {
            continue;
        }
        out.push_str(line);
        out.push('\n');
    }
    out
}

/// Seeds a new home: `/etc/skel` dotfiles and a Git identity.
fn seed_home(home: &Path, user: &UserRecord, login: &str) {
    for name in [".bashrc", ".profile", ".bash_logout"] {
        let source = Path::new("/etc/skel").join(name);
        if source.is_file() {
            let _ = std::fs::copy(&source, home.join(name));
        }
    }
    let email = match (user.github_login.as_deref(), user.github_id) {
        (Some(gh), Some(id)) => format!("{id}+{gh}@users.noreply.github.com"),
        (Some(gh), None) => format!("{gh}@users.noreply.github.com"),
        _ => format!("{login}@users.noreply.github.com"),
    };
    let name: String = user
        .display_name
        .chars()
        .filter(|c| !matches!(c, '"' | '\\' | '\n' | '\r'))
        .collect();
    let name = if name.trim().is_empty() {
        login.to_owned()
    } else {
        name
    };
    let gitconfig = format!("[user]\n\tname = \"{name}\"\n\temail = {email}\n");
    let _ = write_file(&home.join(".gitconfig"), gitconfig.as_bytes(), 0o644);
}

/// Puts `host_dir`, which holds only the `gh` shim, first on `launch`'s `PATH`: bound read-only at
/// [`SANDBOX_BIN`] in a contained run, by its host path otherwise.
fn put_gh_first(launch: &mut LaunchPlan, host_dir: PathBuf) {
    let dir = match &mut launch.containment {
        Containment::Bubblewrap { binds, .. } => {
            binds.push(Bind {
                source: host_dir,
                dest: PathBuf::from(SANDBOX_BIN),
                writable: false,
            });
            PathBuf::from(SANDBOX_BIN)
        }
        Containment::None => host_dir,
    };
    let rest = launch
        .env
        .iter()
        .rev()
        .find(|(key, _)| key == "PATH")
        .map_or_else(
            || "/usr/local/bin:/usr/bin:/bin".to_owned(),
            |(_, value)| value.clone(),
        );
    launch.env.retain(|(key, _)| key != "PATH");
    launch
        .env
        .push(("PATH".into(), format!("{}:{rest}", dir.display())));
}

impl App {
    pub(crate) fn users_dir(&self) -> PathBuf {
        self.config.data_dir.join("users")
    }

    /// The `sudo` shim directory (written on first use).
    fn runtime_bin(&self) -> AppResult<PathBuf> {
        let dir = self.config.data_dir.join("runtime").join("bin");
        std::fs::create_dir_all(&dir).map_err(AppError::internal)?;
        let sudo = dir.join("sudo");
        let current = std::fs::read(&sudo).ok();
        if current.as_deref() != Some(SUDO_SHIM.as_bytes()) {
            write_file(&sudo, SUDO_SHIM.as_bytes(), 0o755)?;
        }
        self.write_gh_shim(&dir)?;
        Ok(dir)
    }

    /// The directory holding only the `gh` shim, for user processes that are not contained
    /// (the `sudo` shim next to it must not shadow the host's `sudo`) and for agent runs.
    fn gh_bin(&self) -> AppResult<PathBuf> {
        let dir = self.config.data_dir.join("runtime").join("gh");
        std::fs::create_dir_all(&dir).map_err(AppError::internal)?;
        self.write_gh_shim(&dir)?;
        Ok(dir)
    }

    /// Writes the `gh` shim and the agents' `yulius` command into `dir` when missing or out of
    /// date.
    fn write_gh_shim(&self, dir: &Path) -> AppResult<()> {
        for (name, text) in [
            ("gh", gh_shim(&self.config.yulius_exe)),
            ("yulius", agent_yulius_shim(&self.config.yulius_exe)),
        ] {
            let shim = dir.join(name);
            if std::fs::read(&shim).ok().as_deref() != Some(text.as_bytes()) {
                write_file(&shim, text.as_bytes(), 0o755)?;
            }
        }
        Ok(())
    }

    /// Puts the `gh` shim first on the `PATH` of an agent run planned with `launch`'s final
    /// environment and containment (F 2026-10-11, SPEC-004): a contained run sees only the
    /// shim's directory, read-only at [`SANDBOX_BIN`] (the rest of `runtime/` stays out of it); an
    /// uncontained one gets its host directory. The shim's `yulius gh-token` asks with the run's
    /// task token for the App's agent token, exactly like the agent's Git (ADR-0114).
    ///
    /// # Errors
    /// `Internal` when the shim cannot be written.
    pub(crate) fn add_agent_gh(&self, launch: &mut LaunchPlan) -> AppResult<()> {
        put_gh_first(launch, self.gh_bin()?);
        Ok(())
    }

    /// Prepares `user`'s space in `sandbox`: home (created and seeded once), identity files
    /// and the containment and environment of their processes.
    ///
    /// # Errors
    /// `Internal` when the directories or files cannot be written.
    pub fn user_space(
        &self,
        user: &UserRecord,
        sandbox: &SandboxRecord,
        join: Option<SandboxJoin>,
    ) -> AppResult<UserSpace> {
        let login = sandbox_login(user);
        let root = self.users_dir().join(user.id.as_str());
        let home_host = root.join("home");
        let fresh = !home_host.exists();
        private_dir(&root)?;
        private_dir(&home_host)?;
        if fresh {
            seed_home(&home_host, user, &login);
        }
        let token = self.workspace_token(&user.id, &sandbox.id);
        let path = std::env::var("PATH").unwrap_or_else(|_| "/usr/local/bin:/usr/bin:/bin".into());
        let mut env = vec![
            ("USER".to_owned(), login.clone()),
            ("LOGNAME".to_owned(), login.clone()),
            ("YULIUS_URL".to_owned(), self.config.internal_url.clone()),
            ("YULIUS_TASK_TOKEN".to_owned(), token),
        ];
        env.extend(crate::supervisor::git_credential_env(
            &self.config.yulius_exe,
        ));
        if !self.config.contained {
            env.push(("HOME".into(), home_host.to_string_lossy().into_owned()));
            let path = format!(
                "{}:{}",
                self.gh_bin()?.display(),
                crate::isolation::with_home_tools(&home_host.to_string_lossy(), &path)
            );
            env.push(("PATH".into(), path));
            return Ok(UserSpace {
                login,
                home: home_host.clone(),
                home_host,
                containment: Containment::None,
                env,
            });
        }
        let home = PathBuf::from("/home").join(&login);
        let identity_dir = root.join("identity");
        private_dir(&identity_dir)?;
        let (mut passwd, mut group) = identity_files(&login, &user.display_name, &home);
        if let Some(join) = &join {
            let read = |path: &Option<PathBuf>| {
                path.as_ref()
                    .and_then(|p| std::fs::read_to_string(p).ok())
                    .filter(|text| text.len() <= 1 << 20)
            };
            if let Some(system) = read(&join.system_passwd) {
                passwd = merged_identity(&passwd, &system, &login);
            }
            if let Some(system) = read(&join.system_group) {
                group = merged_identity(&group, &system, &login);
            }
        }
        write_file(&identity_dir.join("passwd"), passwd.as_bytes(), 0o644)?;
        write_file(&identity_dir.join("group"), group.as_bytes(), 0o644)?;
        let bin = self.runtime_bin()?;
        env.push(("HOME".into(), home.to_string_lossy().into_owned()));
        // In an isolated sandbox (ADR-22) the system layer has root's tools and the sudo shim
        // as /usr/bin/sudo (a dpkg diversion), so only the identity files are bound.
        let path = if join.is_some() {
            crate::isolation::SANDBOX_PATH.to_owned()
        } else {
            path
        };
        let path = crate::isolation::with_home_tools(&home.to_string_lossy(), &path);
        env.push(("PATH".into(), format!("{SANDBOX_BIN}:{path}")));
        let identity_dest = PathBuf::from("/etc");
        let replace_sudo = join.is_none() && Path::new(HOST_SUDO).is_file();
        let extra_readonly: Vec<PathBuf> = join
            .as_ref()
            .map(|j| j.extra_readonly.clone())
            .unwrap_or_default();
        let containment = Containment::Bubblewrap {
            sandbox_root: self.sandbox_dir(&sandbox.id),
            home: home.clone(),
            writable: Vec::new(),
            readonly: std::iter::once(self.config.yulius_exe.clone())
                .chain(self.config.contained_readonly.iter().cloned())
                .chain(extra_readonly)
                .collect(),
            binds: vec![
                Bind {
                    source: home_host.clone(),
                    dest: home.clone(),
                    writable: true,
                },
                Bind {
                    source: identity_dir.join("passwd"),
                    dest: identity_dest.join("passwd"),
                    writable: false,
                },
                Bind {
                    source: identity_dir.join("group"),
                    dest: identity_dest.join("group"),
                    writable: false,
                },
                Bind {
                    source: bin.clone(),
                    dest: PathBuf::from(SANDBOX_BIN),
                    writable: false,
                },
            ]
            .into_iter()
            // Login shells of uid 0 get a fresh PATH from /etc/profile; the shim also replaces
            // the host's setuid sudo, which cannot work in an unprivileged namespace.
            .chain(replace_sudo.then(|| Bind {
                source: bin.join("sudo"),
                dest: PathBuf::from(HOST_SUDO),
                writable: false,
            }))
            .collect(),
            identity: Some(Identity {
                uid: 0,
                gid: 0,
                hostname: sandbox_hostname(&sandbox.slug),
            }),
            join: join.map(Box::new),
        };
        Ok(UserSpace {
            login,
            home_host,
            home,
            containment,
            env,
        })
    }

    /// The workspace token secret: read from the data directory, created there on first use. When
    /// the file cannot be read or written, a secret for this server's lifetime (tokens then end
    /// with it, as before).
    fn workspace_secret(&self) -> &str {
        self.workspace_grants.secret.get_or_init(|| {
            let path = self.config.data_dir.join(WORKSPACE_SECRET_FILE);
            if let Ok(text) = std::fs::read_to_string(&path) {
                let text = text.trim();
                if text.len() == 64 && text.bytes().all(|b| b.is_ascii_hexdigit()) {
                    return text.to_owned();
                }
            }
            let secret = hex_token(&self.deps.crypto.random_bytes());
            let written = (|| -> std::io::Result<()> {
                use std::io::Write;
                let mut file = std::fs::OpenOptions::new()
                    .write(true)
                    .create(true)
                    .truncate(true)
                    .mode(0o600)
                    .open(&path)?;
                file.write_all(secret.as_bytes())
            })();
            if let Err(err) = written {
                tracing::warn!(error = %err, "the workspace token secret could not be saved; tokens end with this server");
            }
            secret
        })
    }

    /// The workspace token of `user` in `sandbox`: the same across restarts (IDN-101).
    fn workspace_token(&self, user: &UserId, sandbox: &SandboxId) -> String {
        let key = (user.clone(), sandbox.clone());
        if let Some(token) = self
            .workspace_grants
            .by_owner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(&key)
        {
            return token.clone();
        }
        let token = self.derive_workspace_token(user, sandbox);
        self.remember_workspace_token(&token, key);
        token
    }

    fn derive_workspace_token(&self, user: &UserId, sandbox: &SandboxId) -> String {
        let material = format!(
            "yulius-workspace\n{}\n{}\n{}",
            self.workspace_secret(),
            user.as_str(),
            sandbox.as_str()
        );
        format!("ws_{}", self.deps.crypto.sha256_hex(material.as_bytes()))
    }

    fn remember_workspace_token(&self, token: &str, key: (UserId, SandboxId)) {
        self.workspace_grants
            .by_owner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(key.clone(), token.to_owned());
        self.workspace_grants
            .by_token
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(token.to_owned(), key);
    }

    /// Whose workspace token `token` is: from the cache, else derived again for every user and
    /// sandbox (a token issued before a restart).
    async fn workspace_owner(&self, token: &str) -> AppResult<(UserId, SandboxId)> {
        if let Some(owner) = self
            .workspace_grants
            .by_token
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(token)
            .cloned()
        {
            return Ok(owner);
        }
        if !token.starts_with("ws_") {
            return Err(AppError::Unauthenticated);
        }
        let users = self.deps.store.users().await?;
        let sandboxes = self.deps.store.sandboxes().await?;
        for user in &users {
            for sandbox in &sandboxes {
                let derived = self.derive_workspace_token(&user.id, &sandbox.id);
                if crate::auth::constant_time_eq(derived.as_bytes(), token.as_bytes()) {
                    let key = (user.id.clone(), sandbox.id.clone());
                    self.remember_workspace_token(&derived, key.clone());
                    return Ok(key);
                }
            }
        }
        Err(AppError::Unauthenticated)
    }

    /// A GitHub credential for a user process: the user's own token (the superadmin without a
    /// GitHub identity: the App's token for this repository, ACC-012), for the sandbox's
    /// repository only, while the user may still work in the sandbox.
    ///
    /// # Errors
    /// `Unauthenticated` for unknown tokens; `Forbidden` for other hosts or repositories or a
    /// user who lost write access; GitHub failures as usual.
    pub async fn workspace_git_credential(
        &self,
        token: &str,
        host: &str,
        path: Option<&str>,
    ) -> AppResult<GitCredential> {
        let (user_id, sandbox_id) = self.workspace_owner(token).await?;
        if host != "github.com" {
            return Err(AppError::Forbidden(
                "credentials are only issued for github.com".into(),
            ));
        }
        let user = self
            .deps
            .store
            .user(&user_id)
            .await?
            .filter(|u| u.disabled_at.is_none())
            .ok_or(AppError::Unauthenticated)?;
        let (sandbox, _) = self.require(&user, &sandbox_id, Action::Work).await?;
        // A playground has no GitHub repository: nothing is issued (SPEC-099 PLY-002).
        if sandbox.is_playground() {
            return Err(AppError::Forbidden(
                "this sandbox is a playground: its Git stays on this server and needs no GitHub \
                 credential"
                    .into(),
            ));
        }
        if let Some(path) = path {
            let wanted = path
                .trim_matches('/')
                .trim_end_matches(".git")
                .to_ascii_lowercase();
            let own = format!("{}/{}", sandbox.repo_owner, sandbox.repo_name).to_ascii_lowercase();
            if wanted != own {
                return Err(AppError::Forbidden(
                    "credentials are only issued for this project's repository".into(),
                ));
            }
        }
        let token = if user.is_superadmin && user.github_login.is_none() {
            self.repository_installation_token(&sandbox).await?
        } else {
            self.user_github_token(&user).await?
        };
        Ok(GitCredential { token })
    }
}

#[cfg(test)]
mod tests {
    use yulius_domain::time::Timestamp;

    use super::*;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    fn user(login: Option<&str>) -> Result<UserRecord, Box<dyn std::error::Error>> {
        Ok(UserRecord {
            id: UserId::parse("u-01hzx3k9q7a1b2c3")?,
            github_login: login.map(str::to_owned),
            github_id: Some(42),
            display_name: "Alice: Example".into(),
            avatar_url: None,
            is_superadmin: false,
            is_instance_admin: false,
            password_hash: None,
            created_at: Timestamp(1),
            disabled_at: None,
        })
    }

    #[test]
    fn logins_are_safe_and_never_root() -> TestResult {
        assert_eq!(sandbox_login(&user(Some("MrFishPL"))?), "mrfishpl");
        assert_eq!(sandbox_login(&user(Some("a-b_c"))?), "a-b_c");
        assert!(sandbox_login(&user(Some("root"))?).starts_with("user-"));
        assert!(sandbox_login(&user(Some("9lives"))?).starts_with("user-"));
        assert!(sandbox_login(&user(None)?).starts_with("user-"));
        Ok(())
    }

    #[test]
    fn hostnames_are_valid() {
        assert_eq!(sandbox_hostname("test-app"), "test-app");
        assert_eq!(sandbox_hostname("my.repo_x"), "my-repo-x");
        assert_eq!(sandbox_hostname("..."), "sandbox");
        assert_eq!(sandbox_hostname(&"a".repeat(80)).len(), 63);
    }

    #[test]
    fn uid_zero_is_named_after_the_user_first() {
        let (passwd, group) = identity_files("alice", "Alice: Example", Path::new("/home/alice"));
        let first = passwd.lines().next().unwrap_or_default();
        assert!(
            first.starts_with("alice:x:0:0:Alice Example:/home/alice:"),
            "{first}"
        );
        assert!(passwd.contains("\nroot:x:0:0:"));
        assert!(group.starts_with("alice:x:0:\n"));
        let merged = merged_identity(
            &passwd,
            "root:x:0:0:root:/root:/bin/bash\n_apt:x:42:65534::/nonexistent:/usr/sbin/nologin\nalice:x:1000:1000::/home/alice:/bin/sh\npostgres:x:113:120::/var/lib/postgresql:/bin/bash\n",
            "alice",
        );
        let names: Vec<&str> = merged
            .lines()
            .map(|l| l.split(':').next().unwrap_or(""))
            .collect();
        assert_eq!(names, vec!["alice", "root", "_apt", "postgres"]);
        assert!(merged.starts_with("alice:x:0:0:"));
    }

    /// A fresh temporary directory path, unique across the tests running in parallel.
    fn unique_dir(prefix: &str) -> PathBuf {
        static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        std::env::temp_dir().join(format!(
            "{prefix}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ))
    }

    /// A directory with only `grep` (what the shim needs besides the shell), so a `gh` installed
    /// on the host never shows up on the tests' `PATH`.
    fn tools_dir(dir: &Path) -> Result<PathBuf, Box<dyn std::error::Error>> {
        let tools = dir.join("tools");
        std::fs::create_dir_all(&tools)?;
        let grep = ["/usr/bin/grep", "/bin/grep"]
            .into_iter()
            .map(Path::new)
            .find(|p| p.is_file())
            .ok_or("no grep on this host")?;
        std::os::unix::fs::symlink(grep, tools.join("grep"))?;
        Ok(tools)
    }

    /// Runs the shim for `exe` with a fake `gh` that prints its token and arguments.
    fn run_shim(
        exe: &str,
        gh_present: bool,
        env: &[(&str, &str)],
    ) -> Result<(i32, String, String), Box<dyn std::error::Error>> {
        let dir = unique_dir("yulius-gh-shim");
        let _ = std::fs::remove_dir_all(&dir);
        let shim_dir = dir.join("shim");
        let real_dir = dir.join("real");
        std::fs::create_dir_all(&shim_dir)?;
        std::fs::create_dir_all(&real_dir)?;
        write_file(
            &shim_dir.join("gh"),
            gh_shim(Path::new(exe)).as_bytes(),
            0o755,
        )?;
        if gh_present {
            write_file(
                &real_dir.join("gh"),
                b"#!/bin/sh\necho \"token=${GH_TOKEN:-none} args=$*\"\n",
                0o755,
            )?;
        }
        let path = format!(
            "{}:{}:{}",
            shim_dir.display(),
            real_dir.display(),
            tools_dir(&dir)?.display()
        );
        let mut command = std::process::Command::new("/bin/sh");
        command
            .arg(shim_dir.join("gh"))
            .args(["auth", "status"])
            .env_clear()
            .env("PATH", path);
        for (key, value) in env {
            command.env(key, value);
        }
        let out = command.output()?;
        let _ = std::fs::remove_dir_all(&dir);
        Ok((
            out.status.code().unwrap_or(-1),
            String::from_utf8_lossy(&out.stdout).trim().to_owned(),
            String::from_utf8_lossy(&out.stderr).into_owned(),
        ))
    }

    /// A fake `yulius` that prints a token for `gh-token`.
    fn fake_exe(body: &str) -> Result<PathBuf, Box<dyn std::error::Error>> {
        let dir = unique_dir("yulius-fake-exe");
        std::fs::create_dir_all(&dir)?;
        let exe = dir.join("yulius");
        write_file(&exe, format!("#!/bin/sh\n{body}\n").as_bytes(), 0o755)?;
        Ok(exe)
    }

    #[test]
    fn gh_shim_logs_gh_in_with_the_users_token() -> TestResult {
        let exe = fake_exe(r#"[ "$1" = gh-token ] && echo ghu_Synthetic"#)?;
        let (code, out, _) = run_shim(&exe.to_string_lossy(), true, &[])?;
        assert_eq!(code, 0);
        assert_eq!(out, "token=ghu_Synthetic args=auth status");
        Ok(())
    }

    #[test]
    fn gh_shim_keeps_a_token_the_user_set() -> TestResult {
        let exe = fake_exe("echo ghu_Other")?;
        let (_, out, _) = run_shim(&exe.to_string_lossy(), true, &[("GH_TOKEN", "mine")])?;
        assert_eq!(out, "token=mine args=auth status");
        let (_, out, _) = run_shim(&exe.to_string_lossy(), true, &[("GITHUB_TOKEN", "theirs")])?;
        assert_eq!(out, "token=none args=auth status");
        Ok(())
    }

    #[test]
    fn gh_shim_leaves_gh_unauthenticated_when_no_token_is_issued() -> TestResult {
        let exe = fake_exe("exit 1")?;
        let (code, out, _) = run_shim(&exe.to_string_lossy(), true, &[])?;
        assert_eq!(code, 0);
        assert_eq!(out, "token=none args=auth status");
        Ok(())
    }

    #[test]
    fn gh_shim_says_so_when_gh_is_not_installed() -> TestResult {
        let exe = fake_exe("echo ghu_Synthetic")?;
        let (code, out, err) = run_shim(&exe.to_string_lossy(), false, &[])?;
        assert_eq!(code, 127);
        assert!(out.is_empty());
        assert!(err.contains("gh is not installed"), "{err}");
        Ok(())
    }

    #[test]
    fn gh_shim_survives_odd_exe_paths_empty_path_entries_and_a_second_shim() -> TestResult {
        let dir = unique_dir("yulius-gh-odd");
        let exe_dir = dir.join("it's here");
        let shim_a = dir.join("a");
        let shim_b = dir.join("b");
        let real = dir.join("real");
        for d in [&exe_dir, &shim_a, &shim_b, &real] {
            std::fs::create_dir_all(d)?;
        }
        let exe = exe_dir.join("yulius");
        write_file(&exe, b"#!/bin/sh\necho ghu_Synthetic\n", 0o755)?;
        let shim = gh_shim(&exe);
        // Two shim directories (contained and uncontained flavours) and a symlinked spelling of
        // one of them: the shim must skip every copy of itself, never loop.
        write_file(&shim_a.join("gh"), shim.as_bytes(), 0o755)?;
        write_file(&shim_b.join("gh"), shim.as_bytes(), 0o755)?;
        let link = dir.join("link");
        std::os::unix::fs::symlink(&shim_b, &link)?;
        write_file(
            &real.join("gh"),
            b"#!/bin/sh\necho \"token=${GH_TOKEN:-none}\"\n",
            0o755,
        )?;
        let path = format!(
            "{}::{}:{}:{}:{}",
            shim_a.display(),
            link.display(),
            shim_b.display(),
            real.display(),
            tools_dir(&dir)?.display()
        );
        let out = std::process::Command::new("/bin/sh")
            .arg(shim_a.join("gh"))
            .env_clear()
            .env("PATH", path)
            .output()?;
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(
            String::from_utf8_lossy(&out.stdout).trim(),
            "token=ghu_Synthetic"
        );
        Ok(())
    }

    #[test]
    fn gh_shim_quotes_the_executable_path() {
        let script = gh_shim(Path::new("/opt/it's here/yulius"));
        assert!(
            script.contains(r#"'/opt/it'\''s here/yulius' gh-token"#),
            "{script}"
        );
    }
    fn agent_launch(containment: Containment) -> LaunchPlan {
        LaunchPlan {
            argv: vec!["claude".into()],
            cwd: PathBuf::from("/data/sandboxes/s1/worktrees/t1"),
            env: vec![
                ("HOME".into(), "/data/sandboxes/s1/home".into()),
                ("PATH".into(), "/usr/bin:/bin".into()),
            ],
            secret_env: vec![("YULIUS_TASK_TOKEN".into(), "t".into())],
            files: Vec::new(),
            containment,
        }
    }

    /// F 2026-10-11: an agent run finds the `gh` shim first on its `PATH` — inside containment
    /// only the shim's directory, at /yulius/bin; on the host its own directory.
    #[test]
    fn agent_runs_get_the_gh_shim_first_on_path() -> TestResult {
        let host = PathBuf::from("/data/runtime/gh");
        let mut uncontained = agent_launch(Containment::None);
        put_gh_first(&mut uncontained, host.clone());
        let paths: Vec<&str> = uncontained
            .env
            .iter()
            .filter(|(k, _)| k == "PATH")
            .map(|(_, v)| v.as_str())
            .collect();
        assert_eq!(paths, ["/data/runtime/gh:/usr/bin:/bin"]);

        let mut contained = agent_launch(Containment::Bubblewrap {
            sandbox_root: PathBuf::from("/data/sandboxes/s1"),
            home: PathBuf::from("/data/sandboxes/s1/home"),
            writable: Vec::new(),
            readonly: Vec::new(),
            binds: Vec::new(),
            identity: None,
            join: None,
        });
        contained
            .env
            .push(("PATH".into(), "/usr/sbin:/usr/bin".into()));
        put_gh_first(&mut contained, host.clone());
        let paths: Vec<&str> = contained
            .env
            .iter()
            .filter(|(k, _)| k == "PATH")
            .map(|(_, v)| v.as_str())
            .collect();
        assert_eq!(
            paths,
            ["/yulius/bin:/usr/sbin:/usr/bin"],
            "the last PATH wins"
        );
        let Containment::Bubblewrap {
            binds, readonly, ..
        } = &contained.containment
        else {
            return Err("containment changed".into());
        };
        assert_eq!(
            binds,
            &[Bind {
                source: host,
                dest: PathBuf::from(SANDBOX_BIN),
                writable: false,
            }]
        );
        assert!(readonly.is_empty(), "the rest of runtime/ stays out");
        Ok(())
    }
}
