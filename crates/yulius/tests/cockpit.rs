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
/// files in its config dir) and environment, then stays as an interactive shell.
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
echo \"token:${CLAUDE_CODE_OAUTH_TOKEN:-none}\"\n\
echo \"interrupted:${CLAUDE_CODE_RESUME_INTERRUPTED_TURN:-none}\"\n\
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
    let gate = Arc::new(GatedHost::new(exe));
    let h = harness_with(HarnessOptions {
        session_host: Some(Arc::clone(&gate) as Arc<dyn SessionHost>),
        dir: Some(dir),
        configure: Some(Box::new(move |config| {
            config.contained = false;
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
    let stored = f.h.store.mcp_servers(&sid()).await?;
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
    assert!(!config.join("launch-settings").exists());
    assert_eq!(f.h.store.mcp_servers(&sid()).await?.len(), 3);
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
        })
        .await?;
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
    fn install_skills(&self, skills: &[(String, PathBuf)], claude_dir: &Path) -> AppResult<()> {
        self.inner.install_skills(skills, claude_dir)
    }
}

#[tokio::test]
async fn a_contained_cockpit_keeps_its_config_in_the_users_home() -> TestResult {
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
    let data = h.app.config.data_dir.clone();
    let repo = data.join("sandboxes/s1/repo");
    std::fs::create_dir_all(&repo)?;
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
    assert!(h.app.sandbox_shells(&sid()).is_empty());
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

    // Exactly the management tools (MGT-006).
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
            "session_read",
            "sessions_search",
            "task_archive",
            "task_create",
            "task_delete",
            "task_get",
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

    // Runs it now, then deletes it, and archives and deletes the task (which ended: no
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
