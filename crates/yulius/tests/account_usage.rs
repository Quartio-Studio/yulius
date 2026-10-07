//! Account usage in the accounts section (SPEC-001 ROT-009, AC-ROT-7): provider windows from
//! the usage probe, failures recorded without losing the last snapshot, and the tokens Yulius
//! observed per account in the last 24 hours and 7 days (unknown stays null).

mod support;

use std::sync::Arc;

use async_trait::async_trait;
use axum::http::{Method, StatusCode};
use support::{HarnessOptions, TestResult, call, harness_with, setup_superadmin_creds};
use yulius_app::model::{AccountRecord, AccountStatus, Secret};
use yulius_app::ports::store::AccountStore;
use yulius_app::ports::usage::UsageProbe;
use yulius_domain::harness::Provider;
use yulius_domain::ids::AccountId;
use yulius_domain::time::Timestamp;
use yulius_domain::usage::{AccountUsage, UsageWindow};

/// Claude answers with windows; Codex refuses the token.
struct FakeProbe;

#[async_trait]
impl UsageProbe for FakeProbe {
    async fn fetch(&self, provider: Provider, credential: &Secret) -> Result<AccountUsage, String> {
        assert!(
            !credential.expose().is_empty(),
            "the stored credential is passed"
        );
        match provider {
            Provider::Anthropic => Ok(AccountUsage {
                primary: Some(UsageWindow {
                    used_percent: Some(42.0),
                    window_minutes: Some(300),
                    resets_at: Some(Timestamp(4_000_000_000_000)),
                }),
                secondary: Some(UsageWindow {
                    used_percent: Some(7.5),
                    window_minutes: Some(10_080),
                    resets_at: None,
                }),
                ..AccountUsage::default()
            }),
            Provider::OpenAi => {
                Err("the provider refused this account's token for usage (HTTP 401)".into())
            }
        }
    }
}

async fn account(h: &support::Harness, id: &str, provider: Provider) -> TestResult {
    h.store
        .insert_account(&AccountRecord {
            id: AccountId::parse(id)?,
            provider,
            label: id.into(),
            credential: Some(h.app.deps.crypto.seal(b"credential")?),
            priority: 0,
            enabled: true,
            status: AccountStatus::Ok,
            usage_json: None,
            last_error: None,
            created_at: Timestamp(1),
            updated_at: Timestamp(1),
        })
        .await?;
    Ok(())
}

#[tokio::test]
async fn admins_refresh_provider_usage_and_see_observed_tokens() -> TestResult {
    let h = harness_with(HarnessOptions {
        usage: Some(Arc::new(FakeProbe)),
        ..HarnessOptions::default()
    })?;
    let admin = setup_superadmin_creds(&h).await?;
    account(&h, "a-claude", Provider::Anthropic).await?;
    account(&h, "o-codex", Provider::OpenAi).await?;

    let refreshed = call(
        &h.router,
        Method::POST,
        "/api/admin/accounts/a-claude/usage",
        None,
        &admin,
    )
    .await?;
    assert_eq!(refreshed.status, StatusCode::OK, "{:?}", refreshed.body);
    assert_eq!(refreshed.body["usage"]["primary"]["used_percent"], 42.0);
    assert_eq!(
        refreshed.body["usage"]["secondary"]["window_minutes"],
        10_080
    );
    assert!(refreshed.body["usage"]["observed_at"].is_string());
    // No runs yet: zero runs, unknown token counts.
    assert_eq!(refreshed.body["tokens_24h"]["runs"], 0);
    assert!(refreshed.body["tokens_24h"]["input"].is_null());

    let refused = call(
        &h.router,
        Method::POST,
        "/api/admin/accounts/o-codex/usage",
        None,
        &admin,
    )
    .await?;
    assert_eq!(
        refused.status,
        StatusCode::BAD_GATEWAY,
        "{:?}",
        refused.body
    );
    let list = call(&h.router, Method::GET, "/api/admin/accounts", None, &admin).await?;
    let codex = list
        .body
        .as_array()
        .and_then(|a| a.iter().find(|x| x["id"] == "o-codex"))
        .ok_or("codex account")?;
    assert!(
        codex["last_error"]
            .as_str()
            .unwrap_or("")
            .contains("HTTP 401")
    );
    assert!(codex["tokens_7d"].is_object(), "token totals in the list");
    Ok(())
}
