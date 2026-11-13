//! A sandbox browser's way out (SPEC-483 E2) end to end over real sockets: what Chrome sends a
//! proxy (`CONNECT`, absolute `http://` requests, no credential), origin servers on the
//! loopback standing for the sandbox's dev server, another sandbox's and Yulius's listener.

use std::collections::HashMap;
use std::net::{Ipv4Addr, SocketAddr};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::{TcpListener, TcpStream};
use yulius_app::ports::egress::{
    EgressCaller, EgressClient, EgressDecision, EgressPolicy, EgressRequest,
};
use yulius_domain::ids::{SandboxId, TaskId};
use yulius_domain::vault::redact::PlainMatcher;
use yulius_egress::{
    BrowserEgress, CertificateAuthority, EgressConfig, EgressProxy, generate_ca_material,
    serve_browser,
};

type TestResult = Result<(), Box<dyn std::error::Error>>;

/// A value of an agent secret, as an origin may echo it.
const VALUE: &str = "sk_test_51NzQ9yulius_value_8f3a";

/// A policy that would intercept every host, inject the secret and redact it: a browser's
/// requests must never reach it, and `asked` counts the times one did.
struct Policy {
    asked: AtomicUsize,
    /// Whether an agent's hosts are intercepted (else tunnelled).
    intercepts: bool,
}

#[async_trait]
impl EgressPolicy for Policy {
    async fn egress_client(&self, _: &str) -> Option<EgressClient> {
        self.asked.fetch_add(1, Ordering::SeqCst);
        Some(EgressClient {
            caller: EgressCaller::Agent {
                attempt_id: "a1".into(),
                task_id: TaskId::parse("t1").ok()?,
                task_number: 1,
            },
            sandbox_id: SandboxId::parse("s1").ok()?,
        })
    }

    async fn egress_intercepts(&self, _: &EgressClient, _: &str, _: u16) -> bool {
        self.asked.fetch_add(1, Ordering::SeqCst);
        self.intercepts
    }

    async fn egress_apply(&self, _: &EgressClient, request: EgressRequest) -> EgressDecision {
        self.asked.fetch_add(1, Ordering::SeqCst);
        let mut headers = request.headers;
        headers.push((
            "authorization".into(),
            format!("Bearer {VALUE}").into_bytes(),
        ));
        EgressDecision::Forward {
            path_and_query: request.path_and_query,
            headers,
            used: vec!["API_KEY".into()],
        }
    }

    async fn egress_redactor(&self, _: &EgressClient) -> Option<Arc<PlainMatcher>> {
        self.asked.fetch_add(1, Ordering::SeqCst);
        Some(Arc::new(PlainMatcher::new(vec![(
            "API_KEY".into(),
            yulius_domain::vault::encode::needles(VALUE),
        )])))
    }
}

/// An origin on the loopback: answers every request with what it received (request line, the
/// `host`, `authorization` and `proxy-*` headers, the body), a header holding [`VALUE`] and a
/// `content-encoding` an agent's answer would be refused for. Keeps what it saw.
async fn origin(
    name: &'static str,
) -> Result<(SocketAddr, Arc<Mutex<Vec<String>>>), Box<dyn std::error::Error>> {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await?;
    let addr = listener.local_addr()?;
    let seen = Arc::new(Mutex::new(Vec::new()));
    let kept = Arc::clone(&seen);
    tokio::spawn(async move {
        while let Ok((mut stream, _)) = listener.accept().await {
            let kept = Arc::clone(&kept);
            tokio::spawn(async move {
                let mut request = Vec::new();
                let mut chunk = [0u8; 4096];
                let (head, mut body) = loop {
                    let Ok(n) = stream.read(&mut chunk).await else {
                        return;
                    };
                    if n == 0 {
                        return;
                    }
                    request.extend_from_slice(&chunk[..n]);
                    if let Some(end) = request.windows(4).position(|w| w == b"\r\n\r\n") {
                        let head = String::from_utf8_lossy(&request[..end]).into_owned();
                        break (head, request[end + 4..].to_vec());
                    }
                };
                let length = head
                    .lines()
                    .find_map(|line| {
                        line.to_ascii_lowercase()
                            .strip_prefix("content-length:")
                            .and_then(|n| n.trim().parse::<usize>().ok())
                    })
                    .unwrap_or(0);
                while body.len() < length {
                    let Ok(n) = stream.read(&mut chunk).await else {
                        return;
                    };
                    if n == 0 {
                        break;
                    }
                    body.extend_from_slice(&chunk[..n]);
                }
                let kept_headers: Vec<&str> = head
                    .lines()
                    .skip(1)
                    .filter(|line| {
                        let line = line.to_ascii_lowercase();
                        ["host:", "authorization:", "proxy-", "x-kept:", "x-drop:"]
                            .iter()
                            .any(|name| line.starts_with(name))
                    })
                    .collect();
                let text = format!(
                    "{name} got [{}] [{}] [{}]",
                    head.lines().next().unwrap_or_default(),
                    kept_headers.join("; "),
                    String::from_utf8_lossy(&body)
                );
                if let Ok(mut kept) = kept.lock() {
                    kept.push(text.clone());
                }
                let answer = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: text/plain\r\ncontent-encoding: gzip\r\nx-secret: {VALUE}\r\nkeep-alive: timeout=5\r\nproxy-authenticate: Basic realm=\"origin\"\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{text}",
                    text.len()
                );
                let _ = stream.write_all(answer.as_bytes()).await;
            });
        }
    });
    Ok((addr, seen))
}

struct Setup {
    /// The browser's own proxy listener.
    proxy: SocketAddr,
    /// The proxy as agent runs reach it (with a run's token).
    agents: SocketAddr,
    policy: Arc<Policy>,
}

/// What a test's proxy is made with.
struct Options {
    /// The ports of sandbox `s1`, whose browser the proxy serves.
    ports: std::ops::RangeInclusive<u16>,
    /// The listener of Yulius's pages.
    pages: Option<SocketAddr>,
    /// Connections per sandbox.
    share: usize,
    /// What names resolve to.
    resolve: HashMap<String, SocketAddr>,
    /// How long a plain request waits for its answer's head without progress.
    head: Duration,
    /// Whether the agents' policy intercepts hosts.
    intercepts: bool,
}

impl Options {
    fn new(ports: std::ops::RangeInclusive<u16>) -> Self {
        Self {
            ports,
            pages: None,
            share: 8,
            resolve: HashMap::new(),
            head: yulius_egress::IDLE_FLOOR,
            intercepts: true,
        }
    }
}

async fn setup(options: Options) -> Result<Setup, Box<dyn std::error::Error>> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let policy = Arc::new(Policy {
        asked: AtomicUsize::new(0),
        intercepts: options.intercepts,
    });
    let proxy = Arc::new(
        EgressProxy::new(
            policy.clone(),
            CertificateAuthority::from_material(&generate_ca_material()?)?,
            EgressConfig {
                max_per_sandbox: options.share,
                resolve: options.resolve,
                response_head_timeout: options.head,
                ..EgressConfig::default()
            },
        )
        .map_err(|e| e.to_string())?,
    );
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await?;
    let addr = listener.local_addr()?;
    let browser = Arc::new(BrowserEgress {
        sandbox: "s1".into(),
        ports: options.ports,
        pages: options
            .pages
            .map(|addr| (addr, vec!["/browser/welcome?".to_owned()])),
        listeners: Vec::new(),
    });
    tokio::spawn(serve_browser(listener, Arc::clone(&proxy), browser));
    let agents = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await?;
    let agents_addr = agents.local_addr()?;
    tokio::spawn(yulius_egress::serve(agents, proxy));
    Ok(Setup {
        proxy: addr,
        agents: agents_addr,
        policy,
    })
}

/// Sends `request` (a whole HTTP/1.1 request asking to close) to the proxy and returns the
/// whole answer.
async fn ask(proxy: SocketAddr, request: &str) -> Result<String, Box<dyn std::error::Error>> {
    let mut stream = TcpStream::connect(proxy).await?;
    stream.write_all(request.as_bytes()).await?;
    let mut answer = Vec::new();
    tokio::time::timeout(Duration::from_secs(10), stream.read_to_end(&mut answer)).await??;
    Ok(String::from_utf8_lossy(&answer).into_owned())
}

/// A plain `GET` of `url` as Chrome sends it to a proxy.
async fn get(proxy: SocketAddr, url: &str) -> Result<String, Box<dyn std::error::Error>> {
    let authority = url
        .strip_prefix("http://")
        .and_then(|rest| rest.split('/').next())
        .unwrap_or_default();
    ask(
        proxy,
        &format!(
            "GET {url} HTTP/1.1\r\nHost: {authority}\r\nProxy-Connection: keep-alive\r\nConnection: close\r\n\r\n"
        ),
    )
    .await
}

/// Opens a `CONNECT` tunnel to `target`: the stream and the proxy's status line.
async fn connect(
    proxy: SocketAddr,
    target: &str,
) -> Result<(TcpStream, String), Box<dyn std::error::Error>> {
    connect_as(proxy, target, "").await
}

/// [`connect`] with more request headers (`headers`, each ending in CRLF): an agent run's
/// proxy credential.
async fn connect_as(
    proxy: SocketAddr,
    target: &str,
    headers: &str,
) -> Result<(TcpStream, String), Box<dyn std::error::Error>> {
    let mut stream = TcpStream::connect(proxy).await?;
    stream
        .write_all(
            format!("CONNECT {target} HTTP/1.1\r\nHost: {target}\r\n{headers}\r\n").as_bytes(),
        )
        .await?;
    let mut head = Vec::new();
    let mut byte = [0u8; 1];
    while !head.ends_with(b"\r\n\r\n") && head.len() < 8192 {
        if tokio::time::timeout(Duration::from_secs(10), stream.read(&mut byte)).await?? == 0 {
            break;
        }
        head.push(byte[0]);
    }
    let status = String::from_utf8_lossy(&head)
        .lines()
        .next()
        .unwrap_or_default()
        .to_owned();
    Ok((stream, status))
}

fn status(answer: &str) -> &str {
    answer.lines().next().unwrap_or_default()
}

/// SPEC-483 E2: the browser loads its sandbox's dev server on the loopback, by address and by
/// name, plainly and through a tunnel (a WebSocket's way); another sandbox's port, a service of
/// the host and a cloud's metadata address are refused before anything is sent.
#[tokio::test]
async fn a_browser_loads_its_sandboxes_ports_and_no_other_local_address() -> TestResult {
    let (own, own_seen) = origin("own").await?;
    let (other, other_seen) = origin("other").await?;
    let setup = setup(Options::new(own.port()..=own.port())).await?;

    for host in ["127.0.0.1", "localhost", "app.localhost"] {
        let answer = get(
            setup.proxy,
            &format!("http://{host}:{}/app?x=1", own.port()),
        )
        .await?;
        assert_eq!(status(&answer), "HTTP/1.1 200 OK", "{answer}");
        assert!(
            answer.contains("own got [GET /app?x=1 HTTP/1.1]"),
            "{answer}"
        );
        assert!(
            answer.contains(&format!("host: {host}:{}", own.port())),
            "the origin sees the host the page asked for: {answer}"
        );
    }
    for host in ["127.0.0.1", "localhost"] {
        let (mut tunnel, opened) = connect(setup.proxy, &format!("{host}:{}", own.port())).await?;
        assert_eq!(opened, "HTTP/1.1 200 OK", "{host}");
        tunnel
            .write_all(b"GET /socket HTTP/1.1\r\nHost: dev\r\n\r\n")
            .await?;
        let mut through = Vec::new();
        tokio::time::timeout(Duration::from_secs(10), tunnel.read_to_end(&mut through)).await??;
        assert!(
            String::from_utf8_lossy(&through).contains("own got [GET /socket HTTP/1.1]"),
            "{host}: {}",
            String::from_utf8_lossy(&through)
        );
    }
    let served = own_seen.lock().map_err(|_| "lock")?.len();
    assert_eq!(served, 5);

    for host in ["127.0.0.1", "localhost", "[::1]", "127.0.0.2", "0.0.0.0"] {
        let url = format!("http://{host}:{}/", other.port());
        let answer = get(setup.proxy, &url).await?;
        assert_eq!(status(&answer), "HTTP/1.1 403 Forbidden", "{url}: {answer}");
        assert!(
            answer.contains("the sandbox browser does not load")
                && answer.contains(&format!("({}-{} on localhost", own.port(), own.port())),
            "{answer}"
        );
        let target = format!("{host}:{}", other.port());
        let (_, refused) = connect(setup.proxy, &target).await?;
        assert_eq!(refused, "HTTP/1.1 403 Forbidden", "{target}");
    }
    for target in [
        "169.254.169.254:80",
        "[fe80::1]:80",
        "[::ffff:127.0.0.1]:80",
    ] {
        let answer = get(setup.proxy, &format!("http://{target}/latest/meta-data/")).await?;
        assert_eq!(
            status(&answer),
            "HTTP/1.1 403 Forbidden",
            "{target}: {answer}"
        );
        let (_, refused) = connect(setup.proxy, target).await?;
        assert_eq!(refused, "HTTP/1.1 403 Forbidden", "{target}");
    }
    assert!(other_seen.lock().map_err(|_| "lock")?.is_empty());
    assert_eq!(own_seen.lock().map_err(|_| "lock")?.len(), served);
    Ok(())
}

/// Yulius's internal listener gives the browser its welcome page and nothing else: not its
/// API by another path, and no tunnel, whose path the proxy would not see.
#[tokio::test]
async fn of_yulius_the_browser_gets_its_welcome_page_only() -> TestResult {
    let (yulius, seen) = origin("yulius").await?;
    let setup = setup(Options {
        pages: Some(yulius),
        ..Options::new(1..=1)
    })
    .await?;
    let base = format!("http://127.0.0.1:{}", yulius.port());

    let welcome = get(setup.proxy, &format!("{base}/browser/welcome?sandbox=s1")).await?;
    assert_eq!(status(&welcome), "HTTP/1.1 200 OK", "{welcome}");
    assert!(
        welcome.contains("yulius got [GET /browser/welcome?sandbox=s1 HTTP/1.1]"),
        "{welcome}"
    );
    for path in ["/internal/version", "/mcp", "/browser/welcome", "/"] {
        let answer = get(setup.proxy, &format!("{base}{path}")).await?;
        assert_eq!(
            status(&answer),
            "HTTP/1.1 403 Forbidden",
            "{path}: {answer}"
        );
    }
    let by_name = get(
        setup.proxy,
        &format!("http://localhost:{}/internal/version", yulius.port()),
    )
    .await?;
    assert_eq!(status(&by_name), "HTTP/1.1 403 Forbidden", "{by_name}");
    let (_, refused) = connect(setup.proxy, &format!("127.0.0.1:{}", yulius.port())).await?;
    assert_eq!(refused, "HTTP/1.1 403 Forbidden");
    assert_eq!(seen.lock().map_err(|_| "lock")?.len(), 1);
    Ok(())
}

/// The page is the sandbox's, not Yulius's: no credential is asked of the browser, an agent
/// secret is neither added to its requests nor taken out of the answers, an answer's encoding
/// is not judged, a body goes through, and of the request only one hop's headers stay behind.
#[tokio::test]
async fn nothing_is_added_to_or_taken_from_a_browsers_requests() -> TestResult {
    let (own, seen) = origin("own").await?;
    let setup = setup(Options::new(own.port()..=own.port())).await?;
    // The address carries a user and a password, as a link may: neither goes on as the host.
    let url = format!("http://user:pw@127.0.0.1:{}/form", own.port());
    let answer = ask(
        setup.proxy,
        &format!(
            "POST {url} HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nProxy-Connection: keep-alive\r\nProxy-Authorization: Basic eXVsaXVzOmdvb2QtdG9rZW4=\r\nX-Kept: yes\r\nX-Drop: for this hop\r\nContent-Length: 9\r\nConnection: close, x-drop\r\n\r\nname=demo",
            own.port()
        ),
    )
    .await?;
    assert_eq!(status(&answer), "HTTP/1.1 200 OK", "{answer}");
    // What the origin said to its own hop stays there.
    let head = answer
        .split("\r\n\r\n")
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase();
    assert!(
        !head.contains("keep-alive") && !head.contains("proxy-authenticate"),
        "{answer}"
    );
    // The answer as the origin gave it: the value it echoed, the encoding it claimed.
    assert!(answer.contains(&format!("x-secret: {VALUE}")), "{answer}");
    assert!(answer.contains("content-encoding: gzip"), "{answer}");
    let seen = seen.lock().map_err(|_| "lock")?.clone();
    assert_eq!(seen.len(), 1);
    assert!(seen[0].contains("[POST /form HTTP/1.1]"), "{}", seen[0]);
    assert!(seen[0].contains("[name=demo]"), "{}", seen[0]);
    assert!(seen[0].contains("x-kept: yes"), "{}", seen[0]);
    let lower = seen[0].to_ascii_lowercase();
    assert!(
        !lower.contains("authorization") && !lower.contains("proxy-") && !lower.contains("x-drop"),
        "nothing injected, nothing of the proxy's hop sent on: {}",
        seen[0]
    );
    assert!(
        lower.contains(&format!("host: 127.0.0.1:{}", own.port())) && !lower.contains("user"),
        "the host without what stood before it: {}",
        seen[0]
    );
    assert_eq!(
        setup.policy.asked.load(Ordering::SeqCst),
        0,
        "the policy of agent runs (interception, injection, redaction) is never asked"
    );
    Ok(())
}

/// An agent run's proxy credential (`yulius:good-token`).
const AGENT: &str = "Proxy-Authorization: Basic eXVsaXVzOmdvb2QtdG9rZW4=\r\n";

/// The browser's connections count toward its sandbox's share of the proxy (SPEC-353), the one
/// its agent runs have: at a share of one, a run's tunnel leaves none for the browser and the
/// browser's leaves none for a run.
#[tokio::test]
async fn a_browsers_tunnels_count_toward_its_sandboxes_share() -> TestResult {
    let (own, _) = origin("own").await?;
    let target = format!("127.0.0.1:{}", own.port());
    let options = || Options {
        share: 1,
        intercepts: false,
        resolve: HashMap::from([("tunnel.test".to_owned(), own)]),
        ..Options::new(own.port()..=own.port())
    };

    let first = setup(options()).await?;
    let (_held, run) = connect_as(first.agents, "tunnel.test:443", AGENT).await?;
    assert_eq!(
        run, "HTTP/1.1 200 OK",
        "an agent run of the sandbox holds its one place"
    );
    let (_, browser) = connect(first.proxy, &target).await?;
    assert_eq!(browser, "HTTP/1.1 503 Service Unavailable");

    let second = setup(options()).await?;
    let (_held, browser) = connect(second.proxy, &target).await?;
    assert_eq!(browser, "HTTP/1.1 200 OK");
    let (_, again) = connect(second.proxy, &target).await?;
    assert_eq!(again, "HTTP/1.1 503 Service Unavailable");
    let (_, run) = connect_as(second.agents, "tunnel.test:443", AGENT).await?;
    assert_eq!(run, "HTTP/1.1 503 Service Unavailable");
    Ok(())
}

/// A name is not an address (review of slice 9): one that a resolver answers with the
/// loopback reaches nothing there, not the sandbox's own port and not the welcome page, by
/// plain request or tunnel. Its owner could answer a public address for a page and the
/// loopback for that page's next request, and the page would read the sandbox's dev server.
#[tokio::test]
async fn a_name_that_resolves_to_the_loopback_reaches_nothing_there() -> TestResult {
    let (own, own_seen) = origin("own").await?;
    let (yulius, yulius_seen) = origin("yulius").await?;
    let setup = setup(Options {
        pages: Some(yulius),
        resolve: HashMap::from([
            ("rebound.test".to_owned(), own),
            ("pages.test".to_owned(), yulius),
        ]),
        ..Options::new(own.port()..=own.port())
    })
    .await?;
    for url in [
        format!("http://rebound.test:{}/", own.port()),
        format!(
            "http://pages.test:{}/browser/welcome?sandbox=s1",
            yulius.port()
        ),
    ] {
        let answer = get(setup.proxy, &url).await?;
        assert_eq!(status(&answer), "HTTP/1.1 403 Forbidden", "{url}: {answer}");
    }
    let (_, refused) = connect(setup.proxy, &format!("rebound.test:{}", own.port())).await?;
    assert_eq!(refused, "HTTP/1.1 403 Forbidden");
    assert!(own_seen.lock().map_err(|_| "lock")?.is_empty());
    assert!(yulius_seen.lock().map_err(|_| "lock")?.is_empty());
    // By its address, both are what they were.
    let answer = get(setup.proxy, &format!("http://127.0.0.1:{}/", own.port())).await?;
    assert_eq!(status(&answer), "HTTP/1.1 200 OK", "{answer}");
    Ok(())
}

/// An origin that takes every connection, reads and never answers.
async fn silent() -> Result<SocketAddr, Box<dyn std::error::Error>> {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await?;
    let addr = listener.local_addr()?;
    tokio::spawn(async move {
        while let Ok((mut stream, _)) = listener.accept().await {
            tokio::spawn(async move {
                let mut chunk = [0u8; 1024];
                while matches!(stream.read(&mut chunk).await, Ok(n) if n > 0) {}
            });
        }
    });
    Ok(addr)
}

/// SPEC-369 PHT-001 for a browser's plain requests: an answer's head is waited for with a
/// bound that restarts with every piece of the request's body sent on, so an upload longer in
/// all than the bound gets its answer, and a server that never answers gets 504.
#[tokio::test]
async fn a_browsers_upload_is_not_cut_and_a_silent_server_is_given_up() -> TestResult {
    let (own, seen) = origin("own").await?;
    let silent = silent().await?;
    let bound = Duration::from_secs(2);
    let setup = setup(Options {
        head: bound,
        ..Options::new(own.port().min(silent.port())..=own.port().max(silent.port()))
    })
    .await?;

    let mut stream = TcpStream::connect(setup.proxy).await?;
    stream
        .write_all(
            format!(
                "POST http://127.0.0.1:{0}/upload HTTP/1.1\r\nHost: 127.0.0.1:{0}\r\nContent-Length: 12\r\nConnection: close\r\n\r\n",
                own.port()
            )
            .as_bytes(),
        )
        .await?;
    // The page's own pace: 2.4 s in all, a piece every 200 ms.
    for _ in 0..12 {
        tokio::time::sleep(bound / 10).await;
        stream.write_all(b"x").await?;
    }
    let mut answer = Vec::new();
    tokio::time::timeout(Duration::from_secs(30), stream.read_to_end(&mut answer)).await??;
    let answer = String::from_utf8_lossy(&answer);
    assert_eq!(status(&answer), "HTTP/1.1 200 OK", "{answer}");
    assert!(
        seen.lock().map_err(|_| "lock")?[0].contains("[xxxxxxxxxxxx]"),
        "the whole body arrived"
    );

    let asked = std::time::Instant::now();
    let answer = get(setup.proxy, &format!("http://127.0.0.1:{}/", silent.port())).await?;
    assert_eq!(status(&answer), "HTTP/1.1 504 Gateway Timeout", "{answer}");
    assert!(asked.elapsed() >= bound, "{:?}", asked.elapsed());
    Ok(())
}
