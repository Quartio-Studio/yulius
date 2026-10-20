//! Driving Yulius from a person's own agent (SPEC-247): personal API tokens with scopes, the
//! remote MCP on the public listener, and a laptop-only onboarding over it.

use crate::support;

use axum::http::{Method, StatusCode};
use serde_json::{Value, json};
use support::FakeRemote as _;
use support::repos::World;
use support::{Creds, TestResult, call};
use yulius_app::model::UserRecord;
use yulius_app::ports::store::SandboxStore as _;
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
        2,
        "the refused and the created automation: {entries:?}"
    );
    for (_, actor, detail) in &remote_calls {
        let detail: Value = serde_json::from_str(detail)?;
        assert_eq!(detail["tool"], "automation_create");
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
    let changes = w
        .data_dir()
        .join("sandboxes")
        .join(sid.id.as_str())
        .join("changes");
    assert!(
        std::fs::read_dir(&changes).map(|d| d.count()).unwrap_or(0) == 0,
        "the scratch clone is gone"
    );

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

    // Only change pull requests merge this way.
    let seed = w
        .scratch
        .path()
        .join("seed")
        .join(support::repos::OWNER)
        .join("shop");
    support::repos::git(&seed, &["checkout", "--quiet", "-b", "feat/other"])?;
    std::fs::write(seed.join("other.txt"), "x\n")?;
    support::repos::git(&seed, &["add", "-A"])?;
    support::repos::git(&seed, &["commit", "--quiet", "-m", "other"])?;
    support::repos::git(
        &seed,
        &[
            "push",
            "--quiet",
            origin.to_str().ok_or("path")?,
            "feat/other",
        ],
    )?;
    let other = yulius_app::ports::github::GitHubPort::create_pull(
        w.h.github.as_ref(),
        &yulius_app::model::Secret("t".into()),
        "Quartio-Studio",
        "shop",
        &yulius_app::ports::github::NewPull {
            head: "feat/other".into(),
            base: "dev".into(),
            title: "other".into(),
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
        "{said}"
    );
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
