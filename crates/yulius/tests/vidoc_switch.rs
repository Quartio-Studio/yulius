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
    instance_on(harness()?).await
}

/// [`instance`] on harness `h`.
async fn instance_on(h: Harness) -> Result<Instance, Box<dyn std::error::Error>> {
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

/// What the egress proxy does with a scan's request to Vidoc from `client`.
#[derive(Debug, PartialEq)]
enum TowardVidoc {
    /// Not intercepted: the request would go to Vidoc as sent.
    Tunneled,
    /// Forwarded with these secrets applied.
    Forwarded(Vec<String>),
    /// Refused with this status and text: nothing reaches Vidoc.
    Refused(u16, String),
}

async fn toward_vidoc(
    app: &Arc<dyn EgressPolicy>,
    token: &str,
) -> Result<TowardVidoc, Box<dyn std::error::Error>> {
    use yulius_app::ports::egress::{EgressDecision, EgressRequest};
    let client = app.egress_client(token).await.ok_or("client")?;
    if !app.egress_intercepts(&client, VIDOC_HOST, 443).await {
        return Ok(TowardVidoc::Tunneled);
    }
    let request = EgressRequest {
        host: VIDOC_HOST.into(),
        port: 443,
        path_and_query: "/v1/messages".into(),
        headers: vec![("x-api-key".into(), b"yulius-secret-VIDOC_API_KEY".to_vec())],
    };
    Ok(match app.egress_apply(&client, request).await {
        EgressDecision::Forward { used, .. } => TowardVidoc::Forwarded(used),
        EgressDecision::Refuse { status, message } => TowardVidoc::Refused(status, message),
    })
}

/// A request refused because Vidoc is off, with the text the scanner recognizes (review round 2).
fn refused_as_off(answer: &TowardVidoc) -> bool {
    use yulius_app::vidoc_key::{VIDOC_OFF_MARKER, VIDOC_OFF_REFUSAL};
    matches!(answer, TowardVidoc::Refused(403, message)
        if message == VIDOC_OFF_REFUSAL && message.starts_with(VIDOC_OFF_MARKER)
            && message.contains("nothing was sent"))
}

/// SPEC-356 (review round 2): the proxy's refusal and the scanner agree on its marker, so a
/// running session's scan says Vidoc is off rather than that Vidoc refused the key.
#[test]
fn the_scanner_recognizes_the_proxys_refusal() {
    use yulius_app::vidoc_key::{VIDOC_OFF_MARKER, VIDOC_OFF_REFUSAL};
    assert!(VIDOC_OFF_REFUSAL.starts_with(VIDOC_OFF_MARKER));
    assert!(
        yulius::security_scan::SCANNER.contains(&format!("\"{VIDOC_OFF_MARKER}\" in message")),
        "scripts/vidoc_scan.py matches the marker"
    );
    assert!(
        include_str!("../../../scripts/test_vidoc_scan.py").contains(VIDOC_OFF_REFUSAL.trim_end()),
        "the scanner's test uses the proxy's own text"
    );
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
/// on again, they do. SPEC-356 (review round 1): a run that started before the switch went off
/// (its environment without `YULIUS_VIDOC=off`) has its scan's request refused by the proxy, so
/// nothing reaches Vidoc.
#[tokio::test]
async fn a_switched_off_project_does_not_get_the_instance_key() -> TestResult {
    let i = instance().await?;
    let owner = i.h.store.user_by_login("alice").await?.ok_or("alice")?.id;
    seed_task(&i.h, "t1", "s1", owner.as_str(), TaskStatus::Running, None).await?;
    seed_live_attempt(&i.h, "t1", TOKEN).await?;
    let app: Arc<dyn EgressPolicy> = i.h.app.clone();
    let on = TowardVidoc::Forwarded(vec!["VIDOC_API_KEY".into()]);
    assert_eq!(toward_vidoc(&app, TOKEN).await?, on);

    switch(&i, json!({"vidoc": false}), &i.alice).await?;
    let answer = toward_vidoc(&app, TOKEN).await?;
    assert!(refused_as_off(&answer), "off: {answer:?}");

    switch(&i, json!({"vidoc": true}), &i.alice).await?;
    assert_eq!(toward_vidoc(&app, TOKEN).await?, on);
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

/// VOF-004: the web UI learns whether the instance has a Vidoc key (the Add sandbox dialog offers
/// the switch only then), never the key.
#[tokio::test]
async fn me_says_whether_the_instance_has_a_vidoc_key() -> TestResult {
    let i = instance().await?;
    let me = call(&i.h.router, Method::GET, "/api/me", None, &i.bob).await?;
    assert_eq!(me.status, StatusCode::OK, "{}", me.body);
    assert_eq!(me.body["instance"]["vidoc"], true, "{}", me.body);
    assert!(!me.body.to_string().contains(KEY));
    Ok(())
}

/// VOF-001 (review round 1): a request whose profile is refused changes nothing, the switch
/// included.
#[tokio::test]
async fn a_refused_profile_leaves_the_switch_as_it_was() -> TestResult {
    let i = instance().await?;
    let bad = json!({"preset": "standard", "integration_branch": "main", "production_branch": "main",
        "merge": "squash", "promote": "merge_commit", "auto_promote": false, "environments": []});
    let refused = switch(&i, json!({"vidoc": false, "profile": bad}), &i.alice).await?;
    assert_eq!(
        refused.status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "{}",
        refused.body
    );
    assert_eq!(shown(&i).await?, true, "the switch did not change");
    assert!(audited(&i).await?.is_empty());
    Ok(())
}

async fn switch_instance(
    i: &Instance,
    enabled: bool,
    who: &Creds,
) -> Result<support::Resp, Box<dyn std::error::Error>> {
    call(
        &i.h.router,
        Method::PUT,
        "/api/admin/vidoc/enabled",
        Some(json!({ "enabled": enabled })),
        who,
    )
    .await
}

/// The instance switch's audit entries, as "actor action", newest first.
async fn instance_audited(i: &Instance) -> Result<Vec<String>, Box<dyn std::error::Error>> {
    use yulius_app::ports::store_ext::AutomationStore as _;
    Ok(i.h
        .store
        .automation_audit("instance:vidoc", 100)
        .await?
        .into_iter()
        .filter(|e| e.action == "vidoc.disabled" || e.action == "vidoc.enabled")
        .map(|e| format!("{} {}", e.actor, e.action))
        .collect())
}

/// SPEC-356 VIO-001: a superadmin, and only a superadmin, switches Vidoc off for the instance
/// and on again; the key stays, and each change is audited.
#[tokio::test]
async fn a_superadmin_switches_vidoc_off_for_the_instance_and_on_again() -> TestResult {
    use yulius_app::ports::store_ext::UpdateStore as _;
    let i = instance().await?;
    let view = call(&i.h.router, Method::GET, "/api/admin/vidoc", None, &i.admin).await?;
    assert_eq!(view.status, StatusCode::OK, "{}", view.body);
    assert_eq!(view.body["enabled"], true, "on by default: {}", view.body);

    let denied = switch_instance(&i, false, &i.alice).await?;
    assert_eq!(denied.status, StatusCode::FORBIDDEN, "{}", denied.body);

    let off = switch_instance(&i, false, &i.admin).await?;
    assert_eq!(off.status, StatusCode::OK, "{}", off.body);
    assert_eq!(off.body["enabled"], false, "{}", off.body);
    assert_eq!(off.body["disabled_by"], "superadmin", "{}", off.body);
    assert!(off.body["disabled_at"].is_string(), "{}", off.body);
    assert_eq!(off.body["configured"], true, "the key is kept");
    assert!(i.h.app.vidoc_key().is_some(), "the key is kept");
    // Stored, so a restart keeps it (review round 1).
    let stored = i.h.store.vidoc_off().await?.ok_or("stored off")?;
    assert_eq!(stored.by.as_deref(), Some("superadmin"));

    let again = switch_instance(&i, false, &i.admin).await?;
    assert_eq!(again.status, StatusCode::OK, "{}", again.body);

    let on = switch_instance(&i, true, &i.admin).await?;
    assert_eq!(on.status, StatusCode::OK, "{}", on.body);
    assert_eq!(on.body["enabled"], true, "{}", on.body);
    assert!(on.body["disabled_by"].is_null(), "{}", on.body);
    assert_eq!(i.h.store.vidoc_off().await?, None, "stored on");

    assert_eq!(
        instance_audited(&i).await?,
        ["superadmin vidoc.enabled", "superadmin vidoc.disabled"],
        "a repeated off changes nothing; newest first"
    );
    Ok(())
}

/// SPEC-356 VIO-001: the switch is stored, so a restarted server starts with Vidoc off.
#[tokio::test]
async fn a_restarted_server_keeps_vidoc_off_for_the_instance() -> TestResult {
    use yulius_app::ports::store_ext::UpdateStore as _;
    let h = harness()?;
    let admin = setup_superadmin_creds(&h).await?;
    h.store
        .set_vidoc_off(
            Some("superadmin"),
            yulius_domain::time::Timestamp(1_800_000_000),
        )
        .await?;
    h.app.set_vidoc(VidocServices::new(
        Arc::new(AnyVidoc) as Arc<dyn VidocConnector>,
        Ok(Some(Secret(KEY.into()))),
    ));
    h.app.load_vidoc_key().await;
    let view = call(&h.router, Method::GET, "/api/admin/vidoc", None, &admin).await?;
    assert_eq!(view.status, StatusCode::OK, "{}", view.body);
    assert_eq!(view.body["enabled"], false, "{}", view.body);
    assert_eq!(view.body["disabled_by"], "superadmin", "{}", view.body);
    Ok(())
}

/// SPEC-356 VIO-002..VIO-005: while the instance is off, a switched-on project gets no instance
/// key, its agents are told, `me` offers no Vidoc box, and the project keeps its own choice,
/// which is followed again once the instance is on.
#[tokio::test]
async fn every_project_follows_the_instance_switch() -> TestResult {
    use yulius_domain::briefing::VidocBriefing;
    let i = instance().await?;
    let owner = i.h.store.user_by_login("alice").await?.ok_or("alice")?.id;
    seed_task(&i.h, "t1", "s1", owner.as_str(), TaskStatus::Running, None).await?;
    seed_live_attempt(&i.h, "t1", TOKEN).await?;
    let app: Arc<dyn EgressPolicy> = i.h.app.clone();
    let s1 = SandboxId::parse("s1")?;

    switch_instance(&i, false, &i.admin).await?;
    let answer = toward_vidoc(&app, TOKEN).await?;
    assert!(refused_as_off(&answer), "instance off: {answer:?}");
    let record = i.h.store.sandbox(&s1).await?.ok_or("s1")?;
    assert_eq!(
        i.h.app.vidoc_briefing(&record),
        VidocBriefing::OffForInstance
    );
    assert!(!i.h.app.vidoc_scans_on(&record), "no scan for the project");
    assert_eq!(shown(&i).await?, true, "the project keeps its own choice");
    let me = call(&i.h.router, Method::GET, "/api/me", None, &i.bob).await?;
    assert_eq!(me.body["instance"]["vidoc"], false, "{}", me.body);

    switch_instance(&i, true, &i.admin).await?;
    assert_eq!(
        toward_vidoc(&app, TOKEN).await?,
        TowardVidoc::Forwarded(vec!["VIDOC_API_KEY".into()])
    );
    let record = i.h.store.sandbox(&s1).await?.ok_or("s1")?;
    assert_eq!(i.h.app.vidoc_briefing(&record), VidocBriefing::On);
    let me = call(&i.h.router, Method::GET, "/api/me", None, &i.bob).await?;
    assert_eq!(me.body["instance"]["vidoc"], true, "{}", me.body);
    Ok(())
}

/// `s1`'s Vidoc view (SPEC-359 VSB-001) as `who`.
async fn vidoc_view(i: &Instance, who: &Creds) -> Result<Value, Box<dyn std::error::Error>> {
    let r = call(
        &i.h.router,
        Method::GET,
        "/api/sandboxes/s1/vidoc",
        None,
        who,
    )
    .await?;
    assert_eq!(r.status, StatusCode::OK, "{}", r.body);
    Ok(r.body)
}

/// `s1`'s clone with `origin/dev` at a commit holding `.config/vidoc.toml` as `config` (none
/// for `None`).
fn integration_head(i: &Instance, config: Option<&str>) -> TestResult {
    let repo = i.h.app.config.data_dir.join("sandboxes/s1/repo");
    std::fs::create_dir_all(repo.join(".config"))?;
    let git = |args: &[&str]| -> TestResult {
        let out = std::process::Command::new("git")
            .args([
                "-c",
                "user.name=Yulius Test",
                "-c",
                "user.email=test@yulius.test",
                "-c",
                "commit.gpgsign=false",
            ])
            .args(args)
            .current_dir(&repo)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .output()?;
        if !out.status.success() {
            return Err(format!("git {args:?}: {}", String::from_utf8_lossy(&out.stderr)).into());
        }
        Ok(())
    };
    if !repo.join(".git").exists() {
        git(&["init", "-q", "-b", "main"])?;
    }
    let file = repo.join(".config/vidoc.toml");
    match config {
        Some(text) => std::fs::write(&file, text)?,
        None => {
            let _ = std::fs::remove_file(&file);
        }
    }
    std::fs::write(repo.join("README.md"), format!("{config:?}\n"))?;
    git(&["add", "-A"])?;
    git(&["commit", "-q", "-m", "head"])?;
    git(&["update-ref", "refs/remotes/origin/dev", "HEAD"])?;
    Ok(())
}

/// SPEC-359 VSB-001, VSB-002: the sandbox's admin (not a superadmin) switches its Vidoc scans off
/// and on in the sandbox; a writer sees the state but cannot change it; the superadmin's change
/// (the Instance card) and the sandbox's are one switch, and each is audited with who and when.
#[tokio::test]
async fn a_sandbox_admin_switches_vidoc_in_the_sandbox_and_the_instance_card_agrees() -> TestResult
{
    let i = instance().await?;
    let seen = vidoc_view(&i, &i.bob).await?;
    assert_eq!(seen["project"], true, "{seen}");
    assert_eq!(seen["instance"], true);
    assert_eq!(seen["key"], true, "the instance key reaches it");
    assert_eq!(seen["repository"], "on", "no clone: nothing says off");
    assert_eq!(seen["scans"], true);
    assert_eq!(seen["can_change"], false, "a writer cannot");
    assert_eq!(seen["changed_by"], Value::Null);
    let denied = switch(&i, json!({"vidoc": false}), &i.bob).await?;
    assert_eq!(denied.status, StatusCode::FORBIDDEN, "{}", denied.body);

    let alice = i.h.store.user_by_login("alice").await?.ok_or("alice")?;
    assert!(!alice.is_superadmin, "alice is only the sandbox's admin");
    assert_eq!(vidoc_view(&i, &i.alice).await?["can_change"], true);
    let off = switch(&i, json!({"vidoc": false}), &i.alice).await?;
    assert_eq!(off.status, StatusCode::OK, "{}", off.body);
    let seen = vidoc_view(&i, &i.bob).await?;
    assert_eq!(seen["project"], false, "{seen}");
    assert_eq!(seen["scans"], false);
    assert_eq!(seen["changed_by"], "alice");
    assert!(
        seen["changed_at"]
            .as_str()
            .is_some_and(|at| at.contains('T')),
        "{seen}"
    );

    // The Instance card reads the sandbox list: the same switch.
    let list = call(&i.h.router, Method::GET, "/api/sandboxes", None, &i.admin).await?;
    let s1 = list
        .body
        .as_array()
        .ok_or("list")?
        .iter()
        .find(|s| s["id"] == "s1")
        .ok_or("s1")?;
    assert_eq!(s1["profile"]["vidoc"], false, "{s1}");
    let on = switch(&i, json!({"vidoc": true}), &i.admin).await?;
    assert_eq!(on.status, StatusCode::OK, "{}", on.body);
    let seen = vidoc_view(&i, &i.alice).await?;
    assert_eq!(seen["project"], true, "{seen}");
    assert_eq!(seen["changed_by"], "superadmin");
    assert_eq!(vidoc_view(&i, &i.admin).await?["can_change"], true);

    assert_eq!(audited(&i).await?, ["superadmin on", "alice off"]);
    Ok(())
}

/// SPEC-359 VSB-003: `enabled = false` committed on the integration branch wins over Yulius's
/// switch; a file the scanner would refuse is said; an instance switched off is said.
#[tokio::test]
async fn the_repositorys_committed_switch_wins_and_is_said() -> TestResult {
    let git = Arc::new(yulius::wiring::git::GitAdapter::new("git"));
    let i = instance_on(support::harness_with_git(git)?).await?;
    integration_head(&i, None)?;
    assert_eq!(vidoc_view(&i, &i.alice).await?["repository"], "on");

    integration_head(&i, Some("# paused\nenabled = false\n"))?;
    let seen = vidoc_view(&i, &i.alice).await?;
    assert_eq!(seen["repository"], "off", "{seen}");
    assert_eq!(seen["branch"], "dev");
    assert_eq!(seen["project"], true, "Yulius's own switch is kept");
    assert_eq!(seen["scans"], false, "the repository wins");

    integration_head(&i, Some("enabled = \"no\"\n"))?;
    let seen = vidoc_view(&i, &i.alice).await?;
    assert_eq!(seen["repository"], "unreadable", "{seen}");
    assert!(
        seen["repository_problem"]
            .as_str()
            .is_some_and(|p| p.contains("enabled")),
        "{seen}"
    );

    integration_head(&i, Some("enabled = true\n"))?;
    switch_instance(&i, false, &i.admin).await?;
    let seen = vidoc_view(&i, &i.alice).await?;
    assert_eq!(seen["repository"], "on", "{seen}");
    assert_eq!(seen["instance"], false);
    assert_eq!(seen["scans"], false);
    Ok(())
}

/// SPEC-359 VSB-001: a playground has no switch, and a stranger does not see the sandbox.
#[tokio::test]
async fn a_playground_has_no_vidoc_view() -> TestResult {
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
        Method::GET,
        "/api/sandboxes/s3/vidoc",
        None,
        &i.alice,
    )
    .await?;
    assert_eq!(r.status, StatusCode::UNPROCESSABLE_ENTITY, "{}", r.body);
    Ok(())
}
