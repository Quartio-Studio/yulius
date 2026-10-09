//! Embedded web UI assets with SPA fallback to `index.html`. Every asset carries an ETag (a hash
//! of its bytes, fixed at build time): `no-cache` files are revalidated with one conditional
//! request (304, no body) instead of being downloaded again on every page load.

use axum::http::{HeaderMap, StatusCode, Uri, header};
use axum::response::{Html, IntoResponse, Response};

include!(concat!(env!("OUT_DIR"), "/assets.rs"));

const PLACEHOLDER: &str = "<!doctype html><html><head><meta charset=\"utf-8\"><title>Yulius</title></head>\
<body style=\"font-family:system-ui;background:#0d1117;color:#e6edf3\"><h1>Yulius</h1>\
<p>The web UI assets are not embedded in this build.</p></body></html>";

type Asset = (&'static str, &'static [u8], &'static str, &'static str);

fn find(path: &str) -> Option<&'static Asset> {
    ASSETS.iter().find(|(p, _, _, _)| *p == path)
}

/// Whether the request's `If-None-Match` names `etag` (or `*`).
fn not_modified(headers: &HeaderMap, etag: &str) -> bool {
    headers
        .get_all(header::IF_NONE_MATCH)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(','))
        .map(|tag| tag.trim().trim_start_matches("W/"))
        .any(|tag| tag == etag || tag == "*")
}

/// The asset, or 304 when the browser's copy is current.
fn respond(headers: &HeaderMap, asset: &'static Asset, cache: &'static str) -> Response {
    let (_, bytes, mime, etag) = *asset;
    if not_modified(headers, etag) {
        return (
            StatusCode::NOT_MODIFIED,
            [(header::ETAG, etag), (header::CACHE_CONTROL, cache)],
        )
            .into_response();
    }
    (
        [
            (header::CONTENT_TYPE, mime),
            (header::CACHE_CONTROL, cache),
            (header::ETAG, etag),
        ],
        bytes,
    )
        .into_response()
}

/// Serves `/assets/...` files and the SPA shell for any other non-API path.
pub async fn serve(uri: Uri, headers: HeaderMap) -> Response {
    let path = uri.path().trim_start_matches('/');
    if path.starts_with("api/") {
        return (
            StatusCode::NOT_FOUND,
            axum::Json(serde_json::json!({"error": {"code": "not_found", "message": "not found"}})),
        )
            .into_response();
    }
    if let Some(asset) = find(path).filter(|_| !path.is_empty()) {
        let cache = if path.starts_with("assets/vendor/") {
            "public, max-age=86400"
        } else {
            "no-cache"
        };
        return respond(&headers, asset, cache);
    }
    if path.starts_with("assets/") {
        return StatusCode::NOT_FOUND.into_response();
    }
    match find("index.html") {
        Some(asset) => respond(&headers, asset, "no-cache"),
        None => Html(PLACEHOLDER).into_response(),
    }
}
