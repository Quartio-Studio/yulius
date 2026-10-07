//! GitHub App connection through the manifest flow (SPEC-001 ACC-006, journey J1 step 3).

mod support;

use axum::http::{Method, StatusCode};
use serde_json::json;
use support::{TestResult, admit_and_sign_in, call, harness, setup_superadmin_creds};

#[tokio::test]
async fn superadmin_connects_github_with_the_manifest_flow() -> TestResult {
    let h = harness()?;
    let admin = setup_superadmin_creds(&h).await?;

    let status = call(&h.router, Method::GET, "/api/admin/github", None, &admin).await?;
    assert_eq!(status.status, StatusCode::OK);
    assert_eq!(status.body["configured"], false);

    let bad = call(
        &h.router,
        Method::POST,
        "/api/admin/github/manifest",
        Some(json!({"owner_login": "bad/name", "owner_kind": "org"})),
        &admin,
    )
    .await?;
    assert_eq!(bad.status, StatusCode::UNPROCESSABLE_ENTITY);

    let prepared = call(
        &h.router,
        Method::POST,
        "/api/admin/github/manifest",
        Some(json!({"owner_login": "Quartio-Studio", "owner_kind": "org"})),
        &admin,
    )
    .await?;
    assert_eq!(prepared.status, StatusCode::OK, "{:?}", prepared.body);
    let post_url = prepared.body["post_url"].as_str().ok_or("post_url")?;
    let state = post_url.split("state=").nth(1).ok_or("state")?.to_owned();
    assert!(state.len() >= 32);
    assert!(prepared.body["manifest"].is_object());

    // A wrong code fails visibly and leaves the instance unconfigured.
    let failed = call(
        &h.router,
        Method::GET,
        &format!("/api/admin/github/manifest/callback?code=wrong&state={state}"),
        None,
        &admin,
    )
    .await?;
    assert!(failed.status.is_redirection());
    assert!(
        failed
            .location
            .as_deref()
            .is_some_and(|l| l.starts_with("/admin/github?error="))
    );

    // The state was consumed; start again and complete with the right code.
    let prepared = call(
        &h.router,
        Method::POST,
        "/api/admin/github/manifest",
        Some(json!({"owner_login": "Quartio-Studio", "owner_kind": "org"})),
        &admin,
    )
    .await?;
    let state = prepared.body["post_url"]
        .as_str()
        .and_then(|u| u.split("state=").nth(1))
        .ok_or("state")?
        .to_owned();
    let done = call(
        &h.router,
        Method::GET,
        &format!("/api/admin/github/manifest/callback?code=manifest-code&state={state}"),
        None,
        &admin,
    )
    .await?;
    assert!(done.status.is_redirection());
    assert_eq!(
        done.location.as_deref(),
        Some("https://github.com/apps/yulius-test-app/installations/new")
    );
    let status = call(&h.router, Method::GET, "/api/admin/github", None, &admin).await?;
    assert_eq!(status.body["configured"], true);
    assert_eq!(status.body["app_slug"], "yulius-test-app");
    assert_eq!(status.body["owner_login"], "Quartio-Studio");
    let me = call(&h.router, Method::GET, "/api/me", None, &admin).await?;
    assert_eq!(
        me.body["instance"]["github_configured"], true,
        "{:?}",
        me.body
    );

    // Replaying a used state is refused; a configured instance refuses a second manifest.
    let replay = call(
        &h.router,
        Method::GET,
        &format!("/api/admin/github/manifest/callback?code=manifest-code&state={state}"),
        None,
        &admin,
    )
    .await?;
    assert!(
        replay
            .location
            .as_deref()
            .is_some_and(|l| l.contains("error="))
    );
    let again = call(
        &h.router,
        Method::POST,
        "/api/admin/github/manifest",
        Some(json!({"owner_login": "Quartio-Studio", "owner_kind": "org"})),
        &admin,
    )
    .await?;
    assert_eq!(again.status, StatusCode::CONFLICT);

    // Only the superadmin manages the connection, even instance admins cannot.
    let alice = admit_and_sign_in(&h, &admin, "alice", 7, true).await?;
    let denied = call(&h.router, Method::GET, "/api/admin/github", None, &alice).await?;
    assert_eq!(denied.status, StatusCode::FORBIDDEN);
    Ok(())
}
