//! Running a brokered command outside the agent's sandbox (SPEC-626 BRK-008..BRK-011,
//! ADR-0459): in the project's production holder on the host's read-only system (the previous
//! containment, or the server's child, where there is none), in a fresh home, with the key in a
//! 0600 file of its own and nothing of the server's environment, bounded in time and files.

use std::io::{Read as _, Write as _};
use std::os::unix::fs::{DirBuilderExt as _, OpenOptionsExt as _};
use std::path::{Path, PathBuf};
use std::time::Duration;

use tokio::sync::watch;
use yulius_domain::cd::CdStage;
use yulius_domain::commands as dom;
use zeroize::Zeroizing;

use super::exec::{DiskBound, Ended, RunLog, Sink};
use crate::context::App;
use crate::delivery::runner::OutputRing;
use crate::model::SandboxRecord;
use crate::ports::session::{Containment, LaunchPlan};

/// The most bytes of a run's files (its output included) before it is stopped (BRK-010).
pub(crate) const MAX_RUN_FILES: u64 = 16 * 1024 * 1024;
/// The most of each stream read back from its file.
const READ_BACK: u64 = 4 * 1024 * 1024;
/// `PATH` of a run: the host's system only.
const RUN_PATH: &str = "/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin";
/// Runs the command with empty stdin and its streams in two files, without a shell reading any
/// argument: `$1` and `$2` are the files, the rest is the command and its arguments.
const WRAPPER: &str = r#"out=$1 err=$2; shift 2; exec "$@" </dev/null >"$out" 2>"$err""#;

/// What a run is given: its command line (expanded) and environment.
pub(crate) struct BrokeredRun<'a> {
    /// The run's id.
    pub id: &'a str,
    /// The binary.
    pub binary: &'a str,
    /// The fixed arguments, as defined.
    pub fixed: &'a [String],
    /// The agent's arguments, checked.
    pub args: &'a [String],
    /// The definition's env, as defined.
    pub env: &'a std::collections::BTreeMap<String, String>,
    /// The key file's text.
    pub key: Zeroizing<String>,
    /// The key's value, for `{secret}` in env.
    pub secret: Zeroizing<String>,
    /// The time limit.
    pub timeout: Duration,
}

/// How a run ended, before redaction.
pub(crate) struct BrokeredOutcome {
    /// `done`, `timed_out`, `output_limit`, `failed` or `lost`.
    pub state: &'static str,
    /// Why, when not `done`.
    pub reason: Option<String>,
    /// The exit code.
    pub exit_code: Option<i32>,
    /// stdout as read back.
    pub stdout: String,
    /// stderr as read back.
    pub stderr: String,
    /// The whole size of stdout and of stderr, of which the text is the start.
    pub sizes: (u64, u64),
}

impl BrokeredOutcome {
    pub(crate) fn failed(reason: String) -> Self {
        Self {
            state: "failed",
            reason: Some(reason),
            exit_code: None,
            stdout: String::new(),
            stderr: String::new(),
            sizes: (0, 0),
        }
    }
}

/// Makes `path` a fresh folder only its owner reads.
fn fresh_dir(path: &Path) -> std::io::Result<()> {
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(path)
}

/// Makes a run's output file and keeps its handle: what Yulius reads back is that file, whatever
/// the run puts at its path later (a link to a host file, a FIFO; review round 1 of slice 2).
fn output_file(path: &Path) -> std::io::Result<std::fs::File> {
    std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .custom_flags(nix::fcntl::OFlag::O_NOFOLLOW.bits())
        .mode(0o600)
        .open(path)
}

/// Reads at most [`READ_BACK`] bytes of an output file through its handle, from its start, and
/// its whole size.
fn read_back(file: &std::fs::File) -> (String, u64) {
    use std::io::{Seek as _, SeekFrom};
    let size = file.metadata().map_or(0, |m| m.len());
    let mut bytes = Vec::new();
    let mut reader = file;
    if reader.seek(SeekFrom::Start(0)).is_ok() {
        let _ = reader.take(READ_BACK).read_to_end(&mut bytes);
    }
    (String::from_utf8_lossy(&bytes).into_owned(), size)
}

impl App {
    /// The run directory of a brokered run (its process host's) and where its files are.
    pub(crate) fn command_run_dirs(
        &self,
        sandbox: &yulius_domain::ids::SandboxId,
        id: &str,
    ) -> (PathBuf, PathBuf) {
        let run_dir = self.config.data_dir.join("runs").join(format!("cmd-{id}"));
        let files = self.run_files_dir(sandbox, CdStage::Prod, &run_dir, &format!("cmd-{id}"));
        (run_dir, files)
    }

    /// Removes a brokered run's files, its key with them, after its process host was told to
    /// end (a restart's recovery, BRK-009).
    pub(crate) async fn remove_command_run_files(
        &self,
        sandbox: &yulius_domain::ids::SandboxId,
        id: &str,
    ) {
        let (run_dir, files) = self.command_run_dirs(sandbox, id);
        if run_dir.exists() {
            let _ = self.deps.session_host.terminate(&run_dir);
        }
        self.remove_run_dirs(&run_dir, &files).await;
    }

    /// Runs `run` of `sandbox` to its end (BRK-008..BRK-010): its files are gone when this
    /// returns, whatever the outcome.
    pub(crate) async fn run_brokered(
        &self,
        sandbox: &SandboxRecord,
        run: BrokeredRun<'_>,
    ) -> BrokeredOutcome {
        let (run_dir, files) = self.command_run_dirs(&sandbox.id, run.id);
        let outcome = self.run_brokered_in(sandbox, &run, &run_dir, &files).await;
        self.remove_run_dirs(&run_dir, &files).await;
        outcome
    }

    async fn run_brokered_in(
        &self,
        sandbox: &SandboxRecord,
        run: &BrokeredRun<'_>,
        run_dir: &Path,
        files: &Path,
    ) -> BrokeredOutcome {
        let work = files.join("work");
        let (home, tmp, log) = (work.join("home"), work.join("tmp"), work.join("log"));
        let (out, err) = (work.join("out"), work.join("err"));
        // Outside the home, in a folder of a random name: an argument without `/` cannot name it.
        let nonce: String = self
            .deps
            .crypto
            .random_bytes()
            .iter()
            .take(12)
            .map(|b| format!("{b:02x}"))
            .collect();
        let keyfile = work.join("key").join(nonce).join("key");
        let _ = std::fs::remove_dir_all(run_dir);
        let _ = std::fs::remove_dir_all(files);
        let prepared = (|| -> std::io::Result<(std::fs::File, std::fs::File)> {
            for dir in [&home, &tmp, keyfile.parent().unwrap_or(&work)] {
                fresh_dir(dir)?;
            }
            std::fs::File::create(&log)?;
            let mut key = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&keyfile)?;
            key.write_all(run.key.as_bytes())?;
            key.sync_all()?;
            Ok((output_file(&out)?, output_file(&err)?))
        })();
        let (out_file, err_file) = match prepared {
            Ok(files) => files,
            Err(e) => {
                return BrokeredOutcome::failed(format!("preparing the run's folder failed: {e}"));
            }
        };
        let (containment, _lease) = match self
            .cd_containment(sandbox, CdStage::Prod, &work, &home, &tmp)
            .await
        {
            Ok(joined) => joined,
            Err(reason) => return BrokeredOutcome::failed(format!("nothing ran: {reason}")),
        };
        let containment = without_docker(containment);
        let (keyfile_text, home_text) = (
            keyfile.to_string_lossy().into_owned(),
            home.to_string_lossy().into_owned(),
        );
        let mut env = vec![
            ("HOME".to_owned(), home_text.clone()),
            ("PATH".to_owned(), RUN_PATH.to_owned()),
            ("LANG".to_owned(), "C.UTF-8".to_owned()),
            ("TERM".to_owned(), "dumb".to_owned()),
            ("TMPDIR".to_owned(), tmp.to_string_lossy().into_owned()),
        ];
        let mut secret_env = Vec::new();
        for (name, value) in run.env {
            if value.contains(dom::SECRET) {
                secret_env.push((
                    name.clone(),
                    dom::expand(value, &keyfile_text, &home_text, Some(&run.secret)),
                ));
            } else {
                env.push((
                    name.clone(),
                    dom::expand(value, &keyfile_text, &home_text, None),
                ));
            }
        }
        let mut argv = vec![
            "/bin/sh".to_owned(),
            "-c".to_owned(),
            WRAPPER.to_owned(),
            "yulius-command".to_owned(),
            out.to_string_lossy().into_owned(),
            err.to_string_lossy().into_owned(),
            run.binary.to_owned(),
        ];
        argv.extend(
            run.fixed
                .iter()
                .map(|a| dom::expand(a, &keyfile_text, &home_text, None)),
        );
        argv.extend(run.args.iter().cloned());
        let plan = LaunchPlan {
            argv,
            cwd: home.clone(),
            env,
            secret_env,
            files: Vec::new(),
            containment,
        };
        let token = self.deps.crypto.random_bytes();
        let (_cancel_tx, mut cancel) = watch::channel(None);
        let mut ring = OutputRing::new(4096);
        let watched = self
            .ci_watch(
                run_dir,
                (run.id, &format!("cmd-{}", run.id)),
                Sink::Command,
                &plan,
                &token,
                RunLog::At(&log),
                (&mut ring, None, None),
                (
                    &mut cancel,
                    Some(DiskBound {
                        dirs: vec![("run files", files.to_path_buf())],
                        max: MAX_RUN_FILES,
                    }),
                    run.timeout,
                ),
            )
            .await;
        // Off the async runtime, through the handles made before the run.
        let ((stdout, stdout_size), (stderr, stderr_size)) =
            tokio::task::spawn_blocking(move || (read_back(&out_file), read_back(&err_file)))
                .await
                .unwrap_or_default();
        let (state, reason, exit_code) = match watched {
            Err(reason) => ("failed", Some(reason), None),
            Ok((Ended::Exit(exit), _)) if exit.spawn_failed => (
                "failed",
                Some("the command could not be started".to_owned()),
                None,
            ),
            Ok((Ended::Exit(exit), _)) => match (exit.code, exit.signal) {
                (Some(code), _) => ("done", None, Some(code)),
                (None, Some(signal)) => ("failed", Some(format!("ended by signal {signal}")), None),
                (None, None) => (
                    "failed",
                    Some("ended without an exit code".to_owned()),
                    None,
                ),
            },
            Ok((Ended::TimedOut, _)) => (
                "timed_out",
                Some(format!(
                    "stopped at its time limit of {} s",
                    run.timeout.as_secs()
                )),
                None,
            ),
            Ok((Ended::OutputLimit | Ended::DiskLimit(_), _)) => (
                "output_limit",
                Some(format!(
                    "stopped: its files (its output, home and temporary files) passed {} MiB",
                    MAX_RUN_FILES >> 20
                )),
                None,
            ),
            Ok((Ended::NoDisk(reason), _)) => {
                ("failed", Some(format!("nothing ran: {reason}")), None)
            }
            Ok((Ended::Cancelled(reason), _)) => ("failed", Some(reason), None),
            Ok((Ended::Lost, _)) => ("lost", Some("its process host was lost".to_owned()), None),
        };
        BrokeredOutcome {
            sizes: (stdout_size, stderr_size),
            state,
            reason,
            exit_code,
            stdout,
            stderr,
        }
    }
}

/// A brokered run gets no Docker engine (ADR-0459).
fn without_docker(mut containment: Containment) -> Containment {
    if let Containment::Bubblewrap {
        join: Some(join), ..
    } = &mut containment
    {
        join.docker_dir = None;
    }
    containment
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ports::isolation::SandboxJoin;

    /// BRK-008: in the production holder, a brokered run is on the host's read-only system with
    /// only its own folder bound, and without the production engine's Docker directory.
    #[test]
    fn a_brokered_run_joins_the_production_holder_without_docker() {
        let join = SandboxJoin {
            holder_pid: 7,
            user_ns: 1,
            net_ns: 2,
            mnt_ns: 3,
            cgroup: None,
            ci_cgroup: None,
            system_root: "/data/runtime/sandboxes/s1.prod/root".into(),
            hostname: "app-prod".into(),
            exec_shim: None,
            extra_binds: Vec::new(),
            private_dirs: Vec::new(),
            system_passwd: None,
            system_group: None,
            host_system: None,
            docker_dir: Some("/data/runtime/sandboxes/s1.prod/docker".into()),
        };
        let work = Path::new("/data/cd/s1/runs/cmd-run-1/work");
        let joined = super::super::cd::production_containment(
            work,
            &work.join("home"),
            &work.join("tmp"),
            Vec::new(),
            &work.join("resolv.conf"),
            join,
        );
        let Containment::Bubblewrap {
            sandbox_root,
            binds,
            join: Some(join),
            ..
        } = without_docker(joined)
        else {
            panic!("a contained run");
        };
        assert_eq!(sandbox_root, work);
        assert!(binds.is_empty());
        assert!(join.host_system.is_some(), "the host's read-only system");
        assert_eq!(join.docker_dir, None);
        assert!(join.extra_binds.is_empty());
    }
}
