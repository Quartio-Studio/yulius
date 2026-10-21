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

/// Answers as a person who sees the secret as it is now.
async fn answer(
    w: &World,
    creds: &Creds,
    allow: bool,
) -> Result<support::Resp, Box<dyn std::error::Error>> {
    let now = w.h.store.vault_get(&w.id).await?.ok_or("secret")?;
    answer_seen(
        w,
        creds,
        &w.id,
        allow,
        now.env_files.as_str(),
        now.env_requested_by.as_deref(),
    )
    .await
}

/// Answers as a person who saw `env_files` and `by`.
async fn answer_seen(
    w: &World,
    creds: &Creds,
    id: &str,
    allow: bool,
    env_files: &str,
    by: Option<&str>,
) -> Result<support::Resp, Box<dyn std::error::Error>> {
    call(
        &w.h.router,
        Method::POST,
        &format!("/api/agent-secrets/{id}/env"),
        Some(json!({"allow": allow, "env_files": env_files, "env_requested_by": by})),
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

    // Review of SPEC-129, L1: an answer names what the person saw. Another manager dismissed
    // and another task asked meanwhile: the stale Allow is refused and allows nothing.
    assert!(ask("#2").await?);
    let stale = answer_seen(&w, &w.writer, &w.id, true, "asked", Some("#1")).await?;
    assert_eq!(stale.status, StatusCode::CONFLICT, "{}", stale.body);
    let stale = answer_seen(&w, &w.writer, &w.id, false, "allowed", None).await?;
    assert_eq!(stale.status, StatusCode::CONFLICT, "{}", stale.body);
    assert_eq!(
        w.h.store.vault_get(&w.id).await?.ok_or("secret")?.env_files,
        EnvFiles::Asked
    );
    let unknown = answer_seen(&w, &w.writer, &w.id, true, "maybe", Some("#2")).await?;
    assert_eq!(
        unknown.status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "{}",
        unknown.body
    );
    // Someone without access to the sandbox does not learn the secret exists.
    let outsider = seed_github_user(&w.h, "u-carol", "carol").await?;
    let hidden = answer(&w, &outsider, true).await?;
    assert_eq!(hidden.status, StatusCode::NOT_FOUND, "{}", hidden.body);

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
    assert_eq!(
        task_activity(&w).await?,
        Some(yulius_domain::task::Activity::Working),
        "SPEC-213 SRA-001: nothing waits for a person any more"
    );
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

async fn task_activity(
    w: &World,
) -> Result<Option<yulius_domain::task::Activity>, Box<dyn std::error::Error>> {
    use yulius_app::ports::store::TaskStore as _;
    let task =
        w.h.store
            .task(&yulius_domain::ids::TaskId::parse("t1")?)
            .await?
            .ok_or("task")?;
    Ok(task.activity)
}

/// SPEC-213 SRA-001 to SRA-003 (#633): the answer to the last request a task waits for ends the
/// "Needs your action" its request raised; a task with a request still waiting, or flagged by
/// something else, keeps it.
#[tokio::test]
async fn the_answer_to_the_last_waiting_request_ends_the_flag() -> TestResult {
    use yulius_app::ports::store::TaskStore as _;
    use yulius_domain::task::Activity;
    let w = world().await?;
    let created = call(
        &w.h.router,
        Method::POST,
        "/api/sandboxes/s1/agent-secrets",
        Some(json!({"name": "MAPS_KEY", "value": "maps-value"})),
        &w.writer,
    )
    .await?;
    assert_eq!(created.status, StatusCode::CREATED, "{}", created.body);
    let maps = created.body["id"].as_str().ok_or("id")?.to_owned();
    let ask = |names: Value| {
        support::mcp_tool(&w.h, TOKEN, "secret_copy_to_env", json!({"secrets": names}))
    };
    let asked = ask(json!(["STRIPE_KEY", "MAPS_KEY"])).await?;
    assert_eq!(
        support::tool_text(&asked)["waiting"],
        json!(["STRIPE_KEY", "MAPS_KEY"]),
        "{asked}"
    );
    assert_eq!(task_activity(&w).await?, Some(Activity::NeedsAttention));

    // SRA-002: MAPS_KEY still waits.
    let allowed = answer(&w, &w.writer, true).await?;
    assert_eq!(allowed.status, StatusCode::OK, "{}", allowed.body);
    assert_eq!(task_activity(&w).await?, Some(Activity::NeedsAttention));

    // SRA-001: a dismissal answers too; nothing waits any more.
    let dismissed = answer_seen(&w, &w.writer, &maps, false, "asked", Some("#1")).await?;
    assert_eq!(dismissed.status, StatusCode::OK, "{}", dismissed.body);
    assert_eq!(task_activity(&w).await?, Some(Activity::Working));

    // SRA-003: a task flagged by something else before it asks keeps that flag.
    w.h.store
        .set_activity(
            &yulius_domain::ids::TaskId::parse("t1")?,
            Some(Activity::NeedsAttention),
            yulius_domain::time::Timestamp(1),
        )
        .await?;
    let again = ask(json!(["MAPS_KEY"])).await?;
    assert_eq!(support::tool_text(&again)["waiting"], json!(["MAPS_KEY"]));
    let allowed = answer_seen(&w, &w.writer, &maps, true, "asked", Some("#1")).await?;
    assert_eq!(allowed.status, StatusCode::OK, "{}", allowed.body);
    assert_eq!(task_activity(&w).await?, Some(Activity::NeedsAttention));
    Ok(())
}

/// SPEC-213 SRA-003 (review of #633, round 1): a flag raised after the request, while the task
/// already reads `needs_attention` (a question of the agent's), is not the request's: the answer
/// leaves it.
#[tokio::test]
async fn a_flag_raised_after_the_request_outlives_its_answer() -> TestResult {
    use yulius_domain::task::Activity;
    let w = world().await?;
    let asked = support::mcp_tool(
        &w.h,
        TOKEN,
        "secret_copy_to_env",
        json!({"secrets": ["STRIPE_KEY"]}),
    )
    .await?;
    assert_eq!(support::tool_text(&asked)["waiting"], json!(["STRIPE_KEY"]));
    let notice = call(
        &w.h.internal,
        Method::POST,
        "/hooks/claude/Notification",
        Some(json!({"notification_type": "elicitation_dialog", "message": "Pick one"})),
        &Creds {
            bearer: Some(TOKEN.into()),
            ..Creds::default()
        },
    )
    .await?;
    assert_eq!(notice.status, StatusCode::OK, "{}", notice.body);
    let allowed = answer(&w, &w.writer, true).await?;
    assert_eq!(allowed.status, StatusCode::OK, "{}", allowed.body);
    assert_eq!(task_activity(&w).await?, Some(Activity::NeedsAttention));
    Ok(())
}

/// SPEC-213 SRA-001 (review of #633, round 1): deleting the secret a task waits for ends the
/// request, and with it the flag.
#[tokio::test]
async fn deleting_the_asked_secret_ends_the_flag() -> TestResult {
    use yulius_domain::task::Activity;
    let w = world().await?;
    let asked = support::mcp_tool(
        &w.h,
        TOKEN,
        "secret_copy_to_env",
        json!({"secrets": ["STRIPE_KEY"]}),
    )
    .await?;
    assert_eq!(support::tool_text(&asked)["waiting"], json!(["STRIPE_KEY"]));
    assert_eq!(task_activity(&w).await?, Some(Activity::NeedsAttention));
    let deleted = call(
        &w.h.router,
        Method::DELETE,
        &format!("/api/agent-secrets/{}", w.id),
        None,
        &w.writer,
    )
    .await?;
    assert!(deleted.status.is_success(), "{}", deleted.body);
    assert_eq!(task_activity(&w).await?, Some(Activity::Working));
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

/// Review of SPEC-129, L2/L4/L5: everything that would refuse the copy is checked before a person
/// is asked; one unallowed secret stops the whole copy; a second task joins the askers; a revoked
/// secret is asked for again; a value no reader quotes alike is refused and nothing is written; a
/// project that ignores only `.env` still gets its copy, with no temporary file left behind.
#[tokio::test]
async fn the_copy_is_checked_before_anyone_is_asked() -> TestResult {
    use yulius_app::ports::store::TaskStore as _;
    let w = world().await?;
    let created = call(
        &w.h.router,
        Method::POST,
        "/api/sandboxes/s1/agent-secrets",
        Some(json!({"name": "WEIRD_KEY", "description": "A key with a backslash", "value": "it-is\\path-1234"})),
        &w.writer,
    )
    .await?;
    assert_eq!(created.status, StatusCode::CREATED, "{}", created.body);
    let weird = created.body["id"].as_str().ok_or("id")?.to_owned();
    let copy = |args: Value| support::mcp_tool(&w.h, TOKEN, "secret_copy_to_env", args);

    // Not ignored: refused, and nobody is asked.
    let refused = copy(json!({"secrets": ["STRIPE_KEY"], "path": "prod.env"})).await?;
    assert_eq!(refused["isError"], true, "{refused}");
    let secret = w.h.store.vault_get(&w.id).await?.ok_or("secret")?;
    assert_eq!(
        secret.env_files,
        EnvFiles::No,
        "a person is not asked for a copy that cannot work"
    );

    // A value no reader quotes alike: refused before asking, nothing written.
    let refused = copy(json!({"secrets": ["STRIPE_KEY", "WEIRD_KEY"]})).await?;
    assert_eq!(refused["isError"], true, "{refused}");
    assert!(!refused.to_string().contains("path-1234"), "{refused}");
    assert_eq!(
        w.h.store.vault_get(&weird).await?.ok_or("weird")?.env_files,
        EnvFiles::No
    );
    w.h.store
        .vault_set_env_files(&weird, (EnvFiles::No, None), (EnvFiles::Allowed, None))
        .await?;
    w.h.store
        .vault_set_env_files(&w.id, (EnvFiles::No, None), (EnvFiles::Allowed, None))
        .await?;
    let refused = copy(json!({"secrets": ["STRIPE_KEY", "WEIRD_KEY"]})).await?;
    assert_eq!(refused["isError"], true, "{refused}");
    assert!(!w.worktree.join(".env").exists(), "all or nothing");

    // One allowed, one not: nothing written, only the unallowed one waits.
    answer(&w, &w.writer, false).await?;
    w.h.store
        .vault_set_env_files(&weird, (EnvFiles::Allowed, None), (EnvFiles::No, None))
        .await?;
    let created = call(
        &w.h.router,
        Method::POST,
        "/api/sandboxes/s1/agent-secrets",
        Some(json!({"name": "DOCS_KEY", "description": "Docs", "value": "docs-key-123456"})),
        &w.writer,
    )
    .await?;
    let docs = created.body["id"].as_str().ok_or("id")?.to_owned();
    w.h.store
        .vault_set_env_files(&docs, (EnvFiles::No, None), (EnvFiles::Allowed, None))
        .await?;
    let mixed = copy(json!({"secrets": ["DOCS_KEY", "STRIPE_KEY"]})).await?;
    assert_ne!(mixed["isError"], true, "{mixed}");
    let text = support::tool_text(&mixed);
    assert_eq!(
        text["waiting"],
        json!(["STRIPE_KEY"]),
        "revoked, so asked again: {text}"
    );
    assert!(!w.worktree.join(".env").exists());

    // A second task asking joins the askers.
    let other = w.h.app.config.data_dir.join("sandboxes/s1/worktrees/t2");
    std::fs::create_dir_all(&other)?;
    git(&other, &["init", "-q", "-b", "dev"])?;
    std::fs::write(other.join(".gitignore"), ".env\n")?;
    seed_task(
        &w.h,
        "t2",
        "s1",
        "u-alice",
        TaskStatus::Running,
        Some(&other),
    )
    .await?;
    seed_live_attempt(&w.h, "t2", "env-task-token-t2-0123456789abcdef").await?;
    let asked = support::mcp_tool(
        &w.h,
        "env-task-token-t2-0123456789abcdef",
        "secret_copy_to_env",
        json!({"secrets": ["STRIPE_KEY"]}),
    )
    .await?;
    assert_ne!(asked["isError"], true, "{asked}");
    let numbers = |id: &str| {
        let store = w.h.store.clone();
        let id = yulius_domain::ids::TaskId::parse(id);
        async move { Ok::<_, Box<dyn std::error::Error>>(store.task(&id?).await?.ok_or("task")?.number) }
    };
    let (n1, n2) = (numbers("t1").await?, numbers("t2").await?);
    assert_eq!(
        w.h.store
            .vault_get(&w.id)
            .await?
            .ok_or("secret")?
            .env_requested_by,
        Some(format!("#{n1}, #{n2}"))
    );

    // Allowed: the second task's project ignores only `.env`, so no temporary name is ignored;
    // the copy is written in place and leaves nothing else behind.
    answer(&w, &w.writer, true).await?;
    let copied = support::mcp_tool(
        &w.h,
        "env-task-token-t2-0123456789abcdef",
        "secret_copy_to_env",
        json!({"secrets": ["STRIPE_KEY"]}),
    )
    .await?;
    assert_ne!(copied["isError"], true, "{copied}");
    assert_eq!(read(&other.join(".env")), format!("STRIPE_KEY={VALUE}\n"));
    let mut left: Vec<String> = std::fs::read_dir(&other)?
        .filter_map(|e| e.ok().map(|e| e.file_name().to_string_lossy().into_owned()))
        .collect();
    left.sort();
    assert_eq!(left, [".env", ".env.example", ".git", ".gitignore"]);
    Ok(())
}

/// #475: a `.env` that cannot be updated without breaking it is refused before a person is
/// asked, and nothing is written: a quote left open at the end of the file (a copied name already
/// holding a value over several lines is replaced whole since SPEC-186). An existing `.env.example` keeps its mode when a
/// name is added to it.
#[tokio::test]
async fn a_dotenv_that_cannot_be_updated_is_refused_and_the_example_keeps_its_mode() -> TestResult {
    use std::os::unix::fs::PermissionsExt as _;
    let w = world().await?;
    let env = w.worktree.join(".env");
    let copy = || {
        support::mcp_tool(
            &w.h,
            TOKEN,
            "secret_copy_to_env",
            json!({"secrets": ["STRIPE_KEY"]}),
        )
    };
    // (A multi-line value already there for the name is replaced whole since SPEC-186.)
    let text = "A=1\nBROKEN='never closed\n";
    std::fs::write(&env, text)?;
    let refused = copy().await?;
    assert_eq!(refused["isError"], true, "{refused}");
    assert!(
        refused
            .to_string()
            .contains("on line 2 of the file never closes"),
        "{refused}"
    );
    assert_eq!(read(&env), text, "nothing is written");
    assert!(!w.worktree.join(".env.example").exists());
    assert_eq!(
        w.h.store.vault_get(&w.id).await?.ok_or("secret")?.env_files,
        EnvFiles::No,
        "a person is not asked for a copy that cannot work"
    );

    std::fs::write(&env, "A=1\n")?;
    let example = w.worktree.join(".env.example");
    std::fs::write(&example, "A=\n")?;
    std::fs::set_permissions(&example, std::fs::Permissions::from_mode(0o755))?;
    w.h.store
        .vault_set_env_files(&w.id, (EnvFiles::No, None), (EnvFiles::Allowed, None))
        .await?;
    let copied = copy().await?;
    assert_ne!(copied["isError"], true, "{copied}");
    assert_eq!(read(&env), format!("A=1\nSTRIPE_KEY={VALUE}\n"));
    assert_eq!(read(&example), "A=\nSTRIPE_KEY=\n");
    assert_eq!(
        std::fs::metadata(&example)?.permissions().mode() & 0o777,
        0o755,
        "the template keeps the mode it was committed with"
    );
    Ok(())
}

/// Review of SPEC-129, L5: a secret of every project is answered by instance administrators
/// only, and its askers name their project.
#[tokio::test]
async fn secrets_of_every_project_are_answered_by_administrators() -> TestResult {
    let w = world().await?;
    support::setup_superadmin_creds(&w.h).await?;
    let admin = support::superadmin_bearer(&w.h).await?;
    let created = call(
        &w.h.router,
        Method::POST,
        "/api/sandboxes/s1/agent-secrets",
        Some(json!({"scope": "instance", "name": "SHARED_KEY", "description": "Shared", "value": "shared-key-123456"})),
        &admin,
    )
    .await?;
    assert_eq!(created.status, StatusCode::CREATED, "{}", created.body);
    let id = created.body["id"].as_str().ok_or("id")?.to_owned();
    let asked = support::mcp_tool(
        &w.h,
        TOKEN,
        "secret_copy_to_env",
        json!({"secrets": ["SHARED_KEY"]}),
    )
    .await?;
    assert_ne!(asked["isError"], true, "{asked}");
    let secret = w.h.store.vault_get(&id).await?.ok_or("secret")?;
    let by = secret.env_requested_by.clone().ok_or("asker")?;
    assert!(by.ends_with(" in app"), "{by}");
    let refused = answer_seen(&w, &w.writer, &id, true, "asked", Some(&by)).await?;
    assert_eq!(refused.status, StatusCode::FORBIDDEN, "{}", refused.body);
    use yulius_domain::task::Activity;
    assert_eq!(task_activity(&w).await?, Some(Activity::NeedsAttention));
    let allowed = answer_seen(&w, &admin, &id, true, "asked", Some(&by)).await?;
    assert_eq!(allowed.status, StatusCode::OK, "{}", allowed.body);
    assert_eq!(allowed.body["env_files"], "allowed");
    // SPEC-213 SRA-001: the task asked as `#1 in app`.
    assert_eq!(task_activity(&w).await?, Some(Activity::Working));
    Ok(())
}

/// SPEC-209 SRE-001/004 (#631): an agent's `.env` request is announced to the open Secrets pages
/// of the secret's sandbox; the same asker again changes nothing and is not announced.
#[tokio::test]
async fn an_env_request_is_announced_to_open_secrets_pages() -> TestResult {
    let w = world().await?;
    let mut announced = support::SecretAnnouncements::new(&w.h.app);
    let copy = || {
        support::mcp_tool(
            &w.h,
            TOKEN,
            "secret_copy_to_env",
            json!({"secrets": ["STRIPE_KEY"]}),
        )
    };
    let asked = copy().await?;
    assert_ne!(asked["isError"], true, "{asked}");
    assert_eq!(announced.take(), vec![Some("s1".to_owned())]);
    let again = copy().await?;
    assert_ne!(again["isError"], true, "{again}");
    assert_eq!(announced.take(), Vec::<Option<String>>::new());
    Ok(())
}

/// A synthetic key file: `-----BEGIN <label>-----`, `body`, `-----END <label>-----`, each on
/// its own line, with a final line break (the armour is built at run time so no scanner takes
/// the source for a key).
pub(crate) fn key_file(label: &str, body: &[&str]) -> String {
    let mut lines = vec![format!("-----BEGIN {label}-----")];
    lines.extend(body.iter().map(|l| (*l).to_owned()));
    lines.push(format!("-----END {label}-----"));
    format!("{}\n", lines.join("\n"))
}

/// A synthetic 3-line PEM key.
pub(crate) fn pem_key() -> String {
    key_file(
        &format!("{} KEY", "SYNTHETIC ED25519"),
        &["c3ludGhldGljLWVkMjU1MTktcGVtLWtleS1ib2R5"],
    )
}

/// A synthetic 7-line OpenSSH key (`n` makes each a different one).
pub(crate) fn openssh_key(n: u8) -> String {
    let body: Vec<String> = (1..=5)
        .map(|line| format!("c3ludGhldGljLW9wZW5zc2gta2V5LXZ{n}-bGluZS0w{line}"))
        .collect();
    let body: Vec<&str> = body.iter().map(String::as_str).collect();
    key_file(&format!("{} PRIVATE KEY", "OPENSSH"), &body)
}

/// What `sh` reads for `name` from the `.env` in `dir`, byte for byte.
pub(crate) fn sourced(dir: &Path, name: &str) -> Result<String, Box<dyn std::error::Error>> {
    let out = Command::new("sh")
        .args(["-c", &format!(". ./.env && printf %s \"${name}\"")])
        .current_dir(dir)
        .output()?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).into_owned().into());
    }
    Ok(String::from_utf8(out.stdout)?)
}

/// SPEC-186 SAV-005: keys stored with literal `\n` (an agent secret never spans lines), with or
/// without a final one, come out of the `.env` byte-identical to their files: a 3-line PEM key
/// and a 7-line OpenSSH key. Their lines stay redacted when the file is read, and copying again
/// after a rotation replaces the whole value.
#[tokio::test]
async fn multi_line_keys_reach_the_env_byte_identical_to_their_files() -> TestResult {
    let w = world().await?;
    let (pem, ssh) = (pem_key(), openssh_key(1));
    assert_eq!((pem.lines().count(), ssh.lines().count()), (3, 7));
    let stored_pem = pem.replace('\n', "\\n");
    let stored_ssh = ssh.trim_end().replace('\n', "\\n");
    let mut ids = Vec::new();
    for (name, value) in [("DEPLOY_PEM", &stored_pem), ("DEPLOY_SSH", &stored_ssh)] {
        let created = call(
            &w.h.router,
            Method::POST,
            "/api/sandboxes/s1/agent-secrets",
            Some(json!({"name": name, "value": value})),
            &w.writer,
        )
        .await?;
        assert_eq!(created.status, StatusCode::CREATED, "{}", created.body);
        let id = created.body["id"].as_str().ok_or("id")?.to_owned();
        assert!(
            w.h.store
                .vault_set_env_files(&id, (EnvFiles::No, None), (EnvFiles::Allowed, None))
                .await?
        );
        ids.push(id);
    }
    std::fs::write(w.worktree.join(".env"), "# local\nOTHER=1\n")?;
    let copied = support::mcp_tool(
        &w.h,
        TOKEN,
        "secret_copy_to_env",
        json!({"secrets": ["DEPLOY_PEM", "DEPLOY_SSH"]}),
    )
    .await?;
    assert_ne!(copied["isError"], true, "{copied}");
    assert_eq!(sourced(&w.worktree, "DEPLOY_PEM")?, pem);
    assert_eq!(sourced(&w.worktree, "DEPLOY_SSH")?, ssh);
    assert_eq!(sourced(&w.worktree, "OTHER")?, "1");

    // The key's lines are redacted wherever the file is shown (`cat .env`).
    let text = read(&w.worktree.join(".env"));
    let shown =
        w.h.app
            .redact_for_sandbox(&yulius_domain::ids::SandboxId::parse("s1")?, &text)
            .await;
    for body in pem
        .lines()
        .chain(ssh.lines())
        .filter(|l| !l.starts_with("-----"))
    {
        assert!(text.contains(body), "{body}");
        assert!(!shown.contains(body), "{body} shown in {shown}");
    }
    assert!(shown.contains("[secret:DEPLOY_SSH]"), "{shown}");

    // Rotated and copied again: the whole old value goes, the new one is read as its file.
    let rotated = openssh_key(2);
    let patched = call(
        &w.h.router,
        Method::PATCH,
        &format!("/api/agent-secrets/{}", ids[1]),
        Some(json!({"value": rotated.replace('\n', "\\n")})),
        &w.writer,
    )
    .await?;
    assert!(patched.status.is_success(), "{}", patched.body);
    let again = support::mcp_tool(
        &w.h,
        TOKEN,
        "secret_copy_to_env",
        json!({"secrets": ["DEPLOY_SSH"]}),
    )
    .await?;
    assert_ne!(again["isError"], true, "{again}");
    assert_eq!(sourced(&w.worktree, "DEPLOY_SSH")?, rotated);
    assert_eq!(sourced(&w.worktree, "DEPLOY_PEM")?, pem);
    let text = read(&w.worktree.join(".env"));
    assert!(
        !text.contains("LXZ1-"),
        "the old key's lines are gone: {text}"
    );
    // Each key's lines, and its closing quote on a line of its own.
    assert_eq!(text.lines().count(), 2 + (3 + 1) + (7 + 1), "{text}");
    Ok(())
}

/// SPEC-256 LOB-004: a secret created already allowed into `.env` files (the person's laptop
/// agent imports a deploy key) is copied by the first `secret_copy_to_env`, with no person;
/// the allowance is audited at creation. Refused for a secret a person confirms (not
/// `auto_allow`) and for a personal one.
#[tokio::test]
async fn a_secret_created_allowed_is_copied_without_a_person() -> TestResult {
    let w = world().await?;
    let created = call(
        &w.h.router,
        Method::POST,
        "/api/sandboxes/s1/agent-secrets",
        Some(json!({"name": "DEPLOY_TOKEN", "description": "Deploys", "value": "deploy-value-0123456789", "env_files": true})),
        &w.writer,
    )
    .await?;
    assert_eq!(created.status, StatusCode::CREATED, "{}", created.body);
    assert_eq!(created.body["env_files"], "allowed", "{}", created.body);
    let copied = support::mcp_tool(
        &w.h,
        TOKEN,
        "secret_copy_to_env",
        json!({"secrets": ["DEPLOY_TOKEN"], "reason": "deploy.sh reads it"}),
    )
    .await?;
    let said = copied.to_string();
    assert!(
        said.contains("DEPLOY_TOKEN") && !said.contains("waiting\":[\"DEPLOY_TOKEN"),
        "{said}"
    );
    assert!(
        read(&w.worktree.join(".env")).contains("DEPLOY_TOKEN=deploy-value-0123456789"),
        "{said}"
    );
    let allowed: Vec<Value> = activity(&w)
        .await?
        .into_iter()
        .filter(|e| e["action"] == "vault.agent.env_allow")
        .collect();
    assert_eq!(allowed.len(), 1, "{allowed:?}");
    assert!(
        allowed[0].to_string().contains("at creation"),
        "{allowed:?}"
    );

    for refused in [
        json!({"name": "CONFIRMED_KEY", "value": "confirmed-value-0123", "env_files": true, "auto_allow": false}),
        json!({"name": "MY_KEY", "value": "personal-value-01234", "env_files": true, "scope": "personal"}),
    ] {
        let answer = call(
            &w.h.router,
            Method::POST,
            "/api/sandboxes/s1/agent-secrets",
            Some(refused.clone()),
            &w.writer,
        )
        .await?;
        assert_eq!(
            answer.status,
            StatusCode::UNPROCESSABLE_ENTITY,
            "{refused}: {}",
            answer.body
        );
    }
    Ok(())
}
