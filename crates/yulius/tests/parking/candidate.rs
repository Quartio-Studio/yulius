//! Original-owner PR notifications across restart with real Git, SQLite and PTYs.
use super::{Error, answered, app_with_pulls, ask, data_dir, open_pull_at, runtime};
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

#[test]
fn a_held_candidate_preserves_a_person_decision_arriving_at_final_verification() -> TestResult {
    cold_candidate_case(ColdCandidate::HeldHumanDecision)
}

#[test]
fn an_armed_owner_outside_a_train_hears_the_queues_actual_reason_once() -> TestResult {
    cold_candidate_case(ColdCandidate::FreeArmNote)
}

#[test]
fn a_pr_left_out_of_the_train_gets_its_own_queue_note_without_the_members_failure() -> TestResult {
    cold_candidate_case(ColdCandidate::LeftOut)
}

#[test]
fn an_ordinary_failure_shared_while_the_train_runs_reaches_its_current_owner() -> TestResult {
    cold_candidate_case(ColdCandidate::SharedWhileRunning)
}

#[test]
fn a_free_queue_note_waits_for_a_person_decision_that_arrives_during_verification() -> TestResult {
    cold_candidate_case(ColdCandidate::FreeHumanDecision)
}

#[test]
fn a_train_that_fails_during_final_verification_reaches_its_owner_once() -> TestResult {
    cold_candidate_case(ColdCandidate::TrainFailsDuringRead)
}

#[test]
fn a_free_arm_kept_with_a_new_queue_reason_defers_the_prepared_check_in() -> TestResult {
    cold_candidate_case(ColdCandidate::FreeNoteChanges)
}

#[test]
fn an_instance_with_ci_disabled_does_not_send_an_older_shared_failure_to_an_armed_owner()
-> TestResult {
    cold_candidate_case(ColdCandidate::DisabledSharedFailure)
}

enum ColdCandidate {
    DisabledSharedFailure,
    FreeNoteChanges,
    TrainFailsDuringRead,
    SharedWhileRunning,
    FreeHumanDecision,
    LeftOut,
    OrdinaryDropAfterRearm,
    UnreadableArm,
    RearmedOutsideTrain,
    RearmedInTrain,
    FreeArmTaken,
    HeldOrdinaryFailure,
    HeardOrdinaryFailure,
    SharedOrdinaryFailure,
    OrdinaryFailsDuringRead,
    HeldHumanDecision,
    FreeArmNote,
}

fn cold_candidate_case(case: ColdCandidate) -> TestResult {
    let (_dir, data) = data_dir()?;
    let rt = runtime()?;
    let (id, head, original) = rt.block_on(seed_candidate(&data, &case))?;
    rt.shutdown_timeout(Duration::from_secs(5));
    let rt = runtime()?;
    rt.block_on(check_after_restart(&data, &case, &id, &head, &original))?;
    rt.shutdown_timeout(Duration::from_secs(5));
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
        RunState::Passed,
        None,
        10,
    );
    green.started_at = Some(yulius_domain::time::Timestamp(11));
    green.ended_at = Some(yulius_domain::time::Timestamp(12));
    green.reference = CiRef::PullRequest(1481);
    if matches!(
        case,
        ColdCandidate::SharedOrdinaryFailure
            | ColdCandidate::SharedWhileRunning
            | ColdCandidate::DisabledSharedFailure
    ) {
        green.sha = "previous-head".into();
    }
    app.deps.store.insert_ci_run(&green, &[job]).await?;
    if matches!(case, ColdCandidate::DisabledSharedFailure) {
        let (mut historical, job) = crate::pr_watch::run(
            &caller.sandbox,
            "shared-before-disable",
            &head,
            RunState::Failed,
            None,
            31,
        );
        historical.reference = CiRef::PullRequest(99);
        app.deps.store.insert_ci_run(&historical, &[job]).await?;
    }
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
        if matches!(
            case,
            ColdCandidate::SharedWhileRunning | ColdCandidate::TrainFailsDuringRead
        ) {
            RunState::Running
        } else {
            RunState::Failed
        },
        Some(failures),
        20,
    );
    if matches!(
        case,
        ColdCandidate::OrdinaryDropAfterRearm
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
    candidate.started_at = Some(yulius_domain::time::Timestamp(21));
    candidate.ended_at = Some(yulius_domain::time::Timestamp(22));
    candidate.reference = CiRef::Train(vec![88, 99]);
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
            json!({"head":head, "message":"Merge candidate candidate-red failed in shard 2: search took 292.1 ms, limit 200 ms. Read the failing job with ci_status.", "singleton":{"pr":1481, "base":base,
        "tree":tested_tree,"run":"candidate-red"}})
            .to_string(),
        ),
    })
    .await?;
    let later_runs = if matches!(case, ColdCandidate::SharedOrdinaryFailure) {
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
    let (cold, remote) = super::configured_app_with_pulls(data, 8, 1, |config| {
        if matches!(case, ColdCandidate::DisabledSharedFailure) {
            config.ci.enabled = false;
        }
    })
    .await?;
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
            | ColdCandidate::LeftOut
            | ColdCandidate::TrainFailsDuringRead
            | ColdCandidate::DisabledSharedFailure
            | ColdCandidate::FreeNoteChanges
            | ColdCandidate::FreeHumanDecision
            | ColdCandidate::SharedWhileRunning
            | ColdCandidate::FreeArmNote
            | ColdCandidate::HeldOrdinaryFailure
            | ColdCandidate::HeardOrdinaryFailure
            | ColdCandidate::SharedOrdinaryFailure
            | ColdCandidate::OrdinaryFailsDuringRead
            | ColdCandidate::HeldHumanDecision
    ) {
        std::fs::write(
            data.join(yulius_app::agent_wait::ARMED_MERGES_FILE),
            serde_json::to_vec(
                &json!([{"task":id.as_str(),"head":head,"summary":"reviewed","armed_at":30}]),
            )?,
        )?;
        cold.restore_armed_merges().await;
        let sandbox = cold.deps.store.task(id).await?.ok_or("owner")?.sandbox_id;
        if matches!(case, ColdCandidate::DisabledSharedFailure) {
            let news = cold.watch_prs(&bounds).await;
            cold.stop_task_now(id).await?;
            assert!(
                news.is_empty(),
                "disabled CI supplied historical ordinary advice: {news:?}"
            );
            return Ok(());
        }
        if matches!(case, ColdCandidate::LeftOut) {
            let path = data.join(yulius_app::merge_train::MERGE_TRAINS_FILE);
            let mut state = json!({sandbox.as_str(): {"train":{"run":"candidate-red","commit":"merge","base":"base","base_tree":"tree","members":[{"task":"other","pr":90,"head":"other","armed_at":25,"tree":"tree"}],"left_out":[{"task":id.as_str(),"pr":99,"files":["old.rs"]},{"task":"another-owner","pr":1481,"files":["another.rs"]}]}}});
            std::fs::write(&path, serde_json::to_vec(&state)?)?;
            let obsolete = cold.watch_prs(&bounds).await;
            assert!(
                obsolete.is_empty(),
                "left-out note for a different owner or PR: {obsolete:?}"
            );
            state[sandbox.as_str()]["train"]["left_out"][0] = json!({"task":id.as_str(),"pr":1481,"head":head,"armed_at":30,"files":["owned.rs"]});
            for (index, (old_head, old_activation)) in [
                (Some("previous-head"), Some(30)),
                (Some(head), Some(29)),
                (None, None),
            ]
            .into_iter()
            .enumerate()
            {
                let mut older = state.clone();
                let excluded = &mut older[sandbox.as_str()]["train"]["left_out"][0];
                excluded["head"] = json!(old_head);
                excluded["armed_at"] = json!(old_activation);
                std::fs::write(&path, serde_json::to_vec(&older)?)?;
                let notices = cold.watch_prs(&bounds).await;
                if index == 0 {
                    assert_eq!(notices.len(), 1, "queue position: {notices:?}");
                    assert!(
                        notices[0].1.contains("waits behind merge train #90")
                            && !notices[0].1.contains("owned.rs"),
                        "{notices:?}"
                    );
                } else {
                    assert!(
                        notices.is_empty(),
                        "same queue position repeated or old files attributed: {notices:?}"
                    );
                }
            }
            std::fs::write(&path, serde_json::to_vec(&state)?)?;
            let news = cold.watch_prs(&bounds).await;
            let repeated = cold.watch_prs(&bounds).await;
            cold.stop_task_now(id).await?;
            assert_eq!(news.len(), 1, "{news:?}");
            assert!(
                news[0].1.contains("left out")
                    && news[0].1.contains("owned.rs")
                    && news[0].1.contains("one by one after them")
                    && !news[0].1.contains("Yulius merges it in order")
                    && !news[0].1.contains("292.1"),
                "{news:?}"
            );
            assert!(repeated.is_empty(), "{repeated:?}");
            return Ok(());
        }
        if matches!(
            case,
            ColdCandidate::FreeArmNote
                | ColdCandidate::FreeHumanDecision
                | ColdCandidate::FreeNoteChanges
        ) {
            let waiting = if matches!(case, ColdCandidate::FreeNoteChanges) {
                let (entered, release) =
                    super::pause_watch_or_queue(super::Point::LivePrFactsRead, id);
                let watcher = Arc::clone(&cold);
                let pass = tokio::spawn(async move {
                    watcher
                        .watch_prs(&WatchBounds {
                            check_in_after_ms: 0,
                            stall_quiet_ms: 0,
                            stall_after_ms: 0,
                        })
                        .await
                });
                tokio::time::timeout(Duration::from_secs(10), entered.notified()).await?;
                Some((release, pass))
            } else {
                None
            };
            let pulls = std::mem::take(&mut *remote.pulls.lock().map_err(|_| "pulls")?);
            cold.merge_armed(&sandbox).await;
            *remote.pulls.lock().map_err(|_| "pulls")? = pulls;
            if let Some((release, pass)) = waiting {
                release.notify_one();
                let outdated = pass.await?;
                assert!(
                    outdated.is_empty(),
                    "the queue kept the same arm with a new reason: {outdated:?}"
                );
            }
            if matches!(case, ColdCandidate::FreeHumanDecision) {
                let (entered, release) =
                    super::pause_watch_or_queue(super::Point::LivePrFactsRead, id);
                let watcher = Arc::clone(&cold);
                let pass = tokio::spawn(async move { watcher.watch_prs(&bounds).await });
                tokio::time::timeout(Duration::from_secs(10), entered.notified()).await?;
                ask(&cold, id).await?;
                release.notify_one();
                let blocked = pass.await?;
                assert!(
                    blocked.is_empty(),
                    "queue note during a person's decision: {blocked:?}"
                );
                cold.deps
                    .store
                    .set_task_needs_you(id, None, cold.now())
                    .await?;
            }
            let news = cold.watch_prs(&bounds).await;
            let repeated = cold.watch_prs(&bounds).await;
            cold.stop_task_now(id).await?;
            assert_eq!(news.len(), 1, "{news:?}");
            assert!(
                news[0].1.contains("cannot read this task's pull request")
                    && !news[0].1.contains("Hands off"),
                "{news:?}"
            );
            assert!(repeated.is_empty(), "{repeated:?}");
            return Ok(());
        }
        if matches!(case, ColdCandidate::FreeArmTaken) {
            let (observed, finish_read) =
                super::pause_watch_or_queue(super::Point::LivePrFactsRead, id);
            let (deciding, finish_decision) =
                super::pause_watch_or_queue(super::Point::FreeArmTaken, id);
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
        let path = data.join(yulius_app::merge_train::MERGE_TRAINS_FILE);
        let mut train = json!({sandbox.as_str(): {"train":{"run":"candidate-red","commit":"merge","base":"base","base_tree":"tree","members":[{"task":id.as_str(),"pr":1481,"head":head,"armed_at":30,"tree":"candidate-tree"}]}}});
        if matches!(
            case,
            ColdCandidate::TrainFailsDuringRead | ColdCandidate::OrdinaryFailsDuringRead
        ) {
            train[sandbox.as_str()]["train"]["members"].as_array_mut().ok_or("train members")?.push(
                json!({"task":"second-owner","pr":99,"head":"other","armed_at":29,"tree":"other-tree"})
            );
        }
        if matches!(case, ColdCandidate::TrainFailsDuringRead) {
            std::fs::write(&path, serde_json::to_vec(&train)?)?;
            let (entered, release) = super::pause_watch_or_queue(super::Point::LivePrFactsRead, id);
            let watcher = Arc::clone(&cold);
            let pass = tokio::spawn(async move { watcher.watch_prs(&bounds).await });
            tokio::time::timeout(Duration::from_secs(10), entered.notified()).await?;
            assert!(
                cold.deps
                    .store
                    .transition_ci_run(
                        "candidate-red",
                        &[RunState::Running],
                        RunState::Failed,
                        &yulius_app::ci::CiRunUpdate {
                            ended_at: Some(cold.now()),
                            ..Default::default()
                        }
                    )
                    .await?
            );
            release.notify_one();
            let obsolete = pass.await?;
            let news = cold.watch_prs(&bounds).await;
            let repeated = cold.watch_prs(&bounds).await;
            cold.stop_task_now(id).await?;
            assert!(obsolete.is_empty(), "outdated train status: {obsolete:?}");
            assert_eq!(news.len(), 1, "{news:?}");
            assert!(
                news[0].1.contains(
                    "Merge candidate #1481, #99 (run candidate-red), which holds it, failed"
                ),
                "{news:?}"
            );
            assert!(repeated.is_empty(), "{repeated:?}");
            return Ok(());
        }
        let heard = matches!(case, ColdCandidate::HeardOrdinaryFailure);
        let (mut red, job) =
            crate::pr_watch::run(&sandbox, "ordinary-red", head, RunState::Failed, None, 31);
        red.reference = CiRef::PullRequest(
            if matches!(
                case,
                ColdCandidate::SharedOrdinaryFailure | ColdCandidate::SharedWhileRunning
            ) {
                99
            } else {
                1481
            },
        );
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
        std::fs::write(path, serde_json::to_vec(&train)?)?;
        if matches!(case, ColdCandidate::HeldHumanDecision) {
            let (observed, finish_read) =
                super::pause_watch_or_queue(super::Point::LivePrFactsRead, id);
            let app = Arc::clone(&cold);
            let pass = tokio::spawn(async move { app.watch_prs(&bounds).await });
            tokio::time::timeout(Duration::from_secs(10), observed.notified()).await?;
            ask(&cold, id).await?;
            finish_read.notify_one();
            let interrupted = pass.await?;
            assert!(interrupted.is_empty(), "person deciding: {interrupted:?}");
            assert!(cold.watch_prs(&bounds).await.is_empty());
            cold.deps
                .store
                .set_task_needs_you(id, None, cold.now())
                .await?;
        }
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
                news[0].1.contains("ordinary-red")
                    && !news[0].1.contains("Do not re-arm")
                    && !news[0].1.contains("Hands off"),
                "{news:?}"
            );
        }
        assert!(repeated.is_empty(), "{repeated:?}");
        return Ok(());
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
            let due = WatchBounds {
                check_in_after_ms: 0,
                stall_quiet_ms: 0,
                stall_after_ms: 0,
            };
            let mut obsolete_news = Vec::new();
            for (run_id, verdict) in [
                ("gone", None),
                ("cancelled", Some(RunState::Cancelled)),
                ("errored", Some(RunState::Error)),
            ] {
                if let Some(verdict) = verdict {
                    let (mut run, job) =
                        crate::pr_watch::run(&task.sandbox_id, run_id, "merge", verdict, None, 31);
                    run.reference = CiRef::Train(vec![1481]);
                    cold.deps.store.insert_ci_run(&run, &[job]).await?;
                }
                let mut ending = current.clone();
                ending[task.sandbox_id.as_str()]["train"]["run"] = json!(run_id);
                std::fs::write(&path, serde_json::to_vec(&ending)?)?;
                obsolete_news.extend(cold.watch_prs(&due).await);
            }
            assert!(
                obsolete_news.is_empty(),
                "train due to dissolve: {obsolete_news:?}"
            );
            let (mut run, job) = crate::pr_watch::run(
                &task.sandbox_id,
                "later-train",
                "merge",
                RunState::Passed,
                None,
                32,
            );
            run.reference = CiRef::Train(vec![1481]);
            cold.deps.store.insert_ci_run(&run, &[job]).await?;
            let mut moving = current.clone();
            let train = &mut moving[task.sandbox_id.as_str()]["train"];
            train["merging"] = json!(true);
            train["members"][0]["head"] = json!("original-admission-head");
            // A restart can see the saved moved arm before the train checkpoint.
            for merging in [false, true] {
                moving[task.sandbox_id.as_str()]["train"]["merging"] = json!(merging);
                std::fs::write(&path, serde_json::to_vec(&moving)?)?;
                let premature = cold.watch_prs(&due).await;
                assert!(premature.is_empty(), "unverified moved head: {premature:?}");
            }
            std::fs::write(&path, b"[")?;
            let unreadable = cold.watch_prs(&due).await;
            assert!(
                unreadable.is_empty(),
                "unreadable holding train: {unreadable:?}"
            );
            moving[task.sandbox_id.as_str()]["train"]["members"][0]["verified_head"] = json!(head);
            std::fs::write(&path, serde_json::to_vec(&moving)?)?;
        }
        let seen = cold.watch_prs(&bounds).await;
        assert!(
            seen.iter()
                .all(|(_, text)| !text.contains("earlier-bisection") && !text.contains("292.1")),
            "resolved bisection after rearm: {seen:?}"
        );
        if matches!(case, ColdCandidate::RearmedInTrain) {
            assert_eq!(seen.len(), 1, "active replacement train: {seen:?}");
            assert!(
                seen[0].1.contains("#1481 is number 1 of 1")
                    && seen[0].1.contains("combined result passed")
                    && seen[0].1.contains("Hands off"),
                "{seen:?}"
            );
            let task = cold.deps.store.task(id).await?.ok_or("owner")?;
            let path = data.join(yulius_app::merge_train::MERGE_TRAINS_FILE);
            let mut current: serde_json::Value = serde_json::from_slice(&std::fs::read(&path)?)?;
            current[task.sandbox_id.as_str()]["train"]["run"] = json!("earlier-bisection");
            std::fs::write(&path, serde_json::to_vec(&current)?)?;
            let held = cold.watch_prs(&bounds).await;
            assert!(
                held.is_empty(),
                "the queue is verifying its sole member: {held:?}"
            );
            current[task.sandbox_id.as_str()]["train"]["run"] = json!("gone");
            for (queued, (run_id, registry)) in [
                ("ordinary-red", serde_json::to_vec(&current)?),
                ("ordinary-red-rerun", b"[".to_vec()),
            ]
            .into_iter()
            .enumerate()
            {
                std::fs::write(&path, registry)?;
                let (mut run, job) = crate::pr_watch::run(
                    &task.sandbox_id,
                    run_id,
                    head,
                    RunState::Failed,
                    None,
                    33 + i64::try_from(queued)?,
                );
                run.reference = CiRef::PullRequest(1481);
                cold.deps.store.insert_ci_run(&run, &[job]).await?;
                let news = cold.watch_prs(&bounds).await;
                assert_eq!(
                    news.len(),
                    1,
                    "ordinary failure with unavailable candidate: {news:?}"
                );
                assert!(
                    news[0].1.contains(run_id) && !news[0].1.contains("Hands off"),
                    "{news:?}"
                );
                assert!(cold.watch_prs(&bounds).await.is_empty());
            }
        }
        cold.stop_task_now(id).await?;
        return Ok(());
    }
    Err("the cold candidate case was not exercised".into())
}
