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
//! writes. It borrows the clone's objects (an alternate), so nothing is copied to make it,
//! and no process of the clone's ever writes in it: what the clone has of the remote is read
//! as a list and written into the courier by the server. A fetch brings the remote's branches
//! into the courier (with the token), and the clone then takes them from the courier as from
//! any folder (without it); a push sends the clone's commit from the courier; a new clone is
//! made in a courier and copied from there. No command that runs in a sandbox's folder, or
//! writes a new repository there, gets the adapter's configuration at all
//! ([`crate::runner`]).

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime};

use crate::error::GitError;
use crate::runner::Invocation;
use crate::validate::validate_branch_name;
use crate::{Confinement, Git};

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
    /// The clone's object store, when the courier borrows it.
    objects: Option<PathBuf>,
}

impl Drop for Courier {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn io(context: &'static str) -> impl FnOnce(std::io::Error) -> GitError {
    move |source| GitError::Io { context, source }
}

/// An empty folder of the server's own for a courier of the sandbox `id`, in `data`.
fn place(data: &Path, id: &str) -> Result<Courier, GitError> {
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
    // Made here, so that a command reaches this courier and not the folder of all of them.
    std::fs::create_dir(&dir).map_err(io("making a courier"))?;
    Ok(Courier { dir, objects: None })
}

/// The lines of a `packed-refs` file for what `listed` (`for-each-ref` with
/// `%(objectname) %(refname)`) names below `prefix`, and whether it names `<prefix>HEAD`
/// (left out: it is the clone's note of the remote's default branch, no branch).
fn refs_as_data(listed: &str, prefix: &str) -> (String, bool) {
    let (mut refs, mut head) = (String::new(), false);
    for line in listed.lines() {
        let Some((id, name)) = line.split_once(' ') else {
            continue;
        };
        let an_id = matches!(id.len(), 40 | 64) && id.bytes().all(|b| b.is_ascii_hexdigit());
        let Some(branch) = name.strip_prefix(prefix) else {
            continue;
        };
        if branch == "HEAD" {
            head = true;
        } else if an_id && validate_branch_name(branch).is_ok() {
            refs.push_str(&format!("{id} {name}\n"));
        }
    }
    (refs, head)
}

/// The branch `ls-remote --symref <url> HEAD` says the remote's `HEAD` names, when it names
/// one that exists there.
fn remote_default(listed: &str) -> Option<&str> {
    let mut named = None;
    let mut exists = false;
    for line in listed.lines() {
        match line.split_once('\t') {
            Some((target, "HEAD")) if target.starts_with("ref: ") => {
                named = target.strip_prefix("ref: refs/heads/");
            }
            Some((_, "HEAD")) => exists = true,
            _ => {}
        }
    }
    named.filter(|branch| exists && validate_branch_name(branch).is_ok())
}

impl Git {
    /// The address `repo`'s configuration gives its remote `remote` (`remote.<name>.url`, the
    /// last when it gives several), read without the adapter's own configuration. The other
    /// keys Git would make an address of (a rewrite, an address for pushes) are not read: the
    /// server's transfers go to this one.
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

    /// The object store of the sandbox's clone `repo`, as Git names it for this repository or
    /// worktree. The answer is the clone's to give, so it counts only inside the sandbox's own
    /// folders (both sides with their links followed: a data folder may be reached through
    /// one), and only as one line of the file that lends it.
    fn objects_to_borrow(
        &self,
        confinement: &Confinement,
        repo: &Path,
    ) -> Result<PathBuf, GitError> {
        let named = self.run_ok(Invocation::new(self, "rev-parse", Some(repo)).args([
            "rev-parse",
            "--git-path",
            "objects",
        ]))?;
        let objects = std::fs::canonicalize(repo.join(named.stdout_trimmed()))
            .map_err(io("finding the clone's objects"))?;
        let refused = |reason: &str| GitError::InvalidInput {
            what: "repository",
            reason: reason.into(),
        };
        let own = confinement
            .reach(repo)
            .iter()
            .filter_map(|folder| std::fs::canonicalize(folder).ok())
            .any(|folder| objects.starts_with(folder));
        if !own {
            return Err(refused("its objects lie outside its sandbox's folders"));
        }
        if objects
            .to_str()
            .is_none_or(|path| path.chars().any(char::is_control))
        {
            return Err(refused(
                "the path of its objects cannot be written on one line",
            ));
        }
        Ok(objects)
    }

    /// A courier for a transfer of `repo`'s, when `repo` lies in a sandbox's folder (its
    /// configuration is its agents'); `None` otherwise (a repository of the server's own, or a
    /// Git that is not confined: the transfer is made where it always was). With `objects` the
    /// courier borrows the clone's objects (a fetch, a push); a listing needs none.
    ///
    /// # Errors
    /// The courier cannot be made, or the clone's objects lie outside its sandbox's folders.
    pub(crate) fn courier(&self, repo: &Path, objects: bool) -> Result<Option<Courier>, GitError> {
        let Some(confinement) = &self.confinement else {
            return Ok(None);
        };
        let Some((data, id)) = confinement.sandbox_of_path(repo) else {
            return Ok(None);
        };
        let objects = objects
            .then(|| self.objects_to_borrow(confinement, repo))
            .transpose()?;
        let mut courier = place(&data, &id)?;
        self.run_ok(
            Invocation::new(self, "init", None)
                .args(["init", "--quiet", "--bare", "--initial-branch=main", "--"])
                .arg(&courier.dir)
                .touching(&courier.dir)
                // Its refs are files: the server writes them itself (`refs_as_data`).
                .env("GIT_DEFAULT_REF_FORMAT", "files"),
        )?;
        if let Some(objects) = objects {
            std::fs::write(
                courier.dir.join("objects/info/alternates"),
                format!("{}\n", objects.display()),
            )
            .map_err(io("lending the clone's objects to the courier"))?;
            courier.objects = Some(objects);
        }
        Ok(Some(courier))
    }

    /// A command in `courier` that reaches `url` with the adapter's configuration.
    fn carrying<'a>(&self, courier: &'a Courier, label: &'static str) -> Invocation<'a> {
        let command = Invocation::new(self, label, Some(&courier.dir));
        match &courier.objects {
            Some(objects) => command.touching(objects),
            None => command,
        }
    }

    /// Fetches `url`'s branches into `repo` as its `remote`'s (`refs/remotes/<remote>/*`,
    /// pruned) through `courier`: the courier is told what the clone already has, fetches from
    /// `url` with the adapter's configuration, and the clone takes the result from the courier.
    /// A clone that has no note of the remote's default branch (`refs/remotes/<remote>/HEAD`)
    /// gets one, as a fetch by the remote's name gives it.
    pub(crate) fn fetch_through(
        &self,
        courier: &Courier,
        repo: &Path,
        remote: &str,
        url: &str,
    ) -> Result<(), GitError> {
        let prefix = format!("refs/remotes/{remote}/");
        let tracking = format!("{prefix}*");
        // What the clone has of the remote, so that only what is new is asked for: a list the
        // clone's Git prints and the server writes down. Its objects are the courier's already
        // (the alternate), so no object is copied and no process of the clone's is started.
        let listed = self.run_ok(Invocation::new(self, "for-each-ref", Some(repo)).args([
            "for-each-ref",
            "--format=%(objectname) %(refname)",
            "--",
            prefix.as_str(),
        ]))?;
        let (has, head_noted) = refs_as_data(&listed.stdout_text(), &prefix);
        std::fs::write(courier.dir.join("packed-refs"), has)
            .map_err(io("telling the courier what the clone has"))?;
        self.run_ok(
            self.carrying(courier, "fetch")
                .args([
                    "fetch",
                    "--quiet",
                    "--prune",
                    "--no-recurse-submodules",
                    "--",
                    url,
                ])
                .arg(format!("+refs/heads/*:{tracking}"))
                .timeout(self.network_timeout),
        )?;
        // Everything that carries the token comes before the clone's own Git works with the
        // courier (it may write there): also the question for the remote's default branch.
        let default = if head_noted {
            None
        } else {
            let listed = self.run_ok(
                self.carrying(courier, "ls-remote")
                    .args(["ls-remote", "--symref", "--", url, "HEAD"])
                    .timeout(self.network_timeout),
            )?;
            remote_default(&listed.stdout_text()).map(str::to_owned)
        };
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
        if let Some(branch) = default {
            self.run_ok(
                Invocation::new(self, "symbolic-ref", Some(repo))
                    .args(["symbolic-ref", "--"])
                    .arg(format!("{prefix}HEAD"))
                    .arg(format!("{prefix}{branch}")),
            )?;
        }
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
            self.carrying(courier, "push")
                .args(["push", "--quiet", "--", url])
                .arg(format!("{sha}:{destination}"))
                .timeout(self.network_timeout),
        )?;
        Ok(())
    }

    /// What `url` lists for `wanted`, asked in `courier`, within `timeout`.
    pub(crate) fn list_through(
        &self,
        courier: &Courier,
        url: &str,
        wanted: &str,
        timeout: Duration,
    ) -> Result<String, GitError> {
        let listed = self.run_ok(
            self.carrying(courier, "ls-remote")
                .args(["ls-remote", "--heads", "--", url, wanted])
                .timeout(timeout),
        )?;
        Ok(listed.stdout_text())
    }

    /// Clones `url` into `dest` in the folder of the sandbox `id` of `data`, when `dest` is to
    /// be made there by a clone that carries the adapter's configuration: the clone is made
    /// in a courier, and `dest` is a copy of the courier with `url` as its origin's address.
    /// A new repository's configuration is written before its objects arrive, and in a
    /// sandbox's folder the sandbox's agents could add to it meanwhile.
    pub(crate) fn clone_through(
        &self,
        (data, id): (&Path, &str),
        url: &str,
        dest: &Path,
    ) -> Result<(), GitError> {
        let courier = place(data, id)?;
        self.run_ok(
            Invocation::new(self, "clone", None)
                .args([
                    "clone",
                    "--quiet",
                    "--bare",
                    "--no-recurse-submodules",
                    "--",
                    url,
                ])
                .arg(&courier.dir)
                .touching(&courier.dir)
                .timeout(self.network_timeout),
        )?;
        self.run_ok(
            Invocation::new(self, "clone", None)
                .args(["clone", "--quiet", "--no-recurse-submodules", "--"])
                .arg(&courier.dir)
                .arg(dest)
                .touching(dest)
                .local(&courier.dir)
                .timeout(self.network_timeout),
        )?;
        self.set_remote_url(dest, "origin", url)
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

#[cfg(test)]
mod tests {
    use super::*;

    /// What the clone has of its remote is written into the courier by the server: branches
    /// with an object's id and a name Git takes for a branch, and nothing else of the list.
    #[test]
    fn the_clones_branches_become_lines_of_the_couriers_refs() {
        let id = "a".repeat(40);
        let long = "b".repeat(64);
        let listed = format!(
            "{id} refs/remotes/origin/main\n\
             {long} refs/remotes/origin/feat/1\n\
             {id} refs/remotes/origin/HEAD\n\
             {id} refs/remotes/other/main\n\
             {id} refs/heads/main\n\
             {id} refs/remotes/origin/a b\n\
             {id} refs/remotes/origin/..\n\
             {id} refs/remotes/origin/\n\
             nothex{id} refs/remotes/origin/x\n\
             abc refs/remotes/origin/short\n\
             no-space\n"
        );
        let (refs, head) = refs_as_data(&listed, "refs/remotes/origin/");
        assert_eq!(
            refs,
            format!("{id} refs/remotes/origin/main\n{long} refs/remotes/origin/feat/1\n")
        );
        assert!(head);
        assert_eq!(
            refs_as_data(
                &format!("{id} refs/remotes/origin/main\n"),
                "refs/remotes/origin/"
            ),
            (format!("{id} refs/remotes/origin/main\n"), false)
        );
    }

    /// The remote's default branch is the one its `HEAD` names, when `HEAD` exists there.
    #[test]
    fn the_remotes_head_names_its_default_branch() {
        let id = "a".repeat(40);
        assert_eq!(
            remote_default(&format!("ref: refs/heads/trunk\tHEAD\n{id}\tHEAD\n")),
            Some("trunk")
        );
        // An empty repository, a detached `HEAD`, a `HEAD` that names no branch, a name Git
        // refuses, another ref's line.
        for listed in [
            String::new(),
            format!("{id}\tHEAD\n"),
            "ref: refs/heads/trunk\tHEAD\n".to_owned(),
            format!("ref: refs/tags/v1\tHEAD\n{id}\tHEAD\n"),
            format!("ref: refs/heads/a..b\tHEAD\n{id}\tHEAD\n"),
            format!("ref: refs/heads/trunk\trefs/remotes/x/HEAD\n{id}\tHEAD\n"),
        ] {
            assert_eq!(remote_default(&listed), None, "{listed:?}");
        }
    }
}
