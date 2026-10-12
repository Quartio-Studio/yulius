//! Opt-in browser check of the built-in editor (SPEC-001 EDT-001..005): the real UI in headless
//! Chrome opens the editor panel, lists the repository, opens a file in Monaco with syntax
//! colors and suggestions, saves an edit to disk, and turns word wrap on with Alt+Z (⌥Z) for
//! every open file, which a reload keeps.
//!
//! ```sh
//! YULIUS_LIVE_CHROME="/Applications/Google Chrome.app/Contents/MacOS/Google Chrome" \
//!   cargo test -p yulius --test main live_ui_editor:: -- --ignored
//! ```

use crate::support;

use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use support::{
    HarnessOptions, TestResult, harness_with, perms, seed_github_user, seed_sandbox,
    setup_superadmin_creds,
};
use yulius_app::model::WebSessionRecord;
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
    Ok(value["result"]["value"].clone())
}

#[tokio::test]
#[ignore = "needs Chrome (YULIUS_LIVE_CHROME)"]
async fn the_editor_panel_opens_colors_suggests_saves_and_wraps() -> TestResult {
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
        })),
        ..HarnessOptions::default()
    })?;
    setup_superadmin_creds(&h).await?;
    seed_github_user(&h, "u-alice", "alice").await?;
    seed_sandbox(&h, "s1", "fixture", "u-alice").await?;
    h.github.set_perm("alice", "acme/fixture", perms("write"));
    let repo = h.app.config.data_dir.join("sandboxes/s1/repo");
    std::fs::create_dir_all(repo.join("src"))?;
    std::fs::write(
        repo.join("src/app.js"),
        "const greeting = 'hi';\nconsole.log(greeting);\n",
    )?;
    std::fs::write(
        repo.join("src/tool.py"),
        "def handle_request(payload):\n    return payload\n",
    )?;
    let user = h
        .store
        .user(&UserId::parse("u-alice")?)
        .await?
        .ok_or("user")?;
    let cookie = "live-ui-editor-session";
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
        cdp.call_session(
            session,
            "Network.setCookie",
            json!({ "name": "yulius_session", "value": cookie, "url": format!("http://{addr}/") }),
        )
        .await?;
        cdp.call_session(session, "Page.navigate", json!({ "url": format!("http://{addr}/s/fixture") }))
            .await?;
        let wait = Duration::from_secs(40);
        until(&cdp, session, "the workspace", "document.querySelector('.tools-toggle')", wait).await?;
        // The tool rail is hidden by default; show it and open the editor.
        eval(&cdp, session, "document.querySelector('.tools-toggle').click(); document.querySelector('.rail-btn[aria-label=\"Editor\"]').click(); true").await?;
        until(&cdp, session, "the explorer", "[...document.querySelectorAll('.ed-row')].some(r => r.textContent.includes('src'))", wait).await?;
        eval(&cdp, session, "[...document.querySelectorAll('.ed-row')].find(r => r.textContent.includes('src')).click(); true").await?;
        until(&cdp, session, "src/app.js in the tree", "[...document.querySelectorAll('.ed-row')].some(r => r.textContent.includes('app.js'))", wait).await?;
        eval(&cdp, session, "[...document.querySelectorAll('.ed-row')].find(r => r.textContent.includes('app.js')).click(); true").await?;
        until(&cdp, session, "Monaco with the file", "window.monaco && document.querySelector('.monaco-editor .view-lines') && document.querySelector('.monaco-editor .view-lines').textContent.includes('greeting')", wait).await?;
        // Syntax colors: the tokenizer gives keywords, strings and identifiers distinct classes.
        until(
            &cdp,
            session,
            "syntax colors",
            "new Set([...document.querySelectorAll('.monaco-editor .view-line span[class*=\"mtk\"]')].map(s => s.className)).size >= 3",
            wait,
        )
        .await?;
        // Suggestions from the TypeScript worker: console members.
        eval(&cdp, session, "(() => { const ed = monaco.editor.getEditors()[0]; const m = ed.getModel(); m.setValue(m.getValue() + 'console.'); const last = m.getLineCount(); ed.setPosition({ lineNumber: last, column: m.getLineMaxColumn(last) }); ed.focus(); ed.trigger('test', 'editor.action.triggerSuggest', {}); return true; })()").await?;
        until(&cdp, session, "suggestions", "[...document.querySelectorAll('.suggest-widget .monaco-list-row')].some(r => (r.getAttribute('aria-label') || '').startsWith('assert'))", wait).await?;
        // Save with the keyboard shortcut (Ctrl+S; Cmd+S on macOS).
        eval(&cdp, session, "(() => { const ed = monaco.editor.getEditors()[0]; ed.trigger('test', 'hideSuggestWidget', {}); ed.getModel().setValue('const greeting = \"saved\";\\n'); ed.focus(); return true; })()").await?;
        let modifier = if cfg!(target_os = "macos") { 4 } else { 2 };
        for kind in ["keyDown", "keyUp"] {
            cdp.call_session(
                session,
                "Input.dispatchKeyEvent",
                json!({ "type": kind, "modifiers": modifier, "key": "s", "code": "KeyS", "windowsVirtualKeyCode": 83 }),
            )
            .await?;
        }
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if std::fs::read_to_string(repo.join("src/app.js"))? == "const greeting = \"saved\";\n" {
                break;
            }
            if Instant::now() > deadline {
                return Err("the edit was not saved to disk".into());
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
        // Keyword suggestions for a language without a browser language service.
        eval(&cdp, session, "[...document.querySelectorAll('.ed-row')].find(r => r.textContent.includes('tool.py')).click(); true").await?;
        until(&cdp, session, "the Python file", "monaco.editor.getEditors()[0].getModel() && monaco.editor.getEditors()[0].getModel().getLanguageId() === 'python'", wait).await?;
        eval(&cdp, session, "(() => { const ed = monaco.editor.getEditors()[0]; const m = ed.getModel(); m.setValue(m.getValue() + 'han'); const last = m.getLineCount(); ed.setPosition({ lineNumber: last, column: m.getLineMaxColumn(last) }); ed.focus(); ed.trigger('test', 'editor.action.triggerSuggest', {}); return true; })()").await?;
        until(&cdp, session, "Python word suggestions", "[...document.querySelectorAll('.suggest-widget .monaco-list-row')].some(r => (r.getAttribute('aria-label') || '').startsWith('handle_request'))", wait).await?;

        // Word wrap (EDT-005, F 2026-10-09): off by default; Alt+Z (⌥Z on macOS, which types "Ω"
        // unless caught) turns it on for every open file without typing, the toolbar shows it, and
        // a reload keeps it.
        let wrap_state = "(() => { const b = document.querySelector('button[aria-label=\"Word wrap\"]'); const ed = monaco.editor.getEditors()[0]; return `${b && b.getAttribute('aria-pressed')} ${ed.getRawOptions().wordWrap}`; })()";
        let text_before = eval(&cdp, session, "(() => { const ed = monaco.editor.getEditors()[0]; ed.trigger('test', 'hideSuggestWidget', {}); ed.focus(); return ed.getModel().getValue(); })()").await?;
        if eval(&cdp, session, wrap_state).await? != json!("false off") {
            return Err("word wrap was not off by default".into());
        }
        let key = if cfg!(target_os = "macos") { "Ω" } else { "z" };
        for kind in ["keyDown", "keyUp"] {
            cdp.call_session(
                session,
                "Input.dispatchKeyEvent",
                json!({ "type": kind, "modifiers": 1, "key": key, "code": "KeyZ", "windowsVirtualKeyCode": 90, "text": if kind == "keyDown" { key } else { "" } }),
            )
            .await?;
        }
        until(&cdp, session, "word wrap on", &format!("{wrap_state} === 'true on'"), wait).await?;
        if eval(&cdp, session, "monaco.editor.getEditors()[0].getModel().getValue()").await? != text_before {
            return Err("Alt+Z typed into the file".into());
        }
        // One switch for every open file: the JavaScript tab is wrapped too.
        eval(&cdp, session, "[...document.querySelectorAll('.ed-tab')].find(t => t.textContent.includes('app.js')).click(); true").await?;
        until(&cdp, session, "app.js wrapped", &format!("monaco.editor.getEditors()[0].getModel().getLanguageId() === 'javascript' && {wrap_state} === 'true on'"), wait).await?;
        // Kept per user in this browser: after a reload the editor opens wrapped.
        cdp.call_session(session, "Page.reload", json!({})).await?;
        until(&cdp, session, "the rail after the reload", "document.querySelector('.rail-btn[aria-label=\"Editor\"]') && document.querySelector('.rail-btn[aria-label=\"Editor\"]').offsetParent", wait).await?;
        eval(&cdp, session, "document.querySelector('.rail-btn[aria-label=\"Editor\"]').click(); true").await?;
        until(&cdp, session, "the explorer after the reload", "[...document.querySelectorAll('.ed-row')].some(r => r.textContent.includes('src'))", wait).await?;
        eval(&cdp, session, "[...document.querySelectorAll('.ed-row')].find(r => r.textContent.includes('src')).click(); true").await?;
        until(&cdp, session, "app.js after the reload", "[...document.querySelectorAll('.ed-row')].some(r => r.textContent.includes('app.js'))", wait).await?;
        eval(&cdp, session, "[...document.querySelectorAll('.ed-row')].find(r => r.textContent.includes('app.js')).click(); true").await?;
        until(&cdp, session, "Monaco wrapped after the reload", &format!("window.monaco && monaco.editor.getEditors()[0] && monaco.editor.getEditors()[0].getModel() && {wrap_state} === 'true on'"), wait).await?;
        Ok::<_, Box<dyn std::error::Error>>(())
    }
    .await;
    server.abort();
    result
}
