//! `WS /api/sandboxes/{sid}/browser`: the shared sandbox browser viewer (docs/api.md "Browser,
//! editor, shell"; SPEC-001 BRW-001..004).
//!
//! Write access is required and re-checked every 60 s. The browser starts on first use. The
//! active tab's screencast is forwarded as binary frames through a small per-connection queue:
//! when the client is slow, frames are dropped (the browser's frame reader is never blocked);
//! JSON messages are queued separately and a client that stops draining them is disconnected.

use std::sync::Arc;
use std::time::Duration;

use axum::extract::ws::{CloseFrame, Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Path, State};
use axum::http::HeaderMap;
use axum::response::Response;
use futures::{SinkExt as _, StreamExt as _};
use tokio::sync::mpsc;
use yulius_app::auth::AuthContext;
use yulius_app::browser::BrowserGrant;
use yulius_app::ports::browser::{BrowserFrames, BrowserNotice, BrowserSession};
use yulius_contracts::browser::{
    TabId, TabInfo, ViewerClientMsg, ViewerErrorCode, ViewerServerMsg, encode_frame,
};

use super::events::check_origin;
use crate::AppState;
use crate::auth::Auth;
use crate::error::ApiResult;

/// Permission re-check interval for open viewers (ACC-008).
const RECHECK: Duration = Duration::from_secs(60);
/// Screencast frames queued per connection; newer frames are dropped while it is full.
pub const FRAME_QUEUE: usize = 4;
/// JSON messages queued per connection.
const CONTROL_QUEUE: usize = 64;
/// How long a JSON message may wait for queue space before the viewer is disconnected.
const CONTROL_SEND_TIMEOUT: Duration = Duration::from_secs(10);
/// Largest accepted client message.
const MAX_CLIENT_MESSAGE: usize = 64 * 1024;
/// Close code: access revoked.
const CLOSE_REVOKED: u16 = 4003;
/// Close code: the viewer did not drain its messages.
const CLOSE_LAGGING: u16 = 4008;
/// Close code: the browser failed or stopped.
const CLOSE_BROWSER_FAILED: u16 = 1011;

pub async fn browser(
    State(app): State<AppState>,
    Auth(ctx): Auth,
    Path(sid): Path<String>,
    headers: HeaderMap,
    ws: WebSocketUpgrade,
) -> ApiResult<Response> {
    check_origin(&app, &headers)?;
    let grant = app.browser_grant(&ctx, &sid).await?;
    Ok(ws
        .max_message_size(MAX_CLIENT_MESSAGE)
        .on_upgrade(move |socket| run(app, ctx, grant, socket)))
}

/// Outgoing queues of one connection.
struct Outbox {
    control: mpsc::Sender<Message>,
    frames: mpsc::Sender<Message>,
    dropped: u64,
}

impl Outbox {
    /// Queues a JSON message; `false` when the client is gone or not draining.
    async fn json(&self, message: &ViewerServerMsg) -> bool {
        let Ok(text) = serde_json::to_string(message) else {
            return false;
        };
        self.control
            .send_timeout(Message::Text(text.into()), CONTROL_SEND_TIMEOUT)
            .await
            .is_ok()
    }

    async fn close(&self, code: u16, reason: &str) {
        let frame = CloseFrame {
            code,
            reason: reason.to_owned().into(),
        };
        let _ = self
            .control
            .send_timeout(Message::Close(Some(frame)), CONTROL_SEND_TIMEOUT)
            .await;
    }

    /// Queues a frame unless the queue is full (then the frame is dropped).
    fn frame(&mut self, tab: &TabId, jpeg: &[u8]) -> bool {
        match self
            .frames
            .try_send(Message::Binary(encode_frame(tab, jpeg).into()))
        {
            Ok(()) => true,
            Err(mpsc::error::TrySendError::Full(_)) => {
                self.dropped += 1;
                true
            }
            Err(mpsc::error::TrySendError::Closed(_)) => false,
        }
    }
}

/// Writes queued messages to the socket, JSON first.
async fn write_loop(
    mut sink: futures::stream::SplitSink<WebSocket, Message>,
    mut control: mpsc::Receiver<Message>,
    mut frames: mpsc::Receiver<Message>,
) {
    loop {
        let message = tokio::select! {
            biased;
            message = control.recv() => message,
            message = frames.recv() => message,
        };
        let Some(message) = message else { break };
        let closing = matches!(message, Message::Close(_));
        if sink.send(message).await.is_err() || closing {
            break;
        }
    }
    let _ = sink.close().await;
}

fn active_tab(tabs: &[TabInfo]) -> Option<TabId> {
    tabs.iter()
        .find(|tab| tab.active)
        .or_else(|| tabs.first())
        .map(|tab| tab.id.clone())
}

async fn next_frame(frames: &mut Option<Box<dyn BrowserFrames>>) -> Option<(TabId, Arc<[u8]>)> {
    match frames.as_mut() {
        Some(subscription) => {
            let tab = subscription.tab_id().clone();
            subscription.next().await.map(|jpeg| (tab, jpeg))
        }
        None => std::future::pending().await,
    }
}

async fn subscribe(
    session: &Arc<dyn BrowserSession>,
    tab: Option<&TabId>,
) -> Option<Box<dyn BrowserFrames>> {
    let tab = tab?;
    match session.frames(tab).await {
        Ok(frames) => Some(frames),
        Err(failure) => {
            tracing::debug!(%failure, "screencast subscription failed");
            None
        }
    }
}

async fn run(app: AppState, ctx: AuthContext, grant: BrowserGrant, socket: WebSocket) {
    let (sink, mut stream) = socket.split();
    let (control_tx, control_rx) = mpsc::channel(CONTROL_QUEUE);
    let (frames_tx, frames_rx) = mpsc::channel(FRAME_QUEUE);
    let writer = tokio::spawn(write_loop(sink, control_rx, frames_rx));
    let mut out = Outbox {
        control: control_tx,
        frames: frames_tx,
        dropped: 0,
    };
    match app.browser_session(&grant.sandbox_id).await {
        Ok(session) => pump(&app, &ctx, &grant, &session, &mut stream, &mut out).await,
        Err(failure) => {
            let _ = out
                .json(&ViewerServerMsg::Error {
                    code: failure.viewer_code(),
                    message: failure.message.clone(),
                })
                .await;
            out.close(CLOSE_BROWSER_FAILED, "browser unavailable").await;
        }
    }
    if out.dropped > 0 {
        tracing::debug!(dropped = out.dropped, "browser viewer dropped frames");
    }
    drop(out);
    let _ = tokio::time::timeout(CONTROL_SEND_TIMEOUT, writer).await;
}

async fn pump(
    app: &AppState,
    ctx: &AuthContext,
    grant: &BrowserGrant,
    session: &Arc<dyn BrowserSession>,
    stream: &mut futures::stream::SplitStream<WebSocket>,
    out: &mut Outbox,
) {
    let mut tabs_rx = session.watch_tabs();
    let mut notices = session.notices();
    let tabs = tabs_rx.borrow_and_update().clone();
    if !out
        .json(&ViewerServerMsg::Tabs { tabs: tabs.clone() })
        .await
    {
        return;
    }
    let mut watching = active_tab(&tabs);
    let mut frames = subscribe(session, watching.as_ref()).await;
    let mut recheck = tokio::time::interval(RECHECK);
    recheck.tick().await;
    loop {
        tokio::select! {
            changed = tabs_rx.changed() => {
                if changed.is_err() {
                    out.close(CLOSE_BROWSER_FAILED, "browser stopped").await;
                    return;
                }
                let tabs = tabs_rx.borrow_and_update().clone();
                let active = active_tab(&tabs);
                if !out.json(&ViewerServerMsg::Tabs { tabs }).await {
                    out.close(CLOSE_LAGGING, "lagging; reconnect").await;
                    return;
                }
                if active != watching || frames.is_none() {
                    // Unsubscribe first so an unwatched screencast can stop.
                    drop(frames.take());
                    frames = subscribe(session, active.as_ref()).await;
                    watching = active;
                }
            }
            notice = notices.next() => {
                match notice {
                    Some(BrowserNotice::ActionRequested { tab_id, reason, task_id }) => {
                        if !out.json(&ViewerServerMsg::ActionRequested { tab_id, reason, task_id }).await {
                            return;
                        }
                    }
                    Some(BrowserNotice::ActionCleared { .. }) => {}
                    Some(BrowserNotice::Disconnected) | None => {
                        let _ = out.json(&ViewerServerMsg::Error {
                            code: ViewerErrorCode::Upstream,
                            message: "the sandbox browser stopped; reconnect to restart it".into(),
                        }).await;
                        out.close(CLOSE_BROWSER_FAILED, "browser stopped").await;
                        return;
                    }
                }
            }
            frame = next_frame(&mut frames) => {
                match frame {
                    Some((tab, jpeg)) => {
                        if !out.frame(&tab, &jpeg) {
                            return;
                        }
                    }
                    // The tab closed; a tab-list update selects the next one.
                    None => frames = None,
                }
            }
            incoming = stream.next() => {
                let Some(Ok(message)) = incoming else { return };
                let text = match message {
                    Message::Text(text) => text,
                    Message::Close(_) => return,
                    Message::Binary(_) => {
                        let error = ViewerServerMsg::Error {
                            code: ViewerErrorCode::Invalid,
                            message: "binary messages are not accepted".into(),
                        };
                        if !out.json(&error).await { return; }
                        continue;
                    }
                    _ => continue,
                };
                let result = match serde_json::from_str::<ViewerClientMsg>(&text) {
                    Ok(message) => session.apply_viewer(message).await.map_err(|failure| {
                        (failure.viewer_code(), failure.message)
                    }),
                    Err(err) => Err((ViewerErrorCode::Invalid, format!("invalid message: {err}"))),
                };
                if let Err((code, message)) = result
                    && !out.json(&ViewerServerMsg::Error { code, message }).await
                {
                    return;
                }
            }
            _ = recheck.tick() => {
                if !app.browser_still_allowed(ctx, grant).await {
                    out.close(CLOSE_REVOKED, "access revoked").await;
                    return;
                }
            }
        }
    }
}
