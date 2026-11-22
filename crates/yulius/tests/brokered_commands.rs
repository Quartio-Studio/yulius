//! Brokered commands (SPEC-626, ADR-0459): a project's administrators define CLIs that need an
//! agent secret as a file; agents and the Superagent list them; task agents run them by name
//! outside their sandbox.

use crate::support;

use axum::http::{Method, StatusCode};
use serde_json::{Value, json};
use support::{
    Creds, Harness, TestResult, call, harness, perms, seed_github_user, seed_live_attempt,
    seed_sandbox, seed_task,
};
use yulius_app::ports::store::{AuditStore as _, TaskStore as _};
use yulius_app::ports::store_ext::CommandStore as _;
use yulius_app::ports::vault::VaultStore as _;
use yulius_domain::task::TaskStatus;

type Error = Box<dyn std::error::Error>;

/// A synthetic key as the Secrets page stores a pasted key file: its line breaks as literal
/// `\n` (an agent secret never spans lines).
const KEY: &str = concat!(
    "-----BEGIN PRIVATE KEY-----\\n",
    "c3ludGhldGljLWRlcGxveS1rZXktYm9keS1saW5lLTAx",
    "\\n-----END PRIVATE KEY-----"
);
const TOKEN_S1: &str = "brokered-task-token-s1-0123456789ab";
const TOKEN_S2: &str = "brokered-task-token-s2-0123456789ab";

struct World {
    h: Harness,
    /// The superadmin.
    admin: Creds,
    /// alice administers `app` (s1) and `api` (s2), with a CLI token.
    alice: Creds,
    /// bob writes `app`.
    bob: Creds,
}

/// Sandboxes s1 (`app`) and s2 (`api`), each with a running task of alice's and a live token,
/// and the agent secret `DEPLOY_KEY` in s1.
async fn world() -> Result<World, Error> {
    world_in(harness()?).await
}

/// [`world`] on the harness `h`.
async fn world_in(h: Harness) -> Result<World, Error> {
    let admin = support::setup_superadmin_creds(&h).await?;
    support::configure_github_app(&h).await?;
    let alice = seed_github_user(&h, "u-alice", "alice").await?;
    let bob = seed_github_user(&h, "u-bob", "bob").await?;
    seed_sandbox(&h, "s1", "app", "u-alice").await?;
    seed_sandbox(&h, "s2", "api", "u-alice").await?;
    h.github.set_perm("alice", "acme/app", perms("admin"));
    h.github.set_perm("alice", "acme/api", perms("admin"));
    h.github.set_perm("bob", "acme/app", perms("write"));
    for (task, sandbox, token) in [("t1", "s1", TOKEN_S1), ("t2", "s2", TOKEN_S2)] {
        seed_task(&h, task, sandbox, "u-alice", TaskStatus::Running, None).await?;
        seed_live_attempt(&h, task, token).await?;
    }
    let created = call(
        &h.router,
        Method::POST,
        "/api/sandboxes/s1/agent-secrets",
        Some(json!({"name": "DEPLOY_KEY", "description": "deploy tool key", "value": KEY})),
        &alice,
    )
    .await?;
    assert_eq!(created.status, StatusCode::CREATED, "{}", created.body);
    Ok(World {
        h,
        admin,
        alice,
        bob,
    })
}

fn deploy() -> Value {
    json!({
        "name": "deploy",
        "description": "Deploys the app",
        "binary": "/usr/local/bin/deploy",
        "secret": "DEPLOY_KEY",
        "fixed": ["--key-file", "{keyfile}"],
        "refuse": ["--key-file", "-k"],
        "subcommands": ["status", "push"],
        "approve": "send",
        "send_flags": ["--send"],
        "env": {"DEPLOY_HOME": "{home}"},
    })
}

async fn define(
    w: &World,
    sandbox: &str,
    body: Value,
    who: &Creds,
) -> Result<support::Resp, Error> {
    call(
        &w.h.router,
        Method::POST,
        &format!("/api/sandboxes/{sandbox}/commands"),
        Some(body),
        who,
    )
    .await
}

/// BRK-001, BRK-002, AC-1: an administrator defines, changes and removes a command; each change
/// is audited without a value; a writer is refused.
#[tokio::test]
async fn administrators_define_change_and_remove_commands() -> TestResult {
    let w = world().await?;
    let made = define(&w, "s1", deploy(), &w.alice).await?;
    assert_eq!(made.status, StatusCode::CREATED, "{}", made.body);
    let id = made.body["id"].as_str().ok_or("id")?.to_owned();
    assert_eq!(made.body["secret_ready"], true);
    assert_eq!(made.body["approval_minutes"], 10);
    assert_eq!(made.body["can_edit"], true);

    // A writer reads them but changes nothing.
    let listed = call(
        &w.h.router,
        Method::GET,
        "/api/sandboxes/s1/commands",
        None,
        &w.bob,
    )
    .await?;
    assert_eq!(listed.status, StatusCode::OK, "{}", listed.body);
    assert_eq!(listed.body["items"][0]["name"], "deploy");
    assert_eq!(listed.body["can_edit"], false);
    let refused = define(
        &w,
        "s1",
        json!({"name": "other", "binary": "/bin/true", "secret": "DEPLOY_KEY"}),
        &w.bob,
    )
    .await?;
    assert_eq!(refused.status, StatusCode::FORBIDDEN, "{}", refused.body);
    let refused = call(
        &w.h.router,
        Method::DELETE,
        &format!("/api/sandboxes/s1/commands/{id}"),
        None,
        &w.bob,
    )
    .await?;
    assert_eq!(refused.status, StatusCode::FORBIDDEN, "{}", refused.body);

    // A name is unique in its project.
    let twice = define(&w, "s1", deploy(), &w.alice).await?;
    assert_eq!(twice.status, StatusCode::CONFLICT, "{}", twice.body);

    let mut changed = deploy();
    changed["approve"] = json!("always");
    changed["approval_minutes"] = json!(3);
    let updated = call(
        &w.h.router,
        Method::PUT,
        &format!("/api/sandboxes/s1/commands/{id}"),
        Some(changed),
        &w.alice,
    )
    .await?;
    assert_eq!(updated.status, StatusCode::OK, "{}", updated.body);
    assert_eq!(updated.body["approve"], "always");
    assert_eq!(updated.body["approval_minutes"], 3);

    // Another project's id is not found there, though alice administers both (review round 1).
    for method in [Method::PUT, Method::DELETE] {
        let elsewhere = call(
            &w.h.router,
            method.clone(),
            &format!("/api/sandboxes/s2/commands/{id}"),
            (method == Method::PUT).then(deploy),
            &w.alice,
        )
        .await?;
        assert_eq!(
            elsewhere.status,
            StatusCode::NOT_FOUND,
            "{method}: {}",
            elsewhere.body
        );
    }
    let still = call(
        &w.h.router,
        Method::GET,
        "/api/sandboxes/s1/commands",
        None,
        &w.alice,
    )
    .await?;
    assert_eq!(still.body["items"][0]["id"], id.as_str(), "{}", still.body);
    let removed = call(
        &w.h.router,
        Method::DELETE,
        &format!("/api/sandboxes/s1/commands/{id}"),
        None,
        &w.alice,
    )
    .await?;
    assert_eq!(removed.status, StatusCode::NO_CONTENT, "{}", removed.body);
    let listed = call(
        &w.h.router,
        Method::GET,
        "/api/sandboxes/s1/commands",
        None,
        &w.alice,
    )
    .await?;
    assert_eq!(listed.body["items"], json!([]));

    for action in ["command.define", "command.update", "command.remove"] {
        let details = w.h.store.audit_details("s1", action, 5).await?;
        assert_eq!(details.len(), 1, "{action}: {details:?}");
        assert!(
            details[0].contains("deploy"),
            "{action} names it: {details:?}"
        );
    }
    Ok(())
}

/// BRK-001, BRK-003, AC-1: refused definitions; a production secret is refused.
#[tokio::test]
async fn refused_definitions_store_nothing() -> TestResult {
    let w = world().await?;
    for (patch, says) in [
        (json!({"binary": "deploy"}), "absolute"),
        (json!({"env": {"LD_PRELOAD": "/x"}}), "set by Yulius"),
        (json!({"fixed": ["{url:api}"]}), "not a placeholder"),
        (json!({"fixed": ["--token={secret}"]}), "only in env"),
        // Review round 2: what a person reads holds nothing they cannot see, and is bounded.
        (json!({"fixed": ["--to", "prod\u{202e}dev"]}), "cannot see"),
        (
            json!({"description": "x".repeat(1900), "fixed": vec!["y".repeat(4000); 5]}),
            "16 KiB",
        ),
        // Axum refuses the body itself (422, its own text).
        (json!({"key": "an inline key"}), ""),
    ] {
        let mut body = deploy();
        if let (Some(obj), Some(p)) = (body.as_object_mut(), patch.as_object()) {
            for (k, v) in p {
                obj.insert(k.clone(), v.clone());
            }
        }
        let r = define(&w, "s1", body, &w.alice).await?;
        assert!(r.status.is_client_error(), "{patch}: {}", r.status);
        assert!(
            says.is_empty() || r.body.to_string().contains(says),
            "{patch}: {}",
            r.body
        );
    }
    let id =
        w.h.store
            .vault_agent_in(&yulius_domain::ids::SandboxId::parse("s1")?)
            .await?
            .into_iter()
            .find(|r| r.name == "DEPLOY_KEY")
            .ok_or("secret")?
            .id;
    // Made a production secret (it has no hosts, as a production secret must not).
    let mut record = w.h.store.vault_get(&id).await?.ok_or("secret")?;
    assert!(w.h.store.vault_delete(&id).await?);
    record.cd_prod = true;
    w.h.store.vault_insert(&record).await?;
    let r = define(&w, "s1", deploy(), &w.alice).await?;
    assert_eq!(r.status, StatusCode::UNPROCESSABLE_ENTITY, "{}", r.body);
    assert!(
        r.body.to_string().contains("production secret"),
        "{}",
        r.body
    );
    let listed = call(
        &w.h.router,
        Method::GET,
        "/api/sandboxes/s1/commands",
        None,
        &w.alice,
    )
    .await?;
    assert_eq!(listed.body["items"], json!([]));
    Ok(())
}

/// BRK-004, AC-2: an agent lists its project's commands, never another project's, and is told
/// whether its task may run each.
#[tokio::test]
async fn agents_list_their_projects_commands_only() -> TestResult {
    let w = world().await?;
    let mut only_t9 = deploy();
    only_t9["name"] = json!("deploy-nine");
    only_t9["tasks"] = json!([9]);
    for body in [deploy(), only_t9] {
        let r = define(&w, "s1", body, &w.alice).await?;
        assert_eq!(r.status, StatusCode::CREATED, "{}", r.body);
    }
    let listed =
        support::tool_text(&support::mcp_tool(&w.h, TOKEN_S1, "commands_list", json!({})).await?);
    let listed: Value = match listed {
        Value::String(text) => serde_json::from_str(&text)?,
        other => other,
    };
    let commands = listed["commands"].as_array().ok_or("commands")?;
    assert_eq!(commands.len(), 2, "{listed}");
    assert_eq!(commands[0]["name"], "deploy");
    assert_eq!(commands[0]["you_may_run"], true);
    assert_eq!(commands[0]["fixed"], json!(["--key-file", "{keyfile}"]));
    assert_eq!(commands[0]["env"], json!(["DEPLOY_HOME"]));
    assert_eq!(commands[1]["you_may_run"], false);
    assert!(
        !listed.to_string().contains("c3ludGhldGlj"),
        "no value: {listed}"
    );

    let other =
        support::tool_text(&support::mcp_tool(&w.h, TOKEN_S2, "commands_list", json!({})).await?);
    let other: Value = match other {
        Value::String(text) => serde_json::from_str(&text)?,
        other => other,
    };
    assert_eq!(other["commands"], json!([]), "another project's: {other}");
    Ok(())
}

/// A secret named `name` in s1 with `auto_allow` as given (its people allow its uses one by one
/// when false); returns its id.
async fn secret_in_s1(w: &World, name: &str, auto_allow: bool) -> Result<String, Error> {
    let created = call(
        &w.h.router,
        Method::POST,
        "/api/sandboxes/s1/agent-secrets",
        Some(json!({
            "name": name,
            "value": format!("{name}-synthetic-value-0042"),
            "auto_allow": auto_allow,
        })),
        &w.alice,
    )
    .await?;
    assert_eq!(created.status, StatusCode::CREATED, "{}", created.body);
    Ok(created.body["id"].as_str().ok_or("id")?.to_owned())
}

/// `commands_list` as task `token`'s agent reads it.
async fn listed_by_agent(w: &World, token: &str) -> Result<Value, Error> {
    let listed =
        support::tool_text(&support::mcp_tool(&w.h, token, "commands_list", json!({})).await?);
    Ok(match listed {
        Value::String(text) => serde_json::from_str(&text)?,
        other => other,
    })
}

/// The command `name` of s1 as the API lists it.
async fn listed_command(w: &World, name: &str) -> Result<Value, Error> {
    let listed = call(
        &w.h.router,
        Method::GET,
        "/api/sandboxes/s1/commands",
        None,
        &w.alice,
    )
    .await?;
    Ok(listed.body["items"]
        .as_array()
        .and_then(|items| items.iter().find(|c| c["name"] == name))
        .cloned()
        .ok_or(format!("no {name}: {}", listed.body))?)
}

/// ADR-0317 through BRK-003 (review round 1): a definition decides which program gets a secret's
/// value, so a secret people allow use by use is bound only from a browser session unlocked with
/// a fresh two-factor code, never with an API token.
#[tokio::test]
async fn binding_a_guarded_secret_needs_a_fresh_two_factor_unlock() -> TestResult {
    let w = world().await?;
    secret_in_s1(&w, "GUARDED_KEY", false).await?;
    let mut body = deploy();
    body["secret"] = json!("GUARDED_KEY");
    let token = define(&w, "s1", body.clone(), &w.alice).await?;
    assert_eq!(token.status, StatusCode::FORBIDDEN, "{}", token.body);
    assert!(
        token.body.to_string().contains("two-factor"),
        "{}",
        token.body
    );
    let session = crate::secrets::browser_session(&w.h, "u-alice").await?;
    let locked = define(&w, "s1", body.clone(), &session).await?;
    assert_eq!(locked.status, StatusCode::FORBIDDEN, "{}", locked.body);
    assert_eq!(
        locked.body["error"]["code"], "reauth_required",
        "{}",
        locked.body
    );
    let listed = call(
        &w.h.router,
        Method::GET,
        "/api/sandboxes/s1/commands",
        None,
        &w.alice,
    )
    .await?;
    assert_eq!(
        listed.body["items"],
        json!([]),
        "nothing saved: {}",
        listed.body
    );
    // The same session, two-factor set up and unlocked.
    crate::secrets::enroll(&w.h, &session).await?;
    let made = define(&w, "s1", body, &session).await?;
    assert_eq!(made.status, StatusCode::CREATED, "{}", made.body);
    assert_eq!(made.body["secret_ready"], true);
    // Changing it again is the same decision.
    let id = made.body["id"].as_str().ok_or("id")?;
    let mut changed = deploy();
    changed["secret"] = json!("GUARDED_KEY");
    changed["binary"] = json!("/usr/bin/curl");
    let again = call(
        &w.h.router,
        Method::PUT,
        &format!("/api/sandboxes/s1/commands/{id}"),
        Some(changed),
        &w.alice,
    )
    .await?;
    assert_eq!(again.status, StatusCode::FORBIDDEN, "{}", again.body);
    Ok(())
}

/// BRK-003 (review round 1): only a project's own or a switched-on global secret is bound, a
/// global one only by an instance administrator (ADR-0186), never a personal one; the command
/// keeps the secret it was saved with, so another one of that name, or the same made a
/// production secret, reaches no run until it is saved again.
#[tokio::test]
async fn a_command_keeps_the_secret_it_was_saved_with() -> TestResult {
    let w = world().await?;
    // A personal secret of alice's is never a command's.
    let personal = call(
        &w.h.router,
        Method::POST,
        "/api/sandboxes/s1/agent-secrets",
        Some(json!({"name": "MY_KEY", "value": "my-personal-synthetic-0042", "scope": "personal"})),
        &w.alice,
    )
    .await?;
    assert_eq!(personal.status, StatusCode::CREATED, "{}", personal.body);
    let mut body = deploy();
    body["secret"] = json!("MY_KEY");
    let refused = define(&w, "s1", body, &w.alice).await?;
    assert_eq!(
        refused.status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "{}",
        refused.body
    );
    assert!(
        refused.body.to_string().contains("no agent secret MY_KEY"),
        "{}",
        refused.body
    );

    // A global secret: an instance administrator binds it, a repository administrator does not.
    let carol = support::admit_and_sign_in(&w.h, &w.admin, "carol", 31, true).await?;
    w.h.github.set_perm("carol", "acme/app", perms("admin"));
    let global = call(
        &w.h.router,
        Method::POST,
        "/api/sandboxes/s1/agent-secrets",
        Some(json!({"name": "SHARED_KEY", "value": "shared-global-synthetic-0042", "scope": "instance"})),
        &carol,
    )
    .await?;
    assert_eq!(global.status, StatusCode::CREATED, "{}", global.body);
    let mut body = deploy();
    body["name"] = json!("shared");
    body["secret"] = json!("SHARED_KEY");
    let refused = define(&w, "s1", body.clone(), &w.alice).await?;
    assert_eq!(refused.status, StatusCode::FORBIDDEN, "{}", refused.body);
    let made = define(&w, "s1", body, &carol).await?;
    assert_eq!(made.status, StatusCode::CREATED, "{}", made.body);
    assert_eq!(made.body["secret_ready"], true);

    // DEPLOY_KEY replaced by another secret of its name: the command no longer gets one.
    let made = define(&w, "s1", deploy(), &w.alice).await?;
    assert_eq!(made.status, StatusCode::CREATED, "{}", made.body);
    let id = made.body["id"].as_str().ok_or("id")?.to_owned();
    let sid = yulius_domain::ids::SandboxId::parse("s1")?;
    let old =
        w.h.store
            .vault_agent_in(&sid)
            .await?
            .into_iter()
            .find(|r| r.name == "DEPLOY_KEY")
            .ok_or("secret")?;
    assert!(w.h.store.vault_delete(&old.id).await?);
    let new = secret_in_s1(&w, "DEPLOY_KEY", true).await?;
    assert_eq!(listed_command(&w, "deploy").await?["secret_ready"], false);
    let tool = listed_by_agent(&w, TOKEN_S1).await?;
    let entry = tool["commands"]
        .as_array()
        .and_then(|c| c.iter().find(|c| c["name"] == "deploy"))
        .ok_or("deploy")?;
    assert_eq!(entry["you_may_run"], false, "{tool}");
    // Saved again, it gets the new one; made a production secret, none.
    let saved = call(
        &w.h.router,
        Method::PUT,
        &format!("/api/sandboxes/s1/commands/{id}"),
        Some(deploy()),
        &w.alice,
    )
    .await?;
    assert_eq!(saved.status, StatusCode::OK, "{}", saved.body);
    assert_eq!(saved.body["secret_ready"], true);
    let mut record = w.h.store.vault_get(&new).await?.ok_or("secret")?;
    assert!(w.h.store.vault_delete(&new).await?);
    record.cd_prod = true;
    w.h.store.vault_insert(&record).await?;
    assert_eq!(listed_command(&w, "deploy").await?["secret_ready"], false);
    Ok(())
}

/// BRK-002 (review round 1): an edit made from what an editor read is not saved over a change
/// made since, and a rename onto another command's name is refused; neither changes anything.
#[tokio::test]
async fn edits_never_overwrite_a_newer_definition() -> TestResult {
    let w = world().await?;
    let made = define(&w, "s1", deploy(), &w.alice).await?;
    assert_eq!(made.status, StatusCode::CREATED, "{}", made.body);
    let id = made.body["id"].as_str().ok_or("id")?.to_owned();
    let read = made.body["version"].as_i64().ok_or("version")?;
    let mut other = deploy();
    other["name"] = json!("other");
    let made_other = define(&w, "s1", other, &w.alice).await?;
    assert_eq!(
        made_other.status,
        StatusCode::CREATED,
        "{}",
        made_other.body
    );

    let mut tightened = deploy();
    tightened["refuse"] = json!(["--key-file", "-k", "--config"]);
    let path = format!("/api/sandboxes/s1/commands/{id}?version={read}");
    let first = call(&w.h.router, Method::PUT, &path, Some(tightened), &w.alice).await?;
    assert_eq!(first.status, StatusCode::OK, "{}", first.body);
    let stale = call(&w.h.router, Method::PUT, &path, Some(deploy()), &w.alice).await?;
    assert_eq!(stale.status, StatusCode::CONFLICT, "{}", stale.body);
    let mut renamed = deploy();
    renamed["name"] = json!("other");
    let taken = call(
        &w.h.router,
        Method::PUT,
        &format!("/api/sandboxes/s1/commands/{id}"),
        Some(renamed),
        &w.alice,
    )
    .await?;
    assert_eq!(taken.status, StatusCode::CONFLICT, "{}", taken.body);
    assert!(
        taken.body.to_string().contains("of that name"),
        "{}",
        taken.body
    );
    let mine = listed_command(&w, "deploy").await?;
    assert_eq!(mine["id"], id.as_str());
    assert_eq!(mine["refuse"], json!(["--key-file", "-k", "--config"]));
    // The store's own condition (review round 2): a write from a stale read changes nothing.
    let sid = yulius_domain::ids::SandboxId::parse("s1")?;
    let current = w.h.store.command(&sid, &id).await?.ok_or("command")?;
    let stale = yulius_app::ports::store_ext::CommandRecord {
        definition: serde_json::to_string(&deploy())?,
        updated_at: current.updated_at.plus_millis(5),
        ..current.clone()
    };
    assert!(
        !w.h.store
            .update_command(&stale, current.updated_at.plus_millis(-1))
            .await?
    );
    assert_eq!(
        w.h.store.command(&sid, &id).await?.ok_or("command")?,
        current
    );
    Ok(())
}

/// The stand-in CLI: it prints its home, its arguments and its key file and env token, checks
/// whether any file under the sandbox's directory holds the key's body while it runs, lists its
/// home, prints its environment to stderr, marks that it ran, then sleeps or floods its output
/// when its first own argument says so, and exits 7.
const CLI: &str = r#"#!/bin/sh
echo "home=$HOME"
echo "keyfile=$2"
cat "$2"
echo "token=$DEPLOY_TOKEN"
body=$(sed -n 2p "$2")
if grep -rqF "$body" "$SANDBOX_DIR"; then echo "sandbox=LEAK"; else echo "sandbox=clean"; fi
ls -A "$HOME"
env | sort >&2
touch "$MARK_DIR/ran"
case "$3" in
  slow) sleep 30 ;;
  loud) head -c 200000 /dev/zero | tr '\0' 'a' ;;
esac
exit 7
"#;

/// The key's body line, which must never show.
const BODY: &str = "c3ludGhldGljLWRlcGxveS1rZXktYm9keS1saW5lLTAx";

struct Runs {
    w: World,
    /// Where the stand-in CLI marks that it ran.
    marks: std::path::PathBuf,
    /// The stand-in CLI's folder.
    _bin: tempfile::TempDir,
}

/// [`world`] with the real process host, uncontained, the stand-in CLI and the command `deploy`
/// of s1 running it without approval, with `patch` applied to its definition.
async fn runs_world(patch: Value) -> Result<Runs, Error> {
    // A short data directory: run sockets live at <data>/runs/cmd-<id>/pty.sock.
    let data = tempfile::Builder::new().prefix("ybc").tempdir_in("/tmp")?;
    // The binary is the host's own, outside /tmp and the data directory (BRK-001).
    let bin = tempfile::Builder::new()
        .prefix("ybc")
        .tempdir_in(env!("CARGO_TARGET_TMPDIR"))?;
    let exe = std::path::PathBuf::from(env!("CARGO_BIN_EXE_yulius"));
    let _ = std::process::Command::new(&exe).arg("version").output();
    let h = support::harness_with(support::HarnessOptions {
        session_host: Some(std::sync::Arc::new(
            yulius::wiring::session::PtySessionHost::new(exe),
        )),
        dir: Some(data),
        configure: Some(Box::new(|config| {
            config.contained = false;
            config.command_wait_ms = 20_000;
        })),
        ..support::HarnessOptions::default()
    })?;
    let w = world_in(h).await?;
    let cli = bin.path().join("deploy");
    std::fs::write(&cli, CLI)?;
    std::fs::set_permissions(
        &cli,
        <std::fs::Permissions as std::os::unix::fs::PermissionsExt>::from_mode(0o755),
    )?;
    let marks = bin.path().join("marks");
    std::fs::create_dir_all(&marks)?;
    let sandbox_dir = w.h.app.config.data_dir.join("sandboxes/s1");
    std::fs::create_dir_all(&sandbox_dir)?;
    let mut body = deploy();
    body["binary"] = json!(cli.to_string_lossy());
    body["approve"] = json!("never");
    body["send_flags"] = json!([]);
    body["subcommands"] = json!([]);
    body["env"] = json!({
        "DEPLOY_TOKEN": "{secret}",
        "SANDBOX_DIR": sandbox_dir.to_string_lossy(),
        "MARK_DIR": marks.to_string_lossy(),
    });
    if let (Some(obj), Some(p)) = (body.as_object_mut(), patch.as_object()) {
        for (k, v) in p {
            obj.insert(k.clone(), v.clone());
        }
    }
    let r = define(&w, "s1", body, &w.alice).await?;
    assert_eq!(r.status, StatusCode::CREATED, "{}", r.body);
    Ok(Runs {
        w,
        marks,
        _bin: bin,
    })
}

/// Calls a tool as task `token`'s agent: `Ok(record)` or `Err(error text)`.
async fn agent_tool(
    h: &Harness,
    token: &str,
    name: &str,
    args: Value,
) -> Result<Result<Value, String>, Error> {
    let result = support::mcp_tool(h, token, name, args).await?;
    let text = result["content"][0]["text"]
        .as_str()
        .unwrap_or_default()
        .to_owned();
    if result["isError"] == true {
        return Ok(Err(text));
    }
    // The tool's record, which the call asks for (`yulius/structured`).
    Ok(Ok(result
        .get("structuredContent")
        .cloned()
        .unwrap_or(Value::String(text))))
}

/// `command_run` as t1's agent, which must not be refused.
async fn run_deploy(r: &Runs, args: Value) -> Result<Value, Error> {
    agent_tool(
        &r.w.h,
        TOKEN_S1,
        "command_run",
        json!({"name": "deploy", "args": args}),
    )
    .await?
    .map_err(|e| format!("refused: {e}").into())
}

/// BRK-008..BRK-012, AC-3: the run gets its key as a file outside its fresh, empty home and its
/// value in env only; nothing of the server's environment; the output, the stored run and the
/// audit show `[secret:DEPLOY_KEY]`; no file under the sandbox holds the key while it runs; its
/// files are gone after it.
#[tokio::test]
async fn the_key_never_reaches_the_sandbox_or_the_output() -> TestResult {
    let r = runs_world(json!({})).await?;
    let ran = run_deploy(&r, json!(["status"])).await?;
    assert_eq!(ran["state"], "done", "{ran}");
    assert_eq!(ran["exit_code"], 7, "{ran}");
    let stdout = ran["stdout"].as_str().ok_or("stdout")?;
    let stderr = ran["stderr"].as_str().ok_or("stderr")?;
    assert!(stdout.contains("[secret:DEPLOY_KEY]"), "{stdout}");
    // The key file and the env token each show it redacted.
    assert!(stdout.contains("token="), "{stdout}");
    assert_eq!(stdout.matches("[secret:DEPLOY_KEY]").count(), 2, "{stdout}");
    assert!(stdout.contains("sandbox=clean"), "{stdout}");
    for text in [stdout, stderr, &ran.to_string()] {
        assert!(!text.contains(BODY), "the key's body shows: {text}");
    }
    let home = stdout
        .lines()
        .find_map(|l| l.strip_prefix("home="))
        .ok_or("home")?;
    let listing: Vec<&str> = stdout
        .lines()
        .skip_while(|l| !l.starts_with("sandbox="))
        .skip(1)
        .collect();
    assert!(listing.is_empty(), "the home held {listing:?}");
    let keyfile = stdout
        .lines()
        .find_map(|l| l.strip_prefix("keyfile="))
        .ok_or("keyfile")?;
    assert!(!keyfile.starts_with(home), "{keyfile} is in {home}");
    let sandbox_dir = r.w.h.app.config.data_dir.join("sandboxes");
    assert!(!std::path::Path::new(keyfile).starts_with(&sandbox_dir));
    assert!(!std::path::Path::new(keyfile).exists(), "{keyfile} is left");
    assert!(!std::path::Path::new(home).exists(), "{home} is left");
    let names: Vec<&str> = stderr
        .lines()
        .filter_map(|l| l.split_once('=').map(|(n, _)| n))
        .collect();
    for name in &names {
        assert!(
            [
                "HOME",
                "PATH",
                "LANG",
                "TERM",
                "TMPDIR",
                "DEPLOY_TOKEN",
                "SANDBOX_DIR",
                "MARK_DIR",
                "PWD",
                "OLDPWD",
                "SHLVL",
                // Set by the host's /bin/sh itself when it is bash.
                "SHELL",
                "_"
            ]
            .contains(name),
            "{name} reached the run: {stderr}"
        );
    }
    let audits = r.w.h.store.audit_details("s1", "command.run", 5).await?;
    assert_eq!(audits.len(), 1, "{audits:?}");
    assert!(!audits[0].contains(BODY), "{audits:?}");
    assert!(audits[0].contains("deploy"), "{audits:?}");
    let id = ran["id"].as_str().ok_or("id")?;
    let stored = r.w.h.store.command_run(id).await?.ok_or("run")?;
    assert!(!format!("{stored:?}").contains(BODY));
    assert!(r.marks.join("ran").exists());
    Ok(())
}

/// BRK-006, AC-4: paths, refused flags and other subcommands are refused, audited, and run
/// nothing.
#[tokio::test]
async fn paths_refused_flags_and_other_subcommands_run_nothing() -> TestResult {
    let r = runs_world(json!({"subcommands": ["status", "push"]})).await?;
    let cases = [
        (json!(["status", "/etc/passwd"]), "file path"),
        (json!(["status", "~/.ssh/id_rsa"]), "file path"),
        (json!(["status", "~"]), "file path"),
        (json!(["status", "a/b"]), "file path"),
        (json!(["status", ".."]), "file path"),
        (json!(["status", "--key-file", "x"]), "set by Yulius"),
        (json!(["status", "--key-file=x"]), "set by Yulius"),
        (json!(["status", "--key-f=x"]), "set by Yulius"),
        (json!(["status", "-key-file", "x"]), "set by Yulius"),
        (json!(["status", "-kx"]), "set by Yulius"),
        (json!(["status", "-vk", "x"]), "set by Yulius"),
        (json!(["rm", "all"]), "allowed subcommand"),
        (json!(["--verbose", "status"]), "allowed subcommand"),
        (json!(["status", "a\u{202e}b"]), "invisible"),
    ];
    for (args, says) in &cases {
        let refused = agent_tool(
            &r.w.h,
            TOKEN_S1,
            "command_run",
            json!({"name": "deploy", "args": args}),
        )
        .await?;
        let reason = refused.err().ok_or(format!("{args} ran"))?;
        assert!(reason.contains(says), "{args}: {reason}");
    }
    assert!(!r.marks.join("ran").exists(), "a refused run ran");
    let audits =
        r.w.h
            .store
            .audit_details("s1", "command.refused", 50)
            .await?;
    assert_eq!(audits.len(), cases.len(), "{audits:?}");
    Ok(())
}

/// BRK-005, AC-5: another project's task, and a task outside `tasks`, are refused; a run is read
/// only by the task that started it.
#[tokio::test]
async fn other_projects_and_other_tasks_are_refused() -> TestResult {
    let r = runs_world(json!({})).await?;
    let refused = agent_tool(
        &r.w.h,
        TOKEN_S2,
        "command_run",
        json!({"name": "deploy", "args": ["status"]}),
    )
    .await?;
    assert!(
        refused.as_ref().is_err_and(|e| e.contains("no command")),
        "{refused:?}"
    );
    let mut only = deploy();
    only["name"] = json!("only-nine");
    only["tasks"] = json!([9]);
    let made = define(&r.w, "s1", only, &r.w.alice).await?;
    assert_eq!(made.status, StatusCode::CREATED, "{}", made.body);
    let refused = agent_tool(
        &r.w.h,
        TOKEN_S1,
        "command_run",
        json!({"name": "only-nine", "args": ["status"]}),
    )
    .await?;
    assert!(
        refused.as_ref().is_err_and(|e| e.contains("may not run")),
        "{refused:?}"
    );
    assert!(!r.marks.join("ran").exists(), "a refused run ran");
    let ran = run_deploy(&r, json!(["status"])).await?;
    let other = agent_tool(&r.w.h, TOKEN_S2, "command_result", json!({"id": ran["id"]})).await?;
    assert!(other.is_err(), "{other:?}");
    let mine = agent_tool(&r.w.h, TOKEN_S1, "command_result", json!({"id": ran["id"]})).await??;
    assert_eq!(mine["state"], "done", "{mine}");
    Ok(())
}

/// BRK-010, AC-7: a run past its time limit is stopped and says so; output past 96 KiB comes
/// back cut, saying how much was left out.
#[tokio::test]
async fn time_and_output_are_bounded() -> TestResult {
    let r = runs_world(json!({"timeout_seconds": 2})).await?;
    let started = std::time::Instant::now();
    let slow = run_deploy(&r, json!(["slow"])).await?;
    assert_eq!(slow["state"], "timed_out", "{slow}");
    assert!(
        slow["reason"]
            .as_str()
            .is_some_and(|r| r.contains("time limit")),
        "{slow}"
    );
    assert!(started.elapsed() < std::time::Duration::from_secs(20));
    let loud = run_deploy(&r, json!(["loud"])).await?;
    assert_eq!(loud["state"], "done", "{loud}");
    let stdout = loud["stdout"].as_str().ok_or("stdout")?;
    assert!(stdout.len() < 100 * 1024, "{}", stdout.len());
    assert!(
        stdout.contains("bytes more]"),
        "{}",
        &stdout[stdout.len() - 60..]
    );
    Ok(())
}

/// BRK-013 in this slice: a run that needs a person's approval is refused until approvals
/// arrive (SPEC-626 slice 3), and runs nothing.
#[tokio::test]
async fn a_run_that_needs_approval_runs_nothing_yet() -> TestResult {
    let r = runs_world(json!({"approve": "send", "send_flags": ["--send"]})).await?;
    for args in [
        json!({"args": ["push", "--send"]}),
        json!({"args": ["push"], "changes": true}),
    ] {
        let mut call = args.clone();
        call["name"] = json!("deploy");
        let refused = agent_tool(&r.w.h, TOKEN_S1, "command_run", call).await?;
        assert!(
            refused.as_ref().is_err_and(|e| e.contains("approval")),
            "{refused:?}"
        );
    }
    assert!(
        !r.marks.join("ran").exists(),
        "a run that needs approval ran"
    );
    let ran = run_deploy(&r, json!(["status"])).await?;
    assert_eq!(ran["state"], "done", "{ran}");
    Ok(())
}

/// BRK-009: a server that starts records a run it did not see end as lost and removes its files,
/// the key's among them.
#[tokio::test]
async fn a_restart_loses_a_running_run_and_removes_its_files() -> TestResult {
    use yulius_app::ports::store_ext::CommandRunRecord;
    let r = runs_world(json!({})).await?;
    let sid = yulius_domain::ids::SandboxId::parse("s1")?;
    let command =
        r.w.h
            .store
            .command_named(&sid, "deploy")
            .await?
            .ok_or("deploy")?;
    let now = r.w.h.app.now();
    let task =
        r.w.h
            .store
            .task(&yulius_domain::ids::TaskId::parse("t1")?)
            .await?
            .ok_or("t1")?;
    let run = CommandRunRecord {
        id: "run-left".into(),
        sandbox_id: sid.clone(),
        command_id: command.id.clone(),
        command: "deploy".into(),
        command_updated_at: command.updated_at,
        task_id: task.id.clone(),
        task_number: task.number,
        args: vec!["status".into()],
        line: "deploy status".into(),
        changes: false,
        state: "running".into(),
        reason: None,
        deadline: None,
        answered_by: None,
        exit_code: None,
        stdout: String::new(),
        stderr: String::new(),
        created_at: now,
        started_at: Some(now),
        ended_at: None,
    };
    r.w.h.store.insert_command_run(&run).await?;
    let files = yulius_contracts::sandbox::production_runs_dir(&r.w.h.app.config.data_dir, "s1")
        .join("cmd-run-left");
    std::fs::create_dir_all(files.join("work/key/x"))?;
    std::fs::write(files.join("work/key/x/key"), "left key")?;
    assert_eq!(r.w.h.app.reconcile_command_runs().await?, 1);
    let lost = r.w.h.store.command_run("run-left").await?.ok_or("run")?;
    assert_eq!(lost.state, "lost");
    assert!(!files.exists(), "{} is left", files.display());
    Ok(())
}

/// BRK-001 (review round 2): no binary in Yulius's data directory, where sandboxes keep what their
/// agents write.
#[tokio::test]
async fn no_binary_of_the_data_directory() -> TestResult {
    let data = tempfile::Builder::new()
        .prefix("ybd")
        .tempdir_in(env!("CARGO_TARGET_TMPDIR"))?;
    let w = world_in(support::harness_with(support::HarnessOptions {
        dir: Some(data),
        ..support::HarnessOptions::default()
    })?)
    .await?;
    let mut inside = deploy();
    let binary = w.h.app.config.data_dir.join("sandboxes/s1/repo/deploy");
    inside["binary"] = json!(binary.to_string_lossy());
    let r = define(&w, "s1", inside, &w.alice).await?;
    assert_eq!(r.status, StatusCode::UNPROCESSABLE_ENTITY, "{}", r.body);
    assert!(r.body.to_string().contains("data directory"), "{}", r.body);
    Ok(())
}
