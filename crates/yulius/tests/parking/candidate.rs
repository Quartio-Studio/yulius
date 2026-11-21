//! Original-owner PR notifications across restart with real Git, SQLite and PTYs.
use super::{Error, answered, app_with_pulls, ask, data_dir, open_pull_at, runtime, set_pull_head};
use crate::support::TestResult;
use serde_json::json;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;
use yulius_app::delivery::DeliveryRecord;
use yulius_app::ports::github::ChecksState;
use yulius_app::pr_watch::WatchBounds;
use yulius_domain::ci::{CiRef, RunState};
use yulius_domain::delivery::DeliveryStage;
use yulius_domain::ids::{AttemptId, TaskId};

/// #331: a failing integration candidate ends immediately before daemon restart.
#[test]
fn a_cold_owner_receives_its_recorded_integration_failure_after_its_rerun() -> TestResult {
    cold_candidate_case(ColdCandidate::OwnerRerun)
}

#[test]
fn a_passed_ordinary_rerun_does_not_send_its_obsolete_failure_with_the_candidate_reason()
-> TestResult {
    cold_candidate_case(ColdCandidate::ReplacedOrdinaryError)
}

#[test]
fn a_backdated_queue_position_does_not_hide_the_newer_failed_candidate() -> TestResult {
    cold_candidate_case(ColdCandidate::Backdated)
}

#[test]
fn an_ordinary_failure_beyond_the_recent_window_precedes_the_saved_candidate() -> TestResult {
    cold_candidate_case(ColdCandidate::OlderOrdinaryFailure)
}

#[test]
fn unrelated_ci_runs_do_not_hide_a_current_candidate() -> TestResult {
    cold_candidate_case(ColdCandidate::UnrelatedRuns)
}

#[test]
fn a_failed_candidate_reaches_its_owner_after_an_ordinary_green_notice() -> TestResult {
    cold_candidate_case(ColdCandidate::PriorNotice)
}

#[test]
fn a_cancelled_ordinary_run_preserves_the_accepted_singleton_diagnosis() -> TestResult {
    cold_candidate_case(ColdCandidate::OrdinaryCancelled)
}

#[test]
fn an_ordinary_infrastructure_error_preserves_the_accepted_singleton_diagnosis() -> TestResult {
    cold_candidate_case(ColdCandidate::OrdinaryError)
}

#[test]
fn an_accepted_singleton_resolves_its_source_trains_hands_off_notice() -> TestResult {
    cold_candidate_case(ColdCandidate::SourceIncludesOwner)
}

#[test]
fn a_moved_hosted_head_cannot_rearm_an_obsolete_ordinary_green() -> TestResult {
    cold_candidate_case(ColdCandidate::MovedHead)
}

#[test]
fn a_singleton_accepted_after_its_ordinary_deadline_reaches_the_owner() -> TestResult {
    cold_candidate_case(ColdCandidate::TimedOutOrdinary)
}

#[test]
fn an_accepted_singleton_resolves_its_earlier_bisected_train() -> TestResult {
    cold_candidate_case(ColdCandidate::PriorBisection)
}

#[test]
fn a_later_stall_reports_the_singleton_status_without_retyping_its_diagnosis() -> TestResult {
    cold_candidate_case(ColdCandidate::Stall)
}

#[test]
fn an_ordinary_drop_after_rearm_cannot_revive_a_historical_bisection() -> TestResult {
    cold_candidate_case(ColdCandidate::OrdinaryDropAfterRearm)
}

#[test]
fn an_inaccessible_arm_registry_keeps_a_saved_rearm_from_receiving_its_old_failure() -> TestResult {
    cold_candidate_case(ColdCandidate::UnreadableArm)
}

#[test]
fn a_later_arm_outside_a_train_gets_no_prior_singleton_hands_off_notice() -> TestResult {
    cold_candidate_case(ColdCandidate::RearmedOutsideTrain)
}

#[test]
fn a_later_train_holding_the_rearmed_owner_does_not_revive_a_bisected_candidate() -> TestResult {
    cold_candidate_case(ColdCandidate::RearmedInTrain)
}

#[test]
fn a_new_ordinary_run_after_a_singleton_deadline_gets_its_own_wait() -> TestResult {
    cold_candidate_case(ColdCandidate::NewerRunAfterExpiry)
}

#[test]
fn a_held_ordinary_failure_tells_the_owner_to_fix_it_once() -> TestResult {
    cold_candidate_case(ColdCandidate::HeldOrdinaryFailure)
}

#[test]
fn a_held_ordinary_failure_already_heard_cannot_send_hands_off() -> TestResult {
    cold_candidate_case(ColdCandidate::HeardOrdinaryFailure)
}

#[test]
fn a_free_arm_taken_during_the_read_cannot_deliver_a_due_check_in() -> TestResult {
    cold_candidate_case(ColdCandidate::FreeArmTaken)
}

#[test]
fn a_held_ordinary_failure_shared_at_the_armed_head_reaches_this_owner() -> TestResult {
    cold_candidate_case(ColdCandidate::SharedOrdinaryFailure)
}

#[test]
fn a_held_ordinary_failure_arriving_during_candidate_verification_is_told_once() -> TestResult {
    cold_candidate_case(ColdCandidate::OrdinaryFailsDuringRead)
}

enum ColdCandidate {
    OrdinaryFailsDuringRead,
    SharedOrdinaryFailure,
    HeldOrdinaryFailure,
    HeardOrdinaryFailure,
    FreeArmTaken,
    OlderOrdinaryFailure,
    ReplacedOrdinaryError,
    OrdinaryDropAfterRearm,
    UnreadableArm,
    NewerRunAfterExpiry,
    RearmedInTrain,
    RearmedOutsideTrain,
    TimedOutOrdinary,
    PriorBisection,
    Stall,
    MovedHead,
    OwnerRerun,
    Backdated,
    UnrelatedRuns,
    PriorNotice,
    OrdinaryCancelled,
    OrdinaryError,
    SourceIncludesOwner,
}

fn cold_candidate_case(case: ColdCandidate) -> TestResult {
    let prior_notice = matches!(case, ColdCandidate::PriorNotice);
    let (_dir, data) = data_dir()?;
    let rt = runtime()?;
    let (id, head, original) = rt.block_on(seed_candidate(&data, &case))?;
    rt.shutdown_timeout(Duration::from_secs(5));
    let rt = runtime()?;
    rt.block_on(check_after_restart(&data, &case, &id, &head, &original))?;
    rt.shutdown_timeout(Duration::from_secs(5));
    if prior_notice {
        // Earlier releases stored the live train decision under "outcome". The same run
        // must stay heard after restart with that historical key and no armed slot.
        let path = data.join(yulius_app::pr_watch::PR_NOTICES_FILE);
        let entries: std::collections::BTreeMap<String, yulius_app::pr_watch::Told> =
            serde_json::from_slice(&std::fs::read(&path)?)?;
        let told = entries.get(id.as_str()).ok_or("saved owner notice")?;
        assert!(
            told.keys.contains_key("outcome"),
            "legacy outcome fingerprint"
        );
        let rt = runtime()?;
        rt.block_on(async {
            let (again, remote) = app_with_pulls(&data, 8, 1).await?;
            open_pull_at(&again, &remote, &id, 1481, &head).await?;
            remote.set_remote(Arc::new(crate::delivery::BareRemote {
                origin: data.join("origin.git"),
            }));
            remote.set_checks(&head, ChecksState::Success);
            again.reconcile().await?;
            assert_eq!(
                again
                    .deps
                    .store
                    .live_attempt(&id)
                    .await?
                    .ok_or("reattached")?
                    .id,
                original
            );
            let duplicate = again.watch_prs(&WatchBounds::default()).await;
            again.stop_task_now(&id).await?;
            assert!(
                duplicate.is_empty(),
                "legacy verdict repeated: {duplicate:?}"
            );
            Ok::<_, Error>(())
        })?;
        rt.shutdown_timeout(Duration::from_secs(5));
    }
    Ok(())
}

async fn seed_candidate(
    data: &Path,
    case: &ColdCandidate,
) -> Result<(TaskId, String, AttemptId), Error> {
    let (app, github) = app_with_pulls(data, 8, 1).await?;
    let (id, caller) = answered(&app, data).await?;
    let task = app.deps.store.task(&id).await?.ok_or("task")?;
    let tree = Path::new(task.worktree_path.as_deref().ok_or("checkout")?);
    crate::pr_watch::git(&data.join("seed"), &["push", "-q", "origin", "main:dev"])?;
    crate::pr_watch::git(tree, &["fetch", "-q", "origin"])?;
    std::fs::write(tree.join("latency-check"), b"search limit")?;
    crate::pr_watch::git(tree, &["add", "latency-check"])?;
    crate::pr_watch::git(tree, &["commit", "-qm", "Check latency"])?;
    let head = app
        .deps
        .git
        .resolve_commit(tree, "HEAD")
        .await?
        .ok_or("head")?;
    open_pull_at(&app, &github, &id, 1481, &head).await?;
    github.set_checks(&head, ChecksState::Success);
    app.deps
        .store
        .transition_delivery(
            &DeliveryStage::None,
            &DeliveryRecord {
                task_id: id.clone(),
                stage: DeliveryStage::PrOpen,
                pr_number: Some(1481),
                updated_at: app.now(),
            },
        )
        .await?;
    let steps = (1..=4)
        .map(|ordinal| {
            Ok(yulius_app::model::StepRecord {
                id: yulius_domain::ids::StepId::parse(&format!("done-{ordinal}"))?,
                task_id: id.clone(),
                ordinal,
                title: format!("Step {ordinal}"),
                status: "done".into(),
                note: None,
                updated_by: None,
                updated_at: None,
                effort: 1,
            })
        })
        .collect::<Result<Vec<_>, Error>>()?;
    app.deps
        .store
        .replace_steps(&id, &steps, "agent", app.now())
        .await?;
    let (mut green, job) = crate::pr_watch::run(
        &caller.sandbox,
        "ordinary-green",
        &head,
        match case {
            ColdCandidate::TimedOutOrdinary | ColdCandidate::NewerRunAfterExpiry => {
                RunState::Running
            }
            ColdCandidate::OlderOrdinaryFailure => RunState::Failed,
            ColdCandidate::OrdinaryCancelled => RunState::Cancelled,
            ColdCandidate::OrdinaryError | ColdCandidate::ReplacedOrdinaryError => RunState::Error,
            _ => RunState::Passed,
        },
        None,
        10,
    );
    green.started_at = Some(yulius_domain::time::Timestamp(11));
    green.ended_at = Some(yulius_domain::time::Timestamp(12));
    green.reference = CiRef::PullRequest(1481);
    if matches!(case, ColdCandidate::SharedOrdinaryFailure) {
        green.sha = "previous-head".into();
    }
    app.deps.store.insert_ci_run(&green, &[job]).await?;
    let failures = concat!(
        "FAIL [0.292s] (1/1) search latency\n",
        "  stderr ───\n",
        "    search took 292.1 ms, limit 200 ms\n",
    );
    let integrated = crate::pr_watch::git(
        tree,
        &[
            "commit-tree",
            "HEAD^{tree}",
            "-p",
            "origin/dev",
            "-m",
            "candidate",
        ],
    )?;
    let (mut candidate, mut job) = crate::pr_watch::run(
        &caller.sandbox,
        "candidate-red",
        &integrated,
        RunState::Failed,
        Some(failures),
        20,
    );
    if matches!(case, ColdCandidate::PriorNotice) {
        let notice = app.watch_prs(&WatchBounds::default()).await;
        assert_eq!(notice.len(), 1);
        assert!(notice[0].1.contains("no merge is armed"), "{notice:?}");
    }
    if matches!(
        case,
        ColdCandidate::PriorBisection
            | ColdCandidate::OrdinaryDropAfterRearm
            | ColdCandidate::UnreadableArm
            | ColdCandidate::RearmedOutsideTrain
            | ColdCandidate::RearmedInTrain
    ) {
        let (mut prior, job) = crate::pr_watch::run(
            &caller.sandbox,
            "earlier-bisection",
            &integrated,
            RunState::Failed,
            Some(failures),
            17,
        );
        prior.reference = CiRef::Train(vec![1481, 99]);
        app.deps.store.insert_ci_run(&prior, &[job]).await?;
    }
    candidate.queued_at =
        yulius_domain::time::Timestamp(if matches!(case, ColdCandidate::Backdated) {
            9
        } else {
            20
        });
    candidate.started_at = Some(yulius_domain::time::Timestamp(21));
    candidate.ended_at = Some(yulius_domain::time::Timestamp(22));
    candidate.reference = CiRef::Train(if matches!(case, ColdCandidate::SourceIncludesOwner) {
        vec![1481, 99]
    } else {
        vec![88, 99]
    });
    job.name = "shard 2".into();
    app.deps.store.insert_ci_run(&candidate, &[job]).await?;
    app.deps
        .store
        .transition_ci_job(
            "candidate-red-quality",
            &[RunState::Failed],
            RunState::Failed,
            &yulius_app::ci::CiJobUpdate {
                failures: Some(failures.into()),
                exit_code: Some(100),
                ..Default::default()
            },
        )
        .await?;
    // A different train tested the same tree. Only the queue's accepted
    // singleton decision connects that source run to this PR owner.
    let base = crate::pr_watch::git(tree, &["rev-parse", "origin/dev"])?;
    let tested_tree = crate::pr_watch::git(tree, &["rev-parse", "HEAD^{tree}"])?;
    app.deps
    .store
    .insert_span(&yulius_app::stages::NewSpan {
        sandbox_id: caller.sandbox.clone(),
        subject_kind: yulius_domain::stages::SubjectKind::Task,
        subject_id: id.as_str().into(),
        key: "wait@15".into(),
        parent_key: None,
        kind: "wait".into(),
        name: "merge_armed".into(),
        environment: None,
        job: None,
        started_at: yulius_domain::time::Timestamp(15),
        ended_at: Some(yulius_domain::time::Timestamp(23)),
        outcome: Some("dropped".into()),
        detail: Some(
            json!({"head":head, "expired_ordinary_run":matches!(case, ColdCandidate::TimedOutOrdinary | ColdCandidate::NewerRunAfterExpiry).then_some("ordinary-green"), "message":"Merge candidate candidate-red failed in shard 2: search took 292.1 ms, limit 200 ms. Read the failing job with ci_status.", "singleton":{"pr":1481, "base":base,
        "tree":tested_tree,"run":"candidate-red"}})
            .to_string(),
        ),
    })
    .await?;
    let later_runs = if matches!(
        case,
        ColdCandidate::UnrelatedRuns
            | ColdCandidate::OlderOrdinaryFailure
            | ColdCandidate::SharedOrdinaryFailure
    ) {
        101
    } else {
        0
    };
    for n in 0..later_runs {
        let (run, job) = crate::pr_watch::run(
            &caller.sandbox,
            &format!("other-{n}"),
            "another-head",
            RunState::Passed,
            None,
            30 + n,
        );
        app.deps.store.insert_ci_run(&run, &[job]).await?;
    }
    let original = app
        .deps
        .store
        .live_attempt(&id)
        .await?
        .ok_or("original attempt")?
        .id;
    Ok((id, head, original))
}

async fn check_after_restart(
    data: &Path,
    case: &ColdCandidate,
    id: &TaskId,
    head: &str,
    original: &AttemptId,
) -> TestResult {
    let prior_notice = matches!(case, ColdCandidate::PriorNotice);
    let (cold, remote) = app_with_pulls(data, 8, 1).await?;
    let origin = data.join("origin.git");
    remote.set_remote(Arc::new(crate::delivery::BareRemote {
        origin: origin.clone(),
    }));
    open_pull_at(&cold, &remote, id, 1481, head).await?;
    cold.reconcile().await?;
    assert_eq!(
        cold.deps
            .store
            .live_attempt(id)
            .await?
            .ok_or("reattached")?
            .id,
        *original
    );
    remote.set_checks(head, ChecksState::Success);
    let bounds = WatchBounds::default();
    ask(&cold, id).await?;
    assert!(cold.watch_prs(&bounds).await.is_empty());
    cold.deps
        .store
        .set_task_needs_you(id, None, cold.now())
        .await?;
    if matches!(
        case,
        ColdCandidate::FreeArmTaken
            | ColdCandidate::HeldOrdinaryFailure
            | ColdCandidate::HeardOrdinaryFailure
            | ColdCandidate::SharedOrdinaryFailure
            | ColdCandidate::OrdinaryFailsDuringRead
    ) {
        std::fs::write(
            data.join(yulius_app::agent_wait::ARMED_MERGES_FILE),
            serde_json::to_vec(
                &json!([{"task":id.as_str(),"head":head,"summary":"reviewed","armed_at":30}]),
            )?,
        )?;
        cold.restore_armed_merges().await;
        let sandbox = cold.deps.store.task(id).await?.ok_or("owner")?.sandbox_id;
        if matches!(case, ColdCandidate::FreeArmTaken) {
            let (observed, finish_read) = hold_at(super::Point::LivePrFactsRead, id);
            let (deciding, finish_decision) = hold_at(super::Point::FreeArmTaken, id);
            let watch = Arc::clone(&cold);
            let watcher = tokio::spawn(async move {
                watch
                    .watch_prs(&WatchBounds {
                        check_in_after_ms: 0,
                        stall_quiet_ms: 0,
                        stall_after_ms: 0,
                    })
                    .await
            });
            tokio::time::timeout(Duration::from_secs(10), observed.notified()).await?;
            let queue = Arc::clone(&cold);
            let pass = tokio::spawn(async move { queue.merge_armed(&sandbox).await });
            tokio::time::timeout(Duration::from_secs(10), deciding.notified()).await?;
            finish_read.notify_one();
            let news = watcher.await?;
            finish_decision.notify_one();
            pass.await?;
            cold.stop_task_now(id).await?;
            assert!(news.is_empty(), "stale free-arm check-in: {news:?}");
            return Ok(());
        }
        let heard = matches!(case, ColdCandidate::HeardOrdinaryFailure);
        let (mut red, job) =
            crate::pr_watch::run(&sandbox, "ordinary-red", head, RunState::Failed, None, 31);
        red.reference =
            CiRef::PullRequest(if matches!(case, ColdCandidate::SharedOrdinaryFailure) {
                99
            } else {
                1481
            });
        let during = matches!(case, ColdCandidate::OrdinaryFailsDuringRead);
        if during {
            let app = Arc::clone(&cold);
            let handle = tokio::runtime::Handle::current();
            yulius_app::race::on(
                super::Point::LivePrFactsRead,
                Path::new(id.as_str()),
                move || {
                    let result = tokio::task::block_in_place(|| {
                        handle.block_on(app.deps.store.insert_ci_run(&red, &[job]))
                    });
                    assert!(result.is_ok(), "ordinary CI update: {result:?}");
                },
            );
        } else {
            cold.deps.store.insert_ci_run(&red, &[job]).await?;
        }
        if heard {
            let news = cold.watch_prs(&bounds).await;
            assert_eq!(news.len(), 1, "{news:?}");
            assert!(news[0].1.contains("ordinary-red"), "{news:?}");
        }
        let train = json!({sandbox.as_str(): {"train":{"run":"candidate-red","commit":"merge","base":"base","base_tree":"tree","members":[{"task":id.as_str(),"pr":1481,"head":head,"armed_at":30,"tree":"candidate-tree"}]}}});
        std::fs::write(
            data.join(yulius_app::merge_train::MERGE_TRAINS_FILE),
            serde_json::to_vec(&train)?,
        )?;
        if during {
            let stale = cold.watch_prs(&bounds).await;
            assert!(
                stale.is_empty(),
                "candidate check-in overtaken by ordinary CI: {stale:?}"
            );
        }
        let news = cold.watch_prs(&bounds).await;
        let repeated = cold.watch_prs(&bounds).await;
        cold.stop_task_now(id).await?;
        assert_eq!(news.len(), usize::from(!heard), "{news:?}");
        if !heard {
            assert!(
                news[0].1.contains("ordinary-red") && !news[0].1.contains("Do not re-arm"),
                "{news:?}"
            );
        }
        assert!(repeated.is_empty(), "{repeated:?}");
        return Ok(());
    }
    if matches!(case, ColdCandidate::MovedHead) {
        set_pull_head(&remote, 1481, "replacement")?;
        let obsolete = cold.watch_prs(&bounds).await;
        assert!(
            obsolete.is_empty(),
            "green notice for the replaced head: {obsolete:?}"
        );
        set_pull_head(&remote, 1481, head)?;
    }
    if matches!(
        case,
        ColdCandidate::OwnerRerun | ColdCandidate::NewerRunAfterExpiry
    ) {
        let gate = Arc::new(tokio::sync::Notify::new());
        *remote.pull_read_hold.lock().map_err(|_| "PR gate")? = Some(gate.clone());
        let watcher = cold.clone();
        let pass = tokio::spawn(async move { watcher.watch_prs(&WatchBounds::default()).await });
        tokio::time::timeout(Duration::from_secs(5), remote.pull_read_entered.notified()).await?;
        let task = cold.deps.store.task(id).await?.ok_or("owner")?;
        let (mut rerun, job) = crate::pr_watch::run(
            &task.sandbox_id,
            "ordinary-rerun",
            head,
            RunState::Running,
            None,
            30,
        );
        rerun.reference = CiRef::PullRequest(1481);
        cold.deps.store.insert_ci_run(&rerun, &[job]).await?;
        *remote.pull_read_hold.lock().map_err(|_| "PR gate")? = None;
        gate.notify_one();
        let typed = tokio::time::timeout(Duration::from_secs(10), pass).await??;
        assert!(
            typed.is_empty(),
            "the owner rerun supersedes a stale green notice: {typed:?}"
        );
        assert!(
            cold.deps
                .store
                .transition_ci_run(
                    "ordinary-rerun",
                    &[RunState::Running],
                    RunState::Passed,
                    &yulius_app::ci::CiRunUpdate {
                        ended_at: Some(cold.now()),
                        ..Default::default()
                    }
                )
                .await?
        );
    }
    if matches!(case, ColdCandidate::ReplacedOrdinaryError) {
        let gate = Arc::new(tokio::sync::Notify::new());
        *remote.pull_read_hold.lock().map_err(|_| "PR gate")? = Some(gate.clone());
        let watcher = cold.clone();
        let pass = tokio::spawn(async move { watcher.watch_prs(&WatchBounds::default()).await });
        tokio::time::timeout(Duration::from_secs(5), remote.pull_read_entered.notified()).await?;
        let sandbox = cold.deps.store.task(id).await?.ok_or("owner")?.sandbox_id;
        let (mut rerun, job) = crate::pr_watch::run(
            &sandbox,
            "superseding-green",
            head,
            RunState::Passed,
            None,
            30,
        );
        rerun.reference = CiRef::PullRequest(1481);
        cold.deps.store.insert_ci_run(&rerun, &[job]).await?;
        *remote.pull_read_hold.lock().map_err(|_| "PR gate")? = None;
        gate.notify_one();
        let mut typed = tokio::time::timeout(Duration::from_secs(10), pass).await??;
        typed.extend(cold.watch_prs(&bounds).await);
        assert_eq!(typed.len(), 1, "{typed:?}");
        assert!(
            typed[0].1.contains("292.1 ms") && !typed[0].1.contains("ordinary-green"),
            "{typed:?}"
        );
        cold.stop_task_now(id).await?;
        return Ok(());
    }
    if matches!(case, ColdCandidate::SourceIncludesOwner) {
        let saved_remote = remote.remote.lock().map_err(|_| "remote")?.take();
        {
            let mut pulls = remote.pulls.lock().map_err(|_| "fixture pull")?;
            pulls.first_mut().ok_or("pull")?.base_sha = "later-base".into();
        }
        let stale = cold.watch_prs(&bounds).await;
        assert!(
            stale
                .iter()
                .all(|(_, text)| !text.contains("Do not re-arm")),
            "obsolete source after a base change: {stale:?}"
        );
        *remote.remote.lock().map_err(|_| "remote")? = saved_remote;
    }
    if prior_notice {
        // The remote base advanced while the daemon's origin/dev still names the tested base.
        let seed = data.join("seed");
        let base = crate::pr_watch::git(&origin, &["rev-parse", "dev"])?;
        std::fs::write(seed.join("changed-base"), b"new integration tree")?;
        crate::pr_watch::git(&seed, &["add", "changed-base"])?;
        crate::pr_watch::git(&seed, &["commit", "-qm", "Advance integration"])?;
        crate::pr_watch::git(&seed, &["push", "-q", "origin", "HEAD:dev"])?;
        let obsolete = cold.watch_prs(&bounds).await;
        crate::pr_watch::git(&origin, &["update-ref", "refs/heads/dev", &base])?;
        assert!(
            obsolete.is_empty(),
            "superseded integration tree: {obsolete:?}"
        );
        for (base, sha) in [("release", head), ("dev", "superseding-head")] {
            {
                let mut pulls = remote.pulls.lock().map_err(|_| "fixture pull")?;
                let pull = pulls.first_mut().ok_or("pull")?;
                pull.base = base.into();
                pull.head_sha = sha.into();
            }
            assert!(
                cold.watch_prs(&bounds).await.is_empty(),
                "obsolete candidate {base} {sha}"
            );
        }
        set_pull_head(&remote, 1481, head)?;
        let arms = data.join(yulius_app::agent_wait::ARMED_MERGES_FILE);
        for rearmed in [false, true] {
            let gate = Arc::new(tokio::sync::Notify::new());
            *remote.pull_read_hold.lock().map_err(|_| "PR gate")? = Some(gate.clone());
            let watcher = cold.clone();
            let pass =
                tokio::spawn(async move { watcher.watch_prs(&WatchBounds::default()).await });
            let entered =
                tokio::time::timeout(Duration::from_secs(5), remote.pull_read_entered.notified())
                    .await;
            if rearmed {
                std::fs::write(
                    &arms,
                    serde_json::to_vec(
                        &json!([{"task":id.as_str(),"head":head,"summary":"approved"}]),
                    )?,
                )?;
            } else {
                ask(&cold, id).await?;
            }
            *remote.pull_read_hold.lock().map_err(|_| "PR gate")? = None;
            gate.notify_one();
            let typed = tokio::time::timeout(Duration::from_secs(10), pass).await??;
            entered?;
            assert!(typed.is_empty(), "decision crossed the PR read: {typed:?}");
            if rearmed {
                std::fs::write(&arms, b"[]")?;
            } else {
                cold.deps
                    .store
                    .set_task_needs_you(id, None, cold.now())
                    .await?;
            }
        }
    }
    if matches!(case, ColdCandidate::OrdinaryDropAfterRearm) {
        let task = cold.deps.store.task(id).await?.ok_or("task")?;
        cold.deps
            .store
            .insert_span(&yulius_app::stages::NewSpan {
                sandbox_id: task.sandbox_id,
                subject_kind: yulius_domain::stages::SubjectKind::Task,
                subject_id: id.to_string(),
                key: "wait@30".into(),
                parent_key: None,
                kind: "wait".into(),
                name: "merge_armed".into(),
                environment: None,
                job: None,
                started_at: yulius_domain::time::Timestamp(30),
                ended_at: Some(yulius_domain::time::Timestamp(32)),
                outcome: Some("dropped".into()),
                detail: Some(json!({"head":head}).to_string()),
            })
            .await?;
        let seen = cold.watch_prs(&bounds).await;
        assert!(
            seen.iter()
                .all(|(_, text)| !text.contains("earlier-bisection") && !text.contains("292.1")),
            "obsolete decision: {seen:?}"
        );
        cold.stop_task_now(id).await?;
        return Ok(());
    }
    if matches!(case, ColdCandidate::UnreadableArm) {
        let path = data.join(yulius_app::agent_wait::ARMED_MERGES_FILE);
        std::fs::write(&path, b"[")?;
        cold.restore_armed_merges().await;
        let due = WatchBounds {
            check_in_after_ms: 0,
            stall_quiet_ms: 0,
            stall_after_ms: 0,
        };
        let unavailable = cold.watch_prs(&due).await;
        assert!(
            unavailable.is_empty(),
            "unresolved arm recovery: {unavailable:?}"
        );
        std::fs::write(
            &path,
            serde_json::to_vec(
                &json!([{"task":id.as_str(),"head":head,"summary":"reviewed","armed_at":30}]),
            )?,
        )?;
        cold.restore_armed_merges().await;
        let seen = cold.watch_prs(&bounds).await;
        assert!(
            seen.iter()
                .all(|(_, text)| !text.contains("earlier-bisection") && !text.contains("292.1")),
            "revived failure: {seen:?}"
        );
        cold.stop_task_now(id).await?;
        return Ok(());
    }
    if matches!(
        case,
        ColdCandidate::RearmedOutsideTrain | ColdCandidate::RearmedInTrain
    ) {
        std::fs::write(
            data.join(yulius_app::agent_wait::ARMED_MERGES_FILE),
            serde_json::to_vec(
                &json!([{"task":id.as_str(),"head":head,"summary":"reviewed","armed_at":30}]),
            )?,
        )?;
        cold.restore_armed_merges().await;
        if matches!(case, ColdCandidate::RearmedInTrain) {
            let task = cold.deps.store.task(id).await?.ok_or("owner")?;
            let current = json!({task.sandbox_id.as_str(): {"train":{"run":"later-train","commit":"merge","base":"base","base_tree":"tree","members":[{"task":id.as_str(),"pr":1481,"head":head,"armed_at":30,"tree":"candidate-tree"}]}}});
            let path = data.join(yulius_app::merge_train::MERGE_TRAINS_FILE);
            for (field, replaced) in [
                ("armed_at", json!(15)),
                ("head", json!("old-head")),
                ("pr", json!(99)),
                ("task", json!("previous-owner")),
            ] {
                let mut obsolete = current.clone();
                let train = &mut obsolete[task.sandbox_id.as_str()]["train"];
                train["run"] = json!("earlier-bisection");
                train["members"][0][field] = replaced;
                std::fs::write(&path, serde_json::to_vec(&obsolete)?)?;
                let news = cold.watch_prs(&bounds).await;
                assert!(
                    news.iter()
                        .all(|(_, text)| !text.contains("earlier-bisection")
                            && !text.contains("Hands off")),
                    "stale member {field}: {news:?}"
                );
            }
            std::fs::write(&path, serde_json::to_vec(&current)?)?;
        }
        let seen = cold.watch_prs(&bounds).await;
        assert!(
            seen.iter()
                .all(|(_, text)| !text.contains("earlier-bisection") && !text.contains("292.1")),
            "resolved bisection after rearm: {seen:?}"
        );
        if matches!(case, ColdCandidate::RearmedInTrain) {
            let task = cold.deps.store.task(id).await?.ok_or("owner")?;
            let path = data.join(yulius_app::merge_train::MERGE_TRAINS_FILE);
            let mut current: serde_json::Value = serde_json::from_slice(&std::fs::read(&path)?)?;
            current[task.sandbox_id.as_str()]["train"]["run"] = json!("earlier-bisection");
            std::fs::write(path, serde_json::to_vec(&current)?)?;
            let held = cold.watch_prs(&bounds).await;
            assert!(
                held.iter()
                    .any(|(_, text)| text.contains("earlier-bisection")
                        && text.contains("Do not re-arm")
                        && !text.contains("#99")
                        && !text.contains("smaller candidates")),
                "the current holding train: {held:?}"
            );
        }
        cold.stop_task_now(id).await?;
        return Ok(());
    }
    let notices = cold.watch_prs(&bounds).await;
    let repeated = cold.watch_prs(&bounds).await;
    if matches!(case, ColdCandidate::OlderOrdinaryFailure) {
        assert_eq!(notices.len(), 1, "{notices:?}");
        assert!(
            notices[0].1.contains("ordinary-green") && !notices[0].1.contains("candidate-red"),
            "{notices:?}"
        );
        assert!(repeated.is_empty(), "{repeated:?}");
        cold.stop_task_now(id).await?;
        return Ok(());
    }
    if matches!(case, ColdCandidate::Stall) {
        let bounds = WatchBounds {
            stall_quiet_ms: 0,
            stall_after_ms: 0,
            ..WatchBounds::default()
        };
        let stalled = cold.watch_prs(&bounds).await;
        assert_eq!(
            stalled.len(),
            1,
            "expected bounded stall check-in: {stalled:?}"
        );
        assert!(
            stalled[0].1.contains("stalled") && stalled[0].1.contains("candidate"),
            "{stalled:?}"
        );
        assert!(
            !stalled[0].1.contains("292.1") && !stalled[0].1.contains("Read the failing job"),
            "repeated actionable diagnosis: {stalled:?}"
        );
    }
    let attempts = cold.deps.store.attempts(id).await?;
    if !prior_notice {
        cold.stop_task_now(id).await?;
    }
    assert_eq!(notices.len(), 1, "{notices:?}");
    assert_eq!(&notices[0].0, id);
    assert!(
        !notices[0].1.contains("Do not re-arm"),
        "resolved source: {notices:?}"
    );
    for evidence in ["candidate-red", "shard 2", "292.1 ms"] {
        assert!(notices[0].1.contains(evidence), "{notices:?}");
    }
    assert!(repeated.is_empty(), "{repeated:?}");
    assert_eq!(attempts.len(), 1);
    Ok(())
}

fn hold_at(
    point: super::Point,
    id: &TaskId,
) -> (Arc<tokio::sync::Notify>, Arc<tokio::sync::Notify>) {
    let entered = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    let signal = entered.clone();
    let gate = release.clone();
    let handle = tokio::runtime::Handle::current();
    yulius_app::race::on(point, Path::new(id.as_str()), move || {
        signal.notify_one();
        let released = tokio::task::block_in_place(|| {
            handle.block_on(tokio::time::timeout(
                Duration::from_secs(15),
                gate.notified(),
            ))
        });
        assert!(released.is_ok(), "test did not release the decision");
    });
    (entered, release)
}
