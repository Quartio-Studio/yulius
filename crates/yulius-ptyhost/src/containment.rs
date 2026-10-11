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
    if let Some(join) = &c.join {
        return joined_argv(c, join, inner_argv, cwd);
    }
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

/// Capabilities a run keeps inside its sandbox (Docker's default set; SPEC-002 ADR-19 §2).
pub const RUN_CAPABILITIES: [&str; 13] = [
    "CAP_CHOWN",
    "CAP_DAC_OVERRIDE",
    "CAP_FOWNER",
    "CAP_FSETID",
    "CAP_KILL",
    "CAP_SETGID",
    "CAP_SETUID",
    "CAP_SETPCAP",
    "CAP_SETFCAP",
    "CAP_NET_BIND_SERVICE",
    "CAP_NET_RAW",
    "CAP_SYS_CHROOT",
    "CAP_AUDIT_WRITE",
];

/// System directories bound writable from the sandbox's system layer.
pub const SYSTEM_LAYER_DIRS: [&str; 9] = [
    "/usr", "/etc", "/var", "/opt", "/root", "/home", "/srv", "/mnt", "/media",
];

/// The merged-`/usr` links of the root.
const ROOT_LINKS: [(&str, &str); 4] = [
    ("usr/bin", "/bin"),
    ("usr/sbin", "/sbin"),
    ("usr/lib", "/lib"),
    ("usr/lib64", "/lib64"),
];

/// `bwrap` argv of a run that joined its sandbox's holder (SPEC-002 ADR-19 §2): the user,
/// network and mount namespaces are the holder's (entered before `bwrap` starts); the run gets
/// its own PID, IPC, UTS and cgroup namespaces, the reduced capability set, a fresh `/proc`,
/// `/dev`, `/tmp` and `/run`, the system layer's directories writable, then the sandbox
/// directory and the same extra mounts as before.
fn joined_argv(
    c: &BubblewrapSpec,
    join: &yulius_contracts::ptyhost::SandboxJoin,
    inner_argv: &[String],
    cwd: &str,
) -> Vec<String> {
    let mut argv: Vec<String> = [
        BWRAP_PROGRAM,
        "--die-with-parent",
        "--unshare-pid",
        "--unshare-ipc",
        "--unshare-uts",
        "--unshare-cgroup-try",
        "--hostname",
        &join.hostname,
        "--cap-drop",
        "ALL",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect();
    for cap in RUN_CAPABILITIES {
        argv.extend(["--cap-add".into(), cap.into()]);
    }
    argv.extend(
        [
            "--proc",
            "/proc",
            "--dev",
            "/dev",
            "--perms",
            "1777",
            "--tmpfs",
            "/dev/shm",
            "--perms",
            "1777",
            "--tmpfs",
            "/tmp",
            "--perms",
            "0755",
            "--tmpfs",
            "/run",
            "--perms",
            "1777",
            "--dir",
            "/run/lock",
        ]
        .map(str::to_owned),
    );
    let root = join.system_root.trim_end_matches('/');
    for dir in SYSTEM_LAYER_DIRS {
        argv.extend(["--bind-try".into(), format!("{root}{dir}"), dir.to_owned()]);
    }
    for (target, link) in ROOT_LINKS {
        argv.extend(["--symlink".into(), target.into(), link.into()]);
    }
    for dir in &join.private_dirs {
        argv.extend(["--tmpfs".into(), dir.clone()]);
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
            writable: vec!["/data/sandboxes/s1/cache".into()],
            readonly: vec!["/data/sandboxes/s1/repo/.git/hooks".into()],
            share_net: true,
            binds: Vec::new(),
            identity: None,
            join: None,
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
        let writable = position(&argv, &["--bind", "/data/sandboxes/s1/cache"]);
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
    fn joined_runs_take_the_system_layer_and_drop_capabilities() {
        let mut c = spec();
        c.binds = vec![yulius_contracts::ptyhost::BindMount {
            source: "/data/users/u1/identity/passwd".into(),
            dest: "/var/lib/extrausers/passwd".into(),
            writable: false,
        }];
        c.readonly.push("/usr/local/bin/yulius".into());
        c.join = Some(Box::new(yulius_contracts::ptyhost::SandboxJoin {
            holder_pid: 4242,
            user_ns: 1,
            net_ns: 2,
            mnt_ns: 3,
            cgroup: None,
            system_root: "/data/runtime/sandboxes/s1/root".into(),
            hostname: "fixture".into(),
            exec_shim: None,
            private_dirs: vec!["/data".into()],
        }));
        let argv = bwrap_argv(&c, &["bash".into()], "/data/sandboxes/s1/repo");
        for flag in ["--unshare-user", "--unshare-net", "--share-net", "--uid"] {
            assert!(!argv.contains(&flag.to_owned()), "{flag} in {argv:?}");
        }
        assert!(position(&argv, &["--hostname", "fixture"]).is_some());
        let drop_all = position(&argv, &["--cap-drop", "ALL"]);
        let chown = position(&argv, &["--cap-add", "CAP_CHOWN"]);
        assert!(drop_all.is_some() && drop_all < chown, "{argv:?}");
        assert!(!argv.contains(&"CAP_SYS_ADMIN".to_owned()));
        let usr = position(
            &argv,
            &["--bind-try", "/data/runtime/sandboxes/s1/root/usr", "/usr"],
        );
        let var = position(
            &argv,
            &["--bind-try", "/data/runtime/sandboxes/s1/root/var", "/var"],
        );
        let sandbox = position(
            &argv,
            &["--bind", "/data/sandboxes/s1", "/data/sandboxes/s1"],
        );
        let exe = position(&argv, &["--ro-bind", "/usr/local/bin/yulius"]);
        let identity = position(
            &argv,
            &[
                "--ro-bind",
                "/data/users/u1/identity/passwd",
                "/var/lib/extrausers/passwd",
            ],
        );
        let private = position(&argv, &["--tmpfs", "/data"]);
        assert!(usr.is_some() && usr < sandbox && sandbox < exe, "{argv:?}");
        assert!(var < private && private < sandbox, "{argv:?}");
        assert!(
            var < identity,
            "identity files override the system layer: {argv:?}"
        );
        assert!(position(&argv, &["--symlink", "usr/bin", "/bin"]).is_some());
        assert!(position(&argv, &["--perms", "1777", "--tmpfs", "/tmp"]).is_some());
        assert_eq!(
            &argv[argv.len() - 2..],
            &["--".to_owned(), "bash".to_owned()]
        );
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
