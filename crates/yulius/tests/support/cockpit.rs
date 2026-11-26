//! The Cockpit fixture: a sandbox, its Superagent's fake Claude and reads of its terminal and records.

use crate::support;
use axum::http::{Method, StatusCode};
use futures::{SinkExt as _, StreamExt as _};
use serde_json::{Value, json};
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;
use support::gated::GatedHost;
use support::{
    Creds, Harness, HarnessOptions, TestResult, call, harness_with, perms, seed_github_user,
    seed_sandbox, ws_connect,
};
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};
use yulius_app::model::{AccountRecord, AccountStatus};
use yulius_app::ports::session::SessionHost;
use yulius_app::ports::store::AccountStore;
use yulius_contracts::terminal::{KIND_OUTPUT, KIND_SNAPSHOT, decode_binary};
use yulius_domain::harness::Provider;
use yulius_domain::ids::AccountId;
use yulius_domain::time::Timestamp;

type Socket = WebSocketStream<MaybeTlsStream<TcpStream>>;

pub(crate) const ORIGIN: &str = "https://yulius.test";

pub(crate) const WAIT: Duration = Duration::from_secs(20);

pub(crate) const TOKEN: &str = "synthetic-oauth-token";

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
echo \"interpreter:${JDK_JAVA_OPTIONS:-none}\"\n\
echo \"config:$CLAUDE_CONFIG_DIR\"\n\
echo \"cwd:$(pwd)\"\n\
echo \"ready\"\n\
if [ -f \"$CLAUDE_CONFIG_DIR/exit-3-on-signal\" ]; then\n\
  trap 'exit 3' HUP INT TERM\n\
  while read -r line; do eval \"$line\"; done\n\
  exit 3\n\
fi\n\
exec /bin/sh\n";

pub(crate) struct Fixture {
    pub(crate) h: Harness,
    /// The session host: holds the next start between its spawn and its attach when armed.
    pub(crate) gate: Arc<GatedHost>,
    pub(crate) alice: Creds,
    pub(crate) bob: Creds,
    pub(crate) carol: Creds,
    pub(crate) data: PathBuf,
}

/// alice and carol (write) and bob (read) on `s1`; one pool subscription unless `account` is
/// false; optionally on the data directory of an earlier fixture (a server restart).
pub(crate) async fn fixture_with(
    account: bool,
    data_dir: Option<PathBuf>,
) -> Result<Fixture, Box<dyn std::error::Error>> {
    fixture_ending(account, data_dir, None).await
}

/// [`fixture`] with the sandboxes' browsers from `browser` (SPEC-437).
pub(crate) async fn fixture_with_browser(
    browser: Arc<dyn yulius_app::ports::browser::BrowserPort>,
) -> Result<Fixture, Box<dyn std::error::Error>> {
    fixture_full(true, None, None, Some(browser)).await
}

/// [`fixture_with`], optionally with another wait for a shell's exit before its host is
/// terminated (`shell_exit_ms`).
pub(crate) async fn fixture_ending(
    account: bool,
    data_dir: Option<PathBuf>,
    exit_ms: Option<u64>,
) -> Result<Fixture, Box<dyn std::error::Error>> {
    fixture_full(account, data_dir, exit_ms, None).await
}

async fn fixture_full(
    account: bool,
    data_dir: Option<PathBuf>,
    exit_ms: Option<u64>,
    browser: Option<Arc<dyn yulius_app::ports::browser::BrowserPort>>,
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
        git: Some(Arc::new(crate::support::confined_git())),
        session_host: Some(Arc::clone(&gate) as Arc<dyn SessionHost>),
        harness: Some(harness),
        browser,
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
                email_typed: false,
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

pub(crate) async fn fixture() -> Result<Fixture, Box<dyn std::error::Error>> {
    fixture_with(true, None).await
}

pub(crate) async fn open_cockpit(
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

pub(crate) async fn cockpit_id(
    f: &Fixture,
    creds: &Creds,
) -> Result<String, Box<dyn std::error::Error>> {
    let resp = open_cockpit(f, creds).await?;
    assert_eq!(resp.status, StatusCode::OK, "{:?}", resp.body);
    Ok(resp.body["id"].as_str().ok_or("id")?.to_owned())
}

pub(crate) async fn attach(
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
pub(crate) async fn read_until(
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
pub(crate) fn printed(screen: &str, key: &str) -> Result<String, Box<dyn std::error::Error>> {
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

/// A conversation file of Claude Code, last written `secs` after the epoch.
pub(crate) fn conversation(project: &Path, id: &str, secs: u64) -> TestResult {
    std::fs::create_dir_all(project)?;
    let file = std::fs::File::create(project.join(format!("{id}.jsonl")))?;
    file.set_modified(std::time::UNIX_EPOCH + Duration::from_secs(1_700_000_000 + secs))?;
    Ok(())
}

/// A JSON-RPC call to the internal `/mcp` with `token`.
pub(crate) async fn mcp(
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
pub(crate) async fn cockpit_tool(
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
pub(crate) async fn task_rests(
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

/// The test clock's start (2026-10-07T00:00:00Z) plus `secs`, as Claude Code writes `timestamp`.
pub(crate) fn at(secs: i64) -> String {
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

pub(crate) fn transcript_record(
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

pub(crate) fn prompt_record(uuid: &str, parent: Option<&str>, secs: i64) -> String {
    transcript_record(
        "user",
        uuid,
        parent,
        secs,
        &json!({"message": {"role": "user", "content": "why?"}, "origin": {"kind": "human"}}),
    )
}

pub(crate) fn answer_record(uuid: &str, parent: &str, secs: i64) -> String {
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
pub(crate) fn limit_record(uuid: &str, parent: &str, secs: i64, resets: i64) -> String {
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

pub(crate) async fn insert_account(h: &Harness, id: &str, token: &str, used: f64) -> TestResult {
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
            email_typed: false,
        })
        .await?;
    Ok(())
}

/// Reads the socket until a JSON message of `kind` arrives.
pub(crate) async fn read_message(
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

/// The conversation of the Cockpit whose screen is `screen`: its project directory.
pub(crate) fn project_of(screen: &str) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let config = PathBuf::from(printed(screen, "config")?);
    let cwd = printed(screen, "cwd")?;
    Ok(yulius_harness::paths::claude_project_dir(
        &config,
        Path::new(&cwd),
    ))
}

/// A conversation that hit the weekly limit a day before the Cockpit started and went on, then
/// hits it again now (resetting at `resets`).
pub(crate) fn limited_conversation(resets: i64) -> String {
    [
        prompt_record("u0", None, -86_400),
        limit_record("old-limit", "u0", -86_399, 1_791_000_000),
        answer_record("a0", "old-limit", -86_000),
        prompt_record("u1", Some("a0"), 5),
        limit_record("new-limit", "u1", 6, resets),
    ]
    .concat()
}

/// Makes the record of shell `id` look written by a Yulius from before SPEC-186 (no launch
/// digest).
pub(crate) fn predate_launch_digest(f: &Fixture, id: &str) -> TestResult {
    let path = f.data.join("shells").join(id).join("shell.json");
    let mut record: Value = serde_json::from_str(&std::fs::read_to_string(&path)?)?;
    assert!(record.get("launch").is_some(), "{record}");
    record.as_object_mut().ok_or("record")?.remove("launch");
    std::fs::write(&path, serde_json::to_vec(&record)?)?;
    Ok(())
}
