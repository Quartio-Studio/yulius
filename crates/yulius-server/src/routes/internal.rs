//! Internal agent endpoints: `POST /hooks/{harness}/{event}` (also Claude Code's status line,
//! `event` = `StatusLine`) and `POST /internal/git-credential`
//! (task-token bearer auth, loopback listener only).

use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use yulius_app::error::AppError;
use yulius_app::hooks::HookReply;
use yulius_contracts::hooks::{HookInput, StatusLineInput, events};

use crate::AppState;

fn bearer(headers: &HeaderMap) -> Option<&str> {
    headers
        .get(header::AUTHORIZATION)?
        .to_str()
        .ok()?
        .strip_prefix("Bearer ")
        .map(str::trim)
}

fn status_of(err: &AppError) -> StatusCode {
    match err {
        AppError::Unauthenticated => StatusCode::UNAUTHORIZED,
        AppError::Forbidden(_) => StatusCode::FORBIDDEN,
        AppError::NotFound => StatusCode::NOT_FOUND,
        AppError::Invalid(_) => StatusCode::BAD_REQUEST,
        AppError::Conflict(_) => StatusCode::CONFLICT,
        _ => StatusCode::BAD_GATEWAY,
    }
}

/// Hook forwarding: the body is the CLI's hook JSON; the response body is printed by the hook
/// client as the hook's stdout (empty = no decision). `202 Accepted` with `Retry-After` holds
/// the agent at its boundary: the hook client sends the same request again after that many
/// seconds (subscription pool pause, SPEC-002 §7.5).
pub async fn hook(
    State(app): State<AppState>,
    Path((harness, event)): Path<(String, String)>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    let Some(token) = bearer(&headers) else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    // Claude Code's status line runs the same client; its input has its own shape and its answer
    // is the line's text (HRN-005; the plan usage it carries feeds the pool, ADR-17).
    if event == events::STATUS_LINE {
        let input: StatusLineInput = serde_json::from_slice(&body).unwrap_or_default();
        return match app.status_line(token, &harness, &input).await {
            Ok(line) => (
                StatusCode::OK,
                [(header::CONTENT_TYPE, "text/plain; charset=utf-8")],
                line,
            )
                .into_response(),
            Err(err) => status_of(&err).into_response(),
        };
    }
    let input: HookInput = serde_json::from_slice(&body).unwrap_or_default();
    match app.hook_event(token, &harness, &event, &input).await {
        Ok(HookReply::Hold { retry_after_secs }) => (
            StatusCode::ACCEPTED,
            [(header::RETRY_AFTER, retry_after_secs.to_string())],
        )
            .into_response(),
        Ok(HookReply::Output(output)) if output.is_empty() => StatusCode::OK.into_response(),
        Ok(HookReply::Output(output)) => Json(output).into_response(),
        Err(err) => {
            tracing::debug!(error = %err, %harness, %event, "hook refused");
            status_of(&err).into_response()
        }
    }
}

#[derive(Deserialize)]
pub struct CredentialRequest {
    #[serde(default)]
    protocol: String,
    #[serde(default)]
    host: String,
    #[serde(default)]
    path: Option<String>,
}

pub async fn git_credential(
    State(app): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<CredentialRequest>,
) -> Response {
    let Some(token) = bearer(&headers) else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    if request.protocol != "https" {
        return StatusCode::FORBIDDEN.into_response();
    }
    // User shells and editors carry workspace tokens; agents carry task tokens.
    let credential = if token.starts_with("ws_") {
        app.workspace_git_credential(token, &request.host, request.path.as_deref())
            .await
    } else {
        app.agent_git_credential(token, &request.host, request.path.as_deref())
            .await
    };
    match credential {
        Ok(credential) => {
            Json(serde_json::json!({"token": credential.token.expose()})).into_response()
        }
        Err(err) => {
            tracing::info!(error = %err, "git credential refused");
            status_of(&err).into_response()
        }
    }
}

#[derive(serde::Deserialize)]
pub struct WelcomeQuery {
    sandbox: String,
}

fn escape(text: &str) -> String {
    text.chars()
        .map(|c| match c {
            '<' => "&lt;".to_owned(),
            '>' => "&gt;".to_owned(),
            '&' => "&amp;".to_owned(),
            '"' => "&quot;".to_owned(),
            '\'' => "&#39;".to_owned(),
            c => c.to_string(),
        })
        .collect()
}

/// `GET /browser/welcome?sandbox=<id>` (loopback): the page new browser tabs open (BRW-006).
pub async fn browser_welcome(
    State(app): State<AppState>,
    axum::extract::Query(query): axum::extract::Query<WelcomeQuery>,
) -> Response {
    let Ok((slug, first, last, open)) = app.sandbox_listening_ports(&query.sandbox).await else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let ports: String = if open.is_empty() {
        format!(
            "<p class=\"muted\">No dev server is listening on ports {first}–{last} yet. Start one in a shell or ask an agent — for example <code>PORT={first} npm run dev</code>.</p>"
        )
    } else {
        open.iter()
            .map(|port| format!("<a class=\"port\" href=\"http://localhost:{port}/\"><b>localhost:{port}</b><span>open</span></a>"))
            .collect()
    };
    let sandbox = escape(&slug);
    let sandbox_id = escape(&query.sandbox);
    let html = format!(
        r#"<!doctype html>
<html lang="en"><head><meta charset="utf-8"><title>Welcome · {sandbox}</title>
<meta name="viewport" content="width=device-width, initial-scale=1">
<style>
:root {{ color-scheme: dark; }}
body {{ margin: 0; min-height: 100vh; background: #0a0c10; color: #d8dee9; font: 15px/1.5 -apple-system, BlinkMacSystemFont, "Segoe UI", Inter, sans-serif; display: flex; align-items: flex-start; justify-content: center; }}
main {{ width: min(720px, 92vw); padding: 56px 0; }}
.logo {{ font-weight: 800; font-size: 13px; letter-spacing: .14em; text-transform: uppercase; color: #79b8ff; }}
h1 {{ font-size: 28px; margin: 6px 0 4px; color: #f0f6fc; }}
.muted {{ color: #8b949e; }}
section {{ margin-top: 28px; padding: 18px 20px; border: 1px solid #21262d; border-radius: 12px; background: #0d1117; }}
h2 {{ font-size: 13px; text-transform: uppercase; letter-spacing: .08em; color: #8b949e; margin: 0 0 12px; }}
.port {{ display: flex; justify-content: space-between; align-items: center; padding: 10px 14px; margin: 6px 0; border-radius: 8px; background: #161b22; color: #f0f6fc; text-decoration: none; border: 1px solid #30363d; }}
.port:hover {{ border-color: #539bf5; }}
.port span {{ color: #79b8ff; font-size: 13px; }}
ul {{ margin: 0; padding-left: 18px; }} li {{ margin: 4px 0; }}
code {{ background: #161b22; padding: 1px 6px; border-radius: 5px; font-size: 13px; }}
</style></head>
<body><main>
<div class="logo">Yulius</div>
<h1>Browser of {sandbox}</h1>
<p class="muted">The sandbox Chrome on the server: agents browse with it, and people watch it from the browser panel.</p>
<section><h2>Running in this sandbox</h2><div id="ports">{ports}</div></section>
<section><h2>Tips</h2><ul>
<li>Type any address in the bar above; <code>localhost</code> means this sandbox's machine.</li>
<li>Agents use this browser. When one clicks or types, viewers see its cursor.</li>
<li>Logins (for example AWS) stay in this sandbox's browser profile. Your own live tabs in the panel have their own.</li>
</ul></section>
</main>
<script>
setInterval(async () => {{
  try {{
    const r = await fetch('/browser/welcome/ports?sandbox={sandbox_id}', {{ cache: 'no-store' }});
    if (r.ok) document.getElementById('ports').innerHTML = await r.text();
  }} catch (_) {{}}
}}, 5000);
</script>
</body></html>"#
    );
    (
        [
            (header::CONTENT_TYPE, "text/html; charset=utf-8"),
            (header::CACHE_CONTROL, "no-store"),
        ],
        html,
    )
        .into_response()
}

/// `GET /browser/welcome/ports?sandbox=<id>` (loopback): the port list of the welcome page.
pub async fn browser_welcome_ports(
    State(app): State<AppState>,
    axum::extract::Query(query): axum::extract::Query<WelcomeQuery>,
) -> Response {
    let Ok((_, first, last, open)) = app.sandbox_listening_ports(&query.sandbox).await else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let body: String = if open.is_empty() {
        format!(
            "<p class=\"muted\">No dev server is listening on ports {first}–{last} yet. Start one in a shell or ask an agent — for example <code>PORT={first} npm run dev</code>.</p>"
        )
    } else {
        open.iter()
            .map(|port| format!("<a class=\"port\" href=\"http://localhost:{port}/\"><b>localhost:{port}</b><span>open</span></a>"))
            .collect()
    };
    (
        [
            (header::CONTENT_TYPE, "text/html; charset=utf-8"),
            (header::CACHE_CONTROL, "no-store"),
        ],
        body,
    )
        .into_response()
}
