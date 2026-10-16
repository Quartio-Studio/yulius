//! `yulius remove`, run by a task's agent (SPEC-192, ADR-0155): removes paths this task created,
//! where Claude Code's own removal check would stop `rm` and wait for a person (a root-level
//! file, a path held in a variable). It runs in the agent's run, so it sees the run's own
//! filesystem, which neither the Yulius server nor a person can reach, and removes a path only
//! when its location proves it is the task's: an untracked or ignored path of the task's worktree,
//! or a path on the run's own tmpfs (`/`, `/tmp`, `/run`, `/dev/shm` of a contained run).

use std::io::Write;
use std::path::{Path, PathBuf};

/// Where the run's own tmpfs can be mounted in a contained run (SPEC-002 ADR-22: `bwrap` makes a
/// fresh root and a fresh `/tmp` for every run, and a joined run a fresh `/run` and `/dev/shm`).
const RUN_PRIVATE_MOUNTS: [&str; 4] = ["/", "/tmp", "/run", "/dev/shm"];

/// The root's links containment makes for the merged `/usr`: the run needs them.
const ROOT_LINKS: [&str; 4] = ["/bin", "/sbin", "/lib", "/lib64"];

/// What `bwrap` mounts as a run's root: the `newroot` directory of its own tmpfs.
const BWRAP_ROOT: &str = "/newroot";

/// Git's variables that would point `git ls-files` at another repository or index.
const GIT_ENV: [&str; 6] = [
    "GIT_DIR",
    "GIT_WORK_TREE",
    "GIT_INDEX_FILE",
    "GIT_COMMON_DIR",
    "GIT_OBJECT_DIRECTORY",
    "GIT_ALTERNATE_OBJECT_DIRECTORIES",
];

/// One line of `/proc/self/mountinfo`: where a filesystem is mounted and its type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mount {
    /// The mounted directory of its filesystem (`/newroot` for a `bwrap` root).
    pub root: PathBuf,
    /// Mount point.
    pub point: PathBuf,
    /// Filesystem type (`tmpfs`, `overlay`, `ext4`, …).
    pub fstype: String,
}

/// Why a path belongs to the task.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Owner {
    /// An untracked or ignored path of the task's worktree.
    Worktree,
    /// A path on the run's own tmpfs.
    RunPrivate,
}

/// The mounts of a `/proc/self/mountinfo` text, in its order (a later mount covers an earlier
/// one at the same point). Lines that do not parse are skipped.
#[must_use]
pub fn parse_mountinfo(text: &str) -> Vec<Mount> {
    text.lines()
        .filter_map(|line| {
            let fields: Vec<&str> = line.split(' ').collect();
            let root = fields.get(3)?;
            let point = fields.get(4)?;
            let separator = fields.iter().position(|field| *field == "-")?;
            let fstype = fields.get(separator + 1)?;
            point.starts_with('/').then(|| Mount {
                root: PathBuf::from(unescape(root)),
                point: PathBuf::from(unescape(point)),
                fstype: (*fstype).to_owned(),
            })
        })
        .collect()
}

/// Decodes the kernel's octal escapes of a mountinfo field (`\040` is a space).
fn unescape(field: &str) -> String {
    let bytes = field.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let octal = bytes
            .get(i + 1..i + 4)
            .filter(|digits| bytes[i] == b'\\' && digits.iter().all(|d| (b'0'..=b'7').contains(d)))
            .and_then(|digits| u8::from_str_radix(std::str::from_utf8(digits).ok()?, 8).ok());
        if let Some(byte) = octal {
            out.push(byte);
            i += 4;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// The mount `path` is on: the last one of the longest mount point holding it.
fn mount_of<'a>(mounts: &'a [Mount], path: &Path) -> Option<&'a Mount> {
    mounts
        .iter()
        .filter(|mount| path.starts_with(&mount.point))
        .max_by_key(|mount| mount.point.components().count())
        .and_then(|longest| {
            mounts
                .iter()
                .rev()
                .find(|mount| mount.point == longest.point)
        })
}

/// Whether `target` (absolute, its directory resolved, its last component not followed) belongs
/// to the task: `worktree` is the task's resolved worktree, `mounts` the run's mount table, and
/// `tracked(worktree, relative)` says whether git tracks anything at or under `relative`.
/// `Err` holds why it does not.
pub fn decide(
    target: &Path,
    worktree: Option<&Path>,
    mounts: &[Mount],
    tracked: &dyn Fn(&Path, &Path) -> Result<bool, String>,
) -> Result<Owner, String> {
    // Without the mount table no mount below a directory could be seen.
    if mounts.is_empty() {
        return Err("the run's mount table cannot be read".to_owned());
    }
    // Removing a directory holding a mount would empty another filesystem (RMV-004).
    if mounts.iter().any(|mount| mount.point.starts_with(target)) {
        return Err("a mount point or a directory holding one".to_owned());
    }
    if let Some(worktree) = worktree
        && let Ok(relative) = target.strip_prefix(worktree)
    {
        if relative.as_os_str().is_empty() {
            return Err("the task's worktree itself".to_owned());
        }
        if relative.starts_with(".git") {
            return Err("the worktree's git link".to_owned());
        }
        return if tracked(worktree, relative)? {
            Err("tracked by git, so not created by this task: use `git rm`".to_owned())
        } else {
            Ok(Owner::Worktree)
        };
    }
    // A host whose root is a tmpfs is not contained: only `bwrap` mounts its `newroot`.
    let contained = mount_of(mounts, Path::new("/")).is_some_and(|root| {
        root.point == Path::new("/") && root.fstype == "tmpfs" && root.root == Path::new(BWRAP_ROOT)
    });
    let private = mount_of(mounts, target).is_some_and(|mount| {
        mount.fstype == "tmpfs"
            && RUN_PRIVATE_MOUNTS
                .iter()
                .any(|point| mount.point == Path::new(point))
    });
    if contained && private {
        if ROOT_LINKS.iter().any(|link| target == Path::new(link)) {
            return Err("a link the run's containment made".to_owned());
        }
        return Ok(Owner::RunPrivate);
    }
    Err("not this task's: other tasks or people share it".to_owned())
}

/// What a refusal adds once: where Yulius removes, and what to do with the rest.
const REFUSED_NOTE: &str = "Yulius removes only what this task created: untracked or ignored \
    paths of its worktree, and paths of this session's own `/`, `/tmp`, `/run` and `/dev/shm`, \
    which nobody else sees and which go when the session ends. Leave the rest and name it in your \
    summary; never ask a person to remove it.";

/// Runs `yulius remove <paths>` in the agent's run; returns the exit code.
pub fn run(out: &mut dyn Write, paths: &[PathBuf], env: &dyn Fn(&str) -> Option<String>) -> i32 {
    let cwd = match std::env::current_dir() {
        Ok(cwd) => cwd,
        Err(err) => {
            let _ = writeln!(out, "yulius remove: no working directory: {err}");
            return 1;
        }
    };
    let worktree = task_worktree(env(yulius_harness::launch::TASK_WORKTREE_ENV));
    let mounts = std::fs::read_to_string("/proc/self/mountinfo")
        .map(|text| parse_mountinfo(&text))
        .unwrap_or_default();
    remove(out, paths, &cwd, worktree.as_deref(), &mounts, &git_tracked)
}

/// The task's worktree named by the environment, resolved: a directory of a sandbox's
/// `worktrees` directory, as Yulius makes them; `None` for anything else.
fn task_worktree(value: Option<String>) -> Option<PathBuf> {
    let worktree = std::fs::canonicalize(value.filter(|path| !path.is_empty())?).ok()?;
    let parent = worktree.parent()?.file_name()?;
    (parent == "worktrees" && worktree.is_dir()).then_some(worktree)
}

/// Removes each of `paths` (relative ones from `cwd`) that belongs to the task, one line per
/// path; 0 when every path is gone, 1 when any was refused or failed, 2 without paths.
pub fn remove(
    out: &mut dyn Write,
    paths: &[PathBuf],
    cwd: &Path,
    worktree: Option<&Path>,
    mounts: &[Mount],
    tracked: &dyn Fn(&Path, &Path) -> Result<bool, String>,
) -> i32 {
    if paths.is_empty() {
        let _ = writeln!(out, "usage: yulius remove <path>…");
        return 2;
    }
    let outcomes: Vec<(&PathBuf, Outcome)> = paths
        .iter()
        .map(|path| (path, remove_one(path, cwd, worktree, mounts, tracked)))
        .collect();
    report(out, &outcomes, worktree.is_some())
}

/// What became of a path.
#[derive(Debug)]
enum Outcome {
    /// It was the task's and is removed (its resolved path).
    Removed(PathBuf),
    /// Nothing was there.
    Gone,
    /// It is not the task's (why).
    Refused(String),
    /// It is the task's but could not be removed (the error).
    Failed(String),
}

/// One line per path, and once the note when any was not the task's; 0 when every path is gone.
fn report(out: &mut dyn Write, outcomes: &[(&PathBuf, Outcome)], worktree_known: bool) -> i32 {
    let mut refused = false;
    let mut failed = false;
    for (path, outcome) in outcomes {
        let line = match outcome {
            Outcome::Removed(target) => format!("removed {}", target.display()),
            Outcome::Gone => format!("already gone: {}", path.display()),
            Outcome::Refused(reason) => {
                refused = true;
                format!("not removed: {}: {reason}", path.display())
            }
            Outcome::Failed(err) => {
                failed = true;
                format!("could not remove {}: {err}", path.display())
            }
        };
        let _ = writeln!(out, "{line}");
    }
    if refused {
        if !worktree_known {
            let _ = writeln!(
                out,
                "This session names no task worktree (`{}`).",
                yulius_harness::launch::TASK_WORKTREE_ENV
            );
        }
        let _ = writeln!(out, "{REFUSED_NOTE}");
    }
    i32::from(refused || failed)
}

fn remove_one(
    path: &Path,
    cwd: &Path,
    worktree: Option<&Path>,
    mounts: &[Mount],
    tracked: &dyn Fn(&Path, &Path) -> Result<bool, String>,
) -> Outcome {
    use std::os::unix::ffi::OsStrExt as _;
    let mut trimmed = path.as_os_str().as_bytes();
    while let Some(shorter) = trimmed.strip_suffix(b"/") {
        trimmed = shorter;
    }
    let last = trimmed
        .rsplit(|byte| *byte == b'/')
        .next()
        .unwrap_or_default();
    if matches!(last, b"" | b"." | b"..") {
        return Outcome::Refused("name a file or a directory".to_owned());
    }
    let absolute = cwd.join(path);
    let (Some(parent), Some(name)) = (absolute.parent(), absolute.file_name()) else {
        return Outcome::Refused("name a file or a directory".to_owned());
    };
    // The directory is resolved (links followed), the last component never is.
    let parent = match std::fs::canonicalize(parent) {
        Ok(parent) => parent,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Outcome::Gone,
        Err(err) => return Outcome::Refused(format!("its directory cannot be resolved: {err}")),
    };
    let target = parent.join(name);
    let metadata = match std::fs::symlink_metadata(&target) {
        Ok(metadata) => metadata,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Outcome::Gone,
        Err(err) => return Outcome::Refused(format!("cannot be read: {err}")),
    };
    if let Err(reason) = decide(&target, worktree, mounts, tracked) {
        return Outcome::Refused(reason);
    }
    let removed = if metadata.is_dir() {
        std::fs::remove_dir_all(&target)
    } else {
        std::fs::remove_file(&target)
    };
    match removed {
        Ok(()) => Outcome::Removed(target),
        Err(err) => Outcome::Failed(err.to_string()),
    }
}

/// `git ls-files` of `relative` in `worktree`, with none of the caller's git environment.
fn git_command(worktree: &Path, relative: &Path) -> std::process::Command {
    let mut pathspec = std::ffi::OsString::from(":(literal)");
    pathspec.push(relative);
    let mut command = std::process::Command::new("git");
    command
        .arg("-C")
        .arg(worktree)
        .args(["ls-files", "-z", "--cached", "--"])
        .arg(pathspec)
        .stdin(std::process::Stdio::null());
    for name in GIT_ENV {
        command.env_remove(name);
    }
    command
}

/// Whether git tracks anything at or under `relative` in `worktree`.
pub fn git_tracked(worktree: &Path, relative: &Path) -> Result<bool, String> {
    let output = git_command(worktree, relative)
        .output()
        .map_err(|err| format!("git could not run: {err}"))?;
    if !output.status.success() {
        return Err(format!(
            "git could not tell whether it is tracked: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(!output.stdout.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    /// The mount table of a contained task run (taken from one, 2026-10-16).
    const CONTAINED: &str = "\
1066 1065 0:120 /newroot / rw,nosuid,nodev,relatime - tmpfs tmpfs rw,uid=0,gid=0
1067 1066 0:123 / /proc rw,nosuid,nodev,noexec,relatime - proc proc rw
1068 1066 0:124 / /dev rw,nosuid,nodev,relatime - tmpfs tmpfs rw,mode=755
1075 1068 0:126 / /dev/shm rw,nosuid,nodev,relatime - tmpfs tmpfs rw
1076 1066 0:127 / /tmp rw,nosuid,nodev,relatime - tmpfs tmpfs rw
1077 1066 0:128 / /run rw,nosuid,nodev,relatime - tmpfs tmpfs rw,mode=755
1078 1066 0:46 /usr /usr rw,relatime - overlay overlay rw,lowerdir=/l,upperdir=/u,workdir=/w
1083 1066 0:46 /home /home rw,relatime - overlay overlay rw,lowerdir=/l,upperdir=/u,workdir=/w
1086 1066 0:129 / /var/lib/yulius rw,nosuid,nodev,relatime - tmpfs tmpfs rw
1087 1086 8:1 /var/lib/yulius/sandboxes/s1 /var/lib/yulius/sandboxes/s1 rw,relatime - ext4 /dev/sda1 rw
1093 1066 8:1 /var/lib/yulius/runtime/gh /yulius/bin ro,relatime - ext4 /dev/sda1 rw
1094 1066 8:1 /a\\040b /mnt/a\\040b ro,relatime - ext4 /dev/sda1 rw
";

    /// An uncontained run: the host's root.
    const HOST: &str = "\
22 1 8:1 / / rw,relatime shared:1 - ext4 /dev/sda1 rw
25 22 0:30 / /tmp rw,nosuid,nodev shared:5 - tmpfs tmpfs rw
";

    const WORKTREE: &str = "/var/lib/yulius/sandboxes/s1/worktrees/t1";

    fn untracked(_: &Path, _: &Path) -> Result<bool, String> {
        Ok(false)
    }

    fn decide_in(table: &str, target: &str) -> Result<Owner, String> {
        decide(
            Path::new(target),
            Some(Path::new(WORKTREE)),
            &parse_mountinfo(table),
            &untracked,
        )
    }

    #[test]
    fn mountinfo_gives_each_mount_point_and_type_in_order() {
        let mounts = parse_mountinfo(CONTAINED);
        assert_eq!(mounts.len(), 12);
        assert_eq!(
            mounts[0],
            Mount {
                root: "/newroot".into(),
                point: "/".into(),
                fstype: "tmpfs".into()
            }
        );
        assert_eq!(mounts[6].point, PathBuf::from("/usr"));
        assert_eq!(mounts[6].fstype, "overlay");
        // Octal escapes of the kernel (`\040` is a space) are decoded.
        assert_eq!(mounts[11].point, PathBuf::from("/mnt/a b"));
        assert!(parse_mountinfo("garbage\n\n").is_empty());
    }

    /// AC-3: the run's own tmpfs is the task's.
    #[test]
    fn paths_on_the_runs_own_tmpfs_belong_to_the_task() {
        for target in [
            "/ci6.log",
            "/tmp/a/b",
            "/run/x",
            "/dev/shm/x",
            "/yulius/notes",
        ] {
            assert_eq!(
                decide_in(CONTAINED, target),
                Ok(Owner::RunPrivate),
                "{target}"
            );
        }
    }

    /// AC-3, RMV-004: mount points, what holds them and the root's links stay.
    #[test]
    fn mount_points_their_holders_and_the_root_links_are_refused() {
        for target in [
            "/",
            "/tmp",
            "/dev/shm",
            "/usr",
            "/var",
            "/var/lib/yulius",
            "/yulius",
            "/bin",
            "/lib64",
        ] {
            assert!(decide_in(CONTAINED, target).is_err(), "{target}");
        }
    }

    /// AC-3, RMV-005: the sandbox's shared files and other tasks' worktrees are not the task's.
    #[test]
    fn shared_files_and_other_worktrees_are_refused() {
        for target in [
            "/usr/x",
            "/home/x",
            "/dev/null",
            "/var/lib/yulius/sandboxes/s1/repo/x",
            "/var/lib/yulius/sandboxes/s1/home/x",
            "/var/lib/yulius/sandboxes/s1/worktrees/t2/x",
            "/var/lib/yulius/sandboxes/s1/worktrees/t10/x",
        ] {
            let decision = decide_in(CONTAINED, target);
            assert!(
                matches!(&decision, Err(reason) if reason.contains("not this task's")),
                "{target}: {decision:?}"
            );
        }
    }

    /// AC-4: without a contained root, the host's `/tmp` is shared, also on a host whose root is
    /// a tmpfs (only `bwrap`'s `/newroot` is a run's own); an unreadable mount table refuses all.
    #[test]
    fn an_uncontained_runs_tmp_is_refused() {
        assert!(decide_in(HOST, "/tmp/x").is_err());
        assert!(decide_in(HOST, "/x").is_err());
        let tmpfs_host = "\
22 1 0:20 / / rw,relatime shared:1 - tmpfs tmpfs rw
25 22 0:30 / /tmp rw,nosuid,nodev shared:5 - tmpfs tmpfs rw
";
        assert!(decide_in(tmpfs_host, "/tmp/x").is_err());
        assert!(decide_in(tmpfs_host, "/x").is_err());
        let worktree_path = format!("{WORKTREE}/target");
        let decision = decide(
            Path::new(&worktree_path),
            Some(Path::new(WORKTREE)),
            &[],
            &untracked,
        );
        assert!(
            matches!(&decision, Err(reason) if reason.contains("mount table")),
            "{decision:?}"
        );
    }

    /// RMV-002: only a directory of a sandbox's `worktrees` is taken as the task's worktree.
    #[test]
    fn the_worktree_named_by_the_environment_must_be_a_task_worktree() -> TestResult {
        let dir = tempfile::tempdir()?;
        let task = dir.path().join("worktrees/t1");
        let other = dir.path().join("repo");
        std::fs::create_dir_all(&task)?;
        std::fs::create_dir_all(&other)?;
        let value = |path: &Path| path.to_str().map(str::to_owned);
        assert_eq!(
            task_worktree(value(&task)),
            Some(std::fs::canonicalize(&task)?)
        );
        assert_eq!(task_worktree(value(&other)), None);
        assert_eq!(
            task_worktree(value(&dir.path().join("worktrees/gone"))),
            None
        );
        assert_eq!(task_worktree(Some(String::new())), None);
        assert_eq!(task_worktree(None), None);
        Ok(())
    }

    /// RMV-002: git asked about the worktree is the worktree's, whatever git environment the
    /// caller has (a hook's `GIT_INDEX_FILE` would make tracked files look untracked).
    #[test]
    fn git_is_asked_without_the_callers_git_environment() {
        let command = git_command(Path::new(WORKTREE), Path::new("src"));
        let cleared: Vec<_> = command
            .get_envs()
            .filter(|(_, value)| value.is_none())
            .map(|(name, _)| name.to_owned())
            .collect();
        for name in GIT_ENV {
            assert!(cleared.iter().any(|c| c == name), "{name} kept");
        }
    }

    /// RMV-006: a removal that fails says so, without the note for paths that are not the task's.
    #[test]
    fn a_failed_removal_is_reported_without_the_refusal_note() {
        let mut out = Vec::new();
        let path = PathBuf::from("/x");
        let code = report(
            &mut out,
            &[(&path, Outcome::Failed("permission denied".into()))],
            true,
        );
        let out = String::from_utf8_lossy(&out);
        assert_eq!(code, 1);
        assert_eq!(out, "could not remove /x: permission denied\n");
    }

    /// AC-2, RMV-002: in the worktree, untracked paths are the task's; the worktree, its `.git`
    /// and tracked paths are not.
    #[test]
    fn the_worktree_gives_its_untracked_paths_only() {
        let tracked = |worktree: &Path, relative: &Path| -> Result<bool, String> {
            assert_eq!(worktree, Path::new(WORKTREE));
            Ok(relative.starts_with("src"))
        };
        let mounts = parse_mountinfo(CONTAINED);
        let decide_wt = |target: &str| {
            decide(
                &Path::new(WORKTREE).join(target),
                Some(Path::new(WORKTREE)),
                &mounts,
                &tracked,
            )
        };
        assert_eq!(decide_wt("target"), Ok(Owner::Worktree));
        assert_eq!(decide_wt("notes/scratch.txt"), Ok(Owner::Worktree));
        let decision = decide_wt("src/main.rs");
        assert!(
            matches!(&decision, Err(reason) if reason.contains("git rm")),
            "{decision:?}"
        );
        assert!(decide_wt("src").is_err());
        assert!(decide_wt(".git").is_err());
        assert!(decide_wt(".git/config").is_err());
        assert!(
            decide(
                Path::new(WORKTREE),
                Some(Path::new(WORKTREE)),
                &mounts,
                &tracked
            )
            .is_err()
        );
        // A failing git refuses rather than guesses.
        let broken = |_: &Path, _: &Path| -> Result<bool, String> { Err("no git".into()) };
        let decision = decide(
            &Path::new(WORKTREE).join("target"),
            Some(Path::new(WORKTREE)),
            &mounts,
            &broken,
        );
        assert!(
            matches!(&decision, Err(reason) if reason.contains("no git")),
            "{decision:?}"
        );
        // Without a known worktree, a worktree path is just a shared path.
        assert!(
            decide(
                &Path::new(WORKTREE).join("target"),
                None,
                &mounts,
                &untracked
            )
            .is_err()
        );
    }

    fn git(dir: &Path, args: &[&str]) -> TestResult {
        let status = std::process::Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(["-c", "user.name=t", "-c", "user.email=t@example.com"])
            .args(args)
            .status()?;
        assert!(status.success(), "git {args:?}");
        Ok(())
    }

    /// A git worktree with a tracked `src/main.rs` and `README`, an ignored `target/`, an
    /// untracked `scratch.txt` and a link `link` to `README`.
    fn worktree() -> Result<tempfile::TempDir, Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let root = dir.path();
        git(root, &["init", "-q"])?;
        std::fs::create_dir_all(root.join("src"))?;
        std::fs::write(root.join("src/main.rs"), "fn main() {}")?;
        std::fs::write(root.join("README"), "read me")?;
        std::fs::write(root.join(".gitignore"), "target/\n")?;
        git(root, &["add", "-A"])?;
        git(root, &["commit", "-qm", "init"])?;
        std::fs::create_dir_all(root.join("target/debug"))?;
        std::fs::write(root.join("target/debug/out"), "built")?;
        std::fs::write(root.join("scratch.txt"), "notes")?;
        std::os::unix::fs::symlink(root.join("README"), root.join("link"))?;
        std::fs::write(root.join("scratch2"), "more notes")?;
        Ok(dir)
    }

    fn remove_in(root: &Path, paths: &[&str]) -> Result<(i32, String), Box<dyn std::error::Error>> {
        let root = std::fs::canonicalize(root)?;
        let mut out = Vec::new();
        let paths: Vec<PathBuf> = paths.iter().map(PathBuf::from).collect();
        let code = remove(
            &mut out,
            &paths,
            &root,
            Some(&root),
            &parse_mountinfo(HOST),
            &git_tracked,
        );
        Ok((code, String::from_utf8(out)?))
    }

    /// AC-2, AC-5: the task's paths go, links as links; the rest stays, each with its line.
    #[test]
    fn it_removes_the_tasks_paths_and_refuses_the_rest() -> TestResult {
        let dir = worktree()?;
        let root = dir.path();
        // A file outside the worktree, in a run that is not contained.
        let elsewhere = tempfile::tempdir()?;
        let outside = elsewhere.path().join("x");
        std::fs::write(&outside, "not the task's")?;
        let outside_text = outside.to_str().ok_or("utf-8")?;
        // Ways out of the worktree: a link to a directory outside, `..`, a link with a trailing
        // slash, and a link inside an ignored directory; none of them reaches `outside`.
        std::os::unix::fs::symlink(elsewhere.path(), root.join("escape"))?;
        std::os::unix::fs::symlink(elsewhere.path(), root.join("dirlink"))?;
        std::os::unix::fs::symlink(&outside, root.join("target/debug/outlink"))?;
        let climb = format!(
            "src/../../{}/x",
            elsewhere
                .path()
                .file_name()
                .and_then(|name| name.to_str())
                .ok_or("name")?
        );
        // Both temporary directories are in the same directory.
        assert_eq!(elsewhere.path().parent(), root.parent());
        let (code, out) = remove_in(
            root,
            &[
                "scratch.txt",
                "target",
                "link",
                "README",
                "src",
                ".git",
                ".",
                outside_text,
                "escape/x",
                &climb,
                "dirlink/",
                "scratch2//",
            ],
        )?;
        assert_eq!(std::fs::read_to_string(&outside)?, "not the task's");
        assert!(std::fs::symlink_metadata(root.join("dirlink")).is_err());
        assert!(!root.join("scratch2").exists());
        assert_eq!(code, 1, "{out}");
        assert!(!root.join("scratch.txt").exists());
        assert!(!root.join("target").exists());
        assert!(std::fs::symlink_metadata(root.join("link")).is_err());
        assert_eq!(std::fs::read_to_string(root.join("README"))?, "read me");
        assert!(root.join("src/main.rs").exists());
        assert!(root.join(".git").exists());
        assert_eq!(
            out.lines().filter(|l| l.starts_with("removed ")).count(),
            5,
            "{out}"
        );
        assert_eq!(
            out.lines()
                .filter(|l| l.starts_with("not removed: "))
                .count(),
            7,
            "{out}"
        );
        assert!(out.contains("git rm"), "{out}");
        // The refusal says where Yulius removes and not to hand the removal to a person.
        assert!(out.contains("never ask a person"), "{out}");
        Ok(())
    }

    /// AC-5: only the task's or missing paths: exit 0; nothing to remove: exit 2.
    #[test]
    fn it_exits_zero_when_every_path_is_gone_and_two_without_paths() -> TestResult {
        let dir = worktree()?;
        let root = dir.path();
        let absolute = std::fs::canonicalize(root)?.join("scratch.txt");
        let absolute = absolute.to_str().ok_or("utf-8")?;
        let (code, out) = remove_in(root, &[absolute, "missing.txt", "gone/deeper"])?;
        assert_eq!(code, 0, "{out}");
        assert!(!root.join("scratch.txt").exists());
        assert_eq!(
            out.lines()
                .filter(|l| l.starts_with("already gone: "))
                .count(),
            2,
            "{out}"
        );
        assert_eq!(remove_in(root, &[])?.0, 2);
        Ok(())
    }
}
