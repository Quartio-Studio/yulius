//! Agent secrets an agent creates from a file (SPEC-211, ADR-0167): Yulius reads a file in the
//! task's worktree or home and stores it as an agent secret of the sandbox, without the value
//! reaching the agent; a person still allows every use.

use crate::support;

use std::path::PathBuf;
use std::sync::Arc;

use axum::http::{Method, StatusCode};
use serde_json::{Value, json};
use support::{
    Creds, TestResult, call, harness_with_git, perms, seed_github_user, seed_live_attempt,
    seed_sandbox, seed_task,
};
use yulius::wiring::git::GitAdapter;
use yulius_app::ports::store::TaskStore as _;
use yulius_app::ports::vault::{EnvFiles, VaultStore as _};
use yulius_domain::ids::SandboxId;
use yulius_domain::task::TaskStatus;

const TOKEN: &str = "create-task-token-s1-0123456789abcdef";
const READER_TOKEN: &str = "create-task-token-s1-reader-0123456789";
const API_TOKEN: &str = "tok_synthetic_created_from_a_file_4821";
const EXISTING: &str = "sk_live_51ExistingValue9q2w";

struct World {
    h: support::Harness,
    writer: Creds,
    worktree: PathBuf,
    home: PathBuf,
}

/// Sandbox s1 (`app`): alice writes it, bob reads it. Task t1 (alice's) runs in a Git worktree
/// that ignores `.env`; task t2 is bob's. The agent secret `STRIPE_KEY` already applies to s1.
async fn world() -> Result<World, Box<dyn std::error::Error>> {
    let h = harness_with_git(Arc::new(GitAdapter::new("git")))?;
    let writer = seed_github_user(&h, "u-alice", "alice").await?;
    seed_github_user(&h, "u-bob", "bob").await?;
    seed_sandbox(&h, "s1", "app", "u-alice").await?;
    h.github.set_perm("alice", "acme/app", perms("write"));
    h.github.set_perm("bob", "acme/app", perms("read"));
    let worktree = h.app.config.data_dir.join("sandboxes/s1/worktrees/t1");
    std::fs::create_dir_all(&worktree)?;
    git(&worktree, &["init", "-q", "-b", "dev"])?;
    std::fs::write(worktree.join(".gitignore"), ".env\n.env.*\n!.env.example\n")?;
    git(&worktree, &["add", "."])?;
    git(&worktree, &["commit", "-q", "-m", "init"])?;
    let home = h.app.config.data_dir.join("sandboxes/s1/home");
    std::fs::create_dir_all(home.join(".ssh"))?;
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
    let other = h.app.config.data_dir.join("sandboxes/s1/worktrees/t2");
    std::fs::create_dir_all(&other)?;
    seed_task(&h, "t2", "s1", "u-bob", TaskStatus::Running, Some(&other)).await?;
    seed_live_attempt(&h, "t2", READER_TOKEN).await?;
    let created = call(
        &h.router,
        Method::POST,
        "/api/sandboxes/s1/agent-secrets",
        Some(json!({"name": "STRIPE_KEY", "value": EXISTING})),
        &writer,
    )
    .await?;
    assert_eq!(created.status, StatusCode::CREATED, "{}", created.body);
    Ok(World {
        h,
        writer,
        worktree,
        home,
    })
}

fn git(dir: &std::path::Path, args: &[&str]) -> TestResult {
    let out = std::process::Command::new("git")
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

fn sid() -> Result<SandboxId, Box<dyn std::error::Error>> {
    Ok(SandboxId::parse("s1")?)
}

async fn create(w: &World, token: &str, args: Value) -> Result<Value, Box<dyn std::error::Error>> {
    support::mcp_tool(&w.h, token, "secret_create_from_file", args).await
}

/// The agent secrets of s1, by name.
async fn names(w: &World) -> Result<Vec<String>, Box<dyn std::error::Error>> {
    Ok(w.h
        .store
        .vault_agent(Some(&sid()?))
        .await?
        .into_iter()
        .map(|r| r.name)
        .collect())
}

/// Allows `.env` copies of `name` as alice.
async fn allow_env(w: &World, name: &str) -> TestResult {
    let record =
        w.h.store
            .vault_agent(Some(&sid()?))
            .await?
            .into_iter()
            .find(|r| r.name == name)
            .ok_or("secret")?;
    let allowed = call(
        &w.h.router,
        Method::POST,
        &format!("/api/agent-secrets/{}/env", record.id),
        Some(json!({"allow": true, "env_files": record.env_files.as_str(), "env_requested_by": record.env_requested_by})),
        &w.writer,
    )
    .await?;
    assert_eq!(allowed.status, StatusCode::OK, "{}", allowed.body);
    Ok(())
}

/// SPEC-211 SCF-002..SCF-006 (AC-2, AC-6, AC-8): a task stores a key in its home and a token in
/// its worktree as agent secrets of its sandbox; neither value reaches the agent, both are stored
/// exactly (a person allows a `.env` copy, which `sh` reads back), each starts with no hosts and
/// no `.env` allowance, created by the task's person and audited as the task; the file is deleted
/// only when asked; from then on a result holding the value shows `[secret:NAME]`.
#[tokio::test]
async fn a_task_stores_files_as_agent_secrets_without_seeing_them() -> TestResult {
    let w = world().await?;
    let key = crate::secrets_env::openssh_key(7);
    std::fs::write(w.home.join(".ssh/deploy_key"), &key)?;
    std::fs::create_dir_all(w.worktree.join("secrets"))?;
    std::fs::write(
        w.worktree.join("secrets/token.txt"),
        format!("{API_TOKEN}\n"),
    )?;

    let from_home = create(
        &w,
        TOKEN,
        json!({"name": "DEPLOY_SSH_KEY", "file": "~/.ssh/deploy_key", "description": "deploy key for the droplet"}),
    )
    .await?;
    assert_ne!(from_home["isError"], true, "{from_home}");
    let text = support::tool_text(&from_home);
    assert_eq!(text["created"], "DEPLOY_SSH_KEY", "{text}");
    assert_eq!(text["file"], "~/.ssh/deploy_key", "{text}");
    assert_eq!(text["deleted"], false, "{text}");
    assert!(
        text["next"]
            .as_str()
            .is_some_and(|n| n.contains("secret_request_hosts") && n.contains("still holds")),
        "{text}"
    );
    assert!(w.home.join(".ssh/deploy_key").exists(), "kept unless asked");

    let from_worktree = create(
        &w,
        TOKEN,
        json!({"name": "API_TOKEN", "file": "secrets/token.txt", "delete_file": true}),
    )
    .await?;
    assert_ne!(from_worktree["isError"], true, "{from_worktree}");
    let text = support::tool_text(&from_worktree);
    assert_eq!(text["file"], "secrets/token.txt", "{text}");
    assert_eq!(text["deleted"], true, "{text}");
    assert!(!w.worktree.join("secrets/token.txt").exists(), "deleted");
    for result in [&from_home, &from_worktree] {
        let raw = result.to_string();
        assert!(!raw.contains(API_TOKEN), "{raw}");
        for line in key.lines().skip(1).take(5) {
            assert!(!raw.contains(line), "{raw}");
        }
    }

    let task =
        w.h.store
            .task(&yulius_domain::ids::TaskId::parse("t1")?)
            .await?
            .ok_or("task")?;
    let records = w.h.store.vault_agent(Some(&sid()?)).await?;
    for name in ["DEPLOY_SSH_KEY", "API_TOKEN"] {
        let record = records.iter().find(|r| r.name == name).ok_or(name)?;
        assert_eq!(record.sandbox_id, Some(sid()?), "{name}");
        assert!(record.hosts.is_empty(), "{name}");
        assert_eq!(record.injection, "none", "{name}");
        assert_eq!(record.env_files, EnvFiles::No, "{name}");
        assert!(!record.cd_prod, "{name}");
        assert_eq!(record.created_by, "u-alice", "{name}");
    }
    let activity = call(
        &w.h.router,
        Method::GET,
        "/api/secrets/activity",
        None,
        &w.writer,
    )
    .await?;
    let created: Vec<&Value> = activity
        .body
        .as_array()
        .ok_or("activity")?
        .iter()
        .filter(|e| e["action"] == "vault.agent.create" && e["actor"] != "alice")
        .collect();
    assert_eq!(created.len(), 2, "{created:?}");
    for entry in &created {
        assert_eq!(entry["actor"], format!("task:{}", task.number), "{entry}");
        let detail = entry["detail"].as_str().unwrap_or_default();
        assert!(
            detail.ends_with(&format!("from a file, by #{}", task.number)),
            "{detail}"
        );
    }
    assert!(!activity.body.to_string().contains(API_TOKEN));

    // The values are the files' (a person allows a .env copy; `sh` reads it back).
    let copy = || {
        support::mcp_tool(
            &w.h,
            TOKEN,
            "secret_copy_to_env",
            json!({"secrets": ["DEPLOY_SSH_KEY", "API_TOKEN"]}),
        )
    };
    let asked = copy().await?;
    assert_eq!(
        support::tool_text(&asked)["waiting"],
        json!(["DEPLOY_SSH_KEY", "API_TOKEN"]),
        "{asked}"
    );
    allow_env(&w, "DEPLOY_SSH_KEY").await?;
    allow_env(&w, "API_TOKEN").await?;
    let copied = copy().await?;
    assert_ne!(copied["isError"], true, "{copied}");
    assert_eq!(
        crate::secrets_env::sourced(&w.worktree, "DEPLOY_SSH_KEY")?,
        key
    );
    assert_eq!(
        crate::secrets_env::sourced(&w.worktree, "API_TOKEN")?,
        API_TOKEN
    );

    // Redacted from then on (AC-8).
    let echoed = support::mcp_tool(
        &w.h,
        TOKEN,
        "steps_set",
        json!({"steps": [format!("Call the API with {API_TOKEN}")]}),
    )
    .await?;
    let raw = echoed.to_string();
    assert!(!raw.contains(API_TOKEN), "{raw}");
    assert!(raw.contains("[secret:API_TOKEN]"), "{raw}");
    Ok(())
}

/// SPEC-211 SCF-002, SCF-003 (AC-3): a link (the file or a folder on the way), a FIFO, a file
/// too large, binary or spanning lines without being a key, an absolute path, `..` and a missing
/// file are refused without quoting content, and nothing is created.
#[tokio::test]
async fn files_yulius_must_not_read_are_refused() -> TestResult {
    let w = world().await?;
    let outside = w.h.app.config.data_dir.join("outside.txt");
    std::fs::write(&outside, "outside-synthetic-value-0001\n")?;
    std::os::unix::fs::symlink(&outside, w.worktree.join("link.txt"))?;
    std::os::unix::fs::symlink(w.h.app.config.data_dir.clone(), w.worktree.join("data"))?;
    std::os::unix::fs::symlink(w.h.app.config.data_dir.clone(), w.home.join("data"))?;
    nix::unistd::mkfifo(
        &w.worktree.join("fifo"),
        nix::sys::stat::Mode::from_bits_truncate(0o600),
    )?;
    std::fs::write(w.worktree.join("big.txt"), "x".repeat(16 * 1024 + 1))?;
    std::fs::write(w.worktree.join("binary"), [0xff_u8, 0xfe, 0x00, 0x41])?;
    std::fs::write(
        w.worktree.join("notes.txt"),
        "multi-line-synthetic-secret\nsecond line\n",
    )?;
    let before = names(&w).await?;
    for file in [
        "link.txt",
        "data/outside.txt",
        "~/data/outside.txt",
        "fifo",
        "big.txt",
        "binary",
        "notes.txt",
        "missing.txt",
        "/etc/hostname",
        "../t2/anything",
        "~root/.ssh/id",
        ".git/config",
    ] {
        let refused = create(&w, TOKEN, json!({"name": "SOME_KEY", "file": file})).await?;
        assert_eq!(refused["isError"], true, "{file}: {refused}");
        let raw = refused.to_string();
        for hidden in ["outside-synthetic-value", "multi-line-synthetic-secret"] {
            assert!(!raw.contains(hidden), "{file}: {raw}");
        }
    }
    assert_eq!(names(&w).await?, before, "nothing is created");
    assert!(outside.exists());
    Ok(())
}

/// SPEC-211 SCF-004 (AC-4, AC-5): an existing name (of this sandbox or of every sandbox) is never
/// replaced, an invalid name is refused, and a task whose person only reads the sandbox cannot
/// add a secret.
#[tokio::test]
async fn names_in_use_and_readers_are_refused() -> TestResult {
    let w = world().await?;
    support::setup_superadmin_creds(&w.h).await?;
    let admin = support::superadmin_bearer(&w.h).await?;
    let shared = call(
        &w.h.router,
        Method::POST,
        "/api/sandboxes/s1/agent-secrets",
        Some(json!({"scope": "instance", "name": "SHARED_KEY", "value": "shared-key-123456"})),
        &admin,
    )
    .await?;
    assert_eq!(shared.status, StatusCode::CREATED, "{}", shared.body);
    std::fs::write(w.worktree.join("key.txt"), "replacement-synthetic-9876\n")?;
    let stored = |name: &'static str| {
        let w = &w;
        async move {
            let records = w.h.store.vault_agent(Some(&sid()?)).await?;
            let record = records.into_iter().find(|r| r.name == name).ok_or(name)?;
            Ok::<_, Box<dyn std::error::Error>>((record.id, record.value_updated_at))
        }
    };
    let (stripe, shared_value) = (stored("STRIPE_KEY").await?, stored("SHARED_KEY").await?);
    for name in ["STRIPE_KEY", "SHARED_KEY"] {
        let refused = create(&w, TOKEN, json!({"name": name, "file": "key.txt"})).await?;
        assert_eq!(refused["isError"], true, "{name}: {refused}");
        assert!(
            refused.to_string().contains("already applies"),
            "{name}: {refused}"
        );
    }
    assert_eq!(stored("STRIPE_KEY").await?, stripe, "never replaced");
    assert_eq!(stored("SHARED_KEY").await?, shared_value, "never replaced");
    for name in ["lower_case", "1STARTS_WITH_DIGIT", ""] {
        let refused = create(&w, TOKEN, json!({"name": name, "file": "key.txt"})).await?;
        assert_eq!(refused["isError"], true, "{name}: {refused}");
    }

    let other = w.h.app.config.data_dir.join("sandboxes/s1/worktrees/t2");
    std::fs::write(other.join("key.txt"), "reader-synthetic-value-5555\n")?;
    let before = names(&w).await?;
    let refused = create(
        &w,
        READER_TOKEN,
        json!({"name": "READER_KEY", "file": "key.txt"}),
    )
    .await?;
    assert_eq!(refused["isError"], true, "{refused}");
    assert!(refused.to_string().contains("write access"), "{refused}");
    assert_eq!(names(&w).await?, before, "nothing is created");
    assert!(other.join("key.txt").exists());
    Ok(())
}
