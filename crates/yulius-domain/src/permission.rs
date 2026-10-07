//! GitHub-mirrored permission levels and what each level allows (SPEC-001 ACC-007).

/// A user's effective access to one sandbox, derived from their GitHub repository permission.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum AccessLevel {
    /// No repository access: the sandbox is invisible.
    None,
    /// GitHub `pull`/`triage`: view only.
    Read,
    /// GitHub `push`/`maintain`: create, continue and steer tasks.
    Write,
    /// GitHub `admin`: also configure the sandbox.
    Admin,
}

/// Permission flags as GitHub reports them for a repository (`permissions` object).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct GitHubPermissions {
    /// `admin`
    pub admin: bool,
    /// `maintain`
    pub maintain: bool,
    /// `push`
    pub push: bool,
    /// `triage`
    pub triage: bool,
    /// `pull`
    pub pull: bool,
}

impl AccessLevel {
    /// Maps GitHub repository permissions to a Yulius access level.
    #[must_use]
    pub fn from_github(perms: GitHubPermissions) -> Self {
        if perms.admin {
            Self::Admin
        } else if perms.maintain || perms.push {
            Self::Write
        } else if perms.triage || perms.pull {
            Self::Read
        } else {
            Self::None
        }
    }

    /// Whether `self` satisfies the requirement `needed`.
    #[must_use]
    pub fn allows(self, action: Action) -> bool {
        self >= action.required_level()
    }

    /// Stable wire name.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Read => "read",
            Self::Write => "write",
            Self::Admin => "admin",
        }
    }

    /// Parses a wire name; unknown text maps to `None` (fail closed).
    #[must_use]
    pub fn parse_or_none(text: &str) -> Self {
        match text {
            "read" => Self::Read,
            "write" => Self::Write,
            "admin" => Self::Admin,
            _ => Self::None,
        }
    }
}

/// Sandbox-scoped actions subject to authorization.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Action {
    /// See the sandbox, tasks, transcripts, steps and read-only terminals.
    View,
    /// Create, continue, stop, switch or type into tasks; use browser, editor and shell.
    Work,
    /// Manage automations, workflows and sandbox MCP servers.
    Configure,
    /// Change delivery profile, skip onboarding, remove the sandbox.
    Administer,
}

impl Action {
    /// Minimum level for this action.
    #[must_use]
    pub fn required_level(self) -> AccessLevel {
        match self {
            Self::View => AccessLevel::Read,
            Self::Work | Self::Configure => AccessLevel::Write,
            Self::Administer => AccessLevel::Admin,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn github_mapping_and_allowances() {
        let none = AccessLevel::from_github(GitHubPermissions::default());
        assert_eq!(none, AccessLevel::None);
        assert!(!none.allows(Action::View));
        let read = AccessLevel::from_github(GitHubPermissions {
            pull: true,
            ..Default::default()
        });
        assert_eq!(read, AccessLevel::Read);
        assert!(read.allows(Action::View) && !read.allows(Action::Work));
        let triage = AccessLevel::from_github(GitHubPermissions {
            triage: true,
            pull: true,
            ..Default::default()
        });
        assert_eq!(triage, AccessLevel::Read);
        let write = AccessLevel::from_github(GitHubPermissions {
            push: true,
            pull: true,
            ..Default::default()
        });
        assert!(
            write.allows(Action::Work)
                && write.allows(Action::Configure)
                && !write.allows(Action::Administer)
        );
        let maintain = AccessLevel::from_github(GitHubPermissions {
            maintain: true,
            ..Default::default()
        });
        assert_eq!(maintain, AccessLevel::Write);
        let admin = AccessLevel::from_github(GitHubPermissions {
            admin: true,
            ..Default::default()
        });
        assert!(admin.allows(Action::Administer));
        assert_eq!(AccessLevel::parse_or_none("bogus"), AccessLevel::None);
    }
}
