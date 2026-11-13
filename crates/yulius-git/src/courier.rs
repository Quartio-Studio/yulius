//! Transfers that carry the server's token, away from the clone's configuration (SPEC-483 G2,
//! ADR-0323).
//!
//! A fetch from GitHub or a push to it carries the server's token (a header in the adapter's
//! own configuration). Run in a sandbox's clone, the connection is shaped by the clone's
//! configuration, which its agents write, and a key for a URL there beats the server's command
//! line: `http.<url>.proxy` with `http.<url>.sslVerify=false` sends the connection, token
//! included, through a proxy of the agents' without a certificate check.
//!
//! So such a transfer is made in a **courier**: a bare repository of the server's own, made
//! for the one transfer outside every sandbox's folder, whose configuration nobody else
//! writes. It borrows the clone's objects (an alternate), so nothing is copied to make it.
//! A fetch brings the remote's branches into the courier (with the token), and the clone then
//! takes them from the courier as from any folder (without it); a push sends the clone's
//! commit from the courier. No command that runs in a sandbox's folder gets the adapter's
//! configuration at all ([`crate::runner`]).

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime};

use crate::Git;
use crate::error::GitError;
use crate::runner::Invocation;

/// The folder of a data folder that holds the couriers.
const COURIERS_FOLDER: &str = "git-couriers";
/// A courier older than this was left by a server that stopped mid-transfer: the next one
/// removes it.
const LEFT_BEHIND: Duration = Duration::from_secs(24 * 3600);
/// Entries of the couriers' folder looked at for ones left behind, per new courier.
const SWEEP_MAX: usize = 256;

static MADE: AtomicU64 = AtomicU64::new(0);

/// A repository of the server's own for one transfer; removed when dropped.
pub(crate) struct Courier {
    /// The bare repository.
    pub(crate) dir: PathBuf,
    /// The clone's object store, which the courier borrows.
    pub(crate) objects: PathBuf,
}

impl Drop for Courier {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn io(context: &'static str) -> impl FnOnce(std::io::Error) -> GitError {
    move |source| GitError::Io { context, source }
}

impl Git {
    /// The address `repo`'s configuration gives its remote `remote`, read without the
    /// adapter's own configuration.
    pub(crate) fn remote_url(&self, repo: &Path, remote: &str) -> Option<String> {
        let asked = self
            .run(
                Invocation::new(self, "config", Some(repo))
                    .args(["config", "--get"])
                    .arg(format!("remote.{remote}.url")),
            )
            .ok()?;
        let url = asked.stdout_trimmed();
        (asked.status.success() && !url.is_empty()).then_some(url)
    }

    /// A courier for a transfer of `repo`'s, when `repo` lies in a sandbox's folder (its
    /// configuration is its agents'); `None` otherwise (a repository of the server's own, or a
    /// Git that is not confined: the transfer is made where it always was).
    ///
    /// # Errors
    /// The courier cannot be made, or the clone's objects lie outside its sandbox's folders.
    pub(crate) fn courier(&self, repo: &Path) -> Result<Option<Courier>, GitError> {
        let Some(confinement) = &self.confinement else {
            return Ok(None);
        };
        let Some((data, id)) = confinement.sandbox_of_path(repo) else {
            return Ok(None);
        };
        // The clone's object store, as Git names it for this repository or worktree. The
        // answer is the clone's to give, so it counts only inside the sandbox's own folders.
        let named = self.run_ok(Invocation::new(self, "rev-parse", Some(repo)).args([
            "rev-parse",
            "--git-path",
            "objects",
        ]))?;
        let objects = std::fs::canonicalize(repo.join(named.stdout_trimmed()))
            .map_err(io("finding the clone's objects"))?;
        if !confinement
            .reach(repo)
            .iter()
            .any(|own| objects.starts_with(own))
        {
            return Err(GitError::InvalidInput {
                what: "repository",
                reason: "its objects lie outside its sandbox's folders".into(),
            });
        }
        let couriers = data.join(COURIERS_FOLDER);
        std::fs::create_dir_all(&couriers).map_err(io("making the couriers' folder"))?;
        sweep(&couriers);
        let dir = couriers.join(format!(
            "{id}-{}-{}-{}.git",
            std::process::id(),
            MADE.fetch_add(1, Ordering::Relaxed),
            SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .map_or(0, |since| since.subsec_nanos())
        ));
        self.init(&dir, "main", true)?;
        let courier = Courier { dir, objects };
        std::fs::write(
            courier.dir.join("objects/info/alternates"),
            format!("{}\n", courier.objects.display()),
        )
        .map_err(io("lending the clone's objects to the courier"))?;
        Ok(Some(courier))
    }

    /// Fetches `url`'s branches into `repo` as its `remote`'s (`refs/remotes/<remote>/*`,
    /// pruned) through `courier`: the courier learns what the clone already has, fetches from
    /// `url` with the adapter's configuration, and the clone takes the result from the courier.
    pub(crate) fn fetch_through(
        &self,
        courier: &Courier,
        repo: &Path,
        remote: &str,
        url: &str,
    ) -> Result<(), GitError> {
        let tracking = format!("refs/remotes/{remote}/*");
        // What the clone has of the remote, so that only what is new is asked for. Its
        // objects are the courier's already (the alternate): no object is copied.
        self.run_ok(
            Invocation::new(self, "fetch", Some(&courier.dir))
                .args([
                    "fetch",
                    "--quiet",
                    "--no-tags",
                    "--no-recurse-submodules",
                    "--",
                ])
                .arg(repo)
                .arg(format!("+{tracking}:{tracking}"))
                .local(repo)
                .timeout(self.network_timeout),
        )?;
        self.run_ok(
            Invocation::new(self, "fetch", Some(&courier.dir))
                .args([
                    "fetch",
                    "--quiet",
                    "--prune",
                    "--no-recurse-submodules",
                    "--",
                    url,
                ])
                .arg(format!("+refs/heads/*:{tracking}"))
                .touching(&courier.objects)
                .timeout(self.network_timeout),
        )?;
        self.run_ok(
            Invocation::new(self, "fetch", Some(repo))
                .args([
                    "fetch",
                    "--quiet",
                    "--prune",
                    "--no-recurse-submodules",
                    "--",
                ])
                .arg(&courier.dir)
                .arg(format!("+{tracking}:{tracking}"))
                .local(&courier.dir)
                .timeout(self.network_timeout),
        )?;
        Ok(())
    }

    /// Pushes the commit `sha` of the clone (the courier has it through the alternate) to
    /// `destination` (`refs/heads/<branch>`) at `url`, never forcing.
    pub(crate) fn push_through(
        &self,
        courier: &Courier,
        url: &str,
        sha: &str,
        destination: &str,
    ) -> Result<(), GitError> {
        self.run_ok(
            Invocation::new(self, "push", Some(&courier.dir))
                .args(["push", "--quiet", "--", url])
                .arg(format!("{sha}:{destination}"))
                .touching(&courier.objects)
                .timeout(self.network_timeout),
        )?;
        Ok(())
    }
}

/// Removes couriers left behind by a server that stopped mid-transfer (best effort, bounded).
fn sweep(couriers: &Path) {
    let Ok(entries) = std::fs::read_dir(couriers) else {
        return;
    };
    for entry in entries.flatten().take(SWEEP_MAX) {
        let old = entry
            .metadata()
            .and_then(|meta| meta.modified())
            .ok()
            .and_then(|at| at.elapsed().ok())
            .is_some_and(|age| age > LEFT_BEHIND);
        if old {
            let _ = std::fs::remove_dir_all(entry.path());
        }
    }
}
