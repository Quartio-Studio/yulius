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
    // Sandboxes whose files this person may read (SPEC-127), decided again with `visible`.
    let mut writable: HashMap<String, bool> = HashMap::new();
    // The person as of the last re-check: their roles decide admin-only events (#722).
    let mut user = ctx.user.clone();
    let mut recheck =
        tokio::time::interval(Duration::from_millis(app.config.viewer_recheck_ms.max(100)));
    recheck.tick().await;
    // A quiet stream still says it is alive, so the page can tell a dead connection from a quiet
    // one and reconnect (SPEC-191 LIV-001).
    let mut heartbeat = tokio::time::interval(Duration::from_millis(
        app.config.events_heartbeat_ms.max(100),
    ));
    heartbeat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    heartbeat.tick().await;
    loop {
        tokio::select! {
            _ = heartbeat.tick() => {
                if send(&mut socket, &UiEvent::Heartbeat).await.is_err() { break; }
            }
            _ = recheck.tick() => {
                // Like the terminal viewers (ACC-008): an ended session closes the stream, and
                // sandbox visibility (#66) and instance administration (#722) are decided again
                // from the current permissions and roles.
                let Some(current) = app.signed_in_user(&ctx).await else {
                    let _ = socket.send(Message::Close(Some(CloseFrame { code: 4003, reason: "signed out".into() }))).await;
                    break;
                };
                user = current;
                visible.clear();
                writable.clear();
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
                        writable.clear();
                        if send(&mut socket, &UiEvent::Resync).await.is_err() { break; }
                        continue;
                    }
                    Err(RecvError::Closed) => break,
                };
                if envelope.admin_only && !(user.is_superadmin || user.is_instance_admin) { continue; }
                // A person's own event (their notepad, SPEC-103) reaches no one else.
                if envelope.user.as_ref().is_some_and(|id| *id != user.id) { continue; }
                if let Some(sandbox) = &envelope.sandbox {
                    let key = sandbox.to_string();
                    let allowed = match visible.get(&key) {
                        Some(v) => *v,
                        None => {
                            let ok = app.can_view(&user, sandbox).await;
                            visible.insert(key.clone(), ok);
                            ok
                        }
                    };
                    if !allowed { continue; }
                    // What the editor shows (who has which file open) is for those who may open it.
                    if envelope.writers_only {
                        let allowed = match writable.get(&key) {
                            Some(v) => *v,
                            None => {
                                let ok = app.can_work_in(&user, sandbox).await;
                                writable.insert(key, ok);
                                ok
                            }
                        };
                        if !allowed { continue; }
                    }
                } else if envelope.writers_only {
                    continue;
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
