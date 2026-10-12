//! UI ↔ server contract (docs/api.md, crates/yulius-server/web/README.md).
//!
//! The web UI is plain ES modules embedded by `crates/yulius-server/build.rs`, so nothing
//! type-checks its calls against the Rust router. These tests read the JavaScript sources at
//! test time and check that:
//!
//! - every `/api/...` path literal the UI uses (HTTP calls, WebSocket paths, links) resolves
//!   to a real route of the public router with the method the UI uses: an unauthenticated probe must reach the handler (401, or for the few public
//!   endpoints anything but 404/405) instead of the API fallback (404 `not_found`) or a
//!   405 for a wrong method;
//! - the `?mock=1` backend (`assets/mock.js`) only fakes routes the server has;
//! - the embedded assets are served with the right content types, every module the UI
//!   imports is embedded, and UI routes fall back to the SPA shell while unknown `/api/`
//!   paths and missing assets stay 404.

use crate::support;

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use axum::Router;
use axum::body::Body;
use axum::http::{Method, Request, StatusCode, header};
use http_body_util::BodyExt;
use support::{TestResult, harness};
use tower::ServiceExt;

/// Paths the UI may call without a session (handlers without the `Auth` extractor).
const PUBLIC_PATHS: &[&str] = &[
    "/api/setup/status",
    "/api/setup",
    "/api/auth/password",
    "/api/auth/recovery",
    "/api/auth/recover",
    "/api/auth/github/start",
    "/api/auth/github/callback",
    "/api/device/start",
    "/api/device/poll",
];

/// Mock-only endpoints: stand-ins for flows the mock cannot run (the GitHub OAuth redirect).
const MOCK_ONLY: &[(&str, &str)] = &[("POST", "/api/auth/mock-github")];

/// Documented in docs/api.md but not routed by the server yet. The UI handles their 404.
/// This list must not go stale: the test fails once a route appears, so the entry and the UI
/// fallback can be revisited.
const KNOWN_UNROUTED: &[(&str, &str)] = &[];

/// A path template with `{}` for each interpolated segment, e.g. `/api/tasks/{}/stop`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct Call {
    method: String,
    path: String,
    origin: String,
}

fn web_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../yulius-server/web")
}

/// UI JavaScript files (vendored xterm excluded), as (relative path, source).
fn js_sources() -> Result<Vec<(String, String)>, Box<dyn std::error::Error>> {
    fn walk(dir: &Path, root: &Path, out: &mut Vec<(String, String)>) -> std::io::Result<()> {
        let mut entries: Vec<PathBuf> = std::fs::read_dir(dir)?
            .filter_map(Result::ok)
            .map(|e| e.path())
            .collect();
        entries.sort();
        for path in entries {
            let rel = path
                .strip_prefix(root)
                .unwrap_or(&path)
                .to_string_lossy()
                .replace('\\', "/");
            if path.is_dir() {
                if rel != "assets/vendor" {
                    walk(&path, root, out)?;
                }
            } else if rel.ends_with(".js") {
                out.push((rel, std::fs::read_to_string(&path)?));
            }
        }
        Ok(())
    }
    let root = web_dir();
    let mut out = Vec::new();
    walk(&root.join("assets"), &root, &mut out)?;
    if out.len() < 10 {
        return Err(format!("expected the UI sources under {}", root.display()).into());
    }
    Ok(out)
}

/// Reads the string or template literal whose opening delimiter is at `start`. Template
/// substitutions `${…}` become `{}`. Returns the normalized text and the index after the
/// closing delimiter.
fn read_literal(src: &[u8], start: usize) -> Option<(String, usize)> {
    let quote = *src.get(start)?;
    let mut text = Vec::new();
    let mut i = start + 1;
    while let Some(&b) = src.get(i) {
        match b {
            b'\\' => {
                text.extend_from_slice(src.get(i..i + 2)?);
                i += 2;
            }
            _ if b == quote => return Some((String::from_utf8_lossy(&text).into_owned(), i + 1)),
            b'\n' if quote != b'`' => return None,
            b'$' if quote == b'`' && src.get(i + 1) == Some(&b'{') => {
                i = skip_expression(src, i + 2)?;
                text.extend_from_slice(b"{}");
            }
            _ => {
                text.push(b);
                i += 1;
            }
        }
    }
    None
}

/// Skips a `${…}` expression starting after `${`; returns the index after its `}`.
fn skip_expression(src: &[u8], mut i: usize) -> Option<usize> {
    let mut depth = 1usize;
    while let Some(&b) = src.get(i) {
        match b {
            b'\'' | b'"' | b'`' => i = read_literal(src, i)?.1,
            b'{' => {
                depth += 1;
                i += 1;
            }
            b'}' => {
                depth -= 1;
                i += 1;
                if depth == 0 {
                    return Some(i);
                }
            }
            _ => i += 1,
        }
    }
    None
}

/// Method used at a literal: `api.get|post|put|patch|del(` → that method, `path:` (WebSocket
/// paths for the socket clients) and everything else (links, iframe sources) → GET.
fn method_at(src: &[u8], start: usize) -> &'static str {
    let before = String::from_utf8_lossy(src.get(..start).unwrap_or_default());
    let trimmed = before.trim_end();
    let Some(call) = trimmed.strip_suffix('(') else {
        return "GET";
    };
    let call = call.trim_end();
    for (suffix, method) in [
        (".get", "GET"),
        (".post", "POST"),
        (".put", "PUT"),
        (".patch", "PATCH"),
        (".del", "DELETE"),
    ] {
        if call.ends_with(suffix) {
            return method;
        }
    }
    "GET"
}

fn line_of(src: &[u8], index: usize) -> usize {
    src.get(..index)
        .map_or(0, |s| s.iter().filter(|&&b| b == b'\n').count())
        + 1
}

/// Every `/api/...` literal of one source file.
fn calls_in(file: &str, source: &str) -> Vec<Call> {
    let src = source.as_bytes();
    let mut out = Vec::new();
    for (i, &b) in src.iter().enumerate() {
        if !matches!(b, b'\'' | b'"' | b'`') {
            continue;
        }
        let rest = src.get(i + 1..).unwrap_or_default();
        if !rest.starts_with(b"/api/") {
            continue;
        }
        let Some((text, _)) = read_literal(src, i) else {
            continue;
        };
        let path = text.split(['?', '#']).next().unwrap_or_default().to_owned();
        let origin = format!("{file}:{}", line_of(src, i));
        out.push(Call {
            method: method_at(src, i).to_owned(),
            path,
            origin,
        });
    }
    out
}

/// Routes the mock backend defines: `route('METHOD', '<path>' | `<path with ${ID}>`, …)`.
fn mock_routes(source: &str) -> Vec<Call> {
    let src = source.as_bytes();
    let mut out = Vec::new();
    let needle = b"route('";
    let mut i = 0;
    while let Some(pos) = src
        .get(i..)
        .and_then(|rest| rest.windows(needle.len()).position(|w| w == needle))
    {
        let start = i + pos;
        i = start + needle.len();
        let Some((method, after)) = read_literal(src, start + needle.len() - 1) else {
            continue;
        };
        let mut j = after;
        while matches!(src.get(j), Some(b',' | b' ')) {
            j += 1;
        }
        if let Some((path, _)) = read_literal(src, j)
            && path.starts_with("/api/")
        {
            out.push(Call {
                method,
                path,
                origin: format!("assets/mock.js:{}", line_of(src, start)),
            });
        }
    }
    out
}

/// Concrete probe path for a template, or an error when an interpolation is not a whole
/// path segment (such paths cannot be checked statically: keep API paths literal).
fn probe_path(template: &str) -> Result<String, String> {
    let segments: Vec<String> = template
        .split('/')
        .map(|segment| {
            if segment == "{}" {
                Ok("probe-id".to_owned())
            } else if segment.contains("{}") {
                Err(format!(
                    "`{template}` interpolates part of a path segment; keep API paths literal"
                ))
            } else {
                Ok(segment.to_owned())
            }
        })
        .collect::<Result<_, _>>()?;
    Ok(segments.join("/"))
}

struct Probe {
    status: StatusCode,
    content_type: String,
    body: Vec<u8>,
}

async fn probe(
    router: &Router,
    method: &str,
    path: &str,
) -> Result<Probe, Box<dyn std::error::Error>> {
    let request = Request::builder()
        .method(Method::from_bytes(method.as_bytes())?)
        .uri(path)
        .body(Body::empty())?;
    let response = router.clone().oneshot(request).await?;
    let status = response.status();
    let content_type = response
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_owned();
    let body = response.into_body().collect().await?.to_bytes().to_vec();
    Ok(Probe {
        status,
        content_type,
        body,
    })
}

/// Why a call does not reach a handler of the real router, if it does not.
async fn route_problem(
    router: &Router,
    call: &Call,
) -> Result<Option<String>, Box<dyn std::error::Error>> {
    let path = match probe_path(&call.path) {
        Ok(path) => path,
        Err(problem) => return Ok(Some(problem)),
    };
    let response = probe(router, &call.method, &path).await?;
    let public = PUBLIC_PATHS.contains(&call.path.as_str());
    let problem = match response.status {
        StatusCode::METHOD_NOT_ALLOWED => {
            Some("the route exists but not for this method (405)".to_owned())
        }
        StatusCode::NOT_FOUND => Some(format!(
            "no route (404: {})",
            String::from_utf8_lossy(&response.body)
        )),
        StatusCode::UNAUTHORIZED => None,
        _ if public => None,
        status => Some(format!(
            "expected the handler's 401 for an unauthenticated request, got {status}"
        )),
    };
    Ok(problem)
}

#[tokio::test]
async fn every_api_call_of_the_ui_exists_in_the_router() -> TestResult {
    let h = harness()?;
    let mut calls = BTreeSet::new();
    for (file, source) in js_sources()? {
        if file == "assets/mock.js" {
            continue;
        }
        for call in calls_in(&file, &source) {
            calls.insert(call);
        }
    }
    // The scan must see the main areas; a parser regression must not pass silently.
    for expected in [
        ("GET", "/api/sandboxes"),
        ("POST", "/api/tasks/{}/stop"),
        ("GET", "/api/tasks/{}/terminal"),
        ("GET", "/api/events"),
        ("PUT", "/api/sandboxes/{}/skills/{}"),
        ("DELETE", "/api/sandboxes/{}"),
        ("GET", "/api/sandboxes/{}/delivery/runs/{}"),
        ("GET", "/api/admin/accounts/login/{}/terminal"),
        ("PUT", "/api/sandboxes/{}/files/content"),
        ("DELETE", "/api/tasks/{}"),
    ] {
        assert!(
            calls
                .iter()
                .any(|c| (c.method.as_str(), c.path.as_str()) == expected),
            "the UI scan did not find {expected:?}"
        );
    }
    let mut problems = Vec::new();
    for call in &calls {
        let key = (call.method.as_str(), call.path.as_str());
        if MOCK_ONLY.contains(&key) || KNOWN_UNROUTED.contains(&key) {
            continue;
        }
        if let Some(problem) = route_problem(&h.router, call).await? {
            problems.push(format!(
                "{} {} ({}): {problem}",
                call.method, call.path, call.origin
            ));
        }
    }
    assert!(
        problems.is_empty(),
        "UI calls without a matching server route:\n{}",
        problems.join("\n")
    );
    Ok(())
}

#[tokio::test]
async fn mock_backend_only_fakes_real_routes() -> TestResult {
    let h = harness()?;
    let source = std::fs::read_to_string(web_dir().join("assets/mock.js"))?;
    let routes: BTreeSet<Call> = mock_routes(&source).into_iter().collect();
    assert!(
        routes.len() > 60,
        "expected the mock route table, found {}",
        routes.len()
    );
    let mut problems = Vec::new();
    for call in &routes {
        if MOCK_ONLY.contains(&(call.method.as_str(), call.path.as_str())) {
            continue;
        }
        if let Some(problem) = route_problem(&h.router, call).await? {
            problems.push(format!(
                "{} {} ({}): {problem}",
                call.method, call.path, call.origin
            ));
        }
    }
    assert!(
        problems.is_empty(),
        "mock routes the server does not have:\n{}",
        problems.join("\n")
    );
    Ok(())
}

#[tokio::test]
async fn known_unrouted_and_mock_only_paths_are_still_absent() -> TestResult {
    let h = harness()?;
    for (method, path) in KNOWN_UNROUTED.iter().chain(MOCK_ONLY) {
        let response = probe(&h.router, method, path).await?;
        assert_eq!(
            response.status,
            StatusCode::NOT_FOUND,
            "{method} {path} is routed now: remove it from the allowlist in tests/ui_contract.rs and revisit the UI fallback"
        );
        let body: serde_json::Value = serde_json::from_slice(&response.body)?;
        assert_eq!(body["error"]["code"], "not_found");
    }
    Ok(())
}

#[tokio::test]
async fn ui_routes_serve_the_shell_and_unknown_api_paths_stay_json_404() -> TestResult {
    let h = harness()?;
    let root = probe(&h.router, "GET", "/").await?;
    assert_eq!(root.status, StatusCode::OK);
    assert!(
        root.content_type.starts_with("text/html"),
        "GET / content type {}",
        root.content_type
    );
    let shell = String::from_utf8(root.body.clone())?;
    assert!(shell.contains(r#"<script type="module" src="/assets/app.js"></script>"#));

    // Route names come from router.js so a new page cannot miss the server fallback.
    let router_js = std::fs::read_to_string(web_dir().join("assets/router.js"))?;
    let set_items = |name: &str| -> Vec<String> {
        router_js
            .split_once(&format!("const {name} = new Set(["))
            .and_then(|(_, rest)| rest.split_once("])"))
            .map(|(items, _)| {
                items
                    .split(',')
                    .map(|item| item.trim().trim_matches('\'').to_owned())
                    .filter(|item| !item.is_empty())
                    .collect()
            })
            .unwrap_or_default()
    };
    let sandbox_pages = set_items("SANDBOX_PAGES");
    let admin_pages = set_items("ADMIN_PAGES");
    assert!(
        sandbox_pages.contains(&"git".to_owned()) && admin_pages.contains(&"accounts".to_owned())
    );
    let mut paths: Vec<String> = [
        "/setup",
        "/signin?next=%2Fs%2Fstudio",
        "/device?code=ABCD-EFGH",
        "/archived",
        "/settings",
        "/design",
        "/s/studio",
        "/s/studio/t/12",
        "/s/studio/browser?tab=E3C8B2A1",
        "/s/studio/skills?source=instance",
    ]
    .iter()
    .map(|p| (*p).to_owned())
    .collect();
    paths.extend(sandbox_pages.iter().map(|page| format!("/s/studio/{page}")));
    paths.extend(admin_pages.iter().map(|page| format!("/admin/{page}")));
    for path in &paths {
        let response = probe(&h.router, "GET", path).await?;
        assert_eq!(response.status, StatusCode::OK, "GET {path}");
        assert!(
            response.content_type.starts_with("text/html"),
            "GET {path}: {}",
            response.content_type
        );
        assert_eq!(
            response.body, root.body,
            "GET {path} must serve the SPA shell"
        );
    }

    let unknown = probe(&h.router, "GET", "/api/no-such-endpoint").await?;
    assert_eq!(unknown.status, StatusCode::NOT_FOUND);
    let body: serde_json::Value = serde_json::from_slice(&unknown.body)?;
    assert_eq!(body["error"]["code"], "not_found");
    for missing in ["/assets/no-such-file.js", "/assets/vendor/README.md"] {
        assert_eq!(
            probe(&h.router, "GET", missing).await?.status,
            StatusCode::NOT_FOUND,
            "GET {missing}"
        );
    }
    Ok(())
}

/// Relative module specifiers of `import … from './x.js'` and `import('./x.js')`.
fn imports_of(source: &str) -> Vec<String> {
    let mut out = Vec::new();
    for marker in ["from '", "import('"] {
        for (pos, _) in source.match_indices(marker) {
            let rest = source.get(pos + marker.len()..).unwrap_or_default();
            if let Some((spec, _)) = rest.split_once('\'')
                && spec.starts_with('.')
            {
                out.push(spec.to_owned());
            }
        }
    }
    out
}

/// Resolves `spec` relative to the asset `file` (both relative to `web/`).
fn resolve(file: &str, spec: &str) -> String {
    let mut parts: Vec<&str> = file.split('/').collect();
    parts.pop();
    for segment in spec.split('/') {
        match segment {
            "." => {}
            ".." => {
                parts.pop();
            }
            other => parts.push(other),
        }
    }
    format!("/{}", parts.join("/"))
}

#[tokio::test]
async fn embedded_assets_serve_with_content_types_and_every_import_resolves() -> TestResult {
    let h = harness()?;
    for (path, mime) in [
        ("/assets/app.js", "text/javascript"),
        ("/assets/styles.css", "text/css"),
        ("/assets/vendor/xterm/xterm.css", "text/css"),
        ("/assets/vendor/xterm/xterm.mjs", "text/javascript"),
    ] {
        let response = probe(&h.router, "GET", path).await?;
        assert_eq!(response.status, StatusCode::OK, "GET {path}");
        assert!(
            response.content_type.starts_with(mime),
            "GET {path}: {}",
            response.content_type
        );
        assert!(!response.body.is_empty(), "GET {path} is empty");
    }
    let index = std::fs::read_to_string(web_dir().join("index.html"))?;
    let mut referenced: BTreeSet<String> = BTreeSet::new();
    for marker in ["href=\"/assets/", "src=\"/assets/"] {
        for (pos, _) in index.match_indices(marker) {
            let rest = index
                .get(pos + marker.len() - "/assets/".len()..)
                .unwrap_or_default();
            if let Some((path, _)) = rest.split_once('"') {
                referenced.insert(path.to_owned());
            }
        }
    }
    for (file, source) in js_sources()? {
        for spec in imports_of(&source) {
            referenced.insert(resolve(&file, &spec));
        }
    }
    assert!(
        referenced.contains("/assets/mock.js")
            && referenced.contains("/assets/vendor/xterm/xterm.mjs")
    );
    for path in &referenced {
        let response = probe(&h.router, "GET", path).await?;
        assert_eq!(
            response.status,
            StatusCode::OK,
            "GET {path} (imported or linked by the UI)"
        );
        let expected = if path.ends_with(".css") {
            "text/css"
        } else if path.ends_with(".woff2") {
            "font/woff2"
        } else {
            "text/javascript"
        };
        assert!(
            response.content_type.starts_with(expected),
            "GET {path}: {}",
            response.content_type
        );
    }
    Ok(())
}

/// Assets carry an ETag, so a reload revalidates them (304, no body) instead of downloading
/// the UI again (F 2026-10-09: the terminal "loads for a while").
#[tokio::test]
async fn embedded_assets_revalidate_with_etags() -> TestResult {
    let h = harness()?;
    for (path, cache) in [
        ("/assets/terminal-core.js", "no-cache"),
        ("/assets/fonts/ibm-plex-mono-latin-400.woff2", "no-cache"),
        ("/assets/vendor/xterm/xterm.mjs", "public, max-age=86400"),
        ("/s/fixture/t/1", "no-cache"),
    ] {
        let get = |etag: Option<&str>| {
            let mut request = Request::builder().method(Method::GET).uri(path);
            if let Some(etag) = etag {
                request = request.header(header::IF_NONE_MATCH, etag);
            }
            request.body(Body::empty())
        };
        let first = h.router.clone().oneshot(get(None)?).await?;
        assert_eq!(first.status(), StatusCode::OK, "GET {path}");
        assert_eq!(first.headers()[header::CACHE_CONTROL], cache, "GET {path}");
        let etag = first.headers()[header::ETAG].to_str()?.to_owned();
        assert!(
            etag.starts_with('"') && etag.ends_with('"') && etag.len() == 18,
            "{etag}"
        );
        let again = h.router.clone().oneshot(get(Some(&etag))?).await?;
        assert_eq!(again.status(), StatusCode::NOT_MODIFIED, "GET {path} again");
        assert_eq!(again.headers()[header::ETAG].to_str()?, etag);
        assert!(again.into_body().collect().await?.to_bytes().is_empty());
        let changed = h
            .router
            .clone()
            .oneshot(get(Some("\"0000000000000000\""))?)
            .await?;
        assert_eq!(
            changed.status(),
            StatusCode::OK,
            "GET {path} with a stale tag"
        );
    }
    Ok(())
}

#[test]
fn literal_scanner_handles_templates_methods_and_partial_segments() -> TestResult {
    let src = "ctx.api.post(`/api/tasks/${enc(task.id)}/stop`);\n\
               const x = live ? await ctx.api.del(\n  `/api/a/${f({ b: '}' })}/c?x=${y}`) : 0;\n\
               socket({ path: '/api/events' });\n\
               el.href = `/api/analytics/tasks?${query({ format: 'csv' })}`;\n";
    let calls = calls_in("t.js", src);
    let pairs: Vec<(&str, &str)> = calls
        .iter()
        .map(|c| (c.method.as_str(), c.path.as_str()))
        .collect();
    assert_eq!(
        pairs,
        vec![
            ("POST", "/api/tasks/{}/stop"),
            ("DELETE", "/api/a/{}/c"),
            ("GET", "/api/events"),
            ("GET", "/api/analytics/tasks"),
        ]
    );
    assert_eq!(calls.first().map(|c| c.origin.as_str()), Some("t.js:1"));
    assert_eq!(
        probe_path("/api/tasks/{}/stop")?,
        "/api/tasks/probe-id/stop"
    );
    assert!(probe_path("/api/tasks/x{}").is_err());
    let mock =
        "route('GET', `/api/sandboxes/${ID}/git`, () => 1);\nroute('POST', '/api/setup', h);";
    let routes: Vec<(String, String)> = mock_routes(mock)
        .into_iter()
        .map(|c| (c.method, c.path))
        .collect();
    assert_eq!(
        routes,
        vec![
            ("GET".to_owned(), "/api/sandboxes/{}/git".to_owned()),
            ("POST".to_owned(), "/api/setup".to_owned())
        ]
    );
    Ok(())
}
