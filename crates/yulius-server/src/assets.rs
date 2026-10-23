//! Embedded web UI assets with SPA fallback to `index.html`. Every asset carries an ETag (a hash
//! of its bytes, fixed at build time): `no-cache` files are revalidated with one conditional
//! request (304, no body) instead of being downloaded again on every page load.

use axum::http::{HeaderMap, StatusCode, Uri, header};
use axum::response::{Html, IntoResponse, Redirect, Response};

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

/// A query component decoded as the page's `URLSearchParams` does: `+` is a space, `%xx` a byte.
fn form_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while let Some(&byte) = bytes.get(i) {
        let hex = bytes
            .get(i + 1..i + 3)
            // Two hex digits exactly: `from_str_radix` would also take a sign (`%+1`).
            .filter(|pair| pair.iter().all(u8::is_ascii_hexdigit))
            .and_then(|pair| std::str::from_utf8(pair).ok())
            .and_then(|pair| u8::from_str_radix(pair, 16).ok());
        match (byte, hex) {
            (b'%', Some(decoded)) => {
                out.push(decoded);
                i += 3;
            }
            (b'+', _) => {
                out.push(b' ');
                i += 1;
            }
            _ => {
                out.push(byte);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Where a page link in mock mode goes instead: the same path without the `mock` parameter. The
/// page is in mock mode when its first `mock` is `1` (app.js); the mock is the e2e suite's
/// backend, served from the source tree, and a real instance does not ship it (ADR-0206).
fn without_mock(uri: &Uri) -> Option<String> {
    let query = uri.query()?;
    let is_mock = |pair: &&str| form_decode(pair.split('=').next().unwrap_or_default()) == "mock";
    let first = query.split('&').find(is_mock)?;
    if form_decode(first.split_once('=').map_or("", |(_, value)| value)) != "1" {
        return None;
    }
    // One leading slash: `//host` or `/\host` would send the browser to another site.
    let path = format!("/{}", uri.path().trim_start_matches(['/', '\\']));
    let rest: Vec<&str> = query
        .split('&')
        .filter(|pair| !pair.is_empty() && !is_mock(pair))
        .collect();
    Some(if rest.is_empty() {
        path
    } else {
        format!("{path}?{}", rest.join("&"))
    })
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
    if !path.starts_with("assets/")
        && let Some(location) = without_mock(&uri)
    {
        return Redirect::to(&location).into_response();
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

#[cfg(test)]
mod tests {
    use super::form_decode;

    #[test]
    fn form_decode_reads_a_query_component_as_url_search_params_does() {
        for (text, decoded) in [
            ("mock", "mock"),
            ("%6Dock", "mock"),
            ("%6dock", "mock"),
            ("a+b", "a b"),
            ("%2B", "+"),
            ("%%31", "%1"),
            ("%", "%"),
            ("%4", "%4"),
            ("%zz", "%zz"),
            ("%+1", "% 1"),
            ("%C3", "\u{FFFD}"),
            ("%C3%A9", "é"),
        ] {
            assert_eq!(form_decode(text), decoded, "{text}");
        }
    }
}
