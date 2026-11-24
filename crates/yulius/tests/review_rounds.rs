//! The independent code review's rounds as Yulius sees them through a task's hooks (SPEC-493
//! REV-002 to REV-004): the Agent tool's `PreToolUse` numbers a round and picks its reviewer,
//! `SubagentStart` and `SubagentStop` link and end it, and a merge closes a pull request's
//! rounds.

use crate::subscription_pool::{add_account, hook, running_task, seed};
use crate::support::{TestResult, harness};

use serde_json::{Value, json};
use yulius_app::delivery::DeliveryRecord;
use yulius_app::model::{ReviewRoundRecord, TaskMode};
use yulius_app::ports::store::{CoordinationStore, TaskStore as _};
use yulius_app::ports::store_ext::DeliveryStore;
use yulius_domain::delivery::DeliveryStage;
use yulius_domain::ids::TaskId;
use yulius_domain::time::Timestamp;

/// A round as the test reads it: number, reviewer, why, subagent, verdict.
type Seen<'a> = (u32, &'a str, &'a str, Option<&'a str>, Option<&'a str>);

/// A merge of `head` whose base is not known (only an approval of that head counts).
fn merged_at(head: &str) -> yulius_app::review::MergedHead {
    yulius_app::review::MergedHead {
        head: head.to_owned(),
        base: None,
        landing: None,
    }
}

/// The Agent tool's `PreToolUse` input for a subagent of `kind`.
fn agent_start(kind: &str) -> Value {
    json!({"session_id": "s", "hook_event_name": "PreToolUse", "tool_name": "Agent",
        "tool_input": {"description": "Review", "prompt": "Review origin/dev...HEAD against the spec.",
            "subagent_type": kind, "run_in_background": true},
        "tool_use_id": "toolu_1"})
}

/// The reviewer the answer runs: the one asked for when it changes nothing.
fn reviewer_run(answer: &Value, asked: &str) -> String {
    answer["hookSpecificOutput"]["updatedInput"]["subagent_type"]
        .as_str()
        .unwrap_or(asked)
        .to_owned()
}

async fn rounds(
    h: &crate::support::Harness,
    task: &TaskId,
    open_only: bool,
) -> Result<Vec<ReviewRoundRecord>, Box<dyn std::error::Error>> {
    Ok(h.store.review_rounds(task, open_only).await?)
}

/// Starts a reviewer as the task asks and, unless `cut_off`, lets it report `report`. Returns
/// the reviewer it ran as.
async fn a_round(
    h: &crate::support::Harness,
    id: &str,
    report: Option<&str>,
) -> Result<String, Box<dyn std::error::Error>> {
    let (code, _, answer) = hook(&h.internal, "PreToolUse", agent_start("yulius-reviewer")).await?;
    assert_eq!(code, 200, "{answer}");
    let ran = reviewer_run(&answer, "yulius-reviewer");
    let (code, _, _) = hook(
        &h.internal,
        "SubagentStart",
        json!({"hook_event_name": "SubagentStart", "agent_id": id, "agent_type": ran}),
    )
    .await?;
    assert_eq!(code, 200);
    if let Some(report) = report {
        let (code, _, _) = hook(
            &h.internal,
            "SubagentStop",
            json!({"hook_event_name": "SubagentStop", "agent_id": id, "agent_type": ran,
                "last_assistant_message": report}),
        )
        .await?;
        assert_eq!(code, 200);
    }
    Ok(ran)
}

/// A first start can clear recovery only if it actually linked to a launched review.
#[tokio::test]
async fn a_first_reviewer_start_is_active_only_when_it_links_its_launch() -> TestResult {
    use yulius_app::ports::store::ReviewStart;
    let h = harness()?;
    seed(&h).await?;
    let account = add_account(&h, "first-review-start", None).await?;
    let task = running_task(&h, TaskMode::Prompt, &account).await?;
    let reviewer = "yulius-reviewer";
    let start = || {
        h.store
            .start_review_subagent(&task, reviewer, "rev", "continued", h.app.now())
    };
    assert_eq!(start().await?, ReviewStart::Ignored);
    hook(&h.internal, "PreToolUse", agent_start(reviewer)).await?;
    assert_eq!(start().await?, ReviewStart::Active);
    Ok(())
}

/// SPEC-493 REV-002, REV-004 (AC-5, AC-6): rounds 1 and 2 of a pull request run the Fable
/// reviewer as asked; the third is given the Opus reviewer as the tool's new input, with the
/// rest of the input kept. A reviewer started again for a round that ended with no report (cut
/// off) keeps its number and reviewer. Each round is recorded with its reviewer, model, head,
/// subagent and verdict. A merge closes the rounds, and the task's next pull request starts at
/// round 1. Another subagent is not touched.
#[tokio::test]
async fn the_third_round_of_a_pull_request_runs_the_opus_reviewer() -> TestResult {
    let h = harness()?;
    seed(&h).await?;
    let account = add_account(&h, "acc-1", None).await?;
    let task = running_task(&h, TaskMode::Prompt, &account).await?;

    // Another subagent: nothing answered, nothing recorded.
    let (code, _, answer) = hook(&h.internal, "PreToolUse", agent_start("Explore")).await?;
    assert_eq!(code, 200);
    assert!(answer.is_null() || answer == json!({}), "{answer}");
    assert!(rounds(&h, &task, false).await?.is_empty());

    // Round 1: the context says which round and model; nothing is changed.
    let (_, _, answer) = hook(&h.internal, "PreToolUse", agent_start("yulius-reviewer")).await?;
    let said = answer["hookSpecificOutput"]["additionalContext"]
        .as_str()
        .unwrap_or_default();
    assert!(
        said.contains("review round 1") && said.contains("Fable 5.2"),
        "{answer}"
    );
    assert!(answer["hookSpecificOutput"]["updatedInput"].is_null());
    hook(
        &h.internal,
        "SubagentStart",
        json!({"agent_id": "rev-1", "agent_type": "yulius-reviewer"}),
    )
    .await?;
    hook(
        &h.internal,
        "SubagentStop",
        json!({"agent_id": "rev-1", "agent_type": "yulius-reviewer",
            "last_assistant_message": "Findings…\nVerdict: request changes"}),
    )
    .await?;

    // Round 2 is cut off (no report) and its reviewer started again: still round 2, Fable.
    assert_eq!(a_round(&h, "rev-2", None).await?, "yulius-reviewer");
    assert_eq!(
        a_round(&h, "rev-2b", Some("x\nVerdict: request changes")).await?,
        "yulius-reviewer"
    );
    // Round 3: the Opus reviewer, the rest of the input as asked.
    let (_, _, answer) = hook(&h.internal, "PreToolUse", agent_start("yulius-reviewer")).await?;
    let updated = &answer["hookSpecificOutput"]["updatedInput"];
    assert_eq!(updated["subagent_type"], "yulius-reviewer-opus", "{answer}");
    assert_eq!(
        updated["prompt"],
        "Review origin/dev...HEAD against the spec."
    );
    assert_eq!(updated["run_in_background"], true);
    assert_eq!(answer["hookSpecificOutput"]["permissionDecision"], "allow");
    assert!(
        answer["hookSpecificOutput"]["additionalContext"]
            .as_str()
            .is_some_and(|s| s.contains("review round 3") && s.contains("Opus 5.5")),
        "{answer}"
    );
    hook(
        &h.internal,
        "SubagentStart",
        json!({"agent_id": "rev-3", "agent_type": "yulius-reviewer-opus"}),
    )
    .await?;
    hook(
        &h.internal,
        "SubagentStop",
        json!({"agent_id": "rev-3", "agent_type": "yulius-reviewer-opus",
            "last_assistant_message": "All good.\n\n**Verdict: approve**"}),
    )
    .await?;

    let open = rounds(&h, &task, true).await?;
    let seen: Vec<Seen<'_>> = open
        .iter()
        .map(|r| {
            (
                r.round,
                r.agent.as_str(),
                r.reason.as_str(),
                r.subagent_id.as_deref(),
                r.verdict.as_deref(),
            )
        })
        .collect();
    assert_eq!(
        seen,
        [
            (
                1,
                "yulius-reviewer",
                "round",
                Some("rev-1"),
                Some("request_changes")
            ),
            (2, "yulius-reviewer", "round", Some("rev-2"), None),
            (
                2,
                "yulius-reviewer",
                "kept",
                Some("rev-2b"),
                Some("request_changes")
            ),
            (
                3,
                "yulius-reviewer-opus",
                "round",
                Some("rev-3"),
                Some("approve")
            ),
        ]
    );
    let models: Vec<&str> = open.iter().map(|r| r.model.as_str()).collect();
    assert_eq!(
        models,
        [
            "claude-fable-5-2",
            "claude-fable-5-2",
            "claude-fable-5-2",
            "claude-opus-5-5"
        ]
    );
    assert!(open[1].ended_at.is_none() && open[2].ended_at.is_some());

    // The pull request merges: its rounds close, and the next one starts at round 1.
    let merged = DeliveryRecord {
        task_id: task.clone(),
        stage: DeliveryStage::MergedIntegration,
        pr_number: Some(41),
        updated_at: Timestamp(7_000),
    };
    assert!(
        h.store
            .transition_delivery(&DeliveryStage::None, &merged)
            .await?
    );
    assert!(rounds(&h, &task, true).await?.is_empty());
    let closed = rounds(&h, &task, false).await?;
    assert!(
        closed
            .iter()
            .all(|r| r.pr_number == Some(41) && r.merged_at == Some(Timestamp(7_000)))
    );
    assert_eq!(a_round(&h, "rev-4", None).await?, "yulius-reviewer");
    assert_eq!(rounds(&h, &task, true).await?[0].round, 1);
    Ok(())
}

/// SPEC-493 REV-003: a resumed reviewer (the same subagent again) changes nothing, and a stop
/// of a subagent Yulius does not know, or of another agent type, ends no round.
#[tokio::test]
async fn a_resumed_reviewer_and_other_subagents_change_no_round() -> TestResult {
    let h = harness()?;
    seed(&h).await?;
    let account = add_account(&h, "acc-1", None).await?;
    let task = running_task(&h, TaskMode::Prompt, &account).await?;
    a_round(&h, "rev-1", None).await?;
    // Resumed with SendMessage: SubagentStart again with the same id.
    hook(
        &h.internal,
        "SubagentStart",
        json!({"agent_id": "rev-1", "agent_type": "yulius-reviewer"}),
    )
    .await?;
    for stop in [
        json!({"agent_id": "other", "agent_type": "yulius-reviewer",
            "last_assistant_message": "Verdict: approve"}),
        json!({"agent_id": "rev-1", "agent_type": "general-purpose",
            "last_assistant_message": "Verdict: approve"}),
    ] {
        hook(&h.internal, "SubagentStop", stop).await?;
    }
    let open = rounds(&h, &task, true).await?;
    assert_eq!(open.len(), 1);
    assert_eq!(open[0].subagent_id.as_deref(), Some("rev-1"));
    assert!(open[0].ended_at.is_none() && open[0].verdict.is_none());
    // Its report ends it.
    hook(
        &h.internal,
        "SubagentStop",
        json!({"agent_id": "rev-1", "agent_type": "yulius-reviewer",
            "last_assistant_message": "no verdict line here"}),
    )
    .await?;
    let open = rounds(&h, &task, true).await?;
    assert!(open[0].ended_at.is_some() && open[0].verdict.is_none());
    Ok(())
}

/// A post-report continuation gets its own head and transcript observations. An old
/// delivery can neither reopen the completed round nor close the one now running.
#[tokio::test]
async fn continued_review_keeps_history_across_repeated_hook_delivery() -> TestResult {
    use crate::subscription_pool::hook_sent_at;
    use crate::support::{confined_git, harness_with_git, repos::git};
    use std::sync::{Arc, atomic::Ordering};
    use yulius_app::ports::store::TaskFieldUpdate;
    let h = harness_with_git(Arc::new(confined_git()))?;
    seed(&h).await?;
    let account = add_account(&h, "acc-1", None).await?;
    let task = running_task(&h, TaskMode::Prompt, &account).await?;
    let repo = h._dir.path().join("sandboxes/s1/worktrees/review");
    std::fs::create_dir_all(&repo)?;
    git(&repo, &["init"])?;
    git(&repo, &["commit", "--allow-empty", "-m", "first head"])?;
    let first_head = git(&repo, &["rev-parse", "HEAD"])?;
    let start_at = h.clock.0.load(Ordering::SeqCst);
    h.store
        .update_task(
            &task,
            &TaskFieldUpdate {
                worktree_path: Some(repo.to_string_lossy().into_owned()),
                ..Default::default()
            },
            Timestamp(start_at),
        )
        .await?;
    a_round(&h, "rev-1", None).await?;
    h.store
        .note_review_subagent(&task, "rev-1", Some("first-model"), true, Some(h.app.now()))
        .await?;
    let start = json!({"agent_id":"rev-1","agent_type":"yulius-reviewer"});
    let stop = json!({"agent_id":"rev-1","agent_type":"yulius-reviewer",
        "last_assistant_message":"Verdict: request changes"});
    h.clock.0.store(start_at + 1, Ordering::SeqCst);
    hook(&h.internal, "SubagentStop", stop.clone()).await?;
    let first = rounds(&h, &task, true).await?.remove(0);
    assert_eq!(first.head.as_deref(), Some(first_head.trim()));
    assert_eq!(first.verdict.as_deref(), Some("request_changes"));
    h.clock.0.store(start_at + 2, Ordering::SeqCst);
    hook(&h.internal, "SubagentStop", stop.clone()).await?;
    hook_sent_at(
        &h.internal,
        "claude",
        "SubagentStart",
        start.clone(),
        Some(start_at),
    )
    .await?;
    assert_eq!(
        rounds(&h, &task, true).await?.as_slice(),
        std::slice::from_ref(&first)
    );

    git(&repo, &["commit", "--allow-empty", "-m", "review fixes"])?;
    let fixed_head = git(&repo, &["rev-parse", "HEAD"])?;
    hook(&h.internal, "SubagentStart", start.clone()).await?;
    hook(&h.internal, "SubagentStart", start).await?;
    hook_sent_at(
        &h.internal,
        "claude",
        "SubagentStop",
        stop,
        Some(start_at + 1),
    )
    .await?;
    let open = rounds(&h, &task, true).await?;
    assert_eq!(open.len(), 2);
    assert_eq!(open[0], first);
    assert_eq!(open[1].round, 2);
    assert_eq!(open[1].reason, "kept");
    assert_eq!(open[1].agent, first.agent);
    assert_eq!(open[1].model, first.model);
    assert_eq!(open[1].subagent_id, first.subagent_id);
    assert_eq!(open[1].head.as_deref(), Some(fixed_head.trim()));
    assert_eq!(open[1].model_answered.as_deref(), Some("first-model"));
    assert_eq!(open[1].started_at, Timestamp(start_at + 2));
    assert!(open[1].ended_at.is_none());
    assert!(!open[1].cut_off);
    h.store
        .note_review_subagent(
            &task,
            "rev-1",
            Some("second-model"),
            false,
            Some(h.app.now()),
        )
        .await?;
    let approve = json!({"agent_id":"rev-1","agent_type":"yulius-reviewer",
        "last_assistant_message":"Verdict: approve"});
    hook(&h.internal, "SubagentStop", approve).await?;
    let ended = rounds(&h, &task, true).await?;
    assert_eq!(ended[0], first);
    assert_eq!(ended[1].verdict.as_deref(), Some("approve"));
    assert_eq!(ended[1].model_answered.as_deref(), Some("second-model"));

    // A different reviewer has since reported. Continuing the first uses the PR's next number.
    a_round(&h, "rev-other", Some("Verdict: request changes")).await?;
    h.clock.0.fetch_add(1, Ordering::SeqCst);
    hook(
        &h.internal,
        "SubagentStart",
        json!({"agent_id":"rev-1","agent_type":"yulius-reviewer"}),
    )
    .await?;
    let after_other = rounds(&h, &task, true).await?;
    assert_eq!(after_other.len(), 4);
    assert_eq!(after_other[3].round, 4);
    assert_eq!(after_other[3].model, first.model);
    Ok(())
}

/// One reviewer can give the exact same report in distinct rounds. Analytics counts
/// the sequence once, and late events for the merged reviewer leave the next PR alone.
#[tokio::test]
async fn continued_review_counts_each_report_in_merged_analytics() -> TestResult {
    use crate::support::{call, setup_superadmin_creds};
    use axum::http::Method;
    use std::sync::atomic::Ordering;
    let h = harness()?;
    seed(&h).await?;
    let admin = setup_superadmin_creds(&h).await?;
    let account = add_account(&h, "acc-1", None).await?;
    let task = running_task(&h, TaskMode::Prompt, &account).await?;
    a_round(&h, "rev-1", Some("Verdict: request changes")).await?;
    let start = json!({"agent_id":"rev-1","agent_type":"yulius-reviewer"});
    let approve = json!({"agent_id":"rev-1","agent_type":"yulius-reviewer",
        "last_assistant_message":"Verdict: approve"});
    for _ in 0..2 {
        h.clock.0.fetch_add(1, Ordering::SeqCst);
        hook(&h.internal, "SubagentStart", start.clone()).await?;
        hook(&h.internal, "SubagentStop", approve.clone()).await?;
        hook(&h.internal, "SubagentStop", approve.clone()).await?;
    }
    assert!(
        h.store
            .transition_delivery(
                &DeliveryStage::None,
                &DeliveryRecord {
                    task_id: task.clone(),
                    stage: DeliveryStage::MergedIntegration,
                    pr_number: Some(7),
                    updated_at: Timestamp(h.clock.0.load(Ordering::SeqCst)),
                }
            )
            .await?
    );
    let r = call(
        &h.router,
        Method::GET,
        "/api/analytics/reviews",
        None,
        &admin,
    )
    .await?;
    assert_eq!(r.status, 200, "{}", r.body);
    assert_eq!(r.body["median_rounds"], 3.0, "{}", r.body);
    assert_eq!(r.body["first_rounds_requesting_changes"], 1);
    assert_eq!(r.body["re_reviews_after_changes"], 1);
    assert_eq!(r.body["rounds_after_approval"], 1);
    assert_eq!(
        r.body["rounds_by_model"],
        json!([{"model":"claude-fable-5-2","rounds":3}])
    );
    let closed = rounds(&h, &task, false).await?;
    hook(&h.internal, "PreToolUse", agent_start("yulius-reviewer")).await?;
    hook(&h.internal, "SubagentStart", start).await?;
    hook(&h.internal, "SubagentStop", approve).await?;
    let next = rounds(&h, &task, true).await?;
    assert_eq!(next.len(), 1);
    assert_eq!(next[0].round, 1);
    assert!(next[0].subagent_id.is_none() && next[0].ended_at.is_none());
    assert_eq!(&rounds(&h, &task, false).await?[..3], closed.as_slice());
    Ok(())
}

/// A stale hook must preserve the resume instruction and the pending subscription move
/// of a continued reviewer. Its own resume and report must clear them.
#[tokio::test]
async fn a_cut_off_continuation_keeps_its_recovery_on_older_hooks() -> TestResult {
    use crate::subscription_pool::hook_sent_at;
    use std::sync::atomic::Ordering;
    let h = harness()?;
    seed(&h).await?;
    let account = add_account(&h, "acc-1", None).await?;
    let task = running_task(&h, TaskMode::Prompt, &account).await?;
    let old = h.clock.0.load(Ordering::SeqCst);
    a_round(&h, "rev", Some("Verdict: approve")).await?;
    let start = json!({"agent_id":"rev","agent_type":"yulius-reviewer"});
    let stop = json!({"agent_id":"rev","agent_type":"yulius-reviewer",
        "last_assistant_message":"Verdict: approve"});
    h.clock.0.fetch_add(1, Ordering::SeqCst);
    hook(&h.internal, "SubagentStart", start.clone()).await?;
    let mut after_replay = Vec::new();
    for (event, body) in [("SubagentStart", &start), ("SubagentStop", &stop)] {
        h.app.cut_off_review_for_test(&task, "rev", None).await;
        let pending = (Some(String::from("rev")), true);
        assert_eq!(h.app.review_recovery_for_test(&task).await, pending);
        hook_sent_at(&h.internal, "claude", event, body.clone(), Some(old)).await?;
        after_replay.push(h.app.review_recovery_for_test(&task).await);
        hook(&h.internal, event, body.clone()).await?;
        assert_eq!(
            h.app.review_recovery_for_test(&task).await,
            (None, false),
            "{event}"
        );
    }
    assert_eq!(
        after_replay,
        [
            (Some("rev".to_owned()), true),
            (Some("rev".to_owned()), true)
        ]
    );
    assert_eq!(
        rounds(&h, &task, true).await?[1].verdict.as_deref(),
        Some("approve")
    );
    Ok(())
}

/// Replaying the current round's own start before its cutoff was observed is still a replay.
#[tokio::test]
async fn a_replayed_continuation_start_cannot_clear_a_later_cut_off() -> TestResult {
    use crate::subscription_pool::hook_sent_at;
    use std::sync::atomic::Ordering;
    let h = harness()?;
    seed(&h).await?;
    let account = add_account(&h, "acc-1", None).await?;
    let task = running_task(&h, TaskMode::Prompt, &account).await?;
    a_round(&h, "rev", Some("Verdict: request changes")).await?;
    let start = json!({"agent_id":"rev","agent_type":"yulius-reviewer"});
    h.clock.0.fetch_add(1, Ordering::SeqCst);
    let sent = h.app.now().0;
    hook_sent_at(
        &h.internal,
        "claude",
        "SubagentStart",
        start.clone(),
        Some(sent),
    )
    .await?;
    // The watcher reads the limit after the genuine resume was sent. Compare send times.
    h.clock.0.store(sent + 3, Ordering::SeqCst);
    h.app
        .cut_off_review_for_test(&task, "rev", Some(Timestamp(sent + 1)))
        .await;
    hook_sent_at(
        &h.internal,
        "claude",
        "SubagentStart",
        start.clone(),
        Some(sent),
    )
    .await?;
    assert_eq!(
        h.app.review_recovery_for_test(&task).await,
        (Some("rev".to_owned()), true)
    );
    hook_sent_at(
        &h.internal,
        "claude",
        "SubagentStart",
        start,
        Some(sent + 2),
    )
    .await?;
    assert_eq!(h.app.review_recovery_for_test(&task).await, (None, false));
    assert_eq!(rounds(&h, &task, true).await?.len(), 2);
    Ok(())
}

/// A delayed usage-limit observation from a completed review belongs to that round.
#[tokio::test]
async fn a_delayed_limit_from_a_completed_round_does_not_move_its_continuation() -> TestResult {
    use std::sync::atomic::Ordering;
    let h = harness()?;
    seed(&h).await?;
    let account = add_account(&h, "acc-1", None).await?;
    let task = running_task(&h, TaskMode::Prompt, &account).await?;
    a_round(&h, "rev", Some("Verdict: request changes")).await?;
    let old = h.app.now();
    h.clock.0.fetch_add(1, Ordering::SeqCst);
    hook(
        &h.internal,
        "SubagentStart",
        json!({"agent_id":"rev","agent_type":"yulius-reviewer"}),
    )
    .await?;
    h.app.cut_off_review_for_test(&task, "rev", Some(old)).await;
    assert_eq!(h.app.review_recovery_for_test(&task).await, (None, false));
    h.app
        .cut_off_review_for_test(&task, "rev", Some(h.app.now()))
        .await;
    assert_eq!(
        h.app.review_recovery_for_test(&task).await,
        (Some("rev".to_owned()), true)
    );
    Ok(())
}

/// A delayed account write keeps its usage evidence while the reviewer resumes or reports.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_reviewer_resuming_during_the_limit_write_cancels_the_move() -> TestResult {
    use crate::support::{hold_point, point_reached};
    use std::sync::atomic::Ordering;
    use yulius_app::ports::store::AccountStore as _;
    use yulius_app::race::Point;
    let h = harness()?;
    seed(&h).await?;
    let account = add_account(&h, "held-review-limit", None).await?;
    let task = running_task(&h, TaskMode::Prompt, &account).await?;
    a_round(&h, "rev", None).await?;
    let mut pending = Vec::new();
    for reports in [false, true] {
        let (writing, release) = hold_point(Point::LimitRecord, account.as_str());
        let limit = {
            let (app, task) = (h.app.clone(), task.clone());
            tokio::spawn(async move {
                app.cut_off_review_for_test(&task, "rev", Some(app.now()))
                    .await
            })
        };
        point_reached(writing).await?;
        h.clock.0.fetch_add(1, Ordering::SeqCst);
        if reports {
            hook(
                &h.internal,
                "SubagentStop",
                json!({"agent_id":"rev","agent_type":"yulius-reviewer",
                "last_assistant_message":"Verdict: request changes"}),
            )
            .await?;
            h.clock.0.fetch_add(1, Ordering::SeqCst);
        }
        hook(
            &h.internal,
            "SubagentStart",
            json!({"agent_id":"rev","agent_type":"yulius-reviewer"}),
        )
        .await?;
        release.send(())?;
        limit.await?;
        let record = h.store.account(&account).await?.ok_or("account")?;
        let usage = yulius_app::usage_json::from_json(record.usage_json.as_deref().unwrap_or("{}"));
        assert_eq!(usage.weekly_fable.and_then(|w| w.used_percent), Some(100.0));
        pending.push(h.app.review_recovery_for_test(&task).await);
    }
    assert_eq!(rounds(&h, &task, true).await?.len(), 2);
    assert_eq!(pending, [(None, false), (None, false)]);
    Ok(())
}

/// The detached start handler can still be reading Git when the reviewer reports.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_continued_report_survives_its_start_time_head_lookup() -> TestResult {
    use crate::support::{confined_git, harness_with_git, repos::git};
    use std::sync::{Arc, atomic::Ordering, mpsc};
    use std::time::Duration;
    use yulius_app::ports::store::TaskFieldUpdate;
    let h = harness_with_git(Arc::new(confined_git()))?;
    seed(&h).await?;
    let account = add_account(&h, "acc-1", None).await?;
    let task = running_task(&h, TaskMode::Prompt, &account).await?;
    a_round(&h, "rev", Some("Verdict: request changes")).await?;
    let repo = h._dir.path().join("sandboxes/s1/worktrees/slow-review");
    std::fs::create_dir_all(&repo)?;
    git(&repo, &["init"])?;
    git(&repo, &["commit", "--allow-empty", "-m", "fixes"])?;
    let head = git(&repo, &["rev-parse", "HEAD"])?;
    h.store
        .update_task(
            &task,
            &TaskFieldUpdate {
                worktree_path: Some(repo.to_string_lossy().into_owned()),
                ..Default::default()
            },
            h.app.now(),
        )
        .await?;
    let (entered, at_git) = tokio::sync::oneshot::channel();
    let (release, held) = mpsc::channel();
    yulius_app::race::on(yulius_app::race::Point::ReviewHead, &repo, move || {
        let _ = entered.send(());
        let _ = held.recv_timeout(Duration::from_secs(20));
    });
    h.clock.0.fetch_add(1, Ordering::SeqCst);
    let router = h.internal.clone();
    let start = tokio::spawn(async move {
        hook(
            &router,
            "SubagentStart",
            json!({"agent_id":"rev","agent_type":"yulius-reviewer"}),
        )
        .await
        .map(|r| r.0)
        .map_err(|e| e.to_string())
    });
    tokio::time::timeout(Duration::from_secs(10), at_git).await??;
    hook(&h.internal, "SubagentStop", json!({"agent_id":"rev","agent_type":"yulius-reviewer","last_assistant_message":"Verdict: approve"})).await?;
    release.send(())?;
    assert_eq!(start.await??, 200);
    let rows = rounds(&h, &task, true).await?;
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].verdict.as_deref(), Some("request_changes"));
    assert_eq!(rows[1].verdict.as_deref(), Some("approve"));
    assert_eq!(rows[1].head.as_deref(), Some(head.trim()));
    Ok(())
}

/// SPEC-493 REV-007 (its part without a move): a first round on a subscription that cannot run
/// Fable 5.2 (its allowance used up) runs the Opus reviewer, records why, and the agent is told.
#[tokio::test]
async fn a_round_on_a_subscription_out_of_fable_runs_the_opus_reviewer() -> TestResult {
    let h = harness()?;
    seed(&h).await?;
    let full = json!({"weekly_fable": {"used_percent": 100.0, "window_minutes": 10080,
        "resets_at": 4_102_444_800_000_i64}});
    let account = add_account(&h, "acc-1", Some(full.to_string())).await?;
    let task = running_task(&h, TaskMode::Prompt, &account).await?;
    let (_, _, answer) = hook(&h.internal, "PreToolUse", agent_start("yulius-reviewer")).await?;
    assert_eq!(
        answer["hookSpecificOutput"]["updatedInput"]["subagent_type"], "yulius-reviewer-opus",
        "{answer}"
    );
    assert!(
        answer["hookSpecificOutput"]["additionalContext"]
            .as_str()
            .is_some_and(|s| s.contains("cannot run Fable 5.2")),
        "{answer}"
    );
    let open = rounds(&h, &task, true).await?;
    assert_eq!(
        (
            open[0].round,
            open[0].agent.as_str(),
            open[0].reason.as_str()
        ),
        (1, "yulius-reviewer-opus", "no_fable")
    );
    // The reviewer is told too, and its report repeats it; the rest of the prompt is kept.
    let prompt = answer["hookSpecificOutput"]["updatedInput"]["prompt"]
        .as_str()
        .unwrap_or_default();
    assert!(
        prompt.starts_with("Review origin/dev...HEAD against the spec.")
            && prompt.contains("runs on Opus 5.5")
            && prompt.contains("summary"),
        "{prompt}"
    );
    assert!(h.store.switches(&task).await?.is_empty(), "nowhere to move");
    Ok(())
}

/// A running task on `acc-1`, whose Fable allowance is used up, with `acc-0` (also out of Fable)
/// and `acc-2` (room for both) in the pool. The harness has no clone, so a move of its session
/// cannot start; the real-process test below moves one.
async fn out_of_fable(
    h: &crate::support::Harness,
) -> Result<yulius_domain::ids::TaskId, Box<dyn std::error::Error>> {
    seed(h).await?;
    let full = json!({"weekly_fable": {"used_percent": 100.0, "window_minutes": 10080,
        "resets_at": 4_102_444_800_000_i64}})
    .to_string();
    let account = add_account(h, "acc-1", Some(full.clone())).await?;
    add_account(h, "acc-0", Some(full)).await?;
    add_account(h, "acc-2", None).await?;
    running_task(h, TaskMode::Prompt, &account).await
}

/// Why the answer refused the Agent call.
fn denied(answer: &Value) -> String {
    assert_eq!(
        answer["hookSpecificOutput"]["permissionDecision"], "deny",
        "{answer}"
    );
    answer["hookSpecificOutput"]["permissionDecisionReason"]
        .as_str()
        .unwrap_or_default()
        .to_owned()
}

/// The answer ran the Opus reviewer because Fable cannot run, and told the reviewer so.
fn ran_opus_without_fable(answer: &Value) {
    assert_eq!(
        answer["hookSpecificOutput"]["updatedInput"]["subagent_type"], "yulius-reviewer-opus",
        "{answer}"
    );
    let prompt = answer["hookSpecificOutput"]["updatedInput"]["prompt"]
        .as_str()
        .unwrap_or_default();
    assert!(
        prompt.starts_with("Review origin/dev...HEAD against the spec.")
            && prompt.contains("runs on Opus 5.5"),
        "{prompt}"
    );
}

/// SPEC-493 REV-007 (AC-8, review round 1 of fix 1d): a move would end work the session has
/// running, or a prompt waiting in Claude Code's queue: the Agent call is refused, each with its
/// own reason, and nothing moves or is recorded.
#[tokio::test]
async fn a_round_out_of_fable_is_refused_while_a_move_would_lose_work() -> TestResult {
    let h = harness()?;
    let task = out_of_fable(&h).await?;
    h.app.background_agents_observed(&task, 1).await;
    let (_, _, answer) = hook(&h.internal, "PreToolUse", agent_start("yulius-reviewer")).await?;
    let why = denied(&answer);
    assert!(
        why.contains("background work") && why.contains("Fable 5.2"),
        "{why}"
    );
    h.app.background_agents_observed(&task, 0).await;
    let (code, _, _) = hook(
        &h.internal,
        "UserPromptSubmit",
        json!({"session_id": "s", "hook_event_name": "UserPromptSubmit", "prompt": "go on"}),
    )
    .await?;
    assert_eq!(code, 200);
    let (_, _, answer) = hook(&h.internal, "PreToolUse", agent_start("yulius-reviewer")).await?;
    let why = denied(&answer);
    assert!(
        why.contains("prompt waits") && why.contains("end your turn"),
        "{why}"
    );
    assert!(rounds(&h, &task, false).await?.is_empty());
    assert!(h.store.switches(&task).await?.is_empty());
    Ok(())
}

/// SPEC-493 REV-007 (review round 1 of fix 1d): a move whose new session could not start (here
/// no clone) is never announced: the round runs the Opus reviewer at once, and nothing moves.
#[tokio::test]
async fn a_round_out_of_fable_whose_move_cannot_start_runs_the_opus_reviewer() -> TestResult {
    let h = harness()?;
    let task = out_of_fable(&h).await?;
    let (_, _, answer) = hook(&h.internal, "PreToolUse", agent_start("yulius-reviewer")).await?;
    ran_opus_without_fable(&answer);
    assert_eq!(rounds(&h, &task, true).await?[0].reason, "no_fable");
    assert!(h.store.switches(&task).await?.is_empty());
    Ok(())
}

/// SPEC-493 REV-007 (review round 1 of fix 1d): a switch refused moments ago is not tried again
/// at every review start: the round runs the Opus reviewer, even while background work runs.
#[tokio::test]
async fn a_round_after_a_refused_move_runs_the_opus_reviewer() -> TestResult {
    let h = harness()?;
    let task = out_of_fable(&h).await?;
    let refused = h
        .app
        .switch_task_now(
            &task,
            Some(yulius_domain::ids::AccountId::parse("acc-2")?),
            "Proactive",
            None,
        )
        .await;
    assert!(refused.is_err());
    h.app.background_agents_observed(&task, 1).await;
    let (_, _, answer) = hook(&h.internal, "PreToolUse", agent_start("yulius-reviewer")).await?;
    ran_opus_without_fable(&answer);
    assert_eq!(
        h.store.switches(&task).await?.len(),
        1,
        "the refused one only"
    );
    Ok(())
}

/// [`out_of_fable`] whose session can start elsewhere: the sandbox has a clone.
async fn out_of_fable_movable(
    h: &crate::support::Harness,
) -> Result<yulius_domain::ids::TaskId, Box<dyn std::error::Error>> {
    let task = out_of_fable(h).await?;
    std::fs::create_dir_all(h.app.config.data_dir.join("sandboxes/s1/repo/.git"))?;
    Ok(task)
}

/// An isolation whose holder takes `hold` to prepare, and says when it was done.
struct SlowIsolation {
    hold: std::time::Duration,
    done: std::sync::atomic::AtomicBool,
}

#[async_trait::async_trait]
impl yulius_app::ports::isolation::SandboxIsolation for SlowIsolation {
    async fn prepare(
        &self,
        _: &yulius_app::ports::isolation::SandboxRef,
    ) -> yulius_app::error::AppResult<Option<yulius_app::ports::isolation::SandboxJoin>> {
        tokio::time::sleep(self.hold).await;
        self.done.store(true, std::sync::atomic::Ordering::SeqCst);
        Ok(None)
    }

    fn release(&self, _: &yulius_domain::ids::SandboxId) {}
}

/// SPEC-493 REV-007 (review round 2 of fix 1d): a decision whose looks take longer than the
/// hook client waits (here the new session's start check) answers within its bound with the
/// Opus reviewer. The check is not cut off: it runs to its end, and its late answer moves
/// nothing.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_slow_move_decision_runs_the_opus_reviewer_within_its_bound() -> TestResult {
    let h = harness()?;
    let task = out_of_fable_movable(&h).await?;
    let slow = std::sync::Arc::new(SlowIsolation {
        hold: std::time::Duration::from_secs(5),
        done: std::sync::atomic::AtomicBool::new(false),
    });
    assert!(h.app.set_isolation(slow.clone()));
    let asked = std::time::Instant::now();
    let (_, _, answer) = hook(&h.internal, "PreToolUse", agent_start("yulius-reviewer")).await?;
    let took = asked.elapsed();
    assert!(took < std::time::Duration::from_millis(4500), "{took:?}");
    ran_opus_without_fable(&answer);
    for _ in 0..400 {
        if slow.done.load(std::sync::atomic::Ordering::SeqCst) {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
    assert!(
        slow.done.load(std::sync::atomic::Ordering::SeqCst),
        "the start check ran to its end"
    );
    assert!(h.store.switches(&task).await?.is_empty());
    Ok(())
}

/// SPEC-493 REV-007 (review round 2 of fix 1d): a move decided for a run that is no longer the
/// task's live one (a later start moved the session meanwhile) moves nothing.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_move_decided_for_an_ended_run_moves_nothing() -> TestResult {
    let h = harness()?;
    let task = out_of_fable_movable(&h).await?;
    let (release, held) = std::sync::mpsc::channel::<()>();
    let (reached, decided) = std::sync::mpsc::channel::<()>();
    yulius_app::race::on(
        yulius_app::race::Point::ReviewMoveDecided,
        std::path::Path::new(task.as_str()),
        move || {
            reached.send(()).ok();
            held.recv_timeout(std::time::Duration::from_secs(20)).ok();
        },
    );
    let (_, _, answer) = hook(&h.internal, "PreToolUse", agent_start("yulius-reviewer")).await?;
    assert!(denied(&answer).contains("moves this session"), "{answer}");
    tokio::task::spawn_blocking(move || decided.recv_timeout(std::time::Duration::from_secs(20)))
        .await??;
    let run = h.store.live_attempt(&task).await?.ok_or("live run")?;
    let ended = yulius_app::ports::store::AttemptUpdate {
        ended_at: Some(Timestamp(5)),
        exit_reason: Some("switched".into()),
        ..yulius_app::ports::store::AttemptUpdate::default()
    };
    assert!(
        h.store
            .transition_attempt(&run.id, yulius_domain::task::AttemptState::Killed, &ended)
            .await?
    );
    release.send(())?;
    // Nothing is to happen: a switch would be recorded at once, so a short look tells.
    for _ in 0..40 {
        assert!(h.store.switches(&task).await?.is_empty());
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
    Ok(())
}

/// SPEC-493 REV-007 (AC-8): round 3 runs Opus anyway, so a subscription out of Fable moves
/// nothing and is not the reason of the round.
#[tokio::test]
async fn a_third_round_out_of_fable_does_not_move() -> TestResult {
    let h = harness()?;
    let task = out_of_fable(&h).await?;
    for round in 1..=2 {
        h.store
            .insert_review_round(&ReviewRoundRecord {
                id: format!("round-{round}"),
                task_id: task.clone(),
                round,
                head: None,
                agent: "yulius-reviewer".into(),
                model: "claude-fable-5-2".into(),
                reason: "round".into(),
                subagent_id: None,
                model_answered: None,
                started_at: Timestamp(1),
                ended_at: Some(Timestamp(2)),
                verdict: Some("request changes".into()),
                cut_off: false,
                pr_number: None,
                merged_at: None,
            })
            .await?;
    }
    let (_, _, answer) = hook(&h.internal, "PreToolUse", agent_start("yulius-reviewer")).await?;
    assert_eq!(
        answer["hookSpecificOutput"]["updatedInput"]["subagent_type"], "yulius-reviewer-opus",
        "{answer}"
    );
    assert_ne!(answer["hookSpecificOutput"]["permissionDecision"], "deny");
    let open = rounds(&h, &task, true).await?;
    assert_eq!((open[2].round, open[2].reason.as_str()), (3, "round"));
    assert!(h.store.switches(&task).await?.is_empty());
    Ok(())
}

/// SPEC-493 REV-007 (review round 1 of fix 1d): the Opus reviewer asked for by name in round 1,
/// on a subscription out of Fable, is told why it runs on Opus like one Yulius switched.
#[tokio::test]
async fn the_opus_reviewer_asked_for_by_name_is_told_why() -> TestResult {
    let h = harness()?;
    out_of_fable(&h).await?;
    let (_, _, answer) = hook(
        &h.internal,
        "PreToolUse",
        agent_start("yulius-reviewer-opus"),
    )
    .await?;
    ran_opus_without_fable(&answer);
    Ok(())
}

/// SPEC-493 REV-002 (review round 1 of fix 1b): a round is counted once, however many of its
/// reviewers report. Round 1 is cut off (its reviewer started again reports), then the cut-off
/// one is resumed and reports too: the next review is round 2, still on the Fable reviewer.
#[tokio::test]
async fn a_round_with_two_reports_is_counted_once() -> TestResult {
    let h = harness()?;
    seed(&h).await?;
    let account = add_account(&h, "acc-1", None).await?;
    let task = running_task(&h, TaskMode::Prompt, &account).await?;
    a_round(&h, "rev-a", None).await?;
    a_round(&h, "rev-b", Some("x\nVerdict: request changes")).await?;
    // The cut-off reviewer, resumed with SendMessage, reports as well.
    hook(
        &h.internal,
        "SubagentStop",
        json!({"agent_id": "rev-a", "agent_type": "yulius-reviewer",
            "last_assistant_message": "y\nVerdict: request changes"}),
    )
    .await?;
    assert_eq!(a_round(&h, "rev-c", None).await?, "yulius-reviewer");
    let numbers: Vec<u32> = rounds(&h, &task, true)
        .await?
        .iter()
        .map(|r| r.round)
        .collect();
    assert_eq!(numbers, [1, 1, 2]);
    Ok(())
}

/// SPEC-493 REV-003 (AC-5): with the store failing, the reviewer starts as asked: an answer
/// that changes and denies nothing.
#[tokio::test]
async fn a_round_yulius_cannot_record_starts_as_asked() -> TestResult {
    let h = harness()?;
    seed(&h).await?;
    let account = add_account(&h, "acc-1", None).await?;
    running_task(&h, TaskMode::Prompt, &account).await?;
    h.store
        .execute_for_tests("DROP TABLE review_rounds")
        .await?;
    let (code, _, answer) = hook(&h.internal, "PreToolUse", agent_start("yulius-reviewer")).await?;
    assert_eq!(code, 200, "{answer}");
    assert!(
        answer["hookSpecificOutput"]["updatedInput"].is_null()
            && answer["hookSpecificOutput"]["permissionDecision"].is_null(),
        "{answer}"
    );
    Ok(())
}

/// SPEC-493 REV-007 (review round 1 of fix 1b): when the run's subscription is no longer known
/// (removed meanwhile), the reviewer starts as asked: the stronger one.
#[tokio::test]
async fn a_round_on_an_unknown_subscription_starts_as_asked() -> TestResult {
    use yulius_app::ports::store::AccountStore;
    let h = harness()?;
    seed(&h).await?;
    let account = add_account(&h, "acc-1", None).await?;
    let task = running_task(&h, TaskMode::Prompt, &account).await?;
    h.store.delete_account(&account).await?;
    assert_eq!(a_round(&h, "rev-1", None).await?, "yulius-reviewer");
    assert_eq!(rounds(&h, &task, true).await?[0].reason, "round");
    Ok(())
}

/// SPEC-493 RLM-003 (AC-19): a round on a subscription with 3 points of room left for its
/// reviewer's model, where no other subscription has more, starts as asked, where it is.
#[tokio::test]
async fn a_tight_round_with_nowhere_roomier_starts_where_it_is() -> TestResult {
    let h = harness()?;
    seed(&h).await?;
    let tight = |used: f64| {
        json!({"primary": {"used_percent": used, "window_minutes": 300,
            "resets_at": 4_102_444_800_000_i64}})
        .to_string()
    };
    let account = add_account(&h, "acc-1", Some(tight(95.0))).await?;
    add_account(&h, "acc-2", Some(tight(96.0))).await?;
    let task = running_task(&h, TaskMode::Prompt, &account).await?;
    let (_, _, answer) = hook(&h.internal, "PreToolUse", agent_start("yulius-reviewer")).await?;
    assert_ne!(
        answer["hookSpecificOutput"]["permissionDecision"], "deny",
        "{answer}"
    );
    let open = rounds(&h, &task, true).await?;
    assert_eq!((open.len(), open[0].agent.as_str()), (1, "yulius-reviewer"));
    assert!(h.store.switches(&task).await?.is_empty());
    Ok(())
}

/// SPEC-493 RLM-003: a move for room gives way to the session's work: with background work
/// running, the round starts where it is rather than being refused.
#[tokio::test]
async fn a_tight_round_with_work_running_starts_where_it_is() -> TestResult {
    let h = harness()?;
    seed(&h).await?;
    let used = |percent: f64| {
        json!({"primary": {"used_percent": percent, "window_minutes": 300,
            "resets_at": 4_102_444_800_000_i64}})
        .to_string()
    };
    let account = add_account(&h, "acc-1", Some(used(95.0))).await?;
    add_account(&h, "acc-2", Some(used(20.0))).await?;
    let task = running_task(&h, TaskMode::Prompt, &account).await?;
    h.app.background_agents_observed(&task, 1).await;
    let (_, _, answer) = hook(&h.internal, "PreToolUse", agent_start("yulius-reviewer")).await?;
    assert_ne!(
        answer["hookSpecificOutput"]["permissionDecision"], "deny",
        "{answer}"
    );
    assert_eq!(rounds(&h, &task, true).await?.len(), 1);
    assert!(h.store.switches(&task).await?.is_empty());
    Ok(())
}

/// SPEC-493 REV-004: what the reviewer's transcript says is noted on its round: the model that
/// answered (the latest), and that a usage limit cut it off, which its later report keeps.
#[tokio::test]
async fn a_rounds_answering_model_and_cut_off_are_noted() -> TestResult {
    let h = harness()?;
    seed(&h).await?;
    let account = add_account(&h, "acc-1", None).await?;
    let task = running_task(&h, TaskMode::Prompt, &account).await?;
    a_round(&h, "rev-1", None).await?;
    let store = &h.store;
    store
        .note_review_subagent(
            &task,
            "rev-1",
            Some("claude-fable-5-2"),
            false,
            Some(h.app.now()),
        )
        .await?;
    store
        .note_review_subagent(&task, "rev-1", None, true, Some(h.app.now()))
        .await?;
    // A subagent that is no round's changes nothing.
    store
        .note_review_subagent(
            &task,
            "other",
            Some("claude-haiku-4-5"),
            true,
            Some(h.app.now()),
        )
        .await?;
    let open = rounds(&h, &task, true).await?;
    assert_eq!(open.len(), 1);
    assert_eq!(open[0].model_answered.as_deref(), Some("claude-fable-5-2"));
    assert!(open[0].cut_off);
    hook(
        &h.internal,
        "SubagentStop",
        json!({"agent_id": "rev-1", "agent_type": "yulius-reviewer",
            "last_assistant_message": "Verdict: approve"}),
    )
    .await?;
    let open = rounds(&h, &task, true).await?;
    assert!(open[0].cut_off && open[0].verdict.as_deref() == Some("approve"));
    Ok(())
}

/// SPEC-493 REV-006 (AC-7): the reviews read gives the numbers of the pull requests merged in
/// the range, for the sandboxes the caller sees; another range sees none of them, and a person
/// not signed in is refused.
#[tokio::test]
async fn the_reviews_read_counts_the_rounds_of_merged_pull_requests() -> TestResult {
    use crate::support::{Creds, call, setup_superadmin_creds};
    use axum::http::Method;
    let h = harness()?;
    seed(&h).await?;
    let admin = setup_superadmin_creds(&h).await?;
    let account = add_account(&h, "acc-1", None).await?;
    let task = running_task(&h, TaskMode::Prompt, &account).await?;
    // Round 1 asks for changes, round 2 approves.
    a_round(&h, "rev-1", Some("x\nVerdict: request changes")).await?;
    a_round(&h, "rev-2", Some("x\nVerdict: approve")).await?;
    // Merged on 2026-11-15 (the day's ms).
    let day = 1_794_700_800_000_i64;
    h.store
        .transition_delivery(
            &DeliveryStage::None,
            &DeliveryRecord {
                task_id: task.clone(),
                stage: DeliveryStage::MergedIntegration,
                pr_number: Some(7),
                updated_at: Timestamp(day + 3_600_000),
            },
        )
        .await?;
    let read = |range: &str| {
        let (router, admin) = (h.router.clone(), admin.clone());
        let uri = format!("/api/analytics/reviews{range}");
        async move { call(&router, Method::GET, &uri, None, &admin).await }
    };
    let r = read("?from=2026-11-15&to=2026-11-15").await?;
    assert_eq!(r.status, 200, "{:?}", r.body);
    assert_eq!(r.body["pull_requests"], 1, "{}", r.body);
    assert_eq!(r.body["first_rounds_requesting_changes"], 1);
    assert_eq!(r.body["first_rounds_without_report"], 0);
    assert_eq!(r.body["median_rounds"], 2.0);
    // SPEC-506 FST-007: round 2 followed a request for changes and approved.
    assert_eq!(r.body["re_reviews_after_changes"], 1, "{}", r.body);
    assert_eq!(r.body["re_reviews_asking_again"], 0, "{}", r.body);
    assert_eq!(r.body["rounds_after_approval"], 0, "{}", r.body);
    assert_eq!(
        r.body["rounds_by_model"],
        json!([{"model": "claude-fable-5-2", "rounds": 2}])
    );
    assert_eq!(r.body["range"]["from"], "2026-11-15");
    let none = read("?from=2026-11-16").await?;
    assert_eq!(none.body["pull_requests"], 0, "{}", none.body);
    assert!(none.body["median_rounds"].is_null());
    let bad = read("?from=2026-11-16&to=2026-11-15").await?;
    assert_eq!(bad.status, 422, "{:?}", bad.body);
    let anonymous = call(
        &h.router,
        Method::GET,
        "/api/analytics/reviews",
        None,
        &Creds::default(),
    )
    .await?;
    assert_eq!(anonymous.status, 401);
    Ok(())
}

/// SPEC-493 REV-005: the merge record names the rounds of the pull request that merged (by its
/// merge, not its number), the models of the runs that worked on it (a run that ended before
/// the previous merge did not; one still going did), and says when no recorded round approved
/// the merged head; without a known head any approving round counts.
#[tokio::test]
async fn the_merge_record_names_this_pull_requests_rounds_and_authors() -> TestResult {
    use yulius_app::ports::store::{AuditStore as _, SandboxStore as _, TaskStore as _};
    let h = harness()?;
    seed(&h).await?;
    let account = add_account(&h, "acc-1", None).await?;
    let task = running_task(&h, TaskMode::Prompt, &account).await?;
    // An earlier run on Fable that ended before the first merge.
    let mut earlier = h.store.attempts(&task).await?[0].clone();
    earlier.id = yulius_domain::ids::AttemptId::parse("at-earlier")?;
    earlier.seq = 0;
    earlier.token_hash = "earlier-run-token-hash".into();
    earlier.model = Some("claude-fable-5-2".into());
    earlier.state = yulius_domain::task::AttemptState::Exited;
    earlier.ended_at = Some(Timestamp(1_000));
    h.store.insert_attempt(&earlier).await?;
    let merge = |pr: u64, expected: DeliveryStage, at: i64| {
        let (store, task) = (h.store.clone(), task.clone());
        async move {
            store
                .transition_delivery(
                    &expected,
                    &DeliveryRecord {
                        task_id: task,
                        stage: DeliveryStage::MergedIntegration,
                        pr_number: Some(pr),
                        updated_at: Timestamp(at),
                    },
                )
                .await
        }
    };
    a_round(&h, "rev-1", Some("Verdict: approve")).await?;
    assert!(merge(3, DeliveryStage::None, 5_000).await?);
    a_round(&h, "rev-2", Some("x\nVerdict: request changes")).await?;
    a_round(&h, "rev-3", None).await?;
    assert!(merge(4, DeliveryStage::MergedIntegration, 9_000).await?);
    let record = h.store.task(&task).await?.ok_or("task")?;
    let sandbox = h
        .store
        .sandbox(&record.sandbox_id)
        .await?
        .ok_or("sandbox")?;
    let target = format!("{}#{}", sandbox.slug, record.number);
    let details = || {
        let (store, target) = (h.store.clone(), target.clone());
        async move {
            store
                .audit_details(&target, "delivery.review.models", 10)
                .await
        }
    };
    h.app
        .record_review_models(
            &task,
            Some(4),
            Timestamp(9_000),
            Some(&merged_at("c0ffee0123456789")),
        )
        .await;
    let detail = details().await?.remove(0);
    assert_eq!(
        detail,
        "pull request #4: written by claude-opus-5-5; review rounds: 1 claude-fable-5-2 \
         request_changes; 2 claude-fable-5-2 no report; no recorded review round approved its \
         head",
        "pull request #3's round is not this one's, nor the run that ended before it merged"
    );
    // The first pull request: its round approved, and its run on Fable wrote it too.
    h.app
        .record_review_models(&task, Some(3), Timestamp(5_000), None)
        .await;
    assert_eq!(
        details().await?.remove(0),
        "pull request #3: written by claude-fable-5-2, claude-opus-5-5; review rounds: 1 \
         claude-fable-5-2 approve"
    );
    Ok(())
}

/// SPEC-493 REV-005: an approval counts for the merged head it approved; one of another head
/// (here with no worktree to tell whether only clean merges follow it) does not.
#[tokio::test]
async fn an_approval_counts_for_the_head_it_approved() -> TestResult {
    use yulius_app::ports::store::{AuditStore as _, SandboxStore as _, TaskStore as _};
    let h = harness()?;
    seed(&h).await?;
    let account = add_account(&h, "acc-1", None).await?;
    let task = running_task(&h, TaskMode::Prompt, &account).await?;
    h.store
        .insert_review_round(&ReviewRoundRecord {
            id: "r1".into(),
            task_id: task.clone(),
            round: 1,
            head: Some("aaaaaaa1111111111".into()),
            agent: "yulius-reviewer".into(),
            model: "claude-fable-5-2".into(),
            reason: "round".into(),
            subagent_id: Some("rev-1".into()),
            model_answered: None,
            started_at: Timestamp(1),
            ended_at: Some(Timestamp(2)),
            verdict: Some("approve".into()),
            cut_off: false,
            pr_number: None,
            merged_at: None,
        })
        .await?;
    assert!(
        h.store
            .transition_delivery(
                &DeliveryStage::None,
                &DeliveryRecord {
                    task_id: task.clone(),
                    stage: DeliveryStage::MergedIntegration,
                    pr_number: Some(9),
                    updated_at: Timestamp(3_000),
                },
            )
            .await?
    );
    let record = h.store.task(&task).await?.ok_or("task")?;
    let sandbox = h
        .store
        .sandbox(&record.sandbox_id)
        .await?
        .ok_or("sandbox")?;
    let target = format!("{}#{}", sandbox.slug, record.number);
    for (head, approved) in [("aaaaaaa1111111111", true), ("bbbbbbb2222222222", false)] {
        h.app
            .record_review_models(&task, Some(9), Timestamp(3_000), Some(&merged_at(head)))
            .await;
        let detail = h
            .store
            .audit_details(&target, "delivery.review.models", 1)
            .await?
            .remove(0);
        assert_eq!(
            !detail.ends_with("no recorded review round approved its head"),
            approved,
            "{head}: {detail}"
        );
    }
    Ok(())
}

/// SPEC-493 REV-007 (AC-8) end to end: a real session on a subscription out of Fable 5.2 asks
/// for the reviewer through the real hook client; the Agent call is refused, the turn is
/// interrupted and the session goes on on the subscription that runs both models, its first
/// prompt asking for the review again.
mod real {
    use std::sync::Arc;
    use std::time::Duration;

    use crate::first_message::{hooked, new_task, start_with};
    use crate::pool_supervision::{
        HOUR, ScriptedUsage, refresh, runtime, screen, seed, stop, temp, usage, wait_for,
    };
    use crate::subscription_moves::{live_run, ready, run_accounts};
    use crate::support::TestResult;
    use crate::support::agents::FakeAgentOptions;
    use yulius_app::model::TaskMode;
    use yulius_domain::usage::UsageWindow;
    use yulius_ptyhost::test_support::locate_fake_agent;

    #[test]
    fn a_session_out_of_fable_moves_and_is_asked_for_the_review_again() -> TestResult {
        let fake =
            locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
        let (_dir, data) = temp()?;
        let rt = runtime()?;
        rt.block_on(async {
            let probe = Arc::new(ScriptedUsage::default());
            let agent = FakeAgentOptions {
                resume_prompts: true,
                ..hooked()
            };
            let (app, _port) = start_with(&data, &fake, agent, 0, probe.clone(), |config| {
                config.claude_code_queue_record_ms = 0;
            })
            .await?;
            let (sandbox, user) = seed(&app, &data, &["a-first", "b-second"]).await?;
            // The first has the most room for Opus, so the task starts there, but its Fable
            // allowance is used up; the second runs both.
            let mut first = usage(5.0, 4 * HOUR, 10.0, 100);
            first.weekly_fable = Some(UsageWindow {
                used_percent: Some(100.0),
                window_minutes: Some(10_080),
                resets_at: first.secondary.as_ref().and_then(|w| w.resets_at),
            });
            probe.set("a-first", first);
            probe.set("b-second", usage(40.0, 4 * HOUR, 50.0, 100));
            refresh(&app, &user, "a-first").await?;
            refresh(&app, &user, "b-second").await?;
            let id = new_task(&app, &sandbox, &user, "Review it", TaskMode::Prompt).await?;
            ready(&app, &id).await?;
            assert_eq!(run_accounts(&app, &id).await, [Some("a-first".to_owned())]);
            let person = app.deps.store.task(&id).await?.ok_or("task")?.created_by;
            let ctx = yulius_app::auth::AuthContext {
                user: app.deps.store.user(&person).await?.ok_or("user")?,
                csrf_token: None,
                session_hash: None,
                token: None,
            };
            let attach = app
                .terminal_open(&ctx, id.as_str(), Some((120, 40)))
                .await?;
            app.terminal_input(&ctx, &attach, b"/agent yulius-reviewer", false)
                .await?;
            app.terminal_input(&ctx, &attach, b"\r", false).await?;
            live_run(&app, &id, 2).await?;
            assert_eq!(
                run_accounts(&app, &id).await,
                [Some("a-first".to_owned()), Some("b-second".to_owned())]
            );
            let asked = wait_for("the prompt", Duration::from_secs(30), || async {
                screen(&app, &id).contains("run Fable 5.2 for the review round you asked for")
            })
            .await;
            assert!(asked.is_ok(), "{}", screen(&app, &id));
            // The switch records its outcome once the new run started, which may be after the
            // run printed its prompt.
            wait_for("the completed switch", Duration::from_secs(30), || async {
                app.deps.store.switches(&id).await.is_ok_and(|switches| {
                    switches.iter().any(|switch| switch.outcome == "completed")
                })
            })
            .await?;
            let switches = app.deps.store.switches(&id).await?;
            assert_eq!(switches.len(), 1, "{switches:?}");
            assert_eq!(switches[0].reason, "review_fable");
            stop(&app, &id).await?;
            Ok::<_, Box<dyn std::error::Error>>(())
        })?;
        rt.shutdown_timeout(Duration::from_secs(5));
        Ok(())
    }

    /// SPEC-493 RLM-001, RLM-002 (AC-18) end to end: a real session on Opus runs its Fable
    /// reviewer, which a refusal of Fable's credits cuts off in its own transcript. The limit is
    /// recorded on the subscription for Fable, the session moves to the one that runs both
    /// models, and its new process is told to resume that reviewer by its id with
    /// `SendMessage`, not to start a new one.
    #[test]
    fn a_cut_off_reviewer_moves_the_session_and_is_resumed_there() -> TestResult {
        a_cut_off_reviewer(true, b"/reviewer-limit yulius-reviewer\r")
    }

    /// RLM-002 with no subscription that can run Fable: nothing moves, and the session is told
    /// to start the review again, which then runs on Opus 5.5 (REV-007).
    #[test]
    fn a_cut_off_reviewer_nobody_can_run_is_told_to_go_on_on_opus() -> TestResult {
        a_cut_off_reviewer(false, b"/reviewer-limit yulius-reviewer\r")
    }

    /// Review round 1 of fix 7: a reviewer refused at its first request, before any answer of
    /// it is read, meets its round's model's limit, not the session's.
    #[test]
    fn a_reviewer_refused_before_any_answer_meets_its_own_models_limit() -> TestResult {
        a_cut_off_reviewer(true, b"/reviewer-limit-first yulius-reviewer\r")
    }

    /// RLM-001, RLM-002: a reviewer's limit of the whole subscription is the session's too: it
    /// moves as at any limit, and the new process is told to resume its reviewer.
    #[test]
    fn a_reviewer_limit_of_the_subscription_moves_as_any_limit() -> TestResult {
        a_cut_off_reviewer(true, b"/reviewer-limit-all yulius-reviewer\r")
    }

    /// RLM-002: a cut-off read in a turn of the session's thread moves it at that turn's end
    /// (its Stop hook), not while it works.
    #[test]
    fn a_reviewer_cut_off_in_a_turn_moves_the_session_at_its_end() -> TestResult {
        a_cut_off_reviewer(true, b"/reviewer-limit-turn yulius-reviewer\r")
    }

    /// SPEC-493 RLM-003 (AC-19) end to end: the session's subscription has room for Opus but 3
    /// points left for Fable, another has room for both. The Fable reviewer's start is refused,
    /// the session moves, and its first prompt there asks for the review again.
    #[test]
    fn a_round_about_to_run_out_moves_the_session_first() -> TestResult {
        let fake =
            locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
        let (_dir, data) = temp()?;
        let rt = runtime()?;
        rt.block_on(async {
            let probe = Arc::new(ScriptedUsage::default());
            let agent = FakeAgentOptions {
                resume_prompts: true,
                ..hooked()
            };
            let (app, _port) = start_with(&data, &fake, agent, 0, probe.clone(), |config| {
                config.claude_code_queue_record_ms = 0;
            })
            .await?;
            let (sandbox, user) = seed(&app, &data, &["a-first", "b-second"]).await?;
            let mut first = usage(5.0, 4 * HOUR, 10.0, 100);
            first.weekly_fable = Some(UsageWindow {
                used_percent: Some(95.0),
                window_minutes: Some(10_080),
                resets_at: first.secondary.as_ref().and_then(|w| w.resets_at),
            });
            probe.set("a-first", first);
            probe.set("b-second", usage(40.0, 4 * HOUR, 50.0, 100));
            refresh(&app, &user, "a-first").await?;
            refresh(&app, &user, "b-second").await?;
            let id = new_task(&app, &sandbox, &user, "Review it", TaskMode::Prompt).await?;
            ready(&app, &id).await?;
            assert_eq!(run_accounts(&app, &id).await, [Some("a-first".to_owned())]);
            app.send_input(&id, b"/agent yulius-reviewer\r").await?;
            live_run(&app, &id, 2).await?;
            assert_eq!(
                run_accounts(&app, &id).await,
                [Some("a-first".to_owned()), Some("b-second".to_owned())]
            );
            let asked = wait_for("the prompt", Duration::from_secs(30), || async {
                screen(&app, &id).contains("with room for Fable 5.2 for the review round you asked")
            })
            .await;
            assert!(asked.is_ok(), "{}", screen(&app, &id));
            wait_for("the completed switch", Duration::from_secs(30), || async {
                app.deps.store.switches(&id).await.is_ok_and(|switches| {
                    switches.iter().any(|switch| switch.outcome == "completed")
                })
            })
            .await?;
            let switches = app.deps.store.switches(&id).await?;
            assert_eq!(switches.len(), 1, "{switches:?}");
            assert_eq!(switches[0].reason, "review_room");
            stop(&app, &id).await?;
            Ok::<_, Box<dyn std::error::Error>>(())
        })?;
        rt.shutdown_timeout(Duration::from_secs(5));
        Ok(())
    }

    /// Review round 1 of fix 7: a cut-off reviewer superseded by a new one is forgotten (here
    /// the new start moves the session for Fable, REV-007, whose prompt asks for the review
    /// again and names no reviewer to resume); a subagent that is no round's, refused before any
    /// answer, is no one's limit: nothing moves.
    #[test]
    fn a_superseded_reviewer_and_an_unknown_subagent_change_nothing_else() -> TestResult {
        for (superseded, cut_off) in [
            (true, &b"/reviewer-limit yulius-reviewer\r"[..]),
            (false, &b"/reviewer-limit-first Explore\r"[..]),
        ] {
            let fake =
                locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
            let (_dir, data) = temp()?;
            let rt = runtime()?;
            rt.block_on(async {
                let probe = Arc::new(ScriptedUsage::default());
                let agent = FakeAgentOptions {
                    resume_prompts: true,
                    transcript: true,
                    ..hooked()
                };
                let (app, _port) = start_with(&data, &fake, agent, 0, probe.clone(), |config| {
                    config.claude_code_queue_record_ms = 0;
                })
                .await?;
                let (sandbox, user) = seed(&app, &data, &["a-first", "b-second"]).await?;
                probe.set("a-first", usage(5.0, 4 * HOUR, 10.0, 100));
                probe.set("b-second", usage(40.0, 4 * HOUR, 50.0, 100));
                refresh(&app, &user, "a-first").await?;
                refresh(&app, &user, "b-second").await?;
                let id = new_task(&app, &sandbox, &user, "Review it", TaskMode::Prompt).await?;
                ready(&app, &id).await?;
                // A first turn, read live: the transcript is followed from here on.
                app.send_input(&id, b"hello\r").await?;
                wait_for("the session waiting", Duration::from_secs(30), || async {
                    app.deps.store.task(&id).await.is_ok_and(|task| {
                        task.is_some_and(|task| {
                            task.activity == Some(yulius_domain::task::Activity::Waiting)
                        })
                    })
                })
                .await?;
                app.send_input(&id, cut_off).await?;
                let cut = wait_for("the cut-off", Duration::from_secs(30), || async {
                    screen(&app, &id).contains("CUT OFF")
                })
                .await;
                assert!(cut.is_ok(), "{}", screen(&app, &id));
                if superseded {
                    // The cut-off is read: the first subscription is out of Fable.
                    wait_for("Fable recorded", Duration::from_secs(30), || async {
                        app.deps
                            .store
                            .account(
                                &yulius_domain::ids::AccountId::parse("a-first")
                                    .unwrap_or_else(|_| unreachable!()),
                            )
                            .await
                            .ok()
                            .flatten()
                            .and_then(|a| a.usage_json)
                            .is_some_and(|json| {
                                yulius_app::usage_json::from_json(&json)
                                    .weekly_fable
                                    .and_then(|w| w.used_percent)
                                    == Some(100.0)
                            })
                    })
                    .await?;
                    app.send_input(&id, b"/agent yulius-reviewer\r").await?;
                    live_run(&app, &id, 2).await?;
                    let asked = wait_for("the prompt", Duration::from_secs(30), || async {
                        screen(&app, &id).contains("for the review round you asked for")
                    })
                    .await;
                    let seen = screen(&app, &id);
                    assert!(asked.is_ok(), "{seen}");
                    assert!(!seen.contains("SendMessage to `arev"), "{seen}");
                    let switches = app.deps.store.switches(&id).await?;
                    assert_eq!(switches[0].reason, "review_fable", "{switches:?}");
                } else {
                    app.send_input(&id, b"carry on\r").await?;
                    let moved = wait_for("no move", Duration::from_secs(10), || async {
                        app.deps
                            .store
                            .attempts(&id)
                            .await
                            .is_ok_and(|a| a.len() > 1)
                    })
                    .await;
                    assert!(moved.is_err(), "the session moved for a limit not its own");
                    let first = app
                        .deps
                        .store
                        .account(&yulius_domain::ids::AccountId::parse("a-first")?)
                        .await?
                        .ok_or("a-first")?;
                    assert_ne!(first.status, yulius_app::model::AccountStatus::Limited);
                }
                stop(&app, &id).await?;
                Ok::<_, Box<dyn std::error::Error>>(())
            })?;
            rt.shutdown_timeout(Duration::from_secs(5));
        }
        Ok(())
    }

    /// Whether the task's second run was started.
    async fn second_run_started(
        app: &yulius_app::context::App,
        id: &yulius_domain::ids::TaskId,
    ) -> bool {
        app.deps
            .store
            .attempts(id)
            .await
            .is_ok_and(|attempts| attempts.len() > 1)
    }

    /// The RLM-002 tests: the second subscription runs Fable or does not; `command` cuts the
    /// reviewer off while the session is idle or inside a turn.
    fn a_cut_off_reviewer(second_runs_fable: bool, command: &'static [u8]) -> TestResult {
        let fake =
            locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
        let (_dir, data) = temp()?;
        let rt = runtime()?;
        rt.block_on(async {
            let probe = Arc::new(ScriptedUsage::default());
            let agent = FakeAgentOptions {
                resume_prompts: true,
                transcript: true,
                ..hooked()
            };
            let (app, _port) = start_with(&data, &fake, agent, 0, probe.clone(), |config| {
                config.claude_code_queue_record_ms = 0;
            })
            .await?;
            let (sandbox, user) = seed(&app, &data, &["a-first", "b-second"]).await?;
            probe.set("a-first", usage(5.0, 4 * HOUR, 10.0, 100));
            let mut second = usage(40.0, 4 * HOUR, 50.0, 100);
            if !second_runs_fable {
                second.weekly_fable = Some(UsageWindow {
                    used_percent: Some(100.0),
                    window_minutes: Some(10_080),
                    resets_at: second.secondary.as_ref().and_then(|w| w.resets_at),
                });
            }
            probe.set("b-second", second);
            refresh(&app, &user, "a-first").await?;
            refresh(&app, &user, "b-second").await?;
            let id = new_task(&app, &sandbox, &user, "Review it", TaskMode::Prompt).await?;
            ready(&app, &id).await?;
            assert_eq!(run_accounts(&app, &id).await, [Some("a-first".to_owned())]);
            // A first turn, after which the session waits for its person (idle), as a session
            // does while its background reviewer reads.
            app.send_input(&id, b"hello\r").await?;
            wait_for("the session waiting", Duration::from_secs(30), || async {
                app.deps.store.task(&id).await.is_ok_and(|task| {
                    task.is_some_and(|task| {
                        task.activity == Some(yulius_domain::task::Activity::Waiting)
                    })
                })
            })
            .await?;
            // Sent as the agent works, not typed by a person (whose typing holds Yulius back).
            app.send_input(&id, command).await?;
            let cut = wait_for("the cut-off reviewer", Duration::from_secs(30), || async {
                screen(&app, &id).contains("CUT OFF")
            })
            .await;
            assert!(cut.is_ok(), "{}", screen(&app, &id));
            // While the session waits, nothing moves: only a turn's end does (review round 2 of
            // fix 7). Longer than a watcher poll and the background check.
            if command == b"/reviewer-limit yulius-reviewer\r" && second_runs_fable {
                let early = wait_for(
                    "a move outside a turn's end",
                    Duration::from_secs(6),
                    || second_run_started(&app, &id),
                )
                .await;
                assert!(early.is_err(), "the session moved while it waited");
                assert!(app.deps.store.switches(&id).await?.is_empty());
            }
            // The turn Claude Code starts with the reviewer's failed task notification; its
            // Stop hook makes the move (an in-turn cut-off ends its own turn).
            if !command.starts_with(b"/reviewer-limit-turn ") {
                app.send_input(&id, b"carry on\r").await?;
            }
            if !second_runs_fable {
                // Nowhere to move: the session stays and is told the round runs on Opus.
                let told = wait_for("the Opus note", Duration::from_secs(30), || async {
                    let seen = screen(&app, &id);
                    seen.contains("no Claude subscription can run Fable 5.2 now")
                        && seen.contains("this round then runs on Opus 5.5")
                })
                .await;
                assert!(told.is_ok(), "{}", screen(&app, &id));
                assert!(app.deps.store.switches(&id).await?.is_empty());
                assert_eq!(run_accounts(&app, &id).await, [Some("a-first".to_owned())]);
                stop(&app, &id).await?;
                return Ok::<_, Box<dyn std::error::Error>>(());
            }
            live_run(&app, &id, 2).await?;
            assert_eq!(
                run_accounts(&app, &id).await,
                [Some("a-first".to_owned()), Some("b-second".to_owned())]
            );
            let told = wait_for("the resume prompt", Duration::from_secs(30), || async {
                let seen = screen(&app, &id);
                seen.contains("Resume that reviewer now: SendMessage to `arev")
                    && seen.contains("Do not start a new reviewer for this round")
            })
            .await;
            assert!(told.is_ok(), "{}", screen(&app, &id));
            wait_for("the completed switch", Duration::from_secs(30), || async {
                app.deps.store.switches(&id).await.is_ok_and(|switches| {
                    switches.iter().any(|switch| switch.outcome == "completed")
                })
            })
            .await?;
            let switches = app.deps.store.switches(&id).await?;
            assert_eq!(switches.len(), 1, "{switches:?}");
            let first = app
                .deps
                .store
                .account(&yulius_domain::ids::AccountId::parse("a-first")?)
                .await?
                .ok_or("a-first")?;
            if command.starts_with(b"/reviewer-limit-all ") {
                // The subscription's own limit: the session moved as at any limit, and is told
                // to resume its reviewer all the same.
                assert_ne!(switches[0].reason, "review_resume", "{switches:?}");
                assert_eq!(first.status, yulius_app::model::AccountStatus::Limited);
                stop(&app, &id).await?;
                return Ok::<_, Box<dyn std::error::Error>>(());
            }
            assert_eq!(switches[0].reason, "review_resume");
            // The limit is Fable's on the first subscription, which Opus may still use.
            let read =
                yulius_app::usage_json::from_json(first.usage_json.as_deref().unwrap_or("{}"));
            assert_eq!(
                read.weekly_fable.and_then(|w| w.used_percent),
                Some(100.0),
                "{:?}",
                first.usage_json
            );
            // Nor is it the session's: Opus is not out there (review round 1 of fix 7).
            assert_ne!(
                read.weekly_opus.and_then(|w| w.used_percent),
                Some(100.0),
                "{:?}",
                first.usage_json
            );
            assert_ne!(first.status, yulius_app::model::AccountStatus::Limited);
            stop(&app, &id).await?;
            Ok::<_, Box<dyn std::error::Error>>(())
        })?;
        rt.shutdown_timeout(Duration::from_secs(5));
        Ok(())
    }
}

/// A reviewer's `SubagentStop` with `report`.
fn reviewer_stop(id: &str, agent: &str, report: &str) -> Value {
    json!({"hook_event_name": "SubagentStop", "agent_id": id, "agent_type": agent,
        "last_assistant_message": report})
}

/// SPEC-633 RRE-001 (#1726): the Agent tool's `PreToolUse` outlasted the hook client's 5 s, so
/// Claude Code started the reviewer and sent `SubagentStart` before Yulius inserted the round,
/// which no start then linked. The reviewer's report still ends that round with its verdict; a
/// stop of another agent type, or one sent before the round started, ends nothing.
#[tokio::test]
async fn a_stop_ends_the_round_its_start_came_before() -> TestResult {
    use crate::subscription_pool::hook_sent_at;
    let h = harness()?;
    seed(&h).await?;
    let account = add_account(&h, "acc-1", None).await?;
    let task = running_task(&h, TaskMode::Prompt, &account).await?;
    let before = h.app.now().0;
    h.clock
        .0
        .fetch_add(1_000, std::sync::atomic::Ordering::SeqCst);
    hook(
        &h.internal,
        "SubagentStart",
        json!({"hook_event_name": "SubagentStart", "agent_id": "rev-1",
            "agent_type": "yulius-reviewer"}),
    )
    .await?;
    let (code, _, answer) = hook(&h.internal, "PreToolUse", agent_start("yulius-reviewer")).await?;
    assert_eq!(code, 200, "{answer}");
    assert_eq!(reviewer_run(&answer, "yulius-reviewer"), "yulius-reviewer");
    let open = rounds(&h, &task, true).await?;
    assert_eq!(open.len(), 1);
    assert!(
        open[0].subagent_id.is_none(),
        "the start came before the round"
    );
    // Not the round's reviewer: another agent type, or a stop sent before the round started.
    hook(
        &h.internal,
        "SubagentStop",
        reviewer_stop("gp-1", "general-purpose", "Verdict: approve"),
    )
    .await?;
    hook_sent_at(
        &h.internal,
        "claude",
        "SubagentStop",
        reviewer_stop("old", "yulius-reviewer", "Verdict: approve"),
        Some(before),
    )
    .await?;
    let open = rounds(&h, &task, true).await?;
    assert!(open[0].ended_at.is_none() && open[0].subagent_id.is_none());
    hook(
        &h.internal,
        "SubagentStop",
        reviewer_stop(
            "rev-1",
            "yulius-reviewer",
            "Findings: none.\nVerdict: approve",
        ),
    )
    .await?;
    let open = rounds(&h, &task, true).await?;
    assert_eq!(open.len(), 1);
    assert_eq!(open[0].subagent_id.as_deref(), Some("rev-1"));
    assert!(open[0].ended_at.is_some(), "the reported round ended");
    assert_eq!(open[0].verdict.as_deref(), Some("approve"));
    Ok(())
}

/// SPEC-633 RRE-001: the fallback takes only a round no subagent holds. With round 2's reviewer
/// linked and running, an unknown reviewer's stop ends neither it nor anything else.
#[tokio::test]
async fn a_stop_of_an_unknown_reviewer_leaves_a_linked_round_alone() -> TestResult {
    let h = harness()?;
    seed(&h).await?;
    let account = add_account(&h, "acc-1", None).await?;
    let task = running_task(&h, TaskMode::Prompt, &account).await?;
    a_round(&h, "rev-1", Some("Verdict: request changes")).await?;
    a_round(&h, "rev-2", None).await?;
    hook(
        &h.internal,
        "SubagentStop",
        reviewer_stop("stranger", "yulius-reviewer", "Verdict: approve"),
    )
    .await?;
    let open = rounds(&h, &task, true).await?;
    assert_eq!(open.len(), 2);
    assert_eq!(open[1].subagent_id.as_deref(), Some("rev-2"));
    assert!(open[1].ended_at.is_none() && open[1].verdict.is_none());
    Ok(())
}

/// SPEC-633 RRE-002 (#1726): Yulius never took the reviewer's `SubagentStop` (it restarted while
/// the reviewer ran in the background, and the hook failed open). The completed notification in
/// the session's transcript ends the round with the report's verdict. An unknown agent's report
/// ends nothing; once the reviewer is resumed with `SendMessage`, a replay of its first report
/// leaves the continuation open, and its second report ends it.
#[tokio::test]
async fn a_reviewers_notification_ends_the_round_its_lost_stop_left_open() -> TestResult {
    use crate::subscription_pool::hook_sent_at;
    let h = harness()?;
    seed(&h).await?;
    let account = add_account(&h, "acc-1", None).await?;
    let task = running_task(&h, TaskMode::Prompt, &account).await?;
    a_round(&h, "rev-1", None).await?;
    let tick = || {
        h.clock
            .0
            .fetch_add(60_000, std::sync::atomic::Ordering::SeqCst);
        h.app.now()
    };
    let first = tick();
    h.app
        .review_reported_for_test(&task, "stranger", "Verdict: approve", Some(first))
        .await;
    assert!(rounds(&h, &task, true).await?[0].ended_at.is_none());
    h.app
        .review_reported_for_test(&task, "rev-1", "Findings…\n\nVerdict: approve", Some(first))
        .await;
    let open = rounds(&h, &task, true).await?;
    assert_eq!(open.len(), 1);
    assert_eq!(open[0].ended_at, Some(first));
    assert_eq!(open[0].verdict.as_deref(), Some("approve"));
    // Resumed after its report: its next round runs under the same id.
    let resumed = tick();
    hook_sent_at(
        &h.internal,
        "claude",
        "SubagentStart",
        json!({"agent_id": "rev-1", "agent_type": "yulius-reviewer"}),
        Some(resumed.0),
    )
    .await?;
    h.app
        .review_reported_for_test(&task, "rev-1", "Verdict: approve", Some(first))
        .await;
    let open = rounds(&h, &task, true).await?;
    assert_eq!(open.len(), 2);
    assert!(open[1].ended_at.is_none(), "a replayed first report");
    let second = tick();
    h.app
        .review_reported_for_test(&task, "rev-1", "Verdict: request changes", Some(second))
        .await;
    let open = rounds(&h, &task, true).await?;
    assert_eq!(open[1].ended_at, Some(second));
    assert_eq!(open[1].verdict.as_deref(), Some("request_changes"));
    // The stop arriving after all changes nothing.
    hook(
        &h.internal,
        "SubagentStop",
        reviewer_stop("rev-1", "yulius-reviewer", "Verdict: approve"),
    )
    .await?;
    let open = rounds(&h, &task, true).await?;
    assert_eq!(open[1].ended_at, Some(second));
    assert_eq!(open[1].verdict.as_deref(), Some("request_changes"));
    Ok(())
}

/// SPEC-633 RRE-003 (#1726): a round still open when its pull request merged carries the merge
/// on the delivery's view, which `delivery_status` reads as unreported.
#[tokio::test]
async fn a_round_its_merge_left_open_carries_the_merge() -> TestResult {
    use crate::support::{call, setup_superadmin_creds};
    use axum::http::Method;
    let h = harness()?;
    seed(&h).await?;
    let admin = setup_superadmin_creds(&h).await?;
    let account = add_account(&h, "acc-1", None).await?;
    let task = running_task(&h, TaskMode::Prompt, &account).await?;
    a_round(&h, "rev-1", Some("Verdict: request changes")).await?;
    a_round(&h, "rev-2", None).await?;
    let delivery = format!("/api/tasks/{task}/delivery");
    let open = call(&h.router, Method::GET, &delivery, None, &admin).await?;
    assert_eq!(open.status, 200, "{}", open.body);
    assert_eq!(open.body["review_rounds"][1]["merged_at"], Value::Null);
    let at = h.app.now();
    assert!(
        h.store
            .transition_delivery(
                &DeliveryStage::None,
                &DeliveryRecord {
                    task_id: task.clone(),
                    stage: DeliveryStage::MergedIntegration,
                    pr_number: Some(5),
                    updated_at: at,
                },
            )
            .await?
    );
    let merged = call(&h.router, Method::GET, &delivery, None, &admin).await?;
    assert_eq!(merged.status, 200, "{}", merged.body);
    let rows = merged.body["review_rounds"].as_array().ok_or("rounds")?;
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[1]["reported"], false);
    assert_eq!(
        rows[1]["merged_at"],
        yulius_domain::time::format_rfc3339(at)
    );
    Ok(())
}

/// SPEC-633 RRE-001, review round 1: the `PreToolUse` answer was lost, so the reviewer ran as
/// asked (`yulius-reviewer`) while Yulius recorded round 3 for the one it decided
/// (`yulius-reviewer-opus`). The stop of the reviewer that ran still ends that round, which
/// then names the reviewer and model that ran.
#[tokio::test]
async fn a_stop_ends_an_unlinked_round_that_decided_another_reviewer() -> TestResult {
    use std::sync::atomic::Ordering;
    let h = harness()?;
    seed(&h).await?;
    let account = add_account(&h, "acc-1", None).await?;
    let task = running_task(&h, TaskMode::Prompt, &account).await?;
    a_round(&h, "rev-1", Some("Verdict: request changes")).await?;
    a_round(&h, "rev-2", Some("Verdict: request changes")).await?;
    h.clock.0.fetch_add(1_000, Ordering::SeqCst);
    hook(
        &h.internal,
        "SubagentStart",
        json!({"hook_event_name": "SubagentStart", "agent_id": "rev-3",
            "agent_type": "yulius-reviewer"}),
    )
    .await?;
    let (code, _, answer) = hook(&h.internal, "PreToolUse", agent_start("yulius-reviewer")).await?;
    assert_eq!(code, 200, "{answer}");
    assert_eq!(
        reviewer_run(&answer, "yulius-reviewer"),
        "yulius-reviewer-opus"
    );
    h.clock.0.fetch_add(1_000, Ordering::SeqCst);
    hook(
        &h.internal,
        "SubagentStop",
        reviewer_stop(
            "rev-3",
            "yulius-reviewer",
            "Findings: none.\nVerdict: approve",
        ),
    )
    .await?;
    let open = rounds(&h, &task, true).await?;
    assert_eq!(open.len(), 3);
    let third = &open[2];
    assert_eq!(third.round, 3);
    assert_eq!(third.subagent_id.as_deref(), Some("rev-3"));
    assert!(third.ended_at.is_some(), "the reported round ended");
    assert_eq!(third.verdict.as_deref(), Some("approve"));
    assert_eq!(
        (third.agent.as_str(), third.model.as_str()),
        ("yulius-reviewer", "claude-fable-5-2"),
        "the round names the reviewer that ran"
    );
    Ok(())
}

/// SPEC-633 RRE-001, review round 1: a round decided as `no_fable` (Opus, since the
/// subscription cannot run Fable) whose answer was lost ran the Fable reviewer as asked. Once its
/// stop ends it, it names that reviewer and no longer counts as a round without Fable.
#[tokio::test]
async fn an_unlinked_no_fable_round_its_asked_reviewer_reported_is_no_fable_no_more() -> TestResult
{
    use std::sync::atomic::Ordering;
    let h = harness()?;
    seed(&h).await?;
    let full = json!({"weekly_fable": {"used_percent": 100.0, "window_minutes": 10080,
        "resets_at": 4_102_444_800_000_i64}});
    let account = add_account(&h, "acc-1", Some(full.to_string())).await?;
    let task = running_task(&h, TaskMode::Prompt, &account).await?;
    hook(
        &h.internal,
        "SubagentStart",
        json!({"agent_id": "rev-1", "agent_type": "yulius-reviewer"}),
    )
    .await?;
    let (_, _, answer) = hook(&h.internal, "PreToolUse", agent_start("yulius-reviewer")).await?;
    assert_eq!(
        reviewer_run(&answer, "yulius-reviewer"),
        "yulius-reviewer-opus"
    );
    h.clock.0.fetch_add(1_000, Ordering::SeqCst);
    hook(
        &h.internal,
        "SubagentStop",
        reviewer_stop("rev-1", "yulius-reviewer", "Verdict: approve"),
    )
    .await?;
    let open = rounds(&h, &task, true).await?;
    assert_eq!(
        (
            open[0].agent.as_str(),
            open[0].model.as_str(),
            open[0].reason.as_str(),
            open[0].verdict.as_deref()
        ),
        (
            "yulius-reviewer",
            "claude-fable-5-2",
            "round",
            Some("approve")
        )
    );
    Ok(())
}

/// SPEC-633 RRE-001, review round 2: when the reviewer that ran is the one the round decided (the
/// Opus reviewer asked for by name on a subscription out of Fable), the stop fallback keeps the
/// round's reason.
#[tokio::test]
async fn an_unlinked_round_its_decided_reviewer_reported_keeps_its_reason() -> TestResult {
    use std::sync::atomic::Ordering;
    let h = harness()?;
    seed(&h).await?;
    let full = json!({"weekly_fable": {"used_percent": 100.0, "window_minutes": 10080,
        "resets_at": 4_102_444_800_000_i64}});
    let account = add_account(&h, "acc-1", Some(full.to_string())).await?;
    let task = running_task(&h, TaskMode::Prompt, &account).await?;
    hook(
        &h.internal,
        "SubagentStart",
        json!({"agent_id": "rev-1", "agent_type": "yulius-reviewer-opus"}),
    )
    .await?;
    let (_, _, answer) = hook(
        &h.internal,
        "PreToolUse",
        agent_start("yulius-reviewer-opus"),
    )
    .await?;
    assert_eq!(
        reviewer_run(&answer, "yulius-reviewer-opus"),
        "yulius-reviewer-opus"
    );
    h.clock.0.fetch_add(1_000, Ordering::SeqCst);
    hook(
        &h.internal,
        "SubagentStop",
        reviewer_stop("rev-1", "yulius-reviewer-opus", "Verdict: approve"),
    )
    .await?;
    let open = rounds(&h, &task, true).await?;
    assert_eq!(
        (
            open[0].agent.as_str(),
            open[0].model.as_str(),
            open[0].reason.as_str(),
            open[0].subagent_id.as_deref(),
            open[0].verdict.as_deref()
        ),
        (
            "yulius-reviewer-opus",
            "claude-opus-5-5",
            "no_fable",
            Some("rev-1"),
            Some("approve")
        )
    );
    Ok(())
}
