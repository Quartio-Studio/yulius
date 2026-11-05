//! The Global Superagent end to end (SPEC-427) with the real detached ptyhost and the Cockpit
//! tests' fake `claude`: one per person, in a folder of its own without built-in tools, served
//! every sandbox's tools for its person through its own bearer, outliving its host sandbox.

use crate::cockpit::{
    Fixture, ORIGIN, answer_record, attach, cockpit_id, cockpit_tool, contained_harness,
    conversation, fixture, fixture_with, insert_account, limited_conversation, mcp,
    predate_launch_digest, printed, project_of, prompt_record, read_message, read_until,
    task_rests, wait_for,
};
use crate::support;

use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;

use axum::http::{Method, StatusCode};
use futures::SinkExt as _;
use serde_json::{Value, json};
use support::{
    TestResult, call, perms, seed_github_user, seed_sandbox, serve, ws_connect, ws_refusal_status,
};
use tokio_tungstenite::tungstenite::Message;
use yulius_domain::ids::{SandboxId, TaskId, UserId};

/// Opens the caller's Global Superagent.
async fn open_global(
    f: &Fixture,
    creds: &support::Creds,
) -> Result<support::Resp, Box<dyn std::error::Error>> {
    call(&f.h.router, Method::POST, "/api/superagent", None, creds).await
}

async fn global_id(
    f: &Fixture,
    creds: &support::Creds,
) -> Result<String, Box<dyn std::error::Error>> {
    let resp = open_global(f, creds).await?;
    assert_eq!(resp.status, StatusCode::OK, "{:?}", resp.body);
    Ok(resp.body["id"].as_str().ok_or("id")?.to_owned())
}

/// alice and carol also work in `s2` (acme/website).
async fn second_sandbox(f: &Fixture) -> TestResult {
    seed_sandbox(&f.h, "s2", "website", "u-alice").await?;
    f.h.github.set_perm("alice", "acme/website", perms("write"));
    f.h.github.set_perm("carol", "acme/website", perms("write"));
    std::fs::create_dir_all(f.data.join("sandboxes/s2/repo"))?;
    Ok(())
}

fn sandbox(id: &str) -> SandboxId {
    SandboxId::parse(id).unwrap_or_else(|_| unreachable!())
}

/// The live Global Superagents of the instance, by host sandbox.
fn globals(f: &Fixture) -> Vec<(String, String)> {
    ["s1", "s2"]
        .into_iter()
        .flat_map(|sid| {
            f.h.app
                .sandbox_shells(&sandbox(sid))
                .into_iter()
                .filter(|shell| shell.is_global_superagent())
                .map(move |shell| (sid.to_owned(), shell.id().to_owned()))
        })
        .collect()
}

/// AC-GSA-1 (GSA-001, GSA-002, GSA-005): one per person, plain Claude Code without built-in
/// tools and with only the Yulius MCP, in a folder of its own; private; refused to a person who
/// can work in no sandbox.
#[tokio::test]
async fn a_person_gets_one_global_superagent_in_its_own_folder_without_built_in_tools() -> TestResult
{
    let f = fixture().await?;
    second_sandbox(&f).await?;
    let served = serve(f.h.router.clone()).await?;

    let (first, second) = tokio::join!(open_global(&f, &f.alice), open_global(&f, &f.alice));
    let (first, second) = (first?, second?);
    assert_eq!(first.status, StatusCode::OK, "{:?}", first.body);
    assert_eq!(second.body["id"], first.body["id"], "one session");
    let id = first.body["id"].as_str().ok_or("id")?.to_owned();
    assert_eq!(globals(&f).len(), 1);
    assert_eq!(global_id(&f, &f.alice).await?, id, "the same one again");
    // The sandbox Superagent is another session.
    assert_ne!(cockpit_id(&f, &f.alice).await?, id);

    let mut socket = attach(served.addr, &id, &f.alice).await?;
    let mut screen = String::new();
    read_until(&mut socket, &mut screen, "ready").await?;
    assert_eq!(
        printed(&screen, "cockpit-args")?,
        "[--strict-mcp-config --tools  --model claude-opus-5-5 --effort high]"
    );
    let own = f.data.join("users/u-alice/superagent");
    let cwd = PathBuf::from(printed(&screen, "cwd")?);
    assert_eq!(cwd, std::fs::canonicalize(own.join("work"))?);
    let config = PathBuf::from(printed(&screen, "config")?);
    assert_eq!(config, own.join(".claude"), "never the person's home");
    let mcp_config: Value =
        serde_json::from_slice(&std::fs::read(config.join("launch-mcp-config"))?)?;
    let servers = mcp_config["mcpServers"].as_object().ok_or("servers")?;
    assert_eq!(
        servers.keys().collect::<Vec<_>>(),
        ["yulius"],
        "only Yulius"
    );
    let bearer = printed(&screen, "bearer")?;
    assert!(bearer.starts_with("gs_"), "{bearer}");
    assert_eq!(printed(&screen, "placeholder")?, "none", "no agent secrets");
    assert_eq!(printed(&screen, "proxy")?, "none", "no egress proxy");
    let record = std::fs::read_to_string(f.data.join("shells").join(&id).join("shell.json"))?;
    assert!(record.contains("\"global\":true"), "{record}");
    assert!(!record.contains(&bearer));

    // Never a terminal of its host; nobody else attaches.
    for sid in ["s1", "s2"] {
        let list = call(
            &f.h.router,
            Method::GET,
            &format!("/api/sandboxes/{sid}/shells"),
            None,
            &f.alice,
        )
        .await?;
        assert_eq!(list.body, json!([]), "{sid}");
    }
    let refused = ws_connect(
        served.addr,
        &format!("/api/shells/{id}/terminal"),
        &f.carol,
        ORIGIN,
    )
    .await;
    assert_eq!(
        refused.as_ref().err().and_then(ws_refusal_status),
        Some(404)
    );
    assert_ne!(global_id(&f, &f.carol).await?, id, "carol's own");

    // bob only reads s1, dave sees nothing: neither has a sandbox to host one.
    let dave = seed_github_user(&f.h, "u-dave", "dave").await?;
    for creds in [&f.bob, &dave] {
        let resp = open_global(&f, creds).await?;
        assert_eq!(resp.status, StatusCode::CONFLICT, "{:?}", resp.body);
        assert!(
            resp.body.to_string().contains("a sandbox you can work in"),
            "{:?}",
            resp.body
        );
    }

    socket
        .send(Message::text(
            json!({"type": "input", "data": "echo still-$((1+1))\r"}).to_string(),
        ))
        .await?;
    read_until(&mut socket, &mut screen, "still-2").await?;
    f.h.app.end_all_shells().await;
    Ok(())
}

/// The bearer of `user`'s Global Superagent.
fn global_token(f: &Fixture, user: &str) -> Result<String, Box<dyn std::error::Error>> {
    Ok(f.h
        .app
        .global_superagent_mcp_token(&UserId::parse(user)?)
        .ok_or("no Global Superagent")?)
}

/// AC-GSA-2 (GSA-003): its bearer is served the remote MCP's tools for its person in every
/// project, without the vault, deploy and onboarding tools; the sandbox Superagent's bearer gets
/// none of them.
#[tokio::test]
async fn the_global_superagent_works_in_every_sandbox_of_its_person() -> TestResult {
    use yulius_app::model::TaskOrigin;
    use yulius_app::ports::store::TaskStore;
    use yulius_app::ports::store_ext::AutomationStore as _;

    let f = fixture().await?;
    second_sandbox(&f).await?;
    global_id(&f, &f.alice).await?;
    let token = global_token(&f, "u-alice")?;
    assert!(token.starts_with("gs_"), "{token}");

    let listed = mcp(&f, &token, "tools/list", json!({})).await?;
    let tools = listed["result"]["tools"].as_array().ok_or("tools")?;
    let names: Vec<&str> = tools.iter().filter_map(|t| t["name"].as_str()).collect();
    for wanted in [
        "projects_list",
        "tasks_list",
        "task_create",
        "task_message",
        "ci_runs",
        "project_file_read",
        "project_change",
        "change_merge",
    ] {
        assert!(names.contains(&wanted), "{wanted} in {names:?}");
    }
    for withheld in [
        "secret_update",
        "secret_hosts_answer",
        "secret_delete",
        "variables_set",
        "variable_delete",
        "cd_deploy",
        "project_import",
        "onboarding_start",
        "onboarding_skip",
        "sandbox_task_create",
        "secret_copy_to_env",
    ] {
        assert!(!names.contains(&withheld), "{withheld} in {names:?}");
    }
    let create = tools
        .iter()
        .find(|t| t["name"] == "task_create")
        .ok_or("task_create")?;
    assert_eq!(create["inputSchema"]["required"][0], "project");
    let init = mcp(&f, &token, "initialize", json!({})).await?;
    let instructions = init["result"]["instructions"].as_str().unwrap_or_default();
    assert!(instructions.contains("Global Superagent"), "{instructions}");
    assert!(instructions.contains("in sync"), "{instructions}");

    // Every sandbox of alice's, at once, as alice.
    let (projects, is_error) = cockpit_tool(&f, &token, "projects_list", json!({})).await?;
    assert!(!is_error, "{projects}");
    let shown: Vec<&str> = projects["projects"]
        .as_array()
        .ok_or("projects")?
        .iter()
        .filter_map(|p| p["project"].as_str())
        .collect();
    assert!(
        shown.contains(&"acme/fixture") && shown.contains(&"acme/website"),
        "{shown:?}"
    );
    let (created, is_error) = cockpit_tool(
        &f,
        &token,
        "task_create",
        json!({"project": "acme/website", "prompt": "Update the API client after the fixture change"}),
    )
    .await?;
    assert!(!is_error, "{created}");
    let task =
        f.h.store
            .task(&TaskId::parse(
                created["task"]["task_id"].as_str().ok_or("task_id")?,
            )?)
            .await?
            .ok_or("task")?;
    assert_eq!(task.sandbox_id, sandbox("s2"));
    assert_eq!(task.created_by, UserId::parse("u-alice")?);
    assert_eq!(task.origin, TaskOrigin::Agent);

    // A project alice cannot see is not found; a withheld tool is refused when called.
    seed_sandbox(&f.h, "s3", "private", "u-bob").await?;
    let (hidden, is_error) =
        cockpit_tool(&f, &token, "tasks_list", json!({"project": "acme/private"})).await?;
    assert!(is_error, "{hidden}");
    assert!(hidden.to_string().contains("not found"), "{hidden}");
    let (deploy, is_error) = cockpit_tool(
        &f,
        &token,
        "cd_deploy",
        json!({"project": "acme/website", "environment": "dev"}),
    )
    .await?;
    assert!(is_error, "{deploy}");
    assert!(deploy.to_string().contains("unknown tool"), "{deploy}");

    // The sandbox Superagent's bearer reaches no other sandbox this way.
    cockpit_id(&f, &f.alice).await?;
    let sandbox_token =
        f.h.app
            .cockpit_mcp_token(&UserId::parse("u-alice")?, &sandbox("s1"))
            .ok_or("sandbox token")?;
    let (refused, is_error) = cockpit_tool(&f, &sandbox_token, "projects_list", json!({})).await?;
    assert!(is_error, "{refused}");
    assert!(refused.to_string().contains("unknown tool"), "{refused}");
    // Nor does the global bearer get the sandbox tools.
    let (refused, is_error) = cockpit_tool(&f, &token, "sandboxes_list", json!({})).await?;
    assert!(is_error, "{refused}");

    // Its changes are audited as the Global Superagent's.
    let audits = f.h.store.automation_audit("s2", 50).await?;
    assert!(
        audits.iter().any(|a| a.action == "remote.tool"
            && a.actor == "superagent:alice"
            && a.detail
                .as_deref()
                .is_some_and(|d| d.contains("global-superagent"))),
        "{audits:?}"
    );

    // A disabled person's bearer is refused.
    use yulius_app::ports::store::UserStore;
    f.h.store
        .update_user_flags(&UserId::parse("u-alice")?, Some(Some(f.h.app.now())), None)
        .await?;
    let refused = mcp(&f, &token, "tools/list", json!({})).await?;
    assert_eq!(refused["error"]["code"], -32001, "{refused}");

    task_rests(&f, &task.id).await?;
    f.h.app.end_all_shells().await;
    Ok(())
}

/// AC-GSA-3 (GSA-005): its host sandbox removed, it ends; the next open is hosted by another
/// sandbox and resumes the same conversation.
#[tokio::test]
async fn the_global_superagent_outlives_its_host_sandbox() -> TestResult {
    let f = fixture().await?;
    second_sandbox(&f).await?;
    f.h.github.set_perm("alice", "acme/fixture", perms("admin"));
    f.h.github.set_perm("alice", "acme/website", perms("admin"));
    let served = serve(f.h.router.clone()).await?;
    let id = global_id(&f, &f.alice).await?;
    let mut socket = attach(served.addr, &id, &f.alice).await?;
    let mut screen = String::new();
    read_until(&mut socket, &mut screen, "ready").await?;
    let config = PathBuf::from(printed(&screen, "config")?);
    let cwd = printed(&screen, "cwd")?;
    let project = yulius_harness::paths::claude_project_dir(&config, Path::new(&cwd));
    conversation(&project, "across-sandboxes", 1_000)?;
    let hosts = globals(&f);
    assert_eq!(hosts.len(), 1);
    let host = hosts[0].0.clone();

    let removed = call(
        &f.h.router,
        Method::DELETE,
        &format!("/api/sandboxes/{host}"),
        None,
        &f.alice,
    )
    .await?;
    assert_eq!(removed.status, StatusCode::NO_CONTENT, "{:?}", removed.body);
    wait_for("the Global Superagent to end with its host", || {
        globals(&f).is_empty()
    })
    .await?;

    let again = global_id(&f, &f.alice).await?;
    assert_ne!(again, id);
    let hosts = globals(&f);
    assert_eq!(hosts.len(), 1);
    assert_ne!(hosts[0].0, host, "hosted by the other sandbox");
    let mut socket = attach(served.addr, &again, &f.alice).await?;
    let mut screen = String::new();
    read_until(&mut socket, &mut screen, "ready").await?;
    // The line is wider than the terminal: its start is enough.
    let args = printed(&screen, "cockpit-args")?;
    assert!(
        args.starts_with("[--resume across-sandboxes --strict-mcp-config --tools  --model"),
        "{args}"
    );
    f.h.app.end_all_shells().await;
    Ok(())
}

/// GSA-005: a person who lost write access to its host gets it back under another sandbox.
#[tokio::test]
async fn a_global_superagent_whose_host_was_lost_moves_to_another_sandbox() -> TestResult {
    let f = fixture().await?;
    second_sandbox(&f).await?;
    let id = global_id(&f, &f.alice).await?;
    let host = globals(&f).first().ok_or("host")?.0.clone();
    let repo = if host == "s1" {
        "acme/fixture"
    } else {
        "acme/website"
    };
    f.h.github.set_perm("alice", repo, perms("read"));
    f.h.clock
        .0
        .fetch_add(f.h.app.config.permission_ttl_ms + 1, Ordering::SeqCst);
    let again = global_id(&f, &f.alice).await?;
    assert_ne!(again, id, "a new session");
    let hosts = globals(&f);
    assert_eq!(hosts.len(), 1, "the old one ended: {hosts:?}");
    assert_ne!(hosts[0].0, host);
    f.h.app.end_all_shells().await;
    Ok(())
}

/// AC-GSA-4 (GSA-002): on a contained server it is planned with only the Yulius MCP and its read
/// tools pre-allowed, its config in its own folder, and contained in a bubblewrap root of that
/// folder without a sandbox holder, identity, repository or the person's home.
#[tokio::test]
async fn a_contained_global_superagent_has_a_root_of_its_own() -> TestResult {
    use yulius_app::ports::session::Containment;

    let (h, recorder, alice) = contained_harness().await?;
    let data = h.app.config.data_dir.clone();
    let own = data.join("users/u-alice/superagent");
    let conversations = yulius_harness::paths::claude_project_dir(
        &own.join(".claude"),
        &std::fs::canonicalize(&data)?.join("users/u-alice/superagent/work"),
    );
    conversation(&conversations, "earlier", 1_000)?;
    let resp = call(&h.router, Method::POST, "/api/superagent", None, &alice).await?;
    assert_eq!(
        resp.status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "{:?}",
        resp.body
    );
    let launch = recorder
        .cockpit
        .lock()
        .map_err(|_| "poisoned")?
        .clone()
        .ok_or("no launch planned")?;
    assert_eq!(launch.config_dir, own.join(".claude"));
    assert_eq!(launch.config_dir_host, own.join(".claude"));
    assert_eq!(launch.cwd, std::fs::canonicalize(own.join("work"))?);
    assert_eq!(launch.resume.as_deref(), Some("earlier"));
    assert!(launch.contained);
    assert!(launch.mcp.is_empty() && launch.project_mcp_servers.is_empty());
    let read = launch.only_yulius.ok_or("only Yulius")?;
    assert!(read.iter().any(|t| t == "projects_list"), "{read:?}");
    assert!(!read.iter().any(|t| t == "task_create"), "{read:?}");

    match h.app.global_containment(&own) {
        Containment::Bubblewrap {
            sandbox_root,
            home,
            writable,
            binds,
            identity,
            join,
            ..
        } => {
            assert_eq!(sandbox_root, own);
            assert_eq!(home, own);
            assert!(writable.is_empty() && binds.is_empty());
            assert!(identity.is_none() && join.is_none());
        }
        Containment::None => return Err("not contained".into()),
    }
    Ok(())
}

/// GSA-005: a Global Superagent running across a server restart is adopted as one: the same
/// session, its bearer still served the remote tools, its folder its own.
#[tokio::test]
async fn a_running_global_superagent_is_adopted_after_a_restart() -> TestResult {
    let f = fixture().await?;
    let id = global_id(&f, &f.alice).await?;
    let token = global_token(&f, "u-alice")?;
    f.h.app.stop_workspace_services().await;

    let g = fixture_with(true, Some(f.data.clone())).await?;
    assert_eq!(g.h.app.adopt_shells().await, (1, 0));
    assert_eq!(global_token(&g, "u-alice")?, token, "its bearer survives");
    assert_eq!(g.h.app.cockpit_refresh_state(&id), "none");
    let tools = mcp(&g, &token, "tools/list", json!({})).await?;
    let names = tools["result"]["tools"].to_string();
    assert!(names.contains("projects_list"), "{tools}");
    assert_eq!(global_id(&g, &g.alice).await?, id, "the same session");
    assert_ne!(cockpit_id(&g, &g.alice).await?, id, "not the sandbox one");
    let info =
        g.h.app
            .sandbox_shells(&sandbox("s1"))
            .into_iter()
            .find(|shell| shell.id() == id)
            .ok_or("adopted")?;
    assert!(info.is_global_superagent());
    g.h.app.end_all_shells().await;
    Ok(())
}

/// GSA-005 (SPEC-074): a usage limit moves the Global Superagent to a subscription with capacity
/// as the sandbox one: still global, in its own folder, continuing the conversation.
#[tokio::test]
async fn a_usage_limit_moves_the_global_superagent_and_keeps_it_global() -> TestResult {
    let f = fixture_with(false, None).await?;
    insert_account(&f.h, "acc-a", "token-a", 5.0).await?;
    insert_account(&f.h, "acc-b", "token-b", 60.0).await?;
    let served = serve(f.h.router.clone()).await?;
    let id = global_id(&f, &f.alice).await?;
    let mut socket = attach(served.addr, &id, &f.alice).await?;
    let mut screen = String::new();
    read_until(&mut socket, &mut screen, "ready").await?;
    assert_eq!(printed(&screen, "token")?, "token-a");
    let project = project_of(&screen)?;
    std::fs::create_dir_all(&project)?;
    std::fs::write(
        project.join("conv-1.jsonl"),
        limited_conversation(1_791_400_000),
    )?;

    let exit = read_message(&mut socket, "exit").await?;
    assert_eq!(exit["status"], "switched", "{exit}");
    let mut again = None;
    wait_for("the Global Superagent on another subscription", || {
        again = globals(&f)
            .into_iter()
            .map(|(_, id)| id)
            .find(|other| *other != id);
        again.is_some()
    })
    .await?;
    let again = again.ok_or("moved")?;
    let mut socket = attach(served.addr, &again, &f.alice).await?;
    let mut screen = String::new();
    read_until(&mut socket, &mut screen, "ready").await?;
    let args = printed(&screen, "cockpit-args")?;
    assert!(
        args.starts_with("[--resume conv-1 --strict-mcp-config --tools  --model"),
        "{args}"
    );
    assert_eq!(printed(&screen, "token")?, "token-b");
    assert!(printed(&screen, "bearer")?.starts_with("gs_"));
    assert_eq!(global_id(&f, &f.alice).await?, again, "the panel finds it");
    f.h.app.end_all_shells().await;
    Ok(())
}

/// GSA-005 (SPEC-186 SAV-006): one started by a Yulius before an update that changed what it is
/// served is adopted, then started again once idle as a Global Superagent, resuming its
/// conversation; its old bearer ends with it.
#[tokio::test]
async fn a_global_superagent_started_before_an_update_restarts_as_global() -> TestResult {
    let f = fixture().await?;
    let id = global_id(&f, &f.alice).await?;
    let old_token = global_token(&f, "u-alice")?;
    let config = f.data.join("users/u-alice/superagent/.claude");
    let work = std::fs::canonicalize(f.data.join("users/u-alice/superagent/work"))?;
    let project = yulius_harness::paths::claude_project_dir(&config, &work);
    std::fs::create_dir_all(&project)?;
    std::fs::write(
        project.join("conv-1.jsonl"),
        [
            prompt_record("u0", None, -60),
            answer_record("a0", "u0", -59),
        ]
        .concat(),
    )?;
    predate_launch_digest(&f, &id)?;
    f.h.app.stop_workspace_services().await;

    let g = fixture_with(true, Some(f.data.clone())).await?;
    let served = serve(g.h.router.clone()).await?;
    assert_eq!(g.h.app.adopt_shells().await, (1, 0));
    let mut again = None;
    wait_for("the Global Superagent to start again", || {
        again = globals(&g)
            .into_iter()
            .map(|(_, other)| other)
            .find(|other| *other != id);
        again.is_some()
    })
    .await?;
    let again = again.ok_or("restarted")?;
    let mut socket = attach(served.addr, &again, &g.alice).await?;
    let mut screen = String::new();
    read_until(&mut socket, &mut screen, "ready").await?;
    let args = printed(&screen, "cockpit-args")?;
    assert!(
        args.starts_with("[--resume conv-1 --strict-mcp-config --tools  --model"),
        "{args}"
    );
    let token = global_token(&g, "u-alice")?;
    assert_ne!(token, old_token);
    let refused = mcp(&g, &old_token, "tools/list", json!({})).await?;
    assert_eq!(refused["error"]["code"], -32001, "{refused}");
    g.h.app.end_all_shells().await;
    Ok(())
}
