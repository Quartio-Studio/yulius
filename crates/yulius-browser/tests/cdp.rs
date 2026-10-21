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

/// The browser's side of a `--remote-debugging-pipe` connection over in-memory pipes.
struct PipePeer {
    requests: tokio::io::BufReader<tokio::io::DuplexStream>,
    replies: tokio::io::DuplexStream,
}

impl PipePeer {
    /// A connected client and its peer; messages up to `max_message_bytes`.
    fn connect(max_message_bytes: usize) -> (CdpClient, Self) {
        let (client_out, peer_in) = tokio::io::duplex(1 << 16);
        let (peer_out, client_in) = tokio::io::duplex(1 << 16);
        let config = CdpConfig {
            max_message_bytes,
            call_timeout: Duration::from_secs(5),
            ..CdpConfig::default()
        };
        let cdp = CdpClient::over_pipe(client_in, client_out, config);
        let peer = Self {
            requests: tokio::io::BufReader::new(peer_in),
            replies: peer_out,
        };
        (cdp, peer)
    }

    /// The next request: one JSON message terminated by a NUL byte.
    async fn request(&mut self) -> Result<serde_json::Value, Box<dyn std::error::Error>> {
        use tokio::io::AsyncBufReadExt as _;
        let mut message = Vec::new();
        tokio::time::timeout(WAIT, self.requests.read_until(0, &mut message)).await??;
        assert_eq!(message.pop(), Some(0), "every request ends with NUL");
        Ok(serde_json::from_slice(&message)?)
    }

    async fn send(&mut self, bytes: &[u8]) -> TestResult {
        use tokio::io::AsyncWriteExt as _;
        self.replies.write_all(bytes).await?;
        Ok(())
    }

    async fn reply(&mut self, message: &serde_json::Value) -> TestResult {
        let mut bytes = message.to_string().into_bytes();
        bytes.push(0);
        self.send(&bytes).await
    }
}

#[tokio::test]
async fn pipe_calls_round_trip_and_events_arrive() -> TestResult {
    let (cdp, mut peer) = PipePeer::connect(1 << 20);
    let mut events = cdp.subscribe_events();
    let call = tokio::spawn({
        let cdp = cdp.clone();
        async move { cdp.call_session("S1", "Page.enable", json!({"a": 1})).await }
    });
    let request = peer.request().await?;
    assert_eq!(request["method"], "Page.enable");
    assert_eq!(request["sessionId"], "S1");
    assert_eq!(request["params"], json!({"a": 1}));
    // An event and the response arrive in one write, each NUL-terminated.
    let mut both = json!({"method": "Target.targetCreated", "params": {"x": 2}})
        .to_string()
        .into_bytes();
    both.push(0);
    both.extend(
        json!({"id": request["id"], "result": {"ok": true}})
            .to_string()
            .into_bytes(),
    );
    both.push(0);
    peer.send(&both).await?;
    assert_eq!(call.await??, json!({"ok": true}));
    let event = tokio::time::timeout(WAIT, events.recv()).await??;
    assert_eq!(event.method, "Target.targetCreated");
    assert_eq!(event.params, json!({"x": 2}));

    let call = tokio::spawn({
        let cdp = cdp.clone();
        async move { cdp.call("Nope.method", json!({})).await }
    });
    let request = peer.request().await?;
    peer.reply(&json!({"id": request["id"], "error": {"code": -32601, "message": "no"}}))
        .await?;
    assert!(matches!(
        call.await?,
        Err(CdpError::Protocol { code: -32601, .. })
    ));
    assert!(!cdp.is_closed());
    Ok(())
}

#[tokio::test]
async fn pipe_screencast_frames_are_acknowledged() -> TestResult {
    let (_cdp, mut peer) = PipePeer::connect(1 << 20);
    peer.reply(&json!({"method": "Page.screencastFrame", "sessionId": "S1",
        "params": {"sessionId": 7, "data": "AAAA", "metadata": {}}}))
        .await?;
    let ack = peer.request().await?;
    assert_eq!(ack["method"], "Page.screencastFrameAck");
    assert_eq!(ack["sessionId"], "S1");
    assert_eq!(ack["params"]["sessionId"], 7);
    Ok(())
}

#[tokio::test]
async fn pipe_message_over_the_limit_closes_the_connection() -> TestResult {
    let (cdp, mut peer) = PipePeer::connect(64);
    let call = tokio::spawn({
        let cdp = cdp.clone();
        async move { cdp.call("Browser.getVersion", json!({})).await }
    });
    let request = peer.request().await?;
    let mut long = json!({"id": request["id"], "result": {"product": "x".repeat(200)}})
        .to_string()
        .into_bytes();
    long.push(0);
    peer.send(&long).await?;
    assert!(matches!(call.await?, Err(CdpError::Closed)));
    tokio::time::timeout(WAIT, cdp.closed()).await?;
    Ok(())
}

#[tokio::test]
async fn pipe_end_of_file_closes_the_connection_and_fails_pending_calls() -> TestResult {
    let (cdp, mut peer) = PipePeer::connect(1 << 20);
    let call = tokio::spawn({
        let cdp = cdp.clone();
        async move { cdp.call("Browser.getVersion", json!({})).await }
    });
    peer.request().await?;
    drop(peer);
    assert!(matches!(call.await?, Err(CdpError::Closed)));
    assert!(cdp.is_closed());
    assert!(matches!(
        cdp.call("Browser.getVersion", json!({})).await,
        Err(CdpError::Closed)
    ));
    Ok(())
}

#[tokio::test]
async fn call_within_bounds_one_call_by_its_own_timeout() -> TestResult {
    let (cdp, mut peer) = PipePeer::connect(1 << 20);
    let started = Instant::now();
    let outcome = cdp
        .call_within("Browser.getVersion", json!({}), Duration::from_millis(200))
        .await;
    assert!(
        matches!(outcome, Err(CdpError::Timeout { ref method, .. }) if method == "Browser.getVersion"),
        "{outcome:?}"
    );
    assert!(
        started.elapsed() < Duration::from_secs(4),
        "not the 5 s call timeout"
    );
    peer.request().await?;
    assert_eq!(cdp.pending_requests(), 0);
    Ok(())
}

#[tokio::test]
async fn pipe_message_of_exactly_the_limit_is_accepted() -> TestResult {
    let reply = json!({"id": 1, "result": {"p": ""}}).to_string();
    let limit = reply.len() + 10;
    let (cdp, mut peer) = PipePeer::connect(limit);
    let call = tokio::spawn({
        let cdp = cdp.clone();
        async move { cdp.call("Browser.getVersion", json!({})).await }
    });
    let request = peer.request().await?;
    let padded = json!({"id": request["id"], "result": {"p": ""}}).to_string();
    let pad = "x".repeat(limit - padded.len());
    let mut exact = json!({"id": request["id"], "result": {"p": pad}})
        .to_string()
        .into_bytes();
    assert_eq!(
        exact.len(),
        limit,
        "the message without its NUL is the limit"
    );
    exact.push(0);
    peer.send(&exact).await?;
    assert_eq!(call.await??["p"].as_str().map(str::len), Some(pad.len()));
    assert!(!cdp.is_closed());
    Ok(())
}
