//! Projects end to end over HTTP with real Git on local remotes: listings, import, create from
//! a template, onboarding gate and removal (AC-PRJ-1, AC-PRJ-3..5, AC-IMP-2, AC-IMP-4).

mod support;

use axum::http::{Method, StatusCode};
use serde_json::{Value, json};
use support::repos::{OWNER, World, git};
use support::{TestResult, call};
use yulius_app::model::{AttemptRecord, LaunchKind, Sealed, TaskOrigin, TaskRecord};
use yulius_app::ports::store::{InstanceStore, SandboxStore, TaskFieldUpdate, TaskStore};
use yulius_domain::harness::Harness;
use yulius_domain::ids::{AttemptId, SandboxId, TaskId};
use yulius_domain::spec_format::{TemplateKind, scaffold_files};
use yulius_domain::task::{AttemptState, TaskStatus};
use yulius_domain::time::Timestamp;

type Error = Box<dyn std::error::Error>;

fn text(value: &Value) -> &str {
    value.as_str().unwrap_or_default()
}

async fn import(w: &World, creds: &support::Creds, repo: &str) -> Result<support::Resp, Error> {
    call(
        &w.h.router,
        Method::POST,
        "/api/sandboxes",
        Some(json!({"mode": "import", "repo": repo})),
        creds,
    )
    .await
}

fn scaffold(name: &str) -> Vec<(String, String)> {
    scaffold_files(name, TemplateKind::Empty)
}

fn as_refs(files: &[(String, String)]) -> Vec<(&str, &str)> {
    files
        .iter()
        .map(|(path, contents)| (path.as_str(), contents.as_str()))
        .collect()
}

async fn task_record(
    w: &World,
    sandbox: &str,
    id: &str,
    status: TaskStatus,
) -> Result<TaskRecord, Error> {
    Ok(w.h
        .store
        .insert_task(&TaskRecord {
            id: TaskId::parse(id)?,
            sandbox_id: SandboxId::parse(sandbox)?,
            number: 0,
            title: format!("task {id}"),
            prompt: "do it".into(),
            harness: Harness::ClaudeCode,
            model: None,
            effort: None,
            workflow_id: None,
            status,
            activity: None,
            branch: None,
            worktree_path: None,
            spec_path: None,
            origin: TaskOrigin::Ui,
            automation_run: None,
            created_by: yulius_domain::ids::UserId::parse("u-alice")?,
            created_at: Timestamp(1),
            updated_at: Timestamp(1),
            closed_at: None,
            archived_at: None,
            queued_at: Some(Timestamp(1)),
            gate_blocks: 0,
            wake_enabled: true,
            mode: yulius_app::model::TaskMode::Prompt,
        })
        .await?)
}

#[tokio::test]
async fn listings_show_templates_and_permitted_owner_repositories() -> TestResult {
    let w = World::new("org").await?;
    let (_, alice) = w.user("alice", false).await?;
    w.github.add_installation_repo(OWNER, "shop", true);
    w.github.add_installation_repo(OWNER, "docs", false);
    w.github.add_installation_repo(OWNER, "secret", true);
    w.github.add_installation_repo("someone-else", "shop", true);
    w.grant("alice", "shop", "admin");
    w.grant("alice", "docs", "push");
    w.grant("alice", "someone-else/shop", "admin");

    let templates = call(&w.h.router, Method::GET, "/api/templates", None, &alice).await?;
    assert_eq!(templates.status, StatusCode::OK);
    let ids: Vec<&str> = templates
        .body
        .as_array()
        .ok_or("array")?
        .iter()
        .map(|t| text(&t["id"]))
        .collect();
    assert_eq!(ids, vec!["empty", "web", "python-data", "rust-service"]);
    assert!(!text(&templates.body[1]["description"]).is_empty());

    let repos = call(
        &w.h.router,
        Method::GET,
        "/api/github/repositories",
        None,
        &alice,
    )
    .await?;
    assert_eq!(repos.status, StatusCode::OK, "{:?}", repos.body);
    assert_eq!(
        repos.body,
        json!([
            {"owner": OWNER, "name": "docs", "private": false, "access": "write", "already_sandbox": false},
            {"owner": OWNER, "name": "shop", "private": true, "access": "admin", "already_sandbox": false}
        ]),
        "only owner repositories the caller can see"
    );
    // The installation id was looked up once and stored.
    assert_eq!(
        w.h.store.instance().await?.and_then(|i| i.installation_id),
        Some(7)
    );
    call(
        &w.h.router,
        Method::GET,
        "/api/github/repositories",
        None,
        &alice,
    )
    .await?;
    assert_eq!(
        w.github
            .installation_lookups
            .load(std::sync::atomic::Ordering::SeqCst),
        1
    );

    // After linking, the listing marks the repository as a sandbox.
    w.remote(OWNER, "shop", &[("README.md", "# Shop\n")], &[])?;
    assert_eq!(
        import(&w, &alice, "Quartio-Studio/shop").await?.status,
        StatusCode::CREATED
    );
    let repos = call(
        &w.h.router,
        Method::GET,
        "/api/github/repositories",
        None,
        &alice,
    )
    .await?;
    assert_eq!(repos.body[1]["already_sandbox"], true);
    Ok(())
}

/// ACC-012 (F 2026-10-09): the superadmin, without a GitHub identity, lists every owner
/// repository the App installation can access with admin access and imports one through the
/// App; repositories the App cannot reach stay out of reach.
#[tokio::test]
async fn the_superadmin_lists_and_imports_repositories_through_the_app() -> TestResult {
    let w = World::new("org").await?;
    let owner = support::setup_superadmin_creds(&w.h).await?;
    w.github.add_installation_repo(OWNER, "shop", true);
    w.github.add_installation_repo(OWNER, "docs", false);
    w.github.add_installation_repo("someone-else", "shop", true);

    let repos = call(
        &w.h.router,
        Method::GET,
        "/api/github/repositories",
        None,
        &owner,
    )
    .await?;
    assert_eq!(repos.status, StatusCode::OK, "{:?}", repos.body);
    assert_eq!(
        repos.body,
        json!([
            {"owner": OWNER, "name": "docs", "private": false, "access": "admin", "already_sandbox": false},
            {"owner": OWNER, "name": "shop", "private": true, "access": "admin", "already_sandbox": false}
        ])
    );

    w.remote(OWNER, "shop", &[("README.md", "# Shop\n")], &[])?;
    let shop = import(&w, &owner, "quartio-studio/SHOP").await?;
    assert_eq!(shop.status, StatusCode::CREATED, "{:?}", shop.body);
    assert_eq!(shop.body["slug"], "shop");
    assert_eq!(shop.body["access"], "admin");
    let ghost = import(&w, &owner, "Quartio-Studio/ghost").await?;
    assert_eq!(
        ghost.status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "{:?}",
        ghost.body
    );
    assert!(text(&ghost.body["error"]["message"]).contains("cannot access it"));
    let foreign = import(&w, &owner, "someone-else/shop").await?;
    assert_eq!(foreign.status, StatusCode::UNPROCESSABLE_ENTITY);

    let repos = call(
        &w.h.router,
        Method::GET,
        "/api/github/repositories",
        None,
        &owner,
    )
    .await?;
    assert_eq!(repos.body[1]["already_sandbox"], true);
    assert_eq!(repos.body[1]["access"], "admin");
    Ok(())
}

#[tokio::test]
async fn import_links_owner_repositories_with_profiles_ports_and_onboarding() -> TestResult {
    let w = World::new("org").await?;
    let (_, alice) = w.user("alice", false).await?;
    let (_, bob) = w.user("bob", false).await?;
    w.remote(OWNER, "shop", &[("README.md", "# Shop\n")], &[])?;
    w.remote(
        OWNER,
        "product",
        &[("README.md", "# Product\n")],
        &["quartio"],
    )?;
    let docs = scaffold("ready");
    w.remote(OWNER, "ready", &as_refs(&docs), &["dev"])?;
    for repo in ["shop", "product", "ready"] {
        w.grant("alice", repo, "admin");
    }
    w.grant("bob", "shop", "push");

    // IMP-002: permission and ownership are enforced with guidance.
    let refused = import(&w, &bob, "Quartio-Studio/shop").await?;
    assert_eq!(refused.status, StatusCode::FORBIDDEN);
    assert!(text(&refused.body["error"]["message"]).contains("admin or maintain"));
    let foreign = import(&w, &alice, "someone-else/shop").await?;
    assert_eq!(foreign.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(
        text(&foreign.body["error"]["message"]).contains("only repositories of Quartio-Studio")
    );
    let ghost = import(&w, &alice, "Quartio-Studio/ghost").await?;
    assert_eq!(ghost.status, StatusCode::UNPROCESSABLE_ENTITY);
    let malformed = import(&w, &alice, "no-slash").await?;
    assert_eq!(malformed.status, StatusCode::UNPROCESSABLE_ENTITY);

    let shop = import(&w, &alice, "Quartio-Studio/shop").await?;
    assert_eq!(shop.status, StatusCode::CREATED, "{:?}", shop.body);
    let shop = shop.body;
    assert_eq!(shop["slug"], "shop");
    assert_eq!(shop["access"], "admin");
    assert_eq!(shop["default_branch"], "main");
    assert_eq!(shop["port_base"], 20000);
    assert_eq!(shop["port_count"], 20);
    assert_eq!(shop["profile"]["preset"], "standard");
    assert_eq!(
        shop["onboarding"], "preparing",
        "the repository lacks the docs format; its preparation task starts by itself"
    );
    let shop_id = text(&shop["id"]).to_owned();
    let clone = w.data_dir().join("sandboxes").join(&shop_id).join("repo");
    assert!(clone.join("README.md").exists(), "cloned into the sandbox");
    assert!(
        w.git
            .calls()
            .iter()
            .any(|c| c == "clone https://github.com/Quartio-Studio/shop.git token=inst-7"),
        "cloned with an installation token: {:?}",
        w.git.calls()
    );
    let again = import(&w, &alice, "quartio-studio/SHOP").await?;
    assert_eq!(
        again.status,
        StatusCode::CONFLICT,
        "one repository, one sandbox"
    );

    // AC-PRJ-5 and the known-repository preset.
    let product = import(&w, &alice, "Quartio-Studio/product").await?.body;
    assert_eq!(product["port_base"], 20020, "non-overlapping port ranges");
    assert_eq!(product["profile"]["preset"], "product");
    assert_eq!(product["profile"]["integration_branch"], "quartio");
    let ready = import(&w, &alice, "Quartio-Studio/ready").await?.body;
    assert_eq!(
        ready["onboarding"], "not_needed",
        "a validating project needs no preparation"
    );
    assert_eq!(ready["profile"]["preset"], "standard");

    // AC-IMP-4: the preparation task exists and blocks other tasks.
    let tasks = call(
        &w.h.router,
        Method::GET,
        &format!("/api/sandboxes/{shop_id}/tasks"),
        None,
        &alice,
    )
    .await?;
    let tasks = tasks.body.as_array().ok_or("tasks")?.clone();
    assert_eq!(tasks.len(), 1);
    assert_eq!(tasks[0]["origin"], "onboarding");
    assert_eq!(tasks[0]["title"], "Prepare this repository for Yulius");
    assert_eq!(
        tasks[0]["status"], "waiting_capacity",
        "started without approval; with no subscription connected yet it waits for the first one (ROT-010)"
    );
    assert_eq!(tasks[0]["archived"], false);
    let prep =
        w.h.store
            .task(&TaskId::parse(text(&tasks[0]["id"]))?)
            .await?
            .ok_or("task")?;
    assert!(prep.prompt.contains("yulius.toml: is missing"));
    assert!(prep.prompt.contains("git-crypt") && prep.prompt.contains("pull request"));
    let record =
        w.h.store
            .sandbox(&SandboxId::parse(&shop_id)?)
            .await?
            .ok_or("sandbox")?;
    assert_eq!(record.onboarding_task_id, Some(prep.id.clone()));

    let blocked = call(
        &w.h.router,
        Method::POST,
        &format!("/api/sandboxes/{shop_id}/tasks"),
        Some(json!({"prompt": "add a cart", "harness": "claude"})),
        &alice,
    )
    .await?;
    assert_eq!(blocked.status, StatusCode::CONFLICT);
    assert!(text(&blocked.body["error"]["message"]).contains("being prepared"));

    // Approving is no longer needed: it changes nothing while the task is preparing.
    let approve = call(
        &w.h.router,
        Method::POST,
        &format!("/api/sandboxes/{shop_id}/onboarding/approve"),
        None,
        &alice,
    )
    .await?;
    assert_eq!(approve.status, StatusCode::OK, "{:?}", approve.body);
    assert_eq!(approve.body["onboarding"], "preparing");
    assert_eq!(
        w.h.store.task(&prep.id).await?.ok_or("task")?.status,
        TaskStatus::WaitingCapacity,
        "paused until a subscription is added"
    );

    // Skipping needs admin; afterwards ordinary tasks are accepted.
    let skip = format!("/api/sandboxes/{shop_id}/onboarding/skip");
    let denied = call(&w.h.router, Method::POST, &skip, None, &bob).await?;
    assert_eq!(denied.status, StatusCode::FORBIDDEN);
    let skipped = call(&w.h.router, Method::POST, &skip, None, &alice).await?;
    assert_eq!(skipped.status, StatusCode::OK);
    assert_eq!(skipped.body["onboarding"], "skipped");
    let created = call(
        &w.h.router,
        Method::POST,
        &format!("/api/sandboxes/{shop_id}/tasks"),
        Some(json!({"prompt": "add a cart", "harness": "claude"})),
        &alice,
    )
    .await?;
    assert_eq!(created.status, StatusCode::CREATED, "{:?}", created.body);
    let nothing = call(
        &w.h.router,
        Method::POST,
        &format!("/api/sandboxes/{shop_id}/onboarding/approve"),
        None,
        &alice,
    )
    .await?;
    assert_eq!(
        nothing.status,
        StatusCode::CONFLICT,
        "nothing left to approve"
    );
    Ok(())
}

#[tokio::test]
async fn onboarding_becomes_ready_when_the_preparation_task_lands() -> TestResult {
    let w = World::new("org").await?;
    let (_, alice) = w.user("alice", false).await?;
    w.remote(
        OWNER,
        "product",
        &[("README.md", "# Product\n")],
        &["quartio"],
    )?;
    w.grant("alice", "product", "admin");
    let product = import(&w, &alice, "Quartio-Studio/product").await?.body;
    let sid = SandboxId::parse(text(&product["id"]))?;
    let record = w.h.store.sandbox(&sid).await?.ok_or("sandbox")?;
    let task_id = record.onboarding_task_id.clone().ok_or("prep task")?;

    // Started by itself at import: preparing (it waits for a subscription here).
    assert_eq!(w.h.app.refresh_onboarding(&sid).await?, "preparing");

    // The task ran (an attempt exists) and closed, but the docs are still missing.
    w.h.store
        .insert_attempt(&AttemptRecord {
            id: AttemptId::parse("a-prep")?,
            task_id: task_id.clone(),
            seq: 1,
            harness: Harness::ClaudeCode,
            account_id: None,
            provider_session_id: None,
            launch_kind: LaunchKind::New,
            state: AttemptState::Exited,
            run_dir: "/tmp/none".into(),
            token_hash: "x".into(),
            host_token: Sealed(vec![0]),
            ptyhost_pid: None,
            child_pid: None,
            started_at: Timestamp(2),
            ended_at: Some(Timestamp(3)),
            exit_code: Some(0),
            exit_reason: None,
        })
        .await?;
    let worktree = w.scratch.path().join("prep-worktree");
    std::fs::create_dir_all(&worktree)?;
    w.h.store
        .update_task(
            &task_id,
            &TaskFieldUpdate {
                worktree_path: Some(worktree.to_string_lossy().into_owned()),
                ..TaskFieldUpdate::default()
            },
            Timestamp(4),
        )
        .await?;
    assert_eq!(w.h.app.refresh_onboarding(&sid).await?, "preparing");
    let blocked = call(
        &w.h.router,
        Method::POST,
        &format!("/api/sandboxes/{sid}/tasks"),
        Some(json!({"prompt": "x", "harness": "claude"})),
        &alice,
    )
    .await?;
    assert_eq!(
        blocked.status,
        StatusCode::CONFLICT,
        "still blocked while preparing"
    );

    // The task closes; the agent delivered the docs in its worktree: ready, tasks unblocked.
    let prep = w.h.store.task(&task_id).await?.ok_or("prep")?;
    assert!(
        w.h.store
            .transition_task(&task_id, Some(prep.status), TaskStatus::Closed, None, Timestamp(5))
            .await?
    );
    assert_eq!(
        w.h.app.refresh_onboarding(&sid).await?,
        "preparing",
        "a closed task without the docs is not ready"
    );
    for (path, contents) in scaffold("product") {
        let target = worktree.join(path);
        std::fs::create_dir_all(target.parent().ok_or("parent")?)?;
        std::fs::write(target, contents)?;
    }
    assert_eq!(w.h.app.refresh_onboarding(&sid).await?, "ready");
    let view = call(
        &w.h.router,
        Method::GET,
        &format!("/api/sandboxes/{sid}"),
        None,
        &alice,
    )
    .await?;
    assert_eq!(view.body["onboarding"], "ready");
    let created = call(
        &w.h.router,
        Method::POST,
        &format!("/api/sandboxes/{sid}/tasks"),
        Some(json!({"prompt": "x", "harness": "claude"})),
        &alice,
    )
    .await?;
    assert_eq!(created.status, StatusCode::CREATED);
    Ok(())
}

#[tokio::test]
async fn create_from_template_pushes_a_validating_scaffold_with_rulesets() -> TestResult {
    let w = World::new("org").await?;
    let (_, admin) = w.user("alice", true).await?;
    let (_, bob) = w.user("bob", false).await?;
    w.grant("alice", "new-shop", "admin");
    let body = json!({"mode": "create", "name": "new-shop", "template": "web", "description": "Demo shop"});

    let denied = call(
        &w.h.router,
        Method::POST,
        "/api/sandboxes",
        Some(body.clone()),
        &bob,
    )
    .await?;
    assert_eq!(
        denied.status,
        StatusCode::FORBIDDEN,
        "creation is for instance admins"
    );
    let bad_template = call(
        &w.h.router,
        Method::POST,
        "/api/sandboxes",
        Some(json!({"mode": "create", "name": "x", "template": "cobol"})),
        &admin,
    )
    .await?;
    assert_eq!(bad_template.status, StatusCode::UNPROCESSABLE_ENTITY);
    let bad_name = call(
        &w.h.router,
        Method::POST,
        "/api/sandboxes",
        Some(json!({"mode": "create", "name": "../x", "template": "web"})),
        &admin,
    )
    .await?;
    assert_eq!(bad_name.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(w.github.created().is_empty(), "nothing created on refusals");

    let created = call(
        &w.h.router,
        Method::POST,
        "/api/sandboxes",
        Some(body.clone()),
        &admin,
    )
    .await?;
    assert_eq!(created.status, StatusCode::CREATED, "{:?}", created.body);
    let sandbox = created.body;
    assert_eq!(sandbox["slug"], "new-shop");
    assert_eq!(sandbox["onboarding"], "not_needed");
    assert_eq!(sandbox["profile"]["preset"], "standard");
    assert_eq!(sandbox["default_branch"], "main");
    assert_eq!(sandbox["access"], "admin");

    let created_repos = w.github.created();
    assert_eq!(created_repos.len(), 1);
    assert_eq!(created_repos[0].owner, OWNER);
    assert_eq!(created_repos[0].name, "new-shop");
    assert!(created_repos[0].is_org);
    assert_eq!(
        created_repos[0].token, "inst-7",
        "org repositories use the installation"
    );

    // Rulesets: pull requests required on the integration and production branches.
    let rulesets = w.github.rulesets();
    let protected: Vec<(&str, bool, Vec<String>)> = rulesets
        .iter()
        .map(|(repo, rule, _)| {
            assert_eq!(repo, "Quartio-Studio/new-shop");
            (
                rule.branch.as_str(),
                rule.require_pr,
                rule.merge_methods.clone(),
            )
        })
        .collect();
    assert_eq!(
        protected,
        vec![
            ("dev", true, vec!["squash".to_owned()]),
            ("main", true, vec!["merge".to_owned()])
        ]
    );

    // The remote holds the scaffold on main and dev, pushed with the installation token.
    let remote = w
        .scratch
        .path()
        .join("remotes")
        .join(OWNER)
        .join("new-shop.git");
    let main = git(&remote, &["rev-parse", "refs/heads/main"])?;
    let dev = git(&remote, &["rev-parse", "refs/heads/dev"])?;
    assert_eq!(main, dev, "dev starts at the scaffold commit");
    let tree = git(&remote, &["ls-tree", "-r", "--name-only", "main"])?;
    for path in [
        "AGENTS.md",
        "CLAUDE.md",
        "yulius.toml",
        "docs/architecture.md",
        "package.json",
    ] {
        assert!(
            tree.lines().any(|l| l == path),
            "{path} missing from {tree}"
        );
    }
    let calls = w.git.calls();
    assert!(
        calls.iter().any(|c| c == "push main->main token=inst-7"),
        "{calls:?}"
    );
    assert!(
        calls.iter().any(|c| c == "push main->dev token=inst-7"),
        "{calls:?}"
    );

    // The sandbox clone validates against the enforced format (AC-PRJ-3).
    let sid = text(&sandbox["id"]).to_owned();
    let sandbox_dir = w.data_dir().join("sandboxes").join(&sid);
    let (report, _) = yulius_app::onboarding::validate_checkout(&sandbox_dir.join("repo"));
    assert!(report.is_valid(), "{:?}", report.problems);
    assert!(
        !sandbox_dir.join("scaffold").exists(),
        "scratch clone removed"
    );

    let duplicate = call(
        &w.h.router,
        Method::POST,
        "/api/sandboxes",
        Some(body),
        &admin,
    )
    .await?;
    assert_eq!(duplicate.status, StatusCode::CONFLICT);
    Ok(())
}

#[tokio::test]
async fn create_in_a_user_account_uses_the_owner_token() -> TestResult {
    let w = World::new("user").await?;
    let (_, admin) = w.user("alice", true).await?;
    w.grant("alice", "tool", "admin");
    let body = json!({"mode": "create", "name": "tool", "template": "empty"});
    let missing_owner = call(
        &w.h.router,
        Method::POST,
        "/api/sandboxes",
        Some(body.clone()),
        &admin,
    )
    .await?;
    assert_eq!(missing_owner.status, StatusCode::CONFLICT);
    assert!(text(&missing_owner.body["error"]["message"]).contains("must sign in"));

    w.user(OWNER, false).await?;
    let created = call(
        &w.h.router,
        Method::POST,
        "/api/sandboxes",
        Some(body),
        &admin,
    )
    .await?;
    assert_eq!(created.status, StatusCode::CREATED, "{:?}", created.body);
    let repos = w.github.created();
    assert_eq!(repos.len(), 1);
    assert!(!repos[0].is_org);
    assert_eq!(repos[0].token, format!("token-{OWNER}"));
    assert!(
        w.github
            .rulesets()
            .iter()
            .all(|(_, _, token)| token == &format!("token-{OWNER}"))
    );
    Ok(())
}

#[tokio::test]
async fn remove_stops_tasks_and_keeps_the_repository() -> TestResult {
    let w = World::new("org").await?;
    let (_, alice) = w.user("alice", false).await?;
    let (_, bob) = w.user("bob", false).await?;
    let remote = w.remote(OWNER, "shop", &[("README.md", "# Shop\n")], &[])?;
    w.grant("alice", "shop", "admin");
    w.grant("bob", "shop", "push");
    let shop = import(&w, &alice, "Quartio-Studio/shop").await?.body;
    let sid = text(&shop["id"]).to_owned();
    let running = task_record(&w, &sid, "t-running", TaskStatus::Running).await?;
    let queued = task_record(&w, &sid, "t-queued", TaskStatus::Queued).await?;

    let path = format!("/api/sandboxes/{sid}");
    let denied = call(&w.h.router, Method::DELETE, &path, None, &bob).await?;
    assert_eq!(denied.status, StatusCode::FORBIDDEN, "removal needs admin");
    let removed = call(&w.h.router, Method::DELETE, &path, None, &alice).await?;
    assert_eq!(removed.status, StatusCode::NO_CONTENT);

    let gone = call(&w.h.router, Method::GET, &path, None, &alice).await?;
    assert_eq!(gone.status, StatusCode::NOT_FOUND);
    let list = call(&w.h.router, Method::GET, "/api/sandboxes", None, &alice).await?;
    assert_eq!(list.body, json!([]));
    let running = w.h.store.task(&running.id).await?.ok_or("task")?;
    assert_eq!(
        running.status,
        TaskStatus::Stopping,
        "live task asked to stop"
    );
    let queued = w.h.store.task(&queued.id).await?.ok_or("task")?;
    assert!(matches!(queued.status, TaskStatus::Stopped(_)));
    // PRJ-009: an export of the tasks is preserved (owner-only).
    let exports: Vec<_> = std::fs::read_dir(w.data_dir().join("exports"))?
        .filter_map(Result::ok)
        .map(|e| e.path())
        .collect();
    assert_eq!(exports.len(), 1, "{exports:?}");
    let doc: serde_json::Value = serde_json::from_slice(&std::fs::read(&exports[0])?)?;
    assert_eq!(doc["sandbox"]["repository"], "Quartio-Studio/shop");
    let exported: Vec<&str> = doc["tasks"]
        .as_array()
        .ok_or("tasks")?
        .iter()
        .filter_map(|t| t["id"].as_str())
        .collect();
    assert!(
        exported.contains(&"t-running") && exported.contains(&"t-queued"),
        "every task of the sandbox is exported (incl. the preparation task): {exported:?}"
    );
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&exports[0])?.permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }
    // PRJ-009: GitHub repository and files are kept.
    assert!(git(&remote, &["rev-parse", "refs/heads/main"]).is_ok());
    assert!(w.github.created().is_empty());
    assert!(
        w.data_dir()
            .join("sandboxes")
            .join(&sid)
            .join("repo/README.md")
            .exists()
    );

    // The repository can be linked again; the old slug and ports stay reserved.
    let again = import(&w, &alice, "Quartio-Studio/shop").await?;
    assert_eq!(again.status, StatusCode::CREATED);
    assert_eq!(again.body["slug"], "shop-2");
    assert_eq!(again.body["port_base"], 20020);
    Ok(())
}
