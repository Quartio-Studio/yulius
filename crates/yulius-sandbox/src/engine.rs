//! The sandbox's Docker engine (ADR-0151, SPEC-187).
//!
//! The server describes the engine in the holder spec ([`EngineSpec`]): the `bwrap` argv of its
//! file view ([`bwrap_argv`]) and its cgroup. The holder serves the sandbox's Docker socket and
//! starts the engine on the first connection.

use yulius_contracts::sandbox::{
    ENGINE_CONTROL_DIR, ENGINE_CONTROL_IN_ENGINE, ENGINE_SOCKET, ENGINE_SOCKET_IN_RUNS,
    ENGINE_SYSFS_DIR, ROOT_LINKS, SYSTEM_LAYER_DIRS,
};

/// `PATH` inside the engine.
pub const ENGINE_PATH: &str = "/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin";
/// Where the engine keeps its images, containers and volumes.
pub const STORAGE_IN_ENGINE: &str = "/var/lib/docker";

/// What the engine's file view is made of (host paths).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EngineView {
    /// `bwrap`.
    pub bwrap: String,
    /// The `yulius` executable (bound read-only; it supervises `dockerd`).
    pub exe: String,
    /// Host name inside (the sandbox's slug).
    pub hostname: String,
    /// The holder's state directory (`runtime/sandboxes/<sandbox>`).
    pub state_dir: String,
    /// The merged system layer in the holder's mount namespace.
    pub system_root: String,
    /// Yulius's data directory, covered by an empty tmpfs as in runs.
    pub data_dir: String,
    /// The sandbox directory, bound at its host path as in runs.
    pub sandbox_dir: String,
    /// The engine's storage (`sysroots/<sandbox>/docker`).
    pub storage: String,
    /// The engine's cgroup (`sandboxes/<sandbox>/engine`), its cgroup namespace's root.
    pub cgroup: String,
}

/// `dockerd` and its arguments inside the engine: its control socket in the control directory
/// (the sandbox's socket at [`ENGINE_SOCKET_IN_RUNS`] is the holder's), cgroupfs (no systemd
/// inside), no IPv6 NAT (sandboxes have no IPv6 egress).
#[must_use]
pub fn dockerd_argv() -> Vec<String> {
    [
        "dockerd",
        "--host",
        &format!("unix://{ENGINE_CONTROL_IN_ENGINE}/docker.sock"),
        "--data-root",
        STORAGE_IN_ENGINE,
        "--exec-root",
        "/run/docker",
        "--pidfile",
        "/run/docker.pid",
        "--exec-opt",
        "native.cgroupdriver=cgroupfs",
        "--ip6tables=false",
        "--log-level",
        "warn",
    ]
    .map(str::to_owned)
    .to_vec()
}

/// The full `bwrap` argv that starts the engine (SPEC-187 DKR-003): new PID, IPC, UTS and cgroup
/// namespaces inside the sandbox's user, network and mount namespaces, every capability of the
/// sandbox's user namespace, a cleared environment, and a run's file view plus only the engine's
/// storage, its control directory, its sysfs and cgroup, the sandbox's Docker socket and the
/// `yulius` executable. Then `yulius sandbox-host engine -- dockerd …`.
#[must_use]
pub fn bwrap_argv(view: &EngineView) -> Vec<String> {
    let state = view.state_dir.trim_end_matches('/');
    let root = view.system_root.trim_end_matches('/');
    let mut argv: Vec<String> = [
        view.bwrap.as_str(),
        "--die-with-parent",
        "--unshare-pid",
        "--unshare-ipc",
        "--unshare-uts",
        "--unshare-cgroup",
        "--hostname",
        &view.hostname,
        "--cap-add",
        "ALL",
        "--clearenv",
        "--setenv",
        "PATH",
        ENGINE_PATH,
        "--setenv",
        "HOME",
        "/root",
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
    .map(str::to_owned)
    .to_vec();
    for dir in SYSTEM_LAYER_DIRS {
        argv.extend(["--bind-try".into(), format!("{root}{dir}"), dir.to_owned()]);
    }
    for (target, link) in ROOT_LINKS {
        argv.extend(["--symlink".into(), target.into(), link.into()]);
    }
    argv.extend(["--tmpfs".into(), view.data_dir.clone()]);
    let pairs: [(&str, String, &str); 7] = [
        ("--bind", view.sandbox_dir.clone(), &view.sandbox_dir),
        ("--ro-bind", format!("{state}/{ENGINE_SYSFS_DIR}"), "/sys"),
        ("--bind", view.cgroup.clone(), "/sys/fs/cgroup"),
        ("--bind", view.storage.clone(), STORAGE_IN_ENGINE),
        (
            "--bind",
            format!("{state}/{ENGINE_CONTROL_DIR}"),
            ENGINE_CONTROL_IN_ENGINE,
        ),
        (
            "--bind",
            format!("{state}/{ENGINE_SOCKET}"),
            ENGINE_SOCKET_IN_RUNS,
        ),
        ("--ro-bind", view.exe.clone(), &view.exe),
    ];
    for (option, source, dest) in pairs {
        argv.extend([option.to_owned(), source, dest.to_owned()]);
    }
    argv.extend(["--chdir".into(), "/".into(), "--".into()]);
    argv.extend([view.exe.clone(), "sandbox-host".into(), "engine".into()]);
    argv.push("--".into());
    argv.extend(dockerd_argv());
    argv
}

#[cfg(test)]
mod tests {
    use super::*;

    fn view() -> EngineView {
        EngineView {
            bwrap: "/usr/bin/bwrap".into(),
            exe: "/usr/local/bin/yulius".into(),
            hostname: "fixture".into(),
            state_dir: "/data/runtime/sandboxes/s1".into(),
            system_root: "/data/runtime/sandboxes/s1/root".into(),
            data_dir: "/data".into(),
            sandbox_dir: "/data/sandboxes/s1".into(),
            storage: "/data/sysroots/s1/docker".into(),
            cgroup: "/sys/fs/cgroup/system.slice/yulius.service/sandboxes/s1/engine".into(),
        }
    }

    fn position(argv: &[String], window: &[&str]) -> Option<usize> {
        argv.windows(window.len())
            .position(|w| w.iter().zip(window).all(|(a, b)| a == b))
    }

    /// Sources of every bind, as (option, source, destination).
    fn binds(argv: &[String]) -> Vec<(String, String, String)> {
        let options = [
            "--bind",
            "--ro-bind",
            "--bind-try",
            "--dev-bind",
            "--ro-bind-try",
        ];
        argv.windows(3)
            .filter(|w| options.contains(&w[0].as_str()))
            .map(|w| (w[0].clone(), w[1].clone(), w[2].clone()))
            .collect()
    }

    #[test]
    fn the_engine_gets_its_own_namespaces_capabilities_and_environment() {
        let argv = bwrap_argv(&view());
        assert_eq!(argv[0], "/usr/bin/bwrap");
        for flag in [
            "--die-with-parent",
            "--unshare-pid",
            "--unshare-ipc",
            "--unshare-uts",
            "--unshare-cgroup",
            "--clearenv",
        ] {
            assert!(argv.contains(&flag.to_owned()), "{flag}: {argv:?}");
        }
        // Never a new user or network namespace: the sandbox's own are the boundary.
        for flag in [
            "--unshare-user",
            "--unshare-net",
            "--unshare-all",
            "--share-net",
        ] {
            assert!(!argv.contains(&flag.to_owned()), "{flag}: {argv:?}");
        }
        assert!(position(&argv, &["--cap-add", "ALL"]).is_some());
        assert!(!argv.contains(&"--cap-drop".to_owned()));
        assert!(position(&argv, &["--setenv", "PATH", ENGINE_PATH]).is_some());
        assert!(position(&argv, &["--setenv", "HOME", "/root"]).is_some());
        assert!(position(&argv, &["--hostname", "fixture"]).is_some());
    }

    #[test]
    fn the_engine_sees_a_runs_view_plus_only_its_own_parts() {
        let argv = bwrap_argv(&view());
        let found = binds(&argv);
        let state = "/data/runtime/sandboxes/s1";
        let mut expected: Vec<(String, String, String)> = SYSTEM_LAYER_DIRS
            .iter()
            .map(|d| ("--bind-try".into(), format!("{state}/root{d}"), (*d).into()))
            .collect();
        for (option, source, dest) in [
            ("--bind", "/data/sandboxes/s1", "/data/sandboxes/s1"),
            ("--ro-bind", "/data/runtime/sandboxes/s1/sys", "/sys"),
            (
                "--bind",
                "/sys/fs/cgroup/system.slice/yulius.service/sandboxes/s1/engine",
                "/sys/fs/cgroup",
            ),
            ("--bind", "/data/sysroots/s1/docker", "/var/lib/docker"),
            (
                "--bind",
                "/data/runtime/sandboxes/s1/engine",
                "/run/yulius-engine",
            ),
            (
                "--bind",
                "/data/runtime/sandboxes/s1/docker.sock",
                "/run/docker.sock",
            ),
            (
                "--ro-bind",
                "/usr/local/bin/yulius",
                "/usr/local/bin/yulius",
            ),
        ] {
            expected.push((option.into(), source.into(), dest.into()));
        }
        assert_eq!(found, expected);
        // Nothing else of the holder's state (relay sockets, records, the overlay's internals),
        // no host `/sys`, no host cgroup tree, no device binds.
        for (_, source, _) in &found {
            assert_ne!(source, "/sys");
            assert_ne!(source, "/sys/fs/cgroup");
            assert_ne!(source, "/");
            for private in [
                "in.sock",
                "out.sock",
                "holder.json",
                "host.json",
                "upper",
                "work",
            ] {
                assert!(!source.ends_with(private), "{source}");
            }
        }
    }

    #[test]
    fn mounts_come_in_an_order_that_keeps_each_one_visible() {
        let argv = bwrap_argv(&view());
        let state = "/data/runtime/sandboxes/s1";
        let var = position(&argv, &["--bind-try", &format!("{state}/root/var"), "/var"]);
        let run = position(&argv, &["--tmpfs", "/run"]);
        let private = position(&argv, &["--tmpfs", "/data"]);
        let sandbox = position(&argv, &["--bind", "/data/sandboxes/s1"]);
        let storage = position(&argv, &["--bind", "/data/sysroots/s1/docker"]);
        let sys = position(&argv, &["--ro-bind", &format!("{state}/sys"), "/sys"]);
        let cgroup = position(&argv, &["--bind", &view().cgroup, "/sys/fs/cgroup"]);
        let socket = position(&argv, &["--bind", &format!("{state}/docker.sock")]);
        let exe = position(&argv, &["--ro-bind", "/usr/local/bin/yulius"]);
        assert!(run < var && var < private && private < sandbox, "{argv:?}");
        assert!(var < storage && sys < cgroup && run < socket, "{argv:?}");
        assert!(
            private < exe,
            "the executable may live in the data directory: {argv:?}"
        );
    }

    #[test]
    fn the_engine_command_is_the_supervised_dockerd() {
        let argv = bwrap_argv(&view());
        let tail = position(
            &argv,
            &[
                "--chdir",
                "/",
                "--",
                "/usr/local/bin/yulius",
                "sandbox-host",
                "engine",
                "--",
            ],
        );
        assert!(tail.is_some(), "{argv:?}");
        let dockerd = dockerd_argv();
        assert_eq!(&argv[argv.len() - dockerd.len()..], dockerd.as_slice());
        assert!(
            position(
                &dockerd,
                &["--host", "unix:///run/yulius-engine/docker.sock"]
            )
            .is_some()
        );
        assert!(position(&dockerd, &["--data-root", "/var/lib/docker"]).is_some());
        assert!(position(&dockerd, &["--exec-opt", "native.cgroupdriver=cgroupfs"]).is_some());
        assert!(
            !dockerd.iter().any(|a| a.contains("/run/docker.sock")),
            "{dockerd:?}"
        );
    }
}
