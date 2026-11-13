//! Transfers of a sandbox's clone, away from the clone's configuration (SPEC-483 G2, G5,
//! ADR-0323).
//!
//! A clone's configuration is its agents' to write, and a transfer made in the clone is
//! shaped by it. A fetch from GitHub or a push to it carries the server's token (a header in
//! the adapter's own configuration), and a key for a URL in the clone beats the server's
//! command line: `http.<url>.proxy` with `http.<url>.sslVerify=false` sends the connection,
//! token included, through a proxy of the agents'. A transfer with a folder of this machine
//! (a playground's origin) starts the other side through the system's shell, and the shell
//! runs what the clone's configuration names for it: a command for a remote to fetch missing
//! objects from, for a submodule's remote, for a password, for a file's filter.
//!
//! So no transfer is made in the clone. It is made in a **courier**: a bare repository of
//! the server's own, made for the one transfer outside every sandbox's folder, whose
//! configuration nobody else writes. The courier works on the clone's objects (its commands
//! get the clone's object store as theirs, `GIT_OBJECT_DIRECTORY`), so nothing is copied and
//! what a fetch brings is in the clone at once. The refs travel as data: the server lists
//! the clone's and writes them into the courier, and after a fetch lists the courier's and
//! has the clone's Git set them (`update-ref`, a command that connects to nothing and starts
//! nothing). A push sends the clone's commit from the courier; a new clone is an empty
//! repository filled by a fetch. No command that runs in a sandbox's folder, or writes a new
//! repository there, gets the adapter's configuration or may start the shell
//! ([`crate::runner`]).

use std::collections::{BTreeMap, BTreeSet};
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
/// Where tags are.
const TAGS: &str = "refs/tags/";

static MADE: AtomicU64 = AtomicU64::new(0);

/// A repository of the server's own for one transfer; removed when dropped.
pub(crate) struct Courier {
    /// The bare repository.
    dir: PathBuf,
    /// The clone's object store, when the courier works on it.
    objects: Option<PathBuf>,
    /// The other side is a folder of this machine that Yulius may transfer with: the
    /// courier's transfer is one between folders ([`Invocation::local`]).
    folder: bool,
}

impl Drop for Courier {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn io(context: &'static str) -> impl FnOnce(std::io::Error) -> GitError {
    move |source| GitError::Io { context, source }
}

/// The refs a `for-each-ref` with `%(objectname) %(refname)` lists below `prefix`, by their
/// name without it: only lines of an object's id and a name without blanks or control
/// characters (what Git itself takes for a ref's name has neither).
fn refs_below(listed: &str, prefix: &str) -> BTreeMap<String, String> {
    listed
        .lines()
        .filter_map(|line| {
            let (id, name) = line.split_once(' ')?;
            let name = name.strip_prefix(prefix)?;
            let an_id = matches!(id.len(), 40 | 64) && id.bytes().all(|b| b.is_ascii_hexdigit());
            let a_name =
                !name.is_empty() && !name.chars().any(|c| c.is_whitespace() || c.is_control());
            (an_id && a_name).then(|| (name.to_owned(), id.to_owned()))
        })
        .collect()
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

/// What `update-ref --stdin -z` is told so that the clone's refs of the remote become
/// `now`'s and it gets the tags of `tags` it has none of: a branch that moved or is new is
/// set, one the remote dropped is deleted, a tag the clone has under any id is left alone
/// (as a fetch leaves it). `had` and `had_tags` are the clone's before the fetch. The note
/// of the remote's default branch (`HEAD`) is no branch: never set, never deleted.
fn ref_changes(
    prefix: &str,
    (had, had_tags): (&BTreeMap<String, String>, &BTreeSet<&String>),
    (now, tags): (&BTreeMap<String, String>, &BTreeMap<String, String>),
) -> Vec<u8> {
    let mut told = String::new();
    for (name, id) in now {
        if name != "HEAD" && had.get(name) != Some(id) {
            told.push_str(&format!("update {prefix}{name}\0{id}\0\0"));
        }
    }
    for name in had.keys() {
        if name != "HEAD" && !now.contains_key(name) {
            told.push_str(&format!("delete {prefix}{name}\0\0"));
        }
    }
    for (name, id) in tags {
        if !had_tags.contains(name) {
            told.push_str(&format!("create {TAGS}{name}\0{id}\0"));
        }
    }
    told.into_bytes()
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

    /// The object store of the sandbox's clone `repo` and the file that says where its
    /// history ends (`shallow`, which need not exist), as Git names them for this repository
    /// or worktree. The answer is the clone's to give, so it counts only inside the sandbox's
    /// own folders (both sides with their links followed: a data folder may be reached
    /// through one).
    fn objects_of(
        &self,
        confinement: &Confinement,
        repo: &Path,
    ) -> Result<(PathBuf, PathBuf), GitError> {
        let named = self.run_ok(Invocation::new(self, "rev-parse", Some(repo)).args([
            "rev-parse",
            "--git-path",
            "objects",
            "--git-path",
            "shallow",
        ]))?;
        let named = named.stdout_trimmed();
        let found = io("finding the clone's objects");
        let Some((objects, shallow)) = named.split_once('\n') else {
            return Err(found(std::io::Error::other("Git named no two paths")));
        };
        let objects = match std::fs::canonicalize(repo.join(objects)) {
            Ok(objects) => objects,
            Err(source) => return Err(found(source)),
        };
        let shallow = repo.join(shallow);
        let beside = shallow
            .parent()
            .and_then(|folder| std::fs::canonicalize(folder).ok());
        let own: Vec<PathBuf> = confinement
            .reach(repo)
            .iter()
            .filter_map(|folder| std::fs::canonicalize(folder).ok())
            .collect();
        let is_own = |path: &Path| own.iter().any(|folder| path.starts_with(folder));
        match (beside, shallow.file_name()) {
            (Some(beside), Some(name))
                if is_own(&objects) && is_own(&beside) && objects.to_str().is_some() =>
            {
                Ok((objects, beside.join(name)))
            }
            _ => Err(GitError::InvalidInput {
                what: "repository",
                reason: "its objects lie outside its sandbox's folders".into(),
            }),
        }
    }

    /// A courier for a transfer of `repo`'s, when `repo` lies in a sandbox's folder (its
    /// configuration is its agents'); `None` otherwise (a repository of the server's own, or a
    /// Git that is not confined: the transfer is made where it always was). With `objects`
    /// the courier works on the clone's objects (a fetch, a push); a listing needs none. With
    /// `folder` the other side is a folder of this machine Yulius may transfer with.
    ///
    /// # Errors
    /// The courier cannot be made, or the clone's objects lie outside its sandbox's folders.
    pub(crate) fn courier(
        &self,
        repo: &Path,
        objects: bool,
        folder: bool,
    ) -> Result<Option<Courier>, GitError> {
        let Some(confinement) = &self.confinement else {
            return Ok(None);
        };
        let Some((data, id)) = confinement.sandbox_of_path(repo) else {
            return Ok(None);
        };
        let borrowed = objects
            .then(|| self.objects_of(confinement, repo))
            .transpose()?;
        let (objects, shallow) = borrowed.unzip();
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
        let courier = Courier {
            dir,
            objects,
            folder,
        };
        self.run_ok(
            Invocation::new(self, "init", None)
                .args(["init", "--quiet", "--bare", "--initial-branch=main", "--"])
                .arg(&courier.dir)
                .touching(&courier.dir)
                // Its refs are files: the server writes them itself.
                .env("GIT_DEFAULT_REF_FORMAT", "files"),
        )?;
        if let Some(shallow) = shallow {
            // Where the clone's history ends is the clone's to know and, after a fetch from a
            // repository whose own history ends early, to learn: the courier's file for it
            // is a link to the clone's, which Git reads and replaces through the link.
            std::os::unix::fs::symlink(shallow, courier.dir.join("shallow"))
                .map_err(io("linking the clone's shallow file"))?;
        }
        Ok(Some(courier))
    }

    /// A command in `courier` that reaches `url`: on the clone's objects when the courier
    /// works on them, with the adapter's configuration unless the other side is a folder.
    /// `words` come before the address.
    fn carrying<'a>(
        &self,
        courier: &'a Courier,
        label: &'static str,
        words: &[&str],
        url: &str,
    ) -> Invocation<'a> {
        let mut command = Invocation::new(self, label, Some(&courier.dir))
            .args(words)
            .arg(url)
            .timeout(self.network_timeout);
        if let Some(objects) = &courier.objects {
            command = command
                .env(
                    "GIT_OBJECT_DIRECTORY",
                    objects.to_string_lossy().into_owned(),
                )
                .touching(objects);
        }
        if courier.folder {
            command = command.local(url);
        }
        command
    }

    /// The refs of the repository in `at` below each of `prefixes`, as `for-each-ref` lists
    /// them; on `objects` when given (the courier's are the clone's).
    fn listed_refs(
        &self,
        at: &Path,
        objects: Option<&PathBuf>,
        prefixes: [&str; 2],
    ) -> Result<String, GitError> {
        let mut list = Invocation::new(self, "for-each-ref", Some(at))
            .args(["for-each-ref", "--format=%(objectname) %(refname)", "--"])
            .args(prefixes);
        if let Some(objects) = objects {
            list = list
                .env(
                    "GIT_OBJECT_DIRECTORY",
                    objects.to_string_lossy().into_owned(),
                )
                .touching(objects);
        }
        Ok(self.run_ok(list)?.stdout_text())
    }

    /// Fetches `url`'s branches into `repo` as its `remote`'s (`refs/remotes/<remote>/*`,
    /// pruned) through `courier`, with the tags that come with them, as a fetch by the
    /// remote's name would:
    ///
    /// 1. the server lists what the clone has of the remote and writes it into the courier
    ///    (so that only what is new is asked for);
    /// 2. the courier fetches from `url` into the clone's object store;
    /// 3. the server lists the courier's refs and has the clone's Git set them.
    ///
    /// A clone without a note of the remote's default branch (`refs/remotes/<remote>/HEAD`)
    /// gets one. No command of these is a transfer made in the clone. Without `tags`, none
    /// come along.
    pub(crate) fn fetch_through(
        &self,
        courier: &Courier,
        repo: &Path,
        (remote, tags): (&str, bool),
        url: &str,
    ) -> Result<(), GitError> {
        let prefix = format!("refs/remotes/{remote}/");
        let listed = self.listed_refs(repo, None, [prefix.as_str(), TAGS])?;
        let (had, had_tags) = (refs_below(&listed, &prefix), refs_below(&listed, TAGS));
        let has: String = (had.iter().filter(|(name, _)| *name != "HEAD"))
            .map(|(name, id)| format!("{id} {prefix}{name}\n"))
            .chain(
                had_tags
                    .iter()
                    .map(|(name, id)| format!("{id} {TAGS}{name}\n")),
            )
            .collect();
        std::fs::write(courier.dir.join("packed-refs"), has)
            .map_err(io("telling the courier what the clone has"))?;
        // The objects are the clone's: nothing of them is tidied away by the courier, which
        // knows only the remote's branches. Tags come along as with any fetch, unless not
        // wanted.
        let mut words = vec![
            "-c",
            "gc.auto=0",
            "-c",
            "maintenance.auto=false",
            "fetch",
            "--quiet",
            "--prune",
            // A remote whose history ends early (a playground made of a shallow upload).
            "--update-shallow",
            "--no-auto-maintenance",
            "--no-recurse-submodules",
        ];
        if !tags {
            words.push("--no-tags");
        }
        words.push("--");
        self.run_ok(
            self.carrying(courier, "fetch", &words, url)
                .arg(format!("+refs/heads/*:{prefix}*")),
        )?;
        let default = if had.contains_key("HEAD") {
            None
        } else {
            let listed = self.run_ok(
                self.carrying(courier, "ls-remote", &["ls-remote", "--symref", "--"], url)
                    .arg("HEAD"),
            )?;
            remote_default(&listed.stdout_text()).map(str::to_owned)
        };
        let listed = self.listed_refs(
            &courier.dir,
            courier.objects.as_ref(),
            [prefix.as_str(), TAGS],
        )?;
        let new_tags = if tags {
            refs_below(&listed, TAGS)
        } else {
            BTreeMap::new()
        };
        let changes = ref_changes(
            &prefix,
            (&had, &had_tags.keys().collect()),
            (&refs_below(&listed, &prefix), &new_tags),
        );
        if !changes.is_empty() {
            self.run_ok(
                Invocation::new(self, "update-ref", Some(repo))
                    .args(["update-ref", "--no-deref", "--stdin", "-z"])
                    .input(changes),
            )?;
        }
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

    /// Pushes the commit `sha` of the clone (the courier works on the clone's objects) to
    /// `destination` (`refs/heads/<branch>`) at `url`, never forcing.
    pub(crate) fn push_through(
        &self,
        courier: &Courier,
        url: &str,
        sha: &str,
        destination: &str,
    ) -> Result<(), GitError> {
        self.run_ok(
            self.carrying(courier, "push", &["push", "--quiet", "--"], url)
                .arg(format!("{sha}:{destination}")),
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
            self.carrying(courier, "ls-remote", &["ls-remote", "--heads", "--"], url)
                .arg(wanted)
                .timeout(timeout),
        )?;
        Ok(listed.stdout_text())
    }

    /// Makes the clone of `url` at `dest` in a sandbox's folder. A new repository there is the
    /// sandbox's agents' to write from its first file on, so it is not made by a transfer
    /// (`git clone` connects, and checks files out, in a repository whose configuration it has
    /// already written): it is an empty repository with `url` as its origin's address, filled
    /// by a fetch through a courier, whose default branch is then checked out by a command in
    /// the clone like any other. `folder`: `url` is a folder the caller names.
    pub(crate) fn clone_into_a_sandbox(
        &self,
        url: &str,
        folder: bool,
        dest: &Path,
    ) -> Result<(), GitError> {
        self.init(dest, "main", false)?;
        self.remote_add(dest, "origin", url)?;
        let Some(courier) = self.courier(dest, true, folder)? else {
            return Err(GitError::InvalidInput {
                what: "clone destination",
                reason: "no sandbox's folder".into(),
            });
        };
        self.fetch_through(&courier, dest, ("origin", true), url)?;
        let noted = self.run(Invocation::new(self, "symbolic-ref", Some(dest)).args([
            "symbolic-ref",
            "--quiet",
            "--short",
            "refs/remotes/origin/HEAD",
        ]))?;
        // An empty remote names no branch: the clone stays without a commit, as a clone of
        // one is.
        let noted = noted.stdout_trimmed();
        if let Some(branch) = noted.strip_prefix("origin/")
            && validate_branch_name(branch).is_ok()
        {
            self.run_ok(
                Invocation::new(self, "checkout", Some(dest))
                    .args(["checkout", "--quiet", "-B", branch, "--track"])
                    .arg(format!("origin/{branch}"))
                    .timeout(self.network_timeout),
            )?;
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    fn map(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(name, id)| ((*name).to_owned(), (*id).to_owned()))
            .collect()
    }

    /// A list of refs is read as data: an object's id and a name below the prefix, and
    /// nothing else of the list.
    #[test]
    fn a_list_of_refs_is_read_line_by_line() {
        let id = "a".repeat(40);
        let long = "b".repeat(64);
        let listed = format!(
            "{id} refs/remotes/origin/main\n\
             {long} refs/remotes/origin/feat/1\n\
             {id} refs/remotes/origin/HEAD\n\
             {id} refs/remotes/other/main\n\
             {id} refs/tags/v1\n\
             {id} refs/remotes/origin/a b\n\
             {id} refs/remotes/origin/\n\
             nothex{id} refs/remotes/origin/x\n\
             abc refs/remotes/origin/short\n\
             no-space\n"
        );
        assert_eq!(
            refs_below(&listed, "refs/remotes/origin/"),
            map(&[("main", &id), ("feat/1", &long), ("HEAD", &id)])
        );
        assert_eq!(refs_below(&listed, TAGS), map(&[("v1", &id)]));
        assert!(refs_below("", TAGS).is_empty());
    }

    /// What the clone's Git is told after a fetch: a moved or new branch set, a dropped one
    /// deleted, an unchanged one and the note of the default branch left alone, and of the
    /// tags only those the clone has none of.
    #[test]
    fn the_clones_refs_follow_the_couriers() {
        let (a, b) = ("a".repeat(40), "b".repeat(40));
        let had = map(&[("main", &a), ("same", &a), ("gone", &a), ("HEAD", &a)]);
        let had_tags = map(&[("v1", &a), ("own", &a)]);
        let now = map(&[("main", &b), ("same", &a), ("new", &b), ("HEAD", &b)]);
        let tags = map(&[("v1", &b), ("v2", &b)]);
        let told = ref_changes(
            "refs/remotes/origin/",
            (&had, &had_tags.keys().collect()),
            (&now, &tags),
        );
        assert_eq!(
            String::from_utf8_lossy(&told),
            format!(
                "update refs/remotes/origin/main\0{b}\0\0\
                 update refs/remotes/origin/new\0{b}\0\0\
                 delete refs/remotes/origin/gone\0\0\
                 create refs/tags/v2\0{b}\0"
            )
        );
        // Nothing changed: nothing is told.
        assert!(
            ref_changes("p/", (&had, &had_tags.keys().collect()), (&had, &had_tags)).is_empty()
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
