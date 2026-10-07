//! Skill catalog and MCP configuration end to end (AC-SKL-1..3, AC-MCP-1/2): sources,
//! toggles, the run skill set installed into both harness homes, the Open Mercato config, write-
//! only sealed MCP secrets, attachments with scope override, and the connection test against a
//! loopback HTTP server and a stdio child.

mod support;

use std::path::Path;

use axum::http::{HeaderMap, Method, StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde_json::{Value, json};
use support::{Creds, TestResult, add_sandbox, call, harness_configured, perms, user_with_token};
use yulius_app::ports::harness::McpAttachment;
use yulius_app::ports::store::SandboxStore;
use yulius_app::skills::pack;
use yulius_domain::ids::SandboxId;

type Error = Box<dyn std::error::Error>;

struct World {
    h: support::Harness,
    alice: Creds,
    bob: Creds,
    root: Creds,
}

async fn world(contained: bool) -> Result<World, Error> {
    let h = harness_configured(move |c| {
        c.max_live_sessions = 0;
        c.contained = contained;
    })?;
    add_sandbox(&h, "s1", "studio", 20000).await?;
    let alice = user_with_token(&h, "u-alice", "alice", false).await?;
    let bob = user_with_token(&h, "u-bob", "bob", false).await?;
    let root = user_with_token(&h, "u-root", "root", true).await?;
    h.github
        .set_perm("alice", "Quartio-Studio/studio", perms("write"));
    h.github
        .set_perm("bob", "Quartio-Studio/studio", perms("read"));
    h.github
        .set_perm("root", "Quartio-Studio/studio", perms("write"));
    Ok(World {
        h,
        alice,
        bob,
        root,
    })
}

fn write(path: &Path, text: &str) -> Result<(), Error> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, text)?;
    Ok(())
}

fn by_name<'a>(list: &'a Value, name: &str) -> Option<&'a Value> {
    list.as_array()?.iter().find(|s| s["name"] == name)
}

#[tokio::test]
async fn skill_catalog_sources_toggles_run_set_and_om_config() -> TestResult {
    let w = world(false).await?;
    let data = w.h.app.config.data_dir.clone();
    assert_eq!(
        w.h.app.install_skill_catalog().await? as usize,
        pack::PACK_FILES.len()
    );
    assert_eq!(w.h.app.install_skill_catalog().await?, 0, "idempotent");
    assert!(data.join("skills/open-mercato/LICENSE").is_file());

    let repo = data.join("sandboxes/s1/repo");
    write(
        &repo.join(".agents/skills/alpha/SKILL.md"),
        "---\nname: alpha\ndescription: >\n  Design pages\n  consistently.\n---\n# Alpha\n",
    )?;
    write(
        &repo.join(".claude/skills/beta/SKILL.md"),
        "---\nname: beta\ndescription: \"Write release notes: short\"\n---\nBody\n",
    )?;
    write(
        &repo.join(".agents/skills/om-fix/SKILL.md"),
        "---\nname: om-fix\ndescription: Our own fix process\n---\n",
    )?;
    let outside = data.join("host-secrets");
    write(
        &outside.join("SKILL.md"),
        "---\nname: evil\n---\nhost secret\n",
    )?;
    std::os::unix::fs::symlink(&outside, repo.join(".agents/skills/evil"))?;
    write(
        &data.join("skills/gamma/SKILL.md"),
        "---\nname: gamma\ndescription: Company brand voice\n---\n",
    )?;

    let list = call(
        &w.h.router,
        Method::GET,
        "/api/sandboxes/s1/skills",
        None,
        &w.bob,
    )
    .await?;
    assert_eq!(list.status, StatusCode::OK);
    let alpha = by_name(&list.body, "alpha").ok_or("alpha")?;
    assert_eq!(alpha["source"], "repository");
    assert_eq!(alpha["description"], "Design pages consistently.");
    assert_eq!(alpha["path"], ".agents/skills/alpha/SKILL.md");
    assert_eq!(alpha["enabled"], true);
    let beta = by_name(&list.body, "beta").ok_or("beta")?;
    assert_eq!(beta["path"], ".claude/skills/beta/SKILL.md");
    assert_eq!(beta["description"], "Write release notes: short");
    assert_eq!(
        by_name(&list.body, "om-fix").ok_or("om-fix")?["source"],
        "repository",
        "the repository shadows the pack"
    );
    assert_eq!(
        by_name(&list.body, "gamma").ok_or("gamma")?["source"],
        "instance"
    );
    assert!(
        by_name(&list.body, "evil").is_none(),
        "a symlink leaving the repository is ignored"
    );
    let om: Vec<&str> = list
        .body
        .as_array()
        .ok_or("list")?
        .iter()
        .filter(|s| s["source"] == "open-mercato")
        .filter_map(|s| s["name"].as_str())
        .collect();
    assert_eq!(om.len(), pack::PACK_SKILLS.len() - 1);
    assert!(om.iter().all(|n| pack::PACK_SKILLS.contains(n)));
    assert_eq!(
        by_name(&list.body, "om-code-review").ok_or("om")?["path"],
        "skills/open-mercato/om-code-review/SKILL.md"
    );

    let content = call(
        &w.h.router,
        Method::GET,
        "/api/sandboxes/s1/skills/om-code-review",
        None,
        &w.bob,
    )
    .await?;
    assert_eq!(content.status, StatusCode::OK);
    assert!(
        content.body["content"]
            .as_str()
            .unwrap_or("")
            .contains("name: om-code-review")
    );
    let evil = call(
        &w.h.router,
        Method::GET,
        "/api/sandboxes/s1/skills/evil",
        None,
        &w.bob,
    )
    .await?;
    assert_eq!(evil.status, StatusCode::NOT_FOUND);
    let traversal = call(
        &w.h.router,
        Method::GET,
        "/api/sandboxes/s1/skills/..",
        None,
        &w.bob,
    )
    .await?;
    assert_ne!(traversal.status, StatusCode::OK);

    let off = json!({"enabled": false});
    let denied = call(
        &w.h.router,
        Method::PUT,
        "/api/sandboxes/s1/skills/om-brainstorm",
        Some(off.clone()),
        &w.bob,
    )
    .await?;
    assert_eq!(denied.status, StatusCode::FORBIDDEN);
    let toggled = call(
        &w.h.router,
        Method::PUT,
        "/api/sandboxes/s1/skills/om-brainstorm",
        Some(off.clone()),
        &w.alice,
    )
    .await?;
    assert_eq!(toggled.status, StatusCode::NO_CONTENT);
    let repo_toggle = call(
        &w.h.router,
        Method::PUT,
        "/api/sandboxes/s1/skills/alpha",
        Some(off.clone()),
        &w.alice,
    )
    .await?;
    assert_eq!(repo_toggle.status, StatusCode::UNPROCESSABLE_ENTITY);
    let instance_toggle = call(
        &w.h.router,
        Method::PUT,
        "/api/sandboxes/s1/skills/gamma",
        Some(off.clone()),
        &w.alice,
    )
    .await?;
    assert_eq!(instance_toggle.status, StatusCode::FORBIDDEN);
    let admin_toggle = call(
        &w.h.router,
        Method::PUT,
        "/api/sandboxes/s1/skills/gamma",
        Some(off.clone()),
        &w.root,
    )
    .await?;
    assert_eq!(admin_toggle.status, StatusCode::NO_CONTENT);
    let after = call(
        &w.h.router,
        Method::GET,
        "/api/sandboxes/s1/skills",
        None,
        &w.alice,
    )
    .await?;
    assert_eq!(
        by_name(&after.body, "om-brainstorm").ok_or("om")?["enabled"],
        false
    );
    assert_eq!(
        by_name(&after.body, "om-spec-writing").ok_or("om")?["enabled"],
        true,
        "default enabled"
    );

    // AC-SKL-1: the run set (enabled instance and pack skills) installs into both harness homes.
    let sandbox =
        w.h.store
            .sandbox(&SandboxId::parse("s1")?)
            .await?
            .ok_or("sandbox")?;
    let run_set = w.h.app.skills_for_run(&sandbox).await?;
    let mut names: Vec<&str> = run_set.iter().map(|(n, _)| n.as_str()).collect();
    names.sort_unstable();
    assert_eq!(
        names,
        [
            "om-check-and-commit",
            "om-code-review",
            "om-root-cause",
            "om-spec-writing",
            "om-ux-shape",
            "om-verify-in-repo"
        ]
    );
    let homes = tempfile::tempdir()?;
    let (claude, codex) = (
        homes.path().join("claude/skills"),
        homes.path().join("codex/skills"),
    );
    w.h.app
        .deps
        .harness
        .install_skills(&run_set, &claude, &codex)?;
    for dir in [&claude, &codex] {
        assert!(dir.join("om-code-review/SKILL.md").is_file());
        assert!(
            dir.join("om-code-review/references/review-checklist.md")
                .is_file()
        );
        assert!(!dir.join("om-brainstorm").exists());
    }

    // The Open Mercato config is rendered from the profile and yulius.toml when absent.
    write(
        &repo.join("yulius.toml"),
        "[commands]\ntest = \"cargo test\"\nlint = \"cargo clippy\"\n\n[docs]\nspecs_dir = \"knowledge/specs\"\n",
    )?;
    let config = w.h.app.om_config_for(&sandbox).await?.ok_or("config")?;
    let config: Value = serde_json::from_str(&config)?;
    assert_eq!(
        config["baseBranch"], "quartio",
        "the sandbox's integration branch"
    );
    assert_eq!(
        config["validation"]["commands"],
        json!(["cargo clippy", "cargo test"])
    );
    assert_eq!(config["paths"]["specs"], "knowledge/specs");
    write(&repo.join(".ai/agentic.config.json"), "{}")?;
    assert_eq!(
        w.h.app.om_config_for(&sandbox).await?,
        None,
        "an existing config wins"
    );
    std::fs::remove_file(repo.join(".ai/agentic.config.json"))?;
    for skill in pack::PACK_SKILLS {
        let r = call(
            &w.h.router,
            Method::PUT,
            &format!("/api/sandboxes/s1/skills/{skill}"),
            Some(off.clone()),
            &w.alice,
        )
        .await?;
        assert!(
            r.status == StatusCode::NO_CONTENT || skill == "om-fix",
            "{skill}: {}",
            r.status
        );
    }
    assert_eq!(
        w.h.app.om_config_for(&sandbox).await?,
        None,
        "pack disabled"
    );
    Ok(())
}

#[tokio::test]
async fn mcp_secrets_are_write_only_sealed_and_attached_with_overrides() -> TestResult {
    let w = world(false).await?;
    let quartio = json!({"name": "quartio", "transport": "http", "url": "https://mcp.quartio.test/mcp",
        "headers": {"Authorization": "Bearer sk-quartio-secret", "X-Org": "q"}, "scope": "instance"});
    let not_admin = call(
        &w.h.router,
        Method::POST,
        "/api/sandboxes/s1/mcp",
        Some(quartio.clone()),
        &w.alice,
    )
    .await?;
    assert_eq!(not_admin.status, StatusCode::FORBIDDEN);
    let created = call(
        &w.h.router,
        Method::POST,
        "/api/sandboxes/s1/mcp",
        Some(quartio),
        &w.root,
    )
    .await?;
    assert_eq!(created.status, StatusCode::CREATED, "{}", created.body);
    assert_eq!(created.body["scope"], "instance");
    assert_eq!(
        created.body["header_keys"],
        json!(["Authorization", "X-Org"])
    );
    let quartio_id = created.body["id"].as_str().ok_or("id")?.to_owned();

    let apify = json!({"name": "apify", "transport": "stdio", "command": "npx", "args": ["-y", "@apify/actors-mcp-server"],
        "env": {"APIFY_TOKEN": "apify-secret-123"}});
    let read_only = call(
        &w.h.router,
        Method::POST,
        "/api/sandboxes/s1/mcp",
        Some(apify.clone()),
        &w.bob,
    )
    .await?;
    assert_eq!(read_only.status, StatusCode::FORBIDDEN);
    let sandbox_apify = call(
        &w.h.router,
        Method::POST,
        "/api/sandboxes/s1/mcp",
        Some(apify.clone()),
        &w.alice,
    )
    .await?;
    assert_eq!(
        sandbox_apify.status,
        StatusCode::CREATED,
        "{}",
        sandbox_apify.body
    );
    assert_eq!(sandbox_apify.body["env_keys"], json!(["APIFY_TOKEN"]));
    let apify_id = sandbox_apify.body["id"].as_str().ok_or("id")?.to_owned();
    let mut duplicate = apify.clone();
    duplicate["name"] = json!("Apify");
    let dup = call(
        &w.h.router,
        Method::POST,
        "/api/sandboxes/s1/mcp",
        Some(duplicate),
        &w.alice,
    )
    .await?;
    assert_eq!(dup.status, StatusCode::CONFLICT);
    let reserved = call(
        &w.h.router,
        Method::POST,
        "/api/sandboxes/s1/mcp",
        Some(json!({"name": "yulius", "transport": "http", "url": "http://x"})),
        &w.alice,
    )
    .await?;
    assert_eq!(reserved.status, StatusCode::UNPROCESSABLE_ENTITY);
    let instance_apify = call(&w.h.router, Method::POST, "/api/sandboxes/s1/mcp", Some(json!({"name": "apify", "transport": "stdio", "command": "instance-apify", "scope": "instance"})), &w.root).await?;
    assert_eq!(
        instance_apify.status,
        StatusCode::CREATED,
        "another scope may reuse the name"
    );

    let list = call(
        &w.h.router,
        Method::GET,
        "/api/sandboxes/s1/mcp",
        None,
        &w.bob,
    )
    .await?;
    assert_eq!(list.status, StatusCode::OK);
    assert_eq!(list.body.as_array().map(Vec::len), Some(3));
    assert_eq!(list.body[0]["scope"], "instance");
    let raw = list.body.to_string();
    assert!(
        !raw.contains("sk-quartio-secret") && !raw.contains("apify-secret-123"),
        "secrets are never returned"
    );

    // Secrets are encrypted at rest.
    let backup = w.h.app.config.data_dir.join("backup.db");
    w.h.store.backup_to(&backup).await?;
    let bytes = std::fs::read(&backup)?;
    for secret in ["sk-quartio-secret", "apify-secret-123"] {
        assert!(
            !bytes
                .windows(secret.len())
                .any(|win| win == secret.as_bytes()),
            "{secret} stored in plaintext"
        );
    }

    // Updating without env keeps the stored secret.
    let mut edit = apify.clone();
    edit.as_object_mut().ok_or("obj")?.remove("env");
    edit["args"] = json!(["-y", "@apify/actors-mcp-server", "--tools", "x-scraper"]);
    let kept = call(
        &w.h.router,
        Method::PUT,
        &format!("/api/mcp/{apify_id}"),
        Some(edit.clone()),
        &w.alice,
    )
    .await?;
    assert_eq!(kept.status, StatusCode::OK, "{}", kept.body);
    assert_eq!(kept.body["env_keys"], json!(["APIFY_TOKEN"]));

    // AC-MCP-1: attachments carry opened secrets; the sandbox server overrides the instance one.
    let sandbox =
        w.h.store
            .sandbox(&SandboxId::parse("s1")?)
            .await?
            .ok_or("sandbox")?;
    let attachments = w.h.app.mcp_attachments(&sandbox).await?;
    assert_eq!(attachments.len(), 2);
    assert!(attachments.contains(&McpAttachment::Http {
        name: "quartio".into(),
        url: "https://mcp.quartio.test/mcp".into(),
        bearer: Some("sk-quartio-secret".into()),
    }));
    assert!(attachments.contains(&McpAttachment::Stdio {
        name: "apify".into(),
        command: "npx".into(),
        args: vec![
            "-y".into(),
            "@apify/actors-mcp-server".into(),
            "--tools".into(),
            "x-scraper".into()
        ],
        env: vec![("APIFY_TOKEN".into(), "apify-secret-123".into())],
    }));

    // Disabling the sandbox override turns the instance server off for this sandbox.
    let mut disabled = edit.clone();
    disabled["enabled"] = json!(false);
    call(
        &w.h.router,
        Method::PUT,
        &format!("/api/mcp/{apify_id}"),
        Some(disabled),
        &w.alice,
    )
    .await?;
    let attachments = w.h.app.mcp_attachments(&sandbox).await?;
    assert_eq!(attachments.len(), 1);

    // An explicit empty map clears the secrets.
    let mut cleared = edit.clone();
    cleared["env"] = json!({});
    let cleared = call(
        &w.h.router,
        Method::PUT,
        &format!("/api/mcp/{apify_id}"),
        Some(cleared),
        &w.alice,
    )
    .await?;
    assert_eq!(cleared.body["env_keys"], json!([]));

    let edit_instance = call(
        &w.h.router,
        Method::PUT,
        &format!("/api/mcp/{quartio_id}"),
        Some(json!({"name": "quartio", "transport": "http", "url": "https://mcp.quartio.test/v2"})),
        &w.alice,
    )
    .await?;
    assert_eq!(edit_instance.status, StatusCode::FORBIDDEN);
    let admin_edit = call(
        &w.h.router,
        Method::PUT,
        &format!("/api/mcp/{quartio_id}"),
        Some(json!({"name": "quartio", "transport": "http", "url": "https://mcp.quartio.test/v2"})),
        &w.root,
    )
    .await?;
    assert_eq!(admin_edit.status, StatusCode::OK);
    assert_eq!(
        admin_edit.body["header_keys"],
        json!(["Authorization", "X-Org"])
    );
    let deleted = call(
        &w.h.router,
        Method::DELETE,
        &format!("/api/mcp/{apify_id}"),
        None,
        &w.alice,
    )
    .await?;
    assert_eq!(deleted.status, StatusCode::NO_CONTENT);
    let gone = call(
        &w.h.router,
        Method::DELETE,
        &format!("/api/mcp/{apify_id}"),
        None,
        &w.alice,
    )
    .await?;
    assert_eq!(gone.status, StatusCode::NOT_FOUND);
    Ok(())
}

/// A minimal Streamable HTTP MCP server: bearer auth, a session id, JSON for `initialize`
/// and an event stream for `tools/list`.
async fn fake_mcp(headers: HeaderMap, body: String) -> Response {
    if headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        != Some("Bearer good")
    {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let message: Value = serde_json::from_str(&body).unwrap_or(Value::Null);
    let id = message["id"].clone();
    match message["method"].as_str() {
        Some("initialize") => (
            [("mcp-session-id", "session-1")],
            axum::Json(json!({"jsonrpc": "2.0", "id": id, "result": {"protocolVersion": "2025-06-18", "capabilities": {"tools": {}}, "serverInfo": {"name": "fake", "version": "1"}}})),
        )
            .into_response(),
        Some("tools/list") => {
            if headers.get("mcp-session-id").and_then(|v| v.to_str().ok()) != Some("session-1") {
                return StatusCode::BAD_REQUEST.into_response();
            }
            let payload = json!({"jsonrpc": "2.0", "id": id, "result": {"tools": [{"name": "search_knowledge"}, {"name": "get_customer"}]}});
            (
                [(header::CONTENT_TYPE, "text/event-stream")],
                format!("event: message\ndata: {payload}\n\n"),
            )
                .into_response()
        }
        _ => StatusCode::ACCEPTED.into_response(),
    }
}

async fn test_server(w: &World, creds: &Creds, id: &str) -> Result<(StatusCode, Value), Error> {
    let r = call(
        &w.h.router,
        Method::POST,
        &format!("/api/mcp/{id}/test"),
        None,
        creds,
    )
    .await?;
    Ok((r.status, r.body))
}

#[tokio::test]
async fn mcp_test_endpoint_probes_http_and_stdio_servers() -> TestResult {
    let w = world(false).await?;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let port = listener.local_addr()?.port();
    let server = tokio::spawn(async move {
        let app = axum::Router::new().route("/mcp", axum::routing::post(fake_mcp));
        let _ = axum::serve(listener, app).await;
    });

    let http = json!({"name": "fake", "transport": "http", "url": format!("http://127.0.0.1:{port}/mcp"), "headers": {"Authorization": "Bearer good"}});
    let created = call(
        &w.h.router,
        Method::POST,
        "/api/sandboxes/s1/mcp",
        Some(http.clone()),
        &w.alice,
    )
    .await?;
    let http_id = created.body["id"].as_str().ok_or("id")?.to_owned();
    let (status, ok) = test_server(&w, &w.alice, &http_id).await?;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        ok,
        json!({"ok": true, "tools": ["search_knowledge", "get_customer"], "error": null})
    );
    let (status, _) = test_server(&w, &w.bob, &http_id).await?;
    assert_eq!(status, StatusCode::FORBIDDEN, "testing needs write access");

    let mut wrong = http.clone();
    wrong["headers"] = json!({"Authorization": "Bearer bad"});
    call(
        &w.h.router,
        Method::PUT,
        &format!("/api/mcp/{http_id}"),
        Some(wrong),
        &w.alice,
    )
    .await?;
    let (_, unauthorized) = test_server(&w, &w.alice, &http_id).await?;
    assert_eq!(unauthorized["ok"], false);
    let error = unauthorized["error"].as_str().unwrap_or("");
    assert!(error.contains("401") && !error.contains("bad"), "{error}");

    let script = concat!(
        "read a; ",
        "echo '{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{\"protocolVersion\":\"2025-06-18\",\"capabilities\":{}}}'; ",
        "read b; read c; ",
        "echo \"{\\\"jsonrpc\\\":\\\"2.0\\\",\\\"id\\\":2,\\\"result\\\":{\\\"tools\\\":[{\\\"name\\\":\\\"$TOOL_NAME\\\"}]}}\"; ",
        "sleep 30"
    );
    let stdio = json!({"name": "local", "transport": "stdio", "command": "/bin/sh", "args": ["-c", script], "env": {"TOOL_NAME": "list_feedback"}});
    let created = call(
        &w.h.router,
        Method::POST,
        "/api/sandboxes/s1/mcp",
        Some(stdio),
        &w.alice,
    )
    .await?;
    let stdio_id = created.body["id"].as_str().ok_or("id")?.to_owned();
    let (_, ok) = test_server(&w, &w.alice, &stdio_id).await?;
    assert_eq!(
        ok,
        json!({"ok": true, "tools": ["list_feedback"], "error": null})
    );

    let missing =
        json!({"name": "missing", "transport": "stdio", "command": "/nonexistent/mcp-server"});
    let created = call(
        &w.h.router,
        Method::POST,
        "/api/sandboxes/s1/mcp",
        Some(missing),
        &w.alice,
    )
    .await?;
    let (_, failed) = test_server(&w, &w.alice, created.body["id"].as_str().ok_or("id")?).await?;
    assert_eq!(failed["ok"], false);
    assert!(
        failed["error"]
            .as_str()
            .unwrap_or("")
            .starts_with("could not start")
    );
    server.abort();
    Ok(())
}

/// On a contained instance a stdio test would run outside the containment, so only instance
/// administrators may run it.
#[tokio::test]
async fn contained_instances_restrict_stdio_tests_to_admins() -> TestResult {
    let w = world(true).await?;
    let created = call(
        &w.h.router,
        Method::POST,
        "/api/sandboxes/s1/mcp",
        Some(
            json!({"name": "missing", "transport": "stdio", "command": "/nonexistent/mcp-server"}),
        ),
        &w.alice,
    )
    .await?;
    let id = created.body["id"].as_str().ok_or("id")?.to_owned();
    let (status, _) = test_server(&w, &w.alice, &id).await?;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, body) = test_server(&w, &w.root, &id).await?;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["ok"], false);
    Ok(())
}
