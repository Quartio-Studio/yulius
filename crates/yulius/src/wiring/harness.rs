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
            // The worktree lies in the sandbox directory, the folder above the home (#801).
            yulius_harness::claude_config::ensure_claude_config(
                &l.sandbox_home.join(".claude"),
                l.sandbox_home.parent().unwrap_or(&l.sandbox_home),
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
            model: l.model.clone(),
            credential: Credential::ClaudeOauthToken(HSecret::new(
                l.credential.expose().to_owned(),
            )),
            yulius: l.yulius.as_ref().map(|(url, token)| launch::CockpitYulius {
                url: url.clone(),
                token: HSecret::new(token.expose().to_owned()),
            }),
            project_mcp_servers: l.project_mcp_servers.clone(),
            mcp_servers,
            // Without Bash, hooks or other servers there is nothing to redact: the Yulius tools
            // redact their own results (SPEC-427).
            redaction_exe: l.only_yulius.is_none().then(|| self.yulius_exe.clone()),
            bypass_permissions: l.contained,
            only_yulius: l.only_yulius.clone(),
        })
        .map_err(|e| AppError::Invalid(e.to_string()))?;
        // Contained, it bypasses approvals as a task run does, its warning accepted (SPEC-208).
        yulius_harness::claude_config::ensure_claude_config(
            &l.config_dir_host,
            &l.sandbox_dir,
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
            limits: WorkflowLimits::default(),
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
        cli_version: acc.cli_version().map(str::to_owned),
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
    /// What the session's own thread and its workflows' agents last said of a usage limit.
    limits: WorkflowLimits,
}

/// A usage limit met by a dynamic workflow's agent is the session's (SPEC-486 WFL-002): Claude
/// Code writes it to that agent's transcript only, pauses the workflow and does not wake the
/// session's thread. It is reported once the session's own transcript is read to its end, and
/// only while it is news: not when the thread did something later than it (a limit the session went
/// on past is history, SPEC-072 LRS-001), not while the thread's own limit stands (that one was
/// reported), and once until the thread does something again.
#[derive(Debug, Default)]
struct WorkflowLimits {
    /// The latest limit a workflow's agent met that is not reported yet, and when.
    held: Option<(Option<Timestamp>, Observation)>,
    /// When the session's own thread last did something: a prompt, work, an answer, an interrupt.
    thread_at: Option<Timestamp>,
    /// The thread's own limit stands, or a workflow agent's was reported: nothing more to say
    /// until the thread does something.
    said: bool,
}

impl WorkflowLimits {
    /// A line of a workflow agent's transcript said `observed`.
    fn side(&mut self, observed: Observed) {
        let Observed::LimitReached { at, .. } = &observed else {
            return;
        };
        let at = *at;
        if self.held.as_ref().is_none_or(|(held, _)| *held <= at)
            && let Some(limit) = limit_observation(observed)
        {
            self.held = Some((at, limit));
        }
    }

    /// The session's own thread said `observation`.
    fn thread(&mut self, observation: &Observation) {
        match observation {
            Observation::LimitReached { .. } | Observation::AccessLost { .. } => self.said = true,
            Observation::UserTurn { at, .. }
            | Observation::TurnInterrupted { at }
            | Observation::AgentWork { at }
            | Observation::TurnComplete { at } => {
                // A sub-agent's turn end (no time) is not the thread's.
                if let Some(at) = at {
                    self.thread_at = self.thread_at.max(Some(*at));
                    self.said = false;
                }
            }
            _ => {}
        }
    }

    /// The held limit, when it is news and the session's transcript is read to its end.
    fn report(&mut self, caught_up: bool) -> Option<Observation> {
        if !caught_up {
            return None;
        }
        let (at, limit) = self.held.take()?;
        if self.said || at.is_some_and(|at| self.thread_at > Some(at)) {
            return None;
        }
        self.said = true;
        Some(limit)
    }
}

/// A limit record as the application hears it.
fn limit_observation(observed: Observed) -> Option<Observation> {
    match observed {
        Observed::LimitReached {
            resets_at,
            message,
            window_minutes,
            scope,
            at,
        } => Some(Observation::LimitReached {
            resets_at,
            message,
            window_minutes,
            scope,
            at,
        }),
        _ => None,
    }
}

impl TranscriptWatch for Watch {
    fn poll(&mut self) -> AppResult<Vec<Observation>> {
        let lines = self.tail.poll().map_err(io)?;
        // Usage counts from a sub-agent's lines, and from a workflow agent's its usage limit
        // too (SPEC-486); one that cannot be read leaves the session's own observations alone.
        match self.subagents.poll_sides() {
            Ok(side) => {
                for side in side {
                    for observed in claude::observe_line(&side.line) {
                        self.acc.observe_side(&observed);
                        if side.workflow {
                            self.limits.side(observed);
                        }
                    }
                }
            }
            Err(err) => tracing::debug!(error = %err, "a sub-agent transcript could not be read"),
        }
        let mut out: Vec<Observation> = Vec::new();
        let mut heard = 0;
        for line in lines {
            if !self.since.keep(&line) {
                continue;
            }
            let (observed, version) = match self.harness {
                Harness::ClaudeCode => claude::observe_versioned(&line),
            };
            // Every record of the run's own thread names its Claude Code (SPEC-461 CCM-011).
            if let Some(version) = version {
                self.acc.note_cli_version(&version);
            }
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
                    Observed::Goal { outcome, at } => out.push(Observation::Goal(outcome, at)),
                    Observed::TurnEnded {
                        background_agents,
                        at,
                        ..
                    } => {
                        out.push(Observation::TurnEnded {
                            background_agents,
                            at,
                        });
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
                    Observed::GoalRetry { armed, notice } => {
                        out.push(Observation::GoalRetry { armed, notice });
                    }
                    Observed::ContextFull => out.push(Observation::ContextFull),
                    Observed::Compacted => out.push(Observation::Compacted),
                    Observed::CompactFailed => out.push(Observation::CompactFailed),
                    Observed::Queue { change, at } => out.push(Observation::Queue(change, at)),
                    Observed::WorkflowLaunched { task, run } => {
                        out.push(Observation::WorkflowLaunched { task, run });
                    }
                    limit @ Observed::LimitReached { .. } => {
                        out.extend(limit_observation(limit));
                    }
                    Observed::AccessLost { message, at } => {
                        out.push(Observation::AccessLost { message, at });
                    }
                    Observed::AssistantMessage { .. } | Observed::Malformed => {}
                }
            }
            for observation in &out[heard..] {
                self.limits.thread(observation);
            }
            heard = out.len();
        }
        out.extend(self.limits.report(self.tail.at_end()));
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

    /// SPEC-471 CCQ-001: a run that resumed a session reads the lines recorded from its own
    /// start on, so an `enqueue` the run before left behind (its process ended with the prompt
    /// still waiting) is not in this run's queue; a run that started the session reads all.
    #[test]
    fn a_resumed_run_does_not_count_what_the_run_before_left_waiting()
    -> Result<(), Box<dyn std::error::Error>> {
        use yulius_domain::agent_cli::QueueChange;
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("session.jsonl");
        let lines = [
            r#"{"type":"queue-operation","operation":"enqueue","timestamp":"2026-11-12T06:00:43.971Z","sessionId":"s","content":"second"}"#,
            r#"{"type":"user","isSidechain":false,"timestamp":"2026-11-12T06:01:10.589Z","sessionId":"s","version":"2.1.322","promptSource":"typed","message":{"role":"user","content":"after"}}"#,
            r#"{"type":"queue-operation","operation":"enqueue","timestamp":"2026-11-12T06:01:11.000Z","sessionId":"s","content":"third"}"#,
        ];
        std::fs::write(&path, format!("{}\n", lines.join("\n")))?;
        let resumed_at =
            yulius_harness::time::parse_rfc3339("2026-11-12T06:01:00.000Z").ok_or("a time")?;
        let queued = |from: WatchFrom| -> Result<usize, Box<dyn std::error::Error>> {
            let mut watch = Watch {
                harness: Harness::ClaudeCode,
                tail: tail::TranscriptTail::open(&path, tail::DEFAULT_MAX_LINE_BYTES)?,
                since: LinesSince::new(match from {
                    WatchFrom::Start => None,
                    WatchFrom::Since(at) => Some(at),
                }),
                subagents: SubagentTails::new(&path, None),
                acc: UsageAccumulator::new(),
                limits: WorkflowLimits::default(),
            };
            let seen = watch.poll().map_err(|err| err.to_string())?;
            let added =
                |one: &&Observation| matches!(one, Observation::Queue(QueueChange::Added, Some(_)));
            Ok(seen.iter().filter(added).count())
        };
        assert_eq!(queued(WatchFrom::Start)?, 2);
        assert_eq!(queued(WatchFrom::Since(resumed_at))?, 1);
        Ok(())
    }

    /// The limit record of a workflow's agent, as Claude Code 2.1.323 writes it (synthetic).
    fn refusal(at: &str) -> String {
        format!(
            r#"{{"isSidechain":true,"agentId":"a8a9","type":"assistant","timestamp":"{at}","message":{{"id":"e-{at}","model":"<synthetic>","role":"assistant","stop_reason":"stop_sequence","usage":{{"input_tokens":0,"output_tokens":0}},"content":[{{"type":"text","text":"You've hit your 5-hour limit"}}]}},"apiError":"usage_limit_reached","apiErrorParams":{{"rate_limit_info":{{"status":"rejected","resetsAt":1794598079,"rateLimitType":"five_hour"}}}},"error":"rate_limit","isApiErrorMessage":true}}"#
        )
    }

    fn watch_of(path: &Path) -> Result<Watch, Box<dyn std::error::Error>> {
        Ok(Watch {
            harness: Harness::ClaudeCode,
            tail: tail::TranscriptTail::open(path, tail::DEFAULT_MAX_LINE_BYTES)?,
            since: LinesSince::new(None),
            subagents: SubagentTails::new(path, None),
            acc: UsageAccumulator::new(),
            limits: WorkflowLimits::default(),
        })
    }

    fn limits_in(seen: &[Observation]) -> usize {
        seen.iter()
            .filter(|one| matches!(one, Observation::LimitReached { .. }))
            .count()
    }

    /// SPEC-486 WFL-002: the limit a workflow's agent met is written to that agent's file only,
    /// and is the session's: reported once, with its reset, after the session's own lines. The
    /// same record in a sub-agent's file outside a workflow is not (the session's thread is
    /// woken and refused itself).
    #[test]
    fn a_workflow_agent_s_limit_is_reported_as_the_session_s_once()
    -> Result<(), Box<dyn std::error::Error>> {
        use std::io::Write as _;
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("s1.jsonl");
        let lines = [
            r#"{"type":"user","isSidechain":false,"timestamp":"2026-11-13T16:27:57.672Z","sessionId":"s1","promptSource":"typed","message":{"role":"user","content":"go"}}"#,
            r#"{"type":"user","isSidechain":false,"timestamp":"2026-11-13T16:27:57.749Z","message":{"role":"user","content":[{"tool_use_id":"t","type":"tool_result","content":"Workflow launched in background."}]},"toolUseResult":{"status":"async_launched","taskId":"wtx0s195j","taskType":"local_workflow","runId":"wf_13b2b557-e88"}}"#,
            r#"{"type":"assistant","isSidechain":false,"timestamp":"2026-11-13T16:27:57.804Z","message":{"id":"m1","model":"claude-opus-5-5","role":"assistant","content":[{"type":"text","text":"I wait."}],"stop_reason":"end_turn"}}"#,
            r#"{"type":"system","subtype":"turn_duration","isSidechain":false,"timestamp":"2026-11-13T16:27:57.833Z","durationMs":169,"messageCount":18,"pendingWorkflowCount":1}"#,
        ];
        std::fs::write(&path, format!("{}\n", lines.join("\n")))?;
        let sub = yulius_harness::subagents::subagent_dir(&path);
        let run = yulius_harness::subagents::workflow_dir(&path).join("wf_13b2b557-e88");
        std::fs::create_dir_all(&run)?;
        let agent = run.join("agent-a8a9.jsonl");
        std::fs::write(&agent, format!("{}\n", refusal("2026-11-13T16:27:59.360Z")))?;
        let mut watch = watch_of(&path)?;
        let seen = watch.poll().map_err(|err| err.to_string())?;
        assert!(
            matches!(
                seen.last(),
                Some(Observation::LimitReached {
                    resets_at: Some(Timestamp(1_794_598_079_000)),
                    at: Some(Timestamp(1_794_587_279_360)),
                    ..
                })
            ),
            "{seen:?}"
        );
        assert_eq!(limits_in(&seen), 1);
        assert!(seen.contains(&Observation::WorkflowLaunched {
            task: "wtx0s195j".into(),
            run: "wf_13b2b557-e88".into(),
        }));
        assert!(seen.iter().any(|one| matches!(
            one,
            Observation::TurnEnded {
                background_agents: Some(1),
                ..
            }
        )));
        // The run's other agents are refused too: said once, until the thread does something.
        let mut file = std::fs::OpenOptions::new().append(true).open(&agent)?;
        writeln!(file, "{}", refusal("2026-11-13T16:28:01.000Z"))?;
        let seen = watch.poll().map_err(|err| err.to_string())?;
        assert_eq!(limits_in(&seen), 0, "{seen:?}");

        // A sub-agent's own refusal, outside a workflow, is not reported.
        let plain = dir.path().join("s2.jsonl");
        std::fs::write(&plain, format!("{}\n", lines[0]))?;
        let sub2 = yulius_harness::subagents::subagent_dir(&plain);
        std::fs::create_dir_all(&sub2)?;
        std::fs::write(
            sub2.join("agent-a1.jsonl"),
            format!("{}\n", refusal("2026-11-13T16:27:59.360Z")),
        )?;
        let seen = watch_of(&plain)?.poll().map_err(|err| err.to_string())?;
        assert_eq!(limits_in(&seen), 0, "{seen:?}");
        assert!(sub.exists());
        Ok(())
    }

    /// SPEC-486 WFL-002: a workflow agent's limit is news only while the session's thread did
    /// nothing after it, while the thread's own limit does not stand, and once the session's
    /// transcript is read to its end (a replay judges it against all that came later).
    #[test]
    fn a_workflow_agent_s_limit_is_reported_only_while_it_is_news() {
        let side = |ms: i64| Observed::LimitReached {
            resets_at: None,
            message: "limit".into(),
            window_minutes: None,
            scope: yulius_domain::usage::LimitScope::Account,
            at: Some(Timestamp(ms)),
        };
        let own = Observation::LimitReached {
            resets_at: None,
            message: "limit".into(),
            window_minutes: None,
            scope: yulius_domain::usage::LimitScope::Account,
            at: Some(Timestamp(5)),
        };
        let work = |ms: i64| Observation::AgentWork {
            at: Some(Timestamp(ms)),
        };
        // Held while the transcript is being caught up on, then said; the latest one.
        let mut limits = WorkflowLimits::default();
        limits.side(side(10));
        limits.side(side(12));
        limits.side(side(11));
        assert_eq!(limits.report(false), None);
        assert!(matches!(
            limits.report(true),
            Some(Observation::LimitReached {
                at: Some(Timestamp(12)),
                ..
            })
        ));
        assert_eq!(limits.report(true), None, "once");
        // The thread worked after it: history. Work before it, or at its very time (the end of
        // the turn that launched the workflow), leaves it news.
        let mut limits = WorkflowLimits::default();
        limits.side(side(10));
        limits.thread(&work(11));
        assert_eq!(limits.report(true), None);
        for before in [9, 10] {
            let mut limits = WorkflowLimits::default();
            limits.thread(&work(before));
            limits.side(side(10));
            assert!(limits.report(true).is_some(), "{before}");
        }
        // A sub-agent's turn end (no time) is not the thread's.
        let mut limits = WorkflowLimits::default();
        limits.side(side(10));
        limits.thread(&Observation::TurnComplete { at: None });
        assert!(limits.report(true).is_some());
        // The thread's own limit stands: that one was reported.
        let mut limits = WorkflowLimits::default();
        limits.thread(&own);
        limits.side(side(10));
        assert_eq!(limits.report(true), None);
        // After the thread did something again, a new limit is news again.
        limits.thread(&Observation::UserTurn {
            at: Some(Timestamp(20)),
            wake: false,
        });
        limits.side(side(30));
        assert!(limits.report(true).is_some());
        // Something else in a workflow agent's file is nothing to report.
        let mut limits = WorkflowLimits::default();
        limits.side(Observed::AgentWork { at: None });
        assert_eq!(limits.report(true), None);
    }

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
            sandbox_dir: dir.path().to_path_buf(),
            config_dir: config.clone(),
            config_dir_host: config.clone(),
            resume: None,
            continue_interrupted: false,
            model: None,
            credential: Secret("token".into()),
            yulius: None,
            project_mcp_servers: Vec::new(),
            mcp: Vec::new(),
            contained: false,
            only_yulius: None,
        })?;
        assert_eq!(style(&config)?, "Concise");
        Ok(())
    }

    /// The lines of a session whose first run (Claude Code 2.1.311) got a prompt, and whose
    /// second run was resumed at 11:56:37 on 2.1.322 with nothing typed: the structure of the
    /// records a real session wrote on 2026-11-11 (its texts are made up). The resumed process
    /// writes a meta record, an attachment and its answers; no prompt record for hours.
    fn resumed_without_a_prompt() -> [&'static str; 7] {
        [
            r#"{"parentUuid":null,"isSidechain":false,"promptId":"p1","type":"user","message":{"role":"user","content":"Fix the bug"},"uuid":"u1","timestamp":"2026-11-11T11:28:42.782Z","userType":"external","entrypoint":"cli","cwd":"/w","sessionId":"s1","version":"2.1.311","gitBranch":"b","slug":"s"}"#,
            r#"{"parentUuid":"u1","isSidechain":false,"message":{"model":"claude-fable-5-2","id":"msg_1","type":"message","role":"assistant","content":[{"type":"text","text":"On it."}],"stop_reason":"tool_use","usage":{"input_tokens":3,"output_tokens":5}},"type":"assistant","uuid":"a1","timestamp":"2026-11-11T11:28:50.000Z","userType":"external","entrypoint":"cli","cwd":"/w","sessionId":"s1","version":"2.1.311"}"#,
            r#"{"type":"last-prompt","lastPrompt":"Fix the bug","sessionId":"s1"}"#,
            r#"{"type":"mode","mode":"default","sessionId":"s1"}"#,
            r#"{"parentUuid":"a1","isSidechain":false,"type":"user","message":{"role":"user","content":[{"type":"text","text":"Continue from where you left off."}]},"isMeta":true,"uuid":"u2","timestamp":"2026-11-11T11:56:37.389Z","turnPosition":{"promptIndex":3,"turnIndex":4},"userType":"external","entrypoint":"cli","cwd":"/w","sessionId":"s1","version":"2.1.322","gitBranch":"b","slug":"s"}"#,
            r#"{"parentUuid":"u2","isSidechain":false,"attachment":{"type":"remote_session_change","url":null,"commit":"c","pr":"p","sendUserFileHint":false,"managedCommit":false,"managedPr":false},"type":"attachment","uuid":"at1","timestamp":"2026-11-11T11:56:37.692Z","userType":"external","entrypoint":"cli","cwd":"/w","sessionId":"s1","version":"2.1.322"}"#,
            r#"{"parentUuid":"at1","isSidechain":false,"message":{"model":"claude-fable-5-2","id":"msg_2","type":"message","role":"assistant","content":[{"type":"thinking","thinking":"hm","signature":"x"}],"stop_reason":"tool_use","usage":{"input_tokens":4,"output_tokens":9}},"type":"assistant","uuid":"a2","timestamp":"2026-11-11T11:57:36.901Z","userType":"external","entrypoint":"cli","cwd":"/w","sessionId":"s1","version":"2.1.322"}"#,
        ]
    }

    /// SPEC-461 CCM-011 (found with a real session's transcript after the merge): a run that
    /// was resumed with nothing typed names its Claude Code with the first record it writes,
    /// not with its next prompt, which may be hours away; and never the earlier run's.
    #[test]
    fn a_run_resumed_with_nothing_typed_names_its_claude_code_at_once()
    -> Result<(), Box<dyn std::error::Error>> {
        let lines = resumed_without_a_prompt();
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("s1.jsonl");
        let adapter = HarnessAdapter::new(PathBuf::from("/bin/yulius"), "http://127.0.0.1:1");
        let ran_on = |watch: &dyn TranscriptWatch| watch.usage().cli_version;

        // The first run, watched from the start.
        std::fs::write(&path, format!("{}\n", lines[..4].join("\n")))?;
        let mut first = adapter.watch(Harness::ClaudeCode, &path, WatchFrom::Start)?;
        first.poll()?;
        assert_eq!(ran_on(first.as_ref()).as_deref(), Some("2.1.311"));

        // The resumed run reads its own lines: before it wrote one, it names nothing.
        let started =
            claude::record_time(r#"{"timestamp":"2026-11-11T11:56:36.000Z"}"#).ok_or("a time")?;
        let mut resumed = adapter.watch(Harness::ClaudeCode, &path, WatchFrom::Since(started))?;
        resumed.poll()?;
        assert_eq!(ran_on(resumed.as_ref()), None, "the earlier run's version");

        // Its first record is no prompt, and names the version.
        let mut file = std::fs::OpenOptions::new().append(true).open(&path)?;
        std::io::Write::write_all(&mut file, format!("{}\n", lines[4]).as_bytes())?;
        resumed.poll()?;
        assert_eq!(ran_on(resumed.as_ref()).as_deref(), Some("2.1.322"));
        std::io::Write::write_all(&mut file, format!("{}\n", lines[5..].join("\n")).as_bytes())?;
        resumed.poll()?;
        assert_eq!(ran_on(resumed.as_ref()).as_deref(), Some("2.1.322"));

        // A recount of a run's window says the same, from that window's lines only.
        let counted = recount::count_run(&path, Some(started), None)?.ok_or("the transcript")?;
        assert_eq!(counted.cli_version(), Some("2.1.322"));
        let first = recount::count_run(&path, None, Some(started))?.ok_or("the transcript")?;
        assert_eq!(
            first.cli_version(),
            Some("2.1.311"),
            "the later run's version"
        );
        let between =
            claude::record_time(r#"{"timestamp":"2026-11-11T11:40:00.000Z"}"#).ok_or("a time")?;
        let idle = recount::count_run(&path, Some(between), Some(started))?.ok_or("transcript")?;
        assert_eq!(
            idle.cli_version(),
            None,
            "a window without a line names nothing"
        );
        Ok(())
    }
}
