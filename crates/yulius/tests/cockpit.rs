//! The Cockpit end to end with the real detached ptyhost and a fake `claude` that prints what it
//! was started with: one plain session per user and sandbox on a pool subscription, private,
//! never listed, renamed or closed as a terminal, resumed with the newest conversation when it
//! starts again, adopted after a server restart, reaped when unwatched for a day (SPEC-021).

use crate::support;

use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use axum::body::Body;
use axum::http::{Method, StatusCode};
use futures::{SinkExt as _, StreamExt as _};
use serde_json::{Value, json};
use support::gated::GatedHost;
use support::{
    Creds, Harness, HarnessOptions, TestResult, call, harness_with, perms, seed_github_user,
    seed_sandbox, send, serve, ws_connect, ws_refusal_status,
};
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};
use yulius_app::error::{AppError, AppResult};
use yulius_app::model::{AccountRecord, AccountStatus};
use yulius_app::ports::harness::{
    CockpitLaunch, HarnessLaunch, HarnessPlan, HarnessPort, OutputMasker, TranscriptWatch,
    WatchFrom,
};
use yulius_app::ports::session::{LaunchPlan, SessionHost};
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

/// Prints its arguments (the MCP config, allowed tools and settings, too long for a line, go to
/// files in its config dir, as does its shell prefix) and environment, then stays as an
/// interactive shell.
const FAKE_CLAUDE: &str = "#!/bin/sh\n\
if [ \"$1\" = --resume ] && [ -f \"$CLAUDE_CONFIG_DIR/fail-resume\" ]; then exit 3; fi\n\
if [ \"$1\" = --resume ] && [ -f \"$CLAUDE_CONFIG_DIR/kill-resume\" ]; then kill -KILL $$; fi\n\
args=; prev=\n\
for a in \"$@\"; do\n\
  case \"$prev\" in\n\
    --mcp-config) printf %s \"$a\" > \"$CLAUDE_CONFIG_DIR/launch-mcp-config\" ;;\n\
    --allowedTools) printf %s \"$a\" > \"$CLAUDE_CONFIG_DIR/launch-allowed-tools\" ;;\n\
    --settings) printf %s \"$a\" > \"$CLAUDE_CONFIG_DIR/launch-settings\" ;;\n\
    *) case \"$a\" in --mcp-config|--allowedTools|--settings) ;; *) args=\"$args $a\" ;; esac ;;\n\
  esac\n\
  prev=$a\n\
done\n\
echo \"cockpit-args:[${args# }]\"\n\
echo \"bearer:${YULIUS_COCKPIT_TOKEN:-none}\"\n\
echo \"workspace:${YULIUS_TASK_TOKEN:-none}\"\n\
printf %s \"${CLAUDE_CODE_SHELL_PREFIX:-none}\" > \"$CLAUDE_CONFIG_DIR/launch-shell-prefix\"\n\
echo \"token:${CLAUDE_CODE_OAUTH_TOKEN:-none}\"\n\
echo \"interrupted:${CLAUDE_CODE_RESUME_INTERRUPTED_TURN:-none}\"\n\
echo \"proxy:${HTTPS_PROXY:-none}\"\n\
echo \"no-proxy:${NO_PROXY:-none}\"\n\
echo \"ca:${SSL_CERT_FILE:-none}\"\n\
echo \"placeholder:${APIFY_KEY:-none}\"\n\
echo \"config:$CLAUDE_CONFIG_DIR\"\n\
echo \"cwd:$(pwd)\"\n\
echo \"ready\"\n\
if [ -f \"$CLAUDE_CONFIG_DIR/exit-3-on-signal\" ]; then\n\
  trap 'exit 3' HUP INT TERM\n\
  while read -r line; do eval \"$line\"; done\n\
  exit 3\n\
fi\n\
exec /bin/sh\n";

struct Fixture {
    h: Harness,
    /// The session host: holds the next start between its spawn and its attach when armed.
    gate: Arc<GatedHost>,
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
    fixture_ending(account, data_dir, None).await
}

/// [`fixture_with`], optionally with another wait for a shell's exit before its host is
/// terminated (`shell_exit_ms`).
async fn fixture_ending(
    account: bool,
    data_dir: Option<PathBuf>,
    exit_ms: Option<u64>,
) -> Result<Fixture, Box<dyn std::error::Error>> {
    // A short base: shell sockets live at <data>/shells/<16 hex>/pty.sock (Unix path limit).
    let dir = tempfile::Builder::new().prefix("yck").tempdir_in("/tmp")?;
    let exe = PathBuf::from(env!("CARGO_BIN_EXE_yulius"));
    let gate = Arc::new(GatedHost::new(exe.clone()));
    // The real `yulius` runs the Superagent's redaction wrapper and hook (SPEC-147).
    let harness = Arc::new(yulius::wiring::harness::HarnessAdapter::new(
        exe.clone(),
        "http://127.0.0.1:1",
    ));
    let h = harness_with(HarnessOptions {
        // Real Git: the Superagent's `.env` copies ask whether Git ignores the file (SPEC-186).
        git: Some(Arc::new(yulius::wiring::git::GitAdapter::new("git"))),
        session_host: Some(Arc::clone(&gate) as Arc<dyn SessionHost>),
        harness: Some(harness),
        dir: Some(dir),
        configure: Some(Box::new(move |config| {
            config.contained = false;
            config.yulius_exe = exe;
            if let Some(data) = data_dir {
                config.data_dir = data;
            }
            if let Some(ms) = exit_ms {
                config.shell_exit_ms = ms;
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
                email: None,
                email_checked_at: None,
            })
            .await?;
    }
    Ok(Fixture {
        h,
        gate,
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
    // The value ends at the line's end or at the terminal's own control output that can follow
    // it on the same line (`\x1b>`, cursor moves: CI on the VM read them as part of the value).
    let line = rest.lines().next().unwrap_or_default();
    Ok(line
        .split('\u{1b}')
        .next()
        .unwrap_or_default()
        .trim()
        .to_owned())
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

/// A conversation file of Claude Code, last written `secs` after the epoch.
fn conversation(project: &Path, id: &str, secs: u64) -> TestResult {
    std::fs::create_dir_all(project)?;
    let file = std::fs::File::create(project.join(format!("{id}.jsonl")))?;
    file.set_modified(std::time::UNIX_EPOCH + Duration::from_secs(1_700_000_000 + secs))?;
    Ok(())
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
        f.h.app.cockpit_starts_pending(),
        0,
        "the start lock goes with the starts"
    );
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
    // The default model and effort (SPEC-023 DEF-004).
    assert_eq!(
        printed(&screen, "cockpit-args")?,
        "[--model claude-opus-5-5 --effort high]"
    );
    assert_eq!(printed(&screen, "token")?, TOKEN);
    let repo = std::fs::canonicalize(f.data.join("sandboxes/s1/repo"))?;
    assert_eq!(printed(&screen, "cwd")?, repo.to_string_lossy());
    let config = PathBuf::from(printed(&screen, "config")?);
    assert!(config.ends_with("home/.claude"), "{config:?}");
    assert!(config.starts_with(&f.data), "{config:?}");
    // The Yulius MCP server, its bearer only in the environment, accepted by `/mcp` (SPEC-037
    // MGT-004, AC-MGT-3).
    let mcp_config: Value =
        serde_json::from_slice(&std::fs::read(config.join("launch-mcp-config"))?)?;
    let yulius = &mcp_config["mcpServers"]["yulius"];
    assert_eq!(
        yulius["url"],
        format!("{}/mcp", f.h.app.config.internal_url)
    );
    assert_eq!(
        yulius["headers"]["Authorization"],
        "Bearer ${YULIUS_COCKPIT_TOKEN}"
    );
    let allowed = std::fs::read_to_string(config.join("launch-allowed-tools"))?;
    assert!(allowed.contains("mcp__yulius__tasks_list"), "{allowed}");
    assert!(allowed.contains("mcp__yulius__secret_verify"), "{allowed}");
    assert!(
        !allowed.contains("task_create"),
        "creating asks first: {allowed}"
    );
    let bearer = printed(&screen, "bearer")?;
    assert!(bearer.starts_with("ck_"), "{bearer}");
    let tools = mcp(&f, &bearer, "tools/list", json!({})).await?;
    assert!(
        tools["result"]["tools"]
            .as_array()
            .is_some_and(|t| t.iter().any(|t| t["name"] == "task_create")),
        "{tools}"
    );
    let settled: Value = serde_json::from_slice(&std::fs::read(config.join(".claude.json"))?)?;
    assert_eq!(settled["hasCompletedOnboarding"], true);
    assert!(settled["bypassPermissionsModeAccepted"].is_null());
    assert!(settled["projects"][repo.to_string_lossy().as_ref()].is_object());
    // The tokens never reach the shell record or argv.
    let record = std::fs::read_to_string(f.data.join("shells").join(&id).join("shell.json"))?;
    assert!(!record.contains(TOKEN));
    assert!(!record.contains(&bearer));
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

    // A file dropped (or an image pasted) on it reaches Claude by path, like a task terminal's;
    // only its owner may drop onto it.
    let drop_path = format!("/api/sandboxes/s1/shells/{id}/attachments?name=shot.png");
    let png = || Body::from(b"png".to_vec());
    let foreign = send(
        &f.h.router,
        Method::POST,
        &drop_path,
        png(),
        Some(3),
        &f.carol,
    )
    .await?;
    assert_eq!(foreign.status, StatusCode::NOT_FOUND, "{:?}", foreign.body);
    let dropped = send(
        &f.h.router,
        Method::POST,
        &drop_path,
        png(),
        Some(3),
        &f.alice,
    )
    .await?;
    assert_eq!(dropped.status, StatusCode::CREATED, "{:?}", dropped.body);

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
    conversation(&project, "before-clear", 1_000)?;
    conversation(&project, "after-clear", 2_000)?;

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
    assert_eq!(
        printed(&screen, "cockpit-args")?,
        "[--resume after-clear --model claude-opus-5-5 --effort high]"
    );

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
    assert_eq!(
        printed(&screen, "cockpit-args")?,
        "[--resume after-clear --model claude-opus-5-5 --effort high]"
    );
    f.h.app.end_all_shells().await;
    Ok(())
}

/// SPEC-206 RMV-003 (#624): removing a sandbox ends its Superagents and terminals. They kept
/// running, and a person watching one saw the removed sandbox's Superagent go on working.
#[tokio::test]
async fn removing_the_sandbox_ends_its_superagent_and_terminals() -> TestResult {
    let f = fixture().await?;
    let served = serve(f.h.router.clone()).await?;
    f.h.github.set_perm("alice", "acme/fixture", perms("admin"));
    let id = cockpit_id(&f, &f.alice).await?;
    let mut socket = attach(served.addr, &id, &f.alice).await?;
    let mut screen = String::new();
    read_until(&mut socket, &mut screen, "ready").await?;
    let terminal = call(
        &f.h.router,
        Method::POST,
        "/api/sandboxes/s1/shells",
        Some(json!({})),
        &f.carol,
    )
    .await?;
    assert_eq!(terminal.status, StatusCode::CREATED, "{:?}", terminal.body);
    assert_eq!(f.h.app.sandbox_shells(&sid()).len(), 2);

    let removed = call(
        &f.h.router,
        Method::DELETE,
        "/api/sandboxes/s1",
        None,
        &f.alice,
    )
    .await?;
    assert_eq!(removed.status, StatusCode::NO_CONTENT, "{:?}", removed.body);

    wait_for("the removed sandbox's shells to end", || {
        f.h.app.sandbox_shells(&sid()).is_empty()
    })
    .await?;
    // The watcher's terminal is told and closed: nothing of the sandbox goes on behind it.
    let closed = tokio::time::timeout(WAIT, async {
        while let Some(Ok(message)) = socket.next().await {
            if matches!(message, Message::Close(_)) {
                break;
            }
        }
    })
    .await;
    assert!(closed.is_ok(), "the viewer's socket closes");
    assert_eq!(run_dirs(&f), 0, "no shell host is left");
    Ok(())
}

/// SPEC-206 RMV-003 (review of #193): a Superagent whose start was under way when its sandbox
/// was removed ends once it registers; it outlived the removal (the removal ended only the
/// shells registered by then).
#[tokio::test]
async fn a_superagent_starting_while_the_sandbox_is_removed_ends() -> TestResult {
    let f = fixture().await?;
    f.h.github.set_perm("alice", "acme/fixture", perms("admin"));
    f.gate.arm();
    let app = f.h.app.clone();
    let router = f.h.router.clone();
    let alice = f.alice.clone();
    let open = tokio::spawn(async move {
        call(
            &router,
            Method::POST,
            "/api/sandboxes/s1/cockpit",
            None,
            &alice,
        )
        .await
        .map_err(|e| e.to_string())
    });
    f.gate.reached().await;
    assert!(app.sandbox_shells(&sid()).is_empty(), "not registered yet");

    let removed = call(
        &f.h.router,
        Method::DELETE,
        "/api/sandboxes/s1",
        None,
        &f.alice,
    )
    .await?;
    assert_eq!(removed.status, StatusCode::NO_CONTENT, "{:?}", removed.body);
    f.gate.release();
    let _ = open.await?;

    wait_for("the late Superagent to end", || {
        f.h.app.sandbox_shells(&sid()).is_empty() && run_dirs(&f) == 0
    })
    .await?;
    Ok(())
}

#[tokio::test]
async fn a_running_cockpit_is_adopted_as_the_cockpit_after_a_restart() -> TestResult {
    let f = fixture().await?;
    let id = cockpit_id(&f, &f.alice).await?;
    let alice = yulius_domain::ids::UserId::parse("u-alice")?;
    let token = f.h.app.cockpit_mcp_token(&alice, &sid());
    assert!(token.is_some());
    f.h.app.stop_workspace_services().await;

    let g = fixture_with(true, Some(f.data.clone())).await?;
    assert_eq!(g.h.app.adopt_shells().await, (1, 0));
    assert_eq!(
        g.h.app.cockpit_mcp_token(&alice, &sid()),
        token,
        "its Yulius MCP token survives the restart (SPEC-037 MGT-005)"
    );
    assert_eq!(
        g.h.app.cockpit_refresh_state(&id),
        "none",
        "started by this Yulius: nothing to refresh (SPEC-186 SAV-006)"
    );
    let tools = mcp(
        &g,
        token.as_deref().ok_or("token")?,
        "tools/list",
        json!({}),
    )
    .await?;
    assert!(
        tools["result"]["tools"].is_array(),
        "accepted again: {tools}"
    );
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
    // The account gate's refusal, with where to connect one (SPEC-246 CLA-002).
    assert_eq!(resp.body["error"]["code"], "claude_account_required");
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

#[tokio::test]
async fn a_resume_that_fails_at_once_starts_fresh_the_next_time() -> TestResult {
    let f = fixture().await?;
    let served = serve(f.h.router.clone()).await?;
    let id = cockpit_id(&f, &f.alice).await?;
    let mut socket = attach(served.addr, &id, &f.alice).await?;
    let mut screen = String::new();
    read_until(&mut socket, &mut screen, "ready").await?;
    let config = PathBuf::from(printed(&screen, "config")?);
    let cwd = printed(&screen, "cwd")?;
    let project = yulius_harness::paths::claude_project_dir(&config, Path::new(&cwd));
    conversation(&project, "broken", 1_000)?;
    socket
        .send(Message::text(
            json!({"type": "input", "data": "exit\r"}).to_string(),
        ))
        .await?;
    wait_for("the Cockpit to end", || {
        f.h.app.sandbox_shells(&sid()).is_empty()
    })
    .await?;

    // Claude Code cannot resume that conversation and exits at once: the Cockpit, which cannot
    // be deleted, must not stay stuck on it.
    std::fs::write(config.join("fail-resume"), "")?;
    cockpit_id(&f, &f.alice).await?;
    wait_for("the failed resume to end", || {
        f.h.app.sandbox_shells(&sid()).is_empty()
    })
    .await?;
    let fresh = cockpit_id(&f, &f.alice).await?;
    let mut socket = attach(served.addr, &fresh, &f.alice).await?;
    let mut screen = String::new();
    read_until(&mut socket, &mut screen, "ready").await?;
    assert_eq!(
        printed(&screen, "cockpit-args")?,
        "[--model claude-opus-5-5 --effort high]",
        "a fresh conversation"
    );
    f.h.app.end_all_shells().await;
    Ok(())
}

/// SPEC-095 PMC-001/003/004: a repository with a `.mcp.json` (as the onboarded project with
/// supabase, apify and pipedream-gmail): its servers join the sandbox's MCP servers, and the
/// Cockpit starts with Claude Code rejecting every one of them, so it never asks the person. An
/// invalid `.mcp.json` still starts the Cockpit, as before.
#[tokio::test]
async fn a_cockpit_takes_the_repository_mcp_servers_into_yulius_without_asking() -> TestResult {
    use yulius_app::ports::store_ext::McpStore as _;
    let f = fixture().await?;
    let served = serve(f.h.router.clone()).await?;
    let repo = f.data.join("sandboxes/s1/repo");
    std::fs::write(
        repo.join(".mcp.json"),
        r#"{"mcpServers": {
            "supabase": {"command": "npx",
                         "args": ["-y", "@supabase/mcp-server-supabase@latest"],
                         "env": {"SUPABASE_ACCESS_TOKEN": "${SUPABASE_ACCESS_TOKEN}"}},
            "apify": {"type": "stdio", "command": "npx", "args": ["-y", "@apify/actors-mcp-server"]},
            "pipedream-gmail": {"type": "http", "url": "https://remote.mcp.pipedream.test/gmail"},
            "legacy": {"type": "sse", "url": "https://legacy.test/sse"}
        }}"#,
    )?;

    let opened = open_cockpit(&f, &f.alice).await?;
    assert_eq!(opened.status, StatusCode::OK, "{:?}", opened.body);
    let id = opened.body["id"].as_str().ok_or("id")?.to_owned();
    let mut socket = attach(served.addr, &id, &f.alice).await?;
    let mut screen = String::new();
    read_until(&mut socket, &mut screen, "ready").await?;
    assert_eq!(
        printed(&screen, "cockpit-args")?,
        "[--model claude-opus-5-5 --effort high]"
    );
    let config = PathBuf::from(printed(&screen, "config")?);
    let settings: Value = serde_json::from_slice(&std::fs::read(config.join("launch-settings"))?)?;
    let mut rejected: Vec<&str> = settings["disabledMcpjsonServers"]
        .as_array()
        .ok_or("disabledMcpjsonServers")?
        .iter()
        .filter_map(Value::as_str)
        .collect();
    rejected.sort_unstable();
    assert_eq!(rejected, ["apify", "legacy", "pipedream-gmail", "supabase"]);
    let stored = f.h.store.mcp_servers(&sid(), None).await?;
    let mut names: Vec<(&str, &str)> = stored
        .iter()
        .map(|s| (s.name.as_str(), s.transport.as_str()))
        .collect();
    names.sort_unstable();
    assert_eq!(
        names,
        [
            ("apify", "stdio"),
            ("pipedream-gmail", "http"),
            ("supabase", "stdio")
        ],
        "the sse server is skipped"
    );
    assert!(
        stored
            .iter()
            .all(|s| s.sandbox_id == Some(sid()) && s.enabled),
        "{stored:?}"
    );

    // An invalid file: carol's Cockpit starts as before, with nothing to reject.
    std::fs::write(repo.join(".mcp.json"), "{ not json")?;
    let opened = open_cockpit(&f, &f.carol).await?;
    assert_eq!(opened.status, StatusCode::OK, "{:?}", opened.body);
    let id = opened.body["id"].as_str().ok_or("id")?.to_owned();
    let mut socket = attach(served.addr, &id, &f.carol).await?;
    let mut screen = String::new();
    read_until(&mut socket, &mut screen, "ready").await?;
    let config = PathBuf::from(printed(&screen, "config")?);
    let settings: Value = serde_json::from_slice(&std::fs::read(config.join("launch-settings"))?)?;
    assert!(settings["disabledMcpjsonServers"].is_null(), "{settings}");
    assert_eq!(f.h.store.mcp_servers(&sid(), None).await?.len(), 3);
    Ok(())
}

/// SPEC-332 MSC-004: a person's Superagent gets their own personal MCP server and the team's;
/// another person's Superagent gets only the team's.
#[tokio::test]
async fn a_superagent_gets_its_persons_personal_mcp_servers_only() -> TestResult {
    let f = fixture().await?;
    let served = serve(f.h.router.clone()).await?;
    for (body, who) in [
        (
            json!({"name": "fleet", "transport": "http", "url": "https://fleet.test/mcp",
            "scope": "instance", "audience": "personal"}),
            &f.alice,
        ),
        (
            json!({"name": "docs", "transport": "http", "url": "https://docs.test/mcp"}),
            &f.carol,
        ),
    ] {
        let added = call(
            &f.h.router,
            Method::POST,
            "/api/sandboxes/s1/mcp",
            Some(body),
            who,
        )
        .await?;
        assert_eq!(added.status, StatusCode::CREATED, "{}", added.body);
    }
    for (who, expected) in [
        (&f.alice, vec!["docs", "fleet", "yulius"]),
        (&f.carol, vec!["docs", "yulius"]),
    ] {
        let opened = open_cockpit(&f, who).await?;
        assert_eq!(opened.status, StatusCode::OK, "{:?}", opened.body);
        let id = opened.body["id"].as_str().ok_or("id")?.to_owned();
        let mut socket = attach(served.addr, &id, who).await?;
        let mut screen = String::new();
        read_until(&mut socket, &mut screen, "ready").await?;
        let config = PathBuf::from(printed(&screen, "config")?);
        let mcp_config: Value =
            serde_json::from_slice(&std::fs::read(config.join("launch-mcp-config"))?)?;
        let mut names: Vec<&str> = mcp_config["mcpServers"]
            .as_object()
            .ok_or("mcpServers")?
            .keys()
            .map(String::as_str)
            .collect();
        names.sort_unstable();
        assert_eq!(names, expected);
    }
    f.h.app.end_all_shells().await;
    Ok(())
}

/// SPEC-218 MOR-001..003 (#654): agents write `.mcp.json`, so a server imported from it never
/// has its command run on the host by the MCP Test, even when an instance administrator presses it
/// on a contained instance. It stays refused through an enable toggle and through a command change
/// that keeps the file's (hidden) environment; once a person changed its command and replaced its
/// environment, it is theirs and the Test runs it. A loopback HTTP server from the file is not
/// tested either.
#[tokio::test]
async fn the_mcp_test_never_runs_a_command_from_the_repository_mcp_json() -> TestResult {
    let (h, _recorder, alice) = contained_harness().await?;
    let root = support::user_with_token(&h, "u-root", "root", true).await?;
    h.github.set_perm("root", "acme/fixture", perms("write"));
    let data = h.app.config.data_dir.clone();
    let marker = data.join("agent-code-ran");
    let touch = format!("touch {}", marker.display());
    std::fs::write(
        data.join("sandboxes/s1/repo/.mcp.json"),
        json!({"mcpServers": {
            "helper": {"command": "/bin/sh", "args": ["-c", touch],
                       "env": {"NODE_OPTIONS": "--require ./agent.js"}},
            "local": {"type": "http", "url": "http://127.0.0.1:9/mcp"},
            "switch": {"command": "/bin/sh", "args": ["-c", "true"],
                       "env": {"LD_PRELOAD": "./agent.so"}}
        }})
        .to_string(),
    )?;
    // Opening the Cockpit imports the file (the launch itself is recorded and refused).
    let opened = call(
        &h.router,
        Method::POST,
        "/api/sandboxes/s1/cockpit",
        None,
        &alice,
    )
    .await?;
    assert_eq!(
        opened.status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "{:?}",
        opened.body
    );
    let listed = call(&h.router, Method::GET, "/api/sandboxes/s1/mcp", None, &root).await?;
    assert_eq!(listed.status, StatusCode::OK, "{:?}", listed.body);
    let server = |name: &str| -> Result<Value, Box<dyn std::error::Error>> {
        Ok(listed
            .body
            .as_array()
            .ok_or("servers")?
            .iter()
            .find(|s| s["name"] == name)
            .ok_or(format!("no {name}"))?
            .clone())
    };
    let helper = server("helper")?;
    let id = helper["id"].as_str().ok_or("id")?.to_owned();
    let test = |id: String| {
        let router = h.router.clone();
        let root = root.clone();
        async move {
            let resp = call(
                &router,
                Method::POST,
                &format!("/api/mcp/{id}/test"),
                None,
                &root,
            )
            .await?;
            assert_eq!(resp.status, StatusCode::OK, "{:?}", resp.body);
            Ok::<Value, Box<dyn std::error::Error>>(resp.body)
        }
    };
    let refused = |result: &Value| {
        result["ok"] == false
            && result["error"]
                .as_str()
                .is_some_and(|e| e.contains(".mcp.json"))
    };

    let result = test(id.clone()).await?;
    assert!(!marker.exists(), "the agent's command ran on the host");
    assert!(refused(&result), "{result}");
    assert_eq!(helper["origin"], "mcp_json", "{helper}");

    // The enable toggle re-sends the same command.
    let put = |body: Value| {
        let router = h.router.clone();
        let root = root.clone();
        let id = id.clone();
        async move {
            let resp = call(
                &router,
                Method::PUT,
                &format!("/api/mcp/{id}"),
                Some(body),
                &root,
            )
            .await?;
            assert_eq!(resp.status, StatusCode::OK, "{:?}", resp.body);
            Ok::<Value, Box<dyn std::error::Error>>(resp.body)
        }
    };
    let toggled = put(
        json!({"name": "helper", "transport": "stdio", "command": "/bin/sh",
                             "args": ["-c", touch], "enabled": false}),
    )
    .await?;
    assert_eq!(toggled["origin"], "mcp_json", "{toggled}");
    let result = test(id.clone()).await?;
    assert!(refused(&result) && !marker.exists(), "{result}");

    // A new command with the file's environment kept is still the agent's.
    let person = format!("{touch} && exit 0");
    let kept_env = put(
        json!({"name": "helper", "transport": "stdio", "command": "/bin/sh",
                              "args": ["-c", person]}),
    )
    .await?;
    assert_eq!(kept_env["origin"], "mcp_json", "{kept_env}");
    let result = test(id.clone()).await?;
    assert!(refused(&result) && !marker.exists(), "{result}");

    // A person's command with a person's environment: the Test runs it.
    let taken = put(
        json!({"name": "helper", "transport": "stdio", "command": "/bin/sh",
                           "args": ["-c", touch], "env": {}}),
    )
    .await?;
    assert_eq!(taken["origin"], "person", "{taken}");
    let result = test(id.clone()).await?;
    assert!(!refused(&result), "{result}");
    assert!(marker.exists(), "a person's command is tested");

    // An imported loopback HTTP server is not probed for the agent either.
    let local = server("local")?;
    assert_eq!(local["origin"], "mcp_json", "{local}");
    let result = test(local["id"].as_str().ok_or("id")?.to_owned()).await?;
    assert!(refused(&result), "{result}");

    // Taken over through another transport, the file's hidden environment does not stay behind
    // for a later switch back to a local command (review of SPEC-218).
    let switch = server("switch")?;
    let resp = call(
        &h.router,
        Method::PUT,
        &format!("/api/mcp/{}", switch["id"].as_str().ok_or("id")?),
        Some(json!({"name": "switch", "transport": "http", "url": "https://mcp.example.test/mcp"})),
        &root,
    )
    .await?;
    assert_eq!(resp.status, StatusCode::OK, "{:?}", resp.body);
    assert_eq!(resp.body["origin"], "person", "{}", resp.body);
    assert_eq!(resp.body["env_keys"], json!([]), "{}", resp.body);
    Ok(())
}

/// The run directories of shells and Cockpits on disk.
fn run_dirs(f: &Fixture) -> usize {
    std::fs::read_dir(f.data.join("shells"))
        .map(|entries| entries.count())
        .unwrap_or(0)
}

/// SPEC-077 DRS-001 (#250): the client goes away while its Cockpit's process runs but before it
/// is registered. The start still ends as the one Cockpit: the next open returns it and no second
/// process or orphan run directory is left.
#[tokio::test]
async fn a_cockpit_start_whose_request_is_dropped_still_ends_as_the_one_cockpit() -> TestResult {
    let f = fixture().await?;
    f.gate.arm();
    {
        let open = open_cockpit(&f, &f.alice);
        tokio::pin!(open);
        tokio::select! {
            resp = &mut open => {
                return Err(format!("the open ended before its start was held: {:?}", resp.map(|r| r.status)).into());
            }
            () = f.gate.reached() => {}
        }
    }
    f.gate.release();
    wait_for("the dropped start to register its Cockpit", || {
        f.h.app.sandbox_shells(&sid()).len() == 1
    })
    .await?;
    let started =
        f.h.app
            .sandbox_shells(&sid())
            .first()
            .map(|shell| shell.id().to_owned())
            .ok_or("no Cockpit")?;
    assert_eq!(
        cockpit_id(&f, &f.alice).await?,
        started,
        "the next open finds it"
    );
    assert_eq!(f.h.app.sandbox_shells(&sid()).len(), 1);
    assert_eq!(run_dirs(&f), 1, "no orphan run directory");
    f.h.app.end_all_shells().await;
    Ok(())
}

/// SPEC-077 DRS-003: a signal ending a resumed Cockpit at once (the host or the machine ended it)
/// says nothing about the conversation, which is resumed again next time; only an error exit of
/// Claude Code itself marks it unresumable (SPEC-021 review round 2).
#[tokio::test]
async fn a_resumed_cockpit_killed_by_a_signal_resumes_the_same_conversation() -> TestResult {
    let f = fixture().await?;
    let served = serve(f.h.router.clone()).await?;
    let id = cockpit_id(&f, &f.alice).await?;
    let mut socket = attach(served.addr, &id, &f.alice).await?;
    let mut screen = String::new();
    read_until(&mut socket, &mut screen, "ready").await?;
    let config = PathBuf::from(printed(&screen, "config")?);
    let cwd = printed(&screen, "cwd")?;
    let project = yulius_harness::paths::claude_project_dir(&config, Path::new(&cwd));
    conversation(&project, "kept", 1_000)?;
    socket
        .send(Message::text(
            json!({"type": "input", "data": "exit\r"}).to_string(),
        ))
        .await?;
    wait_for("the Cockpit to end", || {
        f.h.app.sandbox_shells(&sid()).is_empty()
    })
    .await?;

    // The resumed process is killed by a signal right after it started.
    std::fs::write(config.join("kill-resume"), "")?;
    cockpit_id(&f, &f.alice).await?;
    wait_for("the killed resume to end", || {
        f.h.app.sandbox_shells(&sid()).is_empty()
    })
    .await?;
    std::fs::remove_file(config.join("kill-resume"))?;
    let again = cockpit_id(&f, &f.alice).await?;
    let mut socket = attach(served.addr, &again, &f.alice).await?;
    let mut screen = String::new();
    read_until(&mut socket, &mut screen, "ready").await?;
    assert_eq!(
        printed(&screen, "cockpit-args")?,
        "[--resume kept --model claude-opus-5-5 --effort high]",
        "the same conversation"
    );
    f.h.app.end_all_shells().await;
    Ok(())
}

/// SPEC-077 DRS-004, DRS-005: Cockpit starts are serialised per user and sandbox, not
/// instance-wide. While alice's start in `s1` is held, carol's open there and alice's in `s2`
/// return, and her second open in `s1` waits for her start and gets its Cockpit; the start-lock
/// map keeps no entry once the starts are over.
#[tokio::test]
async fn a_blocked_cockpit_start_delays_no_other_user_or_sandbox() -> TestResult {
    let f = fixture().await?;
    seed_sandbox(&f.h, "s2", "other", "u-alice").await?;
    f.h.github.set_perm("alice", "acme/other", perms("write"));
    std::fs::create_dir_all(f.data.join("sandboxes/s2/repo"))?;
    f.gate.arm();
    let held = open_cockpit(&f, &f.alice);
    tokio::pin!(held);
    tokio::select! {
        resp = &mut held => {
            return Err(format!("the open ended before its start was held: {:?}", resp.map(|r| r.status)).into());
        }
        () = f.gate.reached() => {}
    }
    assert_eq!(f.h.app.cockpit_starts_pending(), 1, "alice's start in s1");
    // A second open of hers there (another tab) waits on the same lock and gets the same Cockpit.
    let again = {
        let (router, alice) = (f.h.router.clone(), f.alice.clone());
        tokio::spawn(async move {
            call(
                &router,
                Method::POST,
                "/api/sandboxes/s1/cockpit",
                None,
                &alice,
            )
            .await
            .map_err(|err| err.to_string())
        })
    };

    let carol = tokio::time::timeout(WAIT, cockpit_id(&f, &f.carol))
        .await
        .map_err(|_| "carol's open waited for alice's start")??;
    let other = tokio::time::timeout(
        WAIT,
        call(
            &f.h.router,
            Method::POST,
            "/api/sandboxes/s2/cockpit",
            None,
            &f.alice,
        ),
    )
    .await
    .map_err(|_| "alice's open in another sandbox waited for her start in s1")??;
    assert_eq!(other.status, StatusCode::OK, "{:?}", other.body);

    f.gate.release();
    let alice = tokio::time::timeout(WAIT, &mut held)
        .await
        .map_err(|_| "the held start never ended")??;
    assert_eq!(alice.status, StatusCode::OK, "{:?}", alice.body);
    let again = tokio::time::timeout(WAIT, again)
        .await
        .map_err(|_| "alice's second open never ended")???;
    assert_eq!(again.status, StatusCode::OK, "{:?}", again.body);
    assert_eq!(again.body["id"], alice.body["id"], "one Cockpit for her");
    assert_eq!(
        f.h.app.sandbox_shells(&sid()).len(),
        2,
        "alice's and carol's"
    );
    assert_ne!(alice.body["id"].as_str(), Some(carol.as_str()));
    assert_ne!(alice.body["id"], other.body["id"]);
    assert_eq!(
        f.h.app.cockpit_starts_pending(),
        0,
        "no idle start lock is kept"
    );
    f.h.app.end_all_shells().await;
    Ok(())
}

#[tokio::test]
async fn a_cockpit_that_cannot_start_leaves_nothing_and_starts_once_it_can() -> TestResult {
    let f = fixture().await?;
    // Claude Code's config in the user's home is unreadable: the start fails after the shell was
    // prepared, and nothing of it is left (CKP-007).
    let config = f.data.join("users/u-alice/home/.claude");
    std::fs::create_dir_all(&config)?;
    std::fs::write(config.join(".claude.json"), "[]")?;
    let failed = open_cockpit(&f, &f.alice).await?;
    assert_eq!(failed.status, StatusCode::BAD_GATEWAY, "{:?}", failed.body);
    assert!(f.h.app.sandbox_shells(&sid()).is_empty());
    let left = std::fs::read_dir(f.data.join("shells"))
        .map(|entries| entries.count())
        .unwrap_or(0);
    assert_eq!(left, 0, "no run directory left behind");
    std::fs::write(config.join(".claude.json"), "{}")?;
    cockpit_id(&f, &f.alice).await?;

    // Before the repository is cloned it says so.
    seed_sandbox(&f.h, "s2", "other", "u-alice").await?;
    f.h.github.set_perm("alice", "acme/other", perms("write"));
    let uncloned = call(
        &f.h.router,
        Method::POST,
        "/api/sandboxes/s2/cockpit",
        None,
        &f.alice,
    )
    .await?;
    assert_eq!(uncloned.status, StatusCode::CONFLICT, "{:?}", uncloned.body);
    f.h.app.end_all_shells().await;
    Ok(())
}

/// SPEC-023 DEF-004: the Cockpit runs Opus 5.5, so a subscription whose weekly Opus cap is spent
/// is passed over even when it has the most room otherwise.
#[tokio::test]
async fn the_cockpit_skips_a_subscription_whose_opus_cap_is_spent() -> TestResult {
    let f = fixture_with(false, None).await?;
    let now = f.h.clock.0.load(Ordering::SeqCst);
    let window = |used: f64| json!({"used_percent": used, "window_minutes": 10080, "resets_at": now + 80 * 3_600_000});
    for (id, token, usage) in [
        (
            "acc-a",
            "spent-opus-token",
            json!({"secondary": window(5.0), "weekly_opus": window(100.0)}),
        ),
        ("acc-b", TOKEN, json!({"secondary": window(60.0)})),
    ] {
        f.h.store
            .insert_account(&AccountRecord {
                id: AccountId::parse(id)?,
                provider: Provider::Anthropic,
                label: id.into(),
                credential: Some(f.h.app.deps.crypto.seal(token.as_bytes())?),
                status: AccountStatus::Ok,
                usage_json: Some(usage.to_string()),
                last_error: None,
                created_at: Timestamp(1),
                updated_at: Timestamp(1),
                email: None,
                email_checked_at: None,
            })
            .await?;
    }
    let served = serve(f.h.router.clone()).await?;
    let id = cockpit_id(&f, &f.alice).await?;
    let mut socket = attach(served.addr, &id, &f.alice).await?;
    let mut screen = String::new();
    read_until(&mut socket, &mut screen, "ready").await?;
    assert_eq!(printed(&screen, "token")?, TOKEN);
    f.h.app.end_all_shells().await;
    Ok(())
}

#[tokio::test]
async fn without_capacity_the_cockpit_waits_for_a_subscription() -> TestResult {
    let f = fixture_with(false, None).await?;
    // Signed out: no usable account, so it asks for one to be signed in (SPEC-246 CLA-002).
    f.h.store
        .insert_account(&AccountRecord {
            id: AccountId::parse("acc-1")?,
            provider: Provider::Anthropic,
            label: "acc-1".into(),
            credential: Some(f.h.app.deps.crypto.seal(TOKEN.as_bytes())?),
            status: AccountStatus::Relogin,
            usage_json: None,
            last_error: None,
            created_at: Timestamp(1),
            updated_at: Timestamp(1),
            email: None,
            email_checked_at: None,
        })
        .await?;
    let resp = open_cockpit(&f, &f.alice).await?;
    assert_eq!(resp.status, StatusCode::CONFLICT, "{:?}", resp.body);
    assert_eq!(resp.body["error"]["code"], "claude_account_required");
    assert!(
        resp.body.to_string().contains("must sign in again"),
        "{:?}",
        resp.body
    );
    // Connected but out of capacity: it waits for capacity.
    support::exhausted_subscription(&f.h).await?;
    let resp = open_cockpit(&f, &f.alice).await?;
    assert_eq!(
        resp.status,
        StatusCode::TOO_MANY_REQUESTS,
        "{:?}",
        resp.body
    );
    assert!(f.h.app.sandbox_shells(&sid()).is_empty());
    Ok(())
}

/// Records the Cockpit launch a contained server asks for and refuses it (no bwrap needed).
struct RecordingHarness {
    inner: yulius::wiring::harness::HarnessAdapter,
    cockpit: std::sync::Mutex<Option<CockpitLaunch>>,
}

impl HarnessPort for RecordingHarness {
    fn plan(&self, launch: &HarnessLaunch) -> AppResult<HarnessPlan> {
        self.inner.plan(launch)
    }
    fn plan_cockpit(&self, launch: &CockpitLaunch) -> AppResult<LaunchPlan> {
        if let Ok(mut recorded) = self.cockpit.lock() {
            *recorded = Some(launch.clone());
        }
        Err(AppError::Invalid("recorded".into()))
    }
    fn claude_transcript(&self, home: &Path, worktree: &Path, session: &str) -> PathBuf {
        self.inner.claude_transcript(home, worktree, session)
    }
    fn step_back_transcript(&self, home: &Path, worktree: &Path, session: &str) -> AppResult<bool> {
        self.inner.step_back_transcript(home, worktree, session)
    }
    fn newest_claude_session(
        &self,
        home: &Path,
        worktree: &Path,
        since: Timestamp,
    ) -> AppResult<Option<(String, PathBuf)>> {
        self.inner.newest_claude_session(home, worktree, since)
    }
    fn watch(
        &self,
        harness: yulius_domain::harness::Harness,
        path: &Path,
        from: WatchFrom,
    ) -> AppResult<Box<dyn TranscriptWatch>> {
        self.inner.watch(harness, path, from)
    }
    fn recount(
        &self,
        harness: yulius_domain::harness::Harness,
        sandbox_home: &Path,
        worktree: &Path,
        session_id: &str,
        from: WatchFrom,
        until: Option<yulius_domain::time::Timestamp>,
    ) -> AppResult<Option<yulius_app::ports::harness::UsageSnapshot>> {
        self.inner
            .recount(harness, sandbox_home, worktree, session_id, from, until)
    }
    fn login_argv(&self, harness: yulius_domain::harness::Harness) -> Vec<String> {
        self.inner.login_argv(harness)
    }
    fn extract_claude_token(&self, output: &str) -> Option<String> {
        self.inner.extract_claude_token(output)
    }
    fn masker(&self) -> Box<dyn OutputMasker> {
        self.inner.masker()
    }
    fn install_skills(
        &self,
        skills: &[(String, PathBuf)],
        root: &Path,
        skills_dir: &Path,
    ) -> AppResult<()> {
        self.inner.install_skills(skills, root, skills_dir)
    }
}

/// A contained server whose Cockpit launches are recorded and refused, with alice (write) on `s1`
/// and one pool subscription.
async fn contained_harness()
-> Result<(Harness, Arc<RecordingHarness>, Creds), Box<dyn std::error::Error>> {
    let exe = PathBuf::from(env!("CARGO_BIN_EXE_yulius"));
    let recorder = Arc::new(RecordingHarness {
        inner: yulius::wiring::harness::HarnessAdapter::new(exe, "http://127.0.0.1:9"),
        cockpit: std::sync::Mutex::new(None),
    });
    let h = harness_with(HarnessOptions {
        harness: Some(recorder.clone()),
        configure: Some(Box::new(|config| config.contained = true)),
        ..HarnessOptions::default()
    })?;
    let alice = seed_github_user(&h, "u-alice", "alice").await?;
    seed_sandbox(&h, "s1", "fixture", "u-alice").await?;
    h.github.set_perm("alice", "acme/fixture", perms("write"));
    std::fs::create_dir_all(h.app.config.data_dir.join("sandboxes/s1/repo"))?;
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
            email: None,
            email_checked_at: None,
        })
        .await?;
    Ok((h, recorder, alice))
}

#[tokio::test]
async fn a_contained_cockpit_keeps_its_config_in_the_users_home() -> TestResult {
    let (h, recorder, alice) = contained_harness().await?;
    let data = h.app.config.data_dir.clone();
    let repo = data.join("sandboxes/s1/repo");
    // The conversation Claude Code wrote, seen from the host.
    let home_host = data.join("users/u-alice/home");
    let project = yulius_harness::paths::claude_project_dir(&home_host.join(".claude"), &repo);
    conversation(&project, "earlier", 1_000)?;

    let resp = call(
        &h.router,
        Method::POST,
        "/api/sandboxes/s1/cockpit",
        None,
        &alice,
    )
    .await?;
    assert_eq!(
        resp.status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "{:?}",
        resp.body
    );
    let launch = recorder
        .cockpit
        .lock()
        .map_err(|_| "poisoned")?
        .clone()
        .ok_or("no Cockpit launch planned")?;
    // Inside its containment the session sees its home at /home/<login>; Yulius prepares the
    // config and finds the conversation on the host side.
    assert_eq!(launch.config_dir, PathBuf::from("/home/alice/.claude"));
    assert_eq!(launch.config_dir_host, home_host.join(".claude"));
    assert_eq!(launch.cwd, repo);
    assert_eq!(launch.resume.as_deref(), Some("earlier"));
    // A contained server's Superagent bypasses approvals, as its tasks do (SPEC-208 BYP-001).
    assert!(launch.contained);
    assert!(h.app.sandbox_shells(&sid()).is_empty());
    Ok(())
}

/// SPEC-208 BYP-001, BYP-002: a contained Superagent starts in bypass-permissions mode as root of
/// its sandbox, its person's Claude config records the bypass accepted; uncontained, neither.
#[test]
fn a_contained_superagent_starts_with_the_approval_bypass_accepted() -> TestResult {
    let dir = tempfile::tempdir()?;
    let cwd = dir.path().join("repo");
    std::fs::create_dir_all(&cwd)?;
    let adapter = yulius::wiring::harness::HarnessAdapter::new(
        PathBuf::from(env!("CARGO_BIN_EXE_yulius")),
        "http://127.0.0.1:9",
    );
    let launch = |home: &str, contained: bool| CockpitLaunch {
        cwd: cwd.clone(),
        config_dir: PathBuf::from("/home/alice/.claude"),
        config_dir_host: dir.path().join(home),
        resume: None,
        continue_interrupted: false,
        credential: yulius_app::model::Secret(TOKEN.into()),
        yulius: None,
        project_mcp_servers: Vec::new(),
        mcp: Vec::new(),
        contained,
    };
    let settled = |home: &str| -> Result<Value, Box<dyn std::error::Error>> {
        let path = dir.path().join(home).join(".claude.json");
        Ok(serde_json::from_slice(&std::fs::read(path)?)?)
    };

    let plan = adapter.plan_cockpit(&launch("contained", true))?;
    assert_eq!(
        plan.argv.last().map(String::as_str),
        Some("--dangerously-skip-permissions"),
        "{:?}",
        plan.argv
    );
    assert!(plan.env.contains(&("IS_SANDBOX".into(), "1".into())));
    assert_eq!(settled("contained")?["bypassPermissionsModeAccepted"], true);

    let plain = adapter.plan_cockpit(&launch("plain", false))?;
    assert!(
        !plain
            .argv
            .iter()
            .any(|a| a == "--dangerously-skip-permissions")
    );
    assert!(!plain.env.iter().any(|(k, _)| k == "IS_SANDBOX"));
    assert!(settled("plain")?["bypassPermissionsModeAccepted"].is_null());
    Ok(())
}

/// A JSON-RPC call to the internal `/mcp` with `token`.
async fn mcp(
    f: &Fixture,
    token: &str,
    method: &str,
    params: Value,
) -> Result<Value, Box<dyn std::error::Error>> {
    let resp = call(
        &f.h.internal,
        Method::POST,
        "/mcp",
        Some(json!({"jsonrpc": "2.0", "id": 1, "method": method, "params": params})),
        &Creds {
            bearer: Some(token.into()),
            ..Creds::default()
        },
    )
    .await?;
    assert_eq!(resp.status, StatusCode::OK, "{:?}", resp.body);
    Ok(resp.body)
}

/// A tool's record (or an error's text), and whether it is an error.
async fn cockpit_tool(
    f: &Fixture,
    token: &str,
    name: &str,
    args: Value,
) -> Result<(Value, bool), Box<dyn std::error::Error>> {
    let body = mcp(
        f,
        token,
        "tools/call",
        json!({"name": name, "arguments": args, "_meta": {"yulius/structured": true}}),
    )
    .await?;
    let text = body["result"]["content"][0]["text"]
        .as_str()
        .ok_or_else(|| format!("no result: {body}"))?;
    // The tool's record (asked for with `_meta`), or the text of an error.
    let record = body["result"]
        .get("structuredContent")
        .cloned()
        .unwrap_or_else(|| Value::String(text.to_owned()));
    Ok((record, body["result"]["isError"].as_bool().unwrap_or(true)))
}

/// Waits until `task`'s start has ended (it rests: failed, stopped or closed).
async fn task_rests(
    f: &Fixture,
    task: &yulius_domain::ids::TaskId,
) -> Result<(), Box<dyn std::error::Error>> {
    use yulius_app::ports::store::TaskStore;
    let deadline = tokio::time::Instant::now() + WAIT;
    loop {
        let status = f.h.store.task(task).await?.ok_or("task")?.status;
        if status.is_resting() {
            return Ok(());
        }
        if tokio::time::Instant::now() > deadline {
            return Err(format!("the task did not come to rest: {status:?}").into());
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// AC-MGT-2, MGT-005..007: a Cockpit's own token reaches the management tools, which act for
/// its owner; nothing else is served, and the token ends with the Cockpit and its owner's
/// access.
#[tokio::test]
async fn a_cockpit_manages_the_project_for_its_owner() -> TestResult {
    use yulius_app::model::TaskOrigin;
    use yulius_app::ports::store::TaskStore;
    use yulius_app::ports::store_ext::AutomationStore;
    use yulius_domain::ids::{TaskId, UserId};

    let f = fixture().await?;
    cockpit_id(&f, &f.alice).await?;
    cockpit_id(&f, &f.carol).await?;
    let alice = UserId::parse("u-alice")?;
    let token =
        f.h.app
            .cockpit_mcp_token(&alice, &sid())
            .ok_or("alice's cockpit token")?;
    assert!(token.starts_with("ck_"), "{token}");
    let carols =
        f.h.app
            .cockpit_mcp_token(&UserId::parse("u-carol")?, &sid())
            .ok_or("carol's cockpit token")?;
    assert_ne!(token, carols, "one token per Cockpit");

    // Exactly the management and vault tools (MGT-006, SPEC-186 SAV-002, SAV-003), and the
    // person's other sandboxes (SPEC-335).
    let listed = mcp(&f, &token, "tools/list", json!({})).await?;
    let mut names: Vec<String> = listed["result"]["tools"]
        .as_array()
        .ok_or("tools")?
        .iter()
        .filter_map(|t| t["name"].as_str().map(str::to_owned))
        .collect();
    names.sort();
    assert_eq!(
        names,
        [
            "automation_create",
            "automation_delete",
            "automation_run_now",
            "automation_template_enable",
            "automation_update",
            "automations_list",
            "sandbox_task_create",
            "sandboxes_list",
            "secret_copy_to_env",
            "secret_create_from_file",
            "secret_request_hosts",
            "secret_verify",
            "secrets_list",
            "session_read",
            "sessions_search",
            "task_archive",
            "task_create",
            "task_delete",
            "task_get",
            "task_message",
            "task_stop",
            "tasks_list",
            "workflows_list",
        ]
    );
    let (refused, is_error) =
        cockpit_tool(&f, &token, "steps_set", json!({"steps": ["x"]})).await?;
    assert!(is_error, "{refused}");
    assert!(refused.to_string().contains("unknown tool"), "{refused}");

    // A task for alice (MGT-001, MGT-007).
    let (created, is_error) = cockpit_tool(
        &f,
        &token,
        "task_create",
        json!({"prompt": "Fix the onboarding wording about CI homes"}),
    )
    .await?;
    assert!(!is_error, "{created}");
    let task =
        f.h.store
            .task(&TaskId::parse(
                created["task"]["task_id"].as_str().ok_or("task_id")?,
            )?)
            .await?
            .ok_or("task")?;
    assert_eq!(task.created_by, alice);
    assert_eq!(task.origin, TaskOrigin::Agent);
    let (listed, is_error) = cockpit_tool(&f, &token, "tasks_list", json!({})).await?;
    assert!(!is_error, "{listed}");
    assert!(
        listed.get("you").is_none(),
        "a Cockpit has no task: {listed}"
    );
    assert_eq!(listed["tasks"][0]["task_id"], task.id.as_str());

    // "Every day at 4am align the docs": an automation of alice's.
    let (automation, is_error) = cockpit_tool(
        &f,
        &token,
        "automation_create",
        json!({"name": "Align docs", "cron": "0 4 * * *", "prompt": "Align the docs to the latest changes if they are not aligned"}),
    )
    .await?;
    assert!(!is_error, "{automation}");
    let id = automation["automation"]["id"].as_str().ok_or("id")?;
    let stored = f.h.store.automation(id).await?.ok_or("automation")?;
    assert_eq!(stored.created_by, alice);
    assert_eq!(stored.created_via.as_str(), "agent");
    let audit = f.h.store.automation_audit(id, 10).await?;
    let entry = audit
        .iter()
        .find(|e| e.action == "automation.created")
        .ok_or("audit entry")?;
    assert_eq!(entry.actor, "cockpit:alice");

    // Runs it now, then deletes it, and archives and deletes the task once it ended (no
    // repository here): each audited with the Cockpit (SPEC-066 MGT-010, MGT-011).
    let (ran, is_error) = cockpit_tool(
        &f,
        &token,
        "automation_run_now",
        json!({"automation_id": id}),
    )
    .await?;
    assert!(!is_error, "{ran}");
    let (deleted, is_error) = cockpit_tool(
        &f,
        &token,
        "automation_delete",
        json!({"automation_id": id}),
    )
    .await?;
    assert!(!is_error, "{deleted}");
    assert!(f.h.store.automation(id).await?.is_none());
    // Its start fails in the background, after `task_create` answered (#460: under load the
    // archive came first and was refused while the task was still queued).
    task_rests(&f, &task.id).await?;
    // A task that ended takes no message, and the Superagent is told what to do instead
    // (SPEC-230 TMS-004).
    let (refused, is_error) = cockpit_tool(
        &f,
        &token,
        "task_message",
        json!({"number": task.number, "text": "Use the DO budget"}),
    )
    .await?;
    assert!(is_error, "{refused}");
    assert!(
        refused.to_string().contains("takes no message now"),
        "{refused}"
    );
    let (archived, is_error) =
        cockpit_tool(&f, &token, "task_archive", json!({"number": task.number})).await?;
    assert!(!is_error, "{archived}");
    let (deleted, is_error) =
        cockpit_tool(&f, &token, "task_delete", json!({"number": task.number})).await?;
    assert!(!is_error, "{deleted}");
    assert!(f.h.store.task(&task.id).await?.is_none());
    // A ready-made automation, enabled for alice.
    let (ready, is_error) = cockpit_tool(
        &f,
        &token,
        "automation_template_enable",
        json!({"template": "open-issues"}),
    )
    .await?;
    assert!(!is_error, "{ready}");
    let ready = ready["automation"]["id"].as_str().ok_or("id")?;
    assert_eq!(
        f.h.store
            .automation(ready)
            .await?
            .ok_or("ready-made")?
            .created_by,
        alice
    );
    for (target, action) in [
        (id, "automation.run_now"),
        (id, "automation.deleted"),
        (task.id.as_str(), "task.archived"),
        (task.id.as_str(), "task.deleted"),
        (ready, "automation.created"),
    ] {
        let audit = f.h.store.automation_audit(target, 10).await?;
        let entry = audit
            .iter()
            .find(|e| e.action == action && e.actor == "cockpit:alice")
            .ok_or_else(|| format!("{action}: {audit:?}"))?;
        // The caller's audit detail names the Cockpit (MGT-010).
        assert!(
            entry
                .detail
                .as_deref()
                .is_some_and(|d| d.contains("\"via\":\"cockpit\"")),
            "{action}: {entry:?}"
        );
    }

    // Unknown tokens, and the token of a Cockpit that ended, are refused (MGT-005).
    let unknown = mcp(&f, "ck_0000", "tools/list", json!({})).await?;
    assert_eq!(unknown["error"]["code"], -32001, "{unknown}");
    f.h.app.end_all_shells().await;
    let ended = mcp(&f, &token, "tools/list", json!({})).await?;
    assert_eq!(ended["error"]["code"], -32001, "{ended}");
    Ok(())
}

/// Requests alice's Superagent in `s1` waits on her to answer (SPEC-335).
async fn waiting_requests(f: &Fixture) -> Result<usize, Box<dyn std::error::Error>> {
    let resp = call(
        &f.h.router,
        Method::GET,
        "/api/sandboxes/s1/superagent/requests",
        None,
        &f.alice,
    )
    .await?;
    assert_eq!(resp.status, StatusCode::OK, "{:?}", resp.body);
    Ok(resp.body.as_array().map_or(0, Vec::len))
}

/// SPEC-335 XSR-004: interrupting the Superagent (Claude Code sends `notifications/cancelled`
/// and may keep the HTTP request open) ends its request to start a task in another sandbox:
/// the card goes, a late approval finds nothing, and the call answers at once.
#[tokio::test]
async fn a_cancelled_cross_sandbox_request_ends_at_once() -> TestResult {
    let f = fixture().await?;
    seed_sandbox(&f.h, "s2", "website", "u-alice").await?;
    f.h.github.set_perm("alice", "acme/website", perms("write"));
    cockpit_id(&f, &f.alice).await?;
    let alice = yulius_domain::ids::UserId::parse("u-alice")?;
    let token =
        f.h.app
            .cockpit_mcp_token(&alice, &sid())
            .ok_or("alice's cockpit token")?;
    let internal = f.h.internal.clone();
    let bearer = Creds {
        bearer: Some(token.clone()),
        ..Creds::default()
    };
    let asking = {
        let bearer = bearer.clone();
        tokio::spawn(async move {
            call(
                &internal,
                Method::POST,
                "/mcp",
                Some(
                    json!({"jsonrpc": "2.0", "id": 7, "method": "tools/call", "params": {
                    "name": "sandbox_task_create",
                    "arguments": {"sandbox": "website", "prompt": "Rewrite the README"}}}),
                ),
                &bearer,
            )
            .await
            .map(|resp| resp.body)
            .map_err(|err| err.to_string())
        })
    };
    let deadline = tokio::time::Instant::now() + WAIT;
    while waiting_requests(&f).await? != 1 {
        assert!(
            tokio::time::Instant::now() < deadline,
            "the request never waited"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    // Another id cancels nothing.
    for request_id in [json!(8), json!("7")] {
        let other = call(
            &f.h.internal,
            Method::POST,
            "/mcp",
            Some(json!({"jsonrpc": "2.0", "method": "notifications/cancelled", "params": {"requestId": request_id}})),
            &bearer,
        )
        .await?;
        assert_eq!(other.status, StatusCode::ACCEPTED);
    }
    assert_eq!(waiting_requests(&f).await?, 1);
    let cancelled = call(
        &f.h.internal,
        Method::POST,
        "/mcp",
        Some(json!({"jsonrpc": "2.0", "method": "notifications/cancelled", "params": {"requestId": 7, "reason": "user interrupted"}})),
        &bearer,
    )
    .await?;
    assert_eq!(cancelled.status, StatusCode::ACCEPTED);
    assert_eq!(
        waiting_requests(&f).await?,
        0,
        "the cancelled request is gone"
    );
    let answered = tokio::time::timeout(WAIT, asking).await???;
    let text = answered["result"]["content"][0]["text"]
        .as_str()
        .unwrap_or("");
    assert!(text.contains("cancelled"), "{answered}");
    use yulius_app::ports::store::TaskStore;
    assert!(
        f.h.store
            .tasks(&yulius_domain::ids::SandboxId::parse("s2")?, false)
            .await?
            .is_empty()
    );
    Ok(())
}

/// MGT-005: once its owner can no longer work in the project, a Cockpit's token is refused.
#[tokio::test]
async fn a_cockpit_whose_owner_lost_access_manages_nothing() -> TestResult {
    use yulius_domain::ids::UserId;

    let f = fixture().await?;
    cockpit_id(&f, &f.alice).await?;
    let token =
        f.h.app
            .cockpit_mcp_token(&UserId::parse("u-alice")?, &sid())
            .ok_or("token")?;
    let allowed = mcp(&f, &token, "tools/list", json!({})).await?;
    assert!(allowed["result"]["tools"].is_array(), "{allowed}");
    f.h.github.set_perm("alice", "acme/fixture", perms("read"));
    f.h.clock
        .0
        .fetch_add(f.h.app.config.permission_ttl_ms + 1, Ordering::SeqCst);
    let refused = mcp(&f, &token, "tools/list", json!({})).await?;
    assert_eq!(refused["error"]["code"], -32001, "{refused}");
    f.h.app.end_all_shells().await;
    Ok(())
}

/// MGT-005: a disabled owner's Cockpit token is refused.
#[tokio::test]
async fn a_disabled_owners_cockpit_manages_nothing() -> TestResult {
    use yulius_app::ports::store::UserStore;
    use yulius_domain::ids::UserId;

    let f = fixture().await?;
    cockpit_id(&f, &f.alice).await?;
    let alice = UserId::parse("u-alice")?;
    let token = f.h.app.cockpit_mcp_token(&alice, &sid()).ok_or("token")?;
    f.h.store
        .update_user_flags(&alice, Some(Some(f.h.app.now())), None)
        .await?;
    let refused = mcp(&f, &token, "tools/list", json!({})).await?;
    assert_eq!(refused["error"]["code"], -32001, "{refused}");
    f.h.app.end_all_shells().await;
    Ok(())
}

/// An agent secret's value, referenced by the sandbox's `apify` MCP server (SPEC-147).
const SECRET_VALUE: &str = "apify_synthetic_9f3Qz7LmWc2Rk8Tn";
const SECRET_LABEL: &str = "[secret:APIFY_KEY]";

/// The label as a terminal screen shows it: as wide as the value (SPEC-239 TRM-002).
fn screen_label() -> String {
    format!(
        "{SECRET_LABEL}{}",
        "*".repeat(SECRET_VALUE.len() - SECRET_LABEL.len())
    )
}
/// A literal bearer typed into an MCP server's form (not an agent secret).
const LITERAL_BEARER: &str = "quartio-literal-bearer-5521";

/// alice adds the agent secret `APIFY_KEY` to `s1`.
async fn add_apify_key(f: &Fixture) -> TestResult {
    let resp = call(
        &f.h.router,
        Method::POST,
        "/api/sandboxes/s1/agent-secrets",
        Some(json!({"name": "APIFY_KEY", "hosts": ["api.apify.test"], "value": SECRET_VALUE})),
        &f.alice,
    )
    .await?;
    assert_eq!(resp.status, StatusCode::CREATED, "{:?}", resp.body);
    Ok(())
}

/// A POST to the internal scrub endpoint `what` with `token`.
async fn scrub(
    f: &Fixture,
    token: &str,
    what: &str,
    body: Value,
) -> Result<support::Resp, Box<dyn std::error::Error>> {
    call(
        &f.h.internal,
        Method::POST,
        &format!("/internal/scrub/{what}"),
        Some(body),
        &Creds {
            bearer: Some(token.to_owned()),
            ..Creds::default()
        },
    )
    .await
}

/// SPEC-147 SAM-001/002/004 (#395, ADR-0121): the Superagent gets the sandbox's MCP servers as a
/// task run does, next to `yulius`, their values only in its secret environment, and the task
/// runs' redaction: a value a Bash command prints through its wrapper, or a tool result holds,
/// comes back as `[secret:NAME]`, as does one its screen shows; while Yulius is unreachable its
/// fingerprint file redacts (SPEC-352 SRF-001); without either a command does not run.
#[tokio::test]
async fn the_superagent_gets_the_sandbox_mcp_servers_with_their_secrets_redacted() -> TestResult {
    let f = fixture().await?;
    let served = serve(f.h.router.clone()).await?;
    let internal = serve(f.h.internal.clone()).await?;
    add_apify_key(&f).await?;
    for server in [
        json!({"name": "apify", "transport": "stdio", "command": "npx",
               "args": ["-y", "@apify/actors-mcp-server"],
               "env": {"APIFY_TOKEN": "${secret:APIFY_KEY}"}}),
        json!({"name": "quartio", "transport": "http", "url": "https://mcp.quartio.test/mcp",
               "headers": {"Authorization": format!("Bearer {LITERAL_BEARER}")}}),
    ] {
        let resp = call(
            &f.h.router,
            Method::POST,
            "/api/sandboxes/s1/mcp",
            Some(server),
            &f.alice,
        )
        .await?;
        assert!(resp.status.is_success(), "{:?}", resp.body);
    }

    let id = cockpit_id(&f, &f.alice).await?;
    let mut socket = attach(served.addr, &id, &f.alice).await?;
    let mut screen = String::new();
    read_until(&mut socket, &mut screen, "ready").await?;
    let config = PathBuf::from(printed(&screen, "config")?);
    let mcp_text = std::fs::read_to_string(config.join("launch-mcp-config"))?;
    let mcp_config: Value = serde_json::from_str(&mcp_text)?;
    assert_eq!(
        mcp_config["mcpServers"]["yulius"]["headers"]["Authorization"],
        "Bearer ${YULIUS_COCKPIT_TOKEN}"
    );
    assert_eq!(
        mcp_config["mcpServers"]["apify"],
        json!({"type": "stdio", "command": "npx", "args": ["-y", "@apify/actors-mcp-server"],
               "env": {"APIFY_TOKEN": "${YULIUS_MCP_APIFY_APIFY_TOKEN}"}})
    );
    assert_eq!(
        mcp_config["mcpServers"]["quartio"],
        json!({"type": "http", "url": "https://mcp.quartio.test/mcp",
               "headers": {"Authorization": "Bearer ${YULIUS_MCP_QUARTIO_TOKEN}"}})
    );
    for value in [SECRET_VALUE, LITERAL_BEARER] {
        assert!(!mcp_text.contains(value), "a value in argv: {mcp_text}");
    }
    // The redaction hook is the only hook; the wrapper runs Bash, hooks and stdio servers.
    let settings: Value = serde_json::from_slice(&std::fs::read(config.join("launch-settings"))?)?;
    let hooks = settings["hooks"].as_object().ok_or("hooks")?;
    assert_eq!(hooks.keys().collect::<Vec<_>>(), ["PostToolUse"]);
    let exe = PathBuf::from(env!("CARGO_BIN_EXE_yulius"));
    let hook = format!("'{}' hook claude PostToolUse", exe.display());
    assert_eq!(
        settings["hooks"]["PostToolUse"][0]["hooks"][0]["command"],
        hook
    );
    assert_eq!(
        std::fs::read_to_string(config.join("launch-shell-prefix"))?,
        format!("{} --scrub-exec", exe.display())
    );
    let record = std::fs::read_to_string(f.data.join("shells").join(&id).join("shell.json"))?;
    for value in [SECRET_VALUE, LITERAL_BEARER] {
        assert!(!record.contains(value), "a value in the shell record");
    }

    // Typed into the session as its Bash tool runs a command: the value is in the environment
    // (its length shows) but the output carries the label.
    let url = format!("http://{}", internal.addr);
    let typed = |line: String| json!({"type": "input", "data": line}).to_string();
    socket
        .send(Message::text(typed(format!(
            "YULIUS_URL={url} $CLAUDE_CODE_SHELL_PREFIX 'echo \"bash=<$YULIUS_MCP_APIFY_APIFY_TOKEN> len=${{#YULIUS_MCP_APIFY_APIFY_TOKEN}} bearer=${{#YULIUS_MCP_QUARTIO_TOKEN}}\"'\r"
        ))))
        .await?;
    read_until(&mut socket, &mut screen, &format!("bash=<{SECRET_LABEL}>")).await?;
    read_until(
        &mut socket,
        &mut screen,
        &format!("len={} bearer={}", SECRET_VALUE.len(), LITERAL_BEARER.len()),
    )
    .await?;
    // A tool result holding the value, as an MCP server returns it, through the settings' hook.
    socket
        .send(Message::text(typed(format!(
            "printf '{{\"tool_name\":\"mcp__apify__run\",\"tool_response\":{{\"text\":\"key %s\"}}}}' \"$YULIUS_MCP_APIFY_APIFY_TOKEN\" | YULIUS_URL={url} {hook}; echo ' hook-done'\r"
        ))))
        .await?;
    read_until(&mut socket, &mut screen, "hook-done\r\n").await?;
    let answer = format!("\"updatedToolOutput\":{{\"text\":\"key {SECRET_LABEL}\"}}");
    assert!(screen.contains(&answer), "{screen}");
    // What reaches the screen past the in-session layers (an MCP error Claude Code shows as it
    // came, a hook that let a result through) is redacted as a task's terminal is (VLT-007).
    socket
        .send(Message::text(typed(
            "echo \"raw=<$YULIUS_MCP_APIFY_APIFY_TOKEN>\"\r".into(),
        )))
        .await?;
    read_until(
        &mut socket,
        &mut screen,
        &format!("raw=<{}>", screen_label()),
    )
    .await?;
    // While Yulius restarts (unreachable), its fingerprint file redacts as a task run's does
    // (SPEC-352 SRF-001): the command runs and the value comes back as the label.
    socket
        .send(Message::text(typed(
            "YULIUS_URL=http://127.0.0.1:1 $CLAUDE_CODE_SHELL_PREFIX 'echo \"file=<$YULIUS_MCP_APIFY_APIFY_TOKEN> ran-$((40+2))\"'\r".into(),
        )))
        .await?;
    read_until(
        &mut socket,
        &mut screen,
        &format!("file=<{SECRET_LABEL}> ran-42"),
    )
    .await?;
    // The file is in its run directory, the person's only, and holds hashes, never the value.
    socket
        .send(Message::text(typed(
            // `printf` keeps the key out of the echoed input line.
            "printf 'sc%s:%s\\n' rub \"$YULIUS_SCRUB_FILE\"\r".into(),
        )))
        .await?;
    read_until(&mut socket, &mut screen, "scrub:/").await?;
    let file = PathBuf::from(printed(&screen, "scrub")?);
    assert_eq!(file, f.data.join("shells").join(&id).join("scrub.json"));
    {
        use std::os::unix::fs::PermissionsExt as _;
        assert_eq!(
            std::fs::metadata(&file)?.permissions().mode() & 0o777,
            0o600
        );
    }
    let held = std::fs::read_to_string(&file)?;
    assert!(held.contains("APIFY_KEY"), "{held}");
    assert!(!held.contains(SECRET_VALUE), "a value in the file");
    // Without its fingerprints (Yulius unreachable and no file) a command does not run.
    socket
        .send(Message::text(typed(
            "YULIUS_URL=http://127.0.0.1:1 YULIUS_SCRUB_FILE=/nonexistent $CLAUDE_CODE_SHELL_PREFIX 'echo ran-$((50+2))'; echo \" status=$?\"\r".into(),
        )))
        .await?;
    read_until(&mut socket, &mut screen, " status=126").await?;
    assert!(!screen.contains("ran-52"), "{screen}");
    assert!(
        !screen.contains(SECRET_VALUE),
        "the value reached the screen"
    );
    f.h.app.end_all_shells().await;
    Ok(())
}

/// SPEC-147 SAM-004: a Superagent adopted after a server restart shows its screen redacted, as
/// it was before the restart.
#[tokio::test]
async fn an_adopted_superagent_screen_stays_redacted() -> TestResult {
    let f = fixture().await?;
    let served = serve(f.h.router.clone()).await?;
    add_apify_key(&f).await?;
    let id = cockpit_id(&f, &f.alice).await?;
    let mut socket = attach(served.addr, &id, &f.alice).await?;
    let mut screen = String::new();
    read_until(&mut socket, &mut screen, "ready").await?;
    socket
        .send(Message::text(
            json!({"type": "input", "data": format!("echo shown-{SECRET_VALUE}-end\r")})
                .to_string(),
        ))
        .await?;
    read_until(
        &mut socket,
        &mut screen,
        &format!("shown-{}-end\r\n", screen_label()),
    )
    .await?;
    assert!(!screen.contains(SECRET_VALUE), "{screen}");
    drop(socket);
    f.h.app.stop_workspace_services().await;

    // The restarted server's vault (the test store is in memory) holds the same secret.
    let g = fixture_with(true, Some(f.data.clone())).await?;
    add_apify_key(&g).await?;
    assert_eq!(g.h.app.adopt_shells().await, (1, 0));
    let served = serve(g.h.router.clone()).await?;
    let mut socket = attach(served.addr, &id, &g.alice).await?;
    let mut screen = String::new();
    read_until(
        &mut socket,
        &mut screen,
        &format!("shown-{}-end", screen_label()),
    )
    .await?;
    assert!(!screen.contains(SECRET_VALUE), "{screen}");
    g.h.app.end_all_shells().await;
    Ok(())
}

/// SPEC-147 SAM-003: the redaction in a Superagent loads its fingerprints with the person's
/// workspace token, which its environment already holds: answered while the person may work in
/// the sandbox, refused once they cannot, and a redaction is audited with their login.
#[tokio::test]
async fn a_persons_workspace_token_reads_the_fingerprints_while_they_may_work() -> TestResult {
    let f = fixture().await?;
    let served = serve(f.h.router.clone()).await?;
    add_apify_key(&f).await?;
    let id = cockpit_id(&f, &f.alice).await?;
    let mut socket = attach(served.addr, &id, &f.alice).await?;
    let mut screen = String::new();
    read_until(&mut socket, &mut screen, "ready").await?;
    let workspace = printed(&screen, "workspace")?;
    assert!(workspace.starts_with("ws_"), "{workspace}");

    let answered = scrub(&f, &workspace, "fingerprints", json!({})).await?;
    assert_eq!(answered.status, StatusCode::OK, "{:?}", answered.body);
    assert_eq!(answered.body["names"], json!(["APIFY_KEY"]));
    assert!(!answered.body.to_string().contains(SECRET_VALUE));
    let reported = scrub(
        &f,
        &workspace,
        "report",
        json!({"channel": "bash", "names": ["APIFY_KEY"]}),
    )
    .await?;
    assert_eq!(reported.status, StatusCode::NO_CONTENT);
    let activity = call(
        &f.h.router,
        Method::GET,
        "/api/secrets/activity",
        None,
        &f.alice,
    )
    .await?;
    assert!(
        activity.body.as_array().is_some_and(|a| a
            .iter()
            .any(|e| e["action"] == "vault.agent.redacted" && e["actor"] == "alice")),
        "{:?}",
        activity.body
    );

    assert_eq!(
        scrub(&f, "ws_0000", "fingerprints", json!({}))
            .await?
            .status,
        StatusCode::UNAUTHORIZED
    );
    // Every Superagent command and tool result asks, within the client's 2 s: a decision on the
    // person's access within the staleness bound answers without asking GitHub again, even
    // while GitHub hangs (#395 review).
    f.h.clock
        .0
        .fetch_add(f.h.app.config.permission_ttl_ms + 1, Ordering::SeqCst);
    f.h.github.hang_permissions.store(true, Ordering::SeqCst);
    let quick = tokio::time::timeout(
        Duration::from_secs(2),
        scrub(&f, &workspace, "fingerprints", json!({})),
    )
    .await
    .map_err(|_| "the fingerprints waited on GitHub")??;
    assert_eq!(quick.status, StatusCode::OK);
    f.h.github.hang_permissions.store(false, Ordering::SeqCst);
    // Past the staleness bound GitHub decides again: a person who lost write access reads and
    // reports nothing.
    f.h.github.set_perm("alice", "acme/fixture", perms("read"));
    f.h.clock
        .0
        .fetch_add(f.h.app.config.permission_stale_ms, Ordering::SeqCst);
    assert_eq!(
        scrub(&f, &workspace, "fingerprints", json!({}))
            .await?
            .status,
        StatusCode::UNAUTHORIZED,
        "a person who lost write access reads nothing"
    );
    let report = json!({"channel": "tool", "names": ["APIFY_KEY"]});
    assert_eq!(
        scrub(&f, &workspace, "report", report).await?.status,
        StatusCode::UNAUTHORIZED
    );
    f.h.app.end_all_shells().await;
    Ok(())
}

/// SPEC-147 SAM-003: a disabled person's workspace token reads no fingerprints.
#[tokio::test]
async fn a_disabled_persons_workspace_token_reads_no_fingerprints() -> TestResult {
    use yulius_app::ports::store::UserStore;
    use yulius_domain::ids::UserId;

    let f = fixture().await?;
    let served = serve(f.h.router.clone()).await?;
    add_apify_key(&f).await?;
    let id = cockpit_id(&f, &f.alice).await?;
    let mut socket = attach(served.addr, &id, &f.alice).await?;
    let mut screen = String::new();
    read_until(&mut socket, &mut screen, "ready").await?;
    let workspace = printed(&screen, "workspace")?;
    f.h.store
        .update_user_flags(&UserId::parse("u-alice")?, Some(Some(f.h.app.now())), None)
        .await?;
    assert_eq!(
        scrub(&f, &workspace, "fingerprints", json!({}))
            .await?
            .status,
        StatusCode::UNAUTHORIZED
    );
    f.h.app.end_all_shells().await;
    Ok(())
}

// SPEC-074: a usage limit in the Cockpit moves it to a subscription with capacity, as a task's.

/// The test clock's start (2026-10-07T00:00:00Z) plus `secs`, as Claude Code writes `timestamp`.
fn at(secs: i64) -> String {
    let base = 1_791_331_200_i64;
    let time = base + secs;
    let (days, rest) = (time.div_euclid(86_400), time.rem_euclid(86_400));
    // 2026-10-07 is day 20_733 after the epoch; the records stay within a few days of it.
    let day = 7 + (days - 20_733);
    format!(
        "2026-10-{day:02}T{:02}:{:02}:{:02}.000Z",
        rest / 3_600,
        rest % 3_600 / 60,
        rest % 60
    )
}

fn transcript_record(
    kind: &str,
    uuid: &str,
    parent: Option<&str>,
    secs: i64,
    extra: &Value,
) -> String {
    let mut record = json!({"type": kind, "uuid": uuid, "parentUuid": parent, "isSidechain": false,
        "sessionId": "conv-1", "timestamp": at(secs)});
    if let (Some(map), Some(more)) = (record.as_object_mut(), extra.as_object()) {
        map.extend(more.clone());
    }
    format!("{record}\n")
}

fn prompt_record(uuid: &str, parent: Option<&str>, secs: i64) -> String {
    transcript_record(
        "user",
        uuid,
        parent,
        secs,
        &json!({"message": {"role": "user", "content": "why?"}, "origin": {"kind": "human"}}),
    )
}

fn answer_record(uuid: &str, parent: &str, secs: i64) -> String {
    transcript_record(
        "assistant",
        uuid,
        Some(parent),
        secs,
        &json!({"message": {"id": format!("msg_{uuid}"), "role": "assistant", "model": "claude-opus-5-5",
            "stop_reason": "end_turn", "content": [{"type": "text", "text": "because"}]}}),
    )
}

/// Claude Code 2.1.292's usage-limit error record, resetting at `resets` (epoch seconds).
fn limit_record(uuid: &str, parent: &str, secs: i64, resets: i64) -> String {
    transcript_record(
        "assistant",
        uuid,
        Some(parent),
        secs,
        &json!({"isApiErrorMessage": true, "error": "rate_limit", "apiError": "usage_limit_reached",
            "apiErrorStatus": 429, "apiErrorParams": {"rate_limit_info": {"status": "rejected",
                "resetsAt": resets, "rateLimitType": "seven_day"}},
            "message": {"id": format!("msg_{uuid}"), "role": "assistant", "model": "<synthetic>",
                "content": [{"type": "text", "text": "You've hit your weekly limit · resets 4am"}]}}),
    )
}

async fn insert_account(h: &Harness, id: &str, token: &str, used: f64) -> TestResult {
    let now = h.clock.0.load(Ordering::SeqCst);
    let window =
        json!({"used_percent": used, "window_minutes": 10080, "resets_at": now + 80 * 3_600_000});
    h.store
        .insert_account(&AccountRecord {
            id: AccountId::parse(id)?,
            provider: Provider::Anthropic,
            label: id.into(),
            credential: Some(h.app.deps.crypto.seal(token.as_bytes())?),
            status: AccountStatus::Ok,
            usage_json: Some(json!({"secondary": window}).to_string()),
            last_error: None,
            created_at: Timestamp(1),
            updated_at: Timestamp(1),
            email: None,
            email_checked_at: None,
        })
        .await?;
    Ok(())
}

/// The stored usage of `account`.
async fn usage_of(h: &Harness, account: &str) -> Result<Value, Box<dyn std::error::Error>> {
    let record = h
        .store
        .account(&AccountId::parse(account)?)
        .await?
        .ok_or("account")?;
    Ok(serde_json::from_str(
        record.usage_json.as_deref().unwrap_or("{}"),
    )?)
}

/// Reads the socket until a JSON message of `kind` arrives.
async fn read_message(
    socket: &mut Socket,
    kind: &str,
) -> Result<Value, Box<dyn std::error::Error>> {
    let deadline = tokio::time::Instant::now() + WAIT;
    loop {
        let message = tokio::time::timeout_at(deadline, socket.next())
            .await
            .map_err(|_| format!("timed out waiting for a {kind} message"))?
            .ok_or("socket closed")??;
        if let Message::Text(text) = message {
            let value: Value = serde_json::from_str(&text)?;
            if value["type"] == kind {
                return Ok(value);
            }
        }
    }
}

/// The live Cockpit other than `old`, once there is one.
async fn other_cockpit(f: &Fixture, old: &str) -> Result<String, Box<dyn std::error::Error>> {
    let mut found = None;
    wait_for("the Cockpit on another subscription", || {
        found =
            f.h.app
                .sandbox_shells(&sid())
                .into_iter()
                .find(|s| s.id() != old)
                .map(|s| s.id().to_owned());
        found.is_some()
    })
    .await?;
    found.ok_or_else(|| "no other Cockpit".into())
}

/// The Claude config dir of the Cockpit whose screen is `screen`.
fn project_config(screen: &str) -> Result<PathBuf, Box<dyn std::error::Error>> {
    Ok(PathBuf::from(printed(screen, "config")?))
}

/// The conversation of the Cockpit whose screen is `screen`: its project directory.
fn project_of(screen: &str) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let config = PathBuf::from(printed(screen, "config")?);
    let cwd = printed(screen, "cwd")?;
    Ok(yulius_harness::paths::claude_project_dir(
        &config,
        Path::new(&cwd),
    ))
}

/// A conversation that hit the weekly limit a day before the Cockpit started and went on, then
/// hits it again now (resetting at `resets`).
fn limited_conversation(resets: i64) -> String {
    [
        prompt_record("u0", None, -86_400),
        limit_record("old-limit", "u0", -86_399, 1_791_000_000),
        answer_record("a0", "old-limit", -86_000),
        prompt_record("u1", Some("a0"), 5),
        limit_record("new-limit", "u1", 6, resets),
    ]
    .concat()
}

#[tokio::test]
async fn a_usage_limit_moves_the_cockpit_to_a_subscription_with_capacity() -> TestResult {
    let f = fixture_with(false, None).await?;
    // acc-a has the most room, so the Cockpit starts there.
    insert_account(&f.h, "acc-a", "token-a", 5.0).await?;
    insert_account(&f.h, "acc-b", TOKEN, 60.0).await?;
    let served = serve(f.h.router.clone()).await?;
    let id = cockpit_id(&f, &f.alice).await?;
    let mut socket = attach(served.addr, &id, &f.alice).await?;
    let mut screen = String::new();
    read_until(&mut socket, &mut screen, "ready").await?;
    assert_eq!(printed(&screen, "token")?, "token-a");
    // SUP-001: the shell record keeps its subscription (and still no token).
    let record = std::fs::read_to_string(f.data.join("shells").join(&id).join("shell.json"))?;
    assert!(record.contains("\"account\":\"acc-a\""), "{record}");

    // The person asks; acc-a's weekly window is full (SUP-002). The limit of a day ago, before
    // this process started, is history.
    let project = project_of(&screen)?;
    std::fs::create_dir_all(&project)?;
    let transcript = project.join("conv-1.jsonl");
    std::fs::write(&transcript, limited_conversation(1_791_400_000))?;

    // SUP-004: the viewer is told the session moved, and a Cockpit on acc-b continues the turn.
    let exit = read_message(&mut socket, "exit").await?;
    assert_eq!(exit["status"], "switched", "{exit}");
    let again = other_cockpit(&f, &id).await?;
    let mut socket = attach(served.addr, &again, &f.alice).await?;
    let mut screen = String::new();
    read_until(&mut socket, &mut screen, "ready").await?;
    assert_eq!(
        printed(&screen, "cockpit-args")?,
        "[--resume conv-1 --model claude-opus-5-5 --effort high]",
        "the same conversation, nothing typed"
    );
    assert_eq!(printed(&screen, "interrupted")?, "1");
    assert_eq!(printed(&screen, "token")?, TOKEN);
    assert_eq!(cockpit_id(&f, &f.alice).await?, again, "the panel finds it");
    // Stepped back over the new limit only (SPEC-034 LIM-001).
    let stepped = std::fs::read_to_string(&transcript)?;
    assert!(!stepped.contains("new-limit"), "{stepped}");
    assert!(
        stepped.contains("old-limit") && stepped.contains("\"u1\""),
        "{stepped}"
    );
    // SUP-003: acc-a is recorded as limited until the new limit's reset, not the old one's.
    let usage = usage_of(&f.h, "acc-a").await?;
    assert_eq!(usage["limit_reached"], true, "{usage}");
    assert_eq!(usage["limit_resets_at"], 1_791_400_000_000_i64, "{usage}");
    let record = std::fs::read_to_string(f.data.join("shells").join(&again).join("shell.json"))?;
    assert!(record.contains("\"account\":\"acc-b\""), "{record}");
    f.h.app.end_all_shells().await;
    Ok(())
}

/// SPEC-146 SHE-002 (#346): a usage limit in a Superagent whose process cannot be ended (the end
/// request is lost and `terminate` fails) starts no second one on the same conversation: the old
/// one stays live on its subscription, the panel finds it, and its viewer is told why.
#[tokio::test]
async fn a_superagent_whose_host_cannot_be_ended_is_not_moved() -> TestResult {
    let f = fixture_ending(false, None, Some(500)).await?;
    insert_account(&f.h, "acc-a", "token-a", 5.0).await?;
    insert_account(&f.h, "acc-b", TOKEN, 60.0).await?;
    let served = serve(f.h.router.clone()).await?;
    let id = cockpit_id(&f, &f.alice).await?;
    let mut socket = attach(served.addr, &id, &f.alice).await?;
    let mut screen = String::new();
    read_until(&mut socket, &mut screen, "ready").await?;
    assert_eq!(printed(&screen, "token")?, "token-a");

    f.gate.refuse_ends(true);
    let project = project_of(&screen)?;
    std::fs::create_dir_all(&project)?;
    std::fs::write(
        project.join("conv-1.jsonl"),
        limited_conversation(1_791_400_000),
    )?;
    let notice = loop {
        let notice = read_message(&mut socket, "notice").await?;
        if notice["text"]
            .as_str()
            .is_some_and(|t| t.contains("could not move"))
        {
            break notice;
        }
    };
    assert_eq!(
        notice["text"],
        "The Superagent could not move to another subscription: its process could not be \
         ended, so it stays on this one."
    );
    // The panel's start waits for the move's start lock: the move is over when it answers.
    assert_eq!(cockpit_id(&f, &f.alice).await?, id, "the same Superagent");
    let live: Vec<String> =
        f.h.app
            .sandbox_shells(&sid())
            .into_iter()
            .filter(|shell| !shell.has_ended())
            .map(|shell| shell.id().to_owned())
            .collect();
    assert_eq!(live, vec![id.clone()], "no second Superagent");
    // SUP-003 still holds: acc-a is recorded limited.
    let usage = usage_of(&f.h, "acc-a").await?;
    assert_eq!(usage["limit_reached"], true, "{usage}");
    socket
        .send(Message::text(
            json!({"type": "input", "data": "echo kept-$((3+4))\r"}).to_string(),
        ))
        .await?;
    read_until(&mut socket, &mut screen, "kept-7").await?;
    f.gate.refuse_ends(false);
    f.h.app.end_all_shells().await;
    Ok(())
}

#[tokio::test]
async fn without_capacity_a_limit_leaves_the_cockpit_where_it_is_and_says_why() -> TestResult {
    let f = fixture().await?;
    let served = serve(f.h.router.clone()).await?;
    let id = cockpit_id(&f, &f.alice).await?;
    let mut socket = attach(served.addr, &id, &f.alice).await?;
    let mut screen = String::new();
    read_until(&mut socket, &mut screen, "ready").await?;
    let project = project_of(&screen)?;
    std::fs::create_dir_all(&project)?;
    std::fs::write(
        project.join("conv-1.jsonl"),
        limited_conversation(1_791_400_000),
    )?;

    // SUP-005: no other subscription; the Cockpit stays, and its viewers learn why.
    let notice = read_message(&mut socket, "notice").await?;
    assert!(
        notice["text"]
            .as_str()
            .is_some_and(|t| t.contains("No other subscription has capacity")),
        "{notice}"
    );
    let usage = usage_of(&f.h, "acc-1").await?;
    assert_eq!(usage["limit_reached"], true, "{usage}");
    assert_eq!(cockpit_id(&f, &f.alice).await?, id, "the same session");
    socket
        .send(Message::text(
            json!({"type": "input", "data": "echo still-$((2+2))\r"}).to_string(),
        ))
        .await?;
    read_until(&mut socket, &mut screen, "still-4").await?;
    f.h.app.end_all_shells().await;
    Ok(())
}

#[tokio::test]
async fn an_adopted_cockpit_keeps_its_subscription_and_moves_past_a_limit() -> TestResult {
    let f = fixture_with(false, None).await?;
    insert_account(&f.h, "acc-a", "token-a", 5.0).await?;
    insert_account(&f.h, "acc-b", TOKEN, 60.0).await?;
    let served = serve(f.h.router.clone()).await?;
    let id = cockpit_id(&f, &f.alice).await?;
    let mut socket = attach(served.addr, &id, &f.alice).await?;
    let mut screen = String::new();
    read_until(&mut socket, &mut screen, "ready").await?;
    let project = project_of(&screen)?;
    drop(socket);
    f.h.app.stop_workspace_services().await;

    // A server restart (SUP-001): the Cockpit is adopted on acc-a and watched again.
    let g = fixture_with(false, Some(f.data.clone())).await?;
    insert_account(&g.h, "acc-a", "token-a", 5.0).await?;
    insert_account(&g.h, "acc-b", TOKEN, 60.0).await?;
    assert_eq!(g.h.app.adopt_shells().await, (1, 0));
    std::fs::create_dir_all(&project)?;
    std::fs::write(
        project.join("conv-1.jsonl"),
        limited_conversation(1_791_400_000),
    )?;
    let again = other_cockpit(&g, &id).await?;
    let usage = usage_of(&g.h, "acc-a").await?;
    assert_eq!(
        usage["limit_reached"], true,
        "the adopted Cockpit's own subscription: {usage}"
    );
    let served = serve(g.h.router.clone()).await?;
    let mut socket = attach(served.addr, &again, &g.alice).await?;
    let mut screen = String::new();
    read_until(&mut socket, &mut screen, "ready").await?;
    assert_eq!(printed(&screen, "token")?, TOKEN);
    assert_eq!(printed(&screen, "interrupted")?, "1");
    g.h.app.end_all_shells().await;
    Ok(())
}

/// Review round 1 (M1): a limit already handled is never acted on again, after `/clear` and back
/// to its conversation (`/resume`), or after a server restart.
#[tokio::test]
async fn a_handled_limit_is_not_acted_on_again_after_clear_and_resume() -> TestResult {
    let f = fixture().await?;
    let served = serve(f.h.router.clone()).await?;
    let id = cockpit_id(&f, &f.alice).await?;
    let mut socket = attach(served.addr, &id, &f.alice).await?;
    let mut screen = String::new();
    read_until(&mut socket, &mut screen, "ready").await?;
    let project = project_of(&screen)?;
    std::fs::create_dir_all(&project)?;
    let first = project.join("conv-1.jsonl");
    std::fs::write(&first, limited_conversation(1_791_400_000))?;
    read_message(&mut socket, "notice").await?;
    // `/clear`: a new conversation, limited too (the watch has moved to it once this is told).
    let second = project.join("conv-2.jsonl");
    std::fs::write(
        &second,
        [
            prompt_record("v1", None, 10),
            limit_record("second-limit", "v1", 11, 1_791_500_000),
        ]
        .concat(),
    )?;
    read_message(&mut socket, "notice").await?;
    // `/resume` back to the first one, which is limited again: only the new limit counts.
    let mut text = std::fs::read_to_string(&first)?;
    text.push_str(&prompt_record("u2", Some("new-limit"), 20));
    text.push_str(&limit_record("third-limit", "u2", 21, 1_791_600_000));
    std::fs::write(&first, text)?;
    let resumed = std::fs::File::options().append(true).open(&first)?;
    resumed.set_modified(std::time::SystemTime::now() + Duration::from_secs(60))?;
    read_message(&mut socket, "notice").await?;
    let usage = usage_of(&f.h, "acc-1").await?;
    assert_eq!(usage["limit_resets_at"], 1_791_600_000_000_i64, "{usage}");
    f.h.app.end_all_shells().await;
    Ok(())
}

#[tokio::test]
async fn a_handled_limit_is_not_acted_on_again_after_a_restart() -> TestResult {
    let f = fixture_with(false, None).await?;
    insert_account(&f.h, "acc-a", "token-a", 5.0).await?;
    let served = serve(f.h.router.clone()).await?;
    let id = cockpit_id(&f, &f.alice).await?;
    let mut socket = attach(served.addr, &id, &f.alice).await?;
    let mut screen = String::new();
    read_until(&mut socket, &mut screen, "ready").await?;
    let project = project_of(&screen)?;
    std::fs::create_dir_all(&project)?;
    let transcript = project.join("conv-1.jsonl");
    // No other subscription: the Cockpit stays with the limit in its conversation.
    std::fs::write(&transcript, limited_conversation(1_791_400_000))?;
    read_message(&mut socket, "notice").await?;
    drop(socket);
    f.h.app.stop_workspace_services().await;

    // While the server is down, the person goes on after the reset and is limited again.
    let mut text = std::fs::read_to_string(&transcript)?;
    text.push_str(&prompt_record("u2", Some("new-limit"), 20));
    text.push_str(&limit_record("later-limit", "u2", 21, 1_791_600_000));
    std::fs::write(&transcript, text)?;
    let g = fixture_with(false, Some(f.data.clone())).await?;
    insert_account(&g.h, "acc-a", "token-a", 5.0).await?;
    insert_account(&g.h, "acc-b", TOKEN, 60.0).await?;
    assert_eq!(g.h.app.adopt_shells().await, (1, 0));
    // Only the later limit counts: acc-a is recorded with its reset, and the Cockpit moves.
    other_cockpit(&g, &id).await?;
    let usage = usage_of(&g.h, "acc-a").await?;
    assert_eq!(usage["limit_resets_at"], 1_791_600_000_000_i64, "{usage}");
    g.h.app.end_all_shells().await;
    Ok(())
}

/// Review round 1 (m3): the start on another subscription failed after the old Cockpit ended;
/// the next start still continues the cut turn instead of leaving the prompt unanswered.
#[tokio::test]
async fn a_failed_move_still_continues_the_cut_turn_at_the_next_start() -> TestResult {
    let f = fixture_with(false, None).await?;
    insert_account(&f.h, "acc-a", "token-a", 5.0).await?;
    insert_account(&f.h, "acc-b", TOKEN, 60.0).await?;
    let served = serve(f.h.router.clone()).await?;
    let id = cockpit_id(&f, &f.alice).await?;
    let mut socket = attach(served.addr, &id, &f.alice).await?;
    let mut screen = String::new();
    read_until(&mut socket, &mut screen, "ready").await?;
    let config = PathBuf::from(printed(&screen, "config")?);
    let project = project_of(&screen)?;
    // Claude Code's config turns unreadable: the start on acc-b fails.
    std::fs::write(config.join(".claude.json"), "[]")?;
    std::fs::create_dir_all(&project)?;
    let transcript = project.join("conv-1.jsonl");
    std::fs::write(&transcript, limited_conversation(1_791_400_000))?;
    let exit = read_message(&mut socket, "exit").await?;
    assert_eq!(exit["status"], "switched", "{exit}");
    wait_for("the old Cockpit to be gone", || {
        f.h.app.sandbox_shells(&sid()).is_empty()
    })
    .await?;

    std::fs::write(config.join(".claude.json"), "{}")?;
    let again = cockpit_id(&f, &f.alice).await?;
    let mut socket = attach(served.addr, &again, &f.alice).await?;
    let mut screen = String::new();
    read_until(&mut socket, &mut screen, "ready").await?;
    assert_eq!(
        printed(&screen, "cockpit-args")?,
        "[--resume conv-1 --model claude-opus-5-5 --effort high]"
    );
    assert_eq!(
        printed(&screen, "interrupted")?,
        "1",
        "the cut turn goes on"
    );
    assert_eq!(printed(&screen, "token")?, TOKEN);
    assert!(!std::fs::read_to_string(&transcript)?.contains("new-limit"));
    f.h.app.end_all_shells().await;
    Ok(())
}

/// Review round 1 (m5): a resumed Cockpit moved again right after it started is not a failed
/// resume: its conversation stays the one the next Cockpit resumes (CKP-003).
#[tokio::test]
async fn a_quick_second_move_keeps_resuming_the_conversation() -> TestResult {
    let f = fixture_with(false, None).await?;
    insert_account(&f.h, "acc-a", "token-a", 5.0).await?;
    insert_account(&f.h, "acc-b", "token-b", 30.0).await?;
    insert_account(&f.h, "acc-c", TOKEN, 60.0).await?;
    let served = serve(f.h.router.clone()).await?;
    let id = cockpit_id(&f, &f.alice).await?;
    let mut socket = attach(served.addr, &id, &f.alice).await?;
    let mut screen = String::new();
    read_until(&mut socket, &mut screen, "ready").await?;
    let project = project_of(&screen)?;
    // The next Cockpits end with an error exit on a signal, as Claude Code itself may.
    std::fs::write(project_config(&screen)?.join("exit-3-on-signal"), "")?;
    std::fs::create_dir_all(&project)?;
    let transcript = project.join("conv-1.jsonl");
    std::fs::write(&transcript, limited_conversation(1_791_400_000))?;
    let second = other_cockpit(&f, &id).await?;
    let mut socket = attach(served.addr, &second, &f.alice).await?;
    let mut screen = String::new();
    read_until(&mut socket, &mut screen, "ready").await?;
    assert_eq!(printed(&screen, "token")?, "token-b");

    // acc-b is limited too, at once: the resumed Cockpit ends within its first seconds.
    let mut text = std::fs::read_to_string(&transcript)?;
    text.push_str(&limit_record("b-limit", "u1", 30, 1_791_500_000));
    std::fs::write(&transcript, text)?;
    let third = loop {
        let found = other_cockpit(&f, &second).await?;
        if found != id {
            break found;
        }
    };
    let mut socket = attach(served.addr, &third, &f.alice).await?;
    let mut screen = String::new();
    read_until(&mut socket, &mut screen, "ready").await?;
    assert_eq!(printed(&screen, "token")?, TOKEN);
    assert_eq!(
        printed(&screen, "cockpit-args")?,
        "[--resume conv-1 --model claude-opus-5-5 --effort high]"
    );
    // Opened again later, it still resumes the conversation.
    socket
        .send(Message::text(
            json!({"type": "input", "data": "exit\r"}).to_string(),
        ))
        .await?;
    wait_for("the Cockpit to end", || {
        f.h.app.sandbox_shells(&sid()).is_empty()
    })
    .await?;
    let fourth = cockpit_id(&f, &f.alice).await?;
    let mut socket = attach(served.addr, &fourth, &f.alice).await?;
    let mut screen = String::new();
    read_until(&mut socket, &mut screen, "ready").await?;
    assert!(
        printed(&screen, "cockpit-args")?.starts_with("[--resume conv-1"),
        "{screen}"
    );
    f.h.app.end_all_shells().await;
    Ok(())
}

/// Review round 2: several new limits read at once count as one, the newest: its reset is what
/// the subscription is recorded with.
#[tokio::test]
async fn limits_read_together_count_once_as_the_newest() -> TestResult {
    let f = fixture().await?;
    let served = serve(f.h.router.clone()).await?;
    let id = cockpit_id(&f, &f.alice).await?;
    let mut socket = attach(served.addr, &id, &f.alice).await?;
    let mut screen = String::new();
    read_until(&mut socket, &mut screen, "ready").await?;
    let project = project_of(&screen)?;
    std::fs::create_dir_all(&project)?;
    let mut text = limited_conversation(1_791_400_000);
    text.push_str(&prompt_record("u2", Some("new-limit"), 7));
    text.push_str(&limit_record("newer-limit", "u2", 8, 1_791_450_000));
    std::fs::write(project.join("conv-1.jsonl"), text)?;
    read_message(&mut socket, "notice").await?;
    let usage = usage_of(&f.h, "acc-1").await?;
    assert_eq!(usage["limit_resets_at"], 1_791_450_000_000_i64, "{usage}");
    f.h.app.end_all_shells().await;
    Ok(())
}

/// SUP-002: a limit recorded before the Cockpit started is history even while its reset is ahead;
/// only the person's next limit moves it.
#[tokio::test]
async fn a_limit_from_before_the_start_does_not_move_the_cockpit() -> TestResult {
    let f = fixture_with(false, None).await?;
    insert_account(&f.h, "acc-a", "token-a", 5.0).await?;
    insert_account(&f.h, "acc-b", TOKEN, 60.0).await?;
    // Yesterday's weekly limit, still in force, in the conversation the Cockpit resumes.
    let config = f.data.join("users/u-alice/home/.claude");
    let repo = std::fs::canonicalize(f.data.join("sandboxes/s1/repo"))?;
    let project = yulius_harness::paths::claude_project_dir(&config, &repo);
    std::fs::create_dir_all(&project)?;
    std::fs::write(
        project.join("conv-1.jsonl"),
        [
            prompt_record("u0", None, -86_400),
            limit_record("old-limit", "u0", -86_399, 1_791_500_000),
        ]
        .concat(),
    )?;
    let served = serve(f.h.router.clone()).await?;
    let id = cockpit_id(&f, &f.alice).await?;
    let mut socket = attach(served.addr, &id, &f.alice).await?;
    let mut screen = String::new();
    read_until(&mut socket, &mut screen, "ready").await?;
    assert_eq!(printed(&screen, "token")?, "token-a");

    // The next prompt is limited: that one moves it, from acc-a.
    let mut text = std::fs::read_to_string(project.join("conv-1.jsonl"))?;
    text.push_str(&prompt_record("u1", Some("old-limit"), 5));
    text.push_str(&limit_record("new-limit", "u1", 6, 1_791_400_000));
    std::fs::write(project.join("conv-1.jsonl"), text)?;
    let exit = read_message(&mut socket, "exit").await?;
    assert_eq!(exit["status"], "switched", "{exit}");
    other_cockpit(&f, &id).await?;
    let usage = usage_of(&f.h, "acc-a").await?;
    assert_eq!(usage["limit_resets_at"], 1_791_400_000_000_i64, "{usage}");
    f.h.app.end_all_shells().await;
    Ok(())
}

/// SPEC-178 VAU-001 (ADR-0144), SPEC-186 SAV-002..004, SAV-007 (ADR-0150): the Superagent lists
/// its own sandbox's agent secrets, names, hosts and placeholders only, as a task's agent does,
/// and is told how it uses and checks them; it is served the vault tools a task's agent is, but
/// not the project browser's.
#[tokio::test]
async fn the_superagent_lists_its_sandboxs_agent_secrets_without_values() -> TestResult {
    use yulius_domain::ids::UserId;

    let f = fixture().await?;
    add_apify_key(&f).await?;
    seed_sandbox(&f.h, "s2", "other", "u-alice").await?;
    f.h.github.set_perm("alice", "acme/other", perms("write"));
    let other = call(
        &f.h.router,
        Method::POST,
        "/api/sandboxes/s2/agent-secrets",
        Some(json!({"name": "OTHER_KEY", "value": "other_synthetic_value_4471"})),
        &f.alice,
    )
    .await?;
    assert_eq!(other.status, StatusCode::CREATED, "{:?}", other.body);
    cockpit_id(&f, &f.alice).await?;
    let token =
        f.h.app
            .cockpit_mcp_token(&UserId::parse("u-alice")?, &sid())
            .ok_or("token")?;

    let (listed, is_error) = cockpit_tool(&f, &token, "secrets_list", json!({})).await?;
    assert!(!is_error, "{listed}");
    assert_eq!(listed["secrets"][0]["name"], "APIFY_KEY", "{listed}");
    assert_eq!(listed["secrets"][0]["hosts"], json!(["api.apify.test"]));
    assert_eq!(
        listed["secrets"].as_array().map(Vec::len),
        Some(1),
        "{listed}"
    );
    // Behind the proxy as a task run is (SAV-001): the placeholder and what the proxy adds.
    assert_eq!(
        listed["secrets"][0]["placeholder"],
        "yulius-secret-APIFY_KEY"
    );
    assert!(listed["secrets"][0]["adds"].is_string(), "{listed}");
    let raw = listed.to_string();
    for hidden in [SECRET_VALUE, "OTHER_KEY", "other_synthetic_value_4471"] {
        assert!(!raw.contains(hidden), "{hidden} in {raw}");
    }
    // The Superagent's own guidance (SAV-007): bound hosts through the proxy, `$NAME`, the
    // repository's `.env`, host requests, `secret_verify`, and the Secrets page.
    let how = listed["how_to_use"].to_string();
    for said in [
        "secret_verify",
        "secret_copy_to_env",
        "secret_request_hosts",
        "secret_create_from_file",
        "$NAME",
        "/s/fixture/secrets?section=agents",
    ] {
        assert!(how.contains(said), "{said}: {how}");
    }
    assert!(!how.contains("browser_fill_secret"), "{how}");

    let tools = mcp(&f, &token, "tools/list", json!({})).await?;
    let served: Vec<&str> = tools["result"]["tools"]
        .as_array()
        .ok_or("tools")?
        .iter()
        .filter_map(|t| t["name"].as_str())
        .collect();
    for tool in [
        "secrets_list",
        "secret_verify",
        "secret_copy_to_env",
        "secret_request_hosts",
        "secret_create_from_file",
    ] {
        assert!(served.contains(&tool), "{tool}: {served:?}");
    }
    // It has no project browser (SAV-004).
    assert!(!served.contains(&"browser_fill_secret"), "{served:?}");
    let (refused, is_error) = cockpit_tool(
        &f,
        &token,
        "browser_fill_secret",
        json!({"secret": "APIFY_KEY"}),
    )
    .await?;
    assert!(is_error, "{refused}");
    assert!(refused.to_string().contains("unknown tool"), "{refused}");
    // Only the tools it is served are called for it, task tools included (SPEC-271's agent
    // states among them).
    for tool in [
        "secret_state_restore",
        "secret_state_save",
        "delivery_merge",
        "scope_claim",
    ] {
        let (refused, is_error) = cockpit_tool(&f, &token, tool, json!({"name": "OPS"})).await?;
        assert!(is_error, "{tool}: {refused}");
        assert!(
            refused.to_string().contains("unknown tool"),
            "{tool}: {refused}"
        );
    }
    f.h.app.end_all_shells().await;
    Ok(())
}

/// A synthetic three-line key, as a PEM file holds it.
const PEM_KEY: &str =
    "-----BEGIN SYNTHETIC KEY-----\nc3ludGhldGljLWtleS1ib2R5LTQ0MTc=\n-----END SYNTHETIC KEY-----";

/// SPEC-178 VAU-002 (ADR-0144): the Superagent checks a value the person holds against the vault
/// by its SHA-256, a multi-line key (which the vault holds on one line, with literal `\n`)
/// included, and learns how a near match differs; it never gets the value or its hash, and each
/// check is audited with the Superagent as actor.
#[tokio::test]
async fn the_superagent_verifies_a_multi_line_value_by_its_hash() -> TestResult {
    use yulius_domain::ids::UserId;

    let f = fixture().await?;
    // An agent secret never spans lines: a multi-line key is stored with literal `\n`.
    let stored = PEM_KEY.replace('\n', "\\n");
    let added = call(
        &f.h.router,
        Method::POST,
        "/api/sandboxes/s1/agent-secrets",
        Some(json!({"name": "FOUNDER_PRIVATE_KEY", "value": stored})),
        &f.alice,
    )
    .await?;
    assert_eq!(added.status, StatusCode::CREATED, "{:?}", added.body);
    cockpit_id(&f, &f.alice).await?;
    let token =
        f.h.app
            .cockpit_mcp_token(&UserId::parse("u-alice")?, &sid())
            .ok_or("token")?;
    let sha = |text: &str| f.h.app.deps.crypto.sha256_hex(text.as_bytes());
    let value_sha = sha(&stored);

    for (candidate, kind) in [
        (stored.clone(), "exact"),
        (format!("{stored}\n"), "trailing_newline"),
        (PEM_KEY.to_owned(), "escaped_newlines"),
        (format!("{PEM_KEY}\n"), "escaped_newlines"),
        (
            format!("{}\r\n", PEM_KEY.replace('\n', "\r\n")),
            "escaped_newlines",
        ),
        (PEM_KEY.replace("LTQ0MTc", "LTQ0MTg"), "different"),
    ] {
        // Upper-case hex is the same hash, and `sha256sum`'s own line is taken as it prints it.
        let hash = match kind {
            "trailing_newline" => sha(&candidate).to_uppercase(),
            "exact" => format!("{}  key.pem\n", sha(&candidate)),
            _ => sha(&candidate),
        };
        let (answer, is_error) = cockpit_tool(
            &f,
            &token,
            "secret_verify",
            json!({"secret": "FOUNDER_PRIVATE_KEY", "sha256": hash}),
        )
        .await?;
        assert!(!is_error, "{kind}: {answer}");
        assert_eq!(answer["match"], kind, "{answer}");
        if kind == "different" {
            assert!(answer.get("lines").is_none(), "{answer}");
        } else {
            assert_eq!(answer["lines"], 3, "{answer}");
        }
        let raw = answer.to_string();
        assert!(!raw.contains("c3ludGhldGljLWtleS1ib2R5LTQ0MTc="), "{raw}");
        assert!(!raw.contains(&value_sha), "{raw}");
    }

    for (args, refusal) in [
        (
            json!({"secret": "NO_SUCH_KEY", "sha256": value_sha}),
            "no agent secret named",
        ),
        (
            json!({"secret": "FOUNDER_PRIVATE_KEY", "sha256": "abc123"}),
            "64 hexadecimal digits",
        ),
        (
            json!({"secret": "FOUNDER_PRIVATE_KEY", "sha256": value_sha, "value": "x"}),
            "invalid arguments",
        ),
    ] {
        let (refused, is_error) = cockpit_tool(&f, &token, "secret_verify", args).await?;
        assert!(is_error, "{refused}");
        assert!(
            refused.to_string().contains(refusal),
            "{refusal}: {refused}"
        );
    }

    let activity = call(
        &f.h.router,
        Method::GET,
        "/api/secrets/activity",
        None,
        &f.alice,
    )
    .await?;
    let checks: Vec<&Value> = activity
        .body
        .as_array()
        .ok_or("activity")?
        .iter()
        .filter(|e| e["action"] == "vault.agent.verify")
        .collect();
    assert_eq!(checks.len(), 6, "one entry per distinct check: {checks:?}");
    assert!(
        checks.iter().all(|e| e["actor"] == "cockpit:alice"),
        "{checks:?}"
    );
    assert!(
        checks
            .iter()
            .any(|e| e["detail"] == "FOUNDER_PRIVATE_KEY → exact"),
        "{checks:?}"
    );
    // The same check again (plain hex this time) answers the same and writes no second entry.
    let (repeated, is_error) = cockpit_tool(
        &f,
        &token,
        "secret_verify",
        json!({"secret": "FOUNDER_PRIVATE_KEY", "sha256": value_sha}),
    )
    .await?;
    assert!(!is_error, "{repeated}");
    assert_eq!(repeated["match"], "exact", "{repeated}");
    assert_eq!(repeated["lines"], 3, "{repeated}");
    let again = call(
        &f.h.router,
        Method::GET,
        "/api/secrets/activity",
        None,
        &f.alice,
    )
    .await?;
    let count = again
        .body
        .as_array()
        .ok_or("activity")?
        .iter()
        .filter(|e| e["action"] == "vault.agent.verify")
        .count();
    assert_eq!(count, 6);

    // After a rotation the same check answers anew and is audited again (review round 1).
    let id = added.body["id"].as_str().ok_or("id")?;
    let rotated = call(
        &f.h.router,
        Method::PATCH,
        &format!("/api/agent-secrets/{id}"),
        Some(json!({"value": "rotated-founder-key-value-1"})),
        &f.alice,
    )
    .await?;
    assert!(rotated.status.is_success(), "{:?}", rotated.body);
    let (after, is_error) = cockpit_tool(
        &f,
        &token,
        "secret_verify",
        json!({"secret": "FOUNDER_PRIVATE_KEY", "sha256": value_sha}),
    )
    .await?;
    assert!(!is_error, "{after}");
    assert_eq!(after["match"], "different", "{after}");
    let rotated_activity = call(
        &f.h.router,
        Method::GET,
        "/api/secrets/activity",
        None,
        &f.alice,
    )
    .await?;
    // The exact candidate's check, audited again under the new generation: a seventh entry. Only
    // the count proves it, since the unrotated "different" candidate wrote the same detail before.
    let verifies: Vec<&Value> = rotated_activity
        .body
        .as_array()
        .ok_or("activity")?
        .iter()
        .filter(|e| e["action"] == "vault.agent.verify")
        .collect();
    assert_eq!(
        verifies.len(),
        7,
        "re-audited after the rotation: {verifies:?}"
    );
    assert_eq!(
        verifies[0]["detail"], "FOUNDER_PRIVATE_KEY → different",
        "{verifies:?}"
    );

    // Another project's secret is not this Superagent's to check (sandbox scope).
    seed_sandbox(&f.h, "s2", "other", "u-alice").await?;
    f.h.github.set_perm("alice", "acme/other", perms("write"));
    let other = call(
        &f.h.router,
        Method::POST,
        "/api/sandboxes/s2/agent-secrets",
        Some(json!({"name": "OTHER_KEY", "value": "other_synthetic_value_4471"})),
        &f.alice,
    )
    .await?;
    assert_eq!(other.status, StatusCode::CREATED, "{:?}", other.body);
    let (refused, is_error) = cockpit_tool(
        &f,
        &token,
        "secret_verify",
        json!({"secret": "OTHER_KEY", "sha256": sha("other_synthetic_value_4471")}),
    )
    .await?;
    assert!(is_error, "{refused}");
    assert!(
        refused.to_string().contains("no agent secret named"),
        "{refused}"
    );

    // A production secret is refused (SPEC-134).
    let production = call(
        &f.h.router,
        Method::POST,
        "/api/sandboxes/s1/agent-secrets",
        Some(
            json!({"name": "PROD_KEY", "value": "prod-synthetic-value-12345", "auto_allow": false}),
        ),
        &f.alice,
    )
    .await?;
    assert_eq!(
        production.status,
        StatusCode::CREATED,
        "{:?}",
        production.body
    );
    {
        use yulius_app::ports::vault::VaultStore as _;
        let id = production.body["id"].as_str().ok_or("id")?;
        assert!(f.h.store.vault_set_cd_prod(id, false, true).await?);
    }
    let (refused, is_error) = cockpit_tool(
        &f,
        &token,
        "secret_verify",
        json!({"secret": "PROD_KEY", "sha256": sha("prod-synthetic-value-12345")}),
    )
    .await?;
    assert!(is_error, "{refused}");
    assert!(
        refused.to_string().contains("production secret"),
        "{refused}"
    );
    f.h.app.end_all_shells().await;
    Ok(())
}

/// Runs `git` in `dir` with a synthetic identity.
fn git(dir: &Path, args: &[&str]) -> TestResult {
    let out = std::process::Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@example.com")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@example.com")
        .output()?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).into_owned().into());
    }
    Ok(())
}

/// The sandbox repository of the fixture as a Git repository that ignores `.env` and `.env.*`
/// but keeps `.env.example`.
fn ignoring_repo(f: &Fixture) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let repo = std::fs::canonicalize(f.data.join("sandboxes/s1/repo"))?;
    git(&repo, &["init", "-q", "-b", "dev"])?;
    std::fs::write(repo.join(".gitignore"), ".env\n.env.*\n!.env.example\n")?;
    git(&repo, &["add", "."])?;
    git(&repo, &["commit", "-q", "-m", "init"])?;
    Ok(repo)
}

/// alice adds the agent secret `name` with `value` to `s1`; its id.
async fn add_secret(
    f: &Fixture,
    name: &str,
    value: &str,
) -> Result<String, Box<dyn std::error::Error>> {
    let resp = call(
        &f.h.router,
        Method::POST,
        "/api/sandboxes/s1/agent-secrets",
        // A person confirms hosts and production (SPEC-216: agents do not set it up).
        Some(json!({"name": name, "value": value, "auto_allow": false})),
        &f.alice,
    )
    .await?;
    assert_eq!(resp.status, StatusCode::CREATED, "{:?}", resp.body);
    Ok(resp.body["id"].as_str().ok_or("id")?.to_owned())
}

/// The secrets activity entries with `action`.
async fn audited(f: &Fixture, action: &str) -> Result<Vec<Value>, Box<dyn std::error::Error>> {
    let activity = call(
        &f.h.router,
        Method::GET,
        "/api/secrets/activity",
        None,
        &f.alice,
    )
    .await?;
    Ok(activity
        .body
        .as_array()
        .ok_or("activity")?
        .iter()
        .filter(|e| e["action"] == action)
        .cloned()
        .collect())
}

/// SPEC-186 SAV-002, SAV-003, SAV-005 (ADR-0150): the Superagent verifies a 7-line OpenSSH key
/// the person holds, copies it into the ignored `.env` of the sandbox repository it works in once
/// a person allowed it (asked first, as a task's agent is), byte-identical to the key file, and
/// asks for hosts; production secrets and files Git would commit are refused.
#[tokio::test]
async fn the_superagent_copies_allowed_secrets_into_its_repositorys_ignored_env() -> TestResult {
    use yulius_app::ports::vault::{EnvFiles, VaultStore as _};
    use yulius_domain::ids::UserId;

    let f = fixture().await?;
    let repo = ignoring_repo(&f)?;
    let key = crate::secrets_env::openssh_key(1);
    let id = add_secret(&f, "DEPLOY_SSH", &key.trim_end().replace('\n', "\\n")).await?;
    let prod = add_secret(&f, "PROD_KEY", "prod-synthetic-value-12345").await?;
    assert!(f.h.store.vault_set_cd_prod(&prod, false, true).await?);
    cockpit_id(&f, &f.alice).await?;
    let token =
        f.h.app
            .cockpit_mcp_token(&UserId::parse("u-alice")?, &sid())
            .ok_or("token")?;

    // The person's key file checks out, all seven lines.
    let (verified, is_error) = cockpit_tool(
        &f,
        &token,
        "secret_verify",
        json!({"secret": "DEPLOY_SSH", "sha256": f.h.app.deps.crypto.sha256_hex(key.as_bytes())}),
    )
    .await?;
    assert!(!is_error, "{verified}");
    assert_eq!(verified["match"], "escaped_newlines", "{verified}");
    assert_eq!(verified["lines"], 7, "{verified}");

    // Not allowed yet: asked for, as alice's Superagent, and nothing is written; the open
    // Secrets pages hear of it (SPEC-209).
    let mut announced = support::SecretAnnouncements::new(&f.h.app);
    let copy = |secrets: Value, path: Option<&str>| {
        let mut args = json!({"secrets": secrets, "reason": "the deploy script signs in"});
        if let Some(path) = path {
            args["path"] = json!(path);
        }
        cockpit_tool(&f, &token, "secret_copy_to_env", args)
    };
    let (asked, is_error) = copy(json!(["DEPLOY_SSH"]), None).await?;
    assert!(!is_error, "{asked}");
    assert_eq!(asked["copied"], json!([]), "{asked}");
    assert_eq!(asked["waiting"], json!(["DEPLOY_SSH"]), "{asked}");
    assert!(
        asked["link"]
            .as_str()
            .is_some_and(|l| l.ends_with("/s/fixture/secrets?section=agents")),
        "{asked}"
    );
    assert!(!repo.join(".env").exists(), "nothing is written");
    assert_eq!(announced.take(), vec![Some(sid().to_string())]);
    let record = f.h.store.vault_get(&id).await?.ok_or("secret")?;
    assert_eq!(record.env_files, EnvFiles::Asked);
    assert_eq!(
        record.env_requested_by.as_deref(),
        Some("alice's Superagent")
    );
    let requests = audited(&f, "vault.agent.env_request").await?;
    assert_eq!(requests.len(), 1, "{requests:?}");
    assert_eq!(requests[0]["actor"], "cockpit:alice", "{requests:?}");

    // A person allows it; the same call writes the key file's bytes into the ignored `.env`.
    let allowed = call(
        &f.h.router,
        Method::POST,
        &format!("/api/agent-secrets/{id}/env"),
        Some(
            json!({"allow": true, "env_files": "asked", "env_requested_by": "alice's Superagent"}),
        ),
        &f.alice,
    )
    .await?;
    assert_eq!(allowed.status, StatusCode::OK, "{:?}", allowed.body);
    let (copied, is_error) = copy(json!(["DEPLOY_SSH"]), None).await?;
    assert!(!is_error, "{copied}");
    assert_eq!(copied["copied"], json!(["DEPLOY_SSH"]), "{copied}");
    assert_eq!(copied["file"], ".env", "{copied}");
    assert!(!copied.to_string().contains("LXZ1-bGluZS0w"), "{copied}");
    assert_eq!(crate::secrets_env::sourced(&repo, "DEPLOY_SSH")?, key);
    assert_eq!(
        std::fs::read_to_string(repo.join(".env.example"))?,
        "DEPLOY_SSH=\n"
    );
    let copies = audited(&f, "vault.agent.env").await?;
    assert_eq!(copies.len(), 1, "{copies:?}");
    assert_eq!(copies[0]["actor"], "cockpit:alice", "{copies:?}");

    // A file Git would commit, and a production secret, are refused.
    let (refused, is_error) = copy(json!(["DEPLOY_SSH"]), Some("deploy.env")).await?;
    assert!(is_error, "{refused}");
    assert!(
        refused.to_string().contains("Git does not ignore"),
        "{refused}"
    );
    let (refused, is_error) = copy(json!(["PROD_KEY"]), None).await?;
    assert!(is_error, "{refused}");
    assert!(refused.to_string().contains("production"), "{refused}");

    // Hosts are asked for, as alice's Superagent; a person allows them on the Secrets page.
    let (requested, is_error) = cockpit_tool(
        &f,
        &token,
        "secret_request_hosts",
        json!({"secret": "DEPLOY_SSH", "hosts": ["deploy.quartio.test"], "reason": "deploys"}),
    )
    .await?;
    assert!(!is_error, "{requested}");
    assert_eq!(requested["requested"], json!(["deploy.quartio.test"]));
    // #631: the person's open Secrets page shows the request and its Allow at once.
    assert_eq!(announced.take(), vec![Some(sid().to_string())]);
    let record = f.h.store.vault_get(&id).await?.ok_or("secret")?;
    assert_eq!(
        record.requested_hosts,
        vec!["deploy.quartio.test".to_owned()]
    );
    assert_eq!(record.requested_by.as_deref(), Some("alice's Superagent"));
    assert!(record.hosts.is_empty(), "never bound by the Superagent");
    let asks = audited(&f, "vault.agent.request").await?;
    assert_eq!(asks.len(), 1, "{asks:?}");
    assert_eq!(asks[0]["actor"], "cockpit:alice", "{asks:?}");
    let (refused, is_error) = cockpit_tool(
        &f,
        &token,
        "secret_request_hosts",
        json!({"secret": "PROD_KEY", "hosts": ["deploy.quartio.test"]}),
    )
    .await?;
    assert!(is_error, "{refused}");
    assert!(refused.to_string().contains("production"), "{refused}");
    f.h.app.end_all_shells().await;
    Ok(())
}

/// #635 (SPEC-254 ACR-001..ACR-003): the Superagent can write its sandbox folder, so it can swap
/// its repository for a link to another folder (here another ignoring Git repository). Yulius walks
/// to the `.env` from the canonical sandbox folder without links, so the copy of an allowed secret
/// is refused and nothing is written, neither in the link's target nor in the moved repository.
#[tokio::test]
async fn the_superagent_never_copies_into_a_repo_swapped_for_a_link() -> TestResult {
    use yulius_domain::ids::UserId;

    let f = fixture().await?;
    let repo = ignoring_repo(&f)?;
    let id = add_secret(&f, "SWAP_KEY", "swap-synthetic-value-0635").await?;
    cockpit_id(&f, &f.alice).await?;
    let token =
        f.h.app
            .cockpit_mcp_token(&UserId::parse("u-alice")?, &sid())
            .ok_or("token")?;
    let copy = || {
        cockpit_tool(
            &f,
            &token,
            "secret_copy_to_env",
            json!({"secrets": ["SWAP_KEY"]}),
        )
    };
    let (asked, is_error) = copy().await?;
    assert!(!is_error, "{asked}");
    assert_eq!(asked["waiting"], json!(["SWAP_KEY"]), "{asked}");
    let allowed = call(
        &f.h.router,
        Method::POST,
        &format!("/api/agent-secrets/{id}/env"),
        Some(
            json!({"allow": true, "env_files": "asked", "env_requested_by": "alice's Superagent"}),
        ),
        &f.alice,
    )
    .await?;
    assert_eq!(allowed.status, StatusCode::OK, "{:?}", allowed.body);

    let other = f.data.join("other");
    std::fs::create_dir_all(&other)?;
    git(&other, &["init", "-q", "-b", "dev"])?;
    std::fs::write(other.join(".gitignore"), ".env\n.env.*\n!.env.example\n")?;
    git(&other, &["add", "."])?;
    git(&other, &["commit", "-q", "-m", "init"])?;
    let moved = f.data.join("sandboxes/s1/repo.old");
    std::fs::rename(&repo, &moved)?;
    std::os::unix::fs::symlink(&other, &repo)?;

    let (refused, is_error) = copy().await?;
    assert!(is_error, "{refused}");
    assert!(
        refused
            .to_string()
            .contains("the folder you work in is a link or not a folder: Yulius follows no links"),
        "{refused}"
    );
    for dir in [&other, &moved] {
        for name in [".env", ".env.example"] {
            assert!(
                !dir.join(name).exists(),
                "{} was written",
                dir.join(name).display()
            );
        }
    }
    assert!(
        audited(&f, "vault.agent.env").await?.is_empty(),
        "no copy is recorded"
    );
    f.h.app.end_all_shells().await;
    Ok(())
}

/// An HTTPS request to `host` as the proxy hands it to the egress policy.
fn egress_request(
    host: &str,
    headers: &[(&str, &str)],
) -> yulius_app::ports::egress::EgressRequest {
    yulius_app::ports::egress::EgressRequest {
        host: host.into(),
        port: 443,
        path_and_query: "/v1/deploys".into(),
        headers: headers
            .iter()
            .map(|(n, v)| ((*n).to_owned(), v.as_bytes().to_vec()))
            .collect(),
    }
}

/// The value of header `name` a request is forwarded with.
fn forwarded(decision: &yulius_app::ports::egress::EgressDecision, name: &str) -> Option<String> {
    match decision {
        yulius_app::ports::egress::EgressDecision::Forward { headers, .. } => headers
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| String::from_utf8_lossy(v).into_owned()),
        yulius_app::ports::egress::EgressDecision::Refuse { .. } => None,
    }
}

/// SPEC-186 SAV-001, SAV-005 (ADR-0150): the Superagent runs behind the egress proxy as a task
/// run does, with its own bearer: the proxy variables, the CA bundle and a placeholder per agent
/// secret are in its environment, the proxy adds a bound secret to its requests for the bound
/// host (a multi-line key as stored), never a production secret, audited as the Superagent, and
/// refuses its bearer once it ended.
#[tokio::test]
async fn the_superagent_runs_behind_the_egress_proxy_with_its_own_bearer() -> TestResult {
    use yulius_app::ports::egress::EgressPolicy as _;
    use yulius_domain::ids::UserId;

    let f = fixture().await?;
    f.h.app
        .enable_egress(Some("synthetic-egress-ca-bundle\n".into()));
    add_apify_key(&f).await?;
    let pem = crate::secrets_env::pem_key().replace('\n', "\\n");
    let deploy = call(
        &f.h.router,
        Method::POST,
        "/api/sandboxes/s1/agent-secrets",
        Some(
            json!({"name": "DEPLOY_PEM", "hosts": ["deploy.quartio.test"], "injection": "header",
            "header_name": "X-Deploy-Key", "value": pem}),
        ),
        &f.alice,
    )
    .await?;
    assert_eq!(deploy.status, StatusCode::CREATED, "{:?}", deploy.body);
    // A production secret bound to a host (kept from before it was one): never used.
    let prod = call(
        &f.h.router,
        Method::POST,
        "/api/sandboxes/s1/agent-secrets",
        Some(
            json!({"name": "PROD_KEY", "hosts": ["prod.quartio.test"], "injection": "bearer",
            "value": "prod-synthetic-value-12345", "auto_allow": false}),
        ),
        &f.alice,
    )
    .await?;
    assert_eq!(prod.status, StatusCode::CREATED, "{:?}", prod.body);
    crate::support::legacy_production_secret(&f.h.store, prod.body["id"].as_str().ok_or("id")?)
        .await?;

    let served = serve(f.h.router.clone()).await?;
    let id = cockpit_id(&f, &f.alice).await?;
    let mut socket = attach(served.addr, &id, &f.alice).await?;
    let mut screen = String::new();
    read_until(&mut socket, &mut screen, "ready").await?;
    let token =
        f.h.app
            .cockpit_mcp_token(&UserId::parse("u-alice")?, &sid())
            .ok_or("token")?;
    let proxy = printed(&screen, "proxy")?;
    assert!(
        proxy.starts_with(&format!("http://yulius:{token}@")),
        "its own bearer: {proxy}"
    );
    assert!(printed(&screen, "no-proxy")?.contains("anthropic.com"));
    assert_eq!(printed(&screen, "placeholder")?, "yulius-secret-APIFY_KEY");
    let ca = PathBuf::from(printed(&screen, "ca")?);
    assert!(
        ca.starts_with(f.data.join("shells").join(&id)),
        "in its run directory: {ca:?}"
    );
    assert_eq!(
        std::fs::read_to_string(&ca)?,
        "synthetic-egress-ca-bundle\n"
    );

    let client = f.h.app.egress_client(&token).await.ok_or("client")?;
    assert_eq!(client.actor(), "cockpit:alice");
    assert!(
        f.h.app
            .egress_intercepts(&client, "api.apify.test", 443)
            .await
    );
    assert!(
        !f.h.app
            .egress_intercepts(&client, "prod.quartio.test", 443)
            .await
    );
    let apify =
        f.h.app
            .egress_apply(
                &client,
                egress_request(
                    "api.apify.test",
                    &[("authorization", "Bearer yulius-secret-APIFY_KEY")],
                ),
            )
            .await;
    assert_eq!(
        forwarded(&apify, "authorization"),
        Some(format!("Bearer {SECRET_VALUE}"))
    );
    // A header cannot carry line breaks: the key goes as stored, byte for byte.
    let key =
        f.h.app
            .egress_apply(&client, egress_request("deploy.quartio.test", &[]))
            .await;
    assert_eq!(forwarded(&key, "x-deploy-key"), Some(pem.clone()));
    let uses: Vec<Value> = audited(&f, "vault.agent.use").await?;
    assert_eq!(uses.len(), 2, "{uses:?}");
    assert!(
        uses.iter().all(|u| u["actor"] == "cockpit:alice"),
        "{uses:?}"
    );

    // Another project's secret is never added for it (review round 1).
    seed_sandbox(&f.h, "s2", "other", "u-alice").await?;
    f.h.github.set_perm("alice", "acme/other", perms("write"));
    let other = call(
        &f.h.router,
        Method::POST,
        "/api/sandboxes/s2/agent-secrets",
        Some(
            json!({"name": "OTHER_KEY", "hosts": ["other.quartio.test"], "injection": "bearer",
            "value": "other_synthetic_value_4471"}),
        ),
        &f.alice,
    )
    .await?;
    assert_eq!(other.status, StatusCode::CREATED, "{:?}", other.body);
    assert!(
        !f.h.app
            .egress_intercepts(&client, "other.quartio.test", 443)
            .await
    );
    let elsewhere =
        f.h.app
            .egress_apply(&client, egress_request("other.quartio.test", &[]))
            .await;
    assert_eq!(forwarded(&elsewhere, "authorization"), None);

    // A Superagent whose person lost write access, or was disabled, gets no proxy client.
    use yulius_app::ports::store::UserStore as _;
    cockpit_id(&f, &f.carol).await?;
    let carol = UserId::parse("u-carol")?;
    let carols =
        f.h.app
            .cockpit_mcp_token(&carol, &sid())
            .ok_or("carol's token")?;
    f.h.github.set_perm("carol", "acme/fixture", perms("read"));
    // Past the access decisions' staleness bound, as for its MCP calls.
    f.h.clock
        .0
        .fetch_add(f.h.app.config.permission_ttl_ms + 1, Ordering::SeqCst);
    assert!(
        f.h.app.egress_client(&carols).await.is_none(),
        "lost access"
    );
    let dave = seed_github_user(&f.h, "u-dave", "dave").await?;
    f.h.github.set_perm("dave", "acme/fixture", perms("write"));
    cockpit_id(&f, &dave).await?;
    let daves =
        f.h.app
            .cockpit_mcp_token(&UserId::parse("u-dave")?, &sid())
            .ok_or("dave's token")?;
    f.h.store
        .update_user_flags(&UserId::parse("u-dave")?, Some(Some(f.h.app.now())), None)
        .await?;
    assert!(f.h.app.egress_client(&daves).await.is_none(), "disabled");

    // Ended, its bearer is refused at once.
    f.h.app.end_all_shells().await;
    assert!(f.h.app.egress_client(&token).await.is_none());
    Ok(())
}

/// SPEC-186 SAV-006 (review round 1): a Superagent started while the egress proxy was not wired
/// is restarted once it is, so it gets the proxy.
#[tokio::test]
async fn a_superagent_started_without_the_proxy_restarts_once_it_is_wired() -> TestResult {
    let f = fixture().await?;
    let id = cockpit_id(&f, &f.alice).await?;
    f.h.app.stop_workspace_services().await;

    let g = fixture_with(true, Some(f.data.clone())).await?;
    g.h.app.enable_egress(None);
    let served = serve(g.h.router.clone()).await?;
    assert_eq!(g.h.app.adopt_shells().await, (1, 0));
    assert_ne!(g.h.app.cockpit_refresh_state(&id), "none");
    let again = other_cockpit(&g, &id).await?;
    let mut socket = attach(served.addr, &again, &g.alice).await?;
    let mut screen = String::new();
    read_until(&mut socket, &mut screen, "ready").await?;
    let token =
        g.h.app
            .cockpit_mcp_token(&yulius_domain::ids::UserId::parse("u-alice")?, &sid())
            .ok_or("token")?;
    assert!(
        printed(&screen, "proxy")?.starts_with(&format!("http://yulius:{token}@")),
        "the new one is behind the proxy: {screen}"
    );
    g.h.app.end_all_shells().await;
    Ok(())
}

/// The project directory of alice's Superagent conversations in the fixture's repository.
fn alice_project(f: &Fixture) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let config = f.data.join("users/u-alice/home/.claude");
    let repo = std::fs::canonicalize(f.data.join("sandboxes/s1/repo"))?;
    Ok(yulius_harness::paths::claude_project_dir(&config, &repo))
}

/// Makes the record of shell `id` look written by a Yulius from before SPEC-186 (no launch
/// digest).
fn predate_launch_digest(f: &Fixture, id: &str) -> TestResult {
    let path = f.data.join("shells").join(id).join("shell.json");
    let mut record: Value = serde_json::from_str(&std::fs::read_to_string(&path)?)?;
    assert!(record.get("launch").is_some(), "{record}");
    record.as_object_mut().ok_or("record")?.remove("launch");
    std::fs::write(&path, serde_json::to_vec(&record)?)?;
    Ok(())
}

/// SPEC-186 SAV-006 (ADR-0150): a Superagent started by a Yulius before an update that changed
/// what it is served is adopted, then started again resuming its conversation once it is idle,
/// and the new one has the new tools and the proxy; its old bearer ends with it.
#[tokio::test]
async fn a_superagent_started_before_an_update_restarts_resuming_its_conversation() -> TestResult {
    let f = fixture().await?;
    let id = cockpit_id(&f, &f.alice).await?;
    let alice = yulius_domain::ids::UserId::parse("u-alice")?;
    let old_token = f.h.app.cockpit_mcp_token(&alice, &sid()).ok_or("token")?;
    let project = alice_project(&f)?;
    std::fs::create_dir_all(&project)?;
    std::fs::write(
        project.join("conv-1.jsonl"),
        [
            prompt_record("u0", None, -60),
            answer_record("a0", "u0", -59),
        ]
        .concat(),
    )?;
    predate_launch_digest(&f, &id)?;
    f.h.app.stop_workspace_services().await;

    let g = fixture_with(true, Some(f.data.clone())).await?;
    g.h.app.enable_egress(None);
    add_apify_key(&g).await?;
    let served = serve(g.h.router.clone()).await?;
    assert_eq!(g.h.app.adopt_shells().await, (1, 0));
    let again = other_cockpit(&g, &id).await?;
    let mut socket = attach(served.addr, &again, &g.alice).await?;
    let mut screen = String::new();
    read_until(&mut socket, &mut screen, "ready").await?;
    assert_eq!(
        printed(&screen, "cockpit-args")?,
        "[--resume conv-1 --model claude-opus-5-5 --effort high]",
        "the same conversation, nothing typed"
    );
    assert_eq!(
        printed(&screen, "interrupted")?,
        "none",
        "no turn is redone"
    );
    let token = g.h.app.cockpit_mcp_token(&alice, &sid()).ok_or("token")?;
    assert_ne!(token, old_token);
    assert!(printed(&screen, "proxy")?.contains(&format!("yulius:{token}@")));
    assert_eq!(printed(&screen, "placeholder")?, "yulius-secret-APIFY_KEY");
    let tools = mcp(&g, &token, "tools/list", json!({})).await?;
    assert!(
        tools.to_string().contains("\"secret_copy_to_env\""),
        "{tools}"
    );
    let (listed, is_error) = cockpit_tool(&g, &token, "secrets_list", json!({})).await?;
    assert!(!is_error, "{listed}");
    assert_eq!(listed["secrets"][0]["name"], "APIFY_KEY", "{listed}");
    let refused = mcp(&g, &old_token, "tools/list", json!({})).await?;
    assert!(refused.get("error").is_some(), "{refused}");
    let record = std::fs::read_to_string(g.data.join("shells").join(&again).join("shell.json"))?;
    assert!(record.contains("\"launch\":"), "{record}");
    assert_eq!(g.h.app.cockpit_refresh_state(&again), "none");
    assert_eq!(cockpit_id(&g, &g.alice).await?, again, "the panel finds it");
    g.h.app.end_all_shells().await;
    Ok(())
}

/// SPEC-186 SAV-006: a Superagent to restart that is in the middle of a turn waits for it to end.
#[tokio::test]
async fn a_superagent_mid_turn_restarts_only_once_its_turn_ends() -> TestResult {
    let f = fixture().await?;
    let id = cockpit_id(&f, &f.alice).await?;
    let project = alice_project(&f)?;
    std::fs::create_dir_all(&project)?;
    let transcript = project.join("conv-1.jsonl");
    let mut text = [
        prompt_record("u0", None, -60),
        answer_record("a0", "u0", -59),
        prompt_record("u1", Some("a0"), -10),
    ]
    .concat();
    std::fs::write(&transcript, &text)?;
    predate_launch_digest(&f, &id)?;
    f.h.app.stop_workspace_services().await;

    let g = fixture_with(true, Some(f.data.clone())).await?;
    assert_eq!(g.h.app.adopt_shells().await, (1, 0));
    assert_eq!(g.h.app.cockpit_refresh_state(&id), "due");
    wait_for("the Superagent to wait for its turn", || {
        g.h.app.cockpit_refresh_state(&id) == "waiting"
    })
    .await?;
    let live: Vec<String> =
        g.h.app
            .sandbox_shells(&sid())
            .iter()
            .map(|s| s.id().to_owned())
            .collect();
    assert_eq!(live, vec![id.clone()], "not restarted mid-turn");

    text.push_str(&answer_record("a1", "u1", -5));
    std::fs::write(&transcript, &text)?;
    other_cockpit(&g, &id).await?;
    g.h.app.end_all_shells().await;
    Ok(())
}

/// SPEC-211 SCF-001, SCF-002, SCF-004 (AC-7): the Superagent stores a key in its person's home as
/// an agent secret without seeing it, created by the person and audited as their Superagent; the
/// stored value is the key file with literal `\n` (`secret_verify` matches the file's hash).
#[tokio::test]
async fn the_superagent_stores_a_key_from_its_persons_home() -> TestResult {
    use yulius_app::ports::vault::{EnvFiles, VaultStore as _};
    use yulius_domain::ids::UserId;

    let f = fixture().await?;
    cockpit_id(&f, &f.alice).await?;
    let token =
        f.h.app
            .cockpit_mcp_token(&UserId::parse("u-alice")?, &sid())
            .ok_or("token")?;
    let home = f.data.join("users/u-alice/home");
    std::fs::create_dir_all(home.join(".ssh"))?;
    let key = crate::secrets_env::openssh_key(4);
    std::fs::write(home.join(".ssh/operator"), &key)?;

    let (created, is_error) = cockpit_tool(
        &f,
        &token,
        "secret_create_from_file",
        json!({
            "name": "DO_SSH_PRIVATE_KEY",
            "file": "~/.ssh/operator",
            "description": "operator SSH key (ed25519)"
        }),
    )
    .await?;
    assert!(!is_error, "{created}");
    assert_eq!(created["created"], "DO_SSH_PRIVATE_KEY", "{created}");
    assert_eq!(created["file"], "~/.ssh/operator", "{created}");
    assert_eq!(created["deleted"], false, "{created}");
    for line in key.lines().skip(1).take(5) {
        assert!(!created.to_string().contains(line), "{created}");
    }
    assert!(home.join(".ssh/operator").exists(), "kept unless asked");

    let records = f.h.store.vault_agent_in(&sid()).await?;
    let record = records
        .iter()
        .find(|r| r.name == "DO_SSH_PRIVATE_KEY")
        .ok_or("stored")?;
    assert_eq!(record.sandbox_id, Some(sid()));
    assert!(record.hosts.is_empty());
    assert_eq!(record.env_files, EnvFiles::No);
    assert!(!record.cd_prod);
    assert_eq!(record.created_by, "u-alice");
    assert_eq!(record.description, "operator SSH key (ed25519)");

    let (verified, is_error) = cockpit_tool(
        &f,
        &token,
        "secret_verify",
        json!({"secret": "DO_SSH_PRIVATE_KEY", "sha256": f.h.app.deps.crypto.sha256_hex(key.as_bytes())}),
    )
    .await?;
    assert!(!is_error, "{verified}");
    assert_eq!(verified["match"], "escaped_newlines", "{verified}");
    assert_eq!(verified["lines"], 7, "{verified}");

    let audit = audited(&f, "vault.agent.create").await?;
    assert_eq!(audit.len(), 1, "{audit:?}");
    assert_eq!(audit[0]["actor"], "cockpit:alice", "{audit:?}");
    let detail = audit[0]["detail"].as_str().unwrap_or_default();
    assert!(
        detail.contains("from a file, by alice's Superagent"),
        "{detail}"
    );

    // Review of SPEC-211, finding 1: the sandbox repository swapped for a link to Yulius' data
    // folder is never followed.
    std::fs::write(f.data.join("outside.txt"), "outside-synthetic-value-0004\n")?;
    let repo = f.data.join("sandboxes/s1/repo");
    std::fs::rename(&repo, f.data.join("sandboxes/s1/repo.old"))?;
    std::os::unix::fs::symlink(&f.data, &repo)?;
    let (refused, is_error) = cockpit_tool(
        &f,
        &token,
        "secret_create_from_file",
        json!({"name": "OUTSIDE_KEY", "file": "outside.txt"}),
    )
    .await?;
    assert!(is_error, "{refused}");
    assert!(
        !refused.to_string().contains("outside-synthetic"),
        "{refused}"
    );
    let records = f.h.store.vault_agent_in(&sid()).await?;
    assert!(!records.iter().any(|r| r.name == "OUTSIDE_KEY"));
    f.h.app.end_all_shells().await;
    Ok(())
}
