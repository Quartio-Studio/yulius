//! `WS /api/tasks/{tid}/terminal` (docs/api.md terminal protocol).

use std::time::Duration;

use axum::extract::ws::{CloseFrame, Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Path, State};
use axum::http::HeaderMap;
use axum::response::Response;
use tokio::sync::broadcast::error::RecvError;
use yulius_app::auth::AuthContext;
use yulius_app::hub::HubFrame;
use yulius_app::terminal::TerminalAttach;
use yulius_contracts::terminal::{
    CLOSE_LAGGING, KIND_OUTPUT, KIND_SNAPSHOT, TerminalClientMsg, TerminalServerMsg, encode_binary,
};

use super::events::check_origin;
use crate::AppState;
use crate::auth::Auth;
use crate::error::ApiResult;

pub async fn terminal(
    State(app): State<AppState>,
    Auth(ctx): Auth,
    Path(tid): Path<String>,
    headers: HeaderMap,
    ws: WebSocketUpgrade,
) -> ApiResult<Response> {
    check_origin(&app, &headers)?;
    let attach = app.terminal_open(&ctx, &tid).await?;
    Ok(ws.on_upgrade(move |socket| pump(app, ctx, attach, socket)))
}

async fn send_json(socket: &mut WebSocket, msg: &TerminalServerMsg) -> bool {
    match serde_json::to_string(msg) {
        Ok(text) => socket.send(Message::Text(text.into())).await.is_ok(),
        Err(_) => false,
    }
}

async fn send_state(app: &AppState, socket: &mut WebSocket, attach: &TerminalAttach) -> bool {
    let Ok(state) = app.terminal_state(&attach.task_id).await else {
        return false;
    };
    send_json(
        socket,
        &TerminalServerMsg::State {
            attempt_seq: state.attempt_seq,
            harness: state.harness,
            cols: state.size.0,
            rows: state.size.1,
            can_write: attach.can_write,
            controller: attach.is_controller(),
            status: state.status,
        },
    )
    .await
}

async fn pump(app: AppState, ctx: AuthContext, mut attach: TerminalAttach, mut socket: WebSocket) {
    if !send_state(&app, &mut socket, &attach).await {
        return;
    }
    let snapshot = encode_binary(
        KIND_SNAPSHOT,
        attach.subscription.seq,
        &attach.subscription.snapshot,
    );
    if socket.send(Message::Binary(snapshot.into())).await.is_err() {
        return;
    }
    let mut last_seq = attach.subscription.seq;
    let mut last_input_seq: Option<u64> = None;
    let mut recheck =
        tokio::time::interval(Duration::from_millis(app.config.viewer_recheck_ms.max(100)));
    recheck.tick().await;
    loop {
        tokio::select! {
            frame = attach.subscription.rx.recv() => {
                let ok = match frame {
                    Ok(HubFrame::Output { seq, bytes }) => {
                        if seq < last_seq { true } else {
                            last_seq = seq;
                            socket.send(Message::Binary(encode_binary(KIND_OUTPUT, seq, &bytes).into())).await.is_ok()
                        }
                    }
                    Ok(HubFrame::Snapshot { seq, bytes }) => {
                        last_seq = seq;
                        socket.send(Message::Binary(encode_binary(KIND_SNAPSHOT, seq, &bytes).into())).await.is_ok()
                    }
                    Ok(HubFrame::Notice(text)) => send_json(&mut socket, &TerminalServerMsg::Notice { text }).await,
                    Ok(HubFrame::State) => send_state(&app, &mut socket, &attach).await,
                    Ok(HubFrame::Exit { status, code }) => send_json(&mut socket, &TerminalServerMsg::Exit { status, code }).await,
                    Err(RecvError::Lagged(_)) => {
                        let _ = socket.send(Message::Close(Some(CloseFrame { code: CLOSE_LAGGING, reason: "lagging; reconnect".into() }))).await;
                        false
                    }
                    Err(RecvError::Closed) => false,
                };
                if !ok { break; }
            }
            incoming = socket.recv() => {
                let Some(Ok(message)) = incoming else { break };
                let text = match message {
                    Message::Text(t) => t,
                    Message::Close(_) => break,
                    _ => continue,
                };
                let Ok(msg) = serde_json::from_str::<TerminalClientMsg>(&text) else {
                    if !send_json(&mut socket, &TerminalServerMsg::Error { code: "invalid".into(), message: "unknown frame".into() }).await { break; }
                    continue;
                };
                let result = match msg {
                    TerminalClientMsg::Hello { cols, rows, .. } | TerminalClientMsg::Resize { cols, rows } => {
                        if attach.can_write { app.terminal_resize(&ctx, &attach, cols, rows).await } else { Ok(()) }
                    }
                    TerminalClientMsg::TakeControl => app.terminal_take_control(&ctx, &attach).await,
                    TerminalClientMsg::Input { data, seq } => {
                        if seq.is_some() && seq <= last_input_seq { Ok(()) } else {
                            if seq.is_some() { last_input_seq = seq; }
                            app.terminal_input(&ctx, &attach, data.as_bytes()).await
                        }
                    }
                };
                if let Err(err) = result
                    && !send_json(&mut socket, &TerminalServerMsg::Error { code: err.code().into(), message: err.public_message() }).await
                {
                    break;
                }
            }
            _ = recheck.tick() => {
                if !app.can_view_task(&ctx, &attach.task_id).await {
                    let _ = socket.send(Message::Close(Some(CloseFrame { code: 4003, reason: "access revoked".into() }))).await;
                    break;
                }
                if attach.can_write && !app.can_work(&ctx, &attach.task_id).await {
                    attach.can_write = false;
                    if !send_state(&app, &mut socket, &attach).await { break; }
                }
            }
        }
    }
}
