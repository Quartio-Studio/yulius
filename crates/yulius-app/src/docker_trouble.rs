//! A sandbox's Docker trouble told to its tasks (SPEC-494 DFJ-007): while a sandbox's Docker
//! front is full (a `docker` connection trouble, SPEC-361's registry), each task of the sandbox
//! with a live session is told once, in its session, what is happening and not to ask a person
//! (unless it lasts); once the trouble has stayed ended for [`BACK_AFTER`] passes, the tasks
//! told hear that it is no longer full, so they rerun. Nothing is typed into a session where a person must
//! act first (a prompt or question on screen, an open Needs you), nor into one paused or held by
//! an update: it is told at a later pass, as is a session that takes no typed text now. A task
//! told keeps waiting for "no longer full" until it rests (closed, stopped, failed). What was told
//! lives in memory: a restarted server, whose registry starts empty, tells again at the next
//! trouble, and the notice says to retry when nothing comes.

use std::collections::{BTreeSet, HashMap};
use std::sync::Arc;
use std::time::Duration;

use yulius_domain::connection_health::Limit;
use yulius_domain::ids::TaskId;
use yulius_domain::task::{Activity, TaskStatus};

use crate::context::App;
use crate::error::AppError;
use crate::task_message::{Sender, Typed};

/// How often the tasks of a sandbox with a Docker trouble are looked at.
pub const TELL_EVERY: Duration = Duration::from_secs(15);

/// Passes a trouble stays ended before its tasks hear that it is no longer full (75 s, past two
/// upkeep ticks of 30 s): a front that refuses in bursts begins and ends troubles tick by tick,
/// and its tasks hear one notice each way, not one per tick (review rounds 1 and 2 of slice 2).
pub const BACK_AFTER: u32 = 5;

/// Told once per trouble to each live task of the sandbox.
pub const DOCKER_FULL: &str = "Yulius: this sandbox's Docker front is full, so Docker calls in \
     this sandbox fail or are refused for now (\"holds all of its connections\"). Yulius sees it, \
     shows it to the sandbox's people and restarts the sandbox's Docker by itself if it stays \
     full (running containers stop, volumes stay): do not raise a Needs you about it. Leave \
     Docker work for later; you are told here when it is no longer full (if nothing comes \
     within 30 minutes, retry).";

/// Told to the tasks that heard [`DOCKER_FULL`] once the trouble stayed ended, and to those
/// whose Docker request the repair answered.
pub const DOCKER_BACK: &str = "Yulius: this sandbox's Docker works again: its front is no longer \
     full, or Yulius restarted it (the containers that ran stopped; volumes stayed). Rerun the \
     Docker calls or tests that failed.";

/// Told to a task whose request about the sandbox's Docker a person answered through another task
/// that shares it (SPEC-494 DFJ-011).
pub const DOCKER_ANSWERED: &str = "Yulius: a person answered this sandbox's shared Docker request \
     through another task that asked it too, so yours is answered as well. If Docker still \
     fails here, call needs_you again with cause docker.";

/// The audit action of a front restarted (SPEC-494 DFJ-013); its detail says why.
pub const DOCKER_RESTARTED: &str = "sandbox.docker.restarted";
/// The audit action of connections that gave way at a front's limit; its detail is how many.
pub const DOCKER_GAVE_WAY: &str = "sandbox.docker.gave_way";
/// The audit action of what CI jobs left in a sandbox's engine and Yulius removed (SPEC-504
/// CLB-006); its detail is `<containers> <networks> <job-end|sweep>`.
pub const DOCKER_CI_LEFTOVERS: &str = "sandbox.docker.ci_leftovers";

/// The Monday (UTC) of the week of `ms`, `YYYY-MM-DD`.
fn week_of(ms: i64) -> String {
    let day = ms.div_euclid(86_400_000);
    // 1970-01-01 was a Thursday: three days after a Monday.
    let monday = day - (day + 3).rem_euclid(7);
    let at = crate::auth::rfc3339(yulius_domain::time::Timestamp(monday * 86_400_000));
    at.chars().take(10).collect()
}

/// What was told of one sandbox's trouble.
#[derive(Debug, Default)]
pub struct Told {
    /// The tasks told it, waiting for "no longer full".
    tasks: BTreeSet<TaskId>,
    /// Passes in a row that found it ended.
    ended_passes: u32,
    /// When a pass first found it ended (ms): requests asked later are not answered by its end.
    ended_at_ms: Option<i64>,
    /// Tasks whose shared request a person answered through another, to be told so.
    answered: BTreeSet<TaskId>,
    /// It saw an open trouble, or a restart (whose time is `ended_at_ms`): only then does its
    /// end answer requests and tell "works again" (review round 2 of slice 4).
    saw_trouble: bool,
}

/// What was told, by sandbox id.
pub type DockerTold = std::sync::Mutex<HashMap<String, Told>>;

impl App {
    /// Has `tasks` hear that a person answered their shared Docker request through another task
    /// of `sandbox` (SPEC-494 DFJ-011).
    pub(crate) fn tell_docker_answered(&self, sandbox: &str, tasks: Vec<TaskId>) {
        if tasks.is_empty() {
            return;
        }
        self.docker_told()
            .entry(sandbox.to_owned())
            .or_default()
            .answered
            .extend(tasks);
    }

    /// The Superagent's `docker_restart` (SPEC-494 DFJ-009): restarts the sandbox's
    /// Docker front and engine now.
    ///
    /// # Errors
    /// `Conflict` with why it could not (no Docker on this server, no running holder).
    pub(crate) async fn restart_docker_for_person(
        &self,
        sandbox: &yulius_domain::ids::SandboxId,
    ) -> crate::error::AppResult<serde_json::Value> {
        let Some(isolation) = self.isolation.get() else {
            return Err(AppError::Conflict(
                "this server runs no sandbox Docker".into(),
            ));
        };
        isolation
            .restart_docker(sandbox)
            .await
            .map_err(AppError::Conflict)?;
        // The repair answers the sandbox's shared Docker requests asked before it; their tasks are
        // told once the trouble (if any) has stayed ended (SPEC-494 DFJ-011).
        let now = self.now().0;
        self.answer_docker_requests(
            sandbox.as_str(),
            now,
            "a restart asked through the Superagent",
        )
        .await;
        {
            // The restart's time is the end, also with no trouble open.
            let mut told = self.docker_told();
            let entry = told.entry(sandbox.as_str().to_owned()).or_default();
            entry.saw_trouble = true;
            entry.ended_at_ms.get_or_insert(now);
        }
        tracing::info!(sandbox = %sandbox, "the sandbox's Docker was restarted at a person's ask");
        Ok(serde_json::json!({
            "restarted": true,
            "said": "The sandbox's Docker front was restarted (or started, if none ran): an engine it ran stopped with it (its running containers stopped; volumes and images stay) and starts again at the next Docker call. The sandbox's Docker requests asked before are answered, and their tasks are told."
        }))
    }

    /// Runs [`Self::tell_docker_troubles`] every [`TELL_EVERY`].
    pub fn start_docker_watch(self: &Arc<Self>) {
        let app = Arc::clone(self);
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(TELL_EVERY);
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                tick.tick().await;
                app.tell_docker_troubles().await;
            }
        });
    }

    fn docker_told(&self) -> std::sync::MutexGuard<'_, HashMap<String, Told>> {
        self.docker_told
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// One pass: tells each live task of a sandbox with a `docker` trouble [`DOCKER_FULL`] once
    /// per trouble, and the tasks told of a trouble ended for [`BACK_AFTER`] passes
    /// [`DOCKER_BACK`]. Returns what was typed into which task.
    pub async fn tell_docker_troubles(self: &Arc<Self>) -> Vec<(TaskId, &'static str)> {
        self.record_docker_events().await;
        let open: BTreeSet<String> = self
            .connection_health
            .troubles()
            .into_iter()
            .filter(|t| t.limit == Limit::Docker)
            .filter_map(|t| t.sandbox)
            .collect();
        // What each sandbox's tasks hear this pass: the notice (`Some(false)`), "no longer full"
        // (`Some(true)`), or only that a shared request was answered (`None`).
        let now = self.now().0;
        let due: Vec<(String, Option<bool>, Option<i64>)> = {
            let mut told = self.docker_told();
            for sandbox in &open {
                // A trouble that ends and begins again before its tasks heard that Docker works
                // goes on as the same one for them: nothing more is said.
                let entry = told.entry(sandbox.clone()).or_default();
                entry.ended_passes = 0;
                entry.ended_at_ms = None;
                entry.saw_trouble = true;
            }
            told.iter_mut()
                .filter_map(|(sandbox, entry)| {
                    if open.contains(sandbox) {
                        return Some((sandbox.clone(), Some(false), None));
                    }
                    entry.ended_passes = entry.ended_passes.saturating_add(1);
                    let ended_at = *entry.ended_at_ms.get_or_insert(now);
                    if entry.saw_trouble && entry.ended_passes >= BACK_AFTER {
                        Some((sandbox.clone(), Some(true), Some(ended_at)))
                    } else {
                        (!entry.answered.is_empty()).then(|| (sandbox.clone(), None, None))
                    }
                })
                .collect()
        };
        if due.is_empty() {
            return Vec::new();
        }
        let attempts = match self.deps.store.live_attempts().await {
            Ok(attempts) => attempts,
            Err(err) => {
                tracing::warn!(error = %err, "the tasks to tell of a Docker trouble could not be read");
                return Vec::new();
            }
        };
        let mut live = Vec::new();
        for attempt in attempts {
            if let Ok(Some(task)) = self.deps.store.task(&attempt.task_id).await
                && task.status == TaskStatus::Running
            {
                live.push(task);
            }
        }
        let held = self.update_holds_starts();
        let mut typed = Vec::new();
        for (sandbox, phase, ended_at) in due {
            let back = phase == Some(true);
            if let Some(ended_at) = ended_at {
                // The trouble's end answers the sandbox's shared Docker requests asked before it;
                // their tasks hear it with the others (SPEC-494 DFJ-011).
                self.answer_docker_requests(
                    &sandbox,
                    ended_at,
                    "the end of the sandbox's Docker trouble",
                )
                .await;
            }
            let (told_tasks, answered) = self
                .docker_told()
                .get(&sandbox)
                .map(|t| (t.tasks.clone(), t.answered.clone()))
                .unwrap_or_default();
            for task in live.iter().filter(|t| t.sandbox_id.as_str() == sandbox) {
                let text = match (phase, told_tasks.contains(&task.id)) {
                    (Some(true), true) => DOCKER_BACK,
                    (Some(false), false) => DOCKER_FULL,
                    _ if answered.contains(&task.id) => DOCKER_ANSWERED,
                    _ => continue,
                };
                // Read again right before typing: earlier notices of this pass took time, and a
                // prompt may have come up since (review round 2).
                let Ok(Some(task)) = self.deps.store.task(&task.id).await else {
                    continue;
                };
                let task = &task;
                let needs_you = !matches!(self.deps.store.task_needs_you(&task.id).await, Ok(None));
                let paused = !matches!(self.deps.store.open_pause(&task.id).await, Ok(None));
                if task.status != TaskStatus::Running
                    || blocked(task.activity, needs_you, paused, held)
                {
                    // Told at a later pass: a paste and an Enter would answer what is on screen.
                    continue;
                }
                let paste = format!("\x1b[200~{text}\x1b[201~");
                match self
                    .type_into_session(task, paste.as_bytes(), Typed::Prompt, Sender::Yulius)
                    .await
                {
                    Ok(()) => {
                        tracing::info!(task = %task.id, sandbox, back, "told the task of its sandbox's Docker trouble");
                        if let Some(entry) = self.docker_told().get_mut(&sandbox) {
                            match text {
                                DOCKER_BACK => {
                                    entry.tasks.remove(&task.id);
                                    entry.answered.remove(&task.id);
                                }
                                DOCKER_FULL => {
                                    entry.tasks.insert(task.id.clone());
                                }
                                _ => {
                                    entry.answered.remove(&task.id);
                                }
                            }
                        }
                        typed.push((task.id.clone(), text));
                    }
                    Err(AppError::Conflict(why)) => {
                        tracing::debug!(task = %task.id, why, "a Docker trouble notice waits");
                    }
                    Err(err) => {
                        tracing::warn!(task = %task.id, error = %err, "could not tell the task of its sandbox's Docker trouble");
                    }
                }
            }
            if phase != Some(false) {
                self.forget_rested(&sandbox).await;
            }
        }
        typed
    }

    /// Writes the fronts' events since the last pass to the audit log, about their sandbox's
    /// slug, for the instance's measures (SPEC-494 DFJ-013).
    pub async fn record_docker_events(&self) {
        let Some(isolation) = self.isolation.get() else {
            return;
        };
        for event in isolation.take_docker_events() {
            let (id, production) =
                match yulius_contracts::sandbox::production_sandbox(&event.sandbox) {
                    Some(id) => (id.to_owned(), true),
                    None => (event.sandbox.clone(), false),
                };
            let Ok(id) = yulius_domain::ids::SandboxId::parse(&id) else {
                continue;
            };
            let Ok(Some(sandbox)) = self.deps.store.sandbox(&id).await else {
                continue;
            };
            let (action, detail) = match event.kind.as_str() {
                "restarted" => (
                    DOCKER_RESTARTED,
                    format!(
                        "{}{}",
                        event.detail,
                        if production {
                            " (production engine)"
                        } else {
                            ""
                        }
                    ),
                ),
                "gave_way" => (DOCKER_GAVE_WAY, event.count.to_string()),
                "ci_leftovers" => (DOCKER_CI_LEFTOVERS, event.detail.clone()),
                _ => continue,
            };
            if let Err(err) = self
                .deps
                .store
                .audit(
                    self.now(),
                    "yulius",
                    action,
                    Some(&sandbox.slug),
                    Some(&detail),
                )
                .await
            {
                tracing::warn!(error = %err, "a Docker front event could not be recorded");
            }
        }
    }

    /// `GET /api/analytics/docker` (SPEC-494 DFJ-013): front restarts, connections that gave way
    /// and Needs you naming the sandbox's Docker, and what CI jobs left that Yulius removed
    /// (SPEC-504 CLB-006), per week, over the range.
    ///
    /// # Errors
    /// An invalid range, a sandbox the caller cannot see, store failures.
    pub async fn analytics_docker(
        &self,
        ctx: &crate::auth::AuthContext,
        sandbox: Option<&str>,
        from: Option<&str>,
        to: Option<&str>,
    ) -> crate::error::AppResult<yulius_contracts::api::analytics::DockerAnalytics> {
        use yulius_contracts::api::analytics::{DockerAnalytics, DockerWeek};
        let (sandboxes, from_at, to_at, range) =
            self.analytics_window(ctx, sandbox, from, to).await?;
        let actions = [
            DOCKER_RESTARTED,
            DOCKER_GAVE_WAY,
            DOCKER_CI_LEFTOVERS,
            crate::needs_you::DOCKER_NEEDS_YOU,
        ];
        let mut weeks: std::collections::BTreeMap<String, DockerWeek> =
            std::collections::BTreeMap::new();
        for id in &sandboxes {
            let Some(record) = self.deps.store.sandbox(id).await? else {
                continue;
            };
            let mut tasks = self.deps.store.tasks(id, false).await?;
            tasks.extend(self.deps.store.tasks(id, true).await?);
            let ids: Vec<String> = tasks.iter().map(|t| t.id.to_string()).collect();
            let entries = self
                .deps
                .store
                .audit_of_sandbox(&record.slug, &ids, &actions, from_at, to_at, 20_000)
                .await?;
            for (_, entry) in entries {
                let week = week_of(entry.at.0);
                let row = weeks.entry(week.clone()).or_insert_with(|| DockerWeek {
                    week,
                    ..DockerWeek::default()
                });
                match entry.action.as_str() {
                    DOCKER_RESTARTED => row.restarts += 1,
                    DOCKER_GAVE_WAY => {
                        row.gave_way += entry
                            .detail
                            .as_deref()
                            .and_then(|d| d.parse::<u64>().ok())
                            .unwrap_or(0);
                    }
                    DOCKER_CI_LEFTOVERS => {
                        let detail = entry.detail.as_deref().unwrap_or_default();
                        let mut words = detail.split_whitespace();
                        let mut number = || words.next().and_then(|w| w.parse::<u64>().ok());
                        let (containers, networks) = (number().unwrap_or(0), number().unwrap_or(0));
                        row.ci_containers += containers;
                        row.ci_networks += networks;
                        if detail.ends_with(" sweep") {
                            row.ci_swept += containers + networks;
                        }
                    }
                    _ => row.needs_you += 1,
                }
            }
        }
        let weeks: Vec<DockerWeek> = weeks.into_values().collect();
        Ok(DockerAnalytics {
            range,
            restarts: weeks.iter().map(|w| w.restarts).sum(),
            gave_way: weeks.iter().map(|w| w.gave_way).sum(),
            needs_you: weeks.iter().map(|w| w.needs_you).sum(),
            ci_containers: weeks.iter().map(|w| w.ci_containers).sum(),
            ci_networks: weeks.iter().map(|w| w.ci_networks).sum(),
            ci_swept: weeks.iter().map(|w| w.ci_swept).sum(),
            weeks,
        })
    }

    /// Ends the requests of `sandbox`'s tasks that name the `docker` cause and were asked by
    /// `asked_until_ms`, and has those tasks hear that Docker works again with the ones told of
    /// the trouble.
    async fn answer_docker_requests(&self, sandbox: &str, asked_until_ms: i64, source: &str) {
        let Ok(id) = yulius_domain::ids::SandboxId::parse(sandbox) else {
            return;
        };
        let ended = self
            .end_shared_needs_you(&id, "docker", asked_until_ms, source)
            .await;
        if ended.is_empty() {
            return;
        }
        let mut told = self.docker_told();
        let entry = told.entry(sandbox.to_owned()).or_default();
        entry.tasks.extend(ended);
    }

    /// Drops the told tasks of `sandbox`'s ended trouble that rest now (or are gone), and the
    /// sandbox once none is left. A task between runs (a capacity pause, a switch) keeps
    /// waiting for "no longer full".
    async fn forget_rested(&self, sandbox: &str) {
        let waiting: BTreeSet<TaskId> = self
            .docker_told()
            .get(sandbox)
            .map(|t| t.tasks.union(&t.answered).cloned().collect())
            .unwrap_or_default();
        let mut rested = Vec::new();
        for id in waiting {
            match self.deps.store.task(&id).await {
                Ok(task) if !keeps_waiting(task.as_ref().map(|t| t.status)) => rested.push(id),
                // Waiting still, or unreadable now: kept for the next pass.
                _ => {}
            }
        }
        let mut told = self.docker_told();
        if let Some(entry) = told.get_mut(sandbox) {
            for id in &rested {
                entry.tasks.remove(id);
                entry.answered.remove(id);
            }
            if entry.tasks.is_empty() && entry.answered.is_empty() {
                told.remove(sandbox);
            }
        }
    }
}

/// Whether nothing may be typed into a task's session now: a person must act first (a prompt or
/// question on screen, an open Needs you), it is paused, or an update holds starts.
fn blocked(activity: Option<Activity>, needs_you: bool, paused: bool, update_holds: bool) -> bool {
    activity == Some(Activity::NeedsAttention) || needs_you || paused || update_holds
}

/// Whether a task told of a trouble still waits for its end: until it rests (closed, stopped,
/// failed) or is gone; a task between runs (a capacity pause, a switch) waits.
fn keeps_waiting(status: Option<TaskStatus>) -> bool {
    status.is_some_and(|status| !status.is_resting())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// SPEC-494 DFJ-013: a week runs from its Monday (UTC).
    #[test]
    fn a_week_is_named_by_its_monday() {
        // 2026-11-15 was a Sunday, 2026-11-16 a Monday.
        let day = |d: i64| d * 86_400_000;
        let sunday = day(20_772);
        assert_eq!(week_of(sunday), "2026-11-09");
        assert_eq!(week_of(sunday + day(1)), "2026-11-16");
        assert_eq!(week_of(sunday + day(7) + 86_399_999), "2026-11-16");
        assert_eq!(week_of(0), "1969-12-29");
    }

    /// Slice 2 review round 2: each reason a session may take no paste blocks it alone.
    #[test]
    fn a_prompt_a_needs_you_a_pause_or_an_update_hold_each_block_typing() {
        assert!(!blocked(None, false, false, false));
        assert!(!blocked(Some(Activity::Working), false, false, false));
        assert!(blocked(Some(Activity::NeedsAttention), false, false, false));
        assert!(blocked(None, true, false, false));
        assert!(blocked(None, false, true, false));
        assert!(blocked(None, false, false, true));
    }

    /// Slice 2 review round 2: a task told of a trouble waits for its end while between runs, and
    /// is forgotten once it rests or is gone.
    #[test]
    fn a_told_task_waits_for_the_end_until_it_rests() {
        assert!(keeps_waiting(Some(TaskStatus::Running)));
        assert!(keeps_waiting(Some(TaskStatus::WaitingCapacity)));
        assert!(keeps_waiting(Some(TaskStatus::Queued)));
        assert!(!keeps_waiting(Some(TaskStatus::Closed)));
        assert!(!keeps_waiting(None));
    }
}
