//! The egress proxy end to end over real sockets: an agent-side HTTPS client behind the proxy,
//! a TLS upstream that echoes what it received (the worst case: it reflects the credential),
//! interception and injection for the bound host, opaque tunnels for others, refusals.

use std::collections::HashMap;
use std::convert::Infallible;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use http_body_util::{BodyExt, Full};
use hyper::body::{Bytes, Incoming};
use hyper::http::{Request, Response};
use hyper_util::rt::TokioIo;
use rcgen::{
    BasicConstraints, CertificateParams, CertifiedIssuer, DnType, IsCa, KeyPair, KeyUsagePurpose,
};
use yulius_app::ports::egress::{
    EgressCaller, EgressClient, EgressDecision, EgressPolicy, EgressRequest,
};
use yulius_domain::ids::{SandboxId, TaskId};
use yulius_domain::vault::encode::needles;
use yulius_domain::vault::redact::PlainMatcher;
use yulius_egress::{CertificateAuthority, EgressConfig, EgressProxy, generate_ca_material};

type TestResult = Result<(), Box<dyn std::error::Error>>;

const VALUE: &str = "sk_test_51NzQ9yulius_value_8f3a";
const BOUND: &str = "api.test.example";
const OTHER: &str = "other.test.example";

struct Policy {
    matcher: Arc<PlainMatcher>,
}

#[async_trait]
impl EgressPolicy for Policy {
    async fn egress_client(&self, token: &str) -> Option<EgressClient> {
        // Two runs of sandbox s1 and one of sandbox s2.
        let (run, sandbox) = match token {
            "good-token" => ("a1", "s1"),
            "sibling-token" => ("a2", "s1"),
            "other-sandbox-token" => ("b1", "s2"),
            _ => return None,
        };
        Some(EgressClient {
            caller: EgressCaller::Agent {
                attempt_id: run.into(),
                task_id: TaskId::parse("t1").ok()?,
                task_number: 1,
            },
            sandbox_id: SandboxId::parse(sandbox).ok()?,
        })
    }

    async fn egress_intercepts(&self, _: &EgressClient, host: &str, port: u16) -> bool {
        host == BOUND && port == 443
    }

    async fn egress_apply(&self, _: &EgressClient, request: EgressRequest) -> EgressDecision {
        if request.path_and_query.contains("refuse") {
            return EgressDecision::Refuse {
                status: 409,
                message: "refused by policy".into(),
            };
        }
        let mut headers = request.headers;
        headers.retain(|(name, _)| name != "authorization");
        headers.push((
            "authorization".into(),
            format!("Bearer {VALUE}").into_bytes(),
        ));
        EgressDecision::Forward {
            path_and_query: request
                .path_and_query
                .replace("yulius-secret-API_KEY", VALUE),
            headers,
            used: vec!["API_KEY".into()],
        }
    }

    async fn egress_redactor(&self, _: &EgressClient) -> Arc<PlainMatcher> {
        self.matcher.clone()
    }
}

/// A TLS server for both test hosts that echoes the request (headers, path, body) in its body
/// and the authorization header in a response header.
async fn upstream(
    issuer: &CertifiedIssuer<'static, KeyPair>,
    seen: Arc<Mutex<Vec<String>>>,
) -> Result<SocketAddr, Box<dyn std::error::Error>> {
    let key = KeyPair::generate()?;
    let cert = CertificateParams::new(vec![BOUND.into(), OTHER.into()])?.signed_by(&key, issuer)?;
    let config = rustls::ServerConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()?
    .with_no_client_auth()
    .with_single_cert(
        vec![cert.der().clone()],
        rustls::pki_types::PrivateKeyDer::Pkcs8(key.serialize_der().into()),
    )?;
    let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(config));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let addr = listener.local_addr()?;
    tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else {
                continue;
            };
            let acceptor = acceptor.clone();
            let seen = seen.clone();
            tokio::spawn(async move {
                let Ok(tls) = acceptor.accept(stream).await else {
                    return;
                };
                let service = hyper::service::service_fn(move |request: Request<Incoming>| {
                    let seen = seen.clone();
                    async move {
                        let auth = request
                            .headers()
                            .get("authorization")
                            .and_then(|v| v.to_str().ok())
                            .unwrap_or("-")
                            .to_owned();
                        let host = request
                            .headers()
                            .get("host")
                            .and_then(|v| v.to_str().ok())
                            .unwrap_or("-")
                            .to_owned();
                        let path = request
                            .uri()
                            .path_and_query()
                            .map_or("/", |p| p.as_str())
                            .to_owned();
                        let body = request
                            .into_body()
                            .collect()
                            .await
                            .map(|b| b.to_bytes())
                            .unwrap_or_default();
                        let line = format!(
                            "host={host} auth={auth} path={path} body={}",
                            String::from_utf8_lossy(&body)
                        );
                        if let Ok(mut seen) = seen.lock() {
                            seen.push(line.clone());
                        }
                        let mut response = Response::new(Full::new(Bytes::from(line)));
                        if let Ok(value) = auth.parse() {
                            response.headers_mut().insert("x-echo-auth", value);
                        }
                        Ok::<_, Infallible>(response)
                    }
                });
                let _ = hyper::server::conn::http1::Builder::new()
                    .serve_connection(TokioIo::new(tls), service)
                    .await;
            });
        }
    });
    Ok(addr)
}

struct Setup {
    proxy: SocketAddr,
    egress_ca: String,
    test_ca: String,
    seen: Arc<Mutex<Vec<String>>>,
}

async fn setup() -> Result<Setup, Box<dyn std::error::Error>> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let mut ca = CertificateParams::default();
    ca.distinguished_name
        .push(DnType::CommonName, "test upstream CA");
    ca.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    ca.key_usages = vec![
        KeyUsagePurpose::KeyCertSign,
        KeyUsagePurpose::DigitalSignature,
    ];
    let issuer = CertifiedIssuer::self_signed(ca, KeyPair::generate()?)?;
    let test_ca = issuer.pem();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let upstream = upstream(&issuer, seen.clone()).await?;
    let authority = CertificateAuthority::from_material(&generate_ca_material()?)?;
    let egress_ca = authority.cert_pem().to_owned();
    let policy = Arc::new(Policy {
        matcher: Arc::new(PlainMatcher::new(vec![("API_KEY".into(), needles(VALUE))])),
    });
    let proxy = Arc::new(
        EgressProxy::new(
            policy,
            authority,
            EgressConfig {
                resolve: HashMap::from([(BOUND.into(), upstream), (OTHER.into(), upstream)]),
                extra_roots_pem: vec![test_ca.clone()],
                max_connections: 16,
                max_per_sandbox: 8,
            },
        )
        .map_err(|e| e.to_string())?,
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let addr = listener.local_addr()?;
    tokio::spawn(yulius_egress::serve(listener, proxy));
    Ok(Setup {
        proxy: addr,
        egress_ca,
        test_ca,
        seen,
    })
}

/// The agent's HTTPS client: the proxy from `HTTPS_PROXY`, the run's CA bundle as its roots.
fn agent(setup: &Setup, token: &str) -> Result<reqwest::Client, Box<dyn std::error::Error>> {
    Ok(reqwest::Client::builder()
        .proxy(reqwest::Proxy::all(format!(
            "http://yulius:{token}@{}",
            setup.proxy
        ))?)
        .tls_certs_only([
            reqwest::Certificate::from_pem(setup.egress_ca.as_bytes())?,
            reqwest::Certificate::from_pem(setup.test_ca.as_bytes())?,
        ])
        .build()?)
}

#[tokio::test]
async fn bound_hosts_get_the_credential_and_the_agent_never_sees_it() -> TestResult {
    let setup = setup().await?;
    let client = agent(&setup, "good-token")?;
    let response = client
        .post(format!(
            "https://{BOUND}/v1/charges?key=yulius-secret-API_KEY"
        ))
        .body("amount=100")
        .send()
        .await?;
    assert_eq!(response.status(), 200);
    let echoed_header = response
        .headers()
        .get("x-echo-auth")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_owned();
    let headers = format!("{:?}", response.headers());
    let body = response.text().await?;
    // The upstream received the real credential (header and placeholder in the query)…
    let seen = setup.seen.lock().map_err(|_| "lock")?.clone();
    assert_eq!(seen.len(), 1);
    assert!(
        seen[0].contains(&format!("auth=Bearer {VALUE}")),
        "{}",
        seen[0]
    );
    assert!(
        seen[0].contains(&format!("path=/v1/charges?key={VALUE}")),
        "{}",
        seen[0]
    );
    assert!(seen[0].contains("body=amount=100"));
    // …and reflected it, but nothing the agent can read contains it.
    for readable in [&body, &headers, &echoed_header] {
        assert!(!readable.contains(VALUE), "leaked: {readable}");
    }
    assert!(body.contains("auth=Bearer [secret:API_KEY]"), "{body}");
    assert_eq!(echoed_header, "Bearer [secret:API_KEY]");
    Ok(())
}

#[tokio::test]
async fn other_hosts_are_opaque_tunnels() -> TestResult {
    let setup = setup().await?;
    let client = agent(&setup, "good-token")?;
    let body = client
        .get(format!("https://{OTHER}/page"))
        .header("authorization", "Bearer agent-own-token")
        .send()
        .await?
        .text()
        .await?;
    // The request reached the upstream unchanged, end to end encrypted with the upstream's own
    // certificate (the agent trusts only it and the egress CA).
    assert!(body.contains("auth=Bearer agent-own-token"), "{body}");
    assert!(body.contains(&format!("host={OTHER}")), "{body}");
    Ok(())
}

#[tokio::test]
async fn refusals() -> TestResult {
    let setup = setup().await?;
    // No or a wrong task token: 407, nothing is forwarded.
    for client in [
        reqwest::Client::builder()
            .proxy(reqwest::Proxy::all(format!("http://{}", setup.proxy))?)
            .build()?,
        agent(&setup, "wrong")?,
    ] {
        let result = client.get(format!("https://{BOUND}/x")).send().await;
        assert!(result.is_err(), "the CONNECT is refused");
        let plain = client.get(format!("http://{BOUND}/x")).send().await?;
        assert_eq!(plain.status(), 407);
    }
    // The policy refuses: its status and text reach the agent.
    let client = agent(&setup, "good-token")?;
    let refused = client.get(format!("https://{BOUND}/refuse")).send().await?;
    assert_eq!(refused.status(), 409);
    assert!(refused.text().await?.contains("refused by policy"));
    // Local destinations are never reachable through the proxy.
    let local = client.get("http://127.0.0.1:9/x").send().await?;
    assert_eq!(local.status(), 403);
    let metadata = client.get("http://169.254.169.254/latest").send().await?;
    assert_eq!(metadata.status(), 403);
    assert!(setup.seen.lock().map_err(|_| "lock")?.is_empty());
    Ok(())
}

/// A plain-HTTP request to a bound host is upgraded to HTTPS: the secret never travels in
/// clear.
#[tokio::test]
async fn plain_requests_to_bound_hosts_go_over_https() -> TestResult {
    let setup = setup().await?;
    let client = agent(&setup, "good-token")?;
    let body = client
        .get(format!("http://{BOUND}/plain"))
        .send()
        .await?
        .text()
        .await?;
    assert!(body.contains("auth=Bearer [secret:API_KEY]"), "{body}");
    let seen = setup.seen.lock().map_err(|_| "lock")?.clone();
    assert!(seen[0].contains(&format!("auth=Bearer {VALUE}")));
    Ok(())
}

/// Opens a `CONNECT` tunnel to the opaque host with `token` and keeps it: the stream and the
/// proxy's status line, followed by a refusal's text.
async fn tunnel(
    setup: &Setup,
    token: &str,
) -> Result<(tokio::net::TcpStream, String), Box<dyn std::error::Error>> {
    use base64::Engine as _;
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
    let mut stream = tokio::net::TcpStream::connect(setup.proxy).await?;
    let auth = base64::engine::general_purpose::STANDARD.encode(format!("yulius:{token}"));
    stream
        .write_all(
            format!(
                "CONNECT {OTHER}:443 HTTP/1.1\r\nHost: {OTHER}:443\r\nProxy-Authorization: Basic {auth}\r\n\r\n"
            )
            .as_bytes(),
        )
        .await?;
    let mut head = Vec::new();
    let mut byte = [0u8; 1];
    while !head.ends_with(b"\r\n\r\n") && head.len() < 8192 {
        if stream.read(&mut byte).await? == 0 {
            break;
        }
        head.push(byte[0]);
    }
    let text = String::from_utf8_lossy(&head).into_owned();
    let status = text.lines().next().unwrap_or_default().to_owned();
    // A refusal's text, by its length (the connection stays open).
    let length = text
        .lines()
        .find_map(|line| {
            line.to_ascii_lowercase()
                .strip_prefix("content-length:")
                .map(str::to_owned)
        })
        .and_then(|n| n.trim().parse::<usize>().ok())
        .unwrap_or(0);
    let mut body = vec![0u8; length.min(8192)];
    stream.read_exact(&mut body).await?;
    Ok((
        stream,
        format!("{status} {}", String::from_utf8_lossy(&body)),
    ))
}

/// SPEC-353 LAP-002 (2026-10-30: the proxy answered every sandbox "too many connections" while
/// one sandbox's leftover processes held its connections, and that sandbox's own calls to Yulius
/// failed): one sandbox gets at most its share of the proxy, whichever of its runs asks, and the
/// refusal says why; another sandbox still gets a tunnel.
#[tokio::test]
async fn one_sandbox_cannot_take_every_connection() -> TestResult {
    let setup = setup().await?;
    let mut held = Vec::new();
    for at in 0..8 {
        let token = if at % 2 == 0 {
            "good-token"
        } else {
            "sibling-token"
        };
        let (stream, status) = tunnel(&setup, token).await?;
        assert!(status.contains(" 200"), "tunnel {at}: {status}");
        held.push(stream);
    }
    let (_refused, status) = tunnel(&setup, "sibling-token").await?;
    assert!(
        status.contains(" 503"),
        "past the sandbox's share: {status}"
    );
    assert!(status.contains("this sandbox"), "{status}");
    let (_other, status) = tunnel(&setup, "other-sandbox-token").await?;
    assert!(status.contains(" 200"), "another sandbox: {status}");
    // A tunnel that ends gives its slot back.
    drop(held.pop());
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        let (stream, status) = tunnel(&setup, "good-token").await?;
        if status.contains(" 200") {
            held.push(stream);
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "the slot never came back: {status}"
        );
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    Ok(())
}
