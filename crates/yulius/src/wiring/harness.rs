//! [`HarnessPort`] over `yulius-harness` (Claude Code 2.1.292 structures).

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use yulius_app::error::{AppError, AppResult};
use yulius_app::ports::harness::{
    CockpitLaunch, HarnessLaunch, HarnessPlan, HarnessPort, McpAttachment, Observation,
    OutputMasker, SessionMode, TranscriptWatch, UsageSnapshot, WatchFrom,
};
use yulius_app::ports::session::{Containment, LaunchPlan, PlannedFile};
use yulius_domain::harness::Harness;
use yulius_domain::time::Timestamp;
use yulius_harness::launch::{
    self, Credential, LaunchMode, LaunchRequest, McpServerSpec, McpTransport,
};
use yulius_harness::observe::{Observed, UsageAccumulator};
use yulius_harness::secret::Secret as HSecret;
use yulius_harness::subagents::{LinesSince, SubagentTails};
use yulius_harness::{claude, login, paths, recount, skills, tail};

/// Harness adapter.
pub struct HarnessAdapter {
    /// Path of the `yulius` executable used by hooks.
    pub yulius_exe: PathBuf,
    /// Internal listener URL.
    pub yulius_url: String,
}

impl HarnessAdapter {
    /// Adapter for hooks calling `yulius_exe` and MCP at `yulius_url`.
    #[must_use]
    pub fn new(yulius_exe: PathBuf, yulius_url: impl Into<String>) -> Self {
        Self {
            yulius_exe,
            yulius_url: yulius_url.into(),
        }
    }
}

fn to_system(ts: Timestamp) -> SystemTime {
    UNIX_EPOCH + Duration::from_millis(u64::try_from(ts.0.max(0)).unwrap_or(0))
}

fn io(err: std::io::Error) -> AppError {
    AppError::internal(err)
}

/// Longest MCP variable name before its numbered suffix: the harness takes names of at most 128
/// characters, and a server name (64) and a variable (128) could make one of 204.
const MCP_VAR_BASE_LEN: usize = 120;

/// The run's MCP servers and the secret environment holding their values. Each value travels in a
/// variable of its own, `YULIUS_MCP_<SERVER>_<VAR>` (`…_TOKEN` for a bearer), never under the
/// server's own name: that name may be an agent secret's placeholder in the agent's environment, or
/// another server's variable (SPEC-095). Agent secret names cannot start with `YULIUS_`. A name is
/// cut to [`MCP_VAR_BASE_LEN`] characters, and names that still meet get a numbered suffix.
fn mcp_specs(mcp: &[McpAttachment]) -> (Vec<McpServerSpec>, Vec<(String, String)>) {
    let mut secrets: Vec<(String, String)> = Vec::new();
    let mut variable = |server: &str, key: &str, value: &str| {
        let mut base = format!("YULIUS_MCP_{server}_{key}")
            .to_ascii_uppercase()
            .replace('-', "_");
        base.truncate(MCP_VAR_BASE_LEN);
        let mut var = base.clone();
        let mut n = 2;
        while secrets.iter().any(|(k, _)| *k == var) {
            var = format!("{base}_{n}");
            n += 1;
        }
        secrets.push((var.clone(), value.to_owned()));
        var
    };
    let servers = mcp
        .iter()
        .map(|m| match m {
            McpAttachment::Stdio {
                name,
                command,
                args,
                env,
            } => McpServerSpec {
                name: name.clone(),
                transport: McpTransport::Stdio {
                    command: command.clone(),
                    args: args.clone(),
                    env_vars: env
                        .iter()
                        .map(|(key, value)| (key.clone(), variable(name, key, value)))
                        .collect(),
                },
            },
            McpAttachment::Http { name, url, bearer } => McpServerSpec {
                name: name.clone(),
                transport: McpTransport::Http {
                    url: url.clone(),
                    bearer_env: bearer.as_ref().map(|b| variable(name, "TOKEN", b)),
                },
            },
        })
        .collect();
    (servers, secrets)
}

impl HarnessPort for HarnessAdapter {
    fn plan(&self, l: &HarnessLaunch) -> AppResult<HarnessPlan> {
        let (mcp_servers, extra_secrets) = mcp_specs(&l.mcp);
        let credential = match l.harness {
            Harness::ClaudeCode => {
                Credential::ClaudeOauthToken(HSecret::new(l.credential.expose().to_owned()))
            }
        };
        let mode = match &l.mode {
            SessionMode::New { session_id, prompt } => LaunchMode::New {
                session_id: session_id.clone(),
                prompt: prompt.clone(),
            },
            SessionMode::Resume {
                session_id,
                prompt,
                continue_interrupted,
            } => LaunchMode::Resume {
                session_id: session_id.clone(),
                prompt: prompt.clone(),
                continue_interrupted: *continue_interrupted,
            },
        };
        let request = LaunchRequest {
            harness: l.harness,
            mode,
            worktree: l.worktree.clone(),
            sandbox_home: l.sandbox_home.clone(),
            run_dir: l.run_dir.clone(),
            model: l.model.clone(),
            effort: l.effort.clone(),
            contained: l.contained,
            yulius_exe: self.yulius_exe.clone(),
            yulius_url: self.yulius_url.clone(),
            mcp_servers,
            credential,
        };
        let plan = launch::plan_launch(&request).map_err(|e| AppError::Invalid(e.to_string()))?;
        if l.harness == Harness::ClaudeCode {
            yulius_harness::claude_config::ensure_claude_config(
                &l.sandbox_home.join(".claude"),
                &l.worktree,
                l.contained,
            )
            .map_err(|e| {
                AppError::Upstream(format!("cannot prepare the Claude Code config: {e}"))
            })?;
        }
        let mut secret_env = plan.secret_env;
        secret_env.extend(extra_secrets);
        Ok(HarnessPlan {
            plan: LaunchPlan {
                argv: plan.argv,
                cwd: plan.cwd,
                env: plan.env,
                secret_env,
                files: plan
                    .files
                    .into_iter()
                    .map(|f| PlannedFile {
                        path: f.path,
                        contents: f.contents,
                        mode: f.mode,
                    })
                    .collect(),
                containment: Containment::None,
            },
            session_id: plan.session_id,
        })
    }

    fn plan_cockpit(&self, l: &CockpitLaunch) -> AppResult<LaunchPlan> {
        let (mcp_servers, mcp_secrets) = mcp_specs(&l.mcp);
        let plan = launch::plan_cockpit(&launch::CockpitRequest {
            cwd: l.cwd.clone(),
            config_dir: l.config_dir.clone(),
            resume: l.resume.clone(),
            continue_interrupted: l.continue_interrupted,
            credential: Credential::ClaudeOauthToken(HSecret::new(
                l.credential.expose().to_owned(),
            )),
            yulius: l.yulius.as_ref().map(|(url, token)| launch::CockpitYulius {
                url: url.clone(),
                token: HSecret::new(token.expose().to_owned()),
            }),
            project_mcp_servers: l.project_mcp_servers.clone(),
            mcp_servers,
            redaction_exe: Some(self.yulius_exe.clone()),
            bypass_permissions: l.contained,
        })
        .map_err(|e| AppError::Invalid(e.to_string()))?;
        // Contained, it bypasses approvals as a task run does, its warning accepted (SPEC-208).
        yulius_harness::claude_config::ensure_claude_config(
            &l.config_dir_host,
            &l.cwd,
            l.contained,
        )
        .map_err(|e| AppError::Upstream(format!("cannot prepare the Claude Code config: {e}")))?;
        // Moved past a usage limit, the conversation steps back over it first (SPEC-074 SUP-004,
        // as a task's does, SPEC-034); a failed step back still resumes.
        if let (Some(session), true) = (&l.resume, l.continue_interrupted) {
            let transcript = paths::claude_transcript_path(&l.config_dir_host, &l.cwd, session);
            match yulius_harness::step_back::step_back_file(&transcript) {
                Ok(changed) => {
                    tracing::info!(
                        changed,
                        "cockpit transcript stepped back over a usage limit"
                    )
                }
                Err(err) => {
                    tracing::warn!(error = %err, "could not step the cockpit transcript back")
                }
            }
        }
        let mut secret_env = plan.secret_env;
        secret_env.extend(mcp_secrets);
        Ok(LaunchPlan {
            argv: plan.argv,
            cwd: plan.cwd,
            env: plan.env,
            secret_env,
            files: Vec::new(),
            containment: Containment::None,
        })
    }

    fn claude_transcript(&self, sandbox_home: &Path, worktree: &Path, session_id: &str) -> PathBuf {
        paths::claude_transcript_path(&sandbox_home.join(".claude"), worktree, session_id)
    }

    fn step_back_transcript(
        &self,
        sandbox_home: &Path,
        worktree: &Path,
        session_id: &str,
    ) -> AppResult<bool> {
        let transcript = self.claude_transcript(sandbox_home, worktree, session_id);
        yulius_harness::step_back::step_back_file(&transcript).map_err(io)
    }

    fn newest_claude_session(
        &self,
        sandbox_home: &Path,
        worktree: &Path,
        since: Timestamp,
    ) -> AppResult<Option<(String, PathBuf)>> {
        let dir = paths::claude_project_dir(&sandbox_home.join(".claude"), worktree);
        Ok(paths::newest_claude_session(&dir, to_system(since))
            .map_err(io)?
            .map(|s| (s.session_id, s.path)))
    }

    fn watch(
        &self,
        harness: Harness,
        path: &Path,
        from: WatchFrom,
    ) -> AppResult<Box<dyn TranscriptWatch>> {
        // Both read the file from its start; a resumed session's run skips its earlier runs' lines.
        let since = match from {
            WatchFrom::Start => None,
            WatchFrom::Since(at) => Some(at),
        };
        Ok(Box::new(Watch {
            harness,
            tail: tail::TranscriptTail::open(path, tail::DEFAULT_MAX_LINE_BYTES).map_err(io)?,
            since: LinesSince::new(since),
            subagents: SubagentTails::new(path, since),
            acc: UsageAccumulator::new(),
        }))
    }

    fn recount(
        &self,
        harness: Harness,
        sandbox_home: &Path,
        worktree: &Path,
        session_id: &str,
        from: WatchFrom,
        until: Option<Timestamp>,
    ) -> AppResult<Option<UsageSnapshot>> {
        // The session id came from the agent's hook: one that is not an id (`../x`, `/x`, which
        // `join` would take as the whole path) names no transcript.
        if !paths::valid_session_id(session_id) {
            return Ok(None);
        }
        let path = self.claude_transcript(sandbox_home, worktree, session_id);
        let since = match from {
            WatchFrom::Start => None,
            WatchFrom::Since(at) => Some(at),
        };
        let counted = match harness {
            Harness::ClaudeCode => recount::count_run(&path, since, until).map_err(io)?,
        };
        Ok(counted.map(|acc| snapshot(&acc)))
    }

    fn login_argv(&self, harness: Harness) -> Vec<String> {
        match harness {
            Harness::ClaudeCode => login::claude_setup_token_argv(),
        }
    }

    fn extract_claude_token(&self, output: &str) -> Option<String> {
        login::extract_claude_oauth_token(output)
    }

    fn masker(&self) -> Box<dyn OutputMasker> {
        Box::new(Masker(login::TokenMasker::new()))
    }

    fn install_skills(
        &self,
        skill_dirs: &[(String, PathBuf)],
        root: &Path,
        skills_dir: &Path,
    ) -> AppResult<()> {
        let dirs: Vec<skills::SkillDir> = skill_dirs
            .iter()
            .map(|(name, path)| skills::SkillDir {
                name: name.clone(),
                path: path.clone(),
            })
            .collect();
        skills::install_skills(&dirs, root, skills_dir)
            .map(|_| ())
            .map_err(AppError::internal)
    }
}

/// A run's figures as the transcript's lines give them (SPEC-087).
fn snapshot(acc: &UsageAccumulator) -> UsageSnapshot {
    UsageSnapshot {
        tokens: acc.usage(),
        model: acc.model().map(str::to_owned),
        turns: acc.turns(),
        active_ms: acc.active().total_ms(),
        turn_open_since: acc.active().open_since(),
    }
}

struct Masker(login::TokenMasker);

impl OutputMasker for Masker {
    fn push(&mut self, chunk: &[u8]) -> Vec<u8> {
        self.0.push(chunk)
    }
    fn finish(&mut self) -> Vec<u8> {
        self.0.finish()
    }
}

struct Watch {
    harness: Harness,
    tail: tail::TranscriptTail,
    /// The run's own lines of a resumed session's transcript (SPEC-087 MET-002).
    since: LinesSince,
    /// Its sub-agents' transcripts: their tokens are the session's too (MET-004).
    subagents: SubagentTails,
    acc: UsageAccumulator,
}

impl TranscriptWatch for Watch {
    fn poll(&mut self) -> AppResult<Vec<Observation>> {
        let lines = self.tail.poll().map_err(io)?;
        // Only usage counts from a sub-agent's lines; one that cannot be read leaves the
        // session's own observations alone.
        match self.subagents.poll() {
            Ok(side) => {
                for line in side {
                    for observed in claude::observe_line(&line) {
                        self.acc.observe_side(&observed);
                    }
                }
            }
            Err(err) => tracing::debug!(error = %err, "a sub-agent transcript could not be read"),
        }
        let mut out = Vec::new();
        for line in lines {
            if !self.since.keep(&line) {
                continue;
            }
            let observed = match self.harness {
                Harness::ClaudeCode => claude::observe_line(&line),
            };
            self.acc.observe_all(observed.iter());
            for o in observed {
                match o {
                    Observed::SessionMeta {
                        session_id, cwd, ..
                    } => out.push(Observation::Session { session_id, cwd }),
                    Observed::UserTurn { at, wake } => {
                        out.push(Observation::UserTurn { at, wake });
                    }
                    Observed::TurnInterrupted { at } => {
                        out.push(Observation::TurnInterrupted { at });
                    }
                    Observed::TurnComplete { at, .. } => {
                        out.push(Observation::TurnComplete { at });
                    }
                    Observed::Goal { outcome } => out.push(Observation::Goal(outcome)),
                    Observed::TurnEnded {
                        background_agents, ..
                    } => {
                        out.push(Observation::TurnEnded { background_agents });
                    }
                    Observed::AgentWork { at } => out.push(Observation::AgentWork { at }),
                    Observed::BackgroundStarted {
                        id,
                        agent,
                        expires_in_ms,
                        at,
                    } => out.push(Observation::BackgroundStarted {
                        id,
                        agent,
                        expires_in_ms,
                        at,
                    }),
                    Observed::BackgroundEnded { id } => {
                        out.push(Observation::BackgroundEnded { id });
                    }
                    Observed::BackgroundResumable { id } => {
                        out.push(Observation::BackgroundResumable { id });
                    }
                    Observed::ContextFull => out.push(Observation::ContextFull),
                    Observed::Compacted => out.push(Observation::Compacted),
                    Observed::CompactFailed => out.push(Observation::CompactFailed),
                    Observed::LimitReached {
                        resets_at,
                        message,
                        window_minutes,
                        at,
                    } => out.push(Observation::LimitReached {
                        resets_at,
                        message,
                        window_minutes,
                        at,
                    }),
                    Observed::AssistantMessage { .. } | Observed::Malformed => {}
                }
            }
        }
        Ok(out)
    }

    fn usage(&self) -> UsageSnapshot {
        snapshot(&self.acc)
    }

    fn caught_up(&self) -> bool {
        self.tail.at_end()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_mcp_value_travels_in_a_variable_of_its_own() {
        let mcp = [
            McpAttachment::Stdio {
                name: "apify".into(),
                command: "npx".into(),
                args: vec![],
                env: vec![("APIFY_TOKEN".into(), "v1".into())],
            },
            McpAttachment::Stdio {
                name: "a".into(),
                command: "x".into(),
                args: vec![],
                env: vec![("B_TOKEN".into(), "v2".into())],
            },
            McpAttachment::Http {
                name: "a-b".into(),
                url: "https://x.test".into(),
                bearer: Some("v3".into()),
            },
        ];
        let (servers, secrets) = mcp_specs(&mcp);
        assert_eq!(
            secrets,
            [
                ("YULIUS_MCP_APIFY_APIFY_TOKEN".to_owned(), "v1".to_owned()),
                ("YULIUS_MCP_A_B_TOKEN".to_owned(), "v2".to_owned()),
                ("YULIUS_MCP_A_B_TOKEN_2".to_owned(), "v3".to_owned()),
            ]
        );
        assert!(matches!(
            &servers[0].transport,
            McpTransport::Stdio { env_vars, .. }
                if env_vars == &[("APIFY_TOKEN".to_owned(), "YULIUS_MCP_APIFY_APIFY_TOKEN".to_owned())]
        ));
        assert!(matches!(
            &servers[2].transport,
            McpTransport::Http { bearer_env: Some(var), .. } if var == "YULIUS_MCP_A_B_TOKEN_2"
        ));
    }

    /// The longest server name and variable still make names the harness takes, so the run
    /// starts.
    #[test]
    fn mcp_variable_names_stay_within_the_harness_bound() -> Result<(), Box<dyn std::error::Error>>
    {
        let name = "s".repeat(64);
        let key = format!("K{}", "_".repeat(127));
        let mcp: Vec<McpAttachment> = (0..12)
            .map(|i| McpAttachment::Stdio {
                name: format!("{}{i:02}", &name[..62]),
                command: "x".into(),
                args: vec![],
                env: vec![(key.clone(), "v".into())],
            })
            .collect();
        let (servers, secrets) = mcp_specs(&mcp);
        assert_eq!(secrets.len(), 12);
        assert!(
            secrets.iter().all(|(var, _)| var.len() <= 128),
            "{secrets:?}"
        );
        let request = launch::LaunchRequest {
            harness: Harness::ClaudeCode,
            mode: LaunchMode::New {
                session_id: None,
                prompt: None,
            },
            worktree: PathBuf::from("/w"),
            sandbox_home: PathBuf::from("/h"),
            run_dir: PathBuf::from("/r"),
            model: None,
            effort: None,
            contained: false,
            yulius_exe: PathBuf::from("/bin/yulius"),
            yulius_url: "http://127.0.0.1:1".into(),
            mcp_servers: servers,
            credential: Credential::ClaudeOauthToken(HSecret::new("token".into())),
        };
        launch::plan_launch(&request)?;
        Ok(())
    }

    /// SPEC-210 CON-001: a task run and a Superagent both start with their config dir's user
    /// settings defaulted to the Concise output style.
    #[test]
    fn every_claude_code_launch_defaults_to_the_concise_output_style()
    -> Result<(), Box<dyn std::error::Error>> {
        use yulius_app::model::Secret;
        let style = |config: &Path| -> Result<serde_json::Value, Box<dyn std::error::Error>> {
            let text = std::fs::read_to_string(config.join("settings.json"))?;
            Ok(serde_json::from_str::<serde_json::Value>(&text)?["outputStyle"].clone())
        };
        let dir = tempfile::tempdir()?;
        let adapter = HarnessAdapter::new(PathBuf::from("/bin/yulius"), "http://127.0.0.1:1");
        let home = dir.path().join("home");
        adapter.plan(&HarnessLaunch {
            harness: Harness::ClaudeCode,
            mode: SessionMode::New {
                session_id: None,
                prompt: None,
            },
            worktree: dir.path().to_path_buf(),
            sandbox_home: home.clone(),
            run_dir: dir.path().join("run"),
            model: None,
            effort: None,
            contained: false,
            mcp: Vec::new(),
            credential: Secret("token".into()),
        })?;
        assert_eq!(style(&home.join(".claude"))?, "Concise");
        let config = dir.path().join("cockpit");
        adapter.plan_cockpit(&CockpitLaunch {
            cwd: dir.path().to_path_buf(),
            config_dir: config.clone(),
            config_dir_host: config.clone(),
            resume: None,
            continue_interrupted: false,
            credential: Secret("token".into()),
            yulius: None,
            project_mcp_servers: Vec::new(),
            mcp: Vec::new(),
            contained: false,
        })?;
        assert_eq!(style(&config)?, "Concise");
        Ok(())
    }
}
