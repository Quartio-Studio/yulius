//! A harness port that plans launches with the real adapter (env, files, secrets, hooks, MCP)
//! and swaps the agent executable for the deterministic fake agent.

use std::path::{Path, PathBuf};

use yulius::wiring::harness::HarnessAdapter;
use yulius_app::error::AppResult;
use yulius_app::ports::harness::{
    HarnessLaunch, HarnessPlan, HarnessPort, OutputMasker, TranscriptWatch,
};
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
}

/// See the module docs.
pub struct FakeAgentHarness {
    pub inner: HarnessAdapter,
    pub fake: PathBuf,
    pub options: FakeAgentOptions,
}

impl FakeAgentHarness {
    /// Fake agent `fake`, hooks calling `yulius_exe`.
    #[must_use]
    pub fn new(yulius_exe: PathBuf, fake: PathBuf, options: FakeAgentOptions) -> Self {
        Self {
            inner: HarnessAdapter::new(yulius_exe, "http://127.0.0.1:9"),
            fake,
            options,
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
        plan.plan.argv = argv;
        Ok(plan)
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
