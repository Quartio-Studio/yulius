//! Shared fixtures moved from `subscription_moves`.

use crate::support::live::pool::{HOUR, ScriptedUsage, new_task, refresh, screen, seed, usage};
use crate::support::{TestResult, eventually};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::broadcast;
use yulius_app::context::App;
use yulius_app::hub::HubFrame;
use yulius_domain::ids::{TaskId, UserId};
use yulius_domain::task::AttemptState;

/// The subscription of every run of `id`, oldest first.
pub(crate) async fn run_accounts(app: &App, id: &TaskId) -> Vec<Option<String>> {
    app.deps
        .store
        .attempts(id)
        .await
        .unwrap_or_default()
        .into_iter()
        .map(|a| a.account_id.map(|acc| acc.as_str().to_owned()))
        .collect()
}

/// Waits until the task's run number `n` (from 1) is live.
pub(crate) async fn live_run(app: &App, id: &TaskId, n: usize) -> TestResult {
    eventually("the next run", Duration::from_secs(60), || async {
        let attempts = app.deps.store.attempts(id).await.unwrap_or_default();
        attempts.len() == n && attempts[n - 1].state == AttemptState::Live
    })
    .await
}

/// The notices said so far on a watch of the task's terminal.
pub(crate) fn notices(watch: &mut broadcast::Receiver<HubFrame>) -> Vec<String> {
    let mut said = Vec::new();
    while let Ok(frame) = watch.try_recv() {
        if let HubFrame::Notice(text) = frame {
            said.push(text);
        }
    }
    said
}

pub(crate) async fn ready(app: &Arc<App>, id: &TaskId) -> TestResult {
    eventually("the run", Duration::from_secs(30), || {
        let app = app.clone();
        let id = id.clone();
        async move { screen(&app, &id).contains("FAKE-AGENT READY") }
    })
    .await
}

/// A task on `a-first` (placed there while `b-second` was nearly full), with `b-second` free
/// again.
pub(crate) async fn running_on_first(
    app: &Arc<App>,
    probe: &ScriptedUsage,
    data: &std::path::Path,
) -> Result<(TaskId, UserId), Box<dyn std::error::Error>> {
    running_on_first_of(app, probe, data, false).await
}

/// [`running_on_first`] for a task a person set to Fable 5.2 (the default model is Opus 5.5,
/// SPEC-493).
pub(crate) async fn fable_running_on_first(
    app: &Arc<App>,
    probe: &ScriptedUsage,
    data: &std::path::Path,
) -> Result<(TaskId, UserId), Box<dyn std::error::Error>> {
    running_on_first_of(app, probe, data, true).await
}

async fn running_on_first_of(
    app: &Arc<App>,
    probe: &ScriptedUsage,
    data: &std::path::Path,
    fable: bool,
) -> Result<(TaskId, UserId), Box<dyn std::error::Error>> {
    let (sandbox, user) = seed(app, data, &["a-first", "b-second"]).await?;
    probe.set("b-second", usage(99.0, 4 * HOUR, 30.0, 100));
    refresh(app, &user, "b-second").await?;
    let id = if fable {
        crate::support::live::pool::new_fable_task(app, &sandbox, &user, "Moves").await?
    } else {
        new_task(app, &sandbox, &user, "Moves").await?
    };
    ready(app, &id).await?;
    assert_eq!(run_accounts(app, &id).await, [Some("a-first".to_owned())]);
    probe.set("b-second", usage(0.0, 5 * HOUR, 30.0, 100));
    refresh(app, &user, "b-second").await?;
    Ok((id, user))
}
