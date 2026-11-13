//! The server's own Git, confined (SPEC-483 G1, ADR-0322): the real adapter exactly as the
//! server builds it, in the server's folder layout, against a clone whose every file an agent
//! writes. Whatever the clone's configuration names is not run, and a link in the clone leads
//! Git nowhere outside the sandbox's folders.

use std::path::{Path, PathBuf};
use std::process::Command;

use yulius::wiring::git::GitAdapter;
use yulius_app::model::Secret;
use yulius_app::ports::git::GitPort;

type TestResult = Result<(), Box<dyn std::error::Error>>;

/// Plain Git, as the agent in its sandbox (or the test's own set-up) runs it.
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

/// A data folder as a server's: sandbox `s1` (a playground: its origin is the folder
/// `playgrounds/s1.git`) with its clone and a task's worktree, sandbox `s2` with a clone of its
/// own origin, and a folder outside both that stands for the rest of the host.
struct World {
    _dir: tempfile::TempDir,
    adapter: GitAdapter,
    /// `<data>/sandboxes/s1`: what the agents of `s1` write.
    sandbox: PathBuf,
    repo: PathBuf,
    wt: PathBuf,
    origin: PathBuf,
    other_origin: PathBuf,
    other_repo: PathBuf,
    outside: PathBuf,
}

/// A bare repository at `path` with one commit on `main` holding `file`.
fn seeded(path: &Path, file: &str, text: &str) -> TestResult {
    std::fs::create_dir_all(path)?;
    git(path, &["init", "--bare", "-q", "-b", "main"])?;
    let seed = path.with_extension("seed");
    std::fs::create_dir_all(&seed)?;
    git(&seed, &["init", "-q", "-b", "main"])?;
    std::fs::write(seed.join(file), text)?;
    git(&seed, &["add", "-A"])?;
    git(&seed, &["commit", "-q", "-m", "first"])?;
    let target = path.to_string_lossy().into_owned();
    git(&seed, &["push", "-q", &target, "main"])?;
    std::fs::remove_dir_all(&seed)?;
    Ok(())
}

async fn world() -> Result<World, Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let root = dir.path().canonicalize()?;
    let origin = root.join("playgrounds/s1.git");
    let other_origin = root.join("playgrounds/s2.git");
    seeded(&origin, "README.md", "# one\n")?;
    seeded(&other_origin, "theirs.txt", "the-other-projects-code\n")?;
    let outside = root.join("outside");
    std::fs::create_dir_all(&outside)?;
    let adapter = crate::support::strict_git();
    let sandbox = root.join("sandboxes/s1");
    let repo = sandbox.join("repo");
    let other_repo = root.join("sandboxes/s2/repo");
    std::fs::create_dir_all(sandbox.join("worktrees"))?;
    std::fs::create_dir_all(root.join("sandboxes/s2"))?;
    adapter
        .sync_clone(&origin.to_string_lossy(), &repo, &token())
        .await?;
    adapter
        .sync_clone(&other_origin.to_string_lossy(), &other_repo, &token())
        .await?;
    let wt = sandbox.join("worktrees/t1");
    let base = git(&repo, &["rev-parse", "HEAD"])?;
    adapter.add_worktree(&repo, &wt, "feat/1", &base).await?;
    adapter
        .set_identity(&wt, "Alice", "alice@example.com")
        .await?;
    Ok(World {
        _dir: dir,
        adapter,
        sandbox,
        repo,
        wt,
        origin,
        other_origin,
        other_repo,
        outside,
    })
}

impl World {
    /// A shell command that leaves `name` in the sandbox's folder, where a confined Git could
    /// write it: its absence says the command did not run, not that it could not write.
    fn marking(&self, name: &str) -> (String, PathBuf) {
        let marker = self.sandbox.join(name);
        (format!("sh -c 'echo ran > {}'", marker.display()), marker)
    }
}

/// The everyday work in the server's layout, confined and with no shell but for the transfers
/// Yulius names itself: a clone from the sandbox's own origin, a worktree, a commit, the
/// review's diff, a push to the origin by its remote's name, the origin's answer, a fetch and
/// merge of the integration branch, and the worktree's removal.
#[tokio::test]
async fn a_sandboxes_everyday_git_works_confined() -> TestResult {
    let w = world().await?;
    std::fs::write(w.wt.join("a.txt"), "a\n")?;
    assert!(w.adapter.has_changes(&w.wt).await?);
    assert!(
        w.adapter
            .commit_all(&w.wt, "feat: a", "Alice", "alice@example.com")
            .await?
    );
    let review = w.adapter.review_changes(&w.wt, "origin/main").await?;
    assert_eq!(review.files.len(), 1);
    w.adapter.push(&w.wt, "feat/1", "feat/1", &token()).await?;
    let head = git(&w.wt, &["rev-parse", "HEAD"])?;
    assert_eq!(
        w.adapter
            .remote_branch_head(&w.wt, "feat/1", &token())
            .await?
            .as_deref(),
        Some(head.as_str())
    );
    assert_eq!(git(&w.origin, &["rev-parse", "feat/1"])?, head);
    // The origin moves on; the worktree takes it in.
    let elsewhere = w.outside.join("elsewhere");
    git(
        &w.outside,
        &["clone", "-q", &w.origin.to_string_lossy(), "elsewhere"],
    )?;
    std::fs::write(elsewhere.join("b.txt"), "b\n")?;
    git(&elsewhere, &["add", "-A"])?;
    git(&elsewhere, &["commit", "-q", "-m", "b"])?;
    git(&elsewhere, &["push", "-q", "origin", "main"])?;
    w.adapter.merge_integration(&w.wt, "main", &token()).await?;
    assert!(
        w.wt.join("b.txt").is_file(),
        "the merge brought the origin's work"
    );
    w.adapter
        .discard_worktree(&w.repo, &w.wt, &w.sandbox.join("worktrees"), Some("feat/1"))
        .await?;
    assert!(!w.wt.exists(), "the worktree is gone");
    Ok(())
}

/// G1: what the clone's configuration names is never run by the server's Git. Each case sets
/// what an agent can set (the clone's `.git/config`, a `.gitattributes`) so that the named
/// command would leave a marker, and then lets the server do its ordinary work.
#[tokio::test]
async fn nothing_the_clones_configuration_names_is_run() -> TestResult {
    let w = world().await?;
    let mut markers = Vec::new();
    let mut mark = |name: &str| {
        let (command, marker) = w.marking(name);
        markers.push(marker);
        command
    };

    // A filter and a diff driver for every file, a signing program, an editor, a pager.
    std::fs::write(
        w.wt.join(".gitattributes"),
        "* filter=mine diff=mine merge=mine\n",
    )?;
    for (key, name) in [
        ("filter.mine.clean", "ran-clean"),
        ("filter.mine.smudge", "ran-smudge"),
        ("filter.mine.process", "ran-process"),
        ("diff.mine.textconv", "ran-textconv"),
        ("diff.mine.command", "ran-diff-command"),
        ("diff.external", "ran-diff-external"),
        ("merge.mine.driver", "ran-merge-driver"),
        ("core.editor", "ran-editor"),
        ("core.pager", "ran-pager"),
        ("gpg.program", "ran-gpg"),
        ("core.sshCommand", "ran-ssh"),
        ("core.fsmonitor", "ran-fsmonitor"),
        ("credential.helper", "ran-credential"),
        ("core.alternateRefsCommand", "ran-alternate-refs"),
    ] {
        let command = mark(name);
        git(&w.repo, &["config", key, &command])?;
    }
    git(&w.repo, &["config", "commit.gpgSign", "true"])?;
    let hook = w.repo.join(".git/hooks/pre-commit");
    std::fs::write(&hook, format!("#!/bin/sh\n{}\n", mark("ran-hook")))?;
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755))?;
    }

    // The server's ordinary work in that worktree. Each may fail (a commit that must be
    // signed by a program that cannot run is not made): nothing of it runs the commands.
    std::fs::write(w.wt.join("a.txt"), "a\n")?;
    let _ = w.adapter.has_changes(&w.wt).await;
    let _ = w
        .adapter
        .commit_all(&w.wt, "feat: a", "Alice", "alice@example.com")
        .await;
    let _ = w.adapter.review_changes(&w.wt, "origin/main").await;
    let _ = w.adapter.diff_stat(&w.wt, "origin/main", "HEAD").await;
    let _ = w.adapter.log(&w.wt, "HEAD", 5).await;
    let _ = w.adapter.push(&w.wt, "feat/1", "feat/1", &token()).await;
    let _ = w.adapter.merge_integration(&w.wt, "main", &token()).await;
    let _ = w
        .adapter
        .sync_clone(&w.origin.to_string_lossy(), &w.repo, &token())
        .await;

    // The remote's own programs, for a remote named by the configuration: an address over
    // ssh, and a program for the other side of a transfer with a folder.
    git(
        &w.repo,
        &[
            "config",
            "remote.origin.url",
            "ssh://git@example.invalid/x.git",
        ],
    )?;
    let _ = w.adapter.merge_integration(&w.wt, "main", &token()).await;
    let _ = w.adapter.push(&w.wt, "feat/1", "feat/1", &token()).await;
    git(
        &w.repo,
        &["config", "remote.origin.url", &w.origin.to_string_lossy()],
    )?;
    let upload = mark("ran-uploadpack");
    let receive = mark("ran-receivepack");
    git(&w.repo, &["config", "remote.origin.uploadpack", &upload])?;
    git(&w.repo, &["config", "remote.origin.receivepack", &receive])?;
    let _ = w.adapter.merge_integration(&w.wt, "main", &token()).await;
    let _ = w.adapter.push(&w.wt, "feat/1", "feat/1", &token()).await;
    let _ = w
        .adapter
        .remote_branch_head(&w.wt, "feat/1", &token())
        .await;
    // A rewritten address that turns the folder into an ssh address, where a transfer with the
    // sandbox's own origin may start the shell.
    let prefix = format!("{}/", w.origin.parent().ok_or("parent")?.display());
    git(
        &w.repo,
        &[
            "config",
            "url.ssh://git@example.invalid/.insteadOf",
            &prefix,
        ],
    )?;
    let _ = w.adapter.push(&w.wt, "feat/1", "feat/1", &token()).await;
    let _ = w.adapter.merge_integration(&w.wt, "main", &token()).await;

    let ran: Vec<&PathBuf> = markers.iter().filter(|marker| marker.exists()).collect();
    assert!(ran.is_empty(), "the server's Git ran: {ran:?}");
    Ok(())
}

/// G1: a link in the clone leads the server's Git nowhere outside the sandbox's folders: a
/// file of the host it would overwrite stays, one it would read is not repeated in an error,
/// and another sandbox's objects are not read through an alternate.
#[tokio::test]
async fn a_link_in_the_clone_leads_git_nowhere_outside() -> TestResult {
    let w = world().await?;
    // Written: Git writes what it fetched to `FETCH_HEAD`, through a link too.
    let victim = w.outside.join("victim");
    std::fs::write(&victim, "the host's own file\n")?;
    let fetch_head = w.repo.join(".git/FETCH_HEAD");
    let _ = std::fs::remove_file(&fetch_head);
    std::os::unix::fs::symlink(&victim, &fetch_head)?;
    let _ = w.adapter.merge_integration(&w.wt, "main", &token()).await;
    let _ = w
        .adapter
        .sync_clone(&w.origin.to_string_lossy(), &w.repo, &token())
        .await;
    assert_eq!(std::fs::read_to_string(&victim)?, "the host's own file\n");
    std::fs::remove_file(&fetch_head)?;

    // Read: Git repeats a line of `shallow` it cannot parse in its error.
    let secret = w.outside.join("key");
    std::fs::write(&secret, "the-instances-secret-line\n")?;
    std::os::unix::fs::symlink(&secret, w.repo.join(".git/shallow"))?;
    let said = [
        format!("{:?}", w.adapter.log(&w.wt, "HEAD", 5).await),
        format!("{:?}", w.adapter.has_changes(&w.wt).await),
        format!(
            "{:?}",
            w.adapter.merge_integration(&w.wt, "main", &token()).await
        ),
        format!("{:?}", w.adapter.review_changes(&w.wt, "origin/main").await),
    ];
    for answer in &said {
        assert!(!answer.contains("the-instances-secret-line"), "{answer}");
    }
    std::fs::remove_file(w.repo.join(".git/shallow"))?;

    // Another sandbox's objects as an alternate, and its commit by id: its file is not shown.
    let theirs = git(&w.other_repo, &["rev-parse", "HEAD"])?;
    std::fs::write(
        w.repo.join(".git/objects/info/alternates"),
        format!("{}\n", w.other_repo.join(".git/objects").display()),
    )?;
    let shown = w.adapter.show_file(&w.repo, &theirs, "theirs.txt").await;
    assert!(
        !format!("{shown:?}").contains("the-other-projects-code"),
        "{shown:?}"
    );
    Ok(())
}

/// G1: of the folders of this machine, a transfer by the remote's name goes only to the
/// sandbox's own origin. A configuration that names another sandbox's origin, another
/// sandbox's clone or a folder of the host gets no transfer: nothing arrives there and nothing
/// comes from there.
#[tokio::test]
async fn only_the_sandboxes_own_origin_is_taken_from_its_configuration() -> TestResult {
    let w = world().await?;
    std::fs::write(w.wt.join("a.txt"), "a\n")?;
    w.adapter
        .commit_all(&w.wt, "feat: a", "Alice", "alice@example.com")
        .await?;
    let bare = w.outside.join("bare.git");
    std::fs::create_dir_all(&bare)?;
    git(&bare, &["init", "--bare", "-q", "-b", "main"])?;
    for elsewhere in [&w.other_origin, &w.other_repo, &bare] {
        git(
            &w.repo,
            &["config", "remote.origin.url", &elsewhere.to_string_lossy()],
        )?;
        let pushed = w.adapter.push(&w.wt, "feat/1", "feat/1", &token()).await;
        assert!(pushed.is_err(), "{}: {pushed:?}", elsewhere.display());
        assert!(
            git(
                elsewhere,
                &["rev-parse", "--verify", "--quiet", "refs/heads/feat/1"]
            )
            .is_err(),
            "the branch arrived at {}",
            elsewhere.display()
        );
        let fetched = w.adapter.merge_integration(&w.wt, "main", &token()).await;
        assert!(fetched.is_err(), "{}: {fetched:?}", elsewhere.display());
        assert!(
            !w.wt.join("theirs.txt").exists(),
            "another project's work was merged from {}",
            elsewhere.display()
        );
        let asked = w.adapter.remote_branch_head(&w.wt, "main", &token()).await;
        assert!(asked.is_err(), "{}: {asked:?}", elsewhere.display());
    }
    // Its own origin again: the push arrives.
    git(
        &w.repo,
        &["config", "remote.origin.url", &w.origin.to_string_lossy()],
    )?;
    w.adapter.push(&w.wt, "feat/1", "feat/1", &token()).await?;
    assert!(git(&w.origin, &["rev-parse", "--verify", "refs/heads/feat/1"]).is_ok());
    Ok(())
}

/// The launcher itself: it finds Git by the host's `PATH`, says why Git was not started, and
/// the Git it starts executes nothing but Git's own programs: not the shell (an alias written
/// as a shell command), not another program by its path, unless the shell was asked for (a
/// transfer between two folders), and then still no other program.
#[test]
fn the_launcher_starts_git_which_executes_gits_programs_only() -> TestResult {
    let yulius = env!("CARGO_BIN_EXE_yulius");
    // In a folder of its own, which is all the confined Git may touch.
    let dir = tempfile::tempdir()?;
    let run = |args: &[&str]| {
        Command::new(yulius)
            .arg("git-confined")
            .arg("--rw")
            .arg(dir.path())
            .args(args)
            .current_dir(dir.path())
            .output()
    };
    let version = run(&["--", "git", "--version"])?;
    assert!(version.status.success(), "{version:?}");
    assert!(String::from_utf8_lossy(&version.stdout).starts_with("git version"));
    let missing = run(&["--", "no-such-git-program", "--version"])?;
    assert_eq!(missing.status.code(), Some(126));
    assert!(
        String::from_utf8_lossy(&missing.stderr).contains("was not found on the host's PATH"),
        "{missing:?}"
    );

    let said = |output: &std::process::Output| String::from_utf8_lossy(&output.stdout).into_owned();
    let by_shell = run(&["--", "git", "-c", "alias.go=!echo ran-by-shell", "go"])?;
    assert!(!said(&by_shell).contains("ran-by-shell"), "{by_shell:?}");
    let by_path = run(&["--", "git", "-c", "alias.go=!/usr/bin/env", "go"])?;
    assert!(!by_path.status.success(), "{by_path:?}");
    // With the shell: a shell command runs, a program that is not Git's still does not.
    let with_shell = run(&[
        "--shell",
        "--",
        "git",
        "-c",
        "alias.go=!echo ran-by-shell",
        "go",
    ])?;
    assert!(said(&with_shell).contains("ran-by-shell"), "{with_shell:?}");
    let other = run(&[
        "--shell",
        "--",
        "git",
        "-c",
        "alias.go=!/usr/bin/env echo ran-another-program",
        "go",
    ])?;
    assert!(!said(&other).contains("ran-another-program"), "{other:?}");
    Ok(())
}
