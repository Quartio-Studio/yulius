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
    // An alternate, so that a transfer has refs of one to list (`core.alternateRefsCommand`).
    let alternate = w.sandbox.join("alternate.git");
    std::fs::create_dir_all(&alternate)?;
    git(&alternate, &["init", "--bare", "-q", "-b", "main"])?;
    std::fs::write(
        w.repo.join(".git/objects/info/alternates"),
        format!("{}\n", alternate.join("objects").display()),
    )?;
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

/// The program loader is among what a confined Git executes (the kernel starts every program
/// through it), and the loader runs any file it can read: one of the clone's. So nothing the
/// configuration names may be started with an argument an agent chooses. Here the clone names
/// the loader as its external diff and text conversion, for a new file in a folder that is a
/// program (a copy of `touch`, which would leave files named after Git's other arguments), and
/// as its `ssh` command for an address whose host part is a path; the server's work in that
/// worktree runs neither.
#[tokio::test]
async fn the_loader_is_never_started_with_a_file_of_the_clone() -> TestResult {
    let Some(loader) = ["/lib64/ld-linux-x86-64.so.2", "/lib/ld-linux-aarch64.so.1"]
        .into_iter()
        .find(|loader| Path::new(loader).exists())
    else {
        eprintln!("skipped: no known program loader on this machine");
        return Ok(());
    };
    let w = world().await?;
    std::fs::create_dir_all(w.wt.join("sub"))?;
    std::fs::copy("/usr/bin/touch", w.wt.join("sub/tool.bin"))?;
    git(&w.wt, &["add", "-N", "sub/tool.bin"])?;
    std::fs::write(w.repo.join(".git/info/attributes"), "*.bin diff=mine\n")?;
    for key in [
        "diff.external",
        "diff.mine.command",
        "diff.mine.textconv",
        "core.sshCommand",
    ] {
        git(&w.repo, &["config", key, loader])?;
    }
    git(&w.repo, &["config", "log.showSignature", "true"])?;
    git(&w.repo, &["config", "gpg.program", loader])?;

    let _ = w.adapter.has_changes(&w.wt).await;
    let _ = w.adapter.review_changes(&w.wt, "origin/main").await;
    let _ = w.adapter.diff_stat(&w.wt, "origin/main", "HEAD").await;
    let _ = w.adapter.log(&w.wt, "HEAD", 5).await;
    let _ = w
        .adapter
        .commit_all(&w.wt, "feat: a tool", "Alice", "alice@example.com")
        .await;
    let _ = w.adapter.review_changes(&w.wt, "origin/main").await;
    let _ = w.adapter.diff_stat(&w.wt, "origin/main", "HEAD").await;
    let _ = w
        .adapter
        .review_commits(&w.wt, "origin/main", "HEAD", 5)
        .await;
    git(
        &w.repo,
        &["config", "remote.origin.url", "ssh://sub/tool.bin"],
    )?;
    let _ = w.adapter.merge_integration(&w.wt, "main", &token()).await;
    let _ = w.adapter.push(&w.wt, "feat/1", "feat/1", &token()).await;

    // `touch` run by the loader would have made files named after Git's other arguments
    // (modes, object ids, the command for the other side) where it ran.
    let made: Vec<String> = [&w.wt, &w.repo]
        .iter()
        .flat_map(|folder| std::fs::read_dir(folder).into_iter().flatten().flatten())
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| !["README.md", "sub", ".git", "a.txt"].contains(&name.as_str()))
        .collect();
    assert!(
        made.is_empty(),
        "the loader ran the clone's program: {made:?}"
    );
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

/// G2 (ADR-0323): a transfer with a remote on the web carries the server's token, and in the
/// clone it would be shaped by the clone's configuration, where a key for a URL beats the
/// server's own. Here the clone names a proxy (for every address and for the remote's) and
/// switches the certificate check off; the remote itself answers nobody. The server's fetch,
/// push and listing fail, as the remote is not there, and the proxy is never asked: they are
/// made in a repository of the server's own, which is gone afterwards.
#[tokio::test]
async fn a_transfer_with_the_token_is_not_shaped_by_the_clone() -> TestResult {
    let w = world().await?;
    std::fs::write(w.wt.join("a.txt"), "a\n")?;
    w.adapter
        .commit_all(&w.wt, "feat: a", "Alice", "alice@example.com")
        .await?;
    let proxy = std::net::TcpListener::bind("127.0.0.1:0")?;
    proxy.set_nonblocking(true)?;
    let at = format!("http://{}", proxy.local_addr()?);
    // Nothing listens at the remote's address: a connection there is refused at once.
    let remote = "https://127.0.0.1:1/acme/app.git";
    git(&w.repo, &["config", "remote.origin.url", remote])?;
    for (key, value) in [
        ("http.proxy", at.as_str()),
        ("http.https://127.0.0.1:1/.proxy", at.as_str()),
        ("http.https://127.0.0.1:1/acme/app.git.proxy", at.as_str()),
        ("http.sslVerify", "false"),
        ("http.https://127.0.0.1:1/.sslVerify", "false"),
    ] {
        git(&w.repo, &["config", key, value])?;
    }
    let secret = Secret("the-servers-token-0451".into());
    let fetched = w.adapter.merge_integration(&w.wt, "main", &secret).await;
    let pushed = w.adapter.push(&w.wt, "feat/1", "feat/1", &secret).await;
    let listed = w.adapter.remote_branch_head(&w.wt, "feat/1", &secret).await;
    for answer in [
        format!("{fetched:?}"),
        format!("{pushed:?}"),
        format!("{listed:?}"),
    ] {
        assert!(answer.starts_with("Err"), "{answer}");
        assert!(!answer.contains("the-servers-token-0451"), "{answer}");
    }
    assert!(
        matches!(proxy.accept(), Err(error) if error.kind() == std::io::ErrorKind::WouldBlock),
        "the clone's proxy was asked for the server's connection"
    );
    // What the transfers were made in is gone.
    let couriers = w.sandbox.join("../../git-couriers");
    let left: Vec<_> = std::fs::read_dir(&couriers)?.flatten().collect();
    assert!(left.is_empty(), "{left:?}");

    // A worktree that names another sandbox's repository as its own: no transfer is made
    // with that repository's objects (the confined Git cannot even read it).
    let commondir = w.repo.join(".git/worktrees/t1/commondir");
    std::fs::write(
        &commondir,
        format!("{}\n", w.other_repo.join(".git").display()),
    )?;
    let lent = w.adapter.push(&w.wt, "feat/1", "feat/1", &secret).await;
    assert!(lent.is_err(), "{lent:?}");
    assert!(
        matches!(proxy.accept(), Err(error) if error.kind() == std::io::ErrorKind::WouldBlock),
        "still not asked"
    );
    Ok(())
}

/// The same transfers when the remote answers: the tests' stand-in for GitHub (a folder that
/// `https://github.com/` is rewritten to, for the server's own configuration only). A fetch
/// through the courier brings the remote's new work, a push arrives, the remote's head is
/// read, a branch gone from the remote goes from the clone, and the clone's own configuration
/// never saw the rewrite.
#[tokio::test]
async fn a_fetch_and_a_push_through_the_servers_own_repository_arrive() -> TestResult {
    let dir = tempfile::tempdir()?;
    let root = dir.path().canonicalize()?;
    let github = root.join("github");
    let origin = github.join("acme/app.git");
    seeded(&origin, "README.md", "# one\n")?;
    git(&origin, &["branch", "gone"])?;
    let adapter = crate::support::strict_git().with_github_base(&github);
    let url = "https://github.com/acme/app.git";
    let sandbox = root.join("sandboxes/s1");
    let (repo, wt) = (sandbox.join("repo"), sandbox.join("worktrees/t1"));
    std::fs::create_dir_all(sandbox.join("worktrees"))?;
    adapter.sync_clone(url, &repo, &token()).await?;
    assert_eq!(git(&repo, &["config", "remote.origin.url"])?, url);
    let base = git(&repo, &["rev-parse", "HEAD"])?;
    adapter.add_worktree(&repo, &wt, "feat/1", &base).await?;
    adapter
        .set_identity(&wt, "Alice", "alice@example.com")
        .await?;
    std::fs::write(wt.join("a.txt"), "a\n")?;
    adapter
        .commit_all(&wt, "feat: a", "Alice", "alice@example.com")
        .await?;

    adapter.push(&wt, "feat/1", "feat/1", &token()).await?;
    let head = git(&wt, &["rev-parse", "HEAD"])?;
    assert_eq!(git(&origin, &["rev-parse", "feat/1"])?, head);
    assert_eq!(
        adapter
            .remote_branch_head(&wt, "feat/1", &token())
            .await?
            .as_deref(),
        Some(head.as_str())
    );
    // A push that would rewind the remote's branch is refused, as before.
    git(&wt, &["reset", "-q", "--hard", &base])?;
    assert!(
        adapter
            .push(&wt, "feat/1", "feat/1", &token())
            .await
            .is_err()
    );
    assert_eq!(git(&origin, &["rev-parse", "feat/1"])?, head);

    // The remote moves on and drops a branch.
    let elsewhere = root.join("elsewhere");
    git(
        &root,
        &["clone", "-q", &origin.to_string_lossy(), "elsewhere"],
    )?;
    std::fs::write(elsewhere.join("b.txt"), "b\n")?;
    git(&elsewhere, &["add", "-A"])?;
    git(&elsewhere, &["commit", "-q", "-m", "b"])?;
    git(&elsewhere, &["push", "-q", "origin", "main"])?;
    git(&elsewhere, &["push", "-q", "origin", "--delete", "gone"])?;
    assert!(git(&repo, &["rev-parse", "--verify", "--quiet", "origin/gone"]).is_ok());
    adapter.merge_integration(&wt, "main", &token()).await?;
    assert!(wt.join("b.txt").is_file(), "the remote's work arrived");
    assert!(
        git(&repo, &["rev-parse", "--verify", "--quiet", "origin/gone"]).is_err(),
        "a branch gone from the remote is pruned"
    );
    adapter.sync_clone(url, &repo, &token()).await?;
    let left: Vec<_> = std::fs::read_dir(root.join("git-couriers"))?
        .flatten()
        .collect();
    assert!(left.is_empty(), "{left:?}");
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

/// `yulius doctor` says whether the server's Git can be confined on this host: here it can.
#[test]
fn the_doctor_reports_the_confinement() -> TestResult {
    let dir = tempfile::tempdir()?;
    let doctor = Command::new(env!("CARGO_BIN_EXE_yulius"))
        .arg("doctor")
        .current_dir(dir.path())
        .output()?;
    let said = String::from_utf8_lossy(&doctor.stdout).into_owned();
    let line = said
        .lines()
        .find(|line| line.contains("git confinement"))
        .ok_or_else(|| format!("no such check in: {said}"))?;
    assert!(line.starts_with("ok"), "{line}");
    Ok(())
}
