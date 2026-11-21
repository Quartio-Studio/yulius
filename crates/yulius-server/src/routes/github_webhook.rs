//! GitHub signs the raw delivery body. Its open-PR event supplies the CI discovery metadata.

use axum::body::Bytes;
use axum::extract::{FromRequest, Request, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use yulius_app::ci::PullOpening;
use yulius_app::error::AppError;

use crate::AppState;
use crate::error::ApiResult;

#[derive(Deserialize)]
struct Id {
    id: u64,
}

#[derive(Deserialize)]
struct Repository {
    id: u64,
    full_name: String,
}

#[derive(Deserialize)]
struct Branch {
    #[serde(rename = "ref")]
    name: String,
    repo: Option<Id>,
}

#[derive(Deserialize)]
struct Head {
    #[serde(flatten)]
    branch: Branch,
    sha: String,
}

#[derive(Deserialize)]
struct Pull {
    state: String,
    created_at: String,
    head: Head,
    base: Branch,
}

#[derive(Deserialize)]
struct Delivery {
    action: String,
    installation: Id,
    repository: Repository,
    number: u64,
    pull_request: Pull,
}

pub async fn receive(
    State(app): State<AppState>,
    headers: HeaderMap,
    request: Request,
) -> ApiResult<Response> {
    let permit = app.ci_webhook_permit()?;
    let mut tag = [0; 32];
    let signature = headers
        .get("x-hub-signature-256")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("sha256="))
        .ok_or(AppError::Unauthenticated)?;
    hex::decode_to_slice(signature, &mut tag).map_err(|_| AppError::Unauthenticated)?;
    let body = match tokio::time::timeout(
        std::time::Duration::from_secs(3),
        Bytes::from_request(request, &app),
    )
    .await
    {
        Ok(Ok(bytes)) => bytes,
        Ok(Err(rejection)) => return Ok(rejection.into_response()),
        Err(_) => return Ok(StatusCode::REQUEST_TIMEOUT.into_response()),
    };
    let stored;
    let secret = if let Some(key) = &app.config.github_webhook_secret {
        key.expose()
    } else {
        stored = app.github_app().await?.webhook_secret;
        stored
            .as_deref()
            .filter(|key| !key.is_empty())
            .ok_or(AppError::Unauthenticated)?
    };
    let key = ring::hmac::Key::new(ring::hmac::HMAC_SHA256, secret.as_bytes());
    ring::hmac::verify(&key, &body, &tag).map_err(|_| AppError::Unauthenticated)?;
    if headers
        .get("x-github-event")
        .is_none_or(|event| event != "pull_request")
    {
        return Ok(StatusCode::NO_CONTENT.into_response());
    }
    let event: Delivery = serde_json::from_slice(&body)
        .map_err(|_| AppError::Invalid("invalid pull request event".into()))?;
    let pull = event.pull_request;
    if event.action != "opened"
        || pull.state != "open"
        || pull.base.repo.as_ref().map(|repo| repo.id) != Some(event.repository.id)
        || pull.head.branch.repo.as_ref().map(|repo| repo.id) != Some(event.repository.id)
    {
        return Ok(StatusCode::NO_CONTENT.into_response());
    }
    app.ci_pull_opened(
        PullOpening {
            installation: event.installation.id,
            repository: event.repository.full_name,
            repo_id: event.repository.id,
            number: event.number,
            base: pull.base.name,
            branch: pull.head.branch.name,
            sha: pull.head.sha,
            created_at: pull.created_at,
        },
        permit,
    )
    .await?;
    Ok(StatusCode::ACCEPTED.into_response())
}
