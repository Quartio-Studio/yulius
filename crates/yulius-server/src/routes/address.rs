//! The instance's address (SPEC-466): whether GitHub accepts its sign-in callback and what the
//! last move did, for Settings → Instance, and the loopback route root's commands ask with the
//! key of `yulius leftovers`.

use axum::Json;
use axum::extract::{Query, State};
use axum::http::HeaderMap;
use serde::Deserialize;
use yulius_app::auth::rfc3339;
use yulius_app::github_callback::{AddressView, CallbackFacts, CallbackState};
use yulius_contracts::api::admin::{
    GitHubCallback, GitHubCallbackAnswer, InstanceAddress, InstanceMoved,
};

use crate::AppState;
use crate::auth::Auth;
use crate::error::ApiResult;

fn callback(facts: CallbackFacts) -> GitHubCallback {
    let (state, reason) = match facts.state {
        CallbackState::Registered => ("registered", None),
        CallbackState::Missing => ("missing", None),
        CallbackState::Unknown(why) => ("unknown", Some(why)),
    };
    GitHubCallback {
        state: state.to_owned(),
        reason,
        callback_url: facts.callback_url,
        homepage_url: facts.homepage_url,
        settings_url: facts.settings_url,
        checked_at: facts.checked_at.map(rfc3339),
    }
}

fn address(view: AddressView) -> InstanceAddress {
    InstanceAddress {
        public_url: view.public_url,
        github: view.github.map(callback),
        moved: view.moved.map(|moved| InstanceMoved {
            at: rfc3339(moved.when()),
            from: moved.from,
            to: moved.to,
            queued: moved.queued,
            left: moved.left,
            failed: moved.failed,
        }),
    }
}

/// `GET /api/admin/instance/address`.
pub async fn view(
    State(app): State<AppState>,
    Auth(ctx): Auth,
) -> ApiResult<Json<InstanceAddress>> {
    Ok(Json(address(app.instance_address(&ctx).await?)))
}

/// `POST /api/admin/instance/address/check`: asks GitHub now.
pub async fn check(
    State(app): State<AppState>,
    Auth(ctx): Auth,
) -> ApiResult<Json<InstanceAddress>> {
    Ok(Json(address(app.check_instance_address(&ctx).await?)))
}

/// Query of `GET /internal/github-callback`.
#[derive(Deserialize)]
pub struct CallbackQuery {
    /// The public URL to ask about (a move's new one); the instance's own without it.
    public_url: Option<String>,
}

/// `GET /internal/github-callback` (loopback, `yulius instance migrate`): asks GitHub now.
pub async fn cli_callback(
    State(app): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<CallbackQuery>,
) -> ApiResult<Json<GitHubCallbackAnswer>> {
    super::leftovers::cli_key(&app, &headers)?;
    let facts = app
        .check_github_callback(query.public_url.as_deref(), 0)
        .await?;
    Ok(Json(GitHubCallbackAnswer {
        github: facts.map(callback),
    }))
}
