//! What a task asks of a person, read and answered for them (SPEC-487 slice 4, QST-001 to
//! QST-005; wall 7): why a task waits for a person, and two decisions for the person: deciding
//! a prompt Yulius held (the card on the task's page) and dismissing the task's request to its
//! person (`needs_you`, its agent's or one Yulius opened for it), which no page did: it ended
//! only with a message to the task or by its agent.
//!
//! Contract:
//! - what a task says about itself (`App::waits_why`) is what its page shows: the list's
//!   group, the task's request (`needs_you`), the reason of its flag, a held prompt, a pause,
//!   and what a queued task waits for (`queue_reason`, SPEC-512 PRK-007).
//!   It is read by everyone who reads the task list, a task's agent included (without a held
//!   prompt's words); the texts are an agent's, Claude Code's or a person's own words, each
//!   shown as one bounded line without what hides or reverses text;
//! - the decisions are the task owner's: a person's function with their [`AuthContext`],
//!   write access checked at the call;
//! - a request is ended only while it still asks what was read: one asked meanwhile stays.

use std::sync::Arc;

use serde_json::{Value, json};
use yulius_contracts::api::intake::{HeldPromptOutcome, ResolveHeldPrompt};
use yulius_contracts::api::tasks::{NeedsYouNoticeView, WaitHistoryEntry};
use yulius_domain::ci::RunState;
use yulius_domain::permission::Action;
use yulius_domain::task::{Activity, AttentionFacts, PullSignal, TaskGroup, TaskStatus};
use yulius_domain::time::Timestamp;

use crate::auth::{AuthContext, rfc3339};
use crate::context::App;
use crate::error::{AppError, AppResult};
use crate::model::TaskRecord;
use crate::ports::store_ext::WaitMessageRecord;

/// Most wait-history entries returned on a task's detail or `task_get`.
const MAX_WAIT_HISTORY: u32 = 50;

/// Stable short hash for synthetic wait-episode keys.
fn episode_hash(text: &str) -> String {
    let hash = text.bytes().fold(0xcbf2_9ce4_8422_2325_u64, |hash, byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3)
    });
    format!("{hash:016x}")
}

/// The most characters of a held prompt a task's brief carries.
const HELD_EXCERPT_CHARS: usize = 300;

/// The most characters of each text of a task's request a brief carries (they are stored
/// within 2 000).
const SHOWN_TEXT_CHARS: usize = 2_000;

/// The decisions on a held prompt an agent makes for its person (the page has two more:
/// `skip_after`, which names steps, and `revert`, which discards the task's work).
pub const HELD_PROMPT_ACTIONS: [&str; 4] = ["send", "dismiss", "parallel", "after"];

/// The list's group of a task, as tools name it.
#[must_use]
pub fn group_name(group: TaskGroup) -> &'static str {
    match group {
        TaskGroup::NeedsYou => "needs_you",
        TaskGroup::Working => "working",
        TaskGroup::Queued => "queued",
        TaskGroup::Done => "done",
        TaskGroup::Ended => "ended",
    }
}

/// Explains a Needs you classification that has no stored question, flag, or held prompt.
fn derived_wait_reason(facts: &AttentionFacts) -> Option<&'static str> {
    if facts.browser_request {
        Some("A request for a person was pending when the task list was read.")
    } else if facts.held_prompt {
        Some("A prompt was held for a decision when the task was checked.")
    } else if facts.pull == Some(PullSignal::Failing) {
        Some("The agent stopped and its pull request checks are failing.")
    } else if facts.plan_open {
        Some("The agent stopped with unfinished steps in its plan.")
    } else if facts.spec_unfinished {
        Some("The agent stopped before its spec was finished.")
    } else if facts.goal_unfinished {
        Some("The agent stopped before its goal was achieved.")
    } else if facts.status == yulius_domain::task::TaskStatus::Parked {
        match facts.pull {
            Some(PullSignal::InReview) => {
                Some("The parked task is waiting for its pull request checks.")
            }
            Some(PullSignal::Errored) => {
                Some("The parked task's pull request could not be verified.")
            }
            Some(PullSignal::Behind) => {
                Some("The parked task's pull request needs to be updated with its base branch.")
            }
            Some(PullSignal::Conflicting) => {
                Some("The parked task's pull request has a merge conflict.")
            }
            _ => None,
        }
    } else {
        None
    }
}

impl App {
    /// Redacts both sandbox vault secrets and generic credential shapes (SPEC-578).
    pub(crate) async fn redact_notice_text(
        &self,
        sandbox: &yulius_domain::ids::SandboxId,
        text: &str,
    ) -> String {
        let first = self.redact_for_sandbox(sandbox, text).await;
        yulius_domain::redact::redact(&first).map_or(first, |r| r.text)
    }

    /// Canonical task page link (`{public_url}/s/{slug}/t/{number}`).
    pub(crate) async fn task_page_link(&self, task: &TaskRecord) -> (Option<String>, String) {
        let base = self.config.public_url.trim_end_matches('/');
        let slug = self
            .deps
            .store
            .sandbox(&task.sandbox_id)
            .await
            .ok()
            .flatten()
            .map(|sandbox| sandbox.slug);
        let link = match &slug {
            Some(slug) => format!("{base}/s/{slug}/t/{}", task.number),
            None => format!("{base}/t/{}", task.number),
        };
        (slug, link)
    }

    /// Resolves the actionable Needs You notice for `task` under `facts` (SPEC-578): what
    /// happened, what a person needs to do, and where to do it, redacted and single-line
    /// bounded. Returns `None` whenever `facts.group() != TaskGroup::NeedsYou`.
    ///
    /// Never includes a held prompt's typed excerpt so it is safe across all callers
    /// (including task agents).
    pub(crate) async fn resolve_needs_you_notice(
        &self,
        task: &TaskRecord,
        facts: &AttentionFacts,
    ) -> Option<NeedsYouNoticeView> {
        self.resolve_needs_you_notice_with_pr(task, facts, None)
            .await
    }

    /// Like [`Self::resolve_needs_you_notice`], with an optional verified pull request number
    /// (e.g. from `parked_status`) used when `observed_pulls` and `delivery` lack it (SPEC-578).
    #[allow(clippy::too_many_lines)]
    pub(crate) async fn resolve_needs_you_notice_with_pr(
        &self,
        task: &TaskRecord,
        facts: &AttentionFacts,
        pr_hint: Option<u64>,
    ) -> Option<NeedsYouNoticeView> {
        if facts.group() != TaskGroup::NeedsYou {
            return None;
        }
        let store = &self.deps.store;
        let (slug, task_link) = self.task_page_link(task).await;
        let episode_at_ms = store
            .task_times(&task.id)
            .await
            .ok()
            .flatten()
            .map_or(task.updated_at.0, |times| times.state_at.0);

        let raw = if facts.held_prompt {
            if let Ok(Some(held)) = store.pending_hold(&task.id).await {
                NeedsYouNoticeView {
                    source: "system".to_owned(),
                    episode: format!("held:{}", held.id),
                    happened: format!(
                        "A typed prompt ({}) was held by the scope check.",
                        held.class.as_str()
                    ),
                    problem: None,
                    action: "Open the task and decide whether to send, dismiss, run in parallel, or queue it after.".to_owned(),
                    link: task_link,
                    decision: true,
                    browser_tab: None,
                    at: rfc3339(held.created_at),
                    at_ms: held.created_at.0,
                }
            } else {
                NeedsYouNoticeView {
                    source: "system".to_owned(),
                    episode: format!("held:checked:{episode_at_ms}"),
                    happened: "A prompt was held for a decision when the task was checked."
                        .to_owned(),
                    problem: None,
                    action: "Open the task to review or send your prompt.".to_owned(),
                    link: task_link,
                    decision: true,
                    browser_tab: None,
                    at: rfc3339(Timestamp(episode_at_ms)),
                    at_ms: episode_at_ms,
                }
            }
        } else if let Some(req) = self.needs_you_view(task).await {
            let is_browser = req.browser_tab.is_some();
            let episode = match &req.browser_tab {
                Some(tab) => format!("browser:{tab}:{}", req.at_ms),
                None => format!("needs_you:{}", req.at_ms),
            };
            let happened = req.happened.clone().unwrap_or_else(|| {
                if req.decision {
                    "The agent asked for your decision.".to_owned()
                } else {
                    "The agent asked for your help.".to_owned()
                }
            });
            NeedsYouNoticeView {
                source: if is_browser { "browser" } else { "agent" }.to_owned(),
                episode,
                happened,
                problem: req.problem,
                action: req.action,
                link: req.link.unwrap_or(task_link),
                decision: req.decision,
                browser_tab: req.browser_tab,
                at: req.at,
                at_ms: req.at_ms,
            }
        } else if let Some(flag) = self.attention_view(task).await {
            let stored_at = store
                .stored_task_attention(&task.id)
                .await
                .ok()
                .flatten()
                .map_or(episode_at_ms, |(_, at)| at.0);
            let is_terminal = flag.kind == "terminal";
            let browser_tab = flag
                .link
                .as_deref()
                .and_then(crate::browser::browser_tab_from_link);
            let source = if browser_tab.is_some() {
                "browser"
            } else if is_terminal {
                "terminal"
            } else {
                "system"
            };
            let happened = match (&flag.problem, is_terminal) {
                (Some(problem), true) => format!("The agent asks in its terminal: {problem}"),
                (Some(problem), false) => problem.clone(),
                (None, true) => {
                    "The agent asked a question or permission prompt in its terminal.".to_owned()
                }
                (None, false) => "Yulius flagged this task for your attention.".to_owned(),
            };
            let action = flag.action.unwrap_or_else(|| {
                if is_terminal {
                    "Open the task terminal and answer the prompt.".to_owned()
                } else {
                    "Open the task and handle the flagged issue.".to_owned()
                }
            });
            NeedsYouNoticeView {
                source: source.to_owned(),
                episode: format!("attention:{}:{stored_at}", flag.kind),
                happened,
                problem: None,
                action,
                link: flag.link.unwrap_or(task_link),
                decision: false,
                browser_tab,
                at: rfc3339(Timestamp(stored_at)),
                at_ms: stored_at,
            }
        } else if facts.chain_blocked {
            let reason = store
                .chain(&task.id)
                .await
                .ok()
                .flatten()
                .and_then(|c| c.blocked_reason)
                .unwrap_or_else(|| "its predecessor did not finish".to_owned());
            NeedsYouNoticeView {
                source: "system".to_owned(),
                episode: format!("chain:{}:{episode_at_ms}", episode_hash(&reason)),
                happened: format!("This chained task cannot start automatically: {reason}."),
                problem: None,
                action: "Open the task to start it manually or adjust its dependency.".to_owned(),
                link: task_link,
                decision: false,
                browser_tab: None,
                at: rfc3339(Timestamp(episode_at_ms)),
                at_ms: episode_at_ms,
            }
        } else if facts.pull == Some(PullSignal::Failing) {
            let pr_num = pr_hint
                .or_else(|| {
                    self.observed_pulls
                        .known_pull(&task.sandbox_id, &task.id)
                        .flatten()
                        .map(|obs| obs.number)
                })
                .or(match store.delivery(&task.id).await.ok().flatten() {
                    Some(rec) => rec.pr_number,
                    None => None,
                });
            let verb = if task.status == TaskStatus::Parked {
                "The task is parked and"
            } else {
                "The agent stopped and"
            };
            if let Some(pr) = pr_num {
                if self.config.ci.enabled
                    && let Some(run) = self.latest_pr_ci_run(&task.sandbox_id, pr).await
                    && matches!(run.state, RunState::Failed | RunState::Error)
                {
                    let jobs = store.ci_jobs(&run.id, true).await.unwrap_or_default();
                    let failed = crate::pr_watch::failed_jobs(&jobs);
                    let ci_link = match &slug {
                        Some(slug) => format!(
                            "{}/s/{slug}/ci?run={}",
                            self.config.public_url.trim_end_matches('/'),
                            run.id
                        ),
                        None => task_link.clone(),
                    };
                    let at_ms = run.ended_at.unwrap_or(run.queued_at).0.max(episode_at_ms);
                    let action = if task.status == TaskStatus::Parked {
                        format!("Inspect CI run {} and resume the task or rerun CI.", run.id)
                    } else {
                        format!(
                            "Inspect CI run {} and reply to the agent or rerun CI.",
                            run.id
                        )
                    };
                    NeedsYouNoticeView {
                        source: "system".to_owned(),
                        episode: format!("pr_fail:{pr}:{}:{episode_at_ms}", run.id),
                        happened: format!(
                            "{verb} pull request #{pr} CI run {} failed: {failed}.",
                            run.id
                        ),
                        problem: None,
                        action,
                        link: ci_link,
                        decision: false,
                        browser_tab: None,
                        at: rfc3339(Timestamp(at_ms)),
                        at_ms,
                    }
                } else {
                    let action = if task.status == TaskStatus::Parked {
                        format!("Review pull request #{pr} checks and resume the task to fix them.")
                    } else {
                        format!(
                            "Review pull request #{pr} checks and reply to the agent to fix them."
                        )
                    };
                    NeedsYouNoticeView {
                        source: "system".to_owned(),
                        episode: format!("pr_fail:{pr}:{episode_at_ms}"),
                        happened: format!("{verb} pull request #{pr} checks are failing."),
                        problem: None,
                        action,
                        link: task_link,
                        decision: false,
                        browser_tab: None,
                        at: rfc3339(Timestamp(episode_at_ms)),
                        at_ms: episode_at_ms,
                    }
                }
            } else {
                NeedsYouNoticeView {
                    source: "system".to_owned(),
                    episode: format!("pr_fail:unknown:{episode_at_ms}"),
                    happened: format!("{verb} its pull request checks are failing."),
                    problem: None,
                    action: "Open the task to review the failing pull request checks and reply or resume."
                        .to_owned(),
                    link: task_link,
                    decision: false,
                    browser_tab: None,
                    at: rfc3339(Timestamp(episode_at_ms)),
                    at_ms: episode_at_ms,
                }
            }
        } else if facts.plan_open {
            let steps = store.steps(&task.id).await.unwrap_or_default();
            let open: Vec<&crate::model::StepRecord> = steps
                .iter()
                .filter(|step| !matches!(step.status.as_str(), "done" | "skipped"))
                .collect();
            let open_count = open.len();
            let next_step = open
                .iter()
                .find(|s| s.status == "in_progress")
                .or_else(|| open.iter().find(|s| s.status == "blocked"))
                .or_else(|| open.first())
                .copied();
            let verb = if task.status == TaskStatus::Parked {
                "The task is parked with"
            } else {
                "The agent stopped with"
            };
            let happened = match next_step {
                Some(next) => format!(
                    "{verb} unfinished steps in its plan ({open_count} open, next: \"{}\").",
                    next.title
                ),
                None => format!("{verb} unfinished steps in its plan ({open_count} open)."),
            };
            let blocked = next_step.filter(|_| open.iter().all(|step| step.status == "blocked"));
            let action = if blocked.is_some() {
                "Resolve the recorded dependency so the agent can finish this step."
            } else if task.status == TaskStatus::Parked {
                "Open the task and resume it to continue the remaining plan steps, or close it if done."
            } else {
                "Open the task and tell the agent to continue the remaining plan steps, or close it if done."
            };
            NeedsYouNoticeView {
                source: "system".to_owned(),
                episode: format!("plan_open:{open_count}:{episode_at_ms}"),
                happened,
                problem: blocked.and_then(|step| step.note.clone()),
                action: action.to_owned(),
                link: task_link,
                decision: false,
                browser_tab: None,
                at: rfc3339(Timestamp(episode_at_ms)),
                at_ms: episode_at_ms,
            }
        } else if facts.spec_unfinished {
            let spec_name = task.spec_path.as_deref().unwrap_or("its spec");
            let verb = if task.status == TaskStatus::Parked {
                "The task is parked before"
            } else {
                "The agent stopped before"
            };
            let action = if task.status == TaskStatus::Parked {
                format!("Open the task and resume it to finish {spec_name} or mark it done.")
            } else {
                format!("Open the task and ask the agent to finish {spec_name} or mark it done.")
            };
            NeedsYouNoticeView {
                source: "system".to_owned(),
                episode: format!(
                    "spec_unfinished:{}:{episode_at_ms}",
                    episode_hash(spec_name)
                ),
                happened: format!("{verb} {spec_name} was marked done."),
                problem: None,
                action,
                link: task_link,
                decision: false,
                browser_tab: None,
                at: rfc3339(Timestamp(episode_at_ms)),
                at_ms: episode_at_ms,
            }
        } else if facts.goal_unfinished {
            let verb = if task.status == TaskStatus::Parked {
                "The task is parked before"
            } else {
                "The agent stopped before"
            };
            let action = if task.status == TaskStatus::Parked {
                "Open the task and resume it to continue toward the goal, or close it."
            } else {
                "Open the task and reply to guide the agent toward the goal, or close it."
            };
            NeedsYouNoticeView {
                source: "system".to_owned(),
                episode: format!("goal_unfinished:{episode_at_ms}"),
                happened: format!("{verb} its goal was achieved."),
                problem: None,
                action: action.to_owned(),
                link: task_link,
                decision: false,
                browser_tab: None,
                at: rfc3339(Timestamp(episode_at_ms)),
                at_ms: episode_at_ms,
            }
        } else if task.status == TaskStatus::Parked {
            let (kind, happened, action) = match facts.pull {
                Some(PullSignal::Conflicting) => (
                    "conflict",
                    "The parked task's pull request has a merge conflict.",
                    "Open the task and resume it to resolve the pull request conflicts.",
                ),
                Some(PullSignal::Behind) => (
                    "behind",
                    "The parked task's pull request needs to be updated with its base branch.",
                    "Open the task and resume it to update the branch with its base.",
                ),
                Some(PullSignal::InReview) => (
                    "in_review",
                    "The parked task is waiting for its pull request checks.",
                    "Open the task to review the pull request status or resume the task.",
                ),
                Some(PullSignal::Errored) => (
                    "errored",
                    "The parked task's pull request could not be verified.",
                    "Open the task and resume it or rerun CI.",
                ),
                _ => (
                    "resume",
                    "The task is parked and waiting for a person to resume it.",
                    "Open the task and resume it to continue.",
                ),
            };
            NeedsYouNoticeView {
                source: "system".to_owned(),
                episode: format!("parked:{kind}:{episode_at_ms}"),
                happened: happened.to_owned(),
                problem: None,
                action: action.to_owned(),
                link: task_link,
                decision: false,
                browser_tab: None,
                at: rfc3339(Timestamp(episode_at_ms)),
                at_ms: episode_at_ms,
            }
        } else if task.activity == Some(Activity::NeedsAttention) {
            NeedsYouNoticeView {
                source: "terminal".to_owned(),
                episode: format!("attention:bare:{episode_at_ms}"),
                happened: "The task was flagged for your attention.".to_owned(),
                problem: None,
                action: "Open the task terminal and respond to the prompt.".to_owned(),
                link: task_link,
                decision: false,
                browser_tab: None,
                at: rfc3339(Timestamp(episode_at_ms)),
                at_ms: episode_at_ms,
            }
        } else {
            let fallback = derived_wait_reason(facts)
                .unwrap_or("The task is waiting for your attention.")
                .to_owned();
            NeedsYouNoticeView {
                source: "system".to_owned(),
                episode: format!("needs_you:state:{episode_at_ms}"),
                happened: fallback,
                problem: None,
                action: "Open the task to review its status and reply or resume.".to_owned(),
                link: task_link,
                decision: false,
                browser_tab: None,
                at: rfc3339(Timestamp(episode_at_ms)),
                at_ms: episode_at_ms,
            }
        };

        let clean =
            |text: &str| crate::attention::shown_line(text, SHOWN_TEXT_CHARS).unwrap_or_default();
        let happened = clean(
            &self
                .redact_notice_text(&task.sandbox_id, &raw.happened)
                .await,
        );
        let action = clean(&self.redact_notice_text(&task.sandbox_id, &raw.action).await);
        let problem = match raw.problem {
            Some(problem) => {
                let redacted = self.redact_notice_text(&task.sandbox_id, &problem).await;
                crate::attention::shown_line(&redacted, SHOWN_TEXT_CHARS)
            }
            None => None,
        };
        let link = clean(&raw.link);

        Some(NeedsYouNoticeView {
            source: raw.source,
            episode: raw.episode,
            happened,
            problem,
            action,
            link,
            decision: raw.decision,
            browser_tab: raw.browser_tab,
            at: raw.at,
            at_ms: raw.at_ms,
        })
    }

    /// Persists the current wait episode in `task_wait_messages` (idempotent across repeated
    /// polls) or marks prior open episodes resolved when the task is no longer in Needs You
    /// (SPEC-578).
    pub(crate) async fn sync_wait_message(
        &self,
        task: &TaskRecord,
        notice: Option<&NeedsYouNoticeView>,
    ) {
        let now = self.now();
        match notice {
            Some(notice) => {
                let record = WaitMessageRecord {
                    task_id: task.id.clone(),
                    episode: notice.episode.clone(),
                    source: notice.source.clone(),
                    happened: notice.happened.clone(),
                    problem: notice.problem.clone(),
                    action: notice.action.clone(),
                    link: notice.link.clone(),
                    decision: notice.decision,
                    created_at: Timestamp(notice.at_ms),
                    resolved_at: None,
                };
                if let Err(err) = self.deps.store.record_wait_message(&record).await {
                    tracing::warn!(task = %task.id, error = %err, "recording wait history failed");
                }
            }
            None => {
                if let Err(err) = self.deps.store.resolve_wait_messages(&task.id, now).await {
                    tracing::warn!(task = %task.id, error = %err, "resolving wait history failed");
                }
            }
        }
    }

    /// Timestamped Needs You wait-episode history for `task` (SPEC-578), redacted and bounded.
    pub(crate) async fn wait_history_view(&self, task: &TaskRecord) -> Vec<WaitHistoryEntry> {
        let Ok(rows) = self
            .deps
            .store
            .task_wait_messages(&task.id, MAX_WAIT_HISTORY)
            .await
        else {
            return Vec::new();
        };
        let mut out = Vec::with_capacity(rows.len());
        for row in rows {
            let happened = self
                .redact_notice_text(&task.sandbox_id, &row.happened)
                .await;
            let action = self.redact_notice_text(&task.sandbox_id, &row.action).await;
            let problem = match row.problem {
                Some(problem) => Some(self.redact_notice_text(&task.sandbox_id, &problem).await),
                None => None,
            };
            out.push(WaitHistoryEntry {
                episode: row.episode,
                source: row.source,
                happened,
                problem,
                action,
                link: row.link,
                decision: row.decision,
                at: rfc3339(row.created_at),
                at_ms: row.created_at.0,
                resolved_at: row.resolved_at.map(rfc3339),
            });
        }
        out
    }

    /// Why `task` waits for a person, or is held, as its page shows it (QST-001): the fields a
    /// task's brief adds, each only when there is one. `needs_you`: what its agent asked;
    /// `attention`: the reason of its flag; `held_prompt`: a typed prompt Yulius held for a
    /// decision; `pause`: who paused it; `needs_you_notice`: actionable explanation whenever the
    /// task is in Needs You (SPEC-578). The held prompt's words (what a person typed, which
    /// never reached the agent or a transcript) go to `person`'s own callers only, their
    /// Superagent and their own agent: a task's agent learns that a prompt is held, and of
    /// which kind.
    pub(crate) async fn waits_why(
        &self,
        task: &TaskRecord,
        person: bool,
        facts: &AttentionFacts,
        pr_hint: Option<u64>,
    ) -> serde_json::Map<String, Value> {
        // What the task's summary shows, also for a task that ended: a prompt held before its
        // end is still decided on its card.
        let mut why = serde_json::Map::new();
        let mut add = |key: &str, value: Option<Value>| {
            if let Some(value) = value.filter(|value| !value.is_null()) {
                why.insert(key.to_owned(), value);
            }
        };
        // An ended task waits for nobody (ADR-0111): a request left from before its end is not
        // shown, as its summary shows none.
        let request = if task.status.is_resting() {
            None
        } else {
            self.needs_you_view(task).await
        };
        add(
            "needs_you",
            request.and_then(|view| {
                // A request stored before its words were cleaned is shown as one now is.
                let mut view = serde_json::to_value(view).ok()?;
                for key in ["action", "happened", "problem", "link"] {
                    if let Some(said) = view.get(key).and_then(Value::as_str) {
                        view[key] = json!(crate::attention::shown_line(said, SHOWN_TEXT_CHARS));
                    }
                }
                Some(view)
            }),
        );
        add(
            "attention",
            self.attention_view(task)
                .await
                .and_then(|view| serde_json::to_value(view).ok()),
        );
        add(
            "held_prompt",
            self.held_prompt_view(&task.id).await.map(|held| {
                let mut shown = json!({"class": held.class, "at": held.created_at});
                if person {
                    // What a person typed, as one line: it may hold anything, a line that
                    // reads like another task's among them.
                    shown["excerpt"] = json!(crate::attention::shown_line(
                        &held.excerpt,
                        HELD_EXCERPT_CHARS
                    ));
                    shown["typed_by"] = json!(held.typed_by);
                }
                shown
            }),
        );
        add(
            "pause",
            self.pause_view(task)
                .await
                .and_then(|pause| serde_json::to_value(pause).ok()),
        );
        // What a queued task waits for (SPEC-512 PRK-007): a supertask's hold is said by the
        // brief's own fields.
        if task.status == yulius_domain::task::TaskStatus::Queued {
            let held = self.supertask_holders(task).await.unwrap_or_default();
            add(
                "queue_reason",
                self.queue_reason_of(task, &held).await.map(Value::String),
            );
        }
        drop(add);
        // Merge any request or held prompt that arrived after `facts` was computed (e.g. after
        // `parked_status`) so `needs_you_notice` and `task_wait_messages` agree with `shows`.
        let mut effective = *facts;
        effective.browser_request |= why.contains_key("needs_you");
        effective.held_prompt |= why.contains_key("held_prompt");
        if effective.group() == TaskGroup::Working
            && !effective.paused
            && let Some(state) = effective.staging_wait
        {
            why.insert("staging_wait".into(), json!(state.task_wait_label()));
        }
        let notice = self
            .resolve_needs_you_notice_with_pr(task, &effective, pr_hint)
            .await;
        self.sync_wait_message(task, notice.as_ref()).await;
        if let Some(notice) = notice {
            if !why.contains_key("needs_you")
                && !why.contains_key("attention")
                && !why.contains_key("held_prompt")
            {
                why.insert("wait_reason".into(), json!(notice.happened));
            }
            if let Ok(value) = serde_json::to_value(notice)
                && !value.is_null()
            {
                why.insert("needs_you_notice".to_owned(), value);
            }
        }
        why
    }

    /// Dismisses what `task` asks of its person (`needs_you`: its agent's request, or one
    /// Yulius opened for it, such as no subscription left), for the task's owner (QST-005):
    /// the request ends without a word to the agent, as when the person has done it elsewhere
    /// or it no longer matters. Only the request that was read is ended: one the
    /// agent made meanwhile stays. Answers what was asked. Audited under the person's name.
    ///
    /// # Errors
    /// `Forbidden` for anyone but the owner; `Conflict` when nothing is asked (a flag says
    /// what ends it instead) or the request changed meanwhile.
    pub async fn dismiss_task_question(&self, ctx: &AuthContext, id: &str) -> AppResult<String> {
        let task = self.load_task(ctx, id, Action::Work).await?;
        let store = &self.deps.store;
        if task.status.is_resting() {
            return Err(AppError::Conflict(format!(
                "task #{} asks nothing of a person that can be dismissed: it has ended ({}), \
                 and an ended task waits for nobody",
                task.number,
                task.status.as_str()
            )));
        }
        let Some(request) = store.task_needs_you(&task.id).await? else {
            // Legacy browser handoff: tab still holds an in-memory action request even though
            // `tasks.needs_you` was never persisted (SPEC-578).
            if let Some((_tab_id, reason)) = self
                .browser_tab_request_for(&task.sandbox_id, &task.id)
                .await
            {
                self.clear_task_browser_actions(&task.sandbox_id, &task.id, None)
                    .await;
                if let Err(err) = store
                    .audit(
                        self.now(),
                        &self.login_of(&ctx.user.id).await,
                        "task.needs_you.dismissed",
                        Some(task.id.as_str()),
                        Some(&reason),
                    )
                    .await
                {
                    tracing::warn!(task = %task.id, error = %err, "a dismissed browser request was not audited");
                }
                self.publish_task(&task.id).await;
                return Ok(
                    crate::attention::shown_line(&reason, SHOWN_TEXT_CHARS).unwrap_or_default()
                );
            }
            let flagged = self.attention_view(&task).await.map(|flag| {
                let said = [flag.problem, flag.action]
                    .into_iter()
                    .flatten()
                    .collect::<Vec<_>>()
                    .join(" ");
                match flag.kind.as_str() {
                    "terminal" => format!(
                        ": Claude Code asks in its terminal ({said}), which its person answers \
                         there"
                    ),
                    _ => format!(": Yulius flagged it ({said}), which ends when that is done"),
                }
            });
            return Err(AppError::Conflict(format!(
                "task #{} asks nothing of a person that can be dismissed{}",
                task.number,
                flagged.unwrap_or_default()
            )));
        };
        if !store
            .end_task_needs_you_asking(&task.id, &request.action, self.now())
            .await?
        {
            return Err(AppError::Conflict(format!(
                "task #{}'s request changed meanwhile: read it again (task_get)",
                task.number
            )));
        }
        self.clear_task_browser_actions(&task.sandbox_id, &task.id, None)
            .await;
        // The request ended: a failed audit write is logged, not reported as a failed call.
        if let Err(err) = store
            .audit(
                self.now(),
                &self.login_of(&ctx.user.id).await,
                "task.needs_you.dismissed",
                Some(task.id.as_str()),
                Some(&request.action),
            )
            .await
        {
            tracing::warn!(task = %task.id, error = %err, "a dismissed request was not audited");
        }
        self.publish_task(&task.id).await;
        // What was asked, as the brief shows it.
        Ok(crate::attention::shown_line(&request.action, SHOWN_TEXT_CHARS).unwrap_or_default())
    }

    /// Withdraws `login` from the people `id`'s open request names (SPEC-512 ANS-005), for the
    /// task's owner: they no longer answer it, and the request is its owner's and the people
    /// still named. Audited `task.needs_you.withdrawn`. Answers the request's view.
    ///
    /// # Errors
    /// `Forbidden` for anyone but the owner; `Conflict` with no request open or one that does
    /// not name `login`.
    pub async fn withdraw_needs_you(
        &self,
        ctx: &AuthContext,
        id: &str,
        login: &str,
    ) -> AppResult<yulius_contracts::api::tasks::NeedsYouView> {
        let task = self.load_task(ctx, id, Action::Work).await?;
        let store = &self.deps.store;
        let login = login.trim().trim_start_matches('@').to_lowercase();
        let Some(read) = store.task_needs_you(&task.id).await? else {
            return Err(AppError::Conflict(format!(
                "task #{} asks nobody anything now: there is nobody to withdraw",
                task.number
            )));
        };
        if !read.named.who.contains(&login) {
            return Err(AppError::Conflict(format!(
                "task #{}'s question does not name @{login}",
                task.number
            )));
        }
        let mut request = read.clone();
        request.named.who.retain(|named| named != &login);
        request.named.withdrawn.push(login.clone());
        // Only the request read: a new one, an answer or another withdrawal meanwhile wins.
        if !store
            .replace_task_needs_you(&task.id, &read, &request, self.now())
            .await?
        {
            return Err(AppError::Conflict(format!(
                "task #{}'s question changed meanwhile: read it again",
                task.number
            )));
        }
        if let Err(err) = store
            .audit(
                self.now(),
                &self.login_of(&ctx.user.id).await,
                "task.needs_you.withdrawn",
                Some(task.id.as_str()),
                Some(&json!({"login": login, "action": request.action}).to_string()),
            )
            .await
        {
            tracing::warn!(task = %task.id, error = %err, "a withdrawal was not audited");
        }
        self.publish_task(&task.id).await;
        Ok(request.view())
    }

    /// Decides the prompt Yulius held in `task` (TSK-013), for the task's owner, as its card
    /// does (QST-005): `action` is one of [`HELD_PROMPT_ACTIONS`]. The page's function runs,
    /// with its checks and its audit.
    ///
    /// # Errors
    /// `Invalid` for another action; `Forbidden` for anyone but the owner; `Conflict` when the
    /// task holds no prompt; the page's errors.
    pub async fn decide_task_held_prompt(
        self: &Arc<Self>,
        ctx: &AuthContext,
        id: &str,
        action: Option<&str>,
    ) -> AppResult<HeldPromptOutcome> {
        let task = self.load_task(ctx, id, Action::Work).await?;
        let action = action
            .filter(|action| HELD_PROMPT_ACTIONS.contains(action))
            .ok_or_else(|| {
                AppError::Invalid(
                    "`action` is send, dismiss, parallel or after (skipping steps and reverting \
                     the task are decided on the task's page)"
                        .into(),
                )
            })?;
        let hold = self
            .deps
            .store
            .pending_hold(&task.id)
            .await?
            .ok_or_else(|| {
                AppError::Conflict(format!("task #{} holds no prompt now", task.number))
            })?;
        let req = ResolveHeldPrompt {
            action: action.to_owned(),
            skip_step_ids: Vec::new(),
            confirm: false,
        };
        self.resolve_held_prompt(ctx, id, &hold.id, &req).await
    }
}

#[cfg(test)]
mod tests {
    use super::group_name;
    use yulius_domain::task::TaskGroup;

    /// A request can be dismissed after the grouping snapshot. Its membership alone
    /// cannot tell us whether it came from the browser or the agent.
    #[test]
    fn a_request_snapshot_without_details_does_not_attribute_it_to_the_browser() {
        use yulius_domain::task::{Activity, AttentionFacts, TaskStatus};
        let mut facts = AttentionFacts::new(TaskStatus::Running, Some(Activity::Working));
        facts.browser_request = true;
        assert_eq!(
            super::derived_wait_reason(&facts),
            Some("A request for a person was pending when the task list was read.")
        );
    }

    #[test]
    fn every_group_has_its_name() {
        let names: Vec<&str> = [
            TaskGroup::NeedsYou,
            TaskGroup::Working,
            TaskGroup::Queued,
            TaskGroup::Done,
            TaskGroup::Ended,
        ]
        .into_iter()
        .map(group_name)
        .collect();
        assert_eq!(names, ["needs_you", "working", "queued", "done", "ended"]);
    }
}
