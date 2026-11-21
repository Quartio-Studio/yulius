//! Original-owner PR notifications across restart with real Git, SQLite and PTYs.
use super::{Error, answered, app_with_pulls, ask, data_dir, open_pull_at, runtime};
use crate::support::TestResult;
use serde_json::json;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;
use yulius_app::delivery::DeliveryRecord;
use yulius_app::pr_watch::WatchBounds;
use yulius_domain::ci::{CiRef, RunState};
use yulius_domain::delivery::DeliveryStage;

#[test]
fn an_armed_notice_preserves_a_person_decision_arriving_at_final_verification() -> TestResult {
    armed_owner_case(OwnerCase::HeldHumanDecision)
}

#[test]
fn a_repair_pushed_during_dispatch_suppresses_the_old_ordinary_failure() -> TestResult {
    armed_owner_case(OwnerCase::RepairPushedDuringRead)
}

#[test]
fn a_repair_published_by_another_clone_during_dispatch_suppresses_stale_advice() -> TestResult {
    armed_owner_case(OwnerCase::ExternalRepair(super::Point::LivePrFactsRead))
}

#[test]
fn an_external_repair_during_the_local_publication_lookup_keeps_the_failure_unheard() -> TestResult
{
    armed_owner_case(OwnerCase::ExternalRepair(
        super::Point::ArmedNoticePublicationRead,
    ))
}

#[test]
fn an_external_repair_while_chromium_restarts_keeps_old_ci_unheard() -> TestResult {
    armed_owner_case(OwnerCase::BrowserRestart)
}

#[test]
fn an_account_switch_during_armed_verification_leaves_the_notice_for_the_new_attempt() -> TestResult
{
    armed_owner_case(OwnerCase::ReplacedAttempt(
        super::Point::ArmedNoticePublicationRead,
    ))
}

#[test]
fn a_replacement_after_a_pr_paste_does_not_receive_the_original_attempts_enter() -> TestResult {
    armed_owner_case(OwnerCase::ReplacedAttempt(super::Point::PrNoticePasted))
}

#[test]
fn an_armed_owner_with_a_durable_scope_decision_receives_ci_after_resolution() -> TestResult {
    armed_owner_case(OwnerCase::ScopeDecision)
}

#[test]
fn an_unverified_hosted_pull_keeps_its_ordinary_failure_unheard_until_recovery() -> TestResult {
    armed_owner_case(OwnerCase::HostedPullUnavailable)
}

#[test]
fn an_update_hold_arriving_before_armed_dispatch_keeps_the_notice_for_release() -> TestResult {
    armed_owner_case(OwnerCase::UpdateHold)
}

#[test]
fn an_ordinary_notice_from_a_previous_sha_waits_for_the_armed_heads_result() -> TestResult {
    armed_owner_case(OwnerCase::PreviousCiHead)
}

#[test]
fn a_turn_beginning_during_the_hosted_read_suppresses_an_idle_check_in() -> TestResult {
    armed_owner_case(OwnerCase::WorkingDuringCheckIn)
}

#[test]
fn a_turn_beginning_during_the_hosted_read_suppresses_a_stall() -> TestResult {
    armed_owner_case(OwnerCase::WorkingDuringStall)
}

#[test]
fn a_terminal_permission_prompt_during_publication_verification_keeps_the_notice_for_its_owner()
-> TestResult {
    armed_owner_case(OwnerCase::TerminalPermission)
}

enum OwnerCase {
    TerminalPermission,
    WorkingDuringCheckIn,
    WorkingDuringStall,
    PreviousCiHead,
    ExternalRepair(super::Point),
    BrowserRestart,
    ReplacedAttempt(super::Point),
    HeldHumanDecision,
    HostedPullUnavailable,
    RepairPushedDuringRead,
    ScopeDecision,
    UpdateHold,
}

fn armed_owner_case(case: OwnerCase) -> TestResult {
    let (_dir, data) = data_dir()?;
    let rt = runtime()?;
    let (id, head, original) = rt.block_on(async {
        let (app, github) = app_with_pulls(&data, 8, 1).await?;
        let (id, caller) = answered(&app, &data).await?;
        let task = app.deps.store.task(&id).await?.ok_or("task")?;
        let tree = Path::new(task.worktree_path.as_deref().ok_or("checkout")?);
        crate::pr_watch::git(&data.join("seed"), &["push", "-q", "origin", "main:dev"])?;
        crate::pr_watch::git(tree, &["fetch", "-q", "origin"])?;
        let branch = task.branch.as_deref().ok_or("branch")?;
        crate::pr_watch::git(
            tree,
            &["push", "-q", "origin", &format!("HEAD:refs/heads/{branch}")],
        )?;
        let head = crate::pr_watch::git(tree, &["rev-parse", "HEAD"])?;
        open_pull_at(&app, &github, &id, 1481, &head).await?;
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
        let sha = if matches!(case, OwnerCase::PreviousCiHead) {
            "previous-sha"
        } else {
            &head
        };
        let (mut run, job) = crate::pr_watch::run(
            &caller.sandbox,
            "ordinary-red",
            sha,
            if matches!(
                case,
                OwnerCase::WorkingDuringCheckIn | OwnerCase::WorkingDuringStall
            ) {
                RunState::Cancelled
            } else {
                RunState::Failed
            },
            None,
            31,
        );
        run.reference = CiRef::PullRequest(1481);
        app.deps.store.insert_ci_run(&run, &[job]).await?;
        let original = app
            .deps
            .store
            .live_attempt(&id)
            .await?
            .ok_or("original attempt")?
            .id;
        Ok::<_, Error>((id, head, original))
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    let rt = runtime()?;
    rt.block_on(async {
        let id = &id;
        let (cold, remote) = app_with_pulls(&data, 8, 1).await?;
        let browser = Arc::new(crate::delivery::HeldBrowserLookup::for_publication());
        let cold = if matches!(case, OwnerCase::BrowserRestart) {
            let mut deps = cold.deps.clone();
            deps.browser = browser.clone();
            yulius_app::context::App::new(cold.config.clone(), deps)
        } else {
            cold
        };
        let origin = data.join("origin.git");
        remote.set_remote(Arc::new(crate::delivery::BareRemote {
            origin: origin.clone(),
        }));
        open_pull_at(&cold, &remote, id, 1481, &head).await?;
        std::fs::write(
            data.join(yulius_app::agent_wait::ARMED_MERGES_FILE),
            serde_json::to_vec(
                &json!([{"task":id.as_str(),"head":head,"summary":"reviewed","armed_at":30}]),
            )?,
        )?;
        cold.reconcile().await?;
        cold.restore_armed_merges().await;
        assert_eq!(
            cold.deps
                .store
                .live_attempt(id)
                .await?
                .ok_or("restored original attempt")?
                .id,
            original
        );
        let bounds = WatchBounds::default();
        if matches!(
            case,
            OwnerCase::WorkingDuringCheckIn | OwnerCase::WorkingDuringStall
        ) {
            let due = WatchBounds {
                check_in_after_ms: 0,
                stall_after_ms: if matches!(case, OwnerCase::WorkingDuringStall) {
                    0
                } else {
                    i64::MAX
                },
                stall_quiet_ms: 0,
                ..WatchBounds::default()
            };
            let release = Arc::new(tokio::sync::Notify::new());
            *remote.pull_read_hold.lock().map_err(|_| "fixture host")? = Some(release.clone());
            let watcher = cold.clone();
            let pass = tokio::spawn(async move { watcher.watch_prs(&due).await });
            tokio::time::timeout(Duration::from_secs(10), remote.pull_read_entered.notified())
                .await?;
            // Exercise the real turn hook with a synthetic bearer owned by this fixture attempt.
            let token = "fixture-owner-turn";
            rusqlite::Connection::open(data.join("yulius.db"))?.execute(
                "UPDATE attempts SET token_hash = ?1 WHERE id = ?2",
                rusqlite::params![
                    cold.deps.crypto.sha256_hex(token.as_bytes()),
                    original.as_str()
                ],
            )?;
            cold.hook_event(
                token,
                "claude",
                "UserPromptSubmit",
                &yulius_contracts::hooks::HookInput {
                    prompt: Some("Continue the review".into()),
                    ..Default::default()
                },
                None,
            )
            .await?;
            assert_eq!(
                cold.deps.store.task(id).await?.ok_or("owner")?.activity,
                Some(yulius_domain::task::Activity::Working),
                "the real prompt hook began the turn"
            );
            *remote.pull_read_hold.lock().map_err(|_| "fixture host")? = None;
            release.notify_one();
            let news = pass.await?;
            assert!(news.is_empty(), "idle advice after turn began: {news:?}");
            let task = cold.deps.store.task(id).await?.ok_or("owner")?;
            let (mut run, job) = crate::pr_watch::run(
                &task.sandbox_id,
                "ordinary-red-current",
                &head,
                RunState::Failed,
                None,
                32,
            );
            run.reference = CiRef::PullRequest(1481);
            cold.deps.store.insert_ci_run(&run, &[job]).await?;
        }
        if let OwnerCase::ReplacedAttempt(point) = case {
            let replacement = crate::pool_supervision::add_account(&cold, "b-replacement").await?;
            let (entered, release) = super::pause_watch_or_queue(point, id);
            let watcher = Arc::clone(&cold);
            let pass = tokio::spawn(async move { watcher.watch_prs(&bounds).await });
            tokio::time::timeout(Duration::from_secs(10), entered.notified()).await?;
            cold.switch_task_now(
                id,
                Some(replacement),
                "manual",
                Some("Replacement session ready".into()),
            )
            .await?;
            super::wait_for(
                "the new attempt to attach",
                Duration::from_secs(10),
                || async {
                    cold.deps
                        .store
                        .live_attempt(id)
                        .await
                        .ok()
                        .flatten()
                        .is_some_and(|a| a.id != original)
                        && super::screen(&cold, id).contains("Replacement session ready")
                        && super::status_of(&cold, id).await.1
                            == Some(yulius_domain::task::Activity::Waiting)
                },
            )
            .await?;
            release.notify_one();
            let stale = pass.await?;
            assert!(
                stale.is_empty(),
                "watch of the previous attempt typed into its replacement: {stale:?}"
            );
        }
        if matches!(case, OwnerCase::TerminalPermission) {
            let token = "fixture-permission-hook";
            rusqlite::Connection::open(data.join("yulius.db"))?.execute(
                "UPDATE attempts SET token_hash = ?1 WHERE id = ?2",
                rusqlite::params![
                    cold.deps.crypto.sha256_hex(token.as_bytes()),
                    original.as_str()
                ],
            )?;
            let (entered, release) =
                super::pause_watch_or_queue(super::Point::ArmedNoticePublicationRead, id);
            let watcher = cold.clone();
            let pass = tokio::spawn(async move { watcher.watch_prs(&bounds).await });
            tokio::time::timeout(Duration::from_secs(10), entered.notified()).await?;
            cold.hook_event(
                token,
                "claude",
                "Notification",
                &yulius_contracts::hooks::HookInput {
                    notification_type: Some("permission_prompt".into()),
                    message: Some("Allow this operation?".into()),
                    ..Default::default()
                },
                None,
            )
            .await?;
            assert_eq!(
                cold.deps.store.task(id).await?.ok_or("owner")?.activity,
                Some(yulius_domain::task::Activity::NeedsAttention)
            );
            release.notify_one();
            let news = pass.await?;
            assert!(
                news.is_empty(),
                "CI entered an active permission prompt: {news:?}"
            );
            assert!(cold.watch_prs(&bounds).await.is_empty());
            cold.hook_event(
                token,
                "claude",
                "UserPromptSubmit",
                &yulius_contracts::hooks::HookInput {
                    prompt: Some("Continue".into()),
                    ..Default::default()
                },
                None,
            )
            .await?;
        }
        if matches!(case, OwnerCase::PreviousCiHead) {
            let previous = cold.watch_prs(&bounds).await;
            assert!(
                previous.is_empty(),
                "advice from prior CI head: {previous:?}"
            );
            let task = cold.deps.store.task(id).await?.ok_or("owner")?;
            let (mut run, job) = crate::pr_watch::run(
                &task.sandbox_id,
                "ordinary-red-current",
                &head,
                RunState::Failed,
                None,
                32,
            );
            run.reference = CiRef::PullRequest(1481);
            cold.deps.store.insert_ci_run(&run, &[job]).await?;
        }
        if matches!(case, OwnerCase::HostedPullUnavailable) {
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
        if matches!(case, OwnerCase::UpdateHold) {
            let (entered, release) =
                super::pause_watch_or_queue(super::Point::ArmedNoticePublicationRead, id);
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
            let read_while_held = Arc::new(std::sync::atomic::AtomicBool::new(false));
            let observed = read_while_held.clone();
            yulius_app::race::on(
                super::Point::LivePrFactsRead,
                Path::new(id.as_str()),
                move || {
                    observed.store(true, std::sync::atomic::Ordering::SeqCst);
                },
            );
            let held = cold.watch_prs(&bounds).await;
            assert!(
                !read_while_held.load(std::sync::atomic::Ordering::SeqCst),
                "a draining instance must skip the live PR read"
            );

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
        if matches!(case, OwnerCase::ScopeDecision) {
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
            OwnerCase::RepairPushedDuringRead
                | OwnerCase::ExternalRepair(_)
                | OwnerCase::BrowserRestart
        ) {
            let (entered, release) = match case {
                OwnerCase::ExternalRepair(point) => super::pause_watch_or_queue(point, id),
                OwnerCase::BrowserRestart => browser.hold_publication_lookup(id),
                _ => super::pause_watch_or_queue(super::Point::LivePrFactsRead, id),
            };
            if matches!(case, OwnerCase::BrowserRestart) {
                use futures::FutureExt;
                let sandbox = cold.deps.store.task(id).await?.ok_or("owner")?.sandbox_id;
                for _ in 0..2 {
                    assert!(
                        cold.deps.browser.running(&sandbox).now_or_never().is_some(),
                        "unrelated browser reads must not consume the owner's publication gate"
                    );
                }
            }
            let watcher = Arc::clone(&cold);
            let pass = tokio::spawn(async move { watcher.watch_prs(&bounds).await });
            tokio::time::timeout(Duration::from_secs(10), entered.notified()).await?;
            let task = cold.deps.store.task(id).await?.ok_or("owner")?;
            let tree = Path::new(task.worktree_path.as_deref().ok_or("checkout")?);
            let branch = task.branch.as_deref().ok_or("branch")?;
            let other = data.join("repair-clone");
            let tree = if matches!(
                case,
                OwnerCase::ExternalRepair(_) | OwnerCase::BrowserRestart
            ) {
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
            if matches!(
                case,
                OwnerCase::ExternalRepair(_) | OwnerCase::BrowserRestart
            ) {
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
        if matches!(case, OwnerCase::HeldHumanDecision) {
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
        let news = cold.watch_prs(&bounds).await;
        let repeated = cold.watch_prs(&bounds).await;
        cold.stop_task_now(id).await?;
        assert_eq!(news.len(), 1, "{news:?}");
        assert!(news[0].1.contains("ordinary-red"), "{news:?}");
        assert!(repeated.is_empty(), "{repeated:?}");
        Ok::<_, Error>(())
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

#[test]
fn an_armed_pr_without_an_ordinary_run_checks_conflicts_at_its_published_head() -> TestResult {
    use crate::pr_watch::git;
    let (_dir, data) = data_dir()?;
    let rt = runtime()?;
    rt.block_on(async {
        let (app, github) = app_with_pulls(&data, 8, 1).await?;
        let (id, _) = answered(&app, &data).await?;
        let task = app.deps.store.task(&id).await?.ok_or("owner")?;
        let tree = Path::new(task.worktree_path.as_deref().ok_or("checkout")?);
        let branch = task.branch.as_deref().ok_or("branch")?;
        std::fs::write(tree.join("README.md"), "task change\n")?;
        git(tree, &["commit", "-qam", "Original task change"])?;
        git(
            tree,
            &["push", "-q", "origin", &format!("HEAD:refs/heads/{branch}")],
        )?;
        let stale = git(tree, &["rev-parse", "HEAD"])?;
        let seed = data.join("seed");
        std::fs::write(seed.join("README.md"), "integration change\n")?;
        git(&seed, &["commit", "-qam", "Conflicting integration change"])?;
        git(&seed, &["push", "-q", "origin", "HEAD:dev"])?;
        git(tree, &["fetch", "-q", "origin"])?;
        assert!(git(tree, &["merge-tree", "--write-tree", &stale, "origin/dev"]).is_err());
        let clone = data.join("integration-repair");
        let origin = data.join("origin.git");
        git(
            &data,
            &[
                "clone",
                "-q",
                "-b",
                branch,
                origin.to_str().ok_or("bare path")?,
                clone.to_str().ok_or("repair path")?,
            ],
        )?;
        git(
            &clone,
            &["merge", "-q", "-X", "theirs", "--no-edit", "origin/dev"],
        )?;
        git(&clone, &["push", "-q", "origin", branch])?;
        let repaired = git(&clone, &["rev-parse", "HEAD"])?;
        assert_eq!(
            git(tree, &["rev-parse", &format!("origin/{branch}")])?,
            stale
        );
        open_pull_at(&app, &github, &id, 1481, &repaired).await?;
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
        std::fs::write(
            data.join(yulius_app::agent_wait::ARMED_MERGES_FILE),
            serde_json::to_vec(
                &json!([{"task":id.as_str(),"head":repaired,"summary":"reviewed","armed_at":30}]),
            )?,
        )?;
        app.restore_armed_merges().await;
        let news = app.watch_prs(&WatchBounds::default()).await;
        app.stop_task_now(&id).await?;
        assert!(news.is_empty(), "obsolete checkout conflict: {news:?}");
        Ok::<_, Error>(())
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}

#[test]
fn a_decision_during_draft_readiness_keeps_the_armed_notice_unheard() -> TestResult {
    use crate::first_message::ShiftedClock;
    use crate::pr_watch::git;
    use std::sync::atomic::{AtomicI64, Ordering};
    let (_dir, data) = data_dir()?;
    let rt = runtime()?;
    rt.block_on(async {
        let fake = yulius_ptyhost::test_support::locate_fake_agent().ok_or("fake agent")?;
        let clock = Arc::new(ShiftedClock(AtomicI64::new(-13 * 60_000)));
        let (app, _, github) = crate::first_message::start_hosted_with_github(
            &data,
            &fake,
            crate::task_message::full_screen(),
            (0, Arc::new(super::ScriptedUsage::default()), clock.clone()),
            None,
            |_| {},
        )
        .await?;
        let (id, caller) = answered(&app, &data).await?;
        let task = app.deps.store.task(&id).await?.ok_or("owner")?;
        let tree = Path::new(task.worktree_path.as_deref().ok_or("checkout")?);
        let head = git(tree, &["rev-parse", "HEAD"])?;
        open_pull_at(&app, &github, &id, 1481, &head).await?;
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
        let (mut run, job) = crate::pr_watch::run(
            &task.sandbox_id,
            "guarded-red",
            &head,
            RunState::Failed,
            None,
            31,
        );
        run.reference = CiRef::PullRequest(1481);
        app.deps.store.insert_ci_run(&run, &[job]).await?;
        std::fs::write(
            data.join(yulius_app::agent_wait::ARMED_MERGES_FILE),
            serde_json::to_vec(
                &json!([{"task":id.as_str(),"head":head,"summary":"reviewed","armed_at":30}]),
            )?,
        )?;
        app.restore_armed_merges().await;
        let (ctx, attach) = crate::task_message::full_screen_terminal(&app, &caller, &id).await?;
        app.terminal_input(&ctx, &attach, b"\x7f", false).await?;
        clock.0.store(-11 * 60_000, Ordering::SeqCst);
        let (entered, release) = super::pause_watch_or_queue(super::Point::BoxSeenEmpty, &id);
        let watcher = app.clone();
        let pass = tokio::spawn(async move { watcher.watch_prs(&WatchBounds::default()).await });
        tokio::time::timeout(Duration::from_secs(10), entered.notified()).await?;
        ask(&app, &id).await?;
        release.notify_one();
        let superseded = pass.await?;
        assert!(
            superseded.is_empty(),
            "CI pasted past the owner's decision: {superseded:?}"
        );
        assert!(app.watch_prs(&WatchBounds::default()).await.is_empty());
        app.deps
            .store
            .set_task_needs_you(&id, None, app.now())
            .await?;
        let news = app.watch_prs(&WatchBounds::default()).await;
        assert_eq!(news.len(), 1, "{news:?}");
        assert!(news[0].1.contains("guarded-red"));
        assert!(app.watch_prs(&WatchBounds::default()).await.is_empty());
        app.terminal_leave(&attach).await;
        app.stop_task_now(&id).await?;
        Ok::<_, Error>(())
    })?;
    rt.shutdown_timeout(Duration::from_secs(5));
    Ok(())
}
