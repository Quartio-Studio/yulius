//! Shared fixtures moved from `antigravity`.

use crate::support;
use crate::support::agents::FakeAgentOptions;
use crate::support::live::supervision::{
    build_full, runtime, screen_contains, seed_records, seed_repo, task,
};
use crate::support::{TestResult, eventually};
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use yulius::wiring::clock::SystemClock;
use yulius::wiring::harness::HarnessAdapter;
use yulius::wiring::session::PtySessionHost;
use yulius_app::context::App;
use yulius_app::context::{AppConfig, Deps};
use yulius_app::lifecycle::NewTask;
use yulius_app::model::{AccountRecord, AccountStatus, LaunchKind, TaskMode, TaskOrigin};
use yulius_domain::harness::{Harness, Provider};
use yulius_domain::ids::{AccountId, SandboxId, TaskId, UserId};
use yulius_domain::task::TaskStatus;
use yulius_domain::time::Timestamp;
use yulius_harness::login::{antigravity_api_key_credential, antigravity_sign_in_credential};
use yulius_ptyhost::test_support::locate_fake_agent;
use yulius_store::SqliteStore;
use yulius_store::crypto::MasterKeyCrypto;

pub(crate) const API_KEY: &str = "SYNTHETIC-gemini-api-key-0123456789";

pub(crate) async fn add_google_account(
    app: &App,
    id: &str,
    credential: &[u8],
) -> Result<AccountId, Box<dyn std::error::Error>> {
    let account = AccountId::parse(id)?;
    app.deps
        .store
        .insert_account(&AccountRecord {
            id: account.clone(),
            provider: Provider::Google,
            label: format!("Antigravity {id}"),
            credential: Some(app.deps.crypto.seal(credential)?),
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

pub(crate) fn api_key() -> Result<Vec<u8>, String> {
    antigravity_api_key_credential(API_KEY).ok_or_else(|| "an API key envelope".to_owned())
}
