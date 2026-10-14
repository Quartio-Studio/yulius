//! Playgrounds (SPEC-099): a team tries Yulius on its own code without GitHub. A playground's Git
//! lives in a bare `origin.git` in its sandbox directory; nothing reaches GitHub. Real Git, a real
//! store, the fake GitHub (which records every state-changing call) and no GitHub App.

use crate::support;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::http::{Method, StatusCode};
use serde_json::{Value, json};
use support::repos::git;
use support::{Creds, Harness, HarnessOptions, TestResult, call, harness_with};
use yulius::wiring::git::GitAdapter;

type BoxResult<T> = Result<T, Box<dyn std::error::Error>>;

/// A harness with the real Git adapter and no GitHub App, and the superadmin signed in.
async fn world() -> BoxResult<(Harness, Creds)> {
    let h = harness_with(HarnessOptions {
        git: Some(Arc::new(GitAdapter::new("git"))),
        ..HarnessOptions::default()
    })?;
    let owner = support::setup_superadmin_creds(&h).await?;
    Ok((h, owner))
}

async fn from_template(h: &Harness, creds: &Creds, name: &str) -> BoxResult<support::Resp> {
    call(
        &h.router,
        Method::POST,
        "/api/sandboxes",
        Some(json!({"mode": "playground", "name": name, "template": "empty"})),
        creds,
    )
    .await
}

fn sandbox_dir(h: &Harness, body: &Value) -> PathBuf {
    h.app
        .config
        .data_dir
        .join("sandboxes")
        .join(body["id"].as_str().unwrap_or_default())
}

/// Branch names of a repository (`refs/heads/*`), sorted.
fn branches(repo: &Path) -> BoxResult<Vec<String>> {
    let out = git(
        repo,
        &["for-each-ref", "--format=%(refname:short)", "refs/heads"],
    )?;
    let mut names: Vec<String> = out.lines().map(str::to_owned).collect();
    names.sort();
    Ok(names)
}

fn no_github_writes(h: &Harness) -> BoxResult<()> {
    let writes = h.github.writes.lock().map_err(|_| "lock")?.clone();
    assert!(writes.is_empty(), "GitHub was written to: {writes:?}");
    Ok(())
}

/// PLY-001, PLY-002: without any GitHub App, a template becomes a playground whose Git is a bare
/// repository in its sandbox directory, with `main` and `dev`; tasks may start at once.
#[tokio::test]
async fn a_playground_starts_from_a_template_without_github() -> TestResult {
    let (h, owner) = world().await?;
    let made = from_template(&h, &owner, "My Shop").await?;
    assert_eq!(made.status, StatusCode::CREATED, "{:?}", made.body);
    let body = &made.body;
    assert_eq!(body["slug"], "my-shop");
    assert_eq!(body["name"], "my-shop");
    assert_eq!(body["playground"], true);
    assert_eq!(body["repo"]["owner"], "");
    assert_eq!(body["repo"]["url"], "");
    assert_eq!(body["access"], "admin");
    assert_eq!(body["onboarding"], "not_needed");
    assert_eq!(body["default_branch"], "main");

    let dir = sandbox_dir(&h, body);
    let origin = dir.join("origin.git");
    assert_eq!(
        git(&origin, &["rev-parse", "--is-bare-repository"])?.trim(),
        "true"
    );
    assert_eq!(branches(&origin)?, vec!["dev", "main"]);
    let repo = dir.join("repo");
    assert_eq!(
        git(&repo, &["remote", "get-url", "origin"])?.trim(),
        origin.to_string_lossy(),
        "the clone's origin is the playground's own bare repository"
    );
    git(&repo, &["rev-parse", "--verify", "origin/dev"])?;
    assert!(
        repo.join("AGENTS.md").exists(),
        "the template's scaffold is checked out"
    );

    // The same name again: a second playground, its own slug.
    let again = from_template(&h, &owner, "My Shop").await?;
    assert_eq!(again.status, StatusCode::CREATED, "{:?}", again.body);
    assert_eq!(again.body["slug"], "my-shop-2");

    // Unknown templates and names that leave nothing are refused.
    let bad = call(
        &h.router,
        Method::POST,
        "/api/sandboxes",
        Some(json!({"mode": "playground", "name": "x", "template": "nope"})),
        &owner,
    )
    .await?;
    assert_eq!(
        bad.status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "{:?}",
        bad.body
    );
    no_github_writes(&h)?;
    Ok(())
}

/// PLY-005: a playground is its maker's and the superadmin's; nobody else sees it.
#[tokio::test]
async fn only_its_maker_and_the_superadmin_see_a_playground() -> TestResult {
    let (h, owner) = world().await?;
    support::configure_github_app(&h).await?;
    let alice = support::seed_github_user(&h, "u-alice", "alice").await?;
    let bob = support::seed_github_user(&h, "u-bob", "bob").await?;
    let made = from_template(&h, &alice, "alices-lab").await?;
    assert_eq!(made.status, StatusCode::CREATED, "{:?}", made.body);
    assert_eq!(made.body["access"], "admin");
    let path = format!("/api/sandboxes/{}", made.body["id"].as_str().ok_or("id")?);

    let seen = |list: &Value| {
        list.as_array()
            .is_some_and(|items| items.iter().any(|s| s["slug"] == "alices-lab"))
    };
    let mine = call(&h.router, Method::GET, "/api/sandboxes", None, &alice).await?;
    assert!(seen(&mine.body), "{:?}", mine.body);
    let theirs = call(&h.router, Method::GET, "/api/sandboxes", None, &bob).await?;
    assert!(!seen(&theirs.body), "{:?}", theirs.body);
    let direct = call(&h.router, Method::GET, &path, None, &bob).await?;
    assert_eq!(direct.status, StatusCode::NOT_FOUND);
    let all = call(&h.router, Method::GET, "/api/sandboxes", None, &owner).await?;
    assert!(seen(&all.body), "the superadmin sees every sandbox");
    no_github_writes(&h)?;
    Ok(())
}
