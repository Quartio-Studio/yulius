//! Repository discovery, unpublished refs and bundles (IMP-001, IMP-002, AC-IMP-2).

use std::fs;

use yulius_import::{RepoError, UnpublishedRef, create_bundle, discover_repo, unpublished_refs};

use crate::support::{RepoFixture, TestResult, git, write};

#[test]
fn discovers_github_remote_head_and_worktrees_from_a_subdirectory() -> TestResult {
    let fixture = RepoFixture::new()?;
    let claude_worktree = fixture.repo.join(".claude").join("worktrees").join("feat");
    let outside_worktree = fixture.temp.path().join("work").join("demo-wt");
    git(
        &fixture.repo,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "feat",
            &claude_worktree.to_string_lossy(),
        ],
    )?;
    git(
        &fixture.repo,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "other",
            &outside_worktree.to_string_lossy(),
        ],
    )?;
    let subdir = fixture.repo.join("dir");

    let info = discover_repo(&subdir)?;

    assert_eq!(info.root, fixture.root()?);
    assert_eq!(info.owner, "Quartio-Studio");
    assert_eq!(info.name, "demo");
    assert_eq!(info.remote_url, "https://github.com/Quartio-Studio/demo");
    assert_eq!(info.head_branch.as_deref(), Some("main"));
    assert_eq!(info.head_sha, git(&fixture.repo, &["rev-parse", "HEAD"])?);
    let mut worktrees = info.worktrees.clone();
    worktrees.sort();
    let mut expected = vec![
        fs::canonicalize(&claude_worktree)?,
        fs::canonicalize(&outside_worktree)?,
    ];
    expected.sort();
    assert_eq!(worktrees, expected);
    assert_eq!(info.checkout_paths().len(), 3);
    assert_eq!(info.descriptor().owner, "Quartio-Studio");

    // From a linked worktree the root is that worktree and the main checkout is listed.
    let from_worktree = discover_repo(&claude_worktree)?;
    assert_eq!(from_worktree.root, fs::canonicalize(&claude_worktree)?);
    assert_eq!(from_worktree.head_branch.as_deref(), Some("feat"));
    assert!(from_worktree.worktrees.contains(&fixture.root()?));
    Ok(())
}

#[test]
fn refuses_non_git_directories() -> TestResult {
    let temp = tempfile::tempdir()?;
    assert!(matches!(
        discover_repo(temp.path()),
        Err(RepoError::NotGitRepository(_))
    ));
    assert!(matches!(
        discover_repo(&temp.path().join("missing")),
        Err(RepoError::NotGitRepository(_))
    ));
    Ok(())
}

#[test]
fn refuses_repositories_without_a_remote() -> TestResult {
    let temp = tempfile::tempdir()?;
    git(temp.path(), &["init", "-q", "-b", "main"])?;
    write(&temp.path().join("a.txt"), "a")?;
    git(temp.path(), &["add", "a.txt"])?;
    git(temp.path(), &["commit", "-q", "-m", "a"])?;
    assert!(matches!(
        discover_repo(temp.path()),
        Err(RepoError::NoRemote)
    ));
    Ok(())
}

#[test]
fn refuses_non_github_remotes_without_echoing_credentials() -> TestResult {
    let temp = tempfile::tempdir()?;
    git(temp.path(), &["init", "-q", "-b", "main"])?;
    git(
        temp.path(),
        &[
            "remote",
            "add",
            "origin",
            "https://deploy:hunter2@gitlab.com/team/app.git",
        ],
    )?;
    let error = discover_repo(temp.path());
    let Err(error @ RepoError::NotGitHub { .. }) = error else {
        return Err(format!("expected NotGitHub, got {error:?}").into());
    };
    let message = error.to_string();
    assert!(message.contains("gitlab.com/team/app.git"), "{message}");
    assert!(!message.contains("hunter2"), "{message}");
    Ok(())
}

#[test]
fn prefers_a_github_remote_when_origin_is_elsewhere() -> TestResult {
    let temp = tempfile::tempdir()?;
    git(temp.path(), &["init", "-q", "-b", "main"])?;
    git(
        temp.path(),
        &["remote", "add", "origin", "git@gitlab.com:team/app.git"],
    )?;
    git(
        temp.path(),
        &[
            "remote",
            "add",
            "github",
            "git@github.com:Quartio-Studio/app.git",
        ],
    )?;
    write(&temp.path().join("a.txt"), "a")?;
    git(temp.path(), &["add", "a.txt"])?;
    git(temp.path(), &["commit", "-q", "-m", "a"])?;
    let info = discover_repo(temp.path())?;
    assert_eq!(info.remote_url, "https://github.com/Quartio-Studio/app");
    Ok(())
}

#[test]
fn refuses_repositories_without_commits_and_reports_detached_head() -> TestResult {
    let temp = tempfile::tempdir()?;
    git(temp.path(), &["init", "-q", "-b", "main"])?;
    git(
        temp.path(),
        &["remote", "add", "origin", "https://github.com/o/r.git"],
    )?;
    assert!(matches!(
        discover_repo(temp.path()),
        Err(RepoError::NoCommits)
    ));

    let fixture = RepoFixture::new()?;
    git(&fixture.repo, &["checkout", "-q", "--detach", "HEAD"])?;
    assert_eq!(discover_repo(&fixture.repo)?.head_branch, None);
    Ok(())
}

#[test]
fn bundles_only_unpublished_branches_deterministically() -> TestResult {
    let fixture = RepoFixture::new()?;
    // `published` is pushed; `wip` points at a pushed commit; `feature` has local commits.
    git(&fixture.repo, &["checkout", "-q", "-b", "published"])?;
    fixture.commit_file("p.txt", "p", "published work")?;
    git(&fixture.repo, &["push", "-q", "origin", "published"])?;
    git(&fixture.repo, &["branch", "wip", "main"])?;
    git(&fixture.repo, &["checkout", "-q", "-b", "feature", "main"])?;
    fixture.commit_file("f1.txt", "one", "feature one")?;
    let feature_head = fixture.commit_file("f2.txt", "two", "feature two")?;

    let refs = unpublished_refs(&fixture.repo)?;
    assert_eq!(
        refs,
        vec![UnpublishedRef {
            branch: "feature".to_owned(),
            sha: feature_head.clone(),
        }]
    );

    let out = fixture.temp.path().join("repo.bundle");
    let Some(bundle) = create_bundle(&fixture.repo, &refs, &out)? else {
        return Err("expected a bundle".into());
    };
    assert_eq!(bundle.name, "repo.bundle");
    assert_eq!(bundle.size, fs::metadata(&out)?.len());

    // A clone of the remote (what the sandbox has) can verify and fetch the bundle.
    let clone = fixture.temp.path().join("sandbox-clone");
    git(
        fixture.temp.path(),
        &[
            "clone",
            "-q",
            &fixture.bare.to_string_lossy(),
            &clone.to_string_lossy(),
        ],
    )?;
    git(&clone, &["bundle", "verify", "-q", &out.to_string_lossy()])?;
    let heads = git(&clone, &["bundle", "list-heads", &out.to_string_lossy()])?;
    assert_eq!(heads, format!("{feature_head} refs/heads/feature"));
    git(
        &clone,
        &[
            "fetch",
            "-q",
            &out.to_string_lossy(),
            "refs/heads/feature:refs/heads/imported",
        ],
    )?;
    assert_eq!(git(&clone, &["rev-parse", "imported"])?, feature_head);

    // Same input, same bytes: a re-run would not upload the bundle again (IMP-006).
    let again = fixture.temp.path().join("again.bundle");
    let second = create_bundle(&fixture.repo, &refs, &again)?;
    assert_eq!(second.map(|file| file.sha256), Some(bundle.sha256));
    Ok(())
}

#[test]
fn nothing_to_bundle_when_everything_is_pushed() -> TestResult {
    let fixture = RepoFixture::new()?;
    assert!(unpublished_refs(&fixture.repo)?.is_empty());
    let out = fixture.temp.path().join("repo.bundle");
    assert_eq!(create_bundle(&fixture.repo, &[], &out)?, None);
    assert!(!out.exists());
    Ok(())
}
