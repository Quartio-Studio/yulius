//! Repository discovery, GitHub remote parsing, unpublished refs and bundles (IMP-001, IMP-002).

use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};

use yulius_contracts::import::{FileRef, RepoDescriptor};

use crate::hash::file_ref;
use crate::process::{
    BUNDLE_TIMEOUT, GitError, LIST_OUTPUT, QUERY_TIMEOUT, SMALL_OUTPUT, bytes_to_path, git,
    git_status, trim_newline,
};

/// Upload name of the unpublished-commits bundle.
pub(crate) const BUNDLE_NAME: &str = "repo.bundle";

/// A local checkout of a GitHub repository.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoInfo {
    /// Top-level directory of the checkout the user pointed at (canonical when resolvable).
    pub root: PathBuf,
    /// Canonical `https://github.com/<owner>/<name>` URL (no credentials, no `.git`).
    pub remote_url: String,
    /// GitHub owner login.
    pub owner: String,
    /// GitHub repository name.
    pub name: String,
    /// Checked-out branch, `None` for a detached `HEAD`.
    pub head_branch: Option<String>,
    /// Full object id of `HEAD`.
    pub head_sha: String,
    /// Other checkouts of the same repository (main and linked worktrees, excluding `root`).
    pub worktrees: Vec<PathBuf>,
}

impl RepoInfo {
    /// `root` followed by `worktrees`: every directory whose sessions belong to the repository.
    #[must_use]
    pub fn checkout_paths(&self) -> Vec<PathBuf> {
        std::iter::once(self.root.clone())
            .chain(self.worktrees.iter().cloned())
            .collect()
    }

    /// The manifest's repository descriptor.
    #[must_use]
    pub fn descriptor(&self) -> RepoDescriptor {
        RepoDescriptor {
            remote_url: self.remote_url.clone(),
            owner: self.owner.clone(),
            name: self.name.clone(),
            head_branch: self.head_branch.clone(),
            head_sha: self.head_sha.clone(),
        }
    }
}

/// Why a directory cannot be imported.
#[derive(Debug, thiserror::Error)]
pub enum RepoError {
    /// The path is not inside a Git working tree.
    #[error("{} is not inside a Git working tree", .0.display())]
    NotGitRepository(PathBuf),
    /// The repository has no remote at all.
    #[error(
        "the repository has no remote; every Yulius project needs a GitHub repository \
         (create it on GitHub, then `git remote add origin https://github.com/<owner>/<name>.git` and push)"
    )]
    NoRemote,
    /// No remote points at GitHub.
    #[error(
        "remote {remote:?} ({url}) is not a GitHub repository; Yulius imports only repositories \
         hosted on GitHub (add a GitHub remote named origin)"
    )]
    NotGitHub {
        /// Remote name.
        remote: String,
        /// Remote URL with any credentials removed.
        url: String,
    },
    /// `HEAD` has no commit yet.
    #[error("the repository has no commits yet; commit and push to GitHub first")]
    NoCommits,
    /// A Git invocation failed.
    #[error(transparent)]
    Git(#[from] GitError),
}

/// A GitHub repository identified from a remote URL.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitHubRemote {
    /// Owner login as written in the URL.
    pub owner: String,
    /// Repository name without `.git`.
    pub name: String,
}

impl GitHubRemote {
    /// `https://github.com/<owner>/<name>`.
    #[must_use]
    pub fn https_url(&self) -> String {
        format!("https://github.com/{}/{}", self.owner, self.name)
    }

    /// `true` when both name the same repository (GitHub names are case-insensitive).
    #[must_use]
    pub fn same_repository(&self, other: &Self) -> bool {
        self.owner.eq_ignore_ascii_case(&other.owner) && self.name.eq_ignore_ascii_case(&other.name)
    }
}

/// Parses GitHub remote URLs in HTTPS, SSH (`ssh://` and scp-like `git@github.com:`) and `git://`
/// forms, with or without `.git`, trailing `/` or credentials. Returns `None` for other hosts or
/// malformed owner/name segments.
#[must_use]
pub fn parse_github_remote(url: &str) -> Option<GitHubRemote> {
    let url = url.trim();
    let path = if let Some((scheme, rest)) = url.split_once("://") {
        if !matches!(
            scheme.to_ascii_lowercase().as_str(),
            "https" | "http" | "ssh" | "git" | "git+ssh" | "ssh+git"
        ) {
            return None;
        }
        let (authority, path) = rest.split_once('/')?;
        let host_port = authority.rsplit_once('@').map_or(authority, |(_, host)| host);
        let host = host_port.split_once(':').map_or(host_port, |(host, _)| host);
        if !is_github_host(host) {
            return None;
        }
        path
    } else {
        // scp-like: [user@]host:owner/name
        let (authority, path) = url.split_once(':')?;
        if authority.contains('/') {
            return None;
        }
        let host = authority.rsplit_once('@').map_or(authority, |(_, host)| host);
        if !is_github_host(host) {
            return None;
        }
        path.strip_prefix('/').unwrap_or(path)
    };
    let path = path.trim_end_matches('/');
    let path = path.strip_suffix(".git").unwrap_or(path);
    let (owner, name) = path.split_once('/')?;
    (is_valid_owner(owner) && is_valid_repo_name(name)).then(|| GitHubRemote {
        owner: owner.to_owned(),
        name: name.to_owned(),
    })
}

fn is_github_host(host: &str) -> bool {
    ["github.com", "www.github.com", "ssh.github.com"]
        .iter()
        .any(|known| host.eq_ignore_ascii_case(known))
}

fn is_valid_owner(owner: &str) -> bool {
    !owner.is_empty()
        && owner.len() <= 39
        && !owner.starts_with('-')
        && owner.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
}

fn is_valid_repo_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 100
        && name != "."
        && name != ".."
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}

/// Removes `user[:password]@` from URLs so errors never echo credentials.
fn redact_credentials(url: &str) -> String {
    if let Some((scheme, rest)) = url.split_once("://") {
        let (authority, path) = rest.split_once('/').unwrap_or((rest, ""));
        if let Some((_, host)) = authority.rsplit_once('@') {
            return format!("{scheme}://{host}/{path}");
        }
    }
    url.to_owned()
}

/// Locates the checkout containing `path`, its GitHub remote, `HEAD` and the repository's other
/// worktrees.
///
/// The remote is `origin` when it is a GitHub URL, otherwise the first GitHub remote. Both the
/// configured URL and the `url.<base>.insteadOf`-expanded URL are considered.
///
/// # Errors
/// [`RepoError::NotGitRepository`], [`RepoError::NoRemote`], [`RepoError::NotGitHub`],
/// [`RepoError::NoCommits`] or a Git failure.
pub fn discover_repo(path: &Path) -> Result<RepoInfo, RepoError> {
    if !path.is_dir() {
        return Err(RepoError::NotGitRepository(path.to_path_buf()));
    }
    let toplevel = match git(
        path,
        &[OsStr::new("rev-parse"), OsStr::new("--show-toplevel")],
        SMALL_OUTPUT,
        QUERY_TIMEOUT,
    ) {
        Ok(output) => bytes_to_path(trim_newline(&output.stdout)),
        Err(GitError::Failed { .. }) => return Err(RepoError::NotGitRepository(path.to_path_buf())),
        Err(other) => return Err(other.into()),
    };
    if toplevel.as_os_str().is_empty() {
        return Err(RepoError::NotGitRepository(path.to_path_buf()));
    }
    let root = canonical(&toplevel);
    let remote = find_github_remote(&root)?;
    let head_sha = head_commit(&root)?;
    let head_branch = head_branch(&root)?;
    let worktrees = list_worktrees(&root)?
        .into_iter()
        .map(|worktree| canonical(&worktree))
        .filter(|worktree| *worktree != root)
        .collect();
    Ok(RepoInfo {
        root,
        remote_url: remote.https_url(),
        owner: remote.owner,
        name: remote.name,
        head_branch,
        head_sha,
        worktrees,
    })
}

fn canonical(path: &Path) -> PathBuf {
    fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

fn text_lines(bytes: &[u8]) -> Vec<String> {
    String::from_utf8_lossy(bytes)
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_owned)
        .collect()
}

fn find_github_remote(root: &Path) -> Result<GitHubRemote, RepoError> {
    let output = git(root, &[OsStr::new("remote")], SMALL_OUTPUT, QUERY_TIMEOUT)?;
    let mut remotes = text_lines(&output.stdout);
    remotes.retain(|remote| !remote.starts_with('-'));
    if remotes.is_empty() {
        return Err(RepoError::NoRemote);
    }
    if let Some(index) = remotes.iter().position(|remote| remote == "origin") {
        let origin = remotes.remove(index);
        remotes.insert(0, origin);
    }
    let mut first_non_github = None;
    for remote in &remotes {
        let configured = config_value(root, &format!("remote.{remote}.url"))?;
        let expanded = git_status(
            root,
            &[OsStr::new("remote"), OsStr::new("get-url"), OsStr::new(remote)],
            SMALL_OUTPUT,
            QUERY_TIMEOUT,
        )?;
        let expanded = expanded
            .status
            .success()
            .then(|| String::from_utf8_lossy(trim_newline(&expanded.stdout)).into_owned());
        for candidate in configured.iter().chain(expanded.iter()) {
            if let Some(parsed) = parse_github_remote(candidate) {
                return Ok(parsed);
            }
        }
        if first_non_github.is_none() {
            let shown = configured.or(expanded).unwrap_or_default();
            first_non_github = Some((remote.clone(), redact_credentials(&shown)));
        }
    }
    match first_non_github {
        Some((remote, url)) => Err(RepoError::NotGitHub { remote, url }),
        None => Err(RepoError::NoRemote),
    }
}

fn config_value(root: &Path, key: &str) -> Result<Option<String>, GitError> {
    let run = git_status(
        root,
        &[OsStr::new("config"), OsStr::new("--get"), OsStr::new(key)],
        SMALL_OUTPUT,
        QUERY_TIMEOUT,
    )?;
    Ok(run
        .status
        .success()
        .then(|| String::from_utf8_lossy(trim_newline(&run.stdout)).into_owned()))
}

fn head_commit(root: &Path) -> Result<String, RepoError> {
    let run = git_status(
        root,
        &[
            OsStr::new("rev-parse"),
            OsStr::new("--verify"),
            OsStr::new("--quiet"),
            OsStr::new("HEAD^{commit}"),
        ],
        SMALL_OUTPUT,
        QUERY_TIMEOUT,
    )?;
    if !run.status.success() {
        return Err(RepoError::NoCommits);
    }
    let sha = String::from_utf8_lossy(trim_newline(&run.stdout)).into_owned();
    if is_object_id(&sha) {
        Ok(sha)
    } else {
        Err(GitError::Unexpected {
            command: "rev-parse".to_owned(),
            detail: "HEAD is not an object id".to_owned(),
        }
        .into())
    }
}

fn head_branch(root: &Path) -> Result<Option<String>, GitError> {
    let run = git_status(
        root,
        &[
            OsStr::new("symbolic-ref"),
            OsStr::new("--quiet"),
            OsStr::new("HEAD"),
        ],
        SMALL_OUTPUT,
        QUERY_TIMEOUT,
    )?;
    if !run.status.success() {
        return Ok(None);
    }
    let reference = String::from_utf8_lossy(trim_newline(&run.stdout)).into_owned();
    Ok(reference.strip_prefix("refs/heads/").map(str::to_owned))
}

fn is_object_id(text: &str) -> bool {
    matches!(text.len(), 40 | 64) && text.bytes().all(|b| b.is_ascii_hexdigit())
}

/// Lists every worktree path of the repository (porcelain `-z` when supported).
fn list_worktrees(root: &Path) -> Result<Vec<PathBuf>, GitError> {
    let porcelain_z = git_status(
        root,
        &[
            OsStr::new("worktree"),
            OsStr::new("list"),
            OsStr::new("--porcelain"),
            OsStr::new("-z"),
        ],
        LIST_OUTPUT,
        QUERY_TIMEOUT,
    )?;
    let (output, separator) = if porcelain_z.status.success() {
        (porcelain_z.stdout, b'\0')
    } else {
        let output = git(
            root,
            &[
                OsStr::new("worktree"),
                OsStr::new("list"),
                OsStr::new("--porcelain"),
            ],
            LIST_OUTPUT,
            QUERY_TIMEOUT,
        )?;
        (output.stdout, b'\n')
    };
    let mut worktrees = Vec::new();
    let mut current: Option<PathBuf> = None;
    for field in output.split(|b| *b == separator) {
        if let Some(path) = field.strip_prefix(b"worktree ") {
            if let Some(done) = current.take() {
                worktrees.push(done);
            }
            current = Some(bytes_to_path(path));
        } else if field == b"bare" {
            current = None;
        }
    }
    worktrees.extend(current);
    Ok(worktrees)
}

/// A local branch with commits that no remote-tracking ref contains.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnpublishedRef {
    /// Branch name without `refs/heads/`.
    pub branch: String,
    /// Object id the branch points at.
    pub sha: String,
}

/// Local branches with at least one commit not reachable from any `refs/remotes/*` ref, sorted
/// by branch name. Branches with non-UTF-8 names are ignored.
///
/// # Errors
/// A Git failure.
pub fn unpublished_refs(repo: &Path) -> Result<Vec<UnpublishedRef>, GitError> {
    let output = git(
        repo,
        &[
            OsStr::new("for-each-ref"),
            OsStr::new("--format=%(refname)%00%(objectname)"),
            OsStr::new("refs/heads/"),
        ],
        LIST_OUTPUT,
        QUERY_TIMEOUT,
    )?;
    let mut unpublished = Vec::new();
    for line in output.stdout.split(|b| *b == b'\n') {
        let Some(split) = line.iter().position(|b| *b == 0) else {
            continue;
        };
        let (Ok(refname), Ok(sha)) = (
            std::str::from_utf8(&line[..split]),
            std::str::from_utf8(&line[split + 1..]),
        ) else {
            continue;
        };
        let Some(branch) = refname.strip_prefix("refs/heads/") else {
            continue;
        };
        if !is_object_id(sha) {
            continue;
        }
        let ahead = git(
            repo,
            &[
                OsStr::new("rev-list"),
                OsStr::new("--max-count=1"),
                OsStr::new(sha),
                OsStr::new("--not"),
                OsStr::new("--remotes"),
            ],
            SMALL_OUTPUT,
            QUERY_TIMEOUT,
        )?;
        if !trim_newline(&ahead.stdout).is_empty() {
            unpublished.push(UnpublishedRef {
                branch: branch.to_owned(),
                sha: sha.to_owned(),
            });
        }
    }
    unpublished.sort_by(|a, b| a.branch.cmp(&b.branch));
    Ok(unpublished)
}

/// Writes a `git bundle` of `refs` minus everything on any remote (`--not --remotes`) to `out`.
///
/// Packing is single-threaded so identical input produces an identical bundle (and digest), which
/// lets the server skip re-uploads (IMP-006). Returns `None` when `refs` is empty or nothing would
/// be bundled.
///
/// # Errors
/// A Git failure, or an I/O error hashing the bundle.
pub fn create_bundle(
    repo: &Path,
    refs: &[UnpublishedRef],
    out: &Path,
) -> Result<Option<FileRef>, GitError> {
    if refs.is_empty() {
        return Ok(None);
    }
    let refnames: Vec<String> = refs
        .iter()
        .map(|reference| format!("refs/heads/{}", reference.branch))
        .collect();
    let mut args: Vec<&OsStr> = vec![
        OsStr::new("-c"),
        OsStr::new("pack.threads=1"),
        OsStr::new("bundle"),
        OsStr::new("create"),
        out.as_os_str(),
    ];
    args.extend(refnames.iter().map(OsStr::new));
    args.push(OsStr::new("--not"));
    args.push(OsStr::new("--remotes"));
    match git(repo, &args, SMALL_OUTPUT, BUNDLE_TIMEOUT) {
        Ok(_) => {}
        Err(error) if error.stderr().is_some_and(|text| text.contains("empty bundle")) => {
            return Ok(None);
        }
        Err(error) => return Err(error),
    }
    file_ref(out, BUNDLE_NAME)
        .map(Some)
        .map_err(|source| GitError::ReadOutput {
            command: "bundle".to_owned(),
            source,
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn remote(owner: &str, name: &str) -> Option<GitHubRemote> {
        Some(GitHubRemote {
            owner: owner.to_owned(),
            name: name.to_owned(),
        })
    }

    #[test]
    fn parses_github_remote_forms() {
        let expected = remote("Quartio-Studio", "studio");
        for url in [
            "https://github.com/Quartio-Studio/studio",
            "https://github.com/Quartio-Studio/studio.git",
            "https://github.com/Quartio-Studio/studio/",
            "https://x-access-token:secret@github.com/Quartio-Studio/studio.git",
            "http://GitHub.com/Quartio-Studio/studio",
            "git@github.com:Quartio-Studio/studio.git",
            "github.com:Quartio-Studio/studio",
            "ssh://git@github.com/Quartio-Studio/studio.git",
            "ssh://git@ssh.github.com:443/Quartio-Studio/studio.git",
            "git://github.com/Quartio-Studio/studio.git",
            "  https://www.github.com/Quartio-Studio/studio.git\n",
        ] {
            assert_eq!(parse_github_remote(url), expected, "{url}");
        }
        assert_eq!(
            parse_github_remote("git@github.com:me/my.repo_v2.git"),
            remote("me", "my.repo_v2")
        );
    }

    #[test]
    fn rejects_non_github_and_malformed_remotes() {
        for url in [
            "https://gitlab.com/Quartio-Studio/studio.git",
            "git@bitbucket.org:team/repo.git",
            "https://github.com.evil.example/a/b",
            "https://evil.example/github.com/a/b",
            "/srv/git/repo.git",
            "file:///srv/git/repo.git",
            "https://github.com/onlyowner",
            "https://github.com/owner/repo/extra",
            "https://github.com/-bad/repo",
            "https://github.com/owner/..",
            "git@github.com:owner/re po.git",
            "",
        ] {
            assert_eq!(parse_github_remote(url), None, "{url}");
        }
    }

    #[test]
    fn same_repository_ignores_case_and_form() {
        let a = parse_github_remote("git@github.com:Quartio-Studio/Studio.git");
        let b = parse_github_remote("https://github.com/quartio-studio/studio");
        assert!(matches!((a, b), (Some(a), Some(b)) if a.same_repository(&b)));
        assert_eq!(
            remote("o", "r").map(|r| r.https_url()),
            Some("https://github.com/o/r".to_owned())
        );
    }

    #[test]
    fn redacts_credentials_in_urls() {
        assert_eq!(
            redact_credentials("https://user:token@gitlab.com/a/b.git"),
            "https://gitlab.com/a/b.git"
        );
        assert_eq!(
            redact_credentials("git@gitlab.com:a/b.git"),
            "git@gitlab.com:a/b.git"
        );
    }
}
