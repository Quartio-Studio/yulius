//! Behavior of the Git adapter against real temporary repositories and a local bare remote.

use std::error::Error;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use yulius_git::{
    Author, BundleSpec, DiffStat, Git, GitError, ImportRef, MergeInto, RemovalRefusal,
    WorktreeRemoval, validate_branch_name,
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
