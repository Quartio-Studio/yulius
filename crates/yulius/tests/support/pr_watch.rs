//! Shared fixtures moved from `pr_watch`.

use crate::support::live::pool::{build_tuned, new_task, runtime, screen, seed, stop, temp};
use crate::support::{eventually, git};
use std::path::Path;
use std::time::Duration;
use yulius_app::ci::{CiJobRecord, CiRunRecord};
use yulius_app::delivery::DeliveryRecord;
use yulius_app::pr_watch::WatchBounds;
use yulius_domain::ci::{CiRef, GateSource, RunState};
use yulius_domain::delivery::DeliveryStage;
use yulius_domain::ids::{SandboxId, TaskId};
use yulius_domain::resources::Need;
use yulius_domain::task::Activity;

type TestResult = Result<(), Box<dyn std::error::Error>>;

pub(crate) async fn leave_verification_step(
    app: &yulius_app::context::App,
    task: &TaskId,
) -> TestResult {
    app.deps
        .store
        .replace_steps(
            task,
            &[yulius_app::model::StepRecord {
                id: yulius_domain::ids::StepId::parse(&format!("verify-{task}"))?,
                task_id: task.clone(),
                ordinal: 1,
                title: "Verify delivery".to_owned(),
                status: "in_progress".to_owned(),
                note: None,
                updated_by: None,
                updated_at: None,
                effort: 1,
            }],
            "person",
            app.now(),
        )
        .await?;
    Ok(())
}

/// A run of pull request 7 at `head` that ended in `state`, with one job.
pub(crate) fn run(
    sandbox: &SandboxId,
    id: &str,
    head: &str,
    state: RunState,
    failures: Option<&str>,
    queued: i64,
) -> (CiRunRecord, CiJobRecord) {
    let at = yulius_domain::time::Timestamp(1);
    let run = CiRunRecord {
        id: id.to_owned(),
        sandbox_id: sandbox.clone(),
        reference: CiRef::PullRequest(7),
        sha: head.to_owned(),
        source: GateSource::ProjectConfig,
        state,
        reason: None,
        requested_by: "yulius".into(),
        status_for: None,
        status_note: None,
        status_attempts: 0,
        queued_at: yulius_domain::time::Timestamp(queued),
        started_at: Some(at),
        ended_at: Some(at),
    };
    let job = CiJobRecord {
        id: format!("{id}-quality"),
        run_id: id.to_owned(),
        ordinal: 0,
        name: "quality".into(),
        key: "quality".into(),
        command: "scripts/dev ci".into(),
        matrix: None,
        state,
        reason: None,
        exit_code: Some(if state == RunState::Failed { 100 } else { 0 }),
        output: String::new(),
        failures: failures.map(str::to_owned),
        started_at: Some(at),
        ended_at: Some(at),
        need: Need::default(),
        timeout_minutes: None,
        exclusive: false,
        waited_ms: None,
    };
    (run, job)
}
