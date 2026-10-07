//! CDP client contract against the fake server: round trips, typed protocol errors,
//! timeouts on missing responses, the pending-request bound and disconnects.

use std::time::{Duration, Instant};

use serde_json::json;
use yulius_browser::fake::FakeCdpServer;
use yulius_browser::{CdpClient, CdpConfig, CdpError};

type TestResult = Result<(), Box<dyn std::error::Error>>;

const WAIT: Duration = Duration::from_secs(5);

#[tokio::test]
async fn calls_round_trip_with_sessions_and_typed_protocol_errors() -> TestResult {
    let fake = FakeCdpServer::start().await?;
    let cdp = CdpClient::connect(&fake.ws_url(), CdpConfig::default()).await?;
    let version = cdp.call("Browser.getVersion", json!({})).await?;
    assert_eq!(version["product"], "FakeChrome/1.0");

    let target = fake.target_ids().into_iter().next().ok_or("no target")?;
    let attached = cdp
        .call(
            "Target.attachToTarget",
            json!({"targetId": target, "flatten": true}),
        )
        .await?;
    let session = attached["sessionId"].as_str().ok_or("no session id")?;
    cdp.call_session(session, "Page.enable", json!({})).await?;
    let recorded = fake.calls_for("Page.enable");
    assert_eq!(recorded.len(), 1);
    assert_eq!(recorded[0].session_id.as_deref(), Some(session));

    match cdp.call("Nope.method", json!({})).await {
        Err(CdpError::Protocol {
            method,
            code,
            message,
        }) => {
            assert_eq!(method, "Nope.method");
            assert_eq!(code, -32601);
            assert!(message.contains("wasn't found"), "{message}");
        }
        other => return Err(format!("expected a protocol error, got {other:?}").into()),
    }
    assert_eq!(cdp.pending_requests(), 0);
    Ok(())
}

#[tokio::test]
async fn missing_responses_time_out_and_free_their_slot() -> TestResult {
    let fake = FakeCdpServer::start().await?;
    fake.mute("Page.navigate");
    let config = CdpConfig {
        call_timeout: Duration::from_millis(300),
        ..CdpConfig::default()
    };
    let cdp = CdpClient::connect(&fake.ws_url(), config).await?;
    let started = Instant::now();
    match cdp
        .call("Page.navigate", json!({"url": "https://example.com"}))
        .await
    {
        Err(CdpError::Timeout { method, timeout }) => {
            assert_eq!(method, "Page.navigate");
            assert_eq!(timeout, Duration::from_millis(300));
        }
        other => return Err(format!("expected a timeout, got {other:?}").into()),
    }
    assert!(started.elapsed() < WAIT);
    assert_eq!(cdp.pending_requests(), 0, "timed-out entry must be removed");
    cdp.call("Browser.getVersion", json!({})).await?;
    Ok(())
}

#[tokio::test]
async fn pending_requests_are_bounded_and_cancellation_frees_slots() -> TestResult {
    let fake = FakeCdpServer::start().await?;
    fake.mute("Page.navigate");
    let config = CdpConfig {
        max_pending: 2,
        call_timeout: Duration::from_secs(30),
        ..CdpConfig::default()
    };
    let cdp = CdpClient::connect(&fake.ws_url(), config).await?;
    let spawn_call =
        |cdp: CdpClient| tokio::spawn(async move { cdp.call("Page.navigate", json!({})).await });
    let first = spawn_call(cdp.clone());
    let second = spawn_call(cdp.clone());
    assert!(fake.wait_for_calls("Page.navigate", 2, WAIT).await);
    assert_eq!(cdp.pending_requests(), 2);
    assert_eq!(
        cdp.call("Browser.getVersion", json!({})).await,
        Err(CdpError::Busy(2))
    );
    first.abort();
    second.abort();
    let _ = first.await;
    let _ = second.await;
    assert_eq!(
        cdp.pending_requests(),
        0,
        "dropped calls must release their slot"
    );
    cdp.call("Browser.getVersion", json!({})).await?;
    Ok(())
}

#[tokio::test]
async fn disconnect_fails_pending_and_later_calls() -> TestResult {
    let fake = FakeCdpServer::start().await?;
    fake.mute("Page.navigate");
    let config = CdpConfig {
        call_timeout: Duration::from_secs(30),
        ..CdpConfig::default()
    };
    let cdp = CdpClient::connect(&fake.ws_url(), config).await?;
    let pending = {
        let cdp = cdp.clone();
        tokio::spawn(async move { cdp.call("Page.navigate", json!({})).await })
    };
    assert!(fake.wait_for_calls("Page.navigate", 1, WAIT).await);
    fake.disconnect_all();
    let outcome = tokio::time::timeout(WAIT, pending).await??;
    assert_eq!(outcome, Err(CdpError::Closed));
    tokio::time::timeout(WAIT, cdp.closed()).await?;
    assert!(cdp.is_closed());
    assert_eq!(
        cdp.call("Browser.getVersion", json!({})).await,
        Err(CdpError::Closed)
    );
    Ok(())
}

#[tokio::test]
async fn connecting_to_a_closed_port_is_a_typed_error() -> TestResult {
    let port = {
        let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
        listener.local_addr()?.port()
    };
    let url = format!("ws://127.0.0.1:{port}/devtools/browser/none");
    match CdpClient::connect(&url, CdpConfig::default()).await {
        Err(CdpError::Connect(_)) => Ok(()),
        other => Err(format!("expected a connect error, got {other:?}").into()),
    }
}
