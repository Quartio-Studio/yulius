//! Agents ask a person to act (SPEC-134 CDT-004, ADR-0111):
//! `needs_you {happened?, problem?, action, decision?, link?}` records one open request per task,
//! read as What happened, Problem and To do (or Decide) (SPEC-327, ADR-0221). While it is open the task is in the sidebar's Needs you
//! group ("Needs you: `<action>`") whatever the agent does meanwhile. It ends when a prompt is typed
//! into the task (a person answered, or did it and said so) or the agent calls
//! `needs_you {resolved: true}`. A request Yulius opens itself for a state of the task also ends
//! when that state does: "no subscription left" when the task runs again (SPEC-447), a goal
//! pause's when the agent works again (SPEC-469, ADR-0304), and one for a usage limit nothing
//! says the end of likewise; both also when Yulius takes the task on at a usage limit, by a
//! move past it or a wait it resumes from (SPEC-464).

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use yulius_contracts::api::cd::CdDeployAction;
use yulius_contracts::api::tasks::NeedsYouView;
use yulius_domain::ids::TaskId;
use yulius_domain::time::Timestamp;

use crate::auth::rfc3339;
use crate::context::App;
use crate::coordination::AgentContext;
use crate::error::{AppError, AppResult};
use crate::model::TaskRecord;

/// Longest `action`, in characters.
pub const MAX_ACTION_CHARS: usize = 200;
/// Longest `happened`, in characters.
pub const MAX_HAPPENED_CHARS: usize = 300;
/// Longest `problem`, in characters.
pub const MAX_PROBLEM_CHARS: usize = 500;

/// One open request (stored as JSON in `tasks.needs_you`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NeedsYouRecord {
    /// What to do, or what to decide when `decision`.
    pub action: String,
    /// What happened.
    #[serde(default)]
    pub happened: Option<String>,
    /// What is wrong or blocked because of it (`why` in rows written before SPEC-327).
    #[serde(default, alias = "why")]
    pub problem: Option<String>,
    /// The person chooses rather than does.
    #[serde(default)]
    pub decision: bool,
    /// Where (a Yulius page).
    #[serde(default)]
    pub link: Option<String>,
    /// When it was asked (ms).
    pub at: i64,
    /// The sandbox-level problem it is about ([`CAUSES`]), shared by the sandbox's tasks that
    /// name it: they show as one card and are answered together (SPEC-494 DFJ-011).
    #[serde(default)]
    pub cause: Option<String>,
    /// Whom besides the owner it waits for (SPEC-512 ANS-001).
    #[serde(default, flatten)]
    pub named: Named,
}

/// The people a request names (SPEC-512 ANS-001, ANS-005): those it still waits for, and those
/// its owner withdrew. A request that named someone, even all withdrawn since, is answered only
/// by its owner and whom it still names (ANS-002).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Named {
    /// GitHub logins it waits for besides the owner.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub who: Vec<String>,
    /// Logins the owner withdrew.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub withdrawn: Vec<String>,
}

impl Named {
    /// Whether the request names anyone, now or before a withdrawal.
    #[must_use]
    pub fn narrows(&self) -> bool {
        !self.who.is_empty() || !self.withdrawn.is_empty()
    }
}

/// The most people a request names besides the owner (SPEC-512 ANS-001).
pub const MAX_NAMED: usize = 10;

/// The audit action of a request naming the sandbox's Docker (SPEC-494 DFJ-013).
pub const DOCKER_NEEDS_YOU: &str = "task.needs_you.docker";

/// The sandbox-level causes a request may name: `docker`, the sandbox's Docker does not answer.
pub const CAUSES: &[&str] = &["docker"];

impl NeedsYouRecord {
    /// The API view.
    #[must_use]
    pub fn view(&self) -> NeedsYouView {
        NeedsYouView {
            action: self.action.clone(),
            happened: self.happened.clone(),
            problem: self.problem.clone(),
            decision: self.decision,
            link: self.link.clone(),
            at: rfc3339(Timestamp(self.at)),
            at_ms: self.at,
            cause: self.cause.clone(),
            episode: None,
            who: self.named.who.clone(),
            named: self.named.narrows(),
            browser_tab: self
                .link
                .as_deref()
                .and_then(crate::browser::browser_tab_from_link),
        }
    }

    /// The request as text for the agent to print in its session: a heading, then What
    /// happened, Problem and To do (or Decide), each on its own line when given.
    #[must_use]
    pub fn printed(&self) -> String {
        let mut text = String::from(if self.decision {
            "Needs your decision"
        } else {
            "Needs you"
        });
        for (label, part) in [
            ("What happened", self.happened.as_deref()),
            ("Problem", self.problem.as_deref()),
            (
                if self.decision { "Decide" } else { "To do" },
                Some(self.action.as_str()),
            ),
        ] {
            if let Some(part) = part {
                text.push_str(&format!("\n{label}: {part}"));
            }
        }
        text
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Args {
    #[serde(default)]
    action: Option<String>,
    #[serde(default)]
    happened: Option<String>,
    #[serde(default)]
    problem: Option<String>,
    /// The older name of `problem` (agents started before SPEC-327 keep the old schema).
    #[serde(default)]
    why: Option<String>,
    #[serde(default)]
    decision: bool,
    #[serde(default)]
    link: Option<String>,
    #[serde(default)]
    resolved: bool,
    /// Whom besides the owner it waits for (SPEC-512 ANS-001).
    #[serde(default)]
    who: Option<Vec<String>>,
    #[serde(default)]
    cause: Option<String>,
}

/// One line of at most `max` characters: control characters become spaces, characters that
/// hide or reverse text are dropped (as in a flag's reason: the words reach people and the
/// Superagent, SPEC-487 QST-001), the rest is trimmed.
fn line(text: &str, max: usize, what: &str) -> AppResult<Option<String>> {
    let clean: String = text
        .chars()
        .filter(|c| !crate::attention::hidden_format(*c))
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    let clean = clean.trim();
    if clean.chars().count() > max {
        return Err(AppError::Invalid(format!(
            "{what} is longer than {max} characters"
        )));
    }
    Ok((!clean.is_empty()).then(|| clean.to_owned()))
}

impl App {
    /// `needs_you`: records what a person should do (or ends the request with `resolved`), flags
    /// the task and tells the people watching it.
    ///
    /// # Errors
    /// `Invalid` arguments (no action, too long, a link outside Yulius).
    pub(crate) async fn tool_needs_you(
        &self,
        ctx: &AgentContext,
        args: &Value,
    ) -> AppResult<Value> {
        let args: Args = serde_json::from_value(args.clone())
            .map_err(|e| AppError::Invalid(format!("invalid arguments: {e}")))?;
        let store = &self.deps.store;
        if args.resolved {
            let cleared_browser = self
                .clear_task_browser_actions(&ctx.task.sandbox_id, &ctx.task.id, None)
                .await
                > 0;
            let ended = store
                .set_task_needs_you(&ctx.task.id, None, self.now())
                .await?
                || cleared_browser;
            if ended {
                self.publish_task(&ctx.task.id).await;
                self.needs_you_audit(ctx, "task.needs_you.resolved", "resolved by the agent")
                    .await;
            }
            return Ok(json!({
                "resolved": ended,
                "next": if ended { "The request is closed; the task no longer waits for a person." } else { "No request was open." },
            }));
        }
        let action = line(
            args.action.as_deref().unwrap_or_default(),
            MAX_ACTION_CHARS,
            "action",
        )?
        .ok_or_else(|| {
            AppError::Invalid("say what the person should do in action (one line)".into())
        })?;
        let happened = line(
            args.happened.as_deref().unwrap_or_default(),
            MAX_HAPPENED_CHARS,
            "happened",
        )?;
        let problem = match (args.problem.as_deref(), args.why.as_deref()) {
            (Some(_), Some(_)) => {
                return Err(AppError::Invalid(
                    "give the problem once, in problem (why is its older name)".into(),
                ));
            }
            (Some(text), None) => line(text, MAX_PROBLEM_CHARS, "problem")?,
            (None, Some(text)) => line(text, MAX_PROBLEM_CHARS, "why")?,
            (None, None) => None,
        };
        let link = match line(args.link.as_deref().unwrap_or_default(), 2_000, "link")? {
            None => None,
            Some(link) => {
                let base = self.config.public_url.trim_end_matches('/');
                if link != base && !link.starts_with(&format!("{base}/")) {
                    return Err(AppError::Invalid(format!(
                        "link must be a page of this Yulius ({base}/…); describe other places in action or problem"
                    )));
                }
                Some(link)
            }
        };
        let cause = match args.cause.as_deref().map(str::trim) {
            None | Some("") => None,
            Some(cause) if CAUSES.contains(&cause) => Some(cause.to_owned()),
            Some(other) => {
                return Err(AppError::Invalid(format!(
                    "cause is one of {} (a problem of the whole sandbox), not {other:?}",
                    CAUSES.join(", ")
                )));
            }
        };
        let who = self
            .named_people(&ctx.task, args.who.as_deref().unwrap_or_default())
            .await?;
        self.refuse_a_disabled_deploy(ctx, &action, &who).await?;
        let record = NeedsYouRecord {
            action: action.clone(),
            happened,
            problem,
            decision: args.decision,
            link: link.clone(),
            at: self.now().0,
            cause: cause.clone(),
            named: Named {
                who: who.clone(),
                withdrawn: Vec::new(),
            },
        };
        // Supersede any stale browser tab handoff that does not match this request (SPEC-578).
        let keep_tab = link
            .as_deref()
            .and_then(crate::browser::browser_tab_from_link)
            .and_then(|id| yulius_contracts::browser::TabId::parse(&id).ok());
        self.clear_task_browser_actions(&ctx.task.sandbox_id, &ctx.task.id, keep_tab.as_ref())
            .await;
        // The request itself puts the task in Needs you (`AttentionFacts`, `taskInfo`), whatever the
        // agent does meanwhile; its activity is left to its turns.
        store
            .set_task_needs_you(&ctx.task.id, Some(&record), Timestamp(record.at))
            .await?;
        self.publish_task(&ctx.task.id).await;
        self.needs_you_audit(ctx, "task.needs_you", &action).await;
        if cause.as_deref() == Some("docker") {
            // Counted per week on the instance (SPEC-494 DFJ-013).
            self.needs_you_audit(ctx, DOCKER_NEEDS_YOU, &action).await;
        }
        let next = match cause.as_deref() {
            Some(cause) => format!(
                "People see the tasks of this sandbox that name cause {cause} under one shared \"Needs you\" card; a person's answer to one of them, the end of the sandbox's Docker trouble, or a restart of its Docker asked through the Superagent closes yours too, and you are told so here. Print the request in the session as it is; go on with other work."
            ),
            None => "People see \"Needs you\" on this task. Print the request (print, and the link) in the session as it is so whoever opens it knows what to do; go on with other work. It closes when someone writes to this task, or call needs_you with resolved: true once you see it done.".to_owned(),
        };
        Ok(json!({
            "asked": action,
            "who": who,
            "print": record.printed(),
            "link": link,
            "cause": cause,
            "next": next,
        }))
    }

    /// Refuses a card whose to-do sends the task's owner to a CD page Deploy button that is
    /// disabled for them (SPEC-650 PST-007), unless a person the card names (`who`) may press it.
    /// It reads the decision the page renders (`CdEnvironment.deploy_action`, for each of them),
    /// and says what works for the reason the button is disabled. A page that cannot be read now
    /// leaves the card as it is.
    ///
    /// # Errors
    /// `Invalid` with the button, why it is disabled and what works.
    async fn refuse_a_disabled_deploy(
        &self,
        ctx: &AgentContext,
        action: &str,
        who: &[String],
    ) -> AppResult<()> {
        if !action.contains("Deploy ") {
            return Ok(());
        }
        let Some(owner) = self.deps.store.user(&ctx.task.owner).await? else {
            return Ok(());
        };
        let Some(cd) = self.deploy_buttons_of(&owner, &ctx.task.sandbox_id).await else {
            return Ok(());
        };
        let Some(env) = cd.environments.iter().find(|env| {
            !env.deploy_action.enabled
                && names_button(action, env.deploy_action.label.trim_end_matches('…'))
        }) else {
            return Ok(());
        };
        // A named person may press it for the owner; otherwise the refusal gives the reason of
        // a named person whose button is disabled for more than access (review round 2 of PR 3).
        let access = [CdDeployAction::ADMINISTRATOR, CdDeployAction::WRITE];
        let mut blocked = (owner.label(), env.deploy_action.clone());
        for login in who {
            let Some(user) = self.deps.store.user_by_login(login).await? else {
                continue;
            };
            let Some(theirs) = self
                .deploy_buttons_of(&user, &ctx.task.sandbox_id)
                .await
                .and_then(|cd| cd.environments.into_iter().find(|e| e.name == env.name))
            else {
                continue;
            };
            if theirs.deploy_action.enabled {
                return Ok(());
            }
            if !access.contains(&theirs.deploy_action.reason.as_deref().unwrap_or_default())
                && access.contains(&blocked.1.reason.as_deref().unwrap_or_default())
            {
                blocked = (user.label(), theirs.deploy_action);
            }
        }
        let (whose, button) = blocked;
        let works = match button.reason.as_deref() {
            Some(CdDeployAction::ADMINISTRATOR) => "an administrator of this project deploys it: say that one does and name them in `who`, without asking the owner to press it".to_owned(),
            Some(CdDeployAction::NOTHING) => format!(
                "[cd.{}] has nothing to deploy yet: its table in yulius.toml comes first",
                env.name
            ),
            Some(CdDeployAction::RUNNING) => "a deploy of it is running: wait for it (`yulius wait`), then say how it ended".to_owned(),
            _ => "name a person with write access in `who`, or say what they should decide".to_owned(),
        };
        Err(AppError::Invalid(format!(
            "nothing was asked: this card tells {}{} to press {} on the CD page, which is unavailable to {whose} ({}). Say instead what works: {works}",
            owner.label(),
            if who.is_empty() {
                String::new()
            } else {
                format!(" (and {})", who.join(", "))
            },
            button.label,
            button.why.as_deref().unwrap_or("disabled"),
        )))
    }

    /// The CD page as `user` reads it, with its Deploy buttons; `None` when it cannot be read.
    async fn deploy_buttons_of(
        &self,
        user: &crate::model::UserRecord,
        sandbox: &yulius_domain::ids::SandboxId,
    ) -> Option<yulius_contracts::api::cd::CdOverview> {
        let person = crate::auth::AuthContext {
            user: user.clone(),
            csrf_token: None,
            session_hash: None,
            token: None,
        };
        self.cd_overview(&person, sandbox.as_str()).await.ok()
    }

    /// The logins `asked` names, cleaned (`@`, case, the owner and repeats dropped), each a person
    /// of the instance with write access to `task`'s project (SPEC-512 ANS-001).
    ///
    /// # Errors
    /// `Invalid` for more than [`MAX_NAMED`] names, or one who is no person of the instance or
    /// may not write to the project.
    async fn named_people(&self, task: &TaskRecord, asked: &[String]) -> AppResult<Vec<String>> {
        let owner = self.login_of(&task.owner).await.to_lowercase();
        let mut named: Vec<String> = Vec::new();
        for login in asked {
            let login = login.trim().trim_start_matches('@').to_lowercase();
            if login.is_empty() || login == owner || named.contains(&login) {
                continue;
            }
            named.push(login);
        }
        if named.len() > MAX_NAMED {
            return Err(AppError::Invalid(format!(
                "who names at most {MAX_NAMED} people besides the task's owner"
            )));
        }
        // Another person's words never reach a run holding the owner's personal secrets or MCP
        // servers (ADR-0370): nobody named could answer.
        if !named.is_empty() && self.reaches_owners_personal(task).await? {
            return Err(AppError::Invalid(format!(
                "who: this task holds @{owner}'s personal secrets or MCP servers, so only \
                 @{owner} can answer it; ask without who (nothing was asked)"
            )));
        }
        for login in &named {
            let user = self.deps.store.user_by_login(login).await?;
            let writes = match &user {
                Some(user) if user.disabled_at.is_none() => {
                    // No access is a refusal; a GitHub that did not answer is no verdict.
                    match self
                        .require(
                            user,
                            &task.sandbox_id,
                            yulius_domain::permission::Action::Work,
                        )
                        .await
                    {
                        Ok(_) => true,
                        Err(AppError::NotFound | AppError::Forbidden(_)) => false,
                        // No usable GitHub sign-in: their access is not known until they sign
                        // in to Yulius again, which no wait changes.
                        Err(AppError::Unauthenticated) => {
                            return Err(AppError::Invalid(format!(
                                "who: @{login} must sign in to Yulius again before their access \
                                 to this project can be checked (nothing was asked)"
                            )));
                        }
                        Err(err) => {
                            return Err(AppError::Upstream(format!(
                                "who: @{login}'s access to this project could not be checked \
                                 now ({}); ask again in a minute (nothing was asked)",
                                err.public_message()
                            )));
                        }
                    }
                }
                _ => false,
            };
            if !writes {
                let why = if user.is_none() {
                    "is no person of this Yulius"
                } else {
                    "has no write access to this project"
                };
                return Err(AppError::Invalid(format!(
                    "who: @{login} {why}, so they could never answer; name people who have \
                     write access to the project, or leave them out (nothing was asked)"
                )));
            }
        }
        Ok(named)
    }

    async fn needs_you_audit(&self, ctx: &AgentContext, action: &str, detail: &str) {
        if let Err(err) = self
            .deps
            .store
            .audit(
                self.now(),
                &ctx.actor(),
                action,
                Some(ctx.task.id.as_str()),
                Some(detail),
            )
            .await
        {
            tracing::warn!(error = %err, task = %ctx.task.id, "needs_you audit failed");
        }
    }

    /// A prompt typed into the task ends its request: a person answered. A request about a
    /// sandbox-level cause, shown on a live task, is answered for every task of the sandbox that
    /// asked it before (one card, SPEC-494 DFJ-011); those tasks are told so. A resting task's
    /// stored request (a Continue) answers no other.
    pub(crate) async fn end_needs_you(&self, task: &TaskId) {
        let shared = match self.deps.store.task_needs_you(task).await {
            Ok(Some(request)) => request.cause,
            _ => None,
        };
        let cleared_browser = if let Ok(Some(record)) = self.deps.store.task(task).await {
            self.clear_task_browser_actions(&record.sandbox_id, task, None)
                .await
                > 0
        } else {
            false
        };
        let ended = self.end_one_needs_you(task).await;
        if !ended && cleared_browser {
            self.publish_task(task).await;
        }
        if let Some(cause) = shared {
            self.answer_shared_cause(task, &cause).await;
        }
    }

    /// The other tasks of `task`'s sandbox that asked about `cause` before now are answered by a
    /// person's answer to `task`, a live one (SPEC-494 DFJ-011), and told so.
    async fn answer_shared_cause(&self, task: &TaskId, cause: &str) {
        if let Ok(Some(record)) = self.deps.store.task(task).await
            && record.status == yulius_domain::task::TaskStatus::Running
        {
            let source = format!("a person's answer to task #{}", record.number);
            let others = self
                .end_shared_needs_you(&record.sandbox_id, cause, self.now().0, &source)
                .await;
            if cause == "docker" {
                self.tell_docker_answered(record.sandbox_id.as_str(), others);
            }
        }
    }

    /// [`Self::end_needs_you`] for the request asked at `asked_at` only (SPEC-512 ANS-004): one
    /// asked after it stays open, and only an ended request answers a shared cause's others.
    pub(crate) async fn end_needs_you_asked_at(
        &self,
        task: &TaskId,
        asked_at: i64,
        cause: Option<String>,
    ) {
        match self
            .deps
            .store
            .end_task_needs_you_at(task, asked_at, self.now())
            .await
        {
            Ok(true) => {
                if let Ok(Some(record)) = self.deps.store.task(task).await {
                    self.clear_task_browser_actions(&record.sandbox_id, task, None)
                        .await;
                }
                self.publish_task(task).await;
            }
            Ok(false) => return,
            Err(err) => {
                tracing::warn!(task = %task, error = %err, "the answered request was not ended");
                return;
            }
        }
        if let Some(cause) = cause {
            self.answer_shared_cause(task, &cause).await;
        }
    }

    async fn end_one_needs_you(&self, task: &TaskId) -> bool {
        match self
            .deps
            .store
            .set_task_needs_you(task, None, self.now())
            .await
        {
            Ok(true) => {
                self.publish_task(task).await;
                true
            }
            Ok(false) => false,
            Err(err) => {
                tracing::warn!(task = %task, error = %err, "ending a needs_you request failed");
                false
            }
        }
    }

    /// When the episode of `task`'s request about `cause` (asked at `at`) began: the oldest open
    /// request naming it among the sandbox's tasks that do not rest (an ended task's stored
    /// request is not shown, SPEC-494 DFJ-011).
    pub(crate) async fn shared_episode(&self, task: &TaskId, cause: &str, at: i64) -> i64 {
        let Ok(Some(record)) = self.deps.store.task(task).await else {
            return at;
        };
        let Ok(others) = self.deps.store.needs_you_tasks(&record.sandbox_id).await else {
            return at;
        };
        let mut oldest = at;
        for other in others {
            // A stopping task shows its request no more than a stopped one (review round 3).
            let resting = match self.deps.store.task(&other).await {
                Ok(Some(t)) => {
                    t.status.is_resting() || t.status == yulius_domain::task::TaskStatus::Stopping
                }
                _ => true,
            };
            if resting {
                continue;
            }
            if let Ok(Some(request)) = self.deps.store.task_needs_you(&other).await
                && request.cause.as_deref() == Some(cause)
            {
                oldest = oldest.min(request.at);
            }
        }
        oldest
    }

    /// Ends the requests of `sandbox`'s tasks that name `cause` and were asked by
    /// `asked_until_ms` (a person answered the shared card, or Yulius repaired the cause); a
    /// request replaced meanwhile stays (`end_task_needs_you_asking`). Returns the tasks whose
    /// request ended.
    pub(crate) async fn end_shared_needs_you(
        &self,
        sandbox: &yulius_domain::ids::SandboxId,
        cause: &str,
        asked_until_ms: i64,
        source: &str,
    ) -> Vec<TaskId> {
        let Ok(tasks) = self.deps.store.needs_you_tasks(sandbox).await else {
            return Vec::new();
        };
        let mut ended = Vec::new();
        for task in tasks {
            let Ok(Some(request)) = self.deps.store.task_needs_you(&task).await else {
                continue;
            };
            if request.cause.as_deref() != Some(cause) || request.at > asked_until_ms {
                continue;
            }
            match self
                .deps
                .store
                .end_task_needs_you_asking(&task, &request.action, self.now())
                .await
            {
                Ok(true) => {
                    self.publish_task(&task).await;
                    // Ended for another task's answer or a repair: said in the audit log, as a
                    // person's dismissal is.
                    if let Err(err) = self
                        .deps
                        .store
                        .audit(
                            self.now(),
                            "yulius",
                            "task.needs_you.answered_shared",
                            Some(task.as_str()),
                            Some(&format!("{cause}: {source}")),
                        )
                        .await
                    {
                        tracing::warn!(task = %task, error = %err, "auditing a shared answer failed");
                    }
                    ended.push(task);
                }
                Ok(false) => {}
                Err(err) => {
                    tracing::warn!(task = %task, error = %err, "ending a shared needs_you request failed");
                }
            }
        }
        if !ended.is_empty() {
            tracing::info!(sandbox = %sandbox, cause, tasks = ended.len(), "a shared Needs you was answered");
        }
        ended
    }

    /// The task's open request, for its summary and MCP projections. Falls back to an active
    /// in-memory browser handoff (and any legacy browser `tasks.attention` timestamp/link) when
    /// `tasks.needs_you` is absent (SPEC-578).
    pub(crate) async fn needs_you_view(&self, task: &TaskRecord) -> Option<NeedsYouView> {
        match self.deps.store.task_needs_you(&task.id).await {
            Ok(Some(request)) => {
                let mut view = request.view();
                view.action = self
                    .redact_notice_text(&task.sandbox_id, &view.action)
                    .await;
                if let Some(happened) = &view.happened {
                    view.happened = Some(self.redact_notice_text(&task.sandbox_id, happened).await);
                }
                if let Some(problem) = &view.problem {
                    view.problem = Some(self.redact_notice_text(&task.sandbox_id, problem).await);
                }
                if let Some(cause) = &request.cause {
                    view.episode = Some(rfc3339(Timestamp(
                        self.shared_episode(&task.id, cause, request.at).await,
                    )));
                }
                return Some(view);
            }
            Ok(None) => {}
            Err(err) => {
                tracing::warn!(task = %task.id, error = %err, "reading a needs_you request failed");
                return None;
            }
        }
        // Legacy browser handoff: tab still requests user action even though `tasks.needs_you`
        // was never written (or before SPEC-578) and `activity` may have moved away from
        // `needs_attention`.
        let (tab_id, reason) = self
            .browser_tab_request_for(&task.sandbox_id, &task.id)
            .await?;
        let stored_attn = self
            .deps
            .store
            .stored_task_attention(&task.id)
            .await
            .ok()
            .flatten();
        let at_ms = stored_attn
            .as_ref()
            .map_or(task.updated_at.0, |(_, at)| at.0);
        let stored_link = stored_attn
            .as_ref()
            .and_then(|(attn, _)| attn.link.clone())
            .filter(|link| crate::browser::browser_tab_from_link(link).as_deref() == Some(&tab_id));
        let link = match stored_link {
            Some(link) => Some(link),
            None => self
                .deps
                .store
                .sandbox(&task.sandbox_id)
                .await
                .ok()
                .flatten()
                .and_then(|sandbox| {
                    yulius_contracts::browser::TabId::parse(&tab_id)
                        .ok()
                        .map(|id| self.browser_tab_link(&sandbox.slug, &id))
                }),
        };
        let happened = crate::attention::shown_line(
            &format!("The agent asked you to act in the sandbox browser: {reason}"),
            MAX_HAPPENED_CHARS,
        );
        let action = crate::attention::shown_line(
            &format!("Open the browser tab: {reason}"),
            MAX_ACTION_CHARS,
        )
        .unwrap_or_else(|| "Open the browser tab and complete the requested action.".to_owned());
        Some(NeedsYouView {
            action,
            happened,
            problem: None,
            decision: false,
            link,
            at: rfc3339(Timestamp(at_ms)),
            at_ms,
            cause: None,
            episode: None,
            who: Vec::new(),
            named: false,
            browser_tab: Some(tab_id),
        })
    }
}

/// Whether `text` names the button `label` ("Deploy dev"), not a longer word ("Deploy devices").
fn names_button(text: &str, label: &str) -> bool {
    text.match_indices(label).any(|(at, _)| {
        text[at + label.len()..]
            .chars()
            .next()
            .is_none_or(|c| !(c.is_alphanumeric() || c == '-' || c == '_'))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_button_is_named_by_its_whole_label() {
        assert!(super::names_button(
            "Press Deploy dev on the CD page",
            "Deploy dev"
        ));
        assert!(super::names_button("press “Deploy dev”.", "Deploy dev"));
        assert!(super::names_button("Deploy dev", "Deploy dev"));
        assert!(!super::names_button("Deploy devices first", "Deploy dev"));
        assert!(!super::names_button("Deploy dev-tools", "Deploy dev"));
        assert!(
            !super::names_button("deploy dev", "Deploy dev"),
            "a button's own capital"
        );
    }

    #[test]
    fn requests_are_single_bounded_lines() {
        assert_eq!(
            line("  Add DEV_SSH_KEY\non Secrets ", 200, "action")
                .ok()
                .flatten(),
            Some("Add DEV_SSH_KEY on Secrets".to_owned())
        );
        assert_eq!(line("   ", 200, "action").ok().flatten(), None);
        assert!(line(&"a".repeat(201), 200, "action").is_err());
        // What hides or reverses text is dropped (SPEC-487 QST-001): the request reaches the
        // Superagent in tasks_list.
        assert_eq!(
            line("Approve\u{202e} the \u{200b}budget\u{e0041}", 200, "action")
                .ok()
                .flatten(),
            Some("Approve the budget".to_owned())
        );
    }

    #[test]
    fn a_row_stored_with_why_reads_it_as_the_problem() -> Result<(), serde_json::Error> {
        let record: NeedsYouRecord =
            serde_json::from_str(r#"{"action":"Add DEV_SSH_KEY","why":"SSH needs it","at":1}"#)?;
        assert_eq!(record.problem.as_deref(), Some("SSH needs it"));
        assert_eq!(record.happened, None);
        assert!(!record.decision);
        assert_eq!(
            record.printed(),
            "Needs you\nProblem: SSH needs it\nTo do: Add DEV_SSH_KEY"
        );
        Ok(())
    }
}
