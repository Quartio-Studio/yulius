//! [`HarnessPort`] over `yulius-harness` (Claude Code 2.1.292 / Codex 0.160.1 structures).

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use yulius_app::error::{AppError, AppResult};
use yulius_app::ports::harness::{
    HarnessLaunch, HarnessPlan, HarnessPort, McpAttachment, Observation, OutputMasker, SessionMode,
    TranscriptWatch, TranslationTarget, UsageSnapshot,
};
use yulius_app::ports::session::{Containment, LaunchPlan, PlannedFile};
use yulius_domain::harness::Harness;
use yulius_domain::time::Timestamp;
use yulius_domain::transcript::{TranslationLimits, plan_translation};
use yulius_harness::launch::{
    self, Credential, LaunchMode, LaunchRequest, McpServerSpec, McpTransport,
};
use yulius_harness::observe::{Observed, UsageAccumulator};
use yulius_harness::secret::Secret as HSecret;
use yulius_harness::{claude, codex, login, paths, skills, tail};

/// Harness adapter.
pub struct HarnessAdapter {
    /// Path of the `yulius` executable used by hooks.
    pub yulius_exe: PathBuf,
    /// Internal listener URL.
    pub yulius_url: String,
}

fn to_system(ts: Timestamp) -> SystemTime {
    UNIX_EPOCH + Duration::from_millis(u64::try_from(ts.0.max(0)).unwrap_or(0))
}

fn io(err: std::io::Error) -> AppError {
    AppError::internal(err)
}

impl HarnessPort for HarnessAdapter {
    fn plan(&self, l: &HarnessLaunch) -> AppResult<HarnessPlan> {
        let mut extra_secrets = Vec::new();
        let mcp_servers = l
            .mcp
            .iter()
            .map(|m| match m {
                McpAttachment::Stdio {
                    name,
                    command,
                    args,
                    env,
                } => {
                    extra_secrets.extend(env.iter().cloned());
                    McpServerSpec {
                        name: name.clone(),
                        transport: McpTransport::Stdio {
                            command: command.clone(),
                            args: args.clone(),
                            env_names: env.iter().map(|(k, _)| k.clone()).collect(),
                        },
                    }
                }
                McpAttachment::Http { name, url, bearer } => {
                    let bearer_env = bearer.as_ref().map(|b| {
                        let var = format!(
                            "YULIUS_MCP_{}_TOKEN",
                            name.to_ascii_uppercase().replace('-', "_")
                        );
                        extra_secrets.push((var.clone(), b.clone()));
                        var
                    });
                    McpServerSpec {
                        name: name.clone(),
                        transport: McpTransport::Http {
                            url: url.clone(),
                            bearer_env,
                        },
                    }
                }
            })
            .collect();
        let credential = match l.harness {
            Harness::ClaudeCode => {
                Credential::ClaudeOauthToken(HSecret::new(l.credential.expose().to_owned()))
            }
            Harness::Codex => {
                Credential::CodexAuthJson(HSecret::new(l.credential.expose().as_bytes().to_vec()))
            }
        };
        let mode = match &l.mode {
            SessionMode::New { session_id, prompt } => LaunchMode::New {
                session_id: session_id.clone(),
                prompt: prompt.clone(),
            },
            SessionMode::Resume { session_id, prompt } => LaunchMode::Resume {
                session_id: session_id.clone(),
                prompt: prompt.clone(),
            },
        };
        let request = LaunchRequest {
            harness: l.harness,
            mode,
            worktree: l.worktree.clone(),
            sandbox_home: l.sandbox_home.clone(),
            run_dir: l.run_dir.clone(),
            codex_home: l.codex_home.clone(),
            model: l.model.clone(),
            effort: l.effort.clone(),
            contained: l.contained,
            yulius_exe: self.yulius_exe.clone(),
            yulius_url: self.yulius_url.clone(),
            mcp_servers,
            credential,
        };
        let plan = launch::plan_launch(&request).map_err(|e| AppError::Invalid(e.to_string()))?;
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

    fn claude_transcript(&self, sandbox_home: &Path, worktree: &Path, session_id: &str) -> PathBuf {
        paths::claude_transcript_path(&sandbox_home.join(".claude"), worktree, session_id)
    }

    fn find_codex_rollout(
        &self,
        sessions_dir: &Path,
        session_id: &str,
    ) -> AppResult<Option<PathBuf>> {
        paths::find_codex_rollout(sessions_dir, session_id).map_err(io)
    }

    fn discover_codex_session(
        &self,
        sessions_dir: &Path,
        worktree: &Path,
        since: Timestamp,
    ) -> AppResult<Option<(String, PathBuf)>> {
        let wanted = worktree.to_string_lossy();
        for path in paths::recent_codex_rollouts(sessions_dir, to_system(since)).map_err(io)? {
            let Ok(file) = std::fs::File::open(&path) else {
                continue;
            };
            let mut first = String::new();
            let mut reader = std::io::BufReader::new(std::io::Read::take(file, 4 << 20));
            let _ = std::io::BufRead::read_line(&mut reader, &mut first);
            for observed in codex::observe_line(first.trim_end()) {
                if let Observed::SessionMeta {
                    session_id,
                    cwd: Some(cwd),
                    ..
                } = observed
                    && cwd.trim_end_matches('/') == wanted.trim_end_matches('/')
                {
                    return Ok(Some((session_id, path.clone())));
                }
            }
        }
        Ok(None)
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
        from_start: bool,
    ) -> AppResult<Box<dyn TranscriptWatch>> {
        let tail = if from_start {
            tail::TranscriptTail::open(path, tail::DEFAULT_MAX_LINE_BYTES)
        } else {
            tail::TranscriptTail::open_at_end(path, tail::DEFAULT_MAX_LINE_BYTES)
        }
        .map_err(io)?;
        Ok(Box::new(Watch {
            harness,
            tail,
            acc: UsageAccumulator::new(),
        }))
    }

    fn translate(
        &self,
        from: Harness,
        source: &Path,
        to: Harness,
        target: &TranslationTarget,
    ) -> AppResult<PathBuf> {
        let text = std::fs::read_to_string(source).map_err(io)?;
        let lines = text.lines();
        let transcript = match from {
            Harness::ClaudeCode => claude::read_transcript(lines),
            Harness::Codex => codex::read_transcript(lines),
        };
        let planned = plan_translation(&transcript, TranslationLimits::default());
        // Record the model the resumed session will use; fall back to the source's model.
        let model = target
            .model
            .clone()
            .or_else(|| transcript.meta.model.clone())
            .unwrap_or_else(|| "default".to_owned());
        let (path, contents) = match to {
            Harness::ClaudeCode => {
                let mut ids = || uuid::Uuid::new_v4().to_string();
                let contents = claude::write_transcript(
                    &planned,
                    claude::WriteParams {
                        session_id: &target.session_id,
                        cwd: &target.cwd.to_string_lossy(),
                        version: "2.1.292",
                        start_ms: target.now.0,
                        model: &model,
                        git_branch: target.git_branch.as_deref().unwrap_or(""),
                        ids: &mut ids,
                    },
                );
                (
                    paths::claude_transcript_path(
                        &target.sandbox_home.join(".claude"),
                        &target.cwd,
                        &target.session_id,
                    ),
                    contents,
                )
            }
            Harness::Codex => {
                let contents = codex::write_rollout(
                    &planned,
                    &codex::RolloutParams {
                        session_id: &target.session_id,
                        cwd: &target.cwd.to_string_lossy(),
                        cli_version: "0.160.1",
                        start_ms: target.now.0,
                        model: &model,
                        effort: target.effort.as_deref(),
                    },
                );
                let dir = target
                    .codex_sessions
                    .join(codex::rollout_relative_dir(target.now.0));
                (
                    dir.join(codex::rollout_file_name(target.now.0, &target.session_id)),
                    contents,
                )
            }
        };
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(io)?;
        }
        let tmp = path.with_extension("jsonl.tmp");
        std::fs::write(&tmp, contents).map_err(io)?;
        std::fs::rename(&tmp, &path).map_err(io)?;
        Ok(path)
    }

    fn login_argv(&self, harness: Harness) -> Vec<String> {
        match harness {
            Harness::ClaudeCode => login::claude_setup_token_argv(),
            Harness::Codex => login::codex_device_login_argv(),
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
        claude_dir: &Path,
        codex_dir: &Path,
    ) -> AppResult<()> {
        let dirs: Vec<skills::SkillDir> = skill_dirs
            .iter()
            .map(|(name, path)| skills::SkillDir {
                name: name.clone(),
                path: path.clone(),
            })
            .collect();
        skills::install_skills(&dirs, claude_dir, codex_dir)
            .map(|_| ())
            .map_err(AppError::internal)
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
    acc: UsageAccumulator,
}

impl TranscriptWatch for Watch {
    fn poll(&mut self) -> AppResult<Vec<Observation>> {
        let lines = self.tail.poll().map_err(io)?;
        let mut out = Vec::new();
        for line in lines {
            let observed = match self.harness {
                Harness::ClaudeCode => claude::observe_line(&line),
                Harness::Codex => codex::observe_line(&line),
            };
            self.acc.observe_all(observed.iter());
            for o in observed {
                match o {
                    Observed::SessionMeta {
                        session_id, cwd, ..
                    } => out.push(Observation::Session { session_id, cwd }),
                    Observed::UserTurn => out.push(Observation::UserTurn),
                    Observed::TurnComplete { .. } => out.push(Observation::TurnComplete),
                    Observed::RateLimits(usage) => out.push(Observation::RateLimits(usage)),
                    Observed::LimitReached { resets_at, message } => {
                        out.push(Observation::LimitReached { resets_at, message })
                    }
                    Observed::TurnContext { .. }
                    | Observed::AssistantMessage { .. }
                    | Observed::CumulativeUsage(_)
                    | Observed::Malformed => {}
                }
            }
        }
        Ok(out)
    }

    fn usage(&self) -> UsageSnapshot {
        UsageSnapshot {
            tokens: self.acc.usage(),
            model: self.acc.model().map(str::to_owned),
            turns: self.acc.turn_complete_signals(),
        }
    }
}
