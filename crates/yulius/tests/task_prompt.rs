//! A task's original prompt (SPEC-043): the text it started with and the files attached to it,
//! read on demand by whoever sees the task; an attached file is served only from the sandbox's
//! attachments folder, never by a path the prompt or the client names.

use crate::support;

use std::path::PathBuf;

use axum::body::Body;
use axum::http::{HeaderMap, Method, Request, StatusCode, header};
use http_body_util::BodyExt as _;
use support::{
    Creds, Harness, TestResult, call, harness_configured, perms, seed_github_user, seed_sandbox,
    seed_task_with_prompt,
};
use tower::ServiceExt as _;
use yulius_domain::task::TaskStatus;

/// Largest file served in these tests (the upload limit).
const LIMIT: u64 = 1024;

/// Alice (write), Bob (read) and Carol (no access) on sandbox `s1`, whose task `t1` was started
/// with `prompt(drafts)`; `drafts` is Alice's composer folder, holding `shot.png` and `notes.txt`.
async fn seeded(
    prompt: impl FnOnce(&str) -> String,
) -> Result<(Harness, PathBuf, [Creds; 3]), Box<dyn std::error::Error>> {
    let h = harness_configured(|c| c.upload_max_bytes = LIMIT)?;
    let alice = seed_github_user(&h, "u-alice", "alice").await?;
    let bob = seed_github_user(&h, "u-bob", "bob").await?;
    let carol = seed_github_user(&h, "u-carol", "carol").await?;
    seed_sandbox(&h, "s1", "fixture", "u-alice").await?;
    h.github.set_perm("alice", "acme/fixture", perms("write"));
    h.github.set_perm("bob", "acme/fixture", perms("read"));
    let data = h.app.config.data_dir.clone();
    std::fs::create_dir_all(data.join("sandboxes/s1/repo"))?;
    let drafts = data.join("sandboxes/s1/attachments/drafts/u-alice");
    std::fs::create_dir_all(&drafts)?;
    // The path as an upload reports it (resolved), as the composer puts it in the prompt.
    let drafts = std::fs::canonicalize(drafts)?;
    std::fs::write(drafts.join("shot.png"), b"\x89PNG shot")?;
    std::fs::write(drafts.join("notes.txt"), b"<script>alert(1)</script>")?;
    let text = prompt(&drafts.to_string_lossy());
    seed_task_with_prompt(&h, "t1", "s1", "u-alice", TaskStatus::Running, None, &text).await?;
    Ok((h, drafts, [alice, bob, carol]))
}

#[tokio::test]
async fn the_original_prompt_is_its_text_and_attached_files_for_whoever_sees_the_task() -> TestResult
{
    let (h, _, [alice, bob, carol]) = seeded(|d| {
        format!(
            "Fix the **login**\n\n- keep the form\n\n@{d}/shot.png\n@'{d}/notes.txt'\n@/etc/hosts"
        )
    })
    .await?;

    for who in [&alice, &bob] {
        let prompt = call(&h.router, Method::GET, "/api/tasks/t1/prompt", None, who).await?;
        assert_eq!(prompt.status, StatusCode::OK, "{:?}", prompt.body);
        assert_eq!(prompt.body["text"], "Fix the **login**\n\n- keep the form");
        assert_eq!(
            prompt.body["attachments"],
            serde_json::json!([
                {"name": "shot.png", "image": true, "available": true},
                {"name": "notes.txt", "image": false, "available": true},
                // Mentioned by hand, not an upload: listed, never served.
                {"name": "hosts", "image": false, "available": false},
            ])
        );
    }
    let hidden = call(&h.router, Method::GET, "/api/tasks/t1/prompt", None, &carol).await?;
    assert_eq!(hidden.status, StatusCode::NOT_FOUND, "{:?}", hidden.body);
    let unknown = call(
        &h.router,
        Method::GET,
        "/api/tasks/nope/prompt",
        None,
        &alice,
    )
    .await?;
    assert_eq!(unknown.status, StatusCode::NOT_FOUND);
    Ok(())
}

#[tokio::test]
async fn a_prompt_without_attachments_or_whose_files_are_gone() -> TestResult {
    let (h, drafts, [alice, ..]) = seeded(|d| format!("@{d}/shot.png")).await?;
    std::fs::remove_file(drafts.join("shot.png"))?;
    let prompt = call(&h.router, Method::GET, "/api/tasks/t1/prompt", None, &alice).await?;
    assert_eq!(prompt.status, StatusCode::OK, "{:?}", prompt.body);
    assert_eq!(prompt.body["text"], "");
    assert_eq!(
        prompt.body["attachments"],
        serde_json::json!([{"name": "shot.png", "image": true, "available": false}])
    );
    Ok(())
}

/// GETs `path` as `creds`: the status, the headers and the body's bytes.
async fn get_raw(
    h: &Harness,
    path: &str,
    creds: &Creds,
) -> Result<(StatusCode, HeaderMap, Vec<u8>), Box<dyn std::error::Error>> {
    let mut req = Request::builder().method(Method::GET).uri(path);
    if let Some(cookie) = &creds.cookie {
        req = req.header(header::COOKIE, support::cookie_header(cookie));
    }
    if let Some(token) = &creds.bearer {
        req = req.header(header::AUTHORIZATION, format!("Bearer {token}"));
    }
    let req = req.body(Body::empty())?;
    let resp = h.router.clone().oneshot(req).await?;
    let status = resp.status();
    let headers = resp.headers().clone();
    let bytes = resp.into_body().collect().await?.to_bytes().to_vec();
    Ok((status, headers, bytes))
}

#[tokio::test]
async fn an_attached_file_is_served_only_from_the_attachments_folder() -> TestResult {
    let (h, drafts, [alice, bob, carol]) = seeded(|d| {
        format!(
            "Look\n@{d}/shot.png\n@{d}/notes.txt\n@/etc/hosts\n@{d}/out.png\n@{d}/big.png\n@{d}\n@{d}/gone.png"
        )
    })
    .await?;
    // A link in the attachments folder to a file outside it, a file over the limit, a folder
    // (the drafts folder itself) and a file removed since.
    let outside = drafts.join("../../../outside.png");
    std::fs::write(&outside, b"secret")?;
    std::os::unix::fs::symlink(&outside, drafts.join("out.png"))?;
    std::fs::write(
        drafts.join("big.png"),
        vec![0u8; usize::try_from(LIMIT)? + 1],
    )?;
    let url = |n: usize| format!("/api/tasks/t1/prompt/attachments/{n}");

    for who in [&alice, &bob] {
        let (status, headers, body) = get_raw(&h, &url(0), who).await?;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body, b"\x89PNG shot");
        assert_eq!(headers[header::CONTENT_TYPE], "image/png");
        assert_eq!(headers[header::X_CONTENT_TYPE_OPTIONS], "nosniff");
        let csp = headers[header::CONTENT_SECURITY_POLICY].to_str()?;
        assert!(csp.split(';').any(|d| d.trim() == "sandbox"), "{csp}");
    }
    // Not an image: downloaded, never shown as a page.
    let (status, headers, body) = get_raw(&h, &url(1), &alice).await?;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, b"<script>alert(1)</script>");
    assert_eq!(headers[header::CONTENT_TYPE], "application/octet-stream");
    assert_eq!(
        headers[header::CONTENT_DISPOSITION],
        "attachment; filename=\"notes.txt\""
    );

    for n in [2, 3, 4, 5, 6, 9] {
        let (status, _, body) = get_raw(&h, &url(n), &alice).await?;
        assert_eq!(status, StatusCode::NOT_FOUND, "attachment {n}");
        assert_ne!(body, b"secret");
    }
    let (status, _, _) = get_raw(&h, &url(0), &carol).await?;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let listed = call(&h.router, Method::GET, "/api/tasks/t1/prompt", None, &alice).await?;
    let available: Vec<bool> = listed.body["attachments"]
        .as_array()
        .ok_or("attachments")?
        .iter()
        .map(|a| a["available"] == true)
        .collect();
    assert_eq!(available, [true, true, false, false, false, false, false]);
    Ok(())
}
