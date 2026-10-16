//! [`SessionHost`] over `yulius-ptyhost`: detached PTY hosts with Unix-socket attach.

use std::collections::{BTreeMap, HashMap};
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use async_trait::async_trait;
use yulius_app::error::{AppError, AppResult};
use yulius_app::ports::session::{
    AttachedRun, Containment, LaunchPlan, PlannedFile, RunEvent, RunExit, RunFreeze, RunInspection,
    RunReader, RunSignal, RunWelcome, RunWriter, SessionHost, SpawnedRun,
};
use yulius_contracts::ptyhost::{
    self as wire, BubblewrapSpec, ClientFrame, ExitReason, ExitRecord, HostFrame, LaunchSpec,
    SignalKind,
};
use yulius_domain::stages::ResourceUse;
use yulius_ptyhost::{
    DEFAULT_SPAWN_TIMEOUT, PtyClient, PtyReader, PtyWriter, RunDirState, SpawnedHost,
    inspect_run_dir, spawn_host, terminate_recorded, token_sha256_hex,
};
use yulius_sandbox::{cgroup, freeze};

/// The ptyhost wire form of an app-level containment plan (shared with the editor adapter).
#[must_use]
pub fn wire_containment(containment: &Containment) -> wire::Containment {
    match containment {
        Containment::None => wire::Containment::None,
        Containment::Bubblewrap {
            sandbox_root,
            home,
            writable,
            readonly,
            binds,
            identity,
            join,
        } => wire::Containment::Bubblewrap(BubblewrapSpec {
            sandbox_root: sandbox_root.to_string_lossy().into_owned(),
            home: home.to_string_lossy().into_owned(),
            writable: writable
                .iter()
                .map(|p| p.to_string_lossy().into_owned())
                .collect(),
            readonly: readonly
                .iter()
                .map(|p| p.to_string_lossy().into_owned())
                .collect(),
            share_net: true,
            binds: binds
                .iter()
                .map(|b| wire::BindMount {
                    source: b.source.to_string_lossy().into_owned(),
                    dest: b.dest.to_string_lossy().into_owned(),
                    writable: b.writable,
                })
                .collect(),
            identity: identity.as_ref().map(|i| wire::SandboxIdentity {
                uid: i.uid,
                gid: i.gid,
                hostname: i.hostname.clone(),
            }),
            join: join.as_ref().map(|j| {
                Box::new(wire::SandboxJoin {
                    holder_pid: j.holder_pid,
                    user_ns: j.user_ns,
                    net_ns: j.net_ns,
                    mnt_ns: j.mnt_ns,
                    cgroup: j.cgroup.as_ref().map(|p| p.to_string_lossy().into_owned()),
                    system_root: j.system_root.to_string_lossy().into_owned(),
                    hostname: j.hostname.clone(),
                    exec_shim: j
                        .exec_shim
                        .as_ref()
                        .map(|p| p.to_string_lossy().into_owned()),
                    private_dirs: j
                        .private_dirs
                        .iter()
                        .map(|p| p.to_string_lossy().into_owned())
                        .collect(),
                    host_system: j
                        .host_system
                        .as_ref()
                        .map(|p| p.to_string_lossy().into_owned()),
                    engine_socket: j
                        .engine_socket
                        .as_ref()
                        .map(|p| p.to_string_lossy().into_owned()),
                })
            }),
        }),
    }
}

/// Process host adapter.
pub struct PtySessionHost {
    /// Executable started as `<exe> ptyhost --run-dir <dir>` (the `yulius` binary).
    pub exe: PathBuf,
    hosts: Mutex<HashMap<PathBuf, SpawnedHost>>,
}

impl PtySessionHost {
    /// Adapter starting hosts with `exe`.
    #[must_use]
    pub fn new(exe: PathBuf) -> Self {
        Self {
            exe,
            hosts: Mutex::new(HashMap::new()),
        }
    }
}

fn write_planned(file: &PlannedFile) -> std::io::Result<()> {
    if let Some(parent) = file.path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let name = file
        .path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let tmp = file.path.with_file_name(format!(".{name}.yulius-tmp"));
    let _ = std::fs::remove_file(&tmp);
    let mut out = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(file.mode)
        .open(&tmp)?;
    out.write_all(&file.contents)?;
    out.sync_all()?;
    std::fs::rename(&tmp, &file.path)
}

/// The run's end, with what it used as its host recorded in `run_dir`'s `exit.json` (written
/// before the `Exit` frame is sent; SPEC-154 USE-001).
fn exit_of(record: &ExitRecord, run_dir: &Path) -> RunExit {
    RunExit {
        code: record.code,
        signal: record.signal,
        spawn_failed: record.reason == ExitReason::SpawnFailed,
        usage: yulius_ptyhost::exit_usage(run_dir)
            .map(|used| ResourceUse::measured(used.cpu_ms, used.max_rss_bytes, used.written_bytes)),
    }
}

fn upstream(context: &str, err: impl std::fmt::Display) -> AppError {
    AppError::Upstream(format!("{context}: {err}"))
}

#[async_trait]
impl SessionHost for PtySessionHost {
    async fn spawn(
        &self,
        run_dir: &Path,
        attempt_id: &str,
        task_id: &str,
        plan: &LaunchPlan,
        token: &[u8; 32],
        size: (u16, u16),
    ) -> AppResult<SpawnedRun> {
        for file in &plan.files {
            write_planned(file).map_err(|e| upstream("cannot write run configuration", e))?;
        }
        let env: BTreeMap<String, String> = plan.env.iter().cloned().collect();
        let containment = wire_containment(&plan.containment);
        let launch = LaunchSpec {
            version: wire::RECORD_VERSION,
            attempt_id: attempt_id.to_owned(),
            task_id: task_id.to_owned(),
            argv: plan.argv.clone(),
            cwd: plan.cwd.to_string_lossy().into_owned(),
            env,
            secret_env_names: plan.secret_env.iter().map(|(k, _)| k.clone()).collect(),
            cols: size.0.max(10),
            rows: size.1.max(5),
            containment,
            ring_bytes: 0,
            token_sha256: token_sha256_hex(token),
            linger_ms: wire::DEFAULT_LINGER_MS,
        };
        let spawned = spawn_host(
            &self.exe,
            run_dir,
            &launch,
            &plan.secret_env,
            DEFAULT_SPAWN_TIMEOUT,
        )
        .await
        .map_err(|e| upstream("the agent process could not be started", e))?;
        let run = SpawnedRun {
            host_pid: spawned.host_pid(),
            child_pid: spawned.status().child_pid,
        };
        if let Ok(mut hosts) = self.hosts.lock() {
            hosts.insert(run_dir.to_path_buf(), spawned);
        }
        Ok(run)
    }

    async fn attach(&self, run_dir: &Path, token: &[u8; 32]) -> AppResult<AttachedRun> {
        let (client, welcome, snapshot) = PtyClient::connect(run_dir, token, true)
            .await
            .map_err(|e| upstream("cannot attach to the session", e))?;
        let (reader, writer) = client.into_split();
        Ok(AttachedRun {
            welcome: RunWelcome {
                child_pid: welcome.child_pid,
                seq: snapshot.as_ref().map_or(welcome.seq, |s| s.seq),
                cols: welcome.cols,
                rows: welcome.rows,
                snapshot: snapshot.map(|s| s.bytes),
            },
            reader: Box::new(Reader(reader, run_dir.to_path_buf())),
            writer: Box::new(Writer(writer)),
        })
    }

    fn inspect(&self, run_dir: &Path) -> RunInspection {
        match inspect_run_dir(run_dir) {
            RunDirState::Missing => RunInspection::Missing,
            RunDirState::Exited(exit) => RunInspection::Exited(exit_of(&exit, run_dir)),
            RunDirState::Recorded {
                host_alive,
                child_alive,
                same_boot,
                ..
            } => RunInspection::Recorded {
                host_alive,
                child_alive,
                same_boot,
            },
        }
    }

    fn background_shells(&self, child_pid: u32) -> Option<u32> {
        yulius_ptyhost::shells::background_shells(child_pid)
    }

    fn background_probe(
        &self,
        child_pid: u32,
    ) -> Option<yulius_app::ports::session::BackgroundProbe> {
        yulius_ptyhost::shells::background_probe(child_pid).map(|count| {
            yulius_app::ports::session::BackgroundProbe {
                shells: count.shells,
                waits: count.waits,
            }
        })
    }

    fn terminate(&self, run_dir: &Path) -> AppResult<()> {
        if let Ok(mut hosts) = self.hosts.lock() {
            hosts.remove(run_dir);
        }
        match inspect_run_dir(run_dir) {
            RunDirState::Recorded { status, .. } => {
                terminate_recorded(&status).map_err(|e| upstream("cannot terminate the session", e))
            }
            _ => Ok(()),
        }
    }

    fn freeze(&self, run_dir: &Path) -> RunFreeze {
        let Some((tree, child, name)) = freeze_target(run_dir) else {
            return RunFreeze::Unsupported;
        };
        match freeze::freeze(&tree, &freeze::Procfs, child, &name) {
            Ok(frozen) if frozen.settled => RunFreeze::Frozen,
            Ok(_) => RunFreeze::Freezing,
            Err(error) => {
                tracing::debug!(run = %name, %error, "the run pauses at its turn boundary");
                // A group frozen before a later write failed must not stay frozen.
                let _ = freeze::thaw(&tree, &freeze::Procfs, child, &name);
                RunFreeze::Unsupported
            }
        }
    }

    fn can_freeze(&self, run_dir: &Path) -> bool {
        freeze_target(run_dir).is_some_and(|(tree, child, name)| {
            freeze::check(&tree, &freeze::Procfs, child, &name).is_ok()
        })
    }

    fn thaw(&self, run_dir: &Path) {
        if let Some((tree, child, name)) = freeze_target(run_dir)
            && let Err(error) = freeze::thaw(&tree, &freeze::Procfs, child, &name)
        {
            tracing::warn!(run = %name, %error, "cannot thaw the run");
        }
    }

    fn thaw_all(&self) -> usize {
        freeze_tree().map_or(0, |tree| freeze::thaw_all(&tree.unit))
    }
}

/// The cgroup tree this server may freeze runs in: its delegated unit (SPEC-160).
fn freeze_tree() -> Option<freeze::Tree> {
    let own = std::fs::read_to_string("/proc/self/cgroup").ok()?;
    let own = cgroup::own_path(&own)?;
    let root = Path::new(cgroup::CGROUP_ROOT);
    let unit = cgroup::detect(root, &own).ok()?;
    Some(freeze::Tree {
        root: root.to_path_buf(),
        unit,
    })
}

/// The tree, the live child and the name of a run: the run directory's name (its attempt).
fn freeze_target(run_dir: &Path) -> Option<(freeze::Tree, u32, String)> {
    let RunDirState::Recorded {
        status,
        child_alive: true,
        ..
    } = inspect_run_dir(run_dir)
    else {
        return None;
    };
    let name = run_dir.file_name()?.to_str()?.to_owned();
    Some((freeze_tree()?, status.child_pid, name))
}

struct Reader(PtyReader, PathBuf);

#[async_trait]
impl RunReader for Reader {
    async fn next(&mut self) -> AppResult<Option<RunEvent>> {
        loop {
            match self
                .0
                .recv()
                .await
                .map_err(|e| upstream("session connection", e))?
            {
                None => return Ok(None),
                Some(HostFrame::Output { seq, bytes }) => {
                    return Ok(Some(RunEvent::Output { seq, bytes }));
                }
                Some(HostFrame::Snapshot(s)) => {
                    return Ok(Some(RunEvent::Snapshot {
                        seq: s.seq,
                        bytes: s.bytes,
                    }));
                }
                Some(HostFrame::Exit(exit)) => {
                    return Ok(Some(RunEvent::Exit(exit_of(&exit, &self.1))));
                }
                Some(HostFrame::Error { code, message }) => {
                    if code == wire::error_code::REPLACED {
                        return Ok(None);
                    }
                    return Err(AppError::Upstream(format!(
                        "session host error {code}: {message}"
                    )));
                }
                Some(HostFrame::Welcome(_) | HostFrame::Pong(_)) => {}
            }
        }
    }
}

struct Writer(PtyWriter);

#[async_trait]
impl RunWriter for Writer {
    async fn input(&mut self, bytes: &[u8]) -> AppResult<()> {
        for frame in wire::input_frames(bytes) {
            self.0
                .send(frame)
                .await
                .map_err(|e| upstream("session input", e))?;
        }
        Ok(())
    }

    async fn resize(&mut self, cols: u16, rows: u16) -> AppResult<()> {
        self.0
            .send(ClientFrame::Resize { cols, rows })
            .await
            .map_err(|e| upstream("session resize", e))
    }

    async fn signal(&mut self, signal: RunSignal) -> AppResult<()> {
        let kind = match signal {
            RunSignal::Interrupt => SignalKind::Interrupt,
            RunSignal::Terminate => SignalKind::Terminate,
            RunSignal::Kill => SignalKind::Kill,
        };
        self.0
            .send(ClientFrame::Signal(kind))
            .await
            .map_err(|e| upstream("session signal", e))
    }

    async fn shutdown(&mut self, grace_ms: u32) -> AppResult<()> {
        self.0
            .send(ClientFrame::Shutdown { grace_ms })
            .await
            .map_err(|e| upstream("session shutdown", e))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// SPEC-134 (review round 3, H1): a production CD run's containment passes the process
    /// host's launch validation and becomes a joined `bwrap` run on the host's read-only system.
    #[test]
    fn a_production_cd_run_launches_on_the_hosts_system() -> Result<(), Box<dyn std::error::Error>>
    {
        let join = yulius_app::ports::isolation::SandboxJoin {
            holder_pid: 4242,
            user_ns: 1,
            net_ns: 2,
            mnt_ns: 3,
            cgroup: Some("/sys/fs/cgroup/yulius/sandboxes/s1/agents".into()),
            ci_cgroup: Some("/sys/fs/cgroup/yulius/sandboxes/s1/ci".into()),
            system_root: "/data/runtime/sandboxes/s1/root".into(),
            hostname: "app".into(),
            exec_shim: Some("/usr/local/bin/yulius".into()),
            extra_readonly: Vec::new(),
            private_dirs: vec!["/data".into()],
            system_passwd: None,
            system_group: None,
            host_system: None,
            engine_socket: None,
        };
        let containment = yulius_app::ci::cd::production_containment(
            std::path::Path::new("/data/runs/cd-r/work"),
            std::path::Path::new("/data/runs/cd-r/work/home"),
            std::path::Path::new("/data/cd/s1/cache/prod"),
            vec!["/usr/local/bin/yulius".into()],
            std::path::Path::new("/data/runs/cd-r/work/resolv.conf"),
            join,
        );
        let launch = LaunchSpec {
            version: wire::RECORD_VERSION,
            attempt_id: "r".into(),
            task_id: "cd-r".into(),
            argv: vec!["/bin/sh".into(), "-c".into(), "true".into()],
            cwd: "/data/runs/cd-r/work/src".into(),
            env: BTreeMap::new(),
            secret_env_names: vec!["PROD_TOKEN".into()],
            cols: 160,
            rows: 48,
            containment: wire_containment(&containment),
            ring_bytes: 0,
            token_sha256: "0".repeat(64),
            linger_ms: wire::DEFAULT_LINGER_MS,
        };
        launch.validate()?;
        let wire::Containment::Bubblewrap(spec) = &launch.containment else {
            return Err("a bubblewrap containment".into());
        };
        let argv = yulius_ptyhost::bwrap_argv(spec, &launch.argv, &launch.cwd);
        assert!(
            argv.windows(3)
                .any(|w| w == ["--ro-bind-try", "/etc", "/etc"]),
            "{argv:?}"
        );
        assert!(
            argv.windows(2).any(|w| w == ["--tmpfs", "/var"]),
            "{argv:?}"
        );
        assert!(
            !argv.iter().any(|a| a.contains("/data/runtime/sandboxes")),
            "{argv:?}"
        );
        assert!(
            !argv.iter().any(|a| a.contains("/data/sandboxes")),
            "{argv:?}"
        );
        Ok(())
    }
}
