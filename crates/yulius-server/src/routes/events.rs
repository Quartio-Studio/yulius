//! `WS /api/events`: permission-filtered UI events with bounded buffering.

use std::collections::HashMap;
use std::time::Duration;

use axum::extract::State;
use axum::extract::ws::{CloseFrame, Message, WebSocket, WebSocketUpgrade};
use axum::http::HeaderMap;
use axum::response::Response;
use tokio::sync::broadcast::error::RecvError;
use yulius_app::auth::AuthContext;
use yulius_app::error::AppError;
use yulius_contracts::events::UiEvent;

use crate::AppState;
use crate::auth::Auth;
use crate::error::ApiResult;

/// Rejects cross-origin WebSocket upgrades (SPEC-002 §8).
pub fn check_origin(app: &AppState, headers: &HeaderMap) -> Result<(), AppError> {
    let origin = headers.get("origin").and_then(|v| v.to_str().ok());
    match origin {
        None => Ok(()),
        Some(o) if app.origin_allowed(o) => Ok(()),
        Some(_) => Err(AppError::Forbidden("cross-origin WebSocket refused".into())),
    }
}

pub async fn events(
    State(app): State<AppState>,
    Auth(ctx): Auth,
    headers: HeaderMap,
    ws: WebSocketUpgrade,
) -> ApiResult<Response> {
    check_origin(&app, &headers)?;
    Ok(ws.on_upgrade(move |socket| pump(app, ctx, socket)))
}

async fn pump(app: AppState, ctx: AuthContext, mut socket: WebSocket) {
    let mut rx = app.events.subscribe();
    let mut visible: HashMap<String, bool> = HashMap::new();
    let is_admin = ctx.user.is_superadmin || ctx.user.is_instance_admin;
    let mut recheck =
        tokio::time::interval(Duration::from_millis(app.config.viewer_recheck_ms.max(100)));
    recheck.tick().await;
    loop {
        tokio::select! {
            _ = recheck.tick() => {
                // Like the terminal viewers (ACC-008): an ended session closes the stream, and
                // sandbox visibility is decided again from the current permissions (#66).
                if !app.still_signed_in(&ctx).await {
                    let _ = socket.send(Message::Close(Some(CloseFrame { code: 4003, reason: "signed out".into() }))).await;
                    break;
                }
                visible.clear();
            }
            incoming = socket.recv() => {
                match incoming {
                    Some(Ok(Message::Close(_))) | None | Some(Err(_)) => break,
                    _ => {}
                }
            }
            event = rx.recv() => {
                let envelope = match event {
                    Ok(envelope) => envelope,
                    Err(RecvError::Lagged(_)) => {
                        visible.clear();
                        if send(&mut socket, &UiEvent::Resync).await.is_err() { break; }
                        continue;
                    }
                    Err(RecvError::Closed) => break,
                };
                if envelope.admin_only && !is_admin { continue; }
                if let Some(sandbox) = &envelope.sandbox {
                    let key = sandbox.to_string();
                    let allowed = match visible.get(&key) {
                        Some(v) => *v,
                        None => {
                            let ok = app.can_view(&ctx.user, sandbox).await;
                            visible.insert(key, ok);
                            ok
                        }
                    };
                    if !allowed { continue; }
                }
                if send(&mut socket, &envelope.event).await.is_err() { break; }
            }
        }
    }
}

async fn send(socket: &mut WebSocket, event: &UiEvent) -> Result<(), ()> {
    let text = serde_json::to_string(event).map_err(|_| ())?;
    socket
        .send(Message::Text(text.into()))
        .await
        .map_err(|_| ())
}
