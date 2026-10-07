//! Internal agent endpoints over HTTP: hook events (briefing, message digests, the Stop
//! discipline gate) and repository-scoped Git credentials (SPEC-002 §7.5, §8; AC-DSC-3,
//! AC-COM-2, AC-COM-4).

mod support;

use std::path::Path;
use std::process::Command;
use std::sync::Arc;

use axum::http::Method;
use serde_json::json;
use support::{Creds, Harness, TestResult, call, harness_with_git};
use yulius::wiring::git::GitAdapter;
use yulius_app::model::{
    AttemptRecord, LaunchKind, MessageRecord, SandboxRecord, TaskOrigin, TaskRecord, UserRecord,
};
use yulius_app::ports::github::GitHubAppConfig;
use yulius_domain::harness::Harness as Agent;
use yulius_domain::ids::{AttemptId, MessageId, SandboxId, TaskId, UserId};
use yulius_domain::task::{Activity, AttemptState, TaskStatus};
use yulius_domain::time::Timestamp;

const TOKEN: &str = "task-token-for-tests-0123456789abcdef";

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

struct Fixture {
    h: Harness,
    task: TaskId,
    sandbox: SandboxId,
    worktree: std::path::PathBuf,
    _repo: tempfile::TempDir,
}

/// A running task with a live attempt whose worktree is a real repository with `origin/dev`.
async fn fixture() -> Result<Fixture, Box<dyn std::error::Error>> {
    let h = harness_with_git(Arc::new(GitAdapter::new("git")))?;
    let repo = tempfile::tempdir()?;
    let origin = repo.path().join("origin.git");
    let worktree = repo.path().join("wt");
    std::fs::create_dir_all(&origin)?;
    std::fs::create_dir_all(&worktree)?;
    git(&origin, &["init", "--bare", "-q", "-b", "dev"])?;
    git(&worktree, &["init", "-q", "-b", "dev"])?;
    std::fs::write(worktree.join("README.md"), "# fixture\n")?;
    std::fs::create_dir_all(worktree.join("docs/decisions"))?;
    std::fs::write(
        worktree.join("docs/decisions/0001-use-rust.md"),
        "# Use Rust\n\nBecause.\n",
    )?;
    std::fs::write(
        worktree.join("docs/product.md"),
        "# Fixture product\n\nSells widgets.\n",
    )?;
    git(&worktree, &["add", "."])?;
    git(&worktree, &["commit", "-q", "-m", "init"])?;
    git(
        &worktree,
        &["remote", "add", "origin", &origin.to_string_lossy()],
    )?;
    git(&worktree, &["push", "-q", "origin", "dev"])?;

    let store = &h.app.deps.store;
    let user = UserRecord {
        id: UserId::parse("u1")?,
        github_login: Some("alice".into()),
        github_id: Some(1),
        display_name: "Alice".into(),
        avatar_url: None,
        is_superadmin: true,
        is_instance_admin: true,
        password_hash: None,
        created_at: Timestamp(1),
        disabled_at: None,
    };
    store.insert_user(&user).await?;
    let sandbox = SandboxRecord {
        id: SandboxId::parse("s1")?,
        slug: "fixture".into(),
        repo_owner: "acme".into(),
        repo_name: "fixture".into(),
        github_repo_id: Some(1),
        default_branch: "dev".into(),
        profile_json: "{}".into(),
        port_base: 42000,
        port_count: 10,
        onboarding_state: "not_needed".into(),
        onboarding_task_id: None,
        created_by: user.id.clone(),
        created_at: Timestamp(1),
        removed_at: None,
    };
    store.insert_sandbox(&sandbox).await?;
    let now = h.app.now();
    let mk_task = |id: &str,
                   number: u32,
                   title: &str,
                   status: TaskStatus|
     -> Result<TaskRecord, Box<dyn std::error::Error>> {
        Ok(TaskRecord {
            id: TaskId::parse(id)?,
            sandbox_id: sandbox.id.clone(),
            number,
            title: title.into(),
            prompt: format!("{title} please"),
            harness: Agent::ClaudeCode,
            model: None,
            effort: None,
            workflow_id: None,
            status,
            activity: None,
            branch: Some(format!("feat/{number}-x")),
            worktree_path: None,
            spec_path: None,
            origin: TaskOrigin::Ui,
            automation_run: None,
            created_by: user.id.clone(),
            created_at: now,
            updated_at: now,
            closed_at: None,
            archived_at: None,
            queued_at: None,
            gate_blocks: 0,
            wake_enabled: true,
        })
    };
    let mut main = mk_task("t-main", 1, "Checkout API", TaskStatus::Running)?;
    main.worktree_path = Some(worktree.to_string_lossy().into_owned());
    main.spec_path = Some("docs/specs/checkout.md".into());
    store.insert_task(&main).await?;
    store
        .insert_task(&mk_task(
            "t-other",
            2,
            "Invoice export",
            TaskStatus::Running,
        )?)
        .await?;
    let hash = h.app.deps.crypto.sha256_hex(TOKEN.as_bytes());
    store
        .insert_attempt(&AttemptRecord {
            id: AttemptId::parse("at1")?,
            task_id: main.id.clone(),
            seq: 1,
            harness: Agent::ClaudeCode,
            account_id: None,
            provider_session_id: None,
            launch_kind: LaunchKind::New,
            state: AttemptState::Live,
            run_dir: repo.path().join("run").to_string_lossy().into_owned(),
            token_hash: hash,
            host_token: h.app.deps.crypto.seal(&[1u8; 32])?,
            ptyhost_pid: None,
            child_pid: None,
            started_at: now,
            ended_at: None,
            exit_code: None,
            exit_reason: None,
        })
        .await?;
    Ok(Fixture {
        h,
        task: main.id,
        sandbox: sandbox.id,
        worktree,
        _repo: repo,
    })
}

fn agent() -> Creds {
    Creds {
        bearer: Some(TOKEN.into()),
        ..Creds::default()
    }
}

async fn hook(
    f: &Fixture,
    event: &str,
    body: serde_json::Value,
) -> Result<serde_json::Value, Box<dyn std::error::Error>> {
    let resp = call(
        &f.h.internal,
        Method::POST,
        &format!("/hooks/claude/{event}"),
        Some(body),
        &agent(),
    )
    .await?;
    assert_eq!(resp.status, 200, "{event}: {:?}", resp.body);
    Ok(resp.body)
}

async fn activity(f: &Fixture) -> Result<Option<Activity>, Box<dyn std::error::Error>> {
    Ok(f.h
        .app
        .deps
        .store
        .task(&f.task)
        .await?
        .ok_or("task")?
        .activity)
}

#[tokio::test]
async fn hooks_require_a_live_task_token() -> TestResult {
    let f = fixture().await?;
    let wrong = Creds {
        bearer: Some("nope".into()),
        ..Creds::default()
    };
    let resp = call(
        &f.h.internal,
        Method::POST,
        "/hooks/claude/Stop",
        Some(json!({})),
        &wrong,
    )
    .await?;
    assert_eq!(resp.status, 401);
    let resp = call(
        &f.h.internal,
        Method::POST,
        "/hooks/claude/Stop",
        Some(json!({})),
        &Creds::default(),
    )
    .await?;
    assert_eq!(resp.status, 401);
    let resp = call(
        &f.h.internal,
        Method::POST,
        "/hooks/other/Stop",
        Some(json!({})),
        &agent(),
    )
    .await?;
    assert_eq!(resp.status, 404);
    Ok(())
}

#[tokio::test]
async fn session_start_returns_the_project_briefing() -> TestResult {
    let f = fixture().await?;
    let body = hook(
        &f,
        "SessionStart",
        json!({"session_id": "sess-1", "hook_event_name": "SessionStart"}),
    )
    .await?;
    let context = body["hookSpecificOutput"]["additionalContext"]
        .as_str()
        .ok_or("no additionalContext")?;
    assert_eq!(body["hookSpecificOutput"]["hookEventName"], "SessionStart");
    for expected in [
        "Checkout API",
        "Invoice export",
        "Use Rust",
        "Sells widgets",
        "42000",
    ] {
        assert!(
            context.contains(expected),
            "briefing lacks {expected}:\n{context}"
        );
    }
    assert!(context.len() <= 12 * 1024 + 64);
    let attempt =
        f.h.app
            .deps
            .store
            .live_attempt(&f.task)
            .await?
            .ok_or("attempt")?;
    assert_eq!(attempt.provider_session_id.as_deref(), Some("sess-1"));
    Ok(())
}

#[tokio::test]
async fn prompt_submit_delivers_message_digests_once() -> TestResult {
    let f = fixture().await?;
    let message = MessageRecord {
        id: MessageId::parse("m1")?,
        sandbox_id: f.sandbox.clone(),
        from_task: Some(TaskId::parse("t-other")?),
        from_user: None,
        to_task: Some(f.task.clone()),
        kind: "info".into(),
        subject: "Schema changed".into(),
        body: "The invoices table gained a currency column.".into(),
        created_at: f.h.app.now(),
    };
    f.h.app
        .deps
        .store
        .insert_message(&message, std::slice::from_ref(&f.task))
        .await?;
    let body = hook(&f, "UserPromptSubmit", json!({"prompt": "go"})).await?;
    let context = body["hookSpecificOutput"]["additionalContext"]
        .as_str()
        .ok_or("no digest")?;
    assert!(context.contains("Schema changed") && context.contains("currency column"));
    assert_eq!(activity(&f).await?, Some(Activity::Working));
    let again = hook(&f, "UserPromptSubmit", json!({"prompt": "go"})).await?;
    assert!(again.is_null(), "digest delivered only once: {again:?}");
    Ok(())
}

#[tokio::test]
async fn stop_gate_blocks_three_times_then_needs_attention() -> TestResult {
    let f = fixture().await?;
    // Clean worktree, nothing ahead of origin/dev: the turn simply ends.
    let body = hook(&f, "Stop", json!({"stop_hook_active": false})).await?;
    assert!(body.is_null(), "{body:?}");
    assert_eq!(activity(&f).await?, Some(Activity::Waiting));

    // Code changed without a spec: blocked with a fix-it reason, at most three times.
    std::fs::write(f.worktree.join("api.rs"), "fn main() {}\n")?;
    for n in 1..=3 {
        let body = hook(&f, "Stop", json!({"stop_hook_active": n > 1})).await?;
        assert_eq!(body["decision"], "block", "block {n}");
        let reason = body["reason"].as_str().ok_or("reason")?;
        assert!(reason.contains("docs/specs/checkout.md"), "{reason}");
    }
    let body = hook(&f, "Stop", json!({"stop_hook_active": true})).await?;
    assert!(body.is_null(), "fourth stop is allowed: {body:?}");
    assert_eq!(activity(&f).await?, Some(Activity::NeedsAttention));

    // A new turn resets the counter; an invalid spec lists what is missing.
    hook(&f, "UserPromptSubmit", json!({"prompt": "fix it"})).await?;
    std::fs::create_dir_all(f.worktree.join("docs/specs"))?;
    std::fs::write(
        f.worktree.join("docs/specs/checkout.md"),
        "---\nyulius-task: t-main\nstatus: in-progress\n---\n# Checkout API\n\n## Goal\n\nPay.\n",
    )?;
    let body = hook(&f, "Stop", json!({})).await?;
    assert_eq!(body["decision"], "block");
    assert!(
        body["reason"].as_str().is_some_and(|r| r.contains("Plan")),
        "{body:?}"
    );

    // A valid spec lets the turn end.
    std::fs::write(
        f.worktree.join("docs/specs/checkout.md"),
        "---\nyulius-task: t-main\nstatus: in-progress\n---\n# Checkout API\n\n\
         ## Goal\n\nCustomers can pay.\n\n## Context\n\nNo checkout yet.\n\n\
         ## Requirements\n\n- Card payments.\n\n## Plan\n\n- [x] Write spec\n- [ ] Implement API\n\n\
         ## Acceptance Criteria\n\nA test order succeeds.\n\n## Test plan\n\n```sh\ncargo test\n```\n\n\
         ## Decisions\n\n- Hosted payment page.\n",
    )?;
    let body = hook(&f, "Stop", json!({})).await?;
    assert!(body.is_null(), "valid spec passes: {body:?}");
    assert_eq!(activity(&f).await?, Some(Activity::Waiting));
    let task = f.h.app.deps.store.task(&f.task).await?.ok_or("task")?;
    assert_eq!(task.gate_blocks, 0);
    Ok(())
}

#[tokio::test]
async fn stop_blocks_once_for_unread_user_messages() -> TestResult {
    let f = fixture().await?;
    let message = MessageRecord {
        id: MessageId::parse("m2")?,
        sandbox_id: f.sandbox.clone(),
        from_task: None,
        from_user: Some(UserId::parse("u1")?),
        to_task: Some(f.task.clone()),
        kind: "request".into(),
        subject: "Also add refunds".into(),
        body: "Refunds should be possible within 14 days.".into(),
        created_at: f.h.app.now(),
    };
    f.h.app
        .deps
        .store
        .insert_message(&message, std::slice::from_ref(&f.task))
        .await?;
    let body = hook(&f, "Stop", json!({})).await?;
    assert_eq!(body["decision"], "block");
    assert!(
        body["reason"]
            .as_str()
            .is_some_and(|r| r.contains("Also add refunds"))
    );
    let body = hook(&f, "Stop", json!({"stop_hook_active": true})).await?;
    assert!(
        body.is_null(),
        "delivered messages do not block again: {body:?}"
    );
    Ok(())
}

#[tokio::test]
async fn permission_requests_need_attention() -> TestResult {
    let f = fixture().await?;
    let body = hook(
        &f,
        "Notification",
        json!({"message": "Claude needs your permission"}),
    )
    .await?;
    assert!(body.is_null());
    assert_eq!(activity(&f).await?, Some(Activity::NeedsAttention));
    Ok(())
}

#[tokio::test]
async fn git_credentials_are_scoped_to_the_project_repository() -> TestResult {
    let f = fixture().await?;
    let store = &f.h.app.deps.store;
    store.init_instance("https://yulius.test", "x").await?;
    let app_config = GitHubAppConfig {
        app_id: 1,
        slug: "yulius-test".into(),
        client_id: "Iv1.test".into(),
        client_secret: "secret".into(),
        private_key_pem: "pem".into(),
        webhook_secret: None,
    };
    let sealed =
        f.h.app
            .deps
            .crypto
            .seal(&serde_json::to_vec(&app_config)?)?;
    store.set_github_app(&sealed, "acme", "org").await?;
    store.set_installation(Some(7)).await?;

    let ok = call(
        &f.h.internal,
        Method::POST,
        "/internal/git-credential",
        Some(json!({"protocol": "https", "host": "github.com", "path": "acme/fixture.git"})),
        &agent(),
    )
    .await?;
    assert_eq!(ok.status, 200);
    assert_eq!(ok.body["token"], "inst");
    for (body, status) in [
        (
            json!({"protocol": "https", "host": "github.com", "path": "acme/other.git"}),
            403,
        ),
        (
            json!({"protocol": "https", "host": "gitlab.com", "path": "acme/fixture.git"}),
            403,
        ),
        (json!({"protocol": "http", "host": "github.com"}), 403),
    ] {
        let resp = call(
            &f.h.internal,
            Method::POST,
            "/internal/git-credential",
            Some(body),
            &agent(),
        )
        .await?;
        assert_eq!(resp.status, status);
    }
    let anon = call(
        &f.h.internal,
        Method::POST,
        "/internal/git-credential",
        Some(json!({"protocol": "https", "host": "github.com"})),
        &Creds::default(),
    )
    .await?;
    assert_eq!(anon.status, 401);
    Ok(())
}

#[tokio::test]
async fn briefing_includes_the_task_workflow_instructions() -> TestResult {
    let f = fixture().await?;
    f.h.app.ensure_builtin_workflows().await?;
    let mut task = f.h.app.deps.store.task(&f.task).await?.ok_or("task")?;
    task.workflow_id = Some("wf-feature".into());
    let briefing = f.h.app.briefing(&task, Agent::ClaudeCode).await?;
    assert!(briefing.contains("## Workflow instructions"), "{briefing}");
    assert!(briefing.contains("Implement only what the spec describes"));
    assert!(briefing.len() <= 12 * 1024 + 64);
    Ok(())
}
