//! Opt-in live check of the sandbox editor: the real openvscode-server behind the Yulius
//! reverse proxy boots in headless Chrome with a cookie session, and its remote connection
//! lists the repository (SPEC-001 EDT-001, AC-EDT-1).
//!
//! Ignored by default. Run on Linux with both prerequisites installed:
//!
//! ```sh
//! YULIUS_LIVE_OPENVSCODE=/opt/openvscode-server/bin/openvscode-server \
//! YULIUS_LIVE_CHROME=google-chrome \
//!   cargo test -p yulius --test live_editor -- --ignored
//! ```

mod support;

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use support::{HarnessOptions, TestResult, harness_with, perms, seed_github_user, seed_sandbox};
use yulius::wiring::editor::VsCodeEditors;
use yulius_app::model::WebSessionRecord;
use yulius_app::ports::store::{SessionStore, UserStore};
use yulius_browser::{CdpClient, CdpConfig};
use yulius_domain::ids::UserId;
use yulius_editor::EditorConfig;

/// Marker file the editor's explorer must list once the remote connection works.
const MARKER: &str = "hello-yulius.txt";
/// Upper bound for the editor to boot and list the repository.
const BOOT: Duration = Duration::from_secs(90);

/// Headless Chrome with a private profile; killed on drop.
struct Chrome {
    child: Child,
}

impl Drop for Chrome {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Starts Chrome with an OS-assigned DevTools port and returns its browser WebSocket URL.
async fn start_chrome(
    exe: &str,
    profile: &Path,
) -> Result<(Chrome, String), Box<dyn std::error::Error>> {
    let child = Command::new(exe)
        .args([
            "--headless=new",
            "--no-sandbox",
            "--disable-gpu",
            "--no-first-run",
            "--remote-debugging-port=0",
            "--window-size=1400,900",
        ])
        .arg(format!("--user-data-dir={}", profile.display()))
        .arg("about:blank")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    let chrome = Chrome { child };
    let port_file = profile.join("DevToolsActivePort");
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        if let Ok(text) = std::fs::read_to_string(&port_file) {
            let mut lines = text.lines();
            if let (Some(port), Some(path)) = (lines.next(), lines.next()) {
                return Ok((chrome, format!("ws://127.0.0.1:{port}{path}")));
            }
        }
        if Instant::now() > deadline {
            return Err("Chrome did not publish its DevTools port".into());
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// `remoteagent.log` files below the editor's log directory.
fn glob_logs(root: &Path) -> Vec<PathBuf> {
    std::fs::read_dir(root)
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.path().join("remoteagent.log"))
        .filter(|path| path.is_file())
        .collect()
}

fn free_port_range() -> Result<std::ops::RangeInclusive<u16>, Box<dyn std::error::Error>> {
    let start = std::net::TcpListener::bind("127.0.0.1:0")?
        .local_addr()?
        .port();
    Ok(start..=start.saturating_add(20))
}

#[tokio::test]
#[ignore = "needs openvscode-server and Chrome (YULIUS_LIVE_OPENVSCODE, YULIUS_LIVE_CHROME)"]
async fn the_real_editor_boots_through_the_proxy_in_chrome() -> TestResult {
    let (Ok(editor_exe), Ok(chrome_exe)) = (
        std::env::var("YULIUS_LIVE_OPENVSCODE"),
        std::env::var("YULIUS_LIVE_CHROME"),
    ) else {
        eprintln!("skipped: set YULIUS_LIVE_OPENVSCODE and YULIUS_LIVE_CHROME");
        return Ok(());
    };
    let dir = tempfile::tempdir()?;
    let data = dir.path().canonicalize()?;
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await?;
    let addr = listener.local_addr()?;
    let origin = format!("http://{addr}");
    let editors = VsCodeEditors::new(EditorConfig::new(
        PathBuf::from(editor_exe),
        data.join("editors"),
        free_port_range()?,
    ));
    let public_url = origin.clone();
    let h = harness_with(HarnessOptions {
        editor: Some(Arc::new(editors)),
        dir: Some(dir),
        configure: Some(Box::new(move |config| config.public_url = public_url)),
        ..HarnessOptions::default()
    })?;
    seed_github_user(&h, "u-alice", "alice").await?;
    seed_sandbox(&h, "s1", "fixture", "u-alice").await?;
    h.github.set_perm("alice", "acme/fixture", perms("write"));
    let repo = data.join("sandboxes/s1/repo");
    std::fs::create_dir_all(&repo)?;
    std::fs::write(repo.join(MARKER), "hello\n")?;
    let user = h
        .store
        .user(&UserId::parse("u-alice")?)
        .await?
        .ok_or("user")?;
    let cookie = "live-editor-session";
    h.store
        .insert_web_session(&WebSessionRecord {
            id_hash: h.app.deps.crypto.sha256_hex(cookie.as_bytes()),
            user_id: user.id,
            csrf_token: "csrf".into(),
            created_at: h.app.now(),
            expires_at: h.app.now().plus_millis(3_600_000),
            revoked_at: None,
        })
        .await?;
    let router = h.router.clone();
    let server = tokio::spawn(async move {
        let _ = axum::serve(listener, router).await;
    });

    let result = async {
        let profile = tempfile::tempdir()?;
        let (_chrome, ws_url) = start_chrome(&chrome_exe, profile.path()).await?;
        let cdp = CdpClient::connect(&ws_url, CdpConfig::default()).await?;
        let target = cdp
            .call("Target.createTarget", json!({ "url": "about:blank" }))
            .await?;
        let target_id = target["targetId"].as_str().ok_or("target id")?;
        let attached = cdp
            .call(
                "Target.attachToTarget",
                json!({ "targetId": target_id, "flatten": true }),
            )
            .await?;
        let session = attached["sessionId"].as_str().ok_or("session id")?;
        let mut events = cdp.subscribe_events();
        let seen = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
        let sink = Arc::clone(&seen);
        let diagnostics = tokio::spawn(async move {
            while let Ok(event) = events.recv().await {
                let p = &event.params;
                let line = match event.method.as_str() {
                    "Network.webSocketCreated" => format!("ws created {}", p["url"]),
                    "Network.webSocketHandshakeResponseReceived" => {
                        format!("ws handshake {}", p["response"]["status"])
                    }
                    "Network.webSocketClosed" => "ws closed".to_owned(),
                    "Network.webSocketFrameSent" | "Network.webSocketFrameReceived" => format!(
                        "{} opcode={} len={}",
                        if event.method.ends_with("Sent") { "sent" } else { "recv" },
                        p["response"]["opcode"],
                        p["response"]["payloadData"].as_str().map_or(0, str::len)
                    ),
                    "Network.webSocketFrameError" => format!("ws frame error {}", p["errorMessage"]),
                    "Network.responseReceived" if p["response"]["status"].as_u64() >= Some(400) => {
                        format!("http {} {}", p["response"]["status"], p["response"]["url"])
                    }
                    "Network.loadingFailed" => format!("failed {} {}", p["errorText"], p["type"]),
                    "Log.entryAdded" => format!("log {} {}", p["entry"]["level"], p["entry"]["text"]),
                    "Runtime.exceptionThrown" => {
                        format!("exception {}", p["exceptionDetails"]["exception"]["description"])
                    }
                    _ => continue,
                };
                if let Ok(mut lines) = sink.lock()
                    && lines.len() < 200
                {
                    lines.push(line);
                }
            }
        });
        cdp.call_session(session, "Network.enable", json!({})).await?;
        cdp.call_session(session, "Log.enable", json!({})).await?;
        cdp.call_session(session, "Runtime.enable", json!({})).await?;
        let set = cdp
            .call_session(
                session,
                "Network.setCookie",
                json!({ "name": "yulius_session", "value": cookie, "url": format!("{origin}/") }),
            )
            .await?;
        assert_eq!(set["success"], Value::Bool(true), "cookie set");
        cdp.call_session(
            session,
            "Page.navigate",
            json!({ "url": format!("{origin}/s/s1/editor/") }),
        )
        .await?;
        let probe = format!(
            "!!document.querySelector('.monaco-workbench') && document.body.innerText.includes('{MARKER}')"
        );
        let deadline = Instant::now() + BOOT;
        let mut booted = false;
        while Instant::now() < deadline {
            let value = cdp
                .call_session(
                    session,
                    "Runtime.evaluate",
                    json!({ "expression": probe, "returnByValue": true }),
                )
                .await?;
            if value["result"]["value"] == Value::Bool(true) {
                booted = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
        let text = cdp
            .call_session(
                session,
                "Runtime.evaluate",
                json!({ "expression": "document.body ? document.body.innerText.slice(0, 400) : ''", "returnByValue": true }),
            )
            .await?;
        diagnostics.abort();
        let mut seen = seen.lock().map(|lines| lines.clone()).unwrap_or_default();
        if !booted {
            for log in glob_logs(&data.join("editors/s1/user-data/logs")) {
                seen.push(format!("--- {}", log.display()));
                seen.extend(std::fs::read_to_string(&log).unwrap_or_default().lines().map(str::to_owned));
            }
        }
        assert!(
            booted,
            "events: {seen:#?}\nthe workbench did not list {MARKER} within {BOOT:?}; page text: {}",
            text["result"]["value"]
        );
        Ok::<_, Box<dyn std::error::Error>>(())
    }
    .await;
    h.app.stop_workspace_services().await;
    server.abort();
    result
}
