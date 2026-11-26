//! Account usage in the accounts section: provider windows from
//! the usage probe, failures recorded without losing the last snapshot, and the tokens Yulius
//! observed per account in the last 24 hours and 7 days (unknown stays null). A `setup-token`
//! sign-in the usage endpoint refuses (HTTP 403, F 2026-10-10) is not an error: its sessions
//! report its usage.

use crate::support;

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use axum::http::{Method, StatusCode};
use support::{HarnessOptions, TestResult, call, harness_with, setup_superadmin_creds};
use yulius_app::model::{AccountRecord, AccountStatus, Secret};
use yulius_app::ports::store::AccountStore;
use yulius_app::ports::usage::{UsageFailure, UsageFailureKind, UsageProbe};
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
            return Err(UsageFailure::of(
                UsageFailureKind::Rejected,
                "Claude did not accept this subscription's sign-in. If its tasks fail too, sign in again.",
            ));
        }
        if credential.expose() == "busy" {
            BUSY_CALLS.fetch_add(1, Ordering::SeqCst);
            return Err(UsageFailure::throttled(Some(Duration::from_secs(120))));
        }
        if credential.expose() == "setup-token" {
            return Err(UsageFailure::not_shared());
        }
        if credential.expose() == "full" {
            return Ok(AccountUsage {
                primary: Some(UsageWindow {
                    used_percent: Some(100.0),
                    window_minutes: Some(300),
                    resets_at: Some(Timestamp(4_000_000_000_000)),
                }),
                ..AccountUsage::default()
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
            Provider::OpenAi | Provider::Google => Err(UsageFailure::not_shared()),
        }
    }

    async fn model_answer(
        &self,
        _: Provider,
        _: &Secret,
        _: &str,
    ) -> Result<yulius_domain::usage::ModelAnswer, UsageFailure> {
        still_refused().await
    }
}

/// Asked about a stored limit, Claude still refuses the model and names no reset: the
/// limit stays as it is stored.
async fn still_refused() -> Result<yulius_domain::usage::ModelAnswer, UsageFailure> {
    Ok(yulius_domain::usage::ModelAnswer {
        refused: Some(yulius_domain::usage::ReportedLimit {
            scope: yulius_domain::usage::LimitScope::Model(None),
            ..yulius_domain::usage::ReportedLimit::default()
        }),
        ..yulius_domain::usage::ModelAnswer::default()
    })
}

/// A usage request during which a running task observes the account's limit (the transcript
/// observer marks it limited), answered with throttling.
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
        Err(UsageFailure::throttled(Some(Duration::from_secs(120))))
    }
}

/// Sets `id`'s last error as an earlier failure left it, its status and usage as they are.
async fn earlier_error(h: &support::Harness, id: &AccountId, error: &str, at: i64) -> TestResult {
    let status = h.store.account(id).await?.ok_or("account")?.status;
    h.store
        .set_account_state(id, status, None, Some(error), Timestamp(at))
        .await?;
    Ok(())
}

async fn account(h: &support::Harness, id: &str, credential: &[u8]) -> TestResult {
    h.store
        .insert_account(&AccountRecord {
            id: AccountId::parse(id)?,
            provider: Provider::Anthropic,
            label: id.into(),
            credential: Some(h.app.deps.crypto.seal(credential)?),
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
    assert_eq!(
        expired["last_error"],
        "usage: Claude did not accept this subscription's sign-in. If its tasks fail too, sign in again."
    );
    assert!(expired["tokens_7d"].is_object(), "token totals in the list");
    Ok(())
}

/// A model's limit stored for a subscription survives a
/// usage reading, which cannot see it; the API shows it as the model's weekly window beside the
/// subscription's own usage, and the subscription reads `limited` only while the default model
/// cannot run on it: one that is out of Fable's allowance alone is `ok`. An
/// allowance whose reset passed is dropped, and a window read at 100 % makes the subscription
/// `limited` like a limit Claude reported.
#[tokio::test]
async fn a_reading_keeps_a_models_limit_and_the_status_follows_the_windows() -> TestResult {
    let h = harness_with(HarnessOptions {
        usage: Some(Arc::new(FakeProbe)),
        ..HarnessOptions::default()
    })?;
    let admin = setup_superadmin_creds(&h).await?;
    let fable_full = |resets_at: i64| {
        format!(
            r#"{{"secondary":{{"used_percent":100.0,"window_minutes":10080,"resets_at":null}},"weekly_fable":{{"used_percent":100.0,"window_minutes":10080,"resets_at":{resets_at}}}}}"#
        )
    };
    // Two days ahead: a reset is believed only as far as a window lasts.
    let in_two_days = h.app.now().0 + 2 * 24 * 3_600_000;
    for (id, credential, stored) in [
        ("a-out", "credential", Some(fable_full(in_two_days))),
        ("a-reset", "credential", Some(fable_full(1_000))),
        ("a-full", "full", None),
    ] {
        account(&h, id, credential.as_bytes()).await?;
        if let Some(stored) = stored {
            h.store
                .set_account_state(
                    &AccountId::parse(id)?,
                    AccountStatus::Ok,
                    Some(&stored),
                    None,
                    Timestamp(2),
                )
                .await?;
        }
    }
    let refresh = |id: &'static str| {
        let (router, admin) = (h.router.clone(), admin.clone());
        async move {
            call(
                &router,
                Method::POST,
                &format!("/api/admin/accounts/{id}/usage"),
                None,
                &admin,
            )
            .await
        }
    };
    let out = refresh("a-out").await?;
    assert_eq!(out.status, StatusCode::OK, "{:?}", out.body);
    let usage = &out.body["usage"];
    assert_eq!(usage["weekly_fable"]["used_percent"], 100.0, "{usage}");
    assert!(usage["weekly_fable"]["resets_at"].is_string(), "{usage}");
    assert_eq!(usage["secondary"]["used_percent"], 7.5, "the week is read");
    assert_eq!(usage["primary"]["used_percent"], 42.0);
    // Only Fable is used up there: the default model, Opus 5.5, still runs.
    assert_eq!(out.body["status"], "ok");

    let reset = refresh("a-reset").await?;
    assert!(
        reset.body["usage"]["weekly_fable"].is_null(),
        "{:?}",
        reset.body
    );
    assert_eq!(reset.body["status"], "ok");

    let full = refresh("a-full").await?;
    assert_eq!(full.body["usage"]["primary"]["used_percent"], 100.0);
    assert_eq!(full.body["status"], "limited", "{:?}", full.body);
    Ok(())
}

/// F 2026-10-10 item 1(a): every subscription Yulius signs in with `claude setup-token` has only
/// the `user:inference` scope, so the usage endpoint refuses it (HTTP 403). That is not an
/// error: Refresh answers with the account as it is, an old note about usage goes away, and the
/// background round asks again only a day later.
#[tokio::test]
async fn a_sign_in_that_cannot_read_usage_is_not_an_error() -> TestResult {
    let h = harness_with(HarnessOptions {
        usage: Some(Arc::new(FakeProbe)),
        ..HarnessOptions::default()
    })?;
    let admin = setup_superadmin_creds(&h).await?;
    account(&h, "a-setup", b"setup-token").await?;
    let id = AccountId::parse("a-setup")?;
    let old = "usage: the provider refused this account's token for usage (HTTP 403); it refreshes on the next run or needs a re-login";
    earlier_error(&h, &id, old, 2).await?;

    let refreshed = call(
        &h.router,
        Method::POST,
        "/api/admin/accounts/a-setup/usage",
        None,
        &admin,
    )
    .await?;
    assert_eq!(refreshed.status, StatusCode::OK, "{:?}", refreshed.body);
    // The answer says that Claude gave no usage reading.
    assert_eq!(refreshed.body["usage_read"], false, "{:?}", refreshed.body);
    assert!(
        refreshed.body["last_error"].is_null(),
        "{:?}",
        refreshed.body
    );
    assert_eq!(refreshed.body["status"], "ok");

    // Other errors are not cleared by it.
    earlier_error(&h, &id, "re-login failed: the sign-in was cancelled", 3).await?;
    let mut not_before = HashMap::new();
    h.app.refresh_usage_round(&mut not_before).await;
    let record = h.store.account(&id).await?.ok_or("a-setup")?;
    assert_eq!(
        record.last_error.as_deref(),
        Some("re-login failed: the sign-in was cancelled")
    );
    let wait = not_before
        .get(&id)
        .ok_or("no back-off")?
        .millis_since(h.app.now());
    assert!(
        (23 * 3_600_000..=24 * 3_600_000).contains(&wait),
        "asked again a day later: {wait} ms"
    );
    Ok(())
}

/// Usage refreshes in the background, and a throttled usage request (HTTP 429) backs off for
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
    // Throttling is not an error: no note on the subscription (F 2026-10-11).
    assert_eq!(busy.last_error, None);
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

/// A failed usage request must not write back the account as it was when the round began:
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
    assert_eq!(hit.last_error, None, "throttling leaves no note");
    Ok(())
}

/// A usage reading never makes a subscription usable that is not. One left
/// `error` with a credential (rows written before a failed re-login kept `relogin`), like one
/// that needs a new sign-in, is not asked by the background round, and a reading an
/// administrator asks for stores its figures and leaves its status and its reason.
#[tokio::test]
async fn a_usage_reading_leaves_a_broken_subscription_broken() -> TestResult {
    let h = harness_with(HarnessOptions {
        usage: Some(Arc::new(FakeProbe)),
        ..HarnessOptions::default()
    })?;
    let admin = setup_superadmin_creds(&h).await?;
    for (name, status) in [
        ("a-error", AccountStatus::Error),
        ("a-relogin", AccountStatus::Relogin),
    ] {
        account(&h, name, b"works").await?;
        let id = AccountId::parse(name)?;
        let why = "the sign-in ended without producing a token";
        h.store
            .set_account_state(&id, status, None, Some(why), Timestamp(2))
            .await?;
        h.app.refresh_usage_round(&mut HashMap::new()).await;
        let kept = h.store.account(&id).await?.ok_or(name)?;
        assert_eq!(kept.status, status, "{name}: the round asks no broken one");
        assert_eq!(kept.usage_json, None, "{name}");
        let refreshed = call(
            &h.router,
            Method::POST,
            &format!("/api/admin/accounts/{name}/usage"),
            None,
            &admin,
        )
        .await?;
        assert_eq!(refreshed.status, StatusCode::OK, "{:?}", refreshed.body);
        assert_eq!(refreshed.body["status"], status.as_str(), "{name}");
        let read = h.store.account(&id).await?.ok_or(name)?;
        assert_eq!(read.status, status, "{name}: a reading proves no sign-in");
        assert_eq!(read.last_error.as_deref(), Some(why), "{name}");
        assert!(read.usage_json.is_some(), "{name}: the figures are kept");
    }
    Ok(())
}

/// A failed usage reading is a note only over no error or an earlier note: a re-login error
/// written while it ran stays.
#[tokio::test]
async fn a_failed_refresh_keeps_a_relogin_error() -> TestResult {
    let h = harness_with(HarnessOptions {
        usage: Some(Arc::new(FakeProbe)),
        ..HarnessOptions::default()
    })?;
    account(&h, "a-expired", b"expired").await?;
    let id = AccountId::parse("a-expired")?;
    let relogin = "re-login failed: the sign-in was cancelled";
    earlier_error(&h, &id, relogin, 2).await?;
    h.app.refresh_usage_round(&mut HashMap::new()).await;
    let kept = h.store.account(&id).await?.ok_or("a-expired")?;
    assert_eq!(kept.last_error.as_deref(), Some(relogin));

    // An earlier note is replaced by the new one.
    earlier_error(&h, &id, "usage: an older note", 3).await?;
    h.app.refresh_usage_round(&mut HashMap::new()).await;
    let noted = h.store.account(&id).await?.ok_or("a-expired")?;
    assert!(
        noted
            .last_error
            .as_deref()
            .unwrap_or("")
            .starts_with("usage: Claude did not accept"),
        "{:?}",
        noted.last_error
    );
    Ok(())
}

/// F 2026-10-11: "usage should be updated more often + every time someone loads its screen".
/// Opening the subscriptions list reads usage again in the background, at most every 30 s.
#[tokio::test]
async fn opening_the_subscriptions_list_reads_usage_again() -> TestResult {
    let h = harness_with(HarnessOptions {
        usage: Some(Arc::new(FakeProbe)),
        ..HarnessOptions::default()
    })?;
    let admin = setup_superadmin_creds(&h).await?;
    account(&h, "a-onload", b"credential").await?;
    let id = AccountId::parse("a-onload")?;
    assert!(
        h.store
            .account(&id)
            .await?
            .ok_or("account")?
            .usage_json
            .is_none()
    );

    let listed = call(&h.router, Method::GET, "/api/admin/accounts", None, &admin).await?;
    assert_eq!(listed.status, StatusCode::OK, "{:?}", listed.body);
    let mut read = false;
    for _ in 0..100 {
        let record = h.store.account(&id).await?.ok_or("account")?;
        if record.usage_json.as_deref().unwrap_or("").contains("42") {
            read = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(read, "the list read the subscription's usage by itself");
    Ok(())
}

/// The real provider probe for Codex, counting the questions about a model's limit (SPEC-503
/// Codex has none to ask); a ChatGPT sign-in's reading is `parse_codex`'s of a reached
/// limit, an API key's has no windows.
struct CodexProbe {
    real: yulius::wiring::usage::ProviderUsage,
    asked: AtomicUsize,
    chatgpt_at_limit: bool,
}

#[async_trait]
impl UsageProbe for CodexProbe {
    async fn fetch(
        &self,
        provider: Provider,
        credential: &Secret,
    ) -> Result<AccountUsage, UsageFailure> {
        if !self.chatgpt_at_limit {
            return self.real.fetch(provider, credential).await;
        }
        let now = Timestamp(
            i64::try_from(
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_or(0, |d| d.as_millis()),
            )
            .unwrap_or(0),
        );
        let reset = now.0 / 1000 + 3600;
        yulius::wiring::usage::parse_codex(
            &serde_json::json!({"rate_limit": {"limit_reached": true,
                "primary_window": {"used_percent": 100, "limit_window_seconds": 18000, "reset_at": reset},
                "secondary_window": {"used_percent": 40, "limit_window_seconds": 604800, "reset_at": reset + 86400}}}),
            now,
        )
        .map_err(UsageFailure::new)
    }

    async fn model_answer(
        &self,
        provider: Provider,
        credential: &Secret,
        model: &str,
    ) -> Result<yulius_domain::usage::ModelAnswer, UsageFailure> {
        self.asked.fetch_add(1, Ordering::SeqCst);
        self.real.model_answer(provider, credential, model).await
    }
}

/// Refresh of a Codex subscription whose limit is in effect
/// reads its usage and asks nothing about a model's limit, which Codex does not have: an API-key
/// subscription (no windows to read) and a ChatGPT one whose reading says the limit is reached
/// both answer 200 with no note on the card.
#[tokio::test]
async fn a_codex_refresh_at_a_limit_asks_no_model_question() -> TestResult {
    let api_key = format!(
        r#"{{"auth_mode":"apikey","OPENAI_API_KEY":"{}"}}"#,
        concat!("sk-", "SYNTHETIC0123456789abcdefghij")
    );
    let chatgpt = r#"{"auth_mode":"chatgpt","tokens":{"id_token":"h.e30.s","access_token":"SYNTHETIC-a","refresh_token":"SYNTHETIC-r","account_id":"acc"}}"#;
    for (credential, chatgpt_at_limit) in [(api_key.as_str(), false), (chatgpt, true)] {
        let probe = Arc::new(CodexProbe {
            real: yulius::wiring::usage::ProviderUsage::new()?,
            asked: AtomicUsize::new(0),
            chatgpt_at_limit,
        });
        let h = harness_with(HarnessOptions {
            usage: Some(probe.clone()),
            ..HarnessOptions::default()
        })?;
        let admin = setup_superadmin_creds(&h).await?;
        let now = h.app.now();
        let limited = AccountUsage {
            primary: Some(UsageWindow {
                used_percent: Some(100.0),
                window_minutes: Some(300),
                resets_at: Some(Timestamp(now.0 + 3_600_000)),
            }),
            limit_reached: true,
            limit_resets_at: Some(Timestamp(now.0 + 3_600_000)),
            observed_at: Some(now),
            ..AccountUsage::default()
        };
        h.store
            .insert_account(&AccountRecord {
                id: AccountId::parse("a-codex")?,
                provider: Provider::OpenAi,
                label: "Codex".into(),
                credential: Some(h.app.deps.crypto.seal(credential.as_bytes())?),
                status: AccountStatus::Limited,
                usage_json: Some(yulius_app::usage_json::to_json(&limited)),
                last_error: None,
                created_at: Timestamp(1),
                updated_at: Timestamp(1),
                email: None,
                email_checked_at: None,
                email_typed: false,
            })
            .await?;
        let refreshed = call(
            &h.router,
            Method::POST,
            "/api/admin/accounts/a-codex/usage",
            None,
            &admin,
        )
        .await?;
        assert_eq!(refreshed.status, StatusCode::OK, "{}", refreshed.body);
        let stored = h
            .store
            .account(&AccountId::parse("a-codex")?)
            .await?
            .ok_or("account")?;
        assert_eq!(stored.last_error, None, "no note on the card");
        assert_eq!(probe.asked.load(Ordering::SeqCst), 0, "no model question");
    }
    Ok(())
}
