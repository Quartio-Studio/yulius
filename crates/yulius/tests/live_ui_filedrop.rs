//! Opt-in browser check of dropped files (SPEC-001 EDT-006, UI-048, AC-EDT-6, AC-UI-43;
//! F 2026-10-10): the real UI in headless Chrome, a real server and the fake agent in a real
//! detached ptyhost. A file from disk dragged with the browser's own drag and drop
//! (`Input.dispatchDragEvent` with files) onto the explorer's `src` folder lands in the
//! repository byte for byte; a PDF dropped onto the task terminal after `/read ` is uploaded as an
//! attachment, its path typed into the terminal, and the agent reads it by path.
//!
//! ```sh
//! YULIUS_LIVE_CHROME="/Applications/Google Chrome.app/Contents/MacOS/Google Chrome" \
//!   cargo test -p yulius --test main live_ui_filedrop:: -- --ignored --nocapture
//! ```

use crate::support;

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use support::agents::{FakeAgentHarness, FakeAgentOptions};
use support::{
    HarnessOptions, TestResult, call, harness_with, perms, seed_github_user, seed_sandbox,
    setup_superadmin_creds,
};
use yulius::wiring::git::GitAdapter;
use yulius::wiring::session::PtySessionHost;
use yulius_app::model::{AccountRecord, AccountStatus, WebSessionRecord};
use yulius_app::ports::store::{AccountStore, SessionStore, UserStore};
use yulius_browser::{CdpClient, CdpConfig};
use yulius_domain::harness::Provider;
use yulius_domain::ids::{AccountId, UserId};
use yulius_domain::time::Timestamp;
use yulius_ptyhost::test_support::locate_fake_agent;

struct Chrome(Child);

impl Drop for Chrome {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

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
            "--window-size=1500,950",
        ])
        .arg(format!("--user-data-dir={}", profile.display()))
        .arg("about:blank")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    let chrome = Chrome(child);
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        if let Ok(text) = std::fs::read_to_string(profile.join("DevToolsActivePort")) {
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

async fn eval(
    cdp: &CdpClient,
    session: &str,
    expression: &str,
) -> Result<Value, Box<dyn std::error::Error>> {
    let value = cdp
        .call_session(
            session,
            "Runtime.evaluate",
            json!({ "expression": expression, "returnByValue": true, "awaitPromise": true }),
        )
        .await?;
    Ok(value["result"]["value"].clone())
}

/// Evaluates `expression` until it returns `true` or `timeout` passes.
async fn until(
    cdp: &CdpClient,
    session: &str,
    what: &str,
    expression: &str,
    timeout: Duration,
) -> TestResult {
    let deadline = Instant::now() + timeout;
    loop {
        if eval(cdp, session, &format!("!!({expression})")).await? == Value::Bool(true) {
            return Ok(());
        }
        if Instant::now() > deadline {
            let text = eval(cdp, session, "document.body.innerText.slice(0, 800)").await?;
            return Err(format!("timed out waiting for {what}; page: {text}").into());
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

/// The centre of the first element matching `selector`, in CSS pixels.
async fn centre(
    cdp: &CdpClient,
    session: &str,
    selector: &str,
) -> Result<(f64, f64), Box<dyn std::error::Error>> {
    let point = eval(
        cdp,
        session,
        &format!("(() => {{ const r = document.querySelector({selector:?}).getBoundingClientRect(); return [r.left + r.width / 2, r.top + r.height / 2]; }})()"),
    )
    .await?;
    Ok((point[0].as_f64().ok_or("x")?, point[1].as_f64().ok_or("y")?))
}

/// A real drag of `files` from the computer to `(x, y)`: enter, a move over the target, drop.
async fn drag_files(
    cdp: &CdpClient,
    session: &str,
    (x, y): (f64, f64),
    files: &[&Path],
    then_drop: bool,
) -> TestResult {
    let data = json!({
        "items": [],
        "files": files.iter().map(|f| f.to_string_lossy().into_owned()).collect::<Vec<_>>(),
        "dragOperationsMask": 1,
    });
    let kinds: &[&str] = if then_drop {
        &["dragEnter", "dragOver", "drop"]
    } else {
        &["dragEnter", "dragOver"]
    };
    for kind in kinds {
        cdp.call_session(
            session,
            "Input.dispatchDragEvent",
            json!({ "type": kind, "x": x, "y": y, "data": data }),
        )
        .await?;
    }
    Ok(())
}

fn git(dir: &Path, args: &[&str]) -> TestResult {
    let out = Command::new("git")
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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "needs Chrome (YULIUS_LIVE_CHROME)"]
async fn files_dragged_from_disk_reach_the_repository_and_the_agent() -> TestResult {
    let Ok(chrome_exe) = std::env::var("YULIUS_LIVE_CHROME") else {
        eprintln!("skipped: set YULIUS_LIVE_CHROME");
        return Ok(());
    };
    let fake = locate_fake_agent().ok_or("fake-agent is not built (cargo build -p fake-agent)")?;
    let exe = PathBuf::from(env!("CARGO_BIN_EXE_yulius"));
    // The UI's origin must be the server's: cookie sessions refuse other origins (ADR-14).
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await?;
    let addr = listener.local_addr()?;
    let dir = tempfile::Builder::new().prefix("ylf").tempdir_in("/tmp")?;
    let h = harness_with(HarnessOptions {
        git: Some(Arc::new(GitAdapter::new("git"))),
        session_host: Some(Arc::new(PtySessionHost::new(exe.clone()))),
        harness: Some(Arc::new(FakeAgentHarness::new(
            exe,
            fake,
            FakeAgentOptions::default(),
        ))),
        dir: Some(dir),
        configure: Some(Box::new(move |config| {
            config.public_url = format!("http://{addr}");
            config.secure_cookies = false;
            config.contained = false;
        })),
        ..HarnessOptions::default()
    })?;
    setup_superadmin_creds(&h).await?;
    let alice = seed_github_user(&h, "u-alice", "alice").await?;
    let sandbox = seed_sandbox(&h, "s1", "fixture", "u-alice").await?;
    h.github.set_perm("alice", "acme/fixture", perms("write"));
    h.store
        .insert_account(&AccountRecord {
            id: AccountId::parse("a1")?,
            provider: Provider::Anthropic,
            label: "Claude".into(),
            credential: Some(h.app.deps.crypto.seal(b"sk-ant-oat01-live-filedrop")?),
            status: AccountStatus::Ok,
            usage_json: None,
            last_error: None,
            created_at: Timestamp(1),
            updated_at: Timestamp(1),
            email: None,
            email_checked_at: None,
        })
        .await?;
    let data = h.app.config.data_dir.clone();
    let origin = data.join("origin.git");
    std::fs::create_dir_all(&origin)?;
    git(&origin, &["init", "--bare", "-q", "-b", "main"])?;
    let seed = data.join("seed");
    std::fs::create_dir_all(seed.join("src"))?;
    git(&seed, &["init", "-q", "-b", "main"])?;
    std::fs::write(seed.join("README.md"), "# fixture\n")?;
    std::fs::write(seed.join("src/app.js"), "console.log('hi');\n")?;
    git(&seed, &["add", "."])?;
    git(&seed, &["commit", "-q", "-m", "init"])?;
    git(
        &seed,
        &["remote", "add", "origin", &origin.to_string_lossy()],
    )?;
    git(&seed, &["push", "-q", "origin", "main"])?;
    let repo = data
        .join("sandboxes")
        .join(sandbox.id.as_str())
        .join("repo");
    std::fs::create_dir_all(repo.parent().ok_or("parent")?)?;
    git(
        &data,
        &[
            "clone",
            "-q",
            &origin.to_string_lossy(),
            &repo.to_string_lossy(),
        ],
    )?;
    let created = call(
        &h.router,
        axum::http::Method::POST,
        "/api/sandboxes/s1/tasks",
        Some(json!({"prompt": "Read what I drop", "harness": "claude", "mode": "prompt"})),
        &alice,
    )
    .await?;
    assert_eq!(created.status, 201, "{:?}", created.body);
    let number = created.body["number"].as_u64().ok_or("task number")?;
    let user = h
        .store
        .user(&UserId::parse("u-alice")?)
        .await?
        .ok_or("user")?;
    let cookie = "live-ui-filedrop-session";
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
    // Files on the user's computer.
    let desktop = tempfile::tempdir()?;
    let logo = desktop.path().join("logo.bin");
    let bytes: Vec<u8> = (0..=255u8).cycle().take(300_000).collect();
    std::fs::write(&logo, &bytes)?;
    let brief = desktop.path().join("Project brief.pdf");
    std::fs::write(&brief, b"%PDF-1.4 a brief for the agent\n")?;
    let router = h.router.clone();
    let server = tokio::spawn(async move {
        let _ = axum::serve(listener, router).await;
    });

    let result = async {
        let profile = tempfile::tempdir()?;
        let (_chrome, ws_url) = start_chrome(&chrome_exe, profile.path()).await?;
        let cdp = CdpClient::connect(&ws_url, CdpConfig::default()).await?;
        let target = cdp.call("Target.createTarget", json!({ "url": "about:blank" })).await?;
        let target_id = target["targetId"].as_str().ok_or("target")?;
        let attached = cdp
            .call("Target.attachToTarget", json!({ "targetId": target_id, "flatten": true }))
            .await?;
        let session = attached["sessionId"].as_str().ok_or("session")?.to_owned();
        let session = session.as_str();
        cdp.call_session(session, "Network.enable", json!({})).await?;
        cdp.call_session(
            session,
            "Network.setCookie",
            json!({ "name": "yulius_session", "value": cookie, "url": format!("http://{addr}/") }),
        )
        .await?;
        cdp.call_session(session, "Page.navigate", json!({ "url": format!("http://{addr}/s/fixture/t/{number}") }))
            .await?;
        let wait = Duration::from_secs(40);
        until(&cdp, session, "the agent in the terminal", "document.querySelector('.taskview-term .xterm-rows') && document.querySelector('.taskview-term .xterm-rows').textContent.includes('FAKE-AGENT READY')", wait).await?;

        // The editor: a real drag of a file from disk onto the src folder.
        eval(&cdp, session, "document.querySelector('.tools-toggle').click(); document.querySelector('.rail-btn[aria-label=\"Editor\"]').click(); true").await?;
        until(&cdp, session, "the explorer", "document.querySelector('.ed-row[data-path=\"src\"]')", wait).await?;
        let src = centre(&cdp, session, ".ed-row[data-path=\"src\"]").await?;
        drag_files(&cdp, session, src, &[&logo], false).await?;
        let label = eval(&cdp, session, "(document.querySelector('.ed-side .drop-overlay') || {}).textContent || ''").await?;
        if label != json!("Drop to add to src/") {
            return Err(format!("unexpected drop overlay: {label}").into());
        }
        drag_files(&cdp, session, src, &[&logo], true).await?;
        let deadline = Instant::now() + Duration::from_secs(20);
        while std::fs::read(repo.join("src/logo.bin")).ok().as_deref() != Some(bytes.as_slice()) {
            if Instant::now() > deadline {
                return Err("the dropped file did not reach the repository".into());
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
        until(&cdp, session, "the file in the explorer", "document.querySelector('.ed-row[data-path=\"src/logo.bin\"]')", wait).await?;

        // The task terminal: `/read `, then a PDF dropped onto it, then Enter.
        eval(&cdp, session, "document.querySelector('.rail-btn[aria-label=\"Editor\"]').click(); document.querySelector('.taskview-term .xterm-helper-textarea').focus(); true").await?;
        cdp.call_session(session, "Input.insertText", json!({ "text": "/read " })).await?;
        let term = centre(&cdp, session, ".taskview-term .xterm-screen").await?;
        drag_files(&cdp, session, term, &[&brief], false).await?;
        let label = eval(&cdp, session, "(document.querySelector('.taskview-term .drop-overlay') || {}).textContent || ''").await?;
        if label != json!("Drop to send to Claude") {
            return Err(format!("unexpected terminal overlay: {label}").into());
        }
        drag_files(&cdp, session, term, &[&brief], true).await?;
        until(&cdp, session, "the path typed into the terminal", "document.querySelector('.taskview-term .xterm-rows').textContent.includes('Project-brief.pdf')", wait).await?;
        for kind in ["keyDown", "keyUp"] {
            cdp.call_session(
                session,
                "Input.dispatchKeyEvent",
                json!({ "type": kind, "key": "Enter", "code": "Enter", "windowsVirtualKeyCode": 13, "text": if kind == "keyDown" { "\r" } else { "" } }),
            )
            .await?;
        }
        until(&cdp, session, "the agent reading the attachment", "document.querySelector('.taskview-term .xterm-rows').textContent.includes('READ Project-brief.pdf OK 31')", wait).await?;
        let folder = data.join("sandboxes/s1/attachments/tasks");
        let stored: Vec<PathBuf> = std::fs::read_dir(&folder)?
            .flatten()
            .flat_map(|task| std::fs::read_dir(task.path()).into_iter().flatten().flatten())
            .map(|entry| entry.path())
            .collect();
        if stored.len() != 1 || !stored[0].ends_with("Project-brief.pdf") {
            return Err(format!("unexpected attachments: {stored:?}").into());
        }
        Ok::<_, Box<dyn std::error::Error>>(())
    }
    .await;
    server.abort();
    result
}
