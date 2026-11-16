//! A sandbox's Docker trouble told to its tasks (SPEC-494 DFJ-007): while a sandbox's Docker
//! front is full (a `docker` connection trouble, SPEC-361's registry), each task of the sandbox
//! with a live session is told once, in its session, what is happening and that nobody needs to
//! be asked; once the trouble has stayed ended for [`BACK_AFTER`] passes, the tasks told hear
//! that Docker works again, so they rerun. Nothing is typed into a session where a person must
//! act first (a prompt or question on screen, an open Needs you), nor into one paused or held by
//! an update: it is told at a later pass, as is a session that takes no typed text now. A task
//! told keeps waiting for "works again" until it rests (closed, stopped, failed). What was told
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
use crate::model::TaskRecord;
use crate::task_message::{Sender, Typed};

/// How often the tasks of a sandbox with a Docker trouble are looked at.
pub const TELL_EVERY: Duration = Duration::from_secs(15);

/// Passes a trouble stays ended before its tasks hear that Docker works again: a front that
/// refuses in bursts begins and ends troubles tick by tick, and its tasks hear one notice each
/// way, not one per tick (review round 1 of slice 2).
pub const BACK_AFTER: u32 = 3;

/// Told once per trouble to each live task of the sandbox.
pub const DOCKER_FULL: &str = "Yulius: this sandbox's Docker front is full, so Docker calls in \
     this sandbox fail or are refused for now (\"holds all of its connections\"). Yulius sees it \
     and shows it to the sandbox's people: do not raise a Needs you about it unless it lasts \
     more than 15 minutes. Leave Docker work for later; you are told here when Docker works \
     again (if nothing comes within 30 minutes, retry).";

/// Told to the tasks that heard [`DOCKER_FULL`] once the trouble ended.
pub const DOCKER_BACK: &str = "Yulius: this sandbox's Docker front is no longer full. Rerun the \
     Docker calls or tests that failed while it was full.";

/// What was told of one sandbox's trouble.
#[derive(Debug, Default)]
pub struct Told {
    /// The tasks told it, waiting for "works again".
    tasks: BTreeSet<TaskId>,
    /// Passes in a row that found it ended.
    ended_passes: u32,
}

/// What was told, by sandbox id.
pub type DockerTold = std::sync::Mutex<HashMap<String, Told>>;

impl App {
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
        let open: BTreeSet<String> = self
            .connection_health
            .troubles()
            .into_iter()
            .filter(|t| t.limit == Limit::Docker)
            .filter_map(|t| t.sandbox)
            .collect();
        // What each sandbox's tasks hear this pass: the notice, or "works again".
        let due: Vec<(String, bool)> = {
            let mut told = self.docker_told();
            for sandbox in &open {
                // A trouble that ends and begins again before its tasks heard that Docker works
                // goes on as the same one for them: nothing more is said.
                told.entry(sandbox.clone()).or_default().ended_passes = 0;
            }
            told.iter_mut()
                .filter_map(|(sandbox, entry)| {
                    if open.contains(sandbox) {
                        return Some((sandbox.clone(), false));
                    }
                    entry.ended_passes = entry.ended_passes.saturating_add(1);
                    (entry.ended_passes >= BACK_AFTER).then(|| (sandbox.clone(), true))
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
        for (sandbox, back) in due {
            let told_tasks = self
                .docker_told()
                .get(&sandbox)
                .map(|t| t.tasks.clone())
                .unwrap_or_default();
            for task in live.iter().filter(|t| t.sandbox_id.as_str() == sandbox) {
                let text = match (back, told_tasks.contains(&task.id)) {
                    (true, true) => DOCKER_BACK,
                    (false, false) => DOCKER_FULL,
                    _ => continue,
                };
                if held || self.person_must_act_or_paused(task).await {
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
                            if back {
                                entry.tasks.remove(&task.id);
                            } else {
                                entry.tasks.insert(task.id.clone());
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
            if back {
                self.forget_rested(&sandbox).await;
            }
        }
        typed
    }

    /// Whether a person must act in `task`'s session first (a prompt or question on screen, an
    /// open Needs you) or it is paused: nothing is typed then.
    async fn person_must_act_or_paused(&self, task: &TaskRecord) -> bool {
        task.activity == Some(Activity::NeedsAttention)
            || !matches!(self.deps.store.task_needs_you(&task.id).await, Ok(None))
            || !matches!(self.deps.store.open_pause(&task.id).await, Ok(None))
    }

    /// Drops the told tasks of `sandbox`'s ended trouble that rest now (or are gone), and the
    /// sandbox once none is left. A task between runs (a capacity pause, a switch) keeps
    /// waiting for "works again".
    async fn forget_rested(&self, sandbox: &str) {
        let told_tasks = self
            .docker_told()
            .get(sandbox)
            .map(|t| t.tasks.clone())
            .unwrap_or_default();
        let mut rested = Vec::new();
        for id in told_tasks {
            match self.deps.store.task(&id).await {
                Ok(Some(task)) if !task.status.is_resting() => {}
                Ok(_) => rested.push(id),
                // Unreadable now: kept for the next pass.
                Err(_) => {}
            }
        }
        let mut told = self.docker_told();
        if let Some(entry) = told.get_mut(sandbox) {
            for id in &rested {
                entry.tasks.remove(id);
            }
            if entry.tasks.is_empty() {
                told.remove(sandbox);
            }
        }
    }
}
