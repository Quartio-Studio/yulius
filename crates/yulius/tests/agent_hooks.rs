//! Internal agent endpoints over HTTP: hook events (briefing, the Stop
//! discipline gate) and repository-scoped Git credentials, the task creator's own (SPEC-002
//! §7.5, §8; AC-DSC-3, AC-COM-2, AC-COM-4, AC-GIT-3).

use crate::support;
use crate::support::git;

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::Ordering;

use axum::http::Method;
use serde_json::json;
use support::{Creds, Harness, TestResult, call};
use yulius_app::hub::HubFrame;
use yulius_app::model::{
    AttemptRecord, GitHubTokenRecord, LaunchKind, MessageRecord, SandboxRecord, TaskMode,
    TaskOrigin, TaskRecord, UserRecord,
};
use yulius_app::ports::github::GitHubAppConfig;
use yulius_app::ports::session::{
    AttachedRun, LaunchPlan, RunEvent, RunExit, RunInspection, RunReader, RunSignal, RunWelcome,
    RunWriter, SessionHost, SpawnedRun,
};
use yulius_app::{
    context::App,
    error::{AppError, AppResult},
};
use yulius_domain::harness::Harness as Agent;
use yulius_domain::ids::{AttemptId, MessageId, SandboxId, TaskId, UserId};
use yulius_domain::task::{Activity, AttemptState, TaskStatus};
use yulius_domain::time::Timestamp;

const TOKEN: &str = "task-token-for-tests-0123456789abcdef";

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
    fixture_with_task(workflow, |_| {}).await
}

/// [`fixture_with_workflow`] whose main task is changed by `tweak` before it is stored.
async fn fixture_with_task(
    workflow: Option<(&str, bool)>,
    tweak: impl FnOnce(&mut TaskRecord),
) -> Result<Fixture, Box<dyn std::error::Error>> {
    fixture_of(workflow, tweak, Agent::ClaudeCode).await
}

/// [`fixture_with_task`] whose task and live run are of `agent` (SPEC-503).
async fn fixture_of(
    workflow: Option<(&str, bool)>,
    tweak: impl FnOnce(&mut TaskRecord),
    agent: Agent,
) -> Result<Fixture, Box<dyn std::error::Error>> {
    fixture_with_read_budget(workflow, tweak, agent, 10_000).await
}

async fn fixture_with_read_budget(
    workflow: Option<(&str, bool)>,
    tweak: impl FnOnce(&mut TaskRecord),
    agent: Agent,
    read_budget_ms: u64,
) -> Result<Fixture, Box<dyn std::error::Error>> {
    let h = support::harness_with(support::HarnessOptions {
        git: Some(Arc::new(support::confined_git())),
        session_host: Some(Arc::new(IdleSessionHost)),
        configure: Some(Box::new(move |config| {
            config.parked_pr_read_ms = read_budget_ms
        })),
        ..Default::default()
    })?;
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
            resume_attempt_id: None,
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
    main.harness = agent;
    tweak(&mut main);
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
            provider: agent.provider(),
            label: "Claude #1".into(),
            credential: Some(
                h.app
                    .deps
                    .crypto
                    .seal(concat!("sk-ant-oat01-", "hooks-test").as_bytes())?,
            ),
            status: yulius_app::model::AccountStatus::Ok,
            usage_json: None,
            last_error: None,
            created_at: now,
            updated_at: now,
            email: None,
            email_checked_at: None,
            email_typed: false,
        })
        .await?;
    let hash = h.app.deps.crypto.sha256_hex(TOKEN.as_bytes());
    store
        .insert_attempt(&AttemptRecord {
            id: AttemptId::parse("at1")?,
            task_id: main.id.clone(),
            seq: 1,
            harness: agent,
            account_id: Some(account),
            provider_session_id: None,
            launch_kind: LaunchKind::New,
            model: None,
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

/// SPEC-363 (task #346): a project whose change notes are a folder, one file per pull request,
/// is told to add one new file there instead of an entry at the top of a shared log, and its
/// Recent changes quote the newest notes.
#[tokio::test]
async fn a_changes_folder_briefs_one_new_file_per_change() -> TestResult {
    let f = fixture().await?;
    std::fs::write(
        f.worktree.join("yulius.toml"),
        "[docs]\nchanges = \"docs/changes/\"\n",
    )?;
    let notes = f.worktree.join("docs/changes");
    std::fs::create_dir_all(&notes)?;
    std::fs::write(notes.join("README.md"), "# One note per pull request\n")?;
    std::fs::write(notes.join("2026-10-30-480-search.md"), "# Search by tag\n")?;
    std::fs::write(
        notes.join("2026-10-31-488-notes.md"),
        "# Change notes as files\n",
    )?;
    let body = hook(
        &f,
        "SessionStart",
        json!({"session_id": "sess-1", "hook_event_name": "SessionStart"}),
    )
    .await?;
    let context = body["hookSpecificOutput"]["additionalContext"]
        .as_str()
        .ok_or("no additionalContext")?;
    assert!(
        context.contains(
            "Add one new file to `docs/changes/` for your change, named \
             `YYYY-MM-DD-1-<short-slug>.md`"
        ),
        "{context}"
    );
    assert!(
        context.contains("never edit other tasks' files"),
        "{context}"
    );
    assert!(!context.contains("entry at the top"), "{context}");
    let recent = &context[context.find("## Recent changes").ok_or("recent")?..];
    let newest = recent.find("# Change notes as files").ok_or(recent)?;
    let older = recent.find("# Search by tag").ok_or(recent)?;
    assert!(newest < older, "{recent}");
    assert!(
        recent.contains("From the newest notes in `docs/changes/`:"),
        "{recent}"
    );
    assert!(!recent.contains("One note per pull request"), "{recent}");
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

/// SPEC-505 AGY-006: only an Antigravity run's Stop of a failed turn skips the gate (its
/// `error`; `antigravity::a_failed_antigravity_turn_leaves_its_task_waiting`): the same field
/// from another harness's hook changes nothing.
#[tokio::test]
async fn only_antigravity_skips_the_gate_for_a_failed_turn() -> TestResult {
    let f = fixture().await?;
    std::fs::write(f.worktree.join("api.rs"), "fn main() {}\n")?;
    let body = hook(
        &f,
        "Stop",
        json!({"stop_hook_active": false, "error": "Error 503, Status: UNAVAILABLE"}),
    )
    .await?;
    assert_eq!(body["decision"], "block", "{body:?}");
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

/// A registered link is found by its path, but the Stop gate needs a regular file.
#[tokio::test]
async fn the_stop_gate_explains_a_linked_spec() -> TestResult {
    let f = fixture().await?;
    std::fs::create_dir_all(f.worktree.join("docs/specs"))?;
    std::fs::write(f.worktree.join("docs/specs/real.md"), "# Draft\n")?;
    std::os::unix::fs::symlink("real.md", f.worktree.join("docs/specs/checkout.md"))?;
    std::fs::write(f.worktree.join("api.rs"), "fn main() {}\n")?;
    let body = hook(&f, "Stop", json!({})).await?;
    let reason = body["reason"]
        .as_str()
        .ok_or("a linked spec blocks the Stop")?;
    assert!(
        reason.contains("link") && reason.contains("regular file"),
        "{reason}"
    );
    Ok(())
}

/// A Plan may cross the read bound, even in the middle of a UTF-8 character. No gate
/// judges a prefix and a refresh preserves the previously recorded plan.
#[tokio::test]
async fn oversized_specs_are_reported_and_never_sync_partial_steps() -> TestResult {
    let f = fixture().await?;
    let dir = f.worktree.join("docs/specs");
    std::fs::create_dir_all(&dir)?;
    let path = dir.join("checkout.md");
    let task = f.h.app.deps.store.task(&f.task).await?.ok_or("task")?;
    let header = "---\nyulius-task: t-main\nstatus: draft\n---\n# Checkout\n\n## Plan\n\n";
    std::fs::write(&path, format!("{header}- [ ] Original plan\n"))?;
    assert!(f.h.app.sync_steps_from_spec(&task).await?);
    std::fs::write(f.worktree.join("api.rs"), "fn main() {}\n")?;
    let mut prefix = format!("{header}- [ ] Incomplete replacement\n");
    prefix.push_str(&" ".repeat(256 * 1024 - 1 - prefix.len()));
    for tail in ["x\n- [ ] Past the cut\n", "é\n- [ ] Past the cut\n"] {
        std::fs::write(&path, format!("{prefix}{tail}"))?;
        let sync = f.h.app.sync_steps_from_spec(&task).await;
        assert!(
            sync.is_err_and(|e| e.to_string().contains("too large")),
            "sync must report the read limit"
        );
        let steps = f.h.app.deps.store.steps(&f.task).await?;
        assert_eq!(steps.len(), 1);
        assert_eq!(steps[0].title, "Original plan");
        let body = hook(&f, "Stop", json!({})).await?;
        let reason = body["reason"]
            .as_str()
            .ok_or("oversized spec blocks the Stop")?;
        assert!(reason.contains("too large"), "{reason}");
        assert_eq!(
            pre_tool_use(&f, "Edit", json!({"file_path": file_of(&f, "api.rs")})).await?,
            None
        );
    }
    Ok(())
}

#[tokio::test]
async fn the_edit_gate_judges_an_exactly_full_spec_and_a_removed_spec() -> TestResult {
    let f = fixture().await?;
    std::fs::create_dir_all(f.worktree.join("docs/specs"))?;
    let path = f.worktree.join("docs/specs/checkout.md");
    let file = file_of(&f, "api.rs");
    std::fs::write(&path, vec![b' '; 256 * 1024])?;
    assert!(
        pre_tool_use(&f, "Edit", json!({"file_path": file}))
            .await?
            .is_some(),
        "the whole file fits and has no Plan"
    );
    std::fs::write(&path, "## Plan\n\n- [ ] This plan would admit the edit\n")?;
    let removed = path.clone();
    yulius_app::race::on(yulius_app::race::Point::RepoFileRead, &path, move || {
        assert!(std::fs::remove_file(removed).is_ok());
    });
    assert!(
        pre_tool_use(&f, "Edit", json!({"file_path": file}))
            .await?
            .is_some(),
        "the registered spec vanished before the read"
    );
    assert!(!path.exists());
    Ok(())
}

/// The gate gives up on the fixture's task: code changed without a spec, three blocks, then a
/// Stop that passes and flags the task.
async fn gate_gives_up(f: &Fixture) -> TestResult {
    std::fs::write(f.worktree.join("api.rs"), "fn main() {}\n")?;
    for n in 1..=3 {
        let body = hook(f, "Stop", json!({"stop_hook_active": n > 1})).await?;
        assert_eq!(body["decision"], "block", "block {n}");
    }
    let body = hook(f, "Stop", json!({"stop_hook_active": true})).await?;
    assert!(body.is_null(), "{body:?}");
    assert_eq!(activity(f).await?, Some(Activity::NeedsAttention));
    Ok(())
}

/// SPEC-472 ATN-001..ATN-004 (#1221): a flag the gate raised says why to whoever opens the task
/// later, also after a restart of Yulius. Its notice reaches only the viewers connected then.
#[tokio::test]
async fn a_flag_the_gate_raised_says_why_to_whoever_comes_later() -> TestResult {
    let f = fixture().await?;
    let mut live = f.h.app.runtime(&f.task).hub.watch();
    gate_gives_up(&f).await?;
    let said = notices(&mut live);
    assert!(
        said.iter()
            .any(|n| n.contains("stopped 3 times without fixing")),
        "{said:?}"
    );
    // Whoever opens the task afterwards gets no notice: the task itself says why.
    let mut later = f.h.app.runtime(&f.task).hub.subscribe().rx;
    assert!(notices(&mut later).is_empty());
    let why = support::attention(&f.h.app, "t-main").await?;
    assert_eq!(why["kind"], "yulius", "{why}");
    let problem = why["problem"].as_str().unwrap_or_default();
    assert!(
        problem.starts_with("The agent stopped 3 times without fixing: ")
            && problem.contains("docs/specs/checkout.md"),
        "{why}"
    );
    assert_eq!(
        why["action"], "Tell it how to go on, in the terminal.",
        "{why}"
    );
    assert!(why["link"].is_null(), "{why}");

    // A restart of Yulius: another app on the same database, with no memory of the run.
    let again = support::harness_with(support::HarnessOptions {
        store: Some(f.h.store.clone()),
        ..support::HarnessOptions::default()
    })?;
    assert_eq!(support::attention(&again.app, "t-main").await?, why);

    // The person answers: the task works again and the reason is gone with its flag.
    f.h.clock.0.fetch_add(1_000, Ordering::SeqCst);
    hook(
        &f,
        "UserPromptSubmit",
        json!({"prompt": "write the spec first"}),
    )
    .await?;
    assert_eq!(activity(&f).await?, Some(Activity::Working));
    assert!(support::attention(&f.h.app, "t-main").await?.is_null());
    // A flag raised afterwards by a path that gives no reason shows none, not the old one.
    f.h.clock.0.fetch_add(1_000, Ordering::SeqCst);
    f.h.app
        .deps
        .store
        .set_activity(&f.task, Some(Activity::NeedsAttention), f.h.app.now())
        .await?;
    assert!(support::attention(&f.h.app, "t-main").await?.is_null());
    Ok(())
}

/// SPEC-472 ATN-005: a flag raised by a Claude Code notice says the question is in the terminal,
/// in Claude Code's own words (one line); a notice without words says only that; the idle
/// reminder flags nothing and says nothing.
#[tokio::test]
async fn a_notices_flag_carries_claude_codes_words() -> TestResult {
    let f = fixture().await?;
    let why = || support::attention(&f.h.app, "t-main");
    hook(&f, "UserPromptSubmit", json!({"prompt": "go"})).await?;
    hook(
        &f,
        "Notification",
        json!({"notification_type": "idle_prompt", "message": "Claude is waiting for your input"}),
    )
    .await?;
    assert!(why().await?.is_null());
    // The input as Claude Code 2.1.322 sends it for a permission dialog left open for six
    // seconds (read from the real CLI run against a local stand-in for the API; the ids and
    // paths here are made up).
    hook(
        &f,
        "Notification",
        json!({
            "session_id": "0f5065b9-0000-4000-8000-000000000001",
            "transcript_path": "/home/agent/.claude/projects/-work/0f5065b9.jsonl",
            "cwd": "/work",
            "prompt_id": "670bf19d-0000-4000-8000-000000000002",
            "hook_event_name": "Notification",
            "message": "Claude needs your permission",
            "notification_type": "permission_prompt",
        }),
    )
    .await?;
    assert_eq!(activity(&f).await?, Some(Activity::NeedsAttention));
    assert_eq!(
        why().await?,
        json!({
            "kind": "terminal",
            "problem": "Claude needs your permission",
            "action": null,
            "link": null,
        })
    );

    // The person's next prompt ends the flag and its reason. Words over several lines are one
    // line in the bar.
    f.h.clock.0.fetch_add(1_000, Ordering::SeqCst);
    hook(&f, "UserPromptSubmit", json!({"prompt": "go on"})).await?;
    assert!(why().await?.is_null());
    f.h.clock.0.fetch_add(1_000, Ordering::SeqCst);
    hook(
        &f,
        "Notification",
        json!({
            "notification_type": "elicitation_dialog",
            "message": "Claude Code needs\n  your input",
        }),
    )
    .await?;
    assert_eq!(
        why().await?["problem"],
        "Claude Code needs your input",
        "one line"
    );

    // A notice that brings no words still says where the question is.
    f.h.clock.0.fetch_add(1_000, Ordering::SeqCst);
    hook(&f, "UserPromptSubmit", json!({"prompt": "go on"})).await?;
    f.h.clock.0.fetch_add(1_000, Ordering::SeqCst);
    hook(
        &f,
        "Notification",
        json!({"notification_type": "permission_prompt"}),
    )
    .await?;
    assert_eq!(
        why().await?,
        json!({"kind": "terminal", "problem": null, "action": null, "link": null})
    );
    Ok(())
}

/// SPEC-472 ATN-002: one flag, one reason, the first. A Claude Code notice on a task the gate
/// flagged changes neither the flag nor what it says: the gate's matter still stands once the
/// notice is answered.
#[tokio::test]
async fn a_notice_on_a_flagged_task_keeps_the_flags_reason() -> TestResult {
    let f = fixture().await?;
    gate_gives_up(&f).await?;
    let why = support::attention(&f.h.app, "t-main").await?;
    assert_eq!(why["kind"], "yulius", "{why}");
    f.h.clock.0.fetch_add(1_000, Ordering::SeqCst);
    hook(
        &f,
        "Notification",
        json!({"notification_type": "permission_prompt", "message": "Claude needs your permission to use Bash"}),
    )
    .await?;
    assert_eq!(support::attention(&f.h.app, "t-main").await?, why);
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

struct IsolatedWithDocker;

#[async_trait::async_trait]
impl yulius_app::ports::isolation::SandboxIsolation for IsolatedWithDocker {
    async fn prepare(
        &self,
        _: &yulius_app::ports::isolation::SandboxRef,
    ) -> yulius_app::error::AppResult<Option<yulius_app::ports::isolation::SandboxJoin>> {
        Ok(None)
    }
    fn release(&self, _: &SandboxId) {}
    async fn docker(&self, _: &yulius_app::ports::isolation::SandboxRef) -> bool {
        true
    }
}

/// SPEC-187 DKR-010: with an engine per sandbox the briefing says Docker works, and no longer
/// that there is no engine.
#[tokio::test]
async fn a_sandbox_with_an_engine_is_told_docker_works() -> TestResult {
    let f = fixture().await?;
    let task = f.h.app.deps.store.task(&f.task).await?.ok_or("task")?;
    assert!(f.h.app.set_isolation(Arc::new(IsolatedWithDocker)));
    let text = f.h.app.briefing(&task, Agent::ClaudeCode).await?;
    assert!(text.contains("## Containers"), "{text}");
    assert!(text.contains("Docker works here"), "{text}");
    assert!(!text.contains("No container engine"), "{text}");
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

/// SPEC-484 PLN-004, PLN-005, AC-11: a spec whose Plan lists every slice of a long task, 36 of
/// them with their parts nested, five ticked, gives 36 steps (30 was the bound, and a longer
/// Plan was cut). What the agent is told about the plan stands before the list in the
/// briefing, where a long list cannot push it out, and nothing there tells it to merge slices.
#[tokio::test]
async fn a_plan_of_every_slice_becomes_steps_and_is_not_told_to_shrink() -> TestResult {
    let f = fixture().await?;
    let plan: String = (1..=36)
        .map(|n| {
            format!(
                "- [{}] Slice {n}: the findings of area {n} fixed and reviewed\n  - [{0}] its fixes, test-first\n  - [{0}] its review answered\n",
                if n <= 5 { "x" } else { " " }
            )
        })
        .collect();
    let spec = format!(
        "---\nyulius-task: t-main\nstatus: in-progress\n---\n# Audit\n\n\
         ## Goal\n\nEvery finding fixed.\n\n## Context\n\nThirty-six slices, one pull request each.\n\n\
         ## Requirements\n\n- Each finding has a failing test first.\n\n## Plan\n\n{plan}\n\
         ## Acceptance Criteria\n\nThe ledger is empty.\n\n## Test plan\n\n```sh\ncargo test\n```\n\n\
         ## Decisions\n\n- One pull request per slice.\n"
    );
    std::fs::create_dir_all(f.worktree.join("docs/specs"))?;
    // The fixture's task spec.
    std::fs::write(f.worktree.join("docs/specs/checkout.md"), spec)?;
    let body = hook(&f, "SessionStart", json!({"session_id": "s2"})).await?;
    let steps = f.h.app.deps.store.steps(&f.task).await?;
    assert_eq!(steps.len(), 36, "every slice is a step; its parts are not");
    assert_eq!(steps.iter().filter(|s| s.status == "done").count(), 5);
    assert_eq!(
        steps[35].title,
        "Slice 36: the findings of area 36 fixed and reviewed"
    );
    let briefing = body["hookSpecificOutput"]["additionalContext"]
        .as_str()
        .ok_or("briefing")?;
    let header = briefing
        .find("Steps (5/36 done, 13 %; current: 6. Slice 6:")
        .ok_or_else(|| format!("no header: {briefing}"))?;
    let nudges = briefing
        .find("Nudges:\n- No step is in progress. Mark \"Slice 6:")
        .ok_or_else(|| format!("no nudges: {briefing}"))?;
    let list = briefing
        .find("1. [done] Slice 1:")
        .ok_or_else(|| format!("no list: {briefing}"))?;
    assert!(header < nudges && nudges < list, "{briefing}");
    // The one nudge of this plan: none about its length.
    let told: Vec<&str> = briefing[nudges..list].lines().skip(1).collect();
    assert_eq!(told.len(), 1, "{told:?}");
    Ok(())
}

/// SPEC-341 LSS-002: the spec sync now also runs while a turn goes on, next to the agent's step
/// tools. It waits for the task's steps lock and reads the steps only then, so a status changed
/// while it waited is kept, never written back to the one before.
#[tokio::test]
async fn the_spec_sync_waits_for_the_steps_lock_and_keeps_a_concurrent_update() -> TestResult {
    use yulius_app::ports::store::TaskFieldUpdate;
    let f = fixture().await?;
    std::fs::create_dir_all(f.worktree.join("docs/specs"))?;
    std::fs::write(
        f.worktree.join("docs/specs/checkout.md"),
        "---\nyulius-task: t-main\nstatus: in-progress\n---\n# Checkout API\n\n\
         ## Goal\n\nCustomers can pay.\n\n## Context\n\nNo checkout yet.\n\n\
         ## Requirements\n\n- Card payments.\n\n## Plan\n\n- [x] Write spec\n- [ ] Implement API\n\n\
         ## Acceptance Criteria\n\nA test order succeeds.\n\n## Test plan\n\n```sh\ncargo test\n```\n\n\
         ## Decisions\n\n- Hosted payment page.\n",
    )?;
    let store = &f.h.app.deps.store;
    let update = TaskFieldUpdate {
        spec_path: Some("docs/specs/checkout.md".into()),
        ..TaskFieldUpdate::default()
    };
    store.update_task(&f.task, &update, Timestamp(5)).await?;
    let task = store.task(&f.task).await?.ok_or("task")?;
    assert!(f.h.app.sync_steps_from_spec(&task).await?);
    let implement = store.steps(&f.task).await?[1].id.clone();

    // The Plan grows while a step tool holds the lock and starts "Implement API".
    std::fs::write(
        f.worktree.join("docs/specs/checkout.md"),
        std::fs::read_to_string(f.worktree.join("docs/specs/checkout.md"))?.replace(
            "- [ ] Implement API\n",
            "- [ ] Implement API\n- [ ] Open PR\n",
        ),
    )?;
    let runtime = f.h.app.runtime(&f.task);
    let held = runtime.steps_lock.lock().await;
    let app = f.h.app.clone();
    let sync_task = task.clone();
    let sync = tokio::spawn(async move { app.sync_steps_from_spec(&sync_task).await });
    // While the lock is held the sync writes nothing: without the lock it writes the grown Plan
    // within milliseconds, so half a second of the old list shows it waits.
    let held_until = tokio::time::Instant::now() + std::time::Duration::from_millis(500);
    while tokio::time::Instant::now() < held_until {
        assert_eq!(
            store.steps(&f.task).await?.len(),
            2,
            "the sync waits for the lock"
        );
        assert!(!sync.is_finished(), "the sync waits for the steps lock");
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    store
        .update_step(&implement, "in_progress", None, "agent", Timestamp(6))
        .await?;
    drop(held);
    assert!(sync.await??, "the grown Plan is synced");
    let view: Vec<(String, String)> = store
        .steps(&f.task)
        .await?
        .into_iter()
        .map(|s| (s.title, s.status))
        .collect();
    assert_eq!(
        view,
        [
            ("Write spec".to_owned(), "done".to_owned()),
            ("Implement API".to_owned(), "in_progress".to_owned()),
            ("Open PR".to_owned(), "pending".to_owned()),
        ],
        "the update made while the sync waited is kept"
    );
    Ok(())
}

/// F 2026-10-11: an agent renamed its spec (SPEC-005 → SPEC-006 to avoid a clash) and the Stop
/// gate kept demanding the old registered path, which the agent cannot change. The gate follows
/// the spec by its frontmatter and registers the new path.
#[tokio::test]
async fn a_renamed_spec_is_followed_by_its_frontmatter() -> TestResult {
    let f = fixture().await?;
    std::fs::write(f.worktree.join("api.rs"), "fn main() {}\n")?;
    std::fs::write(
        f.worktree.join("yulius.toml"),
        "[docs]\nspecs_dir = \"docs/plans\"\n",
    )?;
    std::fs::create_dir_all(f.worktree.join("docs/plans"))?;
    std::fs::write(
        f.worktree.join("docs/plans/renamed.md"),
        "---\nyulius-task: t-main\nstatus: in-progress\n---\n# Checkout API\n\n\
         ## Goal\n\nCustomers can pay.\n\n## Context\n\nNo checkout yet.\n\n\
         ## Requirements\n\n- Card payments.\n\n## Plan\n\n- [x] Write spec\n- [ ] Implement API\n\n\
         ## Acceptance Criteria\n\nA test order succeeds.\n\n## Test plan\n\n```sh\ncargo test\n```\n\n\
         ## Decisions\n\n- Hosted payment page.\n",
    )?;
    let body = hook(&f, "Stop", json!({})).await?;
    assert!(body.is_null(), "the renamed spec is found: {body:?}");
    let task = f.h.app.deps.store.task(&f.task).await?.ok_or("task")?;
    assert_eq!(task.spec_path.as_deref(), Some("docs/plans/renamed.md"));
    Ok(())
}

/// Discovery read only the first 500 directory entries, in directory order, so in a specs
/// directory of 600+ files a task lost its spec. The task's two specs are written before and after
/// 700 others, so one of them lies past the first 500 whether the directory lists oldest or newest
/// first. Discovery lists every name and reads the one named with the task's branch slug first.
#[tokio::test]
async fn a_spec_among_hundreds_is_found_and_the_slug_named_one_wins() -> TestResult {
    let f = fixture().await?;
    std::fs::write(f.worktree.join("api.rs"), "fn main() {}\n")?;
    std::fs::write(
        f.worktree.join("yulius.toml"),
        "[docs]\nspecs_dir = \"docs/plans\"\n",
    )?;
    let plans = f.worktree.join("docs/plans");
    std::fs::create_dir_all(&plans)?;
    let spec = "---\nyulius-task: t-main\nstatus: in-progress\n---\n# Checkout API\n\n\
         ## Goal\n\nCustomers can pay.\n\n## Context\n\nNo checkout yet.\n\n\
         ## Requirements\n\n- Card payments.\n\n## Plan\n\n- [x] Write spec\n- [ ] Implement API\n\n\
         ## Acceptance Criteria\n\nA test order succeeds.\n\n## Test plan\n\n```sh\ncargo test\n```\n\n\
         ## Decisions\n\n- Hosted payment page.\n";
    // The branch is `feat/1-x`: `SPEC-900-x.md` is named with its slug, `0-old-copy.md` sorts first.
    std::fs::write(plans.join("0-old-copy.md"), spec)?;
    for n in 1..=700 {
        std::fs::write(
            plans.join(format!("SPEC-{n:03}-other.md")),
            "---\nyulius-task: someone-else\nstatus: done\n---\n",
        )?;
    }
    std::fs::write(plans.join("SPEC-900-x.md"), spec)?;
    let body = hook(&f, "Stop", json!({})).await?;
    assert!(body.is_null(), "the spec is found: {body:?}");
    let task = f.h.app.deps.store.task(&f.task).await?.ok_or("task")?;
    assert_eq!(task.spec_path.as_deref(), Some("docs/plans/SPEC-900-x.md"));

    // Renamed again, without the slug: still found among the 700.
    std::fs::remove_file(plans.join("SPEC-900-x.md"))?;
    let body = hook(&f, "Stop", json!({})).await?;
    assert!(body.is_null(), "the renamed spec is found: {body:?}");
    let task = f.h.app.deps.store.task(&f.task).await?.ok_or("task")?;
    assert_eq!(task.spec_path.as_deref(), Some("docs/plans/0-old-copy.md"));
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

fn lineage_entry(
    f: &Fixture,
    task: &str,
    session: &str,
    at: i64,
) -> Result<yulius_app::model::ProviderSessionRecord, Box<dyn std::error::Error>> {
    Ok(yulius_app::model::ProviderSessionRecord {
        id: format!("ps-{task}-{session}"),
        sandbox_id: f.sandbox.clone(),
        task_id: TaskId::parse(task)?,
        harness: Agent::ClaudeCode,
        session_id: session.into(),
        transcript_ref: None,
        origin: "native".into(),
        first_seen: Timestamp(at),
        last_seen: Timestamp(at),
    })
}

async fn lineage_of(f: &Fixture, task: &str) -> Result<Vec<String>, Box<dyn std::error::Error>> {
    Ok(f.h
        .app
        .deps
        .store
        .provider_sessions(&TaskId::parse(task)?)
        .await?
        .into_iter()
        .map(|s| s.session_id)
        .collect())
}

async fn attempt_session(f: &Fixture) -> Result<Option<String>, Box<dyn std::error::Error>> {
    Ok(f.h
        .app
        .deps
        .store
        .live_attempt(&f.task)
        .await?
        .ok_or("attempt")?
        .provider_session_id)
}

/// SPEC-281 SSN-001: the TUI's `SessionStart` naming another session (`/clear`) makes it the
/// attempt's and records it in the lineage at once, so a restart before the supervisor's next
/// drift check cannot lose it.
#[tokio::test]
async fn session_start_with_another_session_adopts_it_at_once() -> TestResult {
    let f = fixture().await?;
    let store = &f.h.app.deps.store;
    store
        .upsert_provider_session(&lineage_entry(&f, "t-main", "sess-1", 1)?)
        .await?;
    store
        .set_attempt_session(&AttemptId::parse("at1")?, "sess-1")
        .await?;
    hook(
        &f,
        "SessionStart",
        json!({"session_id": "sess-2", "hook_event_name": "SessionStart", "source": "clear"}),
    )
    .await?;
    assert_eq!(attempt_session(&f).await?.as_deref(), Some("sess-2"));
    assert_eq!(lineage_of(&f, "t-main").await?, ["sess-1", "sess-2"]);
    Ok(())
}

/// SPEC-281 SSN-001: a session id the hook reports is a path component later; one Yulius does not
/// accept (a traversal, an option, too long) changes nothing.
#[tokio::test]
async fn session_start_ignores_an_invalid_session_id() -> TestResult {
    let f = fixture().await?;
    f.h.app
        .deps
        .store
        .set_attempt_session(&AttemptId::parse("at1")?, "sess-1")
        .await?;
    for bad in ["../x", "-rf", "a/b", &"a".repeat(129)] {
        hook(
            &f,
            "SessionStart",
            json!({"session_id": bad, "hook_event_name": "SessionStart", "source": "clear"}),
        )
        .await?;
        assert_eq!(
            attempt_session(&f).await?.as_deref(),
            Some("sess-1"),
            "{bad}"
        );
    }
    assert!(lineage_of(&f, "t-main").await?.is_empty());
    Ok(())
}

/// SPEC-281, HRN-004 "without corrupting another task": a hook naming another task's session (its
/// id is in the sandbox's shared Claude directory) is refused: neither this attempt nor the other
/// task's lineage changes.
#[tokio::test]
async fn session_start_refuses_another_tasks_session() -> TestResult {
    let f = fixture().await?;
    let store = &f.h.app.deps.store;
    store
        .upsert_provider_session(&lineage_entry(&f, "t-main", "sess-1", 1)?)
        .await?;
    store
        .upsert_provider_session(&lineage_entry(&f, "t-other", "other-1", 2)?)
        .await?;
    store
        .upsert_provider_session(&lineage_entry(&f, "t-other", "other-2", 3)?)
        .await?;
    store
        .set_attempt_session(&AttemptId::parse("at1")?, "sess-1")
        .await?;
    hook(
        &f,
        "SessionStart",
        json!({"session_id": "other-1", "hook_event_name": "SessionStart", "source": "resume"}),
    )
    .await?;
    assert_eq!(attempt_session(&f).await?.as_deref(), Some("sess-1"));
    assert_eq!(lineage_of(&f, "t-main").await?, ["sess-1"]);
    assert_eq!(lineage_of(&f, "t-other").await?, ["other-1", "other-2"]);
    Ok(())
}

/// SPEC-358 MSB-007 (review round 2): an answered Stop is kept in the run directory, so a
/// restarted server does not wake the turn it settled.
#[tokio::test]
async fn an_answered_stop_is_kept_in_the_run_directory() -> TestResult {
    let f = fixture().await?;
    let run = f._repo.path().join("run");
    std::fs::create_dir_all(&run)?;
    assert_eq!(yulius_app::outage::stop_answered_at(&run), 0);
    let before = f.h.app.now().0;
    hook(&f, "Stop", json!({})).await?;
    let answered = yulius_app::outage::stop_answered_at(&run);
    assert!(answered >= before && answered > 0, "{answered}");
    Ok(())
}

/// `rel` in `f`'s worktree, as the agent names it.
fn file_of(f: &Fixture, rel: &str) -> String {
    f.worktree.join(rel).to_string_lossy().into_owned()
}

/// What `PreToolUse` answered for `tool` on `input`: the deny reason, or `None` when it passed.
async fn pre_tool_use(
    f: &Fixture,
    tool: &str,
    input: serde_json::Value,
) -> Result<Option<String>, Box<dyn std::error::Error>> {
    let body = hook(
        f,
        "PreToolUse",
        json!({"hook_event_name": "PreToolUse", "tool_name": tool, "tool_input": input}),
    )
    .await?;
    if body.is_null() {
        return Ok(None);
    }
    let out = &body["hookSpecificOutput"];
    assert_eq!(out["hookEventName"], "PreToolUse", "{body:?}");
    assert_eq!(out["permissionDecision"], "deny", "{body:?}");
    Ok(Some(
        out["permissionDecisionReason"]
            .as_str()
            .ok_or("reason")?
            .to_owned(),
    ))
}

/// SPEC-426 EDG-002/003 (AC1, AC2): a code edit of a task that needs a spec waits until the spec
/// has a Plan; documentation, reads and files outside the worktree are never held.
#[tokio::test]
async fn a_code_edit_waits_for_the_spec_plan() -> TestResult {
    let f = fixture().await?;
    let file = |rel: &str| f.worktree.join(rel).to_string_lossy().into_owned();
    let reason = pre_tool_use(&f, "Edit", json!({"file_path": file("api.rs")}))
        .await?
        .ok_or("a code edit without a spec is denied")?;
    assert!(
        reason.contains("docs/specs/checkout.md")
            && reason.contains("Plan")
            && reason.contains("yulius-task: t-main")
            && reason.contains("spec_check"),
        "{reason}"
    );
    for (tool, input) in [
        (
            "Write",
            json!({"file_path": file("docs/specs/checkout.md")}),
        ),
        ("Edit", json!({"file_path": file("README.md")})),
        ("Read", json!({"file_path": file("api.rs")})),
        ("Write", json!({"file_path": "/elsewhere/api.rs"})),
    ] {
        assert_eq!(
            pre_tool_use(&f, tool, input.clone()).await?,
            None,
            "{tool} {input}"
        );
    }
    for (tool, input) in [
        ("Write", json!({"file_path": file("docs/../api.rs")})),
        ("MultiEdit", json!({"file_path": file("src/lib.rs")})),
        (
            "NotebookEdit",
            json!({"notebook_path": file("probe.ipynb")}),
        ),
    ] {
        assert!(
            pre_tool_use(&f, tool, input.clone()).await?.is_some(),
            "{tool} {input}"
        );
    }

    // A spec without a Plan item still holds the edit; one Plan item lets it through.
    std::fs::create_dir_all(f.worktree.join("docs/specs"))?;
    std::fs::write(
        f.worktree.join("docs/specs/checkout.md"),
        "---\nyulius-task: t-main\nstatus: draft\n---\n# Checkout API\n\n## Goal\n\nPay.\n",
    )?;
    assert!(
        pre_tool_use(&f, "Edit", json!({"file_path": file("api.rs")}))
            .await?
            .is_some(),
        "no Plan yet"
    );
    std::fs::write(
        f.worktree.join("docs/specs/checkout.md"),
        "---\nyulius-task: t-main\nstatus: draft\n---\n# Checkout API\n\n## Goal\n\nPay.\n\n\
         ## Plan\n\n- [ ] Card payments work end to end\n",
    )?;
    assert_eq!(
        pre_tool_use(&f, "Edit", json!({"file_path": file("api.rs")})).await?,
        None,
        "a draft spec with a Plan is enough"
    );
    Ok(())
}

/// SPEC-426 AC3: a task whose workflow needs no spec (research) and an onboarding task edit code
/// freely.
#[tokio::test]
async fn edits_without_a_spec_gate_pass() -> TestResult {
    let research = fixture_with_workflow(Some(("wf-research-test", false))).await?;
    let onboarding = fixture_with_task(None, |task| task.origin = TaskOrigin::Onboarding).await?;
    for f in [&research, &onboarding] {
        let path = f.worktree.join("probe.rs");
        assert_eq!(
            pre_tool_use(f, "Write", json!({"file_path": path.to_string_lossy()})).await?,
            None
        );
    }
    Ok(())
}

/// SPEC-426 EDG-002 (review round 1): a relative path is placed from the agent's folder (`cwd`),
/// a path through a link to the worktree is the worktree's, a task without a spec path is told
/// the specs directory, and a Plan of nested items only is no Plan.
#[tokio::test]
async fn edits_are_placed_from_the_agents_folder_and_through_links() -> TestResult {
    let f = fixture().await?;
    let cwd = |rel: &str| f.worktree.join(rel).to_string_lossy().into_owned();
    std::fs::create_dir_all(f.worktree.join("src"))?;
    let placed = |dir: &str, file: &str| {
        json!({"hook_event_name": "PreToolUse", "tool_name": "Write",
               "tool_input": {"file_path": file}, "cwd": cwd(dir)})
    };
    let body = hook(&f, "PreToolUse", placed("docs", "diagram.svg")).await?;
    assert!(
        body.is_null(),
        "docs/diagram.svg is documentation: {body:?}"
    );
    let body = hook(&f, "PreToolUse", placed("src", "lib.rs")).await?;
    assert_eq!(body["hookSpecificOutput"]["permissionDecision"], "deny");
    let body = hook(&f, "PreToolUse", placed("src", "../docs/x.svg")).await?;
    assert!(body.is_null(), "{body:?}");

    let link = f.worktree.with_file_name("wt-link");
    std::os::unix::fs::symlink(&f.worktree, &link)?;
    let through = link.join("api.rs").to_string_lossy().into_owned();
    assert!(
        pre_tool_use(&f, "Edit", json!({"file_path": through}))
            .await?
            .is_some(),
        "an edit through a link to the worktree is held"
    );

    std::fs::create_dir_all(f.worktree.join("docs/specs"))?;
    std::fs::write(
        f.worktree.join("docs/specs/checkout.md"),
        "---\nyulius-task: t-main\nstatus: draft\n---\n# Checkout\n\n## Plan\n\n  - [ ] nested only\n",
    )?;
    assert!(
        pre_tool_use(&f, "Edit", json!({"file_path": cwd("api.rs")}))
            .await?
            .is_some(),
        "nested items are no Plan"
    );

    let unnamed = fixture_with_task(None, |task| task.spec_path = None).await?;
    let file = unnamed
        .worktree
        .join("api.rs")
        .to_string_lossy()
        .into_owned();
    let reason = pre_tool_use(&unnamed, "Edit", json!({"file_path": file}))
        .await?
        .ok_or("denied")?;
    assert!(reason.contains("under `"), "{reason}");
    Ok(())
}

/// SPEC-426 EDG-005 (review round 3): the edit gate never gives up. Claude Code runs each edit's
/// hook as its block finishes streaming, so neither call counts nor time windows tell one
/// response from the next; the way through is always open (the spec is documentation), and an
/// agent that loops meets the Stop gate, which asks a person after three blocks.
#[tokio::test]
async fn the_edit_gate_holds_every_edit_until_the_plan_exists() -> TestResult {
    let f = fixture().await?;
    let mut watch = f.h.app.runtime(&f.task).hub.watch();
    let file = f.worktree.join("api.rs").to_string_lossy().into_owned();
    // One streamed response's edits arrive tens of seconds apart; later responses too.
    for call in 1..=8 {
        assert!(
            pre_tool_use(&f, "Write", json!({"file_path": file}))
                .await?
                .is_some(),
            "call {call}"
        );
        f.h.clock.0.fetch_add(10_000, Ordering::SeqCst);
    }
    assert!(
        notices(&mut watch).is_empty(),
        "denials say nothing to the person"
    );
    // The spec, being documentation, can always be written; then the edit passes.
    std::fs::create_dir_all(f.worktree.join("docs/specs"))?;
    let spec = f.worktree.join("docs/specs/checkout.md");
    assert_eq!(
        pre_tool_use(&f, "Write", json!({"file_path": spec.to_string_lossy()})).await?,
        None
    );
    std::fs::write(
        &spec,
        "---\nyulius-task: t-main\nstatus: draft\n---\n# Checkout\n\n## Plan\n\n- [ ] Pay\n",
    )?;
    assert_eq!(
        pre_tool_use(&f, "Write", json!({"file_path": file})).await?,
        None
    );
    Ok(())
}

/// SPEC-426 EDG-002 (review round 4): what the gate cannot decide passes: a spec that exists but
/// cannot be read (a link) and a workflow the store cannot read never hold an edit, which would
/// otherwise stay held for good.
#[tokio::test]
async fn an_edit_the_gate_cannot_judge_passes() -> TestResult {
    let f = fixture().await?;
    std::fs::create_dir_all(f.worktree.join("docs/specs"))?;
    std::fs::write(f.worktree.join("docs/specs/real.md"), "# Draft\n")?;
    std::os::unix::fs::symlink("real.md", f.worktree.join("docs/specs/checkout.md"))?;
    let file = f.worktree.join("api.rs").to_string_lossy().into_owned();
    assert_eq!(
        pre_tool_use(&f, "Edit", json!({"file_path": file})).await?,
        None,
        "a linked spec cannot be read: not judged"
    );

    // A spec that is not UTF-8, or larger than Yulius reads (its Plan maybe past the cut), is
    // not judged either.
    std::fs::remove_file(f.worktree.join("docs/specs/checkout.md"))?;
    std::fs::write(f.worktree.join("docs/specs/checkout.md"), b"# Draft \xff\n")?;
    assert_eq!(
        pre_tool_use(&f, "Edit", json!({"file_path": file_of(&f, "api.rs")})).await?,
        None,
        "not UTF-8"
    );
    let mut big = "# Big\n\n".to_owned();
    big.push_str(&"filler line\n".repeat(30_000));
    big.push_str("\n## Plan\n\n- [ ] Pay\n");
    std::fs::write(f.worktree.join("docs/specs/checkout.md"), big)?;
    assert_eq!(
        pre_tool_use(&f, "Edit", json!({"file_path": file_of(&f, "api.rs")})).await?,
        None,
        "too large to judge"
    );

    // A feature (its workflow requires a spec) is held; once its workflow cannot be read, not.
    let feature = fixture_with_workflow(Some(("wf-feature-test", true))).await?;
    let file = feature
        .worktree
        .join("probe.rs")
        .to_string_lossy()
        .into_owned();
    assert!(
        pre_tool_use(&feature, "Write", json!({"file_path": file}))
            .await?
            .is_some(),
        "a feature's code edit waits for its Plan"
    );
    feature
        .h
        .store
        .execute_for_tests("DROP TABLE workflows")
        .await?;
    assert_eq!(
        pre_tool_use(&feature, "Write", json!({"file_path": file})).await?,
        None,
        "an unreadable workflow is a failure to decide"
    );
    Ok(())
}

/// SPEC-503 CDX-007: a Codex run's hooks reach Yulius as Claude Code's do: SessionStart names the
/// session (Codex has no `--session-id`) and answers the briefing in the shape Codex reads; a hook
/// of another harness than the run's is refused, and an unknown harness is not a hook.
#[tokio::test]
async fn codex_hooks_reach_yulius_for_codex_runs_only() -> TestResult {
    let f = fixture_of(None, |_| {}, Agent::Codex).await?;
    let session = "019a6b2e-7c1d-7f00-9a3e-1b2c3d4e5f60";
    let resp = call(
        &f.h.internal,
        Method::POST,
        "/hooks/codex/SessionStart",
        Some(
            json!({"session_id": session, "hook_event_name": "SessionStart",
            "source": "startup", "cwd": f.worktree, "model": "gpt-synthetic",
            "permission_mode": "bypassPermissions", "transcript_path": null}),
        ),
        &agent(),
    )
    .await?;
    assert_eq!(resp.status, 200, "{:?}", resp.body);
    let output = &resp.body["hookSpecificOutput"];
    assert_eq!(output["hookEventName"], "SessionStart", "{}", resp.body);
    assert!(
        output["additionalContext"]
            .as_str()
            .is_some_and(|c| c.contains("Checkout API")),
        "{}",
        resp.body
    );
    let attempt =
        f.h.app
            .deps
            .store
            .attempt(&AttemptId::parse("at1")?)
            .await?
            .ok_or("attempt")?;
    assert_eq!(attempt.provider_session_id.as_deref(), Some(session));
    let lineage = f.h.app.deps.store.provider_sessions(&f.task).await?;
    assert!(
        lineage
            .iter()
            .any(|l| l.session_id == session && l.harness == Agent::Codex),
        "{lineage:?}"
    );
    let stop = call(
        &f.h.internal,
        Method::POST,
        "/hooks/codex/Stop",
        Some(json!({"session_id": session, "hook_event_name": "Stop",
            "stop_hook_active": false, "last_assistant_message": "Done."})),
        &agent(),
    )
    .await?;
    assert_eq!(stop.status, 200, "{:?}", stop.body);
    let claude = call(
        &f.h.internal,
        Method::POST,
        "/hooks/claude/Stop",
        Some(json!({})),
        &agent(),
    )
    .await?;
    assert_eq!(claude.status, 401, "a Claude hook from a Codex run");
    let unknown = call(
        &f.h.internal,
        Method::POST,
        "/hooks/gemini/Stop",
        Some(json!({})),
        &agent(),
    )
    .await?;
    assert_eq!(unknown.status, 404);

    let claude_run = fixture().await?;
    let codex = call(
        &claude_run.h.internal,
        Method::POST,
        "/hooks/codex/Stop",
        Some(json!({})),
        &agent(),
    )
    .await?;
    assert_eq!(codex.status, 401, "a Codex hook from a Claude run");
    Ok(())
}

const CODEX_PARENT: &str = "019a6b2e-7c1d-7f00-9a3e-1b2c3d4e5f60";
const REVIEW_HEAD: &str = "ab1234567890ab1234567890ab1234567890ab12";

async fn codex_review_fixture() -> Result<Fixture, Box<dyn std::error::Error>> {
    use yulius_app::ports::store::TaskStore;
    let f = fixture_of(None, |_| {}, Agent::Codex).await?;
    f.h.store
        .set_attempt_session(&AttemptId::parse("at1")?, CODEX_PARENT)
        .await?;
    Ok(f)
}

fn codex_child(f: &Fixture, id: &str, parent: &str, path: &str, depth: u32) -> TestResult {
    let day =
        f.h._dir
            .path()
            .join("sandboxes/s1/codex/sessions/2026/11/21");
    std::fs::create_dir_all(&day)?;
    let meta = json!({
        "type":"session_meta",
        "payload":{
            "id":id, "cwd":f.worktree,
            "timestamp":yulius_domain::time::format_rfc3339(f.h.app.now()),
            "source":{"subagent":{"thread_spawn":{
                "parent_thread_id":parent, "depth":depth, "agent_path":path, "agent_role":null
            }}},
            "git":{"commit_hash":REVIEW_HEAD}
        }
    });
    std::fs::write(
        day.join(format!("rollout-2026-11-21T22-12-05-{id}.jsonl")),
        format!("{meta}\n"),
    )?;
    Ok(())
}

async fn codex_child_hook(f: &Fixture, event: &str, id: &str, report: Option<&str>) -> TestResult {
    let r = call(
        &f.h.internal,
        Method::POST,
        &format!("/hooks/codex/{event}"),
        Some(json!({
            "hook_event_name":event, "session_id":CODEX_PARENT, "agent_id":id,
            "agent_type":"default", "model":if report.is_some() {"gpt-6.2-sol"} else {"gpt-6.2-astra"},
            "transcript_path":"/untrusted/hook-path.jsonl", "last_assistant_message":report
        })),
        &agent(),
    ).await?;
    assert_eq!(r.status, 200, "{}", r.body);
    Ok(())
}

#[tokio::test]
async fn codex_review_starts_belong_to_named_children_of_the_task_session() -> TestResult {
    use yulius_app::ports::store::CoordinationStore;
    let f = codex_review_fixture().await?;
    for (id, parent, path, depth) in [
        ("worker", CODEX_PARENT, "/root/worker", 1),
        (
            "near-match",
            CODEX_PARENT,
            "/root/yulius_review_discovery",
            1,
        ),
        ("nested", CODEX_PARENT, "/root/worker/yulius_review_1", 2),
        (
            "other-parent",
            "unrelated-parent",
            "/root/yulius_review_1",
            1,
        ),
    ] {
        codex_child(&f, id, parent, path, depth)?;
        codex_child_hook(&f, "SubagentStart", id, None).await?;
    }
    assert!(f.h.store.review_rounds(&f.task, true).await?.is_empty());
    codex_child(&f, "review", CODEX_PARENT, "/root/yulius_review_1", 1)?;
    let start = f.h.app.now();
    codex_child_hook(&f, "SubagentStart", "review", None).await?;
    f.h.clock.0.fetch_add(30_000, Ordering::SeqCst);
    codex_child_hook(&f, "SubagentStart", "review", None).await?;
    let rows = f.h.store.review_rounds(&f.task, true).await?;
    assert_eq!(rows.len(), 1);
    let row = &rows[0];
    assert_eq!(row.round, 1);
    assert_eq!(row.head.as_deref(), Some(REVIEW_HEAD));
    assert_eq!(row.subagent_id.as_deref(), Some("review"));
    assert_eq!(row.model, "gpt-6.2-astra");
    assert_eq!(row.started_at, start);
    assert_eq!(row.ended_at, None);
    Ok(())
}

#[tokio::test]
async fn codex_review_reports_settle_only_their_own_round_once() -> TestResult {
    use yulius_app::ports::store::CoordinationStore;
    let f = codex_review_fixture().await?;
    codex_child(&f, "first", CODEX_PARENT, "/root/yulius_review_1", 1)?;
    codex_child_hook(&f, "SubagentStart", "first", None).await?;
    codex_child_hook(&f, "SubagentStop", "untracked", Some("Verdict: approve")).await?;
    codex_child_hook(
        &f,
        "SubagentStop",
        "first",
        Some("Still checking the tests."),
    )
    .await?;
    let first = f.h.store.review_rounds(&f.task, true).await?;
    assert_eq!(first.len(), 1);
    assert_eq!(first[0].ended_at, None);
    let end = Timestamp(f.h.clock.0.fetch_add(12_000, Ordering::SeqCst) + 12_000);
    codex_child_hook(
        &f,
        "SubagentStop",
        "first",
        Some("Verdict: request changes"),
    )
    .await?;
    codex_child(&f, "second", CODEX_PARENT, "/root/yulius_review_2", 1)?;
    codex_child_hook(&f, "SubagentStart", "second", None).await?;
    f.h.clock.0.fetch_add(12_000, Ordering::SeqCst);
    codex_child_hook(&f, "SubagentStop", "first", Some("Verdict: approve")).await?;
    codex_child_hook(&f, "SubagentStop", "second", Some("Verdict: approve")).await?;
    let rows = f.h.store.review_rounds(&f.task, true).await?;
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].ended_at, Some(end));
    assert_eq!(rows[0].verdict.as_deref(), Some("request_changes"));
    assert_eq!(rows[0].model_answered.as_deref(), Some("gpt-6.2-sol"));
    assert_eq!(rows[1].round, 2);
    assert_eq!(rows[1].verdict.as_deref(), Some("approve"));
    Ok(())
}

#[tokio::test]
async fn codex_review_a_verdict_can_arrive_before_its_start_hook() -> TestResult {
    use yulius_app::ports::store::CoordinationStore;
    let f = codex_review_fixture().await?;
    codex_child(&f, "review", CODEX_PARENT, "/root/yulius_review_1", 1)?;
    let start = f.h.app.now();
    f.h.clock.0.fetch_add(35_000, Ordering::SeqCst);
    let end = f.h.app.now();
    codex_child_hook(&f, "SubagentStop", "review", Some("Verdict: approve")).await?;
    f.h.clock.0.fetch_add(10_000, Ordering::SeqCst);
    codex_child_hook(&f, "SubagentStart", "review", None).await?;
    let rows = f.h.store.review_rounds(&f.task, true).await?;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].head.as_deref(), Some(REVIEW_HEAD));
    assert_eq!(rows[0].started_at, start);
    assert_eq!(rows[0].ended_at, Some(end));
    assert_eq!(rows[0].verdict.as_deref(), Some("approve"));
    assert_eq!(rows[0].model_answered.as_deref(), Some("gpt-6.2-sol"));
    Ok(())
}

#[tokio::test]
async fn codex_review_a_merge_without_rounds_still_excludes_delayed_old_hooks() -> TestResult {
    use yulius_app::ports::store::CoordinationStore;
    use yulius_domain::delivery::DeliveryStage;
    let f = codex_review_fixture().await?;
    codex_child(&f, "old", CODEX_PARENT, "/root/yulius_review_1", 1)?;
    f.h.clock.0.fetch_add(60_000, Ordering::SeqCst);
    codex_review_delivery_step(&f, DeliveryStage::None, DeliveryStage::MergedIntegration, 7)
        .await?;
    f.h.clock.0.fetch_add(60_000, Ordering::SeqCst);
    codex_review_delivery_step(
        &f,
        DeliveryStage::MergedIntegration,
        DeliveryStage::PrOpen,
        8,
    )
    .await?;
    codex_child_hook(&f, "SubagentStart", "old", None).await?;
    codex_child_hook(&f, "SubagentStop", "old", Some("Verdict: approve")).await?;
    codex_child(&f, "new", CODEX_PARENT, "/root/yulius_review_2", 1)?;
    codex_child_hook(&f, "SubagentStart", "new", None).await?;
    let rows = f.h.store.review_rounds(&f.task, false).await?;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].subagent_id.as_deref(), Some("new"));
    assert_eq!(rows[0].round, 1);
    Ok(())
}

async fn codex_review_delivery_step(
    f: &Fixture,
    from: yulius_domain::delivery::DeliveryStage,
    to: yulius_domain::delivery::DeliveryStage,
    pr: u64,
) -> TestResult {
    use yulius_app::delivery::DeliveryRecord;
    use yulius_app::ports::store_ext::DeliveryStore;
    assert!(
        f.h.store
            .transition_delivery(
                &from,
                &DeliveryRecord {
                    task_id: f.task.clone(),
                    stage: to,
                    pr_number: Some(pr),
                    updated_at: f.h.app.now(),
                },
            )
            .await?
    );
    Ok(())
}

#[tokio::test]
async fn codex_review_rounds_feed_delivery_and_merged_analytics() -> TestResult {
    use yulius_app::delivery::DeliveryRecord;
    use yulius_app::ports::{store::CoordinationStore, store_ext::DeliveryStore};
    use yulius_domain::delivery::DeliveryStage;
    let f = codex_review_fixture().await?;
    let admin = support::superadmin_bearer(&f.h).await?;
    let delivery = format!("/api/tasks/{}/delivery", f.task);
    codex_child(&f, "first", CODEX_PARENT, "/root/yulius_review_1", 1)?;
    codex_child_hook(&f, "SubagentStart", "first", None).await?;
    let start = f.h.app.now();
    f.h.clock.0.fetch_add(90_000, Ordering::SeqCst);
    codex_child_hook(
        &f,
        "SubagentStop",
        "first",
        Some("Verdict: request changes"),
    )
    .await?;
    let end = f.h.app.now();
    codex_child(&f, "second", CODEX_PARENT, "/root/yulius_review_2", 1)?;
    codex_child_hook(&f, "SubagentStart", "second", None).await?;
    codex_child_hook(&f, "SubagentStop", "second", Some("Verdict: approve")).await?;
    let active = call(&f.h.router, Method::GET, &delivery, None, &admin).await?;
    assert_eq!(active.status, 200, "{}", active.body);
    let rows = active.body["review_rounds"]
        .as_array()
        .ok_or("review rows")?;
    assert_eq!(rows.len(), 2);
    assert_eq!(
        rows[0]["started_at"],
        yulius_domain::time::format_rfc3339(start)
    );
    assert_eq!(
        rows[0]["ended_at"],
        yulius_domain::time::format_rfc3339(end)
    );

    f.h.clock.0.fetch_add(60_000, Ordering::SeqCst);
    assert!(
        f.h.store
            .transition_delivery(
                &DeliveryStage::None,
                &DeliveryRecord {
                    task_id: f.task.clone(),
                    stage: DeliveryStage::MergedIntegration,
                    pr_number: Some(7),
                    updated_at: f.h.app.now(),
                },
            )
            .await?
    );
    let merged = call(&f.h.router, Method::GET, &delivery, None, &admin).await?;
    assert_eq!(merged.status, 200, "{}", merged.body);
    assert_eq!(merged.body["review_rounds"][0]["pr_number"], 7);
    assert_eq!(merged.body["review_rounds"][1]["verdict"], "approve");
    let analytics = call(
        &f.h.router,
        Method::GET,
        "/api/analytics/reviews",
        None,
        &admin,
    )
    .await?;
    assert_eq!(analytics.status, 200, "{}", analytics.body);
    assert_eq!(analytics.body["first_rounds_requesting_changes"], 1);
    assert_eq!(analytics.body["median_rounds"], 2.0);
    assert_eq!(
        analytics.body["rounds_by_model"],
        json!([{"model":"gpt-6.2-sol","rounds":2}])
    );

    f.h.clock.0.fetch_add(60_000, Ordering::SeqCst);
    codex_child_hook(&f, "SubagentStart", "first", None).await?;
    codex_child(&f, "next", CODEX_PARENT, "/root/yulius_review_3", 1)?;
    codex_child_hook(&f, "SubagentStart", "next", None).await?;
    let open = f.h.store.review_rounds(&f.task, true).await?;
    assert_eq!(open.len(), 1);
    assert_eq!(open[0].round, 1);
    assert_eq!(open[0].subagent_id.as_deref(), Some("next"));

    // A malformed earlier PR's row must never be decoded while displaying the current one.
    f.h.store
        .execute_for_tests("UPDATE review_rounds SET started_at = 'invalid' WHERE pr_number = 7")
        .await?;
    let pending = call(&f.h.router, Method::GET, &delivery, None, &admin).await?;
    assert_eq!(pending.status, 200, "{}", pending.body);
    assert_eq!(pending.body["review_rounds"][0]["round"], 1);
    assert_eq!(pending.body["review_rounds"][0]["reported"], false);
    codex_child_hook(&f, "SubagentStop", "next", Some("Verdict: approve")).await?;
    f.h.clock.0.fetch_add(60_000, Ordering::SeqCst);
    codex_review_delivery_step(
        &f,
        DeliveryStage::MergedIntegration,
        DeliveryStage::PrOpen,
        8,
    )
    .await?;
    codex_review_delivery_step(
        &f,
        DeliveryStage::PrOpen,
        DeliveryStage::MergedIntegration,
        8,
    )
    .await?;
    let next = call(&f.h.router, Method::GET, &delivery, None, &admin).await?;
    assert_eq!(next.status, 200, "{}", next.body);
    assert_eq!(
        next.body["review_rounds"]
            .as_array()
            .ok_or("review rounds")?
            .len(),
        1
    );
    assert_eq!(next.body["review_rounds"][0]["pr_number"], 8);
    Ok(())
}

/// A live turn with one actionable plan step.
async fn task_with_open_step(
    agent: Agent,
    mode: TaskMode,
    step: &str,
) -> Result<Fixture, Box<dyn std::error::Error>> {
    let f = fixture_of(
        None,
        |task| {
            task.mode = mode;
            task.activity = Some(Activity::Working);
        },
        agent,
    )
    .await?;
    let ctx = f.h.app.agent_context(TOKEN).await?;
    f.h.app
        .tool_steps_set(
            &ctx,
            &json!({"steps": [{"title": step, "status": "in_progress"}]}),
        )
        .await?;
    Ok(f)
}

async fn stop_of(
    f: &Fixture,
    agent_kind: Agent,
) -> Result<serde_json::Value, Box<dyn std::error::Error>> {
    f.h.clock.0.fetch_add(1, Ordering::SeqCst);
    let reply = call(
        &f.h.internal,
        Method::POST,
        &format!("/hooks/{}/Stop", agent_kind.as_str()),
        Some(json!({"fully_idle": true})),
        &agent(),
    )
    .await?;
    assert_eq!(reply.status, 200, "{}", reply.body);
    Ok(reply.body)
}

async fn record_verification_merge(f: &Fixture) -> TestResult {
    use yulius_domain::delivery::DeliveryStage;
    f.h.app
        .deps
        .store
        .transition_delivery(
            &DeliveryStage::None,
            &yulius_app::delivery::DeliveryRecord {
                task_id: f.task.clone(),
                stage: DeliveryStage::MergedIntegration,
                pr_number: Some(17),
                updated_at: f.h.app.now(),
            },
        )
        .await?;
    Ok(())
}

#[tokio::test]
async fn goal_stop_continues_verification_after_merge_without_a_person() -> TestResult {
    let title = "Verify the merged revision on dev";
    for agent_kind in Agent::ALL {
        let f = task_with_open_step(agent_kind, TaskMode::Goal, title).await?;
        record_verification_merge(&f).await?;
        let stop = stop_of(&f, agent_kind).await?;
        let brief = task_brief(&f).await?;
        assert_eq!(stop["decision"], "block", "{agent_kind:?} settled: {brief}");
        assert!(
            stop["reason"]
                .as_str()
                .is_some_and(|reason| reason.contains(title)),
            "{stop}"
        );
        assert_eq!(brief["shows"], "working");
        assert!(brief["needs_you_notice"].is_null());
        assert!(
            f.h.app
                .deps
                .store
                .task_wait_messages(&f.task, 1)
                .await?
                .is_empty()
        );
        assert_eq!(
            f.h.app.deps.store.steps(&f.task).await?[0].status,
            "in_progress"
        );

        let ctx = f.h.app.agent_context(TOKEN).await?;
        f.h.app
            .tool_step_update(&ctx, &json!({"ordinal": 1, "status": "done"}))
            .await?;
        assert!(stop_of(&f, agent_kind).await?["decision"].is_null());
        assert_eq!(task_brief(&f).await?["shows"], "done");
    }
    Ok(())
}

#[tokio::test]
async fn goal_stop_leaves_the_first_native_verdict_to_claude() -> TestResult {
    for marker in ["launch-goal-pending", "typed-goal-pending"] {
        let f = task_with_open_step(Agent::ClaudeCode, TaskMode::Goal, "Verify dev").await?;
        let store = &f.h.app.deps.store;
        let attempt = store.live_attempt(&f.task).await?.ok_or("live attempt")?;
        let run_dir = std::path::Path::new(&attempt.run_dir);
        std::fs::create_dir_all(run_dir)?;
        std::fs::write(run_dir.join(marker), "")?;
        let update = yulius_app::ports::store::TaskFieldUpdate {
            gate_blocks: Some(2),
            ..Default::default()
        };
        store.update_task(&f.task, &update, f.h.app.now()).await?;

        let stop = stop_of(&f, Agent::ClaudeCode).await?;
        assert!(stop["decision"].is_null(), "{marker}: {stop}");
        assert_eq!(task_brief(&f).await?["shows"], "working");
        assert_eq!(store.task(&f.task).await?.ok_or("task")?.gate_blocks, 2);

        std::fs::remove_file(run_dir.join(marker))?;
        let stop = stop_of(&f, Agent::ClaudeCode).await?;
        assert_eq!(stop["decision"], "block");
        assert!(
            stop["reason"]
                .as_str()
                .is_some_and(|s| s.contains("Verify dev"))
        );
        assert_eq!(store.task(&f.task).await?.ok_or("task")?.gate_blocks, 3);
    }
    Ok(())
}

#[tokio::test]
async fn goal_stop_bounds_spec_diagnostics_and_prompt_mode_keeps_the_full_report() -> TestResult {
    for mode in [TaskMode::Goal, TaskMode::Prompt] {
        let f = task_with_open_step(Agent::Codex, mode, "Fix the spec").await?;
        std::fs::write(f.worktree.join("api.rs"), "fn main() {}\n")?;
        std::fs::create_dir_all(f.worktree.join("docs/specs"))?;
        let spec = format!("## Plan\n{}", "- [ ] repeated step\n".repeat(2000));
        std::fs::write(f.worktree.join("docs/specs/checkout.md"), spec)?;
        let stop = stop_of(&f, Agent::Codex).await?;
        assert_eq!(stop["decision"], "block");
        let reason = stop["reason"].as_str().ok_or("spec instruction")?;
        if mode == TaskMode::Goal {
            assert!(
                reason.starts_with("Continue the authorized goal.")
                    && reason.contains("spec_check")
            );
            assert!(reason.len() <= yulius_app::capacity::CONTINUATION_MAX);
        } else {
            assert!(reason.starts_with("The task spec"));
            assert!(reason.len() > yulius_app::capacity::CONTINUATION_MAX);
        }
    }
    Ok(())
}

#[tokio::test]
async fn goal_stop_continues_unfinished_spec_then_explains_the_retry_limit() -> TestResult {
    let f = task_with_open_step(Agent::Codex, TaskMode::Goal, "Write spec").await?;
    std::fs::create_dir_all(f.worktree.join("docs/specs"))?;
    std::fs::write(
        f.worktree.join("docs/specs/checkout.md"),
        "---\nyulius-task: t-main\nstatus: in-progress\n---\n# Checkout\n\n## Goal\n\nVerify checkout.\n\n## Context\n\nFixture.\n\n## Requirements\n\n- Verify.\n\n## Plan\n\n- [x] Write spec\n\n## Acceptance criteria\n\nCheckout verified.\n\n## Test plan\n\nRun tests.\n\n## Decisions\n\nUse fixtures.\n",
    )?;
    for _ in 0..yulius_app::hooks::MAX_GATE_BLOCKS {
        let stop = stop_of(&f, Agent::Codex).await?;
        assert_eq!(stop["decision"], "block", "{stop}");
        assert!(
            stop["reason"].as_str().is_some_and(|r| r.contains("spec")),
            "{stop}"
        );
        assert_eq!(task_brief(&f).await?["shows"], "working");
    }
    assert!(stop_of(&f, Agent::Codex).await?["decision"].is_null());
    let brief = task_brief(&f).await?;
    assert_eq!(brief["shows"], "needs_you");
    let problem = brief["attention"]["problem"]
        .as_str()
        .ok_or("retry limit explanation")?;
    assert!(
        problem.contains("3") && problem.contains("spec"),
        "{problem}"
    );
    assert_eq!(
        brief["attention"]["action"],
        "Review the last attempt in the task terminal to identify what blocked completion."
    );
    Ok(())
}

#[tokio::test]
async fn goal_stop_leaves_the_final_step_to_the_staging_outcome() -> TestResult {
    use yulius_app::ports::store_ext::StagingRequestRecord;
    use yulius_domain::staging::StagingState;

    let f = task_with_open_step(Agent::Codex, TaskMode::Goal, "Verify staging").await?;
    record_verification_merge(&f).await?;
    let store = &f.h.app.deps.store;
    let request = StagingRequestRecord {
        id: "sr_final".into(),
        sandbox_id: f.sandbox.clone(),
        environment: "staging".into(),
        task_id: Some(f.task.clone()),
        revision: "0123456789abcdef0123456789abcdef01234567".into(),
        state: StagingState::GateFailed,
        ci_run_id: Some("ci-final".into()),
        cd_run_id: None,
        attempts: 0,
        note: None,
        requested_by: "task:1".into(),
        told: false,
        created_at: f.h.app.now(),
        updated_at: f.h.app.now(),
    };
    assert!(store.insert_staging_request(&request).await?);
    let stop = stop_of(&f, Agent::Codex).await?;
    assert!(stop["decision"].is_null(), "{stop}");
    assert_eq!(task_brief(&f).await?["shows"], "working");
    assert_eq!(store.task(&f.task).await?.ok_or("task")?.gate_blocks, 0);
    assert!(store.mark_staging_told(&request).await?);
    assert_eq!(stop_of(&f, Agent::Codex).await?["decision"], "block");
    Ok(())
}

#[tokio::test]
async fn goal_stop_yields_to_a_live_yulius_wait_then_continues_the_step() -> TestResult {
    use std::time::Duration;
    let f = task_with_open_step(Agent::Codex, TaskMode::Goal, "Check the build").await?;
    assert_eq!(stop_of(&f, Agent::Codex).await?["decision"], "block");
    let app = Arc::clone(&f.h.app);
    let initial = app.agent_wait_state(TOKEN, None).await?;
    let waiter = tokio::spawn(async move {
        app.agent_wait(TOKEN, None, Some(&initial), Duration::from_secs(30))
            .await
    });
    tokio::time::timeout(Duration::from_secs(5), async {
        while f.h.app.active_waits_of(&f.task) == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await?;
    assert!(stop_of(&f, Agent::Codex).await?["decision"].is_null());
    assert_eq!(task_brief(&f).await?["shows"], "working");
    assert_eq!(
        f.h.app
            .deps
            .store
            .task(&f.task)
            .await?
            .ok_or("task")?
            .gate_blocks,
        1,
        "the same step waits for machine news"
    );
    waiter.abort();
    assert!(waiter.await.is_err_and(|e| e.is_cancelled()));
    assert_eq!(stop_of(&f, Agent::Codex).await?["decision"], "block");
    Ok(())
}

#[tokio::test]
async fn goal_stop_yields_to_a_pending_merge_notice() -> TestResult {
    let f = task_with_open_step(Agent::Codex, TaskMode::Goal, "Verify checkout").await?;
    std::fs::write(
        f.h.app
            .config
            .data_dir
            .join(yulius_app::pr_watch::PR_NOTICES_FILE),
        serde_json::to_vec(&json!({f.task.as_str(): {"pr":17, "keys": {}}}))?,
    )?;
    record_verification_merge(&f).await?;
    assert!(stop_of(&f, Agent::Codex).await?["decision"].is_null());
    assert_eq!(task_brief(&f).await?["shows"], "working");
    Ok(())
}

#[tokio::test]
async fn goal_stop_after_an_antigravity_model_error_ends_the_turn() -> TestResult {
    let f = task_with_open_step(Agent::Antigravity, TaskMode::Goal, "Verify checkout").await?;
    agy_event(
        &f,
        "Stop",
        json!({"fully_idle": true, "error": "Model unavailable"}),
    )
    .await?;
    assert_eq!(activity(&f).await?, Some(Activity::Waiting));
    Ok(())
}

#[tokio::test]
async fn goal_stop_continues_when_antigravity_reports_its_worker_finished() -> TestResult {
    let f = task_with_open_step(Agent::Antigravity, TaskMode::Goal, "Verify dev").await?;
    agy_event(&f, "Stop", json!({"fully_idle":false})).await?;
    assert_eq!(task_brief(&f).await?["shows"], "working");
    let reply = stop_of(&f, Agent::Antigravity).await?;
    assert_eq!(reply["decision"], "block");
    assert_eq!(task_brief(&f).await?["shows"], "working");
    assert!(
        f.h.app
            .deps
            .store
            .task_wait_messages(&f.task, 1)
            .await?
            .is_empty()
    );
    Ok(())
}

#[tokio::test]
async fn goal_stop_retains_the_one_time_stale_step_guidance() -> TestResult {
    let f = task_with_open_step(Agent::Codex, TaskMode::Goal, "Verify dev").await?;
    f.h.clock.0.fetch_add(90 * 60_000, Ordering::SeqCst);
    let first = stop_of(&f, Agent::Codex).await?;
    let repeated = stop_of(&f, Agent::Codex).await?;
    assert_eq!(first["decision"], "block");
    assert!(
        first["reason"]
            .as_str()
            .is_some_and(|r| r.contains("has been in progress for 90 min")),
        "{first}"
    );
    assert_eq!(repeated["decision"], "block");
    assert!(
        repeated["reason"]
            .as_str()
            .is_some_and(|r| r.contains("Verify dev") && !r.contains("has been in progress")),
        "{repeated}"
    );
    Ok(())
}

#[tokio::test]
async fn goal_stop_rechecks_the_remaining_step_after_a_slow_observation() -> TestResult {
    use std::time::Duration;
    use yulius_app::ports::browser::BrowserPort;
    let f = task_with_open_step(Agent::Codex, TaskMode::Goal, "Build checkout").await?;
    let ctx = f.h.app.agent_context(TOKEN).await?;
    f.h.app.tool_steps_set(&ctx, &json!({"steps":[{"title":"Build checkout","status":"in_progress"},{"title":"Verify dev"}]})).await?;
    for _ in 0..yulius_app::hooks::MAX_GATE_BLOCKS {
        assert_eq!(stop_of(&f, Agent::Codex).await?["decision"], "block");
    }
    let browser = Arc::new(crate::delivery::HeldBrowserLookup::default());
    let (entered, release) = browser.hold_publication_lookup(&f.task);
    browser.running(&f.sandbox).await;
    let mut deps = f.h.app.deps.clone();
    deps.browser = browser;
    let app = yulius_app::context::App::new(f.h.app.config.clone(), deps);
    let hook_app = app.clone();
    let stop = tokio::spawn(async move {
        hook_app
            .hook_event(TOKEN, "codex", "Stop", &Default::default(), None)
            .await
    });
    tokio::time::timeout(Duration::from_secs(4), entered.notified()).await?;
    app.tool_step_update(&ctx, &json!({"ordinal":1,"status":"done"}))
        .await?;
    release.notify_one();
    let yulius_app::hooks::HookReply::Output(reply) = stop.await?? else {
        return Err("held".into());
    };
    let reply = serde_json::to_value(reply)?;
    assert_eq!(reply["decision"], "block", "{reply}");
    assert!(
        reply["reason"]
            .as_str()
            .is_some_and(|r| r.contains("Verify dev")),
        "{reply}"
    );
    let task = app.deps.store.task(&f.task).await?.ok_or("task")?;
    assert_eq!(task.gate_blocks, 1);
    assert_eq!(task.activity, Some(Activity::Working));
    Ok(())
}

#[tokio::test]
async fn goal_stop_reads_the_plan_while_a_watcher_publishes_its_spec_sync() -> TestResult {
    goal_stop_during_spec_publication(false).await
}

#[tokio::test]
async fn goal_stop_reads_the_plan_while_an_explicit_spec_check_publishes() -> TestResult {
    goal_stop_during_spec_publication(true).await
}

async fn goal_stop_during_spec_publication(check: bool) -> TestResult {
    use std::time::Duration;
    use yulius_app::ports::browser::BrowserPort;

    let f = task_with_open_step(Agent::Codex, TaskMode::Goal, "Build checkout").await?;
    std::fs::create_dir_all(f.worktree.join("docs/specs"))?;
    std::fs::write(
        f.worktree.join("docs/specs/checkout.md"),
        "---\nyulius-task: t-main\nstatus: in-progress\n---\n# Checkout\n\n## Goal\n\nVerify checkout.\n\n## Context\n\nFixture.\n\n## Requirements\n\n- Verify.\n\n## Plan\n\n- [x] Build checkout\n- [ ] Verify dev\n\n## Acceptance criteria\n\nCheckout verified.\n\n## Test plan\n\nRun tests.\n\n## Decisions\n\nUse fixtures.\n",
    )?;
    let browser = Arc::new(crate::delivery::HeldBrowserLookup::default());
    let (entered, release) = browser.hold_publication_lookup(&f.task);
    browser.running(&f.sandbox).await;
    let mut deps = f.h.app.deps.clone();
    deps.browser = browser;
    let app = yulius_app::context::App::new(f.h.app.config.clone(), deps);
    let task = app.deps.store.task(&f.task).await?.ok_or("task")?;
    let watcher = app.clone();
    let ctx = app.agent_context(TOKEN).await?;
    if check {
        std::fs::write(f.worktree.join("checkout.rs"), "fn checkout() {}")?;
    }
    let syncing = tokio::spawn(async move {
        if check {
            watcher
                .tool_spec_check(&ctx)
                .await
                .map(|report| report["spec"]["valid"] == true)
        } else {
            watcher.sync_steps_from_spec(&task).await
        }
    });
    tokio::time::timeout(Duration::from_secs(5), entered.notified()).await?;
    let stop = tokio::time::timeout(
        Duration::from_secs(4),
        app.hook_event(TOKEN, "codex", "Stop", &Default::default(), None),
    )
    .await;
    release.notify_one();
    let checked = syncing.await??;
    let yulius_app::hooks::HookReply::Output(reply) =
        stop.map_err(|_| "Stop waited on the spec-check publication")??
    else {
        return Err("held".into());
    };
    assert!(checked);
    let reply = serde_json::to_value(reply)?;
    assert_eq!(reply["decision"], "block");
    assert!(
        reply["reason"]
            .as_str()
            .is_some_and(|r| r.contains("Verify dev")),
        "{reply}"
    );
    Ok(())
}

#[tokio::test]
async fn goal_stop_restores_working_when_the_turn_was_last_seen_waiting() -> TestResult {
    let f = task_with_open_step(Agent::Codex, TaskMode::Goal, "Verify dev").await?;
    f.h.app
        .deps
        .store
        .set_activity(&f.task, Some(Activity::Waiting), f.h.app.now())
        .await?;
    std::fs::create_dir_all(f.worktree.join("docs/specs"))?;
    std::fs::write(
        f.worktree.join("docs/specs/checkout.md"),
        "---\nyulius-task: t-main\nstatus: in-progress\n---\n# Checkout\n\n## Goal\n\nVerify checkout.\n\n## Context\n\nFixture.\n\n## Requirements\n\n- Verify.\n\n## Plan\n\n- [x] Build checkout\n- [ ] Verify dev\n\n## Acceptance criteria\n\nCheckout verified.\n\n## Test plan\n\nRun tests.\n\n## Decisions\n\nUse fixtures.\n",
    )?;
    let mut events = f.h.app.events.subscribe();
    assert_eq!(stop_of(&f, Agent::Codex).await?["decision"], "block");
    let brief = task_brief(&f).await?;
    assert_eq!(brief["shows"], "working");
    assert!(brief["needs_you_notice"].is_null());
    assert!(
        f.h.app
            .deps
            .store
            .task_wait_messages(&f.task, 1)
            .await?
            .is_empty()
    );
    let mut updated = false;
    let mut plan_published = false;
    while let Ok(envelope) = events.try_recv() {
        match envelope.event {
            yulius_contracts::events::UiEvent::TaskUpdated { task }
                if task.id == f.task.as_str() =>
            {
                updated = true;
                assert!(
                    task.needs_you_notice.is_none(),
                    "intermediate notice: {:?}",
                    task.needs_you_notice
                );
            }
            yulius_contracts::events::UiEvent::StepsUpdated { task_id }
                if task_id == f.task.as_str() =>
            {
                plan_published = true;
            }
            _ => {}
        }
    }
    assert!(updated, "the final working state was published");
    assert!(plan_published, "the synchronized steps were published");
    Ok(())
}

/// The PR watcher already pasted run `id`'s failure into the session (SPEC-578): what is
/// left after that is the agent's own Stop.
fn told_red_run(f: &Fixture, id: &str) -> TestResult {
    std::fs::write(
        f.h.app
            .config
            .data_dir
            .join(yulius_app::pr_watch::PR_NOTICES_FILE),
        serde_json::to_vec(&json!({f.task.as_str(): {"pr": 1461, "keys": {"run": id}}}))?,
    )?;
    Ok(())
}

/// The task's own pull request failed Yulius CI at `sha`.
async fn red_ordinary_run(f: &Fixture, id: &str, sha: &str, at: i64) -> TestResult {
    super::delivery::insert_ci_attempt(
        &f.h.store,
        id,
        sha,
        yulius_domain::ci::CiRef::PullRequest(1461),
        yulius_domain::ci::RunState::Failed,
        at,
        &[],
    )
    .await
}

/// Quartio-v2 #325 stopped after five of seven steps with its pull request red (SPEC-558
/// AC-3): either unfinished condition alone continues the goal, up to the Stop bound. The
/// decision reads no GitHub: a held checks lookup cannot delay or change it (ADR-0447).
#[tokio::test]
async fn codex_goal_stop_continues_the_325_shape_and_a_red_pr_alone() -> TestResult {
    let f = task_with_open_step(Agent::Codex, TaskMode::Goal, "Step 1").await?;
    let ctx = f.h.app.agent_context(TOKEN).await?;
    let steps: Vec<_> = (1..=7)
        .map(|n| {
            let status = match n {
                1..=5 => "done",
                6 => "in_progress",
                _ => "pending",
            };
            json!({"title": format!("Step {n}"), "status": status})
        })
        .collect();
    f.h.app
        .tool_steps_set(&ctx, &json!({"steps": steps}))
        .await?;
    install_compatibility_pull(&f).await?;
    let head = crate::support::git(&f.worktree, &["rev-parse", "HEAD"])?;
    red_ordinary_run(&f, "red-at-head", head.trim(), 10).await?;
    told_red_run(&f, "red-at-head")?;
    let hold = f.h.github.hold_checks();
    let stop = stop_of(&f, Agent::Codex).await?;
    assert_eq!(stop["decision"], "block", "{stop}");
    assert!(
        stop["reason"]
            .as_str()
            .is_some_and(|s| s.contains("Step 6")),
        "{stop}"
    );
    // The plan is finished; only the red pull request is left.
    for n in [6, 7] {
        f.h.app
            .tool_step_update(&ctx, &json!({"ordinal": n, "status": "done"}))
            .await?;
    }
    for _ in 0..yulius_app::hooks::MAX_GATE_BLOCKS {
        let stop = tokio::time::timeout(
            yulius_harness::hook_client::HOOK_TIMEOUT,
            stop_of(&f, Agent::Codex),
        )
        .await
        .map_err(|_| "the Codex hook client timed out")??;
        assert_eq!(stop["decision"], "block", "{stop}");
        assert!(
            stop["reason"]
                .as_str()
                .is_some_and(|s| s.contains("#1461") && s.contains("failed")),
            "{stop}"
        );
        assert_ne!(activity(&f).await?, Some(Activity::NeedsAttention));
    }
    let stop = stop_of(&f, Agent::Codex).await?;
    assert!(stop["decision"].is_null(), "{stop}");
    assert_eq!(activity(&f).await?, Some(Activity::NeedsAttention));
    f.h.github.release_checks(&hold);
    Ok(())
}

/// Only a failure at the head the Codex goal is on continues it: a later passing run, or a
/// failure of a commit the agent has moved past, ends the turn and spends no attempt.
#[tokio::test]
async fn codex_goal_stop_ends_once_its_head_moved_past_the_red_run() -> TestResult {
    let f = task_with_open_step(Agent::Codex, TaskMode::Goal, "Finish compatibility").await?;
    let ctx = f.h.app.agent_context(TOKEN).await?;
    f.h.app
        .tool_step_update(&ctx, &json!({"ordinal": 1, "status": "done"}))
        .await?;
    install_compatibility_pull(&f).await?;
    let head = crate::support::git(&f.worktree, &["rev-parse", "HEAD"])?;
    told_red_run(&f, "red-earlier-head")?;
    red_ordinary_run(
        &f,
        "red-earlier-head",
        "0123456789abcdef0123456789abcdef01234567",
        10,
    )
    .await?;
    let stop = stop_of(&f, Agent::Codex).await?;
    assert!(stop["decision"].is_null(), "{stop}");
    super::delivery::insert_ci_attempt(
        &f.h.store,
        "green-at-head",
        head.trim(),
        yulius_domain::ci::CiRef::PullRequest(1461),
        yulius_domain::ci::RunState::Passed,
        11,
        &[],
    )
    .await?;
    let stop = stop_of(&f, Agent::Codex).await?;
    assert!(stop["decision"].is_null(), "{stop}");
    let task = f.h.app.deps.store.task(&f.task).await?.ok_or("task")?;
    assert_eq!(task.gate_blocks, 0);
    Ok(())
}

/// A failure the watch has not pasted in is not the gate's to charge (SPEC-578). Here the
/// watch cannot see the run (a busy sandbox pushed it out of its window), so Stop does not
/// hand over to it, and only the told check keeps the turn from being charged.
#[tokio::test]
async fn codex_goal_stop_leaves_an_untold_failure_uncharged() -> TestResult {
    let f = task_with_open_step(Agent::Codex, TaskMode::Goal, "Finish compatibility").await?;
    let ctx = f.h.app.agent_context(TOKEN).await?;
    f.h.app
        .tool_step_update(&ctx, &json!({"ordinal": 1, "status": "done"}))
        .await?;
    install_compatibility_pull(&f).await?;
    let head = crate::support::git(&f.worktree, &["rev-parse", "HEAD"])?;
    told_red_run(&f, "an-older-run")?;
    red_ordinary_run(&f, "untold-red-at-head", head.trim(), 10).await?;
    busy_sandbox(&f).await?;
    let stop = stop_of(&f, Agent::Codex).await?;
    assert!(stop["decision"].is_null(), "{stop}");
    let task = f.h.app.deps.store.task(&f.task).await?.ok_or("task")?;
    assert_eq!(task.gate_blocks, 0);
    Ok(())
}

/// A hundred newer runs of other pull requests: the sandbox's recent-run window holds none of
/// the task's own.
async fn busy_sandbox(f: &Fixture) -> TestResult {
    for n in 0..100_i64 {
        super::delivery::insert_ci_attempt(
            &f.h.store,
            &format!("other-pr-{n}"),
            "0123456789abcdef0123456789abcdef01234567",
            yulius_domain::ci::CiRef::PullRequest(5000 + u64::try_from(n)?),
            yulius_domain::ci::RunState::Passed,
            11 + n,
            &[],
        )
        .await?;
    }
    Ok(())
}

/// The run is found by its commit, however many newer runs the sandbox has.
#[tokio::test]
async fn codex_goal_stop_finds_its_red_run_in_a_busy_sandbox() -> TestResult {
    let f = task_with_open_step(Agent::Codex, TaskMode::Goal, "Finish compatibility").await?;
    let ctx = f.h.app.agent_context(TOKEN).await?;
    f.h.app
        .tool_step_update(&ctx, &json!({"ordinal": 1, "status": "done"}))
        .await?;
    install_compatibility_pull(&f).await?;
    let head = crate::support::git(&f.worktree, &["rev-parse", "HEAD"])?;
    told_red_run(&f, "red-at-head")?;
    red_ordinary_run(&f, "red-at-head", head.trim(), 10).await?;
    busy_sandbox(&f).await?;
    let stop = stop_of(&f, Agent::Codex).await?;
    assert_eq!(stop["decision"], "block", "{stop}");
    assert!(
        stop["reason"].as_str().is_some_and(|s| s.contains("#1461")),
        "{stop}"
    );
    Ok(())
}

/// A told failure at the worktree's commit is superseded by a newer run of the pull request on
/// another commit (the branch was updated outside the worktree): the turn ends uncharged.
#[tokio::test]
async fn codex_goal_stop_yields_to_a_newer_run_of_its_pr_elsewhere() -> TestResult {
    let f = task_with_open_step(Agent::Codex, TaskMode::Goal, "Finish compatibility").await?;
    let ctx = f.h.app.agent_context(TOKEN).await?;
    f.h.app
        .tool_step_update(&ctx, &json!({"ordinal": 1, "status": "done"}))
        .await?;
    install_compatibility_pull(&f).await?;
    let head = crate::support::git(&f.worktree, &["rev-parse", "HEAD"])?;
    told_red_run(&f, "red-at-head")?;
    red_ordinary_run(&f, "red-at-head", head.trim(), 10).await?;
    super::delivery::insert_ci_attempt(
        &f.h.store,
        "newer-green-elsewhere",
        "0123456789abcdef0123456789abcdef01234567",
        yulius_domain::ci::CiRef::PullRequest(1461),
        yulius_domain::ci::RunState::Passed,
        11,
        &[],
    )
    .await?;
    let stop = stop_of(&f, Agent::Codex).await?;
    assert!(stop["decision"].is_null(), "{stop}");
    let task = f.h.app.deps.store.task(&f.task).await?.ok_or("task")?;
    assert_eq!(task.gate_blocks, 0);
    Ok(())
}

/// A head pushed past the failed commit from elsewhere supersedes the failure, as it does for
/// the watch (`run_superseded_by_pushed_head`): the turn ends uncharged.
#[tokio::test]
async fn codex_goal_stop_yields_to_a_pushed_head_past_its_red_commit() -> TestResult {
    let f = task_with_open_step(Agent::Codex, TaskMode::Goal, "Finish compatibility").await?;
    let ctx = f.h.app.agent_context(TOKEN).await?;
    f.h.app
        .tool_step_update(&ctx, &json!({"ordinal": 1, "status": "done"}))
        .await?;
    install_compatibility_pull(&f).await?;
    let head = crate::support::git(&f.worktree, &["rev-parse", "HEAD"])?;
    let head = head.trim();
    told_red_run(&f, "red-at-head")?;
    red_ordinary_run(&f, "red-at-head", head, 10).await?;
    let branch = ctx.task.branch.clone().ok_or("branch")?;
    crate::support::git(
        &f.worktree,
        &["commit", "-q", "--allow-empty", "-m", "updated elsewhere"],
    )?;
    let pushed = crate::support::git(&f.worktree, &["rev-parse", "HEAD"])?;
    crate::support::git(
        &f.worktree,
        &[
            "update-ref",
            &format!("refs/remotes/origin/{branch}"),
            pushed.trim(),
        ],
    )?;
    crate::support::git(&f.worktree, &["reset", "-q", "--hard", head])?;
    let stop = stop_of(&f, Agent::Codex).await?;
    assert!(stop["decision"].is_null(), "{stop}");
    let task = f.h.app.deps.store.task(&f.task).await?.ok_or("task")?;
    assert_eq!(task.gate_blocks, 0);
    Ok(())
}

/// A pull request merged while red leaves nothing on this branch to repair: a told failure at
/// the head does not continue the goal or spend an attempt.
#[tokio::test]
async fn codex_goal_stop_ends_after_its_red_pr_merged() -> TestResult {
    use yulius_domain::delivery::DeliveryStage;
    let f = task_with_open_step(Agent::Codex, TaskMode::Goal, "Finish compatibility").await?;
    let ctx = f.h.app.agent_context(TOKEN).await?;
    f.h.app
        .tool_step_update(&ctx, &json!({"ordinal": 1, "status": "done"}))
        .await?;
    install_compatibility_pull(&f).await?;
    let head = crate::support::git(&f.worktree, &["rev-parse", "HEAD"])?;
    told_red_run(&f, "red-at-head")?;
    red_ordinary_run(&f, "red-at-head", head.trim(), 10).await?;
    f.h.app
        .deps
        .store
        .transition_delivery(
            &DeliveryStage::PrOpen,
            &yulius_app::delivery::DeliveryRecord {
                task_id: f.task.clone(),
                stage: DeliveryStage::MergedIntegration,
                pr_number: Some(1461),
                updated_at: f.h.app.now(),
            },
        )
        .await?;
    let stop = stop_of(&f, Agent::Codex).await?;
    assert!(stop["decision"].is_null(), "{stop}");
    let task = f.h.app.deps.store.task(&f.task).await?.ok_or("task")?;
    assert_eq!(task.gate_blocks, 0);
    Ok(())
}

/// Claude Code's native goal evaluator and Antigravity's own goal end their turns: the red
/// pull request continues only a Codex goal (SPEC-558 PKR-003).
#[tokio::test]
async fn a_red_pr_continues_no_other_harnesss_finished_goal() -> TestResult {
    for agent_kind in [Agent::ClaudeCode, Agent::Antigravity] {
        let f = task_with_open_step(agent_kind, TaskMode::Goal, "Finish compatibility").await?;
        let ctx = f.h.app.agent_context(TOKEN).await?;
        f.h.app
            .tool_step_update(&ctx, &json!({"ordinal": 1, "status": "done"}))
            .await?;
        install_compatibility_pull(&f).await?;
        let head = crate::support::git(&f.worktree, &["rev-parse", "HEAD"])?;
        told_red_run(&f, "red-at-head")?;
        red_ordinary_run(&f, "red-at-head", head.trim(), 10).await?;
        let stop = stop_of(&f, agent_kind).await?;
        assert!(stop["decision"].is_null(), "{agent_kind:?}: {stop}");
        let task = f.h.app.deps.store.task(&f.task).await?.ok_or("task")?;
        assert_eq!(task.gate_blocks, 0, "{agent_kind:?}");
    }
    Ok(())
}

#[tokio::test]
async fn goal_stop_counts_consecutive_attempts_on_unresolved_work() -> TestResult {
    for replace in [false, true] {
        let f = task_with_open_step(Agent::Codex, TaskMode::Goal, "Build checkout").await?;
        let ctx = f.h.app.agent_context(TOKEN).await?;
        f.h.app.tool_steps_set(&ctx, &json!({"steps":[{"title":"Build checkout","status":"in_progress"},{"title":"Verify dev"}]})).await?;
        for _ in 0..yulius_app::hooks::MAX_GATE_BLOCKS {
            assert_eq!(stop_of(&f, Agent::Codex).await?["decision"], "block");
        }
        if replace {
            f.h.app.tool_steps_set(&ctx, &json!({"steps":[{"title":"Build checkout","status":"done"},{"title":"Verify dev","status":"in_progress"}]})).await?;
        } else {
            f.h.app
                .tool_step_update(&ctx, &json!({"ordinal":1,"status":"done"}))
                .await?;
        }
        let reply = stop_of(&f, Agent::Codex).await?;
        assert_eq!(reply["decision"], "block", "replace={replace}");
        assert!(
            reply["reason"]
                .as_str()
                .is_some_and(|s| s.contains("Verify dev"))
        );
        assert_eq!(
            f.h.app
                .deps
                .store
                .task(&f.task)
                .await?
                .ok_or("task")?
                .gate_blocks,
            1
        );
    }
    Ok(())
}

#[tokio::test]
async fn goal_stop_budget_survives_submission_of_its_own_continuation() -> TestResult {
    let f = task_with_open_step(Agent::Codex, TaskMode::Goal, "Verify dev").await?;
    // Previously accepted viewer input belongs to that prompt. The next submit
    // carries an automatic continuation after one attempt has been consumed.
    f.h.app
        .record_terminal_frame(&f.task, &UserId::parse("u1")?, b"Verify dev\r");
    let entered = call(
        &f.h.internal,
        Method::POST,
        "/hooks/codex/UserPromptSubmit",
        Some(json!({"prompt":"Verify dev"})),
        &agent(),
    )
    .await?;
    assert_eq!(entered.status, 200);
    let reason = stop_of(&f, Agent::Codex).await?["reason"].clone();
    f.h.app.record_typing(&f.task, &UserId::parse("u1")?);
    assert!(
        f.h.app.has_draft(&f.task),
        "the next instruction has not been submitted"
    );
    let response = call(
        &f.h.internal,
        Method::POST,
        "/hooks/codex/UserPromptSubmit",
        Some(json!({"prompt":reason})),
        &agent(),
    )
    .await?;
    assert_eq!(response.status, 200);
    assert!(
        f.h.app.has_draft(&f.task),
        "the automatic submit must preserve the unsent draft"
    );
    assert_eq!(
        f.h.app
            .deps
            .store
            .task(&f.task)
            .await?
            .ok_or("task")?
            .gate_blocks,
        1
    );
    Ok(())
}

async fn make_a_subscription_switch_due(f: &Fixture) -> TestResult {
    let store = &f.h.app.deps.store;
    let current = yulius_domain::ids::AccountId::parse("acc-1")?;
    let mut room = store.account(&current).await?.ok_or("account")?;
    room.id = yulius_domain::ids::AccountId::parse("acc-room")?;
    store.insert_account(&room).await?;
    let usage = json!({"primary": {"used_percent":98.0, "window_minutes":300,
        "resets_at":f.h.app.now().plus_millis(3_600_000).0}})
    .to_string();
    store
        .set_account_state(&current, room.status, Some(&usage), None, f.h.app.now())
        .await?;
    Ok(())
}

#[tokio::test]
async fn prompt_stop_preserves_a_subscription_move_when_its_spec_gate_is_exhausted() -> TestResult {
    let f = fixture().await?;
    std::fs::write(f.worktree.join("api.rs"), "fn main() {}\n")?;
    for _ in 0..yulius_app::hooks::MAX_GATE_BLOCKS {
        assert_eq!(stop_of(&f, Agent::ClaudeCode).await?["decision"], "block");
    }
    make_a_subscription_switch_due(&f).await?;
    let reply =
        f.h.app
            .hook_event(TOKEN, "claude", "Stop", &Default::default(), None)
            .await?;
    assert!(
        matches!(reply, yulius_app::hooks::HookReply::Hold { .. }),
        "prompt-mode move"
    );
    Ok(())
}

#[tokio::test]
async fn goal_stop_explains_exhaustion_before_a_proactive_switch() -> TestResult {
    let f = task_with_open_step(Agent::Codex, TaskMode::Goal, "Verify dev").await?;
    for _ in 0..yulius_app::hooks::MAX_GATE_BLOCKS {
        assert_eq!(stop_of(&f, Agent::Codex).await?["decision"], "block");
    }
    make_a_subscription_switch_due(&f).await?;
    let response = stop_of(&f, Agent::Codex).await?;
    assert!(response["decision"].is_null());
    let brief = task_brief(&f).await?;
    let problem = brief["attention"]["problem"]
        .as_str()
        .ok_or("retry explanation")?;
    assert!(
        problem.contains("3") && problem.contains("Verify dev"),
        "{problem}"
    );
    Ok(())
}

#[tokio::test]
async fn goal_stop_keeps_a_worker_permission_visible_until_answered() -> TestResult {
    let f = task_with_open_step(Agent::ClaudeCode, TaskMode::Goal, "Verify checkout").await?;
    let question = "Allow the worker to access the staging account?";
    hook(
        &f,
        "Notification",
        json!({"notification_type":"worker_permission_prompt", "message":question}),
    )
    .await?;
    assert!(stop_of(&f, Agent::ClaudeCode).await?["decision"].is_null());
    let brief = task_brief(&f).await?;
    assert_eq!(brief["activity"], "needs_attention");
    assert_eq!(brief["attention"]["kind"], "terminal");
    assert!(
        brief["attention"]["problem"]
            .as_str()
            .is_some_and(|text| text.contains(question))
    );
    Ok(())
}

#[tokio::test]
async fn goal_stop_explains_an_explicitly_blocked_plan_and_spends_no_retry() -> TestResult {
    let f = task_with_open_step(Agent::Codex, TaskMode::Goal, "Verify checkout").await?;
    let ctx = f.h.app.agent_context(TOKEN).await?;
    let note = "The staging account is awaiting access from its administrator.";
    f.h.app
        .tool_step_update(&ctx, &json!({"ordinal":1, "status":"blocked", "note":note}))
        .await?;
    assert!(stop_of(&f, Agent::Codex).await?["decision"].is_null());
    let brief = task_brief(&f).await?;
    assert_eq!(brief["needs_you_notice"]["problem"], note);
    assert!(
        !brief["needs_you_notice"]["action"]
            .as_str()
            .ok_or("action")?
            .contains("tell the agent to continue")
    );
    assert_eq!(
        f.h.app
            .deps
            .store
            .task(&f.task)
            .await?
            .ok_or("task")?
            .gate_blocks,
        0
    );
    Ok(())
}

#[tokio::test]
async fn goal_stop_answers_within_the_hook_deadline_while_browser_startup_is_held() -> TestResult {
    use axum::extract::connect_info::MockConnectInfo;
    use std::time::Duration;
    use yulius_app::ports::browser::BrowserPort;

    for hold_first in [true, false] {
        let f = task_with_open_step(Agent::Codex, TaskMode::Goal, "Verify checkout").await?;
        let browser = Arc::new(crate::delivery::HeldBrowserLookup::default());
        if hold_first {
            browser.running(&f.sandbox).await;
        }
        let mut deps = f.h.app.deps.clone();
        deps.browser = browser;
        let app = yulius_app::context::App::new(f.h.app.config.clone(), deps);
        let internal = yulius_server::internal_router(app).layer(MockConnectInfo(
            std::net::SocketAddr::from(([127, 0, 0, 1], 5002)),
        ));
        let response = tokio::time::timeout(
            Duration::from_secs(4),
            call(
                &internal,
                Method::POST,
                "/hooks/codex/Stop",
                Some(json!({})),
                &agent(),
            ),
        )
        .await??;
        assert_eq!(response.status, 200);
        assert_eq!(
            response.body["decision"], "block",
            "held first={hold_first}"
        );
    }
    Ok(())
}

#[tokio::test]
async fn goal_stop_preserves_a_request_for_an_external_permission() -> TestResult {
    let f = task_with_open_step(Agent::Codex, TaskMode::Goal, "Check the build").await?;
    let action = "Grant the build account access to staging";
    support::mcp_tool(&f.h, TOKEN, "needs_you", json!({"action": action})).await?;
    assert!(stop_of(&f, Agent::Codex).await?["decision"].is_null());
    let brief = task_brief(&f).await?;
    assert_eq!(brief["shows"], "needs_you");
    assert_eq!(brief["needs_you"]["action"], action);
    Ok(())
}

#[tokio::test]
async fn goal_stop_without_background_probe_evidence_continues_in_place() -> TestResult {
    let f = task_with_open_step(Agent::Codex, TaskMode::Goal, "Verify checkout").await?;
    // The fixture host cannot read processes. A live PID sends the Stop through that probe.
    assert!(
        f.h.app
            .deps
            .store
            .transition_attempt(
                &AttemptId::parse("at1")?,
                AttemptState::Live,
                &yulius_app::ports::store::AttemptUpdate {
                    child_pid: Some(41),
                    ..Default::default()
                },
            )
            .await?
    );
    assert_eq!(stop_of(&f, Agent::Codex).await?["decision"], "block");
    assert_eq!(task_brief(&f).await?["shows"], "working");
    Ok(())
}

#[tokio::test]
async fn goal_stop_keeps_a_held_scope_decision_with_its_owner() -> TestResult {
    let f = task_with_open_step(Agent::Codex, TaskMode::Goal, "Verify checkout").await?;
    let ctx = f.h.app.agent_context(TOKEN).await?;
    let store = &f.h.app.deps.store;
    store
        .insert_hold(&yulius_app::intake::HoldRecord {
            id: "scope-decision".into(),
            task_id: f.task.clone(),
            class: yulius_domain::intake::PromptClass::NewScope,
            prompt: "Support another platform".into(),
            prompt_sha256: "0".repeat(64),
            percent: 91,
            typed_by: Some(ctx.task.owner),
            state: yulius_app::intake::hold_state::PENDING.into(),
            created_at: f.h.app.now(),
            resolved_by: None,
            resolved_at: None,
            new_task_id: None,
        })
        .await?;
    assert!(stop_of(&f, Agent::Codex).await?["decision"].is_null());
    assert_eq!(task_brief(&f).await?["shows"], "needs_you");
    assert!(store.pending_hold(&f.task).await?.is_some());
    assert_eq!(store.task(&f.task).await?.ok_or("task")?.gate_blocks, 0);
    Ok(())
}

async fn agy_event(f: &Fixture, name: &str, payload: serde_json::Value) -> TestResult {
    f.h.clock.0.fetch_add(1, Ordering::SeqCst);
    let reply = call(
        &f.h.internal,
        Method::POST,
        &format!("/hooks/antigravity/{name}"),
        Some(payload),
        &agent(),
    )
    .await?;
    assert_eq!(reply.status, 200, "{}", reply.body);
    assert!(reply.body["decision"].is_null(), "{}", reply.body);
    Ok(())
}

async fn task_brief(f: &Fixture) -> Result<serde_json::Value, Box<dyn std::error::Error>> {
    let reply = support::mcp_tool(&f.h, TOKEN, "task_get", json!({"number": 1})).await?;
    Ok(support::tool_text(&reply))
}

/// SPEC-528: a background Stop and the agent's wake are both Working.
#[tokio::test]
async fn antigravity_background_stop_and_autonomous_invocation() -> TestResult {
    let f = task_with_open_step(Agent::Antigravity, TaskMode::Goal, "Run the tests").await?;
    agy_event(&f, "Stop", json!({"fully_idle": false})).await?;
    let caller = yulius_app::coordination::ProjectCaller {
        sandbox: f.sandbox.clone(),
        user: f.h.app.agent_context(TOKEN).await?.task.owner,
        task: None,
        channel: yulius_app::coordination::Channel::Superagent,
        actor: "test viewer".into(),
        detail: json!({}),
    };
    let scope = json!({"scope": "needs_you"});
    let list = f.h.app.tool_tasks_list(&caller, &scope).await?;
    assert_eq!(list["tasks"], json!([]), "{list}");
    let busy = task_brief(&f).await?;
    assert_eq!(busy["shows"], "working", "{busy}");
    agy_event(&f, "PreInvocation", json!({})).await?;
    assert_eq!(task_brief(&f).await?["activity"], "working");
    let action = "Approve access to the staging account";
    support::mcp_tool(&f.h, TOKEN, "needs_you", json!({"action": action})).await?;
    agy_event(&f, "Stop", json!({"fully_idle": false})).await?;
    agy_event(&f, "PreInvocation", json!({})).await?;
    let requested = task_brief(&f).await?;
    assert_eq!(requested["shows"], "needs_you");
    assert_eq!(requested["needs_you"]["action"], action);
    let list = f.h.app.tool_tasks_list(&caller, &scope).await?;
    assert_eq!(list["tasks"][0]["number"], 1);
    assert_eq!(list["tasks"][0]["needs_you"]["action"], action);
    Ok(())
}

/// SPEC-528: an idle Stop with open work gives its reason and only a newer wake reopens it.
#[tokio::test]
async fn antigravity_prompt_mode_idle_stop_with_an_open_step_explains_the_wait() -> TestResult {
    let f = task_with_open_step(Agent::Antigravity, TaskMode::Prompt, "Run the tests").await?;
    agy_event(&f, "Stop", json!({"fully_idle": true})).await?;
    let idle = task_brief(&f).await?;
    assert_eq!(idle["activity"], "waiting");
    assert_eq!(idle["shows"], "needs_you");
    assert!(
        idle["wait_reason"]
            .as_str()
            .is_some_and(|s| s.contains("step")),
        "{idle}"
    );
    let plain = call(
        &f.h.internal,
        Method::POST,
        "/mcp",
        Some(json!({
            "jsonrpc": "2.0", "id": 1, "method": "tools/call",
            "params": {"name": "task_get", "arguments": {"number": 1}}
        })),
        &agent(),
    )
    .await?;
    let text = plain.body["result"]["content"][0]["text"]
        .as_str()
        .ok_or("task_get text")?;
    assert!(
        text.contains("NEEDS ITS PERSON") && text.contains("unfinished steps"),
        "{text}"
    );
    let stale = f.h.app.now().0 - 1;
    f.h.app
        .hook_event(
            TOKEN,
            "antigravity",
            "PreInvocation",
            &Default::default(),
            Some(stale),
        )
        .await?;
    assert_eq!(task_brief(&f).await?["activity"], "waiting", "late wake");
    agy_event(&f, "PreInvocation", json!({})).await?;
    assert_eq!(task_brief(&f).await?["shows"], "working");
    // Older clients omitted the idle bit. A model error also really ends the turn.
    for stop in [
        json!({}),
        json!({"fully_idle": false, "error": "Model unavailable"}),
    ] {
        agy_event(&f, "Stop", stop).await?;
        assert_eq!(task_brief(&f).await?["activity"], "waiting");
        agy_event(&f, "PreInvocation", json!({})).await?;
    }
    let ctx = f.h.app.agent_context(TOKEN).await?;
    f.h.app
        .tool_steps_set(
            &ctx,
            &json!({"steps": [{"title": "Run the tests", "status": "done"}]}),
        )
        .await?;
    agy_event(&f, "Stop", json!({"fully_idle": true})).await?;
    let answered = task_brief(&f).await?;
    assert_eq!(answered["shows"], "done");
    assert!(answered.get("wait_reason").is_none());
    Ok(())
}

/// A Stop client can time out while its handler still runs. A model invocation sent
/// afterwards owns the activity, even when the task was already Working at its wake.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn antigravity_late_idle_stop_cannot_settle_a_newer_invocation() -> TestResult {
    for exhausted_gate in [false, true] {
        late_agy_stop(exhausted_gate).await?;
    }
    Ok(())
}

async fn late_agy_stop(exhausted_gate: bool) -> TestResult {
    use std::time::Duration;
    use yulius_app::race::{Point, on};
    let id = TaskId::parse("t-delayed-agy-stop")?;
    let f = fixture_of(
        None,
        |task| {
            task.id = id.clone();
            if exhausted_gate {
                task.gate_blocks = 3;
            }
            task.mode = yulius_app::model::TaskMode::Goal;
            task.activity = Some(Activity::Working);
        },
        Agent::Antigravity,
    )
    .await?;
    if exhausted_gate {
        std::fs::write(f.worktree.join("api.rs"), "fn main() {}\n")?;
    }
    let (reached, parked) = tokio::sync::oneshot::channel();
    let (release, released) = std::sync::mpsc::channel();
    let limit = Duration::from_secs(10);
    on(Point::StopTurnLock, Path::new(id.as_str()), move || {
        let _ = reached.send(());
        assert!(tokio::task::block_in_place(|| released.recv_timeout(limit)).is_ok());
    });
    let app = Arc::clone(&f.h.app);
    let stop = tokio::spawn(async move {
        app.hook_event(TOKEN, "antigravity", "Stop", &Default::default(), None)
            .await
    });
    tokio::time::timeout(limit, parked).await??;
    agy_event(&f, "PreInvocation", json!({})).await?;
    release.send(())?;
    stop.await??;
    assert_eq!(task_brief(&f).await?["activity"], "working");
    Ok(())
}

/// A viewer key may arrive after a submit reaches the server, while its token is being
/// resolved to a task. The fixture clock keeps both events in the same millisecond.
#[test]
fn a_key_during_submit_context_lookup_keeps_its_mark() -> TestResult {
    use futures::FutureExt;
    use std::time::Duration;
    use yulius_contracts::hooks::{HookInput, events};

    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .max_blocking_threads(1)
        .build()?
        .block_on(async {
            let f = fixture_with_task(None, |task| {
                task.mode = TaskMode::Goal;
                task.gate_blocks = 3;
            })
            .await?;
            let app = &f.h.app;
            let run = app.agent_context(TOKEN).await?.attempt.run_dir;
            std::fs::create_dir_all(&run)?;
            let mark = Path::new(&run).join("person-typed");
            let user = UserId::parse("u1")?;
            app.record_terminal_frame(&f.task, &user, b"old text\r");

            // Token lookup submits its SQLite job on the blocking pool. Hold the only
            // worker until the hook has entered and the later viewer key is recorded.
            let (ready, started) = tokio::sync::oneshot::channel();
            let (release, wait) = std::sync::mpsc::channel();
            let worker = tokio::task::spawn_blocking(move || {
                let _ = ready.send(());
                wait.recv_timeout(Duration::from_secs(5))
            });
            tokio::time::timeout(Duration::from_secs(5), started).await??;
            let input = HookInput {
                prompt: Some("old text".into()),
                ..Default::default()
            };
            let mut submit = std::pin::pin!(app.hook_event(
                TOKEN,
                "claude",
                events::USER_PROMPT_SUBMIT,
                &input,
                None
            ));
            assert!(submit.as_mut().now_or_never().is_none());
            app.record_typing(&f.task, &user);
            std::fs::write(&mark, app.now().0.to_string())?;
            release.send(())?;
            worker.await??;
            submit.await?;
            assert!(
                mark.is_file(),
                "the submit erased a key received during lookup"
            );
            assert!(app.has_draft(&f.task));
            assert_eq!(
                app.deps
                    .store
                    .task(&f.task)
                    .await?
                    .ok_or("task")?
                    .gate_blocks,
                0,
                "the already-submitted owner prompt starts its own budget"
            );
            Ok(())
        })
}

/// The phone Send key may be pressed on an empty box. Multiline input can also
/// deliver Enter without submitting anything to the agent.
#[tokio::test]
async fn goal_stop_rejects_empty_and_multiline_enters_as_owner_replies() -> TestResult {
    for frames in [
        vec![b"\r".as_slice()],
        vec![b"line one\\", b"\r"],
        vec![b"\x1b[200~line one", b"\r", b"line two\x1b[201~"],
    ] {
        let f = task_with_open_step(Agent::Codex, TaskMode::Goal, "Verify dev").await?;
        let reason = stop_of(&f, Agent::Codex).await?["reason"]
            .as_str()
            .ok_or("continuation")?
            .to_owned();
        let owner = UserId::parse("u1")?;
        for frame in frames {
            f.h.app.record_terminal_frame(&f.task, &owner, frame);
        }
        let submit = |prompt: &str| yulius_contracts::hooks::HookInput {
            prompt: Some(prompt.to_owned()),
            ..Default::default()
        };
        f.h.app
            .hook_event(TOKEN, "codex", "UserPromptSubmit", &submit(&reason), None)
            .await?;
        assert_eq!(
            f.h.app
                .deps
                .store
                .task(&f.task)
                .await?
                .ok_or("task")?
                .gate_blocks,
            1,
            "a keystroke that submitted nothing cannot claim this automatic prompt"
        );
        f.h.app
            .record_terminal_frame(&f.task, &owner, b"Verify dev");
        f.h.app.record_terminal_frame(&f.task, &owner, b"\r");
        f.h.app
            .hook_event(
                TOKEN,
                "codex",
                "UserPromptSubmit",
                &submit("Verify dev"),
                None,
            )
            .await?;
        assert!(
            !f.h.app.has_draft(&f.task),
            "the owner's actual Enter emptied the box"
        );
        assert_eq!(
            f.h.app
                .deps
                .store
                .task(&f.task)
                .await?
                .ok_or("task")?
                .gate_blocks,
            0
        );
    }
    Ok(())
}

/// The browser splits long terminal input into 8192 UTF-16 units. Each interior
/// cut of the closing escape must allow the next real Enter to submit the paste.
#[tokio::test]
async fn goal_stop_attributes_paste_with_a_split_closing_delimiter() -> TestResult {
    for cut in 1..6 {
        let f = task_with_open_step(Agent::Codex, TaskMode::Goal, "Verify dev").await?;
        assert_eq!(stop_of(&f, Agent::Codex).await?["decision"], "block");
        let text = "v".repeat(8192 - 6 - cut);
        let paste = format!("\x1b[200~{text}\x1b[201~");
        let owner = UserId::parse("u1")?;
        for frame in paste.as_bytes().chunks(8192) {
            f.h.app.record_terminal_frame(&f.task, &owner, frame);
        }
        f.h.app.record_terminal_frame(&f.task, &owner, b"\r");
        f.h.app
            .hook_event(
                TOKEN,
                "codex",
                "UserPromptSubmit",
                &yulius_contracts::hooks::HookInput {
                    prompt: Some(text),
                    ..Default::default()
                },
                None,
            )
            .await?;
        assert_eq!(
            f.h.app
                .deps
                .store
                .task(&f.task)
                .await?
                .ok_or("task")?
                .gate_blocks,
            0,
            "the closing delimiter split after byte {cut}"
        );
        assert!(!f.h.app.has_draft(&f.task));
    }
    Ok(())
}

/// A pause in typing a multiline prompt changes nothing in the live PTY.
#[tokio::test]
async fn goal_stop_preserves_a_multiline_draft_after_a_pause_in_typing() -> TestResult {
    let f = task_with_open_step(Agent::Codex, TaskMode::Goal, "Verify dev").await?;
    let reason = stop_of(&f, Agent::Codex).await?["reason"]
        .as_str()
        .ok_or("continuation")?
        .to_owned();
    let owner = UserId::parse("u1")?;
    f.h.app.record_terminal_frame(&f.task, &owner, b"Verify\\");
    f.h.clock.0.fetch_add(5_001, Ordering::SeqCst);
    f.h.app.record_terminal_frame(&f.task, &owner, b"\r");
    let submit = |prompt: &str| yulius_contracts::hooks::HookInput {
        prompt: Some(prompt.to_owned()),
        ..Default::default()
    };
    f.h.app
        .hook_event(TOKEN, "codex", "UserPromptSubmit", &submit(&reason), None)
        .await?;
    assert_eq!(
        f.h.app
            .deps
            .store
            .task(&f.task)
            .await?
            .ok_or("task")?
            .gate_blocks,
        1
    );
    assert!(f.h.app.has_draft(&f.task));
    f.h.app.record_terminal_frame(&f.task, &owner, b"dev\r");
    f.h.app
        .hook_event(
            TOKEN,
            "codex",
            "UserPromptSubmit",
            &submit("Verify\ndev"),
            None,
        )
        .await?;
    assert_eq!(
        f.h.app
            .deps
            .store
            .task(&f.task)
            .await?
            .ok_or("task")?
            .gate_blocks,
        0
    );
    assert!(!f.h.app.has_draft(&f.task));
    Ok(())
}

/// History and cursor editing belong to the provider composer. Its submission
/// confirms which of the owner's edited instructions actually went through.
#[tokio::test]
async fn goal_stop_accepts_recalled_and_cursor_edited_owner_work() -> TestResult {
    for (frames, prompt) in [
        (
            vec![b"\x1b[200~Verify dev\rRecord evidence\x1b[201~\r".as_slice()],
            "Verify dev\nRecord evidence",
        ),
        (vec![b" x\x7fVerify dev\r".as_slice()], " Verify dev"),
        (vec![b"Review checkout\r".as_slice()], "Review checkout"),
        (vec![b"\x1b[A".as_slice(), b"\r"], "Verify dev"),
        (vec![b"x".as_slice(), b"\x1b[D", b"\x7f", b"\r"], "x"),
    ] {
        let f = task_with_open_step(Agent::Codex, TaskMode::Goal, prompt).await?;
        let owner = UserId::parse("u1")?;
        let submit = |text: &str| yulius_contracts::hooks::HookInput {
            prompt: Some(text.into()),
            ..Default::default()
        };
        let previous = prompt.replace('\n', " ");
        f.h.app
            .record_terminal_frame(&f.task, &owner, format!("{previous}\r").as_bytes());
        f.h.app
            .hook_event(TOKEN, "codex", "UserPromptSubmit", &submit(&previous), None)
            .await?;
        let reason = stop_of(&f, Agent::Codex).await?["reason"]
            .as_str()
            .ok_or("continuation")?
            .to_owned();
        // The initial callback can finish before the viewer starts editing. A
        // lost response can then replay the same instruction after the viewer sends.
        f.h.app
            .hook_event(TOKEN, "codex", "UserPromptSubmit", &submit(&reason), None)
            .await?;
        for frame in frames {
            f.h.app.record_terminal_frame(&f.task, &owner, frame);
        }
        // Repeated callbacks for the same automatic send must leave the owner's
        // pending submission available for the provider to acknowledge.
        for _ in 0..2 {
            f.h.app
                .hook_event(TOKEN, "codex", "UserPromptSubmit", &submit(&reason), None)
                .await?;
            assert_eq!(
                f.h.app
                    .deps
                    .store
                    .task(&f.task)
                    .await?
                    .ok_or("task")?
                    .gate_blocks,
                1
            );
        }
        f.h.app
            .hook_event(TOKEN, "codex", "UserPromptSubmit", &submit(prompt), None)
            .await?;
        assert_eq!(
            f.h.app
                .deps
                .store
                .task(&f.task)
                .await?
                .ok_or("task")?
                .gate_blocks,
            0,
            "{prompt}"
        );
        assert!(!f.h.app.has_draft(&f.task));
    }
    Ok(())
}

/// A lost machine response keeps its invocation. A new edited submission of
/// the identical instruction is the owner's, even when both arrive together.
#[tokio::test]
async fn goal_stop_distinguishes_replay_from_an_edited_copy_of_the_issued_prompt() -> TestResult {
    use axum::{body::Body, http::Request};
    use tower::ServiceExt;

    let f = task_with_open_step(Agent::Codex, TaskMode::Goal, "Verify dev").await?;
    let reason = stop_of(&f, Agent::Codex).await?["reason"]
        .as_str()
        .ok_or("missing continuation")?
        .to_owned();
    let submit = async |id: u128| {
        let req = Request::post("/hooks/codex/UserPromptSubmit")
            .header("Authorization", format!("Bearer {TOKEN}"))
            .header("Content-Type", "application/json")
            .header("x-yulius-hook-invocation", format!("{id:032x}"))
            .body(Body::from(json!({"prompt": reason}).to_string()))?;
        assert_eq!(f.h.internal.clone().oneshot(req).await?.status(), 200);
        Ok::<_, Box<dyn std::error::Error>>(())
    };
    submit(1).await?;
    let owner = UserId::parse("u1")?;
    for frame in [reason.as_bytes(), b"x", b"\x7f", b"\r"] {
        f.h.app.record_terminal_frame(&f.task, &owner, frame);
    }
    submit(1).await?;
    assert_eq!(
        f.h.app
            .deps
            .store
            .task(&f.task)
            .await?
            .ok_or("task")?
            .gate_blocks,
        1
    );
    submit(2).await?;
    assert_eq!(
        f.h.app
            .deps
            .store
            .task(&f.task)
            .await?
            .ok_or("task")?
            .gate_blocks,
        0
    );
    assert!(!f.h.app.has_draft(&f.task));
    Ok(())
}

/// Two invocations can be held at the same boundary. Each release acknowledges
/// its own edited prompt even while the other one's Enter is outstanding.
#[tokio::test]
async fn goal_stop_releases_two_held_owner_submissions_independently() -> TestResult {
    use yulius_app::hooks::HookReply;
    let f = task_with_open_step(Agent::Codex, TaskMode::Goal, "Verify dev").await?;
    let app = &f.h.app;
    let owner = UserId::parse("u1")?;
    let store = &app.deps.store;
    store
        .set_person_pause(&f.task, &owner, &AttemptId::parse("at1")?, app.now())
        .await?;
    let prompts = [(1, "Verify dev"), (2, "Record the result")];
    let submit = async |id, text: &str| {
        app.hook_event(
            TOKEN,
            "codex",
            "UserPromptSubmit",
            &yulius_contracts::hooks::HookInput {
                prompt: Some(text.to_owned()),
                invocation: Some(id),
                ..Default::default()
            },
            None,
        )
        .await
    };
    for (id, text) in prompts {
        app.record_terminal_frame(&f.task, &owner, format!("{text}x\x7f\r").as_bytes());
        assert!(matches!(submit(id, text).await?, HookReply::Hold { .. }));
    }
    store.clear_person_pause(&f.task, app.now()).await?;
    assert!(matches!(
        submit(1, prompts[0].1).await?,
        HookReply::Output(_)
    ));
    assert!(
        app.has_draft(&f.task),
        "the second submission is still held"
    );
    assert!(matches!(
        submit(2, prompts[1].1).await?,
        HookReply::Output(_)
    ));
    assert!(
        !app.has_draft(&f.task),
        "both owner submissions were acknowledged"
    );
    Ok(())
}

/// Two automatic instructions can be issued before either submit hook arrives.
/// The older acknowledgment must leave a later viewer edit available to its own hook.
#[tokio::test]
async fn goal_stop_keeps_both_issued_instructions_until_their_callbacks_arrive() -> TestResult {
    let f = task_with_open_step(Agent::Codex, TaskMode::Goal, "Verify dev").await?;
    let app = &f.h.app;
    let ctx = app.agent_context(TOKEN).await?;
    let first = stop_of(&f, Agent::Codex).await?["reason"]
        .as_str()
        .ok_or("first instruction")?
        .to_owned();
    app.tool_steps_set(
        &ctx,
        &json!({"steps": [{"title":"Record verification", "status":"in_progress"}]}),
    )
    .await?;
    let second = stop_of(&f, Agent::Codex).await?["reason"]
        .as_str()
        .ok_or("second instruction")?
        .to_owned();
    assert_ne!(first, second);
    let spent = app
        .deps
        .store
        .task(&f.task)
        .await?
        .ok_or("task")?
        .gate_blocks;
    assert!(spent > 0);
    app.record_terminal_frame(&f.task, &UserId::parse("u1")?, b"Review checkoutx\x7f\r");
    for (id, text, expected) in [
        (1, first.as_str(), spent),
        (2, second.as_str(), spent),
        (1, first.as_str(), spent),
        (3, "Review checkout", 0),
    ] {
        app.hook_event(
            TOKEN,
            "codex",
            "UserPromptSubmit",
            &yulius_contracts::hooks::HookInput {
                prompt: Some(text.to_owned()),
                invocation: Some(id),
                ..Default::default()
            },
            None,
        )
        .await?;
        assert_eq!(
            app.deps
                .store
                .task(&f.task)
                .await?
                .ok_or("task")?
                .gate_blocks,
            expected
        );
    }
    assert!(!app.has_draft(&f.task));
    Ok(())
}

/// The client has already sent Enter when the next draft reaches Yulius. The HTTP
/// callback arrives later, carrying the client's send time.
#[tokio::test]
async fn a_delayed_goal_submit_retains_the_owner_and_the_next_draft() -> TestResult {
    let f = fixture_with_task(None, |task| {
        task.mode = TaskMode::Goal;
        task.gate_blocks = 3;
    })
    .await?;
    let app = &f.h.app;
    let owner = UserId::parse("u1")?;
    app.record_terminal_frame(&f.task, &owner, b"Verify dev\r");
    f.h.clock.0.fetch_add(1, Ordering::SeqCst);
    let sent = app.now().0;
    // More typing in a long next draft cannot displace the submitted Enter.
    for _ in 0..300 {
        f.h.clock.0.fetch_add(1, Ordering::SeqCst);
        app.record_typing(&f.task, &owner);
    }
    let input = yulius_contracts::hooks::HookInput {
        prompt: Some("Verify dev".into()),
        ..Default::default()
    };
    app.hook_event(TOKEN, "claude", "UserPromptSubmit", &input, Some(sent))
        .await?;
    assert!(app.has_draft(&f.task), "the owner's next draft");
    assert_eq!(
        app.deps
            .store
            .task(&f.task)
            .await?
            .ok_or("task")?
            .gate_blocks,
        0,
        "the instruction already submitted by the owner"
    );
    Ok(())
}

/// The parking fixtures already have a live attempt in the store. Its host can be
/// reattached without starting a CLI or calling a provider.
struct IdleSessionHost;

#[async_trait::async_trait]
impl SessionHost for IdleSessionHost {
    async fn spawn(
        &self,
        _: &Path,
        _: &str,
        _: &str,
        _: &LaunchPlan,
        _: &[u8; 32],
        _: (u16, u16),
    ) -> AppResult<SpawnedRun> {
        Err(AppError::Upstream(
            "fixture only reattaches seeded runs".into(),
        ))
    }

    async fn attach(&self, _: &Path, _: &[u8; 32]) -> AppResult<AttachedRun> {
        let (exit, events) = tokio::sync::oneshot::channel();
        Ok(AttachedRun {
            welcome: RunWelcome {
                child_pid: 2,
                seq: 0,
                cols: 80,
                rows: 24,
                rendered: true,
                snapshot: None,
            },
            reader: Box::new(IdleSessionReader(events)),
            writer: Box::new(IdleSessionWriter(Some(exit))),
        })
    }

    fn inspect(&self, _: &Path) -> RunInspection {
        RunInspection::Missing
    }

    fn terminate(&self, _: &Path) -> AppResult<()> {
        Ok(())
    }
}

struct IdleSessionReader(tokio::sync::oneshot::Receiver<RunEvent>);

#[async_trait::async_trait]
impl RunReader for IdleSessionReader {
    async fn next(&mut self) -> AppResult<Option<RunEvent>> {
        Ok((&mut self.0).await.ok())
    }
}

struct IdleSessionWriter(Option<tokio::sync::oneshot::Sender<RunEvent>>);

#[async_trait::async_trait]
impl RunWriter for IdleSessionWriter {
    async fn input(&mut self, _: &[u8]) -> AppResult<()> {
        Ok(())
    }

    async fn resize(&mut self, _: u16, _: u16) -> AppResult<()> {
        Ok(())
    }

    async fn signal(&mut self, _: RunSignal) -> AppResult<()> {
        self.shutdown(0).await
    }

    async fn shutdown(&mut self, _: u32) -> AppResult<()> {
        if let Some(exit) = self.0.take() {
            let _ = exit.send(RunEvent::Exit(RunExit {
                code: Some(0),
                signal: None,
                spawn_failed: false,
                usage: None,
            }));
        }
        Ok(())
    }
}

async fn attach_idle_run(f: &Fixture, app: &Arc<App>) -> TestResult {
    let attempt = app
        .deps
        .store
        .live_attempt(&f.task)
        .await?
        .ok_or("live run")?;
    let session = "idle-parking-session";
    app.deps
        .store
        .set_attempt_session(&attempt.id, session)
        .await?;
    let home = app
        .config
        .data_dir
        .join("sandboxes")
        .join(f.sandbox.as_str())
        .join("home");
    let transcript = app
        .deps
        .harness
        .claude_transcript(&home, &f.worktree, session);
    std::fs::create_dir_all(transcript.parent().ok_or("transcript directory")?)?;
    std::fs::write(transcript, "")?;
    let runtime = app.runtime(&f.task);
    let before = runtime.transcript_rounds();
    app.reconcile().await?;
    crate::support::eventually(
        "idle run transcript replay",
        std::time::Duration::from_secs(15),
        || async { runtime.transcript_rounds() >= before + 2 },
    )
    .await
}

async fn idle_parking_fixture() -> Result<Fixture, Box<dyn std::error::Error>> {
    let f = fixture().await?;
    let ctx = f.h.app.agent_context(TOKEN).await?;
    std::fs::create_dir_all(&ctx.attempt.run_dir)?;
    f.h.app.runtime(&f.task);
    f.h.app
        .deps
        .store
        .transition_task(
            &f.task,
            None,
            yulius_domain::task::TaskStatus::Running,
            Some(Activity::Waiting),
            f.h.app.now(),
        )
        .await?;
    f.h.clock.0.fetch_add(3_600_000, Ordering::SeqCst);
    attach_idle_run(&f, &f.h.app).await?;
    Ok(f)
}

#[tokio::test]
async fn one_open_step_keeps_an_idle_session_through_digest_and_grace_parking() -> TestResult {
    let f = idle_parking_fixture().await?;
    let ctx = f.h.app.agent_context(TOKEN).await?;
    f.h.app
        .tool_steps_set(
            &ctx,
            &json!({"steps":[{"title":"Merge compatibility","status":"in_progress"}]}),
        )
        .await?;
    assert_eq!(f.h.app.park_idle_sessions_now().await, 0);
    assert_eq!(f.h.app.send_digests_now(Some(1)).await.parked, 0);
    f.h.app
        .tool_step_update(&ctx, &json!({"ordinal":1,"status":"done"}))
        .await?;
    assert_eq!(f.h.app.park_idle_sessions_now().await, 1);
    crate::support::eventually(
        "the idle run to settle as parked",
        std::time::Duration::from_secs(15),
        || async {
            f.h.app
                .deps
                .store
                .task(&f.task)
                .await
                .is_ok_and(|task| task.is_some_and(|task| task.status == TaskStatus::Parked))
        },
    )
    .await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_step_opened_before_the_park_intent_keeps_the_session() -> TestResult {
    let f = idle_parking_fixture().await?;
    let ctx = f.h.app.agent_context(TOKEN).await?;
    let app = f.h.app.clone();
    let handle = tokio::runtime::Handle::current();
    yulius_app::race::on(
        yulius_app::race::Point::ParkPrepared,
        Path::new(f.task.as_str()),
        move || {
            let added = tokio::task::block_in_place(|| {
                handle.block_on(app.tool_steps_set(
            &ctx, &json!({"steps":[{"title":"Verify the new compatibility case","status":"pending"}]}),
        ))
            });
            assert!(added.is_ok(), "{added:?}");
        },
    );
    let parks = f.h.app.park_idle_sessions_now().await;
    let steps = f.h.app.deps.store.steps(&f.task).await?;
    assert_eq!(steps.len(), 1);
    assert_eq!(steps[0].status, "pending");
    assert_eq!(parks, 0, "the newly committed step still needs its session");
    Ok(())
}

#[tokio::test]
async fn a_step_writer_in_progress_defers_parking_until_its_update_finishes() -> TestResult {
    let f = idle_parking_fixture().await?;
    let runtime = f.h.app.runtime(&f.task);
    let writer = runtime.steps_lock.lock().await;
    let parks = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        f.h.app.park_idle_sessions_now(),
    )
    .await?;
    assert_eq!(parks, 0, "the task is updating its declared work");
    drop(writer);
    assert_eq!(f.h.app.park_idle_sessions_now().await, 1);
    Ok(())
}

fn compatibility_pull() -> yulius_app::ports::github::GhPull {
    yulius_app::ports::github::GhPull {
        number: 1461,
        html_url: "https://github.test/acme/fixture/pull/1461".into(),
        title: "Compatibility".into(),
        state: "open".into(),
        merged: false,
        base: "dev".into(),
        base_sha: String::new(),
        head: "feat/1-x".into(),
        head_sha: "abc123".into(),
        mergeable: Some(true),
        behind: false,
        from_fork: false,
        merge_commit_sha: None,
    }
}

async fn install_compatibility_pull(f: &Fixture) -> TestResult {
    use yulius_domain::delivery::DeliveryStage;
    install_app(f).await?;
    f.h.github
        .pulls
        .lock()
        .map_err(|_| "pull fixture")?
        .push(compatibility_pull());
    f.h.app
        .deps
        .store
        .transition_delivery(
            &DeliveryStage::None,
            &yulius_app::delivery::DeliveryRecord {
                task_id: f.task.clone(),
                stage: DeliveryStage::PrOpen,
                pr_number: Some(1461),
                updated_at: f.h.app.now(),
            },
        )
        .await?;
    Ok(())
}

#[tokio::test]
async fn an_idle_task_with_a_pending_pr_keeps_its_run_on_a_cold_cache() -> TestResult {
    use yulius_domain::task::TaskStatus;
    let f = fixture().await?;
    install_compatibility_pull(&f).await?;
    let ctx = f.h.app.agent_context(TOKEN).await?;
    f.h.app
        .tool_steps_set(
            &ctx,
            &json!({"steps":[{"title":"Finish compatibility","status":"done"}]}),
        )
        .await?;
    f.h.github
        .set_checks("abc123", yulius_app::ports::github::ChecksState::Pending);
    std::fs::create_dir_all(&ctx.attempt.run_dir)?;
    f.h.app.runtime(&f.task);
    f.h.app
        .deps
        .store
        .transition_task(
            &f.task,
            None,
            TaskStatus::Running,
            Some(Activity::Waiting),
            f.h.app.now(),
        )
        .await?;
    f.h.clock.0.fetch_add(3_600_000, Ordering::SeqCst);
    attach_idle_run(&f, &f.h.app).await?;
    assert_eq!(f.h.app.park_idle_sessions_now().await, 0);
    assert_eq!(f.h.app.send_digests_now(Some(1)).await.parked, 0);
    let owner = yulius_app::auth::AuthContext {
        user: f
            .h
            .app
            .deps
            .store
            .user(&ctx.task.created_by)
            .await?
            .ok_or("owner")?,
        csrf_token: None,
        session_hash: None,
        token: None,
    };
    f.h.github
        .set_checks("abc123", yulius_app::ports::github::ChecksState::Failure);
    f.h.app.git_overview(&owner, f.sandbox.as_str()).await?;
    // No second overview is opened when the head later becomes green.
    // A green readable source cannot prove that the unreadable source was green too.
    f.h.github.set_checks_report(
        "abc123",
        yulius_app::ports::github::ChecksReport {
            state: yulius_app::ports::github::ChecksState::Success,
            unreadable: vec![yulius_app::ports::github::ChecksSource::WorkflowRuns],
        },
    );
    assert_eq!(f.h.app.park_idle_sessions_now().await, 0);
    // A tested, mergeable head frees this very session once the same lookup succeeds.
    f.h.github
        .set_checks("abc123", yulius_app::ports::github::ChecksState::Success);
    // A new PR is recorded while the old head is being verified. A green
    // response for the old delivery cannot authorize shutdown for the new one.
    let original =
        f.h.app
            .deps
            .store
            .delivery(&f.task)
            .await?
            .ok_or("original PR")?;
    let replacement = yulius_app::delivery::DeliveryRecord {
        pr_number: Some(1462),
        ..original.clone()
    };
    let held = f.h.github.hold_checks();
    let app = f.h.app.clone();
    let checking = tokio::spawn(async move { app.park_idle_sessions_now().await });
    tokio::time::timeout(
        std::time::Duration::from_secs(5),
        f.h.github.checks_entered.notified(),
    )
    .await?;
    assert!(
        f.h.app
            .deps
            .store
            .transition_delivery(&original.stage, &replacement)
            .await?
    );
    f.h.github.release_checks(&held);
    assert_eq!(checking.await?, 0, "the verified PR was replaced");
    assert!(
        f.h.app
            .deps
            .store
            .transition_delivery(&replacement.stage, &original)
            .await?
    );
    let held = f.h.github.hold_checks();
    let app = f.h.app.clone();
    let checking = tokio::spawn(async move { app.park_idle_sessions_now().await });
    tokio::time::timeout(
        std::time::Duration::from_secs(5),
        f.h.github.checks_entered.notified(),
    )
    .await?;
    f.h.github.pulls.lock().map_err(|_| "pull fixture")?[0].head_sha = "new-head".into();
    f.h.github
        .set_checks("new-head", yulius_app::ports::github::ChecksState::Failure);
    f.h.github.release_checks(&held);
    assert_eq!(
        checking.await?,
        0,
        "the verified head was replaced on the same PR"
    );
    f.h.github
        .set_checks("new-head", yulius_app::ports::github::ChecksState::Success);
    assert_eq!(f.h.app.park_idle_sessions_now().await, 1);
    Ok(())
}

#[tokio::test]
async fn parking_a_recorded_pr_waits_for_delivery_if_github_closed_it_unmerged() -> TestResult {
    let f = fixture().await?;
    install_compatibility_pull(&f).await?;
    let ctx = f.h.app.agent_context(TOKEN).await?;
    std::fs::create_dir_all(&ctx.attempt.run_dir)?;
    f.h.app.runtime(&f.task);
    f.h.app
        .deps
        .store
        .transition_task(
            &f.task,
            None,
            yulius_domain::task::TaskStatus::Running,
            Some(Activity::Done),
            f.h.app.now(),
        )
        .await?;
    f.h.clock.0.fetch_add(3_600_000, Ordering::SeqCst);
    attach_idle_run(&f, &f.h.app).await?;
    f.h.github.pulls.lock().map_err(|_| "pull fixture")?[0].state = "closed".into();
    assert_eq!(f.h.app.park_idle_sessions_now().await, 0);
    f.h.github.pulls.lock().map_err(|_| "pull fixture")?[0].merged = true;
    assert_eq!(f.h.app.park_idle_sessions_now().await, 1);
    Ok(())
}

async fn idle_for_parking(
    f: &Fixture,
) -> Result<yulius_app::auth::AuthContext, Box<dyn std::error::Error>> {
    let ctx = f.h.app.agent_context(TOKEN).await?;
    std::fs::create_dir_all(&ctx.attempt.run_dir)?;
    let owner = yulius_app::auth::AuthContext {
        user: f
            .h
            .app
            .deps
            .store
            .user(&ctx.task.created_by)
            .await?
            .ok_or("owner")?,
        csrf_token: None,
        session_hash: None,
        token: None,
    };
    f.h.app.runtime(&f.task);
    f.h.app
        .deps
        .store
        .transition_task(
            &f.task,
            None,
            yulius_domain::task::TaskStatus::Running,
            Some(Activity::Waiting),
            f.h.app.now(),
        )
        .await?;
    f.h.clock.0.fetch_add(3_600_000, Ordering::SeqCst);
    Ok(owner)
}

async fn untracked_parking_fixture()
-> Result<(Fixture, yulius_app::auth::AuthContext), Box<dyn std::error::Error>> {
    use yulius_domain::delivery::DeliveryStage;
    let f = fixture().await?;
    install_compatibility_pull(&f).await?;
    let record =
        f.h.app
            .deps
            .store
            .delivery(&f.task)
            .await?
            .ok_or("delivery")?;
    assert!(
        f.h.app
            .deps
            .store
            .transition_delivery(
                &DeliveryStage::PrOpen,
                &yulius_app::delivery::DeliveryRecord {
                    stage: DeliveryStage::None,
                    pr_number: None,
                    ..record
                }
            )
            .await?
    );
    let owner = idle_for_parking(&f).await?;
    attach_idle_run(&f, &f.h.app).await?;
    Ok((f, owner))
}

#[tokio::test]
async fn a_failing_same_branch_pr_from_a_fork_does_not_keep_an_answered_task_live() -> TestResult {
    use yulius_app::ports::github::ChecksState;
    let (f, owner) = untracked_parking_fixture().await?;
    f.h.github.pulls.lock().map_err(|_| "pull fixture")?[0].from_fork = true;
    f.h.github.set_checks("abc123", ChecksState::Failure);
    let overview = f.h.app.git_overview(&owner, f.sandbox.as_str()).await?;
    assert!(
        overview
            .tasks
            .iter()
            .find(|row| row.task_id == f.task.as_str())
            .ok_or("task")?
            .pr
            .is_none(),
        "a fork does not own the local task row"
    );
    assert!(
        overview
            .pulls
            .iter()
            .any(|p| p.number == 1461 && p.checks == "failure")
    );
    assert_eq!(
        f.h.app.park_idle_sessions_now().await,
        1,
        "only the unrelated fork is red"
    );
    Ok(())
}

#[tokio::test]
async fn parking_rechecks_a_green_overview_for_an_externally_opened_pr() -> TestResult {
    use yulius_app::ports::github::ChecksState;
    let (f, owner) = untracked_parking_fixture().await?;
    f.h.github.set_checks("abc123", ChecksState::Success);
    let overview = f.h.app.git_overview(&owner, f.sandbox.as_str()).await?;
    assert_eq!(
        overview
            .tasks
            .iter()
            .find(|t| t.task_id == f.task.as_str())
            .and_then(|t| t.pr.as_ref())
            .map(|p| p.checks.as_str()),
        Some("success")
    );
    f.h.github.pulls.lock().map_err(|_| "pull fixture")?[0].head_sha = "external-push".into();
    f.h.github.set_checks("external-push", ChecksState::Failure);
    assert_eq!(f.h.app.park_idle_sessions_now().await, 0);
    f.h.github.fail_overview_pulls.store(true, Ordering::SeqCst);
    let partial = f.h.app.git_overview(&owner, f.sandbox.as_str()).await?;
    assert!(
        partial
            .notes
            .iter()
            .any(|note| note.contains("can't be listed"))
    );
    assert!(
        partial
            .tasks
            .iter()
            .find(|t| t.task_id == f.task.as_str())
            .and_then(|t| t.pr.as_ref())
            .is_some_and(|pr| pr.number == 1461 && pr.checks == "success")
    );
    assert_eq!(
        f.h.app.park_idle_sessions_now().await,
        0,
        "a partial overview hid the external red PR"
    );
    assert_eq!(f.h.app.send_digests_now(Some(1)).await.parked, 0);
    let cold = yulius_app::context::App::new(f.h.app.config.clone(), f.h.app.deps.clone());
    attach_idle_run(&f, &cold).await?;
    assert_eq!(
        cold.park_idle_sessions_now().await,
        0,
        "an external PR is also found after restart"
    );
    f.h.github.set_checks("external-push", ChecksState::Success);
    assert_eq!(f.h.app.park_idle_sessions_now().await, 1);
    Ok(())
}

#[tokio::test]
async fn a_merged_recorded_pr_cannot_park_a_new_failing_branch_pr() -> TestResult {
    use yulius_app::ports::github::ChecksState;
    let f = fixture().await?;
    install_compatibility_pull(&f).await?;
    let owner = idle_for_parking(&f).await?;
    attach_idle_run(&f, &f.h.app).await?;
    {
        let mut pulls = f.h.github.pulls.lock().map_err(|_| "pull fixture")?;
        let replacement = yulius_app::ports::github::GhPull {
            number: 1462,
            head_sha: "new-pr".into(),
            ..pulls[0].clone()
        };
        pulls[0].state = "closed".into();
        pulls[0].merged = true;
        pulls.push(replacement);
    }
    f.h.github.set_checks("new-pr", ChecksState::Failure);
    let overview = f.h.app.git_overview(&owner, f.sandbox.as_str()).await?;
    assert_eq!(
        overview
            .tasks
            .iter()
            .find(|t| t.task_id == f.task.as_str())
            .and_then(|t| t.pr.as_ref())
            .map(|p| p.number),
        Some(1462)
    );
    assert_eq!(
        f.h.app
            .deps
            .store
            .delivery(&f.task)
            .await?
            .and_then(|d| d.pr_number),
        Some(1461)
    );
    assert_eq!(
        f.h.app.park_idle_sessions_now().await,
        0,
        "the external replacement is failing"
    );
    assert_eq!(f.h.app.send_digests_now(Some(1)).await.parked, 0);
    // Reconciliation of the original merge must not settle the replacement.
    let recorded =
        f.h.app
            .deps
            .store
            .delivery(&f.task)
            .await?
            .ok_or("recorded PR")?;
    assert!(
        f.h.app
            .deps
            .store
            .transition_delivery(
                &recorded.stage,
                &yulius_app::delivery::DeliveryRecord {
                    stage: yulius_domain::delivery::DeliveryStage::MergedIntegration,
                    ..recorded.clone()
                }
            )
            .await?
    );
    assert_eq!(
        f.h.app.park_idle_sessions_now().await,
        0,
        "only the older PR merged"
    );
    f.h.github.set_checks("new-pr", ChecksState::Success);
    assert_eq!(f.h.app.park_idle_sessions_now().await, 1);
    Ok(())
}

#[tokio::test]
async fn parking_checks_a_release_pr_against_its_actual_base() -> TestResult {
    use yulius_app::ports::github::ChecksState;
    let (f, _) = untracked_parking_fixture().await?;
    let head =
        f.h.app
            .deps
            .git
            .resolve_commit(&f.worktree, "HEAD")
            .await?
            .ok_or("task head")?;
    assert_eq!(
        f.h.app
            .deps
            .git
            .resolve_commit(&f.worktree, "origin/dev")
            .await?
            .as_deref(),
        Some(head.as_str())
    );
    {
        let mut pulls = f.h.github.pulls.lock().map_err(|_| "pull fixture")?;
        pulls[0].base = "release".into();
        pulls[0].head_sha = head.clone();
        pulls[0].mergeable = Some(false);
    }
    f.h.github.set_checks(&head, ChecksState::Success);
    assert_eq!(
        f.h.app.park_idle_sessions_now().await,
        0,
        "GitHub reports a conflict with release even though the local dev head is identical"
    );
    f.h.github.pulls.lock().map_err(|_| "pull fixture")?[0].mergeable = None;
    assert_eq!(
        f.h.app.park_idle_sessions_now().await,
        0,
        "release mergeability has not been reported"
    );
    f.h.github.pulls.lock().map_err(|_| "pull fixture")?[0].mergeable = Some(true);
    assert_eq!(f.h.app.park_idle_sessions_now().await, 1);
    Ok(())
}

#[tokio::test]
async fn parking_needs_positive_mergeability_when_the_integration_pr_head_is_missing_locally()
-> TestResult {
    use yulius_app::ports::github::ChecksState;
    let (f, _) = untracked_parking_fixture().await?;
    let absent = "eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee";
    assert!(
        f.h.app
            .deps
            .git
            .resolve_commit(&f.worktree, "origin/dev")
            .await?
            .is_some()
    );
    assert!(
        f.h.app
            .deps
            .git
            .resolve_commit(&f.worktree, absent)
            .await?
            .is_none()
    );
    f.h.github.set_checks(absent, ChecksState::Success);
    for evidence in [None, Some(false)] {
        {
            let mut pulls = f.h.github.pulls.lock().map_err(|_| "pull fixture")?;
            pulls[0].head_sha = absent.into();
            pulls[0].mergeable = evidence;
        }
        assert_eq!(
            f.h.app.park_idle_sessions_now().await,
            0,
            "the missing local head cannot prove mergeability: {evidence:?}"
        );
    }
    f.h.github.pulls.lock().map_err(|_| "pull fixture")?[0].mergeable = Some(true);
    assert_eq!(f.h.app.park_idle_sessions_now().await, 1);
    Ok(())
}

#[tokio::test]
async fn parking_keeps_an_external_green_pr_that_must_update_its_base() -> TestResult {
    let (f, _) = untracked_parking_fixture().await?;
    f.h.github
        .set_checks("abc123", yulius_app::ports::github::ChecksState::Success);
    f.h.github.pulls.lock().map_err(|_| "pull fixture")?[0].behind = true;
    assert_eq!(f.h.app.park_idle_sessions_now().await, 0);
    f.h.github.pulls.lock().map_err(|_| "pull fixture")?[0].behind = false;
    assert_eq!(f.h.app.park_idle_sessions_now().await, 1);
    Ok(())
}

#[tokio::test]
async fn parking_rechecks_a_recorded_pr_when_its_base_advances_during_checks() -> TestResult {
    let f = fixture().await?;
    install_compatibility_pull(&f).await?;
    idle_for_parking(&f).await?;
    attach_idle_run(&f, &f.h.app).await?;
    // This recorded delivery is visible by number even though branch discovery omits it.
    f.h.github.pulls.lock().map_err(|_| "pull fixture")?[0].head = "published-elsewhere".into();
    f.h.github
        .set_checks("abc123", yulius_app::ports::github::ChecksState::Success);
    let held = f.h.github.hold_checks();
    let app = f.h.app.clone();
    let checking = tokio::spawn(async move { app.park_idle_sessions_now().await });
    tokio::time::timeout(
        std::time::Duration::from_secs(5),
        f.h.github.checks_entered.notified(),
    )
    .await?;
    f.h.github.pulls.lock().map_err(|_| "pull fixture")?[0].behind = true;
    f.h.github.release_checks(&held);
    assert_eq!(checking.await?, 0, "the second read requires a base update");
    f.h.github.pulls.lock().map_err(|_| "pull fixture")?[0].behind = false;
    assert_eq!(f.h.app.park_idle_sessions_now().await, 1);
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn parking_waits_for_external_checks_when_an_old_local_run_finishes() -> TestResult {
    use yulius_app::ci::CiRunUpdate;
    use yulius_domain::ci::{CiRef, RunState};
    let (f, _) = untracked_parking_fixture().await?;
    let mut config = f.h.app.config.clone();
    config.ci.enabled = false;
    let app = yulius_app::context::App::new(config, f.h.app.deps.clone());
    attach_idle_run(&f, &app).await?;
    f.h.github
        .set_checks("abc123", yulius_app::ports::github::ChecksState::Pending);
    super::delivery::insert_ci_attempt(
        &f.h.store,
        "old-ordinary",
        "abc123",
        CiRef::PullRequest(1461),
        RunState::Queued,
        10,
        &[],
    )
    .await?;
    let store = app.deps.store.clone();
    let handle = tokio::runtime::Handle::current();
    yulius_app::race::on(
        yulius_app::race::Point::ParkPrepared,
        Path::new(f.task.as_str()),
        move || {
            assert!(
                tokio::task::block_in_place(|| handle.block_on(store.transition_ci_run(
                    "old-ordinary",
                    &[RunState::Queued],
                    RunState::Passed,
                    &CiRunUpdate::default()
                )))
                .is_ok()
            );
        },
    );
    assert_eq!(
        app.park_idle_sessions_now().await,
        0,
        "an independent pending check still owns the decision"
    );
    // Once the external check passes the final lookup can accept the old run's completion.
    f.h.github
        .set_checks("abc123", yulius_app::ports::github::ChecksState::Success);
    assert_eq!(app.park_idle_sessions_now().await, 1);
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn parking_keeps_an_external_pr_when_its_checked_head_starts_an_unwatched_rerun() -> TestResult
{
    use yulius_app::ci::{CiRunRecord, CiRunUpdate};
    use yulius_domain::ci::{CiRef, GateSource, RunState};
    let (f, _) = untracked_parking_fixture().await?;
    f.h.github
        .set_checks("abc123", yulius_app::ports::github::ChecksState::Success);
    let run = CiRunRecord {
        id: "external-rerun".into(),
        sandbox_id: f.sandbox.clone(),
        reference: CiRef::PullRequest(1461),
        sha: "abc123".into(),
        source: GateSource::ProjectConfig,
        state: RunState::Queued,
        reason: None,
        requested_by: "yulius".into(),
        status_for: None,
        status_note: None,
        status_attempts: 0,
        queued_at: Timestamp(20),
        started_at: None,
        ended_at: None,
    };
    let app = f.h.app.clone();
    let handle = tokio::runtime::Handle::current();
    yulius_app::race::on(
        yulius_app::race::Point::ParkPrepared,
        Path::new(f.task.as_str()),
        move || {
            assert!(
                tokio::task::block_in_place(
                    || handle.block_on(app.deps.store.insert_ci_run(&run, &[]))
                )
                .is_ok()
            );
        },
    );
    assert_eq!(f.h.app.park_idle_sessions_now().await, 0);
    assert!(
        f.h.app
            .deps
            .store
            .transition_ci_run(
                "external-rerun",
                &[RunState::Queued],
                RunState::Passed,
                &CiRunUpdate::default()
            )
            .await?
    );
    assert_eq!(f.h.app.park_idle_sessions_now().await, 1);
    Ok(())
}

/// Both overviews inspect parked task #2 before the partial one waits on #1's recorded PR.
#[tokio::test]
async fn incomplete_git_overviews_preserve_the_current_parked_pr_observation() -> TestResult {
    let f = fixture().await?;
    install_compatibility_pull(&f).await?;
    let owner = idle_for_parking(&f).await?;
    f.h.app
        .deps
        .store
        .set_activity(&f.task, Some(Activity::Working), f.h.app.now())
        .await?;
    let id = TaskId::parse("t-other")?;
    let store = &f.h.app.deps.store;
    store
        .transition_task(
            &id,
            None,
            TaskStatus::Parked,
            Some(Activity::Waiting),
            f.h.app.now(),
        )
        .await?;
    f.h.github
        .set_checks("abc123", yulius_app::ports::github::ChecksState::Success);
    f.h.github
        .set_checks("external", yulius_app::ports::github::ChecksState::Failure);
    let caller = crate::task_message::superagent(&f.sandbox, &owner.user.id, "admin");
    for open in [true, false] {
        // Omit both PRs; task #1 now needs a direct lookup. Hold it after #2's row was read.
        f.h.github.fail_overview_pulls.store(true, Ordering::SeqCst);
        let gate = Arc::new(tokio::sync::Notify::new());
        *f.h.github.pull_read_hold.lock().map_err(|_| "get pull")? = Some(gate.clone());
        let pending = f.h.app.git_overview(&owner, f.sandbox.as_str());
        tokio::pin!(pending);
        tokio::select! {
            result = &mut pending => return Err(format!("partial overview completed before the barrier: {result:?}").into()),
            result = tokio::time::timeout(std::time::Duration::from_secs(5), f.h.github.pull_read_entered.notified()) => result?,
        }
        f.h.github
            .fail_overview_pulls
            .store(false, Ordering::SeqCst);
        *f.h.github.pull_read_hold.lock().map_err(|_| "get pull")? = None;
        {
            let mut pulls = f.h.github.pulls.lock().map_err(|_| "pulls")?;
            if open {
                let external = yulius_app::ports::github::GhPull {
                    number: 1462,
                    head: "feat/2-x".into(),
                    head_sha: "external".into(),
                    base: "release".into(),
                    mergeable: Some(false),
                    ..pulls[0].clone()
                };
                pulls.push(external);
            } else {
                pulls.retain(|pr| pr.number != 1462);
            }
        }
        let fresh = f.h.app.git_overview(&owner, f.sandbox.as_str()).await?;
        if open {
            let pr = fresh
                .tasks
                .iter()
                .find(|t| t.task_id == id.as_str())
                .and_then(|t| t.pr.as_ref())
                .ok_or("external PR")?;
            assert!(pr.conflicts, "use the mergeability of the task's own PR");
        }
        gate.notify_one();
        pending.await?;
        let list =
            f.h.app
                .tool_tasks_list(&caller, &json!({"scope":"needs_you"}))
                .await?;
        let row = list["tasks"]
            .as_array()
            .ok_or("tasks")?
            .iter()
            .find(|row| row["task_id"] == id.as_str());
        assert_eq!(
            row.is_some(),
            open,
            "the partial overview overwrote the concurrent complete observation"
        );
        if let Some(row) = row {
            assert_eq!(
                row["wait_reason"],
                "The task is parked and pull request #1462 checks are failing."
            );
        }
        assert_eq!(
            f.h.app.list_sandboxes(&owner).await?[0]
                .counts
                .needs_attention,
            u32::from(open)
        );
    }
    // A full upstream reply may still omit the external PR from the capped overview.
    {
        let mut pulls = f.h.github.pulls.lock().map_err(|_| "pulls")?;
        let external = yulius_app::ports::github::GhPull {
            number: 1462,
            head: "feat/2-x".into(),
            head_sha: "external".into(),
            base: "release".into(),
            ..pulls[0].clone()
        };
        pulls.push(external);
    }
    f.h.app.git_overview(&owner, f.sandbox.as_str()).await?;
    let cap = yulius_app::delivery::MAX_PULLS;
    {
        let mut pulls = f.h.github.pulls.lock().map_err(|_| "pulls")?;
        for i in 1..cap {
            let other = yulius_app::ports::github::GhPull {
                number: u64::try_from(i)?,
                head: format!("other-{i}"),
                ..pulls[0].clone()
            };
            pulls.insert(0, other);
        }
    }
    let limited = f.h.app.git_overview(&owner, f.sandbox.as_str()).await?;
    assert_eq!(limited.pulls.len(), cap);
    assert!(limited.pulls.iter().all(|pr| pr.number != 1462));
    assert_eq!(
        f.h.app.list_sandboxes(&owner).await?[0]
            .counts
            .needs_attention,
        1
    );
    f.h.github
        .pulls
        .lock()
        .map_err(|_| "pulls")?
        .retain(|pr| pr.number != 1462);
    assert_eq!(
        f.h.app
            .git_overview(&owner, f.sandbox.as_str())
            .await?
            .pulls
            .len(),
        cap
    );
    assert_eq!(
        f.h.app.list_sandboxes(&owner).await?[0]
            .counts
            .needs_attention,
        0
    );
    Ok(())
}

#[tokio::test]
async fn a_partial_overview_still_resolves_the_parked_tasks_recorded_pr() -> TestResult {
    let f = fixture().await?;
    install_compatibility_pull(&f).await?;
    let owner = idle_for_parking(&f).await?;
    f.h.app
        .deps
        .store
        .transition_task(
            &f.task,
            None,
            TaskStatus::Parked,
            Some(Activity::Done),
            f.h.app.now(),
        )
        .await?;
    f.h.github.pulls.lock().map_err(|_| "pull fixture")?[0].base = "release".into();
    let unavailable =
        f.h.github
            .pulls
            .lock()
            .map_err(|_| "pull fixture")?
            .pop()
            .ok_or("recorded PR")?;
    f.h.app.git_overview(&owner, f.sandbox.as_str()).await?;
    f.h.github
        .pulls
        .lock()
        .map_err(|_| "pull fixture")?
        .push(unavailable);
    f.h.github
        .set_checks("abc123", yulius_app::ports::github::ChecksState::Failure);
    // The browser renders the task summary before its independent Git overview is available.
    let detail = f.h.app.task_detail(&owner, f.task.as_str()).await?;
    assert_eq!(serde_json::to_value(detail)?["parked_needs_you"], true);
    f.h.github
        .set_checks("abc123", yulius_app::ports::github::ChecksState::Success);
    let detail = f.h.app.task_detail(&owner, f.task.as_str()).await?;
    let summary = serde_json::to_value(detail)?;
    assert_eq!(summary["parked_needs_you"], false);
    assert_eq!(summary["parked_pr"], json!({"number": 1461, "open": true}));
    let held = f.h.github.hold_checks();
    let app = f.h.app.clone();
    let auth = owner.clone();
    let id = f.task.clone();
    let reading = tokio::spawn(async move { app.task_detail(&auth, id.as_str()).await });
    tokio::time::timeout(
        std::time::Duration::from_secs(5),
        f.h.github.checks_entered.notified(),
    )
    .await?;
    f.h.github.pulls.lock().map_err(|_| "pulls")?[0].state = "closed".into();
    f.h.github.release_checks(&held);
    let during = serde_json::to_value(reading.await??)?;
    assert_eq!(during["parked_needs_you"], true);
    assert_eq!(during["parked_pr"], json!({"number": 1461, "open": false}));
    // The delivery panel reconciled the closed, unmerged record during that
    // detail read. Reopening it starts a new branch observation.
    // The reopened PR merges and a replacement opens while failing checks are in flight.
    f.h.github
        .set_checks("abc123", yulius_app::ports::github::ChecksState::Failure);
    f.h.github.pulls.lock().map_err(|_| "pulls")?[0].state = "open".into();
    // Terminal display reads are local. Let the repository tick observe the reopening.
    f.h.app.reconcile_deliveries().await;
    let held = f.h.github.hold_checks();
    let app = f.h.app.clone();
    let auth = owner.clone();
    let id = f.task.clone();
    let reading = tokio::spawn(async move { app.task_detail(&auth, id.as_str()).await });
    tokio::time::timeout(
        std::time::Duration::from_secs(5),
        f.h.github.checks_entered.notified(),
    )
    .await?;
    {
        let mut pulls = f.h.github.pulls.lock().map_err(|_| "pulls")?;
        pulls[0].merged = true;
        pulls[0].state = "closed".into();
        pulls.push(yulius_app::ports::github::GhPull {
            number: 1462,
            head_sha: "replacement".into(),
            ..compatibility_pull()
        });
    }
    f.h.github.release_checks(&held);
    let during = serde_json::to_value(reading.await??)?;
    assert_eq!(during["parked_needs_you"], true);
    assert_eq!(during["parked_pr"], json!({"number": 1462, "open": true}));
    Ok(())
}

async fn owner_observed_external_pr()
-> Result<(Fixture, yulius_app::auth::AuthContext), Box<dyn std::error::Error>> {
    let f = fixture().await?;
    let owner = idle_for_parking(&f).await?;
    sign_in_with_github(&f, "u1", "alice").await?;
    f.h.app
        .deps
        .store
        .transition_task(
            &f.task,
            None,
            TaskStatus::Parked,
            Some(Activity::Done),
            f.h.app.now(),
        )
        .await?;
    f.h.github
        .pulls
        .lock()
        .map_err(|_| "pulls")?
        .push(yulius_app::ports::github::GhPull {
            base: "release".into(),
            ..compatibility_pull()
        });
    f.h.github
        .set_checks("abc123", yulius_app::ports::github::ChecksState::Failure);
    let overview = f.h.app.git_overview(&owner, f.sandbox.as_str()).await?;
    assert!(
        overview
            .tasks
            .iter()
            .find(|t| t.task_id == f.task.as_str())
            .and_then(|t| t.pr.as_ref())
            .is_some_and(|pr| pr.checks == "failure")
    );
    Ok((f, owner))
}

#[tokio::test]
async fn released_parked_check_reads_leave_the_next_two_reads_unblocked() -> TestResult {
    use yulius_app::model::Secret;
    use yulius_app::ports::github::GitHubPort;
    let github = support::FakeGitHub::default();
    let hold = github.hold_checks();
    github.release_checks(&hold);
    let token = Secret("fixture".into());
    for sha in ["first", "second"] {
        tokio::time::timeout(
            std::time::Duration::from_secs(1),
            github.checks(&token, "acme", "fixture", sha),
        )
        .await??;
    }
    Ok(())
}

#[tokio::test]
async fn a_parked_detail_keeps_the_failed_pr_when_another_pr_opens_during_checks() -> TestResult {
    use yulius_domain::delivery::DeliveryStage;
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum Publication {
        OnBranch,
        RecordedElsewhere,
        Superseded,
        WithdrawnTracked,
    }
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum Checks {
        GitHubRed,
        OrdinaryRed,
        Green,
    }
    use Checks::{GitHubRed, Green, OrdinaryRed};
    use Publication::{OnBranch, RecordedElsewhere, Superseded, WithdrawnTracked};
    for (checks, publication, expected) in [
        (GitHubRed, OnBranch, 1461),
        (OrdinaryRed, OnBranch, 1461),
        (GitHubRed, RecordedElsewhere, 1461),
        (OrdinaryRed, RecordedElsewhere, 1461),
        (GitHubRed, Superseded, 1462),
        (OrdinaryRed, Superseded, 1462),
        (GitHubRed, WithdrawnTracked, 1462),
        (OrdinaryRed, WithdrawnTracked, 1462),
        (Green, OnBranch, 1462),
    ] {
        let (f, owner) = owner_observed_external_pr().await?;
        install_app(&f).await?;
        if publication == RecordedElsewhere {
            f.h.github.pulls.lock().map_err(|_| "pulls")?[0].head = "published-elsewhere".into();
        }
        if matches!(publication, RecordedElsewhere | WithdrawnTracked) {
            assert!(
                f.h.app
                    .deps
                    .store
                    .transition_delivery(
                        &DeliveryStage::None,
                        &yulius_app::delivery::DeliveryRecord {
                            task_id: f.task.clone(),
                            stage: DeliveryStage::PrOpen,
                            pr_number: Some(1461),
                            updated_at: f.h.app.now(),
                        },
                    )
                    .await?
            );
        }
        let held = f.h.github.hold_checks();
        let mut config = f.h.app.config.clone();
        config.ci.enabled = false;
        let app = yulius_app::context::App::new(config, f.h.app.deps.clone());
        if checks != GitHubRed {
            f.h.github
                .set_checks("abc123", yulius_app::ports::github::ChecksState::Success);
        }
        if checks == OrdinaryRed {
            super::delivery::insert_ci_attempt(
                &f.h.store,
                "durable-survivor",
                "abc123",
                yulius_domain::ci::CiRef::PullRequest(1461),
                yulius_domain::ci::RunState::Failed,
                10,
                &[],
            )
            .await?;
        }
        let id = f.task.clone();
        let reading = tokio::spawn(async move { app.task_detail(&owner, id.as_str()).await });
        tokio::time::timeout(
            std::time::Duration::from_secs(5),
            f.h.github.checks_entered.notified(),
        )
        .await?;
        f.h.github
            .pulls
            .lock()
            .map_err(|_| "pulls")?
            .push(yulius_app::ports::github::GhPull {
                number: 1462,
                base: "release-2".into(),
                head_sha: "healthy".into(),
                ..compatibility_pull()
            });
        f.h.github
            .set_checks("healthy", yulius_app::ports::github::ChecksState::Success);
        if publication == Superseded {
            f.h.github
                .change_before_branch_list(1461, |pr| pr.head_sha = "repaired".into())?;
        } else if publication == WithdrawnTracked {
            f.h.github
                .change_before_branch_list(1461, |pr| pr.state = "closed".into())?;
        }
        f.h.github.release_checks(&held);
        let detail = serde_json::to_value(reading.await??)?;
        assert_eq!(detail["parked_needs_you"], true);
        assert_eq!(
            detail["parked_pr"],
            json!({"number":expected,"open":true}),
            "{checks:?}, {publication:?}"
        );
    }
    Ok(())
}

#[tokio::test]
async fn a_parked_detail_keeps_the_confirmed_open_pr_when_its_peer_disappears() -> TestResult {
    let (f, owner) = owner_observed_external_pr().await?;
    install_app(&f).await?;
    f.h.github
        .set_checks("abc123", yulius_app::ports::github::ChecksState::Success);
    f.h.github
        .pulls
        .lock()
        .map_err(|_| "pulls")?
        .push(yulius_app::ports::github::GhPull {
            number: 1462,
            base: "release-2".into(),
            ..compatibility_pull()
        });
    let held = f.h.github.hold_checks();
    let app = f.h.app.clone();
    let id = f.task.clone();
    let reading = tokio::spawn(async move { app.task_detail(&owner, id.as_str()).await });
    tokio::time::timeout(
        std::time::Duration::from_secs(5),
        f.h.github.checks_entered.notified(),
    )
    .await?;
    f.h.github
        .change_before_branch_list(1462, |pr| pr.state = "closed".into())?;
    f.h.github.release_checks(&held);
    let detail = serde_json::to_value(reading.await??)?;
    assert_eq!(detail["parked_needs_you"], true);
    assert_eq!(detail["parked_pr"], json!({"number":1461,"open":true}));
    Ok(())
}

#[tokio::test]
async fn a_parked_detail_checks_each_eligibility_field_in_the_final_branch_listing() -> TestResult {
    for field in ["title", "number", "head", "base", "behind", "mergeable"] {
        let (f, owner) = owner_observed_external_pr().await?;
        install_app(&f).await?;
        f.h.github
            .set_checks("abc123", yulius_app::ports::github::ChecksState::Success);
        // The highest numbered PR is healthy and stable. Only a change to
        // eligibility should make the older PR the one named in the detail.
        f.h.github
            .pulls
            .lock()
            .map_err(|_| "pulls")?
            .push(yulius_app::ports::github::GhPull {
                number: 1463,
                base: "release-3".into(),
                ..compatibility_pull()
            });
        let held = f.h.github.hold_checks();
        let app = f.h.app.clone();
        let id = f.task.clone();
        let reading = tokio::spawn(async move { app.task_detail(&owner, id.as_str()).await });
        tokio::time::timeout(
            std::time::Duration::from_secs(5),
            f.h.github.checks_entered.notified(),
        )
        .await?;
        f.h.github
            .change_before_branch_list(1461, |pr| match field {
                "title" => pr.title = "Owner edited title".into(),
                "number" => pr.number = 1462,
                "head" => pr.head_sha = "new-head".into(),
                "base" => pr.base = "next-release".into(),
                "behind" => pr.behind = true,
                "mergeable" => pr.mergeable = Some(false),
                _ => unreachable!(),
            })?;
        f.h.github.release_checks(&held);
        let detail = serde_json::to_value(reading.await??)?;
        assert_eq!(detail["parked_needs_you"], field != "title", "{field}");
        let number = match field {
            "title" => 1463,
            "number" => 1462,
            _ => 1461,
        };
        assert_eq!(
            detail["parked_pr"],
            json!({"number":number,"open":true}),
            "{field}"
        );
    }
    Ok(())
}

#[tokio::test]
async fn a_parked_detail_keeps_a_direct_merge_when_the_branch_still_lists_it_open() -> TestResult {
    let (f, owner) = owner_observed_external_pr().await?;
    install_app(&f).await?;
    *f.h.github
        .merged_on_direct_read
        .lock()
        .map_err(|_| "pull")? = Some(1461);
    let detail = serde_json::to_value(f.h.app.task_detail(&owner, f.task.as_str()).await?)?;
    assert_eq!(detail["parked_needs_you"], false);
    assert_eq!(detail["parked_pr"], json!({"number":1461,"open":false}));
    Ok(())
}

#[tokio::test]
async fn parking_reports_the_checks_transport_error_and_keeps_the_session() -> TestResult {
    let (f, _) = untracked_parking_fixture().await?;
    let log = tempfile::NamedTempFile::new()?;
    let subscriber = tracing_subscriber::fmt()
        .with_ansi(false)
        .with_writer(std::sync::Mutex::new(log.reopen()?))
        .finish();
    let _guard = tracing::subscriber::set_default(subscriber);
    *f.h.github.checks_error.lock().map_err(|_| "checks")? = Some(
        yulius_app::error::AppError::Upstream("checks fixture unreachable".into()),
    );
    assert_eq!(f.h.app.park_idle_sessions_now().await, 0);
    let written = std::fs::read_to_string(log.path())?;
    assert!(
        written.contains("a park failed") && written.contains("checks fixture unreachable"),
        "{written}"
    );
    Ok(())
}

#[tokio::test]
async fn a_parked_detail_rechecks_a_blocked_pr_that_merges_during_verification() -> TestResult {
    for behind in [true, false] {
        let (f, owner) = owner_observed_external_pr().await?;
        install_app(&f).await?;
        f.h.github.pulls.lock().map_err(|_| "pulls")?[0].behind = behind;
        let hold = if behind {
            let hold = Arc::new(tokio::sync::Notify::new());
            *f.h.github.pull_read_hold.lock().map_err(|_| "pull hold")? = Some(hold.clone());
            hold
        } else {
            *f.h.github.checks_error.lock().map_err(|_| "checks")? =
                Some(yulius_app::error::AppError::Upstream("unavailable".into()));
            f.h.github.hold_checks()
        };
        let app = f.h.app.clone();
        let id = f.task.clone();
        let reading = tokio::spawn(async move { app.task_detail(&owner, id.as_str()).await });
        let limit = std::time::Duration::from_secs(5);
        if behind {
            tokio::time::timeout(limit, f.h.github.pull_read_entered.notified()).await?;
            hold.notify_one();
            tokio::time::timeout(limit, f.h.github.pull_read_entered.notified()).await?;
            *f.h.github.pull_read_hold.lock().map_err(|_| "pull hold")? = None;
        } else {
            tokio::time::timeout(limit, f.h.github.checks_entered.notified()).await?;
        }
        *f.h.github
            .merged_on_direct_read
            .lock()
            .map_err(|_| "merged")? = Some(1461);
        if behind {
            hold.notify_one();
        } else {
            f.h.github.release_checks(&hold);
        }
        let detail = serde_json::to_value(reading.await??)?;
        assert_eq!(detail["parked_needs_you"], false, "behind={behind}");
        assert_eq!(
            detail["parked_pr"],
            json!({"number":1461,"open":false}),
            "behind={behind}"
        );
    }
    Ok(())
}

#[tokio::test]
async fn a_parked_task_names_the_blocking_pr_when_its_branch_has_two_open_prs() -> TestResult {
    let (f, owner) = owner_observed_external_pr().await?;
    install_app(&f).await?;
    f.h.github
        .set_checks("abc123", yulius_app::ports::github::ChecksState::Success);
    {
        let mut pulls = f.h.github.pulls.lock().map_err(|_| "pulls")?;
        pulls[0].behind = true;
        pulls.push(yulius_app::ports::github::GhPull {
            number: 1462,
            base: "release-2".into(),
            ..compatibility_pull()
        });
    }
    for behind in [true, false] {
        {
            let mut pulls = f.h.github.pulls.lock().map_err(|_| "pulls")?;
            pulls[0].behind = behind;
            pulls[0].mergeable = Some(behind);
        }
        let summary = serde_json::to_value(f.h.app.task_detail(&owner, f.task.as_str()).await?)?;
        assert_eq!(summary["parked_needs_you"], true);
        assert_eq!(summary["parked_pr"], json!({"number":1461, "open":true}));
        let caller = crate::task_message::superagent(&f.sandbox, &owner.user.id, "admin");
        let brief =
            f.h.app
                .tool_task_get(&caller, &json!({"task_id":f.task.as_str()}))
                .await?;
        let reason = if behind {
            "The parked task's pull request needs to be updated with its base branch."
        } else {
            "The parked task's pull request has a merge conflict."
        };
        assert_eq!(brief["wait_reason"], reason);
    }
    Ok(())
}

#[tokio::test]
async fn a_parked_task_names_the_ordinary_ci_failure_on_the_older_open_pr() -> TestResult {
    use yulius_domain::ci::{CiRef, RunState};
    let (f, owner) = owner_observed_external_pr().await?;
    install_app(&f).await?;
    {
        let mut pulls = f.h.github.pulls.lock().map_err(|_| "pulls")?;
        pulls.push(yulius_app::ports::github::GhPull {
            number: 1462,
            base: "release-2".into(),
            ..compatibility_pull()
        });
    }
    f.h.github
        .set_checks("abc123", yulius_app::ports::github::ChecksState::Success);
    assert_eq!(f.sandbox.as_str(), "s1");
    super::delivery::insert_ci_attempt(
        &f.h.store,
        "older-ordinary-red",
        "abc123",
        CiRef::PullRequest(1461),
        RunState::Failed,
        10,
        &[],
    )
    .await?;
    super::delivery::insert_ci_attempt(
        &f.h.store,
        "newer-ordinary-green",
        "abc123",
        CiRef::PullRequest(1462),
        RunState::Passed,
        11,
        &[],
    )
    .await?;
    let mut config = f.h.app.config.clone();
    config.ci.enabled = false;
    let app = yulius_app::context::App::new(config, f.h.app.deps.clone());
    let summary = serde_json::to_value(app.task_detail(&owner, f.task.as_str()).await?)?;
    assert_eq!(summary["parked_needs_you"], true);
    assert_eq!(summary["parked_pr"], json!({"number":1461,"open":true}));
    let caller = crate::task_message::superagent(&f.sandbox, &owner.user.id, "admin");
    let brief = app
        .tool_task_get(&caller, &json!({"task_id":f.task.as_str()}))
        .await?;
    assert_eq!(brief["parked_pr"], json!({"number":1461,"open":true}));
    assert_eq!(
        brief["wait_reason"],
        "The task is parked and pull request #1461 checks are failing."
    );
    f.h.github
        .set_checks("abc123", yulius_app::ports::github::ChecksState::Pending);
    let pending = app
        .tool_task_get(&caller, &json!({"task_id":f.task.as_str()}))
        .await?;
    assert_eq!(
        pending["wait_reason"],
        "The task is parked and pull request #1461 checks are failing."
    );
    f.h.github
        .set_checks("abc123", yulius_app::ports::github::ChecksState::Success);
    // Losing the checks endpoint does not make the current PR disappear.
    *f.h.github
        .checks_error
        .lock()
        .map_err(|_| "checks fixture")? = Some(yulius_app::error::AppError::Upstream(
        "checks unavailable".into(),
    ));
    let red = app
        .tool_tasks_list(&caller, &json!({"scope":"needs_you"}))
        .await?;
    let row = red["tasks"]
        .as_array()
        .ok_or("rows")?
        .iter()
        .find(|row| row["task_id"] == f.task.as_str())
        .ok_or("failed PR task")?;
    assert_eq!(row["parked_pr"], json!({"number":1461,"open":true}));
    assert_eq!(
        row["wait_reason"],
        "The task is parked and pull request #1461 checks are failing."
    );
    let unavailable = serde_json::to_value(app.task_detail(&owner, f.task.as_str()).await?)?;
    assert_eq!(unavailable["parked_needs_you"], true);
    assert_eq!(
        unavailable["parked_pr"],
        json!({"number":1461, "open":true})
    );
    *f.h.github
        .checks_error
        .lock()
        .map_err(|_| "checks fixture")? = None;
    // With CI enabled, the newest, failed run belongs to the *other* PR.
    super::delivery::insert_ci_attempt(
        &f.h.store,
        "older-now-green",
        "abc123",
        CiRef::PullRequest(1461),
        RunState::Passed,
        12,
        &[],
    )
    .await?;
    super::delivery::insert_ci_attempt(
        &f.h.store,
        "newer-now-red",
        "abc123",
        CiRef::PullRequest(1462),
        RunState::Failed,
        13,
        &[],
    )
    .await?;
    let mut config = app.config.clone();
    config.ci.enabled = true;
    let enabled = yulius_app::context::App::new(config, app.deps.clone());
    let summary = serde_json::to_value(enabled.task_detail(&owner, f.task.as_str()).await?)?;
    assert_eq!(summary["parked_needs_you"], true);
    assert_eq!(summary["parked_pr"], json!({"number":1462,"open":true}));
    Ok(())
}

#[tokio::test]
async fn a_parked_task_ignores_its_owner_observation_once_the_same_pr_is_merged() -> TestResult {
    use yulius_domain::delivery::DeliveryStage;
    let (f, owner) = owner_observed_external_pr().await?;
    f.h.app
        .deps
        .store
        .transition_delivery(
            &DeliveryStage::None,
            &yulius_app::delivery::DeliveryRecord {
                task_id: f.task.clone(),
                stage: DeliveryStage::MergedIntegration,
                pr_number: Some(1461),
                updated_at: f.h.app.now(),
            },
        )
        .await?;
    let detail = serde_json::to_value(f.h.app.task_detail(&owner, f.task.as_str()).await?)?;
    assert_eq!(detail["parked_needs_you"], false);
    assert_eq!(detail["parked_pr"], json!({"number":1461,"open":false}));
    let ctx = f.h.app.agent_context(TOKEN).await?;
    f.h.app
        .tool_steps_set(&ctx, &json!({"steps":["Document the follow-up"]}))
        .await?;
    let detail = serde_json::to_value(f.h.app.task_detail(&owner, f.task.as_str()).await?)?;
    assert_eq!(detail["parked_needs_you"], true);
    assert_eq!(detail["parked_pr"], json!({"number":1461, "open":false}));
    let caller = crate::task_message::superagent(&f.sandbox, &owner.user.id, "admin");
    let brief =
        f.h.app
            .tool_task_get(&caller, &json!({"task_id":f.task.as_str()}))
            .await?;
    assert_eq!(
        brief["wait_reason"],
        "The task is parked with unfinished steps in its plan (1 open, next: \"Document the follow-up\")."
    );
    Ok(())
}

async fn parked_with_recorded_merge()
-> Result<(Fixture, yulius_app::auth::AuthContext), Box<dyn std::error::Error>> {
    use yulius_domain::delivery::DeliveryStage;
    let f = fixture().await?;
    let owner = idle_for_parking(&f).await?;
    install_app(&f).await?;
    let store = &f.h.app.deps.store;
    store
        .transition_task(
            &f.task,
            None,
            TaskStatus::Parked,
            Some(Activity::Done),
            f.h.app.now(),
        )
        .await?;
    assert!(
        store
            .transition_delivery(
                &DeliveryStage::None,
                &yulius_app::delivery::DeliveryRecord {
                    task_id: f.task.clone(),
                    stage: DeliveryStage::MergedIntegration,
                    pr_number: Some(1491),
                    updated_at: f.h.app.now(),
                }
            )
            .await?
    );
    Ok((f, owner))
}

#[tokio::test]
async fn a_parked_merge_survives_unavailable_followup_discovery_on_every_display() -> TestResult {
    let (f, owner) = parked_with_recorded_merge().await?;
    let app = &f.h.app;
    let caller = crate::task_message::superagent(&f.sandbox, &owner.user.id, "admin");
    app.deps.store.set_installation(None).await?;
    f.h.github.no_installation.store(true, Ordering::SeqCst);
    let detail = app.task_detail(&owner, f.task.as_str()).await?;
    assert_eq!(
        detail.summary.parked_needs_you,
        Some(false),
        "{:?}",
        detail.summary.needs_you_notice
    );
    assert_eq!(
        serde_json::to_value(detail.summary.parked_pr)?,
        json!({"number":1491,"open":false})
    );
    assert!(detail.summary.needs_you_notice.is_none());
    assert!(detail.wait_history.is_empty());
    app.deps
        .store
        .record_wait_message(&yulius_app::ports::store_ext::WaitMessageRecord {
            task_id: f.task.clone(),
            episode: "parked:errored:1000".into(),
            source: "system".into(),
            happened: "The parked task's pull request could not be verified.".into(),
            action: "Open the task and resume it or rerun CI.".into(),
            problem: None,
            link: String::new(),
            decision: false,
            created_at: Timestamp(1000),
            resolved_at: None,
        })
        .await?;
    let rows = app.list_tasks(&owner, f.sandbox.as_str(), false).await?;
    let row = rows
        .iter()
        .find(|r| r.id == f.task.as_str())
        .ok_or("task")?;
    assert_eq!(row.parked_needs_you, Some(false));
    assert!(row.needs_you_notice.is_none());
    let history = app.task_detail(&owner, f.task.as_str()).await?.wait_history;
    assert_eq!(history.len(), 1);
    assert!(history[0].resolved_at.is_some());
    assert_eq!(
        app.list_sandboxes(&owner).await?[0].counts.needs_attention,
        0
    );
    let brief = app
        .tool_task_get(&caller, &json!({"task_id":f.task.as_str()}))
        .await?;
    assert_eq!(brief["shows"], "done");
    let needs = app
        .tool_tasks_list(&caller, &json!({"scope":"needs_you"}))
        .await?;
    assert!(needs["tasks"].as_array().ok_or("rows")?.is_empty());

    // A display fallback is insufficient evidence to drop a prerequisite.
    let created = app
        .tool_task_create(
            &caller,
            &json!({"prompt":"Wait for the current tasks","harness":"claude", "after_all_tasks":true}),
        )
        .await?;
    assert_eq!(created["task"]["start_rule"], "#1 done and #2 done");
    Ok(())
}

#[tokio::test]
async fn a_parked_merge_survives_a_stalled_branch_lookup_and_finds_a_replacement_on_recovery()
-> TestResult {
    let (f, owner) = parked_with_recorded_merge().await?;
    let mut config = f.h.app.config.clone();
    config.parked_pr_read_ms = 750;
    let app = yulius_app::context::App::new(config, f.h.app.deps.clone());
    let caller = crate::task_message::superagent(&f.sandbox, &owner.user.id, "admin");
    let args = json!({"task_id":f.task.as_str()});
    *f.h.github
        .pull_list_hold
        .lock()
        .map_err(|_| "branch fixture")? = Some(Arc::new(tokio::sync::Notify::new()));
    let read = app.tool_task_get(&caller, &args);
    tokio::pin!(read);
    tokio::select! {
        result = &mut read => return Err(format!("discovery never started: {result:?}").into()),
        () = f.h.github.pull_list_entered.notified() => {}
    }
    let brief = tokio::time::timeout(std::time::Duration::from_secs(2), read).await??;
    assert_eq!(brief["shows"], "done", "{brief}");
    assert_eq!(brief["parked_pr"], json!({"number":1491,"open":false}));

    *f.h.github
        .pull_list_hold
        .lock()
        .map_err(|_| "branch fixture")? = None;
    f.h.github
        .pulls
        .lock()
        .map_err(|_| "pulls")?
        .push(yulius_app::ports::github::GhPull {
            number: 1492,
            base: "release".into(),
            ..compatibility_pull()
        });
    f.h.github
        .set_checks("abc123", yulius_app::ports::github::ChecksState::Failure);
    let fresh = app.tool_task_get(&caller, &args).await?;
    assert_eq!(fresh["shows"], "needs_you");
    assert_eq!(fresh["parked_pr"], json!({"number":1492,"open":true}));
    // A direct task read discovered the replacement before the Git overview ran.
    app.deps.store.set_installation(None).await?;
    f.h.github.no_installation.store(true, Ordering::SeqCst);
    let incomplete = app.tool_task_get(&caller, &args).await?;
    assert_eq!(incomplete["shows"], "needs_you");
    assert_eq!(incomplete["parked_pr"], json!({"number":1492,"open":true}));
    f.h.github.no_installation.store(false, Ordering::SeqCst);
    f.h.github.pulls.lock().map_err(|_| "pulls")?.clear();
    assert_eq!(
        app.task_detail(&owner, f.task.as_str())
            .await?
            .summary
            .parked_needs_you,
        Some(false)
    );
    f.h.github.no_installation.store(true, Ordering::SeqCst);
    assert_eq!(app.tool_task_get(&caller, &args).await?["shows"], "done");
    Ok(())
}

async fn verify_followup_outage(closed: bool, second: bool, expected: Option<u64>) -> TestResult {
    let (f, owner) = parked_with_recorded_merge().await?;
    let app = &f.h.app;
    let caller = crate::task_message::superagent(&f.sandbox, &owner.user.id, "admin");
    let args = json!({"task_id":f.task.as_str()});
    {
        let mut pulls = f.h.github.pulls.lock().map_err(|_| "pulls")?;
        pulls.push(yulius_app::ports::github::GhPull {
            number: 1492,
            base: "release".into(),
            ..compatibility_pull()
        });
        if second {
            pulls.push(yulius_app::ports::github::GhPull {
                number: 1493,
                base: "release-2".into(),
                head_sha: "other-head".into(),
                ..compatibility_pull()
            });
        }
    }
    *f.h.github
        .merged_on_direct_read
        .lock()
        .map_err(|_| "direct merge")? = closed.then_some(1492);
    f.h.github
        .set_checks("abc123", yulius_app::ports::github::ChecksState::Failure);
    f.h.github.set_checks(
        "other-head",
        yulius_app::ports::github::ChecksState::Success,
    );
    let verified = app.tool_task_get(&caller, &args).await?;
    assert_eq!(verified["parked_pr"]["number"], expected.unwrap_or(1492));
    assert_eq!(verified["parked_pr"]["open"], expected.is_some());
    app.deps.store.set_installation(None).await?;
    f.h.github.no_installation.store(true, Ordering::SeqCst);
    let outage = app.tool_task_get(&caller, &args).await?;
    assert_eq!(verified["shows"], if closed { "done" } else { "needs_you" });
    assert_eq!(outage["shows"], verified["shows"]);
    assert_eq!(
        outage["parked_pr"],
        json!({"number":expected.unwrap_or(1491), "open":expected.is_some()})
    );
    Ok(())
}

#[tokio::test]
async fn followup_discovery_keeps_direct_closures_during_an_outage() -> TestResult {
    verify_followup_outage(true, false, None).await?;
    verify_followup_outage(true, true, Some(1493)).await
}

#[tokio::test]
async fn followup_discovery_keeps_blocker_identity_during_an_outage() -> TestResult {
    verify_followup_outage(false, true, Some(1492)).await
}

async fn parked_followup_with_stalled_final_list(merged: bool) -> TestResult {
    let (f, owner) = parked_with_recorded_merge().await?;
    let mut config = f.h.app.config.clone();
    config.parked_pr_read_ms = 750;
    let app = yulius_app::context::App::new(config, f.h.app.deps.clone());
    f.h.github
        .pulls
        .lock()
        .map_err(|_| "pulls")?
        .push(yulius_app::ports::github::GhPull {
            number: 1492,
            base: "release".into(),
            ..compatibility_pull()
        });
    f.h.github
        .set_checks("abc123", yulius_app::ports::github::ChecksState::Failure);
    let caller = crate::task_message::superagent(&f.sandbox, &owner.user.id, "admin");
    let args = json!({"task_id":f.task.as_str()});
    assert_eq!(
        app.tool_task_get(&caller, &args).await?["shows"],
        "needs_you"
    );
    let gate = Arc::new(tokio::sync::Notify::new());
    *f.h.github
        .pull_read_hold
        .lock()
        .map_err(|_| "direct hold")? = Some(gate.clone());
    let after_listing = async {
        tokio::time::timeout(
            std::time::Duration::from_secs(5),
            f.h.github.pull_read_entered.notified(),
        )
        .await?;
        {
            let mut pulls = f.h.github.pulls.lock().map_err(|_| "pulls")?;
            pulls[0].state = "closed".into();
            pulls[0].merged = merged;
        }
        *f.h.github
            .pull_read_hold
            .lock()
            .map_err(|_| "direct hold")? = None;
        *f.h.github.pull_list_hold.lock().map_err(|_| "list hold")? =
            Some(Arc::new(tokio::sync::Notify::new()));
        gate.notify_one();
        tokio::time::timeout(
            std::time::Duration::from_secs(5),
            f.h.github.pull_list_entered.notified(),
        )
        .await?;
        Ok::<_, Box<dyn std::error::Error>>(())
    };
    let (status, held) = tokio::join!(app.tool_task_get(&caller, &args), after_listing);
    held?;
    let status = status?;
    if merged {
        assert_eq!(status["shows"], "done", "{status}");
        assert_eq!(status["parked_pr"], json!({"number":1491,"open":false}));
        assert_eq!(app.tool_task_get(&caller, &args).await?["shows"], "done");
    } else {
        assert_eq!(status["shows"], "needs_you", "{status}");
        assert_eq!(status["parked_pr"], json!({"number":1492,"open":false}));
    }
    Ok(())
}

#[tokio::test]
async fn a_parked_merge_survives_stalled_final_discovery_after_direct_closure() -> TestResult {
    parked_followup_with_stalled_final_list(true).await
}

#[tokio::test]
async fn a_parked_followup_closed_without_merge_still_requires_verification() -> TestResult {
    parked_followup_with_stalled_final_list(false).await
}

#[tokio::test]
async fn a_parked_merge_survives_saturated_followup_discovery_and_remains_a_prerequisite()
-> TestResult {
    let (f, owner) = parked_with_recorded_merge().await?;
    let app = &f.h.app;
    let caller = crate::task_message::superagent(&f.sandbox, &owner.user.id, "admin");
    let args = json!({"task_id":f.task.as_str()});
    let gate = Arc::new(tokio::sync::Notify::new());
    f.h.github.hold_overview_list.store(true, Ordering::SeqCst);
    *f.h.github
        .pull_list_hold
        .lock()
        .map_err(|_| "overview hold")? = Some(gate.clone());
    let reads = async {
        tokio::time::timeout(
            std::time::Duration::from_secs(5),
            f.h.github.pull_list_entered.notified(),
        )
        .await?;
        for _ in 0..30 {
            assert_eq!(app.tool_task_get(&caller, &args).await?["shows"], "done");
        }
        let saturated = app.tool_task_get(&caller, &args).await?;
        assert_eq!(saturated["shows"], "done", "{saturated}");
        assert_eq!(saturated["parked_pr"], json!({"number":1491,"open":false}));
        let created = app
            .tool_task_create(
                &caller,
                &json!({"prompt":"Wait for the current tasks","harness":"claude","after_all_tasks":true}),
            )
            .await?;
        assert_eq!(created["task"]["start_rule"], "#1 done and #2 done");
        gate.notify_one();
        Ok::<_, Box<dyn std::error::Error>>(())
    };
    let overview = async {
        Ok::<_, Box<dyn std::error::Error>>(app.git_overview(&owner, f.sandbox.as_str()).await?)
    };
    tokio::try_join!(overview, reads)?;
    Ok(())
}

#[tokio::test]
async fn a_parked_merge_rechecks_local_work_after_stalled_followup_discovery() -> TestResult {
    let (f, owner) = parked_with_recorded_merge().await?;
    let ctx = f.h.app.agent_context(TOKEN).await?;
    let mut config = f.h.app.config.clone();
    config.parked_pr_read_ms = 750;
    let app = yulius_app::context::App::new(config, f.h.app.deps.clone());
    let caller = crate::task_message::superagent(&f.sandbox, &owner.user.id, "admin");
    *f.h.github
        .pull_list_hold
        .lock()
        .map_err(|_| "branch fixture")? = Some(Arc::new(tokio::sync::Notify::new()));
    let args = json!({"task_id":f.task.as_str()});
    let read = app.tool_task_get(&caller, &args);
    let reopen = async {
        tokio::time::timeout(
            std::time::Duration::from_secs(2),
            f.h.github.pull_list_entered.notified(),
        )
        .await?;
        app.tool_steps_set(&ctx, &json!({"steps":["Inspect the follow-up"]}))
            .await?;
        Ok::<_, Box<dyn std::error::Error>>(())
    };
    let (result, reopened) = tokio::join!(read, reopen);
    reopened?;
    let brief = result?;
    assert_eq!(brief["shows"], "needs_you");
    assert!(
        brief["wait_reason"]
            .as_str()
            .ok_or("wait reason")?
            .contains("Inspect the follow-up")
    );
    Ok(())
}

#[tokio::test]
async fn a_parked_task_with_open_steps_keeps_its_owner_observed_pr() -> TestResult {
    let (f, owner) = owner_observed_external_pr().await?;
    let ctx = f.h.app.agent_context(TOKEN).await?;
    f.h.app
        .tool_steps_set(&ctx, &json!({"steps":["Fix the failing check"]}))
        .await?;
    let row = f.h.app.task_detail(&owner, f.task.as_str()).await?;
    assert_eq!(row.summary.parked_needs_you, Some(true));
    assert_eq!(
        serde_json::to_value(row.summary.parked_pr)?,
        json!({"number":1461,"open":true})
    );
    Ok(())
}

#[tokio::test]
async fn parking_a_finished_turn_without_an_app_uses_its_own_delivery_record() -> TestResult {
    let (f, owner) = owner_observed_external_pr().await?;
    idle_for_parking(&f).await?;
    attach_idle_run(&f, &f.h.app).await?;
    f.h.github
        .set_checks("abc123", yulius_app::ports::github::ChecksState::Success);
    f.h.app.git_overview(&owner, f.sandbox.as_str()).await?;
    assert_eq!(f.h.app.park_idle_sessions_now().await, 1);
    Ok(())
}

#[tokio::test]
async fn a_parked_external_pr_seen_by_its_owner_requires_verification_without_an_app() -> TestResult
{
    let (f, owner) = owner_observed_external_pr().await?;
    let detail = serde_json::to_value(f.h.app.task_detail(&owner, f.task.as_str()).await?)?;
    assert_eq!(detail["parked_needs_you"], true);
    assert_eq!(detail["parked_pr"], json!({"number":1461, "open":true}));
    // Even a previously green head needs current evidence from the installation.
    f.h.github
        .set_checks("abc123", yulius_app::ports::github::ChecksState::Success);
    f.h.app.git_overview(&owner, f.sandbox.as_str()).await?;
    assert_eq!(
        serde_json::to_value(f.h.app.task_detail(&owner, f.task.as_str()).await?)?["parked_needs_you"],
        true
    );
    install_app(&f).await?;
    assert_eq!(
        serde_json::to_value(f.h.app.task_detail(&owner, f.task.as_str()).await?)?["parked_needs_you"],
        false
    );
    Ok(())
}

#[tokio::test]
async fn a_parked_external_pr_keeps_its_identity_when_the_app_has_no_installation() -> TestResult {
    let (f, owner) = owner_observed_external_pr().await?;
    install_app(&f).await?;
    f.h.app.deps.store.set_installation(None).await?;
    f.h.github.no_installation.store(true, Ordering::SeqCst);
    f.h.github
        .set_checks("abc123", yulius_app::ports::github::ChecksState::Failure);
    f.h.app.git_overview(&owner, f.sandbox.as_str()).await?;
    let summary = serde_json::to_value(f.h.app.task_detail(&owner, f.task.as_str()).await?)?;
    assert_eq!(
        summary["parked_pr"],
        json!({"number":1461,"open":true}),
        "configured app lacks repository installation"
    );
    assert_eq!(summary["parked_needs_you"], true);
    f.h.github.no_installation.store(false, Ordering::SeqCst);
    f.h.github
        .set_checks("abc123", yulius_app::ports::github::ChecksState::Success);
    assert_eq!(
        serde_json::to_value(f.h.app.task_detail(&owner, f.task.as_str()).await?)?["parked_needs_you"],
        false
    );
    Ok(())
}

#[tokio::test]
async fn parked_list_and_home_classify_a_green_head_while_the_first_is_slow() -> TestResult {
    use yulius_app::ports::github::ChecksState;
    use yulius_app::ports::store::{SandboxStore as _, TaskStore as _};
    use yulius_app::ports::store_ext::DeliveryStore as _;
    use yulius_domain::delivery::DeliveryStage;
    let f = fixture().await?;
    install_compatibility_pull(&f).await?;
    let owner = idle_for_parking(&f).await?;
    let mut slow = f.h.store.task(&f.task).await?.ok_or("task")?;
    let now = f.h.app.now();
    f.h.store
        .transition_task(
            &slow.id,
            None,
            TaskStatus::Parked,
            Some(Activity::Waiting),
            now,
        )
        .await?;
    let slow_id = TaskId::parse("parked-slow")?;
    slow.id = slow_id.clone();
    slow.number = 12;
    slow.branch = Some("feat/slow".into());
    slow.status = TaskStatus::Parked;
    slow.activity = Some(Activity::Waiting);
    slow.updated_at = Timestamp(now.0 + 1);
    f.h.store.insert_task(&slow).await?;
    // Seven healthy parked PRs and the stalled PR fill the first batch.
    // The original healthy PR gets its own share in the second batch.
    for number in 13..20 {
        let mut fast = slow.clone();
        fast.id = TaskId::parse(&format!("parked-fast-{number}"))?;
        fast.number = number;
        fast.branch = Some(format!("feat/fast-{number}"));
        f.h.store.insert_task(&fast).await?;
    }
    let original = f.h.store.delivery(&f.task).await?.ok_or("PR")?;
    f.h.store
        .transition_delivery(
            &DeliveryStage::None,
            &yulius_app::delivery::DeliveryRecord {
                task_id: slow_id.clone(),
                pr_number: Some(1462),
                ..original
            },
        )
        .await?;
    {
        let mut pulls = f.h.github.pulls.lock().map_err(|_| "pulls")?;
        // The fixture's external PRs have authoritative remote mergeability.
        // No local merge-driver subprocess participates in the list deadline.
        pulls[0].base = "release".into();
        let other = yulius_app::ports::github::GhPull {
            number: 1462,
            head_sha: "slow-head".into(),
            head: "feat/slow".into(),
            ..pulls[0].clone()
        };
        pulls.push(other);
        for number in 13..20 {
            let fast = yulius_app::ports::github::GhPull {
                number: 1500 + number,
                head: format!("feat/fast-{number}"),
                ..pulls[0].clone()
            };
            pulls.push(fast);
        }
    }
    f.h.github.set_checks("abc123", ChecksState::Failure);
    f.h.github.set_checks("slow-head", ChecksState::Pending);
    f.h.app.git_overview(&owner, f.sandbox.as_str()).await?;
    f.h.github.set_checks("abc123", ChecksState::Success);
    let hold = f.h.github.hold_checks_for("slow-head")?;
    let rows =
        f.h.app
            .list_tasks(&owner, f.sandbox.as_str(), false)
            .await?;
    assert_eq!(
        rows.get(7).map(|row| row.id.as_str()),
        Some(slow_id.as_str())
    );
    assert_eq!(
        rows.get(9).map(|row| row.id.as_str()),
        Some(f.task.as_str())
    );
    assert_eq!(
        rows.iter()
            .find(|t| t.id == f.task.as_str())
            .and_then(|t| t.parked_needs_you),
        Some(false)
    );
    assert_eq!(
        rows.iter()
            .find(|t| t.id == slow_id.as_str())
            .and_then(|t| t.parked_needs_you),
        Some(true)
    );
    let project = f.h.store.sandbox(&f.sandbox).await?.ok_or("project")?;
    assert_eq!(
        f.h.app
            .sandbox_view(&project, yulius_domain::permission::AccessLevel::Write)
            .await?
            .counts
            .needs_attention,
        1
    );
    f.h.github.release_checks(&hold);
    Ok(())
}

#[tokio::test]
async fn parked_lists_share_one_budget_for_unreadable_pr_heads() -> TestResult {
    use std::time::Duration;
    use yulius_domain::delivery::DeliveryStage;
    let f = fixture_with_read_budget(None, |_| {}, Agent::ClaudeCode, 750).await?;
    let ctx = f.h.app.agent_context(TOKEN).await?;
    install_compatibility_pull(&f).await?;
    let store = &f.h.app.deps.store;
    let owner = yulius_app::auth::AuthContext {
        user: store.user(&ctx.task.owner).await?.ok_or("owner")?,
        csrf_token: None,
        session_hash: None,
        token: None,
    };
    let now = f.h.app.now();
    f.h.app
        .tool_steps_set(&ctx, &json!({"steps":["Complete this open step"]}))
        .await?;
    store
        .transition_task(
            &f.task,
            None,
            TaskStatus::Parked,
            Some(Activity::Waiting),
            now,
        )
        .await?;
    let hold = f.h.github.hold_checks();
    let rows = tokio::select! {
        result = f.h.app.list_tasks(&owner, f.sandbox.as_str(), false) => result?,
        () = f.h.github.checks_entered.notified() => return Err("looked up checks despite a locally unfinished plan".into()),
    };
    let local = rows
        .iter()
        .find(|task| task.id == f.task.as_str())
        .ok_or("parked row")?;
    assert_eq!(local.parked_needs_you, Some(true));
    assert_eq!(
        local.parked_pr.map(|pr| (pr.number, pr.open)),
        Some((1461, true)),
        "locally unfinished work still names its recorded open PR"
    );
    let mut record = store.task(&f.task).await?.ok_or("parked task")?;
    for number in 3..=10 {
        record.id = TaskId::parse(&format!("t-parked-{number}"))?;
        record.number = number;
        store.insert_task(&record).await?;
        store
            .transition_delivery(
                &DeliveryStage::None,
                &yulius_app::delivery::DeliveryRecord {
                    task_id: record.id.clone(),
                    stage: DeliveryStage::PrOpen,
                    pr_number: Some(1461),
                    updated_at: now,
                },
            )
            .await?;
    }
    let limit = Duration::from_secs(4);
    let (tasks, home) = tokio::time::timeout(limit, async {
        tokio::try_join!(
            f.h.app.list_tasks(&owner, f.sandbox.as_str(), false),
            f.h.app.list_sandboxes(&owner)
        )
    })
    .await
    .map_err(|_| "parked PR reads stalled the lists")??;
    // Make the generated rule exceed its atom limit, so this measures its
    // construction independently of the later, authoritative chain evaluation.
    for number in 11..=66 {
        record.id = TaskId::parse(&format!("queued-{number}"))?;
        record.number = number;
        record.status = TaskStatus::Queued;
        record.activity = None;
        store.insert_task(&record).await?;
    }
    let caller = crate::task_message::superagent(&f.sandbox, &owner.user.id, "admin");
    let args = json!({"prompt":"Wait for the outstanding PRs", "after_all_tasks":true});
    let problem = tokio::time::timeout(limit, f.h.app.tool_task_create(&caller, &args))
        .await
        .map_err(|_| "after_all_tasks waited a full deadline for each parked task")?
        .err()
        .ok_or("unreadable parked PRs should be included in the start rule")?;
    assert!(
        problem.to_string().contains("a rule names at most 64"),
        "{problem}"
    );
    f.h.github.release_checks(&hold);
    let parked: Vec<_> = tasks
        .iter()
        .filter(|task| task.status == "parked")
        .collect();
    assert_eq!(parked.len(), 9);
    for row in parked {
        assert_eq!(row.parked_needs_you, Some(row.id == f.task.as_str()));
        if row.id != f.task.as_str() {
            assert!(row.needs_you_notice.is_none());
        }
    }
    assert!(
        home.iter()
            .find(|s| s.id == f.sandbox.as_str())
            .is_some_and(|s| s.counts.needs_attention == 1)
    );
    Ok(())
}

#[tokio::test]
async fn parked_http_reads_show_the_plan_reopened_during_verification() -> TestResult {
    for detail in [false, true] {
        let (f, owner) = owner_observed_external_pr().await?;
        install_app(&f).await?;
        let ctx = f.h.app.agent_context(TOKEN).await?;
        f.h.github
            .set_checks("abc123", yulius_app::ports::github::ChecksState::Success);
        let held = f.h.github.hold_checks();
        let read = async {
            if detail {
                let view = f.h.app.task_detail(&owner, f.task.as_str()).await?;
                return Ok::<_, Box<dyn std::error::Error>>(serde_json::to_value(view)?);
            }
            let rows =
                f.h.app
                    .list_tasks(&owner, f.sandbox.as_str(), false)
                    .await?;
            Ok(serde_json::to_value(
                rows.iter()
                    .find(|t| t.id == f.task.as_str())
                    .ok_or("parked task")?,
            )?)
        };
        let reopen = async {
            tokio::time::timeout(
                std::time::Duration::from_secs(5),
                f.h.github.checks_entered.notified(),
            )
            .await?;
            f.h.app
                .tool_steps_set(&ctx, &json!({"steps":["Verify the repair"]}))
                .await?;
            f.h.github.release_checks(&held);
            Ok::<_, Box<dyn std::error::Error>>(())
        };
        let (row, updated) = tokio::join!(read, reopen);
        updated?;
        let row = row?;
        assert_eq!(row["parked_needs_you"], true);
        assert_eq!(row["steps"]["total"], 1, "detail={detail}");
        if detail {
            assert_eq!(row["steps_list"][0]["title"], "Verify the repair");
        }
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn parked_http_rows_classify_the_same_plan_they_return() -> TestResult {
    for detail in [false, true] {
        let (f, owner) = owner_observed_external_pr().await?;
        install_app(&f).await?;
        f.h.github
            .set_checks("abc123", yulius_app::ports::github::ChecksState::Success);
        let ctx = f.h.app.agent_context(TOKEN).await?;
        let (at, resume) =
            support::hold_point(yulius_app::race::Point::ParkedStatusRead, f.task.as_str());
        let app = Arc::clone(&f.h.app);
        let id = f.task.clone();
        let sandbox = f.sandbox.clone();
        let owner_copy = owner.clone();
        let read = tokio::spawn(async move {
            if detail {
                let row = app.task_detail(&owner_copy, id.as_str()).await?;
                assert!(row.steps_list.is_empty());
                Ok::<_, yulius_app::error::AppError>(row.summary)
            } else {
                let rows = app.list_tasks(&owner_copy, sandbox.as_str(), false).await?;
                rows.into_iter()
                    .find(|row| row.id == id.as_str())
                    .ok_or(yulius_app::error::AppError::NotFound)
            }
        });
        support::point_reached(at).await?;
        f.h.app
            .tool_steps_set(&ctx, &json!({"steps":["Verify the repair"]}))
            .await?;
        resume.send(())?;
        let row = read.await??;
        assert_eq!(row.parked_needs_you, Some(false), "detail={detail}");
        assert_eq!(row.steps.total, 0, "the verified plan was empty");
        let next = f.h.app.task_detail(&owner, f.task.as_str()).await?;
        assert_eq!(next.summary.parked_needs_you, Some(true));
        assert_eq!(next.steps_list[0].title, "Verify the repair");
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_parked_row_with_reopened_declared_work_cannot_show_done() -> TestResult {
    use yulius_app::ports::store::CoordinationStore as _;
    for spec in [true, false] {
        let (f, owner) = owner_observed_external_pr().await?;
        install_app(&f).await?;
        f.h.github
            .set_checks("abc123", yulius_app::ports::github::ChecksState::Success);
        let (at, resume) =
            support::hold_point(yulius_app::race::Point::ParkedStatusRead, f.task.as_str());
        let app = Arc::clone(&f.h.app);
        let id = f.task.clone();
        let read = tokio::spawn(async move { app.task_detail(&owner, id.as_str()).await });
        support::point_reached(at).await?;
        if spec {
            f.h.store
                .set_spec_status(&f.task, Some("in-progress"))
                .await?;
        } else {
            let ctx = f.h.app.agent_context(TOKEN).await?;
            std::fs::write(
                std::path::Path::new(&ctx.attempt.run_dir).join("parked-goal-unfinished"),
                [],
            )?;
        }
        resume.send(())?;
        let row = read.await??.summary;
        assert_eq!(row.spec_unfinished, spec);
        assert_eq!(row.goal_unfinished, !spec);
        assert_eq!(row.parked_needs_you, Some(true));
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn parked_http_rows_honor_the_owner_decisions_they_display() -> TestResult {
    for detail in [false, true] {
        for hold in [false, true] {
            let (f, owner) = owner_observed_external_pr().await?;
            install_app(&f).await?;
            f.h.github
                .set_checks("abc123", yulius_app::ports::github::ChecksState::Success);
            let (at, resume) =
                support::hold_point(yulius_app::race::Point::ParkedStatusRead, f.task.as_str());
            let app = Arc::clone(&f.h.app);
            let id = f.task.clone();
            let sandbox = f.sandbox.clone();
            let read = tokio::spawn(async move {
                if detail {
                    Ok::<_, yulius_app::error::AppError>(
                        app.task_detail(&owner, id.as_str()).await?.summary,
                    )
                } else {
                    app.list_tasks(&owner, sandbox.as_str(), false)
                        .await?
                        .into_iter()
                        .find(|row| row.id == id.as_str())
                        .ok_or(yulius_app::error::AppError::NotFound)
                }
            });
            support::point_reached(at).await?;
            let store = &f.h.app.deps.store;
            if hold {
                store
                    .insert_hold(&yulius_app::intake::HoldRecord {
                        id: "owner-hold".into(),
                        task_id: f.task.clone(),
                        class: yulius_domain::intake::PromptClass::NewScope,
                        prompt: "Review this change".into(),
                        prompt_sha256: "0".repeat(64),
                        percent: 100,
                        typed_by: None,
                        state: yulius_app::intake::hold_state::PENDING.into(),
                        created_at: f.h.app.now(),
                        resolved_by: None,
                        resolved_at: None,
                        new_task_id: None,
                    })
                    .await?;
            } else {
                let decision = serde_json::from_value(
                    json!({"action":"Choose the release","decision":true,"at":1}),
                )?;
                store
                    .set_task_needs_you(&f.task, Some(&decision), f.h.app.now())
                    .await?;
            }
            resume.send(())?;
            let row = read.await??;
            assert_eq!(row.held_prompt.is_some(), hold);
            assert_eq!(row.needs_you.is_some(), !hold);
            assert_eq!(row.parked_needs_you, Some(true));
        }
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn live_row_pr_news_cannot_consume_a_later_parked_prs_budget() -> TestResult {
    use yulius_app::ports::store::TaskStore as _;
    for home in [false, true] {
        let (f, owner) = owner_observed_external_pr().await?;
        install_app(&f).await?;
        f.h.github
            .set_checks("abc123", yulius_app::ports::github::ChecksState::Success);
        let mut earlier = f.h.store.task(&f.task).await?.ok_or("task")?;
        for number in 10..18 {
            earlier.id = TaskId::parse(&format!("earlier-{number}"))?;
            earlier.number = number;
            earlier.status = if number == 17 {
                TaskStatus::Running
            } else {
                TaskStatus::Closed
            };
            earlier.activity = (number == 17).then_some(Activity::Waiting);
            earlier.branch = None;
            f.h.store.insert_task(&earlier).await?;
        }
        let (local_started, release) =
            support::hold_point(yulius_app::race::Point::LiveRowPrNews, "earlier-17");
        let (verified, ready) = tokio::sync::oneshot::channel();
        yulius_app::race::on(
            yulius_app::race::Point::ParkedStatusRead,
            std::path::Path::new(f.task.as_str()),
            move || {
                let _ = verified.send(());
            },
        );
        let app = Arc::clone(&f.h.app);
        let id = f.task.clone();
        let sandbox = f.sandbox.clone();
        let read = tokio::spawn(async move {
            if home {
                let projects = app.list_sandboxes(&owner).await?;
                Ok::<_, yulius_app::error::AppError>(
                    projects
                        .iter()
                        .any(|p| p.id == sandbox.as_str() && p.counts.needs_attention == 0),
                )
            } else {
                let rows = app.list_tasks(&owner, sandbox.as_str(), false).await?;
                Ok(rows
                    .iter()
                    .any(|r| r.id == id.as_str() && r.parked_needs_you == Some(false)))
            }
        });
        support::point_reached(local_started).await?;
        let parked_finished = tokio::time::timeout(std::time::Duration::from_secs(2), ready).await;
        release.send(())?;
        assert!(
            parked_finished.is_ok(),
            "the unrelated live row blocked parked verification, home={home}"
        );
        assert!(
            read.await??,
            "healthy parked row failed verification, home={home}"
        );
    }
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn parked_verification_keeps_the_lists_snapshot_of_queued_task_holders() -> TestResult {
    use yulius_app::ports::store::TaskStore as _;
    let (f, owner) = owner_observed_external_pr().await?;
    install_app(&f).await?;
    f.h.github
        .set_checks("abc123", yulius_app::ports::github::ChecksState::Success);
    let queued_id = TaskId::parse("t-other")?;
    f.h.store
        .transition_task(&queued_id, None, TaskStatus::Queued, None, f.h.app.now())
        .await?;
    let (at, resume) =
        support::hold_point(yulius_app::race::Point::ParkedStatusRead, f.task.as_str());
    let app = Arc::clone(&f.h.app);
    let caller = owner.clone();
    let sandbox = f.sandbox.clone();
    let listing =
        tokio::spawn(async move { app.list_tasks(&caller, sandbox.as_str(), false).await });
    support::point_reached(at).await?;
    let mut added = f.h.store.task(&queued_id).await?.ok_or("queued")?;
    added.id = TaskId::parse("new-supertask")?;
    added.supertask = true;
    let added = f.h.store.insert_task(&added).await?;
    resume.send(())?;
    let first = listing.await??;
    assert!(
        first
            .iter()
            .find(|t| t.id == queued_id.as_str())
            .ok_or("queued")?
            .held_by
            .is_empty()
    );
    let next =
        f.h.app
            .list_tasks(&owner, f.sandbox.as_str(), false)
            .await?;
    assert_eq!(
        next.iter()
            .find(|t| t.id == queued_id.as_str())
            .ok_or("queued")?
            .held_by,
        [added.number]
    );
    Ok(())
}

#[tokio::test]
async fn task_detail_keeps_attempt_and_session_evidence_of_starting() -> TestResult {
    let f = fixture().await?;
    let owner = idle_for_parking(&f).await?;
    assert!(f.h.app.task_detail(&owner, f.task.as_str()).await?.started);
    assert!(!f.h.app.task_detail(&owner, "t-other").await?.started);
    f.h.app
        .deps
        .store
        .upsert_provider_session(&lineage_entry(&f, "t-other", "first", 1)?)
        .await?;
    assert!(f.h.app.task_detail(&owner, "t-other").await?.started);
    Ok(())
}

#[tokio::test]
async fn a_parked_task_keeps_a_confirmed_failure_in_an_incomplete_check_report() -> TestResult {
    use yulius_app::ports::github::{ChecksReport, ChecksSource, ChecksState};
    let (f, owner) = owner_observed_external_pr().await?;
    install_app(&f).await?;
    let caller = crate::task_message::superagent(&f.sandbox, &owner.user.id, "admin");
    for (state, unreadable, reason) in [
        (
            ChecksState::Failure,
            vec![ChecksSource::CommitStatuses],
            "The task is parked and pull request #1461 checks are failing.",
        ),
        (
            ChecksState::Pending,
            vec![],
            "The parked task is waiting for its pull request checks.",
        ),
        (
            ChecksState::Success,
            vec![ChecksSource::CommitStatuses],
            "The parked task is waiting for its pull request checks.",
        ),
    ] {
        f.h.github
            .set_checks_report("abc123", ChecksReport { state, unreadable });
        let brief =
            f.h.app
                .tool_task_get(&caller, &json!({"task_id":f.task.as_str()}))
                .await?;
        assert_eq!(brief["shows"], "needs_you");
        assert_eq!(brief["wait_reason"], reason);
    }
    Ok(())
}

#[tokio::test]
async fn a_parked_task_explains_incomplete_ordinary_checks_on_its_head() -> TestResult {
    use yulius_domain::ci::{CiRef, RunState};
    let (f, owner) = owner_observed_external_pr().await?;
    install_app(&f).await?;
    f.h.github
        .set_checks("abc123", yulius_app::ports::github::ChecksState::Success);
    let caller = crate::task_message::superagent(&f.sandbox, &owner.user.id, "admin");
    for (at, state, reason) in [
        (
            21,
            RunState::Queued,
            "The parked task is waiting for its pull request checks.",
        ),
        (
            22,
            RunState::Error,
            "CI could not complete the checks for the parked task's pull request.",
        ),
        (
            23,
            RunState::Cancelled,
            "CI could not complete the checks for the parked task's pull request.",
        ),
    ] {
        super::delivery::insert_ci_attempt(
            &f.h.store,
            &format!("local-{at}"),
            "abc123",
            CiRef::PullRequest(1461),
            state,
            at,
            &[],
        )
        .await?;
        for enabled in [false, true] {
            let mut config = f.h.app.config.clone();
            config.ci.enabled = enabled;
            let app = yulius_app::context::App::new(config, f.h.app.deps.clone());
            let brief = app
                .tool_task_get(&caller, &json!({"task_id":f.task.as_str()}))
                .await?;
            assert_eq!(brief["shows"], "needs_you");
            assert_eq!(brief["wait_reason"], reason);
        }
    }
    Ok(())
}

#[tokio::test]
async fn a_title_edit_during_a_parked_check_preserves_the_same_heads_failure() -> TestResult {
    let (f, owner) = owner_observed_external_pr().await?;
    install_app(&f).await?;
    let caller = crate::task_message::superagent(&f.sandbox, &owner.user.id, "admin");
    let args = json!({"task_id":f.task.as_str()});
    let held = f.h.github.hold_checks();
    let reading = f.h.app.tool_task_get(&caller, &args);
    let edit = async {
        tokio::time::timeout(
            std::time::Duration::from_secs(5),
            f.h.github.checks_entered.notified(),
        )
        .await?;
        f.h.github.pulls.lock().map_err(|_| "pulls")?[0].title =
            "Updated description of the same change".into();
        f.h.github.release_checks(&held);
        Ok::<_, Box<dyn std::error::Error>>(())
    };
    let (result, edited) = tokio::join!(reading, edit);
    edited?;
    assert_eq!(
        result?["wait_reason"],
        "The task is parked and pull request #1461 checks are failing."
    );
    Ok(())
}

#[tokio::test]
async fn work_reopened_during_a_parked_check_supplies_the_wait_reason() -> TestResult {
    for state in [
        yulius_app::ports::github::ChecksState::Success,
        yulius_app::ports::github::ChecksState::Failure,
    ] {
        let (f, owner) = owner_observed_external_pr().await?;
        install_app(&f).await?;
        let ctx = f.h.app.agent_context(TOKEN).await?;
        f.h.github.set_checks("abc123", state);
        let caller = crate::task_message::superagent(&f.sandbox, &owner.user.id, "admin");
        let args = json!({"task_id":f.task.as_str()});
        let held = f.h.github.hold_checks();
        let reading = f.h.app.tool_task_get(&caller, &args);
        let reopen = async {
            tokio::time::timeout(
                std::time::Duration::from_secs(5),
                f.h.github.checks_entered.notified(),
            )
            .await?;
            f.h.app
                .tool_steps_set(&ctx, &json!({"steps":["Verify the repair"]}))
                .await?;
            f.h.github.release_checks(&held);
            Ok::<_, Box<dyn std::error::Error>>(())
        };
        let (result, reopened) = tokio::join!(reading, reopen);
        reopened?;
        let task = result?;
        assert_eq!(task["shows"], "needs_you");
        assert_eq!(
            task["wait_reason"],
            "The task is parked with unfinished steps in its plan (1 open, next: \"Verify the repair\")."
        );
        assert_eq!(task["steps"]["total"], 1);
        assert_eq!(task["steps_list"][0]["title"], "Verify the repair");
    }
    Ok(())
}

/// Another test of this process (#1644): its own runtime on its own thread, as libtest runs a
/// second test beside a parked-read case, with its own fixture whose main task is also `t-main`.
/// Each [`Neighbour::read`] is one parked `task_get` of that task, which reaches
/// `ParkedStatusRead` with the same key as the case's hook.
struct Neighbour {
    asks: Option<
        std::sync::mpsc::Sender<tokio::sync::oneshot::Sender<Result<serde_json::Value, String>>>,
    >,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Neighbour {
    fn start() -> Self {
        let (asks, inbox) = std::sync::mpsc::channel::<
            tokio::sync::oneshot::Sender<Result<serde_json::Value, String>>,
        >();
        let thread = std::thread::spawn(move || {
            let Ok(runtime) = tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .enable_all()
                .build()
            else {
                return;
            };
            runtime.block_on(async move {
                let ready = owner_observed_external_pr()
                    .await
                    .map_err(|e| format!("neighbour fixture: {e}"));
                while let Ok(reply) = inbox.recv() {
                    let answer = match &ready {
                        Ok((f, owner)) => {
                            let caller = crate::task_message::superagent(
                                &f.sandbox,
                                &owner.user.id,
                                "admin",
                            );
                            // Its own hook proves the read reached the point the case holds.
                            let reached = Arc::new(std::sync::atomic::AtomicBool::new(false));
                            let seen = Arc::clone(&reached);
                            yulius_app::race::on(
                                yulius_app::race::Point::ParkedStatusRead,
                                Path::new(f.task.as_str()),
                                move || seen.store(true, Ordering::SeqCst),
                            );
                            f.h.app
                                .tool_task_get(&caller, &json!({"task_id":f.task.as_str()}))
                                .await
                                .map_err(|e| format!("neighbour read: {e}"))
                                .and_then(|row| {
                                    if reached.load(Ordering::SeqCst) {
                                        Ok(row)
                                    } else {
                                        Err("the neighbour's read never reached ParkedStatusRead"
                                            .to_owned())
                                    }
                                })
                        }
                        Err(e) => Err(e.clone()),
                    };
                    let _ = reply.send(answer);
                }
            });
        });
        Self {
            asks: Some(asks),
            thread: Some(thread),
        }
    }

    /// One parked read of the neighbour's own `t-main`, to its end.
    async fn read(&self) -> Result<serde_json::Value, Box<dyn std::error::Error>> {
        let (reply, answer) = tokio::sync::oneshot::channel();
        self.asks
            .as_ref()
            .ok_or("neighbour gone")?
            .send(reply)
            .map_err(|_| "neighbour gone")?;
        Ok(answer.await.map_err(|_| "neighbour gone")??)
    }
}

impl Drop for Neighbour {
    fn drop(&mut self) {
        drop(self.asks.take());
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// Waits on the hook's `at` in the background; the handle ends once the server reached it. With
/// a `neighbour`, its parked read of its own `t-main` runs first, while the hook is armed and
/// before this case reads: that read reaching this case's hook is the #1644 steal, an error.
async fn armed_beside(
    neighbour: Option<&Neighbour>,
    at: std::sync::mpsc::Receiver<()>,
) -> Result<
    tokio::task::JoinHandle<Result<(), std::sync::mpsc::RecvTimeoutError>>,
    Box<dyn std::error::Error>,
> {
    let mut reached =
        tokio::task::spawn_blocking(move || at.recv_timeout(std::time::Duration::from_secs(60)));
    if let Some(neighbour) = neighbour {
        tokio::select! {
            read = neighbour.read() => {
                read?;
            }
            _ = &mut reached => {
                return Err("another test's parked read of its own t-main took this test's pause hook".into());
            }
        }
    }
    Ok(reached)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn parked_mcp_explains_a_hold_dismissed_during_the_read() -> TestResult {
    hold_dismissed_during_the_read(None).await
}

#[test]
fn a_neighbours_parked_read_leaves_the_dismissed_hold_case_its_pause_hook() -> TestResult {
    let neighbour = Neighbour::start();
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()?
        .block_on(hold_dismissed_during_the_read(Some(&neighbour)))
}

async fn hold_dismissed_during_the_read(neighbour: Option<&Neighbour>) -> TestResult {
    use yulius_app::intake::{HoldRecord, hold_state};
    let (f, owner) = owner_observed_external_pr().await?;
    let store = &f.h.app.deps.store;
    store
        .insert_hold(&HoldRecord {
            id: "dismissed-during-status".into(),
            task_id: f.task.clone(),
            class: yulius_domain::intake::PromptClass::NewScope,
            prompt: "Private prompt".into(),
            prompt_sha256: "0".repeat(64),
            percent: 100,
            typed_by: None,
            state: hold_state::PENDING.into(),
            created_at: f.h.app.now(),
            resolved_by: None,
            resolved_at: None,
            new_task_id: None,
        })
        .await?;
    let caller = crate::task_message::superagent(&f.sandbox, &owner.user.id, "admin");
    let app = Arc::clone(&f.h.app);
    let args = json!({"task_id":f.task.as_str()});
    let (at, resume) =
        support::hold_point(yulius_app::race::Point::ParkedStatusRead, f.task.as_str());
    let reached = armed_beside(neighbour, at).await?;
    let read = tokio::spawn(async move { app.tool_task_get(&caller, &args).await });
    reached
        .await?
        .map_err(|_| "the server never reached the pause point")?;
    assert!(
        store
            .resolve_hold(
                "dismissed-during-status",
                hold_state::PENDING,
                hold_state::DISMISSED,
                "user:admin",
                f.h.app.now(),
                None
            )
            .await?
    );
    resume.send(())?;
    let row = read.await??;
    assert_eq!(row["shows"], "needs_you");
    assert!(row.get("held_prompt").is_none());
    assert_eq!(
        row["wait_reason"],
        "A prompt was held for a decision when the task was checked."
    );
    assert!(!row.to_string().contains("Private prompt"));
    Ok(())
}

#[tokio::test]
async fn tasks_list_skips_queue_capacity_reads_for_needs_you_nonmatches() -> TestResult {
    use std::sync::atomic::{AtomicBool, Ordering};
    use yulius_app::ports::store::TaskStore as _;
    let f = fixture().await?;
    let mut queued = f.h.store.task(&f.task).await?.ok_or("task")?;
    let caller = crate::task_message::superagent(&f.sandbox, &queued.created_by, "admin");
    let now = f.h.app.now();
    f.h.store
        .set_activity(&f.task, Some(Activity::NeedsAttention), now)
        .await?;
    queued.id = TaskId::parse("queue-before-owner")?;
    queued.status = TaskStatus::Queued;
    queued.activity = None;
    queued.updated_at = Timestamp(now.0 + 1);
    f.h.store.insert_task(&queued).await?;
    let read = Arc::new(AtomicBool::new(false));
    let seen = Arc::clone(&read);
    yulius_app::race::on(
        yulius_app::race::Point::QueuedRowCapacity,
        queued.id.as_str().as_ref(),
        move || {
            seen.store(true, Ordering::SeqCst);
        },
    );
    let page =
        f.h.app
            .tool_tasks_list(&caller, &json!({"scope":"needs_you","limit":1}))
            .await?;
    assert_eq!(page["tasks"][0]["task_id"], f.task.as_str());
    assert!(
        !read.load(Ordering::SeqCst),
        "the skipped queued row read session capacity"
    );
    let all =
        f.h.app
            .tool_tasks_list(&caller, &json!({"scope":"all","limit":1}))
            .await?;
    assert_eq!(all["tasks"].as_array().ok_or("tasks")?.len(), 1);
    assert_eq!(all["tasks"][0]["task_id"], queued.id.as_str());
    assert!(read.load(Ordering::SeqCst));
    Ok(())
}

#[tokio::test]
async fn archived_task_list_spends_its_limit_on_archived_rows() -> TestResult {
    use yulius_app::ports::store::TaskStore as _;
    let f = fixture().await?;
    let mut archived = f.h.store.task(&f.task).await?.ok_or("task")?;
    let caller = crate::task_message::superagent(&f.sandbox, &archived.created_by, "admin");
    archived.id = TaskId::parse("archived-task")?;
    archived.number = 33;
    archived.status = TaskStatus::Closed;
    archived.updated_at = Timestamp(1);
    archived.archived_at = Some(Timestamp(1));
    f.h.store.insert_task(&archived).await?;
    let result =
        f.h.app
            .tool_tasks_list(&caller, &json!({"scope":"archived","limit":1}))
            .await?;
    assert_eq!(result["tasks"][0]["task_id"], archived.id.as_str());
    let all =
        f.h.app
            .tool_tasks_list(&caller, &json!({"scope":"all"}))
            .await?;
    let rows = all["tasks"].as_array().ok_or("tasks")?;
    assert!(rows.iter().any(|row| row["task_id"] == f.task.as_str()));
    assert!(
        rows.iter()
            .any(|row| row["task_id"] == archived.id.as_str())
    );

    Ok(())
}

#[tokio::test]
async fn parked_needs_you_list_gives_two_stalled_rows_the_whole_remaining_window() -> TestResult {
    let (f, owner) = owner_observed_external_pr().await?;
    install_app(&f).await?;
    let store = &f.h.app.deps.store;
    let mut peer = store.task(&f.task).await?.ok_or("parked task")?;
    peer.id = TaskId::parse("second-held-pr")?;
    peer.updated_at = Timestamp(f.h.app.now().0 + 1);
    store.insert_task(&peer).await?;
    // Two earlier windows verify healthy parked rows. The final pair gets all
    // the unused time after those four rows leave the eligible count.
    {
        let mut pulls = f.h.github.pulls.lock().map_err(|_| "pulls")?;
        let healthy = yulius_app::ports::github::GhPull {
            number: 1462,
            head: "feat/healthy".into(),
            head_sha: "healthy".into(),
            ..pulls[0].clone()
        };
        pulls.push(healthy);
    }
    f.h.github
        .set_checks("healthy", yulius_app::ports::github::ChecksState::Success);
    for offset in 2..6 {
        let mut healthy = peer.clone();
        healthy.id = TaskId::parse(&format!("healthy-before-stall-{offset}"))?;
        healthy.branch = Some("feat/healthy".into());
        healthy.updated_at = Timestamp(f.h.app.now().0 + offset);
        store.insert_task(&healthy).await?;
    }
    let caller = crate::task_message::superagent(&f.sandbox, &owner.user.id, "admin");
    let mut config = f.h.app.config.clone();
    config.parked_pr_read_ms = 2400;
    let budget = std::time::Duration::from_millis(config.parked_pr_read_ms);
    let app = yulius_app::context::App::new(config, f.h.app.deps.clone());
    app.git_overview(&owner, f.sandbox.as_str()).await?;
    let hold = f.h.github.hold_checks_for("abc123")?;
    let start = tokio::time::Instant::now();
    let args = json!({"scope":"needs_you","limit":2});
    let reading = tokio::time::timeout(budget * 7 / 4, app.tool_tasks_list(&caller, &args)).await;
    f.h.github.release_checks(&hold);
    let reading =
        reading.map_err(|_| "two parked rows exceeded their shared MCP read budget")??;
    let rows = reading["tasks"].as_array().ok_or("tasks")?;
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0]["task_id"], peer.id.as_str());
    assert_eq!(rows[1]["task_id"], f.task.as_str());
    assert!(
        start.elapsed() >= budget,
        "the two parked rows timed out before their shared window expired: {:?}",
        start.elapsed()
    );
    Ok(())
}

#[tokio::test]
async fn parked_list_home_and_mcp_classify_a_green_head_while_the_first_is_slow() -> TestResult {
    use yulius_app::ports::github::ChecksState;
    use yulius_app::ports::store::{SandboxStore as _, TaskStore as _};
    use yulius_app::ports::store_ext::DeliveryStore as _;
    use yulius_domain::delivery::DeliveryStage;
    let f = fixture_with_read_budget(None, |_| {}, Agent::ClaudeCode, 3_000).await?;
    install_compatibility_pull(&f).await?;
    let owner = idle_for_parking(&f).await?;
    let mut slow = f.h.store.task(&f.task).await?.ok_or("task")?;
    let now = f.h.app.now();
    f.h.store
        .transition_task(
            &slow.id,
            None,
            TaskStatus::Parked,
            Some(Activity::Waiting),
            now,
        )
        .await?;
    let slow_id = TaskId::parse("parked-slow")?;
    slow.id = slow_id.clone();
    slow.number = 12;
    slow.branch = Some("feat/slow".into());
    slow.status = TaskStatus::Parked;
    slow.activity = Some(Activity::Waiting);
    slow.updated_at = Timestamp(now.0 + 1);
    f.h.store.insert_task(&slow).await?;
    // Lists order by descending task number. Six closed rows, the slow PR
    // and the fixture’s other task fill the first batch. The healthy PR is ninth.
    for number in 13..19 {
        let mut closed = slow.clone();
        closed.id = TaskId::parse(&format!("closed-{number}"))?;
        closed.number = number;
        closed.branch = None;
        closed.activity = None;
        closed.status = TaskStatus::Closed;
        f.h.store.insert_task(&closed).await?;
    }
    let original = f.h.store.delivery(&f.task).await?.ok_or("PR")?;
    f.h.store
        .transition_delivery(
            &DeliveryStage::None,
            &yulius_app::delivery::DeliveryRecord {
                task_id: slow_id.clone(),
                pr_number: Some(1462),
                ..original
            },
        )
        .await?;
    {
        let mut pulls = f.h.github.pulls.lock().map_err(|_| "pulls")?;
        // The fixture's external PRs have authoritative remote mergeability.
        // No local merge-driver subprocess participates in the list deadline.
        pulls[0].base = "release".into();
        let other = yulius_app::ports::github::GhPull {
            number: 1462,
            head_sha: "slow-head".into(),
            head: "feat/slow".into(),
            ..pulls[0].clone()
        };
        pulls.push(other);
    }
    f.h.github.set_checks("abc123", ChecksState::Failure);
    f.h.github.set_checks("slow-head", ChecksState::Pending);
    f.h.app.git_overview(&owner, f.sandbox.as_str()).await?;
    f.h.github.set_checks("abc123", ChecksState::Success);
    let hold = f.h.github.hold_checks_for("slow-head")?;
    let rows =
        f.h.app
            .list_tasks(&owner, f.sandbox.as_str(), false)
            .await?;
    assert_eq!(
        rows.get(6).map(|row| row.id.as_str()),
        Some(slow_id.as_str())
    );
    assert_eq!(
        rows.get(8).map(|row| row.id.as_str()),
        Some(f.task.as_str())
    );
    assert_eq!(
        rows.iter()
            .find(|t| t.id == f.task.as_str())
            .and_then(|t| t.parked_needs_you),
        Some(false)
    );
    assert_eq!(
        rows.iter()
            .find(|t| t.id == slow_id.as_str())
            .and_then(|t| t.parked_needs_you),
        Some(true)
    );
    let project = f.h.store.sandbox(&f.sandbox).await?.ok_or("project")?;
    assert_eq!(
        f.h.app
            .sandbox_view(&project, yulius_domain::permission::AccessLevel::Write)
            .await?
            .counts
            .needs_attention,
        1
    );
    // MCP all orders by activity. Seven other rows and the slow PR precede
    // the healthy PR at the boundary of the second batch.
    f.h.store
        .set_activity(
            &TaskId::parse("t-other")?,
            Some(Activity::Working),
            Timestamp(now.0 + 1),
        )
        .await?;
    let caller = crate::task_message::superagent(&f.sandbox, &slow.created_by, "admin");
    let all =
        f.h.app
            .tool_tasks_list(&caller, &json!({"scope":"all","limit":9}))
            .await?;
    let rows = all["tasks"].as_array().ok_or("rows")?;
    assert!(
        rows[..8]
            .iter()
            .any(|row| row["task_id"] == slow_id.as_str() && row["shows"] == "needs_you")
    );
    assert_eq!(rows[8]["task_id"], f.task.as_str());
    assert_eq!(rows[8]["shows"], "done");
    // The healthy task is newest for MCP. Apply the owner's limit only after
    // filtering Needs you, otherwise the slow task disappears from the page.
    f.h.store
        .set_activity(&f.task, Some(Activity::Waiting), Timestamp(now.0 + 2))
        .await?;
    let listing =
        f.h.app
            .tool_tasks_list(&caller, &serde_json::json!({"scope":"needs_you","limit":1}))
            .await?;
    assert_eq!(listing["tasks"].as_array().ok_or("tasks")?.len(), 1);
    assert_eq!(listing["tasks"][0]["task_id"], slow_id.as_str());
    // Ten newer local decisions qualify before the held PR is reached.
    // Consume the preceding MCP lookup's signal, then catch any needless read.
    tokio::time::timeout(
        std::time::Duration::from_secs(5),
        f.h.github.checks_entered.notified(),
    )
    .await?;
    for number in 20..30 {
        let mut asks = slow.clone();
        asks.id = TaskId::parse(&format!("decision-{number}"))?;
        asks.branch = None;
        asks.status = TaskStatus::Running;
        asks.activity = Some(Activity::NeedsAttention);
        asks.updated_at = Timestamp(now.0 + number);
        f.h.store.insert_task(&asks).await?;
    }
    let empty =
        f.h.app
            .tool_tasks_list(&caller, &json!({"scope":"needs_you","limit":0}))
            .await?;
    assert_eq!(
        empty["tasks"],
        json!([]),
        "a zero-sized page visits no rows"
    );
    let args = serde_json::json!({"scope":"needs_you","limit":1});
    let listing = tokio::select! {
        result = f.h.app.tool_tasks_list(&caller, &args) => result?,
        () = f.h.github.checks_entered.notified() => return Err("read a later parked PR after the first page was filled".into()),
    };
    assert_eq!(listing["tasks"][0]["task_id"], "decision-29");
    // The last slot of a nine-row page is in the second verification window.
    let args = json!({"scope":"needs_you","limit":9});
    let listing = tokio::select! {
        result = f.h.app.tool_tasks_list(&caller, &args) => result?,
        () = f.h.github.checks_entered.notified() => return Err("read beyond the last slot of the second window".into()),
    };
    let rows = listing["tasks"].as_array().ok_or("tasks")?;
    assert_eq!(rows.len(), 9);
    assert_eq!(rows[8]["task_id"], "decision-21");
    f.h.github.release_checks(&hold);
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 3)]
async fn parked_mcp_reads_classify_the_plan_they_return_and_honor_a_new_decision() -> TestResult {
    plan_and_decision_read_during_the_hold(None).await
}

#[test]
fn a_neighbours_parked_read_leaves_the_plan_and_decision_case_its_pause_hook() -> TestResult {
    let neighbour = Neighbour::start();
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(3)
        .enable_all()
        .build()?
        .block_on(plan_and_decision_read_during_the_hold(Some(&neighbour)))
}

async fn plan_and_decision_read_during_the_hold(neighbour: Option<&Neighbour>) -> TestResult {
    for (tool, decision) in [
        ("task_get", false),
        ("all", false),
        ("needs_you", false),
        ("task_get", true),
        ("all", true),
        ("needs_you", true),
    ] {
        let (f, owner) = owner_observed_external_pr().await?;
        install_app(&f).await?;
        f.h.github
            .set_checks("abc123", yulius_app::ports::github::ChecksState::Success);
        // List readers have a newer slow parked PR ahead of the healthy row.
        let held = if tool != "task_get" {
            let store = &f.h.app.deps.store;
            let mut slow = store.task(&f.task).await?.ok_or("parked task")?;
            slow.id = TaskId::parse("newer-stalled-pr")?;
            slow.branch = Some("feat/newer-stalled".into());
            slow.updated_at = Timestamp(f.h.app.now().0 + 1);
            store.insert_task(&slow).await?;
            let mut pulls = f.h.github.pulls.lock().map_err(|_| "pulls")?;
            let newer = yulius_app::ports::github::GhPull {
                head: "feat/newer-stalled".into(),
                number: 1462,
                head_sha: "stalled".into(),
                ..pulls[0].clone()
            };
            pulls.push(newer);
            Some(f.h.github.hold_checks_for("stalled")?)
        } else {
            None
        };
        let ctx = f.h.app.agent_context(TOKEN).await?;
        let caller = crate::task_message::superagent(&f.sandbox, &owner.user.id, "admin");
        let app = Arc::clone(&f.h.app);
        let id = f.task.clone();
        let (at, resume) =
            support::hold_point(yulius_app::race::Point::ParkedStatusRead, id.as_str());
        let reached = armed_beside(neighbour, at).await?;
        let mut read = tokio::spawn(async move {
            if tool == "task_get" {
                app.tool_task_get(&caller, &json!({"task_id":id.as_str()}))
                    .await
                    .map(Some)
            } else {
                let list = app.tool_tasks_list(&caller, &json!({"scope":tool})).await?;
                Ok(list["tasks"]
                    .as_array()
                    .and_then(|rows| rows.iter().find(|r| r["task_id"] == id.as_str()))
                    .cloned())
            }
        });
        tokio::select! {
            result = &mut read => return Err(format!("MCP returned without parked verification: {:?}", result??).into()),
            reached = reached => reached?.map_err(|_| "the server never reached the pause point")?,
        };
        f.h.app
            .tool_steps_set(&ctx, &json!({"steps":["Verify the repair"]}))
            .await?;
        if decision {
            let ask = serde_json::from_value(
                json!({"action":"Approve the repair","decision":true,"at":1}),
            )?;
            f.h.app
                .deps
                .store
                .set_task_needs_you(&f.task, Some(&ask), f.h.app.now())
                .await?;
        }
        if let Some(hold) = held {
            f.h.github.release_checks(&hold);
        }
        resume.send(())?;
        let row = read.await??;
        if decision || tool != "needs_you" {
            let row = row.ok_or("missing row")?;
            assert_eq!(row["steps"]["total"], 0, "{tool}");
            if tool == "task_get" {
                assert_eq!(row["steps_list"], json!([]));
            }
            assert_eq!(row["shows"], if decision { "needs_you" } else { "done" });
            if decision {
                assert_eq!(row["needs_you"]["action"], "Approve the repair");
            }
        } else {
            assert!(row.is_none());
        }
        let caller = crate::task_message::superagent(&f.sandbox, &owner.user.id, "admin");
        let next =
            f.h.app
                .tool_tasks_list(&caller, &json!({"scope":"needs_you"}))
                .await?;
        let row = next["tasks"]
            .as_array()
            .ok_or("tasks")?
            .iter()
            .find(|r| r["task_id"] == f.task.as_str())
            .ok_or("reopened task")?;
        assert_eq!(row["steps"]["total"], 1);
        assert_eq!(row["shows"], "needs_you");
    }
    Ok(())
}

#[tokio::test]
async fn github_outage_keeps_the_last_verified_parked_pull() -> TestResult {
    let (f, owner) = owner_observed_external_pr().await?;
    install_app(&f).await?;
    f.h.github
        .set_checks("abc123", yulius_app::ports::github::ChecksState::Success);
    let before = f.h.app.task_detail(&owner, f.task.as_str()).await?;
    assert_eq!(before.summary.parked_needs_you, Some(false));
    f.h.github.no_installation.store(true, Ordering::SeqCst);
    f.h.app.deps.store.set_installation(None).await?;
    let during = f.h.app.task_detail(&owner, f.task.as_str()).await?;
    assert_eq!(
        during.summary.parked_needs_you,
        before.summary.parked_needs_you
    );
    assert_eq!(during.summary.parked_pr, before.summary.parked_pr);
    assert!(during.summary.needs_you_notice.is_none());
    Ok(())
}

#[tokio::test]
async fn a_parked_directly_observed_merge_survives_an_unavailable_direct_read() -> TestResult {
    for merged in [true, false] {
        let f = fixture().await?;
        let owner = idle_for_parking(&f).await?;
        install_compatibility_pull(&f).await?;
        let app = &f.h.app;
        app.deps
            .store
            .transition_task(
                &f.task,
                None,
                TaskStatus::Parked,
                Some(Activity::Done),
                app.now(),
            )
            .await?;
        {
            let mut pulls = f.h.github.pulls.lock().map_err(|_| "pulls")?;
            pulls[0].merged = merged;
            pulls[0].state = "closed".into();
        }
        let caller = crate::task_message::superagent(&f.sandbox, &owner.user.id, "admin");
        for unavailable in [false, true] {
            if unavailable {
                f.h.github.pulls.lock().map_err(|_| "pulls")?.clear();
                f.h.github.fail_overview_pulls.store(true, Ordering::SeqCst);
            }
            let row = app
                .tool_task_get(&caller, &json!({"task_id": f.task.as_str()}))
                .await?;
            assert_eq!(
                row["parked_pr"],
                json!({"number":1461,"open":false}),
                "{row}"
            );
            assert_eq!(row["shows"], if merged { "done" } else { "needs_you" });
            if merged {
                assert!(row["needs_you"].is_null());
            }
            if unavailable {
                let overview = app.git_overview(&owner, f.sandbox.as_str()).await?;
                let pull = overview
                    .tasks
                    .iter()
                    .find(|row| row.task_id == f.task.as_str())
                    .and_then(|row| row.pr.as_ref())
                    .ok_or("lost merged PR")?;
                assert_eq!(
                    (pull.number, pull.state.as_str()),
                    (1461, if merged { "merged" } else { "closed" })
                );
                let after = app
                    .tool_task_get(&caller, &json!({"task_id": f.task.as_str()}))
                    .await?;
                assert_eq!(after["parked_pr"], row["parked_pr"]);
                assert_eq!(after["shows"], row["shows"]);
            }
        }
    }
    Ok(())
}

#[tokio::test]
async fn a_rate_limited_github_leaves_merged_tasks_done_and_reports_one_reset() -> TestResult {
    use yulius::wiring::github_client::GitHubAdapter;
    use yulius_github::fake::FAKE_APP_KEY_PEM;
    let (f, owner) = parked_with_recorded_merge().await?;
    let ctx = f.h.app.agent_context(TOKEN).await?;
    f.h.app
        .tool_steps_set(
            &ctx,
            &json!({"steps":[{"title":"Verify merge","status":"done"}]}),
        )
        .await?;
    let reset = yulius_github::Clock::now(&yulius_github::SystemClock).0 / 1000 + 904;
    let server = axum::Router::new().fallback(move || async move {
        (
            axum::http::StatusCode::FORBIDDEN,
            [
                ("x-ratelimit-remaining", "0".to_owned()),
                ("x-ratelimit-reset", reset.to_string()),
            ],
        )
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let base = yulius_github::Url::parse(&format!("http://{}/", listener.local_addr()?))?;
    let serving = tokio::spawn(async move { axum::serve(listener, server).await });
    let app_config = GitHubAppConfig {
        app_id: 1,
        slug: "test".into(),
        client_id: "test".into(),
        client_secret: "test".into(),
        private_key_pem: FAKE_APP_KEY_PEM.into(),
        webhook_secret: None,
    };
    let mut deps = f.h.app.deps.clone();
    deps.store
        .set_github_app(
            &deps.crypto.seal(&serde_json::to_vec(&app_config)?)?,
            "acme",
            "org",
        )
        .await?;
    deps.github = Arc::new(GitHubAdapter::new(yulius_github::GitHubConfig {
        api_base: base,
        ..yulius_github::GitHubConfig::github_com()?
    })?);
    let app = yulius_app::context::App::new(f.h.app.config.clone(), deps);
    let caller = crate::task_message::superagent(&f.sandbox, &owner.user.id, "admin");
    for status in [TaskStatus::Parked, TaskStatus::Running] {
        app.deps
            .store
            .transition_task(&f.task, None, status, Some(Activity::Done), app.now())
            .await?;
        app.git_overview(&owner, f.sandbox.as_str()).await?;
        let row = app
            .tool_task_get(&caller, &json!({"task_id":f.task.as_str()}))
            .await?;
        assert_eq!(row["shows"], "done", "{row}");
        assert!(
            app.task_detail(&owner, f.task.as_str())
                .await?
                .summary
                .needs_you_notice
                .is_none()
        );
    }
    let status = serde_json::to_value(app.status(&owner).await?)?;
    assert_eq!(status["github_reset_at_ms"], reset * 1000);
    let list = app
        .tool_tasks_list(&caller, &json!({"scope":"active"}))
        .await?;
    assert!(
        list["github_wait"]
            .as_str()
            .is_some_and(|s| s.contains("staging")
                && s.contains(&yulius_app::auth::rfc3339(Timestamp(reset * 1000))))
    );
    let queue = app.merge_queue_for_person(&owner, &f.sandbox).await?;
    assert_eq!(queue["github_wait"], list["github_wait"]);
    use yulius_domain::ci::{CiRef, RunState};
    super::delivery::insert_ci_attempt(
        &f.h.store,
        "publish-me",
        "abc123",
        CiRef::PullRequest(1491),
        RunState::Passed,
        app.now().0,
        &[],
    )
    .await?;
    app.ci_retry_statuses().await;
    let run = app.deps.store.ci_run("publish-me").await?.ok_or("run")?;
    assert_eq!(run.status_attempts, 0);
    let detail = app
        .ci_run_detail(&owner, f.sandbox.as_str(), "publish-me")
        .await?;
    assert_eq!(detail.status, list["github_wait"].as_str().ok_or("wait")?);
    serving.abort();
    Ok(())
}

#[tokio::test]
async fn a_finished_live_task_uses_its_recorded_merge_after_an_older_red_read() -> TestResult {
    use yulius_domain::delivery::DeliveryStage;
    let (f, owner) = owner_observed_external_pr().await?;
    let app = &f.h.app;
    install_app(&f).await?;
    let ctx = app.agent_context(TOKEN).await?;
    app.tool_steps_set(
        &ctx,
        &json!({"steps":[{"title":"Verify fix","status":"done"}]}),
    )
    .await?;
    app.deps
        .store
        .transition_task(
            &f.task,
            None,
            TaskStatus::Running,
            Some(Activity::Done),
            app.now(),
        )
        .await?;
    app.deps
        .store
        .transition_delivery(
            &DeliveryStage::None,
            &yulius_app::delivery::DeliveryRecord {
                task_id: f.task.clone(),
                stage: DeliveryStage::MergedIntegration,
                pr_number: Some(1461),
                updated_at: app.now(),
            },
        )
        .await?;
    let caller = crate::task_message::superagent(&f.sandbox, &owner.user.id, "admin");
    let row = app
        .tool_task_get(&caller, &json!({"task_id": f.task.as_str()}))
        .await?;
    assert_eq!(row["shows"], "done", "{row}");
    Ok(())
}

#[tokio::test]
async fn unreadable_checks_keep_a_verified_head_but_do_not_test_a_new_push() -> TestResult {
    use yulius_app::ports::github::ChecksState;
    let (f, owner) = owner_observed_external_pr().await?;
    install_app(&f).await?;
    f.h.github.set_checks("abc123", ChecksState::Success);
    let app = &f.h.app;
    let caller = crate::task_message::superagent(&f.sandbox, &owner.user.id, "admin");
    let args = json!({"task_id":f.task.as_str()});
    assert_eq!(app.tool_task_get(&caller, &args).await?["shows"], "done");
    *f.h.github.checks_error.lock().map_err(|_| "checks")? =
        Some(yulius_app::error::AppError::Upstream("HTTP 503".into()));
    for (head, expected) in [("abc123", "success"), ("new-sha", "unknown")] {
        f.h.github.pulls.lock().map_err(|_| "pulls")?[0].head_sha = head.into();
        let overview = app.git_overview(&owner, f.sandbox.as_str()).await?;
        let pr = overview
            .tasks
            .iter()
            .find(|row| row.task_id == f.task.as_str())
            .and_then(|row| row.pr.as_ref())
            .ok_or("pull")?;
        assert_eq!(pr.checks, expected);
    }
    Ok(())
}

#[tokio::test]
async fn a_parked_new_head_with_unreadable_checks_survives_the_next_outage() -> TestResult {
    use yulius_app::ports::github::ChecksState;
    let (f, owner) = owner_observed_external_pr().await?;
    install_app(&f).await?;
    f.h.github.set_checks("abc123", ChecksState::Success);
    let app = &f.h.app;
    let caller = crate::task_message::superagent(&f.sandbox, &owner.user.id, "admin");
    let args = json!({"task_id":f.task.as_str()});
    assert_eq!(app.tool_task_get(&caller, &args).await?["shows"], "done");
    f.h.github.pulls.lock().map_err(|_| "pulls")?[0].head_sha = "new-sha".into();
    *f.h.github.checks_error.lock().map_err(|_| "checks")? =
        Some(yulius_app::error::AppError::Upstream("HTTP 503".into()));
    assert_eq!(
        app.tool_task_get(&caller, &args).await?["shows"],
        "needs_you"
    );
    f.h.github.no_installation.store(true, Ordering::SeqCst);
    app.deps.store.set_installation(None).await?;
    assert_eq!(
        app.tool_task_get(&caller, &args).await?["shows"],
        "needs_you"
    );
    Ok(())
}

#[tokio::test]
async fn a_real_local_ci_error_supersedes_cached_green_during_a_github_outage() -> TestResult {
    use yulius_domain::ci::{CiRef, RunState};
    let (f, owner) = owner_observed_external_pr().await?;
    install_app(&f).await?;
    f.h.github
        .set_checks("abc123", yulius_app::ports::github::ChecksState::Success);
    let app = &f.h.app;
    let caller = crate::task_message::superagent(&f.sandbox, &owner.user.id, "admin");
    let args = json!({"task_id":f.task.as_str()});
    assert_eq!(app.tool_task_get(&caller, &args).await?["shows"], "done");
    super::delivery::insert_ci_attempt(
        &f.h.store,
        "new-error",
        "abc123",
        CiRef::PullRequest(1461),
        RunState::Error,
        app.now().0,
        &[],
    )
    .await?;
    f.h.github.no_installation.store(true, Ordering::SeqCst);
    app.deps.store.set_installation(None).await?;
    let brief = app.tool_task_get(&caller, &args).await?;
    assert_eq!(
        brief["wait_reason"],
        "CI could not complete the checks for the parked task's pull request."
    );
    app.deps
        .store
        .transition_task(
            &f.task,
            None,
            TaskStatus::Running,
            Some(Activity::Done),
            app.now(),
        )
        .await?;
    let live = app.tool_task_get(&caller, &args).await?;
    f.h.github.fail_overview_pulls.store(true, Ordering::SeqCst);
    let overview = app.git_overview(&owner, f.sandbox.as_str()).await?;
    let pr = overview
        .tasks
        .iter()
        .find(|t| t.task_id == f.task.as_str())
        .and_then(|t| t.pr.as_ref())
        .ok_or("open pull")?;
    assert_eq!(
        (live["shows"].as_str(), pr.checks.as_str()),
        (Some("working"), "error")
    );
    Ok(())
}

#[tokio::test]
async fn a_partial_green_check_report_keeps_the_prior_failure_for_the_same_head() -> TestResult {
    use yulius_app::ports::github::{ChecksReport, ChecksSource, ChecksState};
    let (f, owner) = owner_observed_external_pr().await?;
    f.h.github.set_checks_report(
        "abc123",
        ChecksReport {
            state: ChecksState::Success,
            unreadable: vec![ChecksSource::CommitStatuses],
        },
    );
    let overview = f.h.app.git_overview(&owner, f.sandbox.as_str()).await?;
    let pr = overview
        .tasks
        .iter()
        .find(|t| t.task_id == f.task.as_str())
        .and_then(|t| t.pr.as_ref())
        .ok_or("pull")?;
    assert_eq!(pr.checks, "failure");
    Ok(())
}

#[tokio::test]
async fn a_partial_success_on_a_new_head_cannot_test_that_head_during_an_outage() -> TestResult {
    use yulius_app::ports::github::{ChecksReport, ChecksSource, ChecksState};
    use yulius_domain::ci::{CiRef, RunState};
    let (f, owner) = owner_observed_external_pr().await?;
    install_app(&f).await?;
    let app = &f.h.app;
    let caller = crate::task_message::superagent(&f.sandbox, &owner.user.id, "admin");
    let args = json!({"task_id":f.task.as_str()});
    f.h.github.set_checks("abc123", ChecksState::Success);
    assert_eq!(app.tool_task_get(&caller, &args).await?["shows"], "done");
    for (head, expected) in [("unverified", "unknown"), ("local-error", "error")] {
        f.h.github.pulls.lock().map_err(|_| "pulls")?[0].head_sha = head.into();
        f.h.github.set_checks_report(
            head,
            ChecksReport {
                state: ChecksState::Success,
                unreadable: vec![ChecksSource::CommitStatuses],
            },
        );
        if head == "local-error" {
            super::delivery::insert_ci_attempt(
                &f.h.store,
                "error-at-new-head",
                head,
                CiRef::PullRequest(1461),
                RunState::Error,
                app.now().0,
                &[],
            )
            .await?;
        }
        let overview = app.git_overview(&owner, f.sandbox.as_str()).await?;
        let pull = overview
            .tasks
            .iter()
            .find(|t| t.task_id == f.task.as_str())
            .and_then(|t| t.pr.as_ref())
            .ok_or("pull")?;
        assert_eq!(pull.checks, expected);
        f.h.github.no_installation.store(true, Ordering::SeqCst);
        app.deps.store.set_installation(None).await?;
        assert_eq!(
            app.tool_task_get(&caller, &args).await?["shows"],
            "needs_you"
        );
    }
    Ok(())
}

#[tokio::test]
async fn a_newly_confirmed_conflict_survives_an_unavailable_overview() -> TestResult {
    use yulius_app::ports::github::{ChecksReport, ChecksSource, ChecksState};
    let (f, owner) = owner_observed_external_pr().await?;
    install_app(&f).await?;
    f.h.github.set_checks("abc123", ChecksState::Success);
    let app = &f.h.app;
    let caller = crate::task_message::superagent(&f.sandbox, &owner.user.id, "admin");
    let args = json!({"task_id":f.task.as_str()});
    assert_eq!(app.tool_task_get(&caller, &args).await?["shows"], "done");
    f.h.github.pulls.lock().map_err(|_| "pulls")?[0].mergeable = Some(false);
    f.h.github.set_checks_report(
        "abc123",
        ChecksReport {
            state: ChecksState::Success,
            unreadable: vec![ChecksSource::CommitStatuses],
        },
    );
    for unavailable in [false, true] {
        f.h.github
            .fail_overview_pulls
            .store(unavailable, Ordering::SeqCst);
        let overview = app.git_overview(&owner, f.sandbox.as_str()).await?;
        let pull = overview
            .tasks
            .iter()
            .find(|t| t.task_id == f.task.as_str())
            .and_then(|t| t.pr.as_ref())
            .ok_or("pull")?;
        assert!(pull.conflicts, "{pull:?}");
        assert_eq!(pull.checks, "success");
    }
    f.h.github.no_installation.store(true, Ordering::SeqCst);
    app.deps.store.set_installation(None).await?;
    assert_eq!(
        app.tool_task_get(&caller, &args).await?["shows"],
        "needs_you"
    );
    Ok(())
}
