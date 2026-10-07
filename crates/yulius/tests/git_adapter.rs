//! The real Git adapter against local repositories: the behaviors the application relies on
//! (clone of empty and existing repositories, worktrees on new and existing branches, commits on
//! unborn branches, explicit push refspecs, bundle import names, the pre-push guard).

use std::path::Path;
use std::process::Command;

use yulius::wiring::git::GitAdapter;
use yulius_app::model::Secret;
use yulius_app::ports::git::GitPort;

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn git(dir: &Path, args: &[&str]) -> Result<String, Box<dyn std::error::Error>> {
    let out = Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@example.com")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@example.com")
        .output()?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).into_owned().into());
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_owned())
}

fn token() -> Secret {
    Secret("unused-for-local-remotes".into())
}

#[tokio::test]
async fn clones_commits_pushes_and_reuses_branches() -> TestResult {
    let dir = tempfile::tempdir()?;
    let root = dir.path().canonicalize()?;
    let origin = root.join("origin.git");
    std::fs::create_dir_all(&origin)?;
    git(&origin, &["init", "--bare", "-q", "-b", "main"])?;
    let adapter = GitAdapter::new("git");
    let url = origin.to_string_lossy().into_owned();

    // An empty repository clones; the first commit lands on an unborn branch.
    let repo = root.join("sandbox/repo");
    adapter.sync_clone(&url, &repo, &token()).await?;
    std::fs::write(repo.join("README.md"), "# x\n")?;
    assert!(
        adapter
            .commit_all(&repo, "init", "Yulius", "yulius@example.com")
            .await?
    );
    assert!(
        !adapter
            .commit_all(&repo, "nothing", "Yulius", "yulius@example.com")
            .await?
    );
    adapter.push(&repo, "main", "main", &token()).await?;
    git(&repo, &["branch", "dev"])?;
    adapter.push(&repo, "dev", "dev", &token()).await?;
    // An existing clone fetches.
    adapter.sync_clone(&url, &repo, &token()).await?;
    assert_eq!(adapter.default_branch(&repo).await?, "main");
    assert!(adapter.branch_exists(&repo, "dev").await?);
    assert!(!adapter.branch_exists(&repo, "nope").await?);

    // New branch from a SHA base, then the same branch again after the worktree is removed.
    let base = git(&repo, &["rev-parse", "HEAD"])?;
    let worktrees = root.join("sandbox/worktrees");
    std::fs::create_dir_all(&worktrees)?;
    let wt = worktrees.join("t1");
    adapter.add_worktree(&repo, &wt, "feat/1-x", &base).await?;
    adapter
        .install_push_guard(&wt, &["dev".into(), "main".into()])
        .await?;
    adapter
        .set_identity(&wt, "Alice", "alice@example.com")
        .await?;
    std::fs::write(wt.join("a.txt"), "a\n")?;
    assert!(adapter.has_changes(&wt).await?);
    assert!(
        adapter
            .commit_all(&wt, "feat: a", "Alice", "alice@example.com")
            .await?
    );
    let (ahead, behind) = adapter.ahead_behind(&wt, "origin/dev", "HEAD").await?;
    assert_eq!((ahead, behind), (1, 0));
    let stat = adapter.diff_stat(&wt, "origin/dev", "HEAD").await?;
    assert_eq!((stat.files, stat.insertions), (1, 1));
    // The guard refuses protected branches; the task branch pushes.
    let guarded = Command::new("git")
        .args(["push", "-q", "origin", "HEAD:refs/heads/main"])
        .current_dir(&wt)
        .output()?;
    assert!(!guarded.status.success(), "pre-push guard must refuse main");
    adapter.push(&wt, "feat/1-x", "feat/1-x", &token()).await?;
    assert!(adapter.remove_worktree(&repo, &wt, &worktrees).await?);
    adapter.add_worktree(&repo, &wt, "feat/1-x", &base).await?;
    assert!(
        wt.join("a.txt").is_file(),
        "existing branch reused with its commits"
    );
    let log = adapter.log(&wt, "HEAD", 5).await?;
    assert_eq!(log.first().map(|c| c.subject.as_str()), Some("feat: a"));
    Ok(())
}

#[tokio::test]
async fn bundles_import_under_the_prefix() -> TestResult {
    let dir = tempfile::tempdir()?;
    let root = dir.path().canonicalize()?;
    let src = root.join("src");
    std::fs::create_dir_all(&src)?;
    git(&src, &["init", "-q", "-b", "main"])?;
    std::fs::write(src.join("f"), "1\n")?;
    git(&src, &["add", "f"])?;
    git(&src, &["commit", "-q", "-m", "one"])?;
    git(&src, &["branch", "feature/x"])?;
    let bundle = root.join("repo.bundle");
    git(
        &src,
        &["bundle", "create", "-q", &bundle.to_string_lossy(), "--all"],
    )?;
    let dst = root.join("dst");
    std::fs::create_dir_all(&dst)?;
    git(&dst, &["init", "-q", "-b", "main"])?;
    let adapter = GitAdapter::new("git");
    let mut branches = adapter.import_bundle(&dst, &bundle, "import/alice").await?;
    branches.sort();
    assert_eq!(branches, ["import/alice/feature/x", "import/alice/main"]);
    Ok(())
}
