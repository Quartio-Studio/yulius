//! Superadmin user administration.

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::Redirect;
use yulius_contracts::api::admin::{
    AdminUser, AdmitUser, GitHubStatus, InstanceSettings, ManifestCallback, ManifestRequest,
    ManifestResponse, UpdateUser,
};

use crate::AppState;
use crate::auth::Auth;
use crate::error::ApiResult;

pub async fn list_users(
    State(app): State<AppState>,
    Auth(ctx): Auth,
) -> ApiResult<Json<Vec<AdminUser>>> {
    Ok(Json(app.admin_users(&ctx).await?))
}

pub async fn admit_user(
    State(app): State<AppState>,
    Auth(ctx): Auth,
    Json(req): Json<AdmitUser>,
) -> ApiResult<(StatusCode, Json<AdminUser>)> {
    Ok((StatusCode::CREATED, Json(app.admit_user(&ctx, &req).await?)))
}

pub async fn update_user(
    State(app): State<AppState>,
    Auth(ctx): Auth,
    Path(id): Path<String>,
    Json(req): Json<UpdateUser>,
) -> ApiResult<Json<AdminUser>> {
    Ok(Json(app.update_user(&ctx, &id, &req).await?))
}

pub async fn github(State(app): State<AppState>, Auth(ctx): Auth) -> ApiResult<Json<GitHubStatus>> {
    let view = app.github_admin(&ctx).await?;
    Ok(Json(GitHubStatus {
        configured: view.configured,
        app_slug: view.app_slug,
        owner_login: view.owner_login,
        install_url: view.install_url,
        installed: view.installation_id.is_some(),
    }))
}

pub async fn github_manifest(
    State(app): State<AppState>,
    Auth(ctx): Auth,
    Json(req): Json<ManifestRequest>,
) -> ApiResult<Json<ManifestResponse>> {
    let (post_url, manifest) = app
        .github_manifest(&ctx, &req.owner_login, &req.owner_kind)
        .await?;
    Ok(Json(ManifestResponse { post_url, manifest }))
}

/// GitHub redirects the browser here after the App is created. Success continues to the App's
/// installation page; failures return to the GitHub admin page with a message.
pub async fn github_manifest_callback(
    State(app): State<AppState>,
    Auth(ctx): Auth,
    Query(q): Query<ManifestCallback>,
) -> Redirect {
    match app.github_manifest_callback(&ctx, &q.code, &q.state).await {
        Ok(install) => Redirect::to(&install),
        Err(err) => {
            let message: String = err
                .public_message()
                .chars()
                .map(|c| if c.is_ascii_alphanumeric() { c } else { ' ' })
                .collect();
            let message = message.split_whitespace().collect::<Vec<_>>().join("+");
            Redirect::to(&format!("/admin/github?error={message}"))
        }
    }
}

pub async fn settings(
    State(app): State<AppState>,
    Auth(ctx): Auth,
) -> ApiResult<Json<InstanceSettings>> {
    Ok(Json(app.instance_settings(&ctx).await?))
}

pub async fn update_settings(
    State(app): State<AppState>,
    Auth(ctx): Auth,
    Json(req): Json<InstanceSettings>,
) -> ApiResult<Json<InstanceSettings>> {
    Ok(Json(app.update_instance_settings(&ctx, &req).await?))
}
