//! Live previews end to end (SPEC-002 ADR-14; SPEC-001 BRW-005, BRW-008, AC-BRW-4, AC-BRW-7,
//! AC-BRW-8): the preview proxy in front of a fixture dev server on the sandbox's port — the
//! token handshake, header and redirect rewriting, WebSocket passthrough, refusal of other
//! users, other sandboxes, cross-site callers and Yulius itself — and the guarantee that a page
//! on a preview origin can never use the user's Yulius session.

mod support;

use std::sync::atomic::Ordering;
use std::time::Duration;

use axum::Router;
use axum::body::Body;
use axum::extract::ws::{Message as AxumMessage, WebSocketUpgrade};
use axum::http::{HeaderMap, Method, Request, StatusCode, header};
use axum::response::IntoResponse;
use axum::routing::{get, post};
use futures::{SinkExt as _, StreamExt as _};
use http_body_util::BodyExt as _;
use serde_json::{Value, json};
use support::{Harness, HarnessOptions, TestResult, harness_with, perms, seed_github_user, serve};
use tokio_tungstenite::tungstenite::Message;
use tower::ServiceExt as _;
use yulius_app::model::{SandboxRecord, WebSessionRecord};
use yulius_app::ports::store::{SandboxStore, SessionStore};
use yulius_domain::ids::{SandboxId, UserId};
use yulius_domain::time::Timestamp;

const YULIUS: &str = "https://yulius.test";

/// The fixture dev server: what an agent's `npm run dev` would serve on the sandbox's port.
fn fixture_app(port: u16, tag: String) -> Router {
    Router::new()
        .route(
            "/",
            get(move || async move {
                (
                    [
                        (header::CONTENT_TYPE, "text/html; charset=utf-8".to_owned()),
                        (header::X_FRAME_OPTIONS, "DENY".to_owned()),
                        (
                            header::CONTENT_SECURITY_POLICY,
                            format!("default-src 'self'; frame-ancestors 'none'; connect-src ws://localhost:{port}"),
                        ),
                        (header::STRICT_TRANSPORT_SECURITY, "max-age=60".to_owned()),
                        (header::SET_COOKIE, "sid=app; Domain=localhost; Path=/; HttpOnly".to_owned()),
                    ],
                    "<!doctype html><html><head><link rel=\"stylesheet\" href=\"/app.css\"></head><body><h1>Fixture app</h1></body></html>",
                )
            }),
        )
        .route(
            "/app.css",
            get(|| async { ([(header::CONTENT_TYPE, "text/css")], "h1 { color: red; }") }),
        )
        .route(
            "/redirect",
            get(move || async move {
                (
                    StatusCode::FOUND,
                    [(header::LOCATION, format!("http://localhost:{port}/landed?x=1"))],
                )
            }),
        )
        .route(
            "/sibling",
            get(move || async move {
                (
                    StatusCode::FOUND,
                    [(header::LOCATION, format!("http://127.0.0.1:{}/x", port + 1))],
                )
            }),
        )
        .route(
            "/away",
            get(|| async { (StatusCode::FOUND, [(header::LOCATION, "https://example.com/x")]) }),
        )
        .route(
            "/cors",
            get(move || async move {
                (
                    [(header::ACCESS_CONTROL_ALLOW_ORIGIN, format!("http://localhost:{port}"))],
                    "ok",
                )
            }),
        )
        .route(
            "/headers",
            get(|headers: HeaderMap| async move {
                let pick = |name: &str| {
                    headers
                        .get(name)
                        .and_then(|v| v.to_str().ok())
                        .map(str::to_owned)
                };
                axum::Json(json!({
                    "host": pick("host"), "origin": pick("origin"), "referer": pick("referer"),
                    "cookie": pick("cookie"), "x-forwarded-for": pick("x-forwarded-for"),
                    "x-forwarded-host": pick("x-forwarded-host"),
                }))
            }),
        )
        .route(
            "/upload",
            post(|body: axum::body::Bytes| async move { body.len().to_string() })
                .layer(axum::extract::DefaultBodyLimit::max(16 << 20)),
        )
        .route(
            "/yulius",
            get(move || {
                let tag = tag.clone();
                async move { ([("x-yulius", tag)], "the Yulius UI") }
            }),
        )
        .route(
            "/ws",
            get(|ws: WebSocketUpgrade| async move {
                ws.on_upgrade(|mut socket| async move {
                    while let Some(Ok(message)) = socket.recv().await {
                        if let AxumMessage::Text(text) = message {
                            let reply = format!("echo: {}", text.as_str());
                            if socket.send(AxumMessage::Text(reply.into())).await.is_err() {
                                break;
                            }
                        }
                    }
                })
                .into_response()
            }),
        )
}

struct Fixture {
    h: Harness,
    port: u16,
    key: String,
    /// alice's Yulius session cookie value (write access).
    alice: String,
    /// bob's (read access only).
    bob: String,
    _app: tokio::task::JoinHandle<()>,
}

impl Fixture {
    fn host(&self, port: u16) -> String {
        format!("{port}-{}.yulius.test", self.key)
    }

    fn origin(&self, port: u16) -> String {
        format!("https://{}", self.host(port))
    }
}

async fn web_session(h: &Harness, user: &str, cookie: &str) -> TestResult {
    h.store
        .insert_web_session(&WebSessionRecord {
            id_hash: h.app.deps.crypto.sha256_hex(cookie.as_bytes()),
            user_id: UserId::parse(user)?,
            csrf_token: format!("csrf-{user}"),
            created_at: h.app.now(),
            expires_at: h.app.now().plus_millis(3_600_000),
            revoked_at: None,
        })
        .await?;
    Ok(())
}

async fn sandbox(h: &Harness, id: &str, slug: &str, port_base: u16, count: u16) -> TestResult {
    h.store
        .insert_sandbox(&SandboxRecord {
            id: SandboxId::parse(id)?,
            slug: slug.into(),
            repo_owner: "acme".into(),
            repo_name: slug.into(),
            github_repo_id: Some(1),
            default_branch: "main".into(),
            profile_json: "{}".into(),
            port_base,
            port_count: count,
            onboarding_state: "not_needed".into(),
            onboarding_task_id: None,
            created_by: UserId::parse("u-alice")?,
            created_at: Timestamp(1),
            removed_at: None,
        })
        .await?;
    Ok(())
}

/// The fixture app on an OS port P; sandbox `s1` owns P..=P+2 (P+2 is a Yulius port), `s2` is
/// another sandbox; alice may write `s1` and `s2`, bob may only read `s1`.
async fn fixture() -> Result<Fixture, Box<dyn std::error::Error>> {
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await?;
    let port = listener.local_addr()?.port();
    let h = harness_with(HarnessOptions {
        configure: Some(Box::new(move |config| config.own_ports.push(port + 2))),
        ..HarnessOptions::default()
    })?;
    let app = fixture_app(port, h.app.instance_tag.clone());
    let task = tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    seed_github_user(&h, "u-alice", "alice").await?;
    seed_github_user(&h, "u-bob", "bob").await?;
    sandbox(&h, "s1", "fixture", port, 3).await?;
    sandbox(&h, "s2", "other", port.saturating_add(10), 3).await?;
    h.github.set_perm("alice", "acme/fixture", perms("write"));
    h.github.set_perm("alice", "acme/other", perms("write"));
    h.github.set_perm("bob", "acme/fixture", perms("read"));
    web_session(&h, "u-alice", "alice-session").await?;
    web_session(&h, "u-bob", "bob-session").await?;
    let key = h.app.preview_key(&SandboxId::parse("s1")?);
    Ok(Fixture {
        h,
        port,
        key,
        alice: "alice-session".into(),
        bob: "bob-session".into(),
        _app: task,
    })
}

struct Reply {
    status: StatusCode,
    headers: HeaderMap,
    body: String,
}

impl Reply {
    fn header(&self, name: &str) -> Option<String> {
        self.headers
            .get(name)
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned)
    }

    fn all(&self, name: &str) -> Vec<String> {
        self.headers
            .get_all(name)
            .iter()
            .filter_map(|v| v.to_str().ok())
            .map(str::to_owned)
            .collect()
    }
}

async fn send(
    h: &Harness,
    method: Method,
    host: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: Body,
) -> Result<Reply, Box<dyn std::error::Error>> {
    let mut request = Request::builder()
        .method(method)
        .uri(path)
        .header(header::HOST, host);
    for (name, value) in headers {
        request = request.header(*name, *value);
    }
    let response = h.router.clone().oneshot(request.body(body)?).await?;
    let status = response.status();
    let headers = response.headers().clone();
    let bytes = response.into_body().collect().await?.to_bytes();
    Ok(Reply {
        status,
        headers,
        body: String::from_utf8_lossy(&bytes).into_owned(),
    })
}

async fn fetch(
    h: &Harness,
    host: &str,
    path: &str,
    headers: &[(&str, &str)],
) -> Result<Reply, Box<dyn std::error::Error>> {
    send(h, Method::GET, host, path, headers, Body::empty()).await
}

fn session_cookie(value: &str) -> String {
    format!("__Host-yulius_session={value}")
}

/// Runs the handshake as a browser would and returns the grant cookie (`name=value`).
async fn grant(
    f: &Fixture,
    port: u16,
    session: &str,
) -> Result<String, Box<dyn std::error::Error>> {
    let cookie = session_cookie(session);
    let authorize = fetch(
        &f.h,
        "yulius.test",
        &format!("/api/previews/authorize?host={}&to=/", f.host(port)),
        &[
            ("cookie", &cookie),
            ("sec-fetch-site", "same-site"),
            ("sec-fetch-mode", "navigate"),
        ],
    )
    .await?;
    assert_eq!(
        authorize.status,
        StatusCode::SEE_OTHER,
        "{}",
        authorize.body
    );
    let location = authorize.header("location").ok_or("location")?;
    let path = location
        .strip_prefix(&f.origin(port))
        .ok_or_else(|| format!("handshake on another origin: {location}"))?;
    let handshake = fetch(&f.h, &f.host(port), path, &[]).await?;
    assert_eq!(
        handshake.status,
        StatusCode::SEE_OTHER,
        "{}",
        handshake.body
    );
    let set = handshake.header("set-cookie").ok_or("no grant cookie")?;
    Ok(set.split(';').next().ok_or("cookie")?.to_owned())
}

#[tokio::test]
async fn a_preview_opens_after_a_single_use_handshake() -> TestResult {
    let f = fixture().await?;
    let host = f.host(f.port);
    // Without a grant: a navigation goes through Yulius for one; anything else is refused.
    let nav = fetch(
        &f.h,
        &host,
        "/page?q=1",
        &[("sec-fetch-mode", "navigate"), ("sec-fetch-dest", "iframe")],
    )
    .await?;
    assert_eq!(nav.status, StatusCode::FOUND);
    assert_eq!(
        nav.header("location").as_deref(),
        Some(format!("{YULIUS}/api/previews/authorize?host={host}&to=/page%3Fq%3D1").as_str())
    );
    let xhr = fetch(
        &f.h,
        &host,
        "/headers",
        &[
            ("sec-fetch-mode", "cors"),
            ("sec-fetch-site", "same-origin"),
        ],
    )
    .await?;
    assert_eq!(xhr.status, StatusCode::UNAUTHORIZED);
    // Not signed in to Yulius: the preview's own refusal page (it can show inside the panel).
    let anonymous = fetch(
        &f.h,
        "yulius.test",
        &format!("/api/previews/authorize?host={host}&to=/"),
        &[],
    )
    .await?;
    assert_eq!(anonymous.status, StatusCode::SEE_OTHER);
    assert_eq!(
        anonymous.header("location").as_deref(),
        Some(format!("https://{host}/__yulius/denied?why=signin").as_str())
    );

    // Step 1: Yulius issues a single-use token for this host only.
    let cookie = session_cookie(&f.alice);
    let authorize = fetch(
        &f.h,
        "yulius.test",
        &format!("/api/previews/authorize?host={host}&to=/page?q=1"),
        &[
            ("cookie", &cookie),
            ("sec-fetch-site", "same-site"),
            ("sec-fetch-mode", "navigate"),
        ],
    )
    .await?;
    assert_eq!(authorize.status, StatusCode::SEE_OTHER);
    let location = authorize.header("location").ok_or("location")?;
    assert!(
        location.starts_with(&format!("https://{host}/__yulius/handshake?t=")),
        "{location}"
    );
    let path = location
        .strip_prefix(&format!("https://{host}"))
        .ok_or("path")?;
    // The token is refused on another preview host…
    let elsewhere = fetch(&f.h, &f.host(f.port + 1), path, &[]).await?;
    assert_eq!(elsewhere.status, StatusCode::FORBIDDEN);
    // …and was spent there: tokens work once.
    let spent = fetch(&f.h, &host, path, &[]).await?;
    assert_eq!(spent.status, StatusCode::FORBIDDEN);

    // Step 2: a fresh token becomes a host-only, HttpOnly, Secure grant cookie.
    let authorize = fetch(
        &f.h,
        "yulius.test",
        &format!("/api/previews/authorize?host={host}&to=/page?q=1"),
        &[("cookie", &cookie)],
    )
    .await?;
    let path = authorize
        .header("location")
        .ok_or("location")?
        .strip_prefix(&format!("https://{host}"))
        .ok_or("path")?
        .to_owned();
    let handshake = fetch(&f.h, &host, &path, &[]).await?;
    assert_eq!(handshake.status, StatusCode::SEE_OTHER);
    assert_eq!(
        handshake.header("location").as_deref(),
        Some("/__yulius/landing?to=/page%3Fq%3D1")
    );
    let set = handshake.header("set-cookie").ok_or("set-cookie")?;
    assert!(set.starts_with("__Host-yulius_preview="), "{set}");
    for attribute in ["Path=/", "HttpOnly", "Secure", "SameSite=Lax"] {
        assert!(set.contains(attribute), "{attribute} in {set}");
    }
    assert!(!set.to_ascii_lowercase().contains("domain="), "{set}");
    let grant = set.split(';').next().ok_or("grant")?.to_owned();
    // Step 3: the landing checks the cookie stuck, then the page loads from the dev server.
    let landing = fetch(
        &f.h,
        &host,
        "/__yulius/landing?to=/page%3Fq%3D1",
        &[("cookie", &grant)],
    )
    .await?;
    assert_eq!(landing.status, StatusCode::SEE_OTHER);
    assert_eq!(landing.header("location").as_deref(), Some("/page?q=1"));
    let lost = fetch(&f.h, &host, "/__yulius/landing?to=/", &[]).await?;
    assert_eq!(
        lost.status,
        StatusCode::FORBIDDEN,
        "a blocked cookie is reported, not looped"
    );
    let page = fetch(&f.h, &host, "/", &[("cookie", &grant)]).await?;
    assert_eq!(page.status, StatusCode::OK);
    assert!(page.body.contains("Fixture app"), "{}", page.body);
    // Root-relative assets resolve on the preview origin itself.
    let css = fetch(
        &f.h,
        &host,
        "/app.css",
        &[("cookie", &grant), ("sec-fetch-site", "same-origin")],
    )
    .await?;
    assert_eq!(css.body, "h1 { color: red; }");
    // The grant is for this preview only.
    let other_port = fetch(
        &f.h,
        &f.host(f.port + 1),
        "/",
        &[("cookie", &grant), ("sec-fetch-mode", "navigate")],
    )
    .await?;
    assert_eq!(
        other_port.status,
        StatusCode::FOUND,
        "another port needs its own grant"
    );
    Ok(())
}

#[tokio::test]
async fn responses_are_rewritten_to_work_inside_the_yulius_page() -> TestResult {
    let f = fixture().await?;
    let host = f.host(f.port);
    let grant = grant(&f, f.port, &f.alice).await?;
    let page = fetch(
        &f.h,
        &host,
        "/",
        &[
            ("cookie", &grant),
            ("sec-fetch-mode", "navigate"),
            ("sec-fetch-dest", "iframe"),
        ],
    )
    .await?;
    assert_eq!(page.status, StatusCode::OK);
    assert_eq!(
        page.header("x-frame-options"),
        None,
        "the app's X-Frame-Options is removed"
    );
    assert_eq!(page.header("strict-transport-security"), None);
    let csp = page.all("content-security-policy");
    assert_eq!(
        csp,
        [
            format!("default-src 'self'; connect-src wss://{host}"),
            format!("frame-ancestors 'self' {YULIUS}"),
        ],
        "frame-ancestors is Yulius's; loopback sources point at the preview"
    );
    assert_eq!(
        page.all("set-cookie"),
        ["sid=app; Path=/; HttpOnly; Secure"]
    );
    assert_eq!(
        page.header("x-yulius").as_deref(),
        Some(f.h.app.instance_tag.as_str())
    );
    // Documents get the bridge script (address, title, back and forward for the panel).
    assert!(
        page.body
            .ends_with("<script src=\"/__yulius/bridge.js\" async></script>\n"),
        "{}",
        page.body
    );
    assert_eq!(page.header("content-length"), None);
    let bridge = fetch(&f.h, &host, "/__yulius/bridge.js", &[]).await?;
    assert!(
        bridge
            .body
            .contains(&format!("const YULIUS = \"{YULIUS}\"")),
        "{}",
        bridge.body
    );
    assert!(
        bridge
            .header("content-type")
            .is_some_and(|v| v.starts_with("text/javascript"))
    );
    // Subresources are not touched.
    let css = fetch(
        &f.h,
        &host,
        "/app.css",
        &[("cookie", &grant), ("sec-fetch-dest", "style")],
    )
    .await?;
    assert!(!css.body.contains("bridge"));
    // Redirects to localhost become the preview origins; other sites are left alone.
    let redirect = fetch(&f.h, &host, "/redirect", &[("cookie", &grant)]).await?;
    assert_eq!(redirect.status, StatusCode::FOUND);
    assert_eq!(
        redirect.header("location").as_deref(),
        Some(format!("https://{host}/landed?x=1").as_str())
    );
    let sibling = fetch(&f.h, &host, "/sibling", &[("cookie", &grant)]).await?;
    assert_eq!(
        sibling.header("location").as_deref(),
        Some(format!("{}/x", f.origin(f.port + 1)).as_str())
    );
    let away = fetch(&f.h, &host, "/away", &[("cookie", &grant)]).await?;
    assert_eq!(
        away.header("location").as_deref(),
        Some("https://example.com/x")
    );
    let cors = fetch(&f.h, &host, "/cors", &[("cookie", &grant)]).await?;
    assert_eq!(
        cors.header("access-control-allow-origin").as_deref(),
        Some(f.origin(f.port).as_str())
    );
    Ok(())
}

#[tokio::test]
async fn the_dev_server_sees_a_plain_localhost_request() -> TestResult {
    let f = fixture().await?;
    let host = f.host(f.port);
    let grant = grant(&f, f.port, &f.alice).await?;
    let origin = f.origin(f.port);
    let referer = format!("{origin}/page");
    let cookies = format!("theme=dark; {grant}; __Host-yulius_session=leak; yulius_session=leak");
    let echo = fetch(
        &f.h,
        &host,
        "/headers",
        &[
            ("cookie", &cookies),
            ("origin", &origin),
            ("referer", &referer),
            ("sec-fetch-site", "same-origin"),
            ("x-forwarded-for", "203.0.113.9"),
            ("x-forwarded-host", "attacker.example"),
        ],
    )
    .await?;
    assert_eq!(echo.status, StatusCode::OK, "{}", echo.body);
    let seen: Value = serde_json::from_str(&echo.body)?;
    let local = format!("localhost:{}", f.port);
    assert_eq!(seen["host"], json!(local));
    assert_eq!(seen["origin"], json!(format!("http://{local}")));
    assert_eq!(seen["referer"], json!(format!("http://{local}/page")));
    assert_eq!(
        seen["cookie"],
        json!("theme=dark"),
        "Yulius's cookies never reach the app"
    );
    assert_eq!(seen["x-forwarded-for"], Value::Null);
    assert_eq!(seen["x-forwarded-host"], Value::Null);
    // Large bodies stream through.
    let body = vec![b'x'; 3 << 20];
    let upload = send(
        &f.h,
        Method::POST,
        &host,
        "/upload",
        &[
            ("cookie", &grant),
            ("origin", &origin),
            ("sec-fetch-site", "same-origin"),
        ],
        Body::from(body),
    )
    .await?;
    assert_eq!(upload.body, (3 << 20).to_string());
    Ok(())
}

#[tokio::test]
async fn websockets_pass_through_for_the_preview_only() -> TestResult {
    use tokio_tungstenite::tungstenite::client::IntoClientRequest as _;
    let f = fixture().await?;
    let host = f.host(f.port);
    let grant = grant(&f, f.port, &f.alice).await?;
    let served = serve(f.h.router.clone()).await?;
    let connect = |origin: &str, cookie: &str| {
        let mut request = format!("ws://{}/ws", served.addr).into_client_request();
        if let Ok(request) = request.as_mut() {
            let headers = request.headers_mut();
            for (name, value) in [
                ("host", host.as_str()),
                ("origin", origin),
                ("cookie", cookie),
            ] {
                if let Ok(value) = value.parse() {
                    headers.insert(name, value);
                }
            }
        }
        async move { tokio_tungstenite::connect_async(request?).await }
    };
    let (mut socket, _) = connect(&f.origin(f.port), &grant).await?;
    socket.send(Message::text("hot reload")).await?;
    let reply = tokio::time::timeout(Duration::from_secs(5), socket.next())
        .await?
        .ok_or("closed")??;
    assert_eq!(reply.into_text()?.as_str(), "echo: hot reload");
    // Another site, or no grant, cannot open it.
    let foreign = connect("https://evil.example", &grant).await;
    assert_eq!(
        foreign.err().as_ref().and_then(support::ws_refusal_status),
        Some(403)
    );
    let sslip_neighbour = connect("https://6-6-6-6.yulius.test", &grant).await;
    assert_eq!(
        sslip_neighbour
            .err()
            .as_ref()
            .and_then(support::ws_refusal_status),
        Some(403)
    );
    let anonymous = connect(&f.origin(f.port), "").await;
    assert_eq!(
        anonymous
            .err()
            .as_ref()
            .and_then(support::ws_refusal_status),
        Some(401)
    );
    Ok(())
}

#[tokio::test]
async fn other_users_sandboxes_ports_and_sites_are_refused() -> TestResult {
    let f = fixture().await?;
    let host = f.host(f.port);
    // bob only reads s1: no preview for him.
    let bob = session_cookie(&f.bob);
    let refused = fetch(
        &f.h,
        "yulius.test",
        &format!("/api/previews/authorize?host={host}&to=/"),
        &[("cookie", &bob)],
    )
    .await?;
    assert_eq!(refused.status, StatusCode::SEE_OTHER);
    assert_eq!(
        refused.header("location").as_deref(),
        Some(format!("https://{host}/__yulius/denied?why=access").as_str())
    );
    let info = fetch(
        &f.h,
        "yulius.test",
        "/api/sandboxes/s1/previews",
        &[("cookie", &bob)],
    )
    .await?;
    assert_eq!(info.status, StatusCode::FORBIDDEN);
    // A grant of sandbox s1 does not open sandbox s2's preview.
    let grant = grant(&f, f.port, &f.alice).await?;
    let s2 = h_key(&f, "s2")?;
    let s2_host = format!("{}-{s2}.yulius.test", f.port + 10);
    let cross = fetch(
        &f.h,
        &s2_host,
        "/",
        &[
            ("cookie", &grant),
            ("sec-fetch-mode", "cors"),
            ("sec-fetch-site", "same-origin"),
        ],
    )
    .await?;
    assert_eq!(cross.status, StatusCode::UNAUTHORIZED);
    // Ports outside the sandbox's range, Yulius's own ports and unknown keys are no previews.
    let outside = fetch(&f.h, &f.host(f.port + 5), "/", &[]).await?;
    assert_eq!(outside.status, StatusCode::FORBIDDEN);
    let own = fetch(&f.h, &f.host(f.port + 2), "/", &[]).await?;
    assert_eq!(own.status, StatusCode::FORBIDDEN);
    assert!(own.body.contains("belongs to Yulius"), "{}", own.body);
    let unknown = fetch(
        &f.h,
        &format!("{}-ffffffffffff.yulius.test", f.port),
        "/",
        &[],
    )
    .await?;
    assert_eq!(unknown.status, StatusCode::NOT_FOUND);
    // Cross-site callers cannot use the grant: a form post from another site, a subresource of
    // another *.yulius.test host (same-site, like every other *.sslip.io host).
    let origin = f.origin(f.port);
    let post = send(
        &f.h,
        Method::POST,
        &host,
        "/upload",
        &[
            ("cookie", &grant),
            ("origin", "https://evil.example"),
            ("sec-fetch-site", "cross-site"),
            ("sec-fetch-mode", "navigate"),
        ],
        Body::from("x"),
    )
    .await?;
    assert_eq!(post.status, StatusCode::FORBIDDEN);
    let script = fetch(
        &f.h,
        &host,
        "/app.css",
        &[
            ("cookie", &grant),
            ("sec-fetch-site", "same-site"),
            ("sec-fetch-mode", "no-cors"),
        ],
    )
    .await?;
    assert_eq!(script.status, StatusCode::FORBIDDEN);
    let null = send(
        &f.h,
        Method::POST,
        &host,
        "/upload",
        &[("cookie", &grant), ("origin", "null")],
        Body::from("x"),
    )
    .await?;
    assert_eq!(null.status, StatusCode::FORBIDDEN);
    // The same sandbox's other previews and same-origin requests may.
    let sibling = send(
        &f.h,
        Method::POST,
        &host,
        "/upload",
        &[
            ("cookie", &grant),
            ("origin", &f.origin(f.port + 1)),
            ("sec-fetch-site", "same-site"),
            ("sec-fetch-mode", "cors"),
        ],
        Body::from("xy"),
    )
    .await?;
    assert_eq!(sibling.body, "2");
    let same = send(
        &f.h,
        Method::POST,
        &host,
        "/upload",
        &[
            ("cookie", &grant),
            ("origin", &origin),
            ("sec-fetch-site", "same-origin"),
        ],
        Body::from("xyz"),
    )
    .await?;
    assert_eq!(same.body, "3");
    // A navigation from anywhere is fine (the grant cookie decides).
    let link = fetch(
        &f.h,
        &host,
        "/",
        &[
            ("cookie", &grant),
            ("sec-fetch-site", "cross-site"),
            ("sec-fetch-mode", "navigate"),
        ],
    )
    .await?;
    assert_eq!(link.status, StatusCode::OK);
    // Signing out of Yulius ends preview access at the next check.
    f.h.store
        .revoke_web_session(
            &f.h.app.deps.crypto.sha256_hex(f.alice.as_bytes()),
            f.h.app.now(),
        )
        .await?;
    f.h.clock.0.fetch_add(31_000, Ordering::SeqCst);
    let after = fetch(
        &f.h,
        &host,
        "/",
        &[("cookie", &grant), ("sec-fetch-mode", "navigate")],
    )
    .await?;
    assert_eq!(
        after.status,
        StatusCode::FOUND,
        "back through Yulius, which now refuses"
    );
    assert_eq!(f.h.app.preview_grant_count(), 0);
    Ok(())
}

fn h_key(f: &Fixture, sandbox: &str) -> Result<String, Box<dyn std::error::Error>> {
    Ok(f.h.app.preview_key(&SandboxId::parse(sandbox)?))
}

#[tokio::test]
async fn a_preview_origin_cannot_use_the_yulius_session() -> TestResult {
    let f = fixture().await?;
    let cookie = session_cookie(&f.alice);
    let preview = f.origin(f.port);
    // A page on the preview origin calling the API with the user's cookie (fetch with
    // credentials: the cookie is sent, the preview is same-site): refused, whatever it asks.
    for (method, path) in [
        (Method::GET, "/api/me"),
        (Method::GET, "/api/sandboxes"),
        (Method::POST, "/api/auth/logout"),
    ] {
        let reply = send(
            &f.h,
            method.clone(),
            "yulius.test",
            path,
            &[
                ("cookie", &cookie),
                ("origin", &preview),
                ("sec-fetch-site", "same-site"),
                ("sec-fetch-mode", "cors"),
                ("x-yulius-csrf", "csrf-u-alice"),
            ],
            Body::empty(),
        )
        .await?;
        assert_eq!(
            reply.status,
            StatusCode::FORBIDDEN,
            "{method} {path}: {}",
            reply.body
        );
        // Even a browser without fetch metadata announces the origin.
        let reply = send(
            &f.h,
            method.clone(),
            "yulius.test",
            path,
            &[
                ("cookie", &cookie),
                ("origin", &preview),
                ("x-yulius-csrf", "csrf-u-alice"),
            ],
            Body::empty(),
        )
        .await?;
        assert_eq!(
            reply.status,
            StatusCode::FORBIDDEN,
            "{method} {path}: {}",
            reply.body
        );
    }
    // An embedded image or script of the API (no Origin, same-site): refused too.
    let img = fetch(
        &f.h,
        "yulius.test",
        "/api/me",
        &[
            ("cookie", &cookie),
            ("sec-fetch-site", "same-site"),
            ("sec-fetch-mode", "no-cors"),
        ],
    )
    .await?;
    assert_eq!(img.status, StatusCode::FORBIDDEN);
    // Its WebSockets: refused by origin.
    let served = serve(f.h.router.clone()).await?;
    for path in ["/api/events", "/api/sandboxes/s1/browser"] {
        use tokio_tungstenite::tungstenite::client::IntoClientRequest as _;
        let mut request = format!("ws://{}{path}", served.addr).into_client_request()?;
        request.headers_mut().insert("host", "yulius.test".parse()?);
        request.headers_mut().insert("cookie", cookie.parse()?);
        request.headers_mut().insert("origin", preview.parse()?);
        let refused = tokio_tungstenite::connect_async(request).await;
        assert_eq!(
            refused.err().as_ref().and_then(support::ws_refusal_status),
            Some(403),
            "{path}"
        );
    }
    // Yulius's own pages, same origin, keep working; the session cookie is `__Host-` (no
    // sibling host can plant or shadow it) and host-only.
    let me = fetch(
        &f.h,
        "yulius.test",
        "/api/me",
        &[
            ("cookie", &cookie),
            ("sec-fetch-site", "same-origin"),
            ("sec-fetch-mode", "cors"),
        ],
    )
    .await?;
    assert_eq!(me.status, StatusCode::OK);
    let me = fetch(
        &f.h,
        "yulius.test",
        "/api/me",
        &[("cookie", &cookie), ("origin", YULIUS)],
    )
    .await?;
    assert_eq!(me.status, StatusCode::OK);
    let creds = support::setup_superadmin_creds(&f.h).await?;
    assert!(creds.cookie.is_some());
    let login = send(
        &f.h,
        Method::POST,
        "yulius.test",
        "/api/auth/password",
        &[("content-type", "application/json")],
        Body::from(r#"{"password":"a very long secret"}"#),
    )
    .await?;
    let set = login.header("set-cookie").ok_or("set-cookie")?;
    assert!(set.starts_with("__Host-yulius_session="), "{set}");
    assert!(
        set.contains("Secure")
            && set.contains("Path=/")
            && !set.to_ascii_lowercase().contains("domain="),
        "{set}"
    );
    // A plain-named cookie (planted by a sibling host) is not a session.
    let planted = fetch(
        &f.h,
        "yulius.test",
        "/api/me",
        &[("cookie", &format!("yulius_session={}", f.alice))],
    )
    .await?;
    assert_eq!(planted.status, StatusCode::UNAUTHORIZED);
    // And the preview proxy never forwards or sets Yulius cookies (unit tests in the proxy).
    Ok(())
}

#[tokio::test]
async fn yulius_is_never_shown_inside_itself() -> TestResult {
    let f = fixture().await?;
    let host = f.host(f.port);
    let grant = grant(&f, f.port, &f.alice).await?;
    // A dev server that turns out to be this Yulius (a port forward to it) is refused.
    let looped = fetch(&f.h, &host, "/yulius", &[("cookie", &grant)]).await?;
    assert_eq!(looped.status, StatusCode::LOOP_DETECTED);
    assert!(!looped.body.contains("the Yulius UI"));
    // Yulius's pages refuse every frame, its own included.
    let ui = fetch(&f.h, "yulius.test", "/", &[]).await?;
    assert_eq!(ui.header("x-frame-options").as_deref(), Some("DENY"));
    assert_eq!(
        ui.header("content-security-policy").as_deref(),
        Some("frame-ancestors 'none'")
    );
    // The on-demand TLS `ask` endpoint approves only existing preview hosts.
    for (domain, status) in [
        (host.clone(), StatusCode::OK),
        (f.host(f.port + 1), StatusCode::OK),
        (f.host(f.port + 2), StatusCode::NOT_FOUND),
        (f.host(f.port + 5), StatusCode::NOT_FOUND),
        (
            format!("{}-ffffffffffff.yulius.test", f.port),
            StatusCode::NOT_FOUND,
        ),
        ("yulius.test".to_owned(), StatusCode::NOT_FOUND),
        ("example.com".to_owned(), StatusCode::NOT_FOUND),
    ] {
        let request = Request::builder()
            .uri(format!("/internal/preview-tls?domain={domain}"))
            .body(Body::empty())?;
        let response = f.h.internal.clone().oneshot(request).await?;
        assert_eq!(response.status(), status, "{domain}");
    }
    // The previews API and the sandbox Chrome's welcome page never offer Yulius's ports.
    let own = tokio::net::TcpListener::bind(("127.0.0.1", f.port + 2)).await;
    let cookie = session_cookie(&f.alice);
    let info = fetch(
        &f.h,
        "yulius.test",
        "/api/sandboxes/s1/previews",
        &[("cookie", &cookie)],
    )
    .await?;
    assert_eq!(info.status, StatusCode::OK, "{}", info.body);
    let info: Value = serde_json::from_str(&info.body)?;
    assert_eq!(info["enabled"], json!(true));
    assert_eq!(
        info["origin_template"],
        json!(format!("https://{{port}}-{}.yulius.test", f.key))
    );
    assert_eq!(info["first_port"], json!(f.port));
    assert_eq!(info["excluded"], json!([f.port + 2]));
    let listening = info["listening"].as_array().ok_or("listening")?;
    assert!(
        listening.contains(&json!(f.port)),
        "the app listens: {info}"
    );
    assert!(
        !listening.contains(&json!(f.port + 2)),
        "Yulius's port is skipped: {info}"
    );
    let welcome = Request::builder()
        .uri("/browser/welcome/ports?sandbox=s1")
        .body(Body::empty())?;
    let response = f.h.internal.clone().oneshot(welcome).await?;
    let body = String::from_utf8(response.into_body().collect().await?.to_bytes().to_vec())?;
    assert!(body.contains(&format!("localhost:{}", f.port)), "{body}");
    if own.is_ok() {
        assert!(
            !body.contains(&format!("localhost:{}", f.port + 2)),
            "{body}"
        );
    }
    Ok(())
}
