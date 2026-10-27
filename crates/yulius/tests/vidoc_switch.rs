//! A project's Vidoc switch (SPEC-338): turned off or on per sandbox through the sandbox API,
//! kept across profile updates, audited, and followed by the virtual key, the briefing and the
//! onboarding prompt.

use crate::support;

use std::sync::Arc;

use axum::http::{Method, StatusCode};
use serde_json::{Value, json};
use support::{
    Creds, Harness, TestResult, admit_and_sign_in, call, configure_github_app, harness, perms,
    seed_live_attempt, seed_sandbox, seed_task, setup_superadmin_creds,
};
use yulius_app::model::Secret;
use yulius_app::ports::egress::EgressPolicy;
use yulius_app::ports::store::{SandboxStore as _, UserStore as _};
use yulius_app::ports::vidoc::{VidocCheck, VidocConnector};
use yulius_app::vidoc_key::{VIDOC_HOST, VidocServices};
use yulius_domain::ids::SandboxId;
use yulius_domain::task::TaskStatus;

const KEY: &str = "vidoc-instance-key-0123456789";
const TOKEN: &str = "task-token-vidoc-switch-0123456789";

struct AnyVidoc;

#[async_trait::async_trait]
impl VidocConnector for AnyVidoc {
    async fn check(&self, _key: &Secret) -> VidocCheck {
        VidocCheck::Ok
    }
}

struct Instance {
    h: Harness,
    admin: Creds,
    alice: Creds,
    bob: Creds,
}

/// An instance with a Vidoc key and sandbox `s1` (`acme/app`): alice administers it, bob writes.
async fn instance() -> Result<Instance, Box<dyn std::error::Error>> {
    let h = harness()?;
    let admin = setup_superadmin_creds(&h).await?;
    configure_github_app(&h).await?;
    let alice = admit_and_sign_in(&h, &admin, "alice", 11, true).await?;
    let bob = admit_and_sign_in(&h, &admin, "bob", 12, true).await?;
    h.app.set_vidoc(VidocServices::new(
        Arc::new(AnyVidoc) as Arc<dyn VidocConnector>,
        Ok(Some(Secret(KEY.into()))),
    ));
    h.app.load_vidoc_key().await;
    let owner = h.store.user_by_login("alice").await?.ok_or("alice")?.id;
    seed_sandbox(&h, "s1", "app", owner.as_str()).await?;
    h.github.set_perm("alice", "acme/app", perms("admin"));
    h.github.set_perm("bob", "acme/app", perms("write"));
    Ok(Instance {
        h,
        admin,
        alice,
        bob,
    })
}

async fn switch(
    i: &Instance,
    body: Value,
    who: &Creds,
) -> Result<support::Resp, Box<dyn std::error::Error>> {
    call(
        &i.h.router,
        Method::PATCH,
        "/api/sandboxes/s1",
        Some(body),
        who,
    )
    .await
}

async fn shown(i: &Instance) -> Result<Value, Box<dyn std::error::Error>> {
    let r = call(
        &i.h.router,
        Method::GET,
        "/api/sandboxes/s1",
        None,
        &i.alice,
    )
    .await?;
    assert_eq!(r.status, StatusCode::OK, "{}", r.body);
    Ok(r.body["profile"]["vidoc"].clone())
}

/// The audit entries of `s1`'s Vidoc switch, as "actor detail".
async fn audited(i: &Instance) -> Result<Vec<String>, Box<dyn std::error::Error>> {
    use yulius_app::ports::store_ext::AutomationStore as _;
    Ok(i.h
        .store
        .automation_audit("app", 100)
        .await?
        .into_iter()
        .filter(|e| e.action == "delivery.vidoc.update")
        .map(|e| format!("{} {}", e.actor, e.detail.unwrap_or_default()))
        .collect())
}

/// VOF-001: on by default; an admin turns it off and on, a profile update keeps it, a writer
/// cannot change it, and each change is audited.
#[tokio::test]
async fn an_admin_turns_a_projects_vidoc_scans_off_and_on() -> TestResult {
    let i = instance().await?;
    assert_eq!(shown(&i).await?, true, "on by default");

    let denied = switch(&i, json!({"vidoc": false}), &i.bob).await?;
    assert_eq!(denied.status, StatusCode::FORBIDDEN, "{}", denied.body);
    assert_eq!(shown(&i).await?, true);

    let off = switch(&i, json!({"vidoc": false}), &i.alice).await?;
    assert_eq!(off.status, StatusCode::OK, "{}", off.body);
    assert_eq!(off.body["profile"]["vidoc"], false);
    assert_eq!(shown(&i).await?, false);

    // The delivery profile editor sends the profile without the switch: it stays off.
    let mut profile = off.body["profile"].clone();
    profile["vidoc"] = Value::Bool(true);
    profile["require_checks"] = Value::Bool(false);
    let edited = switch(&i, json!({ "profile": profile }), &i.alice).await?;
    assert_eq!(edited.status, StatusCode::OK, "{}", edited.body);
    assert_eq!(edited.body["profile"]["require_checks"], false);
    assert_eq!(edited.body["profile"]["vidoc"], false, "kept off");

    // The superadmin's Instance page turns it on again.
    let on = switch(&i, json!({"vidoc": true}), &i.admin).await?;
    assert_eq!(on.status, StatusCode::OK, "{}", on.body);
    assert_eq!(shown(&i).await?, true);
    assert_eq!(
        on.body["profile"]["require_checks"], false,
        "the profile stays"
    );

    assert_eq!(
        audited(&i).await?,
        ["superadmin on", "alice off"],
        "newest first"
    );
    Ok(())
}

/// VOF-001: a playground has no CI scan, so it has no switch to turn.
#[tokio::test]
async fn a_playground_refuses_the_switch() -> TestResult {
    let i = instance().await?;
    let mut playground =
        i.h.store
            .sandbox(&SandboxId::parse("s1")?)
            .await?
            .ok_or("s1")?;
    playground.id = SandboxId::parse("s3")?;
    playground.slug = "play".into();
    playground.repo_owner = String::new();
    playground.repo_name = "play".into();
    playground.github_repo_id = None;
    playground.port_base = i.h.store.next_port_base(43000, 20).await?;
    i.h.store.insert_sandbox(&playground).await?;
    let r = call(
        &i.h.router,
        Method::PATCH,
        "/api/sandboxes/s3",
        Some(json!({"vidoc": false})),
        &i.alice,
    )
    .await?;
    assert_eq!(r.status, StatusCode::UNPROCESSABLE_ENTITY, "{}", r.body);
    Ok(())
}

/// VOF-002: a switched-off project's runs do not get the instance key as `VIDOC_API_KEY`; turned
/// on again, they do.
#[tokio::test]
async fn a_switched_off_project_does_not_get_the_instance_key() -> TestResult {
    let i = instance().await?;
    let owner = i.h.store.user_by_login("alice").await?.ok_or("alice")?.id;
    seed_task(&i.h, "t1", "s1", owner.as_str(), TaskStatus::Running, None).await?;
    seed_live_attempt(&i.h, "t1", TOKEN).await?;
    let app: Arc<dyn EgressPolicy> = i.h.app.clone();
    let client = app.egress_client(TOKEN).await.ok_or("client")?;
    assert!(app.egress_intercepts(&client, VIDOC_HOST, 443).await);

    switch(&i, json!({"vidoc": false}), &i.alice).await?;
    let client = app.egress_client(TOKEN).await.ok_or("client")?;
    assert!(
        !app.egress_intercepts(&client, VIDOC_HOST, 443).await,
        "off: the instance key is not applied"
    );

    switch(&i, json!({"vidoc": true}), &i.alice).await?;
    let client = app.egress_client(TOKEN).await.ok_or("client")?;
    assert!(app.egress_intercepts(&client, VIDOC_HOST, 443).await);
    Ok(())
}

/// VOF-003: the briefing and the onboarding prompt say "off" for a switched-off project, and "on"
/// with the instance key once it is switched on again.
#[tokio::test]
async fn agents_are_told_when_a_project_is_switched_off() -> TestResult {
    use yulius_domain::briefing::VidocBriefing;
    let i = instance().await?;
    let s1 = SandboxId::parse("s1")?;
    let record = i.h.store.sandbox(&s1).await?.ok_or("s1")?;
    assert_eq!(i.h.app.vidoc_briefing(&record), VidocBriefing::On);
    switch(&i, json!({"vidoc": false}), &i.alice).await?;
    let record = i.h.store.sandbox(&s1).await?.ok_or("s1")?;
    assert_eq!(i.h.app.vidoc_briefing(&record), VidocBriefing::Off);
    switch(&i, json!({"vidoc": true}), &i.alice).await?;
    let record = i.h.store.sandbox(&s1).await?.ok_or("s1")?;
    assert_eq!(i.h.app.vidoc_briefing(&record), VidocBriefing::On);
    Ok(())
}
