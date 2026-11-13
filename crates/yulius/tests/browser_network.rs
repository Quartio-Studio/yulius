//! The sandbox browsers' way out (SPEC-483 E2, ADR-0321) as the server wires it: a proxy of its
//! own per browser, bound to the egress proxy and to the application's record of a sandbox's
//! ports. With `YULIUS_TEST_CHROMIUM` (never in CI), a real Chromium behind it.

use crate::support;

use std::net::{Ipv4Addr, SocketAddr};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{Value, json};
use support::{Harness, harness, seed_github_user, seed_sandbox};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::{TcpListener, TcpStream};
use yulius::wiring::egress::BrowserProxies;
use yulius_browser::{BrowserConfig, BrowserManager, BrowserNetwork};
use yulius_contracts::browser::OpenParams;
use yulius_domain::ids::SandboxId;

type TestResult = Result<(), Box<dyn std::error::Error>>;

const WAIT: Duration = Duration::from_secs(20);

/// An origin on the loopback at `port` (0: any): a page titled and saying `name`. Keeps the
/// request lines it got.
async fn origin_at(
    name: &'static str,
    port: u16,
) -> std::io::Result<(SocketAddr, Arc<Mutex<Vec<String>>>)> {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, port)).await?;
    let addr = listener.local_addr()?;
    let seen = Arc::new(Mutex::new(Vec::new()));
    let kept = Arc::clone(&seen);
    tokio::spawn(async move {
        while let Ok((mut stream, _)) = listener.accept().await {
            let kept = Arc::clone(&kept);
            tokio::spawn(async move {
                let mut request = Vec::new();
                let mut chunk = [0u8; 4096];
                while !request.windows(4).any(|w| w == b"\r\n\r\n") {
                    match stream.read(&mut chunk).await {
                        Ok(n) if n > 0 => request.extend_from_slice(&chunk[..n]),
                        _ => return,
                    }
                }
                let line = String::from_utf8_lossy(&request)
                    .lines()
                    .next()
                    .unwrap_or_default()
                    .to_owned();
                if let Ok(mut kept) = kept.lock() {
                    kept.push(line);
                }
                let page =
                    format!("<!doctype html><title>{name}</title><p id=\"who\">{name} page</p>");
                let answer = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: text/html\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{page}",
                    page.len()
                );
                let _ = stream.write_all(answer.as_bytes()).await;
            });
        }
    });
    Ok((addr, seen))
}

/// An origin on a free port of `ports` (other tests of this binary use the same ranges).
async fn origin_in(
    name: &'static str,
    ports: std::ops::RangeInclusive<u16>,
) -> Result<(SocketAddr, Arc<Mutex<Vec<String>>>), Box<dyn std::error::Error>> {
    for port in ports {
        if let Ok(origin) = origin_at(name, port).await {
            return Ok(origin);
        }
    }
    Err("no free port in the sandbox's range".into())
}

/// An origin on a port the system gives that is in none of `ranges`: the system's free ports
/// include the fixtures' sandbox ranges, and a "service of the host" that landed in one would
/// be that sandbox's own.
async fn origin_outside(
    name: &'static str,
    ranges: &[std::ops::RangeInclusive<u16>],
) -> Result<(SocketAddr, Arc<Mutex<Vec<String>>>), Box<dyn std::error::Error>> {
    // Those that landed inside are kept until one is outside, so the system gives another.
    let mut inside = Vec::new();
    for _ in 0..64 {
        let origin = origin_at(name, 0).await?;
        if !ranges.iter().any(|range| range.contains(&origin.0.port())) {
            return Ok(origin);
        }
        inside.push(origin);
    }
    Err("no port outside the sandboxes' ranges".into())
}

/// A plain `GET` through `proxy`, as Chrome sends it: the whole answer.
async fn get(proxy: SocketAddr, url: &str) -> Result<String, Box<dyn std::error::Error>> {
    let authority = url
        .strip_prefix("http://")
        .and_then(|rest| rest.split('/').next())
        .unwrap_or_default();
    let mut stream = TcpStream::connect(proxy).await?;
    stream
        .write_all(
            format!("GET {url} HTTP/1.1\r\nHost: {authority}\r\nConnection: close\r\n\r\n")
                .as_bytes(),
        )
        .await?;
    let mut answer = Vec::new();
    tokio::time::timeout(WAIT, stream.read_to_end(&mut answer)).await??;
    Ok(String::from_utf8_lossy(&answer).into_owned())
}

struct Wired {
    h: Harness,
    /// The egress proxy, which the server's router holds as long as it serves.
    egress: Arc<yulius_egress::EgressProxy>,
    proxies: Arc<BrowserProxies>,
    /// What stands for the internal listener, and what it was asked.
    yulius: (SocketAddr, Arc<Mutex<Vec<String>>>),
    /// A dev server of sandbox `s1`, one of `s2`, and a service of the host.
    own: (SocketAddr, Arc<Mutex<Vec<String>>>),
    other: (SocketAddr, Arc<Mutex<Vec<String>>>),
    host: (SocketAddr, Arc<Mutex<Vec<String>>>),
}

/// Sandboxes `s1` and `s2` with their port ranges, the egress proxy, and the browsers' proxies
/// bound to both.
async fn wired() -> Result<Wired, Box<dyn std::error::Error>> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let h = harness()?;
    seed_github_user(&h, "u-alice", "alice").await?;
    let s1 = seed_sandbox(&h, "s1", "fixture", "u-alice").await?;
    let s2 = seed_sandbox(&h, "s2", "other", "u-alice").await?;
    let range = |s: &yulius_app::model::SandboxRecord| s.port_base..=s.port_base + s.port_count - 1;
    let ranges = [range(&s1), range(&s2)];
    let yulius = origin_outside("yulius", &ranges).await?;
    let proxies = Arc::new(BrowserProxies::new(
        yulius.0,
        vec!["/browser/welcome?".to_owned()],
        Vec::new(),
    ));
    let unbound = proxies.open(&SandboxId::parse("s1")?);
    assert!(
        unbound.is_err(),
        "before the egress proxy runs no browser gets a way out, so none starts"
    );
    let egress =
        yulius::wiring::egress::egress_proxy(&h.app, yulius_egress::EgressConfig::default())
            .await?;
    proxies.bind(&h.app, &egress);
    Ok(Wired {
        egress,
        own: origin_in("own", range(&s1)).await?,
        other: origin_in("other", range(&s2)).await?,
        host: origin_outside("host", &ranges).await?,
        yulius,
        proxies,
        h,
    })
}

fn status(answer: &str) -> &str {
    answer.lines().next().unwrap_or_default()
}

/// SPEC-483 E2: each browser's proxy knows its sandbox from the application: `s1`'s reaches a
/// dev server in `s1`'s ports and not one in `s2`'s, and the other way round; neither reaches a
/// service of the host; of Yulius's listener both get the welcome page only. A lease that is
/// let go closes its proxy, and a sandbox that does not exist gets none that answers.
#[tokio::test]
async fn each_browsers_proxy_reaches_its_own_sandboxes_ports() -> TestResult {
    let w = wired().await?;
    let (s1, s2) = (SandboxId::parse("s1")?, SandboxId::parse("s2")?);
    let (first, second) = (w.proxies.open(&s1)?, w.proxies.open(&s2)?);
    assert_ne!(first.proxy, second.proxy);
    let at = |addr: SocketAddr, path: &str| format!("http://127.0.0.1:{}{path}", addr.port());

    for (proxy, mine, theirs) in [
        (first.proxy, w.own.0, w.other.0),
        (second.proxy, w.other.0, w.own.0),
    ] {
        let answer = get(proxy, &at(mine, "/")).await?;
        assert_eq!(status(&answer), "HTTP/1.1 200 OK", "{answer}");
        for refused in [
            at(theirs, "/"),
            at(w.host.0, "/"),
            at(w.yulius.0, "/internal/version"),
        ] {
            let answer = get(proxy, &refused).await?;
            assert_eq!(
                status(&answer),
                "HTTP/1.1 403 Forbidden",
                "{refused}: {answer}"
            );
        }
        let welcome = get(proxy, &at(w.yulius.0, "/browser/welcome?sandbox=x")).await?;
        assert_eq!(status(&welcome), "HTTP/1.1 200 OK", "{welcome}");
    }
    assert_eq!(w.own.1.lock().map_err(|_| "lock")?.len(), 1);
    assert_eq!(w.other.1.lock().map_err(|_| "lock")?.len(), 1);
    assert!(w.host.1.lock().map_err(|_| "lock")?.is_empty());
    assert_eq!(
        w.yulius.1.lock().map_err(|_| "lock")?.len(),
        2,
        "the welcome page, twice"
    );

    // The browser ended: its proxy takes no connection any more.
    let gone = first.proxy;
    drop(first);
    tokio::time::timeout(WAIT, async {
        while TcpStream::connect(gone).await.is_ok() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await?;
    assert_eq!(
        status(&get(second.proxy, &at(w.other.0, "/")).await?),
        "HTTP/1.1 200 OK",
        "another browser's goes on"
    );

    // No such sandbox: nothing is served.
    let nobody = w.proxies.open(&SandboxId::parse("nope")?)?;
    let answer = get(nobody.proxy, &at(w.own.0, "/")).await;
    assert!(
        answer.as_ref().map_or(true, |answer| answer.is_empty()),
        "{answer:?}"
    );
    // The egress proxy is gone (the server no longer serves): no proxy opens, so no browser
    // starts. The proxies hold it weakly; a browser's own, while it runs, holds it.
    let Wired {
        h, proxies, egress, ..
    } = w;
    drop((second, nobody, egress));
    tokio::time::timeout(WAIT, async {
        while proxies.open(&s1).is_ok() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await?;
    h.app.stop_workspace_services().await;
    Ok(())
}

/// What `expression` evaluates to in the tab `target` of `browser`.
async fn evaluate(
    browser: &yulius_browser::BrowserInstance,
    target: &str,
    expression: &str,
) -> Result<Value, Box<dyn std::error::Error>> {
    let cdp = browser.cdp();
    let attached = cdp
        .call(
            "Target.attachToTarget",
            json!({"targetId": target, "flatten": true}),
        )
        .await?;
    let session = attached["sessionId"].as_str().ok_or("no session")?;
    let result = cdp
        .call_session(
            session,
            "Runtime.evaluate",
            json!({"expression": expression, "awaitPromise": true, "returnByValue": true}),
        )
        .await?;
    Ok(result["result"]["value"].clone())
}

/// SPEC-483 E2, opt-in (never in CI): `YULIUS_TEST_CHROMIUM` names a Chromium. A real browser
/// behind its proxy loads its sandbox's dev server by `localhost` (Chrome sends the loopback
/// past a proxy unless told otherwise), and gets nothing of another sandbox's port, a service
/// of the host, a cloud's metadata address or Yulius's API: not by navigation, not over TLS,
/// not from a page's own script.
#[tokio::test]
async fn a_real_chromium_reaches_its_sandboxes_ports_and_no_other_local_address() -> TestResult {
    let Some(exe) = std::env::var_os("YULIUS_TEST_CHROMIUM") else {
        eprintln!("skipped: set YULIUS_TEST_CHROMIUM to a Chromium executable to run");
        return Ok(());
    };
    let w = wired().await?;
    let dir = tempfile::tempdir()?;
    let network: Arc<dyn BrowserNetwork> = w.proxies.clone();
    let manager = BrowserManager::new(BrowserConfig {
        startup_timeout: Duration::from_secs(60),
        network: Some(network),
        ..BrowserConfig::new(exe.into(), dir.path().to_path_buf())
    });
    let sandbox = SandboxId::parse("s1")?;
    let browser = manager
        .ensure(&sandbox, &manager.default_profile_dir(&sandbox))
        .await?;
    let open = async |url: String| -> Result<String, Box<dyn std::error::Error>> {
        // A page the proxy refuses is still a page (its text), or no page at all (a tunnel).
        let tab = browser.tool_open(&OpenParams { url }).await?.tab.id;
        Ok(evaluate(
            &browser,
            tab.as_str(),
            "document.body ? document.body.innerText : ''",
        )
        .await?
        .as_str()
        .unwrap_or_default()
        .to_owned())
    };
    let own = w.own.0.port();

    for host in ["localhost", "127.0.0.1"] {
        let text = open(format!("http://{host}:{own}/")).await?;
        assert!(text.contains("own page"), "{host}: {text:?}");
    }
    let welcome = open(format!(
        "http://127.0.0.1:{}/browser/welcome?sandbox=s1",
        w.yulius.0.port()
    ))
    .await?;
    assert!(welcome.contains("yulius page"), "{welcome:?}");

    let (other, host, yulius) = (w.other.0.port(), w.host.0.port(), w.yulius.0.port());
    for refused in [
        format!("http://localhost:{other}/"),
        format!("http://127.0.0.1:{other}/"),
        format!("http://[::1]:{other}/"),
        format!("http://127.0.0.1:{host}/"),
        format!("http://127.0.0.1:{yulius}/internal/version"),
        "http://169.254.169.254/latest/meta-data/".to_owned(),
        format!("https://127.0.0.1:{other}/"),
        format!("https://localhost:{host}/"),
    ] {
        let text = match open(refused.clone()).await {
            Ok(text) => text,
            // A refused tunnel is a failed navigation.
            Err(error) => error.to_string(),
        };
        assert!(
            !text.contains("other page")
                && !text.contains("host page")
                && !text.contains("yulius page"),
            "{refused}: {text:?}"
        );
        if refused.starts_with("http://") {
            assert!(
                text.contains("the sandbox browser does not load"),
                "{refused}: {text:?}"
            );
        }
    }

    // A page of the sandbox's own server asks for them from its script: an image, a fetch, a
    // WebSocket. Nothing arrives at the other servers.
    let tab = browser
        .tool_open(&OpenParams {
            url: format!("http://localhost:{own}/"),
        })
        .await?
        .tab
        .id;
    let tried = evaluate(
        &browser,
        tab.as_str(),
        &format!(
            "(async () => {{
              const targets = ['http://127.0.0.1:{other}/a', 'http://localhost:{host}/b', 'http://169.254.169.254/c', 'http://127.0.0.1:{yulius}/internal/version'];
              const results = [];
              for (const url of targets) {{
                results.push(await fetch(url, {{mode: 'no-cors'}}).then(() => 'answered', () => 'failed'));
                await new Promise((done) => {{ const i = new Image(); i.onload = i.onerror = done; i.src = url; }});
              }}
              results.push(await new Promise((done) => {{
                const socket = new WebSocket('ws://127.0.0.1:{other}/ws');
                socket.onopen = () => done('opened'); socket.onerror = () => done('failed');
              }}));
              const mine = await fetch('http://localhost:{own}/again').then((r) => r.status, () => 'failed');
              return JSON.stringify([results, mine]);
            }})()"
        ),
    )
    .await?;
    let tried = tried.as_str().unwrap_or_default().to_owned();
    assert!(tried.ends_with(",200]"), "its own server answers: {tried}");
    assert!(!tried.contains("opened"), "{tried}");
    for (name, seen) in [("other", &w.other.1), ("host", &w.host.1)] {
        let seen = seen.lock().map_err(|_| "lock")?.clone();
        assert!(seen.is_empty(), "{name} was reached: {seen:?}");
    }
    let asked = w.yulius.1.lock().map_err(|_| "lock")?.clone();
    assert!(
        asked.iter().all(|line| line.contains("/browser/welcome?")),
        "{asked:?}"
    );
    let _ = manager.shutdown(&sandbox).await;
    w.h.app.stop_workspace_services().await;
    Ok(())
}
