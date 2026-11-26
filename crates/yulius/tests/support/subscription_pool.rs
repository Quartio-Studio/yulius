//! Shared fixtures moved from `subscription_pool`.

use crate::support;
use async_trait::async_trait;
use axum::extract::connect_info::MockConnectInfo;
use axum::http::Method;
use serde_json::json;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use support::{
    Creds, Harness, HarnessOptions, TestResult, admit_and_sign_in, call, configure_github_app,
    harness, harness_with, seed_sandbox, setup_superadmin_creds,
};
use yulius_app::context::App;
use yulius_app::model::Secret;
use yulius_app::model::{
    AccountRecord, AccountStatus, AttemptRecord, LaunchKind, MessageRecord, MetricsRecord,
    PauseKind, PauseRecord, TaskMode, TaskOrigin, TaskRecord, UsageSampleRecord,
};
use yulius_app::ports::store::{AccountStore, CoordinationStore, TaskStore, UserStore};
use yulius_app::ports::store_ext::CapacityStore;
use yulius_app::ports::usage::{UsageFailure, UsageProbe};
use yulius_domain::harness::{Harness as Agent, Provider};
use yulius_domain::ids::{AccountId, AttemptId, TaskId};
use yulius_domain::task::{AttemptState, TaskStatus};
use yulius_domain::time::Timestamp;
use yulius_domain::usage::{AccountUsage, UsageWindow};

pub(crate) const TOKEN: &str = "pool-task-token-0123456789abcdef";

pub(crate) fn now(h: &Harness) -> Timestamp {
    Timestamp(h.clock.0.load(Ordering::SeqCst))
}

pub(crate) async fn add_account(
    h: &Harness,
    id: &str,
    usage_json: Option<String>,
) -> Result<AccountId, Box<dyn std::error::Error>> {
    let account = AccountId::parse(id)?;
    h.store
        .insert_account(&AccountRecord {
            id: account.clone(),
            provider: Provider::Anthropic,
            label: format!("Claude {id}"),
            credential: Some(h.app.deps.crypto.seal(id.as_bytes())?),
            status: AccountStatus::Ok,
            usage_json,
            last_error: None,
            created_at: Timestamp(1),
            updated_at: Timestamp(1),
            email: None,
            email_checked_at: None,
            email_typed: false,
        })
        .await?;
    Ok(account)
}

/// A running task in `mode` whose live attempt (task token [`TOKEN`]) runs on `account`.
pub(crate) async fn running_task(
    h: &Harness,
    mode: TaskMode,
    account: &AccountId,
) -> Result<TaskId, Box<dyn std::error::Error>> {
    running_task_in(h, mode, account, "/nonexistent/run").await
}

/// [`running_task`] whose run directory is `run_dir`.
pub(crate) async fn running_task_in(
    h: &Harness,
    mode: TaskMode,
    account: &AccountId,
    run_dir: &str,
) -> Result<TaskId, Box<dyn std::error::Error>> {
    running_task_of(h, mode, account, run_dir, None).await
}

/// [`running_task_in`] whose run was launched with `model` (`None`: a run recorded before the
/// launch model was, read as the task's own).
async fn running_task_of(
    h: &Harness,
    mode: TaskMode,
    account: &AccountId,
    run_dir: &str,
    model: Option<&str>,
) -> Result<TaskId, Box<dyn std::error::Error>> {
    running_task_set_to(h, mode, account, run_dir, None, model).await
}

/// [`running_task_of`] for a task set to `own` (`None`: the default model, Opus 5.5).
pub(crate) async fn running_task_set_to(
    h: &Harness,
    mode: TaskMode,
    account: &AccountId,
    run_dir: &str,
    own: Option<&str>,
    model: Option<&str>,
) -> Result<TaskId, Box<dyn std::error::Error>> {
    running_task_of_agent(h, Agent::ClaudeCode, mode, account, run_dir, own, model).await
}

/// [`running_task_set_to`] for a task and run of `agent` (SPEC-503).
pub(crate) async fn running_task_of_agent(
    h: &Harness,
    agent: Agent,
    mode: TaskMode,
    account: &AccountId,
    run_dir: &str,
    own: Option<&str>,
    model: Option<&str>,
) -> Result<TaskId, Box<dyn std::error::Error>> {
    let created = now(h);
    let task = h
        .store
        .insert_task(&TaskRecord {
            resume_attempt_id: None,
            id: TaskId::parse("t-pool")?,
            sandbox_id: yulius_domain::ids::SandboxId::parse("s1")?,
            number: 0,
            title: "Pool".into(),
            prompt: "Build the thing".into(),
            harness: agent,
            model: own.map(str::to_owned),
            effort: None,
            workflow_id: None,
            status: TaskStatus::Running,
            activity: None,
            branch: Some("feat/1-pool".into()),
            worktree_path: None,
            spec_path: None,
            origin: TaskOrigin::Ui,
            automation_run: None,
            created_by: yulius_domain::ids::UserId::parse("u1")?,
            owner: yulius_domain::ids::UserId::parse("u1")?,
            created_at: created,
            updated_at: created,
            closed_at: None,
            archived_at: None,
            queued_at: None,
            gate_blocks: 0,
            wake_enabled: true,
            mode,
            supertask: false,
        })
        .await?;
    h.store
        .insert_attempt(&AttemptRecord {
            id: AttemptId::parse("at-pool")?,
            task_id: task.id.clone(),
            seq: 1,
            harness: agent,
            account_id: Some(account.clone()),
            provider_session_id: Some("session-1".into()),
            launch_kind: LaunchKind::New,
            model: model.map(str::to_owned),
            state: AttemptState::Live,
            run_dir: run_dir.into(),
            token_hash: h.app.deps.crypto.sha256_hex(TOKEN.as_bytes()),
            host_token: h.app.deps.crypto.seal(&[1u8; 32])?,
            ptyhost_pid: None,
            child_pid: None,
            started_at: created,
            ended_at: None,
            exit_code: None,
            exit_reason: None,
        })
        .await?;
    Ok(task.id)
}

pub(crate) async fn seed(h: &Harness) -> TestResult {
    h.store
        .insert_user(&yulius_app::model::UserRecord {
            id: yulius_domain::ids::UserId::parse("u1")?,
            github_login: Some("alice".into()),
            github_id: Some(1),
            display_name: "Alice".into(),
            avatar_url: None,
            is_superadmin: false,
            is_instance_admin: false,
            password_hash: None,
            created_at: Timestamp(1),
            disabled_at: None,
        })
        .await?;
    seed_sandbox(h, "s1", "fixture", "u1").await?;
    Ok(())
}

/// POSTs a hook as the agent's hook client would; returns (status, Retry-After, body).
pub(crate) async fn hook(
    internal: &axum::Router,
    event: &str,
    body: serde_json::Value,
) -> Result<(u16, Option<String>, serde_json::Value), Box<dyn std::error::Error>> {
    hook_of(internal, "claude", event, body).await
}

/// [`hook`] from a run of `harness` (`claude` or `codex`).
pub(crate) async fn hook_of(
    internal: &axum::Router,
    harness: &str,
    event: &str,
    body: serde_json::Value,
) -> Result<(u16, Option<String>, serde_json::Value), Box<dyn std::error::Error>> {
    hook_sent_at(internal, harness, event, body, None).await
}

/// A hook delivered with its original client send time, including a delayed delivery.
pub(crate) async fn hook_sent_at(
    internal: &axum::Router,
    harness: &str,
    event: &str,
    body: serde_json::Value,
    sent: Option<i64>,
) -> Result<(u16, Option<String>, serde_json::Value), Box<dyn std::error::Error>> {
    use http_body_util::BodyExt;
    use tower::ServiceExt;
    let mut request = axum::http::Request::builder()
        .method(Method::POST)
        .uri(format!("/hooks/{harness}/{event}"))
        .header(axum::http::header::AUTHORIZATION, format!("Bearer {TOKEN}"))
        .header(axum::http::header::CONTENT_TYPE, "application/json");
    if let Some(sent) = sent {
        request = request.header(yulius_contracts::hooks::SENT_AT_HEADER, sent);
    }
    let request = request.body(axum::body::Body::from(body.to_string()))?;
    let response = internal.clone().oneshot(request).await?;
    let status = response.status().as_u16();
    let retry_after = response
        .headers()
        .get(axum::http::header::RETRY_AFTER)
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    let bytes = response.into_body().collect().await?.to_bytes();
    let body = serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
    Ok((status, retry_after, body))
}

pub(crate) async fn status(
    h: &Harness,
    task: &TaskId,
) -> Result<TaskStatus, Box<dyn std::error::Error>> {
    Ok(h.store.task(task).await?.ok_or("task")?.status)
}
