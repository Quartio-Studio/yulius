//! The command of a delivery environment run (SPEC-219, #656), run as a CD run of its stage.
//!
//! The sandbox clone and the sandbox's home are the agents' to write, so nothing of a run is read
//! from them: the head of the stage's branch comes from the repository's own origin into a mirror
//! only Yulius writes (`<data>/cd/<sandbox>/environments.git`, one commit deep, every object
//! checked), its tree is written into the run's own directory (`<data>/runs/env-<run>`, with a
//! fresh `HOME`), and the command runs there through the process host in CD's containment: an
//! integration environment as a dev run (the sandbox's containment), a production one as a
//! production run. The directory goes with the run.

use std::path::{Path, PathBuf};

use tokio::sync::watch;
use yulius_domain::cd::CdStage;
use yulius_domain::delivery::Stage;
use yulius_domain::ids::SandboxId;

use super::exec::{Ended, Sink};
use crate::context::App;
use crate::delivery::runner::{CommandEnd, OutputRing};
use crate::error::AppResult;
use crate::model::SandboxRecord;
use crate::ports::session::LaunchPlan;

/// `$1` the log, `$2` the command.
const WRAPPER: &str = r#"exec >>"$1" 2>&1 </dev/null
exec /bin/sh -c "$2""#;

impl App {
    /// The mirror environment runs take their commits into; only Yulius writes it.
    fn environment_mirror(&self, sandbox: &SandboxId) -> PathBuf {
        self.cd_private_dir(sandbox).join("environments.git")
    }

    /// The run's own directory (its tree, `HOME` and log).
    pub(crate) fn environment_run_dir(&self, run_id: &str) -> PathBuf {
        self.config
            .data_dir
            .join("runs")
            .join(format!("env-{run_id}"))
    }

    /// The head of `branch` on the repository's origin (GitHub with the installation token, a
    /// playground's origin by its path), fetched into the environment mirror. The token goes only
    /// into that one command's environment; the sandbox clone is not read.
    ///
    /// # Errors
    /// No installation token, or Git failures.
    pub(crate) async fn environment_head(
        &self,
        sandbox: &SandboxRecord,
        branch: &str,
    ) -> AppResult<String> {
        let mirror = self.environment_mirror(&sandbox.id);
        let git = &self.deps.git;
        if sandbox.is_playground() {
            let origin = self.playground_origin(&sandbox.id);
            return git
                .mirror_fetch_head(&mirror, &origin.to_string_lossy(), branch, None)
                .await;
        }
        let token = self.dlv_installation_token(sandbox).await?;
        git.mirror_fetch_head(&mirror, &self.origin_url(sandbox), branch, Some(&token))
            .await
    }

    /// Runs `command` at `sha` (fetched by [`App::environment_head`]) for run `run_id`, with
    /// `vars` added to its environment, its output into `ring` and the run's record. `Err` says
    /// why it could not run or was lost.
    pub(crate) async fn environment_command(
        &self,
        sandbox: &SandboxRecord,
        stage: Stage,
        (run_id, sha): (&str, &str),
        command: &str,
        vars: Vec<(String, String)>,
        ring: &mut OutputRing,
    ) -> Result<CommandEnd, String> {
        let run_dir = self.environment_run_dir(run_id);
        let ended = self
            .environment_launch(sandbox, stage, (run_id, sha), command, vars, &run_dir, ring)
            .await;
        let _ = std::fs::remove_dir_all(&run_dir);
        ended
    }

    #[allow(clippy::too_many_arguments)] // One run's whole context.
    async fn environment_launch(
        &self,
        sandbox: &SandboxRecord,
        stage: Stage,
        (run_id, sha): (&str, &str),
        command: &str,
        vars: Vec<(String, String)>,
        run_dir: &Path,
        ring: &mut OutputRing,
    ) -> Result<CommandEnd, String> {
        let work = run_dir.join("work");
        let (src, home, cache, log) = (
            work.join("src"),
            work.join("home"),
            work.join("cache"),
            work.join("log"),
        );
        let _ = std::fs::remove_dir_all(run_dir);
        for dir in [&home, &cache] {
            std::fs::create_dir_all(dir)
                .map_err(|e| format!("preparing the run's directories failed: {e}"))?;
        }
        self.deps
            .git
            .export_tree(&self.environment_mirror(&sandbox.id), sha, &src)
            .await
            .map_err(|err| format!("writing {sha}'s files failed: {}", err.public_message()))?;
        std::fs::File::create(&log).map_err(|e| format!("creating the run log failed: {e}"))?;
        let env = match stage {
            Stage::Integration => CdStage::Dev,
            Stage::Production => CdStage::Prod,
        };
        let (containment, _lease) = self
            .cd_containment(sandbox, env, &work, &home, &cache)
            .await?;
        let path = std::env::var("PATH")
            .ok()
            .filter(|p| !p.is_empty())
            .unwrap_or_else(|| "/usr/local/bin:/usr/bin:/bin".into());
        let mut env_vars: Vec<(String, String)> = [
            ("HOME", home.to_string_lossy().into_owned()),
            ("PATH", path),
            ("LANG", "C.UTF-8".to_owned()),
            ("TERM", "dumb".to_owned()),
            ("NO_COLOR", "1".to_owned()),
            ("GIT_TERMINAL_PROMPT", "0".to_owned()),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_owned(), v))
        .collect();
        env_vars.extend(vars);
        let plan = LaunchPlan {
            argv: vec![
                "/bin/sh".into(),
                "-c".into(),
                WRAPPER.into(),
                "yulius-environment".into(),
                log.to_string_lossy().into_owned(),
                command.to_owned(),
            ],
            cwd: src,
            env: env_vars,
            secret_env: Vec::new(),
            files: Vec::new(),
            containment,
        };
        let token = self.deps.crypto.random_bytes();
        let group = format!("env-{run_id}");
        // Nothing cancels an environment run: the sender lives as long as the run.
        let (_never, mut cancel) = watch::channel(None);
        let (ended, _usage) = self
            .ci_watch(
                run_dir,
                (run_id, &group),
                Sink::EnvironmentRun(run_id),
                &plan,
                &token,
                &log,
                (ring, None, None),
                &mut cancel,
            )
            .await?;
        match ended {
            Ended::Exit(exit) if exit.spawn_failed => Ok(CommandEnd::SpawnFailed(
                "the process host could not start it".to_owned(),
            )),
            Ended::Exit(exit) => Ok(match (exit.code, exit.signal) {
                (Some(code), _) => CommandEnd::Exited(code),
                (None, Some(signal)) => CommandEnd::Signaled(signal),
                (None, None) => CommandEnd::SpawnFailed("no exit status was reported".to_owned()),
            }),
            Ended::TimedOut => Ok(CommandEnd::TimedOut),
            Ended::OutputLimit => {
                Err("the command wrote more than 256 MiB of output; it was stopped".to_owned())
            }
            Ended::Lost => Err("the run's process host was lost".to_owned()),
            Ended::Cancelled(reason) | Ended::NoDisk(reason) => Err(reason),
        }
    }

    /// Stops what is left of a run that has no watcher any more (a restart): its process host,
    /// and its directory.
    pub(crate) fn environment_end_lost(&self, run_id: &str) {
        let run_dir = self.environment_run_dir(run_id);
        if run_dir.exists() {
            let _ = self.deps.session_host.terminate(&run_dir);
            let _ = std::fs::remove_dir_all(&run_dir);
        }
    }
}
