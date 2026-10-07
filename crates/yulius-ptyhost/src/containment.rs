//! Bubblewrap argument construction (SPEC-002 ADR-03). Pure: no filesystem access.

use std::path::Path;

use yulius_contracts::ptyhost::BubblewrapSpec;

/// Program name of bubblewrap, resolved through the child's `PATH`.
pub const BWRAP_PROGRAM: &str = "bwrap";

/// Host system directories bound read-only at the same path (skipped by bwrap when absent).
pub const SYSTEM_READONLY_DIRS: [&str; 6] = ["/usr", "/bin", "/lib", "/lib64", "/etc", "/opt"];

/// The host resolver configuration.
const RESOLV_CONF: &str = "/etc/resolv.conf";

/// The directory holding the real resolver file when `resolv_conf` is a symbolic link out of the
/// bound system directories (Ubuntu: `/run/systemd/resolve/stub-resolv.conf`). Without it name
/// resolution fails inside the sandbox even though the network is shared.
fn resolver_dir(resolv_conf: &Path) -> Option<String> {
    let target = std::fs::canonicalize(resolv_conf).ok()?;
    let dir = target.parent()?;
    let inside = SYSTEM_READONLY_DIRS
        .iter()
        .any(|system| dir.starts_with(system));
    (!inside).then(|| dir.to_string_lossy().into_owned())
}

/// Builds the full `bwrap` argv that runs `inner_argv` with working directory `cwd` inside the
/// sandbox described by `c`.
///
/// Layout: `--die-with-parent --unshare-pid --unshare-ipc --unshare-uts`, a fresh `/proc`,
/// `/dev` and private `/tmp`; [`SYSTEM_READONLY_DIRS`] via `--ro-bind-try`; the sandbox root,
/// `home` (when outside the root) and every writable path via `--bind`; then every read-only path
/// via `--ro-bind` (later mounts override earlier ones); `--setenv HOME`, `--chdir cwd`; the
/// network is kept with `--share-net` when requested and unshared otherwise; then `--` and the
/// inner argv. Arguments are passed as separate argv elements, never through a shell.
#[must_use]
pub fn bwrap_argv(c: &BubblewrapSpec, inner_argv: &[String], cwd: &str) -> Vec<String> {
    let mut argv: Vec<String> = [
        BWRAP_PROGRAM,
        "--die-with-parent",
        "--unshare-pid",
        "--unshare-ipc",
        "--unshare-uts",
        "--proc",
        "/proc",
        "--dev",
        "/dev",
        "--tmpfs",
        "/tmp",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect();
    if let Some(identity) = &c.identity {
        argv.extend([
            "--unshare-user".into(),
            "--uid".into(),
            identity.uid.to_string(),
            "--gid".into(),
            identity.gid.to_string(),
            "--hostname".into(),
            identity.hostname.clone(),
        ]);
    }
    for dir in SYSTEM_READONLY_DIRS {
        push_bind(&mut argv, "--ro-bind-try", dir);
    }
    if c.share_net
        && let Some(dir) = resolver_dir(Path::new(RESOLV_CONF))
    {
        push_bind(&mut argv, "--ro-bind-try", &dir);
    }
    push_bind(&mut argv, "--bind", &c.sandbox_root);
    let home_mounted = c.binds.iter().any(|b| b.dest == c.home);
    if !home_mounted && !Path::new(&c.home).starts_with(&c.sandbox_root) {
        push_bind(&mut argv, "--bind", &c.home);
    }
    for path in &c.writable {
        push_bind(&mut argv, "--bind", path);
    }
    for path in &c.readonly {
        push_bind(&mut argv, "--ro-bind", path);
    }
    for bind in &c.binds {
        let option = if bind.writable { "--bind" } else { "--ro-bind" };
        argv.extend([option.to_owned(), bind.source.clone(), bind.dest.clone()]);
    }
    argv.extend(["--setenv".into(), "HOME".into(), c.home.clone()]);
    argv.extend(["--chdir".into(), cwd.to_owned()]);
    argv.push(if c.share_net {
        "--share-net".into()
    } else {
        "--unshare-net".into()
    });
    argv.push("--".into());
    argv.extend(inner_argv.iter().cloned());
    argv
}

fn push_bind(argv: &mut Vec<String>, option: &str, path: &str) {
    argv.extend([option.to_owned(), path.to_owned(), path.to_owned()]);
}

#[cfg(test)]
mod tests {
    #[test]
    fn resolver_dir_follows_symlinks_out_of_the_system_dirs()
    -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let run = dir.path().join("run-resolve");
        std::fs::create_dir_all(&run)?;
        std::fs::write(run.join("stub-resolv.conf"), "nameserver 127.0.0.53\n")?;
        let link = dir.path().join("resolv.conf");
        std::os::unix::fs::symlink(run.join("stub-resolv.conf"), &link)?;
        let expected = run.canonicalize()?.to_string_lossy().into_owned();
        assert_eq!(super::resolver_dir(&link), Some(expected));
        assert_eq!(super::resolver_dir(&dir.path().join("missing")), None);
        Ok(())
    }

    use super::*;

    fn spec() -> BubblewrapSpec {
        BubblewrapSpec {
            sandbox_root: "/data/sandboxes/s1".into(),
            home: "/data/sandboxes/s1/home".into(),
            writable: vec!["/data/sandboxes/s1/codex/acct".into()],
            readonly: vec!["/data/sandboxes/s1/repo/.git/hooks".into()],
            share_net: true,
            binds: Vec::new(),
            identity: None,
        }
    }

    #[test]
    fn identity_maps_root_and_binds_the_user_home_elsewhere() {
        let mut c = spec();
        c.home = "/home/alice".into();
        c.binds = vec![
            yulius_contracts::ptyhost::BindMount {
                source: "/data/users/u1/home".into(),
                dest: "/home/alice".into(),
                writable: true,
            },
            yulius_contracts::ptyhost::BindMount {
                source: "/data/users/u1/identity/passwd".into(),
                dest: "/etc/passwd".into(),
                writable: false,
            },
        ];
        c.identity = Some(yulius_contracts::ptyhost::SandboxIdentity {
            uid: 0,
            gid: 0,
            hostname: "s1".into(),
        });
        let argv = bwrap_argv(&c, &["bash".into()], "/data/sandboxes/s1/repo");
        let ns = position(
            &argv,
            &[
                "--unshare-user",
                "--uid",
                "0",
                "--gid",
                "0",
                "--hostname",
                "s1",
            ],
        );
        assert!(ns.is_some(), "{argv:?}");
        assert!(
            position(&argv, &["--bind", "/home/alice", "/home/alice"]).is_none(),
            "{argv:?}"
        );
        let home = position(&argv, &["--bind", "/data/users/u1/home", "/home/alice"]);
        let passwd = position(
            &argv,
            &["--ro-bind", "/data/users/u1/identity/passwd", "/etc/passwd"],
        );
        let etc = position(&argv, &["--ro-bind-try", "/etc", "/etc"]);
        assert!(home.is_some() && passwd > etc, "{argv:?}");
        assert!(position(&argv, &["--setenv", "HOME", "/home/alice"]).is_some());
    }

    fn position(argv: &[String], window: &[&str]) -> Option<usize> {
        argv.windows(window.len())
            .position(|w| w.iter().zip(window).all(|(a, b)| a == b))
    }

    #[test]
    fn namespace_and_filesystem_flags_come_first() {
        let argv = bwrap_argv(
            &spec(),
            &["claude".into()],
            "/data/sandboxes/s1/worktrees/t1",
        );
        assert_eq!(
            &argv[..11],
            &[
                "bwrap",
                "--die-with-parent",
                "--unshare-pid",
                "--unshare-ipc",
                "--unshare-uts",
                "--proc",
                "/proc",
                "--dev",
                "/dev",
                "--tmpfs",
                "/tmp"
            ]
        );
        for dir in SYSTEM_READONLY_DIRS {
            assert!(
                position(&argv, &["--ro-bind-try", dir, dir]).is_some(),
                "{dir}"
            );
        }
    }

    #[test]
    fn writable_binds_precede_readonly_overrides_and_inner_argv_is_last() {
        let inner = vec![
            "/usr/bin/agent".to_owned(),
            "--prompt".to_owned(),
            "text with spaces; rm -rf /".to_owned(),
        ];
        let argv = bwrap_argv(&spec(), &inner, "/data/sandboxes/s1/worktrees/t1");
        let root = position(
            &argv,
            &["--bind", "/data/sandboxes/s1", "/data/sandboxes/s1"],
        );
        let writable = position(&argv, &["--bind", "/data/sandboxes/s1/codex/acct"]);
        let readonly = position(&argv, &["--ro-bind", "/data/sandboxes/s1/repo/.git/hooks"]);
        assert!(root < writable && writable < readonly && root.is_some());
        assert!(position(&argv, &["--setenv", "HOME", "/data/sandboxes/s1/home"]).is_some());
        assert!(position(&argv, &["--chdir", "/data/sandboxes/s1/worktrees/t1"]).is_some());
        assert!(argv.contains(&"--share-net".to_owned()));
        let separator = argv.iter().position(|a| a == "--");
        assert_eq!(separator, Some(argv.len() - inner.len() - 1));
        assert_eq!(&argv[argv.len() - 3..], inner.as_slice());
    }

    #[test]
    fn home_inside_root_is_not_bound_twice_but_outside_home_is() {
        let argv = bwrap_argv(&spec(), &["a".into()], "/");
        assert!(position(&argv, &["--bind", "/data/sandboxes/s1/home"]).is_none());
        let mut outside = spec();
        outside.home = "/data/homes/s1".into();
        let argv = bwrap_argv(&outside, &["a".into()], "/");
        assert!(position(&argv, &["--bind", "/data/homes/s1", "/data/homes/s1"]).is_some());
    }

    #[test]
    fn network_is_unshared_unless_requested() {
        let mut no_net = spec();
        no_net.share_net = false;
        let argv = bwrap_argv(&no_net, &["a".into()], "/");
        assert!(argv.contains(&"--unshare-net".to_owned()));
        assert!(!argv.contains(&"--share-net".to_owned()));
    }
}
