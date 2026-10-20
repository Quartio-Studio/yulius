//! Driving Yulius from a person's own agent (SPEC-247): personal API tokens with scopes, the
//! remote MCP on the public listener, and a laptop-only onboarding over it.

use crate::support;

use axum::http::{Method, StatusCode};
use serde_json::{Value, json};
use support::repos::World;
use support::{Creds, TestResult, call};
use yulius_app::model::UserRecord;
use yulius_app::ports::store_ext::AutomationStore;

type Error = Box<dyn std::error::Error>;

fn text(value: &Value) -> &str {
    value.as_str().unwrap_or_default()
}

/// A token of `scope` made in `browser`'s Settings; returns the creation answer.
async fn make_token(w: &World, browser: &Creds, label: &str, scope: &str) -> Result<Value, Error> {
    let made = call(
        &w.h.router,
        Method::POST,
        "/api/me/tokens",
        Some(json!({"label": label, "scope": scope})),
        browser,
    )
    .await?;
    assert_eq!(made.status, StatusCode::CREATED, "{}", made.body);
    Ok(made.body)
}

/// Bearer credentials of a token.
fn bearer(token: &str) -> Creds {
    Creds {
        bearer: Some(token.to_owned()),
        ..Creds::default()
    }
}

/// A person with an `operate` token: `(user, browser, bearer)`.
async fn person(w: &World, login: &str) -> Result<(UserRecord, Creds, Creds), Error> {
    let (user, _) = w.user(login, false).await?;
    let browser = w.browser(&user).await?;
    let made = make_token(w, &browser, "laptop", "operate").await?;
    Ok((user, browser, bearer(text(&made["token"]))))
}

/// The audit entries about `target`, newest first, as `(action, actor, detail)`.
async fn audits(w: &World, target: &str) -> Result<Vec<(String, String, String)>, Error> {
    Ok(w.h
        .store
        .automation_audit(target, 50)
        .await?
        .into_iter()
        .map(|e| (e.action, e.actor, e.detail.unwrap_or_default()))
        .collect())
}

/// RMT-001..003: a token is made in the browser with a scope, shown once with the commands that
/// use it, listed with its last use, refused at once once revoked, and audited without itself.
#[tokio::test]
async fn tokens_are_made_in_the_browser_listed_and_revoked() -> TestResult {
    let w = World::new("org").await?;
    let (alice, cli) = w.user("alice", false).await?;
    let browser = w.browser(&alice).await?;
    let (_, bob_cli) = w.user("bob", false).await?;

    // A token cannot mint tokens.
    let refused = call(
        &w.h.router,
        Method::POST,
        "/api/me/tokens",
        Some(json!({"label": "x", "scope": "operate"})),
        &cli,
    )
    .await?;
    assert_eq!(refused.status, StatusCode::FORBIDDEN, "{}", refused.body);
    for (label, scope) in [("", "operate"), ("x", "cli"), ("x", "admin")] {
        let bad = call(
            &w.h.router,
            Method::POST,
            "/api/me/tokens",
            Some(json!({"label": label, "scope": scope})),
            &browser,
        )
        .await?;
        assert_eq!(
            bad.status,
            StatusCode::UNPROCESSABLE_ENTITY,
            "{label:?} {scope}"
        );
    }

    let made = make_token(&w, &browser, "laptop Claude Code", "operate").await?;
    let token = text(&made["token"]).to_owned();
    assert!(token.starts_with("yul_"), "{made}");
    assert_eq!(made["mcp_url"], "https://yulius.test/mcp");
    assert_eq!(
        made["claude_command"],
        format!(
            "claude mcp add --transport http yulius https://yulius.test/mcp --header \"Authorization: Bearer {token}\""
        )
    );
    assert_eq!(
        made["cli_command"],
        "yulius remote login https://yulius.test"
    );
    let id = text(&made["view"]["id"]).to_owned();

    let listed = call(&w.h.router, Method::GET, "/api/me/tokens", None, &browser).await?;
    assert_eq!(listed.status, StatusCode::OK);
    let mine = listed.body.as_array().ok_or("list")?;
    assert_eq!(
        mine.len(),
        2,
        "the operate token and the CLI one: {}",
        listed.body
    );
    let entry = mine
        .iter()
        .find(|t| t["id"] == id.as_str())
        .ok_or("listed")?;
    assert_eq!(entry["scope"], "operate");
    assert_eq!(entry["label"], "laptop Claude Code");
    assert!(entry["last_used_at"].is_null());
    assert!(entry.get("token").is_none() && !listed.body.to_string().contains(&token));
    assert!(
        mine.iter().any(|t| t["scope"] == "cli"),
        "a yulius login token is cli: {}",
        listed.body
    );

    // Its use is recorded.
    let me = call(&w.h.router, Method::GET, "/api/me", None, &bearer(&token)).await?;
    assert_eq!(me.status, StatusCode::OK);
    let listed = call(&w.h.router, Method::GET, "/api/me/tokens", None, &browser).await?;
    let entry = listed
        .body
        .as_array()
        .and_then(|l| l.iter().find(|t| t["id"] == id.as_str()).cloned())
        .ok_or("listed")?;
    assert!(entry["last_used_at"].is_string(), "{entry}");

    // Nobody else revokes it; its owner does, from any of their credentials.
    let other = call(
        &w.h.router,
        Method::DELETE,
        &format!("/api/me/tokens/{id}"),
        None,
        &bob_cli,
    )
    .await?;
    assert_eq!(other.status, StatusCode::NOT_FOUND);
    let revoked = call(
        &w.h.router,
        Method::DELETE,
        &format!("/api/me/tokens/{id}"),
        None,
        &cli,
    )
    .await?;
    assert_eq!(revoked.status, StatusCode::NO_CONTENT, "{}", revoked.body);
    let gone = call(&w.h.router, Method::GET, "/api/me", None, &bearer(&token)).await?;
    assert_eq!(gone.status, StatusCode::UNAUTHORIZED);
    let again = call(
        &w.h.router,
        Method::DELETE,
        &format!("/api/me/tokens/{id}"),
        None,
        &cli,
    )
    .await?;
    assert_eq!(again.status, StatusCode::NOT_FOUND);

    let entries = audits(&w, &id).await?;
    let actions: Vec<&str> = entries.iter().map(|(a, _, _)| a.as_str()).collect();
    assert_eq!(actions, ["api_token.revoke", "api_token.create"]);
    assert!(entries.iter().all(|(_, actor, detail)| actor == "alice"
        && detail == "laptop Claude Code (operate)"
        && !detail.contains("yul_")));
    Ok(())
}

/// RMT-002: a read-only token reads the REST API and changes nothing through it.
#[tokio::test]
async fn a_read_token_only_reads_the_rest_api() -> TestResult {
    let w = World::new("org").await?;
    let (alice, _) = w.user("alice", false).await?;
    let browser = w.browser(&alice).await?;
    let made = make_token(&w, &browser, "dashboards", "read").await?;
    let read = bearer(text(&made["token"]));
    let me = call(&w.h.router, Method::GET, "/api/me", None, &read).await?;
    assert_eq!(me.status, StatusCode::OK);
    let write = call(
        &w.h.router,
        Method::POST,
        "/api/sandboxes",
        Some(json!({"mode": "import", "repo": "Quartio-Studio/shop"})),
        &read,
    )
    .await?;
    assert_eq!(write.status, StatusCode::FORBIDDEN, "{}", write.body);
    assert!(text(&write.body["error"]["message"]).contains("read-only"));
    Ok(())
}
