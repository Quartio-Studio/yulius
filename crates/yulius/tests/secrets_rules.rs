//! Method and path rules of agent secrets: a secret with rules goes only
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

/// Rules are stored with the secret, shown to every reader,
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
    assert_eq!(
        seen["rules"][0]["path"], "/repos/*",
        "readers see the rules"
    );

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

/// A placeholder of a secret with rules is replaced only
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
    add(
        &w,
        json!({"name": "OPEN_TOKEN", "hosts": [HOST], "value": OPEN}),
    )
    .await?;
    let s1 = client(&w).await?;
    let read = [("authorization", "Bearer yulius-secret-GH_READ")];

    let get =
        w.h.app
            .egress_apply(
                &s1,
                request("GET", "/repos/acme/app/issues?state=open", &read),
            )
            .await;
    assert_eq!(outcome(&get), Ok(Some(format!("Bearer {READ}"))));

    let post =
        w.h.app
            .egress_apply(&s1, request("POST", "/repos/acme/app/issues", &read))
            .await;
    let message = refusal(&post)?;
    for part in [
        "GH_READ",
        "POST",
        "/repos/acme/app/issues",
        HOST,
        "GET /repos/*",
    ] {
        assert!(message.contains(part), "{part}: {message}");
    }
    assert!(!message.contains(READ));

    let outside =
        w.h.app
            .egress_apply(&s1, request("GET", "/user", &read))
            .await;
    let message = refusal(&outside)?;
    assert!(message.contains("GET /user"), "{message}");

    // The placeholder in the query uses it too.
    let query =
        w.h.app
            .egress_apply(
                &s1,
                request("DELETE", "/repos/acme?token=yulius-secret-GH_READ", &[]),
            )
            .await;
    refusal(&query)?;

    // A secret without rules on the same host: every method and path, as before.
    for (method, path) in [("POST", "/repos/acme/app/issues"), ("DELETE", "/user")] {
        let open =
            w.h.app
                .egress_apply(
                    &s1,
                    request(
                        method,
                        path,
                        &[("authorization", "Bearer yulius-secret-OPEN_TOKEN")],
                    ),
                )
                .await;
        assert_eq!(
            outcome(&open),
            Ok(Some(format!("Bearer {OPEN}"))),
            "{method}"
        );
    }
    // No secret used: forwarded untouched.
    let none =
        w.h.app
            .egress_apply(&s1, request("POST", "/markdown", &[]))
            .await;
    assert_eq!(outcome(&none), Ok(None));
    Ok(())
}

/// An injected secret with rules is added only to matching requests;
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
    let get = app
        .egress_apply(&s1, request("GET", "/v1/items", &[]))
        .await;
    assert_eq!(outcome(&get), Ok(Some(format!("Bearer {READ}"))));
    let post = app
        .egress_apply(&s1, request("POST", "/v1/items", &[]))
        .await;
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

/// For a secret with rules, a matching path that a server could normalise to
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
    add(
        &w,
        json!({"name": "OPEN_TOKEN", "hosts": [HOST], "value": OPEN}),
    )
    .await?;
    let s1 = client(&w).await?;
    let read = ("authorization", "Bearer yulius-secret-GH_READ");
    for path in [
        "/v1/../admin",
        "/v1/%2e%2E/admin",
        "/v1/x\\..\\admin",
        "/v1/x%5Cadmin",
        "/v1/admin%00/read",
        // Review round 1: the proxy's own client drops single-dot segments, Java servers read
        // `;` as path parameters, and a server decoding twice reads `%252e` as a dot.
        "/v1/./items",
        "/v1/items/.",
        "/v1/users;.json",
        "/v1/%252e%252e/admin",
        // A method override in the query (`_method`, honoured by some frameworks).
        "/v1/items?_method=DELETE",
        "/v1/items?a=1&_METHOD=delete",
    ] {
        let decision =
            w.h.app
                .egress_apply(&s1, request("GET", path, &[read]))
                .await;
        refusal(&decision).map_err(|e| format!("{path}: {e}"))?;
    }
    for header in [
        "x-http-method-override",
        "x-http-method",
        "x-method-override",
        // Path overrides some rewrite set-ups honour (review round 1).
        "x-original-url",
        "x-rewrite-url",
        // Underscores: CGI-style servers read them as the dashed name (review round 2).
        "x_http_method_override",
        "x-http_method",
    ] {
        let decision =
            w.h.app
                .egress_apply(
                    &s1,
                    request("GET", "/v1/items", &[read, (header, "DELETE")]),
                )
                .await;
        refusal(&decision).map_err(|e| format!("{header}: {e}"))?;
    }
    for fine in ["/v1/projects/group%2Fapp", "/v1/items?method=x&q=_method"] {
        let decision =
            w.h.app
                .egress_apply(&s1, request("GET", fine, &[read]))
                .await;
        assert_eq!(
            outcome(&decision),
            Ok(Some(format!("Bearer {READ}"))),
            "{fine}"
        );
    }
    // A secret without rules is not held to them.
    let open =
        w.h.app
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

/// Every refusal is audited with the secret's name, the method, the path and the
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
        let post =
            w.h.app
                .egress_apply(&s1, request("POST", "/repos/acme/issues?secret=q", &read))
                .await;
        refusal(&post)?;
    }
    let delete =
        w.h.app
            .egress_apply(&s1, request("DELETE", "/repos/acme", &read))
            .await;
    refusal(&delete)?;
    let refused = activity(&w, "vault.agent.refused").await?;
    assert_eq!(refused.len(), 2, "{refused:?}");
    let details: Vec<&str> = refused
        .iter()
        .filter_map(|e| e["detail"].as_str())
        .collect();
    for (method, path) in [("POST", "/repos/acme/issues"), ("DELETE", "/repos/acme")] {
        assert!(
            details.iter().any(|d| d.contains("GH_READ")
                && d.contains(&format!("{method} {path}"))
                && d.contains(HOST)),
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

/// A task's agent reads each secret's rules in `secrets_list`, and how they work.
#[tokio::test]
async fn secrets_list_shows_the_rules_to_agents() -> TestResult {
    let w = world().await?;
    add(
        &w,
        json!({"name": "GH_READ", "hosts": [HOST], "value": READ,
               "rules": [{"methods": ["GET"], "path": "/repos/*"}]}),
    )
    .await?;
    add(
        &w,
        json!({"name": "OPEN_TOKEN", "hosts": [HOST], "value": OPEN}),
    )
    .await?;
    let listed = support::mcp_tool(&w.h, TOKEN, "secrets_list", json!({})).await?;
    let listed = support::tool_text(&listed);
    let secret = |name: &str| {
        listed["secrets"]
            .as_array()
            .and_then(|a| a.iter().find(|s| s["name"] == name))
            .cloned()
            .unwrap_or_default()
    };
    assert_eq!(
        secret("GH_READ")["rules"],
        json!([{"methods": ["GET"], "path": "/repos/*"}])
    );
    assert_eq!(secret("OPEN_TOKEN")["rules"], json!([]));
    let how = listed["how_to_use"].to_string();
    assert!(how.contains("rules") && how.contains("403"), "{how}");
    assert!(!listed.to_string().contains(READ));
    Ok(())
}

/// Review round 1, finding 1: another secret's value placed in the path cannot move it
/// outside the rules once replaced (`..` resolved by the proxy's client, `?` starting a query,
/// `#` a fragment): the rules hold for the path as sent and as it goes out.
#[tokio::test]
async fn a_placeholder_value_cannot_move_the_path_outside_the_rules() -> TestResult {
    let w = world().await?;
    add(
        &w,
        json!({"name": "COMMENTER", "hosts": [HOST], "injection": "bearer", "value": READ,
               "rules": [{"methods": ["POST"], "path": "/repos/acme/issues/*/comments"}]}),
    )
    .await?;
    for (name, value) in [
        ("UP", "../../../../user/keys?"),
        ("QUERY", "a-value-1?"),
        ("FRAGMENT", "a-value-2#"),
    ] {
        add(&w, json!({"name": name, "hosts": [HOST], "value": value})).await?;
    }
    let s1 = client(&w).await?;
    for name in ["UP", "QUERY", "FRAGMENT"] {
        let path = format!("/repos/acme/issues/yulius-secret-{name}/comments");
        let decision = w.h.app.egress_apply(&s1, request("POST", &path, &[])).await;
        let message = refusal(&decision).map_err(|e| format!("{name}: {e}"))?;
        assert!(message.contains(&path), "the path as sent: {message}");
        assert!(
            !message.contains("a-value") && !message.contains("user/keys"),
            "never the value: {message}"
        );
        if name == "QUERY" {
            assert!(message.contains("value replaced"), "{message}");
        }
    }
    // An ordinary value there still goes, with the commenter's header.
    add(
        &w,
        json!({"name": "ISSUE", "hosts": [HOST], "value": "issue-number-42"}),
    )
    .await?;
    let fine =
        w.h.app
            .egress_apply(
                &s1,
                request(
                    "POST",
                    "/repos/acme/issues/yulius-secret-ISSUE/comments",
                    &[],
                ),
            )
            .await;
    assert_eq!(outcome(&fine), Ok(Some(format!("Bearer {READ}"))));
    // Review round 3: a wrong method is not blamed on the value in the path.
    let wrong =
        w.h.app
            .egress_apply(
                &s1,
                request(
                    "GET",
                    "/repos/acme/issues/yulius-secret-ISSUE/comments",
                    &[],
                ),
            )
            .await;
    let message = refusal(&wrong)?;
    assert!(!message.contains("value replaced"), "{message}");
    Ok(())
}

/// Review round 1, finding 6: a placeholder inside Basic credentials is a use
/// too, and a CI job's request outside the rules is refused and audited as `ci:<run>`.
#[tokio::test]
async fn basic_credentials_and_ci_jobs_are_held_to_the_rules() -> TestResult {
    use base64::Engine as _;
    let w = world().await?;
    add(
        &w,
        json!({"name": "GH_READ", "hosts": [HOST], "value": READ,
               "rules": [{"methods": ["GET"], "path": "/repos/*"}]}),
    )
    .await?;
    let s1 = client(&w).await?;
    let basic = format!(
        "Basic {}",
        base64::engine::general_purpose::STANDARD.encode("bot:yulius-secret-GH_READ")
    );
    let post =
        w.h.app
            .egress_apply(
                &s1,
                request("POST", "/repos/a", &[("authorization", &basic)]),
            )
            .await;
    refusal(&post)?;
    let get =
        w.h.app
            .egress_apply(
                &s1,
                request("GET", "/repos/a", &[("authorization", &basic)]),
            )
            .await;
    let expected = format!(
        "Basic {}",
        base64::engine::general_purpose::STANDARD.encode(format!("bot:{READ}"))
    );
    assert_eq!(outcome(&get), Ok(Some(expected)));

    let sandbox = yulius_domain::ids::SandboxId::parse("s1")?;
    let token =
        w.h.app
            .ci_egress_open(&sandbox, "run-7", "job-1", &["GH_READ".to_owned()])
            .ok_or("ci credential")?;
    let ci = w.h.app.egress_client(&token).await.ok_or("ci client")?;
    let read = [("authorization", "Bearer yulius-secret-GH_READ")];
    let delete =
        w.h.app
            .egress_apply(&ci, request("DELETE", "/repos/a", &read))
            .await;
    refusal(&delete)?;
    let refused = activity(&w, "vault.agent.refused").await?;
    assert!(
        refused.iter().any(|e| e["actor"] == "ci:run-7"
            && e["detail"]
                .as_str()
                .is_some_and(|d| d.contains("DELETE /repos/a"))),
        "{refused:?}"
    );
    Ok(())
}

/// Review round 1, finding 3: a run's distinct refusals of one secret are audited up to
/// a bound, then once as "further refusals not listed", so varying the path cannot flood the trail.
#[tokio::test]
async fn a_runs_distinct_refusals_are_audited_up_to_a_bound() -> TestResult {
    /// `REFUSALS_LISTED` in `yulius-app` `vault/rules.rs`.
    const LISTED: usize = 20;
    let w = world().await?;
    add(
        &w,
        json!({"name": "GH_READ", "hosts": [HOST], "value": READ,
               "rules": [{"methods": ["GET"], "path": "/repos/*"}]}),
    )
    .await?;
    let s1 = client(&w).await?;
    let read = [("authorization", "Bearer yulius-secret-GH_READ")];
    for i in 0..LISTED + 5 {
        let decision =
            w.h.app
                .egress_apply(&s1, request("GET", &format!("/user/{i}"), &read))
                .await;
        refusal(&decision)?;
    }
    let refused = activity(&w, "vault.agent.refused").await?;
    assert_eq!(refused.len(), LISTED + 1, "{refused:?}");
    assert_eq!(
        refused
            .iter()
            .filter(|e| e["detail"]
                .as_str()
                .is_some_and(|d| d.contains("further refusals")))
            .count(),
        1,
        "{refused:?}"
    );
    Ok(())
}

/// Review round 1, finding 2: a secret with rules is not bound to hosts an agent asks
/// for, even with `auto_allow`: a person allows them, so an agent cannot send it to a server of
/// its own and learn the value there. A secret without rules still binds them at once.
#[tokio::test]
async fn hosts_asked_for_a_secret_with_rules_wait_for_a_person() -> TestResult {
    let w = world().await?;
    let ruled = add(
        &w,
        json!({"name": "GH_READ", "hosts": [HOST], "value": READ,
               "rules": [{"methods": ["GET"], "path": "/repos/*"}]}),
    )
    .await?;
    assert_eq!(ruled["auto_allow"], true, "the default");
    add(&w, json!({"name": "OPEN_TOKEN", "value": OPEN})).await?;
    for name in ["GH_READ", "OPEN_TOKEN"] {
        let asked = support::mcp_tool(
            &w.h,
            TOKEN,
            "secret_request_hosts",
            json!({"secret": name, "hosts": ["collector.example.com"]}),
        )
        .await?;
        assert_ne!(asked["isError"], true, "{name}: {asked}");
    }
    let listed = call(
        &w.h.router,
        Method::GET,
        "/api/sandboxes/s1/agent-secrets",
        None,
        &w.writer,
    )
    .await?;
    let secret = |name: &str| {
        listed
            .body
            .as_array()
            .and_then(|a| a.iter().find(|s| s["name"] == name))
            .cloned()
            .unwrap_or_default()
    };
    assert_eq!(secret("GH_READ")["hosts"], json!([HOST]));
    assert_eq!(
        secret("GH_READ")["requested_hosts"],
        json!(["collector.example.com"])
    );
    assert_eq!(
        secret("OPEN_TOKEN")["hosts"],
        json!(["collector.example.com"])
    );
    // The collector is bound to OPEN_TOKEN only: GH_READ's placeholder goes there unreplaced.
    let s1 = client(&w).await?;
    let mut there = request(
        "GET",
        "/repos/x",
        &[("authorization", "Bearer yulius-secret-GH_READ")],
    );
    there.host = "collector.example.com".into();
    let decision = w.h.app.egress_apply(&s1, there).await;
    assert_eq!(
        outcome(&decision),
        Ok(Some("Bearer yulius-secret-GH_READ".to_owned()))
    );
    Ok(())
}
