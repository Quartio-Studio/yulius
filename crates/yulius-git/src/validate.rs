//! Input validation at the Git boundary (GIT-001): branch names, relative paths, remotes,
//! URLs, refspecs and identities. A user-supplied value is never allowed to become an option.

use std::path::{Component, Path, PathBuf};

use crate::error::{GitError, truncate_for_error};

/// Longest accepted branch name in bytes.
pub const MAX_BRANCH_LEN: usize = 255;

/// Validates a short branch name (without `refs/heads/`) with rules equivalent to
/// `git check-ref-format --branch`, plus a length bound and a ban on `HEAD`/`@`.
///
/// Rejected: empty; leading `-`; `..`; `@{`; ASCII/Unicode control characters; space,
/// `~ ^ : ? * [ \`; leading, trailing or doubled `/`; a component starting with `.` or
/// ending with `.lock`; a trailing `.`.
///
/// ```
/// use yulius_git::validate_branch_name;
/// assert!(validate_branch_name("feat/login-page").is_ok());
/// assert!(validate_branch_name("--upload-pack=x").is_err());
/// assert!(validate_branch_name("../x").is_err());
/// assert!(validate_branch_name("a..b").is_err());
/// ```
pub fn validate_branch_name(name: &str) -> Result<(), GitError> {
    let reject = |reason| {
        Err(GitError::InvalidBranchName {
            name: truncate_for_error(name),
            reason,
        })
    };
    if name.is_empty() {
        return reject("empty");
    }
    if name.len() > MAX_BRANCH_LEN {
        return reject("longer than 255 bytes");
    }
    if name.starts_with('-') {
        return reject("starts with '-'");
    }
    if name == "@" || name == "HEAD" {
        return reject("reserved name");
    }
    if name.contains("..") {
        return reject("contains '..'");
    }
    if name.contains("@{") {
        return reject("contains '@{'");
    }
    if name
        .chars()
        .any(|c| c.is_control() || matches!(c, ' ' | '~' | '^' | ':' | '?' | '*' | '[' | '\\'))
    {
        return reject("contains a forbidden character");
    }
    if name.starts_with('/') || name.ends_with('/') {
        return reject("leading or trailing '/'");
    }
    if name.ends_with('.') {
        return reject("ends with '.'");
    }
    for component in name.split('/') {
        if component.is_empty() {
            return reject("empty path component");
        }
        if component.starts_with('.') {
            return reject("component starts with '.'");
        }
        if component.ends_with(".lock") {
            return reject("component ends with '.lock'");
        }
    }
    Ok(())
}

/// Validates a full ref name under `refs/heads/` or `refs/tags/`.
pub(crate) fn validate_full_ref(name: &str) -> Result<(), GitError> {
    let short = name
        .strip_prefix("refs/heads/")
        .or_else(|| name.strip_prefix("refs/tags/"))
        .ok_or_else(|| GitError::InvalidInput {
            what: "ref",
            reason: format!(
                "{} must start with refs/heads/ or refs/tags/",
                truncate_for_error(name)
            ),
        })?;
    validate_branch_name(short)
}

/// Validates a repository-relative path: not empty, not absolute, no `..`, no `.git`
/// component, no NUL or backslash. Returns it normalized (without `.` components).
///
/// ```
/// use yulius_git::validate_relative_path;
/// assert_eq!(validate_relative_path("./src/main.rs").ok(), Some("src/main.rs".into()));
/// assert!(validate_relative_path("../etc/passwd").is_err());
/// assert!(validate_relative_path("/etc/passwd").is_err());
/// assert!(validate_relative_path("a/.git/config").is_err());
/// ```
pub fn validate_relative_path(path: &str) -> Result<PathBuf, GitError> {
    let reject = |reason| {
        Err(GitError::InvalidPath {
            path: truncate_for_error(path),
            reason,
        })
    };
    if path.is_empty() {
        return reject("empty");
    }
    if path.contains('\0') || path.contains('\\') {
        return reject("contains NUL or backslash");
    }
    if path.chars().any(char::is_control) {
        return reject("contains a control character");
    }
    let candidate = Path::new(path);
    if candidate.is_absolute() || path.starts_with('/') {
        return reject("absolute");
    }
    let mut normalized = PathBuf::new();
    for component in candidate.components() {
        match component {
            Component::Normal(part) => {
                if part.eq_ignore_ascii_case(".git") {
                    return reject("targets a .git directory");
                }
                normalized.push(part);
            }
            Component::CurDir => {}
            Component::ParentDir => return reject("contains '..'"),
            Component::RootDir | Component::Prefix(_) => return reject("absolute"),
        }
    }
    if normalized.as_os_str().is_empty() {
        return reject("refers to the repository root");
    }
    Ok(normalized)
}

/// Validates a remote name such as `origin`: `[A-Za-z0-9._-]`, not starting with `-`/`.`.
pub(crate) fn validate_remote_name(name: &str) -> Result<(), GitError> {
    let ok = !name.is_empty()
        && name.len() <= 100
        && !name.starts_with(['-', '.'])
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'));
    if ok {
        Ok(())
    } else {
        Err(GitError::InvalidInput {
            what: "remote name",
            reason: truncate_for_error(name),
        })
    }
}

/// Validates a clone/remote URL: `https`, `http`, `ssh`, `git` or `file` URLs, or an
/// absolute local path. Transport-helper syntax (`ext::`, `fd::`), option-shaped values and
/// scp-like `host:path` forms are rejected.
pub(crate) fn validate_remote_url(url: &str) -> Result<(), GitError> {
    let reject = |reason: &str| {
        Err(GitError::InvalidInput {
            what: "remote URL",
            reason: reason.to_owned(),
        })
    };
    if url.is_empty() || url.len() > 2048 {
        return reject("empty or too long");
    }
    if url.starts_with('-') {
        return reject("starts with '-'");
    }
    if url.chars().any(|c| c.is_control() || c.is_whitespace()) {
        return reject("contains whitespace or control characters");
    }
    if url.contains("::") {
        return reject("transport helper syntax is not allowed");
    }
    if let Some((scheme, _)) = url.split_once("://") {
        let scheme = scheme.to_ascii_lowercase();
        if matches!(scheme.as_str(), "https" | "http" | "ssh" | "git" | "file") {
            return Ok(());
        }
        return reject("unsupported scheme");
    }
    if url.starts_with('/') {
        return Ok(());
    }
    reject("must be an https/http/ssh/git/file URL or an absolute path")
}

/// Accepts a remote given as a configured name or a URL.
pub(crate) fn validate_remote(remote: &str) -> Result<(), GitError> {
    if validate_remote_name(remote).is_ok() {
        return Ok(());
    }
    validate_remote_url(remote)
}

/// Validates an explicit, non-forcing push refspec: `refs/heads/A:refs/heads/B` or
/// `refs/heads/A` (same name on the remote).
pub(crate) fn validate_push_refspec(refspec: &str) -> Result<(), GitError> {
    let reject = |reason: &str| {
        Err(GitError::InvalidInput {
            what: "push refspec",
            reason: format!("{}: {reason}", truncate_for_error(refspec)),
        })
    };
    if refspec.starts_with('+') {
        return reject("forced updates are not allowed");
    }
    let (source, destination) = refspec.split_once(':').unwrap_or((refspec, refspec));
    for side in [source, destination] {
        let Some(branch) = side.strip_prefix("refs/heads/") else {
            return reject("both sides must be explicit refs/heads/... names");
        };
        validate_branch_name(branch)?;
    }
    Ok(())
}

/// Validates an author/committer name or email for use as an environment value or config
/// value: non-empty, at most 256 bytes, no control characters, no `<`/`>`, no leading `-`.
pub(crate) fn validate_identity_part(what: &'static str, value: &str) -> Result<(), GitError> {
    let ok = !value.trim().is_empty()
        && value.len() <= 256
        && !value.starts_with('-')
        && !value
            .chars()
            .any(|c| c.is_control() || c == '<' || c == '>');
    if ok {
        Ok(())
    } else {
        Err(GitError::InvalidInput {
            what,
            reason: truncate_for_error(value),
        })
    }
}

/// Guards a revision expression before it reaches `rev-parse --end-of-options`.
pub(crate) fn validate_revision(rev: &str) -> Result<(), GitError> {
    let ok = !rev.is_empty()
        && rev.len() <= 512
        && !rev.starts_with('-')
        && !rev.chars().any(char::is_control);
    if ok {
        Ok(())
    } else {
        Err(GitError::InvalidInput {
            what: "revision",
            reason: truncate_for_error(rev),
        })
    }
}

/// Requires an absolute path; absolute paths can never be parsed as options.
pub(crate) fn require_absolute(what: &'static str, path: &Path) -> Result<(), GitError> {
    if path.is_absolute() {
        Ok(())
    } else {
        Err(GitError::InvalidInput {
            what,
            reason: format!("{} must be absolute", path.display()),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refspecs_must_be_explicit_and_unforced() {
        assert!(validate_push_refspec("refs/heads/feat/x:refs/heads/feat/x").is_ok());
        assert!(validate_push_refspec("refs/heads/feat/x").is_ok());
        assert!(validate_push_refspec("+refs/heads/x:refs/heads/main").is_err());
        assert!(validate_push_refspec("main").is_err());
        assert!(validate_push_refspec("refs/heads/x:refs/tags/v1").is_err());
        assert!(validate_push_refspec("refs/heads/x:refs/heads/--upload-pack=y").is_err());
        assert!(validate_push_refspec(":refs/heads/main").is_err());
    }

    #[test]
    fn urls_reject_transport_helpers_and_options() {
        assert!(validate_remote_url("https://github.com/o/r.git").is_ok());
        assert!(validate_remote_url("/srv/repos/r.git").is_ok());
        assert!(validate_remote_url("file:///srv/r.git").is_ok());
        assert!(validate_remote_url("ext::sh -c touch% /tmp/pwned").is_err());
        assert!(validate_remote_url("--upload-pack=touch /tmp/x").is_err());
        assert!(validate_remote_url("git@github.com:o/r.git").is_err());
        assert!(validate_remote_url("fd::17").is_err());
        assert!(validate_remote_url("relative/path").is_err());
    }

    #[test]
    fn remote_names_and_identities() {
        assert!(validate_remote_name("origin").is_ok());
        assert!(validate_remote_name("-oops").is_err());
        assert!(validate_remote_name("a b").is_err());
        assert!(validate_identity_part("name", "Ada Lovelace").is_ok());
        assert!(validate_identity_part("name", "Eve\nMallory").is_err());
        assert!(validate_identity_part("email", "<x@y>").is_err());
        assert!(validate_identity_part("name", "  ").is_err());
    }

    #[test]
    fn relative_paths() -> Result<(), GitError> {
        assert_eq!(validate_relative_path("a/./b")?, PathBuf::from("a/b"));
        for bad in [
            "",
            ".",
            "..",
            "a/../../b",
            "/abs",
            ".git",
            "x/.GIT/hooks",
            "a\\b",
            "a\0b",
        ] {
            assert!(validate_relative_path(bad).is_err(), "{bad:?} accepted");
        }
        Ok(())
    }

    #[test]
    fn revisions_cannot_be_options() {
        assert!(validate_revision("main~2").is_ok());
        assert!(validate_revision("--output=/tmp/x").is_err());
        assert!(validate_revision("").is_err());
    }
}
