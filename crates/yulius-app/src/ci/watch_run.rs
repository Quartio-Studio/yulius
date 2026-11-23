//! The process of a script watch's run (SPEC-630 WCH-002 to WCH-004, ADR-0465), started like a CI
//! job: through the process host, in the sandbox's containment (bubblewrap on the sandbox root,
//! joined to the sandbox's namespace holder in CI's group), with no agent, task token, GitHub
//! token or hooks.
//!
//! A run is `/bin/bash -c <wrapper> yulius-watch <out> <err> <cpu> <memory> <output> <script>`:
//! the fixed wrapper sets the run's bounds as soft and hard limits (so the script cannot raise
//! them), sends stdout and stderr to their files and runs the script as its own `bash -c`
//! argument, so nothing is spliced into shell text. Its directories are Yulius's, under
//! `<data>/watches/<sandbox>/`: the project's watch `home` and the watch's `state/<id>` are kept
//! between runs, `runs/<run>` holds this run's output and goes with it. Only those three are
//! bound writable into the containment; their parent is not, so a script cannot put a link where
//! Yulius later writes. Both output files are read through handles opened before the start.

use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::time::Duration;

use tokio::sync::watch;
use yulius_domain::vault::{SecretName, placeholder};
use yulius_domain::watch::{ERROR_TAIL_CHARS, RunBounds, RunEnd, cut_tail};

use super::exec::{CiProxy, DiskBound, Ended, RunLog, Sink, bind_run_files, ci_join};
use crate::context::App;
use crate::delivery::runner::OutputRing;
use crate::model::SandboxRecord;
use crate::ports::session::{Containment, LaunchPlan};
use crate::ports::store_ext::WatchRecord;

/// `$1` stdout, `$2` stderr, `$3` CPU seconds (soft: SIGXCPU), `$4` CPU seconds (hard: SIGKILL),
/// `$5` data KiB, `$6` file KiB, `$7` the script, after `--` so a script starting with `-` is
/// never read as bash's options (review round 1).
const WRAPPER: &str = r#"ulimit -St "$3" && ulimit -Ht "$4" && ulimit -d "$5" && ulimit -f "$6" || exit 125
exec >>"$1" 2>>"$2" </dev/null
exec /bin/bash -c -- "$7""#;
/// CPU seconds a run gets past its soft limit (SIGXCPU) before the hard one kills it.
const CPU_GRACE_SECONDS: u32 = 2;

/// What a run printed and how it ended.
pub(crate) struct WatchRunOutput {
    /// How its process ended.
    pub(crate) end: RunEnd,
    /// Its stdout, at most its output bound.
    pub(crate) output: String,
    /// The tail of its stderr, with Yulius's notes (its agent secrets) first.
    pub(crate) errors: String,
}

impl WatchRunOutput {
    fn failed(reason: String) -> Self {
        Self {
            end: RunEnd::Failed(reason),
            output: String::new(),
            errors: String::new(),
        }
    }
}

impl App {
    /// The project's watch directory (WCH-002), Yulius's own, outside the sandbox.
    pub(crate) fn watch_base_dir(&self, sandbox: &yulius_domain::ids::SandboxId) -> PathBuf {
        self.config.data_dir.join("watches").join(sandbox.as_str())
    }

    /// The process host's directory of run `run_id`.
    pub(crate) fn watch_run_dir(&self, run_id: &str) -> PathBuf {
        self.config
            .data_dir
            .join("runs")
            .join(format!("watch-{run_id}"))
    }

    /// Runs `watch`'s script once as run `run_id` and follows it to its end: its timeout, its
    /// bounds, `cancel`. Never fails: a run that could not start is a failed run.
    pub(crate) async fn watch_execute(
        &self,
        sandbox: &SandboxRecord,
        watch: &WatchRecord,
        (run_id, bounds): (&str, RunBounds),
        cancel: &mut watch::Receiver<Option<String>>,
    ) -> WatchRunOutput {
        let base = self.watch_base_dir(&sandbox.id);
        let (home, state, files) = (
            base.join("home"),
            base.join("state").join(&watch.id),
            base.join("runs").join(run_id),
        );
        let (out, err) = (files.join("out"), files.join("err"));
        let run_dir = self.watch_run_dir(run_id);
        let made = (|| -> std::io::Result<(std::fs::File, std::fs::File)> {
            for dir in [&home, &state, &files] {
                std::fs::create_dir_all(dir)?;
            }
            std::fs::File::create(&err)?;
            std::fs::File::create(&out)?;
            // Both read through handles opened before the start: a link the script puts at
            // either path is never followed.
            Ok((std::fs::File::open(&out)?, std::fs::File::open(&err)?))
        })();
        let (stdout, stderr) = match made {
            Ok(files) => files,
            Err(error) => {
                let _ = std::fs::remove_dir_all(&files);
                return WatchRunOutput::failed(format!(
                    "preparing the run's directories failed: {error}"
                ));
            }
        };
        let result = self
            .watch_process(
                sandbox,
                watch,
                (run_id, bounds),
                [&home, &state, &files],
                (&out, &err),
                cancel,
            )
            .await;
        let output = match result {
            Ok((end, errors)) => {
                // Bounded by the run's output limit, and here too.
                let limit = bounds.output_kib.saturating_mul(1024);
                let mut bytes = Vec::new();
                let _ = stdout.take(limit.saturating_add(1)).read_to_end(&mut bytes);
                // A file at its bound: a write past it was refused (EFBIG) or killed its writer
                // (SIGXFSZ), whichever the run's signal dispositions made it; either way the
                // output is cut, so the run failed on its output bound (WCH-003).
                let full = |len: u64| limit > 0 && len >= limit;
                let err_len = stderr.metadata().map_or(0, |m| m.len());
                let at_bound =
                    full(u64::try_from(bytes.len()).unwrap_or(u64::MAX)) || full(err_len);
                let end = match end {
                    RunEnd::Exited(_) | RunEnd::Signaled(_) if at_bound => RunEnd::OutputLimit,
                    end => end,
                };
                WatchRunOutput {
                    end,
                    output: String::from_utf8_lossy(&bytes).into_owned(),
                    errors,
                }
            }
            Err(reason) => WatchRunOutput::failed(reason),
        };
        let _ = std::fs::remove_dir_all(&files);
        let _ = std::fs::remove_dir_all(&run_dir);
        output
    }

    /// Starts the run's process and follows it; its end and its error output's tail.
    async fn watch_process(
        &self,
        sandbox: &SandboxRecord,
        watch: &WatchRecord,
        (run_id, bounds): (&str, RunBounds),
        [home, state, files]: [&Path; 3],
        (out, err): (&Path, &Path),
        cancel: &mut watch::Receiver<Option<String>>,
    ) -> Result<(RunEnd, String), String> {
        // Held until the run ends: the sandbox's holder is not stopped as idle meanwhile.
        let (containment, _lease) = if self.config.contained {
            let (join, lease) = match self.isolation_join(sandbox).await {
                Ok(joined) => joined.unzip(),
                Err(error) => return Err(error.public_message()),
            };
            let containment = Containment::Bubblewrap {
                sandbox_root: self.sandbox_dir(&sandbox.id),
                home: home.to_path_buf(),
                writable: vec![home.to_path_buf(), state.to_path_buf(), files.to_path_buf()],
                readonly: self.config.contained_readonly.clone(),
                binds: join
                    .iter()
                    .flat_map(|j| j.extra_binds.iter().cloned())
                    .collect(),
                identity: None,
                join: join.map(|join| Box::new(ci_join(join))),
            };
            (containment, lease)
        } else {
            (Containment::None, None)
        };
        let repo = self.sandbox_repo(&sandbox.id);
        let cwd = if repo.is_dir() {
            repo
        } else {
            home.to_path_buf()
        };
        let path = std::env::var("PATH")
            .ok()
            .filter(|p| !p.is_empty())
            .unwrap_or_else(|| "/usr/local/bin:/usr/bin:/bin".into());
        let env: Vec<(String, String)> = [
            ("HOME", home.to_string_lossy().into_owned()),
            ("PATH", path),
            ("LANG", "C.UTF-8".to_owned()),
            ("TERM", "dumb".to_owned()),
            ("NO_COLOR", "1".to_owned()),
            ("GIT_TERMINAL_PROMPT", "0".to_owned()),
            ("YULIUS_WATCH", watch.name.clone()),
            ("YULIUS_WATCH_STATE", state.to_string_lossy().into_owned()),
            ("YULIUS_SANDBOX", sandbox.slug.clone()),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_owned(), v))
        .collect();
        let mut plan = LaunchPlan {
            argv: vec![
                "/bin/bash".into(),
                "-c".into(),
                WRAPPER.into(),
                "yulius-watch".into(),
                out.to_string_lossy().into_owned(),
                err.to_string_lossy().into_owned(),
                bounds.cpu_seconds.to_string(),
                bounds
                    .cpu_seconds
                    .saturating_add(CPU_GRACE_SECONDS)
                    .to_string(),
                bounds.memory_kib.to_string(),
                bounds.output_kib.to_string(),
                watch.script.clone(),
            ],
            cwd,
            env,
            secret_env: Vec::new(),
            files: Vec::new(),
            containment,
        };
        let run_dir = self.watch_run_dir(run_id);
        let mut ring = OutputRing::new(ERROR_TAIL_CHARS * 4);
        let proxy = self
            .watch_secrets(sandbox, watch, run_id, &run_dir, &mut plan, &mut ring)
            .await;
        let _ = std::fs::remove_dir_all(&run_dir);
        std::fs::create_dir_all(&run_dir)
            .map_err(|e| format!("creating the run directory failed: {e}"))?;
        let token = self.deps.crypto.random_bytes();
        let group = format!("watch-{}", watch.id);
        let ended = self
            .ci_watch(
                &run_dir,
                (run_id, &group),
                Sink::Watch,
                &plan,
                &token,
                RunLog::At(err),
                (&mut ring, None, None),
                (
                    cancel,
                    // The project's watch files are outside the sandbox's quota: a run that
                    // grows them past their bound is stopped (review round 1).
                    Some(DiskBound {
                        dirs: vec![("watch files", self.watch_base_dir(&sandbox.id))],
                        max: self.config.watches.disk_max,
                    }),
                    Duration::from_secs(u64::from(watch.timeout_seconds)),
                ),
            )
            .await;
        // The process ended (or its host is gone): its proxy credential ends too.
        drop(proxy);
        let (ended, _used) = ended?;
        let end = match ended {
            Ended::Exit(exit) if exit.spawn_failed => {
                RunEnd::Failed("the process host could not start it".into())
            }
            Ended::Exit(exit) => match (exit.code, exit.signal) {
                (Some(code), _) => RunEnd::Exited(code),
                (None, Some(signal)) => RunEnd::Signaled(signal),
                (None, None) => RunEnd::Failed("no exit status was reported".into()),
            },
            Ended::TimedOut => RunEnd::TimedOut,
            Ended::OutputLimit => RunEnd::Failed("its error output went past 256 MiB".into()),
            Ended::Lost => RunEnd::Failed("its process host was lost".into()),
            Ended::Cancelled(reason) | Ended::NoDisk(reason) | Ended::DiskLimit(reason) => {
                RunEnd::Failed(reason)
            }
        };
        Ok((end, cut_tail(ring.text().trim(), ERROR_TAIL_CHARS)))
    }

    /// Gives a run the agent secrets its watch names (WCH-004), as a CI job's grant is given:
    /// a placeholder for each that reaches runs of no person (never a production-deploy one) and
    /// the egress proxy with a credential covering only those names, closed when it is dropped.
    /// What it gave, and what it could not, is noted in the run's error output.
    async fn watch_secrets(
        &self,
        sandbox: &SandboxRecord,
        watch: &WatchRecord,
        run_id: &str,
        run_dir: &Path,
        plan: &mut LaunchPlan,
        ring: &mut OutputRing,
    ) -> Option<CiProxy<'_>> {
        if watch.secrets.is_empty() {
            return None;
        }
        let records = match self
            .applied_agent_secrets(&sandbox.id, crate::vault::Holder::Nobody)
            .await
        {
            Ok(records) => records,
            Err(error) => {
                tracing::warn!(watch = %watch.id, error = %error, "agent secrets could not be read for a watch");
                Vec::new()
            }
        };
        let (mut given, mut missing) = (Vec::new(), Vec::new());
        for name in &watch.secrets {
            match (
                records.iter().find(|r| &r.name == name && !r.cd_prod),
                SecretName::parse(name),
            ) {
                (Some(_), Ok(parsed)) => {
                    plan.env.push((name.clone(), placeholder(&parsed)));
                    given.push(name.clone());
                }
                _ => missing.push(name.as_str()),
            }
        }
        let mut note = if given.is_empty() {
            "[yulius] agent secrets: none".to_owned()
        } else {
            format!("[yulius] agent secrets: {}", given.join(", "))
        };
        if !missing.is_empty() {
            note.push_str(&format!(
                " (not among the project's agent secrets for runs of no person: {})",
                missing.join(", ")
            ));
        }
        let token = (!given.is_empty())
            .then(|| {
                self.ci_egress_open(
                    &sandbox.id,
                    &format!("watch:{}", watch.name),
                    run_id,
                    &given,
                )
            })
            .flatten()
            .map(|token| CiProxy { app: self, token });
        let proxied = token
            .as_ref()
            .is_some_and(|proxy| self.egress_env("yulius-watch", &proxy.token, run_dir, plan));
        if !given.is_empty() && !proxied {
            note.push_str("; the egress proxy is not available, so no secret reaches a host");
        }
        ring.push_str(&format!("{note}\n"));
        bind_run_files(plan);
        token.filter(|_| proxied)
    }

    /// Ends what is left of run `run_id` that has no watcher any more (a restart, WCH-010): its
    /// process host and its directories.
    pub(crate) fn watch_end_lost(&self, sandbox: &yulius_domain::ids::SandboxId, run_id: &str) {
        let run_dir = self.watch_run_dir(run_id);
        if run_dir.exists() {
            let _ = self.deps.session_host.terminate(&run_dir);
        }
        let _ = std::fs::remove_dir_all(&run_dir);
        let _ = std::fs::remove_dir_all(self.watch_base_dir(sandbox).join("runs").join(run_id));
    }
}
