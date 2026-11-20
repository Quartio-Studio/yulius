//! Merge-first staging end to end (SPEC-575 MFS-003 to MFS-008): a project whose `[cd.staging]`
//! is the merge-first target, the real Git adapter and process host deploying it, tasks' agents
//! asking through `cd_deploy`, and the full gate's verdicts recorded at exact revisions.

use std::time::Duration;

use serde_json::{Value, json};
use yulius_app::ci::{CiJobRecord, CiRunRecord};
use yulius_app::ports::store::{SandboxStore as _, UserStore as _};
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
/// revision staging verified. While the integration head is a newer revision not verified there,
/// it is refused, naming both; once staging verified the head, the same deploy promotes it.
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
    // A newer revision lands but is not verified on staging: refused, the head unpromoted.
    let r2 = land(&f, "t2", "two.txt").await?;
    let refused = f.deploy(&f.bob, "prod", Some("app")).await?;
    let said = refused.body.to_string();
    assert!(
        said.contains(&format!("staging verified {}", &r1[..7])) && said.contains(&r2[..7]),
        "{said}"
    );
    assert!(!git_contains(&f, "main", &r2)?, "nothing promoted");
    // The Delivery page's promotion is held to the same revision (review round 2).
    let page = support::call(
        &f.h.router,
        axum::http::Method::POST,
        "/api/sandboxes/s1/delivery/promote",
        Some(json!({"confirm": "app"})),
        &f.bob,
    )
    .await?;
    assert_eq!(
        page.status,
        axum::http::StatusCode::CONFLICT,
        "{}",
        page.body
    );
    assert!(
        page.body
            .to_string()
            .contains(&format!("staging verified {}", &r1[..7])),
        "{}",
        page.body
    );
    assert!(!git_contains(&f, "main", &r2)?, "nothing promoted");
    // Staging verifies r2: the same deploy promotes exactly it.
    gate(&f, &r2, RunState::Passed).await?;
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
    // CI's round at the same commit adds no branch run of it, and dev moving on keeps it.
    f.h.app.ci_sync_sandbox(&s).await?;
    f.push("dev", &[("later.txt", Some("later"))])?;
    f.h.app.ci_sync_sandbox(&s).await?;
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
    land(&f, "t1", "one.txt").await?;
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
