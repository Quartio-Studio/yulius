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

#[test]
fn after_all_tasks_includes_the_owner_of_the_current_failed_candidate() -> TestResult {
    with_idle_candidate(async |_, app, _, id, _| {
        let owner = app.deps.store.task(&id).await?.ok_or("owner")?;
        app.deps
            .store
            .set_activity(&id, Some(yulius_domain::task::Activity::Done), app.now())
            .await?;
        let caller = super::superagent(&owner.sandbox_id, &owner.owner, "admin");
        let created = app.tool_task_create(&caller, &json!({"prompt":"When current tasks finish", "harness":"claude", "after_all_tasks":true})).await?;
        let rule = created["task"]["start_rule"].as_str().unwrap_or("");
        assert!(
            rule.split(" and ")
                .any(|atom| atom == format!("#{} done", owner.number)),
            "{created}"
        );
        Ok(())
    })
}

#[test]
fn archive_keeps_the_owner_when_a_replacement_arm_arrives_at_close() -> TestResult {
    with_idle_candidate(async |data, app, remote, id, _| {
        let owner = app.deps.store.task(&id).await?.ok_or("owner")?;
        app.deps
            .store
            .set_activity(&id, Some(yulius_domain::task::Activity::Done), app.now())
            .await?;
        remote.remote.lock().map_err(|_| "origin")?.take();
        remote.pulls.lock().map_err(|_| "PR")?[0].base_sha = "advanced-base".into();
        let ctx = yulius_app::auth::AuthContext {
            user: app.deps.store.user(&owner.owner).await?.ok_or("owner")?,
            csrf_token: None,
            session_hash: None,
            token: None,
        };
        // The watch observes the moved base, so only the arm below can keep the session.
        let told = app.watch_prs(&WatchBounds::default()).await;
        let file = data.join(yulius_app::agent_wait::ARMED_MERGES_FILE);
        let arm = replacement_request(&owner)?;
        let reached = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let seen = Arc::clone(&reached);
        yulius_app::race::on(
            super::Point::CloseDecided,
            Path::new(id.as_str()),
            move || {
                assert!(std::fs::write(&file, &arm).is_ok());
                seen.store(true, std::sync::atomic::Ordering::SeqCst);
            },
        );
        let closed = app.archive_task(&ctx, id.as_str(), true).await;
        assert!(
            reached.load(std::sync::atomic::Ordering::SeqCst),
            "the archive decided to close: {closed:?} {told:?}"
        );
        assert!(
            matches!(closed, Err(yulius_app::error::AppError::Conflict(_))),
            "{closed:?}"
        );
        assert!(
            app.deps
                .store
                .task(&id)
                .await?
                .ok_or("owner")?
                .archived_at
                .is_none()
        );
        assert!(app.deps.store.live_attempt(&id).await?.is_some());
        Ok(())
    })
}

/// Delete closes a Done session first: an arm arriving at that close keeps the owner, its
/// session and its records (SPEC-621 FIN-003).
#[test]
fn delete_keeps_the_owner_when_a_replacement_arm_arrives_at_close() -> TestResult {
    with_idle_candidate(async |data, app, remote, id, _| {
        let owner = app.deps.store.task(&id).await?.ok_or("owner")?;
        app.deps
            .store
            .set_activity(&id, Some(yulius_domain::task::Activity::Done), app.now())
            .await?;
        remote.remote.lock().map_err(|_| "origin")?.take();
        remote.pulls.lock().map_err(|_| "PR")?[0].base_sha = "advanced-base".into();
        let ctx = yulius_app::auth::AuthContext {
            user: app.deps.store.user(&owner.owner).await?.ok_or("owner")?,
            csrf_token: None,
            session_hash: None,
            token: None,
        };
        let told = app.watch_prs(&WatchBounds::default()).await;
        let file = data.join(yulius_app::agent_wait::ARMED_MERGES_FILE);
        let arm = replacement_request(&owner)?;
        let reached = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let seen = Arc::clone(&reached);
        yulius_app::race::on(
            super::Point::CloseDecided,
            Path::new(id.as_str()),
            move || {
                assert!(std::fs::write(&file, &arm).is_ok());
                seen.store(true, std::sync::atomic::Ordering::SeqCst);
            },
        );
        let deleted = app.delete_task(&ctx, id.as_str()).await;
        assert!(
            reached.load(std::sync::atomic::Ordering::SeqCst),
            "the delete decided to close: {deleted:?} {told:?}"
        );
        assert!(
            matches!(&deleted, Err(yulius_app::error::AppError::Conflict(why)) if why.contains("being verified")),
            "{deleted:?}"
        );
        assert!(app.deps.store.task(&id).await?.is_some());
        assert!(app.deps.store.live_attempt(&id).await?.is_some());
        Ok(())
    })
}

/// Removing the subscription of a live owner whose integration failure still applies moves
/// the session instead of closing it as Done (SPEC-621 FIN-003).
#[test]
fn removing_the_subscription_moves_an_unverified_owner_instead_of_closing_it() -> TestResult {
    use yulius_domain::task::{Activity, TaskStatus};
    with_idle_candidate(async |_, app, _, id, _| {
        let owner = app.deps.store.task(&id).await?.ok_or("owner")?;
        app.deps
            .store
            .set_activity(&id, Some(Activity::Done), app.now())
            .await?;
        let account = app
            .deps
            .store
            .live_attempt(&id)
            .await?
            .and_then(|run| run.account_id)
            .ok_or("the owner's subscription")?;
        let ctx = yulius_app::auth::AuthContext {
            user: app.deps.store.user(&owner.owner).await?.ok_or("owner")?,
            csrf_token: None,
            session_hash: None,
            token: None,
        };
        let reached = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let seen = Arc::clone(&reached);
        yulius_app::race::on(
            super::Point::CloseDecided,
            Path::new(id.as_str()),
            move || seen.store(true, std::sync::atomic::Ordering::SeqCst),
        );
        app.delete_account(&ctx, account.as_str()).await?;
        assert!(
            reached.load(std::sync::atomic::Ordering::SeqCst),
            "the removal tried to close the owner as Done"
        );
        let status = super::status_of(app, &id).await.0;
        assert_ne!(status, Some(TaskStatus::Closed), "{status:?}");
        Ok(())
    })
}

#[test]
fn a_replacement_merge_is_refused_while_the_reader_settles_a_close() -> TestResult {
    with_idle_candidate(async |_, app, remote, id, head| {
        let ctx = pending_owner_merge(app, remote, &id, &head).await?;
        let (taken, release) = super::pause_watch_or_queue(super::Point::RunEndTaken, &id);
        app.close_task_now(&id).await?;
        tokio::time::timeout(Duration::from_secs(15), taken.notified()).await?;
        let request = app.merge_for_person(&ctx, id.as_str()).await;
        release.notify_one();
        assert!(
            matches!(&request, Err(yulius_app::error::AppError::Conflict(_))),
            "merge while the reader owns Close: {request:?}"
        );
        Ok(())
    })
}

/// The list, Home and the task's own views show a live owner whose achieved goal still has a
/// recorded integration failure as waiting on Yulius, not Done, until the watch clears it
/// (SPEC-621 FIN-004).
#[test]
fn a_live_owner_with_a_pending_candidate_is_shown_waiting_not_done() -> TestResult {
    use yulius_domain::task::{Activity, TaskStatus};
    with_idle_candidate(async |data, app, remote, id, _| {
        app.deps
            .store
            .set_activity(&id, Some(Activity::Done), app.now())
            .await?;
        let owner = app.deps.store.task(&id).await?.ok_or("owner")?;
        let summary = app.task_summary(&owner).await?;
        assert!(summary.candidate_pending, "{summary:?}");
        let value = serde_json::to_value(&summary)?;
        assert_eq!(value["candidate_pending"], true, "{value}");
        let caller = super::superagent(&owner.sandbox_id, &owner.owner, "admin");
        let brief = app
            .tool_task_get(&caller, &json!({"task_id": id.as_str()}))
            .await?;
        assert_eq!(brief["shows"], "working", "{brief}");
        assert!(
            brief["waits_for"]
                .to_string()
                .contains("failed integration candidate"),
            "{brief}"
        );
        // A parked owner has no session to settle it in: the field is the live run's only.
        let now = app.now();
        app.deps
            .store
            .transition_task(&id, None, TaskStatus::Parked, Some(Activity::Done), now)
            .await?;
        let parked = app.deps.store.task(&id).await?.ok_or("owner")?;
        assert!(!app.task_summary(&parked).await?.candidate_pending);
        app.deps
            .store
            .transition_task(&id, None, TaskStatus::Running, Some(Activity::Done), now)
            .await?;
        remote.remote.lock().map_err(|_| "origin")?.take();
        remote.pulls.lock().map_err(|_| "PR")?[0].base_sha = "advanced-base".into();
        let mut events = app.events.subscribe();
        let told = app.watch_prs(&WatchBounds::default()).await;
        // The sidebar and an open terminal hear the cleared failure from the watch itself.
        let mut published = None;
        while let Ok(envelope) = events.try_recv() {
            if let yulius_contracts::events::UiEvent::TaskUpdated { task } = envelope.event
                && task.id == id.as_str()
            {
                published = Some(task.candidate_pending);
            }
        }
        assert_eq!(published, Some(false), "{told:?}");
        // The pass told the owner about its green head, and the fake agent answers with a turn
        // that the session watcher reads on its own clock: once it was read, the owner is at
        // rest again, and what follows hears the watch alone (SPEC-640 FLK-002).
        if told.iter().any(|(task, _)| task == &id) {
            super::wait_for(
                "the owner's answer to the notice",
                Duration::from_secs(30),
                || {
                    let app = app.clone();
                    let id = id.clone();
                    async move {
                        app.deps
                            .store
                            .task(&id)
                            .await
                            .ok()
                            .flatten()
                            .and_then(|t| t.activity)
                            == Some(Activity::Working)
                    }
                },
            )
            .await?;
            let rounds = app.runtime(&id).transcript_rounds();
            super::wait_for(
                "the answer read to its end",
                Duration::from_secs(30),
                || {
                    let app = app.clone();
                    let id = id.clone();
                    async move { app.runtime(&id).transcript_rounds() >= rounds + 2 }
                },
            )
            .await?;
            // A shell-probe round of the watcher can fall while the owner works: it forgets the
            // count, and the next one publishes it again. Probe here so that always happens.
            app.probe_idle_shells(&id).await;
            app.deps
                .store
                .set_activity(&id, Some(Activity::Done), app.now())
                .await?;
        }
        let summary = app.task_summary(&owner).await?;
        assert!(!summary.candidate_pending, "{summary:?} {told:?}");
        let value = serde_json::to_value(&summary)?;
        assert!(value.get("candidate_pending").is_none(), "{value}");
        let brief = app
            .tool_task_get(&caller, &json!({"task_id": id.as_str()}))
            .await?;
        assert_eq!(brief["shows"], "done", "{brief}");
        // The next pass re-reads the pull request; until it answers, the views keep its last
        // answer, and a read that fails keeps it too.
        let (reading, release) =
            super::pause_watch_or_queue(super::Point::CandidateObservationRead, &id);
        let pass = {
            let app = app.clone();
            tokio::spawn(async move { app.watch_prs(&WatchBounds::default()).await })
        };
        tokio::time::timeout(Duration::from_secs(10), reading.notified()).await?;
        assert!(!app.task_summary(&owner).await?.candidate_pending);
        release.notify_one();
        pass.await?;
        let pulls = std::mem::take(&mut *remote.pulls.lock().map_err(|_| "PR")?);
        let failed = app.watch_prs(&WatchBounds::default()).await;
        assert!(
            !app.task_summary(&owner).await?.candidate_pending,
            "{failed:?}"
        );
        *remote.pulls.lock().map_err(|_| "PR")? = pulls;
        // The same answer again is no news.
        let task_updates = |events: &mut tokio::sync::broadcast::Receiver<_>| {
            let mut seen = Vec::new();
            while let Ok(envelope) = events.try_recv() {
                let envelope: yulius_app::events::Envelope = envelope;
                if let yulius_contracts::events::UiEvent::TaskUpdated { task } = envelope.event
                    && task.id == id.as_str()
                {
                    seen.push(task.candidate_pending);
                }
            }
            seen
        };
        assert_eq!(
            app.task_summary(&owner).await?.background_shells,
            Some(0),
            "a forgotten shell count is published again on the watcher's clock"
        );
        let mut events = app.events.subscribe();
        let again = app.watch_prs(&WatchBounds::default()).await;
        assert_eq!(task_updates(&mut events), Vec::<bool>::new(), "{again:?}");
        // A second decision, tested on the moved base, fails too: the cleared answer was the
        // first decision's, so the owner waits again at once and the watch says so.
        let span = app
            .deps
            .store
            .latest_merge_wait(&id)
            .await?
            .ok_or("candidate")?;
        let mut decision: serde_json::Value =
            serde_json::from_str(span.detail.as_deref().ok_or("receipt")?)?;
        decision["singleton"]["base"] = json!("advanced-base");
        let saved = decision.to_string();
        let changed = rusqlite::Connection::open(data.join("yulius.db"))?.execute(
            "UPDATE spans SET detail=?1 WHERE key=?2",
            [saved.as_str(), span.key.as_str()],
        )?;
        assert_eq!(changed, 1);
        let summary = app.task_summary(&owner).await?;
        assert!(summary.candidate_pending, "{summary:?}");
        assert_eq!(summary.background_shells, Some(0), "{summary:?}");
        let mut events = app.events.subscribe();
        // The session's own watcher publishes on its own clock (its shell probe, its
        // transcript): a whole round of it while the pass reads the pull request says nothing
        // new, so the pass's one publish is all the views hear (SPEC-640 FLK-002).
        let (reading, release) =
            super::pause_watch_or_queue(super::Point::CandidateObservationRead, &id);
        let pass = {
            let app = app.clone();
            tokio::spawn(async move { app.watch_prs(&WatchBounds::default()).await })
        };
        tokio::time::timeout(Duration::from_secs(10), reading.notified()).await?;
        let rounds = app.runtime(&id).transcript_rounds();
        super::wait_for(
            "a watcher round during the pass",
            Duration::from_secs(30),
            || {
                let app = app.clone();
                let id = id.clone();
                async move { app.runtime(&id).transcript_rounds() >= rounds + 2 }
            },
        )
        .await?;
        release.notify_one();
        let second = pass.await?;
        // The pass ends by telling the owner about the failure: the owner's answer may already
        // be heard after the watch's one publish, never before it.
        let seen = task_updates(&mut events);
        assert_eq!(seen.first(), Some(&true), "{seen:?} {second:?}");
        assert!(!seen[1..].contains(&true), "{seen:?} {second:?}");
        Ok(())
    })
}

/// A task chained after a live owner whose achieved goal has a recorded integration failure
/// starts only once the watch has observed that failure no longer applies (SPEC-621 FIN-003).
#[test]
fn a_chained_successor_waits_until_the_live_owners_failure_is_cleared() -> TestResult {
    use yulius_domain::task::{Activity, TaskStatus};
    with_idle_candidate(async |_, app, remote, id, _| {
        let owner = app.deps.store.task(&id).await?.ok_or("owner")?;
        app.deps
            .store
            .set_activity(&id, Some(Activity::Done), app.now())
            .await?;
        let peer = candidate_peer(app, &owner, TaskStatus::Chained).await?;
        app.deps
            .store
            .insert_chain(&yulius_app::intake::ChainRecord {
                task_id: peer.clone(),
                after_task_id: Some(id.clone()),
                after_number: owner.number,
                blocked_reason: None,
                created_at: app.now(),
                rule: None,
            })
            .await?;
        app.advance_chains(&id).await;
        assert_eq!(
            super::status_of(app, &peer).await.0,
            Some(TaskStatus::Chained),
            "the failure still applies"
        );
        remote.remote.lock().map_err(|_| "origin")?.take();
        remote.pulls.lock().map_err(|_| "PR")?[0].base_sha = "advanced-base".into();
        let told = app.watch_prs(&WatchBounds::default()).await;
        app.advance_chains(&id).await;
        assert_ne!(
            super::status_of(app, &peer).await.0,
            Some(TaskStatus::Chained),
            "{told:?}"
        );
        app.stop_task_now(&peer).await?;
        Ok(())
    })
}

#[test]
fn a_compound_done_rule_rechecks_its_first_owner_after_reading_the_second() -> TestResult {
    use yulius_domain::task::{Activity, TaskStatus};
    with_idle_candidate(async |data, app, remote, id, _| {
        let owner = app.deps.store.task(&id).await?.ok_or("owner")?;
        app.deps
            .store
            .set_activity(&id, Some(Activity::Done), app.now())
            .await?;
        remote.remote.lock().map_err(|_| "origin")?.take();
        remote.pulls.lock().map_err(|_| "PR")?[0].base_sha = "advanced-base".into();
        let mut second = owner.clone();
        second.id = TaskId::parse("second-rule-owner")?;
        second.branch = None;
        second.worktree_path = None;
        second.activity = Some(Activity::Done);
        let second = app.deps.store.insert_task(&second).await?;
        let peer = candidate_peer(app, &owner, TaskStatus::Chained).await?;
        app.deps
            .store
            .insert_chain(&yulius_app::intake::ChainRecord {
                task_id: peer.clone(),
                after_task_id: Some(id.clone()),
                after_number: owner.number,
                blocked_reason: None,
                created_at: app.now(),
                rule: Some(format!(
                    "#{} done and #{} done",
                    owner.number, second.number
                )),
            })
            .await?;
        // The watch observes the moved base, so only the arm below can hold the successor.
        let told = app.watch_prs(&WatchBounds::default()).await;
        let file = data.join(yulius_app::agent_wait::ARMED_MERGES_FILE);
        let arm = replacement_request(&owner)?;
        yulius_app::race::on(
            super::Point::RuleOwnerRead,
            Path::new(second.id.as_str()),
            move || {
                assert!(std::fs::write(&file, &arm).is_ok());
            },
        );
        app.advance_rule_chains(&owner.sandbox_id).await;
        assert_eq!(
            super::status_of(app, &peer).await.0,
            Some(TaskStatus::Chained),
            "{told:?}"
        );
        std::fs::write(data.join(yulius_app::agent_wait::ARMED_MERGES_FILE), b"[]")?;
        app.advance_rule_chains(&owner.sandbox_id).await;
        assert_ne!(
            super::status_of(app, &peer).await.0,
            Some(TaskStatus::Chained),
            "{told:?}"
        );
        app.stop_task_now(&peer).await?;
        Ok(())
    })
}

#[test]
fn final_notice_verification_keeps_the_session_when_the_candidate_applies_again() -> TestResult {
    use yulius_domain::task::Activity;
    with_idle_candidate(async |data, app, remote, id, _| {
        let owner = app.deps.store.task(&id).await?.ok_or("owner")?;
        app.load_limits([(yulius_domain::limits::Limit::Sessions, 1)].into())
            .await;
        app.deps
            .store
            .set_activity(&id, Some(Activity::Done), app.now())
            .await?;
        let peer = queued_candidate_peer(app, &owner).await?;
        let queued = app.deps.store.task(&peer).await?.ok_or("queued")?;
        remote.remote.lock().map_err(|_| "origin")?.take();
        let base = crate::pr_watch::git(&data.join("origin.git"), &["rev-parse", "dev"])?;
        remote.pulls.lock().map_err(|_| "pulls")?[0].base_sha = "moved-base".into();
        let (checking, release) =
            super::pause_watch_or_queue(super::Point::ArmedNoticePublicationRead, &id);
        let pass = {
            let app = app.clone();
            tokio::spawn(async move { app.watch_prs(&WatchBounds::default()).await })
        };
        tokio::time::timeout(Duration::from_secs(10), checking.notified()).await?;
        remote.pulls.lock().map_err(|_| "pulls")?[0].base_sha = base;
        release.notify_one();
        assert!(
            pass.await?.is_empty(),
            "the earlier green notice was obsolete"
        );
        assert_eq!(
            app.task_summary(&queued).await?.queue_reason.as_deref(),
            Some("Waiting for a free session: 1 of 1 in use")
        );
        let retry = app.watch_prs(&WatchBounds::default()).await;
        assert_eq!(retry.len(), 1, "{retry:?}");
        assert_eq!(retry[0].0, id);
        assert!(retry[0].1.contains("292.1 ms"));
        Ok(())
    })
}

#[test]
fn a_candidate_refresh_waits_for_the_launch_to_reserve_its_verified_slot() -> TestResult {
    use std::sync::atomic::Ordering;
    use yulius_app::supervisor::RunStart;
    use yulius_domain::task::Activity;
    with_idle_candidate(async |data, app, remote, id, _| {
        let owner = app.deps.store.task(&id).await?.ok_or("owner")?;
        app.load_limits([(yulius_domain::limits::Limit::Sessions, 1)].into())
            .await;
        app.deps
            .store
            .set_activity(&id, Some(Activity::Done), app.now())
            .await?;
        let peer = queued_candidate_peer(app, &owner).await?;
        remote.remote.lock().map_err(|_| "origin")?.take();
        let base = crate::pr_watch::git(&data.join("origin.git"), &["rev-parse", "dev"])?;
        {
            let mut pulls = remote.pulls.lock().map_err(|_| "pulls")?;
            pulls[0].state = "closed".into();
            pulls[0].base_sha = base;
        }
        assert!(app.watch_prs(&WatchBounds::default()).await.is_empty());
        let (cleared, release) =
            super::pause_watch_or_queue(super::Point::CandidateCapacityCleared, &peer);
        let launch = {
            let app = app.clone();
            let peer = peer.clone();
            tokio::spawn(async move {
                app.start_run(&peer, RunStart::Fresh { prompt: None }, None)
                    .await
            })
        };
        tokio::time::timeout(Duration::from_secs(10), cleared.notified()).await?;
        remote.pulls.lock().map_err(|_| "pulls")?[0].state = "open".into();
        let before = remote.pull_reads.load(Ordering::SeqCst);
        let pass = tokio::time::timeout(
            Duration::from_secs(5),
            app.watch_prs(&WatchBounds::default()),
        )
        .await;
        let during = remote.pull_reads.load(Ordering::SeqCst);
        release.notify_one();
        launch.await??;
        app.stop_task_now(&peer).await?;
        assert_eq!(
            during, before,
            "candidate verification overtook the slot reservation"
        );
        assert!(pass?.is_empty());
        // Stopping the peer wakes the queue, whose capacity check briefly holds the owner's
        // delivery guard; a pass meeting it defers the hosted read to the next pass.
        let retry = tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                let retry = app.watch_prs(&WatchBounds::default()).await;
                if remote.pull_reads.load(Ordering::SeqCst) > during {
                    return retry;
                }
                assert!(retry.is_empty(), "a deferred pass typed {retry:?}");
                tokio::task::yield_now().await;
            }
        })
        .await?;
        assert_eq!(retry.len(), 1, "{retry:?}");
        assert_eq!(retry[0].0, id);
        assert!(
            retry[0].1.contains("shard 2") && retry[0].1.contains("292.1 ms"),
            "{retry:?}"
        );
        assert!(app.watch_prs(&WatchBounds::default()).await.is_empty());
        Ok(())
    })
}

#[test]
fn a_rearm_on_an_earlier_capacity_row_keeps_a_queued_launch_in_line() -> TestResult {
    use yulius_domain::task::Activity;
    with_idle_candidate(async |data, app, remote, id, head| {
        let _ = pending_owner_merge(app, remote, &id, &head).await?;
        let owner = app.deps.store.task(&id).await?.ok_or("owner")?;
        let second = super::new_task(
            app,
            &owner.sandbox_id,
            &owner.owner,
            "Second capacity owner",
            yulius_app::model::TaskMode::Prompt,
        )
        .await?;
        super::wait_for("second owner idle", Duration::from_secs(20), || async {
            super::status_of(app, &second).await.1 == Some(Activity::Waiting)
        })
        .await?;
        app.deps
            .store
            .set_activity(&id, Some(Activity::Done), app.now())
            .await?;
        app.deps
            .store
            .set_activity(&second, Some(Activity::Done), app.now())
            .await?;
        app.load_limits([(yulius_domain::limits::Limit::Sessions, 1)].into())
            .await;
        let peer = queued_candidate_peer(app, &owner).await?;
        let file = data.join(yulius_app::agent_wait::ARMED_MERGES_FILE);
        let arm = replacement_request(&owner)?;
        yulius_app::race::on(
            super::Point::CandidateCapacityRead,
            Path::new(second.as_str()),
            move || {
                assert!(std::fs::write(&file, &arm).is_ok());
            },
        );
        app.start_run(
            &peer,
            yulius_app::supervisor::RunStart::Fresh { prompt: None },
            None,
        )
        .await?;
        let state = super::status_of(app, &peer).await.0;
        let premature = app.deps.store.live_attempt(&peer).await?.is_some();
        app.stop_task_now(&second).await?;
        app.stop_task_now(&peer).await?;
        assert_eq!(
            (state, premature),
            (Some(yulius_domain::task::TaskStatus::Queued), false)
        );
        Ok(())
    })
}

#[test]
fn a_replacement_arm_keeps_the_session_occupied_during_capacity_accounting() -> TestResult {
    with_idle_candidate(async |data, app, _, id, _| {
        use yulius_domain::task::Activity;
        let owner = &app.deps.store.task(&id).await?.ok_or("owner")?;
        app.load_limits([(yulius_domain::limits::Limit::Sessions, 1)].into())
            .await;
        app.deps
            .store
            .set_activity(&owner.id, Some(Activity::Done), app.now())
            .await?;
        let peer = queued_candidate_peer(app, owner).await?;
        let queued = app.deps.store.task(&peer).await?.ok_or("queued peer")?;
        let file = data.join(yulius_app::agent_wait::ARMED_MERGES_FILE);
        let arm = replacement_request(owner)?;
        yulius_app::race::on(
            super::Point::CandidateCapacityRead,
            Path::new(owner.id.as_str()),
            move || {
                assert!(std::fs::write(&file, &arm).is_ok());
            },
        );
        assert_eq!(
            app.task_summary(&queued).await?.queue_reason.as_deref(),
            Some("Waiting for a free session: 1 of 1 in use")
        );
        Ok(())
    })
}

async fn queued_candidate_peer(
    app: &yulius_app::context::App,
    task: &yulius_app::model::TaskRecord,
) -> Result<TaskId, Error> {
    candidate_peer(app, task, yulius_domain::task::TaskStatus::Queued).await
}

async fn candidate_peer(
    app: &yulius_app::context::App,
    task: &yulius_app::model::TaskRecord,
    status: yulius_domain::task::TaskStatus,
) -> Result<TaskId, Error> {
    let mut peer = task.clone();
    peer.id = TaskId::parse("candidate-peer")?;
    peer.number = 99;
    peer.title = "Second task".into();
    peer.status = status;
    peer.activity = None;
    peer.branch = None;
    peer.worktree_path = None;
    Ok(app.deps.store.insert_task(&peer).await?.id)
}

fn replacement_request(owner: &yulius_app::model::TaskRecord) -> Result<Vec<u8>, Error> {
    let head = crate::pr_watch::git(
        Path::new(owner.worktree_path.as_deref().ok_or("worktree")?),
        &["rev-parse", "HEAD"],
    )?;
    Ok(serde_json::to_vec(
        &json!([{"task":owner.id.as_str(),"head":head,"summary":"replacement"}]),
    )?)
}

#[test]
fn the_watch_releases_the_achieved_owners_slot_after_external_pr_closure() -> TestResult {
    with_idle_candidate(async |data, app, remote, id, _| {
        use yulius_domain::task::Activity;
        let task = &app.deps.store.task(&id).await?.ok_or("owner")?;
        app.load_limits([(yulius_domain::limits::Limit::Sessions, 1)].into())
            .await;
        app.deps
            .store
            .set_activity(&task.id, Some(Activity::Done), app.now())
            .await?;
        let peer = queued_candidate_peer(app, task).await?;
        let queued = app.deps.store.task(&peer).await?.ok_or("queued peer")?;
        assert_eq!(
            app.task_summary(&queued).await?.queue_reason.as_deref(),
            Some("Waiting for a free session: 1 of 1 in use"),
        );
        remote.remote.lock().map_err(|_| "remote")?.take();
        let head = crate::pr_watch::git(
            Path::new(task.worktree_path.as_deref().ok_or("tree")?),
            &["rev-parse", "HEAD"],
        )?;
        let base = crate::pr_watch::git(&data.join("origin.git"), &["rev-parse", "dev"])?;
        {
            let mut pulls = remote.pulls.lock().map_err(|_| "PR")?;
            pulls[0].state = "closed".into();
            pulls[0].base_sha = base;
        }
        // A later ordinary run on the old head cannot start the obsolete
        // integration decision's 15-minute wait when the PR has closed.
        let (mut rerun, job) = crate::pr_watch::run(
            &task.sandbox_id,
            "closed-rerun",
            &head,
            RunState::Running,
            None,
            app.now().0,
        );
        rerun.reference = CiRef::PullRequest(1481);
        app.deps.store.insert_ci_run(&rerun, &[job]).await?;
        assert!(app.watch_prs(&WatchBounds::default()).await.is_empty());
        let receipts: serde_json::Value = serde_json::from_slice(&std::fs::read(
            data.join(yulius_app::pr_watch::PR_NOTICES_FILE),
        )?)?;
        assert!(
            receipts[task.id.as_str()]["singleton_wait"].is_null(),
            "{receipts}"
        );
        assert_eq!(
            app.task_summary(&queued).await?.queue_reason.as_deref(),
            Some("Starting: a session is free"),
        );
        let closed = remote.pulls.lock().map_err(|_| "PR")?.remove(0);
        assert!(app.watch_prs(&WatchBounds::default()).await.is_empty());
        assert_eq!(
            app.task_summary(&queued).await?.queue_reason.as_deref(),
            Some("Waiting for a free session: 1 of 1 in use"),
            "an unavailable PR invalidates the earlier observation",
        );
        remote.pulls.lock().map_err(|_| "PR")?.push(closed);
        assert!(app.watch_prs(&WatchBounds::default()).await.is_empty());
        app.start_run(
            &peer,
            yulius_app::supervisor::RunStart::Fresh { prompt: None },
            None,
        )
        .await?;
        let run = app.deps.store.live_attempt(&peer).await?;
        app.stop_task_now(&peer).await?;
        assert!(
            run.is_some(),
            "the closed PR still consumed the only session slot"
        );
        Ok(())
    })
}

#[test]
fn an_unreadable_open_delivery_holds_the_original_owner_slot() -> TestResult {
    with_idle_candidate(async |data, app, _, id, _| {
        use yulius_domain::task::Activity;
        let task = &app.deps.store.task(&id).await?.ok_or("owner")?;
        app.load_limits([(yulius_domain::limits::Limit::Sessions, 1)].into())
            .await;
        app.deps
            .store
            .set_activity(&task.id, Some(Activity::Done), app.now())
            .await?;
        let peer = queued_candidate_peer(app, task).await?;
        let queued = app.deps.store.task(&peer).await?.ok_or("queued peer")?;
        let db = rusqlite::Connection::open(data.join("yulius.db"))?;
        db.execute(
            "UPDATE delivery SET stage='unreadable' WHERE task_id=?1",
            [task.id.as_str()],
        )?;
        let unreadable_slot = app.task_summary(&queued).await?.queue_reason;
        db.execute(
            "UPDATE delivery SET stage=?1, pr_number=NULL WHERE task_id=?2",
            rusqlite::params![DeliveryStage::PrOpen.wire_name(), task.id.as_str()],
        )?;
        let unidentified_slot = app.task_summary(&queued).await?.queue_reason;
        let full = Some("Waiting for a free session: 1 of 1 in use");
        assert_eq!(
            (unreadable_slot.as_deref(), unidentified_slot.as_deref()),
            (full, full)
        );
        Ok(())
    })
}

#[test]
fn parking_checks_the_saved_candidate_against_the_current_integration_base() -> TestResult {
    park_after_candidate(false)
}

#[test]
fn a_replacement_arm_arriving_at_final_verification_keeps_the_original_owner_live() -> TestResult {
    park_after_candidate(true)
}

fn park_after_candidate(rearm: bool) -> TestResult {
    with_idle_candidate(async |data, app, remote, id, head| {
        super::past_the_grace(app, &id).await?;
        if rearm {
            let path = data.join(yulius_app::agent_wait::ARMED_MERGES_FILE);
            let db_path = data.join("yulius.db");
            let arm = serde_json::to_vec(
                &json!([{"task":id.as_str(), "head":head,"summary":"replacement"}]),
            )?;
            expect_park_deferred(app, &id, move || {
                assert!(std::fs::write(&path, &arm).is_ok());
                let retired = rusqlite::Connection::open(&db_path).and_then(|db| {
                    db.execute("UPDATE spans SET detail=json_set(detail, '$.superseded_at', 1) WHERE key='wait@15'", [])
                });
                assert!(matches!(retired, Ok(1)));
            }).await?;
        } else {
            expect_park_deferred(app, &id, || {}).await?;
            let span = app
                .deps
                .store
                .latest_merge_wait(&id)
                .await?
                .ok_or("candidate")?;
            assert!(
                app.deps
                    .store
                    .retire_singleton(&id, &span, app.now())
                    .await?
            );
            let saved = span.detail.ok_or("decision detail")?;
            let path = data.join("yulius.db");
            expect_park_deferred(app, &id, move || {
                let restored = rusqlite::Connection::open(&path).and_then(|db| {
                    db.execute("UPDATE spans SET detail=?1 WHERE key='wait@15'", [&saved])
                });
                assert!(matches!(restored, Ok(1)));
            })
            .await?;
            remote.remote.lock().map_err(|_| "remote")?.take();
            remote.pulls.lock().map_err(|_| "PR")?[0].base_sha.clear();
            expect_park_deferred(app, &id, || {}).await?;
            remote.pulls.lock().map_err(|_| "PR")?[0].base_sha = "next-base".into();
            super::parked(app, &id).await?;
        }
        Ok(())
    })
}

#[test]
fn a_published_repair_on_the_same_base_releases_the_candidate_owner() -> TestResult {
    with_idle_candidate(async |_, app, remote, id, _| {
        let task = app.deps.store.task(&id).await?.ok_or("task")?;
        let tree = Path::new(task.worktree_path.as_deref().ok_or("checkout")?);
        crate::pr_watch::git(tree, &["commit", "--allow-empty", "-qm", "Repair latency"])?;
        let head = crate::pr_watch::git(tree, &["rev-parse", "HEAD"])?;
        let branch = task.branch.as_deref().ok_or("task branch")?;
        crate::pr_watch::git(
            tree,
            &["push", "-q", "origin", &format!("HEAD:refs/heads/{branch}")],
        )?;
        set_pull_head(remote, 1481, &head)?;
        remote.set_checks(&head, ChecksState::Success);
        let (mut run, job) = crate::pr_watch::run(
            &task.sandbox_id,
            "repair-green",
            &head,
            RunState::Passed,
            None,
            app.now().0,
        );
        run.reference = CiRef::PullRequest(1481);
        app.deps.store.insert_ci_run(&run, &[job]).await?;
        super::parked(app, &id).await
    })
}

#[test]
fn a_new_pr_on_the_same_head_and_base_releases_the_candidate_owner() -> TestResult {
    with_idle_candidate(async |_, app, remote, id, _| {
        let mut current = app.deps.store.delivery(&id).await?.ok_or("delivery")?;
        current.pr_number = Some(1482);
        current.updated_at = app.now();
        assert!(
            app.deps
                .store
                .transition_delivery(&DeliveryStage::PrOpen, &current)
                .await?
        );
        remote.pulls.lock().map_err(|_| "pulls")?[0].number = 1482;
        super::parked(app, &id).await
    })
}

#[test]
fn an_unrelated_or_merged_pr_releases_the_original_candidate_owner() -> TestResult {
    for field in ["fork", "merged", "head branch", "base branch"] {
        with_idle_candidate(async |_, app, remote, id, _| {
            remote.remote.lock().map_err(|_| "remote")?.take();
            {
                let mut pulls = remote.pulls.lock().map_err(|_| "PR")?;
                match field {
                    "fork" => pulls[0].from_fork = true,
                    "merged" => {
                        pulls[0].merged = true;
                        pulls[0].state = "closed".into();
                    }
                    "head branch" => pulls[0].head = "another-owner".into(),
                    "base branch" => pulls[0].base = "another-target".into(),
                    _ => unreachable!(),
                }
            }
            super::parked(app, &id).await
        })?;
    }
    Ok(())
}

#[test]
fn an_open_candidate_with_a_missing_branch_defers_the_park() -> TestResult {
    for missing_head_branch in [true, false] {
        with_idle_candidate(async |_, app, remote, id, _| {
            remote.remote.lock().map_err(|_| "remote")?.take();
            {
                let mut pulls = remote.pulls.lock().map_err(|_| "PR")?;
                if missing_head_branch {
                    pulls[0].head.clear();
                } else {
                    pulls[0].base.clear();
                }
            }
            expect_park_deferred(app, &id, || {}).await
        })?;
    }
    Ok(())
}

fn with_idle_candidate(
    check: impl AsyncFnOnce(
        &Path,
        &Arc<yulius_app::context::App>,
        &Arc<crate::support::FakeGitHub>,
        TaskId,
        String,
    ) -> TestResult,
) -> TestResult {
    let (_dir, data) = data_dir()?;
    let rt = runtime()?;
    let case = ColdCandidate::Backdated;
    let (id, head, original) = rt.block_on(Box::pin(seed_candidate(&data, &case)))?;
    rt.shutdown_timeout(Duration::from_secs(5));
    let rt = runtime()?;
    let (app, remote) =
        rt.block_on(Box::pin(restore_owner(&data, &case, &id, &head, &original)))?;
    rt.block_on(async {
        let result = check(&data, &app, &remote, id.clone(), head).await;
        if !super::status_of(&app, &id)
            .await
            .0
            .is_some_and(|state| state.is_resting())
        {
            app.stop_task_now(&id).await?;
        }
        result
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

#[test]
fn a_singleton_decided_on_a_new_base_during_the_host_probe_keeps_the_owner_live() -> TestResult {
    with_idle_candidate(async |data, app, remote, id, _| {
        let span = app
            .deps
            .store
            .latest_merge_wait(&id)
            .await?
            .ok_or("candidate")?;
        assert!(
            app.deps
                .store
                .retire_singleton(&id, &span, app.now())
                .await?
        );
        let mut decision: serde_json::Value = serde_json::from_str(&span.detail.ok_or("receipt")?)?;
        decision["singleton"]["base"] = json!("new-tested-base");
        let saved = decision.to_string();
        let path = data.join("yulius.db");
        let remote = remote.clone();
        expect_park_deferred(app, &id, move || {
            if let Ok(mut origin) = remote.remote.lock() {
                origin.take();
            }
            if let Ok(mut pulls) = remote.pulls.lock() {
                pulls[0].base_sha = "new-tested-base".into();
            }
            let accepted = rusqlite::Connection::open(&path).and_then(|db| {
                db.execute("UPDATE spans SET detail=?1 WHERE key='wait@15'", [&saved])
            });
            assert!(matches!(accepted, Ok(1)));
        })
        .await?;
        expect_park_deferred(app, &id, || {}).await?;
        Ok(())
    })
}

#[test]
fn the_owner_stays_live_while_its_replacement_merge_is_being_armed() -> TestResult {
    contending_merge_request(true)
}

#[test]
fn a_merge_requested_while_its_owner_is_committed_to_park_is_refused() -> TestResult {
    contending_merge_request(false)
}

fn contending_merge_request(arm_first: bool) -> TestResult {
    with_idle_candidate(async |_, app, remote, id, head| {
        let ctx = pending_owner_merge(app, remote, &id, &head).await?;
        if !arm_first {
            let (decided, release) = super::pause_watch_or_queue(super::Point::ParkDecided, &id);
            let park = {
                let app = app.clone();
                let id = id.clone();
                tokio::spawn(async move {
                    super::parked(&app, &id)
                        .await
                        .map_err(|err| err.to_string())
                })
            };
            tokio::time::timeout(Duration::from_secs(10), decided.notified()).await?;
            let request = app.merge_for_person(&ctx, id.as_str()).await;
            release.notify_one();
            park.await??;
            assert!(
                matches!(&request, Err(yulius_app::error::AppError::Conflict(reason)) if reason.contains("park")),
                "merge during park: {request:?}"
            );
            return Ok(());
        }
        let (entered, release) = super::pause_watch_or_queue(super::Point::ArmWriteStarted, &id);
        let mut arming = {
            let app = app.clone();
            let id = id.clone();
            tokio::spawn(async move { app.merge_for_person(&ctx, id.as_str()).await })
        };
        tokio::select! {
            () = entered.notified() => (),
            result = &mut arming => return Err(format!("merge completed before arm write: {:?}", result?).into()),
        }
        let decision = expect_park_deferred(app, &id, || {}).await;
        release.notify_one();
        let armed = arming.await??;
        decision?;
        assert_eq!(armed["armed"], true);
        Ok(())
    })
}

#[test]
fn a_merge_prepared_while_running_checks_the_settled_park_before_arming() -> TestResult {
    with_idle_candidate(async |_, app, remote, id, head| {
        let ctx = pending_owner_merge(app, remote, &id, &head).await?;
        let (decided, finish_park) = super::pause_watch_or_queue(super::Point::ParkDecided, &id);
        let (admitting, finish_arm) =
            super::pause_watch_or_queue(super::Point::ArmWriteStarted, &id);
        let parking_app = app.clone();
        let parking_id = id.clone();
        let park = tokio::spawn(async move {
            super::parked(&parking_app, &parking_id)
                .await
                .map_err(|err| err.to_string())
        });
        tokio::time::timeout(Duration::from_secs(10), decided.notified()).await?;
        let merge_app = app.clone();
        let merge_id = id.clone();
        let request =
            tokio::spawn(async move { merge_app.merge_for_person(&ctx, merge_id.as_str()).await });
        tokio::time::timeout(Duration::from_secs(10), admitting.notified()).await?;
        finish_park.notify_one();
        let settled = park.await?;
        finish_arm.notify_one();
        settled?;
        let result = request.await?;
        assert!(
            matches!(&result, Err(yulius_app::error::AppError::Conflict(reason)) if reason.contains("parked")),
            "merge after park settled: {result:?}"
        );
        Ok(())
    })
}

#[test]
fn a_completed_replacement_arm_keeps_its_owner_when_parking_takes_the_writer() -> TestResult {
    with_idle_candidate(async |_, app, remote, id, head| {
        let ctx = pending_owner_merge(app, remote, &id, &head).await?;
        let pending_ctx = ctx.clone();
        let arming_app = app.clone();
        let arming_id = id.clone();
        let handle = tokio::runtime::Handle::current();
        expect_park_deferred(app, &id, move || {
            let armed = tokio::task::block_in_place(|| {
                handle.block_on(arming_app.merge_for_person(&ctx, arming_id.as_str()))
            });
            assert!(armed.is_ok_and(|answer| answer["armed"] == true));
        })
        .await?;
        assert_eq!(
            super::status_of(app, &id).await.0,
            Some(yulius_domain::task::TaskStatus::Running)
        );
        assert_eq!(
            app.merge_state_for_person(&pending_ctx, id.as_str())
                .await?["armed"],
            true
        );
        Ok(())
    })
}

#[test]
fn parking_prepared_before_close_leaves_the_settled_owner_free_to_continue() -> TestResult {
    close_during_park_preparation(false)
}

#[test]
fn parking_prepared_before_close_gives_way_to_the_reader_settling_the_run() -> TestResult {
    close_during_park_preparation(true)
}

fn close_during_park_preparation(hold_reader: bool) -> TestResult {
    with_idle_candidate(async |_, app, remote, id, head| {
        let owner = pending_owner_merge(app, remote, &id, &head).await?;
        let gate = hold_reader.then(|| super::pause_watch_or_queue(super::Point::RunEndTaken, &id));
        let taken = gate.as_ref().map(|(taken, _)| taken.clone());
        let mut exited = app.runtime(&id).hub.watch();
        let mut completed = app.runtime(&id).hub.watch();
        let closing = app.clone();
        let closing_id = id.clone();
        let handle = tokio::runtime::Handle::current();
        let decision = expect_park_deferred(app, &id, move || {
            let closed = tokio::task::block_in_place(|| {
                handle.block_on(async {
                    closing.close_task_now(&closing_id).await?;
                    if let Some(taken) = taken {
                        tokio::time::timeout(Duration::from_secs(10), taken.notified()).await?;
                    } else {
                        old_run_exited(&mut exited).await?;
                    }
                    Ok::<_, Error>(())
                })
            });
            assert!(
                closed.is_ok(),
                "Close did not reach the selected point: {closed:?}"
            );
        })
        .await;
        if let Some((_, release)) = gate {
            release.notify_one();
        }
        old_run_exited(&mut completed).await?;
        assert_eq!(
            super::status_of(app, &id).await.0,
            Some(yulius_domain::task::TaskStatus::Closed)
        );
        arm_capacity_delayed_continuation(app, &id, &owner).await?;
        decision
    })
}

#[test]
fn a_closed_owner_can_arm_during_a_capacity_delayed_continue_after_a_superseded_park() -> TestResult
{
    use yulius_domain::task::TaskStatus;
    with_idle_candidate(async |_, app, remote, id, head| {
        let ctx = pending_owner_merge(app, remote, &id, &head).await?;
        let close_app = app.clone();
        let close_id = id.clone();
        let handle = tokio::runtime::Handle::current();
        yulius_app::race::on(
            super::Point::ParkDecided,
            Path::new(id.as_str()),
            move || {
                let result = tokio::task::block_in_place(|| {
                    handle.block_on(close_app.close_task_now(&close_id))
                });
                assert!(result.is_ok(), "{result:?}");
            },
        );
        super::wait_for("the superseding close", Duration::from_secs(30), || async {
            if super::status_of(app, &id).await.0 == Some(TaskStatus::Running) {
                app.park_idle_sessions_now().await;
            }
            super::status_of(app, &id).await.0 == Some(TaskStatus::Closed)
        })
        .await?;
        arm_capacity_delayed_continuation(app, &id, &ctx).await?;
        Ok(())
    })
}

#[test]
fn stop_before_the_parked_run_is_reaped_releases_the_continued_owner() -> TestResult {
    continued_after_capacity_stop(false)
}

#[test]
fn stop_after_the_parked_run_is_reaped_releases_the_continued_owner() -> TestResult {
    continued_after_capacity_stop(true)
}

fn continued_after_capacity_stop(after_reap: bool) -> TestResult {
    use yulius_domain::task::{StopReason, TaskStatus};
    with_idle_candidate(async |_, app, remote, id, head| {
        let ctx = pending_owner_merge(app, remote, &id, &head).await?;
        let mut frames = app.runtime(&id).hub.watch();
        let pause_app = app.clone();
        let pause_id = id.clone();
        let handle = tokio::runtime::Handle::current();
        yulius_app::race::on(
            super::Point::ParkDecided,
            Path::new(id.as_str()),
            move || {
                let stopped = tokio::task::block_in_place(|| {
                    handle.block_on(async {
                        pause_app
                            .pause_task(&pause_id, yulius_app::model::PauseKind::Prompt, None, None)
                            .await?;
                        if !after_reap {
                            assert!(
                                pause_app
                                    .deps
                                    .store
                                    .live_attempt(&pause_id)
                                    .await?
                                    .is_some()
                            );
                            pause_app.stop_task_now(&pause_id).await?;
                        }
                        Ok::<_, yulius_app::error::AppError>(())
                    })
                });
                assert!(stopped.is_ok(), "{stopped:?}");
            },
        );
        if after_reap {
            let stop_app = app.clone();
            let stop_id = id.clone();
            let handle = tokio::runtime::Handle::current();
            yulius_app::race::on(
                super::Point::EndDecided,
                Path::new(id.as_str()),
                move || {
                    let stopped = tokio::task::block_in_place(|| {
                        handle.block_on(async {
                            assert!(stop_app.deps.store.live_attempt(&stop_id).await?.is_none());
                            stop_app.stop_task_now(&stop_id).await
                        })
                    });
                    assert!(stopped.is_ok(), "{stopped:?}");
                },
            );
        }
        super::wait_for("the park before Stop", Duration::from_secs(20), || async {
            app.park_idle_sessions_now().await == 1
        })
        .await?;
        old_run_exited(&mut frames).await?;
        assert_eq!(
            super::status_of(app, &id).await.0,
            Some(TaskStatus::Stopped(StopReason::Cancelled))
        );
        arm_capacity_delayed_continuation(app, &id, &ctx).await
    })
}

#[test]
fn a_queued_continue_can_arm_after_the_old_park_reader_exits() -> TestResult {
    continue_before_old_park_settles(true)
}

#[test]
fn a_capacity_delayed_continue_survives_the_old_park_reader() -> TestResult {
    continue_before_old_park_settles(false)
}

fn continue_before_old_park_settles(queue: bool) -> TestResult {
    use yulius_domain::task::{StopReason, TaskStatus};
    with_idle_candidate(async |_, app, remote, id, head| {
        let ctx = pending_owner_merge(app, remote, &id, &head).await?;
        let task = app.deps.store.task(&id).await?.ok_or("owner")?;
        let peer = if queue {
            let peer = super::new_task(
                app,
                &task.sandbox_id,
                &task.owner,
                "Use the available slot",
                yulius_app::model::TaskMode::Prompt,
            )
            .await?;
            super::wait_for(
                "the occupied session slot",
                Duration::from_secs(30),
                || async { super::screen(app, &peer).contains("ECHO: Use the available slot") },
            )
            .await?;
            app.pause_task_for_person(&ctx, peer.as_str()).await?;
            app.load_limits([(yulius_domain::limits::Limit::Sessions, 1)].into())
                .await;
            assert_eq!(app.max_live_sessions(), 1);
            Some(peer)
        } else {
            None
        };
        let mut frames = app.runtime(&id).hub.watch();
        let (decided, release) = super::pause_watch_or_queue(super::Point::EndDecided, &id);
        let pause_app = app.clone();
        let pause_id = id.clone();
        let handle = tokio::runtime::Handle::current();
        yulius_app::race::on(
            super::Point::ParkDecided,
            Path::new(id.as_str()),
            move || {
                let result = tokio::task::block_in_place(|| {
                    handle.block_on(pause_app.pause_task(
                        &pause_id,
                        yulius_app::model::PauseKind::Prompt,
                        None,
                        None,
                    ))
                });
                assert!(result.is_ok(), "{result:?}");
            },
        );
        super::wait_for(
            "the park with a capacity hold",
            Duration::from_secs(20),
            || async { app.park_idle_sessions_now().await == 1 },
        )
        .await?;
        tokio::time::timeout(Duration::from_secs(10), decided.notified()).await?;
        assert!(app.deps.store.live_attempt(&id).await?.is_none());
        app.stop_task_now(&id).await?;
        assert_eq!(
            super::status_of(app, &id).await.0,
            Some(TaskStatus::Stopped(StopReason::Cancelled))
        );
        if !queue {
            limit_only_subscription(app).await?;
        }
        let reader_wait = Arc::new(tokio::sync::Notify::new());
        let reached = reader_wait.clone();
        yulius_app::race::on(
            super::Point::ContinueWaitsForReader,
            Path::new(id.as_str()),
            move || reached.notify_one(),
        );
        let owner = ctx.clone();
        let resumed = app.clone();
        let owner_id = id.clone();
        let req = serde_json::from_value(json!({}))?;
        let mut continuing =
            tokio::spawn(
                async move { resumed.continue_task(&owner, owner_id.as_str(), &req).await },
            );
        tokio::select! {
            () = reader_wait.notified() => (),
            early = &mut continuing => {
                release.notify_one();
                return Err(format!("Continue finished before reader settlement: {:?}", early?.map(|task| task.status)).into());
            }
        }
        release.notify_one();
        old_run_exited(&mut frames).await?;
        continuing.await??;
        let waiting = if queue {
            TaskStatus::Queued
        } else {
            TaskStatus::WaitingCapacity
        };
        assert_eq!(super::status_of(app, &id).await.0, Some(waiting));
        let arm = app.merge_for_person(&ctx, id.as_str()).await?;
        assert_eq!(arm["armed"], true);
        app.stop_task_now(&id).await?;
        if let Some(peer) = peer {
            app.stop_task_now(&peer).await?;
        }
        Ok(())
    })
}

async fn arm_capacity_delayed_continuation(
    app: &Arc<yulius_app::context::App>,
    id: &TaskId,
    ctx: &yulius_app::auth::AuthContext,
) -> TestResult {
    use yulius_domain::task::TaskStatus;
    limit_only_subscription(app).await?;
    app.continue_task(ctx, id.as_str(), &serde_json::from_value(json!({}))?)
        .await?;
    assert_eq!(
        super::status_of(app, id).await.0,
        Some(TaskStatus::WaitingCapacity)
    );
    assert!(app.deps.store.live_attempt(id).await?.is_none());
    let response = app.merge_for_person(ctx, id.as_str()).await?;
    assert_eq!(response["armed"], true);
    Ok(())
}

async fn limit_only_subscription(app: &yulius_app::context::App) -> TestResult {
    let account = yulius_domain::ids::AccountId::parse("a-only")?;
    let usage = json!({"primary":{"used_percent":100.0,"window_minutes":300,"resets_at":app.now().plus_millis(3_600_000).0}}).to_string();
    app.deps
        .store
        .set_account_state(
            &account,
            yulius_app::model::AccountStatus::Limited,
            Some(&usage),
            None,
            app.now(),
        )
        .await?;
    Ok(())
}

async fn old_run_exited(
    frames: &mut tokio::sync::broadcast::Receiver<yulius_app::hub::HubFrame>,
) -> TestResult {
    tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            if matches!(frames.recv().await?, yulius_app::hub::HubFrame::Exit { .. }) {
                return Ok::<(), tokio::sync::broadcast::error::RecvError>(());
            }
        }
    })
    .await??;
    Ok(())
}

async fn pending_owner_merge(
    app: &Arc<yulius_app::context::App>,
    remote: &Arc<crate::support::FakeGitHub>,
    id: &TaskId,
    head: &str,
) -> Result<yulius_app::auth::AuthContext, Error> {
    let task = app.deps.store.task(id).await?.ok_or("task")?;
    let span = app
        .deps
        .store
        .latest_merge_wait(id)
        .await?
        .ok_or("candidate")?;
    assert!(
        app.deps
            .store
            .retire_singleton(id, &span, app.now())
            .await?
    );
    let (mut rerun, job) = crate::pr_watch::run(
        &task.sandbox_id,
        "ordinary-rerun",
        head,
        RunState::Running,
        None,
        app.now().0,
    );
    rerun.reference = CiRef::PullRequest(1481);
    app.deps.store.insert_ci_run(&rerun, &[job]).await?;
    remote.set_checks(head, ChecksState::Pending);
    remote.pulls.lock().map_err(|_| "pulls")?[0].title = "fix(search): repair latency".into();
    let ctx = yulius_app::auth::AuthContext {
        user: app.deps.store.user(&task.owner).await?.ok_or("owner")?,
        csrf_token: None,
        session_hash: None,
        token: None,
    };
    Ok(ctx)
}

#[test]
fn an_overtaken_switch_refuses_to_arm_after_the_run_reader_takes_the_park() -> TestResult {
    merge_while_switching(true)
}

#[test]
fn a_normal_subscription_switch_allows_the_owners_pending_merge() -> TestResult {
    merge_while_switching(false)
}

fn merge_while_switching(parking: bool) -> TestResult {
    with_idle_candidate(async |_, app, remote, id, head| {
        let ctx = pending_owner_merge(app, remote, &id, &head).await?;
        let (taken, release) = super::pause_watch_or_queue(super::Point::RunEndTaken, &id);
        let app2 = app.clone();
        let id2 = id.clone();
        let settling = if parking {
            let switch_app = app.clone();
            let switch_id = id.clone();
            let handle = tokio::runtime::Handle::current();
            yulius_app::race::on(
                super::Point::ParkDecided,
                Path::new(id.as_str()),
                move || {
                    let switched = tokio::task::block_in_place(|| {
                        handle
                            .block_on(switch_app.switch_task_now(&switch_id, None, "manual", None))
                    });
                    assert!(
                        switched.is_err_and(|err| err
                            .to_string()
                            .contains("the idle session was parked"))
                    );
                },
            );
            tokio::spawn(async move {
                super::parked(&app2, &id2)
                    .await
                    .map_err(|err| err.to_string())
            })
        } else {
            tokio::spawn(async move {
                app2.switch_task_now(&id2, None, "manual", None)
                    .await
                    .map(|_| ())
                    .map_err(|err| err.to_string())
            })
        };
        tokio::time::timeout(Duration::from_secs(10), taken.notified()).await?;
        let state = super::status_of(app, &id).await.0;
        let request = app.merge_for_person(&ctx, id.as_str()).await;
        let armed = app.merge_state_for_person(&ctx, id.as_str()).await?;
        release.notify_one();
        settling.await??;
        assert_eq!(state, Some(yulius_domain::task::TaskStatus::Switching));
        if parking {
            assert!(
                matches!(&request, Err(yulius_app::error::AppError::Conflict(reason)) if reason.contains("park")),
                "{request:?}"
            );
            assert_eq!(armed["armed"], false);
        } else {
            assert_eq!(request?["armed"], true);
            assert_eq!(armed["armed"], true);
        }
        Ok(())
    })
}

#[test]
fn a_capacity_pause_cannot_accept_a_merge_after_parking_commits() -> TestResult {
    capacity_pause_merge(true)
}

#[test]
fn a_capacity_pause_without_a_park_can_accept_a_merge() -> TestResult {
    capacity_pause_merge(false)
}

fn capacity_pause_merge(parking: bool) -> TestResult {
    use yulius_app::model::PauseKind;
    use yulius_domain::task::TaskStatus;
    with_idle_candidate(async |_, app, remote, id, head| {
        let ctx = pending_owner_merge(app, remote, &id, &head).await?;
        let release = if parking {
            let (taken, release) = super::pause_watch_or_queue(super::Point::RunEndTaken, &id);
            super::wait_for(
                "the run to commit its park",
                Duration::from_secs(10),
                || async { app.park_idle_sessions_now().await == 1 },
            )
            .await?;
            tokio::time::timeout(Duration::from_secs(10), taken.notified()).await?;
            Some(release)
        } else {
            None
        };
        app.pause_task(&id, PauseKind::Prompt, None, None).await?;
        let state = super::status_of(app, &id).await.0;
        let request = app.merge_for_person(&ctx, id.as_str()).await;
        let armed = app.merge_state_for_person(&ctx, id.as_str()).await;
        if let Some(release) = release {
            release.notify_one();
            super::wait_for(
                "the paused ending run to close",
                Duration::from_secs(10),
                || async { super::status_of(app, &id).await.0 == Some(TaskStatus::Closed) },
            )
            .await?;
        }
        assert_eq!(state, Some(TaskStatus::WaitingCapacity));
        if parking {
            assert!(
                matches!(&request, Err(yulius_app::error::AppError::Conflict(_))),
                "{request:?}"
            );
            assert_eq!(armed?["armed"], false);
        } else {
            assert_eq!(request?["armed"], true);
            assert_eq!(armed?["armed"], true);
        }
        Ok(())
    })
}

/// A restarted host can still be reattaching. Require an actual eligibility
/// decision so an earlier probe that deferred shutdown cannot satisfy the assertion.
async fn expect_park_deferred(
    app: &Arc<yulius_app::context::App>,
    id: &TaskId,
    change: impl FnOnce() + Send + 'static,
) -> TestResult {
    use std::sync::atomic::{AtomicBool, Ordering};
    let checked = Arc::new(AtomicBool::new(false));
    let reached = checked.clone();
    yulius_app::race::on(
        super::Point::ParkPrepared,
        Path::new(id.as_str()),
        move || {
            change();
            reached.store(true, Ordering::SeqCst);
        },
    );
    let started = std::cell::Cell::new(0);
    super::wait_for(
        "the original owner's final park decision",
        Duration::from_secs(10),
        || async {
            started.set(app.park_idle_sessions_now().await);
            checked.load(Ordering::SeqCst) || started.get() != 0
        },
    )
    .await?;
    if started.get() != 0 {
        return Err("original PR owner was parked during final verification".into());
    }
    Ok(())
}

/// Restore a real owner after restart. Parked cases make the completed queue
/// decision visible after the original process has already parked.
fn with_parked_candidate_owner(
    check: impl AsyncFnOnce(
        &Path,
        &Arc<yulius_app::context::App>,
        &Arc<crate::support::FakeGitHub>,
        &yulius_app::model::TaskRecord,
    ) -> TestResult,
) -> TestResult {
    let (_dir, data) = data_dir()?;
    let rt = runtime()?;
    let case = ColdCandidate::Backdated;
    let (id, head, original) = rt.block_on(Box::pin(seed_candidate(&data, &case)))?;
    rt.shutdown_timeout(Duration::from_secs(5));
    let rt = runtime()?;
    let (app, remote) =
        rt.block_on(Box::pin(restore_owner(&data, &case, &id, &head, &original)))?;
    let verify = async {
        {
            let db = rusqlite::Connection::open(data.join("yulius.db"))?;
            db.execute("UPDATE spans SET outcome='pending' WHERE key='wait@15'", [])?;
            super::parked(&app, &id).await?;
            db.execute("UPDATE spans SET outcome='dropped' WHERE key='wait@15'", [])?;
        }
        let task = app.deps.store.task(&id).await?.ok_or("owner")?;
        let result = check(&data, &app, &remote, &task).await;
        app.stop_task_now(&id).await?;
        result
    };
    rt.block_on(Box::pin(verify))?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

#[test]
fn a_parked_candidate_owner_stays_parked_until_a_person_messages_it() -> TestResult {
    with_parked_candidate_owner(async |_, app, _, task| {
        let id = &task.id;
        let before = app.deps.store.attempts(id).await?;
        let session = before
            .last()
            .and_then(|a| a.provider_session_id.as_deref())
            .ok_or("parked owner session")?;
        for _ in 0..2 {
            assert!(app.watch_prs(&WatchBounds::default()).await.is_empty());
            assert_eq!(
                super::status_of(app, id).await.0,
                Some(yulius_domain::task::TaskStatus::Parked)
            );
        }
        assert_eq!(app.deps.store.attempts(id).await?.len(), before.len());
        let caller = super::superagent(&task.sandbox_id, &task.owner, "admin");
        app.tool_task_message(
            &caller,
            &json!({"task_id": id.as_str(), "text": "Please continue the investigation"}),
        )
        .await?;
        super::wait_for(
            "the person's message in the original session",
            Duration::from_secs(60),
            || async { super::screen(app, id).contains("Please continue the investigation") },
        )
        .await?;
        let after = app.deps.store.attempts(id).await?;
        assert_eq!(after.len(), before.len() + 1);
        assert_eq!(
            after.last().and_then(|a| a.provider_session_id.as_deref()),
            Some(session)
        );
        Ok(())
    })
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

#[test]
fn a_repair_pushed_during_dispatch_suppresses_the_old_ordinary_failure() -> TestResult {
    cold_candidate_case(ColdCandidate::RepairPushedDuringRead)
}

#[test]
fn a_repair_published_by_another_clone_during_dispatch_suppresses_stale_advice() -> TestResult {
    cold_candidate_case(ColdCandidate::ExternalRepair)
}

#[test]
fn an_armed_owner_with_a_durable_scope_decision_receives_ci_after_resolution() -> TestResult {
    cold_candidate_case(ColdCandidate::ScopeDecision)
}

#[test]
fn an_unverified_hosted_pull_keeps_its_ordinary_failure_unheard_until_recovery() -> TestResult {
    cold_candidate_case(ColdCandidate::HostedPullUnavailable)
}

#[test]
fn an_update_hold_arriving_before_armed_dispatch_keeps_the_notice_for_release() -> TestResult {
    cold_candidate_case(ColdCandidate::UpdateHold)
}

#[test]
fn a_reported_ordinary_infrastructure_error_leaves_the_train_failure_due() -> TestResult {
    cold_candidate_case(ColdCandidate::OrdinaryErrorThenTrainFailure)
}

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
fn a_new_ordinary_run_after_a_singleton_deadline_gets_its_own_wait() -> TestResult {
    cold_candidate_case(ColdCandidate::NewerRunAfterExpiry)
}

#[test]
fn a_recorded_candidate_waits_for_an_ordinary_rerun_admitted_at_final_verification() -> TestResult {
    cold_candidate_case(ColdCandidate::LateReceiptRerun)
}

#[test]
fn a_conflicting_replacement_head_without_ordinary_ci_reaches_the_original_owner() -> TestResult {
    cold_candidate_case(ColdCandidate::ReplacementConflict)
}

#[test]
fn a_conflicting_replacement_with_an_older_green_run_reaches_the_original_owner_once() -> TestResult
{
    cold_candidate_case(ColdCandidate::ReplacementWithOldGreen)
}

#[test]
fn a_new_ordinary_reruns_deadline_survives_restart_and_delivers_the_singleton_once() -> TestResult {
    use yulius_app::merge_train::WAIT_LIMIT_MS;
    let (_dir, data) = data_dir()?;
    let rt = runtime()?;
    let (id, head, original) = rt.block_on(Box::pin(seed_candidate(
        &data,
        &ColdCandidate::TimedOutOrdinary,
    )))?;
    rt.shutdown_timeout(Duration::from_secs(5));
    let queued = 1_200_000;
    let rerun_at = queued + WAIT_LIMIT_MS + 1;
    let new_decision_at = rerun_at + WAIT_LIMIT_MS + 1;
    for (now, expect_diagnosis) in [
        (queued, false),
        (queued + WAIT_LIMIT_MS, false),
        (rerun_at, false),
        (new_decision_at, false),
        (new_decision_at + WAIT_LIMIT_MS, false),
        (new_decision_at + WAIT_LIMIT_MS + 1, true),
    ] {
        let rt = runtime()?;
        rt.block_on(async {
            let clock = Arc::new(crate::support::TestClock(now.into()));
            let (app, remote) = super::app_with_pulls_and_clock(&data, 8, 1, clock, |_| {}).await?;
            remote.set_remote(Arc::new(crate::delivery::BareRemote {
                origin: data.join("origin.git"),
            }));
            open_pull_at(&app, &remote, &id, 1481, &head).await?;
            remote.set_checks(&head, ChecksState::Success);
            app.reconcile().await?;
            assert_eq!(
                app.deps
                    .store
                    .live_attempt(&id)
                    .await?
                    .ok_or("original attempt")?
                    .id,
                original
            );
            if now == queued {
                let owner = app.deps.store.task(&id).await?.ok_or("owner")?;
                let (mut run, job) = crate::pr_watch::run(
                    &owner.sandbox_id,
                    "still-queued",
                    &head,
                    RunState::Queued,
                    None,
                    queued,
                );
                run.reference = CiRef::PullRequest(1481);
                run.started_at = None;
                run.ended_at = None;
                app.deps.store.insert_ci_run(&run, &[job]).await?;
                let (peer, job) = crate::pr_watch::run(
                    &owner.sandbox_id,
                    "unrelated-queued",
                    "peer-head",
                    RunState::Queued,
                    None,
                    30_000,
                );
                app.deps.store.insert_ci_run(&peer, &[job]).await?;
                assert!(app.deps.store.ci_run_to_front(&run.id).await?);
                let promoted = app
                    .deps
                    .store
                    .ci_run(&run.id)
                    .await?
                    .ok_or("promoted rerun")?;
                assert!(promoted.queued_at.0 < now - WAIT_LIMIT_MS);
            }
            if now == rerun_at {
                let owner = app.deps.store.task(&id).await?.ok_or("owner")?;
                assert!(
                    app.deps
                        .store
                        .transition_ci_run(
                            "still-queued",
                            &[RunState::Queued],
                            RunState::Cancelled,
                            &yulius_app::ci::CiRunUpdate {
                                ended_at: Some(app.now()),
                                ..Default::default()
                            }
                        )
                        .await?
                );
                let (mut run, job) = crate::pr_watch::run(
                    &owner.sandbox_id,
                    "second-rerun",
                    &head,
                    RunState::Running,
                    None,
                    now,
                );
                run.reference = CiRef::PullRequest(1481);
                app.deps.store.insert_ci_run(&run, &[job]).await?;
            }
            if now == new_decision_at {
                let prior = app
                    .deps
                    .store
                    .latest_merge_wait(&id)
                    .await?
                    .ok_or("decision")?;
                // A later arm tests the same inputs while this rerun is still active.
                app.deps
                    .store
                    .insert_span(&yulius_app::stages::NewSpan {
                        key: format!("wait@{now}"),
                        started_at: app.now(),
                        ended_at: Some(app.now()),
                        sandbox_id: prior.sandbox_id,
                        subject_kind: prior.subject_kind,
                        subject_id: prior.subject_id,
                        parent_key: prior.parent_key,
                        kind: prior.kind,
                        name: prior.name,
                        environment: prior.environment,
                        job: prior.job,
                        outcome: prior.outcome,
                        detail: prior.detail,
                    })
                    .await?;
            }
            let bounds = WatchBounds::default();
            let news = Box::pin(app.watch_prs(&bounds)).await;
            if expect_diagnosis {
                assert_eq!(
                    news.len(),
                    1,
                    "expired rerun suppressed the decision: {news:?}"
                );
                assert!(news[0].1.contains("292.1 ms"), "{news:?}");
                assert!(Box::pin(app.watch_prs(&bounds)).await.is_empty());
                app.stop_task_now(&id).await?;
            } else {
                assert!(news.is_empty(), "rerun still within its wait: {news:?}");
            }
            Ok::<_, Error>(())
        })?;
        rt.shutdown_timeout(Duration::from_secs(5));
    }
    Ok(())
}

enum ColdCandidate {
    ReplacementConflict,
    ReplacementWithOldGreen,
    LateReceiptRerun,
    OlderOrdinaryFailure,
    ReplacedOrdinaryError,
    NewerRunAfterExpiry,
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

    OrdinaryErrorThenTrainFailure,
    UpdateHold,
    HostedPullUnavailable,
    ExternalRepair,
    ScopeDecision,
    RepairPushedDuringRead,
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
    let prior_notice = matches!(case, ColdCandidate::PriorNotice);
    let (_dir, data) = data_dir()?;
    let rt = runtime()?;
    let (id, head, original) = rt.block_on(Box::pin(seed_candidate(&data, &case)))?;
    rt.shutdown_timeout(Duration::from_secs(5));
    let rt = runtime()?;
    let (cold, remote) =
        rt.block_on(Box::pin(restore_owner(&data, &case, &id, &head, &original)))?;
    if matches!(
        case,
        ColdCandidate::OlderOrdinaryFailure
            | ColdCandidate::ReplacedOrdinaryError
            | ColdCandidate::NewerRunAfterExpiry
            | ColdCandidate::TimedOutOrdinary
            | ColdCandidate::PriorBisection
            | ColdCandidate::Stall
            | ColdCandidate::MovedHead
            | ColdCandidate::OwnerRerun
            | ColdCandidate::Backdated
            | ColdCandidate::UnrelatedRuns
            | ColdCandidate::PriorNotice
            | ColdCandidate::OrdinaryCancelled
            | ColdCandidate::OrdinaryError
            | ColdCandidate::SourceIncludesOwner
            | ColdCandidate::LateReceiptRerun
            | ColdCandidate::ReplacementConflict
            | ColdCandidate::ReplacementWithOldGreen
    ) {
        rt.block_on(Box::pin(check_recorded_decision(
            &data, &case, &id, &head, cold, remote,
        )))?;
    } else {
        rt.block_on(Box::pin(check_live_queue(
            &data, &case, &id, &head, cold, remote,
        )))?;
    }
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
            let duplicate = Box::pin(again.watch_prs(&WatchBounds::default())).await;
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
    let branch = task.branch.as_deref().ok_or("task branch")?;
    crate::pr_watch::git(
        tree,
        &["push", "-q", "origin", &format!("HEAD:refs/heads/{branch}")],
    )?;
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
    green.reference = CiRef::PullRequest(if matches!(case, ColdCandidate::ReplacementConflict) {
        99
    } else {
        1481
    });
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
            ColdCandidate::SharedWhileRunning
                | ColdCandidate::TrainFailsDuringRead
                | ColdCandidate::OrdinaryErrorThenTrainFailure
        ) {
            RunState::Running
        } else {
            RunState::Failed
        },
        Some(failures),
        20,
    );
    if matches!(case, ColdCandidate::PriorNotice) {
        let notice = Box::pin(app.watch_prs(&WatchBounds::default())).await;
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

async fn restore_owner(
    data: &Path,
    case: &ColdCandidate,
    id: &TaskId,
    head: &str,
    original: &AttemptId,
) -> Result<
    (
        Arc<yulius_app::context::App>,
        Arc<crate::support::FakeGitHub>,
    ),
    Error,
> {
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
    // The reattached session's watcher starts its first round a second later: it probes the
    // idle shells (unknown until then) and replays the transcript to its end, publishing and
    // settling the activity on its own clock. A test sees the owner after that round, not
    // racing it (SPEC-640 FLK-001).
    super::wait_for(
        "the reattached transcript replay",
        Duration::from_secs(30),
        || async { cold.runtime(id).transcript_rounds() >= 2 },
    )
    .await?;
    remote.set_checks(head, ChecksState::Success);
    let bounds = WatchBounds::default();
    ask(&cold, id).await?;
    assert!(cold.watch_prs(&bounds).await.is_empty());
    cold.deps
        .store
        .set_task_needs_you(id, None, cold.now())
        .await?;
    Ok((cold, remote))
}

async fn check_live_queue(
    data: &Path,
    case: &ColdCandidate,
    id: &TaskId,
    head: &str,
    cold: Arc<yulius_app::context::App>,
    remote: Arc<crate::support::FakeGitHub>,
) -> TestResult {
    let bounds = WatchBounds::default();
    let origin = data.join("origin.git");
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
            | ColdCandidate::RepairPushedDuringRead
            | ColdCandidate::ExternalRepair
            | ColdCandidate::HostedPullUnavailable
            | ColdCandidate::ScopeDecision
            | ColdCandidate::UpdateHold
            | ColdCandidate::OrdinaryErrorThenTrainFailure
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
                            ..WatchBounds::default()
                        })
                        .await
                });
                tokio::time::timeout(Duration::from_secs(10), entered.notified()).await?;
                Some((release, pass))
            } else {
                None
            };
            let pulls = std::mem::take(&mut *remote.pulls.lock().map_err(|_| "pulls")?);
            Box::pin(cold.merge_armed(&sandbox)).await;
            *remote.pulls.lock().map_err(|_| "pulls")? = pulls;
            if let Some((release, pass)) = waiting {
                release.notify_one();
                let outdated = pass.await?;
                assert!(
                    outdated.is_empty(),
                    "the queue kept the same arm with a new reason: {outdated:?}"
                );
            }
            if matches!(case, ColdCandidate::FreeNoteChanges) {
                // The next queue retry returns the same arm and note. Only the count of
                // decisions started reveals that it overtook this notice during dispatch.
                let (entered, release) =
                    super::pause_watch_or_queue(super::Point::ArmedNoticePublicationRead, id);
                let watcher = Arc::clone(&cold);
                let pass = tokio::spawn(async move { watcher.watch_prs(&bounds).await });
                tokio::time::timeout(Duration::from_secs(10), entered.notified()).await?;
                let pulls = std::mem::take(&mut *remote.pulls.lock().map_err(|_| "pulls")?);
                Box::pin(cold.merge_armed(&sandbox)).await;
                *remote.pulls.lock().map_err(|_| "pulls")? = pulls;
                release.notify_one();
                let stale = pass.await?;
                assert!(
                    stale.is_empty(),
                    "completed queue retry overtook dispatch: {stale:?}"
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
                        ..WatchBounds::default()
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
            ColdCandidate::TrainFailsDuringRead
                | ColdCandidate::OrdinaryFailsDuringRead
                | ColdCandidate::OrdinaryErrorThenTrainFailure
        ) {
            train[sandbox.as_str()]["train"]["members"].as_array_mut().ok_or("train members")?.push(
                json!({"task":"second-owner","pr":99,"head":"other","armed_at":29,"tree":"other-tree"})
            );
        }
        if matches!(case, ColdCandidate::OrdinaryErrorThenTrainFailure) {
            std::fs::write(&path, serde_json::to_vec(&train)?)?;
            let (mut ordinary, job) = crate::pr_watch::run(
                &sandbox,
                "ordinary-infrastructure",
                head,
                RunState::Error,
                None,
                31,
            );
            ordinary.reference = CiRef::PullRequest(1481);
            cold.deps.store.insert_ci_run(&ordinary, &[job]).await?;
            let error = cold.watch_prs(&bounds).await;
            assert_eq!(error.len(), 1, "{error:?}");
            assert!(
                error[0].1.contains("ordinary-infrastructure")
                    && !error[0].1.contains("candidate-red")
            );
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
            let failed = cold.watch_prs(&bounds).await;
            let repeated = cold.watch_prs(&bounds).await;
            cold.stop_task_now(id).await?;
            assert_eq!(failed.len(), 1, "candidate after owner error: {failed:?}");
            assert!(
                failed[0]
                    .1
                    .contains("Merge candidate #1481, #99 (run candidate-red)")
                    && !failed[0].1.contains("ordinary-infrastructure")
            );
            assert!(repeated.is_empty());
            return Ok(());
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
            // The scheduler can reuse the same red tree for a different member list.
            let (entered, release) = super::pause_watch_or_queue(super::Point::LivePrFactsRead, id);
            let watcher = Arc::clone(&cold);
            let pass = tokio::spawn(async move { watcher.watch_prs(&bounds).await });
            tokio::time::timeout(Duration::from_secs(10), entered.notified()).await?;
            train[sandbox.as_str()]["train"]["members"][1]["pr"] = json!(101);
            std::fs::write(&path, serde_json::to_vec(&train)?)?;
            release.notify_one();
            let superseded = pass.await?;
            assert!(
                superseded.is_empty(),
                "outdated train membership: {superseded:?}"
            );
            let news = cold.watch_prs(&bounds).await;
            // Simulate the next due check-in after this candidate was already reported.
            let due = WatchBounds {
                check_in_after_ms: 0,
                stall_quiet_ms: 0,
                stall_after_ms: 0,
                ..WatchBounds::default()
            };
            let repeated = cold.watch_prs(&due).await;
            cold.stop_task_now(id).await?;
            assert!(obsolete.is_empty(), "outdated train status: {obsolete:?}");
            assert_eq!(news.len(), 1, "{news:?}");
            assert!(
                news[0].1.contains(
                    "Merge candidate #1481, #101 (run candidate-red), which holds it, failed"
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
        if matches!(case, ColdCandidate::HostedPullUnavailable) {
            let pull = remote
                .pulls
                .lock()
                .map_err(|_| "fixture pull")?
                .pop()
                .ok_or("open pull")?;
            let unseen = cold.watch_prs(&bounds).await;
            assert!(unseen.is_empty(), "unavailable hosted PR: {unseen:?}");
            remote.pulls.lock().map_err(|_| "fixture pull")?.push(pull);
        }
        if matches!(case, ColdCandidate::UpdateHold) {
            let (entered, release) = super::pause_watch_or_queue(super::Point::LivePrFactsRead, id);
            let watcher = Arc::clone(&cold);
            let pass = tokio::spawn(async move { watcher.watch_prs(&bounds).await });
            tokio::time::timeout(Duration::from_secs(10), entered.notified()).await?;
            let now = cold.now();
            cold.deps
                .store
                .insert_update_run(&yulius_app::updates::UpdateRunRecord {
                    id: "update-during-dispatch".into(),
                    target_commit: "b".repeat(40),
                    from_commit: None,
                    summary: "Restart".into(),
                    state: yulius_domain::update::RunState::Draining,
                    outcome: None,
                    error: None,
                    requested_by: "u-admin".into(),
                    created_at: now,
                    state_since: now,
                    finished_at: None,
                })
                .await?;
            cold.restore_update_hold().await;
            release.notify_one();
            let obsolete = pass.await?;
            let held = cold.watch_prs(&bounds).await;
            assert!(
                obsolete.is_empty() && held.is_empty(),
                "update drain: {obsolete:?} {held:?}"
            );
            let ctx = yulius_app::auth::AuthContext {
                user: cold
                    .deps
                    .store
                    .user(&yulius_domain::ids::UserId::parse("u-admin")?)
                    .await?
                    .ok_or("fixture owner")?,
                csrf_token: None,
                session_hash: None,
                token: None,
            };
            cold.cancel_update(&ctx).await?;
        }
        if matches!(case, ColdCandidate::ScopeDecision) {
            let (entered, release) = super::pause_watch_or_queue(super::Point::LivePrFactsRead, id);
            let watcher = Arc::clone(&cold);
            let pass = tokio::spawn(async move { watcher.watch_prs(&bounds).await });
            tokio::time::timeout(Duration::from_secs(10), entered.notified()).await?;
            cold.deps
                .store
                .insert_hold(&yulius_app::intake::HoldRecord {
                    id: "scope-review".into(),
                    task_id: id.clone(),
                    class: yulius_domain::intake::PromptClass::NewScope,
                    prompt: "Add a billing report".into(),
                    prompt_sha256: "0".repeat(64),
                    percent: 91,
                    typed_by: None,
                    state: yulius_app::intake::hold_state::PENDING.into(),
                    created_at: cold.now(),
                    resolved_by: None,
                    resolved_at: None,
                    new_task_id: None,
                })
                .await?;
            release.notify_one();
            let overtaken = pass.await?;
            let pending = cold.watch_prs(&bounds).await;
            assert!(
                overtaken.is_empty() && pending.is_empty(),
                "scope decision: {overtaken:?} {pending:?}"
            );
            assert!(
                cold.deps
                    .store
                    .resolve_hold(
                        "scope-review",
                        yulius_app::intake::hold_state::PENDING,
                        yulius_app::intake::hold_state::DISMISSED,
                        "fixture",
                        cold.now(),
                        None
                    )
                    .await?
            );
        }
        if matches!(
            case,
            ColdCandidate::RepairPushedDuringRead | ColdCandidate::ExternalRepair
        ) {
            let (entered, release) = super::pause_watch_or_queue(super::Point::LivePrFactsRead, id);
            let watcher = Arc::clone(&cold);
            let pass = tokio::spawn(async move { watcher.watch_prs(&bounds).await });
            tokio::time::timeout(Duration::from_secs(10), entered.notified()).await?;
            let task = cold.deps.store.task(id).await?.ok_or("owner")?;
            let tree = Path::new(task.worktree_path.as_deref().ok_or("checkout")?);
            let branch = task.branch.as_deref().ok_or("branch")?;
            let other = data.join("repair-clone");
            let tree = if matches!(case, ColdCandidate::ExternalRepair) {
                crate::pr_watch::git(
                    tree,
                    &["clone", "-q", ".", other.to_str().ok_or("clone path")?],
                )?;
                crate::pr_watch::git(
                    &other,
                    &[
                        "remote",
                        "set-url",
                        "origin",
                        origin.to_str().ok_or("bare origin")?,
                    ],
                )?;
                other.as_path()
            } else {
                tree
            };
            crate::pr_watch::git(
                tree,
                &[
                    "commit",
                    "-q",
                    "--allow-empty",
                    "-m",
                    "Repair the failed check",
                ],
            )?;
            crate::pr_watch::git(
                tree,
                &["push", "-q", "origin", &format!("HEAD:refs/heads/{branch}")],
            )?;
            if matches!(case, ColdCandidate::ExternalRepair) {
                let checkout = Path::new(task.worktree_path.as_deref().ok_or("owner checkout")?);
                assert_eq!(
                    crate::pr_watch::git(
                        checkout,
                        &["rev-parse", &format!("refs/remotes/origin/{branch}")]
                    )?,
                    head
                );
                assert_ne!(
                    crate::pr_watch::git(&origin, &["rev-parse", &format!("refs/heads/{branch}")])?,
                    head
                );
            }
            release.notify_one();
            let superseded = pass.await?;
            let repeat = cold.watch_prs(&bounds).await;
            cold.stop_task_now(id).await?;
            assert!(
                superseded.is_empty(),
                "advice for previous published head: {superseded:?}"
            );
            assert!(repeat.is_empty(), "{repeat:?}");
            return Ok(());
        }
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
            ..WatchBounds::default()
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
                train["members"].as_array_mut().ok_or("members")?.push(
                    json!({"task":"peer","pr":88,"head":"peer-head","armed_at":12,"tree":"peer-tree"})
                );
                std::fs::write(&path, serde_json::to_vec(&obsolete)?)?;
                let news = cold.watch_prs(&bounds).await;
                assert!(news.is_empty(), "stale member {field}: {news:?}");
            }
            let due = WatchBounds {
                check_in_after_ms: 0,
                stall_quiet_ms: 0,
                stall_after_ms: 0,
                ..WatchBounds::default()
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
            let backup = path.with_extension("saved");
            std::fs::rename(&path, &backup)?;
            std::fs::create_dir(&path)?;
            let unavailable = cold.watch_prs(&due).await;
            assert!(
                unavailable.is_empty(),
                "train registry I/O failure: {unavailable:?}"
            );
            std::fs::remove_dir(&path)?;
            std::fs::rename(backup, &path)?;
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
            let receipt = data.join(yulius_app::pr_watch::PR_NOTICES_FILE);
            let told: serde_json::Value = serde_json::from_slice(&std::fs::read(&receipt)?)?;
            assert_eq!(
                told[id.as_str()]["keys"]["green"],
                format!("{head}:held"),
                "a train member keeps its persisted notice identity across restart"
            );
            let task = cold.deps.store.task(id).await?.ok_or("owner")?;
            let path = data.join(yulius_app::merge_train::MERGE_TRAINS_FILE);
            let mut current: serde_json::Value = serde_json::from_slice(&std::fs::read(&path)?)?;
            current[task.sandbox_id.as_str()]["train"]["run"] = json!("earlier-bisection");
            let mut larger = current.clone();
            larger[task.sandbox_id.as_str()]["train"]["members"]
                .as_array_mut().ok_or("members")?
                .push(json!({"task":"peer","pr":99,"head":"peer-head","armed_at":30,"tree":"peer-tree"}));
            std::fs::write(&path, serde_json::to_vec(&larger)?)?;
            let larger_notice = cold.watch_prs(&bounds).await;
            assert_eq!(larger_notice.len(), 1, "{larger_notice:?}");
            assert!(
                larger_notice[0].1.contains("smaller candidates"),
                "{larger_notice:?}"
            );
            let heard: serde_json::Value = serde_json::from_slice(&std::fs::read(&receipt)?)?;
            assert_eq!(
                heard[id.as_str()]["keys"]["candidate"],
                "earlier-bisection",
                "a larger candidate keeps the notice key saved by previous releases"
            );
            assert!(cold.watch_prs(&bounds).await.is_empty());
            // The source run has not been attributed to this member yet. Its raw job
            // reason must stay out of the preliminary verification prompt.
            let private_reason = format!("synthetic-CHECK_KEY-{}", "x".repeat(4096));
            let connection = rusqlite::Connection::open(data.join("yulius.db"))?;
            assert_eq!(connection.execute("UPDATE ci_jobs SET exit_code = NULL, reason = ?1 WHERE run_id = 'earlier-bisection'", [&private_reason])?, 1);
            drop(connection);
            std::fs::write(&path, serde_json::to_vec(&current)?)?;
            let held = cold.watch_prs(&bounds).await;
            assert_eq!(held.len(), 1, "current failed singleton: {held:?}");
            assert!(
                held[0].1.contains("earlier-bisection")
                    && held[0].1.contains("verifying")
                    && !held[0].1.contains("smaller candidates"),
                "{held:?}"
            );
            assert!(
                !held[0].1.contains("synthetic-CHECK_KEY") && held[0].1.len() < 1024,
                "preliminary notice included the raw job reason"
            );
            assert!(cold.watch_prs(&bounds).await.is_empty());
            current[task.sandbox_id.as_str()]["train"]["run"] = json!("gone");
            rusqlite::Connection::open(data.join("yulius.db"))?.execute(
                "UPDATE ci_runs SET reference = 'pr/0' WHERE id = 'earlier-bisection'",
                [],
            )?;
            let mut undecodable = current.clone();
            undecodable[task.sandbox_id.as_str()]["train"]["run"] = json!("earlier-bisection");
            for (queued, (run_id, registry)) in [
                ("ordinary-red", serde_json::to_vec(&current)?),
                ("ordinary-red-rerun", b"[".to_vec()),
                (
                    "ordinary-red-after-candidate-error",
                    serde_json::to_vec(&undecodable)?,
                ),
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
    Err("the live queue case was not exercised".into())
}

async fn check_recorded_decision(
    data: &Path,
    case: &ColdCandidate,
    id: &TaskId,
    head: &str,
    cold: Arc<yulius_app::context::App>,
    remote: Arc<crate::support::FakeGitHub>,
) -> TestResult {
    let bounds = WatchBounds::default();
    let prior_notice = matches!(case, ColdCandidate::PriorNotice);
    if matches!(
        case,
        ColdCandidate::ReplacementConflict | ColdCandidate::ReplacementWithOldGreen
    ) {
        let task = cold.deps.store.task(id).await?.ok_or("task")?;
        let tree = Path::new(task.worktree_path.as_deref().ok_or("checkout")?);
        let seed = data.join("seed");
        for (checkout, contents) in [(tree, "replacement"), (seed.as_path(), "integration")] {
            std::fs::write(checkout.join("replacement-check"), contents)?;
            crate::pr_watch::git(checkout, &["add", "replacement-check"])?;
            crate::pr_watch::git(checkout, &["commit", "-qm", contents])?;
        }
        crate::pr_watch::git(&seed, &["push", "-q", "origin", "HEAD:dev"])?;
        let branch = task.branch.as_deref().ok_or("branch")?;
        crate::pr_watch::git(
            tree,
            &["push", "-q", "origin", &format!("HEAD:refs/heads/{branch}")],
        )?;
        crate::pr_watch::git(tree, &["fetch", "-q", "origin"])?;
        let new_head = crate::pr_watch::git(tree, &["rev-parse", "HEAD"])?;
        set_pull_head(&remote, 1481, &new_head)?;
        let news = Box::pin(cold.watch_prs(&bounds)).await;
        let repeated = Box::pin(cold.watch_prs(&bounds)).await;
        cold.stop_task_now(id).await?;
        assert_eq!(news.len(), 1, "replacement conflict: {news:?}");
        assert!(
            news[0].1.contains(&new_head[..9])
                && news[0].1.contains("conflicts with")
                && !news[0].1.contains("candidate-red")
                && !news[0].1.contains("passed its own"),
            "{news:?}"
        );
        assert!(repeated.is_empty(), "{repeated:?}");
        return Ok(());
    }
    if matches!(case, ColdCandidate::MovedHead) {
        let saved_remote = remote.remote.lock().map_err(|_| "remote")?.take();
        set_pull_head(&remote, 1481, "replacement")?;
        let obsolete = Box::pin(cold.watch_prs(&bounds)).await;
        assert!(
            obsolete.is_empty(),
            "green notice for the replaced head: {obsolete:?}"
        );
        set_pull_head(&remote, 1481, head)?;
        *remote.remote.lock().map_err(|_| "remote")? = saved_remote;
    }
    if matches!(
        case,
        ColdCandidate::OwnerRerun
            | ColdCandidate::NewerRunAfterExpiry
            | ColdCandidate::LateReceiptRerun
    ) {
        let late = matches!(case, ColdCandidate::LateReceiptRerun);
        let (entered, gate) = if late {
            let (entered, gate) = super::pause_watch_or_queue(super::Point::LivePrFactsRead, id);
            (Some(entered), gate)
        } else {
            let gate = Arc::new(tokio::sync::Notify::new());
            *remote.pull_read_hold.lock().map_err(|_| "PR gate")? = Some(gate.clone());
            (None, gate)
        };
        let watcher = cold.clone();
        let pass = tokio::spawn(async move { watcher.watch_prs(&WatchBounds::default()).await });
        let entered = entered.as_deref().unwrap_or(&remote.pull_read_entered);
        tokio::time::timeout(Duration::from_secs(10), entered.notified()).await?;
        let task = cold.deps.store.task(id).await?.ok_or("owner")?;
        let (mut rerun, job) = crate::pr_watch::run(
            &task.sandbox_id,
            "ordinary-rerun",
            head,
            RunState::Running,
            None,
            cold.now().0,
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
        typed.extend(Box::pin(cold.watch_prs(&bounds)).await);
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
        let stale = Box::pin(cold.watch_prs(&bounds)).await;
        assert!(
            stale
                .iter()
                .all(|(_, text)| !text.contains("Do not re-arm")),
            "obsolete source after a base change: {stale:?}"
        );
        *remote.remote.lock().map_err(|_| "remote")? = saved_remote;
    }
    if prior_notice {
        Box::pin(check_changed_owner_inputs(data, id, head, &cold, &remote)).await?;
    }
    let notices = Box::pin(cold.watch_prs(&bounds)).await;
    let repeated = Box::pin(cold.watch_prs(&bounds)).await;
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
        // A later stall check-in requires an idle turn. The first watch returns once its
        // diagnosis is typed, before the real PTY agent necessarily finishes answering it.
        super::wait_for(
            "the candidate diagnosis to be answered",
            Duration::from_secs(30),
            || async {
                super::screen(&cold, id).contains("ECHO: Yulius, about your pull request #1481")
                    && super::status_of(&cold, id).await.1
                        == Some(yulius_domain::task::Activity::Waiting)
            },
        )
        .await?;
        let bounds = WatchBounds {
            stall_quiet_ms: 0,
            stall_after_ms: 0,
            ..WatchBounds::default()
        };
        let stalled = Box::pin(cold.watch_prs(&bounds)).await;
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

/// A previously notified author's old green status cannot validate the trained head.
async fn check_changed_owner_inputs(
    data: &Path,
    id: &TaskId,
    head: &str,
    cold: &Arc<yulius_app::context::App>,
    remote: &Arc<crate::support::FakeGitHub>,
) -> TestResult {
    let bounds = WatchBounds::default();
    let origin = data.join("origin.git");
    // The remote base advanced while the daemon's origin/dev still names the tested base.
    let seed = data.join("seed");
    let base = crate::pr_watch::git(&origin, &["rev-parse", "dev"])?;
    std::fs::write(seed.join("changed-base"), b"new integration tree")?;
    crate::pr_watch::git(&seed, &["add", "changed-base"])?;
    crate::pr_watch::git(&seed, &["commit", "-qm", "Advance integration"])?;
    crate::pr_watch::git(&seed, &["push", "-q", "origin", "HEAD:dev"])?;
    let obsolete = Box::pin(cold.watch_prs(&bounds)).await;
    crate::pr_watch::git(&origin, &["update-ref", "refs/heads/dev", &base])?;
    assert!(
        obsolete.is_empty(),
        "superseded integration tree: {obsolete:?}"
    );
    let saved_remote = remote.remote.lock().map_err(|_| "remote")?.take();
    let mut original_pull = remote.pull(1481).ok_or("owner PR")?;
    original_pull.base_sha = base;
    let changes: [fn(&mut yulius_app::ports::github::GhPull); 7] = [
        |pull| pull.base = "release".into(),
        |pull| pull.head_sha = "superseding-head".into(),
        |pull| pull.head = "feat/another-owner".into(),
        |pull| pull.from_fork = true,
        |pull| pull.merged = true,
        |pull| pull.state = "closed".into(),
        |pull| pull.base_sha.clear(),
    ];
    for change in changes {
        let mut other = original_pull.clone();
        change(&mut other);
        remote.pulls.lock().map_err(|_| "fixture pull")?[0] = other.clone();
        assert!(
            Box::pin(cold.watch_prs(&bounds)).await.is_empty(),
            "unverified owner PR: {other:?}"
        );
    }
    remote.pulls.lock().map_err(|_| "fixture pull")?[0] = original_pull.clone();
    let (entered, gate) = super::pause_watch_or_queue(super::Point::ArmedNoticePublicationRead, id);
    let watcher = cold.clone();
    let pass = tokio::spawn(async move { watcher.watch_prs(&WatchBounds::default()).await });
    tokio::time::timeout(Duration::from_secs(5), entered.notified()).await?;
    remote.pulls.lock().map_err(|_| "fixture pull")?[0].base_sha = "new-base".into();
    gate.notify_one();
    assert!(
        tokio::time::timeout(Duration::from_secs(10), pass)
            .await??
            .is_empty()
    );
    remote.pulls.lock().map_err(|_| "fixture pull")?[0] = original_pull;
    *remote.remote.lock().map_err(|_| "remote")? = saved_remote;
    // Retirement can land after the watcher prepares the diagnosis and before it
    // obtains the original writer. The next pass must also leave it unheard.
    let span = cold
        .deps
        .store
        .latest_merge_wait(id)
        .await?
        .ok_or("recorded decision")?;
    let (entered, gate) = super::pause_watch_or_queue(super::Point::ArmedNoticePublicationRead, id);
    let watcher = cold.clone();
    let pass = tokio::spawn(async move { watcher.watch_prs(&WatchBounds::default()).await });
    tokio::time::timeout(Duration::from_secs(5), entered.notified()).await?;
    assert!(
        cold.deps
            .store
            .retire_singleton(id, &span, cold.now())
            .await?
    );
    gate.notify_one();
    assert!(
        tokio::time::timeout(Duration::from_secs(10), pass)
            .await??
            .is_empty()
    );
    assert!(Box::pin(cold.watch_prs(&bounds)).await.is_empty());
    let db = rusqlite::Connection::open(data.join("yulius.db"))?;
    assert_eq!(
        db.execute(
            "UPDATE spans SET detail=?1 WHERE subject_id=?2 AND key=?3",
            rusqlite::params![span.detail, id.as_str(), span.key]
        )?,
        1
    );
    drop(db);
    let arms = data.join(yulius_app::agent_wait::ARMED_MERGES_FILE);
    for (rearmed, final_read) in [(false, false), (true, false), (true, true)] {
        let (entered, gate) = if final_read {
            let (entered, gate) =
                super::pause_watch_or_queue(super::Point::ArmedNoticePublicationRead, id);
            (Some(entered), gate)
        } else {
            let gate = Arc::new(tokio::sync::Notify::new());
            *remote.pull_read_hold.lock().map_err(|_| "PR gate")? = Some(gate.clone());
            (None, gate)
        };
        let watcher = cold.clone();
        let pass = tokio::spawn(async move { watcher.watch_prs(&WatchBounds::default()).await });
        let entered = tokio::time::timeout(
            Duration::from_secs(5),
            entered
                .as_deref()
                .unwrap_or(&remote.pull_read_entered)
                .notified(),
        )
        .await;
        if rearmed {
            std::fs::write(
                &arms,
                serde_json::to_vec(
                    &json!([{"task":id.as_str(),"head":head,"summary":"approved"}]),
                )?,
            )?;
        } else {
            ask(cold, id).await?;
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
    Ok(())
}
