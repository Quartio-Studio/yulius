//! The independent code review's rounds as Yulius sees them through a task's hooks (SPEC-493
//! REV-002 to REV-004): the Agent tool's `PreToolUse` numbers a round and picks its reviewer,
//! `SubagentStart` and `SubagentStop` link and end it, and a merge closes a pull request's
//! rounds.

use crate::subscription_pool::{add_account, hook, running_task, seed};
use crate::support::{TestResult, harness};

use serde_json::{Value, json};
use yulius_app::delivery::DeliveryRecord;
use yulius_app::model::{ReviewRoundRecord, TaskMode};
use yulius_app::ports::store::CoordinationStore;
use yulius_app::ports::store_ext::DeliveryStore;
use yulius_domain::delivery::DeliveryStage;
use yulius_domain::ids::TaskId;
use yulius_domain::time::Timestamp;

/// A round as the test reads it: number, reviewer, why, subagent, verdict.
type Seen<'a> = (u32, &'a str, &'a str, Option<&'a str>, Option<&'a str>);

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
        .note_review_subagent(&task, "rev-1", Some("claude-fable-5-2"), false)
        .await?;
    store
        .note_review_subagent(&task, "rev-1", None, true)
        .await?;
    // A subagent that is no round's changes nothing.
    store
        .note_review_subagent(&task, "other", Some("claude-haiku-4-5"), true)
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

/// SPEC-493 REV-005: the merge record names the rounds of the pull request that merged, and
/// says so when no recorded round approved it; the rounds of the task's earlier pull request
/// are not this one's.
#[tokio::test]
async fn the_merge_record_names_this_pull_requests_rounds() -> TestResult {
    use yulius_app::ports::store::{AuditStore as _, SandboxStore as _, TaskStore as _};
    let h = harness()?;
    seed(&h).await?;
    let account = add_account(&h, "acc-1", None).await?;
    let task = running_task(&h, TaskMode::Prompt, &account).await?;
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
    h.app.record_review_models(&task, Some(4)).await;
    let record = h.store.task(&task).await?.ok_or("task")?;
    let sandbox = h
        .store
        .sandbox(&record.sandbox_id)
        .await?
        .ok_or("sandbox")?;
    let details = h
        .store
        .audit_details(
            &format!("{}#{}", sandbox.slug, record.number),
            "delivery.review.models",
            5,
        )
        .await?;
    assert_eq!(details.len(), 1, "{details:?}");
    let detail = &details[0];
    assert!(
        detail.starts_with("pull request #4: written by "),
        "{detail}"
    );
    assert!(
        detail.contains("review rounds: 1 claude-fable-5-2 request_changes")
            && detail.contains("; 2 claude-fable-5-2 no report")
            && detail.ends_with("; no recorded review round approved it"),
        "{detail}"
    );
    assert!(
        !detail.contains(" approve "),
        "pull request #3's round is not this one's: {detail}"
    );
    Ok(())
}
