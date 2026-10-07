//! Validation of values that become URL path segments or request fields.

use crate::error::GitHubError;

fn invalid(what: &'static str, value: &str) -> GitHubError {
    let shown: String = value.chars().take(100).collect();
    GitHubError::InvalidInput {
        what,
        reason: format!("{shown:?} is not allowed"),
    }
}

/// GitHub account login: 1–100 of `[A-Za-z0-9_-]`, not starting with `-`.
pub(crate) fn owner(login: &str) -> Result<(), GitHubError> {
    let ok = !login.is_empty()
        && login.len() <= 100
        && !login.starts_with('-')
        && login
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    if ok {
        Ok(())
    } else {
        Err(invalid("owner login", login))
    }
}

/// Repository name: 1–100 of `[A-Za-z0-9._-]`, not `.` or `..`.
pub(crate) fn repo_name(name: &str) -> Result<(), GitHubError> {
    let ok = !name.is_empty()
        && name.len() <= 100
        && name != "."
        && name != ".."
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'));
    if ok {
        Ok(())
    } else {
        Err(invalid("repository name", name))
    }
}

/// Branch name with `git check-ref-format --branch` equivalent rules.
pub(crate) fn branch(name: &str) -> Result<(), GitHubError> {
    let forbidden =
        |c: char| c.is_control() || matches!(c, ' ' | '~' | '^' | ':' | '?' | '*' | '[' | '\\');
    let ok = !name.is_empty()
        && name.len() <= 255
        && !name.starts_with(['-', '/'])
        && !name.ends_with(['/', '.'])
        && name != "@"
        && name != "HEAD"
        && !name.contains("..")
        && !name.contains("@{")
        && !name.chars().any(forbidden)
        && name
            .split('/')
            .all(|c| !c.is_empty() && !c.starts_with('.') && !c.ends_with(".lock"));
    if ok {
        Ok(())
    } else {
        Err(invalid("branch name", name))
    }
}

/// Full SHA-1 or SHA-256 commit id in lowercase hex.
pub(crate) fn sha(value: &str) -> Result<(), GitHubError> {
    let ok = (value.len() == 40 || value.len() == 64)
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b));
    if ok {
        Ok(())
    } else {
        Err(invalid("commit sha", value))
    }
}

/// A branch name or a full commit id.
pub(crate) fn branch_or_sha(value: &str) -> Result<(), GitHubError> {
    if sha(value).is_ok() {
        return Ok(());
    }
    branch(value)
}

/// Opaque one-line value (OAuth code, state): non-empty, bounded, no control characters.
pub(crate) fn opaque(what: &'static str, value: &str) -> Result<(), GitHubError> {
    let ok = !value.is_empty() && value.len() <= 512 && !value.chars().any(char::is_control);
    if ok {
        Ok(())
    } else {
        Err(invalid(what, "<rejected>"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_segments_cannot_escape() {
        assert!(owner("Quartio-Studio").is_ok());
        assert!(owner("../admin").is_err());
        assert!(owner("a/b").is_err());
        assert!(repo_name("yulius-v2").is_ok());
        assert!(repo_name("..").is_err());
        assert!(repo_name("a/b").is_err());
        assert!(branch("feat/x").is_ok());
        assert!(branch("a..b").is_err());
        assert!(branch("-x").is_err());
        assert!(branch("x?y").is_err());
        assert!(sha(&"a".repeat(40)).is_ok());
        assert!(sha("main").is_err());
        assert!(branch_or_sha("main").is_ok());
    }
}
