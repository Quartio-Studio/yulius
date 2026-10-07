//! Opt-in smoke test against a real Chromium (never in CI): set `YULIUS_TEST_CHROMIUM` to a
//! Chromium or Google Chrome executable. Without it the test returns immediately.
//!
//! It serves a local page on loopback, then drives the browser through the adapter: open and
//! load, title from target events, accessibility snapshot, type, click by selector, PNG
//! screenshot, screencast frames, URL policy, and kill-and-reap on shutdown.

use std::time::Duration;

use base64::Engine as _;
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::TcpListener;
use yulius_browser::{BrowserConfig, BrowserError, BrowserManager};
use yulius_contracts::browser::{
    ClickParams, OpenParams, ScreenshotParams, SnapshotParams, TypeParams,
};
use yulius_domain::ids::SandboxId;

type TestResult = Result<(), Box<dyn std::error::Error>>;

const PAGE: &str = "<!doctype html><html><head><title>Yulius test page</title></head><body>\
<h1>Hello</h1><input id=\"name\" aria-label=\"Name\">\
<button id=\"press\" onclick=\"document.getElementById('out').textContent='clicked '+document.getElementById('name').value\">Press</button>\
<p id=\"out\">waiting</p></body></html>";

async fn serve(listener: TcpListener) {
    while let Ok((mut stream, _)) = listener.accept().await {
        tokio::spawn(async move {
            let mut request = [0_u8; 4096];
            let _ = stream.read(&mut request).await;
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{PAGE}",
                PAGE.len()
            );
            let _ = stream.write_all(response.as_bytes()).await;
        });
    }
}

#[tokio::test]
async fn drives_a_real_chromium_when_configured() -> TestResult {
    let Some(exe) = std::env::var_os("YULIUS_TEST_CHROMIUM") else {
        eprintln!("skipped: set YULIUS_TEST_CHROMIUM to a Chromium executable to run");
        return Ok(());
    };
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let page_url = format!("http://127.0.0.1:{}/", listener.local_addr()?.port());
    let server = tokio::spawn(serve(listener));
    let dir = tempfile::tempdir()?;
    let config = BrowserConfig {
        startup_timeout: Duration::from_secs(60),
        ..BrowserConfig::new(exe.into(), dir.path().to_path_buf())
    };
    let manager = BrowserManager::new(config);
    let sandbox = SandboxId::parse("real-smoke")?;
    let browser = manager
        .ensure(&sandbox, &manager.default_profile_dir(&sandbox))
        .await?;
    let pid = browser.pid().ok_or("no pid")?;
    assert!(browser.cdp_ws_url().starts_with("ws://127.0.0.1:"));

    let opened = browser
        .tool_open(&OpenParams {
            url: page_url.clone(),
        })
        .await?;
    assert!(opened.loaded, "load event within the timeout");
    let tab = opened.tab.id.clone();
    eprintln!("step: opened {opened:?}");
    let early = browser
        .tool_snapshot(&SnapshotParams {
            tab_id: tab.clone(),
        })
        .await?;
    eprintln!("step: early snapshot {early:?}");
    let targets = browser
        .cdp()
        .call("Target.getTargets", serde_json::json!({}))
        .await?;
    eprintln!("step: targets {targets}");
    let mut tabs = browser.subscribe_tabs();
    let titled = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if tabs
                .borrow_and_update()
                .iter()
                .any(|t| t.id == tab && t.title == "Yulius test page")
            {
                return true;
            }
            if tabs.changed().await.is_err() {
                return false;
            }
        }
    })
    .await?;
    assert!(titled, "title arrives through target events");
    eprintln!("step: titled");

    let snapshot_params = SnapshotParams {
        tab_id: tab.clone(),
    };
    let snapshot = browser.tool_snapshot(&snapshot_params).await?;
    assert!(
        snapshot.outline.contains("button \"Press\""),
        "{}",
        snapshot.outline
    );
    assert!(
        snapshot.outline.contains("heading \"Hello\""),
        "{}",
        snapshot.outline
    );

    browser
        .tool_type(&TypeParams {
            tab_id: tab.clone(),
            text: "hello".into(),
            selector: Some("#name".into()),
        })
        .await?;
    browser
        .tool_click(&ClickParams {
            tab_id: tab.clone(),
            selector: Some("#press".into()),
            x: None,
            y: None,
        })
        .await?;
    let after = browser.tool_snapshot(&snapshot_params).await?;
    assert!(after.outline.contains("clicked hello"), "{}", after.outline);

    let shot = browser
        .tool_screenshot(&ScreenshotParams {
            tab_id: tab.clone(),
        })
        .await?;
    let png = base64::engine::general_purpose::STANDARD.decode(shot.data_base64)?;
    assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n");

    eprintln!("step: screenshot ok");
    let mut frames = browser.subscribe_frames(&tab).await?;
    browser.navigate(&tab, &format!("{page_url}?again")).await?;
    let frame = tokio::time::timeout(Duration::from_secs(10), frames.recv())
        .await?
        .ok_or("screencast closed")?;
    assert_eq!(&frame.jpeg[..2], &[0xFF, 0xD8]);
    drop(frames);

    assert!(matches!(
        browser.navigate(&tab, "file:///etc/passwd").await,
        Err(BrowserError::UrlRefused(_))
    ));

    assert!(manager.shutdown(&sandbox).await?);
    assert!(
        nix::sys::signal::kill(nix::unistd::Pid::from_raw(i32::try_from(pid)?), None).is_err(),
        "Chromium is reaped after shutdown"
    );
    server.abort();
    Ok(())
}
