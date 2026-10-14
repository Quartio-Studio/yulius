//! Agent secrets copied into a project's git-ignored `.env` (SPEC-129, ADR-0105): a person
//! allows each secret once; `secret_copy_to_env` then writes the real value into an ignored dotenv
//! file of the task's worktree, and the name into `.env.example`, without the agent seeing it.

use crate::support;

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

use axum::http::{Method, StatusCode};
use serde_json::{Value, json};
use support::{
    Creds, TestResult, call, harness_with_git, perms, seed_github_user, seed_live_attempt,
    seed_sandbox, seed_task,
};
use yulius::wiring::git::GitAdapter;
use yulius_app::ports::vault::{EnvFiles, VaultStore as _};
use yulius_domain::task::TaskStatus;

const VALUE: &str = "sk_live_51EnvCopyValue9q2w";
const TOKEN: &str = "env-task-token-s1-0123456789abcdef";

fn git(dir: &Path, args: &[&str]) -> TestResult {
    let out = Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@example.com")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@example.com")
        .output()?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).into_owned().into());
    }
    Ok(())
}

struct World {
    h: support::Harness,
    writer: Creds,
    reader: Creds,
    worktree: PathBuf,
    id: String,
}

/// Sandbox s1 (`app`): alice writes it, bob reads it; task t1 runs with a live MCP token in a
/// real repository inside the sandbox directory whose `.gitignore` ignores `.env` and `.env.*`
/// but keeps `.env.example`; the agent secret `STRIPE_KEY` applies to s1.
async fn world() -> Result<World, Box<dyn std::error::Error>> {
    let h = harness_with_git(Arc::new(GitAdapter::new("git")))?;
    let writer = seed_github_user(&h, "u-alice", "alice").await?;
    let reader = seed_github_user(&h, "u-bob", "bob").await?;
    seed_sandbox(&h, "s1", "app", "u-alice").await?;
    h.github.set_perm("alice", "acme/app", perms("write"));
    h.github.set_perm("bob", "acme/app", perms("read"));
    let worktree = h.app.config.data_dir.join("sandboxes/s1/worktrees/t1");
    std::fs::create_dir_all(&worktree)?;
    git(&worktree, &["init", "-q", "-b", "dev"])?;
    std::fs::write(worktree.join(".gitignore"), ".env\n.env.*\n!.env.example\n")?;
    std::fs::write(worktree.join("README.md"), "# app\n")?;
    git(&worktree, &["add", "."])?;
    git(&worktree, &["commit", "-q", "-m", "init"])?;
    seed_task(&h, "t1", "s1", "u-alice", TaskStatus::Running, Some(&worktree)).await?;
    seed_live_attempt(&h, "t1", TOKEN).await?;
    let created = call(
        &h.router,
        Method::POST,
        "/api/sandboxes/s1/agent-secrets",
        Some(json!({"name": "STRIPE_KEY", "description": "Stripe live key", "value": VALUE})),
        &writer,
    )
    .await?;
    assert_eq!(created.status, StatusCode::CREATED, "{}", created.body);
    let id = created.body["id"].as_str().ok_or("id")?.to_owned();
    Ok(World {
        h,
        writer,
        reader,
        worktree,
        id,
    })
}

async fn answer(
    w: &World,
    creds: &Creds,
    allow: bool,
) -> Result<support::Resp, Box<dyn std::error::Error>> {
    call(
        &w.h.router,
        Method::POST,
        &format!("/api/agent-secrets/{}/env", w.id),
        Some(json!({"allow": allow})),
        creds,
    )
    .await
}

async fn activity(w: &World) -> Result<Vec<Value>, Box<dyn std::error::Error>> {
    let got = call(
        &w.h.router,
        Method::GET,
        "/api/secrets/activity",
        None,
        &w.writer,
    )
    .await?;
    Ok(got.body.as_array().cloned().unwrap_or_default())
}

/// SPEC-129 R4, AC5: a person who manages the secret allows, dismisses or revokes copying it
/// into `.env` files; readers cannot; there must be something to answer.
#[tokio::test]
async fn people_allow_dismiss_and_revoke_env_copies() -> TestResult {
    let w = world().await?;
    let nothing = answer(&w, &w.writer, true).await?;
    assert_eq!(nothing.status, StatusCode::CONFLICT, "nobody asked yet");

    let ask = |by: &'static str| {
        let store = w.h.store.clone();
        let id = w.id.clone();
        async move {
            store
                .vault_set_env_files(&id, (EnvFiles::No, None), (EnvFiles::Asked, Some(by)))
                .await
        }
    };
    assert!(ask("#1").await?);
    let refused = answer(&w, &w.reader, true).await?;
    assert_eq!(refused.status, StatusCode::FORBIDDEN, "{}", refused.body);
    let dismissed = answer(&w, &w.writer, false).await?;
    assert_eq!(dismissed.status, StatusCode::OK, "{}", dismissed.body);
    assert_eq!(dismissed.body["env_files"], "no");
    assert_eq!(dismissed.body["env_requested_by"], Value::Null);

    assert!(ask("#1").await?);
    let allowed = answer(&w, &w.writer, true).await?;
    assert_eq!(allowed.status, StatusCode::OK, "{}", allowed.body);
    assert_eq!(allowed.body["env_files"], "allowed");
    assert_eq!(
        answer(&w, &w.writer, true).await?.body["env_files"],
        "allowed",
        "allowing again changes nothing"
    );
    let revoked = answer(&w, &w.writer, false).await?;
    assert_eq!(revoked.body["env_files"], "no");

    let actions: Vec<String> = activity(&w)
        .await?
        .iter()
        .filter_map(|e| e["action"].as_str().map(str::to_owned))
        .filter(|a| a.starts_with("vault.agent.env"))
        .collect();
    for expected in [
        "vault.agent.env_dismiss",
        "vault.agent.env_allow",
        "vault.agent.env_revoke",
    ] {
        assert!(actions.iter().any(|a| a == expected), "{expected}: {actions:?}");
    }
    let _ = &w.worktree;
    Ok(())
}
