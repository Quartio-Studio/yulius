//! Sandbox shells: `GET`/`POST /api/sandboxes/{sid}/shells`, `PATCH`/`DELETE
//! /api/sandboxes/{sid}/shells/{id}`, the Cockpit `POST /api/sandboxes/{sid}/cockpit` and
//! `WS /api/shells/{id}/terminal` (docs/api.md; SPEC-001 UI-015, UI-016, AC-UI-7; SPEC-021). The WebSocket speaks the task terminal protocol.

use std::time::Duration;

use axum::Json;
use axum::body::Bytes;
use axum::extract::ws::{CloseFrame, Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::Response;
use tokio::sync::broadcast::error::RecvError;
use yulius_app::auth::{AuthContext, rfc3339};
use yulius_app::error::AppError;
use yulius_app::hub::HubFrame;
use yulius_app::shells::{ShellAttach, ShellInfo};
use yulius_contracts::api::shells::{CreateShell, RenameShell, Shell};
use yulius_contracts::terminal::{
    CLOSE_LAGGING, KIND_OUTPUT, KIND_SNAPSHOT, TerminalClientMsg, TerminalServerMsg, encode_binary,
};

use super::events::check_origin;
use super::terminal::InitialSize;
use crate::AppState;
use crate::auth::Auth;
use crate::error::ApiResult;

/// Permission re-check interval for open viewers (ACC-008).
const RECHECK: Duration = Duration::from_secs(60);
/// Close code: access revoked.
const CLOSE_REVOKED: u16 = 4003;

fn dto(info: ShellInfo) -> Shell {
    Shell {
        id: info.id,
        name: info.name,
        task_id: info.task_id,
        folder: info.folder,
        created_at: rfc3339(info.created_at),
    }
}

pub async fn list(
    State(app): State<AppState>,
    Auth(ctx): Auth,
    Path(sid): Path<String>,
) -> ApiResult<Json<Vec<Shell>>> {
    let shells = app.shell_list(&ctx, &sid).await?;
    Ok(Json(shells.into_iter().map(dto).collect()))
}

pub async fn create(
    State(app): State<AppState>,
    Auth(ctx): Auth,
    Path(sid): Path<String>,
    body: Bytes,
) -> ApiResult<(StatusCode, Json<Shell>)> {
    // An empty body means "the sandbox repository" with a default name.
    let request: CreateShell = if body.iter().all(u8::is_ascii_whitespace) {
        CreateShell::default()
    } else {
        serde_json::from_slice(&body)
            .map_err(|err| AppError::Invalid(format!("invalid request body: {err}")))?
    };
    let shell = app
        .shell_create(
            &ctx,
            &sid,
            request.task_id.as_deref(),
            request.name.as_deref(),
        )
        .await?;
    Ok((StatusCode::CREATED, Json(dto(shell))))
}

/// `POST /api/sandboxes/{sid}/cockpit`: the caller's Cockpit, started when it is not running
/// (SPEC-021). Its terminal is `WS /api/shells/{id}/terminal`.
pub async fn cockpit(
    State(app): State<AppState>,
    Auth(ctx): Auth,
    Path(sid): Path<String>,
) -> ApiResult<Json<Shell>> {
    Ok(Json(dto(app.cockpit_open(&ctx, &sid).await?)))
}

pub async fn rename(
    State(app): State<AppState>,
    Auth(ctx): Auth,
    Path((sid, id)): Path<(String, String)>,
    body: Bytes,
) -> ApiResult<Json<Shell>> {
    let request: RenameShell = serde_json::from_slice(&body)
        .map_err(|err| AppError::Invalid(format!("invalid request body: {err}")))?;
    Ok(Json(dto(app
        .shell_rename(&ctx, &sid, &id, &request.name)
        .await?)))
}

pub async fn remove(
    State(app): State<AppState>,
    Auth(ctx): Auth,
    Path((sid, id)): Path<(String, String)>,
) -> ApiResult<StatusCode> {
    app.shell_close(&ctx, &sid, &id).await?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn terminal(
    State(app): State<AppState>,
    Auth(ctx): Auth,
    Path(id): Path<String>,
    Query(size): Query<InitialSize>,
    headers: HeaderMap,
    ws: WebSocketUpgrade,
) -> ApiResult<Response> {
    check_origin(&app, &headers)?;
    // The viewer's size is applied before its snapshot is taken, so the first state and the
    // snapshot already have it: nothing is redrawn at another size after the viewer appears.
    let attach = app.shell_open(&ctx, &id, size.get()).await?;
    Ok(ws.on_upgrade(move |socket| pump(app, ctx, attach, socket)))
}

async fn send_json(socket: &mut WebSocket, msg: &TerminalServerMsg) -> bool {
    match serde_json::to_string(msg) {
        Ok(text) => socket.send(Message::Text(text.into())).await.is_ok(),
        Err(_) => false,
    }
}

async fn send_state(socket: &mut WebSocket, attach: &ShellAttach) -> bool {
    let (cols, rows) = attach.size();
    let status = if attach.has_ended() {
        "closed"
    } else {
        "running"
    };
    send_json(
        socket,
        &TerminalServerMsg::State {
            attempt_seq: None,
            harness: None,
            cols,
            rows,
            can_write: true,
            controller: true,
            status: status.into(),
        },
    )
    .await
}

async fn close(socket: &mut WebSocket, code: u16, reason: &str) {
    let frame = CloseFrame {
        code,
        reason: reason.to_owned().into(),
    };
    let _ = socket.send(Message::Close(Some(frame))).await;
}

async fn pump(app: AppState, ctx: AuthContext, mut attach: ShellAttach, mut socket: WebSocket) {
    if !send_state(&mut socket, &attach).await {
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
    let mut recheck = tokio::time::interval(RECHECK);
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
                    Ok(HubFrame::State) => send_state(&mut socket, &attach).await,
                    Ok(HubFrame::Exit { status, code }) => {
                        // The shell is gone for good: report and close (clients do not reconnect
                        // after an exit frame).
                        if send_json(&mut socket, &TerminalServerMsg::Exit { status, code }).await {
                            close(&mut socket, 1000, "shell ended").await;
                        }
                        false
                    }
                    Err(RecvError::Lagged(_)) => {
                        close(&mut socket, CLOSE_LAGGING, "lagging; reconnect").await;
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
                        app.shell_resize(&ctx, &attach, cols, rows).await
                    }
                    // One user's own terminal: its size follows the last resize.
                    TerminalClientMsg::TakeControl | TerminalClientMsg::Active | TerminalClientMsg::Idle => Ok(()),
                    TerminalClientMsg::Input { data, seq, .. } => {
                        if seq.is_some() && seq <= last_input_seq { Ok(()) } else {
                            if seq.is_some() { last_input_seq = seq; }
                            app.shell_input(&ctx, &attach, data.as_bytes()).await
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
                if !app.still_signed_in(&ctx).await {
                    close(&mut socket, CLOSE_REVOKED, "signed out").await;
                    break;
                }
                if !app.shell_still_allowed(&ctx, &attach).await {
                    close(&mut socket, CLOSE_REVOKED, "access revoked").await;
                    break;
                }
            }
        }
    }
}
