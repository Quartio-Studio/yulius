//! Agent states (SPEC-271, ADR-0200): a folder of operator state a person sends is checked,
//! sealed in the vault and restored by a task into the sandbox home; the sandbox's copy is then
//! the only writer, saved back as new versions and downloadable, never forked by a second send.

use crate::support;

use std::io::Read as _;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};

use axum::body::Body;
use axum::http::{Method, StatusCode};
use flate2::Compression;
use flate2::write::GzEncoder;
use serde_json::{Value, json};
use support::{
    Creds, TestResult, call, harness, perms, seed_github_user, seed_live_attempt, seed_sandbox,
    seed_task, send,
};
use yulius_app::ports::vault::{AgentStateStore as _, VaultStore as _};
use yulius_domain::ids::SandboxId;
use yulius_domain::task::TaskStatus;

const TOKEN: &str = "state-task-token-s1-0123456789abcdef";
/// 50 MiB: the size the operator state must reach at least.
const BIG: usize = 50 << 20;

type Error = Box<dyn std::error::Error>;

struct World {
    h: support::Harness,
    writer: Creds,
    reader: Creds,
    home: PathBuf,
}

/// Sandbox s1: alice writes it, bob reads it; task t1 (alice's) runs in it.
async fn world() -> Result<World, Error> {
    let h = harness()?;
    let writer = seed_github_user(&h, "u-alice", "alice").await?;
    let reader = seed_github_user(&h, "u-bob", "bob").await?;
    seed_sandbox(&h, "s1", "app", "u-alice").await?;
    h.github.set_perm("alice", "acme/app", perms("write"));
    h.github.set_perm("bob", "acme/app", perms("read"));
    let home = h.app.config.data_dir.join("sandboxes/s1/home");
    std::fs::create_dir_all(&home)?;
    seed_task(&h, "t1", "s1", "u-alice", TaskStatus::Running, None).await?;
    seed_live_attempt(&h, "t1", TOKEN).await?;
    Ok(World {
        h,
        writer,
        reader,
        home,
    })
}

/// Bytes that do not compress, so the packed state is as large as its files.
fn noise(len: usize) -> Vec<u8> {
    let mut x: u64 = 0x9e37_79b9_7f4a_7c15;
    let mut out = Vec::with_capacity(len + 8);
    while out.len() < len {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        out.extend_from_slice(&x.to_le_bytes());
    }
    out.truncate(len);
    out
}

/// A packed state of `(path, mode, content)` files (stored, not compressed: quick in tests).
fn archive(files: &[(&str, u32, &[u8])]) -> Result<Vec<u8>, Error> {
    let mut builder = tar::Builder::new(GzEncoder::new(Vec::new(), Compression::none()));
    for (path, mode, content) in files {
        let mut header = tar::Header::new_gnu();
        header.set_entry_type(tar::EntryType::Regular);
        header.set_mode(*mode);
        header.set_size(content.len() as u64);
        builder.append_data(&mut header, path, *content)?;
    }
    Ok(builder.into_inner()?.finish()?)
}

async fn put(
    w: &World,
    name: &str,
    query: &str,
    body: Vec<u8>,
    creds: &Creds,
) -> Result<support::Resp, Error> {
    let len = body.len();
    send(
        &w.h.router,
        Method::PUT,
        &format!("/api/sandboxes/s1/agent-states/{name}{query}"),
        Body::from(body),
        Some(len),
        creds,
    )
    .await
}

async fn tool(w: &World, name: &str, args: Value) -> Result<(bool, Value), Error> {
    let result = support::mcp_tool(&w.h, TOKEN, name, args).await?;
    let error = result["isError"].as_bool().unwrap_or(false);
    Ok((error, support::tool_text(&result)))
}

fn mode_of(path: &Path) -> Result<u32, Error> {
    Ok(std::fs::metadata(path)?.permissions().mode() & 0o7777)
}

/// The files of a packed state, by path.
fn unpacked(bytes: &[u8]) -> Result<Vec<(String, Vec<u8>)>, Error> {
    let mut tar = tar::Archive::new(flate2::read::GzDecoder::new(bytes));
    let mut out = Vec::new();
    for entry in tar.entries()? {
        let mut entry = entry?;
        if entry.header().entry_type().is_file() {
            let mut content = Vec::new();
            entry.read_to_end(&mut content)?;
            out.push((entry.path()?.display().to_string(), content));
        }
    }
    Ok(out)
}

/// SPEC-271 AC2..AC4 (OPS-002..OPS-007): a 50 MiB state is sent, restored with its modes into
/// the home, refused a second time once restored, saved back as a new version, downloaded,
/// replaced only on purpose; every step is audited and no answer holds content.
#[tokio::test]
async fn a_state_is_restored_once_saved_back_and_never_forked() -> TestResult {
    let w = world().await?;
    let big = noise(BIG);
    let first = archive(&[
        ("inventory.sqlite", 0o600, b"SQLite format 3\0inventory-v1"),
        ("rollouts/r1/bundle.bin", 0o644, &big),
        ("bin/ops", 0o755, b"#!/bin/sh\n"),
    ])?;
    assert!(first.len() > BIG, "{}", first.len());

    // A reader cannot send one.
    let refused = put(&w, "OPS", "", first.clone(), &w.reader).await?;
    assert_eq!(refused.status, StatusCode::FORBIDDEN, "{}", refused.body);
    let sent = put(
        &w,
        "OPS",
        "?description=operator%20state",
        first.clone(),
        &w.writer,
    )
    .await?;
    assert_eq!(sent.status, StatusCode::OK, "{}", sent.body);
    assert_eq!(sent.body["version"], 1, "{}", sent.body);
    assert_eq!(sent.body["files"], 3, "{}", sent.body);
    // Not restored yet: the laptop is still its writer, and a new send replaces it.
    let again = put(&w, "OPS", "", first.clone(), &w.writer).await?;
    assert_eq!(again.body["version"], 2, "{}", again.body);
    assert_eq!(
        again.body["description"], "operator state",
        "{}",
        again.body
    );

    // The task restores it into its home.
    let (error, restored) = tool(
        &w,
        "secret_state_restore",
        json!({"name": "OPS", "path": "~/.quartio-operator-20260901"}),
    )
    .await?;
    assert!(!error, "{restored}");
    assert_eq!(
        restored["path"], "~/.quartio-operator-20260901",
        "{restored}"
    );
    assert_eq!(restored["files"], 3, "{restored}");
    assert!(!restored.to_string().contains("inventory-v1"), "{restored}");
    let copy = w.home.join(".quartio-operator-20260901");
    assert_eq!(mode_of(&copy.join("inventory.sqlite"))?, 0o600);
    assert_eq!(mode_of(&copy.join("bin/ops"))?, 0o755);
    assert_eq!(std::fs::read(copy.join("rollouts/r1/bundle.bin"))?, big);

    // Once restored, it is the writer: a second restore and a second send are refused.
    let (error, said) = tool(
        &w,
        "secret_state_restore",
        json!({"name": "OPS", "path": "~/.quartio-operator-20260901"}),
    )
    .await?;
    assert!(error && said.to_string().contains("use it"), "{said}");
    let (error, said) = tool(
        &w,
        "secret_state_restore",
        json!({"name": "OPS", "path": "~/elsewhere"}),
    )
    .await?;
    assert!(error && said.to_string().contains("restored at"), "{said}");
    let forked = put(&w, "OPS", "", first.clone(), &w.writer).await?;
    assert_eq!(forked.status, StatusCode::CONFLICT, "{}", forked.body);
    assert!(
        forked
            .body
            .to_string()
            .contains("~/.quartio-operator-20260901"),
        "{}",
        forked.body
    );

    // A rollout changes it; the task saves it back; the person downloads that copy.
    std::fs::write(
        copy.join("inventory.sqlite"),
        b"SQLite format 3\0inventory-v2",
    )?;
    std::os::unix::fs::symlink("/etc/passwd", copy.join("passwd"))?;
    let (error, saved) = tool(&w, "secret_state_save", json!({"name": "OPS"})).await?;
    assert!(!error, "{saved}");
    assert_eq!(saved["version"], 3, "{saved}");
    assert_eq!(saved["skipped"], json!(["passwd"]), "{saved}");
    let listed = call(
        &w.h.router,
        Method::GET,
        "/api/sandboxes/s1/agent-states",
        None,
        &w.reader,
    )
    .await?;
    assert_eq!(
        listed.body[0]["restored_path"],
        "~/.quartio-operator-20260901"
    );
    assert_eq!(listed.body[0]["version"], 3);
    let state =
        w.h.store
            .agent_state(&SandboxId::parse("s1")?, "OPS")
            .await?
            .ok_or("stored")?;
    // Sealed: no gzip stream (1f 8b) where the archive would begin.
    assert!(
        !state.1.ciphertext[..64]
            .windows(2)
            .any(|w| w == [0x1f, 0x8b]),
        "stored in the clear"
    );
    let download = support::repos::call_raw(
        &w.h.router,
        Method::GET,
        "/api/sandboxes/s1/agent-states/OPS/archive",
        Vec::new(),
        &[],
        &w.reader,
    )
    .await?;
    assert_eq!(download.status, StatusCode::FORBIDDEN);
    let bytes = download_bytes(&w).await?;
    let files = unpacked(&bytes)?;
    assert!(
        files
            .iter()
            .any(|(p, c)| p == "inventory.sqlite" && c == b"SQLite format 3\0inventory-v2"),
        "{:?}",
        files.iter().map(|f| &f.0).collect::<Vec<_>>()
    );

    // The person replaces it on purpose: the task's save is refused until it restores that.
    let replaced = put(&w, "OPS", "?replace=true", first.clone(), &w.writer).await?;
    assert_eq!(replaced.status, StatusCode::OK, "{}", replaced.body);
    assert_eq!(replaced.body["version"], 4);
    let (error, said) = tool(&w, "secret_state_save", json!({"name": "OPS"})).await?;
    assert!(error && said.to_string().contains("newer"), "{said}");
    let (error, restored) = tool(
        &w,
        "secret_state_restore",
        json!({"name": "OPS", "path": "~/.quartio-operator-20260901"}),
    )
    .await?;
    assert!(!error, "{restored}");
    assert_eq!(
        std::fs::read(copy.join("inventory.sqlite"))?,
        b"SQLite format 3\0inventory-v1"
    );
    assert!(!copy.join("passwd").exists());
    let leftovers: Vec<_> = std::fs::read_dir(&w.home)?
        .filter_map(Result::ok)
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(leftovers, vec![".quartio-operator-20260901".to_owned()]);

    // The list an agent reads names it; every step is audited, without content.
    let (error, list) = tool(&w, "secrets_list", json!({})).await?;
    assert!(!error, "{list}");
    assert_eq!(list["states"][0]["name"], "OPS", "{list}");
    assert_eq!(
        list["states"][0]["restored_path"], "~/.quartio-operator-20260901",
        "{list}"
    );
    let audit = w.h.store.vault_audit(50).await?;
    let actions: Vec<(&str, &str)> = audit
        .iter()
        .filter(|e| e.action.starts_with("vault.state."))
        .map(|e| (e.action.as_str(), e.actor.as_str()))
        .collect();
    for expected in [
        ("vault.state.import", "alice"),
        ("vault.state.restore", "task:1"),
        ("vault.state.save", "task:1"),
        ("vault.state.export", "alice"),
        ("vault.state.replace", "alice"),
    ] {
        assert!(actions.contains(&expected), "{expected:?} in {actions:?}");
    }
    assert!(
        audit
            .iter()
            .all(|e| !e.detail.clone().unwrap_or_default().contains("inventory-v")),
        "{audit:?}"
    );
    Ok(())
}

async fn download_bytes(w: &World) -> Result<Vec<u8>, Error> {
    use http_body_util::BodyExt as _;
    use tower::ServiceExt as _;
    let mut req = axum::http::Request::builder()
        .method(Method::GET)
        .uri("/api/sandboxes/s1/agent-states/OPS/archive");
    if let Some(token) = &w.writer.bearer {
        req = req.header(axum::http::header::AUTHORIZATION, format!("Bearer {token}"));
    }
    if let Some(cookie) = &w.writer.cookie {
        req = req.header(axum::http::header::COOKIE, support::cookie_header(cookie));
    }
    let resp = w.h.router.clone().oneshot(req.body(Body::empty())?).await?;
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(
        resp.headers()
            .get(axum::http::header::CACHE_CONTROL)
            .and_then(|v| v.to_str().ok()),
        Some("no-store")
    );
    Ok(resp.into_body().collect().await?.to_bytes().to_vec())
}

/// SPEC-271 AC5, AC6: unusable states are refused before anything is stored, and a restore goes
/// only into the home, never through a link.
#[tokio::test]
async fn unusable_states_and_paths_are_refused() -> TestResult {
    let w = world().await?;
    let good = archive(&[("a", 0o600, b"1")])?;
    let mut linked = tar::Builder::new(GzEncoder::new(Vec::new(), Compression::none()));
    let mut header = tar::Header::new_gnu();
    header.set_entry_type(tar::EntryType::Symlink);
    header.set_size(0);
    linked.append_link(&mut header, "passwd", "/etc/passwd")?;
    let linked = linked.into_inner()?.finish()?;

    let long = format!("?description={}", "x".repeat(501));
    for (name, body, query, said) in [
        ("OPS", linked, "", "holds a link at passwd"),
        (
            "OPS",
            b"not a gzip tar".to_vec(),
            "",
            "not a readable gzip tar",
        ),
        ("ops-state", good.clone(), "", "ops-state"),
        ("OPS", good.clone(), long.as_str(), "at most 500 characters"),
    ] {
        let refused = put(&w, name, query, body, &w.writer).await?;
        assert_eq!(
            refused.status,
            StatusCode::UNPROCESSABLE_ENTITY,
            "{name}{query}: {}",
            refused.body
        );
        assert!(
            refused.body.to_string().contains(said),
            "{said}: {}",
            refused.body
        );
    }
    // Over 64 MiB: refused before the body is read when declared, and as it is read when not.
    let over = send(
        &w.h.router,
        Method::PUT,
        "/api/sandboxes/s1/agent-states/OPS",
        Body::empty(),
        Some((64 << 20) + 1),
        &w.writer,
    )
    .await?;
    assert_eq!(
        over.status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "{}",
        over.body
    );
    let chunks: Vec<Result<Vec<u8>, std::io::Error>> =
        (0..65).map(|_| Ok(vec![0_u8; 1 << 20])).collect();
    let streamed = send(
        &w.h.router,
        Method::PUT,
        "/api/sandboxes/s1/agent-states/OPS",
        Body::from_stream(futures::stream::iter(chunks)),
        None,
        &w.writer,
    )
    .await?;
    assert_eq!(
        streamed.status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "{}",
        streamed.body
    );
    assert!(
        streamed.body.to_string().contains("64 MiB"),
        "{}",
        streamed.body
    );
    assert!(
        w.h.store
            .agent_states(&SandboxId::parse("s1")?)
            .await?
            .is_empty()
    );

    let sent = put(&w, "OPS", "", good, &w.writer).await?;
    assert_eq!(sent.status, StatusCode::OK, "{}", sent.body);
    for path in ["/etc/ops", "ops", "~", "~/../escape", "~root/ops"] {
        let (error, said) = tool(
            &w,
            "secret_state_restore",
            json!({"name": "OPS", "path": path}),
        )
        .await?;
        assert!(error, "{path}: {said}");
    }
    let outside = w.h.app.config.data_dir.join("outside");
    std::fs::create_dir_all(&outside)?;
    std::os::unix::fs::symlink(&outside, w.home.join("linked"))?;
    let (error, said) = tool(
        &w,
        "secret_state_restore",
        json!({"name": "OPS", "path": "~/linked/ops"}),
    )
    .await?;
    assert!(error && said.to_string().contains("link"), "{said}");
    assert_eq!(std::fs::read_dir(&outside)?.count(), 0);
    let (error, said) = tool(&w, "secret_state_save", json!({"name": "OPS"})).await?;
    assert!(
        error && said.to_string().contains("never restored"),
        "{said}"
    );
    let (error, said) = tool(
        &w,
        "secret_state_restore",
        json!({"name": "NOPE", "path": "~/x"}),
    )
    .await?;
    assert!(
        error && said.to_string().contains("no agent state named NOPE"),
        "{said}"
    );
    Ok(())
}
