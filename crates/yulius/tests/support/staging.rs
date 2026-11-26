//! Shared fixtures moved from `staging`.

use crate::support::cd::{BoxResult, Fixture};
use yulius_app::ci::{CiJobRecord, CiRunRecord};
use yulius_app::ports::store::SandboxStore as _;
use yulius_domain::ci::{CiRef, GateSource, RunState};
use yulius_domain::ids::SandboxId;
use yulius_domain::resources::Need;

pub(crate) fn token(task: &str) -> String {
    format!("staging-task-token-{task}-0123456789abcdef")
}

/// The integration branch's CI run at `sha`: the quick checks passed, the full suite `full`.
pub(crate) async fn gate(f: &Fixture, sha: &str, full: RunState) -> BoxResult<String> {
    recorded_gate(&f.h.app, &SandboxId::parse("s1")?, sha, full, None).await
}

pub(crate) async fn recorded_gate(
    app: &yulius_app::context::App,
    sandbox: &SandboxId,
    sha: &str,
    full: RunState,
    failures: Option<&str>,
) -> BoxResult<String> {
    recorded_gate_as(
        app,
        sandbox,
        sha,
        full,
        failures,
        format!("ci-{}", &sha[..12]),
    )
    .await
}

/// [`recorded_gate`] as run `id` (a rerun of the same revision).
pub(crate) async fn recorded_gate_as(
    app: &yulius_app::context::App,
    sandbox: &SandboxId,
    sha: &str,
    full: RunState,
    failures: Option<&str>,
    id: String,
) -> BoxResult<String> {
    let state = if full == RunState::Passed {
        RunState::Passed
    } else {
        full
    };
    let run = CiRunRecord {
        id: id.clone(),
        sandbox_id: sandbox.clone(),
        reference: CiRef::Branch("dev".into()),
        sha: sha.to_owned(),
        source: GateSource::ProjectConfig,
        state,
        reason: None,
        requested_by: "yulius".into(),
        status_for: None,
        status_note: None,
        status_attempts: 0,
        queued_at: app.now(),
        started_at: Some(app.now()),
        ended_at: Some(app.now()),
    };
    let job = |name: &str, state: RunState| CiJobRecord {
        id: format!("{id}-{name}"),
        run_id: id.clone(),
        ordinal: u32::from(name == "full"),
        name: name.to_owned(),
        key: name.to_owned(),
        command: "true".into(),
        matrix: None,
        state,
        reason: None,
        exit_code: Some(i32::from(state != RunState::Passed)),
        output: String::new(),
        failures: None,
        started_at: run.started_at,
        ended_at: run.ended_at,
        need: Need::default(),
        timeout_minutes: None,
        exclusive: false,
        waited_ms: None,
    };
    app.deps
        .store
        .insert_ci_run(&run, &[job("quick", RunState::Passed), job("full", full)])
        .await?;
    if let Some(failures) = failures {
        assert!(
            app.deps
                .store
                .transition_ci_job(
                    &format!("{id}-full"),
                    &[full],
                    full,
                    &yulius_app::ci::CiJobUpdate {
                        failures: Some(failures.to_owned()),
                        ..Default::default()
                    }
                )
                .await?
        );
    }
    Ok(id)
}

/// Whether a new red gate asks people for help in a native-goal session fixture.
pub(crate) async fn red_gate_needs_people(
    app: &yulius_app::context::App,
    project: &SandboxId,
    owner: &yulius_domain::ids::UserId,
) -> BoxResult<bool> {
    let repo = app
        .config
        .data_dir
        .join("sandboxes")
        .join(project.as_str())
        .join("repo");
    let sha = crate::support::git(&repo, &["rev-parse", "HEAD"])?;
    crate::support::git(&repo, &["update-ref", "refs/remotes/origin/dev", &sha])?;
    recorded_gate(
        app,
        project,
        &sha,
        RunState::Failed,
        Some("---- gate::test stdout ----"),
    )
    .await?;
    let ctx = yulius_app::auth::AuthContext {
        user: app.deps.store.user(owner).await?.ok_or("owner")?,
        csrf_token: None,
        session_hash: None,
        token: None,
    };
    let status = app.status(&ctx).await?;
    Ok(status
        .sandboxes
        .iter()
        .flat_map(|s| &s.gate_notices)
        .any(|n| n.test == "gate::test"))
}

pub(crate) async fn sandbox(f: &Fixture) -> BoxResult<yulius_app::model::SandboxRecord> {
    Ok(f.h
        .store
        .sandbox(&SandboxId::parse("s1")?)
        .await?
        .ok_or("s1")?)
}
