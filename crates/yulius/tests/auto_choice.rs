//! "Auto" mode and workflow for new tasks (SPEC-001 TSK-015, AC-TSK-14; SPEC-002 ADR-16 item 5):
//! through the public API, Jev chooses goal or prompt and one of the sandbox's workflows (the
//! catalog's defaults — Feature, Fix, Research — then instance and sandbox workflows) or none; without Jev, or when it fails or is unsure, the task is Goal with the
//! "Feature" workflow (none for a prompt-mode task); a prompt-mode task never gets a spec-first
//! workflow from Auto; picked values are never sent to Jev. Jev is a fake: no real call is made.

mod support;

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use axum::http::{Method, StatusCode};
use serde_json::{Value, json};
use support::{
    Creds, Harness, HarnessOptions, Resp, TestResult, call, harness_with, perms, seed_github_user,
    seed_sandbox,
};
use yulius_app::intake::IntakeServices;
use yulius_app::ports::intake::{ChoiceEvaluator, JevFailure};
use yulius_app::ports::store_ext::AutomationStore;
use yulius_domain::intake::{ChoiceAnswer, ChoiceQuestion};

/// A fake Jev for the Auto questions. Mode: a request with a question mark is a prompt.
/// Workflow (by the option's name): a question gets "Research", "spec it anyway" gets "Feature",
/// "crash" gets "Fix", "typo" gets none, anything else "Feature". "maybe" makes every answer unsure. Records every question it saw.
#[derive(Default)]
struct FakeJev {
    calls: AtomicUsize,
    down: AtomicBool,
    seen: Mutex<Vec<(String, Vec<String>)>>,
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
        if let Ok(mut seen) = self.seen.lock() {
            seen.push((
                question.id.to_owned(),
                question
                    .options
                    .iter()
                    .map(|o| format!("{}={}", o.key, o.description))
                    .collect(),
            ));
        }
        if self.down.load(Ordering::SeqCst) {
            return Err(JevFailure::Unavailable);
        }
        assert!(state.starts_with("REQUEST\n"), "{state}");
        let unsure = state.contains("maybe");
        match question.id {
            "mode" => {
                let winner = if state.contains('?') {
                    "prompt"
                } else {
                    "goal"
                };
                Ok(answer(question, winner, if unsure { 0.55 } else { 0.9 }))
            }
            "workflow" => {
                let wanted = if state.contains("spec it anyway") {
                    "Feature"
                } else if state.contains('?') {
                    "Research"
                } else if state.contains("crash") {
                    "Fix"
                } else if state.contains("typo") {
                    "None"
                } else {
                    "Feature"
                };
                let key = question
                    .options
                    .iter()
                    .find(|o| o.description.starts_with(&format!("{wanted} (")))
                    .map_or("none", |o| o.key);
                Ok(answer(question, key, if unsure { 0.4 } else { 0.85 }))
            }
            _ => Err(JevFailure::Invalid("question")),
        }
    }
}

struct Auto {
    h: Harness,
    alice: Creds,
    jev: Arc<FakeJev>,
}

/// An instance with sandbox `s1` (alice can write) and the default workflows of the catalog; Jev
/// installed when `with_jev`.
async fn auto(with_jev: bool) -> Result<Auto, Box<dyn std::error::Error>> {
    // No session slots: created tasks wait as queued, so nothing needs a real agent.
    let h = harness_with(HarnessOptions {
        configure: Some(Box::new(|c| c.max_live_sessions = 0)),
        ..HarnessOptions::default()
    })?;
    let alice = seed_github_user(&h, "u-alice", "alice").await?;
    seed_sandbox(&h, "s1", "fixture", "u-alice").await?;
    h.github.set_perm("alice", "acme/fixture", perms("write"));
    let jev = Arc::new(FakeJev::default());
    if with_jev {
        h.app.set_intake(IntakeServices::new(
            Some(jev.clone() as Arc<dyn ChoiceEvaluator>),
            None,
        ));
    }
    Ok(Auto { h, alice, jev })
}

async fn create(a: &Auto, body: Value) -> Result<Resp, Box<dyn std::error::Error>> {
    call(
        &a.h.router,
        Method::POST,
        "/api/sandboxes/s1/tasks",
        Some(body),
        &a.alice,
    )
    .await
}

async fn detail(a: &Auto, task: &Value) -> Result<Value, Box<dyn std::error::Error>> {
    let id = task["id"].as_str().ok_or("id")?;
    let r = call(
        &a.h.router,
        Method::GET,
        &format!("/api/tasks/{id}"),
        None,
        &a.alice,
    )
    .await?;
    assert_eq!(r.status, StatusCode::OK, "{:?}", r.body);
    Ok(r.body)
}

/// The steps of the sandbox's workflow named `name`, as the workflows API lists them (the
/// catalog's contents are not repeated here).
async fn workflow_steps(a: &Auto, name: &str) -> Result<Vec<String>, Box<dyn std::error::Error>> {
    let r = call(
        &a.h.router,
        Method::GET,
        "/api/sandboxes/s1/workflows",
        None,
        &a.alice,
    )
    .await?;
    let workflow = r
        .body
        .as_array()
        .and_then(|list| list.iter().find(|w| w["name"] == name))
        .ok_or("workflow")?;
    Ok(workflow["steps"]
        .as_array()
        .ok_or("steps")?
        .iter()
        .filter_map(|s| s.as_str().map(str::to_owned))
        .collect())
}

fn step_titles(detail: &Value) -> Vec<String> {
    detail["steps_list"]
        .as_array()
        .map(|steps| {
            steps
                .iter()
                .filter_map(|s| s["title"].as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default()
}

/// AC-TSK-14: a feature request on Auto runs as a goal with the Feature workflow Jev chose from
/// the store; the decision is in the summary, in the task's detail and in the audit log.
#[tokio::test]
async fn auto_chooses_goal_and_a_workflow_from_the_store() -> TestResult {
    let a = auto(true).await?;
    let r = create(
        &a,
        json!({"prompt": "Add a health check endpoint to the API and show it on the status page", "mode": "auto", "workflow_id": "auto"}),
    )
    .await?;
    assert_eq!(r.status, StatusCode::CREATED, "{:?}", r.body);
    assert_eq!(r.body["mode"], "goal");
    assert_eq!(
        r.body["auto"],
        json!({"mode": "jev", "workflow": "jev", "workflow_name": "Feature"})
    );
    let d = detail(&a, &r.body).await?;
    assert_eq!(d["auto"], r.body["auto"]);
    let feature_steps = workflow_steps(&a, "Feature").await?;
    assert!(!feature_steps.is_empty());
    assert_eq!(step_titles(&d), feature_steps);

    // Jev saw every workflow the sandbox lists (the defaults first) and "none".
    let seen = a.jev.seen.lock().map_err(|_| "lock")?.clone();
    assert_eq!(seen.len(), 2, "{seen:?}");
    let workflow = seen
        .iter()
        .find(|(id, _)| id == "workflow")
        .ok_or("workflow question")?;
    let options = workflow.1.join("\n");
    assert!(options.contains("w1=Feature (spec first): "), "{options}");
    assert!(options.contains("=Fix (no spec): "), "{options}");
    assert!(options.contains("=Research (no spec): "), "{options}");
    assert!(options.contains("Use when: "), "{options}");
    assert!(workflow.1.last().is_some_and(|o| o.starts_with("none=")));

    // The audit log by target (the reader the automations' history uses).
    let task_id = r.body["id"].as_str().ok_or("id")?;
    let audit = a.h.store.automation_audit(task_id, 10).await?;
    assert!(
        audit.iter().any(|e| e.action == "task.auto_choice"
            && e.detail
                .as_deref()
                .is_some_and(|d| d.contains("mode goal (jev 90%)")
                    && d.contains("workflow Feature (jev 85%)"))),
        "{audit:?}"
    );
    Ok(())
}

/// AC-TSK-14: a question about the product runs as a prompt with the no-spec workflow Jev chose
/// ("features need specs; research and questions don't"); Jev's spec-first pick for a question
/// is overridden to no workflow; Jev's confident "none" is followed.
#[tokio::test]
async fn questions_run_as_prompts_without_a_spec_first_workflow() -> TestResult {
    let a = auto(true).await?;
    let r = create(
        &a,
        json!({"prompt": "How does the subscription pool pick an account for a new task?", "mode": "auto", "workflow_id": "auto"}),
    )
    .await?;
    assert_eq!(r.status, StatusCode::CREATED, "{:?}", r.body);
    assert_eq!(r.body["mode"], "prompt");
    assert_eq!(
        r.body["auto"],
        json!({"mode": "jev", "workflow": "jev", "workflow_name": "Research"})
    );
    assert_eq!(
        step_titles(&detail(&a, &r.body).await?),
        workflow_steps(&a, "Research").await?
    );

    let r = create(
        &a,
        json!({"prompt": "Why is the build slow on the VM? spec it anyway", "mode": "auto", "workflow_id": "auto"}),
    )
    .await?;
    assert_eq!(r.body["mode"], "prompt");
    assert_eq!(
        r.body["auto"],
        json!({"mode": "jev", "workflow": "default", "workflow_name": null})
    );
    assert!(step_titles(&detail(&a, &r.body).await?).is_empty());

    let r = create(
        &a,
        json!({"prompt": "Fix the typo in the README heading", "mode": "auto", "workflow_id": "auto"}),
    )
    .await?;
    assert_eq!(r.body["mode"], "goal");
    assert_eq!(
        r.body["auto"],
        json!({"mode": "jev", "workflow": "jev", "workflow_name": null})
    );

    // A bug gets the Fix workflow.
    let r = create(
        &a,
        json!({"prompt": "The checkout page crashes on an empty cart", "mode": "auto", "workflow_id": "auto"}),
    )
    .await?;
    assert_eq!(
        r.body["auto"],
        json!({"mode": "jev", "workflow": "jev", "workflow_name": "Fix"})
    );
    Ok(())
}

/// AC-TSK-14: no Jev key, Jev down or an unsure answer — the defaults apply predictably: Goal
/// and the workflow named "Feature" (the sandbox's own first), none for a prompt-mode task. The
/// catalog always has Feature; "none without one" is a domain unit test.
#[tokio::test]
async fn without_a_confident_jev_auto_is_goal_and_feature() -> TestResult {
    let defaults = json!({"mode": "default", "workflow": "default", "workflow_name": "Feature"});
    let body =
        json!({"prompt": "Add a health check endpoint", "mode": "auto", "workflow_id": "auto"});

    // No Jev configured.
    let a = auto(false).await?;
    let r = create(&a, body.clone()).await?;
    assert_eq!(r.status, StatusCode::CREATED, "{:?}", r.body);
    assert_eq!(r.body["mode"], "goal");
    assert_eq!(r.body["auto"], defaults);
    assert_eq!(a.jev.calls.load(Ordering::SeqCst), 0);

    // Jev down, then unsure.
    let a = auto(true).await?;
    a.jev.down.store(true, Ordering::SeqCst);
    let r = create(&a, body.clone()).await?;
    assert_eq!(r.body["mode"], "goal");
    assert_eq!(r.body["auto"], defaults);
    assert_eq!(a.jev.calls.load(Ordering::SeqCst), 2);
    a.jev.down.store(false, Ordering::SeqCst);
    let r = create(
        &a,
        json!({"prompt": "maybe a dark mode for the settings page", "mode": "auto", "workflow_id": "auto"}),
    )
    .await?;
    assert_eq!(r.body["mode"], "goal");
    assert_eq!(r.body["auto"], defaults);

    // A person picked prompt mode: an unsure workflow answer means no workflow.
    let r = create(
        &a,
        json!({"prompt": "maybe look into the slow build", "mode": "prompt", "workflow_id": "auto"}),
    )
    .await?;
    assert_eq!(r.body["mode"], "prompt");
    assert_eq!(
        r.body["auto"],
        json!({"mode": null, "workflow": "default", "workflow_name": null})
    );

    // The sandbox's own "Feature" wins over the instance's.
    let own = call(
        &a.h.router,
        Method::POST,
        "/api/sandboxes/s1/workflows",
        Some(json!({"name": "Feature", "description": "This project's feature flow.", "steps": ["Spec", "Build"], "instructions": "", "gates": {"require_spec": true, "require_tests": true}, "scope": "sandbox"})),
        &a.alice,
    )
    .await?;
    assert_eq!(own.status, StatusCode::CREATED, "{:?}", own.body);
    a.jev.down.store(true, Ordering::SeqCst);
    let r = create(&a, body.clone()).await?;
    assert_eq!(step_titles(&detail(&a, &r.body).await?), ["Spec", "Build"]);

    Ok(())
}

/// AC-TSK-14: picked values are never sent to Jev and leave no Auto record; mixing a picked mode
/// with an Auto workflow asks only about the workflow; a bad mode or workflow is refused before
/// Jev is asked.
#[tokio::test]
async fn picked_values_skip_jev_and_bad_values_are_refused() -> TestResult {
    let a = auto(true).await?;
    let r = create(
        &a,
        json!({"prompt": "How are tasks chained?", "mode": "goal", "workflow_id": "wf-fix"}),
    )
    .await?;
    assert_eq!(r.status, StatusCode::CREATED, "{:?}", r.body);
    assert_eq!(r.body["mode"], "goal");
    assert_eq!(r.body["auto"], Value::Null);
    // The API default stays Goal without a workflow.
    let r = create(&a, json!({"prompt": "Add a nickname field"})).await?;
    assert_eq!(r.body["mode"], "goal");
    assert_eq!(r.body["auto"], Value::Null);
    assert_eq!(a.jev.calls.load(Ordering::SeqCst), 0);

    let r = create(
        &a,
        json!({"prompt": "Add a nickname field", "mode": "goal", "workflow_id": "auto"}),
    )
    .await?;
    assert_eq!(
        r.body["auto"],
        json!({"mode": null, "workflow": "jev", "workflow_name": "Feature"})
    );
    assert_eq!(a.jev.calls.load(Ordering::SeqCst), 1);
    let r = create(
        &a,
        json!({"prompt": "What does the Git page show?", "mode": "auto"}),
    )
    .await?;
    assert_eq!(r.body["mode"], "prompt");
    assert_eq!(
        r.body["auto"],
        json!({"mode": "jev", "workflow": null, "workflow_name": null})
    );
    assert_eq!(a.jev.calls.load(Ordering::SeqCst), 2);

    for body in [
        json!({"prompt": "Add a field", "mode": "magic"}),
        json!({"prompt": "Add a field", "mode": "auto", "workflow_id": "wf-missing"}),
        json!({"prompt": "   ", "mode": "auto", "workflow_id": "auto"}),
    ] {
        let r = create(&a, body.clone()).await?;
        assert_eq!(
            r.status,
            StatusCode::UNPROCESSABLE_ENTITY,
            "{body} {:?}",
            r.body
        );
    }
    assert_eq!(a.jev.calls.load(Ordering::SeqCst), 2);
    Ok(())
}
