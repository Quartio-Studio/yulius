//! A project's production protection requirement (SPEC-423, ADR-0267): a sandbox admin turns it
//! off when the GitHub plan cannot have rulesets, through the sandbox API; it is kept across
//! profile updates, audited, and silences the onboarding warning about the production branch.

use crate::support;

use axum::http::{Method, StatusCode};
use serde_json::{Value, json};
use support::{
    Creds, Harness, TestResult, admit_and_sign_in, call, configure_github_app, harness, perms,
    seed_sandbox, setup_superadmin_creds,
};
use yulius_app::ports::store::{SandboxStore as _, UserStore as _};
use yulius_domain::ids::SandboxId;

struct Instance {
    h: Harness,
    admin: Creds,
    alice: Creds,
    bob: Creds,
}

/// Sandbox `s1` (`acme/app`, production branch `main`, unprotected): alice administers it, bob
/// writes.
async fn instance() -> Result<Instance, Box<dyn std::error::Error>> {
    let h = harness()?;
    let admin = setup_superadmin_creds(&h).await?;
    configure_github_app(&h).await?;
    let alice = admit_and_sign_in(&h, &admin, "alice", 11, true).await?;
    let bob = admit_and_sign_in(&h, &admin, "bob", 12, true).await?;
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

async fn patch(
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
    let r = call(&i.h.router, Method::GET, "/api/sandboxes/s1", None, &i.bob).await?;
    assert_eq!(r.status, StatusCode::OK, "{}", r.body);
    Ok(r.body["profile"]["protect_production"].clone())
}

/// The audit entries of `s1`'s switch, as "actor detail".
async fn audited(i: &Instance) -> Result<Vec<String>, Box<dyn std::error::Error>> {
    use yulius_app::ports::store_ext::AutomationStore as _;
    Ok(i.h
        .store
        .automation_audit("app", 100)
        .await?
        .into_iter()
        .filter(|e| e.action == "delivery.production_protection.update")
        .map(|e| format!("{} {}", e.actor, e.detail.unwrap_or_default()))
        .collect())
}

async fn warnings(i: &Instance) -> Result<Vec<String>, Box<dyn std::error::Error>> {
    let record =
        i.h.store
            .sandbox(&SandboxId::parse("s1")?)
            .await?
            .ok_or("s1")?;
    Ok(i.h.app.onboarding_status(&record).await?.warnings)
}

/// PPR-001/002 (AC1): required by default; a writer cannot change it; an admin turns it off, a
/// profile update keeps it, a superadmin turns it on again; each change is audited.
#[tokio::test]
async fn an_admin_turns_the_production_protection_requirement_off_and_on() -> TestResult {
    let i = instance().await?;
    assert_eq!(shown(&i).await?, true, "required by default");

    let denied = patch(&i, json!({"protect_production": false}), &i.bob).await?;
    assert_eq!(denied.status, StatusCode::FORBIDDEN, "{}", denied.body);
    assert_eq!(shown(&i).await?, true);

    let off = patch(&i, json!({"protect_production": false}), &i.alice).await?;
    assert_eq!(off.status, StatusCode::OK, "{}", off.body);
    assert_eq!(off.body["profile"]["protect_production"], false);
    assert_eq!(off.body["profile"]["vidoc"], true, "the Vidoc switch stays");
    assert_eq!(shown(&i).await?, false);

    // The delivery profile editor sends the profile without the switch: it stays off.
    let mut profile = off.body["profile"].clone();
    profile["protect_production"] = Value::Bool(true);
    profile["require_checks"] = Value::Bool(false);
    let edited = patch(&i, json!({ "profile": profile }), &i.alice).await?;
    assert_eq!(edited.status, StatusCode::OK, "{}", edited.body);
    assert_eq!(edited.body["profile"]["require_checks"], false);
    assert_eq!(
        edited.body["profile"]["protect_production"], false,
        "kept off"
    );

    // Sent with a profile, both are written.
    let mut profile = edited.body["profile"].clone();
    profile["require_checks"] = Value::Bool(true);
    let both = patch(
        &i,
        json!({ "profile": profile, "protect_production": true }),
        &i.admin,
    )
    .await?;
    assert_eq!(both.status, StatusCode::OK, "{}", both.body);
    assert_eq!(both.body["profile"]["require_checks"], true);
    assert_eq!(shown(&i).await?, true);

    // An unchanged value writes no audit entry.
    let same = patch(&i, json!({"protect_production": true}), &i.alice).await?;
    assert_eq!(same.status, StatusCode::OK, "{}", same.body);
    assert_eq!(
        audited(&i).await?,
        ["superadmin on", "alice off"],
        "newest first"
    );
    Ok(())
}

/// PPR-003 (AC2): while the requirement is off, onboarding no longer warns about the unprotected
/// production branch; on again, it does.
#[tokio::test]
async fn switched_off_the_unprotected_production_branch_is_not_warned_about() -> TestResult {
    let i = instance().await?;
    let before = warnings(&i).await?;
    assert_eq!(before.len(), 1, "{before:?}");

    let off = patch(&i, json!({"protect_production": false}), &i.alice).await?;
    assert_eq!(off.status, StatusCode::OK, "{}", off.body);
    assert!(warnings(&i).await?.is_empty());

    let on = patch(&i, json!({"protect_production": true}), &i.alice).await?;
    assert_eq!(on.status, StatusCode::OK, "{}", on.body);
    assert_eq!(warnings(&i).await?, before);
    Ok(())
}

/// PPR-004 (AC3): the warning says exactly where the requirement is turned off.
#[tokio::test]
async fn the_warning_names_the_git_page_switch() -> TestResult {
    let i = instance().await?;
    let warning = warnings(&i).await?.join("\n");
    assert!(
        warning.contains("/s/app/git?show=production-protection")
            && warning.contains("Production protection")
            && warning.contains("Require a GitHub ruleset on `main`"),
        "{warning}"
    );
    Ok(())
}

/// PPR-002 (review round 1): waiving the protection is a person's decision, so a `yulius login`
/// token, which an agent may hold, cannot turn it off; it can turn it on again.
#[tokio::test]
async fn a_cli_token_cannot_waive_the_protection() -> TestResult {
    let i = instance().await?;
    let alice = i.h.store.user_by_login("alice").await?.ok_or("alice")?;
    let token =
        i.h.app
            .issue_api_token(&alice, "tests", yulius_domain::permission::TokenScope::Cli)
            .await?;
    let cli = Creds {
        bearer: Some(token.expose().to_owned()),
        ..Creds::default()
    };
    let refused = patch(&i, json!({"protect_production": false}), &cli).await?;
    assert_eq!(refused.status, StatusCode::FORBIDDEN, "{}", refused.body);
    assert!(
        refused.body.to_string().contains("Git page"),
        "{}",
        refused.body
    );
    assert_eq!(shown(&i).await?, true);

    let off = patch(&i, json!({"protect_production": false}), &i.alice).await?;
    assert_eq!(off.status, StatusCode::OK, "{}", off.body);
    let on = patch(&i, json!({"protect_production": true}), &cli).await?;
    assert_eq!(on.status, StatusCode::OK, "{}", on.body);
    assert_eq!(shown(&i).await?, true);
    Ok(())
}

/// PPR-002 (review round 1): a refused profile changes nothing, the switch included; both switches
/// sent without a profile are written together.
#[tokio::test]
async fn a_refused_profile_leaves_the_switch_alone() -> TestResult {
    let i = instance().await?;
    let mut profile = patch(&i, json!({}), &i.alice).await?.body["profile"].clone();
    profile["production_branch"] = profile["integration_branch"].clone();
    let refused = patch(
        &i,
        json!({"profile": profile, "protect_production": false}),
        &i.alice,
    )
    .await?;
    assert_eq!(
        refused.status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "{}",
        refused.body
    );
    assert_eq!(shown(&i).await?, true);
    assert!(audited(&i).await?.is_empty());

    let both = patch(
        &i,
        json!({"vidoc": false, "protect_production": false}),
        &i.alice,
    )
    .await?;
    assert_eq!(both.status, StatusCode::OK, "{}", both.body);
    assert_eq!(both.body["profile"]["vidoc"], false);
    assert_eq!(both.body["profile"]["protect_production"], false);
    let record =
        i.h.store
            .sandbox(&SandboxId::parse("s1")?)
            .await?
            .ok_or("s1")?;
    let stored: Value = serde_json::from_str(&record.profile_json)?;
    assert_eq!(stored["vidoc"], false, "{stored}");
    assert_eq!(stored["protect_production"], false, "{stored}");
    Ok(())
}

/// PPR-002: a playground has no GitHub branch to protect, so it has no switch to turn.
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
        Some(json!({"protect_production": false})),
        &i.alice,
    )
    .await?;
    assert_eq!(r.status, StatusCode::UNPROCESSABLE_ENTITY, "{}", r.body);
    Ok(())
}
