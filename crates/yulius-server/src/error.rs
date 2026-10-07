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

/// Status code for an application error.
#[must_use]
pub fn status_for(err: &AppError) -> StatusCode {
    match err {
        AppError::Unauthenticated => StatusCode::UNAUTHORIZED,
        AppError::Forbidden(_) => StatusCode::FORBIDDEN,
        AppError::NotFound => StatusCode::NOT_FOUND,
        AppError::Conflict(_) => StatusCode::CONFLICT,
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
