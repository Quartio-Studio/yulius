//! The sandbox editor reverse proxy end to end against the fake openvscode-server: write
//! access, connection-token cookie injection without exposing the token, header hygiene,
//! WebSocket upgrades, worktree selection and the missing-binary 503 (SPEC-001 EDT-001/002,
//! AC-EDT-1).

mod support;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use serde_json::Value;
use support::{
    Creds, Harness, HarnessOptions, TestResult, harness_with, perms, seed_github_user,
    seed_sandbox, seed_task, serve,
};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::TcpStream;
use yulius::wiring::editor::VsCodeEditors;
use yulius_app::model::WebSessionRecord;
use yulius_app::ports::store::{SessionStore, UserStore};
use yulius_domain::ids::UserId;
use yulius_domain::task::TaskStatus;
use yulius_editor::{EditorConfig, StopTimeouts};

const ORIGIN: &str = "https://yulius.test";
const WAIT: Duration = Duration::from_secs(10);

/// The fake editor binary built by `cargo build -p yulius-editor --features fake` (the
/// workspace test run builds it).
fn fake_editor() -> Result<PathBuf, Box<dyn std::error::Error>> {
    let exe = std::env::current_exe()?;
    exe.ancestors()
        .skip(1)
        .take(4)
        .map(|dir| dir.join("fake-openvscode-server"))
        .find(|candidate| candidate.is_file())
        .ok_or_else(|| {
            "fake-openvscode-server is not built (cargo build -p yulius-editor --features fake)"
                .into()
        })
}

fn editor_config(exe: &Path, data: &Path) -> Result<EditorConfig, Box<dyn std::error::Error>> {
    let start = std::net::TcpListener::bind("127.0.0.1:0")?
        .local_addr()?
        .port();
    Ok(EditorConfig {
        startup_timeout: Duration::from_secs(30),
        stop: StopTimeouts {
            graceful: Duration::ZERO,
            term: Duration::from_secs(2),
            kill: Duration::from_secs(5),
        },
        ..EditorConfig::new(
            exe.to_path_buf(),
            data.join("editors"),
            start..=start.saturating_add(50),
        )
    })
}

struct Fixture {
    h: Harness,
    writer: Creds,
    reader: Creds,
}

/// alice (write) and bob (read) on sandbox `s1` with a cloned repository folder and task `t1`
/// with a worktree; sandbox `s2` has a task too.
async fn fixture(exe: &Path) -> Result<Fixture, Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let data = dir.path().canonicalize()?;
    let editors = VsCodeEditors::new(editor_config(exe, &data)?);
    let h = harness_with(HarnessOptions {
        editor: Some(Arc::new(editors)),
        dir: Some(dir),
        ..HarnessOptions::default()
    })?;
    let writer = seed_github_user(&h, "u-alice", "alice").await?;
    let reader = seed_github_user(&h, "u-bob", "bob").await?;
    seed_sandbox(&h, "s1", "fixture", "u-alice").await?;
    seed_sandbox(&h, "s2", "other", "u-alice").await?;
    for repo in ["acme/fixture", "acme/other"] {
        h.github.set_perm("alice", repo, perms("write"));
        h.github.set_perm("bob", repo, perms("read"));
    }
    std::fs::create_dir_all(data.join("sandboxes/s1/repo"))?;
    let worktree = data.join("sandboxes/s1/worktrees/t1 dir");
    std::fs::create_dir_all(&worktree)?;
    seed_task(
        &h,
        "t1",
        "s1",
        "u-alice",
        TaskStatus::Running,
        Some(&worktree),
    )
    .await?;
    seed_task(&h, "t-bare", "s1", "u-alice", TaskStatus::Queued, None).await?;
    let foreign = data.join("sandboxes/s2/worktrees/t2");
    std::fs::create_dir_all(&foreign)?;
    seed_task(
        &h,
        "t2",
        "s2",
        "u-alice",
        TaskStatus::Running,
        Some(&foreign),
    )
    .await?;
    Ok(Fixture { h, writer, reader })
}

/// A raw HTTP/1.1 exchange: returns the response head (lower-cased header names) and the body
/// read until the server closes or `limit` bytes arrived.
async fn raw_request(
    addr: std::net::SocketAddr,
    request: &str,
) -> Result<(String, String), Box<dyn std::error::Error>> {
    let mut stream = TcpStream::connect(addr).await?;
    stream.write_all(request.as_bytes()).await?;
    let mut bytes = Vec::new();
    let _ = tokio::time::timeout(WAIT, stream.read_to_end(&mut bytes)).await;
    let text = String::from_utf8_lossy(&bytes).into_owned();
    let (head, body) = text.split_once("\r\n\r\n").unwrap_or((&text, ""));
    Ok((head.to_ascii_lowercase(), body.to_owned()))
}

/// Sends `request` and reads only the response head (for keep-alive and upgraded responses).
async fn request_head(
    addr: std::net::SocketAddr,
    request: &str,
) -> Result<(TcpStream, String), Box<dyn std::error::Error>> {
    let mut stream = TcpStream::connect(addr).await?;
    stream.write_all(request.as_bytes()).await?;
    let mut head = Vec::new();
    let mut byte = [0_u8; 1];
    while !head.ends_with(b"\r\n\r\n") {
        let read = tokio::time::timeout(WAIT, stream.read(&mut byte)).await??;
        if read == 0 {
            return Err(format!("closed early: {}", String::from_utf8_lossy(&head)).into());
        }
        head.push(byte[0]);
    }
    Ok((stream, String::from_utf8_lossy(&head).to_ascii_lowercase()))
}

fn get_request(addr: std::net::SocketAddr, path: &str, extra: &str) -> String {
    format!("GET {path} HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n{extra}\r\n")
}

fn bearer(creds: &Creds) -> String {
    format!(
        "Authorization: Bearer {}\r\n",
        creds.bearer.as_deref().unwrap_or_default()
    )
}

fn status_of(head: &str) -> u16 {
    head.split_whitespace()
        .nth(1)
        .and_then(|code| code.parse().ok())
        .unwrap_or(0)
}

fn editor_pid(data: &Path) -> Result<i32, Box<dyn std::error::Error>> {
    Ok(
        std::fs::read_to_string(data.join("editors/s1/user-data/fake-pid"))?
            .trim()
            .parse()?,
    )
}

async fn wait_dead(pid: i32) -> bool {
    let deadline = tokio::time::Instant::now() + WAIT;
    while tokio::time::Instant::now() < deadline {
        if nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid), None).is_err() {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    false
}

#[tokio::test]
async fn the_editor_proxy_serves_write_users_with_the_token_kept_server_side() -> TestResult {
    let f = fixture(&fake_editor()?).await?;
    let result = proxy_scenario(&f).await;
    f.h.app.stop_workspace_services().await;
    result
}

async fn proxy_scenario(f: &Fixture) -> TestResult {
    let served = serve(f.h.router.clone()).await?;
    let addr = served.addr;
    let data = f.h.app.config.data_dir.clone();

    // Without credentials, with read access or for an invisible sandbox: refused.
    let (head, _) = raw_request(addr, &get_request(addr, "/s/s1/editor/", "")).await?;
    assert_eq!(status_of(&head), 401);
    let (head, _) = raw_request(
        addr,
        &get_request(addr, "/s/s1/editor/", &bearer(&f.reader)),
    )
    .await?;
    assert_eq!(status_of(&head), 403, "read users cannot open the editor");
    let (head, _) = raw_request(
        addr,
        &get_request(addr, "/s/nope/editor/", &bearer(&f.writer)),
    )
    .await?;
    assert_eq!(status_of(&head), 404);

    // A write user reaches the editor: the fake answers 200 only with the right token cookie.
    let (head, body) = raw_request(
        addr,
        &get_request(addr, "/s/s1/editor/", &bearer(&f.writer)),
    )
    .await?;
    assert_eq!(status_of(&head), 200, "{head}");
    assert_eq!(body, "fake editor");
    assert!(
        !head.contains("x-fake-hop"),
        "hop-by-hop headers are stripped: {head}"
    );
    let pid = editor_pid(&data)?;
    let argv = std::fs::read_to_string(data.join("editors/s1/user-data/fake-argv.txt"))?;
    assert!(
        argv.contains(&data.join("sandboxes/s1/repo").display().to_string()),
        "{argv}"
    );
    assert!(argv.contains("/s/s1/editor"));

    // What the editor receives: the real token, none of the client's credentials.
    let token = std::fs::read_to_string(data.join("editors/s1/connection-token"))?;
    let extra = format!(
        "{}Cookie: yulius_session=abc; vscode-tkn=forged; theme=dark\r\nX-Yulius-CSRF: csrf\r\n",
        bearer(&f.writer)
    );
    let (head, upstream) = raw_request(
        addr,
        &get_request(addr, "/s/s1/editor/fake-headers", &extra),
    )
    .await?;
    assert_eq!(status_of(&head), 200);
    let upstream = upstream.to_ascii_lowercase();
    assert!(upstream.contains(&format!("vscode-tkn={}", token.trim())));
    assert!(upstream.contains("theme=dark"));
    for leaked in [
        "forged",
        "yulius_session",
        "authorization:",
        "x-yulius-csrf",
        "yul_",
    ] {
        assert!(
            !upstream.contains(leaked),
            "{leaked} must not reach the editor"
        );
    }

    // The token never reaches the browser, other editor cookies do.
    let (head, _) = raw_request(
        addr,
        &get_request(addr, "/s/s1/editor/fake-cookies", &bearer(&f.writer)),
    )
    .await?;
    assert!(head.contains("set-cookie: fake-pref=1"), "{head}");
    assert!(!head.contains("vscode-tkn"), "{head}");
    assert!(!head.contains(token.trim()));

    // `/s/{sid}/editor` gains its trailing slash.
    let (head, _) = raw_request(addr, &get_request(addr, "/s/s1/editor?x=1", "")).await?;
    assert_eq!(status_of(&head), 308);
    assert!(head.contains("location: /s/s1/editor/?x=1"));

    // Worktree selection: only worktrees of this sandbox, redirected to `?folder=`.
    let (head, _) = raw_request(
        addr,
        &get_request(addr, "/s/s1/editor/?worktree=t1", &bearer(&f.writer)),
    )
    .await?;
    assert_eq!(status_of(&head), 302);
    let worktree = data
        .join("sandboxes/s1/worktrees/t1 dir")
        .display()
        .to_string();
    assert!(
        head.contains(
            &format!(
                "location: /s/s1/editor/?folder={}",
                worktree.replace(' ', "%20")
            )
            .to_ascii_lowercase()
        ),
        "{head}"
    );
    for (task, status) in [("t2", 404), ("t-bare", 409), ("missing", 404)] {
        let (head, _) = raw_request(
            addr,
            &get_request(
                addr,
                &format!("/s/s1/editor/?worktree={task}"),
                &bearer(&f.writer),
            ),
        )
        .await?;
        assert_eq!(status_of(&head), status, "worktree={task}");
    }

    // WebSocket upgrades are proxied byte for byte; cross-origin and read users are refused.
    let upgrade = |creds: &Creds, origin: &str| {
        format!(
            "GET /s/s1/editor/remote?reconnectionToken=x HTTP/1.1\r\nHost: {addr}\r\n{}Origin: {origin}\r\nConnection: Upgrade\r\nUpgrade: websocket\r\nSec-WebSocket-Version: 13\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\r\n",
            bearer(creds)
        )
    };
    let (mut stream, head) = request_head(addr, &upgrade(&f.writer, ORIGIN)).await?;
    assert_eq!(status_of(&head), 101, "{head}");
    assert!(
        head.contains("sec-websocket-accept: fake-dghlihnhbxbszsbub25jzq=="),
        "{head}"
    );
    assert!(head.contains("upgrade: websocket"));
    stream.write_all(b"ping-through-proxy").await?;
    let mut echo = vec![0_u8; "ping-through-proxy".len()];
    tokio::time::timeout(WAIT, stream.read_exact(&mut echo)).await??;
    assert_eq!(echo, b"ping-through-proxy");
    drop(stream);
    let (_, head) = request_head(addr, &upgrade(&f.writer, "https://evil.test")).await?;
    assert_eq!(status_of(&head), 403);
    let (_, head) = request_head(addr, &upgrade(&f.reader, ORIGIN)).await?;
    assert_eq!(status_of(&head), 403);

    // Oversized bodies are refused before reaching the editor.
    let post = format!(
        "POST /s/s1/editor/upload HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n{}Content-Length: 99999999\r\n\r\n",
        bearer(&f.writer)
    );
    let (head, _) = raw_request(addr, &post).await?;
    assert_eq!(status_of(&head), 413);

    // Server shutdown stops and reaps the editor.
    f.h.app.stop_workspace_services().await;
    assert!(wait_dead(pid).await, "editor reaped");
    Ok(())
}

#[tokio::test]
async fn cookie_sessions_must_be_same_origin_for_state_changing_editor_requests() -> TestResult {
    let f = fixture(&fake_editor()?).await?;
    let result = async {
        let served = serve(f.h.router.clone()).await?;
        let addr = served.addr;
        let user = f
            .h
            .store
            .user(&UserId::parse("u-alice")?)
            .await?
            .ok_or("user")?;
        let cookie = "editor-session-cookie-value";
        f.h.store
            .insert_web_session(&WebSessionRecord {
                id_hash: f.h.app.deps.crypto.sha256_hex(cookie.as_bytes()),
                user_id: user.id.clone(),
                csrf_token: "csrf".into(),
                created_at: f.h.app.now(),
                expires_at: f.h.app.now().plus_millis(3_600_000),
                revoked_at: None,
            })
            .await?;
        let post = |origin: Option<&str>| {
            let origin = origin.map(|o| format!("Origin: {o}\r\n")).unwrap_or_default();
            format!(
                "POST /s/s1/editor/action HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\nCookie: yulius_session={cookie}\r\n{origin}Content-Length: 2\r\n\r\n{{}}"
            )
        };
        let (head, _) = raw_request(addr, &post(None)).await?;
        assert_eq!(status_of(&head), 403, "no origin: refused");
        let (head, _) = raw_request(addr, &post(Some("https://evil.test"))).await?;
        assert_eq!(status_of(&head), 403, "cross-site: refused");
        let (head, body) = raw_request(addr, &post(Some(ORIGIN))).await?;
        assert_eq!(status_of(&head), 200, "same origin: proxied {head}");
        assert_eq!(body, "fake editor");
        let (head, _) = raw_request(
            addr,
            &get_request(addr, "/s/s1/editor/", &format!("Cookie: yulius_session={cookie}\r\n")),
        )
        .await?;
        assert_eq!(status_of(&head), 200, "cookie GET");
        Ok::<_, Box<dyn std::error::Error>>(())
    }
    .await;
    f.h.app.stop_workspace_services().await;
    result
}

#[tokio::test]
async fn a_missing_openvscode_server_or_repository_is_reported_clearly() -> TestResult {
    let f = fixture(Path::new("/nonexistent/yulius-test/openvscode-server")).await?;
    let served = serve(f.h.router.clone()).await?;
    let addr = served.addr;
    let (head, body) = raw_request(
        addr,
        &get_request(addr, "/s/s1/editor/", &bearer(&f.writer)),
    )
    .await?;
    assert_eq!(status_of(&head), 503);
    let error: Value = serde_json::from_str(&body)?;
    assert_eq!(error["error"]["code"], "unavailable");
    assert!(
        error["error"]["message"]
            .as_str()
            .is_some_and(|m| m.contains("openvscode-server executable not found")),
        "{error}"
    );

    // A sandbox whose repository is not cloned yet cannot open the editor.
    let (head, body) = raw_request(
        addr,
        &get_request(addr, "/s/s2/editor/", &bearer(&f.writer)),
    )
    .await?;
    assert_eq!(status_of(&head), 409, "{body}");
    Ok(())
}
