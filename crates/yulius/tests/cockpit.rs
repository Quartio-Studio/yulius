//! The Cockpit end to end with the real detached ptyhost and a fake `claude` that prints what it
//! was started with: one plain session per user and sandbox on a pool subscription, private,
//! never listed, renamed or closed as a terminal, resumed with the newest conversation when it
//! starts again, adopted after a server restart, reaped when unwatched for a day (SPEC-021).

mod support;

use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use axum::http::{Method, StatusCode};
use futures::{SinkExt as _, StreamExt as _};
use serde_json::{Value, json};
use support::{
    Creds, Harness, HarnessOptions, TestResult, call, harness_with, perms, seed_github_user,
    seed_sandbox, serve, ws_connect, ws_refusal_status,
};
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};
use yulius::wiring::session::PtySessionHost;
use yulius_app::model::{AccountRecord, AccountStatus};
use yulius_app::ports::store::AccountStore;
use yulius_app::shells::{MAX_SHELLS_PER_SANDBOX, SHELL_IDLE_MS};
use yulius_contracts::terminal::{KIND_OUTPUT, KIND_SNAPSHOT, decode_binary};
use yulius_domain::harness::Provider;
use yulius_domain::ids::{AccountId, SandboxId};
use yulius_domain::time::Timestamp;

type Socket = WebSocketStream<MaybeTlsStream<TcpStream>>;

const ORIGIN: &str = "https://yulius.test";
const WAIT: Duration = Duration::from_secs(20);
const TOKEN: &str = "synthetic-oauth-token";

/// Prints its arguments and environment, then stays as an interactive shell.
const FAKE_CLAUDE: &str = "#!/bin/sh\n\
echo \"cockpit-args:[$*]\"\n\
echo \"token:${CLAUDE_CODE_OAUTH_TOKEN:-none}\"\n\
echo \"config:$CLAUDE_CONFIG_DIR\"\n\
echo \"cwd:$(pwd)\"\n\
echo \"ready\"\n\
exec /bin/sh\n";

struct Fixture {
    h: Harness,
    alice: Creds,
    bob: Creds,
    carol: Creds,
    data: PathBuf,
}

/// alice and carol (write) and bob (read) on `s1`; one pool subscription unless `account` is
/// false; optionally on the data directory of an earlier fixture (a server restart).
async fn fixture_with(
    account: bool,
    data_dir: Option<PathBuf>,
) -> Result<Fixture, Box<dyn std::error::Error>> {
    // A short base: shell sockets live at <data>/shells/<16 hex>/pty.sock (Unix path limit).
    let dir = tempfile::Builder::new().prefix("yck").tempdir_in("/tmp")?;
    let exe = PathBuf::from(env!("CARGO_BIN_EXE_yulius"));
    let h = harness_with(HarnessOptions {
        session_host: Some(Arc::new(PtySessionHost::new(exe))),
        dir: Some(dir),
        configure: Some(Box::new(move |config| {
            config.contained = false;
            if let Some(data) = data_dir {
                config.data_dir = data;
            }
        })),
        ..HarnessOptions::default()
    })?;
    let data = h.app.config.data_dir.clone();
    assert!(h.app.set_shell_program(PathBuf::from("/bin/sh")));
    let fake = data.join("fake-claude");
    std::fs::write(&fake, FAKE_CLAUDE)?;
    std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755))?;
    assert!(h.app.set_cockpit_program(fake));
    let alice = seed_github_user(&h, "u-alice", "alice").await?;
    let bob = seed_github_user(&h, "u-bob", "bob").await?;
    let carol = seed_github_user(&h, "u-carol", "carol").await?;
    seed_sandbox(&h, "s1", "fixture", "u-alice").await?;
    h.github.set_perm("alice", "acme/fixture", perms("write"));
    h.github.set_perm("carol", "acme/fixture", perms("write"));
    h.github.set_perm("bob", "acme/fixture", perms("read"));
    std::fs::create_dir_all(data.join("sandboxes/s1/repo"))?;
    if account {
        h.store
            .insert_account(&AccountRecord {
                id: AccountId::parse("acc-1")?,
                provider: Provider::Anthropic,
                label: "acc-1".into(),
                credential: Some(h.app.deps.crypto.seal(TOKEN.as_bytes())?),
                status: AccountStatus::Ok,
                usage_json: None,
                last_error: None,
                created_at: Timestamp(1),
                updated_at: Timestamp(1),
            })
            .await?;
    }
    Ok(Fixture {
        h,
        alice,
        bob,
        carol,
        data,
    })
}

async fn fixture() -> Result<Fixture, Box<dyn std::error::Error>> {
    fixture_with(true, None).await
}

async fn open_cockpit(
    f: &Fixture,
    creds: &Creds,
) -> Result<support::Resp, Box<dyn std::error::Error>> {
    call(
        &f.h.router,
        Method::POST,
        "/api/sandboxes/s1/cockpit",
        None,
        creds,
    )
    .await
}

async fn cockpit_id(f: &Fixture, creds: &Creds) -> Result<String, Box<dyn std::error::Error>> {
    let resp = open_cockpit(f, creds).await?;
    assert_eq!(resp.status, StatusCode::OK, "{:?}", resp.body);
    Ok(resp.body["id"].as_str().ok_or("id")?.to_owned())
}

async fn attach(
    addr: std::net::SocketAddr,
    id: &str,
    creds: &Creds,
) -> Result<Socket, Box<dyn std::error::Error>> {
    let mut socket = ws_connect(
        addr,
        &format!("/api/shells/{id}/terminal?cols=100&rows=30"),
        creds,
        ORIGIN,
    )
    .await?;
    socket
        .send(Message::text(
            json!({"type": "hello", "last_seq": null, "cols": 100, "rows": 30}).to_string(),
        ))
        .await?;
    Ok(socket)
}

/// Reads frames until the accumulated terminal text contains `needle`.
async fn read_until(
    socket: &mut Socket,
    text: &mut String,
    needle: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let deadline = tokio::time::Instant::now() + WAIT;
    while !text.contains(needle) {
        let message = tokio::time::timeout_at(deadline, socket.next())
            .await
            .map_err(|_| format!("timed out waiting for {needle:?}; screen so far: {text:?}"))?
            .ok_or("socket closed")??;
        if let Message::Binary(bytes) = message
            && let Some((kind, _, payload)) = decode_binary(&bytes)
            && (kind == KIND_OUTPUT || kind == KIND_SNAPSHOT)
        {
            text.push_str(&String::from_utf8_lossy(payload));
        }
    }
    Ok(())
}

/// The value printed after `key:` on the fake's screen.
fn printed(screen: &str, key: &str) -> Result<String, Box<dyn std::error::Error>> {
    let start = screen
        .find(&format!("{key}:"))
        .ok_or_else(|| format!("{key} not printed"))?
        + key.len()
        + 1;
    let rest = screen.get(start..).unwrap_or_default();
    Ok(rest.lines().next().unwrap_or_default().trim().to_owned())
}

async fn wait_for(what: &str, mut check: impl FnMut() -> bool) -> TestResult {
    let deadline = tokio::time::Instant::now() + WAIT;
    while tokio::time::Instant::now() < deadline {
        if check() {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    Err(format!("timed out waiting for {what}").into())
}

fn sid() -> SandboxId {
    SandboxId::parse("s1").unwrap_or_else(|_| unreachable!())
}

#[tokio::test]
async fn a_write_user_gets_one_plain_cockpit_on_a_pool_subscription() -> TestResult {
    let f = fixture().await?;
    let served = serve(f.h.router.clone()).await?;

    // Two opens at once (a double click, two tabs) start one Cockpit (CKP-001).
    let (first, second) = tokio::join!(open_cockpit(&f, &f.alice), open_cockpit(&f, &f.alice));
    let (first, second) = (first?, second?);
    assert_eq!(first.status, StatusCode::OK, "{:?}", first.body);
    assert_eq!(second.status, StatusCode::OK, "{:?}", second.body);
    let id = first.body["id"].as_str().ok_or("id")?.to_owned();
    assert_eq!(second.body["id"], first.body["id"]);
    assert_eq!(f.h.app.sandbox_shells(&sid()).len(), 1);
    assert_eq!(
        cockpit_id(&f, &f.alice).await?,
        id,
        "the same Cockpit again"
    );

    // Plain Claude Code on the pool's subscription, in the sandbox repository, its config in
    // the user's own home (CKP-002).
    let mut socket = attach(served.addr, &id, &f.alice).await?;
    let mut screen = String::new();
    read_until(&mut socket, &mut screen, "ready").await?;
    assert_eq!(printed(&screen, "cockpit-args")?, "[]");
    assert_eq!(printed(&screen, "token")?, TOKEN);
    let repo = std::fs::canonicalize(f.data.join("sandboxes/s1/repo"))?;
    assert_eq!(printed(&screen, "cwd")?, repo.to_string_lossy());
    let config = PathBuf::from(printed(&screen, "config")?);
    assert!(config.ends_with("home/.claude"), "{config:?}");
    assert!(config.starts_with(&f.data), "{config:?}");
    let settled: Value = serde_json::from_slice(&std::fs::read(config.join(".claude.json"))?)?;
    assert_eq!(settled["hasCompletedOnboarding"], true);
    assert!(settled["bypassPermissionsModeAccepted"].is_null());
    assert!(settled["projects"][repo.to_string_lossy().as_ref()].is_object());
    // The token never reaches the shell record or argv.
    let record = std::fs::read_to_string(f.data.join("shells").join(&id).join("shell.json"))?;
    assert!(!record.contains(TOKEN));
    assert!(record.contains("\"kind\":\"cockpit\""), "{record}");

    // Not a terminal: never listed, renamed or closed, and not counted against the terminals
    // (CKP-004). There is no way to delete it.
    let list = call(
        &f.h.router,
        Method::GET,
        "/api/sandboxes/s1/shells",
        None,
        &f.alice,
    )
    .await?;
    assert_eq!(list.body, json!([]));
    let rename = call(
        &f.h.router,
        Method::PATCH,
        &format!("/api/sandboxes/s1/shells/{id}"),
        Some(json!({"name": "mine"})),
        &f.alice,
    )
    .await?;
    assert_eq!(rename.status, StatusCode::NOT_FOUND);
    let close = call(
        &f.h.router,
        Method::DELETE,
        &format!("/api/sandboxes/s1/shells/{id}"),
        None,
        &f.alice,
    )
    .await?;
    assert_eq!(close.status, StatusCode::NOT_FOUND);
    let delete = call(
        &f.h.router,
        Method::DELETE,
        "/api/sandboxes/s1/cockpit",
        None,
        &f.alice,
    )
    .await?;
    assert_eq!(delete.status, StatusCode::METHOD_NOT_ALLOWED);
    for _ in 0..MAX_SHELLS_PER_SANDBOX {
        let shell = call(
            &f.h.router,
            Method::POST,
            "/api/sandboxes/s1/shells",
            None,
            &f.alice,
        )
        .await?;
        assert_eq!(shell.status, StatusCode::CREATED, "{:?}", shell.body);
    }
    assert_eq!(cockpit_id(&f, &f.alice).await?, id, "still there");

    // Private (CKP-005): carol gets her own, cannot attach to alice's; bob only reads.
    let carols = cockpit_id(&f, &f.carol).await?;
    assert_ne!(carols, id);
    let refused = ws_connect(
        served.addr,
        &format!("/api/shells/{id}/terminal"),
        &f.carol,
        ORIGIN,
    )
    .await;
    assert_eq!(
        refused.as_ref().err().and_then(ws_refusal_status),
        Some(404)
    );
    assert_eq!(
        open_cockpit(&f, &f.bob).await?.status,
        StatusCode::FORBIDDEN
    );

    // `/clear` is Claude Code's own: the process stays, so does the Cockpit.
    socket
        .send(Message::text(
            json!({"type": "input", "data": "echo still-$((1+1))\r"}).to_string(),
        ))
        .await?;
    read_until(&mut socket, &mut screen, "still-2").await?;
    f.h.app.end_all_shells().await;
    Ok(())
}

#[tokio::test]
async fn a_cockpit_started_again_resumes_the_newest_conversation() -> TestResult {
    let f = fixture().await?;
    let served = serve(f.h.router.clone()).await?;
    let id = cockpit_id(&f, &f.alice).await?;
    let mut socket = attach(served.addr, &id, &f.alice).await?;
    let mut screen = String::new();
    read_until(&mut socket, &mut screen, "ready").await?;
    let config = PathBuf::from(printed(&screen, "config")?);
    let cwd = printed(&screen, "cwd")?;

    // Two conversations so far, the newest one after a `/clear` (CKP-003).
    let project = yulius_harness::paths::claude_project_dir(&config, Path::new(&cwd));
    std::fs::create_dir_all(&project)?;
    std::fs::write(project.join("before-clear.jsonl"), "{}\n")?;
    std::thread::sleep(Duration::from_millis(20));
    std::fs::write(project.join("after-clear.jsonl"), "{}\n")?;

    // The user leaves it (`/exit`): the Cockpit is not running any more.
    socket
        .send(Message::text(
            json!({"type": "input", "data": "exit\r"}).to_string(),
        ))
        .await?;
    wait_for("the Cockpit to end", || {
        f.h.app.sandbox_shells(&sid()).is_empty()
    })
    .await?;

    let again = cockpit_id(&f, &f.alice).await?;
    assert_ne!(again, id, "a new process");
    let mut socket = attach(served.addr, &again, &f.alice).await?;
    let mut screen = String::new();
    read_until(&mut socket, &mut screen, "ready").await?;
    assert_eq!(printed(&screen, "cockpit-args")?, "[--resume after-clear]");

    // Unwatched for a day it ends like a terminal, and comes back with its conversation.
    drop(socket);
    wait_for("viewers to leave", || {
        f.h.app
            .sandbox_shells(&sid())
            .iter()
            .all(|shell| shell.viewer_count() == 0)
    })
    .await?;
    f.h.clock
        .0
        .fetch_add(SHELL_IDLE_MS + 1_000, Ordering::SeqCst);
    assert_eq!(f.h.app.reap_idle_shells().await, 1);
    assert!(f.h.app.sandbox_shells(&sid()).is_empty());
    let back = cockpit_id(&f, &f.alice).await?;
    let mut socket = attach(served.addr, &back, &f.alice).await?;
    let mut screen = String::new();
    read_until(&mut socket, &mut screen, "ready").await?;
    assert_eq!(printed(&screen, "cockpit-args")?, "[--resume after-clear]");
    f.h.app.end_all_shells().await;
    Ok(())
}

#[tokio::test]
async fn a_running_cockpit_is_adopted_as_the_cockpit_after_a_restart() -> TestResult {
    let f = fixture().await?;
    let id = cockpit_id(&f, &f.alice).await?;
    f.h.app.stop_workspace_services().await;

    let g = fixture_with(true, Some(f.data.clone())).await?;
    assert_eq!(g.h.app.adopt_shells().await, (1, 0));
    assert_eq!(
        cockpit_id(&g, &g.alice).await?,
        id,
        "the same session (CKP-006)"
    );
    let list = call(
        &g.h.router,
        Method::GET,
        "/api/sandboxes/s1/shells",
        None,
        &g.alice,
    )
    .await?;
    assert_eq!(list.body, json!([]), "still not a terminal");
    g.h.app.end_all_shells().await;
    Ok(())
}

#[tokio::test]
async fn without_a_subscription_the_cockpit_says_so_and_nothing_runs() -> TestResult {
    let f = fixture_with(false, None).await?;
    let resp = open_cockpit(&f, &f.alice).await?;
    assert_eq!(resp.status, StatusCode::CONFLICT, "{:?}", resp.body);
    assert!(
        resp.body.to_string().contains("no Claude account"),
        "{:?}",
        resp.body
    );
    assert!(f.h.app.sandbox_shells(&sid()).is_empty());
    let left = std::fs::read_dir(f.data.join("shells"))
        .map(|entries| entries.count())
        .unwrap_or(0);
    assert_eq!(left, 0, "no run directory left behind (CKP-007)");
    Ok(())
}
