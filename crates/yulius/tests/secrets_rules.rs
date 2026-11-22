//! Method and path rules of agent secrets (SPEC-627, ADR-0458): a secret with rules goes only
//! into requests that match them; any other request that would use it is refused and audited.

use crate::support;

use axum::http::{Method, StatusCode};
use serde_json::{Value, json};
use support::{
    Creds, TestResult, call, perms, seed_github_user, seed_live_attempt, seed_sandbox, seed_task,
};
use yulius_app::ports::egress::{EgressClient, EgressDecision, EgressPolicy as _, EgressRequest};
use yulius_domain::task::TaskStatus;

const TOKEN: &str = "rules-task-token-s1-0123456789abcdefgh";
const HOST: &str = "api.rules.test";
const READ: &str = "read-only-value-5150";
const OPEN: &str = "open-token-value-7a7a";

struct World {
    h: support::Harness,
    writer: Creds,
    reader: Creds,
}

/// Sandbox s1 (`app`): alice writes it, bob reads it; task t1 runs there with a live run.
async fn world() -> Result<World, Box<dyn std::error::Error>> {
    let h = support::harness()?;
    let writer = seed_github_user(&h, "u-alice", "alice").await?;
    let reader = seed_github_user(&h, "u-bob", "bob").await?;
    seed_sandbox(&h, "s1", "app", "u-alice").await?;
    h.github.set_perm("alice", "acme/app", perms("write"));
    h.github.set_perm("bob", "acme/app", perms("read"));
    seed_task(&h, "t1", "s1", "u-alice", TaskStatus::Running, None).await?;
    seed_live_attempt(&h, "t1", TOKEN).await?;
    Ok(World { h, writer, reader })
}

async fn add(w: &World, body: Value) -> Result<Value, Box<dyn std::error::Error>> {
    let created = call(
        &w.h.router,
        Method::POST,
        "/api/sandboxes/s1/agent-secrets",
        Some(body),
        &w.writer,
    )
    .await?;
    assert_eq!(created.status, StatusCode::CREATED, "{}", created.body);
    Ok(created.body)
}

async fn client(w: &World) -> Result<EgressClient, Box<dyn std::error::Error>> {
    Ok(w.h.app.egress_client(TOKEN).await.ok_or("proxy client")?)
}

fn request(method: &str, path: &str, headers: &[(&str, &str)]) -> EgressRequest {
    EgressRequest {
        method: method.into(),
        host: HOST.into(),
        port: 443,
        path_and_query: path.into(),
        headers: headers
            .iter()
            .map(|(n, v)| ((*n).to_owned(), v.as_bytes().to_vec()))
            .collect(),
    }
}

/// The `authorization` header forwarded, or the refusal's status and text.
fn outcome(decision: &EgressDecision) -> Result<Option<String>, (u16, String)> {
    match decision {
        EgressDecision::Forward { headers, .. } => Ok(headers
            .iter()
            .find(|(n, _)| n == "authorization")
            .map(|(_, v)| String::from_utf8_lossy(v).into_owned())),
        EgressDecision::Refuse { status, message } => Err((*status, message.clone())),
    }
}

/// The refusal of `decision`, or an error when it was forwarded.
fn refusal(decision: &EgressDecision) -> Result<String, Box<dyn std::error::Error>> {
    match outcome(decision) {
        Err((403, message)) => Ok(message),
        other => Err(format!("expected a 403 refusal, got {other:?}").into()),
    }
}

async fn activity(w: &World, action: &str) -> Result<Vec<Value>, Box<dyn std::error::Error>> {
    let listed = call(
        &w.h.router,
        Method::GET,
        "/api/secrets/activity",
        None,
        &w.writer,
    )
    .await?;
    Ok(listed
        .body
        .as_array()
        .ok_or("activity")?
        .iter()
        .filter(|e| e["action"] == action)
        .cloned()
        .collect())
}

/// AC-6 (MPR-001, MPR-006, MPR-007): rules are stored with the secret, shown to every reader,
/// replaced and cleared by its managers, and refused with 422 when malformed.
#[tokio::test]
async fn rules_are_stored_shown_replaced_and_validated() -> TestResult {
    let w = world().await?;
    let created = add(
        &w,
        json!({"name": "GH_READ", "hosts": [HOST], "value": READ,
               "rules": [{"methods": ["get", "HEAD"], "path": "/repos/*"}]}),
    )
    .await?;
    assert_eq!(
        created["rules"],
        json!([{"methods": ["GET", "HEAD"], "path": "/repos/*"}])
    );
    let id = created["id"].as_str().ok_or("id")?.to_owned();
    let plain = add(&w, json!({"name": "PLAIN", "hosts": [HOST], "value": OPEN})).await?;
    assert_eq!(plain["rules"], json!([]), "no rules unless given");

    let listed = call(
        &w.h.router,
        Method::GET,
        "/api/sandboxes/s1/agent-secrets",
        None,
        &w.reader,
    )
    .await?;
    let seen = listed
        .body
        .as_array()
        .and_then(|a| a.iter().find(|s| s["name"] == "GH_READ"))
        .ok_or("listed")?;
    assert_eq!(seen["rules"][0]["path"], "/repos/*", "readers see the rules");

    let patch = |body: Value, creds: &Creds| {
        let (router, path, creds) = (
            w.h.router.clone(),
            format!("/api/agent-secrets/{id}"),
            creds.clone(),
        );
        async move { call(&router, Method::PATCH, &path, Some(body), &creds).await }
    };
    let refused = patch(json!({"rules": []}), &w.reader).await?;
    assert_eq!(refused.status, StatusCode::FORBIDDEN, "{}", refused.body);
    let described = patch(json!({"description": "reads issues"}), &w.writer).await?;
    assert_eq!(described.status, StatusCode::OK, "{}", described.body);
    assert_eq!(
        described.body["rules"][0]["methods"],
        json!(["GET", "HEAD"]),
        "an edit without rules keeps them"
    );
    let replaced = patch(
        json!({"rules": [{"methods": ["POST"], "path": "/graphql"}, {"methods": ["GET"], "path": "/user"}]}),
        &w.writer,
    )
    .await?;
    assert_eq!(replaced.status, StatusCode::OK, "{}", replaced.body);
    assert_eq!(
        replaced.body["rules"],
        json!([{"methods": ["POST"], "path": "/graphql"}, {"methods": ["GET"], "path": "/user"}])
    );
    for bad in [
        json!({"rules": [{"methods": [], "path": "/v1/*"}]}),
        json!({"rules": [{"methods": ["GET"], "path": "v1/*"}]}),
        json!({"rules": [{"methods": ["GET"], "path": "/v1/../admin"}]}),
        json!({"rules": [{"methods": ["GET"], "path": "/v1/*?q=1"}]}),
    ] {
        let invalid = patch(bad.clone(), &w.writer).await?;
        assert_eq!(invalid.status, StatusCode::UNPROCESSABLE_ENTITY, "{bad}");
    }
    let kept = patch(json!({"description": "still"}), &w.writer).await?;
    assert_eq!(kept.body["rules"].as_array().map(Vec::len), Some(2));
    let cleared = patch(json!({"rules": []}), &w.writer).await?;
    assert_eq!(cleared.body["rules"], json!([]));
    let audited = activity(&w, "vault.agent.update").await?;
    assert!(
        audited
            .iter()
            .any(|e| e["detail"].as_str().is_some_and(|d| d.contains("rules"))),
        "{audited:?}"
    );
    Ok(())
}

/// AC-1, AC-2 (MPR-002, MPR-003, MPR-008): a placeholder of a secret with rules is replaced only
/// in a matching request; a POST and a path outside the rules are refused with a text naming the
/// secret, the method and the path; a secret without rules on the same host works as before, and
/// a request that uses no secret goes out untouched.
#[tokio::test]
async fn a_secret_with_rules_is_sent_only_for_matching_requests() -> TestResult {
    let w = world().await?;
    add(
        &w,
        json!({"name": "GH_READ", "hosts": [HOST], "value": READ,
               "rules": [{"methods": ["GET"], "path": "/repos/*"}]}),
    )
    .await?;
    add(&w, json!({"name": "OPEN_TOKEN", "hosts": [HOST], "value": OPEN})).await?;
    let s1 = client(&w).await?;
    let read = [("authorization", "Bearer yulius-secret-GH_READ")];

    let get = w
        .h
        .app
        .egress_apply(&s1, request("GET", "/repos/acme/app/issues?state=open", &read))
        .await;
    assert_eq!(outcome(&get), Ok(Some(format!("Bearer {READ}"))));

    let post = w
        .h
        .app
        .egress_apply(&s1, request("POST", "/repos/acme/app/issues", &read))
        .await;
    let message = refusal(&post)?;
    for part in ["GH_READ", "POST", "/repos/acme/app/issues", HOST, "GET /repos/*"] {
        assert!(message.contains(part), "{part}: {message}");
    }
    assert!(!message.contains(READ));

    let outside = w
        .h
        .app
        .egress_apply(&s1, request("GET", "/user", &read))
        .await;
    let message = refusal(&outside)?;
    assert!(message.contains("GET /user"), "{message}");

    // The placeholder in the query uses it too.
    let query = w
        .h
        .app
        .egress_apply(
            &s1,
            request("DELETE", "/repos/acme?token=yulius-secret-GH_READ", &[]),
        )
        .await;
    refusal(&query)?;

    // A secret without rules on the same host: every method and path, as before.
    for (method, path) in [("POST", "/repos/acme/app/issues"), ("DELETE", "/user")] {
        let open = w
            .h
            .app
            .egress_apply(
                &s1,
                request(method, path, &[("authorization", "Bearer yulius-secret-OPEN_TOKEN")]),
            )
            .await;
        assert_eq!(outcome(&open), Ok(Some(format!("Bearer {OPEN}"))), "{method}");
    }
    // No secret used: forwarded untouched.
    let none = w
        .h
        .app
        .egress_apply(&s1, request("POST", "/markdown", &[]))
        .await;
    assert_eq!(outcome(&none), Ok(None));
    Ok(())
}

/// AC-3 (MPR-002, MPR-003): an injected secret with rules is added only to matching requests;
/// of two secrets injecting one header, each goes with the requests its rules allow; a request
/// neither allows, and one naming a secret outside its rules, is refused.
#[tokio::test]
async fn injection_follows_the_rules() -> TestResult {
    let w = world().await?;
    add(
        &w,
        json!({"name": "READER", "hosts": [HOST], "injection": "bearer", "value": READ,
               "rules": [{"methods": ["GET", "HEAD"], "path": "/*"}]}),
    )
    .await?;
    add(
        &w,
        json!({"name": "WRITER", "hosts": [HOST], "injection": "bearer", "value": OPEN,
               "rules": [{"methods": ["POST"], "path": "/v1/items"}]}),
    )
    .await?;
    let s1 = client(&w).await?;
    let app = &w.h.app;
    let get = app.egress_apply(&s1, request("GET", "/v1/items", &[])).await;
    assert_eq!(outcome(&get), Ok(Some(format!("Bearer {READ}"))));
    let post = app.egress_apply(&s1, request("POST", "/v1/items", &[])).await;
    assert_eq!(outcome(&post), Ok(Some(format!("Bearer {OPEN}"))));
    let delete = app
        .egress_apply(&s1, request("DELETE", "/v1/items", &[]))
        .await;
    let message = refusal(&delete)?;
    assert!(
        message.contains("READER") && message.contains("WRITER"),
        "{message}"
    );
    let named = app
        .egress_apply(
            &s1,
            request("POST", "/v1/items", &[("x-yulius-secret", "READER")]),
        )
        .await;
    assert!(refusal(&named)?.contains("READER"));
    // The agent's own header: nothing is injected, nothing is used, nothing refused.
    let own = app
        .egress_apply(
            &s1,
            request("DELETE", "/v1/items", &[("authorization", "Bearer mine")]),
        )
        .await;
    assert_eq!(outcome(&own), Ok(Some("Bearer mine".into())));
    Ok(())
}

/// AC-4 (MPR-004): for a secret with rules, a matching path that a server could normalise to
/// another resource, and a method-override header, are refused; an encoded slash is not.
#[tokio::test]
async fn paths_and_overrides_that_could_escape_the_rules_are_refused() -> TestResult {
    let w = world().await?;
    add(
        &w,
        json!({"name": "GH_READ", "hosts": [HOST], "value": READ,
               "rules": [{"methods": ["GET"], "path": "/v1/*"}]}),
    )
    .await?;
    add(&w, json!({"name": "OPEN_TOKEN", "hosts": [HOST], "value": OPEN})).await?;
    let s1 = client(&w).await?;
    let read = ("authorization", "Bearer yulius-secret-GH_READ");
    for path in [
        "/v1/../admin",
        "/v1/%2e%2E/admin",
        "/v1/x\\..\\admin",
        "/v1/x%5Cadmin",
        "/v1/admin%00/read",
    ] {
        let decision = w.h.app.egress_apply(&s1, request("GET", path, &[read])).await;
        refusal(&decision).map_err(|e| format!("{path}: {e}"))?;
    }
    for header in ["x-http-method-override", "x-http-method", "x-method-override"] {
        let decision = w
            .h
            .app
            .egress_apply(&s1, request("GET", "/v1/items", &[read, (header, "DELETE")]))
            .await;
        refusal(&decision).map_err(|e| format!("{header}: {e}"))?;
    }
    let slash = w
        .h
        .app
        .egress_apply(&s1, request("GET", "/v1/projects/group%2Fapp", &[read]))
        .await;
    assert_eq!(outcome(&slash), Ok(Some(format!("Bearer {READ}"))));
    // A secret without rules is not held to them.
    let open = w
        .h
        .app
        .egress_apply(
            &s1,
            request(
                "GET",
                "/v1/../admin",
                &[("authorization", "Bearer yulius-secret-OPEN_TOKEN")],
            ),
        )
        .await;
    assert_eq!(outcome(&open), Ok(Some(format!("Bearer {OPEN}"))));
    Ok(())
}

/// AC-5 (MPR-005): every refusal is audited with the secret's name, the method, the path and the
/// host, never a value or the query; the same refusal repeated by the same run is one entry.
#[tokio::test]
async fn refusals_are_audited_once_per_run_without_values() -> TestResult {
    let w = world().await?;
    add(
        &w,
        json!({"name": "GH_READ", "hosts": [HOST], "value": READ,
               "rules": [{"methods": ["GET"], "path": "/repos/*"}]}),
    )
    .await?;
    let s1 = client(&w).await?;
    let read = [("authorization", "Bearer yulius-secret-GH_READ")];
    for _ in 0..3 {
        let post = w
            .h
            .app
            .egress_apply(&s1, request("POST", "/repos/acme/issues?secret=q", &read))
            .await;
        refusal(&post)?;
    }
    let delete = w
        .h
        .app
        .egress_apply(&s1, request("DELETE", "/repos/acme", &read))
        .await;
    refusal(&delete)?;
    let refused = activity(&w, "vault.agent.refused").await?;
    assert_eq!(refused.len(), 2, "{refused:?}");
    let details: Vec<&str> = refused.iter().filter_map(|e| e["detail"].as_str()).collect();
    for (method, path) in [("POST", "/repos/acme/issues"), ("DELETE", "/repos/acme")] {
        assert!(
            details
                .iter()
                .any(|d| d.contains("GH_READ") && d.contains(&format!("{method} {path}")) && d.contains(HOST)),
            "{method} {path}: {details:?}"
        );
    }
    for entry in &refused {
        assert_eq!(entry["actor"], "task:1");
        assert!(entry["secret_id"].is_string());
        let text = entry.to_string();
        assert!(!text.contains(READ) && !text.contains("secret=q"), "{text}");
    }
    assert!(
        activity(&w, "vault.agent.use").await?.is_empty(),
        "nothing was sent"
    );
    Ok(())
}
