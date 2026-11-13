//! Mapping application errors to HTTP responses (`{"error": {"code", "message"}}`).

use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use yulius_app::error::AppError;
use yulius_contracts::api::common::{ErrorBody, ErrorDetail};

/// HTTP error wrapper.
#[derive(Debug)]
pub struct ApiError(pub AppError);

impl From<AppError> for ApiError {
    fn from(err: AppError) -> Self {
        Self(err)
    }
}

/// Status code for an application error: the one table of the server, for the public API, the
/// remote MCP endpoint and the loopback listener the agents' own clients call alike. A client
/// reads the kind of an answer from it (`yulius wait` takes 502 to 504 for something that
/// cannot be reached and waits it out, and any other error for an answer), so the same error
/// must not have two statuses.
#[must_use]
pub fn status_for(err: &AppError) -> StatusCode {
    match err {
        AppError::Unauthenticated => StatusCode::UNAUTHORIZED,
        AppError::Forbidden(_) | AppError::CsrfInvalid => StatusCode::FORBIDDEN,
        AppError::NotFound => StatusCode::NOT_FOUND,
        AppError::Conflict(_) | AppError::AccountRequired(_) => StatusCode::CONFLICT,
        AppError::Invalid(_) => StatusCode::UNPROCESSABLE_ENTITY,
        AppError::Capacity(_) => StatusCode::TOO_MANY_REQUESTS,
        AppError::Upstream(_) => StatusCode::BAD_GATEWAY,
        AppError::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        if let AppError::Internal(detail) = &self.0 {
            tracing::error!(%detail, "internal error");
        }
        let body = ErrorBody {
            error: ErrorDetail {
                code: self.0.code().to_owned(),
                message: self.0.public_message(),
            },
        };
        (status_for(&self.0), Json(body)).into_response()
    }
}

/// Handler result.
pub type ApiResult<T> = Result<T, ApiError>;

#[cfg(test)]
mod tests {
    use super::*;

    /// SPEC-483 QB1: the table, whole. A failure inside Yulius is 500, never 502, which the
    /// agents' clients read as "cannot be reached".
    #[test]
    fn every_application_error_has_one_status() {
        let text = || "why".to_owned();
        for (err, status) in [
            (AppError::Unauthenticated, 401),
            (AppError::Forbidden(text()), 403),
            (AppError::CsrfInvalid, 403),
            (AppError::NotFound, 404),
            (AppError::Conflict(text()), 409),
            (AppError::AccountRequired(text()), 409),
            (AppError::Invalid(text()), 422),
            (AppError::Capacity(text()), 429),
            (AppError::Upstream(text()), 502),
            (AppError::Internal(text()), 500),
        ] {
            assert_eq!(status_for(&err).as_u16(), status, "{err:?}");
        }
    }

    /// SPEC-483 QB1: no route keeps a table of its own. The loopback listener had one
    /// (`Invalid` 400, everything else 502) and the remote MCP endpoint another, beside this
    /// one: the same error had three statuses.
    #[test]
    fn no_route_maps_an_application_error_to_a_status_itself() -> std::io::Result<()> {
        let routes = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/routes");
        let mut own = Vec::new();
        let mut folders = vec![routes];
        while let Some(folder) = folders.pop() {
            for entry in std::fs::read_dir(folder)? {
                let path = entry?.path();
                if path.is_dir() {
                    folders.push(path);
                    continue;
                }
                let source = std::fs::read_to_string(&path)?;
                for (number, line) in source.lines().enumerate() {
                    let arm = line.trim_start();
                    if arm.starts_with("AppError::") && arm.contains("=> StatusCode::") {
                        own.push(format!("{}:{}: {arm}", path.display(), number + 1));
                    }
                }
            }
        }
        assert!(own.is_empty(), "{own:#?}");
        Ok(())
    }
}
