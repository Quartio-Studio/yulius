//! Project variables (SPEC-275, ADR-0203): a project's non-secret values in plain, listed to
//! everyone who sees it, set and deleted with write access, audited without values, and never
//! under the name of one of its agent secrets.

use crate::support;

use axum::http::{Method, StatusCode};
use serde_json::{Value, json};
use support::{Creds, TestResult, call, harness, perms, seed_github_user, seed_sandbox};
use yulius_app::ports::store_ext::AutomationStore as _;

type Error = Box<dyn std::error::Error>;

const VARS: &str = "/api/sandboxes/s1/variables";

struct World {
    h: support::Harness,
    alice: Creds,
    reader: Creds,
    admin: Creds,
}

/// Sandbox s1 (`acme/app`) written by alice and read by carol; the superadmin administers.
async fn world() -> Result<World, Error> {
    let h = harness()?;
    let alice = seed_github_user(&h, "u-alice", "alice").await?;
    let reader = seed_github_user(&h, "u-carol", "carol").await?;
    seed_sandbox(&h, "s1", "app", "u-alice").await?;
    h.github.set_perm("alice", "acme/app", perms("write"));
    h.github.set_perm("carol", "acme/app", perms("read"));
    support::setup_superadmin_creds(&h).await?;
    let admin = support::superadmin_bearer(&h).await?;
    Ok(World {
        h,
        alice,
        reader,
        admin,
    })
}

async fn put(w: &World, creds: &Creds, vars: Value) -> Result<support::Resp, Error> {
    call(
        &w.h.router,
        Method::PUT,
        VARS,
        Some(json!({ "variables": vars })),
        creds,
    )
    .await
}

async fn list(w: &World, creds: &Creds) -> Result<Value, Error> {
    let listed = call(&w.h.router, Method::GET, VARS, None, creds).await?;
    assert_eq!(listed.status, StatusCode::OK, "{}", listed.body);
    Ok(listed.body)
}

/// VAR-001, VAR-003: set in a batch (created, changed, unchanged; an absent description kept),
/// listed in plain to a reader, deleted; every change audited by name, never with the value.
#[tokio::test]
async fn variables_are_set_listed_and_deleted_with_write_access() -> TestResult {
    let w = world().await?;
    let set = put(
        &w,
        &w.alice,
        json!([
            {"name": "OVH_ENDPOINT", "value": "ovh-ca", "description": "OVH region"},
            {"name": "APP_ENV", "value": "production"},
        ]),
    )
    .await?;
    assert_eq!(set.status, StatusCode::OK, "{}", set.body);
    assert_eq!(set.body["created"], json!(["OVH_ENDPOINT", "APP_ENV"]));

    let again = put(
        &w,
        &w.alice,
        json!([
            {"name": "OVH_ENDPOINT", "value": "ovh-us"},
            {"name": "APP_ENV", "value": "production"},
        ]),
    )
    .await?;
    assert_eq!(again.body["changed"], json!(["OVH_ENDPOINT"]));
    assert_eq!(again.body["unchanged"], json!(["APP_ENV"]));

    let seen = list(&w, &w.reader).await?;
    assert_eq!(seen["can_manage"], json!(false));
    let ovh = &seen["variables"][1];
    assert_eq!(
        (
            &ovh["name"],
            &ovh["value"],
            &ovh["description"],
            &ovh["updated_by"]
        ),
        (
            &json!("OVH_ENDPOINT"),
            &json!("ovh-us"),
            &json!("OVH region"),
            &json!("alice")
        )
    );
    assert_eq!(list(&w, &w.alice).await?["can_manage"], json!(true));

    let refused = put(&w, &w.reader, json!([{"name": "X_Y", "value": "z"}])).await?;
    assert_eq!(refused.status, StatusCode::FORBIDDEN, "{}", refused.body);
    let refused = call(
        &w.h.router,
        Method::DELETE,
        &format!("{VARS}/APP_ENV"),
        None,
        &w.reader,
    )
    .await?;
    assert_eq!(refused.status, StatusCode::FORBIDDEN);

    let deleted = call(
        &w.h.router,
        Method::DELETE,
        &format!("{VARS}/APP_ENV"),
        None,
        &w.alice,
    )
    .await?;
    assert_eq!(deleted.status, StatusCode::NO_CONTENT);
    let missing = call(
        &w.h.router,
        Method::DELETE,
        &format!("{VARS}/APP_ENV"),
        None,
        &w.alice,
    )
    .await?;
    assert_eq!(missing.status, StatusCode::NOT_FOUND);
    assert_eq!(
        list(&w, &w.alice).await?["variables"]
            .as_array()
            .map(Vec::len),
        Some(1)
    );

    let audit: Vec<(String, String, String)> =
        w.h.store
            .automation_audit("s1", 50)
            .await?
            .into_iter()
            .filter(|e| e.action.starts_with("variable."))
            .map(|e| (e.action, e.actor, e.detail.unwrap_or_default()))
            .collect();
    let row = |a: &str, d: &str| (a.to_owned(), "alice".to_owned(), d.to_owned());
    assert_eq!(
        audit,
        [
            row("variable.delete", "APP_ENV"),
            row("variable.update", "OVH_ENDPOINT (value)"),
            row("variable.create", "APP_ENV"),
            row("variable.create", "OVH_ENDPOINT"),
        ]
    );
    let text = format!("{audit:?}");
    assert!(
        !text.contains("ovh-") && !text.contains("production"),
        "{text}"
    );
    Ok(())
}

/// VAR-001: names, values and descriptions are checked, and a bad batch writes nothing.
#[tokio::test]
async fn bad_variables_are_refused_and_nothing_of_the_batch_is_written() -> TestResult {
    let w = world().await?;
    for (bad, why) in [
        (json!({"name": "lower", "value": "v"}), "invalid name"),
        (json!({"name": "PATH", "value": "/x"}), "reserved"),
        (json!({"name": "YULIUS_X", "value": "v"}), "reserved"),
        (json!({"name": "A_B", "value": "two\nlines"}), "one line"),
        (
            json!({"name": "A_B", "value": "x".repeat(4097)}),
            "longer than 4096",
        ),
        (
            json!({"name": "A_B", "value": "v", "description": "d".repeat(1001)}),
            "longer than 1000",
        ),
    ] {
        let refused = put(&w, &w.alice, json!([{"name": "GOOD", "value": "ok"}, bad])).await?;
        assert_eq!(
            refused.status,
            StatusCode::UNPROCESSABLE_ENTITY,
            "{}",
            refused.body
        );
        assert!(
            refused.body.to_string().to_lowercase().contains(why),
            "{why}: {}",
            refused.body
        );
    }
    let twice = put(
        &w,
        &w.alice,
        json!([{"name": "A_B", "value": "1"}, {"name": "A_B", "value": "2"}]),
    )
    .await?;
    assert_eq!(
        twice.status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "{}",
        twice.body
    );
    assert_eq!(list(&w, &w.alice).await?["variables"], json!([]));

    let many: Vec<Value> = (0..201)
        .map(|i| json!({"name": format!("V_{i}"), "value": "x"}))
        .collect();
    let over = put(&w, &w.alice, Value::Array(many)).await?;
    assert_eq!(
        over.status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "{}",
        over.body
    );
    Ok(())
}

/// VAR-002: a project secret's name, or a global secret's switched on for the project, cannot
/// be a variable; nothing of that batch is written.
#[tokio::test]
async fn a_name_that_is_an_agent_secret_of_the_project_is_refused() -> TestResult {
    let w = world().await?;
    let created = call(
        &w.h.router,
        Method::POST,
        "/api/sandboxes/s1/agent-secrets",
        Some(json!({"name": "SHOP_TOKEN", "value": "token-value-123456"})),
        &w.alice,
    )
    .await?;
    assert_eq!(created.status, StatusCode::CREATED, "{}", created.body);
    let global = call(
        &w.h.router,
        Method::POST,
        "/api/sandboxes/s1/agent-secrets",
        Some(json!({"scope": "instance", "name": "TEAM_KEY", "value": "global-value-123456"})),
        &w.admin,
    )
    .await?;
    assert_eq!(global.status, StatusCode::CREATED, "{}", global.body);

    for name in ["SHOP_TOKEN", "TEAM_KEY"] {
        let refused = put(
            &w,
            &w.alice,
            json!([{"name": "REGION", "value": "eu"}, {"name": name, "value": "plain"}]),
        )
        .await?;
        assert_eq!(refused.status, StatusCode::CONFLICT, "{}", refused.body);
        assert!(refused.body.to_string().contains(name), "{}", refused.body);
    }
    assert_eq!(list(&w, &w.alice).await?["variables"], json!([]));
    Ok(())
}
