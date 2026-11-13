//! Git command-line adapter: clones, worktrees, branches, bundles, diffs and hook installation
//! with safe argument boundaries (SPEC-002 GIT-001).
//!
//! The adapter is synchronous; async callers run it on blocking tasks
//! (`tokio::task::spawn_blocking`). Every command:
//!
//! - is spawned with an executable plus argv (never a shell), revisions are resolved with
//!   `rev-parse --verify --end-of-options` before reuse, paths are absolute or follow `--`,
//!   and branch names pass [`validate_branch_name`];
//! - gets a cleared environment plus a small allowlist (`PATH`, `HOME`, …), the caller's
//!   [`Git::env`], `GIT_TERMINAL_PROMPT=0` and `LC_ALL=C`; `-c` options are constants, never
//!   input;
//! - captures at most [`Git::max_output_bytes`] of stdout (64 KiB of stderr) and is killed
//!   with its whole process group, then reaped, after its timeout.
//!
//! Identity is worktree-scoped through `extensions.worktreeConfig` (see
//! [`Git::set_local_identity`]) so agents can run plain `git commit` inside their worktree.

use std::fmt;
use std::path::PathBuf;
use std::time::Duration;

pub mod agent_call;
mod bundle;
mod cd;
mod ci;
mod commit;
mod config_scrub;
#[cfg(target_os = "linux")]
pub mod confine;
mod courier;
pub mod credential;
mod error;
#[cfg(unix)]
mod handles;
mod hooks;
mod merges;
mod redact;
mod repo;
mod review;
mod runner;
mod status;
mod train;
mod validate;
mod worktree;

pub use bundle::{BundleHead, BundleSpec, IMPORT_PREFIX, ImportRef};
pub use ci::BranchTip;
pub use commit::{Author, MAX_COMMIT_MESSAGE, MergeInto, REMOTE_HEAD_TIMEOUT};
pub use credential::{run_credential_helper, run_gh_token};
pub use error::GitError;
pub use hooks::HOOKS_DIR_NAME;
pub use merges::{ATTRIBUTES_BEGIN, ATTRIBUTES_END, SquashInput, SquashLanding};
pub use redact::redact;
pub use review::{
    CommitStat, DiffHunk, DiffLine, FileDiff, FileStatus, LineKind, MAX_REVIEW_COMMITS,
    MAX_REVIEW_FILES, MAX_REVIEW_LINES, WorktreeDiff,
};
pub use status::{AheadBehind, Commit, DiffStat, MAX_LOG_ENTRIES, StatusEntry};
pub use train::{TrainBuild, TrainInput, TrainLanding, TrainStep};
pub use validate::{MAX_BRANCH_LEN, validate_branch_name, validate_relative_path};
pub use worktree::{RemovalRefusal, RemoveBelow, Worktree, WorktreeRemoval};

/// Default deadline for local commands.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(120);
/// Default deadline for network commands (clone, fetch, push).
pub const DEFAULT_NETWORK_TIMEOUT: Duration = Duration::from_secs(30 * 60);
/// Default bound on captured standard output.
pub const DEFAULT_MAX_OUTPUT_BYTES: usize = 16 * 1024 * 1024;

/// Handle to a Git executable with its environment and bounds.
///
/// `env` is for non-secret configuration only (for example `GIT_CONFIG_COUNT`/`KEY`/`VALUE`
/// entries that register the `yulius git-credential` helper); its values are not shown by
/// `Debug`.
#[derive(Clone)]
pub struct Git {
    /// Path or name of the `git` executable.
    pub exe: PathBuf,
    /// Extra environment variables for every command (applied after the allowlist).
    pub env: Vec<(String, String)>,
    /// Deadline for local commands.
    pub timeout: Duration,
    /// Deadline for clone, fetch, push and bundle fetches.
    pub network_timeout: Duration,
    /// Bound on captured stdout; larger output kills the command.
    pub max_output_bytes: usize,
    /// How every command is confined (ADR-0322); `None` runs Git as it is (tests of this
    /// crate, a system without Landlock support in Yulius).
    pub confinement: Option<std::sync::Arc<Confinement>>,
}

/// How the host's own Git is confined (ADR-0322): each command is started through
/// `<launcher> git-confined`, which lets it execute Git's own programs only and touch only the
/// folders the command works in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Confinement {
    /// The program that confines and then starts Git: `yulius` itself.
    pub launcher: PathBuf,
    /// The instance's data folder, or with [`Self::data_depth`] a folder that many lie in.
    pub data: PathBuf,
    /// How many path components below [`Self::data`] name a data folder: 0 on a server; 1 in
    /// tests, whose data folders are the children of the system's temporary folder.
    pub data_depth: usize,
    /// Folders every command may also read and write (a test's stand-in for GitHub).
    pub shared: Vec<PathBuf>,
    /// Every command may start the system's shell, as one that transfers between two local
    /// repositories must, and a remote's configured address is taken as it stands when it is
    /// a folder: only for a test's stand-in for GitHub, which makes every transfer with
    /// "GitHub" a local one.
    pub shell: bool,
}

/// The folders of a data folder that hold, by sandbox id, what a sandbox's agents write and
/// Git works in: its clone and task worktrees, and the worktrees of its imports. A command
/// working in one of a sandbox's gets all of that sandbox's and no other sandbox's.
const SANDBOX_FOLDERS: [&str; 2] = ["sandboxes", "import-worktrees"];
/// The folder of a data folder with the origins that are folders of this machine: the sandbox
/// `<id>` has `<id>.git` there and no other. A clone's configured address is its agents' to
/// write, so no other folder is taken from it.
const ORIGINS_FOLDER: &str = "playgrounds";

impl Confinement {
    /// The data folder and sandbox id of `path`, when it lies in a sandbox's folder, and that
    /// folder (`<data>/sandboxes/<id>` or `<data>/import-worktrees/<id>`).
    fn sandbox_folder_of(&self, path: &std::path::Path) -> Option<(PathBuf, String, PathBuf)> {
        let below = path.strip_prefix(&self.data).ok()?;
        let mut parts = below.components();
        let mut data = self.data.clone();
        for _ in 0..self.data_depth {
            data.push(parts.next()?);
        }
        let mut name = || match parts.next()? {
            std::path::Component::Normal(name) => Some(name.to_str()?.to_owned()),
            _ => None,
        };
        let (folder, id) = (name()?, name()?);
        let own = data.join(&folder).join(&id);
        SANDBOX_FOLDERS
            .contains(&folder.as_str())
            .then_some((data, id, own))
    }

    /// The data folder and sandbox id of a path as a caller gives it (made absolute first),
    /// when it lies in a sandbox's folder: `Some` says that the folder is a sandbox's, whose
    /// every file, a clone's configuration included, its agents write.
    #[must_use]
    pub fn sandbox_of_path(&self, path: &std::path::Path) -> Option<(PathBuf, String)> {
        self.sandbox_folder_of(&plain(path))
            .map(|(data, id, _)| (data, id))
    }

    /// What a command touching `path` may read and write for it: its sandbox's folders when
    /// `path` lies in one of them (of a sandbox that has none yet, the one `path` lies in,
    /// which the caller makes: never the folder all sandboxes are in), else `path` itself.
    /// What is still to be made elsewhere (a new repository, a file) is made in the nearest
    /// folder that exists. Nothing is made here.
    #[must_use]
    pub fn reach(&self, path: &std::path::Path) -> Vec<PathBuf> {
        let path = plain(path);
        let nearest = |mut folder: PathBuf| {
            while !folder.is_dir() {
                match folder.parent() {
                    Some(parent) if parent != folder => folder = parent.to_path_buf(),
                    _ => break,
                }
            }
            folder
        };
        match self.sandbox_folder_of(&path) {
            Some((data, id, lies_in)) => {
                let own: Vec<PathBuf> = SANDBOX_FOLDERS
                    .iter()
                    .map(|folder| data.join(folder).join(&id))
                    .filter(|folder| folder.is_dir())
                    .collect();
                if own.is_empty() { vec![lies_in] } else { own }
            }
            None => vec![nearest(path)],
        }
    }

    /// The folder a transfer with `repo`'s remote goes to, when the address its configuration
    /// gives, `url`, is a folder of this machine that may be taken from it: the sandbox's own
    /// origin (or, with [`Self::shell`], any folder).
    #[must_use]
    pub fn local_origin(&self, repo: &std::path::Path, url: &str) -> Option<PathBuf> {
        let path = std::path::Path::new(url);
        if !path.is_absolute() {
            return None;
        }
        if self.shell {
            return Some(path.to_path_buf());
        }
        let (data, id) = self.sandbox_of_path(repo)?;
        let origin = data.join(ORIGINS_FOLDER).join(format!("{id}.git"));
        (path == origin).then_some(origin)
    }
}

/// `path` made absolute, without its `.` and `..` parts (as written: no link is followed).
/// Which sandbox a path lies in is read from its parts, so `..` must not be one of them.
fn plain(path: &std::path::Path) -> PathBuf {
    let absolute = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
    let mut plain = PathBuf::new();
    for part in absolute.components() {
        match part {
            std::path::Component::ParentDir => {
                plain.pop();
            }
            std::path::Component::CurDir => {}
            other => plain.push(other),
        }
    }
    plain
}

impl Git {
    /// Creates a handle for `exe` with default bounds and no extra environment.
    #[must_use]
    pub fn new(exe: impl Into<PathBuf>) -> Self {
        Self {
            exe: exe.into(),
            env: Vec::new(),
            timeout: DEFAULT_TIMEOUT,
            network_timeout: DEFAULT_NETWORK_TIMEOUT,
            max_output_bytes: DEFAULT_MAX_OUTPUT_BYTES,
            confinement: None,
        }
    }

    /// Confines every command (ADR-0322).
    #[must_use]
    pub fn confined(mut self, confinement: std::sync::Arc<Confinement>) -> Self {
        self.confinement = Some(confinement);
        self
    }

    /// Adds one environment variable for every command.
    #[must_use]
    pub fn with_env(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.env.push((key.into(), value.into()));
        self
    }
}

impl Default for Git {
    fn default() -> Self {
        Self::new("git")
    }
}

impl fmt::Debug for Git {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let keys: Vec<&str> = self.env.iter().map(|(key, _)| key.as_str()).collect();
        f.debug_struct("Git")
            .field("exe", &self.exe)
            .field("env_keys", &keys)
            .field("timeout", &self.timeout)
            .field("network_timeout", &self.network_timeout)
            .field("max_output_bytes", &self.max_output_bytes)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn confinement(data: &std::path::Path, data_depth: usize) -> Confinement {
        Confinement {
            launcher: "yulius".into(),
            data: data.to_path_buf(),
            data_depth,
            shared: Vec::new(),
            shell: false,
        }
    }

    /// ADR-0322: a command working anywhere in a sandbox's folders gets that sandbox's (its
    /// clone and worktrees, and the worktrees of its imports) and no other sandbox's; one
    /// working elsewhere gets the folder it names, or the nearest one that exists.
    #[test]
    fn a_command_reaches_its_own_sandboxes_folders() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let data = dir.path().canonicalize()?;
        for folder in [
            "sandboxes/s1/repo/.git",
            "sandboxes/s1/worktrees/t1",
            "sandboxes/s2/repo",
            "import-worktrees/s1/import-7",
            "playgrounds/s1.git",
            "cd/s1",
        ] {
            std::fs::create_dir_all(data.join(folder))?;
        }
        let server = confinement(&data, 0);
        let own = vec![data.join("sandboxes/s1"), data.join("import-worktrees/s1")];
        for inside in [
            "sandboxes/s1/repo",
            "sandboxes/s1/worktrees/t1",
            "sandboxes/s1/worktrees/not-made-yet",
            "import-worktrees/s1/import-7",
            "sandboxes/s1",
        ] {
            assert_eq!(server.reach(&data.join(inside)), own, "{inside}");
        }
        // A sandbox without imports has no folder of them to name.
        assert_eq!(
            server.reach(&data.join("sandboxes/s2/repo")),
            vec![data.join("sandboxes/s2")]
        );
        // Outside a sandbox: the folder itself; what is still to be made, where it is made.
        assert_eq!(
            server.reach(&data.join("playgrounds/s1.git")),
            vec![data.join("playgrounds/s1.git")]
        );
        assert_eq!(
            server.reach(&data.join("playgrounds/new.git")),
            vec![data.join("playgrounds")]
        );
        // A sandbox that is being made gets its own folder, not all sandboxes'; a command for
        // a sandbox that is gone does not bring its folder back.
        assert_eq!(
            server.reach(&data.join("sandboxes/s9/repo")),
            vec![data.join("sandboxes/s9")]
        );
        assert!(!data.join("sandboxes/s9").exists());
        // `..` names no sandbox: the path counts as what it leads to.
        assert_eq!(
            server.reach(&data.join("sandboxes/../cd/s1")),
            vec![data.join("cd/s1")]
        );
        assert_eq!(server.reach(&data.join("sandboxes/s2/../s1/repo")), own);
        assert_eq!(server.sandbox_of_path(&data.join("sandboxes/..")), None);
        assert_eq!(server.reach(&data.join("cd/s1")), vec![data.join("cd/s1")]);
        // The folder of all sandboxes is no sandbox.
        assert_eq!(
            server.reach(&data.join("sandboxes")),
            vec![data.join("sandboxes")]
        );

        // Tests: the data folders are the children of a folder.
        let parent = data.parent().ok_or("parent")?;
        let tests = confinement(parent, 1);
        assert_eq!(tests.reach(&data.join("sandboxes/s1/repo")), own);
        // On a server, a folder that only looks like one is nothing special.
        assert_eq!(
            confinement(parent, 0).reach(&data.join("sandboxes/s1/repo")),
            vec![data.join("sandboxes/s1/repo")]
        );
        Ok(())
    }

    /// Of the folders of this machine, the only origin taken from a clone's configuration is
    /// the sandbox's own (`<data>/playgrounds/<id>.git`): not another sandbox's, not a clone,
    /// not a folder of the host, and none for a repository outside the sandboxes.
    #[test]
    fn only_a_sandboxes_own_origin_is_a_local_one() -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let data = dir.path().canonicalize()?;
        std::fs::create_dir_all(data.join("sandboxes/s1/worktrees/t1"))?;
        std::fs::create_dir_all(data.join("import-worktrees/s1/import-7"))?;
        let server = confinement(&data, 0);
        let own = data.join("playgrounds/s1.git");
        let asked = |repo: &str, url: &std::path::Path| {
            server.local_origin(&data.join(repo), &url.to_string_lossy())
        };
        for repo in [
            "sandboxes/s1/repo",
            "sandboxes/s1/worktrees/t1",
            "import-worktrees/s1/import-7",
        ] {
            assert_eq!(asked(repo, &own), Some(own.clone()), "{repo}");
            for other in [
                data.join("playgrounds/s2.git"),
                data.join("playgrounds/s1.git/objects"),
                data.join("playgrounds"),
                data.join("sandboxes/s2/repo"),
                data.clone(),
                std::path::PathBuf::from("/"),
            ] {
                assert_eq!(asked(repo, &other), None, "{repo} -> {}", other.display());
            }
        }
        assert_eq!(
            asked("playgrounds/s1.git", &own),
            None,
            "no sandbox's repository"
        );
        assert_eq!(asked("cd/s1", &own), None);
        // An address that is no folder of this machine is none.
        for url in ["https://github.com/acme/app.git", "origin", "../s1.git", ""] {
            assert_eq!(
                server.local_origin(&data.join("sandboxes/s1/repo"), url),
                None,
                "{url}"
            );
        }
        // The tests' stand-in for GitHub: any folder.
        let seam = Confinement {
            shell: true,
            ..server.clone()
        };
        assert_eq!(
            seam.local_origin(&data.join("sandboxes/s1/repo"), "/tmp/origin.git"),
            Some("/tmp/origin.git".into())
        );
        Ok(())
    }
}
