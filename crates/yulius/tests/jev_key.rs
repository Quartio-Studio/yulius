//! The Jev key managed by the superadmin (SPEC-001 ADM-001, ADM-002): set (as setup's optional
//! step does), replaced, removed and tested through the API, sealed at rest, used by task intake
//! at once, `YULIUS_JEV_API_KEY_FILE` honoured while nothing is saved, and never returned.

use crate::support;

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use axum::http::{Method, StatusCode};
use serde_json::{Value, json};
use support::{
    Creds, Harness, TestResult, admit_and_sign_in, call, configure_github_app, harness, perms,
    seed_sandbox, setup_superadmin_creds,
};
use yulius_app::intake::IntakeServices;
use yulius_app::model::Secret;
use yulius_app::ports::intake::{ChoiceEvaluator, JevCheck, JevConnector, JevFailure};
use yulius_app::ports::store::UserStore as _;
use yulius_app::ports::store_ext::UpdateStore as _;
use yulius_domain::intake::{ChoiceAnswer, ChoiceQuestion};

const SAVED: &str = "jev-saved-key-0123456789";
const REPLACED: &str = "jev-replaced-key-0123456789";
const CONFIG: &str = "jev-config-key-0123456789";
const REFUSED: &str = "jev-refused-key-0123456789";

/// An evaluator bound to one key: it says "several" for every split question.
struct KeyedJev {
    key: String,
    log: Arc<Mutex<Vec<String>>>,
}

#[async_trait]
impl ChoiceEvaluator for KeyedJev {
    async fn evaluate(
        &self,
        _state: &str,
        question: &ChoiceQuestion<'_>,
        _deadline: Duration,
    ) -> Result<ChoiceAnswer, JevFailure> {
        if let Ok(mut log) = self.log.lock() {
            log.push(self.key.clone());
        }
        let p = 0.99;
        let rest = (1.0 - p) / (question.options.len() - 1) as f64;
        Ok(ChoiceAnswer {
            choice: "several".into(),
            confidence: p,
            probabilities: question
                .options
                .iter()
                .map(|o| (o.key.to_owned(), if o.key == "several" { p } else { rest }))
                .collect(),
        })
    }
}

/// Builds [`KeyedJev`]s; refuses short keys like the real connector; "tests" by key.
#[derive(Default)]
struct FakeKeys {
    log: Arc<Mutex<Vec<String>>>,
    checks: AtomicUsize,
}

#[async_trait]
impl JevConnector for FakeKeys {
    fn connect(&self, key: &Secret) -> Result<Arc<dyn ChoiceEvaluator>, String> {
        if key.expose().len() < 8 || key.expose().contains(' ') {
            return Err("the Jev key must be 8-4096 printable characters without spaces".into());
        }
        Ok(Arc::new(KeyedJev {
            key: key.expose().to_owned(),
            log: self.log.clone(),
        }))
    }

    async fn check(&self, key: &Secret) -> JevCheck {
        self.checks.fetch_add(1, Ordering::SeqCst);
        if key.expose() == REFUSED {
            JevCheck::Refused
        } else {
            JevCheck::Ok
        }
    }
}

struct Instance {
    h: Harness,
    admin: Creds,
    alice: Creds,
    keys: Arc<FakeKeys>,
}

async fn instance(
    config: Result<Option<Secret>, String>,
) -> Result<Instance, Box<dyn std::error::Error>> {
    let h = harness()?;
    let admin = setup_superadmin_creds(&h).await?;
    configure_github_app(&h).await?;
    let alice = admit_and_sign_in(&h, &admin, "alice", 11, true).await?;
    let alice_id = h.store.user_by_login("alice").await?.ok_or("alice")?.id;
    seed_sandbox(&h, "s1", "fixture", alice_id.as_str()).await?;
    h.github.set_perm("alice", "acme/fixture", perms("write"));
    let keys = Arc::new(FakeKeys::default());
    h.app.set_intake(
        IntakeServices::new(None, None)
            .with_key_management(keys.clone() as Arc<dyn JevConnector>, config),
    );
    h.app.load_jev_key().await;
    Ok(Instance {
        h,
        admin,
        alice,
        keys,
    })
}

async fn api(
    i: &Instance,
    method: Method,
    path: &str,
    body: Option<Value>,
    who: &Creds,
) -> Result<support::Resp, Box<dyn std::error::Error>> {
    let r = call(&i.h.router, method, path, body, who).await?;
    let text = r.body.to_string();
    for key in [SAVED, REPLACED, CONFIG, REFUSED] {
        assert!(!text.contains(key), "a response carried a key: {text}");
    }
    Ok(r)
}

/// A request long enough for the split check (40 words or more).
fn long_prompt() -> String {
    "Please build the reporting dashboard with charts and filters and also rework the billing \
     backend so invoices are generated nightly, then migrate the old customer table to the new \
     schema, update the documentation for both parts, add tests for the dashboard and the billing \
     jobs, and make sure everything deploys cleanly to the staging environment today."
        .into()
}

async fn split_checked(i: &Instance) -> Result<Value, Box<dyn std::error::Error>> {
    let r = api(
        i,
        Method::POST,
        "/api/sandboxes/s1/intake/check",
        Some(json!({"prompt": long_prompt()})),
        &i.alice,
    )
    .await?;
    assert_eq!(r.status, StatusCode::OK, "{}", r.body);
    Ok(r.body)
}

/// AC-ADM-2: the superadmin sets, replaces, tests and removes the key; intake uses the key in
/// use at once; the key is sealed at rest and never in a response.
#[tokio::test]
async fn the_superadmin_manages_the_key_and_intake_follows_at_once() -> TestResult {
    let i = instance(Ok(None)).await?;
    let view = api(&i, Method::GET, "/api/admin/jev", None, &i.admin).await?;
    assert_eq!(view.status, StatusCode::OK);
    assert_eq!(view.body["configured"], false);
    assert!(view.body["source"].is_null());
    assert_eq!(
        split_checked(&i).await?["checked"],
        false,
        "no key: nothing checked"
    );
    let r = api(&i, Method::POST, "/api/admin/jev/test", None, &i.admin).await?;
    assert_eq!(r.body["result"], "none");

    let r = api(
        &i,
        Method::PUT,
        "/api/admin/jev",
        Some(json!({"key": "short"})),
        &i.admin,
    )
    .await?;
    assert_eq!(r.status, StatusCode::UNPROCESSABLE_ENTITY);
    let r = api(
        &i,
        Method::PUT,
        "/api/admin/jev",
        Some(json!({"key": SAVED})),
        &i.admin,
    )
    .await?;
    assert_eq!(r.status, StatusCode::OK, "{}", r.body);
    assert_eq!(r.body["configured"], true);
    assert_eq!(r.body["source"], "settings");
    assert_eq!(r.body["set_by"], "superadmin");
    assert!(r.body["set_at"].is_string());
    let verdict = split_checked(&i).await?;
    assert_eq!(
        verdict,
        json!({"verdict": "several", "checked": true, "supertask": false})
    );
    assert_eq!(
        i.keys
            .log
            .lock()
            .map_err(|_| "lock")?
            .last()
            .map(String::as_str),
        Some(SAVED)
    );

    // Sealed at rest.
    let stored = i.h.store.jev_key().await?.ok_or("stored key")?;
    assert!(
        !String::from_utf8_lossy(&stored.sealed.0).contains(SAVED),
        "the key is not stored in clear"
    );

    // Replaced: the next check uses the new key, without a restart.
    api(
        &i,
        Method::PUT,
        "/api/admin/jev",
        Some(json!({"key": REPLACED})),
        &i.admin,
    )
    .await?;
    split_checked(&i).await?;
    assert_eq!(
        i.keys
            .log
            .lock()
            .map_err(|_| "lock")?
            .last()
            .map(String::as_str),
        Some(REPLACED)
    );
    let r = api(&i, Method::POST, "/api/admin/jev/test", None, &i.admin).await?;
    assert_eq!(r.body["ok"], true, "{}", r.body);
    assert_eq!(r.body["result"], "ok");
    api(
        &i,
        Method::PUT,
        "/api/admin/jev",
        Some(json!({"key": REFUSED})),
        &i.admin,
    )
    .await?;
    let r = api(&i, Method::POST, "/api/admin/jev/test", None, &i.admin).await?;
    assert_eq!(r.body["ok"], false);
    assert_eq!(r.body["result"], "refused");
    assert_eq!(
        i.keys.checks.load(Ordering::SeqCst),
        2,
        "one request per test"
    );

    // A restart reads the saved key back.
    let restarted = yulius_app::context::App::new(i.h.app.config.clone(), i.h.app.deps.clone());
    let keys = Arc::new(FakeKeys::default());
    restarted.set_intake(
        IntakeServices::new(None, None)
            .with_key_management(keys.clone() as Arc<dyn JevConnector>, Ok(None)),
    );
    restarted.load_jev_key().await;
    let ctx = yulius_app::auth::AuthContext {
        user: i.h.store.superadmin().await?.ok_or("superadmin")?,
        csrf_token: None,
        session_hash: None,
    };
    assert_eq!(
        restarted.jev_key_view(&ctx).await?.source.as_deref(),
        Some("settings")
    );

    // Removed: intake checks nothing again.
    let r = api(&i, Method::DELETE, "/api/admin/jev", None, &i.admin).await?;
    assert_eq!(r.status, StatusCode::OK, "{}", r.body);
    assert_eq!(r.body["configured"], false);
    assert!(i.h.store.jev_key().await?.is_none());
    assert_eq!(split_checked(&i).await?["checked"], false);
    Ok(())
}

/// AC-ADM-2: only the superadmin may read or change the key.
#[tokio::test]
async fn only_the_superadmin_manages_the_key() -> TestResult {
    let i = instance(Ok(None)).await?;
    for (method, path, body) in [
        (Method::GET, "/api/admin/jev", None),
        (Method::PUT, "/api/admin/jev", Some(json!({"key": SAVED}))),
        (Method::DELETE, "/api/admin/jev", None),
        (Method::POST, "/api/admin/jev/test", None),
        (Method::POST, "/api/admin/jev/adopt", None),
    ] {
        let r = api(&i, method.clone(), path, body, &i.alice).await?;
        assert_eq!(r.status, StatusCode::FORBIDDEN, "{method} {path}");
        let r = api(&i, method.clone(), path, None, &Creds::default()).await?;
        assert_eq!(
            r.status,
            StatusCode::UNAUTHORIZED,
            "{method} {path} signed out"
        );
    }
    assert!(i.h.store.jev_key().await?.is_none());
    Ok(())
}

/// AC-ADM-2: `YULIUS_JEV_API_KEY_FILE` is honoured while no key is saved ("from config"); a saved
/// key wins; removing it falls back to the file; one click moves the file's key into settings.
#[tokio::test]
async fn the_key_file_is_honoured_as_config_and_can_move_into_settings() -> TestResult {
    let i = instance(Ok(Some(Secret(CONFIG.into())))).await?;
    let view = api(&i, Method::GET, "/api/admin/jev", None, &i.admin).await?;
    assert_eq!(view.body["configured"], true);
    assert_eq!(view.body["source"], "config");
    assert_eq!(view.body["config_present"], true);
    split_checked(&i).await?;
    assert_eq!(
        i.keys
            .log
            .lock()
            .map_err(|_| "lock")?
            .last()
            .map(String::as_str),
        Some(CONFIG)
    );

    api(
        &i,
        Method::PUT,
        "/api/admin/jev",
        Some(json!({"key": SAVED})),
        &i.admin,
    )
    .await?;
    split_checked(&i).await?;
    assert_eq!(
        i.keys
            .log
            .lock()
            .map_err(|_| "lock")?
            .last()
            .map(String::as_str),
        Some(SAVED)
    );

    let r = api(&i, Method::DELETE, "/api/admin/jev", None, &i.admin).await?;
    assert_eq!(r.body["source"], "config", "back to the key file");
    split_checked(&i).await?;
    assert_eq!(
        i.keys
            .log
            .lock()
            .map_err(|_| "lock")?
            .last()
            .map(String::as_str),
        Some(CONFIG)
    );

    let r = api(&i, Method::POST, "/api/admin/jev/adopt", None, &i.admin).await?;
    assert_eq!(r.status, StatusCode::OK, "{}", r.body);
    assert_eq!(r.body["source"], "settings");
    let stored = i.h.store.jev_key().await?.ok_or("stored")?;
    let opened = i.h.app.deps.crypto.open(&stored.sealed)?;
    assert_eq!(opened, CONFIG.as_bytes(), "the file's key, sealed");

    // An unusable key file is said, and nothing is configured from it.
    let broken = instance(Err(
        "/var/lib/yulius/jev.key is readable by other users (mode 644); make it 0600".into(),
    ))
    .await?;
    let view = api(&broken, Method::GET, "/api/admin/jev", None, &broken.admin).await?;
    assert_eq!(view.body["configured"], false);
    assert_eq!(view.body["config_present"], false);
    assert!(
        view.body["config_error"]
            .as_str()
            .unwrap_or_default()
            .contains("0600")
    );
    let r = api(
        &broken,
        Method::POST,
        "/api/admin/jev/adopt",
        None,
        &broken.admin,
    )
    .await?;
    assert_eq!(r.status, StatusCode::CONFLICT);
    Ok(())
}
