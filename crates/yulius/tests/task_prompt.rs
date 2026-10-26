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
use yulius_app::auth::AuthContext;
use yulius_app::ports::store::UserStore as _;
use yulius_app::uploads::MAX_CONCURRENT_UPLOADS;
use yulius_domain::ids::UserId;
use yulius_domain::task::TaskStatus;

/// Largest file served in these tests (the upload limit).
const LIMIT: u64 = 1024;

/// Alice (write), Bob (read) and Carol (no access) on sandbox `s1`, whose task `t1` was started
/// with `prompt(drafts)`; `drafts` is Alice's composer folder, holding `shot.png` and `notes.txt`.
async fn seeded(
    prompt: impl FnOnce(&str) -> String,
) -> Result<(Harness, PathBuf, [Creds; 3]), Box<dyn std::error::Error>> {
    seeded_with(LIMIT, prompt).await
}

/// [`seeded`] with `limit` as the upload limit.
async fn seeded_with(
    limit: u64,
    prompt: impl FnOnce(&str) -> String,
) -> Result<(Harness, PathBuf, [Creds; 3]), Box<dyn std::error::Error>> {
    let h = harness_configured(move |c| c.upload_max_bytes = limit)?;
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

/// SPEC-124 INL-008: the prompt in order, each file where it sits, so the view shows it there; an
/// older prompt's files follow its text.
#[tokio::test]
async fn the_original_prompt_keeps_each_file_at_its_place() -> TestResult {
    let (h, _, [alice, ..]) =
        seeded(|d| format!("This part @'{d}/shot.png' looks wrong.\n\n@{d}/notes.txt")).await?;
    let prompt = call(&h.router, Method::GET, "/api/tasks/t1/prompt", None, &alice).await?;
    assert_eq!(prompt.status, StatusCode::OK, "{:?}", prompt.body);
    assert_eq!(prompt.body["text"], "This part looks wrong.");
    assert_eq!(
        prompt.body["parts"],
        serde_json::json!([
            {"text": "This part "},
            {"attachment": 0},
            {"text": " looks wrong.\n\n"},
            {"attachment": 1},
        ])
    );
    assert_eq!(prompt.body["attachments"][0]["name"], "shot.png");
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
    request_raw(h, Method::GET, path, creds).await
}

/// Sends a bodiless `method` request for `path` as `creds`: the status, headers and body.
async fn request_raw(
    h: &Harness,
    method: Method,
    path: &str,
    creds: &Creds,
) -> Result<(StatusCode, HeaderMap, Vec<u8>), Box<dyn std::error::Error>> {
    let mut req = Request::builder().method(method).uri(path);
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

/// SPEC-325 UPL-003: the web UI asks with HEAD whether a file is there, then lets the browser
/// download it from its link: HEAD answers like GET, without the file.
#[tokio::test]
async fn a_head_request_says_whether_an_attached_file_is_there_without_sending_it() -> TestResult {
    let (h, _drafts, [alice, _, carol]) =
        seeded(|d| format!("Look\n@{d}/notes.txt\n@{d}/gone.mp4")).await?;
    let url = |n: usize| format!("/api/tasks/t1/prompt/attachments/{n}");
    let (status, headers, body) = request_raw(&h, Method::HEAD, &url(0), &alice).await?;
    assert_eq!(status, StatusCode::OK);
    assert!(body.is_empty(), "{body:?}");
    assert_eq!(headers[header::CONTENT_LENGTH], "25");
    assert_eq!(
        headers[header::CONTENT_DISPOSITION],
        "attachment; filename=\"notes.txt\""
    );
    let (status, _, _) = request_raw(&h, Method::HEAD, &url(1), &alice).await?;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _, _) = request_raw(&h, Method::HEAD, &url(0), &carol).await?;
    assert_eq!(status, StatusCode::NOT_FOUND);
    Ok(())
}

/// SPEC-325 review: the browser downloads the link itself, and a `Content-Disposition` filename
/// wins over the page's `download` name, so a name an upload keeps (non-ASCII letters) is sent
/// as `filename*` (RFC 6266), with an ASCII fallback, not as "attachment".
#[tokio::test]
async fn a_downloaded_file_keeps_its_non_ascii_name() -> TestResult {
    let (h, drafts, [alice, _, _]) = seeded(|d| format!("Look\n@{d}/błąd-ekranu.mp4")).await?;
    std::fs::write(drafts.join("błąd-ekranu.mp4"), b"mp4")?;
    let (status, headers, _) = get_raw(&h, "/api/tasks/t1/prompt/attachments/0", &alice).await?;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        headers[header::CONTENT_DISPOSITION],
        "attachment; filename=\"b__d-ekranu.mp4\"; filename*=UTF-8''b%C5%82%C4%85d-ekranu.mp4"
    );
    Ok(())
}

#[tokio::test]
async fn only_the_tasks_own_attachment_folders_are_served_never_through_a_link() -> TestResult {
    let (h, drafts, [alice, ..]) = seeded(|d| {
        // `d` is …/sandboxes/s1/attachments/drafts/u-alice.
        let base = d.trim_end_matches("/drafts/u-alice");
        format!(
            "Look\n@{base}/tasks/t1/term.png\n@{d}/../../../outside.png\n@{base}/drafts/u-bob/bob.png\n@{base}/shells/sh1/shell.png\n@{d}/shot.png"
        )
    })
    .await?;
    let attachments = std::fs::canonicalize(drafts.join("../.."))?;
    let sandbox = std::fs::canonicalize(attachments.join(".."))?;
    std::fs::write(sandbox.join("outside.png"), b"secret")?;
    for (folder, name) in [
        ("tasks/t1", "term.png"),
        ("drafts/u-bob", "bob.png"),
        ("shells/sh1", "shell.png"),
    ] {
        std::fs::create_dir_all(attachments.join(folder))?;
        std::fs::write(attachments.join(folder).join(name), b"\x89PNG file")?;
    }
    let url = |n: &str| format!("/api/tasks/t1/prompt/attachments/{n}");

    // The task's own terminal attachments are served; a path out of the folder, another user's
    // drafts and a shell's files are not; neither is an index that is not a number.
    let (status, _, body) = get_raw(&h, &url("0"), &alice).await?;
    assert_eq!(
        (status, body.as_slice()),
        (StatusCode::OK, b"\x89PNG file".as_slice())
    );
    for n in ["1", "2", "3", "x"] {
        let (status, _, body) = get_raw(&h, &url(n), &alice).await?;
        assert_eq!(status, StatusCode::NOT_FOUND, "attachment {n}");
        assert_ne!(body, b"secret");
    }

    // A sandbox process (root inside its sandbox) replaces the attachments folder with a link to
    // a folder it controls elsewhere: nothing is served through it.
    let elsewhere = sandbox.join("elsewhere");
    std::fs::rename(&attachments, &elsewhere)?;
    std::os::unix::fs::symlink(&elsewhere, &attachments)?;
    for n in ["0", "4"] {
        let (status, _, _) = get_raw(&h, &url(n), &alice).await?;
        assert_eq!(
            status,
            StatusCode::NOT_FOUND,
            "attachment {n} through the link"
        );
    }
    let listed = call(&h.router, Method::GET, "/api/tasks/t1/prompt", None, &alice).await?;
    assert!(
        listed.body["attachments"]
            .as_array()
            .ok_or("attachments")?
            .iter()
            .all(|a| a["available"] == false),
        "{}",
        listed.body
    );
    Ok(())
}

/// GETs `path` as `creds` and returns the response with its body not read yet.
async fn get_unread(
    h: &Harness,
    path: &str,
    creds: &Creds,
) -> Result<axum::response::Response, Box<dyn std::error::Error>> {
    let mut req = Request::builder().method(Method::GET).uri(path);
    if let Some(cookie) = &creds.cookie {
        req = req.header(header::COOKIE, support::cookie_header(cookie));
    }
    if let Some(token) = &creds.bearer {
        req = req.header(header::AUTHORIZATION, format!("Bearer {token}"));
    }
    Ok(h.router.clone().oneshot(req.body(Body::empty())?).await?)
}

/// #303: the dialog loads every thumbnail at once; reads used to take the uploads' slots and
/// failed with 429 beyond 8, or while uploads ran.
#[tokio::test]
async fn twelve_images_are_served_together_while_every_upload_slot_is_taken() -> TestResult {
    let (h, drafts, [alice, ..]) = seeded(|d| {
        (0..12)
            .map(|n| format!("@{d}/img-{n}.png"))
            .collect::<Vec<_>>()
            .join("\n")
    })
    .await?;
    for n in 0..12 {
        std::fs::write(
            drafts.join(format!("img-{n}.png")),
            format!("\u{89}PNG {n}"),
        )?;
    }
    let user = h
        .store
        .user(&UserId::parse("u-alice")?)
        .await?
        .ok_or("user")?;
    let ctx = AuthContext {
        user,
        csrf_token: None,
        session_hash: None,
        token: None,
    };
    let mut uploads = Vec::new();
    for n in 0..MAX_CONCURRENT_UPLOADS {
        uploads.push(
            h.app
                .begin_file_upload(&ctx, "s1", None, &format!("up-{n}.txt"), false, None)
                .await?,
        );
    }

    // All twelve answer while their bodies are still unread (each holds its read slot).
    let url = |n: usize| format!("/api/tasks/t1/prompt/attachments/{n}");
    let responses = futures::future::try_join_all((0..12).map(|n| {
        let (h, alice, url) = (&h, &alice, url(n));
        async move { get_unread(h, &url, alice).await }
    }))
    .await?;
    for (n, response) in responses.into_iter().enumerate() {
        assert_eq!(response.status(), StatusCode::OK, "image {n}");
        let body = response.into_body().collect().await?.to_bytes();
        assert_eq!(body, format!("\u{89}PNG {n}").as_bytes(), "image {n}");
    }
    drop(uploads);
    Ok(())
}

/// #303: an attachment is streamed in bounded chunks, never read whole into memory.
#[tokio::test]
async fn an_attachment_is_streamed_in_bounded_chunks_with_its_length() -> TestResult {
    const BIG: u64 = 1024 * 1024;
    let (h, drafts, [alice, ..]) = seeded_with(BIG, |d| format!("@{d}/big.bin")).await?;
    let content: Vec<u8> = (0..BIG).map(|i| (i % 251) as u8).collect();
    std::fs::write(drafts.join("big.bin"), &content)?;

    let response = get_unread(&h, "/api/tasks/t1/prompt/attachments/0", &alice).await?;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()[header::CONTENT_LENGTH], BIG.to_string());
    let mut body = response.into_body();
    let (mut received, mut frames) = (Vec::new(), 0);
    while let Some(frame) = body.frame().await {
        let data = frame?.into_data().map_err(|_| "not data")?;
        assert!(
            data.len() <= yulius_app::task_prompt::ATTACHMENT_CHUNK,
            "{}",
            data.len()
        );
        received.extend_from_slice(&data);
        frames += 1;
    }
    assert!(frames > 1, "{frames}");
    assert!(received == content, "the bytes differ");
    Ok(())
}
