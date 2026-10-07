//! Embedded web UI assets with SPA fallback to `index.html`.

use axum::http::{StatusCode, Uri, header};
use axum::response::{Html, IntoResponse, Response};

include!(concat!(env!("OUT_DIR"), "/assets.rs"));

const PLACEHOLDER: &str = "<!doctype html><html><head><meta charset=\"utf-8\"><title>Yulius</title></head>\
<body style=\"font-family:system-ui;background:#0d1117;color:#e6edf3\"><h1>Yulius</h1>\
<p>The web UI assets are not embedded in this build.</p></body></html>";

fn find(path: &str) -> Option<&'static (&'static str, &'static [u8], &'static str)> {
    ASSETS.iter().find(|(p, _, _)| *p == path)
}

/// Serves `/assets/...` files and the SPA shell for any other non-API path.
pub async fn serve(uri: Uri) -> Response {
    let path = uri.path().trim_start_matches('/');
    if path.starts_with("api/") {
        return (
            StatusCode::NOT_FOUND,
            axum::Json(serde_json::json!({"error": {"code": "not_found", "message": "not found"}})),
        )
            .into_response();
    }
    if let Some((_, bytes, mime)) = find(path).filter(|_| !path.is_empty()) {
        let cache = if path.starts_with("assets/vendor/") {
            "public, max-age=86400"
        } else {
            "no-cache"
        };
        return (
            [
                (header::CONTENT_TYPE, *mime),
                (header::CACHE_CONTROL, cache),
            ],
            *bytes,
        )
            .into_response();
    }
    if path.starts_with("assets/") {
        return StatusCode::NOT_FOUND.into_response();
    }
    match find("index.html") {
        Some((_, bytes, mime)) => (
            [
                (header::CONTENT_TYPE, *mime),
                (header::CACHE_CONTROL, "no-cache"),
            ],
            *bytes,
        )
            .into_response(),
        None => Html(PLACEHOLDER).into_response(),
    }
}
