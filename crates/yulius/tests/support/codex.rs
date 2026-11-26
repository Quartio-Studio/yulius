//! Shared fixtures moved from `codex`.

use crate::support::TestResult;
use std::sync::Arc;
use yulius_app::context::App;
use yulius_app::lifecycle::NewTask;
use yulius_app::model::{AccountRecord, AccountStatus, TaskOrigin};
use yulius_domain::harness::{Harness, Provider};
use yulius_domain::ids::{AccountId, SandboxId, TaskId, UserId};
use yulius_domain::time::Timestamp;

/// A synthetic Codex `auth.json` refreshed at `last_refresh`.
pub(crate) fn codex_auth(last_refresh: &str) -> String {
    format!(
        r#"{{"auth_mode":"chatgpt","OPENAI_API_KEY":null,"tokens":{{"id_token":"h.e30.s","access_token":"SYNTHETIC-access-{last_refresh}","refresh_token":"SYNTHETIC-refresh","account_id":"acc-synthetic"}},"last_refresh":"{last_refresh}"}}"#
    )
}

/// Connects a synthetic Codex sign-in for tests that create Codex tasks through other interfaces.
pub(crate) async fn connect_codex(app: &App) -> TestResult {
    add_codex_account(app, "a-codex", &codex_auth("2026-11-01T10:00:00Z")).await?;
    Ok(())
}

pub(crate) async fn add_codex_account(
    app: &App,
    id: &str,
    auth: &str,
) -> Result<AccountId, Box<dyn std::error::Error>> {
    let account = AccountId::parse(id)?;
    app.deps
        .store
        .insert_account(&AccountRecord {
            id: account.clone(),
            provider: Provider::OpenAi,
            label: format!("Codex {id}"),
            credential: Some(app.deps.crypto.seal(auth.as_bytes())?),
            status: AccountStatus::Ok,
            usage_json: None,
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

pub(crate) async fn new_codex_task(
    app: &Arc<App>,
    sandbox: &SandboxId,
    user: &UserId,
    title: &str,
) -> Result<TaskId, Box<dyn std::error::Error>> {
    new_codex_task_in(
        app,
        sandbox,
        user,
        title,
        yulius_app::model::TaskMode::Prompt,
    )
    .await
}

pub(crate) async fn new_codex_task_in(
    app: &Arc<App>,
    sandbox: &SandboxId,
    user: &UserId,
    title: &str,
    mode: yulius_app::model::TaskMode,
) -> Result<TaskId, Box<dyn std::error::Error>> {
    let record = app
        .create_task_internal(NewTask {
            sandbox: sandbox.clone(),
            created_by: user.clone(),
            prompt: format!("{title}: answer with an echo"),
            harness: Harness::Codex,
            title: Some(title.into()),
            workflow_id: None,
            steps: Vec::new(),
            model: None,
            effort: None,
            origin: TaskOrigin::Ui,
            automation_run: None,
            start: true,
            mode,
            supertask: false,
        })
        .await?;
    Ok(record.id)
}
