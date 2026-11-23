//! A harness port that plans launches with the real adapter (env, files, secrets, hooks, MCP)
//! and swaps the agent executable for the deterministic fake agent.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use yulius::wiring::harness::HarnessAdapter;
use yulius_app::error::AppResult;
use yulius_app::ports::harness::{
    CockpitLaunch, HarnessLaunch, HarnessPlan, HarnessPort, OutputMasker, TranscriptWatch,
    WatchFrom,
};
use yulius_app::ports::session::LaunchPlan;
use yulius_domain::harness::Harness;
use yulius_domain::time::Timestamp;

/// Fake-agent behaviour for every launch planned by [`FakeAgentHarness`].
#[derive(Debug, Clone, Default)]
pub struct FakeAgentOptions {
    /// Counts the Codex CLI installs asked of the harness (`provide_codex(true)`, SPEC-503).
    pub codex_installs: Option<std::sync::Arc<std::sync::atomic::AtomicUsize>>,
    /// Count the Antigravity CLI installs asked for (`provide_antigravity(true)`).
    pub antigravity_installs: Option<std::sync::Arc<std::sync::atomic::AtomicUsize>>,
    /// Write a Claude-shaped transcript at the path Yulius watches.
    pub transcript: bool,
    /// Report a usage limit after this many turns.
    pub limit_after: Option<u32>,
    /// After the limit, continue the cut turn by itself like Claude Code at the reset, this many
    /// milliseconds later (`--limit-continues-ms`).
    pub limit_continues_ms: Option<u64>,
    /// The continuation is only the answer ending the turn (`--limit-continues-answer-only`).
    pub limit_continues_answer_only: bool,
    /// The record after `limit_after` turns is a lapsed subscription's refusal, not a usage
    /// limit (`--access-lost`).
    pub access_lost: bool,
    /// The record after `limit_after` turns is the refusal of a model whose usage credits are
    /// used up, as Claude Code 2.1.311 writes it (`--credits-out`).
    pub credits_out: bool,
    /// That refusal says, as Claude Code 2.1.322's does, that the model's allowance resets this
    /// many seconds ahead (`--credits-reset-s`).
    pub credits_reset_s: Option<u64>,
    /// Draw full screen like Claude Code (`--tui`), redrawing on every resize.
    pub tui: bool,
    /// Run Claude Code's hooks through `yulius hook` like the real CLI (`--hooks`).
    pub hooks: bool,
    /// Leave a background shell of this many seconds from the goal turn (`--goal-background`).
    pub goal_background: Option<u64>,
    /// The goal turn ends with a background agent pending that finishes after this many seconds
    /// and wakes the session (`--goal-agent`).
    pub goal_agent: Option<u64>,
    /// The background agent first stops with background work of its own still running (its
    /// notification says its result may be interim) and notifies its end this many seconds
    /// later (`--goal-agent-interim`).
    pub goal_agent_interim: Option<u64>,
    /// The goal turn starts a Monitor stream that expires after this many seconds and never
    /// reports (`--goal-monitor`), or ends after half of them (`goal_monitor_ends`).
    pub goal_monitor: Option<u64>,
    /// See `goal_monitor` (`--goal-monitor-ends`).
    pub goal_monitor_ends: bool,
    /// The goal evaluator's last verdict: `met` (default), `failed` or `none`
    /// (`--goal-verdict`).
    pub goal_verdict: Option<&'static str>,
    /// With `goal_background`: the goal turn gets no verdict, and the turn the shell's end wakes
    /// records the goal as achieved this many milliseconds after its Stop hook
    /// (`--goal-met-on-wake-ms`).
    pub goal_met_on_wake_ms: Option<u64>,
    /// With `goal_met_on_wake_ms`: the woken turn's Stop hook never reaches Yulius
    /// (`--wake-stop-lost`).
    pub wake_stop_lost: bool,
    /// With `goal_met_on_wake_ms`: the woken turn's prompt hook never reaches Yulius
    /// (`--wake-prompt-lost`).
    pub wake_prompt_lost: bool,
    /// With `goal_background`: Claude Code wakes the session this many milliseconds after the
    /// shell exits, as it notices the end only when it polls (`--wake-delay-ms`).
    pub wake_delay_ms: Option<u64>,
    /// After the goal turn's stop the agent works on for this many milliseconds with no new prompt
    /// (`--goal-continue-ms`): with `goal_continue_verdict` the goal evaluator says "not met"
    /// (`--goal-continue-verdict`), otherwise another Stop hook continued the turn.
    pub goal_continue_ms: Option<u64>,
    /// See `goal_continue_ms`.
    pub goal_continue_verdict: bool,
    /// With the verdict: the continued turn dies (an API error) and closes with no Stop hook
    /// (`--goal-continue-dies`).
    pub goal_continue_dies: bool,
    /// The goal turn and the goal retries Claude Code makes after it die on an API error, this
    /// many turns in all (`--goal-api-errors`); more than three spends the retries.
    pub goal_api_errors: Option<u32>,
    /// Milliseconds before each goal retry (`--goal-retry-ms`).
    pub goal_retry_ms: Option<u64>,
    /// With `goal_api_errors`: each failed turn ends cleanly and its goal check fails
    /// (`--goal-check-errors`).
    pub goal_check_errors: bool,
    /// With `goal_api_errors`: the goal is cleared while the first retry is armed
    /// (`--goal-cleared-on-retry`).
    pub goal_cleared_on_retry: bool,
    /// With `goal_api_errors`: only a new session meets them. A session Yulius resumes works, as
    /// when the API answers again by then.
    pub api_recovers_on_resume: bool,
    /// A session Yulius resumes starts with the line Claude Code writes itself when it resumes
    /// with nothing to answer, "No response requested." (`--no-response-line`).
    pub no_response_line: bool,
    /// The session whose goal was paused is woken once this file exists, as Claude Code wakes an
    /// idle session when background work ends, and its turn works (`--paused-wake-file`).
    pub paused_wake_file: Option<PathBuf>,
    /// With `paused_wake_file`: the API refuses the woken turn again (`--paused-wake-dies`).
    pub paused_wake_dies: bool,
    /// With `paused_wake_file`: the session goes on with no prompt anyone sees, a hidden
    /// continuation with no prompt hook (`--paused-wake-hidden`).
    pub paused_wake_hidden: bool,
    /// With `goal_api_errors`: the notice that pauses the goal is written only once this file
    /// exists (`--pause-hold-file`).
    pub pause_hold_file: Option<PathBuf>,
    /// Delay the goal turn by this many milliseconds (`--goal-delay-ms`).
    pub goal_delay_ms: Option<u64>,
    /// The idle session compacts itself this many milliseconds after the goal turn, like Claude
    /// Code before its prompt cache expires (`--idle-compact-ms`).
    pub idle_compact_ms: Option<u64>,
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
    /// The agent never takes a `/goal` launch prompt, like a Claude Code that refuses the
    /// condition (SPEC-117): it is not passed on.
    pub ignores_goal_prompt: bool,
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
    /// The prompt of any resume reaches the agent, as `claude --resume <id> <prompt>` gives it
    /// (`--prompt`); without it only a prompt that resumes workflows does.
    pub resume_prompts: bool,
    /// Open on Claude Code's folder-trust dialog, answered by the first line (`--trust-prompt`).
    pub trust_prompt: bool,
    /// The transcript's records name the Claude Code version read from this file at each start
    /// (`--cli-version-file`): rewriting it stands for a newer Claude Code at the next start.
    pub cli_version_file: Option<PathBuf>,
    /// A workflow the agent launches (`/workflow`) has its second agent refused by a usage
    /// limit, written to that agent's transcript only (`--workflow-limit`, SPEC-486).
    pub workflow_limit: bool,
    /// That refusal is of the usage credits of the model the agent runs: this model, or the
    /// session's own for `session` (`--workflow-limit-model`).
    pub workflow_limit_model: Option<String>,
    /// Planning a launch fails while this file exists: the run cannot be started at all (a
    /// harness, MCP or isolation error), after a switch's preflight passed.
    pub fail_launch_file: Option<PathBuf>,
    /// While this file exists the agent exits at once when it starts (`--exit-at-start-file`),
    /// like a release that cannot start or cannot resume a session.
    pub exit_at_start_file: Option<PathBuf>,
    /// While this file exists the agent stays at its start without starting its session
    /// (`--stuck-at-start-file`), like a release that opens on a dialog nobody answers.
    pub stuck_at_start_file: Option<PathBuf>,
    /// A goal turn or an echoed turn ends only once this file exists (`--turn-hold-file`).
    pub turn_hold_file: Option<PathBuf>,
    /// An echoed turn takes the lines entered while it runs as Claude Code takes prompts
    /// entered during a turn (`--queue-prompts`, SPEC-471): the prompt hook at the Enter, an
    /// `enqueue` record, nothing released by a blocked Stop, and one turn for all that wait
    /// after the Stop that ends the turn.
    pub queue_prompts: bool,
    /// A Stop a hook blocks continues with a tool call and its result before the answer
    /// (`--stop-tool`, SPEC-493 DEF-001).
    pub stop_tool: bool,
    /// A waiting line's `enqueue` record is written only once this file exists
    /// (`--queue-record-file`).
    pub queue_record_file: Option<PathBuf>,
    /// The queue is handed on only once this file exists (`--queue-handover-file`).
    pub queue_handover_file: Option<PathBuf>,
    /// The handed-on turn starts only once this file exists (`--queue-turn-file`).
    pub queue_turn_file: Option<PathBuf>,
    /// The agent writes a meta record, no prompt, with its version at each start, as a real
    /// Claude Code may before any prompt when it resumes an interrupted turn
    /// (`--resume-record`).
    pub resume_record: bool,
    /// Simulate Claude skipping goal restoration because workspace trust or hook policy forbids it.
    pub forget_goal: bool,
    /// The Codex CLI the harness says it provides (SPEC-503 CLI-002); its program is recorded in
    /// `fake-launch.txt`, the fake agent runs instead.
    pub provided_codex: Option<yulius_app::ports::harness::ProvidedCli>,
    /// The Antigravity CLI the harness provides (SPEC-505 AGY-007); `None`: the host's `agy`.
    pub provided_antigravity: Option<yulius_app::ports::harness::ProvidedCli>,
    /// The Codex CLI cannot be provided, for this reason (SPEC-503 CLI-002).
    pub codex_unavailable: Option<String>,
    /// Each answer about the Codex CLI waits for a permit of this gate (a test watches the task
    /// before its start goes on).
    pub codex_gate: Option<std::sync::Arc<tokio::sync::Semaphore>>,
    /// The same for the Antigravity CLI.
    pub antigravity_gate: Option<std::sync::Arc<tokio::sync::Semaphore>>,
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
    fn provide_codex(&self, install: bool) -> yulius_app::ports::harness::ProvideFuture<'_> {
        if let (true, Some(count)) = (install, &self.options.codex_installs) {
            count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        }
        let provided = self.options.provided_codex.clone();
        let unavailable = self.options.codex_unavailable.clone();
        let gate = self.options.codex_gate.clone();
        Box::pin(async move {
            if let Some(gate) = gate {
                let _permit = gate.acquire_owned().await;
            }
            match unavailable {
                Some(reason) => Err(yulius_app::error::AppError::Upstream(reason)),
                None => Ok(provided),
            }
        })
    }

    fn provide_antigravity(&self, install: bool) -> yulius_app::ports::harness::ProvideFuture<'_> {
        if let (true, Some(count)) = (install, &self.options.antigravity_installs) {
            count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        }
        let provided = self.options.provided_antigravity.clone();
        let gate = self.options.antigravity_gate.clone();
        Box::pin(async move {
            if let Some(gate) = gate {
                let _permit = gate.acquire_owned().await;
            }
            Ok(provided)
        })
    }

    fn plan(&self, launch: &HarnessLaunch) -> AppResult<HarnessPlan> {
        if self
            .options
            .fail_launch_file
            .as_ref()
            .is_some_and(|file| file.exists())
        {
            return Err(yulius_app::error::AppError::Conflict(
                "the agent CLI did not start".into(),
            ));
        }
        let mut plan = self.inner.plan(launch)?;
        // Record the request and the actual Codex settings before replacing the CLI with the fake.
        let mut settings = format!(
            "mode={:?}\nmodel={:?}\neffort={:?}\nprogram={:?}\n",
            launch.mode,
            launch.model,
            launch.effort,
            plan.plan.argv.first()
        );
        if launch.harness == Harness::Codex {
            for args in plan.plan.argv.windows(2).take_while(|args| args[0] != "--") {
                if args[0] == "-m"
                    || (args[0] == "-c" && args[1].starts_with("model_reasoning_effort="))
                {
                    let _ = writeln!(settings, "{} {}", args[0], args[1]);
                }
            }
        }
        let _ = std::fs::write(launch.run_dir.join("fake-launch.txt"), settings);
        // A resume continues the same session (and transcript), like `claude --resume <id>`.
        // A fork (SPEC-628) starts a new session holding a copy of the one it names.
        let (resumed, forked) = match &launch.mode {
            yulius_app::ports::harness::SessionMode::Resume {
                session_id,
                fork: Some(_),
                ..
            } => (None, Some(session_id.clone())),
            yulius_app::ports::harness::SessionMode::Resume { session_id, .. } => {
                (Some(session_id.clone()), None)
            }
            yulius_app::ports::harness::SessionMode::New { .. } => (None, None),
        };
        let resumed_session = resumed.is_some();
        let codex = launch.harness == Harness::Codex;
        let antigravity = launch.harness == Harness::Antigravity;
        // Codex names its own session: a new fake one takes the run directory's name (the
        // attempt id), which Yulius learns from the rollout's `session_meta` or the hook.
        let session = plan.session_id.clone().or(resumed).unwrap_or_else(|| {
            if codex || antigravity {
                launch
                    .run_dir
                    .file_name()
                    .map_or_else(|| "fake-codex".into(), |n| n.to_string_lossy().into_owned())
            } else {
                "fake-session".into()
            }
        });
        let mut argv = vec![
            self.fake.to_string_lossy().into_owned(),
            "--mode".into(),
            "echo".into(),
            "--session-id".into(),
            session.clone(),
            "--report-env".into(),
            "YULIUS_TASK_TOKEN".into(),
        ];
        if let (true, Some(home)) = (codex, &launch.codex_home) {
            // The rollout lives in the sandbox's shared sessions, reached through the home's
            // `sessions` link as Codex reaches it (SPEC-503 CDX-003).
            let sessions = home.join("sessions");
            let path = match self.inner.find_codex_rollout(&sessions, &session) {
                Ok(Some(existing)) => existing,
                _ => sessions
                    .join("2026/11/09")
                    .join(format!("rollout-2026-11-09T00-00-00-{session}.jsonl")),
            };
            if self.options.transcript {
                argv.extend(["--transcript".into(), path.to_string_lossy().into_owned()]);
            }
            argv.extend(["--transcript-kind".into(), "codex".into()]);
        } else if let (true, Some(home)) = (antigravity, &launch.antigravity_home) {
            // The conversation lives in the sandbox's shared brain, reached through the home's
            // `brain` link as the CLI reaches it (SPEC-505 AGY-003).
            let path = yulius_harness::paths::antigravity_transcript_path(
                &home.join(".gemini/antigravity-cli/brain"),
                &session,
            );
            if self.options.transcript {
                argv.extend(["--transcript".into(), path.to_string_lossy().into_owned()]);
            }
            argv.extend(["--transcript-kind".into(), "antigravity".into()]);
        } else if self.options.transcript {
            let path =
                self.inner
                    .claude_transcript(&launch.sandbox_home, &launch.worktree, &session);
            argv.extend([
                "--transcript".into(),
                path.to_string_lossy().into_owned(),
                "--transcript-kind".into(),
                "claude".into(),
            ]);
            // Claude Code reads the forked session where `--resume` finds it: the fork
            // worktree's project folder.
            if let Some(source) = &forked {
                let from =
                    self.inner
                        .claude_transcript(&launch.sandbox_home, &launch.worktree, source);
                argv.extend(["--fork-from".into(), from.to_string_lossy().into_owned()]);
            }
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
        if self.options.access_lost {
            argv.push("--access-lost".into());
        }
        if self.options.credits_out {
            argv.push("--credits-out".into());
        }
        if let Some(seconds) = self.options.credits_reset_s {
            argv.extend(["--credits-reset-s".into(), seconds.to_string()]);
        }
        if self.options.tui {
            argv.push("--tui".into());
        }
        for (flag, path) in [
            ("--cli-version-file", &self.options.cli_version_file),
            ("--exit-at-start-file", &self.options.exit_at_start_file),
            ("--stuck-at-start-file", &self.options.stuck_at_start_file),
            ("--turn-hold-file", &self.options.turn_hold_file),
            ("--queue-record-file", &self.options.queue_record_file),
            ("--queue-handover-file", &self.options.queue_handover_file),
            ("--queue-turn-file", &self.options.queue_turn_file),
        ] {
            if let Some(path) = path {
                argv.extend([flag.into(), path.to_string_lossy().into_owned()]);
            }
        }
        if self.options.stop_tool {
            argv.push("--stop-tool".into());
        }
        if self.options.queue_prompts {
            argv.push("--queue-prompts".into());
        }
        if resumed_session && !codex && !antigravity {
            argv.push("--resume".into());
            if self.options.forget_goal {
                argv.push("--forget-goal".into());
            }
        }
        if self.options.resume_record {
            argv.push("--resume-record".into());
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
        if let Some(seconds) = self.options.goal_agent_interim {
            argv.extend(["--goal-agent-interim".into(), seconds.to_string()]);
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
        if let Some(ms) = self.options.goal_met_on_wake_ms {
            argv.extend(["--goal-met-on-wake-ms".into(), ms.to_string()]);
        }
        if self.options.wake_stop_lost {
            argv.push("--wake-stop-lost".into());
        }
        if self.options.wake_prompt_lost {
            argv.push("--wake-prompt-lost".into());
        }
        if let Some(ms) = self.options.wake_delay_ms {
            argv.extend(["--wake-delay-ms".into(), ms.to_string()]);
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
        let api_errors = self
            .options
            .goal_api_errors
            .filter(|_| !(self.options.api_recovers_on_resume && resumed_session));
        if let Some(n) = api_errors {
            argv.extend(["--goal-api-errors".into(), n.to_string()]);
        }
        if self.options.no_response_line && resumed_session {
            argv.push("--no-response-line".into());
        }
        if let Some(file) = &self.options.paused_wake_file {
            argv.extend([
                "--paused-wake-file".into(),
                file.to_string_lossy().into_owned(),
            ]);
        }
        if self.options.paused_wake_dies {
            argv.push("--paused-wake-dies".into());
        }
        if self.options.paused_wake_hidden {
            argv.push("--paused-wake-hidden".into());
        }
        if let Some(file) = &self.options.pause_hold_file {
            argv.extend([
                "--pause-hold-file".into(),
                file.to_string_lossy().into_owned(),
            ]);
        }
        if let Some(ms) = self.options.goal_retry_ms {
            argv.extend(["--goal-retry-ms".into(), ms.to_string()]);
        }
        if self.options.goal_check_errors {
            argv.push("--goal-check-errors".into());
        }
        if self.options.goal_cleared_on_retry {
            argv.push("--goal-cleared-on-retry".into());
        }
        if let Some(ms) = self.options.goal_delay_ms {
            argv.extend(["--goal-delay-ms".into(), ms.to_string()]);
        }
        if let Some(ms) = self.options.idle_compact_ms {
            argv.extend(["--idle-compact-ms".into(), ms.to_string()]);
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
        if self.options.trust_prompt {
            argv.push("--trust-prompt".into());
        }
        if self.options.workflow_limit {
            argv.push("--workflow-limit".into());
        }
        if let Some(model) = &self.options.workflow_limit_model {
            argv.extend(["--workflow-limit-model".into(), model.clone()]);
        }
        // The prompt of a resume that tells the agent to resume its workflows reaches it, as
        // `claude --resume <id> <prompt>` gives it (SPEC-486 WFL-004).
        if let yulius_app::ports::harness::SessionMode::Resume {
            prompt: Some(prompt),
            ..
        } = &launch.mode
            && (self.options.resume_prompts
                || prompt.contains("resumeFromRunId")
                || forked.is_some())
        {
            argv.extend(["--prompt".into(), prompt.clone()]);
        }
        // The Antigravity CLI runs no SessionStart for a resumed conversation (SPEC-505).
        if self.options.hooks && !self.options.no_session_start && !(antigravity && resumed_session)
        {
            argv.push("--session-start".into());
        }
        // A continued resume (CLAUDE_CODE_RESUME_INTERRUPTED_TURN) opens with the turn Claude
        // Code redoes, like a first message (SPEC-034).
        // A `/goal` launch prompt sets the goal and runs the goal's own turn, as Claude Code
        // does (SPEC-117): no opening turn before it.
        let goal_prompt = matches!(
            &launch.mode,
            yulius_app::ports::harness::SessionMode::New { prompt: Some(prompt), .. }
                if prompt.starts_with("/goal ")
        );
        let first_message = match &launch.mode {
            yulius_app::ports::harness::SessionMode::New { prompt, .. } => {
                prompt.is_some() && !goal_prompt
            }
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
        ) = (
            self.options.answers_first_message || goal_prompt,
            &launch.mode,
        ) && !(goal_prompt && self.options.ignores_goal_prompt)
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
    fn last_claude_goal(
        &self,
        anchor: &Path,
        path: &Path,
    ) -> AppResult<Option<yulius_domain::task::GoalOutcome>> {
        self.inner.last_claude_goal(anchor, path)
    }

    fn submit_origin(
        &self,
        file: std::fs::File,
        session: &str,
        id: &str,
        prompt: &str,
        run_to_submit: std::ops::RangeInclusive<yulius_domain::time::Timestamp>,
    ) -> yulius_domain::harness::SubmitOrigin {
        self.inner
            .submit_origin(file, session, id, prompt, run_to_submit)
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
        from: WatchFrom,
    ) -> AppResult<Box<dyn TranscriptWatch>> {
        self.inner.watch(harness, path, from)
    }
    fn recount(
        &self,
        harness: Harness,
        sandbox_home: &Path,
        worktree: &Path,
        session_id: &str,
        from: WatchFrom,
        until: Option<yulius_domain::time::Timestamp>,
    ) -> AppResult<Option<yulius_app::ports::harness::UsageSnapshot>> {
        self.inner
            .recount(harness, sandbox_home, worktree, session_id, from, until)
    }
    fn discover_antigravity_session(
        &self,
        gemini_dir: &Path,
        brain: &Path,
        worktree: &Path,
        since: Timestamp,
    ) -> Option<String> {
        self.inner
            .discover_antigravity_session(gemini_dir, brain, worktree, since)
    }
    fn antigravity_api_key_credential(&self, key: &str) -> Option<Vec<u8>> {
        self.inner.antigravity_api_key_credential(key)
    }
    fn antigravity_sign_in_credential(&self, token_file: &[u8]) -> Option<Vec<u8>> {
        self.inner.antigravity_sign_in_credential(token_file)
    }
    fn antigravity_refreshed_credential(&self, fresh: &[u8], stored: &[u8]) -> Option<Vec<u8>> {
        self.inner.antigravity_refreshed_credential(fresh, stored)
    }
    fn antigravity_transcript(&self, brain: &Path, session: &str) -> PathBuf {
        self.inner.antigravity_transcript(brain, session)
    }
    fn find_codex_rollout(&self, dir: &Path, session: &str) -> AppResult<Option<PathBuf>> {
        self.inner.find_codex_rollout(dir, session)
    }
    fn discover_codex_session(
        &self,
        dir: &Path,
        worktree: &Path,
        since: Timestamp,
    ) -> AppResult<Option<(String, PathBuf)>> {
        self.inner.discover_codex_session(dir, worktree, since)
    }
    fn codex_auth_problem(&self, auth_json: &[u8]) -> Option<String> {
        self.inner.codex_auth_problem(auth_json)
    }
    fn codex_import_problem(&self, auth_json: &[u8]) -> Option<String> {
        self.inner.codex_import_problem(auth_json)
    }
    fn codex_api_key_auth(&self, api_key: &str) -> Option<Vec<u8>> {
        self.inner.codex_api_key_auth(api_key)
    }
    fn codex_auth_replaces(&self, fresh: &[u8], stored: &[u8], now: Timestamp) -> bool {
        self.inner.codex_auth_replaces(fresh, stored, now)
    }
    fn codex_login_config(&self) -> Vec<u8> {
        self.inner.codex_login_config()
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
    fn install_skills(
        &self,
        skills: &[(String, PathBuf)],
        root: &Path,
        skills_dir: &Path,
    ) -> AppResult<()> {
        self.inner.install_skills(skills, root, skills_dir)
    }
    fn refresh_skills(
        &self,
        skills: &[(String, PathBuf)],
        root: &Path,
        skills_dir: &Path,
    ) -> AppResult<yulius_app::ports::harness::SkillsRefresh> {
        self.inner.refresh_skills(skills, root, skills_dir)
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
                && std::fs::read_to_string(&path).is_ok_and(|text| {
                    text.contains("usage_limit_reached")
                        || text.contains("model_requires_usage_credits")
                })
            {
                problems.push(format!("a limit record left in {}", path.display()));
            }
        }
    }
    problems
}
