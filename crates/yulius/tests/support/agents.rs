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
    /// After the limit, continue the cut turn by itself like Claude Code at the reset, this many
    /// milliseconds later (`--limit-continues-ms`).
    pub limit_continues_ms: Option<u64>,
    /// The continuation is only the answer ending the turn (`--limit-continues-answer-only`).
    pub limit_continues_answer_only: bool,
    /// Draw full screen like Claude Code (`--tui`), redrawing on every resize.
    pub tui: bool,
    /// Run Claude Code's hooks through `yulius hook` like the real CLI (`--hooks`).
    pub hooks: bool,
    /// Leave a background shell of this many seconds from the goal turn (`--goal-background`).
    pub goal_background: Option<u64>,
    /// The goal turn ends with a background agent pending that finishes after this many seconds
    /// and wakes the session (`--goal-agent`).
    pub goal_agent: Option<u64>,
    /// The goal turn starts a Monitor stream that expires after this many seconds and never
    /// reports (`--goal-monitor`), or ends after half of them (`goal_monitor_ends`).
    pub goal_monitor: Option<u64>,
    /// See `goal_monitor` (`--goal-monitor-ends`).
    pub goal_monitor_ends: bool,
    /// The goal evaluator's last verdict: `met` (default), `failed` or `none`
    /// (`--goal-verdict`).
    pub goal_verdict: Option<&'static str>,
    /// After the goal turn's stop the agent works on for this many milliseconds with no new prompt
    /// (`--goal-continue-ms`): with `goal_continue_verdict` the goal evaluator says "not met"
    /// (`--goal-continue-verdict`), otherwise another Stop hook continued the turn.
    pub goal_continue_ms: Option<u64>,
    /// See `goal_continue_ms`.
    pub goal_continue_verdict: bool,
    /// With the verdict: the continued turn dies (an API error) and closes with no Stop hook
    /// (`--goal-continue-dies`).
    pub goal_continue_dies: bool,
    /// Delay the goal turn by this many milliseconds (`--goal-delay-ms`).
    pub goal_delay_ms: Option<u64>,
    /// Lose the first typed lines like a TUI that drops a submit (`--drop-lines`).
    pub drop_lines: Option<u32>,
    /// Handle Ctrl+C and SIGTERM like Claude Code (`--interrupts`).
    pub interrupts: bool,
    /// Run the status line command with the rate limits in this file (`--rate-limits-file`).
    pub rate_limits_file: Option<PathBuf>,
    /// With `hooks`, leave out Claude Code's SessionStart hook at start, like a CLI too old to
    /// have it. Every supported Claude Code runs it before its prompt box reads, so the fake
    /// does too by default (`--session-start`); without it a goal is typed only after the
    /// hookless fallback, which made every goal test wait 45 s (SPEC-082 TST-001).
    pub no_session_start: bool,
    /// Run a turn of this many milliseconds first when the launch has a first message
    /// (`--opening-turn-ms`).
    pub opening_turn_ms: Option<u64>,
    /// Each echoed message is a silent turn of this many milliseconds (`--echo-turn-ms`).
    pub echo_turn_ms: Option<u64>,
    /// Also report these environment variables at start (`--report-env`: set and length, never
    /// the value).
    pub report_env: Vec<String>,
    /// The context is full after this many echoed messages (`--context-full-after`).
    pub context_full_after: Option<u32>,
    /// `/compact` fails (`--compact-fails`).
    pub compact_fails: bool,
    /// `/compact` never finishes (`--compact-hangs`).
    pub compact_hangs: bool,
    /// The agent answers a launch's first message like a typed one (`--prompt`).
    pub answers_first_message: bool,
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
        // A resume continues the same session (and transcript), like `claude --resume <id>`.
        let resumed = match &launch.mode {
            yulius_app::ports::harness::SessionMode::Resume { session_id, .. } => {
                Some(session_id.clone())
            }
            yulius_app::ports::harness::SessionMode::New { .. } => None,
        };
        let session = plan
            .session_id
            .clone()
            .or(resumed)
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
        if let Some(ms) = self.options.limit_continues_ms {
            argv.extend(["--limit-continues-ms".into(), ms.to_string()]);
        }
        if self.options.limit_continues_answer_only {
            argv.push("--limit-continues-answer-only".into());
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
        if let Some(seconds) = self.options.goal_monitor {
            argv.extend(["--goal-monitor".into(), seconds.to_string()]);
        }
        if self.options.goal_monitor_ends {
            argv.push("--goal-monitor-ends".into());
        }
        if let Some(verdict) = self.options.goal_verdict {
            argv.extend(["--goal-verdict".into(), verdict.into()]);
        }
        if let Some(ms) = self.options.goal_continue_ms {
            argv.extend(["--goal-continue-ms".into(), ms.to_string()]);
        }
        if self.options.goal_continue_verdict {
            argv.push("--goal-continue-verdict".into());
        }
        if self.options.goal_continue_dies {
            argv.push("--goal-continue-dies".into());
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
        if self.options.hooks && !self.options.no_session_start {
            argv.push("--session-start".into());
        }
        // A continued resume (CLAUDE_CODE_RESUME_INTERRUPTED_TURN) opens with the turn Claude
        // Code redoes, like a first message (SPEC-034).
        let first_message = match &launch.mode {
            yulius_app::ports::harness::SessionMode::New { prompt, .. } => prompt.is_some(),
            yulius_app::ports::harness::SessionMode::Resume {
                prompt,
                continue_interrupted,
                ..
            } => prompt.is_some() || *continue_interrupted,
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
        if let Some(n) = self.options.context_full_after {
            argv.extend(["--context-full-after".into(), n.to_string()]);
        }
        if self.options.compact_fails {
            argv.push("--compact-fails".into());
        }
        if self.options.compact_hangs {
            argv.push("--compact-hangs".into());
        }
        if let (
            true,
            yulius_app::ports::harness::SessionMode::New {
                prompt: Some(prompt),
                ..
            },
        ) = (self.options.answers_first_message, &launch.mode)
        {
            argv.extend(["--prompt".into(), prompt.clone()]);
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
    fn step_back_transcript(&self, home: &Path, worktree: &Path, session: &str) -> AppResult<bool> {
        self.inner.step_back_transcript(home, worktree, session)
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

/// What is wrong with a resume after a usage limit (SPEC-034): the launch in `run_dir` must be a
/// native resume without a prompt that asks Claude Code to continue the interrupted turn, and no
/// Claude transcript under `data` may still hold a usage-limit record. Empty when all is right.
#[must_use]
pub fn limit_resume_problems(run_dir: &Path, data: &Path) -> Vec<String> {
    let mut problems = Vec::new();
    match std::fs::read_to_string(run_dir.join("fake-launch.txt")) {
        Ok(launch) => {
            if !launch.contains("Resume {") || !launch.contains("prompt: None") {
                problems.push(format!("a native resume without a prompt: {launch}"));
            }
            if !launch.contains("continue_interrupted: true") {
                problems.push(format!("the interrupted turn continued: {launch}"));
            }
        }
        Err(err) => problems.push(format!("no launch record: {err}")),
    }
    let mut dirs = vec![data.to_path_buf()];
    while let Some(dir) = dirs.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if entry.file_type().is_ok_and(|t| t.is_dir()) {
                dirs.push(path);
            } else if path.extension().is_some_and(|e| e == "jsonl")
                && path.components().any(|c| c.as_os_str() == "projects")
                && std::fs::read_to_string(&path)
                    .is_ok_and(|text| text.contains("usage_limit_reached"))
            {
                problems.push(format!("a limit record left in {}", path.display()));
            }
        }
    }
    problems
}
