//! Task intake and chaining (SPEC-001 TSK-011..013, AC-TSK-10..12; SPEC-002 ADR-16): the split
//! check and proposal of a new task, chained tasks that start when their predecessor is Done and
//! need a person when it ends otherwise, and the `UserPromptSubmit` scope check with the
//! decisions on a held prompt. Jev and Claude are fakes: no real call is made.

use crate::support;

use std::path::Path;
use std::process::Command;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use axum::http::{Method, StatusCode};
use serde_json::{Value, json};
use support::{
    Creds, Harness, HarnessOptions, Resp, TestResult, call, harness, harness_with, perms,
    seed_github_user, seed_live_attempt, seed_sandbox, seed_task,
};
use yulius_app::intake::IntakeServices;
use yulius_app::model::{
    AccountRecord, AccountStatus, AttemptRecord, LaunchKind, Secret, StepRecord, TaskRecord,
};
use yulius_app::ports::intake::{ChoiceEvaluator, JevFailure, ProposerFailure, SplitProposer};
use yulius_app::ports::store::{AccountStore, CoordinationStore, TaskStore};
use yulius_app::ports::store_ext::DeliveryStore;
use yulius_domain::harness::{Harness as Agent, Provider};
use yulius_domain::ids::{AccountId, AttemptId, SandboxId, StepId, TaskId, UserId};
use yulius_domain::intake::{ChoiceAnswer, ChoiceQuestion, ProposedTask};
use yulius_domain::task::{Activity, AttemptState, StopReason, TaskStatus};
use yulius_domain::time::Timestamp;

const TOKEN: &str = "task-token-for-intake-tests-0123456789";

/// Michał's own 10-item goal of 2026-10-10, shaped: UI fixes plus backend features.
const TEN_ITEMS: &str = "Make it the final state, everything below has to land.\n\
    1. The attention bar must be opaque over the terminal.\n\
    2. The tools rail pills overflow the rail on narrow windows.\n\
    3. Let the editor wrap lines when the user wants it.\n\
    4. The task terminal should appear once at its final size.\n\
    5. The messages panel shows the wrong unread count.\n\
    6. Subscription pool: route new tasks past exhausted accounts.\n\
    7. Make Done reliable on the server side.\n\
    8. Task intake with Jev split detection.\n\
    9. Chaining tasks to run right after the current one.\n\
    10. Usage refresh must honour Retry-After.";

/// A fake Jev: the split question says "several" for requests mixing UI and backend work; the
/// breadth question says "broad" for the whole product; the scope question answers by keywords
/// of the new message. Counts calls; can be "down", or `held`: each call says it `entered` and
/// answers once `release`d (a scope check slower than the hook client's wait).
#[derive(Default)]
struct FakeJev {
    calls: AtomicUsize,
    down: AtomicBool,
    held: AtomicBool,
    entered: tokio::sync::Notify,
    release: tokio::sync::Notify,
}

fn answer(question: &ChoiceQuestion<'_>, winner: &str, p: f64) -> ChoiceAnswer {
    let rest = (1.0 - p) / (question.options.len() - 1) as f64;
    ChoiceAnswer {
        choice: winner.to_owned(),
        confidence: p,
        probabilities: question
            .options
            .iter()
            .map(|o| (o.key.to_owned(), if o.key == winner { p } else { rest }))
            .collect(),
    }
}

#[async_trait]
impl ChoiceEvaluator for FakeJev {
    async fn evaluate(
        &self,
        state: &str,
        question: &ChoiceQuestion<'_>,
        _deadline: Duration,
    ) -> Result<ChoiceAnswer, JevFailure> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if self.held.load(Ordering::SeqCst) {
            self.entered.notify_one();
            self.release.notified().await;
        }
        if self.down.load(Ordering::SeqCst) {
            return Err(JevFailure::Unavailable);
        }
        Ok(match question.id {
            "split" if state.contains("Subscription pool") => answer(question, "several", 0.96),
            "split" => answer(question, "one", 0.97),
            "breadth" if state.contains("whole product") => answer(question, "broad", 0.93),
            "breadth" if state.contains("maybe broad") => answer(question, "broad", 0.7),
            "breadth" => answer(question, "focused", 0.95),
            _ => {
                let message = state.split("NEW MESSAGE").nth(1).unwrap_or_default();
                if message.contains("dark mode") {
                    answer(question, "new_scope", 0.92)
                } else if message.contains("drop the export") {
                    answer(question, "drops_scope", 0.9)
                } else if message.contains("rewrite everything") {
                    answer(question, "redirect", 0.95)
                } else if message.contains("maybe") {
                    answer(question, "new_scope", 0.6)
                } else {
                    answer(question, "context", 0.93)
                }
            }
        })
    }
}

/// A fake Claude proposer returning a fixed proposal.
struct FakeClaude(Mutex<Vec<ProposedTask>>);

#[async_trait]
impl SplitProposer for FakeClaude {
    async fn propose(
        &self,
        credential: &Secret,
        _request: &str,
    ) -> Result<Vec<ProposedTask>, ProposerFailure> {
        assert_eq!(
            credential.expose(),
            "oauth-token",
            "the account's own credential"
        );
        Ok(self.0.lock().map(|t| t.clone()).unwrap_or_default())
    }
}

fn workstreams() -> Vec<ProposedTask> {
    let items: Vec<&str> = TEN_ITEMS.lines().skip(1).map(str::trim).collect();
    vec![
        ProposedTask {
            title: "UX: layout, terminal and messages fixes".into(),
            prompt: items[..5].join("\n"),
            after_previous: false,
        },
        ProposedTask {
            title: "Backend: subscription pool and task intake".into(),
            prompt: items[5..].join("\n"),
            after_previous: false,
        },
    ]
}

struct Intake {
    h: Harness,
    alice: Creds,
    jev: Arc<FakeJev>,
    claude: Arc<FakeClaude>,
}

async fn intake_with(h: Harness) -> Result<Intake, Box<dyn std::error::Error>> {
    let alice = seed_github_user(&h, "u-alice", "alice").await?;
    seed_sandbox(&h, "s1", "fixture", "u-alice").await?;
    h.github.set_perm("alice", "acme/fixture", perms("write"));
    h.store
        .insert_account(&AccountRecord {
            id: AccountId::parse("acc-1")?,
            provider: Provider::Anthropic,
            label: "acc-1".into(),
            credential: Some(h.app.deps.crypto.seal(b"oauth-token")?),
            status: AccountStatus::Ok,
            usage_json: None,
            last_error: None,
            created_at: Timestamp(1),
            updated_at: Timestamp(1),
            email: None,
            email_checked_at: None,
        })
        .await?;
    let jev = Arc::new(FakeJev::default());
    let claude = Arc::new(FakeClaude(Mutex::new(workstreams())));
    h.app.set_intake(IntakeServices::new(
        Some(jev.clone() as Arc<dyn ChoiceEvaluator>),
        Some(claude.clone() as Arc<dyn SplitProposer>),
    ));
    Ok(Intake {
        h,
        alice,
        jev,
        claude,
    })
}

async fn post(i: &Intake, path: &str, body: Value) -> Result<Resp, Box<dyn std::error::Error>> {
    call(&i.h.router, Method::POST, path, Some(body), &i.alice).await
}

async fn summary(i: &Intake, task: &str) -> Result<Value, Box<dyn std::error::Error>> {
    let detail = call(
        &i.h.router,
        Method::GET,
        &format!("/api/tasks/{task}"),
        None,
        &i.alice,
    )
    .await?;
    Ok(detail.body)
}

/// AC-TSK-10: short prompts are one task without asking Jev; a request mixing UI and backend
/// work gets a two-workstream proposal; one task per item, too many tasks or a single task are
/// never shown as a split; without Jev or with Jev down the answer is "one".
#[tokio::test]
async fn split_check_proposes_workstreams_and_defaults_to_one_task() -> TestResult {
    let i = intake_with(harness()?).await?;
    let check = "/api/sandboxes/s1/intake/check";
    let single = "Add a nickname field.\n1. add the column\n2. migrate the table\n3. show it in the profile UI";
    let r = post(&i, check, json!({"prompt": single})).await?;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.body);
    assert_eq!(
        r.body,
        json!({"verdict": "one", "checked": true, "supertask": false})
    );
    assert_eq!(
        i.jev.calls.load(Ordering::SeqCst),
        1,
        "a short prompt is asked about breadth only, never about a split"
    );

    let r = post(&i, check, json!({"prompt": TEN_ITEMS})).await?;
    assert_eq!(
        r.body,
        json!({"verdict": "several", "checked": true, "supertask": false})
    );
    let long_one = format!(
        "Polish the checkout page. {}",
        "Keep the same thread of work. ".repeat(10)
    );
    let r = post(&i, check, json!({"prompt": long_one})).await?;
    assert_eq!(
        r.body,
        json!({"verdict": "one", "checked": true, "supertask": false})
    );

    let proposal = "/api/sandboxes/s1/intake/proposal";
    let r = post(&i, proposal, json!({"prompt": TEN_ITEMS})).await?;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.body);
    let tasks = r.body["tasks"].as_array().ok_or("tasks")?;
    assert_eq!(tasks.len(), 2, "UX and backend, not 10: {tasks:?}");
    assert!(
        tasks[0]["title"]
            .as_str()
            .is_some_and(|t| t.starts_with("UX"))
    );
    assert!(
        tasks[1]["prompt"]
            .as_str()
            .is_some_and(|p| p.contains("Usage refresh must honour Retry-After."))
    );

    // One task per item (10) is rejected rather than shown.
    let per_item: Vec<ProposedTask> = TEN_ITEMS
        .lines()
        .skip(1)
        .map(|l| ProposedTask {
            title: l.trim().chars().take(20).collect(),
            prompt: l.trim().to_owned(),
            after_previous: false,
        })
        .collect();
    *i.claude.0.lock().map_err(|_| "lock")? = per_item;
    let r = post(&i, proposal, json!({"prompt": TEN_ITEMS})).await?;
    assert_eq!(r.status, StatusCode::BAD_GATEWAY, "{:?}", r.body);
    // A single task means "keep it as one".
    *i.claude.0.lock().map_err(|_| "lock")? = vec![ProposedTask {
        title: "All of it".into(),
        prompt: TEN_ITEMS.into(),
        after_previous: false,
    }];
    let r = post(&i, proposal, json!({"prompt": TEN_ITEMS})).await?;
    assert_eq!(r.body, json!({"tasks": []}));

    // Jev down: one task, unchecked.
    i.jev.down.store(true, Ordering::SeqCst);
    let r = post(&i, check, json!({"prompt": TEN_ITEMS})).await?;
    assert_eq!(
        r.body,
        json!({"verdict": "one", "checked": false, "supertask": false})
    );

    // Read users cannot ask; an instance without intake checks nothing and proposes nothing.
    let bob = seed_github_user(&i.h, "u-bob", "bob").await?;
    i.h.github.set_perm("bob", "acme/fixture", perms("read"));
    let r = call(
        &i.h.router,
        Method::POST,
        check,
        Some(json!({"prompt": TEN_ITEMS})),
        &bob,
    )
    .await?;
    assert_eq!(r.status, StatusCode::FORBIDDEN);
    let plain = harness()?;
    let carol = seed_github_user(&plain, "u-carol", "carol").await?;
    seed_sandbox(&plain, "s1", "fixture", "u-carol").await?;
    plain
        .github
        .set_perm("carol", "acme/fixture", perms("write"));
    let r = call(
        &plain.router,
        Method::POST,
        check,
        Some(json!({"prompt": TEN_ITEMS})),
        &carol,
    )
    .await?;
    assert_eq!(
        r.body,
        json!({"verdict": "one", "checked": false, "supertask": false})
    );
    let r = call(
        &plain.router,
        Method::POST,
        proposal,
        Some(json!({"prompt": TEN_ITEMS})),
        &carol,
    )
    .await?;
    assert_eq!(r.status, StatusCode::CONFLICT);
    Ok(())
}

/// SPEC-188 OSP-004: broad work is proposed as a supertask, also when it is short; work that
/// splits never is; a prompt under 3 words, an unsure Jev and Jev down answer false.
#[tokio::test]
async fn intake_check_proposes_a_supertask_for_broad_work_that_does_not_split() -> TestResult {
    let i = intake_with(harness()?).await?;
    let check = "/api/sandboxes/s1/intake/check";
    let r = post(
        &i,
        check,
        json!({"prompt": "Harden the security of the whole product"}),
    )
    .await?;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.body);
    assert_eq!(
        r.body,
        json!({"verdict": "one", "checked": true, "supertask": true})
    );
    // Broad, but it splits ("fix all GitHub issues" is split, not run alone).
    let splits = format!("{TEN_ITEMS}\n11. Then harden the whole product.");
    let r = post(&i, check, json!({"prompt": splits})).await?;
    assert_eq!(
        r.body,
        json!({"verdict": "several", "checked": true, "supertask": false})
    );
    let r = post(&i, check, json!({"prompt": "this is maybe broad work"})).await?;
    assert_eq!(
        r.body,
        json!({"verdict": "one", "checked": true, "supertask": false}),
        "an unsure answer proposes nothing"
    );
    let calls = i.jev.calls.load(Ordering::SeqCst);
    let r = post(&i, check, json!({"prompt": "whole product"})).await?;
    assert_eq!(
        r.body,
        json!({"verdict": "one", "checked": false, "supertask": false})
    );
    assert_eq!(
        i.jev.calls.load(Ordering::SeqCst),
        calls,
        "a prompt under 3 words is never sent"
    );
    i.jev.down.store(true, Ordering::SeqCst);
    let r = post(
        &i,
        check,
        json!({"prompt": "Harden the security of the whole product"}),
    )
    .await?;
    assert_eq!(
        r.body,
        json!({"verdict": "one", "checked": false, "supertask": false})
    );
    Ok(())
}

/// A fake Jev for the check's two questions: each call waits until the other question is asked
/// too (a sequential check would never get there and time out), then answers "several" and
/// "broad" — or fails the split question when `split_down` is set.
struct TogetherJev {
    both: tokio::sync::Barrier,
    split_down: bool,
}

#[async_trait]
impl ChoiceEvaluator for TogetherJev {
    async fn evaluate(
        &self,
        _state: &str,
        question: &ChoiceQuestion<'_>,
        deadline: Duration,
    ) -> Result<ChoiceAnswer, JevFailure> {
        tokio::time::timeout(deadline, self.both.wait())
            .await
            .map_err(|_| JevFailure::Timeout)?;
        match question.id {
            "split" if self.split_down => Err(JevFailure::Unavailable),
            "split" => Ok(answer(question, "several", 0.96)),
            _ => Ok(answer(question, "broad", 0.93)),
        }
    }
}

/// SPEC-188 OSP-004: the split and breadth questions are asked at once, within one deadline,
/// and the check counts as checked when only one of them answers.
#[tokio::test]
async fn intake_check_asks_split_and_breadth_at_once() -> TestResult {
    let long_broad = format!(
        "Harden the whole product. {}",
        "Review every endpoint, header and secret. ".repeat(8)
    );
    for (split_down, expected) in [
        (
            false,
            json!({"verdict": "several", "checked": true, "supertask": false}),
        ),
        (
            true,
            json!({"verdict": "one", "checked": true, "supertask": true}),
        ),
    ] {
        let jev = Arc::new(TogetherJev {
            both: tokio::sync::Barrier::new(2),
            split_down,
        });
        let h = harness()?;
        let alice = seed_github_user(&h, "u-alice", "alice").await?;
        seed_sandbox(&h, "s1", "fixture", "u-alice").await?;
        h.github.set_perm("alice", "acme/fixture", perms("write"));
        h.app.set_intake(IntakeServices::new(
            Some(jev as Arc<dyn ChoiceEvaluator>),
            None,
        ));
        let r = call(
            &h.router,
            Method::POST,
            "/api/sandboxes/s1/intake/check",
            Some(json!({"prompt": long_broad})),
            &alice,
        )
        .await?;
        assert_eq!(r.status, StatusCode::OK, "{:?}", r.body);
        assert_eq!(r.body, expected, "split down: {split_down}");
    }
    Ok(())
}

/// AC-TSK-10/11: the composer's actions — parallel tasks, a chain, one task — through the
/// public API; a chained task waits as `chained` and both tasks show the link.
#[tokio::test]
async fn split_card_creates_parallel_tasks_or_a_chain() -> TestResult {
    // No session slots: started tasks wait as queued, so nothing needs a real agent.
    let h = harness_with(HarnessOptions {
        configure: Some(Box::new(|c| c.max_live_sessions = 0)),
        ..HarnessOptions::default()
    })?;
    let i = intake_with(h).await?;
    let create = "/api/sandboxes/s1/tasks";
    let first = post(
        &i,
        create,
        json!({"prompt": "UX: layout fixes", "title": "UX: layout fixes"}),
    )
    .await?;
    assert_eq!(first.status, StatusCode::CREATED, "{:?}", first.body);
    let first_id = first.body["id"].as_str().ok_or("id")?.to_owned();
    let second = post(
        &i,
        create,
        json!({"prompt": "Backend: subscription pool", "after_task_id": first_id}),
    )
    .await?;
    assert_eq!(second.status, StatusCode::CREATED, "{:?}", second.body);
    assert_eq!(second.body["status"], "chained");
    assert_eq!(second.body["after"]["number"], first.body["number"]);
    assert_eq!(second.body["after"]["blocked"], Value::Null);
    let first_now = summary(&i, &first_id).await?;
    assert_eq!(
        first_now["next"][0]["number"], second.body["number"],
        "the chain shows on both"
    );

    // Chaining after an ended or foreign task is refused.
    let ended = seed_task(&i.h, "t-ended", "s1", "u-alice", TaskStatus::Closed, None).await?;
    let r = post(
        &i,
        create,
        json!({"prompt": "after it", "after_task_id": ended.id.as_str()}),
    )
    .await?;
    assert_eq!(r.status, StatusCode::UNPROCESSABLE_ENTITY, "{:?}", r.body);
    let r = post(
        &i,
        create,
        json!({"prompt": "after nothing", "after_task_id": "nope"}),
    )
    .await?;
    assert_eq!(r.status, StatusCode::UNPROCESSABLE_ENTITY);
    Ok(())
}

/// AC-TSK-11: a chained task starts once its predecessor is Done; when the predecessor ends any
/// other way it needs a person (never starts by itself); Start now and Cancel work; continuing
/// the predecessor puts the chain back to waiting; deleting it blocks the chain too.
#[tokio::test]
async fn chained_tasks_start_on_done_and_wait_for_a_person_otherwise() -> TestResult {
    let h = harness_with(HarnessOptions {
        configure: Some(Box::new(|c| c.max_live_sessions = 0)),
        ..HarnessOptions::default()
    })?;
    let i = intake_with(h).await?;
    let store = &i.h.store;
    let pred = seed_task(&i.h, "t-pred", "s1", "u-alice", TaskStatus::Running, None).await?;
    let create = "/api/sandboxes/s1/tasks";
    let next = post(
        &i,
        create,
        json!({"prompt": "next step", "after_task_id": "t-pred"}),
    )
    .await?;
    let next_id = next.body["id"].as_str().ok_or("id")?.to_owned();
    assert_eq!(next.body["status"], "chained");

    // Working, then waiting without a pull request: not Done, still waiting.
    i.h.app.advance_chains(&pred.id).await;
    store
        .set_activity(&pred.id, Some(Activity::Waiting), i.h.app.now())
        .await?;
    i.h.app.advance_chains(&pred.id).await;
    assert_eq!(summary(&i, &next_id).await?["status"], "chained");

    // Done: a finished turn and a merged pull request.
    store
        .transition_delivery(
            &yulius_domain::delivery::DeliveryStage::None,
            &yulius_app::delivery::DeliveryRecord {
                task_id: pred.id.clone(),
                stage: yulius_domain::delivery::DeliveryStage::MergedIntegration,
                pr_number: Some(7),
                updated_at: i.h.app.now(),
            },
        )
        .await?;
    i.h.app.advance_chains(&pred.id).await;
    let started = summary(&i, &next_id).await?;
    assert_ne!(started["status"], "chained", "{started}");
    assert_eq!(
        started["status"], "queued",
        "released into the normal start"
    );

    // A predecessor that crashes: the chained task needs a person and does not start.
    let pred2 = seed_task(&i.h, "t-pred2", "s1", "u-alice", TaskStatus::Running, None).await?;
    let waiting = post(
        &i,
        create,
        json!({"prompt": "after the crash", "after_task_id": "t-pred2"}),
    )
    .await?;
    let waiting_id = waiting.body["id"].as_str().ok_or("id")?.to_owned();
    store
        .transition_task(
            &pred2.id,
            Some(TaskStatus::Running),
            TaskStatus::Stopped(StopReason::Crashed),
            None,
            i.h.app.now(),
        )
        .await?;
    i.h.app.advance_chains(&pred2.id).await;
    let blocked = summary(&i, &waiting_id).await?;
    assert_eq!(blocked["status"], "chained");
    let reason = blocked["after"]["blocked"].as_str().ok_or("blocked")?;
    assert!(
        reason.contains("stopped unexpectedly before it was done"),
        "{reason}"
    );
    // The background sweep does not start it either.
    i.h.app.advance_all_chains().await;
    assert_eq!(summary(&i, &waiting_id).await?["status"], "chained");

    // Continuing the predecessor puts it back to waiting (it queues: no session slots).
    let r = post(&i, "/api/tasks/t-pred2/continue", json!({})).await?;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.body);
    assert_eq!(
        summary(&i, &waiting_id).await?["after"]["blocked"],
        Value::Null
    );

    // Start now releases it whatever the predecessor does.
    let r = post(&i, &format!("/api/tasks/{waiting_id}/start-now"), json!({})).await?;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.body);
    assert_eq!(r.body["status"], "queued");
    let again = post(&i, &format!("/api/tasks/{waiting_id}/start-now"), json!({})).await?;
    assert_eq!(
        again.status,
        StatusCode::CONFLICT,
        "only a waiting chained task"
    );

    // Cancel a waiting chained task; deleting a predecessor blocks its chain.
    let pred3 = seed_task(&i.h, "t-pred3", "s1", "u-alice", TaskStatus::Closed, None).await?;
    store
        .transition_task(&pred3.id, None, TaskStatus::Running, None, i.h.app.now())
        .await?;
    let a = post(
        &i,
        create,
        json!({"prompt": "cancel me", "after_task_id": "t-pred3"}),
    )
    .await?;
    let b = post(
        &i,
        create,
        json!({"prompt": "orphaned", "after_task_id": "t-pred3"}),
    )
    .await?;
    let (a_id, b_id) = (
        a.body["id"].as_str().ok_or("id")?.to_owned(),
        b.body["id"].as_str().ok_or("id")?.to_owned(),
    );
    let r = post(&i, &format!("/api/tasks/{a_id}/stop"), json!({})).await?;
    assert_eq!(r.body["status"], "stopped_cancelled", "{:?}", r.body);
    store
        .transition_task(
            &pred3.id,
            None,
            TaskStatus::Stopped(StopReason::Cancelled),
            None,
            i.h.app.now(),
        )
        .await?;
    let r = call(
        &i.h.router,
        Method::DELETE,
        "/api/tasks/t-pred3",
        None,
        &i.alice,
    )
    .await?;
    assert_eq!(r.status, StatusCode::NO_CONTENT, "{:?}", r.body);
    let orphan = summary(&i, &b_id).await?;
    assert_eq!(orphan["status"], "chained");
    assert!(
        orphan["after"]["blocked"]
            .as_str()
            .is_some_and(|r| r.contains("was deleted"))
    );
    assert_eq!(orphan["after"]["task_id"], Value::Null);
    Ok(())
}

async fn hook(h: &Harness, prompt: &str) -> Result<Resp, Box<dyn std::error::Error>> {
    call(
        &h.internal,
        Method::POST,
        "/hooks/claude/UserPromptSubmit",
        Some(json!({"hook_event_name": "UserPromptSubmit", "session_id": "s", "prompt": prompt})),
        &Creds {
            bearer: Some(TOKEN.into()),
            ..Creds::default()
        },
    )
    .await
}

/// A running task `id` with a live run on the pool's account, so a Stop stays on it instead of
/// switching.
async fn live_task(i: &Intake, id: &str) -> Result<TaskRecord, Box<dyn std::error::Error>> {
    let task = seed_task(&i.h, id, "s1", "u-alice", TaskStatus::Running, None).await?;
    i.h.store
        .insert_attempt(&AttemptRecord {
            id: AttemptId::parse(&format!("a-{id}"))?,
            task_id: task.id.clone(),
            seq: 1,
            harness: Agent::ClaudeCode,
            account_id: Some(AccountId::parse("acc-1")?),
            provider_session_id: None,
            launch_kind: LaunchKind::New,
            state: AttemptState::Live,
            run_dir: "/nonexistent/run".into(),
            token_hash: i.h.app.deps.crypto.sha256_hex(TOKEN.as_bytes()),
            host_token: i.h.app.deps.crypto.seal(&[1u8; 32])?,
            ptyhost_pid: None,
            child_pid: None,
            started_at: i.h.app.now(),
            ended_at: None,
            exit_code: None,
            exit_reason: None,
        })
        .await?;
    Ok(task)
}

/// A hook to the internal listener with the time its client says it sent it.
async fn hook_sent(
    i: &Intake,
    event: &str,
    body: Value,
    sent: &str,
) -> Result<StatusCode, Box<dyn std::error::Error>> {
    Ok(hook_to(
        i.h.internal.clone(),
        event.to_owned(),
        body,
        Some(sent.to_owned()),
    )
    .await?)
}

/// [`hook_sent`] on `internal`, with or without a sent time, for a task of its own.
async fn hook_to(
    internal: axum::Router,
    event: String,
    body: Value,
    sent: Option<String>,
) -> Result<StatusCode, String> {
    use tower::ServiceExt as _;
    let mut request = axum::http::Request::post(format!("/hooks/claude/{event}"))
        .header("authorization", format!("Bearer {TOKEN}"))
        .header("content-type", "application/json");
    if let Some(sent) = sent {
        request = request.header("x-yulius-hook-sent-ms", sent);
    }
    let request = request
        .body(axum::body::Body::from(body.to_string()))
        .map_err(|e| e.to_string())?;
    let response = internal.oneshot(request).await.map_err(|e| e.to_string())?;
    Ok(response.status())
}

fn stop_body() -> Value {
    json!({"hook_event_name": "Stop", "session_id": "s", "stop_hook_active": false})
}

fn prompt_body(prompt: &str) -> Value {
    json!({"hook_event_name": "UserPromptSubmit", "session_id": "s", "prompt": prompt})
}

/// SPEC-346 LPH-002 (#921, review round 2): the turn's Stop arrives while its prompt, past its
/// turn-end check, is still setting Working: the Stop waits for it and then settles the task as
/// it is, Your turn. It used to settle from the copy it read before waiting, which already read
/// Your turn, so the task stayed Working with the agent idle.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_stop_that_waited_for_its_prompt_settles_after_it() -> TestResult {
    use yulius_app::race::{Point, on};
    let i = intake_with(harness()?).await?;
    let task = live_task(&i, "t-order").await?;
    i.h.store
        .set_activity(&task.id, Some(Activity::Waiting), i.h.app.now())
        .await?;
    let key = Path::new(task.id.as_str()).to_path_buf();
    let (read_tx, read_rx) = std::sync::mpsc::channel();
    on(Point::StopTurnLock, &key, move || {
        let _ = read_tx.send(());
    });
    let (stop_tx, stop_rx) = tokio::sync::oneshot::channel();
    let (internal, runtime) = (i.h.internal.clone(), tokio::runtime::Handle::current());
    on(Point::PromptTurnChecked, &key, move || {
        // The Stop comes now and reads the task (Your turn) before the prompt sets Working.
        let stop = runtime.spawn(hook_to(internal, "Stop".into(), stop_body(), None));
        let _ = stop_tx.send(stop);
        let read = tokio::task::block_in_place(|| read_rx.recv_timeout(Duration::from_secs(10)));
        assert!(read.is_ok(), "the Stop read the task");
    });
    let prompt = call(
        &i.h.internal,
        Method::POST,
        "/hooks/claude/UserPromptSubmit",
        Some(prompt_body("continue with the report page")),
        &Creds {
            bearer: Some(TOKEN.into()),
            ..Creds::default()
        },
    )
    .await?;
    assert_eq!(prompt.status, StatusCode::OK);
    let stop = tokio::time::timeout(Duration::from_secs(10), stop_rx).await??;
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(10), stop).await???,
        StatusCode::OK
    );
    let activity = i.h.store.task(&task.id).await?.and_then(|t| t.activity);
    assert_eq!(
        activity,
        Some(Activity::Waiting),
        "the Stop came after the prompt"
    );
    Ok(())
}

/// SPEC-346 LPH-002 (#921, review round 3): a late Stop of the turn before (sent before this
/// prompt) settles the task while the prompt's scope check runs: the prompt still sets Working,
/// from the task as it is now. It used to compare with the copy it read on arrival (Working) and
/// write nothing, so the task read Your turn while the agent worked.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_prompt_sets_working_over_an_older_stop_that_settled_meanwhile() -> TestResult {
    let i = intake_with(harness()?).await?;
    let task = live_task(&i, "t-older").await?;
    i.h.store
        .set_activity(&task.id, Some(Activity::Working), i.h.app.now())
        .await?;
    i.h.clock.0.fetch_add(1_000, Ordering::SeqCst);
    let now = i.h.app.now().0;
    i.h.app.record_typing(&task.id, &UserId::parse("u-alice")?);
    i.jev.held.store(true, Ordering::SeqCst);
    let prompt = tokio::spawn(hook_to(
        i.h.internal.clone(),
        "UserPromptSubmit".into(),
        prompt_body("use the existing chart component for the report"),
        Some(now.to_string()),
    ));
    tokio::time::timeout(Duration::from_secs(10), i.jev.entered.notified()).await?;
    let older = (now - 500).to_string();
    assert_eq!(
        hook_sent(&i, "Stop", stop_body(), &older).await?,
        StatusCode::OK
    );
    assert_eq!(
        i.h.store.task(&task.id).await?.and_then(|t| t.activity),
        Some(Activity::Waiting),
        "the older Stop settled the task"
    );
    i.jev.release.notify_one();
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(10), prompt).await???,
        StatusCode::OK
    );
    assert_eq!(
        i.h.store.task(&task.id).await?.and_then(|t| t.activity),
        Some(Activity::Working),
        "the prompt's turn runs"
    );
    Ok(())
}

/// SPEC-346 LPH-001: a hook's sent time is never taken later than Yulius's clock, and one not
/// after the epoch counts as none: a Stop the client says it sent in an hour, or a prompt from
/// 1970, never makes the next prompt look like history.
#[tokio::test]
async fn a_hooks_sent_time_is_clamped_to_now() -> TestResult {
    let i = intake_with(harness()?).await?;
    let task = live_task(&i, "t-sent").await?;
    let activity = || async {
        i.h.store
            .task(&task.id)
            .await
            .ok()
            .flatten()
            .and_then(|t| t.activity)
    };
    let stop = json!({"hook_event_name": "Stop", "session_id": "s", "stop_hook_active": false});
    let prompt = json!({"hook_event_name": "UserPromptSubmit", "session_id": "s",
        "prompt": "use the existing chart component for the report"});
    let future = (i.h.app.now().0 + 3_600_000).to_string();
    assert_eq!(
        hook_sent(&i, "Stop", stop.clone(), &future).await?,
        StatusCode::OK
    );
    assert_eq!(activity().await, Some(Activity::Waiting));
    i.h.clock.0.fetch_add(1_000, Ordering::SeqCst);
    let now = i.h.app.now().0.to_string();
    assert_eq!(
        hook_sent(&i, "UserPromptSubmit", prompt.clone(), &now).await?,
        StatusCode::OK
    );
    assert_eq!(
        activity().await,
        Some(Activity::Working),
        "a Stop from the future"
    );
    for sent in ["0", "-5"] {
        let now = i.h.app.now().0.to_string();
        assert_eq!(
            hook_sent(&i, "Stop", stop.clone(), &now).await?,
            StatusCode::OK
        );
        assert_eq!(activity().await, Some(Activity::Waiting));
        i.h.clock.0.fetch_add(1_000, Ordering::SeqCst);
        let status = hook_sent(&i, "UserPromptSubmit", prompt.clone(), sent).await?;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(activity().await, Some(Activity::Working), "sent at {sent}");
    }
    Ok(())
}

/// SPEC-346 LPH-002 (#921): a typed prompt whose handling outlasts the hook client's wait (here
/// its scope check) is answered after the turn it started ended: the client failed open, the
/// agent ran the turn and its Stop settled the task. The late prompt leaves the task as that Stop
/// left it; before, it set Working again and nothing ended that turn.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_prompt_answered_after_its_turn_ended_leaves_the_task_settled() -> TestResult {
    let i = intake_with(harness()?).await?;
    let task = live_task(&i, "t-late").await?;
    let activity = || async {
        i.h.store
            .task(&task.id)
            .await
            .ok()
            .flatten()
            .and_then(|t| t.activity)
    };
    i.h.app.record_typing(&task.id, &UserId::parse("u-alice")?);
    i.jev.held.store(true, Ordering::SeqCst);
    let internal = i.h.internal.clone();
    let prompt = tokio::spawn(async move {
        call(
            &internal,
            Method::POST,
            "/hooks/claude/UserPromptSubmit",
            Some(
                json!({"hook_event_name": "UserPromptSubmit", "session_id": "s",
                "prompt": "use the existing chart component for the report"}),
            ),
            &Creds {
                bearer: Some(TOKEN.into()),
                ..Creds::default()
            },
        )
        .await
        .map(|r| r.status)
        .map_err(|e| e.to_string())
    });
    tokio::time::timeout(Duration::from_secs(10), i.jev.entered.notified()).await?;
    // The client gave up waiting (5 s): the agent ran the turn, and its Stop settles the task.
    // Neither request says when it was sent (an older client): Yulius orders them by arrival.
    i.h.clock.0.fetch_add(5_000, Ordering::SeqCst);
    let stop = call(
        &i.h.internal,
        Method::POST,
        "/hooks/claude/Stop",
        Some(json!({"hook_event_name": "Stop", "session_id": "s", "stop_hook_active": false})),
        &Creds {
            bearer: Some(TOKEN.into()),
            ..Creds::default()
        },
    )
    .await?;
    assert_eq!(stop.status, StatusCode::OK, "{:?}", stop.body);
    assert_eq!(
        activity().await,
        Some(Activity::Waiting),
        "the Stop settled the turn"
    );
    i.jev.release.notify_one();
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(10), prompt).await???,
        StatusCode::OK
    );
    assert_eq!(
        activity().await,
        Some(Activity::Waiting),
        "the late prompt's turn is over"
    );
    Ok(())
}

fn held(r: &Resp) -> bool {
    r.body["decision"] == "block"
}

/// A running task with two open steps and a live run, prompts typed by alice.
async fn running_task(i: &Intake) -> Result<TaskId, Box<dyn std::error::Error>> {
    let task = seed_task(&i.h, "t-run", "s1", "u-alice", TaskStatus::Running, None).await?;
    seed_live_attempt(&i.h, "t-run", TOKEN).await?;
    let now = i.h.app.now();
    let step =
        |id: &str, ordinal: u32, title: &str| -> Result<StepRecord, Box<dyn std::error::Error>> {
            Ok(StepRecord {
                id: StepId::parse(id)?,
                task_id: task.id.clone(),
                ordinal,
                title: title.into(),
                status: "pending".into(),
                note: None,
                updated_by: Some("system".into()),
                updated_at: Some(now),
                effort: 1,
            })
        };
    i.h.store
        .replace_steps(
            &task.id,
            &[
                step("st-1", 1, "Build the report page")?,
                step("st-2", 2, "Add CSV export")?,
            ],
            "system",
            now,
        )
        .await?;
    Ok(task.id)
}

/// AC-TSK-12: slash commands, short answers, Claude Code's own turns, the task's own prompt,
/// prompts nobody typed and every Jev failure pass untouched; a confident `new_scope` is held
/// with a reason for the person (never the agent) and a card; "Send anyway" lets the identical
/// prompt through once and records the override; parallel and "after" create tasks.
#[tokio::test]
async fn user_prompt_submit_holds_new_scope_and_passes_everything_else() -> TestResult {
    let h = harness_with(HarnessOptions {
        configure: Some(Box::new(|c| c.max_live_sessions = 0)),
        ..HarnessOptions::default()
    })?;
    let i = intake_with(h).await?;
    let task = running_task(&i).await?;
    let alice = UserId::parse("u-alice")?;
    let scope_prompt = "please also add a dark mode to the settings page";

    // Nobody typed: Claude Code's or Yulius's own turn passes without a Jev call.
    let r = hook(&i.h, scope_prompt).await?;
    assert_eq!(r.status, StatusCode::OK);
    assert!(!held(&r), "{:?}", r.body);
    assert_eq!(i.jev.calls.load(Ordering::SeqCst), 0);

    i.h.app.record_typing(&task, &alice);
    for prompt in [
        "yes",
        "continue",
        "/goal finish the report page and verify it",
        "/compact",
        "<task-notification> the background agent finished </task-notification>",
        "do things",
    ] {
        let r = hook(&i.h, prompt).await?;
        assert!(!held(&r), "{prompt}: {:?}", r.body);
    }
    assert_eq!(
        i.jev.calls.load(Ordering::SeqCst),
        0,
        "bypasses never call Jev"
    );
    let r = hook(&i.h, "use the existing chart component for the report").await?;
    assert!(!held(&r), "context passes");
    let r = hook(&i.h, "maybe also add a print view for the page").await?;
    assert!(!held(&r), "below the threshold passes");
    i.jev.down.store(true, Ordering::SeqCst);
    let r = hook(&i.h, scope_prompt).await?;
    assert!(!held(&r), "Jev down passes");
    i.jev.down.store(false, Ordering::SeqCst);

    let r = hook(&i.h, scope_prompt).await?;
    assert!(held(&r), "{:?}", r.body);
    let reason = r.body["reason"].as_str().ok_or("reason")?;
    assert!(reason.starts_with("Held by Yulius"), "{reason}");
    assert!(
        r.body.get("hookSpecificOutput").is_none(),
        "nothing reaches the agent"
    );
    let card = summary(&i, "t-run").await?["held_prompt"].clone();
    assert_eq!(card["class"], "new_scope");
    assert_eq!(card["typed_by"], "alice");
    assert_eq!(card["excerpt"], scope_prompt);
    let hold = card["id"].as_str().ok_or("hold id")?.to_owned();

    // Send anyway: recorded; no run is attached here, so the person presses Enter.
    let r = post(
        &i,
        &format!("/api/tasks/t-run/held-prompts/{hold}"),
        json!({"action": "send"}),
    )
    .await?;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.body);
    assert!(r.body["note"].as_str().is_some_and(|n| n.contains("Enter")));
    assert_eq!(r.body["task"]["held_prompt"], Value::Null);
    let again = post(
        &i,
        &format!("/api/tasks/t-run/held-prompts/{hold}"),
        json!({"action": "parallel"}),
    )
    .await?;
    assert_eq!(
        again.status,
        StatusCode::CONFLICT,
        "one decision per held prompt"
    );
    let r = hook(&i.h, scope_prompt).await?;
    assert!(!held(&r), "the identical prompt passes once: {:?}", r.body);
    let r = hook(&i.h, scope_prompt).await?;
    assert!(held(&r), "only once");

    // Start in parallel: a new task with the prompt, running settings of this one.
    let hold = summary(&i, "t-run").await?["held_prompt"]["id"]
        .as_str()
        .ok_or("hold")?
        .to_owned();
    let r = post(
        &i,
        &format!("/api/tasks/t-run/held-prompts/{hold}"),
        json!({"action": "parallel"}),
    )
    .await?;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.body);
    assert_eq!(r.body["created"]["status"], "queued");
    assert_eq!(r.body["created"]["after"], Value::Null);

    // Run after this task: chained.
    i.h.app.record_typing(&task, &alice);
    assert!(held(&hook(&i.h, scope_prompt).await?));
    let hold = summary(&i, "t-run").await?["held_prompt"]["id"]
        .as_str()
        .ok_or("hold")?
        .to_owned();
    let r = post(
        &i,
        &format!("/api/tasks/t-run/held-prompts/{hold}"),
        json!({"action": "after"}),
    )
    .await?;
    assert_eq!(r.body["created"]["status"], "chained", "{:?}", r.body);
    assert_eq!(r.body["created"]["after"]["task_id"], "t-run");

    // A newer held prompt replaces an unanswered one; dismiss closes the card.
    assert!(held(&hook(&i.h, scope_prompt).await?));
    assert!(held(
        &hook(&i.h, "and add a dark mode toggle to the header too").await?
    ));
    let card = summary(&i, "t-run").await?["held_prompt"].clone();
    assert!(
        card["excerpt"]
            .as_str()
            .is_some_and(|e| e.contains("header"))
    );
    let r = post(
        &i,
        &format!(
            "/api/tasks/t-run/held-prompts/{}",
            card["id"].as_str().ok_or("id")?
        ),
        json!({"action": "dismiss"}),
    )
    .await?;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(summary(&i, "t-run").await?["held_prompt"], Value::Null);
    Ok(())
}

/// AC-TSK-12: drops scope — the chosen steps become `skipped` (finished), the agent gets a
/// message, the new direction is chained after this task.
#[tokio::test]
async fn drops_scope_skips_the_chosen_steps_and_chains_the_new_direction() -> TestResult {
    let h = harness_with(HarnessOptions {
        configure: Some(Box::new(|c| c.max_live_sessions = 0)),
        ..HarnessOptions::default()
    })?;
    let i = intake_with(h).await?;
    let task = running_task(&i).await?;
    i.h.app.record_typing(&task, &UserId::parse("u-alice")?);
    let r = hook(&i.h, "drop the export, show the report as a chart instead").await?;
    assert!(held(&r), "{:?}", r.body);
    let hold = summary(&i, "t-run").await?["held_prompt"].clone();
    assert_eq!(hold["class"], "drops_scope");
    let path = format!(
        "/api/tasks/t-run/held-prompts/{}",
        hold["id"].as_str().ok_or("id")?
    );
    let bad = post(
        &i,
        &path,
        json!({"action": "skip_after", "skip_step_ids": ["nope"]}),
    )
    .await?;
    assert_eq!(bad.status, StatusCode::UNPROCESSABLE_ENTITY);
    let r = post(
        &i,
        &path,
        json!({"action": "skip_after", "skip_step_ids": ["st-2"]}),
    )
    .await?;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.body);
    assert_eq!(r.body["created"]["status"], "chained");
    assert!(
        r.body["note"]
            .as_str()
            .is_some_and(|n| n.starts_with("1 step skipped"))
    );
    let steps = i.h.store.steps(&task).await?;
    assert_eq!(steps[0].status, "pending");
    assert_eq!(steps[1].status, "skipped");
    assert!(
        steps[1]
            .note
            .as_deref()
            .is_some_and(|n| n.contains("Ruled out"))
    );
    assert_eq!(steps[1].updated_by.as_deref(), Some("user:alice"));
    assert_eq!(
        r.body["task"]["steps"]["done"], 1,
        "skipped counts as finished"
    );
    // MSR-002 (SPEC-028): the skipped steps carry the change; no message is stored.
    assert!(i.h.store.task_messages(&task, 10).await?.is_empty());
    Ok(())
}

/// SPEC-341 LSS-002: a step the person rules out is skipped under the task's steps lock. A step
/// the agent finished while the skip waited stays done, and the answer says no open step was
/// left instead of "0 steps skipped".
#[tokio::test]
async fn a_step_finished_while_the_skip_waits_stays_done() -> TestResult {
    let h = harness_with(HarnessOptions {
        configure: Some(Box::new(|c| c.max_live_sessions = 0)),
        ..HarnessOptions::default()
    })?;
    let i = intake_with(h).await?;
    let task = running_task(&i).await?;
    i.h.app.record_typing(&task, &UserId::parse("u-alice")?);
    let r = hook(&i.h, "drop the export, show the report as a chart instead").await?;
    assert!(held(&r), "{:?}", r.body);
    let hold = summary(&i, "t-run").await?["held_prompt"].clone();
    let path = format!(
        "/api/tasks/t-run/held-prompts/{}",
        hold["id"].as_str().ok_or("id")?
    );
    let sandbox = SandboxId::parse("s1")?;
    let before = i.h.store.tasks(&sandbox, false).await?.len();
    let runtime = i.h.app.runtime(&task);
    let held_lock = runtime.steps_lock.lock().await;
    let agent = async {
        // The new direction exists: the skip checked the step and now waits for the lock.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while i.h.store.tasks(&sandbox, false).await?.len() == before {
            if std::time::Instant::now() > deadline {
                return Err::<(), Box<dyn std::error::Error>>("no chained task".into());
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        let step = StepId::parse("st-2")?;
        i.h.store
            .update_step(&step, "done", None, "agent", i.h.app.now())
            .await?;
        drop(held_lock);
        Ok(())
    };
    let (r, finished) = tokio::join!(
        post(
            &i,
            &path,
            json!({"action": "skip_after", "skip_step_ids": ["st-2"]}),
        ),
        agent
    );
    finished?;
    let r = r?;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.body);
    assert!(
        r.body["note"]
            .as_str()
            .is_some_and(|n| n.starts_with("No open step was left to skip")),
        "{:?}",
        r.body
    );
    assert_eq!(i.h.store.steps(&task).await?[1].status, "done");
    Ok(())
}

fn git(dir: &Path, args: &[&str]) -> TestResult {
    let out = Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@example.com")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@example.com")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .output()?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).into_owned().into());
    }
    Ok(())
}

/// AC-TSK-12: redirect — reverting needs an explicit confirmation; then the task is cancelled
/// and archived, its committed and uncommitted work kept on a `yulius/reverted/…` branch, its
/// worktree reset, and a new task started with the prompt.
#[tokio::test]
async fn redirect_reverts_only_with_confirmation_and_keeps_the_work() -> TestResult {
    let h = harness_with(HarnessOptions {
        git: Some(Arc::new(yulius::wiring::git::GitAdapter::new("git"))),
        configure: Some(Box::new(|c| c.max_live_sessions = 0)),
        ..HarnessOptions::default()
    })?;
    let i = intake_with(h).await?;
    let repo = tempfile::tempdir()?;
    let worktree = repo.path().join("wt");
    std::fs::create_dir_all(&worktree)?;
    git(&worktree, &["init", "-q", "-b", "dev"])?;
    std::fs::write(worktree.join("README.md"), "# fixture\n")?;
    git(&worktree, &["add", "-A"])?;
    git(&worktree, &["commit", "-q", "-m", "base"])?;
    git(&worktree, &["checkout", "-q", "-b", "feat/1-task"])?;
    std::fs::write(worktree.join("work.txt"), "committed\n")?;
    git(&worktree, &["add", "-A"])?;
    git(&worktree, &["commit", "-q", "-m", "work"])?;
    std::fs::write(worktree.join("draft.txt"), "uncommitted\n")?;

    let task = seed_task(
        &i.h,
        "t-run",
        "s1",
        "u-alice",
        TaskStatus::Running,
        Some(&worktree),
    )
    .await?;
    seed_live_attempt(&i.h, "t-run", TOKEN).await?;
    i.h.app.record_typing(&task.id, &UserId::parse("u-alice")?);
    let r = hook(&i.h, "forget this, rewrite everything as a CLI tool in Go").await?;
    assert!(held(&r), "{:?}", r.body);
    let hold = summary(&i, "t-run").await?["held_prompt"].clone();
    assert_eq!(hold["class"], "redirect");
    let path = format!(
        "/api/tasks/t-run/held-prompts/{}",
        hold["id"].as_str().ok_or("id")?
    );
    let refused = post(&i, &path, json!({"action": "revert"})).await?;
    assert_eq!(
        refused.status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "confirmation required"
    );
    assert!(
        worktree.join("draft.txt").exists(),
        "nothing touched without confirmation"
    );

    // The person stopped the run meanwhile (a live run would be stopped and reaped first).
    let attempt = yulius_domain::ids::AttemptId::parse("a-t-run")?;
    i.h.store
        .transition_attempt(
            &attempt,
            yulius_domain::task::AttemptState::Exited,
            &Default::default(),
        )
        .await?;
    i.h.store
        .transition_task(
            &task.id,
            None,
            TaskStatus::Stopped(StopReason::Cancelled),
            None,
            i.h.app.now(),
        )
        .await?;
    let r = post(&i, &path, json!({"action": "revert", "confirm": true})).await?;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.body);
    let note = r.body["note"].as_str().ok_or("note")?;
    assert!(note.contains("yulius/reverted/"), "{note}");
    assert!(r.body["task"]["archived"].as_bool().unwrap_or(false));
    assert_eq!(r.body["created"]["status"], "queued");
    let branch = note
        .split_whitespace()
        .find(|w| w.starts_with("yulius/reverted/"))
        .map(|w| w.trim_end_matches([';', '.']))
        .ok_or("branch")?;
    git(&worktree, &["rev-parse", "--verify", "-q", branch])?;
    git(
        &worktree,
        &["cat-file", "-e", &format!("{branch}:draft.txt")],
    )?;
    git(
        &worktree,
        &["cat-file", "-e", &format!("{branch}:work.txt")],
    )?;
    assert!(!worktree.join("work.txt").exists(), "worktree reverted");
    assert!(!worktree.join("draft.txt").exists());
    Ok(())
}

/// SPEC-017 RUL-003/RUL-005: a task waits for a logical sentence over other tasks: `and` needs
/// both, `or` either, `ended` fires when a task stops, a rule that can no longer be true needs a
/// person, and every refusal is a 422 with a reason.
#[tokio::test]
async fn a_task_starts_when_its_start_rule_is_true() -> TestResult {
    let h = harness_with(HarnessOptions {
        configure: Some(Box::new(|c| c.max_live_sessions = 0)),
        ..HarnessOptions::default()
    })?;
    let i = intake_with(h).await?;
    let store = &i.h.store;
    let create = "/api/sandboxes/s1/tasks";
    let a = seed_task(&i.h, "t-a", "s1", "u-alice", TaskStatus::Running, None).await?;
    let b = seed_task(&i.h, "t-b", "s1", "u-alice", TaskStatus::Running, None).await?;
    let (na, nb) = (a.number, b.number);
    let done = |id: TaskId| {
        let now = i.h.app.now();
        async move { store.set_activity(&id, Some(Activity::Done), now).await }
    };
    let rule_task = |rule: String| {
        let i = &i;
        async move {
            let r = post(i, create, json!({"prompt": "ruled", "start_rule": rule})).await?;
            assert_eq!(r.status, StatusCode::CREATED, "{:?}", r.body);
            Ok::<_, Box<dyn std::error::Error>>(r.body["id"].as_str().ok_or("id")?.to_owned())
        }
    };

    // and: both must be Done.
    let both = rule_task(format!("#{na} done and #{nb} finished")).await?;
    let either = rule_task(format!("#{na} or #{nb}")).await?;
    let shown = summary(&i, &both).await?;
    assert_eq!(shown["status"], "chained");
    assert_eq!(
        shown["after"]["rule"],
        format!("#{na} done and #{nb} done"),
        "the canonical rule shows"
    );
    done(a.id.clone()).await?;
    i.h.app.advance_chains(&a.id).await;
    assert_eq!(summary(&i, &both).await?["status"], "chained", "needs both");
    assert_eq!(
        summary(&i, &either).await?["status"],
        "queued",
        "or: either"
    );
    done(b.id.clone()).await?;
    i.h.app.advance_chains(&b.id).await;
    assert_eq!(summary(&i, &both).await?["status"], "queued");

    // A rule that can no longer be true needs a person; `ended` fires when a task stops.
    let c = seed_task(&i.h, "t-c", "s1", "u-alice", TaskStatus::Running, None).await?;
    let d = seed_task(&i.h, "t-d", "s1", "u-alice", TaskStatus::Running, None).await?;
    let needs_c = rule_task(format!("#{} done and #{} done", c.number, d.number)).await?;
    let on_end = rule_task(format!("#{} ended", c.number)).await?;
    store
        .transition_task(
            &c.id,
            Some(TaskStatus::Running),
            TaskStatus::Stopped(StopReason::Crashed),
            None,
            i.h.app.now(),
        )
        .await?;
    i.h.app.advance_chains(&c.id).await;
    let blocked = summary(&i, &needs_c).await?;
    assert_eq!(blocked["status"], "chained");
    assert!(
        blocked["after"]["blocked"]
            .as_str()
            .is_some_and(|r| r.contains("can no longer become true")),
        "{blocked}"
    );
    assert_eq!(summary(&i, &on_end).await?["status"], "queued");
    i.h.app.advance_all_chains().await;
    assert_eq!(summary(&i, &needs_c).await?["status"], "chained");

    // Refusals.
    for (body, reason) in [
        (json!({"prompt": "x", "start_rule": "#1 and"}), "ends"),
        (json!({"prompt": "x", "start_rule": "#999 done"}), "#999"),
        (
            json!({"prompt": "x", "start_rule": "#1", "after_task_id": "t-a"}),
            "not both",
        ),
    ] {
        let r = post(&i, create, body).await?;
        assert_eq!(r.status, StatusCode::UNPROCESSABLE_ENTITY, "{:?}", r.body);
        assert!(r.body.to_string().contains(reason), "{:?}", r.body);
    }
    Ok(())
}

/// SPEC-017: the background tick starts a task whose rule became true, Start now releases a
/// rule task, and deleting a task a rule names counts as that task ending.
#[tokio::test]
async fn start_rules_follow_the_tick_start_now_and_deletions() -> TestResult {
    let h = harness_with(HarnessOptions {
        configure: Some(Box::new(|c| c.max_live_sessions = 0)),
        ..HarnessOptions::default()
    })?;
    let i = intake_with(h).await?;
    let store = &i.h.store;
    let create = "/api/sandboxes/s1/tasks";
    let a = seed_task(&i.h, "t-a", "s1", "u-alice", TaskStatus::Running, None).await?;
    let b = seed_task(&i.h, "t-b", "s1", "u-alice", TaskStatus::Running, None).await?;
    let mut ids = Vec::new();
    for rule in [
        format!("!#{} && (#{} || #{})", a.number, a.number, b.number),
        format!("#{} ended", b.number),
        format!("#{} done and #{} done", a.number, b.number),
    ] {
        let r = post(&i, create, json!({"prompt": "ruled", "start_rule": rule})).await?;
        assert_eq!(r.status, StatusCode::CREATED, "{:?}", r.body);
        ids.push(r.body["id"].as_str().ok_or("id")?.to_owned());
    }
    // Nothing is true yet; the tick leaves them all waiting.
    i.h.app.advance_all_chains().await;
    for id in &ids {
        assert_eq!(summary(&i, id).await?["status"], "chained");
    }
    // The tick alone starts a task whose rule became true (no event for the change).
    store
        .set_activity(&b.id, Some(Activity::Done), i.h.app.now())
        .await?;
    i.h.app.advance_all_chains().await;
    assert_eq!(
        summary(&i, &ids[0]).await?["status"],
        "chained",
        "a is still running"
    );
    // Deleting the task a rule names counts as it ending.
    store
        .transition_task(&b.id, None, TaskStatus::Closed, None, i.h.app.now())
        .await?;
    let r = call(
        &i.h.router,
        Method::DELETE,
        "/api/tasks/t-b",
        None,
        &i.alice,
    )
    .await?;
    assert_eq!(r.status, StatusCode::NO_CONTENT, "{:?}", r.body);
    assert_eq!(summary(&i, &ids[1]).await?["status"], "queued", "#b ended");
    // The rule that needed the deleted task can never be true: it needs a person, who can
    // still start it now.
    let blocked = summary(&i, &ids[2]).await?;
    assert!(blocked["after"]["blocked"].is_string(), "{blocked}");
    let r = post(&i, &format!("/api/tasks/{}/start-now", ids[2]), json!({})).await?;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.body);
    assert_eq!(r.body["status"], "queued");
    Ok(())
}
