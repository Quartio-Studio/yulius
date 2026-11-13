//! The real Git adapter against local repositories: the behaviors the application relies on
//! (clone of empty and existing repositories, worktrees on new and existing branches, commits on
//! unborn branches, explicit push refspecs, bundle import names, the pre-push guard).

use std::path::Path;
use std::process::Command;

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
    let adapter = crate::support::confined_git();
    let url = origin.to_string_lossy().into_owned();

    // An empty repository clones; the first commit lands on an unborn branch.
    let repo = root.join("sandboxes/s1/repo");
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
    let worktrees = root.join("sandboxes/s1/worktrees");
    std::fs::create_dir_all(&worktrees)?;
    let wt = worktrees.join("t1");
    adapter.add_worktree(&repo, &wt, "feat/1-x", &base).await?;
    adapter
        .install_push_guard(&repo, &wt, &["dev".into(), "main".into()])
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
    let adapter = crate::support::confined_git();
    let mut branches = adapter.import_bundle(&dst, &bundle, "import/alice").await?;
    branches.sort();
    assert_eq!(branches, ["import/alice/feature/x", "import/alice/main"]);
    Ok(())
}

/// SPEC-219 ENV-001: an environment run's head comes from the repository's GitHub URL, with the
/// token, straight into Yulius's own mirror (the test seam stands a local bare repository in for
/// `https://github.com/`).
#[tokio::test]
async fn an_environment_head_is_fetched_from_github_into_the_mirror() -> TestResult {
    let dir = tempfile::tempdir()?;
    let root = dir.path().canonicalize()?;
    let origin = root.join("github/acme/app.git");
    std::fs::create_dir_all(&origin)?;
    git(&origin, &["init", "--bare", "-q", "-b", "main"])?;
    let work = root.join("work");
    git(&root, &["clone", "-q", &origin.to_string_lossy(), "work"])?;
    git(&work, &["commit", "-q", "--allow-empty", "-m", "init"])?;
    git(&work, &["push", "-q", "origin", "HEAD:refs/heads/dev"])?;
    let adapter = crate::support::confined_git().with_github_base(&root.join("github"));
    let mirror = root.join("cd/environments.git");
    let head = adapter
        .mirror_fetch_head(
            &mirror,
            "https://github.com/acme/app.git",
            "dev",
            Some(&token()),
        )
        .await?;
    assert_eq!(head, git(&origin, &["rev-parse", "refs/heads/dev"])?);
    Ok(())
}

/// A sandbox (`<root>/sandbox`: its clone `repo` with no tracked file and Git told to ignore
/// everything, and the worktree `worktrees/t1`), and a folder of someone else's at
/// `<root>/other/victim`. The victim's `.git` points back to the worktree's record, and Git
/// ignores everything: that is what Git checks through the path before it deletes. Here it
/// stands in for the swap that a racing agent makes again during Git's own run, between Git's
/// checks and its deletion through the path.
#[cfg(target_os = "linux")]
struct Swapped {
    _dir: tempfile::TempDir,
    sandbox: std::path::PathBuf,
    repo: std::path::PathBuf,
    worktrees: std::path::PathBuf,
    wt: std::path::PathBuf,
    victim: std::path::PathBuf,
}

#[cfg(target_os = "linux")]
fn swapped() -> Result<Swapped, Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let root = dir.path().canonicalize()?;
    // As on a server: `<data>/sandboxes/<id>` holds the clone and its worktrees.
    let sandbox = root.join("sandboxes/s1");
    let repo = sandbox.join("repo");
    std::fs::create_dir_all(&repo)?;
    git(&repo, &["init", "-q", "-b", "main"])?;
    git(&repo, &["commit", "-q", "--allow-empty", "-m", "init"])?;
    std::fs::write(repo.join(".git/info/exclude"), "*\n")?;
    let worktrees = sandbox.join("worktrees");
    std::fs::create_dir_all(&worktrees)?;
    let wt = worktrees.join("t1");
    git(
        &repo,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "feat/1",
            wt.to_str().ok_or("path")?,
        ],
    )?;
    let victim = root.join("other/victim");
    std::fs::create_dir_all(victim.join("src"))?;
    std::fs::write(victim.join("src/work.txt"), "someone else's\n")?;
    let record = repo.join(".git/worktrees/t1");
    std::fs::write(
        victim.join(".git"),
        format!("gitdir: {}\n", record.display()),
    )?;
    Ok(Swapped {
        _dir: dir,
        sandbox,
        repo,
        worktrees,
        wt,
        victim,
    })
}

#[cfg(target_os = "linux")]
impl Swapped {
    /// Swaps the worktree for a link to the victim once the server's checks passed and nothing
    /// is removed yet, as a contained agent of the sandbox can. The flag says the swap happened:
    /// a removal that never reaches the pause point (Git's own) must fail the test too.
    fn swap_after_the_checks(&self) -> std::sync::Arc<std::sync::atomic::AtomicBool> {
        let swapped = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let flag = swapped.clone();
        let (wt, aside, victim) = (
            self.wt.clone(),
            self.worktrees.join("t1.aside"),
            self.victim.clone(),
        );
        yulius_app::race::on(
            yulius_app::race::Point::WorktreeRemove,
            &self.sandbox,
            move || {
                std::fs::rename(&wt, &aside).ok();
                std::os::unix::fs::symlink(&victim, &wt).ok();
                flag.store(true, std::sync::atomic::Ordering::SeqCst);
            },
        );
        swapped
    }
}

/// #844 (SPEC-401 VCD-001): a clean worktree swapped for a link once its checks passed has the
/// link removed, never what it names.
#[cfg(target_os = "linux")]
#[tokio::test]
async fn a_worktree_swapped_for_a_link_during_its_removal_touches_nothing_behind_it() -> TestResult
{
    let s = swapped()?;
    let record = s.repo.join(".git/worktrees/t1");
    let swapped = s.swap_after_the_checks();
    let adapter = crate::support::confined_git();
    let removed = adapter.remove_worktree(&s.repo, &s.wt, &s.worktrees).await;
    assert!(
        s.victim.join("src/work.txt").exists(),
        "the folder behind the link was deleted ({removed:?})"
    );
    assert!(
        swapped.load(std::sync::atomic::Ordering::SeqCst),
        "the race was not run"
    );
    assert!(removed?, "the worktree (now the link) and its record go");
    assert!(!s.wt.exists() && !record.exists());
    // The worktree's own path, not `t1`: the random temporary folder's name may contain that.
    let listed = git(&s.repo, &["worktree", "list", "--porcelain"])?;
    let wt = format!("worktree {}", s.wt.display());
    assert!(!listed.lines().any(|line| line == wt), "{listed}");
    assert!(git(&s.repo, &["branch", "--list", "feat/1"])?.contains("feat/1"));
    Ok(())
}

/// #844 (SPEC-401 VCD-001): a task's deletion discards its worktree; one swapped for a link once
/// the checks passed has the link removed, never the folder behind it.
#[cfg(target_os = "linux")]
#[tokio::test]
async fn a_discarded_worktree_swapped_for_a_link_touches_nothing_behind_it() -> TestResult {
    let s = swapped()?;
    std::fs::write(s.wt.join("draft.txt"), "uncommitted\n")?;
    let swapped = s.swap_after_the_checks();
    let adapter = crate::support::confined_git();
    let discarded = adapter
        .discard_worktree(&s.repo, &s.wt, &s.worktrees, Some("feat/1"))
        .await;
    assert!(
        s.victim.join("src/work.txt").exists(),
        "the folder behind the link was deleted ({discarded:?})"
    );
    assert!(
        swapped.load(std::sync::atomic::Ordering::SeqCst),
        "the race was not run"
    );
    discarded?;
    assert!(!s.wt.exists() && !s.repo.join(".git/worktrees/t1").exists());
    assert!(!git(&s.repo, &["branch", "--list", "feat/1"])?.contains("feat/1"));
    Ok(())
}
