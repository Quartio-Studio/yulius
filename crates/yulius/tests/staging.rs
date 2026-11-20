//! Merge-first staging end to end (SPEC-575 MFS-003 to MFS-008): a project whose `[cd.staging]`
//! is the merge-first target, the real Git adapter and process host deploying it, tasks' agents
//! asking through `cd_deploy`, and the full gate's verdicts recorded at exact revisions.

use std::time::Duration;

use serde_json::{Value, json};
use yulius_app::ci::{CiJobRecord, CiRunRecord};
use yulius_app::ports::store::SandboxStore as _;
use yulius_app::ports::store_ext::{
    CdStore as _, CiStore as _, StagingChange, StagingRequestRecord, StagingStore as _,
};
use yulius_domain::ci::{CiRef, GateSource, RunState};
use yulius_domain::ids::{SandboxId, TaskId};
use yulius_domain::resources::Need;
use yulius_domain::staging::StagingState;
use yulius_domain::task::TaskStatus;

use crate::cd::{BoxResult, Fixture, TestResult, fixture};
use crate::support;

const WAIT: Duration = Duration::from_secs(60);

/// Quick checks and an `after_merge` full suite; `[cd.staging]` deploys and verifies with the
/// fixture's scripts (the marks directory replaces `{marks}`).
fn staging_toml() -> String {
    "[[ci.jobs]]\nname = \"quick\"\ncommand = \"true\"\n\n\
     [[ci.jobs]]\nname = \"full\"\ncommand = \"true\"\nafter_merge = true\n\n\
     [cd.dev]\nabout = \"no dev machine\"\n\n[cd.prod]\nabout = \"nothing yet\"\n\n\
     [cd.staging]\nstage = \"dev\"\nmerge_first = true\n\
     deploy = \"MARKS={marks} sh scripts/cd/deploy.sh\"\n\
     verify = \"MARKS={marks} sh scripts/cd/verify.sh\"\n\
     rollback = \"true\"\nmigrations = \"none: the test app keeps no data\"\n"
        .to_owned()
}

fn token(task: &str) -> String {
    format!("staging-task-token-{task}-0123456789abcdef")
}

/// A task with a live agent.
async fn agent(f: &Fixture, task: &str) -> TestResult {
    support::seed_task(&f.h, task, "s1", "u-alice", TaskStatus::Running, None).await?;
    support::seed_live_attempt(&f.h, task, &token(task)).await?;
    Ok(())
}

/// A new commit on `dev` that `task`'s pull request landed: its merged revision.
async fn land(f: &Fixture, task: &str, file: &str) -> BoxResult<String> {
    let sha = f.push("dev", &[(file, Some(task))])?;
    f.h.store
        .execute_for_tests(&format!(
            "INSERT INTO task_merges(task_id, sandbox_id, merged_sha, branch, pr_number, head_sha) \
             VALUES ('{task}', 's1', '{sha}', 'dev', NULL, '{sha}')"
        ))
        .await?;
    Ok(sha)
}

/// The integration branch's CI run at `sha`: the quick checks passed, the full suite `full`.
async fn gate(f: &Fixture, sha: &str, full: RunState) -> BoxResult<String> {
    let id = format!("ci-{}", &sha[..12]);
    let state = if full == RunState::Passed {
        RunState::Passed
    } else {
        full
    };
    let run = CiRunRecord {
        id: id.clone(),
        sandbox_id: SandboxId::parse("s1")?,
        reference: CiRef::Branch("dev".into()),
        sha: sha.to_owned(),
        source: GateSource::ProjectConfig,
        state,
        reason: None,
        requested_by: "yulius".into(),
        status_for: None,
        status_note: None,
        status_attempts: 0,
        queued_at: f.h.app.now(),
        started_at: Some(f.h.app.now()),
        ended_at: Some(f.h.app.now()),
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
    f.h.store
        .insert_ci_run(&run, &[job("quick", RunState::Passed), job("full", full)])
        .await?;
    Ok(id)
}

/// `task`'s agent asks for staging; the request it was given.
async fn ask(f: &Fixture, task: &str) -> BoxResult<Value> {
    let answer = support::mcp_tool(
        &f.h,
        &token(task),
        "cd_deploy",
        json!({"environment": "staging"}),
    )
    .await?;
    assert_eq!(answer["isError"], false, "{answer}");
    Ok(support::tool_text(&answer)["staging"].clone())
}

async fn request(f: &Fixture, id: &str) -> BoxResult<StagingRequestRecord> {
    Ok(f.h.store.staging_request(id).await?.ok_or("request")?)
}

/// Rounds of the queue until no run of `[cd.staging]` runs and no request deploys.
async fn settle(f: &Fixture) -> TestResult {
    let sandbox =
        f.h.store
            .sandbox(&SandboxId::parse("s1")?)
            .await?
            .ok_or("s1")?;
    let deadline = tokio::time::Instant::now() + WAIT;
    loop {
        f.h.app.staging_advance(&sandbox, "staging").await?;
        let running =
            f.h.store
                .running_cd_runs()
                .await?
                .into_iter()
                .any(|run| run.environment == "staging");
        let deploying =
            f.h.store
                .pending_staging_requests(&sandbox.id, "staging")
                .await?
                .iter()
                .any(|r| matches!(r.state, StagingState::Deploying | StagingState::Ready));
        if !running && !deploying {
            return Ok(());
        }
        if tokio::time::Instant::now() > deadline {
            return Err("the staging queue did not settle".into());
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// What staging serves: the revision its deploy script marked.
fn serving(f: &Fixture) -> BoxResult<String> {
    Ok(std::fs::read_to_string(f.marks.join("staging"))?
        .trim()
        .to_owned())
}

/// The revisions the staging deploys ran, oldest first.
async fn deployed(f: &Fixture) -> BoxResult<Vec<String>> {
    let mut runs: Vec<_> = f
        .h
        .store
        .cd_runs(&SandboxId::parse("s1")?, 100)
        .await?
        .into_iter()
        .filter(|run| {
            run.environment == "staging" && run.kind == yulius_domain::rollback::CdRunKind::Deploy
        })
        .collect();
    runs.reverse();
    Ok(runs.into_iter().map(|run| run.sha).collect())
}

/// SPEC-575 acceptance (failed candidate, then a fix; a duplicate request): a task's request
/// deploys its merged revision once its full gate passed. A later task's failing candidate is
/// reported with its run id while staging keeps serving the earlier release. Its fix lands,
/// passes and is the revision verified there. Asking again answers with the same request and
/// deploys nothing.
#[tokio::test]
async fn a_failing_candidate_leaves_staging_on_its_release_and_the_fix_deploys() -> TestResult {
    let f = fixture(&staging_toml()).await?;
    agent(&f, "t1").await?;
    agent(&f, "t2").await?;

    // Nothing merged yet: nothing to deploy.
    let early = support::mcp_tool(
        &f.h,
        &token("t1"),
        "cd_deploy",
        json!({"environment": "staging"}),
    )
    .await?;
    assert_eq!(early["isError"], true, "{early}");
    assert!(early.to_string().contains("merged nothing"), "{early}");
    // Nobody deploys the staging target's head directly.
    let direct = f.deploy(&f.alice, "staging", None).await?;
    assert!(
        direct
            .body
            .to_string()
            .contains("merge-first staging target"),
        "{}",
        direct.body
    );

    let r1 = land(&f, "t1", "one.txt").await?;
    let ci1 = gate(&f, &r1, RunState::Passed).await?;
    let first = ask(&f, "t1").await?;
    assert_eq!(first["revision"], r1.as_str(), "{first}");
    assert_eq!(first["state"], "testing", "{first}");
    settle(&f).await?;
    let one = request(&f, first["id"].as_str().ok_or("id")?).await?;
    assert_eq!(one.state, StagingState::Verified, "{one:?}");
    assert_eq!(one.ci_run_id.as_deref(), Some(ci1.as_str()));
    let release1 = one.cd_run_id.clone().ok_or("deploy run")?;
    assert_eq!(f.ended(&release1).await?["commit"], r1.as_str());
    assert_eq!(serving(&f)?, r1);

    // t2's merged candidate fails its full gate: staging stays on r1, the task reads why.
    let r2 = land(&f, "t2", "two.txt").await?;
    let ci2 = gate(&f, &r2, RunState::Failed).await?;
    let failing = ask(&f, "t2").await?;
    settle(&f).await?;
    let failed = request(&f, failing["id"].as_str().ok_or("id")?).await?;
    assert_eq!(failed.state, StagingState::GateFailed, "{failed:?}");
    assert_eq!(failed.ci_run_id.as_deref(), Some(ci2.as_str()));
    let note = failed.note.clone().unwrap_or_default();
    assert!(
        note.contains(&ci2) && note.contains("full") && note.contains("staging did not change"),
        "{note}"
    );
    assert_eq!(
        serving(&f)?,
        r1,
        "staging still serves the last verified release"
    );
    assert_eq!(
        deployed(&f).await?,
        std::slice::from_ref(&r1),
        "the failing candidate never deployed"
    );
    // Asking again for the same failing revision deploys nothing: it is fixed forward.
    assert_eq!(ask(&f, "t2").await?["id"], failing["id"]);

    // The fix lands at r3 and passes: that tested revision is the one verified on staging.
    let r3 = land(&f, "t2", "fix.txt").await?;
    let ci3 = gate(&f, &r3, RunState::Passed).await?;
    let fixed = ask(&f, "t2").await?;
    assert_ne!(fixed["id"], failing["id"]);
    settle(&f).await?;
    let three = request(&f, fixed["id"].as_str().ok_or("id")?).await?;
    assert_eq!(three.state, StagingState::Verified, "{three:?}");
    assert_eq!(three.ci_run_id.as_deref(), Some(ci3.as_str()));
    let release3 = three.cd_run_id.clone().ok_or("deploy run")?;
    assert_eq!(f.ended(&release3).await?["commit"], r3.as_str());
    assert_eq!(serving(&f)?, r3);
    assert_eq!(deployed(&f).await?, [r1.clone(), r3.clone()]);

    // A duplicate request is the same request; nothing deploys again.
    let again = ask(&f, "t2").await?;
    assert_eq!(again["id"], fixed["id"]);
    assert_eq!(again["state"], "verified");
    settle(&f).await?;
    assert_eq!(deployed(&f).await?, [r1, r3]);
    Ok(())
}

/// SPEC-575 acceptance (concurrent tasks, a retried deploy): a deploy lost to a restart is
/// tried again and verified. Two tasks asking together deploy in revision order, never back to
/// an older revision. A delivery staging already contains is answered without a deploy. Each
/// task has exactly one successful staging deployment.
#[tokio::test]
async fn a_lost_deploy_is_retried_and_concurrent_tasks_deploy_in_revision_order() -> TestResult {
    let f = fixture(&staging_toml()).await?;
    for task in ["t1", "t2", "t3"] {
        agent(&f, task).await?;
    }
    let r1 = land(&f, "t1", "one.txt").await?;
    gate(&f, &r1, RunState::Passed).await?;
    let first = ask(&f, "t1").await?;
    let id1 = first["id"].as_str().ok_or("id")?.to_owned();
    // The server stopped while its deploy ran: the run reconciled as an error at the start.
    let lost = format!("lost-{}", &r1[..12]);
    let at = f.h.app.now();
    f.h.store
        .insert_cd_run(&yulius_app::ci::cd::CdRunRecord {
            id: lost.clone(),
            sandbox_id: SandboxId::parse("s1")?,
            environment: "staging".into(),
            stage: yulius_domain::cd::CdStage::Dev,
            branch: "dev".into(),
            sha: r1.clone(),
            state: RunState::Running,
            reason: None,
            requested_by: "alice".into(),
            task_id: None,
            deploy: Some("true".into()),
            verify: None,
            note: None,
            output: String::new(),
            exit_code: None,
            started_at: at,
            ended_at: None,
            kind: yulius_domain::rollback::CdRunKind::Deploy,
            source: None,
            steps: Vec::new(),
        })
        .await?;
    for (from, to) in [
        (StagingState::Testing, StagingState::Ready),
        (StagingState::Ready, StagingState::Deploying),
    ] {
        let change = StagingChange {
            cd_run_id: Some(lost.clone()),
            ..StagingChange::default()
        };
        assert!(
            f.h.store
                .move_staging_request(&id1, from, to, &change, at)
                .await?
        );
    }
    assert_eq!(
        f.h.app.reconcile_cd_runs().await?,
        1,
        "the restart's reconcile"
    );
    settle(&f).await?;
    let one = request(&f, &id1).await?;
    assert_eq!(one.state, StagingState::Verified, "{one:?}");
    assert_eq!(
        one.attempts, 2,
        "the lost attempt and the one that verified"
    );
    let retried = one.cd_run_id.clone().ok_or("deploy run")?;
    assert_ne!(retried, lost);
    assert_eq!(f.ended(&retried).await?["state"], "passed");
    assert_eq!(serving(&f)?, r1);

    // Two tasks land and ask together, the older one twice: staging moves forward in revision
    // order, never back to the older revision once the newer one is verified.
    let r2 = land(&f, "t2", "two.txt").await?;
    let r3 = land(&f, "t3", "three.txt").await?;
    gate(&f, &r2, RunState::Passed).await?;
    gate(&f, &r3, RunState::Passed).await?;
    let (two, three) = (ask(&f, "t2").await?, ask(&f, "t3").await?);
    assert_eq!(
        ask(&f, "t2").await?["id"],
        two["id"],
        "a duplicate is the same request"
    );
    settle(&f).await?;
    let two = request(&f, two["id"].as_str().ok_or("id")?).await?;
    let three = request(&f, three["id"].as_str().ok_or("id")?).await?;
    assert_eq!(three.state, StagingState::Verified, "{three:?}");
    assert!(two.state.is_success(), "{two:?}");
    assert_eq!(serving(&f)?, r3);
    let revisions = deployed(&f).await?;
    let after: Vec<&String> = revisions.iter().filter(|sha| **sha != r1).collect();
    assert!(
        after == [&r3] || after == [&r2, &r3],
        "in revision order, each at most once: {revisions:?}"
    );
    // A delivery staging already contains (here one recorded before merge-first, landing at r2)
    // is answered at once and deploys nothing.
    agent(&f, "t4").await?;
    f.h.store
        .execute_for_tests(&format!(
            "INSERT INTO task_merges(task_id, sandbox_id, merged_sha, branch, pr_number, head_sha) \
             VALUES ('t4', 's1', '{r2}', 'dev', NULL, '{r2}')"
        ))
        .await?;
    let inside = ask(&f, "t4").await?;
    assert_eq!(inside["state"], "included", "{inside}");
    assert_eq!(inside["cd_run_id"], json!(three.cd_run_id), "{inside}");
    settle(&f).await?;
    assert_eq!(deployed(&f).await?, revisions, "nothing deployed twice");
    // One successful staging deployment per task.
    for task in ["t1", "t2", "t3", "t4"] {
        let mine =
            f.h.store
                .staging_requests_of(
                    &SandboxId::parse("s1")?,
                    "staging",
                    Some(&TaskId::parse(task)?),
                    "",
                )
                .await?;
        assert_eq!(
            mine.iter().filter(|r| r.state.is_success()).count(),
            1,
            "{task}: {mine:?}"
        );
    }
    Ok(())
}

/// SPEC-575 MFS-003: a revision no run judged gets one candidate run of the full gate at exactly
/// that commit, with the full suite in it. Another round queues no second one, and a run of the
/// revision that skipped the full suite judges nothing.
#[tokio::test]
async fn a_revision_nobody_judged_gets_one_candidate_run_of_the_full_gate() -> TestResult {
    let f = fixture(&staging_toml()).await?;
    agent(&f, "t1").await?;
    let r1 = land(&f, "t1", "one.txt").await?;
    // A run at the revision that skipped the full suite (as off the merge) is no verdict.
    let skipped = gate(&f, &r1, RunState::Passed).await?;
    f.h.store
        .execute_for_tests(&format!(
            "UPDATE ci_jobs SET state = 'skipped', reason = '{}' WHERE id = '{skipped}-full'",
            yulius_domain::ci::AFTER_MERGE_SKIP_REASON
        ))
        .await?;
    let asked = ask(&f, "t1").await?;
    let sandbox =
        f.h.store
            .sandbox(&SandboxId::parse("s1")?)
            .await?
            .ok_or("s1")?;
    f.h.app.staging_advance(&sandbox, "staging").await?;
    f.h.app.staging_advance(&sandbox, "staging").await?;
    let one = request(&f, asked["id"].as_str().ok_or("id")?).await?;
    assert_eq!(one.state, StagingState::Testing, "{one:?}");
    let runs = f.h.store.ci_runs_for_sha(&sandbox.id, &r1).await?;
    let candidates: Vec<_> = runs
        .iter()
        .filter(|run| run.reference == CiRef::Candidate(r1.clone()))
        .collect();
    assert_eq!(candidates.len(), 1, "{runs:?}");
    assert_eq!(one.ci_run_id.as_deref(), Some(candidates[0].id.as_str()));
    let jobs = f.h.store.ci_jobs(&candidates[0].id, false).await?;
    assert!(
        jobs.iter()
            .any(|j| j.key == "full" && j.state != RunState::Skipped),
        "the full suite runs on the candidate: {jobs:?}"
    );
    assert!(
        deployed(&f).await?.is_empty(),
        "nothing deploys before its verdict"
    );
    Ok(())
}
