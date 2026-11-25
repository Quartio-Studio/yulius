//! The read-only memory guard of a contained run (SPEC-652 MEM-001, MEM-003, MEM-004), run by
//! `yulius sandbox-exec --guard` in the view `bwrap` starts in.
//!
//! Each guarded path is opened from its anchor (a folder Yulius made) with `openat2`, refusing
//! every symbolic link on the way, and `bwrap` binds the open descriptor (`--ro-bind-fd`,
//! `--bind-fd`), so a folder an agent swapped for a link never shows the link's target. A missing
//! path, a link or the wrong kind gets an empty read-only placeholder instead (a pin gets
//! nothing), so a session always starts. The binds reach `bwrap` through two pipes at
//! [`GUARD_PIN_ARGS_FD`] and [`GUARD_ARGS_FD`], which `bwrap_argv` names with `--args` where they
//! belong; `bwrap` closes every one of these descriptors before the run starts.

use std::io;
use std::path::{Component, Path};

#[cfg(target_os = "linux")]
use yulius_contracts::ptyhost::{GUARD_ARGS_FD, GUARD_PIN_ARGS_FD};
use yulius_contracts::ptyhost::{GUARD_MAX_PATHS, GuardKind, GuardPath};

/// What a guarded path turned out to be when it was opened.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Found {
    /// A folder (not a link).
    Dir,
    /// A regular file (not a link).
    File,
    /// Missing, a link, or anything else.
    Other,
}

/// How a guarded path is bound, decided from its kind and what was found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Bind {
    /// `--ro-bind-fd <fd> <dest>`.
    ReadOnly,
    /// `--bind-fd <fd> <dest>` (a pin).
    Pin,
    /// An empty read-only folder: `--tmpfs <dest> --remount-ro <dest>`.
    EmptyDir,
    /// An empty read-only file: `--ro-bind-data <empty fd> <dest>`.
    EmptyFile,
    /// Nothing (a pin of a path that is not a folder).
    Skip,
}

/// The bind of a path of `kind` that was found as `found` (MEM-003).
#[must_use]
pub fn decide(kind: GuardKind, found: Found) -> Bind {
    match (kind, found) {
        (GuardKind::Dir, Found::Dir) | (GuardKind::File, Found::File) => Bind::ReadOnly,
        (GuardKind::Dir, _) => Bind::EmptyDir,
        (GuardKind::File, _) => Bind::EmptyFile,
        (GuardKind::Pin, Found::Dir) => Bind::Pin,
        (GuardKind::Pin, _) => Bind::Skip,
        (GuardKind::Mask, _) => Bind::EmptyDir,
    }
}

/// `relative` is a plain relative path: at least one component, none of them `.`, `..` or a
/// root.
fn plain_relative(relative: &str) -> bool {
    let path = Path::new(relative);
    !relative.is_empty()
        && path
            .components()
            .all(|component| matches!(component, Component::Normal(_)))
}

/// Checks a guard's paths: at most [`GUARD_MAX_PATHS`], absolute anchors and destinations, plain
/// relative paths, no NUL (they travel NUL-separated to `bwrap`).
///
/// # Errors
/// `InvalidInput` naming the first path that is not.
pub fn check(paths: &[GuardPath]) -> io::Result<()> {
    if paths.len() > GUARD_MAX_PATHS {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("more than {GUARD_MAX_PATHS} guarded paths"),
        ));
    }
    for path in paths {
        let fine = Path::new(&path.anchor).is_absolute()
            && Path::new(&path.dest).is_absolute()
            && plain_relative(&path.relative)
            && ![&path.anchor, &path.relative, &path.dest]
                .iter()
                .any(|text| text.contains('\0'));
        if !fine {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("guarded path {:?} is not a plain path", path.dest),
            ));
        }
    }
    Ok(())
}

/// NUL-separated `bwrap` arguments, as `--args` reads them.
#[must_use]
pub fn nul_separated(args: &[String]) -> Vec<u8> {
    let mut bytes = Vec::new();
    for arg in args {
        bytes.extend_from_slice(arg.as_bytes());
        bytes.push(0);
    }
    bytes
}

/// Most names tried when moving an entry aside.
#[cfg(target_os = "linux")]
const ASIDE_TRIES: u32 = 100;

/// Moves aside every link or entry of the wrong kind at a guarded path or on the way to it
/// (MEM-003): bwrap cannot mount a placeholder on a link whose target the namespace lacks, nor a
/// folder on a file, and following a link would put the bind on its target. Each walk starts at
/// the path's anchor and opens one folder at a time without following a link; an entry is moved
/// to `<name>.yulius-moved-<ms>` in its own folder, never replacing anything. Returns the moved
/// paths (relative to their anchors).
///
/// # Errors
/// An anchor that cannot be opened, or a move that fails.
#[cfg(target_os = "linux")]
pub fn clear_ways(paths: &[GuardPath]) -> io::Result<Vec<String>> {
    use rustix::fs::{AtFlags, FileType, Mode, OFlags, RenameFlags, openat, renameat_with, statat};

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis());
    let mut moved = Vec::new();
    for path in paths {
        let mut dir = rustix::fs::open(
            path.anchor.as_str(),
            OFlags::PATH | OFlags::DIRECTORY | OFlags::CLOEXEC,
            Mode::empty(),
        )?;
        let parts: Vec<&std::ffi::OsStr> = Path::new(&path.relative).iter().collect();
        for (index, part) in parts.iter().enumerate() {
            let last = index + 1 == parts.len();
            let kind = match statat(&dir, *part, AtFlags::SYMLINK_NOFOLLOW) {
                Ok(stat) => FileType::from_raw_mode(stat.st_mode),
                Err(rustix::io::Errno::NOENT) => break,
                Err(err) => return Err(err.into()),
            };
            // A mask's folder, a pin's and every folder on the way must be folders.
            let fits = match (last, path.kind) {
                (true, GuardKind::File) => kind == FileType::RegularFile,
                _ => kind == FileType::Directory,
            };
            if !fits {
                let shown = Path::new(&path.relative)
                    .iter()
                    .take(index + 1)
                    .collect::<std::path::PathBuf>();
                let mut tries = 0;
                loop {
                    let suffix = if tries == 0 {
                        String::new()
                    } else {
                        format!("-{tries}")
                    };
                    let mut aside = part.to_os_string();
                    aside.push(format!(".yulius-moved-{now}{suffix}"));
                    match renameat_with(&dir, *part, &dir, &aside, RenameFlags::NOREPLACE) {
                        Ok(()) => break,
                        Err(rustix::io::Errno::EXIST) if tries < ASIDE_TRIES => tries += 1,
                        Err(err) => return Err(err.into()),
                    }
                }
                moved.push(shown.to_string_lossy().into_owned());
                break;
            }
            if last {
                break;
            }
            dir = openat(
                &dir,
                *part,
                OFlags::PATH | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                Mode::empty(),
            )?;
        }
    }
    Ok(moved)
}

/// The guard, opened: the descriptors `bwrap` binds and the two argument pipes, kept open until
/// this process execs `bwrap`.
#[cfg(target_os = "linux")]
pub struct Opened {
    /// The pins' `bwrap` arguments.
    pub pins: Vec<String>,
    /// The read-only binds' and placeholders' `bwrap` arguments.
    pub binds: Vec<String>,
    held: Vec<std::os::fd::OwnedFd>,
}

/// Whether opening a guarded path failed because it is missing or a link (or a wrong kind) is on
/// the way, which a placeholder answers (MEM-003). Any other error fails the start: an empty
/// placeholder there would hide notes that exist (review round 1).
#[cfg(target_os = "linux")]
#[must_use]
pub fn placeholder_for(errno: rustix::io::Errno) -> bool {
    use rustix::io::Errno;
    matches!(
        errno,
        Errno::NOENT | Errno::LOOP | Errno::NOTDIR | Errno::XDEV
    )
}

/// The `bwrap` arguments of `path`, found as `found` and opened as `fd`: the descriptors they
/// name (kept open until the exec) and whether they belong in the pins' pipe.
///
/// # Errors
/// A descriptor that cannot be made inheritable, or `/dev/null` that cannot be opened.
#[cfg(target_os = "linux")]
pub fn bind_args(
    path: &GuardPath,
    found: Found,
    fd: Option<std::os::fd::OwnedFd>,
) -> io::Result<(Vec<String>, Vec<std::os::fd::OwnedFd>, bool)> {
    use std::os::fd::AsRawFd as _;

    use rustix::fs::{Mode, OFlags};
    use rustix::io::{FdFlags, fcntl_setfd};

    let dest = path.dest.clone();
    Ok(match (decide(path.kind, found), fd) {
        (bind @ (Bind::ReadOnly | Bind::Pin), Some(fd)) => {
            fcntl_setfd(&fd, FdFlags::empty())?;
            let option = if bind == Bind::Pin {
                "--bind-fd"
            } else {
                "--ro-bind-fd"
            };
            let args = vec![option.to_owned(), fd.as_raw_fd().to_string(), dest];
            (args, vec![fd], bind == Bind::Pin)
        }
        (Bind::EmptyFile, _) => {
            let empty =
                rustix::fs::open("/dev/null", OFlags::RDONLY | OFlags::CLOEXEC, Mode::empty())?;
            fcntl_setfd(&empty, FdFlags::empty())?;
            let args = vec![
                "--ro-bind-data".to_owned(),
                empty.as_raw_fd().to_string(),
                dest,
            ];
            (args, vec![empty], false)
        }
        (Bind::EmptyDir | Bind::ReadOnly, _) => {
            let args = vec![
                "--tmpfs".to_owned(),
                dest.clone(),
                "--remount-ro".to_owned(),
                dest,
            ];
            (args, Vec::new(), false)
        }
        (Bind::Skip | Bind::Pin, _) => (Vec::new(), Vec::new(), true),
    })
}

/// Opens every guarded path and its placeholders and writes their binds into the argument pipes
/// at [`GUARD_PIN_ARGS_FD`] (when a path is a pin) and [`GUARD_ARGS_FD`].
///
/// # Errors
/// An invalid path list, an anchor that cannot be opened, a guarded path that cannot be opened
/// for another reason than [`placeholder_for`], or a descriptor that cannot be made.
#[cfg(target_os = "linux")]
pub fn open(paths: &[GuardPath]) -> io::Result<Opened> {
    use std::os::fd::AsRawFd as _;

    use rustix::fs::{Mode, OFlags, ResolveFlags, openat2};
    use rustix::io::{FdFlags, fcntl_setfd};

    check(paths)?;
    for moved in clear_ways(paths)? {
        eprintln!(
            "yulius: moved {moved} aside: a protected memory or instruction path cannot be a \
             link or of another kind (SPEC-652)"
        );
    }
    let mut opened = Opened {
        pins: Vec::new(),
        binds: Vec::new(),
        held: Vec::new(),
    };
    for path in paths {
        let (found, fd) = if path.kind == GuardKind::Mask {
            (Found::Other, None)
        } else {
            let anchor = rustix::fs::open(
                path.anchor.as_str(),
                OFlags::PATH | OFlags::DIRECTORY | OFlags::CLOEXEC,
                Mode::empty(),
            )
            .map_err(|e| {
                io::Error::new(
                    io::ErrorKind::NotFound,
                    format!("guard anchor {}: {e}", path.anchor),
                )
            })?;
            match openat2(
                &anchor,
                path.relative.as_str(),
                OFlags::PATH | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                Mode::empty(),
                ResolveFlags::NO_SYMLINKS | ResolveFlags::NO_MAGICLINKS | ResolveFlags::BENEATH,
            ) {
                Ok(fd) => {
                    let stat = rustix::fs::fstat(&fd)?;
                    let found = match rustix::fs::FileType::from_raw_mode(stat.st_mode) {
                        rustix::fs::FileType::Directory => Found::Dir,
                        rustix::fs::FileType::RegularFile => Found::File,
                        _ => Found::Other,
                    };
                    (found, Some(fd))
                }
                Err(errno) if placeholder_for(errno) => (Found::Other, None),
                Err(errno) => {
                    return Err(io::Error::other(format!(
                        "cannot open the guarded path {}: {errno}",
                        path.dest
                    )));
                }
            }
        };
        let (args, held, pin) = bind_args(path, found, fd)?;
        if pin {
            opened.pins.extend(args);
        } else {
            opened.binds.extend(args);
        }
        opened.held.extend(held);
    }
    let any_pin = paths.iter().any(|p| p.kind == GuardKind::Pin);
    let pipes = [
        (any_pin, GUARD_PIN_ARGS_FD, nul_separated(&opened.pins)),
        (
            !paths.is_empty(),
            GUARD_ARGS_FD,
            nul_separated(&opened.binds),
        ),
    ];
    for (wanted, at, bytes) in pipes {
        if wanted {
            let read = args_pipe(&bytes)?;
            let placed = rustix::io::fcntl_dupfd_cloexec(&read, at)?;
            if placed.as_raw_fd() != at {
                return Err(io::Error::other(format!(
                    "descriptor {at} for the guard's arguments is taken"
                )));
            }
            fcntl_setfd(&placed, FdFlags::empty())?;
            opened.held.push(placed);
        }
    }
    Ok(opened)
}

/// A pipe holding `bytes`, its write end closed: the read end, close-on-exec.
#[cfg(target_os = "linux")]
fn args_pipe(bytes: &[u8]) -> io::Result<std::os::fd::OwnedFd> {
    use std::io::Write as _;

    // A pipe holds at least 64 KiB on Linux: the arguments are written before anyone reads.
    const PIPE_BYTES: usize = 64 * 1024;
    if bytes.len() >= PIPE_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "the guard's arguments do not fit a pipe",
        ));
    }
    let (read, write) = rustix::pipe::pipe_with(rustix::pipe::PipeFlags::CLOEXEC)?;
    let mut writer = std::fs::File::from(write);
    writer.write_all(bytes)?;
    drop(writer);
    Ok(read)
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;

    fn path(anchor: &Path, relative: &str, kind: GuardKind) -> GuardPath {
        GuardPath {
            anchor: anchor.to_string_lossy().into_owned(),
            relative: relative.into(),
            dest: format!("/in/{relative}"),
            kind,
        }
    }

    #[test]
    fn each_kind_binds_what_it_finds_and_a_placeholder_otherwise() {
        assert_eq!(decide(GuardKind::Dir, Found::Dir), Bind::ReadOnly);
        assert_eq!(decide(GuardKind::File, Found::File), Bind::ReadOnly);
        assert_eq!(decide(GuardKind::Dir, Found::File), Bind::EmptyDir);
        assert_eq!(decide(GuardKind::Dir, Found::Other), Bind::EmptyDir);
        assert_eq!(decide(GuardKind::File, Found::Dir), Bind::EmptyFile);
        assert_eq!(decide(GuardKind::File, Found::Other), Bind::EmptyFile);
        assert_eq!(decide(GuardKind::Pin, Found::Dir), Bind::Pin);
        assert_eq!(decide(GuardKind::Pin, Found::File), Bind::Skip);
        assert_eq!(decide(GuardKind::Pin, Found::Other), Bind::Skip);
        for found in [Found::Dir, Found::File, Found::Other] {
            assert_eq!(decide(GuardKind::Mask, found), Bind::EmptyDir);
        }
    }

    /// Review round 1: only a missing path, a link or a wrong kind on the way gets a
    /// placeholder; any other error opening a guarded path fails the start instead of hiding
    /// existing notes behind an empty folder.
    #[test]
    fn only_missing_paths_and_links_get_placeholders() {
        use rustix::io::Errno;
        for errno in [Errno::NOENT, Errno::LOOP, Errno::NOTDIR, Errno::XDEV] {
            assert!(placeholder_for(errno), "{errno:?}");
        }
        for errno in [
            Errno::ACCESS,
            Errno::MFILE,
            Errno::NOSYS,
            Errno::IO,
            Errno::PERM,
        ] {
            assert!(!placeholder_for(errno), "{errno:?}");
        }
    }

    /// MEM-010: a mask covers what is there with an empty read-only folder, needs no descriptor,
    /// and a link at it is moved aside first.
    #[test]
    fn a_mask_hides_the_folder_whatever_it_holds() -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        let anchor = root.path().join("anchor");
        std::fs::create_dir_all(anchor.join("home/.claude/projects"))?;
        std::os::unix::fs::symlink(root.path(), anchor.join("codex"))?;
        let paths = [
            path(&anchor, "home/.claude", GuardKind::Mask),
            path(&anchor, "codex", GuardKind::Mask),
        ];
        assert_eq!(clear_ways(&paths)?, ["codex"]);
        let mut binds = Vec::new();
        for guarded in &paths {
            binds.extend(bind_args(guarded, Found::Other, None)?.0);
        }
        assert_eq!(
            binds,
            [
                "--tmpfs",
                "/in/home/.claude",
                "--remount-ro",
                "/in/home/.claude",
                "--tmpfs",
                "/in/codex",
                "--remount-ro",
                "/in/codex"
            ]
        );
        Ok(())
    }

    #[test]
    fn only_plain_paths_pass() {
        let anchor = Path::new("/a");
        assert!(check(&[path(anchor, "x/y", GuardKind::Dir)]).is_ok());
        for bad in ["", "../x", "x/../y", "/x", "./x", "x\0y"] {
            assert!(
                check(&[path(anchor, bad, GuardKind::Dir)]).is_err(),
                "{bad:?}"
            );
        }
        let relative = GuardPath {
            anchor: "a".into(),
            ..path(anchor, "x", GuardKind::Dir)
        };
        assert!(check(&[relative]).is_err());
        let many = vec![path(anchor, "x", GuardKind::Dir); GUARD_MAX_PATHS + 1];
        assert!(check(&many).is_err());
    }

    /// MEM-003, MEM-004: a real folder and file are bound by descriptor; a link to a file outside
    /// (and a link on the way) and a missing path get placeholders; a pin of a link is skipped.
    #[test]
    fn links_and_missing_paths_get_placeholders() -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        let anchor = root.path().join("anchor");
        let outside = root.path().join("secret");
        std::fs::create_dir_all(anchor.join("cfg/memory"))?;
        std::fs::write(anchor.join("cfg/CLAUDE.md"), "note")?;
        std::fs::write(&outside, "secret")?;
        std::os::unix::fs::symlink(&outside, anchor.join("cfg/AGENTS.md"))?;
        std::os::unix::fs::symlink(anchor.join("cfg"), anchor.join("linked"))?;
        let paths = [
            path(&anchor, "cfg", GuardKind::Pin),
            path(&anchor, "linked", GuardKind::Pin),
            path(&anchor, "cfg/memory", GuardKind::Dir),
            path(&anchor, "cfg/CLAUDE.md", GuardKind::File),
            path(&anchor, "cfg/AGENTS.md", GuardKind::File),
            path(&anchor, "linked/memory", GuardKind::Dir),
            path(&anchor, "cfg/rules", GuardKind::Dir),
        ];
        let opened = open(&paths)?;
        assert_eq!(opened.pins.len(), 3, "{:?}", opened.pins);
        assert_eq!(opened.pins[0], "--bind-fd");
        assert_eq!(opened.pins[2], "/in/cfg");
        // Every descriptor named points at what it should: the folder, the file, `/dev/null`.
        let mut named = Vec::new();
        let mut args = opened.binds.iter();
        while let Some(option) = args.next() {
            match option.as_str() {
                "--ro-bind-fd" | "--ro-bind-data" => {
                    let fd = args.next().ok_or("fd")?;
                    let dest = args.next().ok_or("dest")?;
                    let target = std::fs::read_link(format!("/proc/self/fd/{fd}"))?;
                    named.push((option.clone(), dest.clone(), target));
                }
                "--tmpfs" => {
                    let dest = args.next().ok_or("dest")?;
                    assert_eq!(args.next().map(String::as_str), Some("--remount-ro"));
                    assert_eq!(args.next(), Some(dest));
                    named.push((option.clone(), dest.clone(), Default::default()));
                }
                other => return Err(format!("unexpected {other}").into()),
            }
        }
        let expect = |dest: &str| named.iter().find(|(_, d, _)| d == dest).cloned();
        let memory = expect("/in/cfg/memory").ok_or("memory")?;
        assert_eq!(memory.0, "--ro-bind-fd");
        assert_eq!(memory.2, anchor.join("cfg/memory"));
        let note = expect("/in/cfg/CLAUDE.md").ok_or("note")?;
        assert_eq!(note.0, "--ro-bind-fd");
        assert_eq!(note.2, anchor.join("cfg/CLAUDE.md"));
        let link = expect("/in/cfg/AGENTS.md").ok_or("link")?;
        assert_eq!(link.0, "--ro-bind-data");
        assert_eq!(link.2, Path::new("/dev/null"));
        assert_eq!(expect("/in/linked/memory").ok_or("linked")?.0, "--tmpfs");
        assert_eq!(expect("/in/cfg/rules").ok_or("rules")?.0, "--tmpfs");
        assert!(named.iter().all(|(_, _, target)| target != &outside));
        Ok(())
    }

    /// MEM-003: a link or an entry of the wrong kind at a guarded path, or on the way to it, is
    /// moved aside without following it, so the bind lands on that path and the session starts;
    /// the link's target is untouched.
    #[test]
    fn links_and_wrong_kinds_are_moved_aside() -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        let anchor = root.path().join("anchor");
        let outside = root.path().join("outside");
        std::fs::create_dir_all(anchor.join("cfg"))?;
        std::fs::create_dir_all(&outside)?;
        std::fs::write(outside.join("note"), "kept")?;
        std::os::unix::fs::symlink(outside.join("note"), anchor.join("cfg/CLAUDE.md"))?;
        std::fs::write(anchor.join("cfg/rules"), "a file, not a folder")?;
        std::os::unix::fs::symlink(&outside, anchor.join("linked"))?;
        std::fs::create_dir_all(anchor.join("cfg/memory"))?;
        let paths = [
            path(&anchor, "cfg/CLAUDE.md", GuardKind::File),
            path(&anchor, "cfg/rules", GuardKind::Dir),
            path(&anchor, "linked/memory", GuardKind::Dir),
            path(&anchor, "cfg/memory", GuardKind::Dir),
        ];
        let moved = clear_ways(&paths)?;
        assert_eq!(moved.len(), 3, "{moved:?}");
        let names: Vec<String> = std::fs::read_dir(anchor.join("cfg"))?
            .filter_map(Result::ok)
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        assert!(!names.contains(&"CLAUDE.md".to_owned()), "{names:?}");
        assert!(!names.contains(&"rules".to_owned()), "{names:?}");
        assert!(
            names
                .iter()
                .any(|n| n.starts_with("CLAUDE.md.yulius-moved-"))
        );
        assert!(names.iter().any(|n| n.starts_with("rules.yulius-moved-")));
        assert!(names.contains(&"memory".to_owned()), "a real folder stays");
        assert!(std::fs::symlink_metadata(anchor.join("linked")).is_err());
        assert_eq!(std::fs::read_to_string(outside.join("note"))?, "kept");
        // Nothing left to move the second time.
        assert!(clear_ways(&paths)?.is_empty());
        Ok(())
    }

    #[test]
    fn arguments_travel_nul_separated() {
        assert_eq!(
            nul_separated(&["--bind-fd".into(), "5".into(), "/x y".into()]),
            b"--bind-fd\x005\x00/x y\x00".to_vec()
        );
    }

    #[test]
    fn a_missing_anchor_fails_the_start() {
        let paths = [path(
            Path::new("/nonexistent/yulius-anchor"),
            "x",
            GuardKind::Dir,
        )];
        assert!(open(&paths).is_err());
    }
}
