//! Confinement of the host's own Git (SPEC-483 G1, ADR-0322).
//!
//! The server runs Git as the service user, outside every containment, in clones and worktrees
//! whose every file an agent writes, `.git/config` included. A repository's configuration can
//! make Git run a command (a filter, a diff or merge driver, `core.sshCommand`, a remote's
//! `uploadpack`, an editor: the list grows with Git) and its files can be links that lead Git's
//! reads and writes anywhere the service user reaches. Naming each key loses to the next one.
//!
//! So Git is started through [`exec_confined`], which first takes from the process, with the
//! kernel's Landlock, what Git does not need:
//!
//! - **executing** anything but Git's own programs (its executable, its `exec-path`, the
//!   program loader). A command from the configuration is run through `/bin/sh`, which is not
//!   among them; a single word names a program, which is not either;
//! - **writing** anywhere but the folders the command works in (its sandbox's) and
//!   `/dev/null`;
//! - **reading** anything but those folders, the system's programs, libraries and
//!   configuration, and the service user's own Git configuration: not the instance's key or
//!   database, not another sandbox.
//!
//! The restriction is inherited by everything Git starts and cannot be lifted.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use landlock::{
    ABI, Access, AccessFs, BitFlags, PathBeneath, PathFd, Ruleset, RulesetAttr, RulesetCreatedAttr,
    RulesetStatus,
};

/// What a confined Git may read besides its working folders: programs, libraries, the
/// system's configuration (Git's, the resolver's, certificates) and the resolver's files.
const SYSTEM: &[&str] = &[
    "/usr",
    "/lib",
    "/lib32",
    "/lib64",
    "/bin",
    "/sbin",
    "/etc",
    "/run/systemd/resolve",
    "/run/resolvconf",
    "/dev/urandom",
    "/dev/random",
];

/// The program loaders of the systems Yulius runs on: started by the kernel for every program,
/// so they are executed too.
const LOADERS: &[&str] = &[
    "/lib64/ld-linux-x86-64.so.2",
    "/lib/ld-linux-aarch64.so.1",
    "/lib/ld-linux-armhf.so.3",
    "/lib/ld-musl-x86_64.so.1",
    "/lib/ld-musl-aarch64.so.1",
];

/// Why Git was not started.
#[derive(Debug, thiserror::Error)]
pub enum ConfineError {
    /// The kernel has no Landlock, or it is switched off: Git is not run unconfined instead.
    #[error(
        "this kernel does not enforce Landlock, so Yulius does not run Git in a sandbox's repository: it could execute what the repository's configuration names. Linux 5.13 or later with `landlock` among its security modules is needed (see docs/operations.md)"
    )]
    NotEnforced,
    /// The rules could not be made or applied.
    #[error("confining Git failed: {0}")]
    Landlock(String),
    /// No Git was found: it is started by its path on the host, never by a bare name the
    /// confined process would look up again.
    #[error("{0} was not found on the host's PATH")]
    NotFound(PathBuf),
    /// Git itself could not be started.
    #[error("starting {program}: {source}")]
    Exec {
        /// The program.
        program: PathBuf,
        /// Why.
        source: std::io::Error,
    },
}

/// `git` as a path: itself when it names one, else the first executable file of that name in
/// an absolute folder of `PATH`.
fn resolve(git: &Path) -> Option<PathBuf> {
    use std::os::unix::fs::PermissionsExt as _;
    if git.components().count() > 1 {
        return Some(git.to_path_buf());
    }
    std::env::split_paths(&std::env::var_os("PATH")?)
        .filter(|dir| dir.is_absolute())
        .map(|dir| dir.join(git))
        .find(|candidate| {
            std::fs::metadata(candidate)
                .is_ok_and(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
        })
}

/// Git's own programs: the executable (links followed) and its `exec-path`.
fn git_programs(git: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    if let Ok(real) = std::fs::canonicalize(git) {
        found.push(real);
    }
    let asked = std::process::Command::new(git)
        .arg("--exec-path")
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output();
    if let Ok(output) = asked
        && output.status.success()
    {
        let path = String::from_utf8_lossy(&output.stdout).trim().to_owned();
        if !path.is_empty() {
            found.push(PathBuf::from(path));
        }
    }
    found
}

/// The service user's own Git configuration, which Git reads for every command.
fn own_configuration() -> Vec<PathBuf> {
    let mut found = Vec::new();
    if let Some(home) = std::env::var_os("HOME").map(PathBuf::from) {
        found.push(home.join(".gitconfig"));
        found.push(home.join(".config/git"));
    }
    if let Some(config) = std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from) {
        found.push(config.join("git"));
    }
    found
}

/// Adds a rule for each of `paths` that exists: `access` below a folder, what of it applies to
/// a file for a file.
fn allow<P: AsRef<Path>>(
    mut rules: landlock::RulesetCreated,
    paths: impl IntoIterator<Item = P>,
    access: BitFlags<AccessFs>,
    abi: ABI,
) -> Result<landlock::RulesetCreated, ConfineError> {
    for path in paths {
        let Ok(fd) = PathFd::new(path.as_ref()) else {
            continue;
        };
        let access = if path.as_ref().is_dir() {
            access
        } else {
            access & AccessFs::from_file(abi)
        };
        if access.is_empty() {
            continue;
        }
        rules = rules
            .add_rule(PathBeneath::new(fd, access))
            .map_err(|error| ConfineError::Landlock(error.to_string()))?;
    }
    Ok(rules)
}

/// The system's shell, which Git starts the other side of a transfer between two repositories
/// of this machine with (`git-upload-pack '<path>'`), and nothing else it is asked to here.
const SHELL: &str = "/bin/sh";

/// Confines this process and replaces it with `git args`: it may write (and read) below
/// `writable` and nowhere else, read `readable` and the system besides, and execute Git's own
/// programs only, with `shell` also the system's shell. Returns only when Git was not started.
#[must_use]
pub fn exec_confined(
    writable: &[PathBuf],
    readable: &[PathBuf],
    shell: bool,
    git: &Path,
    args: &[OsString],
) -> ConfineError {
    use std::os::unix::process::CommandExt as _;
    let Some(git) = resolve(git) else {
        return ConfineError::NotFound(git.to_path_buf());
    };
    let git = git.as_path();
    let abi = ABI::V5;
    let read = AccessFs::ReadFile | AccessFs::ReadDir;
    let write = AccessFs::from_all(abi) & !AccessFs::Execute;
    let programs = git_programs(git);
    let confined = (|| {
        let rules = Ruleset::default()
            .handle_access(AccessFs::from_all(abi))
            .and_then(Ruleset::create)
            .map_err(|error| ConfineError::Landlock(error.to_string()))?;
        let rules = allow(rules, SYSTEM, read, abi)?;
        let rules = allow(rules, own_configuration(), read, abi)?;
        let rules = allow(rules, &programs, read | AccessFs::Execute, abi)?;
        let rules = allow(rules, LOADERS, read | AccessFs::Execute, abi)?;
        let shells: &[&str] = if shell { &[SHELL] } else { &[] };
        let rules = allow(rules, shells, read | AccessFs::Execute, abi)?;
        let rules = allow(
            rules,
            ["/dev/null"],
            AccessFs::ReadFile | AccessFs::WriteFile,
            abi,
        )?;
        let rules = allow(rules, readable, read, abi)?;
        let rules = allow(rules, writable, write, abi)?;
        rules
            .restrict_self()
            .map_err(|error| ConfineError::Landlock(error.to_string()))
    })();
    match confined {
        Ok(status) if status.ruleset != RulesetStatus::NotEnforced => {}
        Ok(_) => return ConfineError::NotEnforced,
        Err(error) => return error,
    }
    let source = std::process::Command::new(git).args(args).exec();
    ConfineError::Exec {
        program: git.to_path_buf(),
        source,
    }
}
