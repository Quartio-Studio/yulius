//! The real [`Host`]: this machine's files and commands, run as root.

use std::io::Read as _;
use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};
use std::os::unix::process::CommandExt as _;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use super::{Cmd, Host, Out};

/// This machine. `quiet` keeps every command's output captured (for `--json`, where standard
/// output is the report).
pub struct RealHost {
    /// Never let a command's output through.
    pub quiet: bool,
}

/// Milliseconds since the epoch.
#[must_use]
pub fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
}

impl Host for RealHost {
    fn read(&self, path: &Path) -> Option<String> {
        let mut text = String::new();
        std::fs::File::open(path)
            .ok()?
            .take(4 << 20)
            .read_to_string(&mut text)
            .ok()?;
        Some(text)
    }

    fn write(&mut self, path: &Path, content: &str, mode: u32) -> Result<(), String> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        }
        // The caller names the file (`cannot write <file>: <why>`).
        crate::wiring::update_files::write_atomic(path, content.as_bytes(), mode)
            .map_err(|e| e.to_string())
    }

    fn is_file(&self, path: &Path) -> bool {
        path.is_file()
    }

    fn list(&self, dir: &Path) -> Vec<String> {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return Vec::new();
        };
        let mut names: Vec<String> = entries
            .flatten()
            .filter_map(|e| e.file_name().to_str().map(str::to_owned))
            .collect();
        names.sort();
        names
    }

    fn memory_stopgap(&mut self, data_dir: &Path, clear: bool) -> super::StopgapScan {
        #[cfg(target_os = "linux")]
        {
            use rustix::fs::IFlags;
            let mut scan = super::StopgapScan::default();
            let mut folders = std::collections::BTreeSet::new();
            let end = walk_memory(data_dir, super::STOPGAP_MAX_PATHS, &mut |path, fd| {
                let Ok(flags) = rustix::fs::ioctl_getflags(fd) else {
                    return;
                };
                if !flags.contains(IFlags::IMMUTABLE) {
                    return;
                }
                scan.flagged += 1;
                if let Some(folder) = memory_folder_of(path) {
                    folders.insert(folder);
                }
                if !clear {
                    return;
                }
                // On the descriptor the flag was read from: what it names cannot be swapped.
                match rustix::fs::ioctl_setflags(fd, flags - IFlags::IMMUTABLE) {
                    Ok(()) => scan.cleared += 1,
                    Err(err) => {
                        scan.failed += 1;
                        scan.first_failure
                            .get_or_insert_with(|| format!("{}: {err}", path.display()));
                    }
                }
            });
            scan.folders = folders.len();
            scan.cut = end.cut;
            scan.unread = end.unread_count;
            scan.first_unread = end.unread.into_iter().next();
            scan
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = (data_dir, clear);
            super::StopgapScan::default()
        }
    }

    fn stat(&self, path: &Path) -> Option<(u32, u32)> {
        std::fs::symlink_metadata(path)
            .ok()
            .map(|m| (m.uid(), m.mode()))
    }

    fn gid(&self, path: &Path) -> Option<u32> {
        std::fs::symlink_metadata(path).ok().map(|m| m.gid())
    }

    fn canonical(&self, path: &Path) -> Option<std::path::PathBuf> {
        std::fs::canonicalize(path).ok()
    }

    /// Through one descriptor opened without following a link, so a link swapped in after a
    /// check never makes root change its target (review of SPEC-203).
    fn own_by_root(&mut self, path: &Path) -> Result<(), String> {
        use std::os::unix::fs::OpenOptionsExt as _;
        let file = std::fs::OpenOptions::new()
            .read(true)
            .custom_flags((nix::fcntl::OFlag::O_NOFOLLOW | nix::fcntl::OFlag::O_NONBLOCK).bits())
            .open(path)
            .map_err(|e| format!("{}: {e}", path.display()))?;
        let meta = file
            .metadata()
            .map_err(|e| format!("{}: {e}", path.display()))?;
        if !meta.is_file() {
            return Err(format!("{} is not a regular file", path.display()));
        }
        std::os::unix::fs::fchown(&file, Some(0), Some(0))
            .map_err(|e| format!("chown {}: {e}", path.display()))?;
        let mode = meta.mode() & 0o7777 & !0o022;
        file.set_permissions(std::fs::Permissions::from_mode(mode))
            .map_err(|e| format!("chmod {}: {e}", path.display()))
    }

    fn fingerprint(&self) -> String {
        yulius_sandbox::base::fingerprint(Path::new("/"), yulius_app::identity::SUDO_SHIM)
    }

    fn run(&mut self, cmd: &Cmd) -> Result<Out, String> {
        let (program, args) = cmd.argv.split_first().ok_or("an empty command")?;
        let show = cmd.show && !self.quiet;
        let mut command = Command::new(program);
        // Its own process group, so a timeout ends what it started too (apt's dpkg).
        command.args(args).stdin(Stdio::null()).process_group(0);
        for (key, value) in &cmd.env {
            command.env(key, value);
        }
        if show {
            command.stdout(Stdio::inherit()).stderr(Stdio::inherit());
        } else {
            command.stdout(Stdio::piped()).stderr(Stdio::piped());
        }
        let mut child = command
            .spawn()
            .map_err(|e| format!("cannot run {program}: {e}"))?;
        let reader = |pipe: Option<Box<dyn std::io::Read + Send>>| {
            std::thread::spawn(move || {
                let mut text = String::new();
                if let Some(mut pipe) = pipe {
                    let _ = pipe.by_ref().take(1 << 20).read_to_string(&mut text);
                }
                text
            })
        };
        let out = reader(
            child
                .stdout
                .take()
                .map(|p| Box::new(p) as Box<dyn std::io::Read + Send>),
        );
        let err = reader(
            child
                .stderr
                .take()
                .map(|p| Box::new(p) as Box<dyn std::io::Read + Send>),
        );
        let deadline = Instant::now() + cmd.timeout;
        let status = loop {
            match child.try_wait() {
                Ok(Some(status)) => break Some(status),
                Ok(None) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(50));
                }
                _ => {
                    if let Ok(pid) = i32::try_from(child.id()) {
                        let _ = nix::sys::signal::killpg(
                            nix::unistd::Pid::from_raw(pid),
                            nix::sys::signal::Signal::SIGKILL,
                        );
                    }
                    let _ = child.kill();
                    let _ = child.wait();
                    break None;
                }
            }
        };
        let stdout = out.join().unwrap_or_default();
        let stderr = err.join().unwrap_or_default();
        Ok(match status {
            Some(status) => Out {
                code: status.code().unwrap_or(-1),
                stdout,
                stderr,
            },
            None => Out {
                code: -1,
                stdout,
                stderr: format!(
                    "{program} did not finish within {} s",
                    cmd.timeout.as_secs()
                ),
            },
        })
    }
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;

    /// Review of SPEC-652 part 2: the stopgap walk opens every folder by descriptor without
    /// following a link, from the data directory down to the memory folders and everything in
    /// them: a link an agent put at `memory`, `projects` or `home` is never entered, and a walk
    /// past its bound says it was cut.
    #[cfg(target_os = "linux")]
    #[test]
    fn the_stopgap_walk_never_follows_a_link() -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        let data = root.path().join("data");
        let outside = root.path().join("outside");
        std::fs::create_dir_all(outside.join("deep"))?;
        std::fs::write(outside.join("secret.conf"), "x")?;
        std::fs::write(outside.join("deep/more.conf"), "x")?;
        let memory = data.join("sandboxes/s1/home/.claude/projects/-k/memory");
        std::fs::create_dir_all(memory.join("sub"))?;
        std::fs::write(memory.join("MEMORY.md"), "- a note")?;
        std::fs::write(memory.join("sub/topic.md"), "- a topic")?;
        std::os::unix::fs::symlink(outside.join("secret.conf"), memory.join("link.md"))?;
        let planted = data.join("sandboxes/s1/home/.claude/projects/x");
        std::fs::create_dir_all(&planted)?;
        std::os::unix::fs::symlink(&outside, planted.join("memory"))?;
        std::fs::create_dir_all(data.join("sandboxes/s2/home/.claude"))?;
        std::os::unix::fs::symlink(&outside, data.join("sandboxes/s2/home/.claude/projects"))?;
        std::fs::create_dir_all(data.join("users/u1"))?;
        std::os::unix::fs::symlink(&outside, data.join("users/u1/home"))?;
        // A regular file beside `memory` is no memory folder's (#1767).
        std::fs::write(
            data.join("sandboxes/s1/home/.claude/projects/-k/session.jsonl"),
            "{}",
        )?;
        let mut seen = Vec::new();
        let end = walk_memory(&data, 100, &mut |path, _| seen.push(path.to_path_buf()));
        seen.sort();
        assert!(!end.cut);
        assert!(end.unread.is_empty(), "{:?}", end.unread);
        assert_eq!(
            seen,
            [
                memory.clone(),
                memory.join("MEMORY.md"),
                memory.join("sub"),
                memory.join("sub/topic.md"),
            ]
        );
        let mut few = 0;
        assert!(
            walk_memory(&data, 2, &mut |_, _| few += 1).cut,
            "cut at its bound"
        );
        assert!(few <= 2);
        Ok(())
    }

    /// #1767: every entry the walk examines counts toward its bound, links and FIFOs too, so a
    /// folder of many links cuts it; a name that is not UTF-8 is reported, not passed over.
    #[cfg(target_os = "linux")]
    #[test]
    fn every_examined_entry_counts_and_an_unreadable_name_is_reported()
    -> Result<(), Box<dyn std::error::Error>> {
        use std::os::unix::ffi::OsStrExt as _;
        let root = tempfile::tempdir()?;
        let data = root.path().join("data");
        let memory = data.join("sandboxes/s1/home/.claude/projects/-k/memory");
        std::fs::create_dir_all(&memory)?;
        for n in 0..50 {
            std::os::unix::fs::symlink("/nowhere", memory.join(format!("l{n}")))?;
        }
        let mut visited = 0;
        let end = walk_memory(&data, 20, &mut |_, _| visited += 1);
        assert!(end.cut, "50 links past a bound of 20");
        let odd = memory.join(std::ffi::OsStr::from_bytes(b"bad-\xff.md"));
        std::fs::write(&odd, "x")?;
        let end = walk_memory(&data, 1_000, &mut |_, _| {});
        assert!(!end.cut);
        assert_eq!(end.unread.len(), 1, "{:?}", end.unread);
        assert!(end.unread[0].contains("not UTF-8"), "{:?}", end.unread);
        Ok(())
    }

    /// A failed write names no path: the item's own words name the file (`cannot write <file>:`),
    /// so the operator reads it once (#674).
    #[test]
    fn a_failed_write_does_not_repeat_the_path() -> Result<(), String> {
        // procfs takes no new file, even from root.
        let path = Path::new("/proc/yulius-host-setup-test.conf");
        let Err(why) = (RealHost { quiet: true }).write(path, "x\n", 0o644) else {
            return Err("a file was written into /proc".to_owned());
        };
        assert!(!why.contains("/proc"), "{why}");
        Ok(())
    }
}

/// The memory folder a path found by [`walk_memory`] is in (the path up to `…/memory`).
#[cfg(target_os = "linux")]
fn memory_folder_of(path: &Path) -> Option<std::path::PathBuf> {
    path.ancestors()
        .find(|p| p.file_name().is_some_and(|n| n == "memory"))
        .map(Path::to_path_buf)
}

/// Walks the harness memory folders below `data_dir`
/// (`{sandboxes,users}/*/home/.claude/projects/*/memory` and every folder and regular file in
/// them), opening each component from its folder's descriptor with `O_NOFOLLOW`, so a link an
/// agent put anywhere below the data directory is never entered (SPEC-652 MEM-009, review of part
/// 2). `visit` gets each folder and regular file with an open descriptor of it, `O_RDONLY` (a
/// FIFO or device is never opened for reading). Every entry it examines counts toward `bound`,
/// a link or FIFO it skips too (#1767); the walk stops there.
#[cfg(target_os = "linux")]
pub(crate) fn walk_memory(
    data_dir: &Path,
    bound: usize,
    visit: &mut dyn FnMut(&Path, &std::os::fd::OwnedFd),
) -> WalkEnd {
    use rustix::fs::{Mode, OFlags};
    let dir_flags = OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC;
    let open_dir = |dir: &std::os::fd::OwnedFd, name: &str| {
        rustix::fs::openat(dir, name, dir_flags, Mode::empty()).ok()
    };
    let mut end = WalkEnd::default();
    // The folder the descriptor names, read from the descriptor itself (no `/proc` needed),
    // whatever its path became meanwhile; a name that is not UTF-8 or an entry that cannot be
    // read is noted.
    let names = |dir: &std::os::fd::OwnedFd, path: &Path, end: &mut WalkEnd| -> Vec<String> {
        let mut found = Vec::new();
        let listing = match rustix::fs::Dir::read_from(dir) {
            Ok(listing) => listing,
            Err(err) => {
                end.note(format!("{}: {err}", path.display()));
                return found;
            }
        };
        for entry in listing {
            match entry {
                Ok(entry) => {
                    let name = entry.file_name().to_bytes();
                    if name == b"." || name == b".." {
                        continue;
                    }
                    match std::str::from_utf8(name) {
                        Ok(name) => found.push(name.to_owned()),
                        Err(_) => end.note(format!(
                            "{}: a name that is not UTF-8 ({})",
                            path.display(),
                            String::from_utf8_lossy(name)
                        )),
                    }
                }
                Err(err) => {
                    end.note(format!("{}: {err}", path.display()));
                    break;
                }
            }
        }
        found
    };
    let Ok(data) = rustix::fs::open(
        data_dir,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
        Mode::empty(),
    ) else {
        return end;
    };
    let mut opened = 0_usize;
    for group in ["sandboxes", "users"] {
        let Some(group_fd) = open_dir(&data, group) else {
            continue;
        };
        for owner in names(&group_fd, &data_dir.join(group), &mut end) {
            let mut dir = open_dir(&group_fd, &owner);
            for part in ["home", ".claude", "projects"] {
                dir = dir.and_then(|d| open_dir(&d, part));
            }
            let Some(projects) = dir else {
                continue;
            };
            let base = data_dir
                .join(group)
                .join(&owner)
                .join("home/.claude/projects");
            for key in names(&projects, &base, &mut end) {
                let Some(memory) = open_dir(&projects, &key).and_then(|d| open_dir(&d, "memory"))
                else {
                    continue;
                };
                let mut pending = vec![(base.join(&key).join("memory"), memory)];
                while let Some((path, fd)) = pending.pop() {
                    if opened >= bound {
                        end.cut = true;
                        return end;
                    }
                    opened += 1;
                    for name in names(&fd, &path, &mut end) {
                        if opened >= bound {
                            end.cut = true;
                            return end;
                        }
                        opened += 1;
                        if let Some(sub) = open_dir(&fd, &name) {
                            pending.push((path.join(&name), sub));
                            continue;
                        }
                        // Not a folder: a regular file only, never a link, FIFO or device.
                        let Ok(file) = rustix::fs::openat(
                            &fd,
                            name.as_str(),
                            OFlags::PATH | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                            Mode::empty(),
                        ) else {
                            continue;
                        };
                        let regular = rustix::fs::fstat(&file).is_ok_and(|st| {
                            rustix::fs::FileType::from_raw_mode(st.st_mode)
                                == rustix::fs::FileType::RegularFile
                        });
                        if !regular {
                            continue;
                        }
                        let Ok(readable) = rustix::fs::openat(
                            &fd,
                            name.as_str(),
                            OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
                            Mode::empty(),
                        ) else {
                            continue;
                        };
                        // The same file as was checked, not one swapped in between.
                        let same = match (rustix::fs::fstat(&file), rustix::fs::fstat(&readable)) {
                            (Ok(a), Ok(b)) => (a.st_dev, a.st_ino) == (b.st_dev, b.st_ino),
                            _ => false,
                        };
                        if !same {
                            continue;
                        }
                        visit(&path.join(&name), &readable);
                    }
                    visit(&path, &fd);
                }
            }
        }
    }
    end
}

/// How a [`walk_memory`] ended.
#[cfg(target_os = "linux")]
#[derive(Debug, Default)]
pub(crate) struct WalkEnd {
    /// It stopped at its bound: entries beyond it were not looked at.
    pub(crate) cut: bool,
    /// Entries it could not read (a name that is not UTF-8, a listing that failed): the first
    /// [`UNREAD_KEPT`] of them.
    pub(crate) unread: Vec<String>,
    /// How many there were in all.
    pub(crate) unread_count: usize,
}

/// The unread entries a walk keeps to name.
#[cfg(target_os = "linux")]
const UNREAD_KEPT: usize = 10;

#[cfg(target_os = "linux")]
impl WalkEnd {
    fn note(&mut self, what: String) {
        self.unread_count += 1;
        if self.unread.len() < UNREAD_KEPT {
            self.unread.push(what);
        }
    }
}
