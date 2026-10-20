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

/// What a personal API token may do (SPEC-247, ADR-0188). A token never grants more than its
/// person's access to a sandbox; its scope can only narrow it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TokenScope {
    /// A `yulius login` token and every token made before scopes: the REST API as before, never
    /// the remote MCP, and never starting onboarding (SPEC-051 OBD-002).
    Cli,
    /// Read-only: what view access allows, through the REST API (GET/HEAD) and the remote MCP.
    Read,
    /// Everything the person may do, through the REST API and the remote MCP.
    Operate,
}

impl TokenScope {
    /// Stable wire name.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Cli => "cli",
            Self::Read => "read",
            Self::Operate => "operate",
        }
    }

    /// Parses a wire name.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "cli" => Some(Self::Cli),
            "read" => Some(Self::Read),
            "operate" => Some(Self::Operate),
            _ => None,
        }
    }

    /// Whether a token of this scope may take `action` (the person's access is checked apart).
    #[must_use]
    pub fn allows(self, action: Action) -> bool {
        match self {
            Self::Read => action == Action::View,
            Self::Cli | Self::Operate => true,
        }
    }

    /// Whether this scope may use the remote MCP.
    #[must_use]
    pub fn remote(self) -> bool {
        matches!(self, Self::Read | Self::Operate)
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn token_scopes_narrow_and_round_trip() {
        for scope in [TokenScope::Cli, TokenScope::Read, TokenScope::Operate] {
            assert_eq!(TokenScope::parse(scope.as_str()), Some(scope));
        }
        assert_eq!(TokenScope::parse("admin"), None);
        assert!(TokenScope::Read.allows(Action::View));
        for action in [Action::Work, Action::Configure, Action::Administer] {
            assert!(!TokenScope::Read.allows(action), "{action:?}");
            assert!(TokenScope::Operate.allows(action), "{action:?}");
        }
        assert!(!TokenScope::Cli.remote());
        assert!(TokenScope::Read.remote() && TokenScope::Operate.remote());
    }

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
