//! Merge-first staging end to end (SPEC-575 MFS-003 to MFS-008): a project whose `[cd.staging]`
//! is the merge-first target, the real Git adapter and process host deploying it, tasks' agents
//! asking through `cd_deploy`, and the full gate's verdicts recorded at exact revisions.

use std::time::Duration;

use serde_json::{Value, json};
use yulius_app::ci::{CiJobRecord, CiRunRecord};
use yulius_app::ports::store::{AuditStore as _, SandboxStore as _, UserStore as _};
use yulius_app::ports::store_ext::{
    CdStore as _, CiStore as _, DeliveryStore as _, StagingChange, StagingRequestRecord,
    StagingStore as _,
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
async fn staging_answer(f: &Fixture, task: &str) -> BoxResult<Value> {
    let answer = support::mcp_tool(
        &f.h,
        &token(task),
        "cd_deploy",
        json!({"environment": "staging"}),
    )
    .await?;
    assert_eq!(answer["isError"], false, "{answer}");
    Ok(support::tool_text(&answer))
}

async fn ask(f: &Fixture, task: &str) -> BoxResult<Value> {
    Ok(staging_answer(f, task).await?["staging"].clone())
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
    assert!(f.h.store.mark_staging_told(&failed).await?);
    let repeated = staging_answer(&f, "t2").await?;
    assert_eq!(repeated["staging"]["id"], failing["id"]);
    let text = repeated["text"].as_str().ok_or("receipt text")?;
    assert!(
        text.contains("fix it forward") && text.contains(&ci2),
        "{text}"
    );
    assert!(
        !text.contains("End this turn") && !text.contains("wakes the task"),
        "{text}"
    );

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
    assert!(f.h.store.mark_staging_told(&three).await?);
    let again = staging_answer(&f, "t2").await?;
    assert_eq!(again["staging"]["id"], fixed["id"]);
    let text = again["text"].as_str().ok_or("receipt text")?;
    assert!(
        text.contains("this task's staging deployment is done") && !text.contains("wakes the task"),
        "{text}"
    );
    assert_eq!(again["staging"]["state"], "verified");
    settle(&f).await?;
    assert_eq!(deployed(&f).await?, [r1.clone(), r3.clone()]);

    // The queue as people and agents read it, with its run and revision ids (SPEC-575).
    let overview = f.get("/api/sandboxes/s1/cd").await?;
    let staging = overview["environments"]
        .as_array()
        .ok_or("environments")?
        .iter()
        .find(|env| env["name"] == "staging")
        .ok_or("staging")?["staging"]
        .clone();
    assert_eq!(staging["verified"], r3.as_str(), "{staging}");
    let states: Vec<(&str, &str)> = staging["requests"]
        .as_array()
        .ok_or("requests")?
        .iter()
        .map(|r| {
            (
                r["revision"].as_str().unwrap_or_default(),
                r["state"].as_str().unwrap_or_default(),
            )
        })
        .collect();
    assert_eq!(
        states,
        [
            (r3.as_str(), "verified"),
            (r2.as_str(), "gate_failed"),
            (r1.as_str(), "verified")
        ],
        "{staging}"
    );
    let status = support::mcp_tool(&f.h, &token("t2"), "cd_status", json!({})).await?;
    let text = support::tool_text(&status)["text"]
        .as_str()
        .unwrap_or_default()
        .to_owned();
    assert!(
        text.contains(&format!("verified revision {}", &r3[..12]))
            && text.contains(&format!(
                "request {} task #",
                fixed["id"].as_str().unwrap_or("?")
            ))
            && text.contains(&format!("CI run {ci2}"))
            && text.contains(&format!("CD run {release3}")),
        "{text}"
    );
    Ok(())
}

/// A previously admitted request loses its gate before any run is queued. Restoring CI
/// leaves that revision's decision final. The next delivery can be judged on its own gate.
#[tokio::test]
async fn a_gate_failure_without_a_run_directs_the_task_to_a_new_landing() -> TestResult {
    let f = fixture(&staging_toml()).await?;
    agent(&f, "t1").await?;
    let revision = land(&f, "t1", "one.txt").await?;
    let sandbox = sandbox(&f).await?;
    let pending = StagingRequestRecord {
        id: "sr_without_gate".into(),
        sandbox_id: sandbox.id.clone(),
        task_id: Some(TaskId::parse("t1")?),
        environment: "staging".into(),
        revision,
        state: StagingState::Testing,
        ci_run_id: None,
        cd_run_id: None,
        attempts: 0,
        note: None,
        requested_by: "task:1".into(),
        told: false,
        created_at: f.h.app.now(),
        updated_at: f.h.app.now(),
    };
    assert!(f.h.store.insert_staging_request(&pending).await?);
    let mut no_ci = f.h.app.config.clone();
    no_ci.ci.enabled = false;
    let disabled = yulius_app::context::App::new(no_ci, f.h.app.deps.clone());
    disabled.staging_advance(&sandbox, "staging").await?;
    let failed = request(&f, &pending.id).await?;
    assert_eq!(failed.state, StagingState::GateFailed);
    assert!(failed.ci_run_id.is_none());

    let repeated = staging_answer(&f, "t1").await?;
    assert_eq!(repeated["staging"]["id"], pending.id);
    let text = repeated["text"].as_str().ok_or("terminal receipt")?;
    assert!(
        text.contains("no CI run")
            && text.contains("fix it forward")
            && text.contains("delivery_merge"),
        "{text}"
    );
    assert!(deployed(&f).await?.is_empty());

    let fixed = land(&f, "t1", "restored-gate.txt").await?;
    gate(&f, &fixed, RunState::Passed).await?;
    let next = ask(&f, "t1").await?;
    assert_ne!(next["id"], pending.id);
    settle(&f).await?;
    assert_eq!(serving(&f)?, fixed);
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
    let previous = staging_answer(&f, "t1").await?;
    assert_eq!(previous["staging"]["revision"], r1);
    let text = previous["text"].as_str().ok_or("verified receipt")?;
    assert!(
        text.contains("Staging deployed and verified your revision")
            && !text.contains("Staging serves your tested revision"),
        "the receipt for r1 is historical while staging serves r3: {text}"
    );
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
    let included = staging_answer(&f, "t4").await?;
    let text = included["text"].as_str().ok_or("included receipt")?;
    assert!(
        text.contains("included your revision")
            && text.contains(three.cd_run_id.as_deref().ok_or("containing run")?),
        "{text}"
    );
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

/// A rollback that redeploys the kept release (it marks the release's commit) and a verify that
/// checks what it should serve: the release's commit in a rollback, else the deployed one.
const ROLLBACK: &str = "#!/bin/sh\necho \"$YULIUS_CD_RELEASE_SHA\" > \"$MARKS/staging\"\necho restored >> \"$MARKS/restores\"\n";
const STAGING_VERIFY: &str = "#!/bin/sh\ntest \"$(cat \"$MARKS/staging\")\" = \"${YULIUS_CD_RELEASE_SHA:-$YULIUS_CD_SHA}\" && echo verified\n";

/// SPEC-575 MFS-007: a revision whose full gate passed but whose deploy fails is
/// `deploy_failed` with that run. Yulius puts staging back on its last verified release with one
/// restore (a rollback run of that release, verified), never a second one, and the same
/// revision may be asked again (a retried deployment).
#[tokio::test]
async fn a_failed_deploy_puts_staging_back_on_its_last_verified_release() -> TestResult {
    let toml = staging_toml()
        .replace(
            "rollback = \"true\"",
            "rollback = \"MARKS={marks} sh scripts/cd/rollback.sh\"",
        )
        .replace(
            "verify = \"MARKS={marks} sh scripts/cd/verify.sh\"\n\
     rollback",
            "verify = \"MARKS={marks} sh scripts/cd/staging-verify.sh\"\n\
     rollback",
        );
    let f = fixture(&toml).await?;
    assert!(toml.contains("staging-verify.sh"), "{toml}");
    agent(&f, "t1").await?;
    agent(&f, "t2").await?;
    let r1 = f.push(
        "dev",
        &[
            ("scripts/cd/rollback.sh", Some(ROLLBACK)),
            ("scripts/cd/staging-verify.sh", Some(STAGING_VERIFY)),
        ],
    )?;
    f.h.store
        .execute_for_tests(&format!(
            "INSERT INTO task_merges(task_id, sandbox_id, merged_sha, branch, pr_number, head_sha) \
             VALUES ('t1', 's1', '{r1}', 'dev', NULL, '{r1}')"
        ))
        .await?;
    gate(&f, &r1, RunState::Passed).await?;
    let first = ask(&f, "t1").await?;
    settle(&f).await?;
    assert_eq!(
        request(&f, first["id"].as_str().ok_or("id")?).await?.state,
        StagingState::Verified
    );
    assert_eq!(serving(&f)?, r1);

    // r2 passes its gate, but its deploy fails (the deploy script exits on `fail`).
    let r2 = land(&f, "t2", "fail").await?;
    gate(&f, &r2, RunState::Passed).await?;
    let second = ask(&f, "t2").await?;
    settle(&f).await?;
    let failed = request(&f, second["id"].as_str().ok_or("id")?).await?;
    assert_eq!(failed.state, StagingState::DeployFailed, "{failed:?}");
    let failed_run = failed.cd_run_id.clone().ok_or("run")?;
    assert_eq!(f.ended(&failed_run).await?["state"], "failed");
    // One restore, verified, of r1's release; another round adds none.
    settle(&f).await?;
    let sandbox = SandboxId::parse("s1")?;
    let restores =
        |runs: Vec<yulius_app::ci::cd::CdRunRecord>| -> Vec<yulius_app::ci::cd::CdRunRecord> {
            runs.into_iter()
                .filter(|run| {
                    run.kind == yulius_domain::rollback::CdRunKind::Rollback
                        && run
                            .note
                            .as_deref()
                            .is_some_and(|n| n.contains(&format!("after failed run {failed_run}")))
                })
                .collect()
        };
    let deadline = tokio::time::Instant::now() + WAIT;
    let restore = loop {
        let found = restores(f.h.store.cd_runs(&sandbox, 50).await?);
        if let Some(run) = found.first()
            && run.state != RunState::Running
        {
            break run.clone();
        }
        if tokio::time::Instant::now() > deadline {
            return Err("no restore ended".into());
        }
        settle(&f).await?;
        tokio::time::sleep(Duration::from_millis(100)).await;
    };
    assert_eq!(restore.state, RunState::Passed, "{restore:?}");
    assert_eq!(serving(&f)?, r1, "back on the last verified release");
    settle(&f).await?;
    assert_eq!(restores(f.h.store.cd_runs(&sandbox, 50).await?).len(), 1);
    assert_eq!(
        std::fs::read_to_string(f.marks.join("restores"))?
            .lines()
            .count(),
        1,
        "restored once"
    );
    // The same revision may be asked again: a retried deployment of the failed request.
    let again = ask(&f, "t2").await?;
    assert_eq!(again["id"], second["id"]);
    assert_eq!(again["state"], "ready");
    Ok(())
}

/// SPEC-575 MFS-009: with a merge-first target, a person's production deploy promotes only the
/// revision staging verified, including while the next integration commit is still checking.
#[tokio::test]
async fn production_is_promoted_only_from_the_verified_staging_revision() -> TestResult {
    let toml = staging_toml().replace(
        "[cd.prod]\nabout = \"nothing yet\"\n",
        "[cd.prod]\nabout = \"a push to main is the release\"\npromote = true\n",
    );
    let f = fixture(&toml).await?;
    agent(&f, "t1").await?;
    agent(&f, "t2").await?;
    let r1 = land(&f, "t1", "one.txt").await?;
    gate(&f, &r1, RunState::Passed).await?;
    // Nothing verified on staging yet: no promotion.
    let early = f.deploy(&f.bob, "prod", Some("app")).await?;
    assert!(
        early
            .body
            .to_string()
            .contains("staging verified nothing yet"),
        "{}",
        early.body
    );
    assert_ne!(f.head("main")?, r1);
    ask(&f, "t1").await?;
    settle(&f).await?;
    assert_eq!(serving(&f)?, r1);
    // A newer revision is checking. The page offers the verified revision for confirmation.
    let r2 = land(&f, "t2", "two.txt").await?;
    let checking = gate(&f, &r2, RunState::Running).await?;
    crate::cd::git(
        &f.h.app.config.data_dir.join("sandboxes/s1/repo"),
        &["fetch", "-q", "origin"],
    )?;
    f.h.store
        .execute_for_tests(
            "INSERT INTO delivery (task_id, stage, pr_number, updated_at)
         VALUES ('t1', 'merged_integration', NULL, 1), ('t2', 'merged_integration', NULL, 1)",
        )
        .await?;
    let overview = f.get("/api/sandboxes/s1/cd").await?;
    let prod = overview["environments"]
        .as_array()
        .ok_or("envs")?
        .iter()
        .find(|env| env["name"] == "prod")
        .ok_or("prod")?;
    assert_eq!(prod["promotes_from"], r1);
    // An existing release ref pointing elsewhere is refused and never overwritten.
    let pin = format!("refs/heads/yulius/release/{r1}");
    crate::cd::git(&f.origin, &["update-ref", &pin, &r2])?;
    let moved = f.deploy(&f.bob, "prod", Some("app")).await?;
    assert_eq!(moved.status, axum::http::StatusCode::CONFLICT);
    assert_eq!(crate::cd::git(&f.origin, &["rev-parse", &pin])?, r2);
    assert!(!git_contains(&f, "main", &r1)?);
    crate::cd::git(&f.origin, &["update-ref", "-d", &pin, &r2])?;
    // An open promotion PR for the moving dev head remains independent of A's release.
    use yulius_app::ports::github::{GitHubPort as _, NewPull};
    let current_head_pr =
        f.h.github
            .create_pull(
                &yulius_app::model::Secret("fixture-bob".into()),
                "acme",
                "app",
                &NewPull {
                    head: "dev".into(),
                    base: "main".into(),
                    title: "release: promote dev to main".into(),
                    body: String::new(),
                },
            )
            .await?;
    f.h.github
        .concurrent_release_ref
        .store(true, std::sync::atomic::Ordering::SeqCst);
    // GitHub lands the release, then reading its resulting commit fails. A retry must
    // reconcile the landed tasks after confirming the tree, without another merge.
    f.h.github
        .fail_pinned_merge_read
        .store(true, std::sync::atomic::Ordering::SeqCst);
    let uncertain = f.deploy(&f.bob, "prod", Some("app")).await?;
    assert!(!uncertain.status.is_success(), "{}", uncertain.body);
    assert!(git_contains(&f, "main", &r1)?);
    assert_eq!(
        f.h.github
            .pull(current_head_pr.number)
            .ok_or("other promotion")?
            .state,
        "open"
    );

    // The release reached GitHub, but its required checks must still permit the retry.
    let mut recheck =
        f.h.store
            .ci_run(&format!("ci-{}", &r1[..12]))
            .await?
            .ok_or("first gate")?;
    recheck.id = "recheck-staged-release".into();
    recheck.state = RunState::Failed;
    f.h.store.insert_ci_run(&recheck, &[]).await?;
    let refused = f.deploy(&f.bob, "prod", Some("app")).await?;
    assert_eq!(
        refused.status,
        axum::http::StatusCode::CONFLICT,
        "{}",
        refused.body
    );
    assert_eq!(
        f.h.store
            .delivery(&TaskId::parse("t1")?)
            .await?
            .ok_or("t1")?
            .stage,
        yulius_domain::delivery::DeliveryStage::MergedIntegration
    );
    f.h.store
        .execute_for_tests(
            "UPDATE ci_runs SET state = 'passed' WHERE id = 'recheck-staged-release'",
        )
        .await?;

    let deploy = f.deploy(&f.bob, "prod", Some("app")).await?;
    assert!(deploy.status.is_success(), "{}", deploy.body);
    let run = f
        .ended(deploy.body["run_id"].as_str().ok_or("run")?)
        .await?;
    assert_eq!(run["state"], "passed");
    assert!(git_contains(&f, "main", &r1)?);
    assert!(!git_contains(&f, "main", &r2)?);
    use yulius_domain::delivery::DeliveryStage;
    assert_eq!(
        f.h.store
            .delivery(&TaskId::parse("t1")?)
            .await?
            .ok_or("t1 stage")?
            .stage,
        DeliveryStage::ProductionDone
    );
    assert_eq!(
        f.h.store
            .delivery(&TaskId::parse("t2")?)
            .await?
            .ok_or("t2 stage")?
            .stage,
        DeliveryStage::MergedIntegration
    );
    // Staging verifies r2: the same deploy now promotes it.
    f.h.store
        .execute_for_tests(&format!(
            "UPDATE ci_runs SET state = 'passed' WHERE id = '{checking}';
         UPDATE ci_jobs SET state = 'passed' WHERE run_id = '{checking}'"
        ))
        .await?;
    ask(&f, "t2").await?;
    settle(&f).await?;
    assert_eq!(serving(&f)?, r2);
    let promoted = f.deploy(&f.bob, "prod", Some("app")).await?;
    assert!(promoted.status.is_success(), "{}", promoted.body);
    let run = f
        .ended(promoted.body["run_id"].as_str().ok_or("run")?)
        .await?;
    assert_eq!(run["state"], "passed", "{run}");
    assert!(
        git_contains(&f, "main", &r2)?,
        "main holds the verified revision"
    );
    Ok(())
}

/// Without a staging target, branch CI supplies the release while the next head runs.
#[tokio::test]
async fn running_dev_ci_releases_only_the_preceding_green_revision() -> TestResult {
    let f = fixture(
        "[[ci.jobs]]\nname = \"full\"\ncommand = \"true\"\n[cd.dev]\n[cd.prod]\npromote = true\n",
    )
    .await?;
    fast_forward_profile(&f, false).await?;
    let a = f.push("dev", &[("a.txt", Some("verified"))])?;
    let b = f.push("dev", &[("b.txt", Some("still running"))])?;
    let active = gate(&f, &b, RunState::Running).await?;
    let first = f.deploy(&f.bob, "prod", Some("app")).await?;
    assert_eq!(
        first.status,
        axum::http::StatusCode::CONFLICT,
        "{}",
        first.body
    );
    // A pass retained from a former branch history cannot become a production candidate.
    let side = f.push("old-dev", &[("side.txt", Some("old history"))])?;
    gate(&f, &side, RunState::Passed).await?;
    let overview = f.get("/api/sandboxes/s1/cd").await?;
    assert!(
        overview["environments"][1]["promotes_from"].is_null(),
        "{overview}"
    );
    let off_branch = f.deploy(&f.bob, "prod", Some("app")).await?;
    assert_eq!(
        off_branch.status,
        axum::http::StatusCode::CONFLICT,
        "{}",
        off_branch.body
    );
    gate(&f, &a, RunState::Passed).await?;
    // Dev moves once more before the CI scheduler admits C. B is still verifying.
    let c = f.push("dev", &[("newest.txt", Some("awaiting CI admission"))])?;
    let deployed = f.deploy(&f.bob, "prod", Some("app")).await?;
    assert!(deployed.status.is_success(), "{}", deployed.body);
    assert_eq!(f.head("main")?, a);
    assert_eq!(
        f.h.store.ci_run(&active).await?.ok_or("active")?.state,
        RunState::Running
    );
    // A green rerun of older A must not displace C when the next revision checks.
    gate(&f, &c, RunState::Passed).await?;
    let mut rerun =
        f.h.store
            .ci_run(&format!("ci-{}", &a[..12]))
            .await?
            .ok_or("A")?;
    rerun.id = "green-rerun-A".into();
    f.h.store.insert_ci_run(&rerun, &[]).await?;
    // Cross a complete batch of first-parent history without any intervening verdict.
    let mut d = f.push("dev", &[("d.txt", Some("checking"))])?;
    let tree = crate::cd::git(&f.work, &["rev-parse", "HEAD^{tree}"])?;
    for _ in 0..128 {
        d = crate::cd::git(&f.work, &["commit-tree", &tree, "-p", &d, "-m", "pending"])?;
    }
    crate::cd::git(
        &f.work,
        &["push", "-q", "origin", &format!("{d}:refs/heads/dev")],
    )?;
    gate(&f, &d, RunState::Running).await?;
    let newest = f.deploy(&f.bob, "prod", Some("app")).await?;
    assert!(newest.status.is_success(), "{}", newest.body);
    assert_eq!(f.head("main")?, c);
    Ok(())
}

async fn deploy_with_shared_ci(newer_pending: bool) -> TestResult {
    let f = fixture(concat!(
        "[[ci.jobs]]\nname = \"security\"\ncommand = \"yulius security-scan --ci\"\n",
        "[[ci.jobs]]\nname = \"full\"\ncommand = \"true\"\npaths = [\"src/**\"]\n",
        "[cd.prod]\npromote = true\n",
    ))
    .await?;
    fast_forward_profile(&f, false).await?;
    let a = f.push("dev", &[("verified.txt", Some("A"))])?;
    let id = gate(&f, &a, RunState::Passed).await?;
    // A disabled security scan is a legitimate skip of the configured job.
    f.h.store.execute_for_tests(&format!("UPDATE ci_jobs SET name='security', job_key='security', command='yulius security-scan --ci',
        state='skipped', reason=NULL, exit_code=78 WHERE run_id='{id}' AND name='quick'")).await?;
    if newer_pending {
        let b = f.push("dev", &[("pending.txt", Some("B"))])?;
        gate(&f, &b, RunState::Running).await?;
    }
    let original = f.head("main")?;
    f.h.store
        .execute_for_tests(&format!(
            "UPDATE ci_runs SET reference = 'train/1' WHERE id = '{id}'"
        ))
        .await?;
    let private = f.deploy(&f.bob, "prod", Some("app")).await?;
    assert_eq!(
        private.status,
        axum::http::StatusCode::CONFLICT,
        "{}",
        private.body
    );
    let skipped = yulius_domain::ci::AFTER_MERGE_SKIP_REASON;
    f.h.store.execute_for_tests(&format!("UPDATE ci_runs SET reference = 'pr/17' WHERE id = '{id}';
        UPDATE ci_jobs SET state = 'skipped', reason = '{skipped}' WHERE run_id = '{id}' AND name = 'full'")).await?;
    let partial = f.deploy(&f.bob, "prod", Some("app")).await?;
    assert_eq!(
        partial.status,
        axum::http::StatusCode::CONFLICT,
        "{}",
        partial.body
    );
    assert_eq!(f.head("main")?, original);
    f.h.store.execute_for_tests(&format!("UPDATE ci_jobs SET state = 'passed', reason = NULL WHERE run_id = '{id}' AND name = 'full'")).await?;
    // A copied path skip can carry a generic note. A non-final train verdict looks like
    // a PR run, although only the later combined tree ran its jobs. Neither verifies A.
    let (label, change) = if newer_pending {
        (
            "path skip",
            format!(
                "UPDATE ci_jobs SET state='skipped', reason='copied verdict' WHERE run_id='{id}' AND name='full'"
            ),
        )
    } else {
        (
            "non-final train step",
            format!(
                "UPDATE ci_runs SET requested_by='{}' WHERE id='{id}'",
                yulius_app::merge_train::TRAIN_VERDICT_BY
            ),
        )
    };
    f.h.store.execute_for_tests(&change).await?;
    let incomplete = f.deploy(&f.bob, "prod", Some("app")).await?;
    assert_eq!(
        incomplete.status,
        axum::http::StatusCode::CONFLICT,
        "{label}: {}",
        incomplete.body
    );
    f.h.store
        .execute_for_tests(&format!(
            "UPDATE ci_jobs SET state='passed', reason=NULL WHERE run_id='{id}' AND name='full';
         UPDATE ci_runs SET requested_by='yulius' WHERE id='{id}'"
        ))
        .await?;
    // A green job for an earlier command cannot satisfy this commit's gate.
    f.h.store
        .execute_for_tests(&format!(
            "UPDATE ci_jobs SET command='old command' WHERE run_id='{id}' AND name='full'"
        ))
        .await?;
    let changed = f.deploy(&f.bob, "prod", Some("app")).await?;
    assert_eq!(
        changed.status,
        axum::http::StatusCode::CONFLICT,
        "{}",
        changed.body
    );
    f.h.store
        .execute_for_tests(&format!(
            "UPDATE ci_jobs SET command='true' WHERE run_id='{id}' AND name='full'"
        ))
        .await?;
    let released = f.deploy(&f.bob, "prod", Some("app")).await?;
    assert!(released.status.is_success(), "{}", released.body);
    assert_eq!(f.head("main")?, a);
    f.ended(released.body["run_id"].as_str().ok_or("run")?)
        .await?;
    let mut explicit = f.h.store.ci_run(&id).await?.ok_or("shared run")?;
    explicit.id = "explicit-branch-rerun".into();
    explicit.reference = CiRef::Branch("dev".into());
    explicit.state = RunState::Running;
    explicit.ended_at = None;
    let mut jobs = f.h.store.ci_jobs(&id, false).await?;
    for job in &mut jobs {
        job.id = format!("explicit-{}", job.key);
        job.run_id.clone_from(&explicit.id);
        if job.key == "full" {
            // Path filters do not prevent a branch job from running. It can start and
            // decide to skip, or have that executed verdict copied from the same tree.
            job.state = RunState::Skipped;
            job.exit_code = Some(yulius_domain::ci::SKIP_EXIT_CODE);
            job.reason = newer_pending.then(|| yulius_domain::ci::EXECUTED_SKIP_REASON.into());
        } else {
            job.state = RunState::Passed;
            job.exit_code = Some(0);
            job.reason = None;
        }
    }
    f.h.store.insert_ci_run(&explicit, &jobs).await?;
    let checking = f.deploy(&f.bob, "prod", Some("app")).await?;
    assert_eq!(
        checking.status,
        axum::http::StatusCode::CONFLICT,
        "{}",
        checking.body
    );
    f.h.store
        .execute_for_tests("UPDATE ci_runs SET state='passed' WHERE id='explicit-branch-rerun'")
        .await?;
    let checked = f.deploy(&f.bob, "prod", Some("app")).await?;
    assert!(
        checked.status.is_success(),
        "executed path-scoped job: {}",
        checked.body
    );
    assert_eq!(f.head("main")?, a);
    Ok(())
}

/// Dev can deploy a fast-forwarded SHA that the ordinary scheduler already tested in full.
#[tokio::test]
async fn dev_head_accepts_its_full_shared_ci_verdict() -> TestResult {
    deploy_with_shared_ci(false).await
}

/// The same shared verification stays eligible after the next commit starts checking.
#[tokio::test]
async fn prior_dev_revision_accepts_its_full_shared_ci_verdict() -> TestResult {
    deploy_with_shared_ci(true).await
}

/// Rerunning A while B is current can apply B's job configuration to A.
async fn historical_rerun_covers_the_release(branch: bool) -> TestResult {
    let config = "[[ci.jobs]]\nname = \"full\"\ncommand = \"true\"\n[cd.prod]\npromote = true\n";
    let f = fixture(config).await?;
    fast_forward_profile(&f, false).await?;
    let a = f.push("dev", &[("candidate.txt", Some("A"))])?;
    let original = gate(&f, &a, RunState::Failed).await?;
    let revised = if branch {
        config.replace("command = \"true\"", "command = \"echo easier\"")
    } else {
        config.replace("[cd.prod]", "paths = [\"src/**\"]\n[cd.prod]")
    };
    let b = f.push("dev", &[("yulius.toml", Some(&revised))])?;
    gate(&f, &b, RunState::Running).await?;
    let mut run = f.h.store.ci_run(&original).await?.ok_or("old run")?;
    if !branch {
        run.reference = CiRef::PullRequest(17);
        f.h.store
            .execute_for_tests(&format!(
                "UPDATE ci_runs SET reference='pr/17' WHERE id='{original}'"
            ))
            .await?;
    }
    run.id = "historical-rerun".into();
    run.state = RunState::Passed;
    let mut jobs = f.h.store.ci_jobs(&original, false).await?;
    for job in &mut jobs {
        job.id = format!("rerun-{}", job.key);
        job.run_id.clone_from(&run.id);
        if job.key == "full" {
            if branch {
                job.state = RunState::Passed;
                job.exit_code = Some(0);
                job.command = "echo easier".into();
            } else {
                job.state = RunState::Skipped;
                job.exit_code = Some(yulius_domain::ci::SKIP_EXIT_CODE);
                job.reason = Some(yulius_domain::ci::PATHS_SKIP_REASON.into());
            }
        }
    }
    f.h.store.insert_ci_run(&run, &jobs).await?;
    let production = f.head("main")?;
    let refused = f.deploy(&f.bob, "prod", Some("app")).await?;
    assert_eq!(
        refused.status,
        axum::http::StatusCode::CONFLICT,
        "{}",
        refused.body
    );
    assert_eq!(f.head("main")?, production);
    if !branch {
        // Old copied verdicts have lost the reason for their skip.
        f.h.store
            .execute_for_tests(
                "UPDATE ci_jobs SET reason='not run again: same tree' WHERE id='rerun-full'",
            )
            .await?;
        let ambiguous = f.deploy(&f.bob, "prod", Some("app")).await?;
        assert_eq!(
            ambiguous.status,
            axum::http::StatusCode::CONFLICT,
            "{}",
            ambiguous.body
        );
        assert_eq!(f.head("main")?, production);
    }
    f.h.store.execute_for_tests("UPDATE ci_jobs SET state='passed', command='true', reason=NULL, exit_code=0 WHERE id='rerun-full'").await?;
    let released = f.deploy(&f.bob, "prod", Some("app")).await?;
    assert!(released.status.is_success(), "{}", released.body);
    assert_eq!(f.head("main")?, a);
    Ok(())
}

#[tokio::test]
async fn an_explicit_dev_rerun_must_execute_the_releases_own_job() -> TestResult {
    historical_rerun_covers_the_release(true).await
}

#[tokio::test]
async fn a_pr_rerun_cannot_skip_an_unscoped_release_job() -> TestResult {
    historical_rerun_covers_the_release(false).await
}

/// Removing staging in a newer commit cannot release a candidate whose own staging never passed.
#[tokio::test]
async fn the_release_keeps_its_own_staging_requirement() -> TestResult {
    let f = fixture(&promoting_toml()).await?;
    let a = f.push("dev", &[("a.txt", Some("branch CI only"))])?;
    gate(&f, &a, RunState::Passed).await?;
    let config = "[[ci.jobs]]\nname = \"checks\"\ncommand = \"true\"\n[cd.prod]\npromote = true\n";
    let b = f.push("dev", &[("yulius.toml", Some(config))])?;
    gate(&f, &b, RunState::Running).await?;
    let production = f.head("main")?;
    let refused = f.deploy(&f.bob, "prod", Some("app")).await?;
    assert_eq!(
        refused.status,
        axum::http::StatusCode::CONFLICT,
        "{}",
        refused.body
    );
    assert_eq!(f.head("main")?, production);
    let overview = f.get("/api/sandboxes/s1/cd").await?;
    let prod = overview["environments"]
        .as_array()
        .ok_or("environments")?
        .iter()
        .find(|env| env["name"] == "prod")
        .ok_or("prod")?;
    assert!(prod["promotes_from"].is_null(), "{prod}");
    Ok(())
}

/// Staging can serve B while its own rollback proof is running. Its previous proof
/// supplies A only when both the drill and staging verified that revision.
#[tokio::test]
async fn staging_and_the_running_drill_agree_on_the_release_revision() -> TestResult {
    let config = promoting_toml()
        .replace(
            "[cd.prod]",
            "[cd.prod]\ndeploy = \"true\"\nrollback = \"true\"\nmigrations = \"none: no data\"",
        )
        .replace(
            "[cd.dev]",
            "[cd.dev]\ndeploy = \"true\"\nrollback = \"true\"\nmigrations = \"none: no data\"",
        );
    let f = fixture(&config).await?;
    let original = f.head("main")?;
    agent(&f, "t1").await?;
    let a = land(&f, "t1", "first.txt").await?;
    gate(&f, &a, RunState::Passed).await?;
    ask(&f, "t1").await?;
    settle(&f).await?;
    f.prove("dev").await?;
    agent(&f, "t2").await?;
    let b = land(&f, "t2", "second.txt").await?;
    gate(&f, &b, RunState::Passed).await?;
    ask(&f, "t2").await?;
    settle(&f).await?;
    let proof_id = format!("drill-{}", &a[..12]);
    let mut running =
        f.h.store
            .cd_run(&proof_id, false)
            .await?
            .ok_or("A's drill")?;
    running.id = "drill-current-staged".into();
    running.sha.clone_from(&b);
    running.state = RunState::Running;
    running.ended_at = None;
    f.h.store.insert_cd_run(&running).await?;
    crate::cd::git(
        &f.h.app.config.data_dir.join("sandboxes/s1/repo"),
        &["fetch", "-q", "origin"],
    )?;
    let offered = |overview: Value| -> BoxResult<Value> {
        Ok(overview["environments"]
            .as_array()
            .ok_or("environments")?
            .iter()
            .find(|env| env["name"] == "prod")
            .ok_or("prod")?["promotes_from"]
            .clone())
    };
    assert_eq!(offered(f.get("/api/sandboxes/s1/cd").await?)?, a);
    f.h.store
        .execute_for_tests(&format!(
            "UPDATE cd_runs SET state='failed' WHERE id='{proof_id}'"
        ))
        .await?;
    assert!(offered(f.get("/api/sandboxes/s1/cd").await?)?.is_null());
    // Rechecking the same commit cannot borrow its own old passing attempt.
    f.h.store
        .execute_for_tests(&format!(
            "UPDATE cd_runs SET state='passed', sha='{b}' WHERE id='{proof_id}'"
        ))
        .await?;
    assert!(offered(f.get("/api/sandboxes/s1/cd").await?)?.is_null());
    // A drill for a revision that was never staged cannot stand in for the prior release.
    f.h.store
        .execute_for_tests(&format!(
            "UPDATE cd_runs SET sha='{original}' WHERE id='{proof_id}'"
        ))
        .await?;
    assert!(offered(f.get("/api/sandboxes/s1/cd").await?)?.is_null());
    // A drill for C does not displace already-staged B.
    let c = f.push("dev", &[("third.txt", Some("checking"))])?;
    f.h.store
        .execute_for_tests(&format!(
            "UPDATE cd_runs SET sha='{c}' WHERE id='drill-current-staged'"
        ))
        .await?;
    assert_eq!(offered(f.get("/api/sandboxes/s1/cd").await?)?, b);
    Ok(())
}

/// When dev retires staging, a previously verified staging release remains eligible while
/// its unstaged successor and the new configuration cannot qualify.
#[tokio::test]
async fn retiring_staging_keeps_its_last_verified_release_available() -> TestResult {
    let f = fixture(&promoting_toml()).await?;
    agent(&f, "t1").await?;
    let a = land(&f, "t1", "staged.txt").await?;
    gate(&f, &a, RunState::Passed).await?;
    ask(&f, "t1").await?;
    settle(&f).await?;
    let b = f.push("dev", &[("unstaged.txt", Some("only CI passed"))])?;
    gate(&f, &b, RunState::Passed).await?;
    let config = "[[ci.jobs]]\nname = \"checks\"\ncommand = \"true\"\n[cd.prod]\npromote = true\n";
    let c = f.push("dev", &[("yulius.toml", Some(config))])?;
    gate(&f, &c, RunState::Running).await?;
    crate::cd::git(
        &f.h.app.config.data_dir.join("sandboxes/s1/repo"),
        &["fetch", "-q", "origin"],
    )?;
    let overview = f.get("/api/sandboxes/s1/cd").await?;
    let prod = overview["environments"]
        .as_array()
        .ok_or("environments")?
        .iter()
        .find(|env| env["name"] == "prod")
        .ok_or("prod")?;
    assert_eq!(prod["promotes_from"], a, "{prod}");
    let released = f.deploy(&f.bob, "prod", Some("app")).await?;
    assert!(released.status.is_success(), "{}", released.body);
    assert!(git_contains(&f, "main", &a)?);
    assert!(!git_contains(&f, "main", &b)?);
    assert!(!git_contains(&f, "main", &c)?);
    Ok(())
}

/// If production changes in the hosting provider's merge window, CD refuses the resulting
/// tree and keeps the included task on integration.
#[tokio::test]
async fn a_concurrent_production_change_never_records_the_wrong_tree_as_released() -> TestResult {
    let f = fixture(&promoting_toml()).await?;
    let unrelated = f.push("other-release", &[("other.txt", Some("concurrent"))])?;
    agent(&f, "t1").await?;
    let a = land(&f, "t1", "a.txt").await?;
    gate(&f, &a, RunState::Passed).await?;
    ask(&f, "t1").await?;
    settle(&f).await?;
    f.push("dev", &[("b.txt", Some("pending"))])?;
    f.h.store.execute_for_tests("INSERT INTO delivery (task_id, stage, pr_number, updated_at) VALUES ('t1', 'merged_integration', NULL, 1)").await?;
    *f.h.github
        .advance_base_on_merge
        .lock()
        .map_err(|_| "poisoned")? = Some(unrelated);
    let refused = f.deploy(&f.bob, "prod", Some("app")).await?;
    assert_eq!(
        refused.status,
        axum::http::StatusCode::CONFLICT,
        "{}",
        refused.body
    );
    assert_eq!(
        f.h.store
            .delivery(&TaskId::parse("t1")?)
            .await?
            .ok_or("stage")?
            .stage,
        yulius_domain::delivery::DeliveryStage::MergedIntegration
    );
    Ok(())
}

/// A same-tree commit with an unexpected acknowledgement is rechecked before recording
/// deliveries. A retry honors the delivery profile's optional checks.
#[tokio::test]
async fn a_release_retry_verifies_the_merge_receipt_and_honors_optional_checks() -> TestResult {
    let f = fixture(&promoting_toml()).await?;
    agent(&f, "t1").await?;
    let a = land(&f, "t1", "verified.txt").await?;
    gate(&f, &a, RunState::Passed).await?;
    ask(&f, "t1").await?;
    settle(&f).await?;
    f.push("dev", &[("pending.txt", Some("B"))])?;
    f.h.store.execute_for_tests("INSERT INTO delivery (task_id, stage, pr_number, updated_at) VALUES ('t1', 'merged_integration', NULL, 1)").await?;
    f.h.github
        .report_source_as_merge
        .store(true, std::sync::atomic::Ordering::SeqCst);
    let refused = f.deploy(&f.bob, "prod", Some("app")).await?;
    assert_eq!(
        refused.status,
        axum::http::StatusCode::CONFLICT,
        "{}",
        refused.body
    );
    let landed = f.head("main")?;
    assert_ne!(landed, a);
    assert_eq!(
        crate::cd::git(&f.origin, &["rev-parse", "main^{tree}"])?,
        crate::cd::git(&f.origin, &["rev-parse", &format!("{a}^{{tree}}")])?
    );
    let id = TaskId::parse("t1")?;
    assert_eq!(
        f.h.store.delivery(&id).await?.ok_or("t1")?.stage,
        yulius_domain::delivery::DeliveryStage::MergedIntegration
    );
    fast_forward_profile(&f, false).await?;
    let mut recheck =
        f.h.store
            .ci_run(&format!("ci-{}", &a[..12]))
            .await?
            .ok_or("gate")?;
    recheck.id = "optional-recheck".into();
    recheck.state = RunState::Failed;
    f.h.store.insert_ci_run(&recheck, &[]).await?;
    let retry = f.deploy(&f.bob, "prod", Some("app")).await?;
    assert!(retry.status.is_success(), "{}", retry.body);
    assert_eq!(f.head("main")?, landed);
    assert_eq!(
        f.h.store.delivery(&id).await?.ok_or("t1")?.stage,
        yulius_domain::delivery::DeliveryStage::ProductionDone
    );
    Ok(())
}

/// Whether `branch` of origin contains `sha`.
fn git_contains(f: &Fixture, branch: &str, sha: &str) -> BoxResult<bool> {
    Ok(std::process::Command::new("git")
        .args(["merge-base", "--is-ancestor", sha, branch])
        .current_dir(&f.origin)
        .status()?
        .success())
}

async fn sandbox(f: &Fixture) -> BoxResult<yulius_app::model::SandboxRecord> {
    Ok(f.h
        .store
        .sandbox(&SandboxId::parse("s1")?)
        .await?
        .ok_or("s1")?)
}

/// The agent's `cd_deploy` of staging when it is refused: its error text.
async fn refused(f: &Fixture, task: &str) -> BoxResult<String> {
    let answer = support::mcp_tool(
        &f.h,
        &token(task),
        "cd_deploy",
        json!({"environment": "staging"}),
    )
    .await?;
    assert_eq!(answer["isError"], true, "{answer}");
    Ok(answer.to_string())
}

/// SPEC-575 review round 1 of slice 2 (B1): staging deploys for a task under its owner's write
/// access, as every deploy for a task does. A request is refused without it, and a request
/// whose owner lost it before its turn ends `deploy_failed` with nothing deployed.
#[tokio::test]
async fn staging_deploys_only_under_the_owners_write_access() -> TestResult {
    let f = fixture(&staging_toml()).await?;
    agent(&f, "t1").await?;
    let r1 = land(&f, "t1", "one.txt").await?;
    let drop_access = |f: &Fixture| {
        f.h.github
            .set_perm("alice", "acme/app", support::perms("read"));
        f.h.clock.0.fetch_add(
            f.h.app.config.permission_ttl_ms + 1,
            std::sync::atomic::Ordering::SeqCst,
        );
    };
    // Asked while the owner has write access, then the access goes before the gate's verdict.
    let asked = ask(&f, "t1").await?;
    drop_access(&f);
    gate(&f, &r1, RunState::Passed).await?;
    settle(&f).await?;
    let one = request(&f, asked["id"].as_str().ok_or("id")?).await?;
    assert_eq!(one.state, StagingState::DeployFailed, "{one:?}");
    assert!(
        one.note
            .as_deref()
            .unwrap_or_default()
            .contains("write access"),
        "{one:?}"
    );
    assert!(deployed(&f).await?.is_empty(), "nothing deployed for it");
    // A new request is refused at once.
    agent(&f, "t2").await?;
    land(&f, "t2", "two.txt").await?;
    let said = refused(&f, "t2").await?;
    assert!(said.contains("write access"), "{said}");
    Ok(())
}

/// SPEC-575 review round 1 of slice 2: a person's cancel of a staging deploy stands. The request
/// ends `deploy_failed` saying so, and nothing deploys that revision again by itself.
#[tokio::test]
async fn a_cancelled_staging_deploy_is_not_started_again() -> TestResult {
    let f = fixture(&staging_toml()).await?;
    agent(&f, "t1").await?;
    // Its deploy sleeps (the deploy script waits on `slow`), so a person can cancel it.
    let r1 = land(&f, "t1", "slow").await?;
    gate(&f, &r1, RunState::Passed).await?;
    let asked = ask(&f, "t1").await?;
    let id = asked["id"].as_str().ok_or("id")?.to_owned();
    let deadline = tokio::time::Instant::now() + WAIT;
    let run = loop {
        f.h.app
            .staging_advance(&sandbox(&f).await?, "staging")
            .await?;
        if let Some(run) = request(&f, &id).await?.cd_run_id {
            break run;
        }
        if tokio::time::Instant::now() > deadline {
            return Err("the deploy did not start".into());
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    };
    let stop = support::call(
        &f.h.router,
        axum::http::Method::POST,
        &format!("/api/sandboxes/s1/cd/runs/{run}/cancel"),
        None,
        &f.alice,
    )
    .await?;
    assert!(stop.status.is_success(), "{}", stop.body);
    assert_eq!(f.ended(&run).await?["state"], "cancelled");
    settle(&f).await?;
    let one = request(&f, &id).await?;
    assert_eq!(one.state, StagingState::DeployFailed, "{one:?}");
    assert!(
        one.note
            .as_deref()
            .unwrap_or_default()
            .contains("cancelled"),
        "{one:?}"
    );
    assert_eq!(deployed(&f).await?, [r1], "one deploy, the cancelled one");
    Ok(())
}

/// SPEC-575 review round 1 of slice 2: while another run of the environment goes on (a person's
/// rollback, a drill), a ready request waits for it, costing no attempt, then deploys.
#[tokio::test]
async fn a_ready_request_waits_for_another_run_of_the_environment() -> TestResult {
    let f = fixture(&staging_toml()).await?;
    agent(&f, "t1").await?;
    let r1 = land(&f, "t1", "one.txt").await?;
    gate(&f, &r1, RunState::Passed).await?;
    let other = "other-run";
    let at = f.h.app.now();
    f.h.store
        .insert_cd_run(&yulius_app::ci::cd::CdRunRecord {
            id: other.into(),
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
            note: Some("a person's rollback".into()),
            output: String::new(),
            exit_code: None,
            started_at: at,
            ended_at: None,
            kind: yulius_domain::rollback::CdRunKind::Rollback,
            source: None,
            steps: Vec::new(),
        })
        .await?;
    let asked = ask(&f, "t1").await?;
    let id = asked["id"].as_str().ok_or("id")?.to_owned();
    for _ in 0..4 {
        f.h.app
            .staging_advance(&sandbox(&f).await?, "staging")
            .await?;
    }
    let waiting = request(&f, &id).await?;
    assert_eq!(
        (waiting.state, waiting.attempts),
        (StagingState::Ready, 0),
        "{waiting:?}"
    );
    f.h.store
        .finish_cd_run(
            other,
            &yulius_app::ci::cd::CdRunEnd {
                state: RunState::Cancelled,
                reason: Some("ended".into()),
                exit_code: None,
                output: String::new(),
                ended_at: f.h.app.now(),
                usage: None,
            },
        )
        .await?;
    settle(&f).await?;
    let one = request(&f, &id).await?;
    assert_eq!(
        (one.state, one.attempts),
        (StagingState::Verified, 1),
        "{one:?}"
    );
    Ok(())
}

async fn fill_planning_window(f: &Fixture) -> TestResult {
    f.h.store.execute_for_tests("WITH RECURSIVE n(i) AS (VALUES(1) UNION ALL SELECT i+1 FROM n WHERE i < 201)
        INSERT INTO ci_runs(id,sandbox_id,reference,sha,source,state,requested_by,queued_at)
        SELECT 'pr-noise-'||i,'s1','pr/'||i,'unrelated-'||i,'yulius_toml','passed','yulius',(SELECT MAX(queued_at) FROM ci_runs)+i FROM n").await?;
    Ok(())
}

#[tokio::test]
async fn a_completed_old_candidate_is_still_the_current_heads_verdict() -> TestResult {
    let f = fixture(&staging_toml()).await?;
    let sha = f.push("dev", &[("broken", Some("broken"))])?;
    let run = gate(&f, &sha, RunState::Failed).await?;
    f.h.store
        .execute_for_tests(&format!(
            "UPDATE ci_runs SET reference='candidate/' || sha WHERE id='{run}'"
        ))
        .await?;
    fill_planning_window(&f).await?;
    let project = sandbox(&f).await?;
    f.h.app.ci_sync_sandbox(&project).await?;
    let at_head = f.h.store.ci_runs_for_sha(&project.id, &sha).await?;
    assert_eq!(
        at_head.len(),
        1,
        "the failed candidate already judged this commit"
    );
    assert_eq!(at_head[0].id, run);
    Ok(())
}

/// SPEC-575 MFS-003 (review round 1 of slice 2): the candidate run queued for a revision is not
/// superseded when the branch moves on, CI runs it to its verdict, the request deploys on that
/// verdict, and the branch reaching the same commit queues no second full run of it.
#[tokio::test]
async fn a_candidate_run_survives_the_branch_moving_on_and_decides() -> TestResult {
    let f = fixture(&staging_toml()).await?;
    agent(&f, "t1").await?;
    let r1 = land(&f, "t1", "one.txt").await?;
    let asked = ask(&f, "t1").await?;
    let id = asked["id"].as_str().ok_or("id")?.to_owned();
    let s = sandbox(&f).await?;
    f.h.app.staging_advance(&s, "staging").await?;
    // More recent PR runs can fill CI's bounded planning history while this suite runs.
    fill_planning_window(&f).await?;
    // CI's round at the same commit adds no branch run of it, and dev moving on keeps it.
    f.h.app.ci_sync_sandbox(&s).await?;
    let later = f.push("dev", &[("later.txt", Some("later"))])?;
    f.h.app.ci_sync_sandbox(&s).await?;
    assert!(
        f.h.store.ci_runs_for_sha(&s.id, &later).await?.is_empty(),
        "the active suite still owns the gate outside the planning window"
    );
    let at_r1 = f.h.store.ci_runs_for_sha(&s.id, &r1).await?;
    assert_eq!(
        at_r1
            .iter()
            .map(|r| (r.reference.key(), r.state.is_active()))
            .collect::<Vec<_>>(),
        [(format!("candidate/{r1}"), true)],
        "one full run of the revision, still active"
    );
    let deadline = tokio::time::Instant::now() + WAIT;
    loop {
        f.h.app.ci_dispatch().await;
        f.h.app.staging_advance(&s, "staging").await?;
        let one = request(&f, &id).await?;
        if one.state == StagingState::Verified {
            assert_eq!(one.ci_run_id.as_deref(), Some(at_r1[0].id.as_str()));
            break;
        }
        if tokio::time::Instant::now() > deadline {
            return Err(format!("the candidate's verdict did not deploy: {one:?}").into());
        }
        tokio::time::sleep(Duration::from_millis(150)).await;
    }
    assert_eq!(deployed(&f).await?, [r1]);
    Ok(())
}

/// SPEC-575 MFS-004 and MFS-005 (review round 1 of slice 2): a task with an open step is
/// refused, a project with no CI gate is refused, and a newer landing of the same task
/// supersedes its waiting request.
#[tokio::test]
async fn requests_are_refused_or_superseded_as_the_task_stands() -> TestResult {
    let f = fixture(&staging_toml()).await?;
    agent(&f, "t1").await?;
    let r1 = land(&f, "t1", "one.txt").await?;
    gate(&f, &r1, RunState::Running).await?;
    let steps = support::mcp_tool(
        &f.h,
        &token("t1"),
        "steps_set",
        json!({"steps": [{"title": "Ship it", "status": "pending"}]}),
    )
    .await?;
    assert_eq!(steps["isError"], false, "{steps}");
    let said = refused(&f, "t1").await?;
    assert!(said.contains("still open"), "{said}");
    let done = support::mcp_tool(
        &f.h,
        &token("t1"),
        "step_update",
        json!({"ordinal": 1, "status": "done"}),
    )
    .await?;
    assert_eq!(done["isError"], false, "{done}");
    let first = ask(&f, "t1").await?;
    let r2 = land(&f, "t1", "two.txt").await?;
    let receipt = delivery_text(&f, "t1").await?;
    assert!(
        receipt.contains(&format!("your merged revision {}", &r2[..12]))
            && receipt.contains("cd_deploy {\"environment\":\"staging\"}"),
        "{receipt}"
    );
    let second = ask(&f, "t1").await?;
    assert_eq!(second["revision"], r2.as_str());
    let old = request(&f, first["id"].as_str().ok_or("id")?).await?;
    assert_eq!(old.state, StagingState::Superseded, "{old:?}");

    let bare = fixture(
        "[cd.dev]\n[cd.prod]\n\n[cd.staging]\nstage = \"dev\"\nmerge_first = true\n\
         deploy = \"true\"\nverify = \"true\"\nrollback = \"true\"\nmigrations = \"none\"\n",
    )
    .await?;
    agent(&bare, "t1").await?;
    land(&bare, "t1", "one.txt").await?;
    let said = refused(&bare, "t1").await?;
    assert!(said.contains("no CI gate"), "{said}");
    Ok(())
}

/// SPEC-575 MFS-006 (review round 1 of slice 2): a waiting request whose revision left the
/// integration branch's own history, or whose environment stopped being the merge-first target,
/// is superseded, saying why, and never deploys.
#[tokio::test]
async fn a_request_whose_revision_or_target_went_away_is_superseded() -> TestResult {
    let f = fixture(&staging_toml()).await?;
    agent(&f, "t1").await?;
    agent(&f, "t2").await?;
    let before = f.head("dev")?;
    let r1 = land(&f, "t1", "one.txt").await?;
    let gone = ask(&f, "t1").await?;
    // dev is reset past r1 (a force push): r1 is no commit of its history any more.
    git_origin(&f, &["update-ref", "refs/heads/dev", &before])?;
    f.h.app
        .staging_advance(&sandbox(&f).await?, "staging")
        .await?;
    let one = request(&f, gone["id"].as_str().ok_or("id")?).await?;
    assert_eq!(one.state, StagingState::Superseded, "{one:?}");
    assert!(
        one.note
            .as_deref()
            .unwrap_or_default()
            .contains("no longer on"),
        "{one:?}"
    );
    let _ = r1;
    // The environment stops being the target.
    let r2 = land(&f, "t2", "two.txt").await?;
    let waiting = ask(&f, "t2").await?;
    let toml =
        std::fs::read_to_string(f.work.join("yulius.toml"))?.replace("merge_first = true\n", "");
    f.push("dev", &[("yulius.toml", Some(&toml))])?;
    f.h.app
        .staging_advance(&sandbox(&f).await?, "staging")
        .await?;
    let two = request(&f, waiting["id"].as_str().ok_or("id")?).await?;
    assert_eq!(two.state, StagingState::Superseded, "{two:?}");
    assert!(
        two.note
            .as_deref()
            .unwrap_or_default()
            .contains("no longer the merge-first staging target"),
        "{two:?}"
    );
    let _ = r2;
    assert!(deployed(&f).await?.is_empty());
    Ok(())
}

fn git_origin(f: &Fixture, args: &[&str]) -> BoxResult<String> {
    crate::cd::git(&f.origin, args)
}

/// A deploy script that runs an irreversible migration, says so, and fails.
const DEPLOY_PAST_A_BARRIER: &str = "#!/bin/sh\necho migrated > \"$MARKS/schema\"\necho \"::yulius-rollback-barrier dropped column x\"\nexit 3\n";

/// `staging_toml` whose rollback redeploys the kept release and whose verify checks it.
fn restoring_toml() -> String {
    staging_toml()
        .replace(
            "rollback = \"true\"",
            "rollback = \"MARKS={marks} sh scripts/cd/rollback.sh\"",
        )
        .replace(
            "verify = \"MARKS={marks} sh scripts/cd/verify.sh\"\n\
     rollback",
            "verify = \"MARKS={marks} sh scripts/cd/staging-verify.sh\"\n\
     rollback",
        )
}

/// t1's first revision with the restoring scripts, deployed and verified on staging.
async fn first_verified(f: &Fixture) -> BoxResult<String> {
    let r1 = f.push(
        "dev",
        &[
            ("scripts/cd/rollback.sh", Some(ROLLBACK)),
            ("scripts/cd/staging-verify.sh", Some(STAGING_VERIFY)),
        ],
    )?;
    f.h.store
        .execute_for_tests(&format!(
            "INSERT INTO task_merges(task_id, sandbox_id, merged_sha, branch, pr_number, head_sha) \
             VALUES ('t1', 's1', '{r1}', 'dev', NULL, '{r1}')"
        ))
        .await?;
    gate(f, &r1, RunState::Passed).await?;
    ask(f, "t1").await?;
    settle(f).await?;
    assert_eq!(serving(f)?, r1);
    Ok(r1)
}

/// SPEC-575 MFS-007 with RBK-022 (review round 2 of slice 2): a failed deploy that announced a
/// rollback barrier is not restored past it. Its request says why, and nothing rolls staging
/// back.
#[tokio::test]
async fn a_failed_deploy_past_a_rollback_barrier_is_not_restored() -> TestResult {
    let f = fixture(&restoring_toml()).await?;
    agent(&f, "t1").await?;
    agent(&f, "t2").await?;
    first_verified(&f).await?;
    let r2 = f.push(
        "dev",
        &[("scripts/cd/deploy.sh", Some(DEPLOY_PAST_A_BARRIER))],
    )?;
    f.h.store
        .execute_for_tests(&format!(
            "INSERT INTO task_merges(task_id, sandbox_id, merged_sha, branch, pr_number, head_sha) \
             VALUES ('t2', 's1', '{r2}', 'dev', NULL, '{r2}')"
        ))
        .await?;
    gate(&f, &r2, RunState::Passed).await?;
    let second = ask(&f, "t2").await?;
    settle(&f).await?;
    settle(&f).await?;
    let failed = request(&f, second["id"].as_str().ok_or("id")?).await?;
    assert_eq!(failed.state, StagingState::DeployFailed, "{failed:?}");
    let note = failed.note.clone().unwrap_or_default();
    assert!(
        note.contains("rollback barrier (dropped column x)") && note.contains("a person decides"),
        "{note}"
    );
    let runs = f.h.store.cd_runs(&SandboxId::parse("s1")?, 50).await?;
    assert!(
        !runs
            .iter()
            .any(|run| run.kind == yulius_domain::rollback::CdRunKind::Rollback),
        "nothing rolled staging back past the barrier: {runs:?}"
    );
    assert!(!f.marks.join("restores").exists());
    Ok(())
}

/// SPEC-575 (review round 2 of slice 2): after a person's rollback of staging, the queue shows
/// what staging serves, not the newest verified request, and production would promote that.
#[tokio::test]
async fn the_queue_shows_what_staging_serves_after_a_rollback() -> TestResult {
    let f = fixture(&restoring_toml()).await?;
    agent(&f, "t1").await?;
    agent(&f, "t2").await?;
    let r1 = first_verified(&f).await?;
    let r2 = land(&f, "t2", "two.txt").await?;
    gate(&f, &r2, RunState::Passed).await?;
    ask(&f, "t2").await?;
    settle(&f).await?;
    // Each passed deploy's drill moves staging back and forth: wait for it to end on r2.
    settle(&f).await?;
    assert_eq!(serving(&f)?, r2);
    let back = support::call(
        &f.h.router,
        axum::http::Method::POST,
        "/api/sandboxes/s1/cd/staging/rollback",
        Some(json!({})),
        &f.alice,
    )
    .await?;
    assert!(back.status.is_success(), "{}", back.body);
    f.ended(back.body["run_id"].as_str().ok_or("run")?).await?;
    assert_eq!(serving(&f)?, r1);
    let overview = f.get("/api/sandboxes/s1/cd").await?;
    let staging = overview["environments"]
        .as_array()
        .ok_or("environments")?
        .iter()
        .find(|env| env["name"] == "staging")
        .ok_or("staging")?["staging"]
        .clone();
    assert_eq!(staging["verified"], r1.as_str(), "{staging}");
    Ok(())
}

/// SPEC-575 MFS-008 (review round 2 of slice 2): while an update holds new deploys, a ready
/// request waits, costing no attempt, and deploys once the gate opens.
#[tokio::test]
async fn a_ready_request_waits_while_an_update_holds_deploys() -> TestResult {
    let f = fixture(&staging_toml()).await?;
    agent(&f, "t1").await?;
    let r1 = land(&f, "t1", "one.txt").await?;
    gate(&f, &r1, RunState::Passed).await?;
    f.h.app.hold_deploys_for_tests(true);
    let asked = ask(&f, "t1").await?;
    let id = asked["id"].as_str().ok_or("id")?.to_owned();
    for _ in 0..4 {
        f.h.app
            .staging_advance(&sandbox(&f).await?, "staging")
            .await?;
    }
    let waiting = request(&f, &id).await?;
    assert_eq!(
        (waiting.state, waiting.attempts),
        (StagingState::Ready, 0),
        "{waiting:?}"
    );
    f.h.app.hold_deploys_for_tests(false);
    settle(&f).await?;
    let one = request(&f, &id).await?;
    assert_eq!(
        (one.state, one.attempts),
        (StagingState::Verified, 1),
        "{one:?}"
    );
    Ok(())
}

/// SPEC-575 MFS-003 (review rounds 1 and 2 of slice 2): a pinned deploy runs only a full
/// commit of the integration head's own first-parent history, and only for a dev-stage
/// environment.
#[tokio::test]
async fn a_pinned_deploy_runs_only_a_commit_of_the_branchs_own_history() -> TestResult {
    let f = fixture(&staging_toml()).await?;
    let s = sandbox(&f).await?;
    let alice =
        f.h.store
            .user(&yulius_domain::ids::UserId::parse("u-alice")?)
            .await?
            .ok_or("alice")?;
    let r1 = f.push("dev", &[("one.txt", Some("one"))])?;
    // A commit of another branch, never merged into dev.
    let side = f.push("side", &[("side.txt", Some("side"))])?;
    git_origin(&f, &["update-ref", "refs/heads/dev", &r1])?;
    let deploy = |revision: String| {
        let (f, s, alice) = (&f, &s, &alice);
        async move {
            f.h.app
                .cd_deploy_revision_for_tests(s, "staging", alice, &revision)
                .await
        }
    };
    for (revision, says) in [
        (side.clone(), "is not a commit of dev's own history"),
        (r1[..12].to_owned(), "is not a commit of dev's own history"),
        ("z".repeat(40), "is not a commit of dev's own history"),
    ] {
        let refused = deploy(revision.clone()).await.err().ok_or("refused")?;
        assert!(refused.to_string().contains(says), "{revision}: {refused}");
    }
    let prod =
        f.h.app
            .cd_deploy_revision_for_tests(&s, "prod", &alice, &r1)
            .await
            .err()
            .ok_or("refused")?;
    assert!(
        prod.to_string().contains("only a dev-stage environment"),
        "{prod}"
    );
    let run = deploy(r1.clone()).await?;
    assert_eq!(f.ended(&run.id).await?["commit"], r1.as_str());
    Ok(())
}

/// SPEC-575 acceptance (an existing project adopts the default): a project whose staging was
/// deployed by hand before merge-first lands the bootstrap change that marks it merge-first.
/// A delivery staging already serves is answered `included` without a deploy. The next task's
/// merged revision passes its full gate, is deployed by the queue, and is the revision verified
/// there.
#[tokio::test]
async fn a_project_that_adopts_merge_first_deploys_its_next_delivery_through_the_queue()
-> TestResult {
    let before = staging_toml().replace("merge_first = true\n", "");
    let f = fixture(&before).await?;
    agent(&f, "t0").await?;
    agent(&f, "t1").await?;
    // Before the switch: a person deploys staging's head by hand, carrying t0's landing.
    let r0 = land(&f, "t0", "zero.txt").await?;
    let by_hand = f.deploy(&f.alice, "staging", None).await?;
    assert!(by_hand.status.is_success(), "{}", by_hand.body);
    f.ended(by_hand.body["run_id"].as_str().ok_or("run")?)
        .await?;
    assert_eq!(serving(&f)?, r0);
    // The bootstrap change lands: staging is merge-first from now on.
    f.push(
        "dev",
        &[(
            "yulius.toml",
            Some(&staging_toml().replace("{marks}", f.marks.to_str().ok_or("marks")?)),
        )],
    )?;
    let refused = f.deploy(&f.alice, "staging", None).await?;
    assert!(
        refused
            .body
            .to_string()
            .contains("merge-first staging target"),
        "{}",
        refused.body
    );
    // t0's delivery is already on staging: answered at once, deployed again never.
    let inside = ask(&f, "t0").await?;
    assert_eq!(inside["state"], "included", "{inside}");
    // t1's delivery goes through the queue.
    let r1 = land(&f, "t1", "one.txt").await?;
    let ci = gate(&f, &r1, RunState::Passed).await?;
    let asked = ask(&f, "t1").await?;
    settle(&f).await?;
    let one = request(&f, asked["id"].as_str().ok_or("id")?).await?;
    assert_eq!(one.state, StagingState::Verified, "{one:?}");
    assert_eq!(one.ci_run_id.as_deref(), Some(ci.as_str()));
    assert_eq!(serving(&f)?, r1);
    let revisions = deployed(&f).await?;
    assert_eq!(revisions, [r0, r1], "one deploy by hand, one by the queue");
    Ok(())
}

/// What `task`'s agent reads in `delivery_status`.
async fn delivery_text(f: &Fixture, task: &str) -> BoxResult<String> {
    let answer = support::mcp_tool(&f.h, &token(task), "delivery_status", json!({})).await?;
    assert_eq!(answer["isError"], false, "{answer}");
    Ok(answer["content"][0]["text"]
        .as_str()
        .ok_or("text")?
        .to_owned())
}

/// Slice A is confirmed. Slice B's merge command succeeded and the closed PR cannot yet be read.
#[tokio::test]
async fn the_final_request_never_stages_an_earlier_slice_while_the_current_landing_is_unconfirmed()
-> TestResult {
    let f = fixture(&staging_toml()).await?;
    agent(&f, "t1").await?;
    let first = land(&f, "t1", "first.txt").await?;
    f.h.store
        .execute_for_tests("UPDATE task_merges SET pr_number = 7 WHERE task_id = 't1'")
        .await?;
    let second = f.push("dev", &[("second.txt", Some("second"))])?;
    f.h.store
        .execute_for_tests(&format!(
        "INSERT INTO delivery(task_id, stage, pr_number, updated_at) VALUES ('t1','pr_open',8,1);
         INSERT INTO task_merge_recovery(task_id, pr_number, branch, head_sha, merged_sha)
           VALUES ('t1',8,'dev','{second}','{second}')"))
        .await?;
    // The person merged PR 8 externally. Until reconciliation its local row still reads open.
    assert!(refused(&f, "t1").await?.contains("delivery_status"));
    f.h.store
        .execute_for_tests("UPDATE delivery SET stage = 'merged_integration' WHERE task_id = 't1'")
        .await?;
    let text = delivery_text(&f, "t1").await?;
    assert!(
        !text.contains(&format!("your merged revision {}", &first[..12])),
        "{text}"
    );
    assert!(!text.contains("ask once with cd_deploy"), "{text}");
    assert!(refused(&f, "t1").await?.contains("delivery_status"));
    f.h.store
        .execute_for_tests(&format!(
            "INSERT INTO task_merges(task_id, sandbox_id, merged_sha, branch, pr_number, head_sha)
           VALUES ('t1','s1','{second}','dev',8,'{second}')"
        ))
        .await?;
    assert_eq!(ask(&f, "t1").await?["revision"], second);
    Ok(())
}

#[tokio::test]
async fn a_busy_task_view_cannot_hold_the_staging_queue() -> TestResult {
    let f = fixture(&staging_toml()).await?;
    agent(&f, "t1").await?;
    f.h.store
        .execute_for_tests("UPDATE tasks SET activity = 'waiting' WHERE id = 't1'")
        .await?;
    let sha = land(&f, "t1", "one.txt").await?;
    let run = gate(&f, &sha, RunState::Failed).await?;
    let sandbox = sandbox(&f).await?;
    let staged = StagingRequestRecord {
        id: "sr_held_view".into(),
        sandbox_id: sandbox.id.clone(),
        environment: "staging".into(),
        task_id: Some(TaskId::parse("t1")?),
        revision: sha,
        state: StagingState::Testing,
        ci_run_id: Some(run),
        cd_run_id: None,
        attempts: 0,
        note: None,
        requested_by: "task:1".into(),
        told: false,
        created_at: f.h.app.now(),
        updated_at: f.h.app.now(),
    };
    assert!(f.h.store.insert_staging_request(&staged).await?);
    let browser = std::sync::Arc::new(crate::delivery::HeldBrowserLookup::for_publication());
    let mut deps = f.h.app.deps.clone();
    deps.browser = browser.clone();
    let app = yulius_app::context::App::new(f.h.app.config.clone(), deps);
    let mut events = app.events.subscribe();
    let start = std::sync::Arc::new(tokio::sync::Notify::new());
    let advance = tokio::spawn({
        let start = start.clone();
        async move {
            start.notified().await;
            app.staging_advance(&sandbox, "staging").await
        }
    });
    let (entered, _release) = browser.hold_lookup_for(advance.id());
    start.notify_one();
    tokio::time::timeout(WAIT, entered.notified()).await?;
    tokio::time::timeout(Duration::from_secs(5), advance)
        .await??
        .map_err(|e| e.to_string())?;
    assert_eq!(
        request(&f, &staged.id).await?.state,
        StagingState::GateFailed
    );
    let mut resync = false;
    while let Ok(event) = events.try_recv() {
        resync |= matches!(event.event, yulius_contracts::events::UiEvent::Resync);
    }
    assert!(resync, "a view that missed the update must refetch");
    Ok(())
}

#[tokio::test]
async fn the_final_step_waits_for_staging_until_its_answer_is_delivered() -> TestResult {
    let f = fixture(&staging_toml()).await?;
    agent(&f, "t1").await?;
    let revision = land(&f, "t1", "one.txt").await?;
    let run = gate(&f, &revision, RunState::Running).await?;
    let steps = support::mcp_tool(
        &f.h,
        &token("t1"),
        "steps_set",
        json!({"steps": [{"title": "Verify staging", "status": "in_progress"}]}),
    )
    .await?;
    assert_eq!(steps["isError"], false, "{steps}");
    let asked = ask(&f, "t1").await?;
    let id = asked["id"].as_str().ok_or("request id")?;
    f.h.store
        .execute_for_tests("UPDATE tasks SET activity = 'waiting' WHERE id = 't1'")
        .await?;
    let task = TaskId::parse("t1")?;
    let summary = || async {
        let task = f.h.app.deps.store.task(&task).await?.ok_or("task")?;
        BoxResult::Ok(serde_json::to_value(f.h.app.task_summary(&task).await?)?)
    };
    let view = summary().await?;
    assert!(view["needs_you_notice"].is_null(), "{view}");
    assert_eq!(
        view["staging_wait"],
        "Waiting for the merged revision’s full suite"
    );
    let brief = support::mcp_tool(&f.h, &token("t1"), "task_get", json!({"number": 1})).await?;
    let fields = support::tool_text(&brief);
    assert_eq!(fields["shows"], "working", "{fields}");
    assert_eq!(fields["staging_wait"], view["staging_wait"]);
    assert!(
        brief["content"][0]["text"]
            .as_str()
            .is_some_and(|text| text.contains("Waiting for the merged revision’s full suite"))
    );
    f.h.store
        .execute_for_tests("UPDATE tasks SET status = 'parked' WHERE id = 't1'")
        .await?;
    let parked = summary().await?;
    assert!(parked["needs_you_notice"].is_null(), "{parked}");
    assert_eq!(parked["staging_wait"], view["staging_wait"]);
    assert!(
        f.h.store
            .transition_ci_job(
                &format!("{run}-full"),
                &[RunState::Running],
                RunState::Failed,
                &yulius_app::ci::CiJobUpdate::default()
            )
            .await?
    );
    assert!(
        f.h.store
            .transition_ci_run(
                &run,
                &[RunState::Running],
                RunState::Failed,
                &yulius_app::ci::CiRunUpdate::default()
            )
            .await?
    );
    f.h.app
        .staging_advance(&sandbox(&f).await?, "staging")
        .await?;
    let failed = request(&f, id).await?;
    assert_eq!(failed.state, StagingState::GateFailed);
    assert_eq!(
        summary().await?["staging_wait"],
        "Waiting for the staging outcome to reach the agent"
    );
    assert!(f.h.store.mark_staging_told(&failed).await?);
    let answered = summary().await?;
    assert!(answered["staging_wait"].is_null());
    assert!(answered["needs_you_notice"].is_object(), "{answered}");
    Ok(())
}

/// SPEC-575 MFS-013: a task whose work merged into a merge-first project reads in
/// `delivery_status` that staging waits for its request, then where its request stands. A task
/// whose session began before its project switched (the switch's own task) learns it there,
/// since every merge receipt sends the agent to `delivery_status`.
#[tokio::test]
async fn delivery_status_asks_a_merged_task_for_its_staging_request() -> TestResult {
    let f = fixture(&staging_toml()).await?;
    agent(&f, "t1").await?;
    let before = delivery_text(&f, "t1").await?;
    assert!(!before.contains("Staging"), "nothing merged yet: {before}");
    let r1 = land(&f, "t1", "one.txt").await?;
    let asks = delivery_text(&f, "t1").await?;
    let short = &r1[..12];
    assert!(
        asks.contains(&format!(
            "Staging ([cd.staging], merge-first): your merged revision {short} has no staging \
             request. Once your whole delivery has landed, ask once with cd_deploy {{\"environment\":\"staging\"}} \
             before ending your turn. Yulius runs the full suite, then deploys and verifies that revision. \
             If your final step requires the outcome, leave it in progress. Yulius wakes this task with the outcome."
        )),
        "{asks}"
    );
    f.h.store
        .execute_for_tests("UPDATE tasks SET activity = 'waiting' WHERE id = 't1'")
        .await?;
    let mut updates = f.h.app.events.subscribe();
    let ci = gate(&f, &r1, RunState::Passed).await?;
    let asked = ask(&f, "t1").await?;
    settle(&f).await?;
    let id = asked["id"].as_str().ok_or("id")?;
    let done = request(&f, id).await?;
    assert_eq!(done.state, StagingState::Verified, "{done:?}");
    let mut labels = Vec::new();
    while let Ok(event) = updates.try_recv() {
        if let yulius_contracts::events::UiEvent::TaskUpdated { task } = event.event
            && task.id == "t1"
            && let Some(label) = task.staging_wait
        {
            labels.push(label);
        }
    }
    assert!(
        labels
            .iter()
            .any(|label| label == StagingState::Deploying.task_wait_label()),
        "the task's mounted view must see the deploy start: {labels:?}"
    );
    let after = delivery_text(&f, "t1").await?;
    assert!(
        after.contains(&format!(
            "Staging ([cd.staging], merge-first): request {id} for revision {short} is verified, \
             CI run {ci}, CD run {}",
            done.cd_run_id.as_deref().ok_or("cd run")?
        )),
        "{after}"
    );
    assert!(!after.contains("has no staging request"), "{after}");
    let r2 = land(&f, "t1", "two.txt").await?;
    let later = delivery_text(&f, "t1").await?;
    assert!(
        later.contains(&r2[..12]) && later.contains("already has its staging deployment"),
        "{later}"
    );
    assert!(!later.contains("ask once with cd_deploy"), "{later}");
    // Without a merge-first target a merge asks for nothing.
    let plain = fixture(&staging_toml().replace("merge_first = true\n", "")).await?;
    agent(&plain, "t2").await?;
    land(&plain, "t2", "two.txt").await?;
    let none = delivery_text(&plain, "t2").await?;
    assert!(!none.contains("Staging"), "{none}");
    Ok(())
}

/// SPEC-575 MFS-013 (review round 1 of MFS-013): after a failed gate the line says how to go on,
/// and once the fix merged it names the fix's revision as due, then the newer request.
#[tokio::test]
async fn delivery_status_after_a_failed_gate_names_the_fix() -> TestResult {
    let f = fixture(&staging_toml()).await?;
    agent(&f, "t1").await?;
    let r1 = land(&f, "t1", "one.txt").await?;
    let ci1 = gate(&f, &r1, RunState::Failed).await?;
    let failing = ask(&f, "t1").await?;
    settle(&f).await?;
    let id1 = failing["id"].as_str().ok_or("id")?;
    assert_eq!(request(&f, id1).await?.state, StagingState::GateFailed);
    let failed = delivery_text(&f, "t1").await?;
    assert!(
        failed.contains(&format!(
            "request {id1} for revision {} is gate_failed, CI run {ci1}",
            &r1[..12]
        )) && failed.contains(
            "fix it forward on this branch, merge with delivery_merge, then cd_deploy again"
        ),
        "{failed}"
    );
    let r2 = land(&f, "t1", "fix.txt").await?;
    let due = delivery_text(&f, "t1").await?;
    assert!(
        due.contains(&format!(
            "your merged revision {} has no staging request",
            &r2[..12]
        )),
        "{due}"
    );
    gate(&f, &r2, RunState::Passed).await?;
    let fixed = ask(&f, "t1").await?;
    settle(&f).await?;
    let id2 = fixed["id"].as_str().ok_or("id")?;
    assert_ne!(id1, id2);
    let verified = delivery_text(&f, "t1").await?;
    assert!(
        verified.contains(&format!(
            "request {id2} for revision {} is verified",
            &r2[..12]
        )),
        "{verified}"
    );
    Ok(())
}

/// `staging_toml` whose production promotes.
fn promoting_toml() -> String {
    staging_toml().replace(
        "[cd.prod]\nabout = \"nothing yet\"\n",
        "[cd.prod]\nabout = \"a push to main is the release\"\npromote = true\n",
    )
}

/// SPEC-591 STG-008: Yulius's own `yulius.toml` makes `[cd.dev]` its merge-first staging target, so
/// a person's production deploy and the Delivery page's promotion are refused while staging has
/// not verified a revision whose full suite passed: a red or untested dev is fixed forward, never
/// promoted.
#[tokio::test]
async fn yuliuss_own_config_promotes_only_what_staging_verified() -> TestResult {
    let f = fixture(include_str!("../../../yulius.toml")).await?;
    agent(&f, "t1").await?;
    let r1 = land(&f, "t1", "one.txt").await?;
    let deploy = f.deploy(&f.bob, "prod", Some("app")).await?;
    assert_eq!(
        deploy.status,
        axum::http::StatusCode::CONFLICT,
        "{}",
        deploy.body
    );
    let why = deploy.body.to_string();
    assert!(
        why.contains("[cd.dev] staging verified nothing yet"),
        "{why}"
    );
    let page = promote_page(&f).await?;
    assert_eq!(
        page.status,
        axum::http::StatusCode::CONFLICT,
        "{}",
        page.body
    );
    let page_why = page.body.to_string();
    assert!(
        page_why.contains("[cd.dev]") && page_why.contains("staging verified nothing yet"),
        "{page_why}"
    );
    assert_ne!(f.head("main")?, r1);
    assert!(!git_contains(&f, "main", &r1)?);
    Ok(())
}

/// The Delivery page's promotion, as an administrator who typed the slug.
async fn promote_page(f: &Fixture) -> BoxResult<support::Resp> {
    support::call(
        &f.h.router,
        axum::http::Method::POST,
        "/api/sandboxes/s1/delivery/promote",
        Some(json!({"confirm": "app"})),
        &f.bob,
    )
    .await
}

/// SPEC-575 MFS-009 (review round 3 of slice 2): the Delivery page's promotion waits, like a
/// production deploy, until staging serves the integration head, verified. It reads that through
/// the verified CD mirror: an agent rewriting the `yulius.toml` object in its clone only makes
/// it refuse. Once staging verified the head, the same promotion promotes exactly it.
#[tokio::test]
async fn every_promotion_waits_for_the_revision_staging_verified() -> TestResult {
    let f = fixture(&promoting_toml()).await?;
    fast_forward_profile(&f, false).await?;
    agent(&f, "t1").await?;
    agent(&f, "t2").await?;
    let r1 = land(&f, "t1", "one.txt").await?;
    gate(&f, &r1, RunState::Passed).await?;
    ask(&f, "t1").await?;
    settle(&f).await?;
    let r2 = land(&f, "t2", "two.txt").await?;
    // What an agent can do in its own clone: rewrite the table's object without merge_first.
    // The mirror keeps the object it already holds, so the promotion is refused as unverified.
    let clone = f.h.app.config.data_dir.join("sandboxes/s1/repo");
    crate::cd::git(&clone, &["fetch", "-q", "origin"])?;
    let blob = crate::cd::git(&clone, &["rev-parse", &format!("{r2}:yulius.toml")])?;
    let text = std::fs::read_to_string(f.work.join("yulius.toml"))?;
    let object = clone.join(".git/objects").join(&blob[..2]).join(&blob[2..]);
    let rewrite = "import sys, zlib, os\nc = open(sys.argv[2], 'rb').read()\np = sys.argv[1]\n\
                   os.makedirs(os.path.dirname(p), exist_ok=True)\n\
                   os.path.exists(p) and os.unlink(p)\n\
                   open(p, 'wb').write(zlib.compress(b'blob %d\\x00' % len(c) + c))\n";
    let write_object = |contents: &str, name: &str| -> TestResult {
        let path = f.marks.join(name);
        std::fs::write(&path, contents)?;
        assert!(
            std::process::Command::new("python3")
                .args(["-I", "-c", rewrite])
                .arg(&object)
                .arg(&path)
                .status()?
                .success()
        );
        Ok(())
    };
    write_object(&text.replace("merge_first = true\n", ""), "tampered.toml")?;
    let tampered = promote_page(&f).await?;
    assert_eq!(
        tampered.status,
        axum::http::StatusCode::CONFLICT,
        "{}",
        tampered.body
    );
    assert!(
        !git_contains(&f, "main", &r2)?,
        "the tampered clone promoted nothing"
    );
    write_object(&text, "original.toml")?;
    // The verified head cannot be read (the CD mirror is broken): refused, naming why, rather
    // than promoted unchecked.
    let mirror = f.h.app.config.data_dir.join("cd/s1/source.git");
    let aside = f.marks.join("mirror-aside");
    std::fs::rename(&mirror, &aside)?;
    std::fs::write(&mirror, "not a repository")?;
    let unreadable = promote_page(&f).await?;
    assert_eq!(
        unreadable.status,
        axum::http::StatusCode::CONFLICT,
        "{}",
        unreadable.body
    );
    assert!(
        unreadable.body.to_string().contains("cannot be read now"),
        "{}",
        unreadable.body
    );
    assert!(
        !git_contains(&f, "main", &r2)?,
        "an unreadable head promoted nothing"
    );
    std::fs::remove_file(&mirror)?;
    std::fs::rename(&aside, &mirror)?;
    // Read as it is: the head is not what staging verified.
    let refused = promote_page(&f).await?;
    assert_eq!(
        refused.status,
        axum::http::StatusCode::CONFLICT,
        "{}",
        refused.body
    );
    assert!(
        refused
            .body
            .to_string()
            .contains(&format!("staging verified {}", &r1[..12])),
        "{}",
        refused.body
    );
    assert!(!git_contains(&f, "main", &r2)?);
    // Staging verifies r2: the page's promotion now promotes it.
    gate(&f, &r2, RunState::Passed).await?;
    ask(&f, "t2").await?;
    settle(&f).await?;
    assert_eq!(serving(&f)?, r2);
    let promoted = promote_page(&f).await?;
    assert!(promoted.status.is_success(), "{}", promoted.body);
    assert_eq!(f.head("main")?, r2, "exactly the verified revision");
    Ok(())
}

/// SPEC-575 MFS-009 (review round 3 of slice 2): with the profile's automatic promotion, a merge
/// promotes nothing while staging has not verified it. The queue's verified deploy of that
/// revision runs the promotion that waited, and production gets exactly it.
#[tokio::test]
async fn automatic_promotion_follows_the_verified_staging_deploy() -> TestResult {
    let f = fixture(&promoting_toml()).await?;
    fast_forward_profile(&f, true).await?;
    agent(&f, "t1").await?;
    let r1 = land(&f, "t1", "one.txt").await?;
    f.h.store
        .execute_for_tests(
            "INSERT INTO delivery (task_id, stage, pr_number, updated_at) \
             VALUES ('t1', 'merged_integration', NULL, 1)",
        )
        .await?;
    assert!(!git_contains(&f, "main", &r1)?);
    gate(&f, &r1, RunState::Passed).await?;
    ask(&f, "t1").await?;
    settle(&f).await?;
    assert_eq!(serving(&f)?, r1);
    assert!(
        git_contains(&f, "main", &r1)?,
        "promoted once staging verified it"
    );
    Ok(())
}

/// The sandbox's delivery profile: promotion by fast-forward (so `main` shows what was
/// promoted), automatic or not.
async fn fast_forward_profile(f: &Fixture, auto_promote: bool) -> TestResult {
    f.h.store
        .set_profile(
            &SandboxId::parse("s1")?,
            &json!({
                "preset": "custom", "integration_branch": "dev", "production_branch": "main",
                "merge": "squash", "promote": "fast_forward", "auto_promote": auto_promote,
                "require_checks": false, "environments": []
            })
            .to_string(),
        )
        .await?;
    Ok(())
}

/// SPEC-575 MFS-009 (review round 1 of slice 4, probe R7-A): a merge-first project that has no
/// staging request yet (right after it adopted merge-first) is refused a promotion when its
/// verified head cannot be read. An agent breaking its clone never promotes an unverified head.
#[tokio::test]
async fn a_promotion_whose_verified_head_cannot_be_read_is_refused() -> TestResult {
    let f = fixture(&promoting_toml()).await?;
    fast_forward_profile(&f, false).await?;
    agent(&f, "t1").await?;
    let r1 = land(&f, "t1", "one.txt").await?;
    let refused = promote_page(&f).await?;
    assert_eq!(
        refused.status,
        axum::http::StatusCode::CONFLICT,
        "{}",
        refused.body
    );
    assert!(
        refused
            .body
            .to_string()
            .contains("staging verified nothing yet"),
        "{}",
        refused.body
    );
    // What an agent can do in its own clone: break its Git configuration.
    let config =
        f.h.app
            .config
            .data_dir
            .join("sandboxes/s1/repo/.git/config");
    let original = std::fs::read_to_string(&config)?;
    std::fs::write(&config, format!("{original}\n[broken\n"))?;
    let broken = promote_page(&f).await?;
    assert_eq!(
        broken.status,
        axum::http::StatusCode::CONFLICT,
        "{}",
        broken.body
    );
    assert!(
        broken.body.to_string().contains("cannot be read now"),
        "{}",
        broken.body
    );
    assert!(
        !git_contains(&f, "main", &r1)?,
        "nothing unverified was promoted"
    );
    std::fs::write(&config, original)?;
    Ok(())
}

/// SPEC-575 MFS-009 (review round 1 of slice 4): with automatic promotion, staging verifying an
/// older revision while the integration head moved on promotes nothing; the verified deploy of
/// the head promotes exactly it.
#[tokio::test]
async fn automatic_promotion_never_takes_an_unverified_head() -> TestResult {
    let f = fixture(&promoting_toml()).await?;
    fast_forward_profile(&f, true).await?;
    agent(&f, "t1").await?;
    agent(&f, "t2").await?;
    let r1 = land(&f, "t1", "one.txt").await?;
    gate(&f, &r1, RunState::Passed).await?;
    f.h.store
        .execute_for_tests(
            "INSERT INTO delivery (task_id, stage, pr_number, updated_at) \
             VALUES ('t1', 'merged_integration', NULL, 1), ('t2', 'merged_integration', NULL, 1)",
        )
        .await?;
    ask(&f, "t1").await?;
    // t2 lands before the queue deploys r1: the head is r2, which nothing verified.
    let r2 = land(&f, "t2", "two.txt").await?;
    settle(&f).await?;
    assert_eq!(serving(&f)?, r1);
    assert!(
        !git_contains(&f, "main", &r1)? && !git_contains(&f, "main", &r2)?,
        "staging verified r1, the head is r2: nothing is promoted"
    );
    // That is waiting for staging, not a failed promotion (review round 2 of slice 4).
    let audit = |action: &'static str| {
        let store = f.h.store.clone();
        async move {
            store
                .audit_of_sandbox("app", &[], &[action], None, None, 10)
                .await
        }
    };
    let waits = audit("delivery.promote.waiting").await?;
    assert!(
        waits.iter().any(|(_, e)| e
            .detail
            .as_deref()
            .is_some_and(|d| d.contains(&format!("staging verified {}", &r1[..12])))),
        "{waits:?}"
    );
    assert!(audit("delivery.promote.failed").await?.is_empty());
    gate(&f, &r2, RunState::Passed).await?;
    ask(&f, "t2").await?;
    settle(&f).await?;
    assert_eq!(serving(&f)?, r2);
    assert_eq!(f.head("main")?, r2, "the verified head is promoted");
    Ok(())
}

/// SPEC-575 MFS-009 (review round 3 of slice 4, probe R9-A): an automatic promotion whose
/// verified head cannot be read fails (audited as failed, not as waiting for staging), and
/// production stays where it was. It runs outside the queue: a person confirms a manual
/// integration environment.
#[tokio::test]
async fn an_unreadable_head_fails_the_automatic_promotion() -> TestResult {
    let f = fixture(&promoting_toml()).await?;
    f.h.store
        .set_profile(
            &SandboxId::parse("s1")?,
            &json!({
                "preset": "custom", "integration_branch": "dev", "production_branch": "main",
                "merge": "squash", "promote": "fast_forward", "auto_promote": true,
                "require_checks": false,
                "environments": [
                    {"name": "check", "after": "integration", "verify": "manual", "command": null}
                ]
            })
            .to_string(),
        )
        .await?;
    agent(&f, "t1").await?;
    let r1 = land(&f, "t1", "one.txt").await?;
    f.h.store
        .execute_for_tests(
            "INSERT INTO delivery (task_id, stage, pr_number, updated_at) \
             VALUES ('t1', 'merged_integration', NULL, 1)",
        )
        .await?;
    let config =
        f.h.app
            .config
            .data_dir
            .join("sandboxes/s1/repo/.git/config");
    let original = std::fs::read_to_string(&config)?;
    std::fs::write(&config, format!("{original}\n[broken\n"))?;
    let confirmed = support::call(
        &f.h.router,
        axum::http::Method::POST,
        "/api/sandboxes/s1/delivery/environments/check/confirm",
        Some(json!({"confirm": true})),
        &f.bob,
    )
    .await?;
    std::fs::write(&config, original)?;
    assert!(
        confirmed
            .body
            .to_string()
            .contains("Automatic promotion did not happen"),
        "{}",
        confirmed.body
    );
    let rows = |action: &'static str| {
        let store = f.h.store.clone();
        async move {
            store
                .audit_of_sandbox("app", &[], &[action], None, None, 10)
                .await
        }
    };
    let failed = rows("delivery.promote.failed").await?;
    assert!(
        failed.iter().any(|(_, e)| e
            .detail
            .as_deref()
            .is_some_and(|d| d.contains("cannot be read now"))),
        "{failed:?}"
    );
    assert!(rows("delivery.promote.waiting").await?.is_empty());
    assert!(!git_contains(&f, "main", &r1)?, "nothing was promoted");
    Ok(())
}
