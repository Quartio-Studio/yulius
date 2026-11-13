//! Opt-in smoke test against a real Chromium (never in CI): set `YULIUS_TEST_CHROMIUM` to a
//! Chromium or Google Chrome executable. Without it the test returns immediately.
//!
//! It serves a local page on loopback, then drives the browser through the adapter: open and
//! load, title from target events, accessibility snapshot, type, click by selector, PNG
//! screenshot, the secret probe (SPEC-450: a password field keeps screenshots, a revealed one or
//! a text field refuses them, and so does a frame the probe cannot read: of another site, or
//! sandboxed; a PDF keeps them), screencast frames, URL policy, and kill-and-reap on shutdown. DevTools runs over
//! the pipe: the browser writes no `DevToolsActivePort` and (Linux) listens on no TCP port.

use std::time::Duration;

use base64::Engine as _;
use serde_json::json;
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::TcpListener;
use yulius_browser::{BrowserConfig, BrowserError, BrowserInstance, BrowserManager};
use yulius_contracts::browser::{
    ClickParams, NavigateParams, OpenParams, ScreenshotParams, SnapshotParams, TypeParams,
};
use yulius_domain::ids::SandboxId;

type TestResult = Result<(), Box<dyn std::error::Error>>;

const PAGE: &str = "<!doctype html><html><head><title>Yulius test page</title></head><body>\
<h1>Hello</h1><input id=\"name\" aria-label=\"Name\">\
<button id=\"press\" onclick=\"document.getElementById('out').textContent='clicked '+document.getElementById('name').value\">Press</button>\
<p id=\"out\">waiting</p>\
<input id=\"pw\" type=\"password\" aria-label=\"Password\">\
<button id=\"show\" onclick=\"document.getElementById('pw').type='text'\">Show</button>\
<form name=\"documentElement\"></form><img name=\"querySelectorAll\" alt=\"\"></body></html>";

/// A one-page PDF.
const PDF: &str = "%PDF-1.4\n\
1 0 obj\n\
<< /Type /Catalog /Pages 2 0 R >>\n\
endobj\n\
2 0 obj\n\
<< /Type /Pages /Kids [3 0 R] /Count 1 >>\n\
endobj\n\
3 0 obj\n\
<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 200] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >>\n\
endobj\n\
4 0 obj\n\
<< /Length 46 >>\n\
stream\n\
BT /F1 18 Tf 20 100 Td (Yulius test PDF) Tj ET\n\
endstream\n\
endobj\n\
5 0 obj\n\
<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>\n\
endobj\n\
xref\n\
0 6\n\
0000000000 65535 f \n\
0000000009 00000 n \n\
0000000058 00000 n \n\
0000000115 00000 n \n\
0000000241 00000 n \n\
0000000337 00000 n \n\
trailer\n\
<< /Size 6 /Root 1 0 R >>\n\
startxref\n\
407\n\
%%EOF\n";

/// Serves [`PAGE`]; at `/frame` a page framing `framed`; at `/own-frame`, `/sandboxed-frame` and
/// `/sandboxed-page` a page framing this server's own `/`: plainly, in a sandboxed frame, and
/// from a page that is sandboxed itself (`Content-Security-Policy: sandbox`); at `/doc.pdf`
/// [`PDF`], at `/pdf-frame` a page framing it, and at `/pdf-and-embed` one that also holds `/`
/// in an `embed`, which the probe does not read.
async fn serve(listener: TcpListener, framed: String) {
    while let Ok((mut stream, _)) = listener.accept().await {
        let framed = framed.clone();
        tokio::spawn(async move {
            let mut request = [0_u8; 4096];
            let _ = stream.read(&mut request).await;
            let framing = |frame: &str| {
                format!("<!doctype html><html><body><p>Framing</p>{frame}</body></html>")
            };
            let mut policy = "";
            let mut kind = "text/html; charset=utf-8";
            let page = if request.starts_with(b"GET /doc.pdf") {
                kind = "application/pdf";
                PDF.to_owned()
            } else if request.starts_with(b"GET /pdf-frame") {
                framing("<iframe src=\"/doc.pdf\"></iframe>")
            } else if request.starts_with(b"GET /pdf-and-embed") {
                framing("<iframe src=\"/doc.pdf\"></iframe><embed src=\"/\" type=\"text/html\">")
            } else if request.starts_with(b"GET /frame") {
                framing(&format!("<iframe src=\"{framed}\"></iframe>"))
            } else if request.starts_with(b"GET /own-frame") {
                framing("<iframe src=\"/\"></iframe>")
            } else if request.starts_with(b"GET /sandboxed-frame") {
                framing("<iframe sandbox src=\"/\"></iframe>")
            } else if request.starts_with(b"GET /sandboxed-page") {
                policy = "Content-Security-Policy: sandbox\r\n";
                framing("<iframe src=\"/\"></iframe>")
            } else {
                PAGE.to_owned()
            };
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: {kind}\r\n{policy}Content-Length: {}\r\nConnection: close\r\n\r\n{page}",
                page.len()
            );
            let _ = stream.write_all(response.as_bytes()).await;
        });
    }
}

/// Waits until Chrome's PDF viewer shows `wanted` PDFs: it makes its frames, one of them at the
/// PDF's address, after the page's load.
async fn pdf_frames(browser: &BrowserInstance, wanted: usize) -> TestResult {
    tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            let targets = browser.cdp().call("Target.getTargets", json!({})).await?;
            let shown = targets["targetInfos"].as_array().map_or(0, |infos| {
                infos
                    .iter()
                    .filter(|info| {
                        info["type"] == "iframe"
                            && info["url"]
                                .as_str()
                                .is_some_and(|url| url.ends_with("/doc.pdf"))
                    })
                    .count()
            });
            if shown == wanted {
                return Ok(());
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await?
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
    let port = listener.local_addr()?.port();
    let page_url = format!("http://127.0.0.1:{port}/");
    let framed = format!("http://localhost:{port}/");
    let server = tokio::spawn(serve(listener, framed.clone()));
    // A third site on the loopback network, framing the second (SPEC-450).
    let framing = TcpListener::bind("127.0.0.2:0").await?;
    let framing_url = format!("http://127.0.0.2:{}/", framing.local_addr()?.port());
    let framing_server = tokio::spawn(serve(framing, framed));
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

    // SPEC-450 on a real page: the probe reads a password field as masked, so a secret typed
    // there keeps screenshots; revealed, or typed into a text field, it refuses them. The page's
    // named elements shadow `document.documentElement` and `document.querySelectorAll`, which
    // the probe reads through the prototypes.
    let shot_params = ScreenshotParams {
        tab_id: tab.clone(),
    };
    let secret = |selector: &str, text: &str| TypeParams {
        tab_id: tab.clone(),
        text: text.to_owned(),
        selector: Some(selector.to_owned()),
    };
    browser
        .tool_type_secret(&secret("#pw", "real-secret-7731-pw"))
        .await?;
    browser
        .tool_screenshot_within(&shot_params, 1 << 20)
        .await?;
    browser
        .tool_click(&ClickParams {
            tab_id: tab.clone(),
            selector: Some("#show".into()),
            x: None,
            y: None,
        })
        .await?;
    assert!(matches!(
        browser.tool_screenshot_within(&shot_params, 1 << 20).await,
        Err(BrowserError::SecretOnPage)
    ));
    let other_origin = page_url.replace("127.0.0.1", "localhost");
    browser.navigate(&tab, &other_origin).await?;
    browser
        .tool_screenshot_within(&shot_params, 1 << 20)
        .await?;
    browser
        .tool_type_secret(&secret("#name", "real-secret-8812-api"))
        .await?;
    assert!(matches!(
        browser.tool_screenshot_within(&shot_params, 1 << 20).await,
        Err(BrowserError::SecretOnPage)
    ));
    // A page of another site is captured, unless it frames the site the secret went into
    // (out of the probe's reach, so the frame tree tells).
    let load = |url: String| NavigateParams {
        tab_id: tab.clone(),
        url,
    };
    browser.tool_navigate(&load(framing_url.clone())).await?;
    browser
        .tool_screenshot_within(&shot_params, 1 << 20)
        .await?;
    // Waits for the load event, so the frame is there.
    browser
        .tool_navigate(&load(format!("{framing_url}frame")))
        .await?;
    assert!(matches!(
        browser.tool_screenshot_within(&shot_params, 1 << 20).await,
        Err(BrowserError::SecretOnPage)
    ));
    // A password sign-in on the framing site keeps its screenshots, also on a page framing its
    // own origin, which the probe reads. A sandboxed frame of that origin is out of the probe's
    // reach (Chrome runs it in a process of its own: a target on the page's own host), and so is
    // every frame of a page that is sandboxed itself, though the frame tree says the origin of
    // the frame's address for both.
    browser.tool_navigate(&load(framing_url.clone())).await?;
    browser
        .tool_type_secret(&secret("#pw", "real-secret-9951-pw"))
        .await?;
    browser
        .tool_screenshot_within(&shot_params, 1 << 20)
        .await?;
    browser
        .tool_navigate(&load(format!("{framing_url}own-frame")))
        .await?;
    browser
        .tool_screenshot_within(&shot_params, 1 << 20)
        .await?;
    for unreadable in ["sandboxed-frame", "sandboxed-page"] {
        browser
            .tool_navigate(&load(format!("{framing_url}{unreadable}")))
            .await?;
        assert!(
            matches!(
                browser.tool_screenshot_within(&shot_params, 1 << 20).await,
                Err(BrowserError::SecretOnPage)
            ),
            "{unreadable}"
        );
    }
    // A PDF of that site is pixels, as an image is: Chrome's viewer shows it in frames of its
    // own, one of them at the PDF's address, and it keeps its screenshots as the page and in a
    // frame. The probe counts no PDF: beside one, an `embed` of the site's own page, which it
    // does not read, still refuses.
    for pdf in ["doc.pdf", "pdf-frame"] {
        browser
            .tool_navigate(&load(format!("{framing_url}{pdf}")))
            .await?;
        pdf_frames(&browser, 1).await?;
        assert!(
            browser
                .tool_screenshot_within(&shot_params, 1 << 20)
                .await
                .is_ok(),
            "{pdf}"
        );
        browser.tool_navigate(&load(framing_url.clone())).await?;
        pdf_frames(&browser, 0).await?;
    }
    browser
        .tool_navigate(&load(format!("{framing_url}pdf-and-embed")))
        .await?;
    pdf_frames(&browser, 1).await?;
    assert!(matches!(
        browser.tool_screenshot_within(&shot_params, 1 << 20).await,
        Err(BrowserError::SecretOnPage)
    ));

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
    framing_server.abort();
    Ok(())
}

/// The windows of display `name` that are no part of the kiosk: `xwininfo -root -tree` lines
/// whose title says a file dialog (GTK's "Open File", "Open Files", "Save File", "All Files",
/// Chrome's own "File Upload").
fn file_dialogs(env: &[(std::ffi::OsString, std::ffi::OsString)]) -> Result<Vec<String>, String> {
    let output = std::process::Command::new("xwininfo")
        .args(["-root", "-tree"])
        .envs(env.iter().map(|(k, v)| (k.as_os_str(), v.as_os_str())))
        .output()
        .map_err(|e| format!("xwininfo: {e}"))?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).into_owned());
    }
    Ok(String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter(|line| {
            [
                "Open File",
                "Save File",
                "File Upload",
                "All Files",
                "Select",
            ]
            .iter()
            .any(|title| line.contains(title))
        })
        .map(|line| line.trim().to_owned())
        .collect())
}

/// SPEC-483 E1 (ADR-0318), opt-in (never in CI): `YULIUS_TEST_CHROMIUM` and an `Xvfb` on `PATH`.
/// A viewer's input on the virtual display is real X input, and the browser runs on the host
/// as the service user: a click on a page's file input must open no native file chooser, in
/// which the viewer could pick any file the service user reads (the instance's master key,
/// another sandbox's repository) for the page to read. With the host's managed policy
/// installed (`yulius host setup`) the click reaches the page and Chrome opens nothing, nor do
/// its accelerators for Open File, Save as, Print, the page's source and the developer tools
/// (no window shown, no `devtools:`, `view-source:`, `file:` or print page), and a link to
/// another program's address is blocked as a page instead of handed to `xdg-open`; without
/// it the click is refused. Before ADR-0318 the chooser opened: "Open File", over the whole display
/// (Google Chrome 157).
#[tokio::test]
async fn a_viewers_click_on_a_file_input_opens_no_file_chooser() -> TestResult {
    use yulius_browser::{DisplayConfig, DisplayMode, MouseInput};
    use yulius_contracts::browser::{MouseButton, MouseEventKind};
    let Some(exe) = std::env::var_os("YULIUS_TEST_CHROMIUM") else {
        eprintln!("skipped: set YULIUS_TEST_CHROMIUM to a Chromium executable to run");
        return Ok(());
    };
    const UPLOAD: &str = "<!doctype html><html><head><title>upload</title></head><body>\
<input type=\"file\" id=\"f\" style=\"position:fixed;left:0;top:0;width:600px;height:400px;font-size:200px\" \
onclick=\"document.title='clicked'\" onchange=\"document.title='picked '+this.files[0].name\">\
<a href=\"yulius-test-scheme://open\" style=\"position:fixed;left:0;top:450px;width:600px;height:200px\" \
onclick=\"document.title='linked'\">another program</a></body></html>";
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let port = listener.local_addr()?.port();
    let server = tokio::spawn(async move {
        while let Ok((mut stream, _)) = listener.accept().await {
            tokio::spawn(async move {
                let mut request = [0_u8; 4096];
                let _ = stream.read(&mut request).await;
                let answer = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: text/html\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{UPLOAD}",
                    UPLOAD.len()
                );
                let _ = stream.write_all(answer.as_bytes()).await;
            });
        }
    });
    let dir = tempfile::tempdir()?;
    let config = BrowserConfig {
        startup_timeout: Duration::from_secs(60),
        display: DisplayMode::Virtual(DisplayConfig::new(dir.path().join("displays"))),
        // Chrome refuses to run as root with its own sandbox (a test host's choice).
        no_sandbox: std::env::var_os("YULIUS_TEST_CHROMIUM_NO_SANDBOX").is_some(),
        ..BrowserConfig::new(exe.into(), dir.path().join("profiles"))
    };
    let manager = BrowserManager::new(config);
    let sandbox = SandboxId::parse("real-upload")?;
    let browser = manager
        .ensure(&sandbox, &manager.default_profile_dir(&sandbox))
        .await?;
    let env = browser.display().ok_or("no virtual display")?.env();
    let tab = browser.open(&format!("http://127.0.0.1:{port}/")).await?;
    assert_eq!(file_dialogs(&env)?, Vec::<String>::new());

    let at = |event, button| MouseInput {
        event,
        x: 300.0,
        y: 200.0,
        button,
        dx: 0.0,
        dy: 0.0,
        modifiers: 0,
    };
    // Clicks until the page saw one (its window may still be mapping), then gives a chooser
    // the time to show: none must have by then.
    // The page's title as the browser window shows it ("<title> - Google Chrome").
    let title = || -> String {
        let tree = std::process::Command::new("xwininfo")
            .args(["-root", "-tree"])
            .envs(env.iter().map(|(k, v)| (k.as_os_str(), v.as_os_str())))
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
            .unwrap_or_default();
        tree.lines()
            .filter_map(|line| line.split('"').nth(1))
            .find_map(|name| {
                name.strip_suffix(" - Google Chrome")
                    .or_else(|| name.strip_suffix(" - Chromium"))
            })
            .unwrap_or_default()
            .to_owned()
    };
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    while tokio::time::Instant::now() < deadline && title() != "upload" {
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert_eq!(title(), "upload", "the page did not load");
    let mut shown = Vec::new();
    let mut refused = false;
    'clicks: while tokio::time::Instant::now() < deadline && shown.is_empty() && title() == "upload"
    {
        for (event, button) in [
            (MouseEventKind::Move, MouseButton::None),
            (MouseEventKind::Down, MouseButton::Left),
            (MouseEventKind::Up, MouseButton::Left),
        ] {
            match browser.dispatch_mouse(&tab, at(event, button)).await {
                Ok(()) => {}
                Err(BrowserError::InputLocked) => {
                    refused = true;
                    break 'clicks;
                }
                Err(error) => return Err(error.into()),
            }
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
        shown = file_dialogs(&env)?;
    }
    eprintln!(
        "{}",
        if refused {
            "the host's browser policy is not installed: the click was refused"
        } else {
            "the host's browser policy is in force: the click reached the page"
        }
    );
    assert!(
        refused || title() != "upload",
        "the click never reached the page"
    );
    let settle = tokio::time::Instant::now() + Duration::from_secs(3);
    while tokio::time::Instant::now() < settle && shown.is_empty() {
        tokio::time::sleep(Duration::from_millis(250)).await;
        shown = file_dialogs(&env)?;
    }
    // A link to another program's address: Chrome offers to run `xdg-open` on the host.
    let mut handed_over = false;
    if !refused && shown.is_empty() {
        let link = |event, button| MouseInput {
            y: 550.0,
            ..at(event, button)
        };
        for (event, button) in [
            (MouseEventKind::Move, MouseButton::None),
            (MouseEventKind::Down, MouseButton::Left),
            (MouseEventKind::Up, MouseButton::Left),
        ] {
            browser.dispatch_mouse(&tab, link(event, button)).await?;
        }
        // The policy blocks the address as a page (the tab shows it blocked); without that
        // rule Chrome keeps the page and hands the address to `xdg-open`.
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        while tokio::time::Instant::now() < deadline
            && matches!(title().as_str(), "clicked" | "linked")
        {
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
        handed_over = matches!(title().as_str(), "clicked" | "linked");
    }
    // With input taken, Chrome's own accelerators arrive too: Open File, Save as, Print, the
    // developer tools, the page's source. None may open a window of its own or a page of
    // Chrome's.
    let mut own = Vec::new();
    if !refused && shown.is_empty() {
        use yulius_browser::KeyInput;
        use yulius_contracts::browser::KeyEventKind;
        // The top-level windows shown on the display (mapped: GTK also keeps unmapped ones).
        let windows = || -> Vec<String> {
            let xwininfo = |args: &[&str]| {
                std::process::Command::new("xwininfo")
                    .args(args)
                    .envs(env.iter().map(|(k, v)| (k.as_os_str(), v.as_os_str())))
                    .output()
                    .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
                    .unwrap_or_default()
            };
            xwininfo(&["-root", "-children"])
                .lines()
                .filter_map(|line| line.trim().split(' ').next())
                .filter(|id| id.starts_with("0x"))
                .filter(|id| xwininfo(&["-id", id]).contains("Map State: IsViewable"))
                .map(str::to_owned)
                .collect()
        };
        let before = windows();
        let key = |event, key: &str, code: &str| KeyInput {
            event,
            key: Some(key.to_owned()),
            code: Some(code.to_owned()),
            text: None,
            modifiers: 0,
        };
        for (held, name, code) in [
            (vec![("Control", "ControlLeft")], "o", "KeyO"),
            (vec![("Control", "ControlLeft")], "s", "KeyS"),
            (vec![("Control", "ControlLeft")], "p", "KeyP"),
            (vec![("Control", "ControlLeft")], "u", "KeyU"),
            (vec![], "F12", "F12"),
            (
                vec![("Control", "ControlLeft"), ("Shift", "ShiftLeft")],
                "I",
                "KeyI",
            ),
        ] {
            for (modifier, code) in &held {
                browser
                    .dispatch_key(&tab, key(KeyEventKind::Down, modifier, code))
                    .await?;
            }
            browser
                .dispatch_key(&tab, key(KeyEventKind::Down, name, code))
                .await?;
            browser
                .dispatch_key(&tab, key(KeyEventKind::Up, name, code))
                .await?;
            for (modifier, code) in held.iter().rev() {
                browser
                    .dispatch_key(&tab, key(KeyEventKind::Up, modifier, code))
                    .await?;
            }
            tokio::time::sleep(Duration::from_millis(700)).await;
            let dialogs = file_dialogs(&env)?;
            let targets = browser.cdp().call("Target.getTargets", json!({})).await?;
            let pages: Vec<String> = targets["targetInfos"]
                .as_array()
                .map(|infos| {
                    infos
                        .iter()
                        .filter_map(|t| t["url"].as_str())
                        .filter(|url| {
                            ["devtools://", "view-source:", "file://", "chrome://print"]
                                .iter()
                                .any(|own| url.starts_with(own))
                        })
                        .map(str::to_owned)
                        .collect()
                })
                .unwrap_or_default();
            if !dialogs.is_empty() || !pages.is_empty() || windows() != before {
                own.push(format!(
                    "{held:?}+{name}: dialogs {dialogs:?}, pages {pages:?}, windows {:?} (were {before:?})",
                    windows()
                ));
            }
        }
    }
    let _ = manager.shutdown(&sandbox).await;
    server.abort();
    assert_eq!(shown, Vec::<String>::new(), "a native file dialog opened");
    assert_eq!(
        own,
        Vec::<String>::new(),
        "Chrome opened a window or page of its own"
    );
    assert!(
        !handed_over,
        "a link to another program's address was not blocked as a page"
    );
    Ok(())
}
