//! A harness port that plans launches with the real adapter (env, files, secrets, hooks, MCP)
//! and swaps the agent executable for the deterministic fake agent.

use std::path::{Path, PathBuf};

use yulius::wiring::harness::HarnessAdapter;
use yulius_app::error::AppResult;
use yulius_app::ports::harness::{
    CockpitLaunch, HarnessLaunch, HarnessPlan, HarnessPort, OutputMasker, TranscriptWatch,
};
use yulius_app::ports::session::LaunchPlan;
use yulius_domain::harness::Harness;
use yulius_domain::time::Timestamp;

/// Fake-agent behaviour for every launch planned by [`FakeAgentHarness`].
#[derive(Debug, Clone, Default)]
pub struct FakeAgentOptions {
    /// Write a Claude-shaped transcript at the path Yulius watches.
    pub transcript: bool,
    /// Report a usage limit after this many turns.
    pub limit_after: Option<u32>,
    /// Draw full screen like Claude Code (`--tui`), redrawing on every resize.
    pub tui: bool,
    /// Run Claude Code's hooks through `yulius hook` like the real CLI (`--hooks`).
    pub hooks: bool,
    /// Leave a background shell of this many seconds from the goal turn (`--goal-background`).
    pub goal_background: Option<u64>,
    /// The goal turn ends with a background agent pending that finishes after this many seconds
    /// and wakes the session (`--goal-agent`).
    pub goal_agent: Option<u64>,
    /// After the goal turn's stop the agent works on for this many milliseconds with no new prompt
    /// (`--goal-continue-ms`): with `goal_continue_verdict` the goal evaluator says "not met"
    /// (`--goal-continue-verdict`), otherwise another Stop hook continued the turn.
    pub goal_continue_ms: Option<u64>,
    /// See `goal_continue_ms`.
    pub goal_continue_verdict: bool,
    /// Delay the goal turn by this many milliseconds (`--goal-delay-ms`).
    pub goal_delay_ms: Option<u64>,
    /// Lose the first typed lines like a TUI that drops a submit (`--drop-lines`).
    pub drop_lines: Option<u32>,
    /// Handle Ctrl+C and SIGTERM like Claude Code (`--interrupts`).
    pub interrupts: bool,
    /// Run the status line command with the rate limits in this file (`--rate-limits-file`).
    pub rate_limits_file: Option<PathBuf>,
    /// Run Claude Code's SessionStart hook at start, like the real CLI (`--session-start`).
    pub session_start: bool,
    /// Run a turn of this many milliseconds first when the launch has a first message
    /// (`--opening-turn-ms`).
    pub opening_turn_ms: Option<u64>,
    /// Each echoed message is a silent turn of this many milliseconds (`--echo-turn-ms`).
    pub echo_turn_ms: Option<u64>,
    /// Also report these environment variables at start (`--report-env`: set and length, never
    /// the value).
    pub report_env: Vec<String>,
}

/// See the module docs.
pub struct FakeAgentHarness {
    pub inner: HarnessAdapter,
    pub fake: PathBuf,
    pub options: FakeAgentOptions,
    pub yulius_exe: PathBuf,
}

impl FakeAgentHarness {
    /// Fake agent `fake`, hooks calling `yulius_exe`.
    #[must_use]
    pub fn new(yulius_exe: PathBuf, fake: PathBuf, options: FakeAgentOptions) -> Self {
        Self {
            inner: HarnessAdapter::new(yulius_exe.clone(), "http://127.0.0.1:9"),
            fake,
            options,
            yulius_exe,
        }
    }
}

impl HarnessPort for FakeAgentHarness {
    fn plan(&self, launch: &HarnessLaunch) -> AppResult<HarnessPlan> {
        let mut plan = self.inner.plan(launch)?;
        // Tests read what each launch asked for from its run directory.
        let _ = std::fs::write(
            launch.run_dir.join("fake-launch.txt"),
            format!(
                "mode={:?}\nmodel={:?}\neffort={:?}\n",
                launch.mode, launch.model, launch.effort
            ),
        );
        let session = plan
            .session_id
            .clone()
            .unwrap_or_else(|| "fake-session".into());
        let mut argv = vec![
            self.fake.to_string_lossy().into_owned(),
            "--mode".into(),
            "echo".into(),
            "--session-id".into(),
            session.clone(),
            "--report-env".into(),
            "YULIUS_TASK_TOKEN".into(),
        ];
        if self.options.transcript {
            let path =
                self.inner
                    .claude_transcript(&launch.sandbox_home, &launch.worktree, &session);
            argv.extend([
                "--transcript".into(),
                path.to_string_lossy().into_owned(),
                "--transcript-kind".into(),
                "claude".into(),
            ]);
        }
        if let Some(n) = self.options.limit_after {
            argv.extend(["--limit-after".into(), n.to_string()]);
        }
        if self.options.tui {
            argv.push("--tui".into());
        }
        if self.options.hooks {
            argv.extend([
                "--hooks".into(),
                self.yulius_exe.to_string_lossy().into_owned(),
            ]);
        }
        if let Some(seconds) = self.options.goal_background {
            argv.extend(["--goal-background".into(), seconds.to_string()]);
        }
        if let Some(seconds) = self.options.goal_agent {
            argv.extend(["--goal-agent".into(), seconds.to_string()]);
        }
        if let Some(ms) = self.options.goal_continue_ms {
            argv.extend(["--goal-continue-ms".into(), ms.to_string()]);
        }
        if self.options.goal_continue_verdict {
            argv.push("--goal-continue-verdict".into());
        }
        if let Some(ms) = self.options.goal_delay_ms {
            argv.extend(["--goal-delay-ms".into(), ms.to_string()]);
        }
        if let Some(n) = self.options.drop_lines {
            argv.extend(["--drop-lines".into(), n.to_string()]);
        }
        if self.options.interrupts {
            argv.push("--interrupts".into());
        }
        if let Some(file) = &self.options.rate_limits_file {
            argv.extend([
                "--rate-limits-file".into(),
                file.to_string_lossy().into_owned(),
            ]);
        }
        if self.options.session_start {
            argv.push("--session-start".into());
        }
        let first_message = match &launch.mode {
            yulius_app::ports::harness::SessionMode::New { prompt, .. }
            | yulius_app::ports::harness::SessionMode::Resume { prompt, .. } => prompt.is_some(),
        };
        if let Some(ms) = self.options.echo_turn_ms {
            argv.extend(["--echo-turn-ms".into(), ms.to_string()]);
        }
        if let (Some(ms), true) = (self.options.opening_turn_ms, first_message) {
            argv.extend(["--opening-turn-ms".into(), ms.to_string()]);
        }
        for name in &self.options.report_env {
            argv.extend(["--report-env".into(), name.clone()]);
        }
        plan.plan.argv = argv;
        Ok(plan)
    }
    fn plan_cockpit(&self, launch: &CockpitLaunch) -> AppResult<LaunchPlan> {
        self.inner.plan_cockpit(launch)
    }

    fn claude_transcript(&self, home: &Path, worktree: &Path, session: &str) -> PathBuf {
        self.inner.claude_transcript(home, worktree, session)
    }
    fn newest_claude_session(
        &self,
        home: &Path,
        worktree: &Path,
        since: Timestamp,
    ) -> AppResult<Option<(String, PathBuf)>> {
        self.inner.newest_claude_session(home, worktree, since)
    }
    fn watch(
        &self,
        harness: Harness,
        path: &Path,
        from_start: bool,
    ) -> AppResult<Box<dyn TranscriptWatch>> {
        self.inner.watch(harness, path, from_start)
    }
    fn login_argv(&self, harness: Harness) -> Vec<String> {
        self.inner.login_argv(harness)
    }
    fn extract_claude_token(&self, output: &str) -> Option<String> {
        self.inner.extract_claude_token(output)
    }
    fn masker(&self) -> Box<dyn OutputMasker> {
        self.inner.masker()
    }
    fn install_skills(&self, skills: &[(String, PathBuf)], claude: &Path) -> AppResult<()> {
        self.inner.install_skills(skills, claude)
    }
}
