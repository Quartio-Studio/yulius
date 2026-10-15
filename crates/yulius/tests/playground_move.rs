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
use yulius_app::ports::github::GhRepo;
use yulius_app::ports::store::{SandboxStore, TaskStore};
use yulius_domain::ids::{SandboxId, TaskId};
use yulius_domain::task::TaskStatus;

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
