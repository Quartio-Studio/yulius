//! Behavior of the Git adapter against real temporary repositories and a local bare remote.

use std::error::Error;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use yulius_git::{
    Author, BundleSpec, DiffStat, Git, GitError, ImportRef, MergeInto, RemovalRefusal, SquashInput,
    SquashLanding, TrainInput, TrainLanding, TrainStep, WorktreeRemoval, validate_branch_name,
};

type TestResult = Result<(), Box<dyn Error>>;

struct Fixture {
    _tmp: tempfile::TempDir,
    root: PathBuf,
    home: PathBuf,
    git: Git,
}

impl Fixture {
    fn new() -> Result<Self, Box<dyn Error>> {
        let tmp = tempfile::tempdir()?;
        let root = tmp.path().canonicalize()?;
        let home = root.join("home");
        fs::create_dir(&home)?;
        let git = isolated(Git::new("git"), &home);
        Ok(Self {
            _tmp: tmp,
            root,
            home,
            git,
        })
    }

    fn path(&self, relative: &str) -> PathBuf {
        self.root.join(relative)
    }

    /// Pushes the way an agent does: its own Git, which runs the repository's hooks (the
    /// server's `Git` never does, #54). Returns Git's stderr on failure.
    fn agent_push(&self, cwd: &Path, refspec: &str) -> Result<(), String> {
        self.raw(cwd, &["push", "--quiet", "origin", refspec])
            .map(|_| ())
            .map_err(|err| err.to_string())
    }

    /// Runs git directly for fixture setup (author identity provided by environment).
    fn raw(&self, cwd: &Path, args: &[&str]) -> Result<String, Box<dyn Error>> {
        self.raw_with(cwd, args, true)
    }

    fn raw_with(
        &self,
        cwd: &Path,
        args: &[&str],
        author_env: bool,
    ) -> Result<String, Box<dyn Error>> {
        let mut command = Command::new("git");
        command
            .args(args)
            .current_dir(cwd)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("HOME", &self.home)
            .env_remove("GIT_AUTHOR_NAME")
            .env_remove("GIT_AUTHOR_EMAIL")
            .env_remove("GIT_COMMITTER_NAME")
            .env_remove("GIT_COMMITTER_EMAIL");
        if author_env {
            command
                .env("GIT_AUTHOR_NAME", "Fixture")
                .env("GIT_AUTHOR_EMAIL", "fixture@example.invalid")
                .env("GIT_COMMITTER_NAME", "Fixture")
                .env("GIT_COMMITTER_EMAIL", "fixture@example.invalid");
        }
        let output = command.output()?;
        if !output.status.success() {
            return Err(format!(
                "git {args:?} failed: {}",
                String::from_utf8_lossy(&output.stderr)
            )
            .into());
        }
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
    }

    /// A bare `remote.git` and a `repo` clone-equivalent with one commit pushed to `main`.
    fn repo_with_remote(&self) -> Result<(PathBuf, PathBuf), Box<dyn Error>> {
        let remote = self.path("remote.git");
        let repo = self.path("repo");
        self.git.init(&remote, "main", true)?;
        self.git.init(&repo, "main", false)?;
        fs::write(repo.join("README.md"), "hello\n")?;
        fs::write(repo.join(".gitignore"), "target/\n")?;
        fs::write(repo.join("a.txt"), "one\ntwo\nthree\n")?;
        self.git.commit_all(&repo, "initial", &author())?;
        self.git.remote_add(&repo, "origin", path_str(&remote)?)?;
        self.git.push(&repo, "origin", "refs/heads/main")?;
        Ok((remote, repo))
    }
}

fn isolated(git: Git, home: &Path) -> Git {
    git.with_env("GIT_CONFIG_GLOBAL", "/dev/null")
        .with_env("GIT_CONFIG_NOSYSTEM", "1")
        .with_env("HOME", home.to_string_lossy())
}

fn author() -> Author {
    Author::new("Yulius Test", "yulius@example.invalid")
}

fn path_str(path: &Path) -> Result<&str, Box<dyn Error>> {
    path.to_str().ok_or_else(|| "non-UTF-8 temp path".into())
}

#[test]
fn branch_validation_matches_git_check_ref_format() -> TestResult {
    let fx = Fixture::new()?;
    let outside_repo = fx.path("not-a-repo");
    fs::create_dir(&outside_repo)?;
    let names = [
        "main",
        "feat/login",
        "release-1.2",
        "feat/ünïcode",
        "x@y",
        "a./b",
        "-x",
        "--upload-pack=x",
        "../x",
        "a..b",
        "x/",
        "/x",
        "x.lock",
        "a/b.lock/c",
        "a//b",
        ".a",
        "a/.b",
        "a b",
        "a~b",
        "a^",
        "a:b",
        "a?b",
        "a*b",
        "a[b",
        "a\\b",
        "a\tb",
        "x.",
        "",
    ];
    for name in names {
        let ours = validate_branch_name(name).is_ok();
        let output = Command::new("git")
            .args(["check-ref-format", "--branch", name])
            .current_dir(&outside_repo)
            .env("GIT_CEILING_DIRECTORIES", &fx.root)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .output()?;
        assert_eq!(
            ours,
            output.status.success(),
            "validator disagrees with git check-ref-format for {name:?}"
        );
    }
    // Names Git would expand (`@`, `@{-1}`) or reserve are rejected outright.
    for name in ["@", "@{-1}", "HEAD", "a@{b"] {
        assert!(validate_branch_name(name).is_err(), "{name:?} accepted");
    }
    Ok(())
}

#[test]
fn merging_the_integration_branch_applies_merge_drivers_and_aborts_conflicts() -> TestResult {
    // DLV-203: Yulius brings an armed branch up to date with a local merge, so `merge=union`
    // resolves a shared changes log that a host's server-side update reports as a conflict.
    let fx = Fixture::new()?;
    let (_remote, repo) = fx.repo_with_remote()?;
    fs::write(repo.join(".gitattributes"), "changes.md merge=union\n")?;
    fs::write(repo.join("changes.md"), "# Changes\n\n- first\n")?;
    fx.git.commit_all(&repo, "changes log", &author())?;
    let feature = fx.path("feature");
    fx.git.worktree_add(&repo, &feature, "feat/x", "main")?;
    fx.git
        .set_local_identity(&feature, "Agent", "agent@example.invalid")?;
    fs::write(
        feature.join("changes.md"),
        "# Changes\n\n- feature\n- first\n",
    )?;
    fx.git.commit_all(&feature, "feature entry", &author())?;
    fs::write(
        repo.join("changes.md"),
        "# Changes\n\n- other pull request\n- first\n",
    )?;
    fx.git.commit_all(&repo, "other entry", &author())?;

    let merged = fx.git.merge_into(&feature, "main")?;
    let MergeInto::Merged(head) = merged else {
        return Err(format!("expected a merge, got {merged:?}").into());
    };
    assert_eq!(fx.git.rev_parse(&feature, "HEAD")?, head);
    let log = fs::read_to_string(feature.join("changes.md"))?;
    assert!(
        log.contains("- feature") && log.contains("- other pull request"),
        "{log}"
    );
    assert_eq!(fx.git.merge_into(&feature, "main")?, MergeInto::UpToDate);

    // A real conflict is aborted and named; the worktree is left as it was.
    fs::write(feature.join("a.txt"), "feature\n")?;
    fx.git.commit_all(&feature, "feature a", &author())?;
    fs::write(repo.join("a.txt"), "main\n")?;
    fx.git.commit_all(&repo, "main a", &author())?;
    let before = fx.git.rev_parse(&feature, "HEAD")?;
    assert_eq!(
        fx.git.merge_into(&feature, "main")?,
        MergeInto::Conflict(vec!["a.txt".to_owned()])
    );
    assert_eq!(fx.git.rev_parse(&feature, "HEAD")?, before);
    assert!(fx.git.status_porcelain(&feature)?.is_empty(), "left clean");
    Ok(())
}

#[test]
fn ci_checkouts_created_at_once_all_succeed() -> TestResult {
    // Two CI lanes create their job checkouts at the same time (SPEC-021); each creation prunes
    // stale registrations, which must never remove another creation's half-written one.
    let fx = Fixture::new()?;
    let (_remote, repo) = fx.repo_with_remote()?;
    let sha = fx.git.rev_parse(&repo, "main")?;
    let ci = fx.path("ci");
    fs::create_dir(&ci)?;
    for round in 0..2 {
        let handles: Vec<_> = (0..8)
            .map(|n| {
                let (git, repo, sha) = (Clone::clone(&fx.git), repo.clone(), sha.clone());
                let (ci, path) = (ci.clone(), ci.join("jobs").join(format!("j{round}.{n}")));
                std::thread::spawn(move || git.ci_checkout(&repo, &path, &sha, &ci))
            })
            .collect();
        for handle in handles {
            handle.join().map_err(|_| "a checkout thread panicked")??;
        }
    }
    assert_eq!(fx.git.worktree_list(&repo)?.len(), 1 + 2 * 8);
    Ok(())
}

#[test]
fn worktree_lifecycle_and_guarded_removal() -> TestResult {
    let fx = Fixture::new()?;
    let (_remote, repo) = fx.repo_with_remote()?;
    let owned = fx.path("worktrees");
    fs::create_dir(&owned)?;
    let main_sha = fx.git.rev_parse(&repo, "main")?;

    let t1 = owned.join("t1");
    assert_eq!(
        fx.git.worktree_add(&repo, &t1, "task/t1", "main")?,
        main_sha
    );
    assert!(matches!(
        fx.git
            .worktree_add(&repo, &owned.join("again"), "task/t1", "main"),
        Err(GitError::BranchExists(_))
    ));
    assert!(matches!(
        fx.git
            .worktree_add(&repo, &owned.join("evil"), "--upload-pack=x", "main"),
        Err(GitError::InvalidBranchName { .. })
    ));
    assert!(!owned.join("evil").exists() && !owned.join("again").exists());
    let list = fx.git.worktree_list(&repo)?;
    assert_eq!(list.len(), 2);
    assert_eq!(list[1].branch.as_deref(), Some("task/t1"));
    assert_eq!(list[1].head.as_deref(), Some(main_sha.as_str()));

    // Untracked, non-ignored file: refused, nothing removed.
    fs::write(t1.join("notes.txt"), "draft")?;
    assert_eq!(
        fx.git.worktree_remove_if_safe(&repo, &t1, &owned)?,
        WorktreeRemoval::Refused(RemovalRefusal::Dirty { entries: 1 })
    );
    assert!(t1.join("notes.txt").exists());
    fs::remove_file(t1.join("notes.txt"))?;
    // Tracked modification: refused.
    fs::write(t1.join("a.txt"), "changed\n")?;
    assert_eq!(
        fx.git.worktree_remove_if_safe(&repo, &t1, &owned)?,
        WorktreeRemoval::Refused(RemovalRefusal::Dirty { entries: 1 })
    );
    fs::write(t1.join("a.txt"), "one\ntwo\nthree\n")?;

    // Foreign worktree (registered, but outside the owned root) and a symlink to it.
    let foreign = fx.path("elsewhere/t2");
    fx.git.worktree_add(&repo, &foreign, "task/t2", "main")?;
    assert_eq!(
        fx.git.worktree_remove_if_safe(&repo, &foreign, &owned)?,
        WorktreeRemoval::Refused(RemovalRefusal::OutsideOwnedRoot)
    );
    #[cfg(unix)]
    {
        let link = owned.join("link-to-foreign");
        std::os::unix::fs::symlink(&foreign, &link)?;
        assert_eq!(
            fx.git.worktree_remove_if_safe(&repo, &link, &owned)?,
            WorktreeRemoval::Refused(RemovalRefusal::OutsideOwnedRoot)
        );
    }
    assert!(foreign.join("README.md").exists());

    // Unregistered directory, missing path, the main worktree.
    let plain = owned.join("plain");
    fs::create_dir(&plain)?;
    assert_eq!(
        fx.git.worktree_remove_if_safe(&repo, &plain, &owned)?,
        WorktreeRemoval::Refused(RemovalRefusal::NotRegistered)
    );
    assert_eq!(
        fx.git
            .worktree_remove_if_safe(&repo, &owned.join("gone"), &owned)?,
        WorktreeRemoval::Refused(RemovalRefusal::Missing)
    );
    assert_eq!(
        fx.git.worktree_remove_if_safe(&repo, &repo, &fx.root)?,
        WorktreeRemoval::Refused(RemovalRefusal::MainWorktree)
    );

    // Locked worktree.
    let t3 = owned.join("t3");
    fx.git.worktree_add(&repo, &t3, "task/t3", "main")?;
    fx.raw(
        &repo,
        &[
            "worktree",
            "lock",
            "--reason",
            "agent running",
            path_str(&t3)?,
        ],
    )?;
    assert_eq!(
        fx.git.worktree_remove_if_safe(&repo, &t3, &owned)?,
        WorktreeRemoval::Refused(RemovalRefusal::Locked)
    );
    assert!(fx.git.worktree_list(&repo)?.iter().any(|w| w.locked));

    // Clean (ignored build output only): removed, branch kept.
    fs::create_dir(t1.join("target"))?;
    fs::write(t1.join("target/out.bin"), "build output")?;
    assert_eq!(
        fx.git.worktree_remove_if_safe(&repo, &t1, &owned)?,
        WorktreeRemoval::Removed
    );
    assert!(!t1.exists());
    assert!(fx.git.branch_exists(&repo, "task/t1")?);
    assert_eq!(fx.git.worktree_list(&repo)?.len(), 3);
    Ok(())
}

#[test]
fn clone_fetch_default_branch_and_branches() -> TestResult {
    let fx = Fixture::new()?;
    let (remote, repo) = fx.repo_with_remote()?;
    let clone = fx.path("clone");
    fx.git.clone(path_str(&remote)?, &clone)?;
    assert_eq!(fx.git.default_branch(&clone)?, "main");

    let sha = fx.git.create_branch(&clone, "task/a", "origin/main")?;
    assert!(fx.git.branch_exists(&clone, "task/a")?);
    assert_eq!(fx.git.rev_parse(&clone, "task/a")?, sha);
    assert!(matches!(
        fx.git.create_branch(&clone, "task/a", "main"),
        Err(GitError::BranchExists(_))
    ));
    assert!(!fx.git.branch_exists(&clone, "task/missing")?);
    assert!(matches!(
        fx.git.rev_parse(&clone, "no-such-branch"),
        Err(GitError::RevisionNotFound(_))
    ));
    assert!(matches!(
        fx.git.rev_parse(&clone, "--output=/tmp/x"),
        Err(GitError::InvalidInput { .. })
    ));

    // Remote advances; fetch brings it in.
    fs::write(repo.join("b.txt"), "b\n")?;
    let new_sha = fx
        .git
        .commit_all(&repo, "second", &author())?
        .ok_or("no commit")?;
    fx.git.push(&repo, "origin", "refs/heads/main")?;
    fx.git.fetch(&clone, "origin")?;
    assert_eq!(fx.git.rev_parse(&clone, "origin/main")?, new_sha);

    for url in [
        "--upload-pack=touch /tmp/pwned",
        "ext::sh -c touch% /tmp/pwned",
    ] {
        assert!(matches!(
            fx.git.clone(url, &fx.path("bad")),
            Err(GitError::InvalidInput { .. })
        ));
    }
    assert!(matches!(
        fx.git.fetch(&clone, "--upload-pack=x"),
        Err(GitError::InvalidInput { .. })
    ));
    Ok(())
}

#[test]
fn commit_all_and_push_with_explicit_refspecs() -> TestResult {
    let fx = Fixture::new()?;
    let remote = fx.path("remote.git");
    let scaffold = fx.path("scaffold");
    fx.git.init(&remote, "main", true)?;
    fx.git.init(&scaffold, "main", false)?;
    fs::create_dir_all(scaffold.join("docs"))?;
    fs::write(scaffold.join("docs/README.md"), "# Project\n")?;
    let sha = fx
        .git
        .commit_all(&scaffold, "chore: scaffold project", &author())?
        .ok_or("expected a commit")?;
    assert_eq!(fx.git.commit_all(&scaffold, "nothing", &author())?, None);
    fx.git.remote_add(&scaffold, "origin", path_str(&remote)?)?;
    fx.git
        .push(&scaffold, "origin", "refs/heads/main:refs/heads/main")?;
    assert_eq!(fx.git.rev_parse(&remote, "main")?, sha);
    let log = fx.git.log(&remote, "main", 5)?;
    assert_eq!(log.len(), 1);
    assert_eq!(log[0].author, "Yulius Test");
    assert_eq!(log[0].subject, "chore: scaffold project");

    for refspec in [
        "+refs/heads/main:refs/heads/main",
        "main",
        "refs/heads/main:refs/tags/v1",
        "refs/heads/main:refs/heads/../x",
    ] {
        assert!(
            matches!(
                fx.git.push(&scaffold, "origin", refspec),
                Err(GitError::InvalidInput { .. } | GitError::InvalidBranchName { .. })
            ),
            "{refspec} accepted"
        );
    }
    assert!(matches!(
        fx.git
            .commit_all(&scaffold, "x", &Author::new("Eve\nInjected", "e@x")),
        Err(GitError::InvalidInput { .. })
    ));
    Ok(())
}

/// SPEC-139: the remote's own answer for a branch, which a push that reported a failure may
/// have moved, and none for a branch it does not have.
#[test]
fn remote_branch_head_reads_the_remote_itself() -> TestResult {
    let fx = Fixture::new()?;
    let remote = fx.path("remote.git");
    let work = fx.path("work");
    fx.git.init(&remote, "main", true)?;
    fx.git.init(&work, "main", false)?;
    fs::write(work.join("a.txt"), "a\n")?;
    let sha = fx
        .git
        .commit_all(&work, "chore: a", &author())?
        .ok_or("expected a commit")?;
    fx.git.remote_add(&work, "origin", path_str(&remote)?)?;
    assert_eq!(fx.git.remote_branch_head(&work, "origin", "main")?, None);
    fx.git
        .push(&work, "origin", "refs/heads/main:refs/heads/feat/1-x")?;
    assert_eq!(
        fx.git.remote_branch_head(&work, "origin", "feat/1-x")?,
        Some(sha)
    );
    assert_eq!(fx.git.remote_branch_head(&work, "origin", "feat/1")?, None);
    assert!(matches!(
        fx.git.remote_branch_head(&work, "origin", "-x"),
        Err(GitError::InvalidBranchName { .. })
    ));
    Ok(())
}

#[test]
fn status_reports_hostile_file_names_verbatim() -> TestResult {
    let fx = Fixture::new()?;
    let (_remote, repo) = fx.repo_with_remote()?;
    let names = [
        "-rf",
        "--upload-pack=x",
        "quote\"d name",
        "new\nline",
        "ünï code",
    ];
    for name in names {
        fs::write(repo.join(name), "x")?;
    }
    assert!(fx.git.has_changes(&repo)?);
    let entries = fx.git.status_porcelain(&repo)?;
    let mut paths: Vec<&str> = entries.iter().map(|e| e.path.as_str()).collect();
    paths.sort_unstable();
    let mut expected = names.to_vec();
    expected.sort_unstable();
    assert_eq!(paths, expected);
    assert!(entries.iter().all(|e| e.is_untracked()));
    fx.git.commit_all(&repo, "add odd names", &author())?;
    assert!(!fx.git.has_changes(&repo)?);
    Ok(())
}

#[test]
fn diff_stat_ahead_behind_and_log() -> TestResult {
    let fx = Fixture::new()?;
    let (_remote, repo) = fx.repo_with_remote()?;
    let task = fx.path("task");
    fx.git.worktree_add(&repo, &task, "task/stats", "main")?;
    fs::write(task.join("a.txt"), "one\nTWO\nthree\nfour\nfive\n")?;
    fs::write(task.join("b.txt"), "b1\nb2\n")?;
    fx.git.commit_all(&task, "feat: first change", &author())?;
    fs::write(task.join("logo.bin"), [0_u8, 159, 146, 150, 0, 1])?;
    fx.git.commit_all(&task, "feat: add binary", &author())?;
    // The integration branch moves on independently.
    fs::write(repo.join("c.txt"), "c\n")?;
    fx.git.commit_all(&repo, "chore: unrelated", &author())?;

    assert_eq!(
        fx.git.diff_stat(&repo, "main", "task/stats")?,
        DiffStat {
            files: 3,
            insertions: 5,
            deletions: 1
        }
    );
    let counts = fx.git.ahead_behind(&repo, "task/stats", "main")?;
    assert_eq!((counts.ahead, counts.behind), (2, 1));
    let log = fx.git.log(&repo, "task/stats", 10)?;
    let subjects: Vec<&str> = log.iter().map(|c| c.subject.as_str()).collect();
    assert_eq!(
        subjects,
        ["feat: add binary", "feat: first change", "initial"]
    );
    assert!(
        log.iter()
            .all(|c| c.sha.len() == 40 && c.at.0 > 1_600_000_000_000)
    );
    assert_eq!(fx.git.log(&repo, "task/stats", 1)?.len(), 1);
    Ok(())
}

#[test]
fn bundle_round_trip_into_import_branches() -> TestResult {
    let fx = Fixture::new()?;
    let (_remote, repo) = fx.repo_with_remote()?;
    let first = fx.git.rev_parse(&repo, "main")?;
    fx.git.create_branch(&repo, "feat/x", "main")?;
    let full = fx.path("full.bundle");
    fx.git.create_bundle(
        &repo,
        &full,
        &[
            BundleSpec::Include("refs/heads/main".to_owned()),
            BundleSpec::Include("refs/heads/feat/x".to_owned()),
        ],
    )?;

    let target = fx.path("target");
    fx.git.init(&target, "main", false)?;
    let heads = fx.git.verify_bundle(&target, &full)?;
    assert_eq!(heads.len(), 2);
    assert!(heads.iter().all(|h| h.sha == first));
    let imported = fx.git.fetch_bundle_into(
        &target,
        &full,
        &[
            ImportRef {
                source: "refs/heads/main".to_owned(),
                target_branch: "import/main".to_owned(),
            },
            ImportRef {
                source: "refs/heads/feat/x".to_owned(),
                target_branch: "import/feat/x".to_owned(),
            },
        ],
    )?;
    assert_eq!(
        imported,
        vec![
            ("import/main".to_owned(), first.clone()),
            ("import/feat/x".to_owned(), first.clone())
        ]
    );

    // Incremental bundle: requires the first commit as a prerequisite.
    fs::write(repo.join("later.txt"), "later\n")?;
    let second = fx
        .git
        .commit_all(&repo, "later", &author())?
        .ok_or("no commit")?;
    let incremental = fx.path("incremental.bundle");
    fx.git.create_bundle(
        &repo,
        &incremental,
        &[
            BundleSpec::Include("refs/heads/main".to_owned()),
            BundleSpec::Exclude(first.clone()),
        ],
    )?;
    let empty = fx.path("empty");
    fx.git.init(&empty, "main", false)?;
    assert!(matches!(
        fx.git.verify_bundle(&empty, &incremental),
        Err(GitError::Failed { .. })
    ));
    fx.git.verify_bundle(&target, &incremental)?;
    let moved = fx.git.fetch_bundle_into(
        &target,
        &incremental,
        &[ImportRef {
            source: "refs/heads/main".to_owned(),
            target_branch: "import/main".to_owned(),
        }],
    )?;
    assert_eq!(moved, vec![("import/main".to_owned(), second)]);

    // Imports only land under import/, and empty bundles are refused before running git.
    assert!(matches!(
        fx.git.fetch_bundle_into(
            &target,
            &full,
            &[ImportRef {
                source: "refs/heads/main".to_owned(),
                target_branch: "main".to_owned(),
            }],
        ),
        Err(GitError::InvalidInput { .. })
    ));
    assert!(matches!(
        fx.git.create_bundle(
            &repo,
            &fx.path("none.bundle"),
            &[BundleSpec::Exclude(first)]
        ),
        Err(GitError::InvalidInput { .. })
    ));
    Ok(())
}

#[test]
fn pre_push_guard_blocks_protected_branches_only() -> TestResult {
    let fx = Fixture::new()?;
    let (remote, repo) = fx.repo_with_remote()?;
    let main_before = fx.git.rev_parse(&remote, "main")?;
    let task = fx.path("worktrees/task-x");
    fx.git.worktree_add(&repo, &task, "task/x", "main")?;
    let hook = fx
        .git
        .install_pre_push_guard(&task, &["main".to_owned(), "dev".to_owned()])?;
    assert!(hook.ends_with("yulius-hooks/pre-push"));
    fs::write(task.join("feature.txt"), "feature\n")?;
    let feature_sha = fx
        .git
        .commit_all(&task, "feat: x", &author())?
        .ok_or("no commit")?;

    for protected in ["main", "dev"] {
        let refspec = format!("refs/heads/task/x:refs/heads/{protected}");
        match fx.agent_push(&task, &refspec) {
            Err(stderr) => {
                assert!(
                    stderr.contains(&format!("protected branch {protected}")),
                    "{stderr}"
                );
            }
            Ok(()) => return Err(format!("push to {protected} not refused").into()),
        }
    }
    assert_eq!(fx.git.rev_parse(&remote, "main")?, main_before);
    fx.agent_push(&task, "refs/heads/task/x:refs/heads/feat/x")?;
    assert_eq!(fx.git.rev_parse(&remote, "feat/x")?, feature_sha);

    // The guard leaves the working tree clean and does not affect the primary checkout.
    assert!(!fx.git.has_changes(&task)?);
    fs::write(repo.join("hotfix.txt"), "fix\n")?;
    fx.git.commit_all(&repo, "fix: primary", &author())?;
    fx.git.push(&repo, "origin", "refs/heads/main")?;
    // Reinstalling rewrites the protected list.
    fx.git
        .install_pre_push_guard(&task, &["release".to_owned()])?;
    assert!(
        fx.agent_push(&task, "refs/heads/task/x:refs/heads/release")
            .is_err()
    );
    Ok(())
}

#[cfg(unix)]
fn write_hook(path: &Path, body: &str) -> TestResult {
    use std::os::unix::fs::PermissionsExt;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, body)?;
    fs::set_permissions(path, fs::Permissions::from_mode(0o755))?;
    Ok(())
}

#[cfg(unix)]
#[test]
fn pre_push_guard_chains_existing_hooks_and_honors_hooks_path() -> TestResult {
    let fx = Fixture::new()?;
    let (_remote, repo) = fx.repo_with_remote()?;

    // An existing hook in the default hooks directory is preserved and still runs.
    let record = fx.path("pre-push-input.txt");
    let default_hook = repo.join(".git/hooks/pre-push");
    write_hook(
        &default_hook,
        &format!("#!/bin/sh\ncat > '{}'\nexit 0\n", record.display()),
    )?;
    let task = fx.path("worktrees/a");
    fx.git.worktree_add(&repo, &task, "task/a", "main")?;
    fx.git.install_pre_push_guard(&task, &["main".to_owned()])?;
    assert!(default_hook.exists(), "existing hook must be preserved");
    fs::write(task.join("a2.txt"), "a\n")?;
    fx.git.commit_all(&task, "feat: a", &author())?;
    fx.agent_push(&task, "refs/heads/task/a:refs/heads/feat/a")?;
    let seen = fs::read_to_string(&record)?;
    assert!(
        seen.contains("refs/heads/feat/a"),
        "chained hook saw {seen:?}"
    );

    // A repository-configured relative core.hooksPath (tracked .githooks/) is honored.
    write_hook(
        &repo.join(".githooks/pre-push"),
        "#!/bin/sh\necho 'repository hook says no' >&2\nexit 1\n",
    )?;
    fx.git
        .commit_all(&repo, "chore: repository hooks", &author())?;
    fx.raw(&repo, &["config", "--local", "core.hooksPath", ".githooks"])?;
    let task_b = fx.path("worktrees/b");
    fx.git.worktree_add(&repo, &task_b, "task/b", "main")?;
    fx.git
        .install_pre_push_guard(&task_b, &["main".to_owned()])?;
    match fx.agent_push(&task_b, "refs/heads/task/b:refs/heads/feat/b") {
        Err(stderr) => {
            assert!(stderr.contains("repository hook says no"), "{stderr}");
        }
        Ok(()) => return Err("chained .githooks hook did not run".into()),
    }
    match fx.agent_push(&task_b, "refs/heads/task/b:refs/heads/main") {
        Err(stderr) => {
            assert!(stderr.contains("protected branch main"), "{stderr}");
            assert!(
                !stderr.contains("repository hook says no"),
                "guard must run first"
            );
        }
        Ok(()) => return Err("protected push not refused".into()),
    }
    // Nothing was written into the tracked hooks directory.
    assert!(!fx.git.has_changes(&task_b)?);
    let tracked_hooks: Vec<_> = fs::read_dir(task_b.join(".githooks"))?.collect();
    assert_eq!(tracked_hooks.len(), 1);
    Ok(())
}

/// #54: the server's own Git runs outside containment in repositories whose Git directory the
/// agents can write, so it never runs repository hooks; the agents' own Git still does.
#[cfg(unix)]
#[test]
fn server_side_commands_never_run_repository_hooks() -> TestResult {
    let fx = Fixture::new()?;
    let (_remote, repo) = fx.repo_with_remote()?;
    let ran = fx.path("hooks-ran.txt");
    for name in [
        "post-checkout",
        "pre-commit",
        "post-commit",
        "pre-push",
        "reference-transaction",
    ] {
        write_hook(
            &repo.join(".git/hooks").join(name),
            &format!("#!/bin/sh\necho {name} >> '{}'\n", ran.display()),
        )?;
    }
    let task = fx.path("worktrees/h");
    fx.git.worktree_add(&repo, &task, "task/h", "main")?;
    fs::write(task.join("h.txt"), "h\n")?;
    fx.git.commit_all(&task, "feat: h", &author())?;
    fx.git
        .push(&task, "origin", "refs/heads/task/h:refs/heads/feat/h")?;
    fx.git.fetch(&repo, "origin")?;
    assert!(
        !ran.exists(),
        "server-side Git ran hooks: {:?}",
        fs::read_to_string(&ran).ok()
    );
    // Control: the agents' own Git runs them.
    fx.raw(
        &task,
        &["commit", "--allow-empty", "--quiet", "-m", "agent"],
    )?;
    assert!(fs::read_to_string(&ran)?.contains("pre-commit"));
    Ok(())
}

#[test]
fn local_identity_is_worktree_scoped() -> TestResult {
    let fx = Fixture::new()?;
    let (_remote, repo) = fx.repo_with_remote()?;
    let task = fx.path("worktrees/ident");
    fx.git.worktree_add(&repo, &task, "task/ident", "main")?;
    fx.git
        .set_local_identity(&task, "Agent Ada", "ada@agents.invalid")?;
    fs::write(task.join("ada.txt"), "ada\n")?;
    fx.raw_with(&task, &["add", "--all"], false)?;
    fx.raw_with(&task, &["commit", "--quiet", "-m", "agent commit"], false)?;
    assert_eq!(
        fx.raw_with(&task, &["log", "-1", "--format=%an <%ae>"], false)?,
        "Agent Ada <ada@agents.invalid>"
    );
    // The primary checkout does not inherit the worktree identity.
    assert!(fx.raw_with(&repo, &["config", "user.name"], false).is_err());
    assert!(matches!(
        fx.git
            .set_local_identity(&task, "-c core.pager=evil", "x@y"),
        Err(GitError::InvalidInput { .. })
    ));
    Ok(())
}

/// SPEC-139: a remote that never answers is given up on within `REMOTE_HEAD_TIMEOUT`, not the
/// half-hour network deadline, so `yulius wait` stays within its own bound.
#[cfg(unix)]
#[test]
fn reading_a_remote_branch_gives_up_on_a_stalled_remote() -> TestResult {
    let fx = Fixture::new()?;
    let script = fx.path("stalled-git");
    write_hook(&script, "#!/bin/sh\nexec sleep 60\n")?;
    let git = Git::new(&script);
    let started = Instant::now();
    match git.remote_branch_head(&fx.root, "origin", "feat/1-x") {
        Err(GitError::Timeout { .. }) => {}
        other => return Err(format!("expected timeout, got {other:?}").into()),
    }
    let elapsed = started.elapsed();
    assert!(
        elapsed >= yulius_git::REMOTE_HEAD_TIMEOUT && elapsed < yulius_git::REMOTE_HEAD_TIMEOUT * 3,
        "{elapsed:?}"
    );
    Ok(())
}

#[cfg(unix)]
#[test]
fn timeout_kills_the_whole_process_group() -> TestResult {
    let fx = Fixture::new()?;
    let pids = fx.path("pids");
    let script = fx.path("hanging-git");
    write_hook(
        &script,
        &format!(
            "#!/bin/sh\necho $$ > '{p}'\nsleep 30 &\necho $! >> '{p}'\nwait\n",
            p = pids.display()
        ),
    )?;
    let mut git = Git::new(&script);
    // Long enough for a loaded host to start `sh` and record both pids before the kill.
    git.timeout = Duration::from_secs(3);
    let started = Instant::now();
    match git.rev_parse(&fx.root, "HEAD") {
        Err(GitError::Timeout { .. }) => {}
        other => return Err(format!("expected timeout, got {other:?}").into()),
    }
    assert!(started.elapsed() < Duration::from_secs(10));
    let recorded = fs::read_to_string(&pids)?;
    let deadline = Instant::now() + Duration::from_secs(5);
    for pid in recorded.split_whitespace() {
        loop {
            let alive = Command::new("kill").args(["-0", pid]).status()?.success();
            let zombie = Command::new("ps")
                .args(["-o", "stat=", "-p", pid])
                .output()
                .map(|o| String::from_utf8_lossy(&o.stdout).trim().starts_with('Z'))
                .unwrap_or(false);
            if !alive || zombie {
                break;
            }
            if Instant::now() > deadline {
                return Err(format!("process {pid} survived the timeout").into());
            }
            std::thread::yield_now();
        }
    }
    Ok(())
}

#[cfg(unix)]
#[test]
fn output_bound_kills_the_command() -> TestResult {
    let fx = Fixture::new()?;
    let script = fx.path("noisy-git");
    write_hook(&script, "#!/bin/sh\nexec head -c 3000000 /dev/zero\n")?;
    let mut git = Git::new(&script);
    git.max_output_bytes = 1024 * 1024;
    match git.rev_parse(&fx.root, "HEAD") {
        Err(GitError::OutputTooLarge { limit, .. }) => assert_eq!(limit, 1024 * 1024),
        other => return Err(format!("expected output bound, got {other:?}").into()),
    }
    Ok(())
}

#[test]
fn missing_executable_is_a_spawn_error() -> TestResult {
    let fx = Fixture::new()?;
    let git = Git::new(fx.path("no-such-git"));
    assert!(matches!(
        git.rev_parse(&fx.root, "HEAD"),
        Err(GitError::Spawn(_))
    ));
    Ok(())
}

/// SPEC-002 ADR-16 ("redirect"): the task's committed and uncommitted work is kept on a new
/// branch, the worktree goes back to where it forked, and an existing branch is never replaced.
#[test]
fn keep_and_revert_keeps_all_work_and_resets_the_worktree() -> TestResult {
    let fx = Fixture::new()?;
    let (_remote, repo) = fx.repo_with_remote()?;
    let fork = fx.git.rev_parse(&repo, "main")?;
    let worktree = fx.path("worktrees/t1");
    fs::create_dir_all(fx.path("worktrees"))?;
    fx.git
        .worktree_add(&repo, &worktree, "feat/1-task", "main")?;
    fs::write(worktree.join("a.txt"), "committed change\n")?;
    fx.git.commit_all(&worktree, "work", &author())?;
    fs::write(worktree.join("draft.txt"), "uncommitted\n")?;
    let bases = vec!["origin/missing".to_owned(), "origin/main".to_owned()];
    let kept = fx.git.keep_and_revert(
        &worktree,
        "yulius/reverted/1-100",
        &bases,
        "yulius: keep the work of #1 before reverting it",
        &author(),
    )?;
    assert_eq!(fx.git.rev_parse(&repo, "yulius/reverted/1-100")?, kept);
    assert_eq!(fx.git.rev_parse(&worktree, "HEAD")?, fork, "worktree reset");
    assert!(!worktree.join("draft.txt").exists());
    assert_eq!(
        fs::read_to_string(worktree.join("a.txt"))?,
        "one\ntwo\nthree\n"
    );
    let files = fx.raw(&repo, &["show", "--name-only", "--format=", &kept])?;
    assert!(
        files.contains("draft.txt"),
        "uncommitted work kept: {files}"
    );
    let log = fx.git.log(&repo, "yulius/reverted/1-100", 5)?;
    assert_eq!(log.len(), 3, "both commits kept on the branch");
    // An existing branch is refused before anything is reset.
    fs::write(worktree.join("more.txt"), "x\n")?;
    assert!(matches!(
        fx.git.keep_and_revert(
            &worktree,
            "yulius/reverted/1-100",
            &bases,
            "again",
            &author()
        ),
        Err(GitError::BranchExists(_))
    ));
    assert_eq!(
        fx.git.rev_parse(&repo, "yulius/reverted/1-100")?,
        kept,
        "kept branch untouched"
    );
    assert!(matches!(
        fx.git
            .keep_and_revert(&worktree, "x", &["nowhere".to_owned()], "m", &author()),
        Err(GitError::InvalidInput { .. })
    ));
    Ok(())
}

/// Lands `head` on `branch` of `repo` as a host's squash merge of an up-to-date pull request: a
/// new commit with the head's tree and the branch as its only parent.
fn squash_land(
    fx: &Fixture,
    repo: &Path,
    branch: &str,
    head: &str,
) -> Result<String, Box<dyn Error>> {
    let tree = fx.git.tree_of(repo, head)?.ok_or("tree")?;
    let parent = fx.git.rev_parse(repo, branch)?;
    let commit = fx.raw(repo, &["commit-tree", &tree, "-p", &parent, "-m", "squash"])?;
    fx.raw(
        repo,
        &[
            "update-ref",
            &format!("refs/heads/{branch}"),
            &commit,
            &parent,
        ],
    )?;
    Ok(commit)
}

#[test]
fn a_merge_train_predicts_the_trees_of_merging_one_by_one() -> TestResult {
    // SPEC-084 TRN-002: the train merges the integration branch into each head as DLV-203 does
    // (merge drivers apply: entries at the top of a union-merged changes log), lands each as a
    // squash, and leaves out a head that conflicts with the ones before it. Merging the heads
    // one by one afterwards gives exactly the trees it predicted.
    let fx = Fixture::new()?;
    let (_remote, repo) = fx.repo_with_remote()?;
    fs::write(repo.join(".gitattributes"), "changes.md merge=union\n")?;
    fs::write(repo.join("changes.md"), "# Changes\n\n- first\n")?;
    fs::write(repo.join("shared.txt"), "base\n")?;
    fx.git.commit_all(&repo, "changes log", &author())?;
    fx.raw(&repo, &["branch", "dev", "main"])?;
    let mut worktrees = Vec::new();
    for (n, extra) in [(1, "shared.txt"), (2, "two.txt"), (3, "shared.txt")] {
        let worktree = fx.path(&format!("feature{n}"));
        fx.git
            .worktree_add(&repo, &worktree, &format!("feat/{n}"), "dev")?;
        fx.git
            .set_local_identity(&worktree, "Agent", "agent@example.invalid")?;
        fs::write(
            worktree.join("changes.md"),
            format!("# Changes\n\n- entry {n}\n- first\n"),
        )?;
        fs::write(worktree.join(extra), format!("from {n}\n"))?;
        fx.git
            .commit_all(&worktree, &format!("feature {n}"), &author())?;
        worktrees.push(worktree);
    }
    let heads: Vec<String> = worktrees
        .iter()
        .map(|w| fx.git.rev_parse(w, "HEAD"))
        .collect::<Result<_, _>>()?;
    let owned = fx.path("ci");
    fs::create_dir(&owned)?;
    let scratch = owned.join("train");
    let build = fx.git.build_train(
        &repo,
        &scratch,
        &owned,
        &TrainInput {
            base: "dev",
            heads: &heads,
            landing: TrainLanding::Squash,
            author: &author(),
        },
    )?;
    assert_eq!(build.base, fx.git.rev_parse(&repo, "dev")?);
    let [
        TrainStep::Clean { tree: first },
        TrainStep::Clean { tree: second },
        third,
    ] = build.steps.as_slice()
    else {
        return Err(format!("unexpected steps: {:?}", build.steps).into());
    };
    assert_eq!(third, &TrainStep::Conflict(vec!["shared.txt".to_owned()]));
    assert_eq!(fx.git.tree_of(&repo, &build.head)?.as_ref(), Some(second));
    assert_eq!(
        fx.git.rev_parse(&scratch, "HEAD")?,
        build.head,
        "kept reachable"
    );

    // One by one, as Yulius and the host do it: bring up to date, then squash.
    assert_eq!(
        fx.git.merge_into(&worktrees[0], "dev")?,
        MergeInto::UpToDate
    );
    squash_land(&fx, &repo, "dev", &heads[0])?;
    assert_eq!(fx.git.tree_of(&repo, "dev")?.as_ref(), Some(first));
    let MergeInto::Merged(second_head) = fx.git.merge_into(&worktrees[1], "dev")? else {
        return Err("expected a merge".into());
    };
    assert_eq!(fx.git.tree_of(&repo, &second_head)?.as_ref(), Some(second));
    squash_land(&fx, &repo, "dev", &second_head)?;
    assert_eq!(fx.git.tree_of(&repo, "dev")?.as_ref(), Some(second));
    let log = fx.raw(&repo, &["show", "dev:changes.md"])?;
    assert!(
        log.contains("- entry 1") && log.contains("- entry 2") && !log.contains("- entry 3"),
        "{log}"
    );
    // The third conflicts with the integration branch now, as the train said.
    assert_eq!(
        fx.git.merge_into(&worktrees[2], "dev")?,
        MergeInto::Conflict(vec!["shared.txt".to_owned()])
    );

    // A merge-commit landing has both parents; the scratch worktree is reused.
    let again = fx.git.build_train(
        &repo,
        &scratch,
        &owned,
        &TrainInput {
            base: &build.base,
            heads: &heads[..2],
            landing: TrainLanding::MergeCommit,
            author: &author(),
        },
    )?;
    let parents = fx.raw(&repo, &["show", "--no-patch", "--format=%P", &again.head])?;
    assert_eq!(parents.split_whitespace().count(), 2, "{parents}");
    assert_eq!(fx.git.tree_of(&repo, &again.head)?.as_ref(), Some(second));
    Ok(())
}

#[test]
fn merge_attributes_in_info_reach_every_worktree() -> TestResult {
    // SPEC-083 MRG-001: the logs every task appends to merge by union in a repository that does
    // not commit a `.gitattributes` for them, in its linked worktrees too.
    let fx = Fixture::new()?;
    let (_remote, repo) = fx.repo_with_remote()?;
    fs::write(repo.join("changes.md"), "# Changes\n\n- first\n")?;
    fx.git.commit_all(&repo, "changes log", &author())?;
    let feature = fx.path("feature");
    fx.git.worktree_add(&repo, &feature, "feat/x", "main")?;
    fx.git
        .set_local_identity(&feature, "Agent", "agent@example.invalid")?;
    fs::write(
        feature.join("changes.md"),
        "# Changes\n\n- feature\n- first\n",
    )?;
    fx.git.commit_all(&feature, "feature entry", &author())?;
    fs::write(repo.join("changes.md"), "# Changes\n\n- other\n- first\n")?;
    fx.git.commit_all(&repo, "other entry", &author())?;
    assert_eq!(
        fx.git.merge_tree(&feature, "HEAD", "main")?,
        None,
        "without the attribute the logs conflict"
    );

    let info = repo.join(".git/info/attributes");
    fs::write(&info, "*.png binary\n")?;
    let lines = vec!["/changes.md merge=union".to_owned()];
    assert!(fx.git.set_managed_attributes(&feature, &lines)?);
    assert!(!fx.git.set_managed_attributes(&repo, &lines)?, "unchanged");
    let text = fs::read_to_string(&info)?;
    assert!(text.starts_with("*.png binary\n"), "{text}");
    assert!(text.contains("/changes.md merge=union"), "{text}");

    assert!(fx.git.merge_tree(&feature, "HEAD", "main")?.is_some());
    let merged = fx.git.merge_into(&feature, "main")?;
    assert!(matches!(merged, MergeInto::Merged(_)), "{merged:?}");
    let log = fs::read_to_string(feature.join("changes.md"))?;
    assert!(
        log.contains("- feature") && log.contains("- other"),
        "{log}"
    );

    assert!(fx.git.set_managed_attributes(&repo, &[])?, "removed");
    assert_eq!(fs::read_to_string(&info)?, "*.png binary\n");
    assert!(
        fx.git
            .set_managed_attributes(&repo, &["a\nb".to_owned()])
            .is_err()
    );
    Ok(())
}

#[test]
fn only_clean_merges_of_the_integration_branch_keep_a_review() -> TestResult {
    // SPEC-083 REV-001: a head that is the reviewed head plus Git's own merges of commits the
    // integration branch contains keeps the review; anything else needs a new one.
    let fx = Fixture::new()?;
    let (_remote, repo) = fx.repo_with_remote()?;
    let start = fx.git.rev_parse(&repo, "main")?;
    let feature = fx.path("feature");
    fx.git.worktree_add(&repo, &feature, "feat/x", "main")?;
    fx.git
        .set_local_identity(&feature, "Agent", "agent@example.invalid")?;
    fs::write(feature.join("feature.txt"), "feature\n")?;
    fx.git.commit_all(&feature, "feature", &author())?;
    let reviewed = fx.git.rev_parse(&feature, "HEAD")?;
    let covers = |head: &str| {
        fx.git
            .only_clean_merges_since(&feature, &reviewed[..9], head, "main", 8)
    };
    assert!(covers(&reviewed)?, "the reviewed head itself");

    // Two clean merges of main in a row.
    for round in 0..2 {
        fs::write(repo.join(format!("main{round}.txt")), "main\n")?;
        fx.git.commit_all(&repo, "main moves", &author())?;
        assert!(matches!(
            fx.git.merge_into(&feature, "main")?,
            MergeInto::Merged(_)
        ));
    }
    let merged = fx.git.rev_parse(&feature, "HEAD")?;
    assert!(covers(&merged)?);
    assert!(
        !fx.git
            .only_clean_merges_since(&feature, &reviewed, &merged, "main", 1)?,
        "limit"
    );
    assert!(
        !fx.git
            .only_clean_merges_since(&feature, "0123456789", &merged, "main", 8)?
    );

    // New work after the merge.
    fs::write(feature.join("feature.txt"), "feature v2\n")?;
    fx.git.commit_all(&feature, "more", &author())?;
    let worked = fx.git.rev_parse(&feature, "HEAD")?;
    assert!(!covers(&worked)?);
    fx.raw(&feature, &["reset", "--quiet", "--hard", &merged])?;

    // A change slipped into a merge commit.
    fs::write(repo.join("main2.txt"), "main\n")?;
    fx.git.commit_all(&repo, "main moves", &author())?;
    fx.raw(
        &feature,
        &["merge", "--quiet", "--no-ff", "--no-commit", "main"],
    )?;
    fs::write(feature.join("feature.txt"), "slipped in\n")?;
    fx.raw(&feature, &["commit", "--quiet", "-am", "merge main"])?;
    let evil = fx.git.rev_parse(&feature, "HEAD")?;
    assert!(!covers(&evil)?);
    fx.raw(&feature, &["reset", "--quiet", "--hard", &merged])?;

    // A conflict resolved by hand.
    fs::write(feature.join("a.txt"), "feature\n")?;
    fx.git.commit_all(&feature, "feature a", &author())?;
    let reviewed_a = fx.git.rev_parse(&feature, "HEAD")?;
    fs::write(repo.join("a.txt"), "main\n")?;
    fx.git.commit_all(&repo, "main a", &author())?;
    assert!(fx.raw(&feature, &["merge", "--quiet", "main"]).is_err());
    fs::write(feature.join("a.txt"), "both\n")?;
    fx.raw(&feature, &["commit", "--quiet", "-am", "resolved"])?;
    let resolved = fx.git.rev_parse(&feature, "HEAD")?;
    assert!(
        !fx.git
            .only_clean_merges_since(&feature, &reviewed_a, &resolved, "main", 8)?
    );

    // A merge of work the integration branch does not contain.
    fx.raw(&feature, &["reset", "--quiet", "--hard", &reviewed_a])?;
    let other = fx.path("other");
    fx.git.worktree_add(&repo, &other, "feat/other", &start)?;
    fs::write(other.join("other.txt"), "other\n")?;
    fx.git.commit_all(&other, "other work", &author())?;
    fx.raw(&feature, &["merge", "--quiet", "--no-edit", "feat/other"])?;
    let foreign = fx.git.rev_parse(&feature, "HEAD")?;
    assert!(
        !fx.git
            .only_clean_merges_since(&feature, &reviewed_a, &foreign, "main", 8)?
    );
    Ok(())
}

#[test]
fn a_squash_lands_one_commit_on_the_remote_branch_or_names_the_conflicts() -> TestResult {
    // SPEC-099 PLY-007: a playground's merge squashes a task branch onto its integration branch
    // as one commit with that branch as its only parent, pushed without forcing.
    let fx = Fixture::new()?;
    let (remote, repo) = fx.repo_with_remote()?;
    fx.git
        .push(&repo, "origin", "refs/heads/main:refs/heads/dev")?;
    fx.git.fetch(&repo, "origin")?;
    let feature = fx.path("feature");
    fx.git
        .worktree_add(&repo, &feature, "feat/x", "origin/dev")?;
    fs::write(feature.join("x.txt"), "x\n")?;
    fx.git
        .commit_all(&feature, "feat: x, part one", &author())?;
    fs::write(feature.join("y.txt"), "y\n")?;
    fx.git
        .commit_all(&feature, "feat: x, part two", &author())?;
    let head = fx.git.rev_parse(&feature, "HEAD")?;
    let base = fx.git.rev_parse(&repo, "origin/dev")?;

    let landed = fx.git.squash_land(
        &repo,
        &SquashInput {
            base: "origin/dev",
            head: &head,
            message: "feat: x\n\nTask #3.",
            author: &author(),
            remote: "origin",
            branch: "dev",
        },
    )?;
    let SquashLanding::Landed(commit) = landed else {
        return Err(format!("not landed: {landed:?}").into());
    };
    let dev = fx.git.rev_parse(&remote, "dev")?;
    assert_eq!(dev, commit, "the remote's dev is the squash");
    assert_eq!(
        fx.git.parents(&remote, &dev)?,
        vec![base],
        "one parent: the old dev"
    );
    assert_eq!(
        fx.git.tree_of(&remote, &dev)?,
        fx.git.tree_of(&repo, &head)?,
        "the head's tree"
    );
    let subject = Command::new("git")
        .args(["-C", path_str(&remote)?, "log", "-1", "--format=%B", "dev"])
        .output()?;
    assert_eq!(
        String::from_utf8_lossy(&subject.stdout).trim(),
        "feat: x\n\nTask #3."
    );

    // The same head again changes nothing.
    fx.git.fetch(&repo, "origin")?;
    assert_eq!(
        fx.git.squash_land(
            &repo,
            &SquashInput {
                base: "origin/dev",
                head: &head,
                message: "again",
                author: &author(),
                remote: "origin",
                branch: "dev",
            },
        )?,
        SquashLanding::Unchanged
    );

    // A branch that conflicts with dev pushes nothing and names the files.
    let other = fx.path("other");
    fx.git
        .worktree_add(&repo, &other, "feat/y", "origin/main")?;
    fs::write(other.join("x.txt"), "a different x\n")?;
    fx.git.commit_all(&other, "feat: y", &author())?;
    let conflict = fx.git.squash_land(
        &repo,
        &SquashInput {
            base: "origin/dev",
            head: "feat/y",
            message: "feat: y",
            author: &author(),
            remote: "origin",
            branch: "dev",
        },
    )?;
    assert_eq!(conflict, SquashLanding::Conflict(vec!["x.txt".to_owned()]));
    assert_eq!(fx.git.rev_parse(&remote, "dev")?, commit, "nothing pushed");
    Ok(())
}

/// SPEC-134 (review C1): CD reads a project's scripts from a mirror only Yulius writes. A
/// replacement planted in the sandbox clone (`git replace`, writable by its agents) never reaches
/// it, a tracking ref that moved away from the expected commit is refused, and the exported tree
/// is exactly the commit's.
#[test]
fn the_cd_mirror_takes_only_the_expected_commit_and_ignores_replacements() -> TestResult {
    let fx = Fixture::new()?;
    let (_remote, repo) = fx.repo_with_remote()?;
    fs::create_dir_all(repo.join("scripts/cd"))?;
    fs::write(repo.join("scripts/cd/deploy.sh"), "echo honest\n")?;
    fx.git.commit_all(&repo, "deploy script", &author())?;
    fx.raw(&repo, &["push", "--quiet", "origin", "main"])?;
    fx.raw(&repo, &["fetch", "--quiet", "origin"])?;
    let sha = fx.git.rev_parse(&repo, "origin/main")?;

    // An agent plants a replacement for the script's blob in the sandbox clone.
    let honest = fx.raw(
        &repo,
        &["rev-parse", &format!("{sha}:scripts/cd/deploy.sh")],
    )?;
    fs::write(fx.path("evil.sh"), "echo evil\n")?;
    let evil = fx.raw(
        &repo,
        &[
            "hash-object",
            "-w",
            fx.path("evil.sh").to_str().ok_or("path")?,
        ],
    )?;
    fx.raw(&repo, &["replace", honest.trim(), evil.trim()])?;

    let mirror = fx.path("cd/source.git");
    fx.git.cd_mirror_fetch(&mirror, &repo, "main", &sha)?;
    assert_eq!(
        fx.git
            .show_file(&mirror, &sha, "scripts/cd/deploy.sh")?
            .as_deref(),
        Some("echo honest\n")
    );
    let dest = fx.path("run/src");
    fx.git.export_tree(&mirror, &sha, &dest)?;
    assert_eq!(
        fs::read_to_string(dest.join("scripts/cd/deploy.sh"))?,
        "echo honest\n"
    );
    assert!(dest.join("README.md").exists() || fs::read_dir(&dest)?.count() > 1);
    assert!(!dest.join(".git").exists(), "a plain tree, no repository");

    // The clone's tracking ref moved to another commit: refused, nothing fetched for it.
    let other = fx.raw(
        &repo,
        &["commit-tree", "-m", "x", &format!("{sha}^{{tree}}")],
    )?;
    fx.raw(
        &repo,
        &["update-ref", "refs/remotes/origin/main", other.trim()],
    )?;
    let refused = fx.git.cd_mirror_fetch(&mirror, &repo, "main", &sha);
    assert!(refused.is_err(), "{refused:?}");
    Ok(())
}

/// SPEC-219 ENV-001 (#656): an environment run takes its branch's head from origin itself into a
/// shallow mirror only Yulius writes, never through the sandbox clone; a later head arrives on the
/// next fetch, and a missing branch or an option-like source is refused.
#[test]
fn a_branch_head_is_taken_from_origin_into_a_shallow_mirror() -> TestResult {
    let fx = Fixture::new()?;
    let (origin, repo) = fx.repo_with_remote()?;
    fs::write(repo.join("a.txt"), "second\n")?;
    fx.git.commit_all(&repo, "second", &author())?;
    fx.git.push(&repo, "origin", "refs/heads/main")?;
    let mirror = fx.path("cd/environments.git");
    let source = path_str(&origin)?;

    let head = fx.git.mirror_fetch_head(&mirror, source, "main")?;
    assert_eq!(head, fx.git.rev_parse(&origin, "refs/heads/main")?);
    assert_eq!(
        fx.raw(&mirror, &["rev-parse", "--is-shallow-repository"])?,
        "true",
        "one commit's tree is all a run needs"
    );
    let dest = fx.path("run/src");
    fx.git.export_tree(&mirror, &head, &dest)?;
    assert_eq!(fs::read_to_string(dest.join("a.txt"))?, "second\n");

    fs::write(repo.join("a.txt"), "third\n")?;
    fx.git.commit_all(&repo, "third", &author())?;
    fx.git.push(&repo, "origin", "refs/heads/main")?;
    let next = fx.git.mirror_fetch_head(&mirror, source, "main")?;
    assert_eq!(next, fx.git.rev_parse(&origin, "refs/heads/main")?);
    assert_ne!(next, head);

    assert!(fx.git.mirror_fetch_head(&mirror, source, "absent").is_err());
    assert!(fx.git.mirror_fetch_head(&mirror, "-x", "main").is_err());
    assert!(
        fx.git
            .mirror_fetch_head(&mirror, "relative/path", "main")
            .is_err()
    );
    Ok(())
}

/// SPEC-176 MPG-002/003: every branch and tag of a repository reaches an empty remote in one
/// push, the remote's refs are read back, and a remote is repointed.
#[test]
fn every_branch_and_tag_is_pushed_at_once_and_read_back() -> TestResult {
    let fx = Fixture::new()?;
    let (origin, repo) = fx.repo_with_remote()?;
    fx.git
        .push(&repo, "origin", "refs/heads/main:refs/heads/dev")?;
    fx.raw(&repo, &["tag", "v1"])?;
    fx.raw(&repo, &["push", "--quiet", "origin", "refs/tags/v1"])?;
    let main = fx.git.rev_parse(&repo, "HEAD")?;
    let target = fx.path("github.git");
    fx.git.init(&target, "main", true)?;
    assert!(fx.git.remote_refs(&origin, path_str(&target)?)?.is_empty());

    fx.git
        .push_all_refs(&origin, path_str(&target)?, Some("dev"))?;
    let want = vec![
        ("refs/heads/dev".to_owned(), main.clone()),
        ("refs/heads/main".to_owned(), main.clone()),
        ("refs/tags/v1".to_owned(), main.clone()),
    ];
    assert_eq!(fx.git.remote_refs(&origin, path_str(&target)?)?, want);
    assert_eq!(fx.git.remote_refs(&repo, path_str(&origin)?)?, want);

    fx.git.set_remote_url(&repo, "origin", path_str(&target)?)?;
    assert_eq!(
        fx.raw(&repo, &["remote", "get-url", "origin"])?,
        path_str(&target)?
    );
    assert!(
        fx.git
            .set_remote_url(&repo, "origin", "ext::sh -c x")
            .is_err()
    );
    Ok(())
}

/// SPEC-176 MPG-005: the push never forces and is atomic: when one ref is refused, none moves.
#[test]
fn a_push_of_every_ref_is_all_or_nothing() -> TestResult {
    let fx = Fixture::new()?;
    let (origin, repo) = fx.repo_with_remote()?;
    let target = fx.path("github.git");
    fx.git.init(&target, "main", true)?;
    // The remote's `main` has a commit the source does not.
    let other = fx.path("other");
    fx.git.init(&other, "main", false)?;
    fs::write(other.join("b.txt"), "b\n")?;
    fx.git.commit_all(&other, "other", &author())?;
    fx.git.remote_add(&other, "origin", path_str(&target)?)?;
    fx.git.push(&other, "origin", "refs/heads/main")?;
    let before = fx.git.remote_refs(&repo, path_str(&target)?)?;
    fx.git
        .push(&repo, "origin", "refs/heads/main:refs/heads/dev")?;

    assert!(
        fx.git
            .push_all_refs(&origin, path_str(&target)?, None)
            .is_err()
    );
    assert!(
        fx.git
            .push_all_refs(&origin, path_str(&target)?, Some("-x"))
            .is_err(),
        "the first branch is a branch name"
    );
    assert_eq!(
        fx.git.remote_refs(&repo, path_str(&target)?)?,
        before,
        "nothing moved, `dev` included"
    );
    Ok(())
}

/// SPEC-176 (final review): before Yulius's Git carries a token in a clone whose configuration its
/// sandbox could write, every local setting that steers transport or loads more configuration is
/// removed; the rest stays.
#[test]
fn transport_settings_of_a_clone_are_scrubbed() -> TestResult {
    let fx = Fixture::new()?;
    let (_remote, repo) = fx.repo_with_remote()?;
    let planted = [
        ("http.proxy", "http://127.0.0.1:9"),
        ("http.https://github.com/.sslVerify", "false"),
        ("url./tmp/evil/.insteadOf", "https://github.com/"),
        ("include.path", "/tmp/evil.cfg"),
        ("includeIf.gitdir:/.path", "/tmp/evil.cfg"),
        ("core.sshCommand", "evil"),
        ("credential.helper", "evil"),
        ("remote.origin.proxy", "http://127.0.0.1:9"),
        ("remote.origin.pushurl", "/tmp/evil.git"),
        ("core.alternateRefsCommand", "evil"),
        ("fetch.recurseSubmodules", "true"),
        ("submodule.recurse", "true"),
        ("push.recurseSubmodules", "on-demand"),
        ("user.name", "Kept Name"),
        ("submodule.lib.url", "https://example.invalid/lib.git"),
        ("submodule.recurse-lib.url", "https://example.invalid/r.git"),
    ];
    // The same key twice, apart.
    fx.raw(
        &repo,
        &[
            "config",
            "--local",
            "--add",
            "include.path",
            "/tmp/evil2.cfg",
        ],
    )?;
    for (key, value) in planted {
        fx.raw(&repo, &["config", "--local", key, value])?;
    }
    fx.git.scrub_transport_config(&repo)?;
    let left = fx.raw(&repo, &["config", "--local", "--list", "--name-only"])?;
    let left: Vec<&str> = left.lines().collect();
    for (key, _) in &planted[..13] {
        assert!(
            !left.iter().any(|k| k.eq_ignore_ascii_case(key)),
            "{key} is left: {left:?}"
        );
    }
    assert!(left.contains(&"user.name"), "{left:?}");
    assert!(
        left.contains(&"submodule.lib.url"),
        "an initialised submodule stays: {left:?}"
    );
    assert!(
        left.contains(&"submodule.recurse-lib.url"),
        "a submodule named recurse-… stays: {left:?}"
    );
    assert!(left.contains(&"remote.origin.url"), "{left:?}");
    Ok(())
}

/// SPEC-176 (review of the delivered head): the per-worktree configuration Git reads once
/// `extensions.worktreeConfig` is on is scrubbed too.
#[test]
fn transport_settings_of_a_clone_s_worktree_config_are_scrubbed() -> TestResult {
    let fx = Fixture::new()?;
    let (_remote, repo) = fx.repo_with_remote()?;
    fx.raw(
        &repo,
        &["config", "--local", "extensions.worktreeConfig", "true"],
    )?;
    fx.raw(
        &repo,
        &["config", "--worktree", "http.proxy", "http://127.0.0.1:9"],
    )?;
    fx.raw(
        &repo,
        &[
            "config",
            "--worktree",
            "url./tmp/evil/.insteadOf",
            "https://github.com/",
        ],
    )?;
    fx.raw(
        &repo,
        &["config", "--worktree", "user.email", "kept@example.invalid"],
    )?;
    fx.git.scrub_transport_config(&repo)?;
    let left = fx.raw(&repo, &["config", "--worktree", "--list", "--name-only"])?;
    assert_eq!(left, "user.email", "{left}");
    Ok(())
}

/// SPEC-176 (review of the delivered head): a configuration file that is a link is refused, never
/// written through: Git would write to the file it points at.
#[test]
fn a_linked_clone_configuration_is_refused_untouched() -> TestResult {
    let fx = Fixture::new()?;
    let (_remote, repo) = fx.repo_with_remote()?;
    let outside = fx.path("outside.cfg");
    fs::write(&outside, "[http]\n\tproxy = http://127.0.0.1:9\n")?;
    std::os::unix::fs::symlink(&outside, repo.join(".git/config.worktree"))?;
    assert!(fx.git.scrub_transport_config(&repo).is_err());
    assert_eq!(
        fs::read_to_string(&outside)?,
        "[http]\n\tproxy = http://127.0.0.1:9\n",
        "the file the link points at is untouched"
    );
    Ok(())
}

/// SPEC-176 (review of the delivered head): a clone whose `.git` is a pointer file, or whose
/// `.git/config` is a link, is refused before anything is written.
#[test]
fn a_redirected_clone_git_directory_is_refused() -> TestResult {
    let fx = Fixture::new()?;
    let (_remote, repo) = fx.repo_with_remote()?;
    let elsewhere = fx.path("elsewhere.git");
    fs::rename(repo.join(".git"), &elsewhere)?;
    fs::write(
        repo.join(".git"),
        format!("gitdir: {}\n", elsewhere.display()),
    )?;
    fx.raw(
        &repo,
        &["config", "--local", "http.proxy", "http://127.0.0.1:9"],
    )?;
    assert!(fx.git.scrub_transport_config(&repo).is_err());
    assert_eq!(
        fx.raw(&repo, &["config", "--local", "http.proxy"])?,
        "http://127.0.0.1:9",
        "nothing was written"
    );

    let fx = Fixture::new()?;
    let (_remote, repo) = fx.repo_with_remote()?;
    let outside = fx.path("outside.cfg");
    fs::rename(repo.join(".git/config"), &outside)?;
    std::os::unix::fs::symlink(&outside, repo.join(".git/config"))?;
    let before = fs::read_to_string(&outside)?;
    assert!(fx.git.scrub_transport_config(&repo).is_err());
    assert_eq!(fs::read_to_string(&outside)?, before);
    Ok(())
}
