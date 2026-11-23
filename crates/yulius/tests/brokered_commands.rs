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
use yulius_app::ports::store_ext::NeedsYouStore as _;
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
  big) head -c 20000000 /dev/zero > "$HOME/big"; sleep 30 ;;
  swap)
    echo "before the swap"
    rm -f "$HOME/../out" "$HOME/../err"
    ln -s "$OUTSIDE" "$HOME/../out"
    mkfifo "$HOME/../err"
    ;;
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
    runs_world_on(patch, None, 20_000).await
}

/// [`runs_world`]; with `host`, runs are contained (`config.contained`) and go to that host.
async fn runs_world_on(
    patch: Value,
    host: Option<std::sync::Arc<dyn yulius_app::ports::session::SessionHost>>,
    wait_ms: u64,
) -> Result<Runs, Error> {
    let contained = host.is_some();
    // A short data directory: run sockets live at <data>/runs/cmd-<id>/pty.sock.
    let data = tempfile::Builder::new().prefix("ybc").tempdir_in("/tmp")?;
    // The binary is the host's own, outside /tmp and the data directory (BRK-001).
    let bin = tempfile::Builder::new()
        .prefix("ybc")
        .tempdir_in(env!("CARGO_TARGET_TMPDIR"))?;
    let exe = std::path::PathBuf::from(env!("CARGO_BIN_EXE_yulius"));
    let _ = std::process::Command::new(&exe).arg("version").output();
    let h = support::harness_with(support::HarnessOptions {
        session_host: Some(host.unwrap_or_else(|| {
            std::sync::Arc::new(yulius::wiring::session::PtySessionHost::new(exe))
        })),
        dir: Some(data),
        configure: Some(Box::new(move |config| {
            config.contained = contained;
            config.command_wait_ms = wait_ms;
            // An approval minute is 300 ms here: the default 10 minutes are 3 s.
            config.command_minute_ms = 300;
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
        "OUTSIDE": bin.path().join("outside").to_string_lossy(),
    });
    std::fs::write(bin.path().join("outside"), "OUTSIDE-MARKER host file\n")?;
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
                "OUTSIDE",
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
        (json!(["status", "--key-file", "x"]), "which Yulius sets"),
        (json!(["status", "--key-file=x"]), "which Yulius sets"),
        (json!(["status", "--key-f=x"]), "which Yulius sets"),
        (json!(["status", "-key-file", "x"]), "which Yulius sets"),
        (json!(["status", "-kx"]), "which Yulius sets"),
        (json!(["status", "-vk", "x"]), "which Yulius sets"),
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

/// BRK-009: a server that starts records a run it did not see end as lost, ends its process and
/// removes its files, the key's among them (review round 1 of slice 2: with a live run).
#[tokio::test]
async fn a_restart_loses_a_running_run_and_removes_its_files() -> TestResult {
    let r = runs_world(json!({"timeout_seconds": 120})).await?;
    let started = std::time::Instant::now();
    let call = agent_tool(
        &r.w.h,
        TOKEN_S1,
        "command_run",
        json!({"name": "deploy", "args": ["slow"]}),
    );
    let restart = async {
        // Until the stand-in CLI runs.
        let ran = r.marks.join("ran");
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        while !ran.exists() {
            if std::time::Instant::now() > deadline {
                return Err::<_, Error>("the run never started".into());
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        let tid = yulius_domain::ids::TaskId::parse("t1")?;
        let run =
            r.w.h
                .store
                .latest_command_run_of_task(&tid)
                .await?
                .ok_or("run")?;
        let files =
            yulius_contracts::sandbox::production_runs_dir(&r.w.h.app.config.data_dir, "s1")
                .join(format!("cmd-{}", run.id));
        assert!(files.join("work/key").exists());
        // Its own executor sees its process end and may record that first.
        assert!(r.w.h.app.reconcile_command_runs().await? <= 1);
        let lost = r.w.h.store.command_run(&run.id).await?.ok_or("run")?;
        assert_eq!(lost.state, "lost");
        assert!(!files.exists(), "{} is left", files.display());
        Ok(())
    };
    let (answered, restarted) = tokio::join!(call, restart);
    restarted?;
    // Its process ended: the call answers long before the CLI's 30 s sleep.
    let answered = answered?.map_err(|e| format!("refused: {e}"))?;
    assert_eq!(answered["state"], "lost", "{answered}");
    assert!(started.elapsed() < std::time::Duration::from_secs(25));
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

/// Review round 1 of slice 2: what comes back is read through the handles Yulius made before the
/// run. A run that puts a link to a host file at its output's path, or a FIFO, gets neither read:
/// the agent sees what the CLI wrote and the run ends.
#[tokio::test]
async fn output_is_read_from_the_files_yulius_made_not_from_their_paths() -> TestResult {
    let r = runs_world(json!({"timeout_seconds": 20})).await?;
    let ran = run_deploy(&r, json!(["swap"])).await?;
    assert_eq!(ran["state"], "done", "{ran}");
    let stdout = ran["stdout"].as_str().ok_or("stdout")?;
    assert!(stdout.contains("before the swap"), "{stdout}");
    assert!(!ran.to_string().contains("OUTSIDE-MARKER"), "{ran}");
    Ok(())
}

/// BRK-010 (review round 1 of slice 2): a run whose files pass 16 MiB is stopped and says so.
#[tokio::test]
async fn a_run_whose_files_pass_the_bound_is_stopped() -> TestResult {
    let r = runs_world(json!({"timeout_seconds": 60})).await?;
    let started = std::time::Instant::now();
    let big = run_deploy(&r, json!(["big"])).await?;
    assert_eq!(big["state"], "output_limit", "{big}");
    assert!(
        big["reason"].as_str().is_some_and(|r| r.contains("16 MiB")),
        "{big}"
    );
    assert!(started.elapsed() < std::time::Duration::from_secs(30));
    Ok(())
}

/// BRK-012 (review round 1 of slice 2): a refused argument holding a value is audited without it,
/// and the agent is told which argument by its place.
#[tokio::test]
async fn a_refusal_audits_no_value() -> TestResult {
    let r = runs_world(json!({"refuse": ["--key-file", "-k", "--token"]})).await?;
    let value = "tok-synthetic-abcdef0123456789";
    let made = call(
        &r.w.h.router,
        Method::POST,
        "/api/sandboxes/s1/agent-secrets",
        Some(json!({"name": "API_TOKEN", "value": value})),
        &r.w.alice,
    )
    .await?;
    assert_eq!(made.status, StatusCode::CREATED, "{}", made.body);
    for args in [
        json!(["status", format!("--token={value}")]),
        json!(["status", format!("{value}/x")]),
    ] {
        let refused = agent_tool(
            &r.w.h,
            TOKEN_S1,
            "command_run",
            json!({"name": "deploy", "args": args}),
        )
        .await?;
        let reason = refused.err().ok_or("ran")?;
        assert!(reason.contains("argument 2"), "{reason}");
        assert!(!reason.contains(value), "{reason}");
    }
    let audits =
        r.w.h
            .store
            .audit_details("s1", "command.refused", 10)
            .await?;
    assert_eq!(audits.len(), 2, "{audits:?}");
    for audit in &audits {
        assert!(!audit.contains("tok-synthetic"), "{audit}");
    }
    Ok(())
}

/// RUN-003 (review round 1 of slice 2): at most four runs of a project's commands at once; the
/// fifth is refused, audited, and runs nothing.
#[tokio::test]
async fn at_most_four_runs_of_a_project_at_once() -> TestResult {
    let r = runs_world(json!({"timeout_seconds": 6})).await?;
    let slow = || {
        agent_tool(
            &r.w.h,
            TOKEN_S1,
            "command_run",
            json!({"name": "deploy", "args": ["slow"]}),
        )
    };
    let fifth = async {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            use yulius_app::ports::store_ext::CommandStore as _;
            let running = r.w.h.store.command_runs_in_state("running").await?.len();
            if running == 4 {
                break;
            }
            if std::time::Instant::now() > deadline {
                return Err::<_, Error>(format!("{running} runs started").into());
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        let refused = agent_tool(
            &r.w.h,
            TOKEN_S1,
            "command_run",
            json!({"name": "deploy", "args": ["status"]}),
        )
        .await?;
        Ok(refused)
    };
    let (a, b, c, d, refused) = tokio::join!(slow(), slow(), slow(), slow(), fifth);
    for ended in [a, b, c, d] {
        assert_eq!(ended??["state"], "timed_out");
    }
    let refused = refused?;
    assert!(
        refused.as_ref().is_err_and(|e| e.contains("4 runs")),
        "{refused:?}"
    );
    let audits =
        r.w.h
            .store
            .audit_details("s1", "command.refused", 5)
            .await?;
    assert_eq!(audits.len(), 1, "{audits:?}");
    Ok(())
}

/// BRK-007 (review round 1 of slice 2): `yulius run` against Yulius's own `/mcp` prints the run's
/// redacted streams and exits with its code; `command_result` without an id reads the task's
/// newest run.
#[tokio::test]
async fn yulius_run_prints_the_run_and_exits_with_its_code() -> TestResult {
    let r = runs_world(json!({})).await?;
    let served = support::serve(r.w.h.internal.clone()).await?;
    let url = format!("http://{}", served.addr);
    let (code, out, err) = tokio::task::spawn_blocking(move || {
        let env = move |name: &str| match name {
            "YULIUS_URL" => Some(url.clone()),
            "YULIUS_TASK_TOKEN" => Some(TOKEN_S1.to_owned()),
            _ => None,
        };
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let code = yulius::run_cli::run(
            &mut out,
            &mut err,
            "deploy",
            &["status".to_owned()],
            yulius::tool_cli::Pace::default(),
            &env,
        );
        (code, out, err)
    })
    .await?;
    let (out, err) = (String::from_utf8(out)?, String::from_utf8(err)?);
    assert_eq!(code, 7, "{out}\n{err}");
    assert!(out.contains("sandbox=clean"), "{out}");
    assert!(out.contains("[secret:DEPLOY_KEY]"), "{out}");
    assert!(!out.contains(BODY) && !err.contains(BODY));
    assert!(err.contains("DEPLOY_TOKEN=[secret:DEPLOY_KEY]"), "{err}");
    let newest = agent_tool(&r.w.h, TOKEN_S1, "command_result", json!({})).await??;
    assert_eq!(newest["state"], "done", "{newest}");
    assert_eq!(newest["exit_code"], 7);
    Ok(())
}

/// BRK-008 (review round 1 of slice 2): on a contained server the run joins the project's
/// production holder (never the agents' sandbox) on the host's read-only system, with only its own
/// folder and without Docker.
#[tokio::test]
async fn a_contained_run_joins_the_production_holder_without_docker() -> TestResult {
    let exe = std::path::PathBuf::from(env!("CARGO_BIN_EXE_yulius"));
    let recording = std::sync::Arc::new(crate::cd::Recording {
        inner: yulius::wiring::session::PtySessionHost::new(exe),
        seen: std::sync::Mutex::default(),
    });
    let r = runs_world_on(json!({}), Some(recording.clone()), 20_000).await?;
    let holders = std::sync::Arc::new(crate::cd::Holders::default());
    assert!(r.w.h.app.set_isolation(holders.clone()));
    let ran = run_deploy(&r, json!(["status"])).await?;
    assert_eq!(ran["state"], "done", "{ran}");
    assert_eq!(
        holders.calls(),
        vec!["prepare_production", "release_production"]
    );
    let seen = recording.seen.lock().map_err(|_| "poisoned")?.clone();
    assert_eq!(seen.len(), 1);
    let yulius_app::ports::session::Containment::Bubblewrap {
        sandbox_root,
        binds,
        join: Some(join),
        ..
    } = &seen[0]
    else {
        return Err(format!("not contained: {:?}", seen[0]).into());
    };
    let id = ran["id"].as_str().ok_or("id")?;
    let work = yulius_contracts::sandbox::production_runs_dir(&r.w.h.app.config.data_dir, "s1")
        .join(format!("cmd-{id}"))
        .join("work");
    assert_eq!(sandbox_root, &work);
    assert!(binds.is_empty());
    assert!(join.host_system.is_some(), "the host's read-only system");
    assert_eq!(join.docker_dir, None);
    assert!(join.extra_binds.is_empty());
    assert_eq!(join.hostname, "s1-prod");
    Ok(())
}

/// BRK-003 (review round 2 of slice 2): a run reads its command again before it runs, and runs
/// nothing when the command was changed or removed since it was asked.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_command_changed_or_removed_before_its_run_runs_nothing() -> TestResult {
    let r = runs_world(json!({})).await?;
    let sid = yulius_domain::ids::SandboxId::parse("s1")?;
    let path = format!(
        "/api/sandboxes/s1/commands/{}",
        r.w.h
            .store
            .command_named(&sid, "deploy")
            .await?
            .ok_or("deploy")?
            .id
    );
    for (change, says) in [("change", "changed since"), ("remove", "was removed")] {
        let (reached_tx, reached_rx) = std::sync::mpsc::channel::<()>();
        let (go_tx, go_rx) = std::sync::mpsc::channel::<()>();
        yulius_app::race::on(
            yulius_app::race::Point::CommandRunStored,
            &r.w.h.app.config.data_dir,
            move || {
                let _ = reached_tx.send(());
                let _ = go_rx.recv_timeout(std::time::Duration::from_secs(60));
            },
        );
        let run = agent_tool(
            &r.w.h,
            TOKEN_S1,
            "command_run",
            json!({"name": "deploy", "args": ["status"]}),
        );
        let meanwhile = async {
            tokio::task::spawn_blocking(move || {
                reached_rx.recv_timeout(std::time::Duration::from_secs(60))
            })
            .await??;
            // The run is stored and has not read its command yet.
            let current = listed_command(&r.w, "deploy").await?;
            let r2 = if change == "change" {
                let mut body = current.clone();
                for key in [
                    "id",
                    "secret_ready",
                    "version",
                    "created_by",
                    "created_at",
                    "updated_by",
                    "updated_at",
                    "can_edit",
                ] {
                    if let Some(obj) = body.as_object_mut() {
                        obj.remove(key);
                    }
                }
                body["binary"] = json!("/usr/bin/true");
                call(&r.w.h.router, Method::PUT, &path, Some(body), &r.w.alice).await?
            } else {
                call(&r.w.h.router, Method::DELETE, &path, None, &r.w.alice).await?
            };
            assert!(r2.status.is_success(), "{}", r2.body);
            let _ = go_tx.send(());
            Ok::<_, Error>(())
        };
        let (ran, changed) = tokio::join!(run, meanwhile);
        changed?;
        let ran = ran?.map_err(|e| format!("refused: {e}"))?;
        assert_eq!(ran["state"], "failed", "{ran}");
        assert!(
            ran["reason"].as_str().is_some_and(|r| r.contains(says)),
            "{ran}"
        );
        assert!(!r.marks.join("ran").exists(), "the run ran");
    }
    Ok(())
}

/// A browser session of alice, who manages s1's secrets.
async fn alice_in_a_browser(r: &Runs) -> Result<Creds, Error> {
    crate::secrets::browser_session(&r.w.h, "u-alice").await
}

/// Waits until task t1's run waits for a person; returns it as the runs list shows it.
async fn waiting_run(r: &Runs, who: &Creds) -> Result<Value, Error> {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        let listed = call(
            &r.w.h.router,
            Method::GET,
            "/api/sandboxes/s1/command-runs",
            None,
            who,
        )
        .await?;
        if let Some(run) = listed.body["items"]
            .as_array()
            .and_then(|items| items.iter().find(|i| i["state"] == "waiting"))
        {
            return Ok(run.clone());
        }
        if std::time::Instant::now() > deadline {
            return Err(format!("no run waits: {}", listed.body).into());
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
}

async fn answer(r: &Runs, id: &str, approve: bool, who: &Creds) -> Result<support::Resp, Error> {
    call(
        &r.w.h.router,
        Method::POST,
        &format!("/api/sandboxes/s1/command-runs/{id}"),
        Some(json!({"approve": approve})),
        who,
    )
    .await
}

/// BRK-013..BRK-015, AC-6: an `always` run waits; its task is in Needs you with the full command
/// line and how to answer; an API token cannot answer it; a person who manages the project's
/// secrets approves it in a browser and it runs once; every step is audited.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_always_run_waits_for_a_person_and_runs_once_approved() -> TestResult {
    let r = runs_world(json!({"approve": "always"})).await?;
    let browser = alice_in_a_browser(&r).await?;
    let asked = run_deploy(&r, json!(["status"]));
    let person = async {
        let run = waiting_run(&r, &browser).await?;
        assert_eq!(run["can_answer"], true, "{run}");
        assert!(
            run["line"]
                .as_str()
                .is_some_and(|l| l.ends_with("--key-file {keyfile} status")),
            "{run}"
        );
        assert!(!r.marks.join("ran").exists(), "it ran before its approval");
        let tid = yulius_domain::ids::TaskId::parse("t1")?;
        let request =
            r.w.h
                .store
                .task_needs_you(&tid)
                .await?
                .ok_or("no Needs you")?;
        assert!(request.decision);
        assert!(
            request.action.contains("Approve or reject"),
            "{}",
            request.action
        );
        let problem = request.problem.clone().unwrap_or_default();
        assert!(problem.contains("--key-file {keyfile} status"), "{problem}");
        assert!(problem.contains("no answer is a no"), "{problem}");
        let id = run["id"].as_str().ok_or("id")?.to_owned();
        // Not with an API token (alice's own).
        let token = answer(&r, &id, true, &r.w.alice).await?;
        assert_eq!(token.status, StatusCode::FORBIDDEN, "{}", token.body);
        let approved = answer(&r, &id, true, &browser).await?;
        assert_eq!(approved.status, StatusCode::OK, "{}", approved.body);
        assert_eq!(approved.body["answered_by"], "alice");
        // Its request ended with its answer.
        assert!(r.w.h.store.task_needs_you(&tid).await?.is_none());
        // A second answer acts no more.
        let again = answer(&r, &id, false, &browser).await?;
        assert_eq!(again.status, StatusCode::CONFLICT, "{}", again.body);
        Ok::<_, Error>(())
    };
    let (ran, answered) = tokio::join!(asked, person);
    answered?;
    let ran = ran?;
    assert_eq!(ran["state"], "done", "{ran}");
    assert_eq!(ran["exit_code"], 7, "{ran}");
    for action in ["command.ask", "command.approve", "command.run"] {
        let details = r.w.h.store.audit_details("s1", action, 5).await?;
        assert_eq!(details.len(), 1, "{action}: {details:?}");
        assert!(!details[0].contains(BODY), "{action}");
    }
    Ok(())
}

/// BRK-015, AC-6: a rejected run runs nothing, and a later approval of it is refused.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_rejected_run_runs_nothing() -> TestResult {
    let r = runs_world(json!({"approve": "always"})).await?;
    let browser = alice_in_a_browser(&r).await?;
    let asked = run_deploy(&r, json!(["status"]));
    let person = async {
        let run = waiting_run(&r, &browser).await?;
        let id = run["id"].as_str().ok_or("id")?.to_owned();
        let rejected = answer(&r, &id, false, &browser).await?;
        assert_eq!(rejected.status, StatusCode::OK, "{}", rejected.body);
        assert_eq!(rejected.body["state"], "rejected");
        let late = answer(&r, &id, true, &browser).await?;
        assert_eq!(late.status, StatusCode::CONFLICT, "{}", late.body);
        Ok::<_, Error>(())
    };
    let (ran, answered) = tokio::join!(asked, person);
    answered?;
    assert_eq!(ran?["state"], "rejected");
    assert!(!r.marks.join("ran").exists(), "a rejected run ran");
    assert_eq!(
        r.w.h
            .store
            .audit_details("s1", "command.reject", 5)
            .await?
            .len(),
        1
    );
    Ok(())
}

/// BRK-016, AC-6: no answer by the deadline is a no: the run ends `expired`, runs nothing, its
/// request ends, and an answer after it is refused.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn no_answer_in_time_is_a_no() -> TestResult {
    let r = runs_world(json!({"approve": "always", "approval_minutes": 1})).await?;
    let browser = alice_in_a_browser(&r).await?;
    let started = std::time::Instant::now();
    let asked = run_deploy(&r, json!(["status"]));
    let time_passes = async {
        waiting_run(&r, &browser).await?;
        // The harness's clock stands still: a minute (300 ms here) passes on it.
        r.w.h
            .clock
            .0
            .fetch_add(301, std::sync::atomic::Ordering::SeqCst);
        Ok::<_, Error>(())
    };
    let (ran, passed) = tokio::join!(asked, time_passes);
    passed?;
    let ran = ran?;
    assert_eq!(ran["state"], "expired", "{ran}");
    assert!(started.elapsed() < std::time::Duration::from_secs(15));
    assert!(!r.marks.join("ran").exists(), "an expired run ran");
    let tid = yulius_domain::ids::TaskId::parse("t1")?;
    assert!(r.w.h.store.task_needs_you(&tid).await?.is_none());
    let late = answer(&r, ran["id"].as_str().ok_or("id")?, true, &browser).await?;
    assert_eq!(late.status, StatusCode::CONFLICT, "{}", late.body);
    assert!(!r.marks.join("ran").exists());
    assert_eq!(
        r.w.h
            .store
            .audit_details("s1", "command.expired", 5)
            .await?
            .len(),
        1
    );
    Ok(())
}

/// BRK-016: the answer's own condition holds the deadline, also when nothing expired the run
/// yet (a restart lost its timer): an approval past it is refused and runs nothing.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_approval_past_the_deadline_is_refused_by_its_write() -> TestResult {
    use yulius_app::ports::store_ext::CommandStore as _;
    let r = runs_world(json!({"approve": "always", "approval_minutes": 1})).await?;
    let sid = yulius_domain::ids::SandboxId::parse("s1")?;
    let command =
        r.w.h
            .store
            .command_named(&sid, "deploy")
            .await?
            .ok_or("deploy")?;
    let task =
        r.w.h
            .store
            .task(&yulius_domain::ids::TaskId::parse("t1")?)
            .await?
            .ok_or("t1")?;
    let now = r.w.h.app.now();
    let run = yulius_app::ports::store_ext::CommandRunRecord {
        id: "run-old".into(),
        sandbox_id: sid.clone(),
        command_id: command.id.clone(),
        command: "deploy".into(),
        command_updated_at: command.updated_at,
        task_id: task.id.clone(),
        task_number: task.number,
        args: vec!["status".into()],
        line: "deploy status".into(),
        changes: false,
        state: "waiting".into(),
        reason: None,
        deadline: Some(now.plus_millis(-1)),
        answered_by: None,
        exit_code: None,
        stdout: String::new(),
        stderr: String::new(),
        created_at: now.plus_millis(-60_000),
        started_at: None,
        ended_at: None,
    };
    assert_eq!(
        r.w.h.store.insert_command_run(&run, 4).await?,
        yulius_app::ports::store_ext::RunInsert::Stored
    );
    assert!(
        !r.w.h
            .store
            .answer_command_run("run-old", true, "alice", now, 4)
            .await?
    );
    let browser = alice_in_a_browser(&r).await?;
    let late = answer(&r, "run-old", true, &browser).await?;
    assert_eq!(late.status, StatusCode::CONFLICT, "{}", late.body);
    assert!(
        late.body.to_string().contains("time ran out"),
        "{}",
        late.body
    );
    assert_eq!(
        r.w.h
            .store
            .command_run("run-old")
            .await?
            .ok_or("run")?
            .state,
        "expired"
    );
    assert!(!r.marks.join("ran").exists());
    Ok(())
}

/// BRK-013: `send` asks only for a run the agent says changes something or that sets a send
/// flag; one run of a task waits at a time.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn send_asks_only_for_a_run_that_changes_something() -> TestResult {
    let r = runs_world(json!({"approve": "send", "send_flags": ["--send"]})).await?;
    let plain = run_deploy(&r, json!(["status"])).await?;
    assert_eq!(plain["state"], "done", "{plain}");
    let browser = alice_in_a_browser(&r).await?;
    for call_args in [
        json!({"name": "deploy", "args": ["push", "--send"]}),
        json!({"name": "deploy", "args": ["push"], "changes": true}),
    ] {
        let asked = agent_tool(&r.w.h, TOKEN_S1, "command_run", call_args.clone());
        let person = async {
            let run = waiting_run(&r, &browser).await?;
            // A second one of the same task is refused while it waits.
            let second = agent_tool(&r.w.h, TOKEN_S1, "command_run", call_args.clone()).await?;
            assert!(
                second
                    .as_ref()
                    .is_err_and(|e| e.contains("waits for a person's approval already")),
                "{second:?}"
            );
            let id = run["id"].as_str().ok_or("id")?.to_owned();
            let rejected = answer(&r, &id, false, &browser).await?;
            assert_eq!(rejected.status, StatusCode::OK, "{}", rejected.body);
            Ok::<_, Error>(())
        };
        let (ran, answered) = tokio::join!(asked, person);
        answered?;
        assert_eq!(ran??["state"], "rejected");
    }
    Ok(())
}

/// A run of t1 as the store holds it, for tests that set a state the server reaches only over
/// time (a deadline passed, runs that run).
async fn stored_run(
    r: &Runs,
    id: &str,
    state: &str,
    deadline_ms: Option<i64>,
) -> Result<yulius_app::ports::store_ext::CommandRunRecord, Error> {
    let sid = yulius_domain::ids::SandboxId::parse("s1")?;
    let command =
        r.w.h
            .store
            .command_named(&sid, "deploy")
            .await?
            .ok_or("deploy")?;
    let task =
        r.w.h
            .store
            .task(&yulius_domain::ids::TaskId::parse("t1")?)
            .await?
            .ok_or("t1")?;
    let now = r.w.h.app.now();
    Ok(yulius_app::ports::store_ext::CommandRunRecord {
        id: id.into(),
        sandbox_id: sid,
        command_id: command.id.clone(),
        command: "deploy".into(),
        command_updated_at: command.updated_at,
        task_id: task.id.clone(),
        task_number: task.number,
        args: vec!["status".into()],
        line: "deploy status".into(),
        changes: false,
        state: state.into(),
        reason: None,
        deadline: deadline_ms.map(|ms| now.plus_millis(ms)),
        answered_by: None,
        exit_code: None,
        stdout: String::new(),
        stderr: String::new(),
        created_at: now,
        started_at: (state == "running").then_some(now),
        ended_at: None,
    })
}

/// BRK-014 (review round 1 of slice 3): one run of a task waits at a time, held by the store's
/// own transaction, also for calls made together.
#[tokio::test]
async fn one_run_of_a_task_waits_at_a_time_in_the_store() -> TestResult {
    use yulius_app::ports::store_ext::RunInsert;
    let r = runs_world(json!({"approve": "always"})).await?;
    let first = stored_run(&r, "run-a", "waiting", Some(60_000)).await?;
    let second = stored_run(&r, "run-b", "waiting", Some(60_000)).await?;
    assert_eq!(
        r.w.h.store.insert_command_run(&first, 4).await?,
        RunInsert::Stored
    );
    assert_eq!(
        r.w.h.store.insert_command_run(&second, 4).await?,
        RunInsert::TaskWaits
    );
    assert!(r.w.h.store.command_run("run-b").await?.is_none());
    Ok(())
}

/// BRK-015 (review round 1 of slice 3): an answer needs write access to the project (it manages
/// its secrets) from a browser; another project's run id is not found there; the list says who
/// may answer; a non-member sees nothing.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn who_answers_a_run_and_where() -> TestResult {
    let r = runs_world(json!({"approve": "always"})).await?;
    let browser = alice_in_a_browser(&r).await?;
    seed_github_user(&r.w.h, "u-carol", "carol").await?;
    r.w.h.github.set_perm("carol", "acme/app", perms("read"));
    let carol_browser = crate::secrets::browser_session(&r.w.h, "u-carol").await?;
    let dave = seed_github_user(&r.w.h, "u-dave", "dave").await?;
    let asked = run_deploy(&r, json!(["status"]));
    let people = async {
        let run = waiting_run(&r, &browser).await?;
        let id = run["id"].as_str().ok_or("id")?.to_owned();
        let read = waiting_run(&r, &carol_browser).await?;
        assert_eq!(read["can_answer"], false, "a reader: {read}");
        let token = waiting_run(&r, &r.w.alice).await?;
        assert_eq!(token["can_answer"], false, "an API token: {token}");
        let none = call(
            &r.w.h.router,
            Method::GET,
            "/api/sandboxes/s1/command-runs",
            None,
            &dave,
        )
        .await?;
        assert!(none.status.is_client_error(), "a non-member: {}", none.body);
        let refused = answer(&r, &id, true, &carol_browser).await?;
        assert_eq!(refused.status, StatusCode::FORBIDDEN, "{}", refused.body);
        let elsewhere = call(
            &r.w.h.router,
            Method::POST,
            &format!("/api/sandboxes/s2/command-runs/{id}"),
            Some(json!({"approve": true})),
            &browser,
        )
        .await?;
        assert_eq!(
            elsewhere.status,
            StatusCode::NOT_FOUND,
            "{}",
            elsewhere.body
        );
        assert_eq!(
            waiting_run(&r, &browser).await?["id"],
            id.as_str(),
            "it still waits"
        );
        let rejected = answer(&r, &id, false, &browser).await?;
        assert_eq!(rejected.status, StatusCode::OK, "{}", rejected.body);
        Ok::<_, Error>(())
    };
    let (ran, answered) = tokio::join!(asked, people);
    answered?;
    assert_eq!(ran?["state"], "rejected");
    assert!(!r.marks.join("ran").exists());
    Ok(())
}

/// BRK-014 (review round 1 of slice 3): the run's request replaces the agent's open one; the
/// agent's request opened after it stays when the run is answered; the agent's call answers
/// `waiting` with the deadline after its wait, and `command_result` reads the end.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_agent_waits_and_its_own_request_survives_the_runs_answer() -> TestResult {
    let r = runs_world_on(json!({"approve": "always"}), None, 300).await?;
    let browser = alice_in_a_browser(&r).await?;
    let tid = yulius_domain::ids::TaskId::parse("t1")?;
    let asked = agent_tool(
        &r.w.h,
        TOKEN_S1,
        "needs_you",
        json!({"action": "Pick a colour"}),
    )
    .await?;
    assert!(asked.is_ok(), "{asked:?}");
    let waiting = run_deploy(&r, json!(["status"])).await?;
    assert_eq!(waiting["state"], "waiting", "{waiting}");
    assert!(waiting["approval_until"].as_str().is_some(), "{waiting}");
    let id = waiting["id"].as_str().ok_or("id")?.to_owned();
    let request = r.w.h.store.task_needs_you(&tid).await?.ok_or("request")?;
    assert!(
        request.action.contains("Approve or reject"),
        "replaced: {}",
        request.action
    );
    // The agent asks a person something again while its run waits.
    let again = agent_tool(
        &r.w.h,
        TOKEN_S1,
        "needs_you",
        json!({"action": "Pick a font"}),
    )
    .await?;
    assert!(again.is_ok(), "{again:?}");
    let approved = answer(&r, &id, true, &browser).await?;
    assert_eq!(approved.status, StatusCode::OK, "{}", approved.body);
    let kept =
        r.w.h
            .store
            .task_needs_you(&tid)
            .await?
            .ok_or("the agent's request was ended")?;
    assert_eq!(kept.action, "Pick a font");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    let ended = loop {
        let read = agent_tool(&r.w.h, TOKEN_S1, "command_result", json!({"id": id})).await??;
        if read["state"] != "running" && read["state"] != "waiting" {
            break read;
        }
        assert!(std::time::Instant::now() < deadline, "{read}");
    };
    assert_eq!(ended["state"], "done", "{ended}");
    let audits = r.w.h.store.audit_details("s1", "command.run", 5).await?;
    assert!(audits[0].contains("alice"), "approved_by: {audits:?}");
    Ok(())
}

/// BRK-015 (review round 1 of slice 3): an approval while four runs of the project run is
/// refused and the run still waits; once one ended it is approved.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_approval_waits_for_a_free_run() -> TestResult {
    use yulius_app::ports::store_ext::{CommandRunEnd, RunInsert};
    let r = runs_world(json!({"approve": "always"})).await?;
    let browser = alice_in_a_browser(&r).await?;
    for n in 0..4 {
        let busy = stored_run(&r, &format!("run-busy-{n}"), "running", None).await?;
        assert_eq!(
            r.w.h.store.insert_command_run(&busy, 4).await?,
            RunInsert::Stored
        );
    }
    let asked = run_deploy(&r, json!(["status"]));
    let person = async {
        let run = waiting_run(&r, &browser).await?;
        let id = run["id"].as_str().ok_or("id")?.to_owned();
        let refused = answer(&r, &id, true, &browser).await?;
        assert_eq!(refused.status, StatusCode::CONFLICT, "{}", refused.body);
        assert!(
            refused.body.to_string().contains("4 runs"),
            "{}",
            refused.body
        );
        assert_eq!(waiting_run(&r, &browser).await?["id"], id.as_str());
        let end = CommandRunEnd {
            state: "done".into(),
            reason: None,
            exit_code: Some(0),
            stdout: String::new(),
            stderr: String::new(),
            at: r.w.h.app.now(),
        };
        assert!(
            r.w.h
                .store
                .end_command_run("run-busy-0", "running", &end)
                .await?
        );
        let approved = answer(&r, &id, true, &browser).await?;
        assert_eq!(approved.status, StatusCode::OK, "{}", approved.body);
        Ok::<_, Error>(())
    };
    let (ran, answered) = tokio::join!(asked, person);
    answered?;
    assert_eq!(ran?["state"], "done");
    Ok(())
}

/// BRK-016 (review round 1 of slice 3): a run that waited past its deadline while Yulius was
/// down is expired at the start, and its request ends.
#[tokio::test]
async fn a_start_expires_a_run_past_its_deadline() -> TestResult {
    let r = runs_world(json!({"approve": "always"})).await?;
    let run = stored_run(&r, "run-left-waiting", "waiting", Some(-1)).await?;
    assert_eq!(
        r.w.h.store.insert_command_run(&run, 4).await?,
        yulius_app::ports::store_ext::RunInsert::Stored
    );
    let request = yulius_app::needs_you::NeedsYouRecord {
        action: "Approve or reject task #1's run of the command deploy".into(),
        happened: None,
        problem: None,
        decision: true,
        link: None,
        at: run.created_at.0,
        cause: Some("command-run:run-left-waiting".into()),
        named: yulius_app::needs_you::Named::default(),
    };
    r.w.h
        .store
        .set_task_needs_you(&run.task_id, Some(&request), run.created_at)
        .await?;
    r.w.h.app.reconcile_command_runs().await?;
    assert_eq!(
        r.w.h
            .store
            .command_run("run-left-waiting")
            .await?
            .ok_or("run")?
            .state,
        "expired"
    );
    assert!(r.w.h.store.task_needs_you(&run.task_id).await?.is_none());
    Ok(())
}
