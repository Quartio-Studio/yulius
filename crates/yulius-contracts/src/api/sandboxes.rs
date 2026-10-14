//! Sandboxes (projects), repositories, templates and delivery profiles.

use serde::{Deserialize, Serialize};

/// A sandbox as seen by one user.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Sandbox {
    /// Sandbox id.
    pub id: String,
    /// URL slug (unique per instance).
    pub slug: String,
    /// Display name (repository name).
    pub name: String,
    /// GitHub repository (a playground's owner and URL are empty).
    pub repo: RepoRef,
    /// A playground (SPEC-099): its Git stays on this server, nothing goes to GitHub.
    #[serde(default)]
    pub playground: bool,
    /// The caller's access level: `read`, `write` or `admin`.
    pub access: String,
    /// Repository default branch.
    pub default_branch: String,
    /// Delivery profile.
    pub profile: Profile,
    /// `not_needed`, `pending`, `preparing`, `ready` or `skipped`.
    pub onboarding: String,
    /// First development-server port of the sandbox range.
    pub port_base: u16,
    /// Number of ports in the range.
    pub port_count: u16,
    /// Live task counters.
    pub counts: SandboxCounts,
}

/// GitHub repository reference.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepoRef {
    /// Owner login.
    pub owner: String,
    /// Repository name.
    pub name: String,
    /// Browser URL.
    pub url: String,
}

/// What a sandbox's onboarding does: its task's plan, or the one that task would get while it does
/// not exist yet (SPEC-097).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OnboardingPlan {
    /// The onboarding task's title.
    pub title: String,
    /// Its steps' titles, in order.
    pub steps: Vec<String>,
    /// The task's number once it exists.
    pub task_number: Option<u32>,
}

/// Live counters for a sandbox: three of the web UI sidebar's groups, by its rule (SPEC-001
/// UI-050; `yulius_domain::task::AttentionFacts::group`). `running` plus `queued` is the sidebar's
/// Working group.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct SandboxCounts {
    /// Working tasks: running (working, kept going by background shells or in review), starting,
    /// continuing on another subscription or paused for capacity.
    pub running: u32,
    /// Tasks needing a person (the "Needs you" group).
    pub needs_attention: u32,
    /// Tasks in line for a slot or chained after another task.
    pub queued: u32,
}

/// Delivery profile (SPEC-001 DLV-001).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Profile {
    /// `standard`, `product`, `studio` or `custom`.
    pub preset: String,
    /// Branch feature PRs target.
    pub integration_branch: String,
    /// Production branch.
    pub production_branch: String,
    /// `squash`, `merge` or `rebase`.
    pub merge: String,
    /// `merge_commit` or `fast_forward`.
    pub promote: String,
    /// Promote automatically after an integration merge (Studio).
    pub auto_promote: bool,
    /// Merges wait for green checks. Absent or null in a request means the preset's default
    /// (`false` for `studio`, otherwise `true`); responses always carry a value.
    #[serde(default)]
    pub require_checks: Option<bool>,
    /// Deployment/verification environments.
    pub environments: Vec<Environment>,
    /// The CI gate Yulius runs on every pull request head and on the integration and
    /// production heads (SPEC-001 DLV-006): shell text set by a repository administrator.
    /// Absent or null: the `[commands] gate` of `yulius.toml` on the integration branch, if any.
    #[serde(default)]
    pub gate: Option<String>,
}

/// One environment step of a profile.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Environment {
    /// Name (e.g. `dev-vm`).
    pub name: String,
    /// `integration` or `production`: which stage it follows.
    pub after: String,
    /// Command run on explicit confirmation, if any: repository-administrator-configured shell
    /// text executed as `/bin/sh -c <command>` in the sandbox clone (docs/api.md trust contract).
    pub command: Option<String>,
    /// `manual` or `command`.
    pub verify: String,
}

/// `POST /api/sandboxes`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum CreateSandbox {
    /// Link an existing repository of the instance owner.
    Import {
        /// `owner/name`.
        repo: String,
    },
    /// Create a new repository from a template.
    Create {
        /// Repository name.
        name: String,
        /// Template id.
        template: String,
        /// Repository description.
        #[serde(default)]
        description: String,
    },
    /// Start a playground from a template (SPEC-099): no GitHub, its Git stays on this server.
    Playground {
        /// Name (its slug is derived from it).
        name: String,
        /// Template id.
        template: String,
    },
}

/// `PATCH /api/sandboxes/{sid}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpdateSandbox {
    /// New delivery profile.
    pub profile: Option<Profile>,
}

/// `GET /api/github/repositories` item.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AvailableRepo {
    /// Owner login.
    pub owner: String,
    /// Repository name.
    pub name: String,
    /// Private repository.
    pub private: bool,
    /// Caller's access level.
    pub access: String,
    /// Already linked to a sandbox.
    pub already_sandbox: bool,
}

/// `GET /api/templates` item.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Template {
    /// Template id.
    pub id: String,
    /// Name.
    pub name: String,
    /// Description.
    pub description: String,
}
