//! Connection troubles on the panel (SPEC-361): a run that fills its sandbox's share of the real
//! egress proxy is listed by `GET /api/connections` with its task and count, to the sandbox's
//! people only, with one `connections.updated` when it begins and one when it ends.

use crate::support;

use axum::http::{Method, StatusCode};
use base64::Engine as _;
use support::{
    Creds, TestResult, call, perms, seed_github_user, seed_live_attempt, seed_sandbox, seed_task,
};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use yulius_app::ports::store::TaskStore as _;
use yulius_contracts::events::UiEvent;
use yulius_domain::ids::TaskId;
use yulius_domain::task::TaskStatus;

const TOKEN: &str = "health-task-token-s1-0123456789abcdef";

async fn connect(
    addr: std::net::SocketAddr,
) -> Result<(tokio::net::TcpStream, String), Box<dyn std::error::Error>> {
    let credentials = base64::engine::general_purpose::STANDARD.encode(format!("yulius:{TOKEN}"));
    let mut stream = tokio::net::TcpStream::connect(addr).await?;
    stream
        .write_all(
            format!(
                "CONNECT tunnel.test:443 HTTP/1.1\r\nHost: tunnel.test:443\r\nProxy-Authorization: Basic {credentials}\r\n\r\n"
            )
            .as_bytes(),
        )
        .await?;
    let mut buffer = vec![0u8; 1024];
    let n = tokio::time::timeout(std::time::Duration::from_secs(10), stream.read(&mut buffer))
        .await??;
    Ok((stream, String::from_utf8_lossy(&buffer[..n]).into_owned()))
}

async fn troubles(
    h: &support::Harness,
    creds: &Creds,
) -> Result<Vec<serde_json::Value>, Box<dyn std::error::Error>> {
    let r = call(&h.router, Method::GET, "/api/connections", None, creds).await?;
    assert_eq!(r.status, StatusCode::OK, "{}", r.body);
    Ok(r.body["troubles"].as_array().cloned().unwrap_or_default())
}

/// The acceptance test of task #344: fill a sandbox's share, see it with the run's task and its
/// count, then see it cleared once the run's tunnels end.
#[tokio::test]
async fn a_full_share_is_listed_with_the_runs_task_until_its_tunnels_end() -> TestResult {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let h = support::harness()?;
    let reader = seed_github_user(&h, "u-bob", "bob").await?;
    let outsider = seed_github_user(&h, "u-carol", "carol").await?;
    seed_github_user(&h, "u-alice", "alice").await?;
    seed_sandbox(&h, "s1", "app", "u-alice").await?;
    h.github.set_perm("bob", "acme/app", perms("read"));
    seed_task(&h, "t1", "s1", "u-alice", TaskStatus::Running, None).await?;
    seed_live_attempt(&h, "t1", TOKEN).await?;
    let task = h.store.task(&TaskId::parse("t1")?).await?.ok_or("t1")?;

    let echo = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let echo_addr = echo.local_addr()?;
    // Echoes until the client closes, then closes too, which ends the tunnel.
    tokio::spawn(async move {
        while let Ok((mut stream, _)) = echo.accept().await {
            tokio::spawn(async move {
                let (mut read, mut write) = stream.split();
                let _ = tokio::io::copy(&mut read, &mut write).await;
            });
        }
    });
    let proxy = yulius::wiring::egress::egress_proxy(
        &h.app,
        yulius_egress::EgressConfig {
            resolve: std::collections::HashMap::from([("tunnel.test".to_owned(), echo_addr)]),
            max_per_sandbox: 4,
            ..yulius_egress::EgressConfig::default()
        },
    )
    .await?;
    let router = yulius::wiring::egress::with_egress(
        yulius_server::internal_router(h.app.clone()),
        proxy,
        &h.app.config.internal_url,
    )
    .layer(axum::extract::connect_info::MockConnectInfo(
        std::net::SocketAddr::from(([127, 0, 0, 1], 5001)),
    ));
    let served = support::serve(router).await?;
    let mut events = h.app.events.subscribe();

    let mut tunnels = Vec::new();
    for at in 0..4 {
        let (stream, status) = connect(served.addr).await?;
        assert!(status.starts_with("HTTP/1.1 200"), "tunnel {at}: {status}");
        tunnels.push(stream);
    }
    assert!(
        troubles(&h, &reader).await?.is_empty(),
        "nothing refused yet"
    );
    for _ in 0..2 {
        let (_, status) = connect(served.addr).await?;
        assert!(status.starts_with("HTTP/1.1 503"), "{status}");
    }

    let listed = troubles(&h, &reader).await?;
    assert_eq!(listed.len(), 1, "{listed:?}");
    let trouble = &listed[0];
    assert_eq!(trouble["limit"], "share", "{trouble}");
    assert_eq!(trouble["sandbox_id"], "s1");
    assert_eq!(trouble["sandbox_name"], "app");
    assert_eq!(
        (trouble["held"].as_u64(), trouble["max"].as_u64()),
        (Some(4), Some(4))
    );
    assert_eq!(trouble["refused"].as_u64(), Some(2));
    let holder = &trouble["holders"][0];
    assert_eq!(holder["kind"], "task", "{holder}");
    assert_eq!(holder["task_id"], "t1");
    assert_eq!(holder["task_number"].as_u64(), Some(u64::from(task.number)));
    assert_eq!(holder["task_title"], "task t1");
    assert_eq!(holder["held"].as_u64(), Some(4));
    assert!(
        troubles(&h, &outsider).await?.is_empty(),
        "someone who cannot view the sandbox does not see its trouble"
    );

    tunnels.clear();
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(10);
    while !troubles(&h, &reader).await?.is_empty() {
        assert!(
            tokio::time::Instant::now() < deadline,
            "the trouble never ended"
        );
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }

    let mut published = Vec::new();
    while let Ok(envelope) = events.try_recv() {
        if let UiEvent::ConnectionsUpdated { sandbox_id } = envelope.event {
            assert_eq!(envelope.sandbox.map(|s| s.to_string()), sandbox_id.clone());
            published.push(sandbox_id);
        }
    }
    assert_eq!(
        published,
        [Some("s1".to_owned()), Some("s1".to_owned())],
        "once when it began, once when it ended, never per refusal"
    );
    Ok(())
}

/// SPEC-361 CHR-004: the proxy's own trouble reaches everyone, naming only the sandboxes they can
/// view; a relay trouble reaches its sandbox's viewers only.
#[tokio::test]
async fn the_proxys_trouble_names_only_sandboxes_the_person_can_view() -> TestResult {
    use yulius_domain::connection_health::{Limit, Refusal, busiest};
    let h = support::harness()?;
    let reader = seed_github_user(&h, "u-bob", "bob").await?;
    let outsider = seed_github_user(&h, "u-carol", "carol").await?;
    seed_github_user(&h, "u-alice", "alice").await?;
    seed_sandbox(&h, "s1", "app", "u-alice").await?;
    seed_sandbox(&h, "s2", "other", "u-alice").await?;
    h.github.set_perm("bob", "acme/app", perms("read"));
    let health = h.app.connection_health();
    health.refused(
        Refusal {
            limit: Limit::Proxy,
            sandbox: None,
            held: 256,
            max: 256,
            holders: busiest([("s2".to_owned(), 130), ("s1".to_owned(), 126)]),
        },
        1_000,
    );
    health.refused(
        Refusal {
            limit: Limit::Relay,
            sandbox: Some("s1".to_owned()),
            held: 256,
            max: 256,
            holders: Vec::new(),
        },
        2_000,
    );
    let listed = troubles(&h, &reader).await?;
    let limits: Vec<_> = listed.iter().map(|t| t["limit"].clone()).collect();
    assert_eq!(limits, ["proxy", "relay"], "{listed:?}");
    let holders: Vec<_> = listed[0]["holders"]
        .as_array()
        .ok_or("holders")?
        .iter()
        .map(|h| (h["kind"].clone(), h["label"].clone(), h["held"].clone()))
        .collect();
    assert_eq!(
        holders,
        [
            ("other_sandbox".into(), "another sandbox".into(), 130.into()),
            ("sandbox".into(), "app".into(), 126.into()),
        ]
    );
    assert_eq!(listed[1]["sandbox_name"], "app");
    let theirs = troubles(&h, &outsider).await?;
    assert_eq!(theirs.len(), 1, "only the proxy's: {theirs:?}");
    assert_eq!(theirs[0]["holders"][1]["label"], "another sandbox");
    Ok(())
}
