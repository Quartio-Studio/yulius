//! A sandbox's Docker trouble told to its tasks (SPEC-494 DFJ-007): while a sandbox's Docker
//! front is full (a `docker` connection trouble, SPEC-361's registry), each task of the sandbox
//! with a live session is told once, in its session, what is happening and that nobody needs to
//! be asked; once the trouble ends, the tasks told hear that Docker works again, so they rerun.
//! A task whose session takes no typed text now (starting, a person's draft) is told at a later
//! pass. What was told lives in memory: a restarted server, whose registry starts empty, tells
//! again at the next trouble.

use std::collections::{BTreeSet, HashMap};
use std::sync::Arc;
use std::time::Duration;

use yulius_domain::connection_health::Limit;
use yulius_domain::ids::TaskId;
use yulius_domain::task::TaskStatus;

use crate::context::App;
use crate::error::AppError;
use crate::task_message::{Sender, Typed};

/// How often the tasks of a sandbox with a Docker trouble are looked at.
pub const TELL_EVERY: Duration = Duration::from_secs(15);

/// Told once per trouble to each live task of the sandbox.
pub const DOCKER_FULL: &str = "Yulius: this sandbox's Docker front is full, so Docker calls in \
     this sandbox fail or are refused for now (\"holds all of its connections\"). Yulius sees it \
     and shows it to the sandbox's people: do not raise a Needs you about it. Leave Docker work \
     for later; you are told here when Docker works again.";

/// Told to the tasks that heard [`DOCKER_FULL`] once the trouble ended.
pub const DOCKER_BACK: &str = "Yulius: this sandbox's Docker works again (its front has room). \
     Rerun the Docker calls or tests that failed while it was full.";

/// What was told of one sandbox's trouble.
#[derive(Debug, Default)]
pub struct Told {
    /// The tasks told it.
    tasks: BTreeSet<TaskId>,
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

    /// One pass: tells each live task of a sandbox with a `docker` trouble [`DOCKER_FULL`] once
    /// per trouble, and the tasks told of a trouble that ended [`DOCKER_BACK`]. Returns what was
    /// typed into which task.
    pub async fn tell_docker_troubles(self: &Arc<Self>) -> Vec<(TaskId, &'static str)> {
        let open: BTreeSet<String> = self
            .connection_health
            .troubles()
            .into_iter()
            .filter(|t| t.limit == Limit::Docker)
            .filter_map(|t| t.sandbox)
            .collect();
        let sandboxes: Vec<String> = {
            let mut told = self
                .docker_told
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            // A trouble that ended and began again before its tasks heard that Docker works
            // again goes on as the same one for them: nothing more is said.
            for sandbox in &open {
                told.entry(sandbox.clone()).or_default();
            }
            told.keys().cloned().collect()
        };
        if sandboxes.is_empty() {
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
        let mut typed = Vec::new();
        for sandbox in sandboxes {
            let told_tasks = {
                let told = self
                    .docker_told
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                told.get(&sandbox)
                    .map(|t| t.tasks.clone())
                    .unwrap_or_default()
            };
            // The tasks told of a trouble that ended hear that Docker works again; a task gone
            // since needs nothing.
            let ended = !open.contains(&sandbox);
            for task in live.iter().filter(|t| t.sandbox_id.as_str() == sandbox) {
                let text = match (ended, told_tasks.contains(&task.id)) {
                    (true, true) => DOCKER_BACK,
                    (false, false) => DOCKER_FULL,
                    _ => continue,
                };
                let paste = format!("\x1b[200~{text}\x1b[201~");
                match self
                    .type_into_session(task, paste.as_bytes(), Typed::Prompt, Sender::Yulius)
                    .await
                {
                    Ok(()) => {
                        tracing::info!(task = %task.id, sandbox, back = ended, "told the task of its sandbox's Docker trouble");
                        let mut told = self
                            .docker_told
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner);
                        if let Some(entry) = told.get_mut(&sandbox) {
                            if ended {
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
            if ended {
                let live_here: BTreeSet<&TaskId> = live
                    .iter()
                    .filter(|t| t.sandbox_id.as_str() == sandbox)
                    .map(|t| &t.id)
                    .collect();
                let mut told = self
                    .docker_told
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                if let Some(entry) = told.get_mut(&sandbox) {
                    entry.tasks.retain(|task| live_here.contains(task));
                    if entry.tasks.is_empty() {
                        told.remove(&sandbox);
                    }
                }
            }
        }
        typed
    }
}
