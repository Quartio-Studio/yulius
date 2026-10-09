//! Account usage in the accounts section (SPEC-001 ROT-009, AC-ROT-7): provider windows from
//! the usage probe, failures recorded without losing the last snapshot, and the tokens Yulius
//! observed per account in the last 24 hours and 7 days (unknown stays null).

mod support;

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use axum::http::{Method, StatusCode};
use support::{HarnessOptions, TestResult, call, harness_with, setup_superadmin_creds};
use yulius_app::model::{AccountRecord, AccountStatus, Secret};
use yulius_app::ports::store::AccountStore;
use yulius_app::ports::usage::{UsageFailure, UsageProbe};
use yulius_domain::harness::Provider;
use yulius_domain::ids::AccountId;
use yulius_domain::time::Timestamp;
use yulius_domain::usage::{AccountUsage, UsageWindow};

/// Usage requests the throttled ("busy") token received.
static BUSY_CALLS: AtomicUsize = AtomicUsize::new(0);

/// Answers with windows, refuses an expired token and throttles a busy one.
struct FakeProbe;

#[async_trait]
impl UsageProbe for FakeProbe {
    async fn fetch(
        &self,
        provider: Provider,
        credential: &Secret,
    ) -> Result<AccountUsage, UsageFailure> {
        assert!(
            !credential.expose().is_empty(),
            "the stored credential is passed"
        );
        if credential.expose() == "expired" {
            return Err(UsageFailure::new(
                "the provider refused this account's token for usage (HTTP 401)",
            ));
        }
        if credential.expose() == "busy" {
            BUSY_CALLS.fetch_add(1, Ordering::SeqCst);
            return Err(UsageFailure {
                message: "the provider is limiting usage requests for this account (HTTP 429); Yulius asks again in about 2 min".into(),
                retry_after: Some(Duration::from_secs(120)),
            });
        }
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
        }
    }
}

/// A usage request during which a running task observes the account's limit (the transcript
/// observer marks it limited), answered with throttling (#72).
struct LimitDuringRequest(std::sync::Mutex<Option<yulius_store::SqliteStore>>);

/// The usage a task's limit hit stores: the session window full.
const LIMITED_USAGE: &str = r#"{"primary":{"used_percent":100.0,"window_minutes":300,"resets_at":null},"limit_reached":true}"#;

#[async_trait]
impl UsageProbe for LimitDuringRequest {
    async fn fetch(
        &self,
        _provider: Provider,
        _credential: &Secret,
    ) -> Result<AccountUsage, UsageFailure> {
        let store = self.0.lock().ok().and_then(|s| s.clone());
        if let (Some(store), Ok(id)) = (store, AccountId::parse("a-hit")) {
            let _ = store
                .set_account_state(
                    &id,
                    AccountStatus::Limited,
                    Some(LIMITED_USAGE),
                    None,
                    Timestamp(2),
                )
                .await;
        }
        Err(UsageFailure {
            message: "the provider is limiting usage requests for this account (HTTP 429)".into(),
            retry_after: Some(Duration::from_secs(120)),
        })
    }
}

async fn account(h: &support::Harness, id: &str, credential: &[u8]) -> TestResult {
    h.store
        .insert_account(&AccountRecord {
            id: AccountId::parse(id)?,
            provider: Provider::Anthropic,
            label: id.into(),
            credential: Some(h.app.deps.crypto.seal(credential)?),
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
    account(&h, "a-claude", b"credential").await?;
    account(&h, "a-expired", b"expired").await?;

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
        "/api/admin/accounts/a-expired/usage",
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
    let expired = list
        .body
        .as_array()
        .and_then(|a| a.iter().find(|x| x["id"] == "a-expired"))
        .ok_or("expired account")?;
    assert!(
        expired["last_error"]
            .as_str()
            .unwrap_or("")
            .contains("HTTP 401")
    );
    assert!(expired["tokens_7d"].is_object(), "token totals in the list");
    Ok(())
}

/// #27: usage refreshes in the background, and a throttled usage request (HTTP 429) backs off for
/// the provider's `Retry-After` and keeps the last known usage instead of losing it.
#[tokio::test]
async fn background_rounds_refresh_usage_and_back_off_on_throttling() -> TestResult {
    let h = harness_with(HarnessOptions {
        usage: Some(Arc::new(FakeProbe)),
        ..HarnessOptions::default()
    })?;
    account(&h, "a-claude", b"credential").await?;
    account(&h, "a-busy", b"busy").await?;
    let known = r#"{"primary":{"used_percent":12.0,"window_minutes":300,"resets_at":null}}"#;
    h.store
        .set_account_state(
            &AccountId::parse("a-busy")?,
            AccountStatus::Ok,
            Some(known),
            None,
            Timestamp(1),
        )
        .await?;
    BUSY_CALLS.store(0, Ordering::SeqCst);

    let mut not_before = HashMap::new();
    h.app.refresh_usage_round(&mut not_before).await;
    let claude = h
        .store
        .account(&AccountId::parse("a-claude")?)
        .await?
        .ok_or("a-claude")?;
    assert!(
        claude.usage_json.as_deref().unwrap_or("").contains("42"),
        "refreshed without anyone pressing Refresh"
    );
    let busy = h
        .store
        .account(&AccountId::parse("a-busy")?)
        .await?
        .ok_or("a-busy")?;
    assert!(
        busy.last_error
            .as_deref()
            .unwrap_or("")
            .contains("HTTP 429")
    );
    assert_eq!(busy.usage_json.as_deref(), Some(known), "last usage kept");
    assert_eq!(
        busy.status,
        AccountStatus::Ok,
        "throttling is not a broken account"
    );
    assert_eq!(BUSY_CALLS.load(Ordering::SeqCst), 1);

    // Within the Retry-After the busy account is not asked again; the other one is.
    h.app.refresh_usage_round(&mut not_before).await;
    assert_eq!(
        BUSY_CALLS.load(Ordering::SeqCst),
        1,
        "backed off for Retry-After"
    );
    let wait = not_before
        .get(&AccountId::parse("a-busy")?)
        .ok_or("no back-off")?
        .millis_since(h.app.now());
    assert!((60_000..=120_000).contains(&wait), "{wait} ms");
    Ok(())
}

/// #72: a failed usage request must not write back the account as it was when the round began:
/// a limit a running task observed meanwhile stays, so routing keeps skipping the account.
#[tokio::test]
async fn a_failed_refresh_keeps_a_limit_observed_meanwhile() -> TestResult {
    let probe = Arc::new(LimitDuringRequest(std::sync::Mutex::new(None)));
    let h = harness_with(HarnessOptions {
        usage: Some(probe.clone()),
        ..HarnessOptions::default()
    })?;
    if let Ok(mut slot) = probe.0.lock() {
        *slot = Some(h.store.clone());
    }
    account(&h, "a-hit", b"credential").await?;
    let mut not_before = HashMap::new();
    h.app.refresh_usage_round(&mut not_before).await;
    let hit = h
        .store
        .account(&AccountId::parse("a-hit")?)
        .await?
        .ok_or("a-hit")?;
    assert_eq!(
        hit.status,
        AccountStatus::Limited,
        "the limit observed during the request is kept"
    );
    assert_eq!(hit.usage_json.as_deref(), Some(LIMITED_USAGE));
    assert!(
        hit.last_error.as_deref().unwrap_or("").contains("HTTP 429"),
        "the failure is still reported"
    );
    Ok(())
}
