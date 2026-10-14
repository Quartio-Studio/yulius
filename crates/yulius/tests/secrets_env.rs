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
    seed_task(
        &h,
        "t1",
        "s1",
        "u-alice",
        TaskStatus::Running,
        Some(&worktree),
    )
    .await?;
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
        assert!(
            actions.iter().any(|a| a == expected),
            "{expected}: {actions:?}"
        );
    }
    let _ = &w.worktree;
    Ok(())
}

fn read(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap_or_default()
}

/// SPEC-129 R1, R3, R6, R7, AC1, AC4: an agent asks to copy a secret, which writes nothing and
/// waits for a person; once allowed, the same call writes the real value into the ignored `.env`
/// (mode 0600) and the name into `.env.example`, and the agent never sees the value.
#[tokio::test]
async fn an_allowed_secret_is_copied_into_the_ignored_env() -> TestResult {
    use std::os::unix::fs::PermissionsExt as _;
    use yulius_app::ports::store::TaskStore as _;
    let w = world().await?;
    let env = w.worktree.join(".env");
    std::fs::write(&env, "# local\nOTHER=1\nSTRIPE_KEY=[secret:STRIPE_KEY]\n")?;

    let asked = support::mcp_tool(
        &w.h,
        TOKEN,
        "secret_copy_to_env",
        json!({"secrets": ["STRIPE_KEY"], "reason": "the deployed checkout charges cards"}),
    )
    .await?;
    assert_ne!(asked["isError"], true, "{asked}");
    let text = support::tool_text(&asked);
    assert_eq!(text["copied"], json!([]), "{text}");
    assert_eq!(text["waiting"], json!(["STRIPE_KEY"]), "{text}");
    assert!(
        text["link"]
            .as_str()
            .is_some_and(|l| l.ends_with("/s/app/secrets?section=agents")),
        "{text}"
    );
    assert_eq!(
        read(&env),
        "# local\nOTHER=1\nSTRIPE_KEY=[secret:STRIPE_KEY]\n",
        "nothing is written before a person allows it"
    );
    assert!(!w.worktree.join(".env.example").exists());
    let secret = w.h.store.vault_get(&w.id).await?.ok_or("secret")?;
    assert_eq!(secret.env_files, EnvFiles::Asked);
    let task =
        w.h.store
            .task(&yulius_domain::ids::TaskId::parse("t1")?)
            .await?
            .ok_or("task")?;
    assert_eq!(secret.env_requested_by, Some(format!("#{}", task.number)));
    assert_eq!(
        task.activity,
        Some(yulius_domain::task::Activity::NeedsAttention)
    );

    let allowed = answer(&w, &w.writer, true).await?;
    assert_eq!(allowed.status, StatusCode::OK, "{}", allowed.body);
    let copied = support::mcp_tool(
        &w.h,
        TOKEN,
        "secret_copy_to_env",
        json!({"secrets": ["STRIPE_KEY"]}),
    )
    .await?;
    assert_ne!(copied["isError"], true, "{copied}");
    assert!(!copied.to_string().contains(VALUE), "{copied}");
    let text = support::tool_text(&copied);
    assert_eq!(text["copied"], json!(["STRIPE_KEY"]), "{text}");
    assert_eq!(text["file"], ".env");
    assert_eq!(text["example_added"], json!(["STRIPE_KEY"]), "{text}");
    assert_eq!(
        read(&env),
        format!("# local\nOTHER=1\nSTRIPE_KEY={VALUE}\n"),
        "the masked line is replaced by the real value, the rest is kept"
    );
    assert_eq!(std::fs::metadata(&env)?.permissions().mode() & 0o777, 0o600);
    assert_eq!(read(&w.worktree.join(".env.example")), "STRIPE_KEY=\n");

    // In a folder of its own, with an example that already names it.
    std::fs::create_dir_all(w.worktree.join("deploy"))?;
    std::fs::write(
        w.worktree.join("deploy/.env.example"),
        "# keys\nSTRIPE_KEY=\n",
    )?;
    let nested = support::mcp_tool(
        &w.h,
        TOKEN,
        "secret_copy_to_env",
        json!({"secrets": ["STRIPE_KEY"], "path": "deploy/.env.production"}),
    )
    .await?;
    assert_ne!(nested["isError"], true, "{nested}");
    assert_eq!(support::tool_text(&nested)["example_added"], json!([]));
    assert_eq!(
        read(&w.worktree.join("deploy/.env.production")),
        format!("STRIPE_KEY={VALUE}\n")
    );
    assert_eq!(
        read(&w.worktree.join("deploy/.env.example")),
        "# keys\nSTRIPE_KEY=\n"
    );

    let copies = activity(&w)
        .await?
        .iter()
        .filter(|e| e["action"] == "vault.agent.env")
        .count();
    assert_eq!(copies, 2, "one entry per file");
    let requests = activity(&w)
        .await?
        .iter()
        .filter(|e| e["action"] == "vault.agent.env_request")
        .count();
    assert_eq!(requests, 1);
    Ok(())
}

/// SPEC-129 R2, AC2: nothing is written where Git could commit it, outside the worktree, into a
/// template or through a link; unknown secrets and unallowed ones are not copied either.
#[tokio::test]
async fn copies_are_refused_where_git_could_commit_them() -> TestResult {
    let w = world().await?;
    let outside = tempfile::tempdir()?;
    let outside_file = outside.path().join("stolen.env");
    std::fs::write(&outside_file, "")?;
    std::os::unix::fs::symlink(&outside_file, w.worktree.join(".env.local"))?;
    std::fs::write(w.worktree.join("tracked.env"), "")?;
    std::fs::write(
        w.worktree.join(".gitignore"),
        ".env\n.env.*\n!.env.example\n*.env\n",
    )?;
    git(&w.worktree, &["add", "-f", "tracked.env"])?;
    git(&w.worktree, &["commit", "-q", "-m", "a tracked env file"])?;
    std::fs::write(
        w.worktree.join(".gitignore"),
        ".env\n.env.*\n!.env.example\n",
    )?;
    std::fs::create_dir_all(w.worktree.join("docs"))?;

    // Unallowed: an unknown name, or more than 16.
    let many: Vec<String> = (0..17).map(|i| format!("K{i}")).collect();
    for args in [
        json!({"secrets": ["NOPE"]}),
        json!({"secrets": []}),
        json!({"secrets": many}),
    ] {
        let refused = support::mcp_tool(&w.h, TOKEN, "secret_copy_to_env", args.clone()).await?;
        assert_eq!(refused["isError"], true, "{args}: {refused}");
    }
    w.h.store
        .vault_set_env_files(&w.id, (EnvFiles::No, None), (EnvFiles::Allowed, None))
        .await?;
    for path in [
        "README.md",
        ".env.example",
        "../escape.env",
        "/tmp/absolute.env",
        ".git/x.env",
        "missing/.env",
        ".env.local",
        "tracked.env",
        "prod.env",
        "docs/prod.env",
    ] {
        let refused = support::mcp_tool(
            &w.h,
            TOKEN,
            "secret_copy_to_env",
            json!({"secrets": ["STRIPE_KEY"], "path": path}),
        )
        .await?;
        assert_eq!(refused["isError"], true, "{path}: {refused}");
    }
    assert_eq!(read(&outside_file), "", "nothing reached the link's target");
    assert_eq!(read(&w.worktree.join("tracked.env")), "");
    assert!(!w.worktree.join("prod.env").exists());
    assert!(!w.worktree.join(".env.example").exists());
    assert!(
        !w.h.app
            .config
            .data_dir
            .join("sandboxes/s1/worktrees/escape.env")
            .exists()
    );
    let refused = support::mcp_tool(
        &w.h,
        TOKEN,
        "secret_copy_to_env",
        json!({"secrets": ["STRIPE_KEY"], "path": "prod.env"}),
    )
    .await?;
    assert!(
        refused.to_string().contains(".gitignore"),
        "the refusal says how to fix it: {refused}"
    );
    Ok(())
}

/// SPEC-129 R8, AC6: `secrets_list` says whether each secret may be copied into `.env` files and
/// that keys live in the git-ignored `.env`, copied with `secret_copy_to_env`.
#[tokio::test]
async fn secrets_list_teaches_the_env_default() -> TestResult {
    let w = world().await?;
    let listed =
        support::tool_text(&support::mcp_tool(&w.h, TOKEN, "secrets_list", json!({})).await?);
    assert_eq!(listed["secrets"][0]["env_files"], "no", "{listed}");
    let advice = listed["how_to_use"].to_string();
    for needle in [".env.example", "secret_copy_to_env", "git-ignored .env"] {
        assert!(advice.contains(needle), "{needle}: {advice}");
    }
    Ok(())
}
