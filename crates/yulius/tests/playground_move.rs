//! Moving a playground into a GitHub repository (SPEC-176, ADR-0142): its branches and tags are
//! pushed to an empty repository of the instance owner, its clone's `origin` and its record become
//! GitHub's, and its tasks stay. Real Git and a real store; the fake GitHub answers permissions,
//! and `https://github.com/` is a directory of local bare repositories
//! (`GitAdapter::with_github_base`).

use crate::support;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::http::{Method, StatusCode};
use serde_json::{Value, json};
use support::repos::git;
use support::{Creds, Harness, HarnessOptions, TestResult, call, harness_with};
use yulius::wiring::git::GitAdapter;
use yulius_app::ci::CiRunRecord;
use yulius_app::model::{AttemptRecord, LaunchKind, StepRecord};
use yulius_app::ports::github::GhRepo;
use yulius_app::ports::store::{CoordinationStore, SandboxStore, TaskStore};
use yulius_app::ports::store_ext::CiStore;
use yulius_domain::ci::{CiRef, GateSource, RunState};
use yulius_domain::harness::Harness as Agent;
use yulius_domain::ids::{AttemptId, SandboxId, StepId, TaskId};
use yulius_domain::task::AttemptState;
use yulius_domain::task::TaskStatus;
use yulius_domain::time::Timestamp;

type BoxResult<T> = Result<T, Box<dyn std::error::Error>>;

const OWNER: &str = "Quartio-Studio";

/// A harness with GitHub connected (owner `Quartio-Studio`), the real Git adapter whose
/// `https://github.com/` is the returned directory, the superadmin and alice (GitHub-linked).
struct World {
    h: Harness,
    superadmin: Creds,
    alice: Creds,
    github: PathBuf,
    _github_dir: tempfile::TempDir,
}

async fn world() -> BoxResult<World> {
    let github_dir = tempfile::tempdir()?;
    let github = github_dir.path().canonicalize()?;
    let h = harness_with(HarnessOptions {
        git: Some(Arc::new(GitAdapter::new("git").with_github_base(&github))),
        ..HarnessOptions::default()
    })?;
    let superadmin = support::setup_superadmin_creds(&h).await?;
    support::configure_github_app(&h).await?;
    let alice = support::seed_github_user(&h, "u-alice", "alice").await?;
    Ok(World {
        h,
        superadmin,
        alice,
        github,
        _github_dir: github_dir,
    })
}

impl World {
    /// An empty GitHub repository `Quartio-Studio/<name>`.
    fn empty_repository(&self, name: &str) -> BoxResult<PathBuf> {
        let path = self.github.join(OWNER).join(format!("{name}.git"));
        std::fs::create_dir_all(&path)?;
        git(
            &path,
            &["init", "--quiet", "--bare", "--initial-branch=main"],
        )?;
        Ok(path)
    }

    /// A playground of `creds` from the Empty template, with a second branch and a tag in its
    /// `origin`; returns its view and its `origin`.
    async fn playground(&self, creds: &Creds, name: &str) -> BoxResult<(Value, PathBuf)> {
        let made = call(
            &self.h.router,
            Method::POST,
            "/api/sandboxes",
            Some(json!({"mode": "playground", "name": name, "template": "empty"})),
            creds,
        )
        .await?;
        assert_eq!(made.status, StatusCode::CREATED, "{:?}", made.body);
        let origin = self
            .h
            .app
            .config
            .data_dir
            .join("playgrounds")
            .join(format!("{}.git", made.body["id"].as_str().ok_or("id")?));
        git(&origin, &["branch", "feat/1-idea", "dev"])?;
        git(&origin, &["tag", "v0.1", "main"])?;
        Ok((made.body, origin))
    }

    async fn move_it(
        &self,
        creds: &Creds,
        sandbox: &Value,
        name: &str,
    ) -> BoxResult<support::Resp> {
        call(
            &self.h.router,
            Method::POST,
            &format!(
                "/api/sandboxes/{}/move-to-github",
                sandbox["id"].as_str().ok_or("id")?
            ),
            Some(json!({"name": name})),
            creds,
        )
        .await
    }

    fn clone_of(&self, sandbox: &Value) -> BoxResult<PathBuf> {
        Ok(self
            .h
            .app
            .config
            .data_dir
            .join("sandboxes")
            .join(sandbox["id"].as_str().ok_or("id")?)
            .join("repo"))
    }

    async fn record(&self, sandbox: &Value) -> BoxResult<yulius_app::model::SandboxRecord> {
        let id = SandboxId::parse(sandbox["id"].as_str().ok_or("id")?)?;
        Ok(self.h.store.sandbox(&id).await?.ok_or("no sandbox")?)
    }
}

/// Every branch and tag of a repository with its commit, sorted.
fn refs(repo: &Path) -> BoxResult<Vec<String>> {
    let out = git(
        repo,
        &[
            "for-each-ref",
            "--format=%(refname) %(objectname)",
            "refs/heads",
            "refs/tags",
        ],
    )?;
    Ok(out.lines().map(str::to_owned).collect())
}

/// MPG-001, MPG-003, MPG-004: the maker, admin of the empty repository on GitHub, moves the
/// playground. GitHub holds its branches and tags, the clone's `origin` is GitHub, the bare
/// `origin` is gone, the sandbox is a GitHub sandbox with the same slug and its task stays.
#[tokio::test]
async fn a_playground_moves_into_an_empty_github_repository() -> TestResult {
    let w = world().await?;
    let (sandbox, origin) = w.playground(&w.alice, "alices-lab").await?;
    let sid = sandbox["id"].as_str().ok_or("id")?.to_owned();
    let task =
        support::seed_task(&w.h, "t-kept", &sid, "u-alice", TaskStatus::Closed, None).await?;
    let now = w.h.app.now();
    let step = StepRecord {
        id: StepId::parse("st-1")?,
        task_id: task.id.clone(),
        ordinal: 1,
        title: "An idea".into(),
        status: "done".into(),
        note: None,
        updated_by: Some("system".into()),
        updated_at: Some(now),
        effort: 1,
    };
    w.h.store
        .replace_steps(&task.id, &[step], "system", now)
        .await?;
    w.h.store
        .insert_attempt(&AttemptRecord {
            id: AttemptId::parse("at-kept")?,
            task_id: task.id.clone(),
            seq: 1,
            harness: Agent::ClaudeCode,
            account_id: None,
            provider_session_id: Some("session-1".into()),
            launch_kind: LaunchKind::New,
            state: AttemptState::Exited,
            run_dir: "/nonexistent/run".into(),
            token_hash: "x".into(),
            host_token: w.h.app.deps.crypto.seal(&[1u8; 32])?,
            ptyhost_pid: None,
            child_pid: None,
            started_at: now,
            ended_at: Some(now),
            exit_code: Some(0),
            exit_reason: None,
        })
        .await?;
    let before = refs(&origin)?;
    assert_eq!(
        before.len(),
        4,
        "main, dev, feat/1-idea and v0.1: {before:?}"
    );
    let github = w.empty_repository("alices-lab")?;
    w.h.github.set_perm(
        "alice",
        &format!("{OWNER}/alices-lab"),
        support::perms("admin"),
    );

    let moved = w.move_it(&w.alice, &sandbox, "alices-lab").await?;
    assert_eq!(moved.status, StatusCode::OK, "{:?}", moved.body);
    assert_eq!(moved.body["playground"], false);
    assert_eq!(moved.body["slug"], "alices-lab");
    assert_eq!(moved.body["id"], sid.as_str());
    assert_eq!(moved.body["repo"]["owner"], OWNER);
    assert_eq!(moved.body["repo"]["name"], "alices-lab");
    assert_eq!(moved.body["access"], "admin", "alice's GitHub permission");

    assert_eq!(refs(&github)?, before, "every branch and tag is on GitHub");
    assert!(!origin.exists(), "the bare origin on the server is deleted");
    let clone = w.clone_of(&sandbox)?;
    assert_eq!(
        git(&clone, &["remote", "get-url", "origin"])?.trim(),
        format!("https://github.com/{OWNER}/alices-lab.git")
    );
    git(&clone, &["rev-parse", "--verify", "origin/feat/1-idea"])?;
    let record = w.record(&sandbox).await?;
    assert!(!record.is_playground());
    assert_eq!(record.github_repo_id, Some(1));
    let kept =
        w.h.store
            .task(&TaskId::parse("t-kept")?)
            .await?
            .ok_or("task gone")?;
    assert_eq!(kept.sandbox_id, task.sandbox_id);
    assert_eq!(kept.number, task.number);
    let steps = w.h.store.steps(&task.id).await?;
    assert_eq!(steps.len(), 1, "its steps stay");
    let attempts = w.h.store.attempts(&task.id).await?;
    assert_eq!(
        attempts
            .iter()
            .map(|a| a.provider_session_id.as_deref())
            .collect::<Vec<_>>(),
        vec![Some("session-1")],
        "its session stays"
    );
    let audit = {
        use yulius_app::ports::store_ext::AutomationStore;
        w.h.store.automation_audit(&sid, 100).await?
    };
    let entry = audit
        .iter()
        .find(|e| e.action == "sandbox.moved_to_github")
        .ok_or("no audit row for the move")?;
    assert!(
        entry
            .detail
            .as_deref()
            .is_some_and(|d| d.contains(&format!("{OWNER}/alices-lab"))),
        "{:?}",
        entry.detail
    );

    // It is a GitHub sandbox now: moving it again is refused.
    let again = w.move_it(&w.alice, &sandbox, "alices-lab").await?;
    assert_eq!(again.status, StatusCode::CONFLICT, "{:?}", again.body);
    Ok(())
}

/// MPG-001, MPG-004: only an admin of the playground who could import the repository moves it.
/// The superadmin moves one into a repository the App can access; its maker then has only what
/// GitHub gives them.
#[tokio::test]
async fn only_who_could_import_the_repository_moves_a_playground() -> TestResult {
    let w = world().await?;
    let (sandbox, origin) = w.playground(&w.alice, "alices-lab").await?;
    let github = w.empty_repository("alices-lab")?;
    let repo = format!("{OWNER}/alices-lab");

    // `write` on GitHub is not enough to link a repository (IMP-002).
    w.h.github.set_perm("alice", &repo, support::perms("write"));
    let refused = w.move_it(&w.alice, &sandbox, "alices-lab").await?;
    assert_eq!(refused.status, StatusCode::FORBIDDEN, "{:?}", refused.body);
    // Someone who cannot see the playground cannot move it.
    let bob = support::seed_github_user(&w.h, "u-bob", "bob").await?;
    w.h.github.set_perm("bob", &repo, support::perms("admin"));
    let hidden = w.move_it(&bob, &sandbox, "alices-lab").await?;
    assert_eq!(hidden.status, StatusCode::NOT_FOUND, "{:?}", hidden.body);
    // Not a repository name.
    let bad = w.move_it(&w.alice, &sandbox, "../x").await?;
    assert_eq!(
        bad.status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "{:?}",
        bad.body
    );
    // A repository the App installation cannot access, for the superadmin.
    let unknown = w.move_it(&w.superadmin, &sandbox, "alices-lab").await?;
    assert_eq!(
        unknown.status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "{:?}",
        unknown.body
    );
    assert!(w.record(&sandbox).await?.is_playground());
    assert!(refs(&github)?.is_empty(), "nothing was pushed");
    assert!(origin.exists());

    // The superadmin moves it into a repository the App can access; alice has no permission on
    // it, so she no longer sees her playground.
    if let Ok(mut repos) = w.h.github.installation_repos.lock() {
        repos.push(GhRepo {
            id: 42,
            owner: OWNER.into(),
            name: "alices-lab".into(),
            private: true,
            default_branch: "main".into(),
            html_url: format!("https://github.com/{repo}"),
            clone_url: format!("https://github.com/{repo}.git"),
        });
    }
    w.h.github
        .perms
        .lock()
        .map_err(|_| "lock")?
        .remove(&("alice".to_owned(), repo.clone()));
    let moved = w.move_it(&w.superadmin, &sandbox, "alices-lab").await?;
    assert_eq!(moved.status, StatusCode::OK, "{:?}", moved.body);
    assert_eq!(w.record(&sandbox).await?.github_repo_id, Some(42));
    let path = format!("/api/sandboxes/{}", sandbox["id"].as_str().ok_or("id")?);
    let gone = call(&w.h.router, Method::GET, &path, None, &w.alice).await?;
    assert_eq!(gone.status, StatusCode::NOT_FOUND, "{:?}", gone.body);
    Ok(())
}

/// MPG-001: without the GitHub App, a playground has nowhere to go.
#[tokio::test]
async fn a_move_needs_github_connected() -> TestResult {
    let h = harness_with(HarnessOptions {
        git: Some(Arc::new(GitAdapter::new("git"))),
        ..HarnessOptions::default()
    })?;
    let superadmin = support::setup_superadmin_creds(&h).await?;
    let made = call(
        &h.router,
        Method::POST,
        "/api/sandboxes",
        Some(json!({"mode": "playground", "name": "lab", "template": "empty"})),
        &superadmin,
    )
    .await?;
    assert_eq!(made.status, StatusCode::CREATED, "{:?}", made.body);
    let moved = call(
        &h.router,
        Method::POST,
        &format!(
            "/api/sandboxes/{}/move-to-github",
            made.body["id"].as_str().ok_or("id")?
        ),
        Some(json!({"name": "lab"})),
        &superadmin,
    )
    .await?;
    assert_eq!(moved.status, StatusCode::CONFLICT, "{:?}", moved.body);
    Ok(())
}

/// What a failed or refused move must leave: the record a playground's, the clone's `origin` its
/// bare repository, and that repository's branches and tags as they were.
async fn assert_as_it_was(
    w: &World,
    sandbox: &Value,
    origin: &Path,
    refs_before: &[String],
) -> TestResult {
    assert!(w.record(sandbox).await?.is_playground());
    assert_eq!(refs(origin)?, refs_before);
    assert_eq!(
        git(&w.clone_of(sandbox)?, &["remote", "get-url", "origin"])?.trim(),
        origin.to_string_lossy()
    );
    Ok(())
}

/// MPG-002, MPG-005: a repository with anything of its own is refused before any push.
#[tokio::test]
async fn a_repository_that_is_not_empty_is_refused() -> TestResult {
    let w = world().await?;
    let (sandbox, origin) = w.playground(&w.alice, "alices-lab").await?;
    let before = refs(&origin)?;
    let github = w.empty_repository("alices-lab")?;
    w.h.github.set_perm(
        "alice",
        &format!("{OWNER}/alices-lab"),
        support::perms("admin"),
    );
    // GitHub's "Add a README": one commit of its own on `main`.
    let readme = w.github.join("readme");
    git(
        &w.github,
        &["init", "--quiet", "--initial-branch=main", "readme"],
    )?;
    std::fs::write(readme.join("README.md"), "# alices-lab\n")?;
    git(&readme, &["add", "README.md"])?;
    git(&readme, &["commit", "--quiet", "-m", "Initial commit"])?;
    git(
        &readme,
        &["push", "--quiet", &github.to_string_lossy(), "main"],
    )?;
    let theirs = refs(&github)?;

    let refused = w.move_it(&w.alice, &sandbox, "alices-lab").await?;
    assert_eq!(refused.status, StatusCode::CONFLICT, "{:?}", refused.body);
    assert!(
        refused.body.to_string().contains("not empty"),
        "{:?}",
        refused.body
    );
    assert_eq!(refs(&github)?, theirs, "nothing was pushed");
    assert_as_it_was(&w, &sandbox, &origin, &before).await
}

/// MPG-005: a push GitHub refuses changes nothing.
#[tokio::test]
async fn a_failed_push_leaves_the_playground_as_it_was() -> TestResult {
    let w = world().await?;
    let (sandbox, origin) = w.playground(&w.alice, "alices-lab").await?;
    let before = refs(&origin)?;
    let github = w.empty_repository("alices-lab")?;
    w.h.github.set_perm(
        "alice",
        &format!("{OWNER}/alices-lab"),
        support::perms("admin"),
    );
    // The receiving side refuses any pack (as GitHub refuses a push over its limits).
    git(&github, &["config", "receive.maxInputSize", "1"])?;

    let failed = w.move_it(&w.alice, &sandbox, "alices-lab").await?;
    assert!(
        failed.status.is_server_error(),
        "{:?} {:?}",
        failed.status,
        failed.body
    );
    assert!(refs(&github)?.is_empty(), "an atomic push leaves nothing");
    assert_as_it_was(&w, &sandbox, &origin, &before).await?;
    Ok(())
}

/// MPG-002: a move whose push completed but which failed after it is retried: the repository holds
/// exactly the playground's branches and tags, so nothing more is pushed and the move completes.
#[tokio::test]
async fn a_move_retried_after_its_push_completes() -> TestResult {
    let w = world().await?;
    let (sandbox, origin) = w.playground(&w.alice, "alices-lab").await?;
    let before = refs(&origin)?;
    let github = w.empty_repository("alices-lab")?;
    w.h.github.set_perm(
        "alice",
        &format!("{OWNER}/alices-lab"),
        support::perms("admin"),
    );
    git(
        &origin,
        &[
            "push",
            "--quiet",
            &github.to_string_lossy(),
            "refs/heads/*:refs/heads/*",
            "refs/tags/*:refs/tags/*",
        ],
    )?;

    let moved = w.move_it(&w.alice, &sandbox, "alices-lab").await?;
    assert_eq!(moved.status, StatusCode::OK, "{:?}", moved.body);
    assert_eq!(refs(&github)?, before);
    assert!(!w.record(&sandbox).await?.is_playground());
    Ok(())
}

/// MPG-005: a live or queued task holds the move; one prepared for later does not count.
#[tokio::test]
async fn a_live_task_holds_the_move() -> TestResult {
    let w = world().await?;
    let (sandbox, origin) = w.playground(&w.alice, "alices-lab").await?;
    let sid = sandbox["id"].as_str().ok_or("id")?.to_owned();
    let before = refs(&origin)?;
    let github = w.empty_repository("alices-lab")?;
    w.h.github.set_perm(
        "alice",
        &format!("{OWNER}/alices-lab"),
        support::perms("admin"),
    );
    let running = support::seed_task(
        &w.h,
        "t-running",
        &sid,
        "u-alice",
        TaskStatus::Running,
        None,
    )
    .await?;
    let prepared =
        support::seed_task(&w.h, "t-later", &sid, "u-alice", TaskStatus::Prepared, None).await?;

    let held = w.move_it(&w.alice, &sandbox, "alices-lab").await?;
    assert_eq!(held.status, StatusCode::CONFLICT, "{:?}", held.body);
    let text = held.body.to_string();
    assert!(text.contains(&format!("#{}", running.number)), "{text}");
    assert!(!text.contains(&format!("#{}", prepared.number)), "{text}");
    assert!(refs(&github)?.is_empty(), "nothing was pushed");
    assert_as_it_was(&w, &sandbox, &origin, &before).await
}

/// MPG-005 (review round 1): a move that fails after its push (here the clone's fetch: a ref it
/// must write is locked) points the clone back at its own `origin` and leaves the playground as it
/// was; once the cause is gone, the move is retried and completes with what GitHub already holds.
#[tokio::test]
async fn a_move_failing_after_its_push_points_the_clone_back_and_is_retried() -> TestResult {
    let w = world().await?;
    let (sandbox, origin) = w.playground(&w.alice, "alices-lab").await?;
    let before = refs(&origin)?;
    let github = w.empty_repository("alices-lab")?;
    w.h.github.set_perm(
        "alice",
        &format!("{OWNER}/alices-lab"),
        support::perms("admin"),
    );
    let lock = w
        .clone_of(&sandbox)?
        .join(".git/refs/remotes/origin/feat/1-idea.lock");
    std::fs::create_dir_all(lock.parent().ok_or("parent")?)?;
    std::fs::write(&lock, "")?;

    let failed = w.move_it(&w.alice, &sandbox, "alices-lab").await?;
    assert!(
        failed.status.is_server_error(),
        "{:?} {:?}",
        failed.status,
        failed.body
    );
    assert_eq!(refs(&github)?, before, "the push went through");
    assert_as_it_was(&w, &sandbox, &origin, &before).await?;

    std::fs::remove_file(&lock)?;
    let moved = w.move_it(&w.alice, &sandbox, "alices-lab").await?;
    assert_eq!(moved.status, StatusCode::OK, "{:?}", moved.body);
    assert!(!w.record(&sandbox).await?.is_playground());
    assert!(!origin.exists());
    Ok(())
}

/// MPG-002 (review round 1): a playground that moved on after a failed move (a merge landed) is
/// still taken by its repository: GitHub's refs are behind the playground's, and the non-forcing
/// push fast-forwards them.
#[tokio::test]
async fn a_retry_after_the_playground_moved_on_fast_forwards() -> TestResult {
    let w = world().await?;
    let (sandbox, origin) = w.playground(&w.alice, "alices-lab").await?;
    let github = w.empty_repository("alices-lab")?;
    w.h.github.set_perm(
        "alice",
        &format!("{OWNER}/alices-lab"),
        support::perms("admin"),
    );
    git(
        &origin,
        &[
            "push",
            "--quiet",
            &github.to_string_lossy(),
            "refs/heads/*:refs/heads/*",
            "refs/tags/*:refs/tags/*",
        ],
    )?;
    // A merge landed on `dev` since.
    let tree = git(&origin, &["rev-parse", "dev^{tree}"])?;
    let next = git(
        &origin,
        &[
            "commit-tree",
            tree.trim(),
            "-p",
            "dev",
            "-m",
            "feat: landed after",
        ],
    )?;
    git(&origin, &["update-ref", "refs/heads/dev", next.trim()])?;
    let before = refs(&origin)?;

    let moved = w.move_it(&w.alice, &sandbox, "alices-lab").await?;
    assert_eq!(moved.status, StatusCode::OK, "{:?}", moved.body);
    assert_eq!(refs(&github)?, before, "GitHub's dev fast-forwarded");
    Ok(())
}

/// MPG-001 (review round 1): a repository another sandbox holds is refused.
#[tokio::test]
async fn a_repository_that_is_already_a_sandbox_is_refused() -> TestResult {
    let w = world().await?;
    let (sandbox, origin) = w.playground(&w.alice, "alices-lab").await?;
    let before = refs(&origin)?;
    w.empty_repository("alices-lab")?;
    w.h.github.set_perm(
        "alice",
        &format!("{OWNER}/alices-lab"),
        support::perms("admin"),
    );
    let mut other = w.record(&sandbox).await?;
    other.id = SandboxId::parse("s-other")?;
    other.slug = "other".into();
    other.repo_owner = OWNER.into();
    other.repo_name = "alices-lab".into();
    other.port_base = 21000;
    w.h.store.insert_sandbox(&other).await?;

    let refused = w.move_it(&w.alice, &sandbox, "alices-lab").await?;
    assert_eq!(refused.status, StatusCode::CONFLICT, "{:?}", refused.body);
    assert!(
        refused.body.to_string().contains("/s/other"),
        "{:?}",
        refused.body
    );
    assert_as_it_was(&w, &sandbox, &origin, &before).await
}

/// MPG-005 (review round 1): a CI run of the playground that is queued or running holds the move:
/// it tests a commit that may exist only on this server.
#[tokio::test]
async fn an_active_ci_run_holds_the_move() -> TestResult {
    let w = world().await?;
    let (sandbox, origin) = w.playground(&w.alice, "alices-lab").await?;
    let before = refs(&origin)?;
    let github = w.empty_repository("alices-lab")?;
    w.h.github.set_perm(
        "alice",
        &format!("{OWNER}/alices-lab"),
        support::perms("admin"),
    );
    w.h.store
        .insert_ci_run(
            &CiRunRecord {
                id: "ci-task-1".into(),
                sandbox_id: SandboxId::parse(sandbox["id"].as_str().ok_or("id")?)?,
                reference: CiRef::Task(1),
                sha: "abc1234".into(),
                source: GateSource::ProjectConfig,
                state: RunState::Running,
                reason: None,
                requested_by: "yulius".into(),
                status_for: None,
                status_note: None,
                status_attempts: 0,
                queued_at: Timestamp(1),
                started_at: Some(Timestamp(2)),
                ended_at: None,
            },
            &[],
        )
        .await?;

    let held = w.move_it(&w.alice, &sandbox, "alices-lab").await?;
    assert_eq!(held.status, StatusCode::CONFLICT, "{:?}", held.body);
    assert!(held.body.to_string().contains("CI"), "{:?}", held.body);
    assert!(refs(&github)?.is_empty(), "nothing was pushed");
    assert_as_it_was(&w, &sandbox, &origin, &before).await
}

/// MPG-005 (review round 1): a move outlives its request. The client goes away while the move
/// waits for GitHub; the move still ends, completed, not half done.
#[tokio::test]
async fn a_move_ends_when_its_client_goes_away() -> TestResult {
    let w = world().await?;
    let (sandbox, origin) = w.playground(&w.alice, "alices-lab").await?;
    let before = refs(&origin)?;
    let github = w.empty_repository("alices-lab")?;
    w.h.github.set_perm(
        "alice",
        &format!("{OWNER}/alices-lab"),
        support::perms("admin"),
    );
    let hold = w.h.github.hold_access();
    {
        let request = w.move_it(&w.alice, &sandbox, "alices-lab");
        tokio::pin!(request);
        let entered = tokio::time::timeout(std::time::Duration::from_secs(30), async {
            tokio::select! {
                done = &mut request => Err(format!("the move ended while GitHub was held: {:?}", done.map(|r| r.status).map_err(|e| e.to_string()))),
                () = w.h.github.access_entered.notified() => Ok(()),
            }
        })
        .await?;
        entered?;
        // The request is dropped here: its client went away.
    }
    w.h.github.release_access(&hold);

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    while w.record(&sandbox).await?.is_playground() || origin.exists() {
        assert!(std::time::Instant::now() < deadline, "the move never ended");
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert_eq!(refs(&github)?, before);
    Ok(())
}
