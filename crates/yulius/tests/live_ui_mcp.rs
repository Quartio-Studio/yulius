//! Opt-in browser check of the MCP page against the real server (SPEC-001 MCP-001, UI-038; #136,
//! "When I click Add mcp server, nothing happens"): with no server attached, the empty state's
//! Add server opens the add page; a pasted `claude mcp add …` line fills it; Add stores the server
//! (its secret sealed, the server tested at once) and a run gets it; an edit and Remove reach the
//! server too.
//!
//! ```sh
//! YULIUS_LIVE_CHROME="/Applications/Google Chrome.app/Contents/MacOS/Google Chrome" \
//!   cargo test -p yulius --test live_ui_mcp -- --ignored
//! ```

mod support;

use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use support::{
    HarnessOptions, TestResult, harness_with, perms, seed_github_user, seed_sandbox,
    setup_superadmin_creds,
};
use yulius_app::model::WebSessionRecord;
use yulius_app::ports::harness::McpAttachment;
use yulius_app::ports::store::{SessionStore, UserStore};
use yulius_browser::{CdpClient, CdpConfig};
use yulius_domain::ids::UserId;

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
            "--window-size=1440,900",
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
        let value = cdp
            .call_session(
                session,
                "Runtime.evaluate",
                json!({ "expression": format!("!!({expression})"), "returnByValue": true }),
            )
            .await?;
        if value["result"]["value"] == Value::Bool(true) {
            return Ok(());
        }
        if Instant::now() > deadline {
            let text = cdp
                .call_session(session, "Runtime.evaluate", json!({ "expression": "document.body.innerText.slice(0, 600)", "returnByValue": true }))
                .await?;
            return Err(format!(
                "timed out waiting for {what}; page: {}",
                text["result"]["value"]
            )
            .into());
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
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
    if let Some(error) = value.get("exceptionDetails") {
        return Err(format!("the page threw: {error}").into());
    }
    Ok(value["result"]["value"].clone())
}

/// Clicks the first `selector` element whose text includes `text`.
fn click(selector: &str, text: &str) -> String {
    format!(
        "(() => {{ const el = [...document.querySelectorAll({selector:?})].find((e) => e.textContent.includes({text:?})); if (!el) throw new Error('nothing to click'); el.click(); return true; }})()"
    )
}

#[tokio::test]
#[ignore = "needs Chrome (YULIUS_LIVE_CHROME)"]
async fn add_server_works_from_an_empty_mcp_page_and_reaches_runs() -> TestResult {
    let Ok(chrome_exe) = std::env::var("YULIUS_LIVE_CHROME") else {
        eprintln!("skipped: set YULIUS_LIVE_CHROME");
        return Ok(());
    };
    // The UI's origin must be the server's: cookie sessions refuse other origins (ADR-14).
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await?;
    let addr = listener.local_addr()?;
    let h = harness_with(HarnessOptions {
        configure: Some(Box::new(move |config| {
            config.public_url = format!("http://{addr}");
            config.secure_cookies = false;
            config.contained = false;
        })),
        ..HarnessOptions::default()
    })?;
    setup_superadmin_creds(&h).await?;
    seed_github_user(&h, "u-alice", "alice").await?;
    let sandbox = seed_sandbox(&h, "s1", "fixture", "u-alice").await?;
    h.github.set_perm("alice", "acme/fixture", perms("write"));
    let user = h
        .store
        .user(&UserId::parse("u-alice")?)
        .await?
        .ok_or("user")?;
    let cookie = "live-ui-mcp-session";
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
        let target = cdp.call("Target.createTarget", json!({ "url": "about:blank" })).await?;
        let target_id = target["targetId"].as_str().ok_or("target")?;
        let attached = cdp
            .call("Target.attachToTarget", json!({ "targetId": target_id, "flatten": true }))
            .await?;
        let session = attached["sessionId"].as_str().ok_or("session")?.to_owned();
        let session = session.as_str();
        cdp.call_session(session, "Network.enable", json!({})).await?;
        cdp.call_session(session, "Page.enable", json!({})).await?;
        cdp.call_session(
            session,
            "Network.setCookie",
            json!({ "name": "yulius_session", "value": cookie, "url": format!("http://{addr}/") }),
        )
        .await?;
        // Uncaught errors are kept for the end: the reported bug was one ("edit is not defined").
        cdp.call_session(session, "Page.addScriptToEvaluateOnNewDocument", json!({ "source": "window.__errors = []; addEventListener('error', (e) => window.__errors.push(String(e.message)));" }))
            .await?;
        cdp.call_session(session, "Page.navigate", json!({ "url": format!("http://{addr}/s/fixture/mcp") }))
            .await?;
        let wait = Duration::from_secs(40);
        until(&cdp, session, "the empty MCP page", "document.querySelector('#main .empty-state') && document.querySelector('#main .empty-state').textContent.includes('No MCP servers yet')", wait).await?;
        eval(&cdp, session, "document.querySelector('#main .empty-state .btn-primary').click(), true").await?;
        until(&cdp, session, "the add page", "location.pathname === '/s/fixture/mcp/new' && document.querySelector('.mc-paste')", wait).await?;

        // Paste what a server's docs give; the form fills itself.
        eval(&cdp, session, "(() => { const box = document.querySelector('.mc-paste'); box.value = 'claude mcp add linear -e LINEAR_KEY=lin-secret-1 -- /nonexistent/linear-mcp --stdio'; box.dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', bubbles: true })); return true; })()").await?;
        until(&cdp, session, "the filled form", "document.querySelector('.wf-name').value === 'linear' && document.querySelector('.mc-conn input.input').value === '/nonexistent/linear-mcp --stdio'", wait).await?;
        eval(&cdp, session, &click(".wf-head button", "Add server")).await?;
        // After Add the server opens and is tested at once (this command cannot start).
        until(&cdp, session, "the server page with its test", "/^\\/s\\/fixture\\/mcp\\/[^/]+$/.test(location.pathname) && !location.pathname.endsWith('/new') && document.querySelector('.mc-test-result') && /Couldn’t|Connected/.test(document.querySelector('.mc-test-result').textContent)", wait).await?;
        let stored = h.app.mcp_attachments(&sandbox).await?;
        if stored
            != [McpAttachment::Stdio {
                name: "linear".into(),
                command: "/nonexistent/linear-mcp".into(),
                args: vec!["--stdio".into()],
                env: vec![("LINEAR_KEY".into(), "lin-secret-1".into())],
            }]
        {
            return Err(format!("a run would get {stored:?}").into());
        }
        let page = eval(&cdp, session, "document.body.innerText").await?;
        if page.as_str().unwrap_or("").contains("lin-secret-1") {
            return Err("the secret is shown back".into());
        }

        // Edit: the command changes, the stored secret stays.
        eval(&cdp, session, "(() => { const i = document.querySelector('.mc-conn input.input'); i.value = '/nonexistent/linear-mcp --stdio --verbose'; i.dispatchEvent(new Event('input', { bubbles: true })); return true; })()").await?;
        eval(&cdp, session, &click(".wf-head button", "Save")).await?;
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let now = h.app.mcp_attachments(&sandbox).await?;
            if matches!(now.first(), Some(McpAttachment::Stdio { args, env, .. }) if args.len() == 2 && env.len() == 1) {
                break;
            }
            if Instant::now() > deadline {
                return Err(format!("the edit did not reach the server: {now:?}").into());
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }

        // Remove.
        eval(&cdp, session, &click(".wf-head button", "Remove")).await?;
        until(&cdp, session, "the confirmation", "[...document.querySelectorAll('dialog[open] button')].some((b) => b.textContent.trim() === 'Remove')", wait).await?;
        eval(&cdp, session, &click("dialog[open] button", "Remove")).await?;
        until(&cdp, session, "the empty list again", "location.pathname === '/s/fixture/mcp' && document.querySelector('#main .empty-state')", wait).await?;
        if !h.app.mcp_attachments(&sandbox).await?.is_empty() {
            return Err("the server was not removed".into());
        }
        let errors = eval(&cdp, session, "window.__errors").await?;
        if errors != json!([]) {
            return Err(format!("the page threw: {errors}").into());
        }
        Ok::<_, Box<dyn std::error::Error>>(())
    }
    .await;
    server.abort();
    result
}
