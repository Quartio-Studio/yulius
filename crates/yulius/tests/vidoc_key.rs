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
    Creds, Harness, TestResult, admit_and_sign_in, call, configure_github_app, harness, mcp_tool,
    perms, seed_live_attempt, seed_sandbox, seed_task, setup_superadmin_creds, tool_text,
};
use yulius_app::model::Secret;
use yulius_app::ports::egress::{EgressDecision, EgressPolicy, EgressRequest};
use yulius_app::ports::store::{SandboxStore as _, UserStore as _};
use yulius_app::ports::store_ext::UpdateStore as _;
use yulius_app::ports::vidoc::{VidocCheck, VidocConnector};
use yulius_app::vidoc_key::VIDOC_HOST;
use yulius_app::vidoc_key::VidocServices;
use yulius_domain::ids::SandboxId;
use yulius_domain::task::TaskStatus;
use yulius_domain::vault::redact::redact_text;

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

/// The audit entries of the instance Vidoc key, by action.
async fn audited(i: &Instance, action: &str) -> Result<Vec<String>, Box<dyn std::error::Error>> {
    use yulius_app::ports::store_ext::AutomationStore as _;
    Ok(i.h
        .store
        .automation_audit("instance:vidoc", 100)
        .await?
        .into_iter()
        .filter(|e| e.action == action)
        .map(|e| e.actor)
        .collect())
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

    // Audited, by who changed it; removing nothing records nothing.
    assert_eq!(audited(&i, "vidoc.key_set").await?.len(), 3);
    assert_eq!(audited(&i, "vidoc.key_removed").await?, ["superadmin"]);
    api(&i, Method::DELETE, "/api/admin/vidoc", None, &i.admin).await?;
    assert_eq!(audited(&i, "vidoc.key_removed").await?.len(), 1);
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

const TOKEN_A: &str = "vidoc-task-token-a-0123456789abcdef";
const TOKEN_B: &str = "vidoc-task-token-b-0123456789abcdef";
const OWN: &str = "vidoc-project-own-key-0123456789";

fn request(host: &str, headers: &[(&str, &str)]) -> EgressRequest {
    EgressRequest {
        host: host.into(),
        port: 443,
        path_and_query: "/v1/messages".into(),
        headers: headers
            .iter()
            .map(|(n, v)| ((*n).to_owned(), v.as_bytes().to_vec()))
            .collect(),
    }
}

fn forwarded(decision: &EgressDecision, name: &str) -> Option<(String, Vec<String>)> {
    match decision {
        EgressDecision::Forward { headers, used, .. } => headers
            .iter()
            .find(|(n, _)| n == name)
            .and_then(|(_, v)| String::from_utf8(v.clone()).ok())
            .map(|v| (v, used.clone())),
        EgressDecision::Refuse { .. } => None,
    }
}

/// Sandboxes `s1` (no Vidoc secret of its own) and `s2` (its own `VIDOC_API_KEY`), each with a
/// running task.
async fn proxied(i: &Instance) -> TestResult {
    let alice = i.h.store.user_by_login("alice").await?.ok_or("alice")?.id;
    seed_sandbox(&i.h, "s1", "app", alice.as_str()).await?;
    seed_sandbox(&i.h, "s2", "other", alice.as_str()).await?;
    i.h.github.set_perm("alice", "acme/app", perms("write"));
    i.h.github.set_perm("alice", "acme/other", perms("write"));
    seed_task(&i.h, "t1", "s1", alice.as_str(), TaskStatus::Running, None).await?;
    seed_live_attempt(&i.h, "t1", TOKEN_A).await?;
    seed_task(&i.h, "t2", "s2", alice.as_str(), TaskStatus::Running, None).await?;
    seed_live_attempt(&i.h, "t2", TOKEN_B).await?;
    let own = call(
        &i.h.router,
        Method::POST,
        "/api/sandboxes/s2/agent-secrets",
        Some(json!({"name": "VIDOC_API_KEY", "hosts": ["model.vidocsecurity.com"], "value": OWN})),
        &i.alice,
    )
    .await?;
    assert_eq!(own.status, StatusCode::CREATED, "{}", own.body);
    Ok(())
}

/// VDK-005: the instance key is the virtual agent secret `VIDOC_API_KEY`: its placeholder is
/// replaced toward model.vidocsecurity.com only, the value is redacted, `secrets_list` names it,
/// a project's own `VIDOC_API_KEY` wins, and a CI job gets it only when granted.
#[tokio::test]
async fn the_instance_key_reaches_vidoc_through_the_proxy_only() -> TestResult {
    let i = instance(Ok(None)).await?;
    proxied(&i).await?;
    let app: Arc<dyn EgressPolicy> = i.h.app.clone();
    let s1 = app.egress_client(TOKEN_A).await.ok_or("s1 client")?;
    let s2 = app.egress_client(TOKEN_B).await.ok_or("s2 client")?;
    assert!(
        !app.egress_intercepts(&s1, VIDOC_HOST, 443).await,
        "no key: nothing to add"
    );

    api(
        &i,
        Method::PUT,
        "/api/admin/vidoc",
        Some(json!({ "key": SAVED })),
        &i.admin,
    )
    .await?;
    assert!(app.egress_intercepts(&s1, VIDOC_HOST, 443).await);
    assert!(!app.egress_intercepts(&s1, "api.example.test", 443).await);
    assert!(!app.egress_intercepts(&s1, VIDOC_HOST, 8443).await);
    let placeholder = "yulius-secret-VIDOC_API_KEY";
    let sent = app
        .egress_apply(&s1, request(VIDOC_HOST, &[("x-api-key", placeholder)]))
        .await;
    assert_eq!(
        forwarded(&sent, "x-api-key"),
        Some((SAVED.to_owned(), vec!["VIDOC_API_KEY".to_owned()]))
    );
    assert_eq!(
        forwarded(&sent, "authorization"),
        None,
        "nothing is added: the key replaces its placeholder only"
    );
    assert_eq!(
        audited(&i, "vault.agent.use").await?,
        ["task:1"],
        "each use is audited under the instance key"
    );
    let redactor = app.egress_redactor(&s1).await;
    let (redacted, _) = redact_text(redactor.as_ref(), &format!("echo {SAVED}"));
    assert_eq!(redacted, "echo [secret:VIDOC_API_KEY]");

    // A project's own key wins over the instance's.
    let own = app
        .egress_apply(&s2, request(VIDOC_HOST, &[("x-api-key", placeholder)]))
        .await;
    assert_eq!(
        forwarded(&own, "x-api-key").map(|(v, _)| v).as_deref(),
        Some(OWN)
    );

    // Replaced: in use at once, redaction too.
    api(
        &i,
        Method::PUT,
        "/api/admin/vidoc",
        Some(json!({ "key": REPLACED })),
        &i.admin,
    )
    .await?;
    let sent = app
        .egress_apply(&s1, request(VIDOC_HOST, &[("x-api-key", placeholder)]))
        .await;
    assert_eq!(
        forwarded(&sent, "x-api-key").map(|(v, _)| v).as_deref(),
        Some(REPLACED)
    );
    let (redacted, _) = redact_text(app.egress_redactor(&s1).await.as_ref(), REPLACED);
    assert_eq!(redacted, "[secret:VIDOC_API_KEY]");

    // Agents see it listed, without a value.
    let listed = tool_text(&mcp_tool(&i.h, TOKEN_A, "secrets_list", json!({})).await?).to_string();
    assert!(listed.contains("VIDOC_API_KEY"), "{listed}");
    assert!(listed.contains("model.vidocsecurity.com"), "{listed}");
    assert!(listed.contains("instance"), "{listed}");
    assert!(!listed.contains(REPLACED), "{listed}");

    // A CI job: only when granted.
    let sid = SandboxId::parse("s1")?;
    let granted =
        i.h.app
            .ci_egress_open(&sid, "run-1", "job-1", &["VIDOC_API_KEY".to_owned()])
            .ok_or("credential")?;
    let ci = app.egress_client(&granted).await.ok_or("ci client")?;
    assert!(app.egress_intercepts(&ci, VIDOC_HOST, 443).await);
    let sent = app
        .egress_apply(&ci, request(VIDOC_HOST, &[("x-api-key", placeholder)]))
        .await;
    assert_eq!(
        forwarded(&sent, "x-api-key").map(|(v, _)| v).as_deref(),
        Some(REPLACED)
    );
    let other =
        i.h.app
            .ci_egress_open(&sid, "run-1", "job-2", &[])
            .ok_or("credential")?;
    let ci = app.egress_client(&other).await.ok_or("ci client")?;
    assert!(!app.egress_intercepts(&ci, VIDOC_HOST, 443).await);

    // Removed: nothing is added any more.
    api(&i, Method::DELETE, "/api/admin/vidoc", None, &i.admin).await?;
    assert!(!app.egress_intercepts(&s1, VIDOC_HOST, 443).await);
    Ok(())
}

/// VDK-005: the instance key is never handed out in clear: no `.env` copy, no MCP reference, no
/// browser filling, and the Secrets page does not list it.
#[tokio::test]
async fn the_instance_key_is_never_handed_out_in_clear() -> TestResult {
    let i = instance(Ok(Some(Secret(CONFIG.into())))).await?;
    proxied(&i).await?;
    let copied = mcp_tool(
        &i.h,
        TOKEN_A,
        "secret_copy_to_env",
        json!({"secrets": ["VIDOC_API_KEY"], "reason": "test"}),
    )
    .await?;
    let text = tool_text(&copied).to_string();
    assert!(!text.contains(CONFIG), "{text}");
    let s1 =
        i.h.store
            .sandbox(&SandboxId::parse("s1")?)
            .await?
            .ok_or("s1")?;
    assert_eq!(
        i.h.app.resolve_secret_ref(&s1, "VIDOC_API_KEY").await?,
        None,
        "no MCP ${{secret:…}} reference resolves to it"
    );
    let filled = tool_text(
        &mcp_tool(
            &i.h,
            TOKEN_A,
            "browser_fill_secret",
            json!({"tab_id": "tab-1", "secret": "VIDOC_API_KEY"}),
        )
        .await?,
    )
    .to_string();
    assert!(filled.contains("no agent secret named"), "{filled}");
    assert!(!filled.contains(CONFIG), "{filled}");
    let page = call(
        &i.h.router,
        Method::GET,
        "/api/sandboxes/s1/agent-secrets",
        None,
        &i.alice,
    )
    .await?;
    assert_eq!(page.status, StatusCode::OK, "{}", page.body);
    assert!(
        page.body
            .as_array()
            .is_some_and(|a| a.iter().all(|s| s["name"] != "VIDOC_API_KEY")),
        "{}",
        page.body
    );
    Ok(())
}
