//! Provider account administration and the login terminal WebSocket (instance admins).

use axum::Json;
use axum::extract::ws::{CloseFrame, Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::Response;
use tokio::sync::broadcast::error::RecvError;
use yulius_app::accounts::LoginTerminal;
use yulius_app::hub::HubFrame;
use yulius_contracts::api::admin::{
    AccountLoginStarted, AccountView, CreateAccount, UpdateAccount,
};
use yulius_contracts::terminal::{
    CLOSE_LAGGING, KIND_OUTPUT, KIND_SNAPSHOT, TerminalClientMsg, TerminalServerMsg, encode_binary,
};

use super::events::check_origin;
use crate::AppState;
use crate::auth::Auth;
use crate::error::ApiResult;

pub async fn list(
    State(app): State<AppState>,
    Auth(ctx): Auth,
) -> ApiResult<Json<Vec<AccountView>>> {
    Ok(Json(app.list_accounts(&ctx).await?))
}

pub async fn create(
    State(app): State<AppState>,
    Auth(ctx): Auth,
    Json(req): Json<CreateAccount>,
) -> ApiResult<(StatusCode, Json<AccountLoginStarted>)> {
    Ok((
        StatusCode::CREATED,
        Json(app.create_account(&ctx, &req).await?),
    ))
}

pub async fn update(
    State(app): State<AppState>,
    Auth(ctx): Auth,
    Path(id): Path<String>,
    Json(req): Json<UpdateAccount>,
) -> ApiResult<Json<AccountView>> {
    Ok(Json(app.update_account(&ctx, &id, &req).await?))
}

pub async fn delete(
    State(app): State<AppState>,
    Auth(ctx): Auth,
    Path(id): Path<String>,
) -> ApiResult<StatusCode> {
    app.delete_account(&ctx, &id).await?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn relogin(
    State(app): State<AppState>,
    Auth(ctx): Auth,
    Path(id): Path<String>,
) -> ApiResult<Json<AccountLoginStarted>> {
    Ok(Json(app.relogin_account(&ctx, &id).await?))
}

/// `WS /api/admin/accounts/login/{login_id}/terminal` (terminal protocol, docs/api.md).
pub async fn login_terminal(
    State(app): State<AppState>,
    Auth(ctx): Auth,
    headers: HeaderMap,
    Path(login_id): Path<String>,
    ws: WebSocketUpgrade,
) -> ApiResult<Response> {
    check_origin(&app, &headers)?;
    let terminal = app.login_terminal(&ctx, &login_id)?;
    Ok(ws.on_upgrade(move |socket| pump(terminal, socket)))
}

async fn send_json(socket: &mut WebSocket, msg: &TerminalServerMsg) -> Result<(), ()> {
    let text = serde_json::to_string(msg).map_err(|_| ())?;
    socket
        .send(Message::Text(text.into()))
        .await
        .map_err(|_| ())
}

async fn send_binary(socket: &mut WebSocket, kind: u8, seq: u64, bytes: &[u8]) -> Result<(), ()> {
    socket
        .send(Message::Binary(encode_binary(kind, seq, bytes).into()))
        .await
        .map_err(|_| ())
}

async fn pump(terminal: LoginTerminal, mut socket: WebSocket) {
    let mut sub = terminal.subscribe();
    let _ = serve(&terminal, &mut sub, &mut socket).await;
    terminal.unsubscribe();
}

async fn serve(
    terminal: &LoginTerminal,
    sub: &mut yulius_app::hub::Subscription,
    socket: &mut WebSocket,
) -> Result<(), ()> {
    send_json(socket, &terminal.state()).await?;
    send_binary(socket, KIND_SNAPSHOT, sub.seq, &sub.snapshot).await?;
    if let Some(exit) = terminal.exit() {
        send_json(socket, &exit).await?;
    }
    loop {
        tokio::select! {
            incoming = socket.recv() => match incoming {
                Some(Ok(Message::Text(text))) => {
                    match serde_json::from_str::<TerminalClientMsg>(&text) {
                        Ok(TerminalClientMsg::Input { data, .. }) => {
                            if let Err(err) = terminal.input(data.as_bytes()).await {
                                send_json(socket, &TerminalServerMsg::Error {
                                    code: err.code().to_owned(),
                                    message: err.public_message(),
                                }).await?;
                            }
                        }
                        // The PTY width is fixed so tokens and URLs never wrap; size requests
                        // and control hand-over only refresh the state.
                        Ok(TerminalClientMsg::Hello { .. } | TerminalClientMsg::Resize { .. }
                            | TerminalClientMsg::TakeControl) => {
                            send_json(socket, &terminal.state()).await?;
                        }
                        Err(_) => {
                            send_json(socket, &TerminalServerMsg::Error {
                                code: "invalid".into(),
                                message: "unrecognized terminal message".into(),
                            }).await?;
                        }
                    }
                }
                Some(Ok(Message::Close(_))) | None | Some(Err(_)) => return Ok(()),
                Some(Ok(_)) => {}
            },
            frame = sub.rx.recv() => match frame {
                Ok(HubFrame::Output { seq, bytes }) => send_binary(socket, KIND_OUTPUT, seq, &bytes).await?,
                Ok(HubFrame::Snapshot { seq, bytes }) => send_binary(socket, KIND_SNAPSHOT, seq, &bytes).await?,
                Ok(HubFrame::Notice(text)) => send_json(socket, &TerminalServerMsg::Notice { text }).await?,
                Ok(HubFrame::State) => send_json(socket, &terminal.state()).await?,
                Ok(HubFrame::Exit { status, code }) => {
                    send_json(socket, &terminal.state()).await?;
                    send_json(socket, &TerminalServerMsg::Exit { status, code }).await?;
                }
                Err(RecvError::Lagged(_)) => {
                    let _ = socket.send(Message::Close(Some(CloseFrame {
                        code: CLOSE_LAGGING,
                        reason: "viewer lagged".into(),
                    }))).await;
                    return Ok(());
                }
                Err(RecvError::Closed) => return Ok(()),
            },
        }
    }
}

pub async fn refresh_usage(
    State(app): State<AppState>,
    Auth(ctx): Auth,
    Path(id): Path<String>,
) -> ApiResult<Json<AccountView>> {
    Ok(Json(app.refresh_account_usage(&ctx, &id).await?))
}
