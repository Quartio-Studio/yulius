//! The Subscriptions page's refresh (SPEC-473): `POST /api/admin/accounts/{id}/usage` reads the
//! usage again and then asks Claude about each limit stored for the subscription, with the
//! model that limit binds. Claude's answer ends the limit or gives it its reset; an answer that
//! does not come leaves the limit and says why.
//!
//! The probe is the real one (`ProviderUsage`), pointed at a local stand-in for Claude's API
//! that answers by path and model and records what it was asked.

use crate::support;

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use axum::http::{Method, StatusCode};
use support::{HarnessOptions, TestResult, call, harness_with, setup_superadmin_creds};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use yulius::wiring::usage::ProviderUsage;
use yulius_app::model::{AccountRecord, AccountStatus, Secret};
use yulius_app::ports::store::AccountStore;
use yulius_app::ports::usage::{UsageFailure, UsageProbe};
use yulius_app::usage_json;
use yulius_domain::harness::Provider;
use yulius_domain::ids::AccountId;
use yulius_domain::time::Timestamp;
use yulius_domain::usage::{AccountUsage, ModelAnswer, UsageWindow, WEEKLY_WINDOW_MINUTES};

/// The cheapest model, which the probe reads the 5-hour and weekly windows with.
const CHEAPEST: &str = "claude-haiku-4-5-20251001";
const DEFAULT: &str = "claude-fable-5-2";
const CREDENTIAL: &str = "sk-ant-oat01-test";
const DAY_MS: i64 = 24 * 3_600_000;

/// What the stand-in was asked, in order: `usage` for the usage endpoint, else the model of a
/// messages request.
type Asked = Arc<Mutex<Vec<String>>>;

/// A whole HTTP response.
fn reply(status: &str, headers: &[(&str, &str)], body: &str) -> String {
    let mut out = format!("HTTP/1.1 {status}\r\n");
    for (name, value) in headers {
        out.push_str(&format!("{name}: {value}\r\n"));
    }
    out.push_str(&format!(
        "content-length: {}\r\nconnection: close\r\n\r\n{body}",
        body.len()
    ));
    out
}

/// A response that ran, with the 5-hour and weekly windows a subscription's responses carry.
fn ran(more: &[(&str, &str)]) -> String {
    let mut headers = vec![
        ("anthropic-ratelimit-unified-status", "allowed"),
        ("anthropic-ratelimit-unified-5h-utilization", "0.25"),
        ("anthropic-ratelimit-unified-5h-reset", "4000000000"),
        ("anthropic-ratelimit-unified-7d-utilization", "0.61"),
        ("anthropic-ratelimit-unified-7d-reset", "4000000000"),
    ];
    headers.extend_from_slice(more);
    reply("200 OK", &headers, "{}")
}

/// One request, read whole: its head and its body.
async fn read_request(sock: &mut tokio::net::TcpStream) -> (String, String) {
    let mut bytes = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        let text = String::from_utf8_lossy(&bytes).into_owned();
        if let Some((head, body)) = text.split_once("\r\n\r\n") {
            let length = head
                .lines()
                .filter_map(|line| line.split_once(':'))
                .find(|(name, _)| name.eq_ignore_ascii_case("content-length"))
                .and_then(|(_, value)| value.trim().parse::<usize>().ok())
                .unwrap_or(0);
            if body.len() >= length {
                return (head.to_owned(), body.to_owned());
            }
        }
        match sock.read(&mut chunk).await {
            Ok(n) if n > 0 => bytes.extend_from_slice(&chunk[..n]),
            _ => return (text, String::new()),
        }
    }
}

/// A stand-in for Claude's API on a local port. The usage endpoint refuses the sign-in, as it
/// does for every `setup-token`; a messages request is answered by `answer(model)`, and only
/// when it is the probe's: the stored credential as a bearer token and one output token.
async fn claude(
    answer: impl Fn(&str) -> String + Send + Sync + 'static,
) -> Result<(String, Asked), Box<dyn std::error::Error>> {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let base = format!("http://{}", listener.local_addr()?);
    let asked = Asked::default();
    let seen = Arc::clone(&asked);
    tokio::spawn(async move {
        while let Ok((mut sock, _)) = listener.accept().await {
            let (head, body) = read_request(&mut sock).await;
            let response = if head.starts_with("GET /api/oauth/usage") {
                seen.lock().expect("asked").push("usage".into());
                reply("403 Forbidden", &[], "")
            } else if head.starts_with("POST /v1/messages") {
                let request: serde_json::Value = serde_json::from_str(&body).unwrap_or_default();
                let model = request["model"].as_str().unwrap_or("").to_owned();
                seen.lock().expect("asked").push(model.clone());
                let authorized = head
                    .to_ascii_lowercase()
                    .contains(&format!("authorization: bearer {}", CREDENTIAL.to_lowercase()));
                if authorized && request["max_tokens"] == 1 {
                    answer(&model)
                } else {
                    reply("400 Bad Request", &[], "")
                }
            } else {
                reply("404 Not Found", &[], "")
            };
            let _ = sock.write_all(response.as_bytes()).await;
        }
    });
    Ok((base, asked))
}

fn asked(asked: &Asked) -> Vec<String> {
    asked.lock().expect("asked").clone()
}

fn full_week(resets_at: Option<i64>) -> UsageWindow {
    UsageWindow {
        used_percent: Some(100.0),
        window_minutes: Some(WEEKLY_WINDOW_MINUTES),
        resets_at: resets_at.map(Timestamp),
    }
}

/// A signed-in subscription with `usage` stored.
async fn account(h: &support::Harness, id: &str, usage: &AccountUsage) -> TestResult {
    h.store
        .insert_account(&AccountRecord {
            id: AccountId::parse(id)?,
            provider: Provider::Anthropic,
            label: id.into(),
            credential: Some(h.app.deps.crypto.seal(CREDENTIAL.as_bytes())?),
            status: AccountStatus::Limited,
            usage_json: Some(usage_json::to_json(usage)),
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

async fn stored(h: &support::Harness, id: &str) -> Result<AccountUsage, Box<dyn std::error::Error>> {
    let record = h
        .store
        .account(&AccountId::parse(id)?)
        .await?
        .ok_or("the account")?;
    Ok(yulius_app::accounts::account_usage(&record))
}

async fn refresh(
    h: &support::Harness,
    admin: &support::Creds,
    id: &str,
) -> Result<support::Resp, Box<dyn std::error::Error>> {
    call(
        &h.router,
        Method::POST,
        &format!("/api/admin/accounts/{id}/usage"),
        None,
        admin,
    )
    .await
}

async fn harness_at(base: String) -> Result<support::Harness, Box<dyn std::error::Error>> {
    harness_with(HarnessOptions {
        usage: Some(Arc::new(ProviderUsage::at(base)?)),
        ..HarnessOptions::default()
    })
}

/// RFR-002, RFR-003: a subscription stored as out of the default model's allowance until its
/// reset, whose owner bought usage credits. The default model answers (on those credits, with
/// its allowance still at 100 %), so the limit ends now: the window is gone, the subscription
/// reads `ok`, and Claude was asked once with the default model.
#[tokio::test]
async fn a_refresh_ends_a_models_limit_that_claude_no_longer_holds() -> TestResult {
    let (base, seen) = claude(|model| {
        if model == DEFAULT {
            ran(&[
                ("anthropic-ratelimit-unified-status", "rejected"),
                ("anthropic-ratelimit-unified-overage-status", "allowed"),
                ("anthropic-ratelimit-unified-7d_oi-utilization", "1.0"),
                ("anthropic-ratelimit-unified-7d_oi-reset", "4000000000"),
            ])
        } else {
            ran(&[])
        }
    })
    .await?;
    let h = harness_at(base).await?;
    let admin = setup_superadmin_creds(&h).await?;
    let in_two_days = h.app.now().0 + 2 * DAY_MS;
    account(
        &h,
        "a-out",
        &AccountUsage {
            weekly_fable: Some(full_week(Some(in_two_days))),
            ..AccountUsage::default()
        },
    )
    .await?;

    let refreshed = refresh(&h, &admin, "a-out").await?;
    assert_eq!(refreshed.status, StatusCode::OK, "{:?}", refreshed.body);
    let usage = &refreshed.body["usage"];
    assert!(usage["weekly_fable"].is_null(), "the limit ends: {usage}");
    assert_eq!(refreshed.body["status"], "ok", "{:?}", refreshed.body);
    assert_eq!(usage["primary"]["used_percent"], 25.0, "the reading: {usage}");
    assert_eq!(usage["secondary"]["used_percent"], 61.0);
    assert!(refreshed.body["last_error"].is_null());
    assert_eq!(asked(&seen), ["usage", CHEAPEST, DEFAULT]);
    Ok(())
}

/// RFR-002, RFR-003: a limit recorded without a reset (a session on Claude Code 2.1.311 wrote
/// none). The reading does not end it on a refresh: Claude is asked, refuses the default model
/// for its allowance and names the reset, which the stored window gets. A later reading keeps
/// it, as it keeps every model's window that has a reset.
#[tokio::test]
async fn a_refresh_gives_a_limit_recorded_without_a_reset_its_reset() -> TestResult {
    let (base, seen) = claude(|model| {
        if model == DEFAULT {
            reply(
                "429 Too Many Requests",
                &[
                    ("anthropic-ratelimit-unified-status", "rejected"),
                    (
                        "anthropic-ratelimit-unified-representative-claim",
                        "seven_day_overage_included",
                    ),
                    ("anthropic-ratelimit-unified-reset", "4000000000"),
                ],
                r#"{"type":"error","error":{"type":"rate_limit_error","message":"out of credits"}}"#,
            )
        } else {
            ran(&[])
        }
    })
    .await?;
    let h = harness_at(base).await?;
    // 4000000000 s is past what a reset is believed for, so the clock is set a day before it.
    h.clock
        .0
        .store(4_000_000_000_000 - DAY_MS, std::sync::atomic::Ordering::SeqCst);
    let admin = setup_superadmin_creds(&h).await?;
    account(
        &h,
        "a-blind",
        &AccountUsage {
            weekly_fable: Some(full_week(None)),
            ..AccountUsage::default()
        },
    )
    .await?;

    let refreshed = refresh(&h, &admin, "a-blind").await?;
    assert_eq!(refreshed.status, StatusCode::OK, "{:?}", refreshed.body);
    assert_eq!(
        stored(&h, "a-blind").await?.weekly_fable,
        Some(full_week(Some(4_000_000_000_000))),
        "{:?}",
        refreshed.body
    );
    assert_eq!(refreshed.body["status"], "limited");
    assert_eq!(
        refreshed.body["usage"]["secondary"]["used_percent"], 61.0,
        "the week keeps its own reading"
    );
    assert_eq!(asked(&seen), ["usage", CHEAPEST, DEFAULT]);

    // The background round's reading keeps the window now that it has a reset.
    h.app
        .refresh_usage_round(&mut std::collections::HashMap::new())
        .await;
    assert_eq!(
        stored(&h, "a-blind").await?.weekly_fable,
        Some(full_week(Some(4_000_000_000_000)))
    );
    Ok(())
}

/// RFR-004: Claude does not answer the question (a server error, a refused sign-in, a
/// throttle). The refresh fails with one plain sentence, kept as the usage note, and what was
/// stored about the limit stays: with its reset, and without one.
#[tokio::test]
async fn a_refresh_that_cannot_ask_leaves_the_limit_and_says_why() -> TestResult {
    let status = Arc::new(Mutex::new("500 Internal Server Error"));
    let answering = Arc::clone(&status);
    let (base, seen) = claude(move |model| {
        if model == DEFAULT {
            reply(*answering.lock().expect("status"), &[("retry-after", "30")], "")
        } else {
            ran(&[])
        }
    })
    .await?;
    let h = harness_at(base).await?;
    let admin = setup_superadmin_creds(&h).await?;
    let in_two_days = h.app.now().0 + 2 * DAY_MS;
    for (id, resets_at) in [("a-dated", Some(in_two_days)), ("a-blind", None)] {
        account(
            &h,
            id,
            &AccountUsage {
                weekly_fable: Some(full_week(resets_at)),
                ..AccountUsage::default()
            },
        )
        .await?;
    }
    let unasked =
        "Claude did not answer when asked whether the limit still holds; the limit stays as it was.";
    let refused_sign_in = "Claude did not accept this subscription's sign-in. If its tasks fail too, sign in again.";
    for (answer, sentence) in [
        ("500 Internal Server Error", unasked),
        ("401 Unauthorized", refused_sign_in),
        ("429 Too Many Requests", unasked),
    ] {
        *status.lock().expect("status") = answer;
        for (id, resets_at) in [("a-dated", Some(in_two_days)), ("a-blind", None)] {
            let refreshed = refresh(&h, &admin, id).await?;
            assert_eq!(
                refreshed.status,
                StatusCode::BAD_GATEWAY,
                "{answer}: {:?}",
                refreshed.body
            );
            let said = refreshed.body.to_string();
            assert!(said.contains(sentence), "{answer}: {said}");
            assert!(!said.contains("HTTP"), "{said}");
            let after = stored(&h, id).await?;
            assert_eq!(
                after.weekly_fable,
                Some(full_week(resets_at)),
                "{answer}, {id}: the limit stays"
            );
            assert_eq!(
                after.secondary.and_then(|w| w.used_percent),
                Some(61.0),
                "the reading itself is stored"
            );
            let record = h
                .store
                .account(&AccountId::parse(id)?)
                .await?
                .ok_or("the account")?;
            assert_eq!(
                record.last_error.as_deref(),
                Some(format!("usage: {sentence}").as_str())
            );
            assert_eq!(record.status, AccountStatus::Limited);
        }
    }
    assert_eq!(
        asked(&seen).iter().filter(|m| *m == DEFAULT).count(),
        6,
        "one question per refresh"
    );
    Ok(())
}

/// RFR-003: nothing is asked when no limit is stored, whatever the windows read; and a limit
/// of the whole subscription that a reading ends is not asked about either.
#[tokio::test]
async fn a_refresh_asks_only_about_a_limit_that_is_stored() -> TestResult {
    let (base, seen) = claude(|_| ran(&[])).await?;
    let h = harness_at(base).await?;
    let admin = setup_superadmin_creds(&h).await?;
    account(&h, "a-fine", &AccountUsage::default()).await?;
    account(
        &h,
        "a-reported",
        &AccountUsage {
            primary: Some(UsageWindow {
                used_percent: Some(100.0),
                window_minutes: Some(300),
                resets_at: None,
            }),
            limit_reached: true,
            // A model's window that reset: no limit any more.
            weekly_fable: Some(full_week(Some(1_000))),
            ..AccountUsage::default()
        },
    )
    .await?;
    for id in ["a-fine", "a-reported"] {
        let refreshed = refresh(&h, &admin, id).await?;
        assert_eq!(refreshed.status, StatusCode::OK, "{:?}", refreshed.body);
        assert_eq!(refreshed.body["status"], "ok", "{:?}", refreshed.body);
        assert_eq!(refreshed.body["usage"]["limit_reached"], false);
    }
    assert_eq!(asked(&seen), ["usage", CHEAPEST, "usage", CHEAPEST]);
    Ok(())
}

/// RFR-003, RFR-004: a sign-in whose usage cannot be read at all (the usage endpoint refuses
/// it and the cheapest model's response carries no headers) is no failure, and a limit of the
/// whole subscription stored for it is asked about with the default model: it runs, so the
/// limit ends, and the windows its answer carried are the reading.
#[tokio::test]
async fn a_limit_of_the_subscription_is_asked_about_with_the_default_model() -> TestResult {
    let (base, seen) = claude(|model| {
        if model == DEFAULT {
            ran(&[])
        } else {
            reply("200 OK", &[], "{}")
        }
    })
    .await?;
    let h = harness_at(base).await?;
    let admin = setup_superadmin_creds(&h).await?;
    account(
        &h,
        "a-unread",
        &AccountUsage {
            limit_reached: true,
            ..AccountUsage::default()
        },
    )
    .await?;
    let refreshed = refresh(&h, &admin, "a-unread").await?;
    assert_eq!(refreshed.status, StatusCode::OK, "{:?}", refreshed.body);
    let usage = &refreshed.body["usage"];
    assert_eq!(usage["limit_reached"], false, "{usage}");
    assert_eq!(usage["primary"]["used_percent"], 25.0, "{usage}");
    assert_eq!(refreshed.body["status"], "ok");
    assert_eq!(asked(&seen), ["usage", CHEAPEST, DEFAULT]);
    Ok(())
}

/// RFR-003: a limit is asked about with the model it binds. An Opus window stored without a
/// reset is asked about with the Opus model Yulius offers, and gets the reset of Claude's
/// refusal; the default model is not asked, and still runs on the subscription.
#[tokio::test]
async fn another_models_window_is_asked_about_with_that_model() -> TestResult {
    let (base, seen) = claude(|model| {
        if model.contains("opus") {
            reply(
                "429 Too Many Requests",
                &[
                    (
                        "anthropic-ratelimit-unified-representative-claim",
                        "seven_day_opus",
                    ),
                    ("anthropic-ratelimit-unified-reset", "4000000000"),
                ],
                "",
            )
        } else {
            ran(&[])
        }
    })
    .await?;
    let h = harness_at(base).await?;
    h.clock
        .0
        .store(4_000_000_000_000 - DAY_MS, std::sync::atomic::Ordering::SeqCst);
    let admin = setup_superadmin_creds(&h).await?;
    account(
        &h,
        "a-opus",
        &AccountUsage {
            weekly_opus: Some(full_week(None)),
            ..AccountUsage::default()
        },
    )
    .await?;
    let refreshed = refresh(&h, &admin, "a-opus").await?;
    assert_eq!(refreshed.status, StatusCode::OK, "{:?}", refreshed.body);
    assert_eq!(
        stored(&h, "a-opus").await?.weekly_opus,
        Some(full_week(Some(4_000_000_000_000)))
    );
    assert_eq!(refreshed.body["status"], "ok", "the default model runs");
    assert_eq!(asked(&seen), ["usage", CHEAPEST, "claude-opus-5-5"]);
    Ok(())
}

/// A probe during whose question a session reports the model's limit anew: the store to write
/// it to and the usage it stores, once the test has them.
#[derive(Default)]
struct LimitWhileAsked(Mutex<Option<(yulius_store::SqliteStore, AccountUsage)>>);

#[async_trait]
impl UsageProbe for LimitWhileAsked {
    async fn fetch(&self, _: Provider, _: &Secret) -> Result<AccountUsage, UsageFailure> {
        Err(UsageFailure::not_shared())
    }

    async fn model_answer(
        &self,
        _: Provider,
        _: &Secret,
        _: &str,
    ) -> Result<ModelAnswer, UsageFailure> {
        let reported = self.0.lock().ok().and_then(|slot| slot.clone());
        if let (Some((store, newer)), Ok(id)) = (reported, AccountId::parse("a-raced")) {
            let _ = store
                .set_account_state(
                    &id,
                    AccountStatus::Limited,
                    Some(&usage_json::to_json(&newer)),
                    None,
                    Timestamp(2),
                )
                .await;
        }
        Ok(ModelAnswer::default())
    }
}

/// RFR-005: a limit Claude reports to a session while the refresh is asking is newer than the
/// answer, and is kept: the answer "the model ran" does not end it.
#[tokio::test]
async fn a_limit_recorded_while_claude_is_asked_is_kept() -> TestResult {
    let probe = Arc::new(LimitWhileAsked::default());
    let h = harness_with(HarnessOptions {
        usage: Some(probe.clone()),
        ..HarnessOptions::default()
    })?;
    let admin = setup_superadmin_creds(&h).await?;
    let now = h.app.now().0;
    let newer = AccountUsage {
        weekly_fable: Some(full_week(Some(now + 3 * DAY_MS))),
        ..AccountUsage::default()
    };
    if let Ok(mut slot) = probe.0.lock() {
        *slot = Some((h.store.clone(), newer));
    }
    account(
        &h,
        "a-raced",
        &AccountUsage {
            weekly_fable: Some(full_week(Some(now + 2 * DAY_MS))),
            ..AccountUsage::default()
        },
    )
    .await?;
    let refreshed = refresh(&h, &admin, "a-raced").await?;
    assert_eq!(refreshed.status, StatusCode::OK, "{:?}", refreshed.body);
    assert_eq!(
        stored(&h, "a-raced").await?.weekly_fable,
        newer.weekly_fable,
        "the limit recorded during the question stays"
    );
    assert_eq!(refreshed.body["status"], "limited");
    Ok(())
}
