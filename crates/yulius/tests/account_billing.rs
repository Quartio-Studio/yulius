//! When a subscription charges its card: an administrator gives one charge date and
//! how often it repeats, and every view of the subscription says when it charges next.

use crate::support;

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use async_trait::async_trait;
use axum::http::{Method, StatusCode};
use serde_json::{Value, json};
use support::{
    HarnessOptions, TestResult, admit_and_sign_in, call, configure_github_app, harness_with,
    setup_superadmin_creds,
};
use yulius_app::model::{AccountRecord, AccountStatus, Secret};
use yulius_app::ports::store::{AccountStore, AuditStore as _};
use yulius_app::ports::usage::{UsageFailure, UsageProbe};
use yulius_domain::harness::Provider;
use yulius_domain::ids::AccountId;
use yulius_domain::time::Timestamp;
use yulius_domain::usage::AccountUsage;

/// One day in milliseconds.
const DAY_MS: i64 = 86_400_000;

async fn subscription(h: &support::Harness, id: &str) -> TestResult {
    h.store
        .insert_account(&AccountRecord {
            id: AccountId::parse(id)?,
            provider: Provider::Anthropic,
            label: id.into(),
            credential: Some(h.app.deps.crypto.seal(b"synthetic-credential")?),
            status: AccountStatus::Ok,
            usage_json: None,
            last_error: None,
            email: None,
            email_checked_at: None,
            email_typed: false,
            created_at: Timestamp(1),
            updated_at: Timestamp(1),
        })
        .await?;
    Ok(())
}

async fn account_event(
    rx: &mut tokio::sync::broadcast::Receiver<yulius_app::events::Envelope>,
    id: &str,
    done: impl Fn(&Value) -> bool,
) -> Result<Value, Box<dyn std::error::Error>> {
    let wait = async {
        loop {
            let event = serde_json::to_value(&rx.recv().await?.event)?;
            if event["type"] == "account.updated"
                && event["account"]["id"] == id
                && done(&event["account"])
            {
                return Ok::<Value, Box<dyn std::error::Error>>(event["account"].clone());
            }
        }
    };
    tokio::time::timeout(Duration::from_secs(10), wait).await?
}

/// The subscription `id` as the list shows it.
async fn listed(
    h: &support::Harness,
    creds: &support::Creds,
    id: &str,
) -> Result<Value, Box<dyn std::error::Error>> {
    let list = call(&h.router, Method::GET, "/api/admin/accounts", None, creds).await?;
    assert_eq!(list.status, StatusCode::OK, "{}", list.body);
    let found = list
        .body
        .as_array()
        .and_then(|all| all.iter().find(|a| a["id"] == id))
        .cloned()
        .ok_or("listed")?;
    Ok(found)
}

/// An administrator sets, changes and removes the charge date; the
/// answer, the list and the published update say when the next charge is, counted from today.
#[tokio::test]
async fn an_administrator_sets_and_removes_when_a_subscription_charges() -> TestResult {
    let h = harness_with(HarnessOptions::default())?;
    let admin = setup_superadmin_creds(&h).await?;
    configure_github_app(&h).await?;
    let operator = admit_and_sign_in(&h, &admin, "operator", 4243, true).await?;
    subscription(&h, "a-max").await?;
    subscription(&h, "a-pro").await?;
    let path = "/api/admin/accounts/a-max/billing";
    // The harness's clock reads 2026-10-07 (UTC).
    assert_eq!(listed(&h, &admin, "a-max").await?["billing"], Value::Null);

    let mut events = h.app.events.subscribe();
    let set = call(
        &h.router,
        Method::PUT,
        path,
        Some(json!({"anchor": "2026-11-24", "period": "month"})),
        &operator,
    )
    .await?;
    assert_eq!(set.status, StatusCode::OK, "{}", set.body);
    let expected =
        json!({"anchor": "2026-11-24", "period": "month", "next_charge_on": "2026-10-24"});
    assert_eq!(set.body["billing"], expected, "{}", set.body);
    assert_eq!(set.body["label"], "a-max");
    let event = account_event(&mut events, "a-max", |a| !a["billing"].is_null()).await?;
    assert_eq!(event["billing"], expected);
    assert_eq!(listed(&h, &admin, "a-max").await?["billing"], expected);
    assert_eq!(listed(&h, &admin, "a-pro").await?["billing"], Value::Null);
    assert_eq!(
        h.store
            .audit_details("a-max", "account.billing_set", 5)
            .await?,
        vec!["anchor=2026-11-24 period=month".to_owned()]
    );

    // A day after the charge, the next one is a month later.
    h.clock.0.fetch_add(18 * DAY_MS, Ordering::SeqCst);
    assert_eq!(
        listed(&h, &admin, "a-max").await?["billing"]["next_charge_on"],
        "2026-11-24"
    );

    // Another rename or answer keeps the cycle in the view it answers with.
    let renamed = call(
        &h.router,
        Method::PATCH,
        "/api/admin/accounts/a-max",
        Some(json!({"label": "Claude Max"})),
        &admin,
    )
    .await?;
    assert_eq!(renamed.status, StatusCode::OK, "{}", renamed.body);
    assert_eq!(renamed.body["billing"]["anchor"], "2026-11-24");

    // Yearly.
    let yearly = call(
        &h.router,
        Method::PUT,
        path,
        Some(json!({"anchor": "2024-02-29", "period": "year"})),
        &admin,
    )
    .await?;
    assert_eq!(yearly.status, StatusCode::OK, "{}", yearly.body);
    assert_eq!(
        yearly.body["billing"],
        json!({"anchor": "2024-02-29", "period": "year", "next_charge_on": "2027-02-28"})
    );

    // Removed, and removing none is no error.
    for _ in 0..2 {
        let removed = call(&h.router, Method::DELETE, path, None, &admin).await?;
        assert_eq!(removed.status, StatusCode::OK, "{}", removed.body);
        assert_eq!(removed.body["billing"], Value::Null);
    }
    assert_eq!(listed(&h, &admin, "a-max").await?["billing"], Value::Null);
    assert_eq!(
        h.store
            .audit_details("a-max", "account.billing_removed", 5)
            .await?,
        vec![
            "none was set".to_owned(),
            "was anchor=2024-02-29 period=year".to_owned()
        ],
        "newest first"
    );
    Ok(())
}

/// A date that is not `YYYY-MM-DD`, a year out of range, another period or
/// field are refused with nothing written; members are refused; an unknown subscription is 404.
#[tokio::test]
async fn a_charge_date_is_refused_unless_an_administrator_gives_a_real_one() -> TestResult {
    let h = harness_with(HarnessOptions::default())?;
    let admin = setup_superadmin_creds(&h).await?;
    configure_github_app(&h).await?;
    let member = admit_and_sign_in(&h, &admin, "member", 4242, false).await?;
    subscription(&h, "a-max").await?;
    let path = "/api/admin/accounts/a-max/billing";

    for bad in [
        json!({}),
        json!({"anchor": "2026-02-30", "period": "month"}),
        json!({"anchor": "24.11.2026", "period": "month"}),
        json!({"anchor": "1999-12-31", "period": "month"}),
        json!({"anchor": "2026-11-24", "period": "week"}),
        json!({"anchor": "2026-11-24"}),
        json!({"period": "month"}),
        json!({"anchor": 20_261_124, "period": "month"}),
        json!({"anchor": "2026-11-24", "period": "month", "amount": 200}),
    ] {
        let r = call(&h.router, Method::PUT, path, Some(bad.clone()), &admin).await?;
        assert_eq!(
            r.status,
            StatusCode::UNPROCESSABLE_ENTITY,
            "{bad}: {}",
            r.body
        );
        assert_eq!(r.body["error"]["code"], "invalid", "{bad}: {}", r.body);
    }
    let ok = json!({"anchor": "2026-11-24", "period": "month"});
    let refused = call(&h.router, Method::PUT, path, Some(ok.clone()), &member).await?;
    assert_eq!(refused.status, StatusCode::FORBIDDEN, "{}", refused.body);
    let refused = call(&h.router, Method::DELETE, path, None, &member).await?;
    assert_eq!(refused.status, StatusCode::FORBIDDEN, "{}", refused.body);
    assert!(
        h.store.account_billing().await?.is_empty(),
        "nothing written"
    );
    assert!(
        h.store
            .audit_details("a-max", "account.billing_set", 5)
            .await?
            .is_empty()
    );

    let unknown = "/api/admin/accounts/a-gone/billing";
    let missing = call(&h.router, Method::PUT, unknown, Some(ok), &admin).await?;
    assert_eq!(missing.status, StatusCode::NOT_FOUND, "{}", missing.body);
    let missing = call(&h.router, Method::DELETE, unknown, None, &admin).await?;
    assert_eq!(missing.status, StatusCode::NOT_FOUND, "{}", missing.body);
    Ok(())
}

/// Claude shares no usage, and the email `pawel@example.com` for every sign-in.
struct EmailOnly;

#[async_trait]
impl UsageProbe for EmailOnly {
    async fn fetch(&self, _: Provider, _: &Secret) -> Result<AccountUsage, UsageFailure> {
        Err(UsageFailure::not_shared())
    }

    async fn account_email(&self, _: Provider, _: &Secret) -> Result<Option<String>, UsageFailure> {
        Ok(Some("pawel@example.com".into()))
    }
}

/// An update or answer about a subscription for another reason still carries
/// its charge date: the email the background round reads (`publish_account_id`), a usage
/// refresh, a pasted token; a removal publishes `null`.
#[tokio::test]
async fn every_update_and_answer_about_a_subscription_carries_its_charge_date() -> TestResult {
    let h = harness_with(HarnessOptions {
        usage: Some(Arc::new(EmailOnly)),
        ..HarnessOptions::default()
    })?;
    let admin = setup_superadmin_creds(&h).await?;
    subscription(&h, "a-max").await?;
    let billing = "/api/admin/accounts/a-max/billing";
    let set = call(
        &h.router,
        Method::PUT,
        billing,
        Some(json!({"anchor": "2026-11-24", "period": "month"})),
        &admin,
    )
    .await?;
    assert_eq!(set.status, StatusCode::OK, "{}", set.body);
    let expected = set.body["billing"].clone();
    assert!(!expected.is_null(), "{}", set.body);

    // The background round reads the email and publishes the subscription again.
    let mut events = h.app.events.subscribe();
    h.app.refresh_usage_round(&mut HashMap::new()).await;
    let read = account_event(&mut events, "a-max", |a| !a["email"].is_null()).await?;
    assert_eq!(read["billing"], expected, "{read}");

    let refreshed = call(
        &h.router,
        Method::POST,
        "/api/admin/accounts/a-max/usage",
        None,
        &admin,
    )
    .await?;
    assert_eq!(refreshed.status, StatusCode::OK, "{}", refreshed.body);
    assert_eq!(refreshed.body["billing"], expected, "{}", refreshed.body);

    let token = concat!(
        "sk-ant-oat01-",
        "SYNTHETICbilling0123456789abcdefghijklmnopqrstu-test"
    );
    let pasted = call(
        &h.router,
        Method::PUT,
        "/api/admin/accounts/a-max/token",
        Some(json!({"token": token})),
        &admin,
    )
    .await?;
    assert_eq!(pasted.status, StatusCode::OK, "{}", pasted.body);
    assert_eq!(pasted.body["billing"], expected, "a new sign-in keeps it");

    let mut events = h.app.events.subscribe();
    let removed = call(&h.router, Method::DELETE, billing, None, &admin).await?;
    assert_eq!(removed.status, StatusCode::OK, "{}", removed.body);
    let event = account_event(&mut events, "a-max", |_| true).await?;
    assert_eq!(event["billing"], Value::Null, "{event}");
    Ok(())
}
