//! The instance Vidoc key managed by the superadmin (SPEC-195 VDK-001, VDK-002): set, replaced,
//! removed, adopted from `YULIUS_VIDOC_API_KEY_FILE` and tested through the API, sealed at rest,
//! in use at once, optional, and never returned.

use crate::support;

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use async_trait::async_trait;
use axum::http::{Method, StatusCode};
use serde_json::{Value, json};
use support::{
    Creds, Harness, TestResult, admit_and_sign_in, call, configure_github_app, harness,
    setup_superadmin_creds,
};
use yulius_app::model::Secret;
use yulius_app::ports::store_ext::UpdateStore as _;
use yulius_app::ports::vidoc::{VidocCheck, VidocConnector};
use yulius_app::vidoc_key::VidocServices;

const SAVED: &str = "vidoc-saved-key-0123456789";
const REPLACED: &str = "vidoc-replaced-key-0123456789";
const CONFIG: &str = "vidoc-config-key-0123456789";
const REFUSED: &str = "vidoc-refused-key-0123456789";

/// "Tests" a key by its value, counting requests.
#[derive(Default)]
struct FakeVidoc {
    checks: AtomicUsize,
    last: std::sync::Mutex<Option<String>>,
}

#[async_trait]
impl VidocConnector for FakeVidoc {
    async fn check(&self, key: &Secret) -> VidocCheck {
        self.checks.fetch_add(1, Ordering::SeqCst);
        if let Ok(mut last) = self.last.lock() {
            *last = Some(key.expose().to_owned());
        }
        if key.expose() == REFUSED {
            VidocCheck::Refused
        } else {
            VidocCheck::Ok
        }
    }
}

struct Instance {
    h: Harness,
    admin: Creds,
    alice: Creds,
    vidoc: Arc<FakeVidoc>,
}

async fn instance(
    config: Result<Option<Secret>, String>,
) -> Result<Instance, Box<dyn std::error::Error>> {
    let h = harness()?;
    let admin = setup_superadmin_creds(&h).await?;
    configure_github_app(&h).await?;
    let alice = admit_and_sign_in(&h, &admin, "alice", 11, true).await?;
    let vidoc = Arc::new(FakeVidoc::default());
    h.app.set_vidoc(VidocServices::new(
        vidoc.clone() as Arc<dyn VidocConnector>,
        config,
    ));
    h.app.load_vidoc_key().await;
    Ok(Instance {
        h,
        admin,
        alice,
        vidoc,
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

fn in_use(i: &Instance) -> Option<String> {
    i.h.app.vidoc_key().map(|k| k.expose().to_owned())
}

/// VDK-001, VDK-002: optional; set, replaced, tested and removed, in use at once, sealed at rest,
/// read back after a restart, and never in a response.
#[tokio::test]
async fn the_superadmin_manages_the_optional_key() -> TestResult {
    let i = instance(Ok(None)).await?;
    let view = api(&i, Method::GET, "/api/admin/vidoc", None, &i.admin).await?;
    assert_eq!(view.status, StatusCode::OK, "{}", view.body);
    assert_eq!(view.body["configured"], false);
    assert!(view.body["source"].is_null());
    assert_eq!(in_use(&i), None, "no key: the scan is off");
    let r = api(&i, Method::POST, "/api/admin/vidoc/test", None, &i.admin).await?;
    assert_eq!(r.body["result"], "none");
    assert_eq!(i.vidoc.checks.load(Ordering::SeqCst), 0);

    for bad in ["short", "has a space in it", ""] {
        let r = api(
            &i,
            Method::PUT,
            "/api/admin/vidoc",
            Some(json!({ "key": bad })),
            &i.admin,
        )
        .await?;
        assert_eq!(r.status, StatusCode::UNPROCESSABLE_ENTITY, "{bad:?}");
    }
    let r = api(
        &i,
        Method::PUT,
        "/api/admin/vidoc",
        Some(json!({ "key": format!("  {SAVED}\n") })),
        &i.admin,
    )
    .await?;
    assert_eq!(r.status, StatusCode::OK, "{}", r.body);
    assert_eq!(r.body["configured"], true);
    assert_eq!(r.body["source"], "settings");
    assert_eq!(r.body["set_by"], "superadmin");
    assert!(r.body["set_at"].is_string());
    assert_eq!(
        in_use(&i).as_deref(),
        Some(SAVED),
        "trimmed, in use at once"
    );
    let stored = i.h.store.vidoc_key().await?.ok_or("stored key")?;
    assert!(
        !String::from_utf8_lossy(&stored.sealed.0).contains(SAVED),
        "the key is not stored in clear"
    );

    api(
        &i,
        Method::PUT,
        "/api/admin/vidoc",
        Some(json!({ "key": REPLACED })),
        &i.admin,
    )
    .await?;
    assert_eq!(in_use(&i).as_deref(), Some(REPLACED));
    let r = api(&i, Method::POST, "/api/admin/vidoc/test", None, &i.admin).await?;
    assert_eq!(r.body["ok"], true, "{}", r.body);
    assert_eq!(r.body["result"], "ok");
    assert_eq!(
        i.vidoc.last.lock().map_err(|_| "lock")?.as_deref(),
        Some(REPLACED),
        "the key in use is tested"
    );
    api(
        &i,
        Method::PUT,
        "/api/admin/vidoc",
        Some(json!({ "key": REFUSED })),
        &i.admin,
    )
    .await?;
    let r = api(&i, Method::POST, "/api/admin/vidoc/test", None, &i.admin).await?;
    assert_eq!(r.body["ok"], false);
    assert_eq!(r.body["result"], "refused");
    assert_eq!(
        i.vidoc.checks.load(Ordering::SeqCst),
        2,
        "one request per test"
    );

    let restarted = yulius_app::context::App::new(i.h.app.config.clone(), i.h.app.deps.clone());
    restarted.set_vidoc(VidocServices::new(
        Arc::new(FakeVidoc::default()) as Arc<dyn VidocConnector>,
        Ok(None),
    ));
    restarted.load_vidoc_key().await;
    assert_eq!(
        restarted
            .vidoc_key()
            .map(|k| k.expose().to_owned())
            .as_deref(),
        Some(REFUSED),
        "a restart reads the saved key back"
    );

    let r = api(&i, Method::DELETE, "/api/admin/vidoc", None, &i.admin).await?;
    assert_eq!(r.status, StatusCode::OK, "{}", r.body);
    assert_eq!(r.body["configured"], false);
    assert!(i.h.store.vidoc_key().await?.is_none());
    assert_eq!(in_use(&i), None);
    Ok(())
}

/// VDK-002: only the superadmin reads or changes the key.
#[tokio::test]
async fn only_the_superadmin_manages_the_vidoc_key() -> TestResult {
    let i = instance(Ok(None)).await?;
    for (method, path, body) in [
        (Method::GET, "/api/admin/vidoc", None),
        (
            Method::PUT,
            "/api/admin/vidoc",
            Some(json!({ "key": SAVED })),
        ),
        (Method::DELETE, "/api/admin/vidoc", None),
        (Method::POST, "/api/admin/vidoc/test", None),
        (Method::POST, "/api/admin/vidoc/adopt", None),
    ] {
        let r = api(&i, method.clone(), path, body, &i.alice).await?;
        assert_eq!(r.status, StatusCode::FORBIDDEN, "{method} {path}");
        let r = api(&i, method.clone(), path, None, &Creds::default()).await?;
        assert_eq!(r.status, StatusCode::UNAUTHORIZED, "{method} {path}");
    }
    assert!(i.h.store.vidoc_key().await?.is_none());
    assert_eq!(in_use(&i), None);
    Ok(())
}

/// VDK-001: `YULIUS_VIDOC_API_KEY_FILE` is used while nothing is saved, a saved key wins,
/// removing it falls back to the file, and adopt moves the file's key into settings.
#[tokio::test]
async fn the_vidoc_key_file_is_honoured_as_config_and_can_move_into_settings() -> TestResult {
    let i = instance(Ok(Some(Secret(CONFIG.into())))).await?;
    let view = api(&i, Method::GET, "/api/admin/vidoc", None, &i.admin).await?;
    assert_eq!(view.body["configured"], true);
    assert_eq!(view.body["source"], "config");
    assert_eq!(view.body["config_present"], true);
    assert_eq!(in_use(&i).as_deref(), Some(CONFIG));

    api(
        &i,
        Method::PUT,
        "/api/admin/vidoc",
        Some(json!({ "key": SAVED })),
        &i.admin,
    )
    .await?;
    assert_eq!(in_use(&i).as_deref(), Some(SAVED), "a saved key wins");
    let r = api(&i, Method::DELETE, "/api/admin/vidoc", None, &i.admin).await?;
    assert_eq!(r.body["source"], "config", "back to the key file");
    assert_eq!(in_use(&i).as_deref(), Some(CONFIG));

    let r = api(&i, Method::POST, "/api/admin/vidoc/adopt", None, &i.admin).await?;
    assert_eq!(r.status, StatusCode::OK, "{}", r.body);
    assert_eq!(r.body["source"], "settings");
    let stored = i.h.store.vidoc_key().await?.ok_or("stored")?;
    assert_eq!(i.h.app.deps.crypto.open(&stored.sealed)?, CONFIG.as_bytes());

    let broken = instance(Err(
        "/var/lib/yulius/vidoc.key is readable by other users (mode 644); make it 0600".into(),
    ))
    .await?;
    let view = api(
        &broken,
        Method::GET,
        "/api/admin/vidoc",
        None,
        &broken.admin,
    )
    .await?;
    assert_eq!(view.body["configured"], false);
    assert!(
        view.body["config_error"]
            .as_str()
            .unwrap_or_default()
            .contains("0600")
    );
    let r = api(
        &broken,
        Method::POST,
        "/api/admin/vidoc/adopt",
        None,
        &broken.admin,
    )
    .await?;
    assert_eq!(r.status, StatusCode::CONFLICT);
    Ok(())
}

/// VDK-001: an instance built without Vidoc services still answers: no key, nothing to manage.
#[tokio::test]
async fn without_vidoc_services_the_key_is_simply_off() -> TestResult {
    let h = harness()?;
    let admin = setup_superadmin_creds(&h).await?;
    let r = call(&h.router, Method::GET, "/api/admin/vidoc", None, &admin).await?;
    assert_eq!(r.status, StatusCode::OK, "{}", r.body);
    assert_eq!(r.body["configured"], false);
    assert!(h.app.vidoc_key().is_none());
    Ok(())
}
