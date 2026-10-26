//! Tasks the Superagent starts in its person's other sandboxes (SPEC-335): `sandboxes_list`
//! names where it may ask, `sandbox_task_create` waits for the person's answer, and only the
//! person, from a browser session, approves or declines each request.

use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use axum::http::{Method, StatusCode};
use serde_json::{Value, json};
use yulius_app::auth::AuthContext;
use yulius_app::context::App;
use yulius_app::coordination::{Channel, ProjectCaller};
use yulius_app::error::AppResult;
use yulius_app::model::TaskOrigin;
use yulius_app::ports::store::{AuditStore, TaskStore, UserStore};
use yulius_contracts::events::UiEvent;
use yulius_domain::ids::{SandboxId, TaskId, UserId};

use crate::support::{
    self, Creds, Harness, TestResult, add_sandbox, admit_and_sign_in, call, configure_github_app,
    perms, setup_superadmin_creds,
};

type Error = Box<dyn std::error::Error>;

/// How long a test waits for a condition.
const WAIT: Duration = Duration::from_secs(10);

/// alice's world: she writes `studio` (s1, her Superagent's) and `website` (s2), reads `docs`
/// (s3) and cannot see `secret` (s4); bob writes all of them. Created tasks stay queued.
struct World {
    h: Harness,
    alice: Creds,
    alice_id: UserId,
    bob: Creds,
    bob_id: UserId,
}

async fn world(wait_ms: u64) -> Result<World, Error> {
    let h = support::harness_configured(move |c| {
        c.max_live_sessions = 0;
        c.cross_request_wait_ms = wait_ms;
    })?;
    support::exhausted_subscription(&h).await?;
    for (id, slug, port) in [
        ("s1", "studio", 20000),
        ("s2", "website", 20020),
        ("s3", "docs", 20040),
        ("s4", "secret", 20060),
    ] {
        add_sandbox(&h, id, slug, port).await?;
    }
    for (repo, level) in [("studio", "write"), ("website", "write"), ("docs", "read")] {
        h.github
            .set_perm("alice", &format!("Quartio-Studio/{repo}"), perms(level));
    }
    for repo in ["studio", "website", "docs", "secret"] {
        h.github
            .set_perm("bob", &format!("Quartio-Studio/{repo}"), perms("write"));
    }
    let admin = setup_superadmin_creds(&h).await?;
    configure_github_app(&h).await?;
    let alice = admit_and_sign_in(&h, &admin, "alice", 101, false).await?;
    let bob = admit_and_sign_in(&h, &admin, "bob", 102, false).await?;
    let alice_id = me(&h, &alice).await?;
    let bob_id = me(&h, &bob).await?;
    Ok(World {
        h,
        alice,
        alice_id,
        bob,
        bob_id,
    })
}

async fn me(h: &Harness, creds: &Creds) -> Result<UserId, Error> {
    let me = call(&h.router, Method::GET, "/api/me", None, creds).await?;
    Ok(UserId::parse(
        me.body["user"]["id"].as_str().ok_or("no user id")?,
    )?)
}

/// The Superagent of `user` in `sandbox`, as `cockpit_caller` makes it.
fn superagent(sandbox: &str, user: &UserId, login: &str) -> Result<ProjectCaller, Error> {
    Ok(ProjectCaller {
        sandbox: SandboxId::parse(sandbox)?,
        user: user.clone(),
        task: None,
        channel: Channel::Superagent,
        actor: format!("cockpit:{login}"),
        detail: json!({"via": "cockpit", "shell": "sh-test"}),
    })
}

/// Starts `sandbox_task_create` as alice's Superagent in studio; it waits for her answer.
fn ask(w: &World, args: Value) -> Result<tokio::task::JoinHandle<AppResult<Value>>, Error> {
    let app: Arc<App> = w.h.app.clone();
    let caller = superagent("s1", &w.alice_id, "alice")?;
    Ok(tokio::spawn(async move {
        app.tool_sandbox_task_create(&caller, &args).await
    }))
}

async fn requests(w: &World, creds: &Creds, sandbox: &str) -> Result<Vec<Value>, Error> {
    let resp = call(
        &w.h.router,
        Method::GET,
        &format!("/api/sandboxes/{sandbox}/superagent/requests"),
        None,
        creds,
    )
    .await?;
    assert_eq!(resp.status, StatusCode::OK, "{:?}", resp.body);
    Ok(resp.body.as_array().cloned().unwrap_or_default())
}

/// Waits until alice's studio Superagent has `count` requests waiting, and returns them.
async fn waiting(w: &World, count: usize) -> Result<Vec<Value>, Error> {
    let deadline = tokio::time::Instant::now() + WAIT;
    loop {
        let listed = requests(w, &w.alice, "s1").await?;
        if listed.len() == count {
            return Ok(listed);
        }
        if tokio::time::Instant::now() > deadline {
            return Err(format!("expected {count} waiting requests, have {listed:?}").into());
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

async fn answer(
    w: &World,
    creds: &Creds,
    sandbox: &str,
    id: &str,
    approve: bool,
) -> Result<support::Resp, Error> {
    call(
        &w.h.router,
        Method::POST,
        &format!("/api/sandboxes/{sandbox}/superagent/requests/{id}"),
        Some(json!({"approve": approve})),
        creds,
    )
    .await
}

async fn task_count(w: &World, sandbox: &str) -> Result<usize, Error> {
    Ok(w.h
        .store
        .tasks(&SandboxId::parse(sandbox)?, false)
        .await?
        .len())
}

/// XSR-001: the Superagent lists the other sandboxes its person can work in, not its own, not
/// one they only read, not one they cannot see.
#[tokio::test]
async fn the_superagent_lists_the_other_sandboxes_its_person_can_work_in() -> TestResult {
    let w = world(60_000).await?;
    let listed =
        w.h.app
            .tool_sandboxes_list(&superagent("s1", &w.alice_id, "alice")?)
            .await?;
    let slugs: Vec<&str> = listed["sandboxes"]
        .as_array()
        .ok_or("sandboxes")?
        .iter()
        .filter_map(|s| s["sandbox"].as_str())
        .collect();
    assert_eq!(slugs, ["website"], "{listed}");
    let website = &listed["sandboxes"][0];
    assert_eq!(website["repository"], "Quartio-Studio/website");
    assert_eq!(website["onboarding"], "not_needed");
    Ok(())
}

/// AC-XSR-1: the request waits, shows its whole prompt to its person only, and on approval from
/// a browser session the task is created in the other sandbox for the person, exactly as shown.
#[tokio::test]
async fn an_approved_request_creates_the_task_in_the_other_sandbox() -> TestResult {
    let w = world(60_000).await?;
    let prompt = "Update the API client to the v2 endpoints.\nDone when the tests pass.";
    let call_ = ask(
        &w,
        json!({"sandbox": "website", "prompt": prompt, "title": "API client v2", "prepared": true}),
    )?;
    let listed = waiting(&w, 1).await?;
    let request = &listed[0];
    assert_eq!(request["sandbox"]["slug"], "website", "{request}");
    assert_eq!(request["prompt"], prompt);
    assert_eq!(request["title"], "API client v2");
    assert_eq!(request["options"]["prepared"], true);
    assert!(request["ends_at"].is_string(), "{request}");
    assert_eq!(task_count(&w, "s2").await?, 0, "nothing before the answer");
    // Only its person sees it.
    assert!(requests(&w, &w.bob, "s1").await?.is_empty());

    let id = request["id"].as_str().ok_or("id")?;
    let approved = answer(&w, &w.alice, "s1", id, true).await?;
    assert_eq!(approved.status, StatusCode::OK, "{:?}", approved.body);
    let number = approved.body["task"]["number"].as_u64().ok_or("number")?;
    assert_eq!(approved.body["sandbox"], "website");

    let outcome = call_.await??;
    assert_eq!(outcome["outcome"], "approved", "{outcome}");
    assert_eq!(outcome["task"]["number"], json!(number));
    let id = TaskId::parse(outcome["task"]["task_id"].as_str().ok_or("task_id")?)?;
    let task = w.h.store.task(&id).await?.ok_or("task")?;
    assert_eq!(task.sandbox_id, SandboxId::parse("s2")?);
    assert_eq!(task.prompt, prompt);
    assert_eq!(task.title, "API client v2");
    assert_eq!(task.created_by, w.alice_id);
    assert_eq!(task.origin, TaskOrigin::Agent);
    assert!(
        requests(&w, &w.alice, "s1").await?.is_empty(),
        "answered once"
    );
    // A second answer finds nothing.
    let again = answer(
        &w,
        &w.alice,
        "s1",
        request["id"].as_str().ok_or("id")?,
        true,
    )
    .await?;
    assert_eq!(again.status, StatusCode::NOT_FOUND);
    assert_eq!(task_count(&w, "s2").await?, 1);
    Ok(())
}

/// AC-XSR-2: a declined request creates nothing and the call says so.
#[tokio::test]
async fn a_declined_request_creates_nothing() -> TestResult {
    let w = world(60_000).await?;
    let call_ = ask(
        &w,
        json!({"sandbox": "Quartio-Studio/website", "prompt": "Rewrite it"}),
    )?;
    let listed = waiting(&w, 1).await?;
    let declined = answer(
        &w,
        &w.alice,
        "s1",
        listed[0]["id"].as_str().ok_or("id")?,
        false,
    )
    .await?;
    assert_eq!(declined.status, StatusCode::OK, "{:?}", declined.body);
    let outcome = call_.await??;
    assert_eq!(outcome["outcome"], "declined", "{outcome}");
    assert_eq!(task_count(&w, "s2").await?, 0);
    assert!(requests(&w, &w.alice, "s1").await?.is_empty());
    Ok(())
}

/// AC-XSR-2: nobody answers within the bound: the request ends and nothing is created.
#[tokio::test]
async fn an_unanswered_request_ends_with_nothing_created() -> TestResult {
    let w = world(300).await?;
    let outcome = ask(&w, json!({"sandbox": "website", "prompt": "Rewrite it"}))?.await??;
    assert_eq!(outcome["outcome"], "unanswered", "{outcome}");
    assert!(requests(&w, &w.alice, "s1").await?.is_empty());
    assert_eq!(task_count(&w, "s2").await?, 0);
    Ok(())
}

/// XSR-004: a dropped call (the Superagent was interrupted) takes its request with it.
#[tokio::test]
async fn a_dropped_call_takes_its_request_with_it() -> TestResult {
    let w = world(60_000).await?;
    let call_ = ask(&w, json!({"sandbox": "website", "prompt": "Rewrite it"}))?;
    let listed = waiting(&w, 1).await?;
    call_.abort();
    waiting(&w, 0).await?;
    let late = answer(
        &w,
        &w.alice,
        "s1",
        listed[0]["id"].as_str().ok_or("id")?,
        true,
    )
    .await?;
    assert_eq!(late.status, StatusCode::NOT_FOUND);
    assert_eq!(task_count(&w, "s2").await?, 0);
    Ok(())
}

/// AC-XSR-3: an API token cannot answer (the Superagent could hold one), and another person
/// cannot answer someone else's request; the request still waits.
#[tokio::test]
async fn only_the_person_in_a_browser_answers() -> TestResult {
    let w = world(60_000).await?;
    let call_ = ask(&w, json!({"sandbox": "website", "prompt": "Rewrite it"}))?;
    let listed = waiting(&w, 1).await?;
    let id = listed[0]["id"].as_str().ok_or("id")?;

    let alice_token = alice_api_token(&w).await?;
    let by_token = answer(&w, &alice_token, "s1", id, true).await?;
    assert_eq!(
        by_token.status,
        StatusCode::FORBIDDEN,
        "{:?}",
        by_token.body
    );
    let by_bob = answer(&w, &w.bob, "s1", id, true).await?;
    assert_eq!(by_bob.status, StatusCode::NOT_FOUND, "{:?}", by_bob.body);
    assert_eq!(waiting(&w, 1).await?.len(), 1, "still waiting");
    assert_eq!(task_count(&w, "s2").await?, 0);
    call_.abort();
    Ok(())
}

/// An operate API token of alice's own, made through Settings.
async fn alice_api_token(w: &World) -> Result<Creds, Error> {
    let made = call(
        &w.h.router,
        Method::POST,
        "/api/me/tokens",
        Some(json!({"label": "laptop", "scope": "operate"})),
        &w.alice,
    )
    .await?;
    assert!(made.status.is_success(), "{:?}", made.body);
    Ok(Creds {
        bearer: Some(made.body["token"].as_str().ok_or("token")?.to_owned()),
        ..Creds::default()
    })
}

/// AC-XSR-4: what cannot be asked is refused at once, and nothing waits.
#[tokio::test]
async fn what_cannot_be_asked_is_refused_without_asking() -> TestResult {
    let w = world(60_000).await?;
    let caller = superagent("s1", &w.alice_id, "alice")?;
    for (args, says) in [
        (json!({"sandbox": "studio", "prompt": "x"}), "task_create"),
        (json!({"sandbox": "docs", "prompt": "x"}), "not found"),
        (json!({"sandbox": "secret", "prompt": "x"}), "not found"),
        (json!({"sandbox": "nowhere", "prompt": "x"}), "not found"),
        (json!({"sandbox": "website", "prompt": "  "}), "prompt"),
        (json!({"sandbox": "website"}), "prompt"),
        (
            json!({"sandbox": "website", "prompt": "x", "colour": "red"}),
            "colour",
        ),
    ] {
        let refused = w.h.app.tool_sandbox_task_create(&caller, &args).await;
        let err = refused.err().ok_or_else(|| format!("{args} was asked"))?;
        assert!(
            err.public_message().contains(says),
            "{args}: {}",
            err.public_message()
        );
    }
    assert!(requests(&w, &w.alice, "s1").await?.is_empty());
    Ok(())
}

/// XSR-004: at most 3 requests wait per person and sandbox.
#[tokio::test]
async fn a_fourth_waiting_request_is_refused() -> TestResult {
    let w = world(60_000).await?;
    let calls = (0..3)
        .map(|i| {
            ask(
                &w,
                json!({"sandbox": "website", "prompt": format!("Job {i}")}),
            )
        })
        .collect::<Result<Vec<_>, _>>()?;
    waiting(&w, 3).await?;
    let fourth =
        w.h.app
            .tool_sandbox_task_create(
                &superagent("s1", &w.alice_id, "alice")?,
                &json!({"sandbox": "website", "prompt": "Job 4"}),
            )
            .await;
    let err = fourth.err().ok_or("a fourth request was asked")?;
    assert!(
        err.public_message().contains("3"),
        "{}",
        err.public_message()
    );
    // bob's Superagent has its own bound.
    let app = w.h.app.clone();
    let bob = superagent("s1", &w.bob_id, "bob")?;
    let bobs = tokio::spawn(async move {
        app.tool_sandbox_task_create(&bob, &json!({"sandbox": "website", "prompt": "Job b"}))
            .await
    });
    let deadline = tokio::time::Instant::now() + WAIT;
    while requests(&w, &w.bob, "s1").await?.len() != 1 {
        assert!(
            tokio::time::Instant::now() < deadline,
            "bob's request never waited"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    bobs.abort();
    for c in calls {
        c.abort();
    }
    Ok(())
}

/// XSR-006: approving rechecks the person's access in the target: one who lost it gets the
/// refusal, as does the waiting call, and nothing is created.
#[tokio::test]
async fn approval_rechecks_access_to_the_target() -> TestResult {
    let w = world(60_000).await?;
    let call_ = ask(&w, json!({"sandbox": "website", "prompt": "Rewrite it"}))?;
    let listed = waiting(&w, 1).await?;
    w.h.github
        .set_perm("alice", "Quartio-Studio/website", perms("read"));
    // Once the cached access is stale, GitHub is asked again.
    w.h.clock
        .0
        .fetch_add(w.h.app.config.permission_ttl_ms + 1, Ordering::SeqCst);
    let refused = answer(
        &w,
        &w.alice,
        "s1",
        listed[0]["id"].as_str().ok_or("id")?,
        true,
    )
    .await?;
    assert!(!refused.status.is_success(), "{:?}", refused.body);
    assert!(call_.await?.is_err());
    assert_eq!(task_count(&w, "s2").await?, 0);
    Ok(())
}

/// The answers `superagent.cross_task` audited about `sandbox`, oldest first (XSR-009).
async fn audited(w: &World, sandbox: &str) -> Result<Vec<Value>, Error> {
    let mut rows =
        w.h.store
            .audit_details(sandbox, "superagent.cross_task", 20)
            .await?
            .iter()
            .map(|d| serde_json::from_str(d))
            .collect::<Result<Vec<Value>, _>>()?;
    rows.reverse();
    Ok(rows)
}

/// XSR-009: every way a request ends is audited on the target, with the source and the
/// caller and never the prompt; XSR-007: its announcements reach its person only.
#[tokio::test]
async fn answers_are_audited_without_the_prompt_and_announced_to_the_person_only() -> TestResult {
    let w = world(60_000).await?;
    let mut events = w.h.app.events.subscribe();
    let secret_words = "Rotate the payment keys";
    let approved = ask(&w, json!({"sandbox": "website", "prompt": secret_words}))?;
    let id = waiting(&w, 1).await?[0]["id"]
        .as_str()
        .ok_or("id")?
        .to_owned();
    assert_eq!(
        answer(&w, &w.alice, "s1", &id, true).await?.status,
        StatusCode::OK
    );
    approved.await??;
    let declined = ask(&w, json!({"sandbox": "website", "prompt": secret_words}))?;
    let id = waiting(&w, 1).await?[0]["id"]
        .as_str()
        .ok_or("id")?
        .to_owned();
    assert_eq!(
        answer(&w, &w.alice, "s1", &id, false).await?.status,
        StatusCode::OK
    );
    declined.await??;

    let rows = audited(&w, "s2").await?;
    let outcomes: Vec<&str> = rows.iter().filter_map(|r| r["outcome"].as_str()).collect();
    assert_eq!(outcomes, ["approved", "declined"], "{rows:?}");
    for row in &rows {
        assert_eq!(row["from_sandbox"], "s1", "{row}");
        assert_eq!(row["via"], "cockpit", "{row}");
        assert!(!row.to_string().contains(secret_words), "{row}");
    }

    let mut announced = 0;
    while let Ok(envelope) = events.try_recv() {
        if let UiEvent::SuperagentRequests { sandbox_id } = &envelope.event {
            assert_eq!(sandbox_id, "s1");
            assert_eq!(envelope.user.as_ref(), Some(&w.alice_id), "to alice only");
            announced += 1;
        }
    }
    assert!(announced >= 4, "asked and answered, twice: {announced}");
    Ok(())
}

/// An unanswered request is audited `expired` on the target.
#[tokio::test]
async fn an_unanswered_request_is_audited_as_expired() -> TestResult {
    let w = world(300).await?;
    ask(&w, json!({"sandbox": "website", "prompt": "Rewrite it"}))?.await??;
    let rows = audited(&w, "s2").await?;
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!(rows[0]["outcome"], "expired");
    Ok(())
}

/// Review finding: an approval whose page went away mid-way (its request cancelled after the
/// request was taken) still creates the task and tells the waiting call.
#[tokio::test]
async fn an_approval_whose_page_went_away_still_reaches_the_call() -> TestResult {
    let w = world(60_000).await?;
    let call_ = ask(&w, json!({"sandbox": "website", "prompt": "Rewrite it"}))?;
    let id = waiting(&w, 1).await?[0]["id"]
        .as_str()
        .ok_or("id")?
        .to_owned();
    let alice = w.h.store.user(&w.alice_id).await?.ok_or("alice")?;
    let ctx = AuthContext {
        user: alice,
        csrf_token: Some("csrf".into()),
        session_hash: None,
        token: None,
    };
    // Polled once (the request is taken), then dropped, as a cancelled HTTP request is.
    let _ = tokio::time::timeout(
        Duration::ZERO,
        w.h.app.superagent_request_answer(&ctx, "s1", &id, true),
    )
    .await;
    let outcome = tokio::time::timeout(WAIT, call_).await???;
    assert_eq!(outcome["outcome"], "approved", "{outcome}");
    assert_eq!(task_count(&w, "s2").await?, 1);
    Ok(())
}
