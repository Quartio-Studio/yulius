//! A task's original prompt (SPEC-043): the text it started with and the files attached to it,
//! read on demand by whoever sees the task; an attached file is served only from the sandbox's
//! attachments folder, never by a path the prompt or the client names.

use crate::support;

use std::path::PathBuf;

use axum::http::{Method, StatusCode};
use support::{
    Creds, Harness, TestResult, call, harness, perms, seed_github_user, seed_sandbox,
    seed_task_with_prompt,
};
use yulius_domain::task::TaskStatus;

/// Alice (write), Bob (read) and Carol (no access) on sandbox `s1`, whose task `t1` was started
/// with `prompt(drafts)`; `drafts` is Alice's composer folder, holding `shot.png` and `notes.txt`.
async fn seeded(
    prompt: impl FnOnce(&str) -> String,
) -> Result<(Harness, PathBuf, [Creds; 3]), Box<dyn std::error::Error>> {
    let h = harness()?;
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
async fn the_original_prompt_is_its_text_and_attached_files_for_whoever_sees_the_task()
-> TestResult {
    let (h, _, [alice, bob, carol]) = seeded(|d| {
        format!("Fix the **login**\n\n- keep the form\n\n@{d}/shot.png\n@'{d}/notes.txt'\n@/etc/hosts")
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
    let unknown = call(&h.router, Method::GET, "/api/tasks/nope/prompt", None, &alice).await?;
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
