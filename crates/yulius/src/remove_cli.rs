//! `yulius remove`, run by a task's agent (SPEC-192, ADR-0155): removes paths this task created,
//! where Claude Code's own removal check would stop `rm` and wait for a person (a root-level
//! file, a path held in a variable). It runs in the agent's run, so it sees the run's own
//! filesystem, which neither the Yulius server nor a person can reach, and removes a path only
//! when its location proves it is the task's: an untracked or ignored path of the task's worktree,
//! or a path on the run's own tmpfs (`/`, `/tmp`, `/run`, `/dev/shm` of a contained run).

use std::path::{Path, PathBuf};

/// Where the run's own tmpfs is mounted in a contained run (SPEC-002 ADR-22: `bwrap` makes a
/// fresh root and fresh `/tmp`, `/run` and `/dev/shm` for every run).
const RUN_PRIVATE_MOUNTS: [&str; 4] = ["/", "/tmp", "/run", "/dev/shm"];

/// The root's links containment makes for the merged `/usr`: the run needs them.
const ROOT_LINKS: [&str; 4] = ["/bin", "/sbin", "/lib", "/lib64"];

/// One line of `/proc/self/mountinfo`: where a filesystem is mounted and its type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mount {
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
            let point = fields.get(4)?;
            let separator = fields.iter().position(|field| *field == "-")?;
            let fstype = fields.get(separator + 1)?;
            point.starts_with('/').then(|| Mount {
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
    let contained = mount_of(mounts, Path::new("/"))
        .is_some_and(|root| root.point == Path::new("/") && root.fstype == "tmpfs");
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

#[cfg(test)]
mod tests {
    use super::*;

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
        for target in ["/ci6.log", "/tmp/a/b", "/run/x", "/dev/shm/x", "/yulius/notes"] {
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
            let reason = decide_in(CONTAINED, target).expect_err(target);
            assert!(reason.contains("not this task's"), "{target}: {reason}");
        }
    }

    /// AC-4: without a contained root, the host's `/tmp` is shared.
    #[test]
    fn an_uncontained_runs_tmp_is_refused() {
        assert!(decide_in(HOST, "/tmp/x").is_err());
        assert!(decide_in(HOST, "/x").is_err());
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
        let reason = decide_wt("src/main.rs").expect_err("tracked");
        assert!(reason.contains("git rm"), "{reason}");
        assert!(decide_wt("src").is_err());
        assert!(decide_wt(".git").is_err());
        assert!(decide_wt(".git/config").is_err());
        assert!(decide(Path::new(WORKTREE), Some(Path::new(WORKTREE)), &mounts, &tracked).is_err());
        // A failing git refuses rather than guesses.
        let broken = |_: &Path, _: &Path| -> Result<bool, String> { Err("no git".into()) };
        let reason = decide(
            &Path::new(WORKTREE).join("target"),
            Some(Path::new(WORKTREE)),
            &mounts,
            &broken,
        )
        .expect_err("git failed");
        assert!(reason.contains("no git"), "{reason}");
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
}
