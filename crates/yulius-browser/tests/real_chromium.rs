//! Opt-in smoke test against a real Chromium (never in CI): set `YULIUS_TEST_CHROMIUM` to a
//! Chromium or Google Chrome executable. Without it the test returns immediately.
//!
//! It serves a local page on loopback, then drives the browser through the adapter: open and
//! load, title from target events, accessibility snapshot, type, click by selector, PNG
//! screenshot, screencast frames, URL policy, and kill-and-reap on shutdown. DevTools runs over
//! the pipe: the browser writes no `DevToolsActivePort` and (Linux) listens on no TCP port.

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

/// The process group of `pid` (Linux `/proc/<pid>/stat`: the third field after the command).
#[cfg(target_os = "linux")]
fn process_group(pid: u32) -> Option<u32> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let (_, fields) = stat.rsplit_once(')')?;
    fields.split_whitespace().nth(2)?.parse().ok()
}

/// Listening TCP sockets (IPv4 and IPv6) held by the processes `include` selects.
#[cfg(target_os = "linux")]
fn listening_sockets(
    include: impl Fn(u32) -> bool,
) -> Result<Vec<String>, Box<dyn std::error::Error>> {
    let mut listening = std::collections::HashSet::new();
    for table in ["/proc/net/tcp", "/proc/net/tcp6"] {
        let Ok(text) = std::fs::read_to_string(table) else {
            continue;
        };
        for line in text.lines().skip(1) {
            let fields: Vec<&str> = line.split_whitespace().collect();
            if fields.get(3) == Some(&"0A")
                && let Some(inode) = fields.get(9)
            {
                listening.insert(format!("socket:[{inode}]"));
            }
        }
    }
    let mut found = Vec::new();
    for entry in std::fs::read_dir("/proc")?.flatten() {
        let Some(pid) = entry
            .file_name()
            .to_str()
            .and_then(|n| n.parse::<u32>().ok())
        else {
            continue;
        };
        if !include(pid) {
            continue;
        }
        let Ok(fds) = std::fs::read_dir(format!("/proc/{pid}/fd")) else {
            continue;
        };
        for fd in fds.flatten() {
            if let Ok(target) = std::fs::read_link(fd.path()) {
                let target = target.to_string_lossy().into_owned();
                if listening.contains(&target) {
                    found.push(format!("pid {pid}: {target}"));
                }
            }
        }
    }
    Ok(found)
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
    // DevTools over the pipe only: nothing else can connect to the browser (#626, ADR-0191).
    assert_eq!(browser.cdp().url(), yulius_browser::PIPE_URL);
    #[cfg(target_os = "linux")]
    {
        let own = listening_sockets(|candidate| candidate == std::process::id())?;
        assert!(!own.is_empty(), "the check sees this test's page server");
        let group = listening_sockets(|candidate| process_group(candidate) == Some(pid))?;
        assert!(
            group.is_empty(),
            "the browser listens on no TCP port: {group:?}"
        );
    }
    assert!(
        !manager
            .default_profile_dir(&sandbox)
            .join("DevToolsActivePort")
            .exists()
    );

    let opened = browser
        .tool_open(&OpenParams {
            url: page_url.clone(),
        })
        .await?;
    assert!(opened.loaded, "load event within the timeout");
    let tab = opened.tab.id.clone();
    assert_eq!(
        opened.tab.title, "Yulius test page",
        "tool results carry the loaded title"
    );
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
