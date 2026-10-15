//! Internal agent endpoints over HTTP: hook events (briefing, the Stop
//! discipline gate) and repository-scoped Git credentials, the task creator's own (SPEC-002
//! §7.5, §8; AC-DSC-3, AC-COM-2, AC-COM-4, AC-GIT-3).

use crate::support;

use std::path::Path;
use std::process::Command;
use std::sync::Arc;

use axum::http::Method;
use serde_json::json;
use support::{Creds, Harness, TestResult, call, harness_with_git};
use yulius::wiring::git::GitAdapter;
use yulius_app::hub::HubFrame;
use yulius_app::model::{
    AttemptRecord, GitHubTokenRecord, LaunchKind, MessageRecord, SandboxRecord, TaskOrigin,
    TaskRecord, UserRecord,
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
    fixture_with_workflow(None).await
}

/// [`fixture`] whose task follows a sandbox workflow `(id, require_spec)`.
async fn fixture_with_workflow(
    workflow: Option<(&str, bool)>,
) -> Result<Fixture, Box<dyn std::error::Error>> {
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
            owner: user.id.clone(),
            created_at: now,
            updated_at: now,
            closed_at: None,
            archived_at: None,
            queued_at: None,
            gate_blocks: 0,
            wake_enabled: true,
            mode: yulius_app::model::TaskMode::Prompt,
            supertask: false,
        })
    };
    let mut main = mk_task("t-main", 1, "Checkout API", TaskStatus::Running)?;
    if let Some((id, require_spec)) = workflow {
        store
            .insert_workflow(
                &yulius_app::workflows::WorkflowRecord {
                    id: id.into(),
                    sandbox_id: Some(sandbox.id.clone()),
                    name: id.into(),
                    description: String::new(),
                    steps: Vec::new(),
                    instructions: String::new(),
                    require_spec,
                    require_tests: false,
                    created_at: Timestamp(1),
                    builtin: false,
                    when_to_use: String::new(),
                },
                10,
            )
            .await?;
        main.workflow_id = Some(id.into());
    }
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
    // The run's subscription has capacity (its usage is not known yet), so turn boundaries are
    // never held for capacity here (ADR-17; holds are covered by subscription_pool.rs).
    let account = yulius_domain::ids::AccountId::parse("acc-1")?;
    store
        .insert_account(&yulius_app::model::AccountRecord {
            id: account.clone(),
            provider: yulius_domain::harness::Provider::Anthropic,
            label: "Claude #1".into(),
            credential: Some(h.app.deps.crypto.seal(b"sk-ant-oat01-hooks-test")?),
            status: yulius_app::model::AccountStatus::Ok,
            usage_json: None,
            last_error: None,
            created_at: now,
            updated_at: now,
        })
        .await?;
    let hash = h.app.deps.crypto.sha256_hex(TOKEN.as_bytes());
    store
        .insert_attempt(&AttemptRecord {
            id: AttemptId::parse("at1")?,
            task_id: main.id.clone(),
            seq: 1,
            harness: Agent::ClaudeCode,
            account_id: Some(account),
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
    // The discipline of #139: product context first, the pipeline, the docs index, sessions as
    // docs, and the foundations this project lacks (no yulius.toml, no test or lint command).
    for expected in [
        "## Read before you build",
        "`sessions_search`",
        "steps of similar effort",
        "independent review in a fresh session",
        "skill `git-hygiene`",
        "Workflow: none",
        "## Project foundations",
        "a test command (`[commands] test`)",
        "skill `infra-bootstrap`",
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

/// MSR-004 (SPEC-028): stored messages reach no hook: no digest, no turn-end block.
#[tokio::test]
async fn stored_messages_reach_no_hook() -> TestResult {
    let f = fixture().await?;
    for (id, from_user) in [("m1", None), ("m2", Some(UserId::parse("u1")?))] {
        let message = MessageRecord {
            id: MessageId::parse(id)?,
            sandbox_id: f.sandbox.clone(),
            from_task: None,
            from_user,
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
    }
    let body = hook(&f, "UserPromptSubmit", json!({"prompt": "go"})).await?;
    assert!(body.is_null(), "no digest: {body:?}");
    assert_eq!(activity(&f).await?, Some(Activity::Working));
    let body = hook(&f, "Stop", json!({})).await?;
    assert!(
        body.is_null(),
        "unread messages do not block a turn end: {body:?}"
    );
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

/// DSC-013 (#139): features and fixes need a spec; research, questions, docs and chores do not.
#[tokio::test]
async fn stop_gate_asks_for_a_spec_only_where_it_matters() -> TestResult {
    let f = fixture().await?;
    // No workflow: a documentation-only change ends the turn without a spec.
    std::fs::write(f.worktree.join("docs/setup.md"), "# Setup\n")?;
    let body = hook(&f, "Stop", json!({})).await?;
    assert!(body.is_null(), "docs need no spec: {body:?}");
    // A code change does.
    std::fs::write(f.worktree.join("api.rs"), "fn main() {}\n")?;
    let body = hook(&f, "Stop", json!({})).await?;
    assert_eq!(body["decision"], "block", "{body:?}");
    assert!(
        body["reason"]
            .as_str()
            .is_some_and(|r| r.contains("needs a spec")),
        "{body:?}"
    );
    // A research task changes code without a spec (its workflow says so).
    let research = fixture_with_workflow(Some(("wf-research-test", false))).await?;
    std::fs::write(research.worktree.join("probe.rs"), "fn main() {}\n")?;
    let body = hook(&research, "Stop", json!({})).await?;
    assert!(body.is_null(), "research needs no spec: {body:?}");
    // A feature needs one even for documentation.
    let feature = fixture_with_workflow(Some(("wf-feature-test", true))).await?;
    std::fs::write(feature.worktree.join("docs/setup.md"), "# Setup\n")?;
    let body = hook(&feature, "Stop", json!({})).await?;
    assert_eq!(
        body["decision"], "block",
        "a feature needs a spec: {body:?}"
    );
    Ok(())
}

/// STP-008 (#139): changing the repository while no step of the plan was ever started is
/// nudged once at a turn end; the nudge is not a gate block and never repeats.
#[tokio::test]
async fn untracked_work_is_nudged_once() -> TestResult {
    let f = fixture().await?;
    let now = f.h.app.now();
    let step = |id: &str,
                ordinal: u32,
                title: &str|
     -> Result<yulius_app::model::StepRecord, Box<dyn std::error::Error>> {
        Ok(yulius_app::model::StepRecord {
            id: yulius_domain::ids::StepId::parse(id)?,
            task_id: f.task.clone(),
            ordinal,
            title: title.into(),
            status: "pending".into(),
            note: None,
            updated_by: Some("system".into()),
            updated_at: Some(now),
            effort: 1,
        })
    };
    f.h.app
        .deps
        .store
        .replace_steps(
            &f.task,
            &[
                step("st-1", 1, "Describe the setup")?,
                step("st-2", 2, "Describe the deploy")?,
            ],
            "system",
            now,
        )
        .await?;
    // Nothing changed: no nudge.
    assert!(hook(&f, "Stop", json!({})).await?.is_null());
    std::fs::write(f.worktree.join("docs/setup.md"), "# Setup\n")?;
    let body = hook(&f, "Stop", json!({})).await?;
    assert_eq!(body["decision"], "block", "{body:?}");
    let reason = body["reason"].as_str().ok_or("reason")?;
    assert!(
        reason.contains("no step of your plan is in progress")
            && reason.contains("\"Describe the setup\""),
        "{reason}"
    );
    let task = f.h.app.deps.store.task(&f.task).await?.ok_or("task")?;
    assert_eq!(task.gate_blocks, 0, "a nudge is not a gate block");
    assert!(hook(&f, "Stop", json!({})).await?.is_null(), "given once");
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

/// F 2026-10-11: Claude Code's idle reminder ("Claude is waiting for your input", about a
/// minute after a turn ended) made every finished task need attention, so chains and onboarding
/// never saw it finished. Only notices that ask a person to act do.
#[tokio::test]
async fn the_idle_reminder_keeps_a_finished_task_waiting() -> TestResult {
    let f = fixture().await?;
    let store = &f.h.app.deps.store;
    store
        .set_activity(&f.task, Some(Activity::Waiting), Timestamp(10))
        .await?;
    for notice in [
        json!({"message": "Claude is waiting for your input", "notification_type": "idle_prompt"}),
        json!({"message": "Claude is waiting for your input"}),
        json!({"message": "Signed in", "notification_type": "auth_success"}),
    ] {
        let body = hook(&f, "Notification", notice.clone()).await?;
        assert!(body.is_null());
        assert_eq!(
            activity(&f).await?,
            Some(Activity::Waiting),
            "{notice} keeps the task waiting"
        );
    }
    hook(
        &f,
        "Notification",
        json!({"message": "Claude needs your permission to use Bash", "notification_type": "permission_prompt"}),
    )
    .await?;
    assert_eq!(activity(&f).await?, Some(Activity::NeedsAttention));
    Ok(())
}

/// STA-002 (SPEC-032, task #23 shown "Needs you" while healthy): Claude Code 2.1.292 sends notices
/// that ask nobody to act (a background agent finished, a push notification, a quota resume);
/// they leave a working task working. Only the notices that ask a person do flag it.
#[tokio::test]
async fn informational_notices_leave_a_working_task_alone() -> TestResult {
    let f = fixture().await?;
    let store = &f.h.app.deps.store;
    store
        .set_activity(&f.task, Some(Activity::Working), Timestamp(10))
        .await?;
    for kind in [
        "agent_completed",
        "push_notification",
        "quota_auto_resume_fired",
        "quota_auto_resume_stale",
        "quota_auto_resume_disabled",
        "computer_use_enter",
        "computer_use_exit",
        "model_refusal_fallback",
        "auth_storage_failure",
        "a_type_added_by_a_later_release",
    ] {
        let body = hook(
            &f,
            "Notification",
            json!({"message": "Agent \"Review\" finished", "notification_type": kind}),
        )
        .await?;
        assert!(body.is_null());
        assert_eq!(
            activity(&f).await?,
            Some(Activity::Working),
            "{kind} needs nobody"
        );
    }
    for kind in [
        "permission_prompt",
        "worker_permission_prompt",
        "elicitation_dialog",
        "elicitation_url_dialog",
        "agent_needs_input",
    ] {
        store
            .set_activity(&f.task, Some(Activity::Working), Timestamp(11))
            .await?;
        hook(
            &f,
            "Notification",
            json!({"message": "Claude needs you", "notification_type": kind}),
        )
        .await?;
        assert_eq!(
            activity(&f).await?,
            Some(Activity::NeedsAttention),
            "{kind} asks a person"
        );
    }
    Ok(())
}

/// Stores the GitHub App with installation 7 (the fake's installation token is `inst`).
async fn install_app(f: &Fixture) -> TestResult {
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
    Ok(())
}

/// Saves `user`'s GitHub token, as a GitHub sign-in does (`token-<login>` in the fake).
async fn sign_in_with_github(f: &Fixture, user: &str, login: &str) -> TestResult {
    let access =
        f.h.app
            .deps
            .crypto
            .seal(format!("token-{login}").as_bytes())?;
    f.h.app
        .deps
        .store
        .save_github_token(
            &GitHubTokenRecord {
                user_id: UserId::parse(user)?,
                access,
                refresh: None,
                expires_at: None,
                refresh_expires_at: None,
            },
            Timestamp(1),
        )
        .await?;
    Ok(())
}

/// The token the credential helper gets for the run with task token `token`.
async fn agent_credential(f: &Fixture, token: &str) -> Result<String, Box<dyn std::error::Error>> {
    let resp = call(
        &f.h.internal,
        Method::POST,
        "/internal/git-credential",
        Some(json!({"protocol": "https", "host": "github.com", "path": "acme/fixture.git"})),
        &Creds {
            bearer: Some(token.into()),
            ..Creds::default()
        },
    )
    .await?;
    assert_eq!(resp.status, 200, "{}", resp.body);
    Ok(resp.body["token"].as_str().ok_or("no token")?.to_owned())
}

/// Notices the task's terminal showed since `watch` subscribed.
fn notices(watch: &mut tokio::sync::broadcast::Receiver<HubFrame>) -> Vec<String> {
    let mut seen = Vec::new();
    while let Ok(frame) = watch.try_recv() {
        if let HubFrame::Notice(text) = frame {
            seen.push(text);
        }
    }
    seen
}

/// ADR-0114 (SPEC-141 AC1): the agent's Git and `gh` hold the App's narrowed agent token, never
/// a person's: not before its owner signs in with GitHub, not after, and not for an owner without
/// a linked account. A task with an owner who can act says nothing about the App.
#[tokio::test]
async fn the_agents_git_never_holds_the_owners_token() -> TestResult {
    let f = fixture().await?;
    install_app(&f).await?;
    let mut watch = f.h.app.runtime(&f.task).hub.watch();

    assert_eq!(agent_credential(&f, TOKEN).await?, "agent-inst");
    sign_in_with_github(&f, "u1", "alice").await?;
    assert_eq!(agent_credential(&f, TOKEN).await?, "agent-inst");
    // `yulius gh-token` (the agent's `gh`) asks without a repository path: the same token.
    let gh = call(
        &f.h.internal,
        Method::POST,
        "/internal/git-credential",
        Some(json!({"protocol": "https", "host": "github.com"})),
        &agent(),
    )
    .await?;
    assert_eq!(gh.status, 200);
    assert_eq!(gh.body["token"], "agent-inst");
    assert!(notices(&mut watch).is_empty());

    // A task of an owner without a linked GitHub account gets the same token.
    let store = &f.h.app.deps.store;
    let unlinked = UserRecord {
        id: UserId::parse("u-local")?,
        github_login: None,
        github_id: None,
        display_name: "Local".into(),
        avatar_url: None,
        is_superadmin: false,
        is_instance_admin: false,
        password_hash: None,
        created_at: Timestamp(1),
        disabled_at: None,
    };
    store.insert_user(&unlinked).await?;
    let mut task = store.task(&f.task).await?.ok_or("task")?;
    task.id = TaskId::parse("t-local")?;
    task.number = 3;
    task.created_by = unlinked.id.clone();
    task.owner = unlinked.id.clone();
    store.insert_task(&task).await?;
    let mut attempt = store.live_attempt(&f.task).await?.ok_or("attempt")?;
    attempt.id = AttemptId::parse("at-local")?;
    attempt.task_id = task.id.clone();
    attempt.token_hash = f.h.app.deps.crypto.sha256_hex(b"local-task-token");
    store.insert_attempt(&attempt).await?;
    assert_eq!(
        agent_credential(&f, "local-task-token").await?,
        "agent-inst"
    );

    // Without the App installed there is nothing to hand out: refused, never a person's token.
    store.set_installation(None).await?;
    let none = call(
        &f.h.internal,
        Method::POST,
        "/internal/git-credential",
        Some(json!({"protocol": "https", "host": "github.com", "path": "acme/fixture.git"})),
        &agent(),
    )
    .await?;
    assert_eq!(none.status, 409, "{}", none.body);
    Ok(())
}

#[tokio::test]
async fn git_credentials_are_scoped_to_the_project_repository() -> TestResult {
    let f = fixture().await?;
    install_app(&f).await?;
    sign_in_with_github(&f, "u1", "alice").await?;

    assert_eq!(agent_credential(&f, TOKEN).await?, "agent-inst");
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
    let mut task = f.h.app.deps.store.task(&f.task).await?.ok_or("task")?;
    // The default Feature workflow (catalog/workflows/feature.md) answers to the old id.
    task.workflow_id = Some("wf-feature".into());
    let feature =
        yulius_app::workflows::catalog::default_workflow("wf-feature").ok_or("feature")?;
    let first_line = feature.instructions.lines().next().ok_or("instructions")?;
    let briefing = f.h.app.briefing(&task, Agent::ClaudeCode).await?;
    assert!(briefing.contains("## Workflow instructions"), "{briefing}");
    assert!(briefing.contains(first_line), "{briefing}");
    assert!(briefing.len() <= 12 * 1024 + 64);
    Ok(())
}

/// An isolation adapter that joins nothing: only its presence matters here.
struct Isolated;

#[async_trait::async_trait]
impl yulius_app::ports::isolation::SandboxIsolation for Isolated {
    async fn prepare(
        &self,
        _: &yulius_app::ports::isolation::SandboxRef,
    ) -> yulius_app::error::AppResult<Option<yulius_app::ports::isolation::SandboxJoin>> {
        Ok(None)
    }
    fn release(&self, _: &SandboxId) {}
}

/// SPEC-173 CTR-001 (task #163, #550): the briefing says there is no container engine exactly
/// when the server runs sandboxes isolated (an isolation adapter is installed).
#[tokio::test]
async fn an_isolated_sandboxs_briefing_names_the_missing_container_engine() -> TestResult {
    let f = fixture().await?;
    let task = f.h.app.deps.store.task(&f.task).await?.ok_or("task")?;
    let plain = f.h.app.briefing(&task, Agent::ClaudeCode).await?;
    assert!(!plain.contains("## Containers"), "{plain}");
    assert!(f.h.app.set_isolation(Arc::new(Isolated)));
    let isolated = f.h.app.briefing(&task, Agent::ClaudeCode).await?;
    assert!(isolated.contains("## Containers"), "{isolated}");
    assert!(isolated.contains("`DOCKER_HOST=ssh://"), "{isolated}");
    Ok(())
}

#[tokio::test]
async fn steps_follow_the_spec_plan_checklist() -> TestResult {
    let f = fixture().await?;
    let spec = |implement_done: bool| {
        format!(
            "---\nyulius-task: t-main\nstatus: in-progress\n---\n# Checkout API\n\n\
             ## Goal\n\nCustomers can pay.\n\n## Context\n\nNo checkout yet.\n\n\
             ## Requirements\n\n- Card payments.\n\n## Plan\n\n- [x] Write spec\n- [{}] Implement API\n  - [ ] Validate input\n- [ ] Open PR\n\n\
             ## Acceptance Criteria\n\nA test order succeeds.\n\n## Test plan\n\n```sh\ncargo test\n```\n\n\
             ## Decisions\n\n- Hosted payment page.\n",
            if implement_done { "x" } else { " " }
        )
    };
    std::fs::create_dir_all(f.worktree.join("docs/specs"))?;
    std::fs::write(f.worktree.join("docs/specs/checkout.md"), spec(false))?;
    hook(&f, "Stop", json!({})).await?;
    let steps = f.h.app.deps.store.steps(&f.task).await?;
    let view: Vec<(String, String)> = steps
        .iter()
        .map(|s| (s.title.clone(), s.status.clone()))
        .collect();
    assert_eq!(
        view,
        [
            ("Write spec".to_owned(), "done".to_owned()),
            ("Implement API".to_owned(), "pending".to_owned()),
            ("Open PR".to_owned(), "pending".to_owned()),
        ],
        "top-level plan items become steps"
    );
    let implement_id = steps[1].id.clone();

    // Ticking the box in the spec marks the step done (same step id); the briefing shows it.
    std::fs::write(f.worktree.join("docs/specs/checkout.md"), spec(true))?;
    let body = hook(&f, "SessionStart", json!({"session_id": "s2"})).await?;
    let steps = f.h.app.deps.store.steps(&f.task).await?;
    assert_eq!(steps[1].status, "done");
    assert_eq!(steps[1].id, implement_id);
    assert_eq!(steps[1].updated_by.as_deref(), Some("spec"));
    let briefing = body["hookSpecificOutput"]["additionalContext"]
        .as_str()
        .ok_or("briefing")?;
    assert!(briefing.contains("Implement API"), "{briefing}");
    Ok(())
}

/// F 2026-10-11: an agent renamed its spec (SPEC-005 → SPEC-006 to avoid a clash) and the Stop
/// gate kept demanding the old registered path, which the agent cannot change. The gate follows
/// the spec by its frontmatter and registers the new path.
#[tokio::test]
async fn a_renamed_spec_is_followed_by_its_frontmatter() -> TestResult {
    let f = fixture().await?;
    std::fs::write(f.worktree.join("api.rs"), "fn main() {}\n")?;
    std::fs::create_dir_all(f.worktree.join("docs/specs"))?;
    std::fs::write(
        f.worktree.join("docs/specs/renamed.md"),
        "---\nyulius-task: t-main\nstatus: in-progress\n---\n# Checkout API\n\n\
         ## Goal\n\nCustomers can pay.\n\n## Context\n\nNo checkout yet.\n\n\
         ## Requirements\n\n- Card payments.\n\n## Plan\n\n- [x] Write spec\n- [ ] Implement API\n\n\
         ## Acceptance Criteria\n\nA test order succeeds.\n\n## Test plan\n\n```sh\ncargo test\n```\n\n\
         ## Decisions\n\n- Hosted payment page.\n",
    )?;
    let body = hook(&f, "Stop", json!({})).await?;
    assert!(body.is_null(), "the renamed spec is found: {body:?}");
    let task = f.h.app.deps.store.task(&f.task).await?.ok_or("task")?;
    assert_eq!(task.spec_path.as_deref(), Some("docs/specs/renamed.md"));
    Ok(())
}

/// SPEC-140 HKS-001 (#404): the hook client gives up after `HOOK_TIMEOUT` and lets the agent go on
/// (fail-open), closing its connection, and hyper then drops the request's handler. A hook Yulius
/// received is still applied to its end, whatever becomes of its handler. Here a notice that asks a
/// person: a layer starts its handler and never drives it further (as a dropped handler), and the
/// client hangs up. Before, the hook's work was the handler's, so on a loaded host a Stop that took
/// longer than the client's wait never made its task Done. (That hyper drops the handler when the
/// client hangs up was shown with the layer resuming it after a while: SPEC-140 Evidence.)
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_hook_whose_client_hangs_up_is_still_applied() -> TestResult {
    use tokio::io::AsyncWriteExt;
    let f = fixture().await?;
    f.h.app
        .deps
        .store
        .set_activity(&f.task, Some(Activity::Working), Timestamp(10))
        .await?;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let addr = listener.local_addr()?;
    let (started_tx, mut started) = tokio::sync::watch::channel(false);
    let held = axum::middleware::from_fn(
        move |request: axum::extract::Request, next: axum::middleware::Next| {
            let started_tx = started_tx.clone();
            async move {
                let (parts, body) = request.into_parts();
                let bytes = axum::body::to_bytes(body, 1 << 20)
                    .await
                    .unwrap_or_default();
                let request =
                    axum::extract::Request::from_parts(parts, axum::body::Body::from(bytes));
                let mut handling = std::pin::pin!(next.run(request));
                // The handler starts, and the server takes it no further.
                if let std::task::Poll::Ready(response) = futures::poll!(handling.as_mut()) {
                    return response;
                }
                started_tx.send_replace(true);
                std::future::pending::<()>().await;
                handling.await
            }
        },
    );
    let internal =
        f.h.internal
            .clone()
            .layer(held)
            .into_make_service_with_connect_info::<std::net::SocketAddr>();
    tokio::spawn(async move { axum::serve(listener, internal).await });
    let body = json!({"message": "Claude needs you", "notification_type": "permission_prompt"})
        .to_string();
    let request = format!(
        "POST /hooks/claude/Notification HTTP/1.1\r\nHost: {addr}\r\n\
         Authorization: Bearer {TOKEN}\r\nContent-Type: application/json\r\n\
         Content-Length: {}\r\n\r\n{body}",
        body.len()
    );
    let mut client = tokio::net::TcpStream::connect(addr).await?;
    client.write_all(request.as_bytes()).await?;
    tokio::time::timeout(std::time::Duration::from_secs(10), started.wait_for(|s| *s)).await??;
    drop(client);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while activity(&f).await? != Some(Activity::NeedsAttention) {
        if std::time::Instant::now() > deadline {
            return Err(format!("the notice was not applied: {:?}", activity(&f).await?).into());
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    Ok(())
}
