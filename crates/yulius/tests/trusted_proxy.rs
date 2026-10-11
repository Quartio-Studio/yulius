//! Per-client sign-in limits behind the TLS reverse proxy (SPEC-001 ACC-013, AC-ACC-9; #88).
//!
//! The public router is served on a real loopback listener with peer addresses recorded, as
//! `yulius serve` does, so every request's TCP peer is `127.0.0.1` like behind the proxy. The
//! proxy's client address counts only with the matching shared secret; a missing or wrong
//! secret, and forged headers sent straight to the listener, count against the peer; the secret
//! never reaches a response or a log.

mod support;

use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use support::{Harness, TestResult, harness_configured, setup_superadmin_creds};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use yulius_app::auth::{LOGIN_ATTEMPTS_PER_MINUTE, MAX_LOGIN_CLIENTS};
use yulius_app::error::AppError;
use yulius_app::model::Secret;
use yulius_server::client::{CLIENT_ADDRESS_HEADER, PROXY_SECRET_HEADER};

/// A synthetic secret (64 hex characters, like `openssl rand -hex 32`).
const SECRET: &str = "c0ffee00d15ea5e5b1e55ed5ca1ab1e0c0ffee00d15ea5e5b1e55ed5ca1ab1e0";
/// The superadmin password `setup_superadmin_creds` chooses.
const PASSWORD: &str = "a very long secret";
const WRONG: &str = "not the password";

/// Yulius with a superadmin, its public router on `127.0.0.1:<OS port>` with peer addresses.
struct Server {
    h: Harness,
    addr: SocketAddr,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for Server {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn start(secret: Option<&str>) -> Result<Server, Box<dyn std::error::Error>> {
    let secret = secret.map(|s| Secret(s.to_owned()));
    let h = harness_configured(move |c| c.proxy_secret = secret)?;
    setup_superadmin_creds(&h).await?;
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await?;
    let addr = listener.local_addr()?;
    let service = yulius_server::public_router(h.app.clone())
        .into_make_service_with_connect_info::<SocketAddr>();
    let task = tokio::spawn(async move {
        let _ = axum::serve(listener, service).await;
    });
    Ok(Server { h, addr, task })
}

/// One HTTP/1.1 exchange over a fresh connection: the status and the raw response.
async fn exchange(
    addr: SocketAddr,
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: Option<&str>,
) -> Result<(u16, String), Box<dyn std::error::Error>> {
    let mut request =
        format!("{method} {path} HTTP/1.1\r\nHost: yulius.test\r\nConnection: close\r\n");
    for (name, value) in headers {
        request.push_str(&format!("{name}: {value}\r\n"));
    }
    if let Some(body) = body {
        request.push_str(&format!(
            "Content-Type: application/json\r\nContent-Length: {}\r\n",
            body.len()
        ));
    }
    request.push_str("\r\n");
    request.push_str(body.unwrap_or(""));
    let mut stream = tokio::net::TcpStream::connect(addr).await?;
    stream.write_all(request.as_bytes()).await?;
    let mut raw = Vec::new();
    tokio::time::timeout(Duration::from_secs(20), stream.read_to_end(&mut raw)).await??;
    let raw = String::from_utf8_lossy(&raw).into_owned();
    let status = raw
        .split(' ')
        .nth(1)
        .and_then(|code| code.parse().ok())
        .ok_or_else(|| format!("no status line: {raw}"))?;
    Ok((status, raw))
}

/// A password sign-in with `headers`; the status and the raw response.
async fn sign_in(
    s: &Server,
    password: &str,
    headers: &[(&str, &str)],
) -> Result<(u16, String), Box<dyn std::error::Error>> {
    let body = serde_json::json!({ "password": password }).to_string();
    exchange(s.addr, "POST", "/api/auth/password", headers, Some(&body)).await
}

/// The headers the proxy sets for a client at `address`.
fn proxied(address: &str) -> [(&'static str, &str); 2] {
    [
        (CLIENT_ADDRESS_HEADER, address),
        (PROXY_SECRET_HEADER, SECRET),
    ]
}

/// Uses up the bucket of the client `headers` describe (every attempt a wrong password).
async fn exhaust(s: &Server, headers: &[(&str, &str)]) -> TestResult {
    for _ in 0..LOGIN_ATTEMPTS_PER_MINUTE {
        assert_eq!(sign_in(s, WRONG, headers).await?.0, 401);
    }
    Ok(())
}

/// AC-ACC-9: with the secret configured, two clients behind the proxy have separate buckets;
/// an IPv6 client is its /64 network.
#[tokio::test]
async fn proxied_clients_have_separate_sign_in_limits() -> TestResult {
    let s = start(Some(SECRET)).await?;
    exhaust(&s, &proxied("203.0.113.7")).await?;
    assert_eq!(
        sign_in(&s, PASSWORD, &proxied("203.0.113.7")).await?.0,
        429,
        "the client that failed five times waits"
    );
    // Another client is not locked out by the first one's failures.
    assert_eq!(
        sign_in(&s, PASSWORD, &proxied("198.51.100.9")).await?.0,
        200
    );
    for _ in 1..LOGIN_ATTEMPTS_PER_MINUTE {
        assert_eq!(sign_in(&s, WRONG, &proxied("198.51.100.9")).await?.0, 401);
    }
    // Nor is someone on the server itself (the peer's bucket is untouched).
    assert_eq!(sign_in(&s, PASSWORD, &[]).await?.0, 200);

    // IPv6: one subscriber's /64 shares a bucket, another /64 does not.
    exhaust(&s, &proxied("2001:db8:1:2::1")).await?;
    assert_eq!(
        sign_in(&s, PASSWORD, &proxied("2001:db8:1:2::99")).await?.0,
        429
    );
    assert_eq!(
        sign_in(&s, PASSWORD, &proxied("2001:db8:1:3::1")).await?.0,
        200
    );

    // A minute later the first client may try again.
    s.h.clock
        .0
        .fetch_add(61_000, std::sync::atomic::Ordering::SeqCst);
    assert_eq!(sign_in(&s, PASSWORD, &proxied("203.0.113.7")).await?.0, 200);
    Ok(())
}

/// AC-ACC-9: a missing, wrong, partial or repeated secret, or the secret with an unusable
/// address, counts against the TCP peer, whatever address the request names.
#[tokio::test]
async fn a_missing_or_wrong_secret_falls_back_to_the_peer_bucket() -> TestResult {
    let s = start(Some(SECRET)).await?;
    let wrong = format!("{}x", &SECRET[..SECRET.len() - 1]);
    let untrusted: [Vec<(&str, &str)>; LOGIN_ATTEMPTS_PER_MINUTE] = [
        vec![(CLIENT_ADDRESS_HEADER, "192.0.2.1")],
        vec![
            (CLIENT_ADDRESS_HEADER, "192.0.2.2"),
            (PROXY_SECRET_HEADER, &wrong),
        ],
        vec![
            (CLIENT_ADDRESS_HEADER, "192.0.2.3"),
            (PROXY_SECRET_HEADER, &SECRET[..32]),
        ],
        vec![
            (CLIENT_ADDRESS_HEADER, "192.0.2.4"),
            (PROXY_SECRET_HEADER, SECRET),
            (PROXY_SECRET_HEADER, SECRET),
        ],
        vec![
            (CLIENT_ADDRESS_HEADER, "not-an-address"),
            (PROXY_SECRET_HEADER, SECRET),
        ],
    ];
    for headers in &untrusted {
        assert_eq!(sign_in(&s, WRONG, headers).await?.0, 401);
    }
    // All five counted against 127.0.0.1: a sixth request from the peer waits, even with the
    // right password and yet another address.
    assert_eq!(sign_in(&s, PASSWORD, &[]).await?.0, 429);
    assert_eq!(
        sign_in(
            &s,
            PASSWORD,
            &[
                (CLIENT_ADDRESS_HEADER, "192.0.2.6"),
                (PROXY_SECRET_HEADER, &wrong)
            ]
        )
        .await?
        .0,
        429
    );
    // The proxy's own requests keep their clients' buckets, including an address an untrusted
    // request named before.
    assert_eq!(sign_in(&s, PASSWORD, &proxied("192.0.2.1")).await?.0, 200);
    Ok(())
}

/// AC-ACC-9: a process that reaches the listener on loopback (a sandbox without isolation)
/// forging a new client address per request stays in the peer's bucket, with or without a
/// secret configured; without the setting, every request is keyed on the peer as before.
#[tokio::test]
async fn forged_headers_on_a_direct_loopback_request_fall_back_to_the_peer_bucket() -> TestResult {
    for configured in [Some(SECRET), None] {
        let s = start(configured).await?;
        for n in 1..=LOGIN_ATTEMPTS_PER_MINUTE {
            let forged = format!("198.18.0.{n}");
            let guess = format!("{n:064x}");
            assert_eq!(
                sign_in(
                    &s,
                    WRONG,
                    &[
                        (CLIENT_ADDRESS_HEADER, &forged),
                        (PROXY_SECRET_HEADER, &guess),
                        ("x-forwarded-for", &forged),
                        ("x-real-ip", &forged),
                    ]
                )
                .await?
                .0,
                401
            );
        }
        let (status, _) = sign_in(
            &s,
            PASSWORD,
            &[
                (CLIENT_ADDRESS_HEADER, "198.18.0.99"),
                ("x-forwarded-for", "198.18.0.99"),
            ],
        )
        .await?;
        assert_eq!(status, 429, "forged addresses never open new buckets");
        let proxied_status = sign_in(&s, PASSWORD, &proxied("203.0.113.50")).await?.0;
        match configured {
            Some(_) => assert_eq!(proxied_status, 200, "the proxy's clients are unaffected"),
            // Without the setting the proxy's headers mean nothing: unchanged behaviour.
            None => assert_eq!(proxied_status, 429),
        }
    }
    Ok(())
}

/// A log sink for the current thread (the server's tasks run on the test's thread).
#[derive(Clone, Default)]
struct Logs(Arc<Mutex<Vec<u8>>>);

impl std::io::Write for Logs {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        if let Ok(mut logs) = self.0.lock() {
            logs.extend_from_slice(buf);
        }
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl Logs {
    fn text(&self) -> String {
        self.0
            .lock()
            .map(|logs| String::from_utf8_lossy(&logs).into_owned())
            .unwrap_or_default()
    }
}

/// AC-ACC-9: the secret never appears in a response (success, failure, limit, untrusted
/// requests, the signed-in user's own views), in a log at any level, in the configuration's
/// debug form, or in what loading the secret file logs, usable or not.
#[tokio::test]
async fn the_proxy_secret_never_reaches_a_response_or_a_log() -> TestResult {
    let logs = Logs::default();
    let sink = logs.clone();
    let subscriber = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::TRACE)
        .with_ansi(false)
        .with_writer(move || sink.clone())
        .finish();
    let _guard = tracing::subscriber::set_default(subscriber);

    let s = start(Some(SECRET)).await?;
    let mut responses = Vec::new();
    let (status, ok) = sign_in(&s, PASSWORD, &proxied("203.0.113.7")).await?;
    assert_eq!(status, 200);
    let cookie = ok
        .lines()
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.eq_ignore_ascii_case("set-cookie")
                .then(|| value.trim().split(';').next().map(str::to_owned))
                .flatten()
        })
        .ok_or("session cookie")?;
    responses.push(ok);
    for _ in 0..LOGIN_ATTEMPTS_PER_MINUTE {
        responses.push(sign_in(&s, WRONG, &proxied("203.0.113.8")).await?.1);
    }
    let (status, limited) = sign_in(&s, PASSWORD, &proxied("203.0.113.8")).await?;
    assert_eq!(status, 429);
    responses.push(limited);
    let wrong = format!("{}x", &SECRET[..SECRET.len() - 1]);
    responses.push(
        sign_in(
            &s,
            WRONG,
            &[
                (CLIENT_ADDRESS_HEADER, "192.0.2.1"),
                (PROXY_SECRET_HEADER, &wrong),
            ],
        )
        .await?
        .1,
    );
    responses.push(
        sign_in(
            &s,
            WRONG,
            &[
                (CLIENT_ADDRESS_HEADER, "bad"),
                (PROXY_SECRET_HEADER, SECRET),
            ],
        )
        .await?
        .1,
    );
    for path in ["/api/me", "/api/setup/status", "/api/auth/recovery"] {
        let (status, raw) = exchange(
            s.addr,
            "GET",
            path,
            &[("cookie", &cookie), (PROXY_SECRET_HEADER, SECRET)],
            None,
        )
        .await?;
        assert_eq!(status, 200, "{path}: {raw}");
        responses.push(raw);
    }
    for raw in &responses {
        assert!(
            !raw.contains(SECRET),
            "a response carries the secret: {raw}"
        );
        assert!(
            !raw.contains(&SECRET[..16]),
            "a response carries part of the secret: {raw}"
        );
    }
    let config = format!("{:?}", s.h.app.config);
    assert!(!config.contains(SECRET) && config.contains("Secret(<redacted>)"));

    // Loading the file, usable or not, says what happened without the secret.
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("proxy.secret");
    std::fs::write(&path, format!("{SECRET}\n"))?;
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644))?;
        assert!(yulius::wiring::proxy::load(Some(&path)).is_none());
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
        let loaded = yulius::wiring::proxy::load(Some(&path)).ok_or("loaded")?;
        assert_eq!(loaded.expose(), SECRET);
    }
    let text = logs.text();
    assert!(text.contains("proxy secret file is not usable"), "{text}");
    assert!(text.contains("trusted reverse proxy"), "{text}");
    assert!(
        !text.contains(SECRET),
        "a log line carries the secret:\n{text}"
    );
    assert!(
        !text.contains(&SECRET[..16]),
        "a log line carries part of the secret:\n{text}"
    );
    Ok(())
}

/// The limiter's memory is bounded (SPEC-002 §9): clients idle for a minute are forgotten first;
/// with the table full of active clients, a new client waits, while known clients keep their
/// buckets.
#[tokio::test]
async fn the_sign_in_limiter_tracks_a_bounded_number_of_clients() -> TestResult {
    // No superadmin: every attempt is refused without hashing, so the table fills quickly.
    let h = harness_configured(|_| {})?;
    let wrong = Secret(WRONG.into());
    for n in 0..MAX_LOGIN_CLIENTS {
        let refused = h.app.password_login(&wrong, &format!("client-{n}")).await;
        assert!(matches!(refused, Err(AppError::Unauthenticated)), "{n}");
    }
    assert!(matches!(
        h.app.password_login(&wrong, "newcomer").await,
        Err(AppError::Capacity(_))
    ));
    assert!(matches!(
        h.app.password_login(&wrong, "client-0").await,
        Err(AppError::Unauthenticated)
    ));
    h.clock
        .0
        .fetch_add(61_000, std::sync::atomic::Ordering::SeqCst);
    assert!(matches!(
        h.app.password_login(&wrong, "newcomer").await,
        Err(AppError::Unauthenticated)
    ));
    Ok(())
}
