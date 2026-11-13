//! [`HarnessPort`] over `yulius-harness` (Claude Code 2.1.292 structures).

use std::collections::BTreeMap;
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
use yulius_domain::usage::{LimitScope, ModelFamily};
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

/// The plugins a session with the config dir `config_dir`, started in `worktree`, would load a
/// language server from, which its run switches off (SPEC-485 LSP-003). Said in the log, with
/// what the lookup could not read: a plugin behind that is not known to bring none.
fn language_server_plugins(config_dir: &Path, sandbox_dir: &Path, worktree: &Path) -> Vec<String> {
    let found =
        yulius_harness::claude_plugins::language_server_plugins(config_dir, sandbox_dir, worktree);
    if !found.plugins.is_empty() {
        tracing::info!(
            config_dir = %config_dir.display(),
            plugins = ?found.plugins,
            "switching off the plugins that bring a language server for this run"
        );
    }
    if !found.unread.is_empty() {
        tracing::warn!(
            config_dir = %config_dir.display(),
            unread = ?found.unread,
            "could not read everything about this session's plugins; a language server may still start"
        );
    }
    found.plugins
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
            // A run's language servers are Yulius's to decide, and they are off: the plugins of
            // the sandbox home that bring one are switched off in the run's own settings, which
            // outrank the files a session, a `/plugin` or Claude Code's install dialog writes
            // (SPEC-485 LSP-003, ADR-0327).
            disabled_plugins: match l.harness {
                Harness::ClaudeCode => language_server_plugins(
                    &l.sandbox_home.join(".claude"),
                    l.sandbox_home.parent().unwrap_or(&l.sandbox_home),
                    &l.worktree,
                ),
            },
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
            // As a task run: no plugin's language server (SPEC-485 LSP-003).
            disabled_plugins: language_server_plugins(&l.config_dir_host, &l.sandbox_dir, &l.cwd),
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
/// only while it is news:
/// - not when a prompt recorded later than it was worked on by the session's thread (the
///   model answered after the refusal: a limit the session went on past is history, SPEC-072
///   LRS-001); an answer that was under way when the agent was refused belongs to an earlier
///   prompt and changes nothing;
/// - not while the thread's own limit stands (that one was reported);
/// - not when its reset has passed (Claude Code continues the workflow by itself then);
/// - once, until a prompt reaches the thread again.
#[derive(Debug, Default)]
struct WorkflowLimits {
    /// The latest limit a workflow's agent met that is not reported yet.
    held: Option<HeldLimit>,
    /// When the thread's last prompt was recorded.
    prompt_at: Option<Timestamp>,
    /// When the last prompt the thread worked on was recorded.
    answered_at: Option<Timestamp>,
    /// The thread's own limit stands, or a workflow agent's was reported: nothing more to say
    /// until a prompt reaches the thread.
    said: bool,
    /// The models the workflows' agents run.
    agents: WorkflowAgents,
}

/// A workflow agent's limit and what decides whether it is news.
#[derive(Debug)]
struct HeldLimit {
    at: Option<Timestamp>,
    resets_at: Option<Timestamp>,
    limit: Observation,
}

impl WorkflowLimits {
    /// A line of the workflow agent's transcript `agent` said `observed`.
    fn side(&mut self, agent: &Path, observed: Observed) {
        let (resets_at, message, window_minutes, scope, at) = match observed {
            Observed::LimitReached {
                resets_at,
                message,
                window_minutes,
                scope,
                at,
            } => (resets_at, message, window_minutes, scope, at),
            Observed::AssistantMessage {
                model: Some(model), ..
            } => return self.agents.answered(agent, &model),
            _ => return,
        };
        if self.held.as_ref().is_some_and(|held| held.at > at) {
            return;
        }
        // A limit of "the model that runs" is that agent's model's (WFL-002).
        let Some(scope) = self.agents.scope(agent, scope) else {
            return;
        };
        self.held = Some(HeldLimit {
            at,
            resets_at,
            limit: Observation::WorkflowLimitReached {
                resets_at,
                message,
                window_minutes,
                scope,
                at,
            },
        });
    }

    /// The session's own thread said `observation`.
    fn thread(&mut self, observation: &Observation) {
        match observation {
            Observation::LimitReached { .. } | Observation::AccessLost { .. } => self.said = true,
            Observation::UserTurn { at: Some(at), .. } => {
                self.prompt_at = self.prompt_at.max(Some(*at));
                self.said = false;
            }
            // A sub-agent's turn end (no time) is not the thread's.
            Observation::AgentWork { at: Some(_) } | Observation::TurnComplete { at: Some(_) } => {
                self.answered_at = self.answered_at.max(self.prompt_at);
            }
            _ => {}
        }
    }

    /// The held limit, at `now`, when it is news and the session's transcript is read to its
    /// end.
    fn report(&mut self, caught_up: bool, now: Timestamp) -> Option<Observation> {
        if !caught_up {
            return None;
        }
        let held = self.held.take()?;
        let answered_since = held
            .at
            .is_some_and(|at| self.answered_at.is_some_and(|answered| answered > at));
        let reset = held.resets_at.is_some_and(|resets| resets <= now);
        if self.said || answered_since || reset {
            return None;
        }
        self.said = true;
        Some(held.limit)
    }
}

/// Longest `agent-<id>.meta.json` read.
const AGENT_META_BYTES: u64 = 64 * 1024;

/// What an agent's `agent-<id>.meta.json` says of the model it runs.
#[derive(Debug, Clone, PartialEq, Eq)]
struct AgentMeta {
    /// The model the workflow's script gave the agent (`model`, an id or an alias).
    model: Option<String>,
    /// The agent is of the workflow's default type (`agentType: "workflow-subagent"`), which
    /// has no model of its own; another agent type may name one in its definition, which the
    /// meta file does not repeat.
    default_type: bool,
}

/// The meta file beside the workflow agent's transcript `agent`; `None` when it cannot be read
/// as one (the folder is the agent's to write: a link is not followed).
fn read_agent_meta(agent: &Path) -> Option<AgentMeta> {
    let file = tail::open_regular(&agent.with_extension("meta.json")).ok()?;
    let mut text = String::new();
    std::io::Read::read_to_string(&mut std::io::Read::take(file, AGENT_META_BYTES), &mut text)
        .ok()?;
    let meta: serde_json::Value = serde_json::from_str(&text).ok()?;
    let model = match meta.get("model") {
        None | Some(serde_json::Value::Null) => None,
        Some(model) => Some(model.as_str()?.to_owned()),
    };
    Some(AgentMeta {
        model,
        default_type: meta.get("agentType").and_then(serde_json::Value::as_str)
            == Some("workflow-subagent"),
    })
}

/// The models a session's workflow agents run, as far as their files say (SPEC-486 WFL-002).
/// A limit of "the model that runs" (`LimitScope::Model(None)`, a model's usage credits) met by
/// a workflow's agent is of that agent's model, which is not always the session's: the script
/// may give an agent a model, an agent type may have one, and the environment may give every
/// workflow agent one (`CLAUDE_CODE_WORKFLOW_SUBAGENT_MODEL`). Claude Code 2.1.323 writes the
/// first to the agent's meta file and none of the others anywhere but in the records the model
/// answered. Bounded by the workflow agents' files followed.
#[derive(Debug, Default)]
struct WorkflowAgents {
    /// Each agent's meta file, read once.
    metas: BTreeMap<PathBuf, Option<AgentMeta>>,
    /// The model each agent's own answered records name.
    answered: BTreeMap<PathBuf, String>,
    /// For each run's folder, the model an agent of the default type with no model of its own
    /// was answered by: what such an agent of that run runs.
    defaults: BTreeMap<PathBuf, String>,
}

impl WorkflowAgents {
    fn meta(&mut self, agent: &Path) -> Option<AgentMeta> {
        self.metas
            .entry(agent.to_path_buf())
            .or_insert_with(|| read_agent_meta(agent))
            .clone()
    }

    /// A record of `agent`'s transcript says `model` answered it.
    fn answered(&mut self, agent: &Path, model: &str) {
        if self.answered.get(agent).map(String::as_str) == Some(model) {
            return;
        }
        self.answered.insert(agent.to_path_buf(), model.to_owned());
        let plain = self
            .meta(agent)
            .is_some_and(|meta| meta.default_type && meta.model.is_none());
        if let (true, Some(run)) = (plain, agent.parent()) {
            self.defaults.insert(run.to_path_buf(), model.to_owned());
        }
    }

    /// What a limit of scope `scope`, met by the workflow agent whose transcript is `agent`,
    /// binds. A limit of the subscription or of a named family is what it says. A limit of
    /// "the model that runs" is of the family of the agent's model: the one its script gave
    /// it, else the one that answered it before, else, for an agent of the default type, the
    /// one that answered such agents of its run. With none of these such an agent runs the
    /// session's model (`Model(None)`). `None` when the model cannot be told (no meta file, an
    /// agent type of its own that was never answered) or is of no family with a limit of its
    /// own: such a limit is not reported.
    fn scope(&mut self, agent: &Path, scope: LimitScope) -> Option<LimitScope> {
        if scope != LimitScope::Model(None) {
            return Some(scope);
        }
        let meta = self.meta(agent)?;
        let known = meta
            .model
            .as_ref()
            .or_else(|| self.answered.get(agent))
            .or_else(|| {
                let run = agent.parent().filter(|_| meta.default_type)?;
                self.defaults.get(run)
            });
        match known {
            Some(model) => match ModelFamily::of(Some(model)) {
                ModelFamily::Any => None,
                family => Some(LimitScope::Model(Some(family))),
            },
            None => meta.default_type.then_some(LimitScope::Model(None)),
        }
    }
}

/// A limit record of the session's own thread as the application hears it.
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
                        if let Some(agent) = &side.agent {
                            self.limits.side(agent, observed);
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
        let now = Timestamp(
            i64::try_from(
                SystemTime::now()
                    .duration_since(SystemTime::UNIX_EPOCH)
                    .map_or(0, |since| since.as_millis()),
            )
            .unwrap_or(i64::MAX),
        );
        out.extend(self.limits.report(self.tail.at_end(), now));
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

    /// The limit record of a workflow's agent, as Claude Code 2.1.323 writes it (synthetic), with
    /// a reset far ahead (one that has passed is not news).
    fn refusal(at: &str) -> String {
        format!(
            r#"{{"isSidechain":true,"agentId":"a8a9","type":"assistant","timestamp":"{at}","message":{{"id":"e-{at}","model":"<synthetic>","role":"assistant","stop_reason":"stop_sequence","usage":{{"input_tokens":0,"output_tokens":0}},"content":[{{"type":"text","text":"You've hit your 5-hour limit"}}]}},"apiError":"usage_limit_reached","apiErrorParams":{{"rate_limit_info":{{"status":"rejected","resetsAt":4102444800,"rateLimitType":"five_hour"}}}},"error":"rate_limit","isApiErrorMessage":true}}"#
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

    /// The limits in `seen`: the thread's own, and those of a workflow's agents.
    fn limits_in(seen: &[Observation]) -> (usize, usize) {
        let count = |own: bool| {
            seen.iter()
                .filter(|one| match one {
                    Observation::LimitReached { .. } => own,
                    Observation::WorkflowLimitReached { .. } => !own,
                    _ => false,
                })
                .count()
        };
        (count(true), count(false))
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
                Some(Observation::WorkflowLimitReached {
                    resets_at: Some(Timestamp(4_102_444_800_000)),
                    at: Some(Timestamp(1_794_587_279_360)),
                    scope: LimitScope::Account,
                    ..
                })
            ),
            "{seen:?}"
        );
        assert_eq!(
            limits_in(&seen),
            (0, 1),
            "as the agents', not the thread's own"
        );
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
        assert_eq!(limits_in(&seen), (0, 0), "{seen:?}");

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
        assert_eq!(limits_in(&seen), (0, 0), "{seen:?}");
        assert!(sub.exists());
        Ok(())
    }

    /// SPEC-486 WFL-002: a workflow agent's limit is news only while no prompt recorded later
    /// than it was worked on, while the thread's own limit does not stand, before its reset,
    /// and once the session's transcript is read to its end (a replay judges it against all
    /// that came later).
    #[test]
    fn a_workflow_agent_s_limit_is_reported_only_while_it_is_news() {
        let agent = Path::new("/nowhere/agent-a.jsonl");
        let side_until = |ms: i64, resets: Option<i64>| Observed::LimitReached {
            resets_at: resets.map(Timestamp),
            message: "limit".into(),
            window_minutes: None,
            scope: LimitScope::Account,
            at: Some(Timestamp(ms)),
        };
        let side = |ms: i64| side_until(ms, None);
        let own = Observation::LimitReached {
            resets_at: None,
            message: "limit".into(),
            window_minutes: None,
            scope: LimitScope::Account,
            at: Some(Timestamp(5)),
        };
        let work = |ms: i64| Observation::AgentWork {
            at: Some(Timestamp(ms)),
        };
        let prompt = |ms: i64| Observation::UserTurn {
            at: Some(Timestamp(ms)),
            wake: false,
        };
        let now = Timestamp(100);
        // Held while the transcript is being caught up on, then said; the latest one.
        let mut limits = WorkflowLimits::default();
        limits.side(agent, side(10));
        limits.side(agent, side(12));
        limits.side(agent, side(11));
        assert_eq!(limits.report(false, now), None);
        assert!(matches!(
            limits.report(true, now),
            Some(Observation::WorkflowLimitReached {
                at: Some(Timestamp(12)),
                ..
            })
        ));
        assert_eq!(limits.report(true, now), None, "once");
        // More refusals of the same pause say nothing, also after the launch turn's answer
        // (which was under way) is read; a prompt that reaches the thread makes one news again.
        limits.side(agent, side(13));
        limits.thread(&work(14));
        assert_eq!(limits.report(true, now), None);
        limits.thread(&prompt(20));
        limits.side(agent, side(30));
        assert!(limits.report(true, now).is_some());

        // A prompt recorded later than the refusal and worked on: the model answered, history.
        let mut limits = WorkflowLimits::default();
        limits.side(agent, side(10));
        limits.thread(&prompt(11));
        limits.thread(&work(12));
        assert_eq!(limits.report(true, now), None);
        // That prompt refused (no work after it), or not worked on yet: still news.
        let mut limits = WorkflowLimits::default();
        limits.side(agent, side(10));
        limits.thread(&prompt(11));
        assert!(limits.report(true, now).is_some());
        // The answer of the turn that launched the workflow, written after the refusal: its
        // prompt is older than the refusal, so it changes nothing.
        let mut limits = WorkflowLimits::default();
        limits.thread(&prompt(5));
        limits.side(agent, side(10));
        limits.thread(&Observation::TurnComplete {
            at: Some(Timestamp(11)),
        });
        assert!(limits.report(true, now).is_some());
        // A sub-agent's turn end (no time) is not the thread's.
        let mut limits = WorkflowLimits::default();
        limits.side(agent, side(10));
        limits.thread(&prompt(11));
        limits.thread(&Observation::TurnComplete { at: None });
        assert!(limits.report(true, now).is_some());
        // The thread's own limit stands: that one was reported.
        let mut limits = WorkflowLimits::default();
        limits.thread(&own);
        limits.side(agent, side(10));
        assert_eq!(limits.report(true, now), None);
        // Its reset has passed (Claude Code continues the workflow by itself): not news.
        let mut limits = WorkflowLimits::default();
        limits.side(agent, side_until(10, Some(100)));
        assert_eq!(limits.report(true, now), None);
        let mut limits = WorkflowLimits::default();
        limits.side(agent, side_until(10, Some(101)));
        assert!(limits.report(true, now).is_some());
        // Something else in a workflow agent's file is nothing to report.
        let mut limits = WorkflowLimits::default();
        limits.side(agent, Observed::AgentWork { at: None });
        assert_eq!(limits.report(true, now), None);
    }

    /// SPEC-486 WFL-002: a limit of "the model that runs", met by a workflow's agent, is of that
    /// agent's model, told from what its files say. The meta files are shaped as Claude Code
    /// 2.1.323 writes them (probe runs): `model` only when the script gave the agent one; an
    /// agent type with a model of its own, and a session whose environment gives every workflow
    /// agent a model, leave no trace there, only in the records the model answered.
    #[test]
    fn a_model_s_limit_met_by_a_workflow_agent_is_of_that_agent_s_model()
    -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let unnamed = LimitScope::Model(None);
        let family = |family| Some(LimitScope::Model(Some(family)));
        // An agent's transcript path in run folder `run`, with its meta file when `meta` is one.
        let agent = |run: &str, name: &str, meta: Option<&str>| -> std::io::Result<PathBuf> {
            let folder = dir.path().join(run);
            std::fs::create_dir_all(&folder)?;
            if let Some(meta) = meta {
                std::fs::write(folder.join(format!("agent-{name}.meta.json")), meta)?;
            }
            Ok(folder.join(format!("agent-{name}.jsonl")))
        };
        const PLAIN: &str = r#"{"agentType":"workflow-subagent","isBuiltIn":true,"description":"alpha","workflowPhase":"Wave","spawnDepth":1,"pickedEffortModel":"claude-sonnet-4-5"}"#;
        const PINNED: &str = r#"{"agentType":"pinned","isBuiltIn":false,"description":"beta","workflowPhase":"Wave","spawnDepth":1,"pickedEffortModel":"claude-sonnet-4-5"}"#;

        // The default type, nothing else known: the session's model.
        let mut agents = WorkflowAgents::default();
        let plain = agent("wf_1", "a", Some(PLAIN))?;
        assert_eq!(agents.scope(&plain, unnamed), Some(unnamed));
        // A model the script gave it, as an id and as an alias (`pickedEffortModel` is the
        // session's either way).
        let opus = agent(
            "wf_1",
            "b",
            Some(
                r#"{"agentType":"workflow-subagent","model":"claude-opus-4-5","pickedEffortModel":"claude-sonnet-4-5"}"#,
            ),
        )?;
        assert_eq!(agents.scope(&opus, unnamed), family(ModelFamily::Opus));
        let alias = agent(
            "wf_1",
            "c",
            Some(r#"{"agentType":"pinned","model":"sonnet"}"#),
        )?;
        assert_eq!(agents.scope(&alias, unnamed), family(ModelFamily::Sonnet));
        // An agent type of its own: its definition may name a model, which the meta file does
        // not repeat. Never answered: cannot be told. Answered before: that model's family.
        let pinned = agent("wf_1", "d", Some(PINNED))?;
        assert_eq!(agents.scope(&pinned, unnamed), None);
        agents.answered(&pinned, "claude-opus-5-5");
        assert_eq!(agents.scope(&pinned, unnamed), family(ModelFamily::Opus));
        // Its answers say nothing of the default type's agents.
        assert_eq!(agents.scope(&plain, unnamed), Some(unnamed));
        // The default type's agents of a run answered by another model than the session's (the
        // environment gives workflow agents one): a later agent of that run, refused at its
        // first request, runs that model. Another run's agents are not told by it.
        let mut agents = WorkflowAgents::default();
        let first = agent("wf_2", "a", Some(PLAIN))?;
        let later = agent("wf_2", "b", Some(PLAIN))?;
        agents.answered(&first, "claude-opus-5-5");
        assert_eq!(agents.scope(&later, unnamed), family(ModelFamily::Opus));
        assert_eq!(agents.scope(&first, unnamed), family(ModelFamily::Opus));
        let elsewhere = agent("wf_3", "a", Some(PLAIN))?;
        assert_eq!(agents.scope(&elsewhere, unnamed), Some(unnamed));
        // A model of no family with a limit of its own: nothing to report.
        agents.answered(&elsewhere, "claude-haiku-5-5");
        assert_eq!(agents.scope(&elsewhere, unnamed), None);
        let haiku = agent(
            "wf_4",
            "a",
            Some(r#"{"agentType":"workflow-subagent","model":"haiku"}"#),
        )?;
        assert_eq!(agents.scope(&haiku, unnamed), None);
        // No meta file, one that is not one, a model that is no text, a link: cannot be told.
        let mut agents = WorkflowAgents::default();
        for (name, meta) in [("e", None), ("f", Some("[")), ("g", Some(r#"{"model":7}"#))] {
            assert_eq!(
                agents.scope(&agent("wf_5", name, meta)?, unnamed),
                None,
                "{name}"
            );
        }
        #[cfg(unix)]
        {
            let linked = agent("wf_5", "h", None)?;
            std::os::unix::fs::symlink(
                dir.path().join("wf_1").join("agent-a.meta.json"),
                dir.path().join("wf_5").join("agent-h.meta.json"),
            )?;
            assert_eq!(agents.scope(&linked, unnamed), None, "a link is not read");
        }
        // What the record itself says is kept, whatever the files say.
        let named = LimitScope::Model(Some(ModelFamily::Fable));
        for scope in [LimitScope::Account, named] {
            assert_eq!(agents.scope(&agent("wf_5", "e", None)?, scope), Some(scope));
            assert_eq!(agents.scope(&opus, scope), Some(scope));
        }
        // Through the watch's rules: an agent's answers are heard from its lines, and a limit
        // whose model cannot be told is not held.
        let credits = |ms: i64| Observed::LimitReached {
            resets_at: None,
            message: "out of usage credits".into(),
            window_minutes: None,
            scope: unnamed,
            at: Some(Timestamp(ms)),
        };
        let mut limits = WorkflowLimits::default();
        let pinned = agent("wf_6", "d", Some(PINNED))?;
        limits.side(&pinned, credits(10));
        assert_eq!(limits.report(true, Timestamp(100)), None);
        limits.side(
            &pinned,
            Observed::AssistantMessage {
                message_id: "m1".into(),
                model: Some("claude-opus-5-5".into()),
                usage: yulius_domain::usage::TokenUsage::UNKNOWN,
            },
        );
        limits.side(&pinned, credits(20));
        assert!(matches!(
            limits.report(true, Timestamp(100)),
            Some(Observation::WorkflowLimitReached {
                scope: LimitScope::Model(Some(ModelFamily::Opus)),
                ..
            })
        ));
        Ok(())
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
            disabled_plugins: Vec::new(),
        };
        launch::plan_launch(&request)?;
        Ok(())
    }

    /// SPEC-485 LSP-003 (ADR-0327): a task run and a Superagent switch off, in their own settings,
    /// the plugins of their config dir that bring a language server, though the home's
    /// `settings.json` switches them on; another plugin is left as the home has it.
    #[test]
    fn every_claude_code_launch_switches_its_language_server_plugins_off()
    -> Result<(), Box<dyn std::error::Error>> {
        use yulius_app::model::Secret;
        // A home as Claude Code's install dialog leaves it: the plugin installed, its language
        // server named in the marketplace's entry, and switched on for every session.
        let seed = |config: &Path| -> std::io::Result<()> {
            let plugins = config.join("plugins");
            let catalog = plugins.join("marketplaces/official/.claude-plugin");
            std::fs::create_dir_all(&catalog)?;
            std::fs::write(
                catalog.join("marketplace.json"),
                r#"{"name":"official","plugins":[
                    {"name":"x-lsp","lspServers":{"x":{"command":"x-language-server"}}},
                    {"name":"skills"}]}"#,
            )?;
            std::fs::write(
                plugins.join("installed_plugins.json"),
                r#"{"version":2,"plugins":{"x-lsp@official":[{"scope":"user"}],"skills@official":[{"scope":"user"}]}}"#,
            )?;
            std::fs::write(
                config.join("settings.json"),
                r#"{"enabledPlugins":{"x-lsp@official":true,"skills@official":true}}"#,
            )
        };
        // A plugin only the repository switches on, from a marketplace that is a folder of the
        // repository: found only when the lookup is given the session's own working directory
        // and the sandbox directory that folder lies in.
        let repository = |config: &Path, work: &Path| -> std::io::Result<()> {
            let market = work.join("market");
            std::fs::create_dir_all(market.join(".claude-plugin"))?;
            std::fs::write(
                market.join(".claude-plugin/marketplace.json"),
                r#"{"name":"folder","plugins":[{"name":"y-lsp","lspServers":{"y":{"command":"y-language-server"}}}]}"#,
            )?;
            std::fs::write(
                config.join("plugins/known_marketplaces.json"),
                serde_json::json!({ "folder": { "installLocation": market } }).to_string(),
            )?;
            std::fs::create_dir_all(work.join(".claude"))?;
            std::fs::write(
                work.join(".claude/settings.json"),
                r#"{"enabledPlugins":{"y-lsp@folder":true}}"#,
            )
        };
        let off = serde_json::json!({ "x-lsp@official": false, "y-lsp@folder": false });
        let dir = tempfile::tempdir()?;
        let adapter = HarnessAdapter::new(PathBuf::from("/bin/yulius"), "http://127.0.0.1:1");
        let home = dir.path().join("home");
        seed(&home.join(".claude"))?;
        let worktree = dir.path().join("worktrees").join("t1");
        repository(&home.join(".claude"), &worktree)?;
        let planned = adapter.plan(&HarnessLaunch {
            harness: Harness::ClaudeCode,
            mode: SessionMode::New {
                session_id: None,
                prompt: None,
            },
            worktree: worktree.clone(),
            sandbox_home: home.clone(),
            run_dir: dir.path().join("run"),
            model: None,
            effort: None,
            contained: false,
            mcp: Vec::new(),
            credential: Secret("token".into()),
        })?;
        let settings = planned
            .plan
            .files
            .iter()
            .find(|file| file.path.ends_with("claude-settings.json"))
            .ok_or("the run's settings file")?;
        let settings: serde_json::Value = serde_json::from_slice(&settings.contents)?;
        assert_eq!(settings["enabledPlugins"], off);
        // The home's file is the person's and Claude Code's: it stays as it was.
        let kept: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(
            home.join(".claude/settings.json"),
        )?)?;
        assert_eq!(kept["enabledPlugins"]["x-lsp@official"], true);

        // A Superagent: its config dir in the person's own folder, its working directory the
        // sandbox's repository, in another folder.
        let config = dir.path().join("users").join("u1").join(".claude");
        seed(&config)?;
        let sandbox = dir.path().join("sandboxes").join("s1");
        repository(&config, &sandbox.join("repo"))?;
        let plan = adapter.plan_cockpit(&CockpitLaunch {
            cwd: sandbox.join("repo"),
            sandbox_dir: sandbox,
            config_dir: PathBuf::from("/home/person/.claude"),
            config_dir_host: config,
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
        let settings = plan
            .argv
            .windows(2)
            .find(|pair| pair[0] == "--settings")
            .ok_or("the Superagent's --settings")?;
        let settings: serde_json::Value = serde_json::from_str(&settings[1])?;
        assert_eq!(settings["enabledPlugins"], off);
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
