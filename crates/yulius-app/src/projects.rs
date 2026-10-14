//! Sandbox (project) creation, import and removal, plus the repository and template listings
//! (SPEC-001 §5.2 PRJ-001..010, J2, J4, IMP-002; SPEC-002 §4, §8a).
//!
//! Import links an existing repository of the instance owner: the caller needs `admin` or
//! `maintain` on GitHub, the repository is cloned with an installation token into
//! `<data_dir>/sandboxes/<id>/repo`, a delivery profile is chosen (the repository's
//! `yulius.toml` `[delivery]`, else a preset for the known Quartio repositories, else
//! `standard`) and the documentation format is validated; a failing project gets the onboarding
//! meta-task (see `onboarding`). Create makes a new private repository, pushes the scaffold of a
//! template to `main` and `dev`, protects both with rulesets and clones it. Removal stops live
//! tasks and keeps the GitHub repository and all files (PRJ-009).

use std::path::{Path, PathBuf};
use std::sync::Arc;

use yulius_contracts::api::sandboxes::{AvailableRepo, CreateSandbox, Profile, Sandbox, Template};
use yulius_contracts::events::UiEvent;
use yulius_domain::delivery::{DeliveryProfile, MergeMethod, Preset, PromoteMethod};
use yulius_domain::ids::SandboxId;
use yulius_domain::permission::{AccessLevel, Action};
use yulius_domain::spec_format::{
    ProjectFiles, TemplateKind, parse_yulius_toml, scaffold_files, validate_project,
};

use crate::auth::AuthContext;
use crate::context::App;
use crate::error::{AppError, AppResult};
use crate::model::{SandboxRecord, Secret, TaskRecord, UserRecord};
use crate::ports::github::{GhRepo, GitHubAppConfig, RepoAccess, RulesetRequest};

/// First port of the first sandbox range (PRJ-010).
pub const PORT_RANGE_START: u16 = 20_000;
/// Ports per sandbox (PRJ-010).
pub const PORT_RANGE_SIZE: u16 = 20;
/// Longest GitHub repository name accepted.
pub const MAX_REPO_NAME_BYTES: usize = 100;
/// Most installation repositories examined by one listing (each needs a permission lookup).
pub const MAX_LISTED_REPOS: usize = 300;
/// Longest generated slug.
const MAX_SLUG_BYTES: usize = 40;
/// Slug candidates tried before giving up (`name`, `name-2`, …).
const MAX_SLUG_ATTEMPTS: u32 = 20;
/// Longest repository description passed to GitHub.
const MAX_DESCRIPTION_CHARS: usize = 350;

/// Contracts view of a domain delivery profile.
#[must_use]
pub fn profile_view(profile: &DeliveryProfile) -> Profile {
    crate::delivery::profile_to_contract(profile)
}

fn has_delivery_section(yulius_toml: &str) -> bool {
    yulius_toml.lines().any(|line| {
        line.trim()
            .strip_prefix("[delivery]")
            .is_some_and(|rest| rest.trim().is_empty() || rest.trim_start().starts_with('#'))
    })
}

/// The preset of a known repository (per the founders' clarifications): Quartio Product uses
/// `product`, Quartio Studio uses `studio`; everything else starts as `standard`.
#[must_use]
pub fn known_preset(owner: &str, name: &str) -> Preset {
    if !owner.eq_ignore_ascii_case("Quartio-Studio") {
        return Preset::Standard;
    }
    if name.eq_ignore_ascii_case("product") {
        Preset::Product
    } else if name.eq_ignore_ascii_case("studio") {
        Preset::Studio
    } else {
        Preset::Standard
    }
}

/// Delivery profile of an imported repository: the `[delivery]` table of a valid `yulius.toml`
/// when present, else [`known_preset`]. The result is stored on the sandbox and editable later.
#[must_use]
pub fn import_profile(owner: &str, name: &str, yulius_toml: Option<&str>) -> DeliveryProfile {
    if let Some(text) = yulius_toml
        && has_delivery_section(text)
        && let Ok(config) = parse_yulius_toml(text)
    {
        return config.delivery;
    }
    DeliveryProfile::for_preset(known_preset(owner, name))
}

/// URL slug for a repository name: lowercase ASCII letters and digits separated by single `-`,
/// at most 40 bytes, `project` when nothing remains.
#[must_use]
pub fn slug_base(name: &str) -> String {
    let mut slug = String::new();
    for c in name.chars() {
        if slug.len() >= MAX_SLUG_BYTES {
            break;
        }
        if c.is_ascii_alphanumeric() {
            slug.push(c.to_ascii_lowercase());
        } else if !slug.is_empty() && !slug.ends_with('-') {
            slug.push('-');
        }
    }
    let slug = slug.trim_end_matches('-');
    if slug.is_empty() {
        "project".to_owned()
    } else {
        slug.to_owned()
    }
}

/// Whether `name` is a repository name GitHub accepts and Yulius can use in paths and URLs:
/// 1–100 ASCII letters, digits, `-`, `_` or `.`, not starting with `.` and not ending in `.git`.
#[must_use]
pub fn valid_repo_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= MAX_REPO_NAME_BYTES
        && !name.starts_with('.')
        && !name.to_ascii_lowercase().ends_with(".git")
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
}

/// Whether `login` is a GitHub owner login (ASCII letters, digits and `-`, 1–39 bytes).
#[must_use]
pub fn valid_owner(login: &str) -> bool {
    !login.is_empty()
        && login.len() <= 39
        && !login.starts_with('-')
        && login
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-')
}

/// HTTPS clone URL of a GitHub repository.
#[must_use]
pub fn clone_url(owner: &str, name: &str) -> String {
    format!("https://github.com/{owner}/{name}.git")
}

/// Branch rulesets protecting a profile's integration and production branches (DSC-007):
/// pull requests required, merge methods per the profile.
#[must_use]
pub fn rulesets_for(profile: &DeliveryProfile) -> Vec<RulesetRequest> {
    let promote = match profile.promote {
        PromoteMethod::MergeCommit => MergeMethod::Merge,
        PromoteMethod::FastForward => profile.merge,
    };
    vec![
        RulesetRequest {
            name: format!("yulius-integration-{}", profile.integration_branch),
            branch: profile.integration_branch.clone(),
            require_pr: true,
            merge_methods: vec![profile.merge.as_str().to_owned()],
        },
        RulesetRequest {
            name: format!("yulius-production-{}", profile.production_branch),
            branch: profile.production_branch.clone(),
            require_pr: true,
            merge_methods: vec![promote.as_str().to_owned()],
        },
    ]
}

/// Runs blocking file-system work off the async executor (RUST-009).
pub(crate) async fn blocking<T, F>(work: F) -> AppResult<T>
where
    T: Send + 'static,
    F: FnOnce() -> AppResult<T> + Send + 'static,
{
    tokio::task::spawn_blocking(work)
        .await
        .map_err(AppError::internal)?
}

/// Removes a directory tree Yulius created, ignoring failures (best-effort cleanup).
pub(crate) async fn remove_tree(path: PathBuf) {
    let _ = blocking(move || {
        if path.exists() {
            std::fs::remove_dir_all(&path).map_err(AppError::internal)?;
        }
        Ok(())
    })
    .await;
}

/// Commit identity for a user: their display name (else their GitHub login) and the GitHub
/// no-reply address that GitHub links to their account, `{id}+{login}@users.noreply.github.com`
/// (F 2026-10-11; `{login}@users.noreply.github.com` while the id is unknown); Yulius's own
/// identity without a linked GitHub account.
pub(crate) fn identity(user: &UserRecord) -> (String, String) {
    match &user.github_login {
        Some(login) => (
            if user.display_name.trim().is_empty() {
                login.clone()
            } else {
                user.display_name.clone()
            },
            match user.github_id {
                Some(id) => format!("{id}+{login}@users.noreply.github.com"),
                None => format!("{login}@users.noreply.github.com"),
            },
        ),
        None => yulius_identity(),
    }
}

/// [`identity`] of an optional user (a task's owner, the acting person): Yulius's own
/// identity without one.
pub(crate) fn commit_identity(user: Option<&UserRecord>) -> (String, String) {
    user.map_or_else(yulius_identity, identity)
}

fn yulius_identity() -> (String, String) {
    (
        "Yulius".to_owned(),
        "yulius@users.noreply.github.com".to_owned(),
    )
}

fn foreign_owner(owner: &str, name: &str, instance_owner: &str) -> AppError {
    AppError::Invalid(format!(
        "{owner}/{name} belongs to {owner}, but this Yulius instance hosts only repositories of \
         {instance_owner}; transfer or fork the repository to {instance_owner} on GitHub first"
    ))
}

impl App {
    /// The instance owner login and whether it is an organization.
    ///
    /// # Errors
    /// `Conflict` before the GitHub App is configured.
    pub(crate) async fn instance_owner(&self) -> AppResult<(String, bool)> {
        let instance = self
            .deps
            .store
            .instance()
            .await?
            .ok_or_else(|| AppError::Conflict("instance not initialized".into()))?;
        let owner = instance.owner_login.ok_or_else(|| {
            AppError::Conflict("the GitHub owner of this instance is not configured yet".into())
        })?;
        Ok((owner, instance.owner_kind.as_deref() == Some("org")))
    }

    /// The GitHub App and its installation on the instance owner; a missing installation id is
    /// looked up and stored.
    ///
    /// # Errors
    /// `Conflict` when the App is not configured or not installed on the owner.
    pub(crate) async fn installation(&self) -> AppResult<(GitHubAppConfig, u64)> {
        let app = self.github_app().await?;
        let store = &self.deps.store;
        let instance = store
            .instance()
            .await?
            .ok_or_else(|| AppError::Conflict("instance not initialized".into()))?;
        if let Some(id) = instance.installation_id {
            return Ok((app, id));
        }
        let owner = instance.owner_login.ok_or_else(|| {
            AppError::Conflict("the GitHub owner of this instance is not configured yet".into())
        })?;
        let id = self
            .deps
            .github
            .find_installation(&app, &owner)
            .await?
            .ok_or_else(|| {
                AppError::Conflict(format!(
                    "the Yulius GitHub App is not installed on {owner}; install it from \
                     Administration → GitHub"
                ))
            })?;
        store.set_installation(Some(id)).await?;
        Ok((app, id))
    }

    /// A short-lived installation token for Git transport and App-level API calls.
    ///
    /// # Errors
    /// Installation lookup or GitHub failures.
    pub(crate) async fn installation_token(&self) -> AppResult<Secret> {
        let (app, id) = self.installation().await?;
        self.deps.github.installation_token(&app, id).await
    }

    /// The repository `owner/name` among those the App installation can access.
    ///
    /// # Errors
    /// Installation lookup or GitHub failures.
    async fn installation_repo(&self, owner: &str, name: &str) -> AppResult<Option<GhRepo>> {
        let (app, installation) = self.installation().await?;
        Ok(self
            .deps
            .github
            .installation_repos(&app, installation)
            .await?
            .into_iter()
            .find(|repo| {
                repo.owner.eq_ignore_ascii_case(owner) && repo.name.eq_ignore_ascii_case(name)
            }))
    }

    /// The caller's GitHub user token; users without a GitHub identity get a clear refusal.
    async fn github_identity_token(&self, user: &UserRecord) -> AppResult<Secret> {
        if user.github_login.is_none() {
            return Err(AppError::Forbidden(
                "sign in with GitHub to work with repositories".into(),
            ));
        }
        self.user_github_token(user).await
    }

    /// Notifies viewers to refetch (`resync`): scoped to a sandbox's viewers, or everyone.
    /// Sandbox views are per-user (access level), so no view is broadcast.
    pub(crate) fn publish_resync(&self, sandbox: Option<SandboxId>) {
        self.events.publish(sandbox, false, UiEvent::Resync);
    }

    /// `GET /api/github/repositories`: repositories of the instance owner that the App
    /// installation can access and the caller can at least read, with the caller's level and
    /// whether each is already a sandbox. The superadmin has admin on each (ACC-012) and needs
    /// no GitHub identity.
    ///
    /// # Errors
    /// `Forbidden` without a GitHub identity, `Conflict` when the App is missing, `Upstream`.
    pub async fn github_repositories(&self, ctx: &AuthContext) -> AppResult<Vec<AvailableRepo>> {
        let token = if ctx.user.is_superadmin {
            None
        } else {
            Some(self.github_identity_token(&ctx.user).await?)
        };
        let (owner, _) = self.instance_owner().await?;
        let (app, installation) = self.installation().await?;
        let mut repos = self
            .deps
            .github
            .installation_repos(&app, installation)
            .await?;
        repos.retain(|repo| repo.owner.eq_ignore_ascii_case(&owner));
        repos.sort_by_key(|repo| repo.name.to_ascii_lowercase());
        repos.truncate(MAX_LISTED_REPOS);
        let mut out = Vec::new();
        for repo in repos {
            let existing = self
                .deps
                .store
                .sandbox_by_repo(&repo.owner, &repo.name)
                .await?;
            let level = match (&existing, &token) {
                (Some(sandbox), _) => self
                    .access_level(&ctx.user, sandbox)
                    .await
                    .unwrap_or(AccessLevel::None),
                (None, None) => AccessLevel::Admin,
                (None, Some(token)) => match self
                    .deps
                    .github
                    .repo_access(token, &repo.owner, &repo.name)
                    .await
                {
                    Ok(RepoAccess::Access { permissions, .. }) => {
                        AccessLevel::from_github(permissions)
                    }
                    Ok(RepoAccess::NoAccess) => AccessLevel::None,
                    Err(err) => {
                        tracing::warn!(error = %err, repo = %repo.name, "permission lookup failed");
                        AccessLevel::None
                    }
                },
            };
            if level == AccessLevel::None {
                continue;
            }
            out.push(AvailableRepo {
                owner: repo.owner,
                name: repo.name,
                private: repo.private,
                access: level.as_str().to_owned(),
                already_sandbox: existing.is_some(),
            });
        }
        Ok(out)
    }

    /// `GET /api/templates` (PRJ-008).
    #[must_use]
    pub fn templates(&self) -> Vec<Template> {
        TemplateKind::ALL
            .iter()
            .map(|kind| Template {
                id: kind.id().to_owned(),
                name: kind.name().to_owned(),
                description: kind.description().to_owned(),
            })
            .collect()
    }

    /// `POST /api/sandboxes`: import an existing repository or create one from a template.
    ///
    /// # Errors
    /// `Invalid` input or foreign repositories, `Forbidden` without the needed permission,
    /// `Conflict` for repositories that are already sandboxes, `Upstream` GitHub/Git failures.
    pub async fn create_sandbox(
        self: &Arc<Self>,
        ctx: &AuthContext,
        req: &CreateSandbox,
    ) -> AppResult<Sandbox> {
        let record = match req {
            CreateSandbox::Import { repo } => {
                let (owner, name) = repo
                    .trim()
                    .split_once('/')
                    .ok_or_else(|| AppError::Invalid("repo must be owner/name".into()))?;
                // Adding it in the browser is the explicit action that starts onboarding; a token
                // import (the CLI, a script) waits for Start onboarding (SPEC-051).
                self.import_repository(&ctx.user, owner.trim(), name.trim(), ctx.is_browser())
                    .await?
            }
            CreateSandbox::Create {
                name,
                template,
                description,
            } => {
                self.create_repository(ctx, name.trim(), template, description)
                    .await?
            }
            CreateSandbox::Playground { name, template } => {
                self.playground_from_template(&ctx.user, name, template)
                    .await?
            }
        };
        let level = self
            .access_level(&ctx.user, &record)
            .await
            .unwrap_or(AccessLevel::None);
        self.sandbox_view(&record, level).await
    }

    /// Links an existing repository of the instance owner as a new sandbox (import mode,
    /// IMP-002): requires `admin` or `maintain` on GitHub (the superadmin: a repository the App
    /// installation can access, ACC-012); clones it, chooses the profile, validates the
    /// documentation format and creates the onboarding task when it fails. With
    /// `start_preparation` that task starts at once; through `yulius onboard` it waits for a
    /// person to click Start onboarding on the sandbox page (SPEC-051).
    ///
    /// # Errors
    /// See [`App::create_sandbox`].
    pub(crate) async fn import_repository(
        self: &Arc<Self>,
        user: &UserRecord,
        owner: &str,
        name: &str,
        start_preparation: bool,
    ) -> AppResult<SandboxRecord> {
        let store = &self.deps.store;
        if let Some(existing) = store.sandbox_by_repo(owner, name).await? {
            return Err(AppError::Conflict(format!(
                "{owner}/{name} is already the sandbox /s/{}",
                existing.slug
            )));
        }
        let repo = self.import_permission(user, owner, name).await?;
        let record = self.link_repository(user, owner, name, repo).await?;
        if start_preparation && record.onboarding_task_id.is_some() {
            let actor = self.login_of(&user.id).await;
            if let Err(err) = self.start_onboarding(&record.id, &actor).await {
                tracing::warn!(sandbox = %record.id, error = %err, "the preparation task did not start");
            }
            return self
                .deps
                .store
                .sandbox(&record.id)
                .await?
                .ok_or(AppError::NotFound);
        }
        Ok(record)
    }

    /// Whether `user` may link `owner/name` as a new sandbox (IMP-002): a repository of the
    /// instance owner on which they have `admin` or `maintain` (the superadmin: one the App
    /// installation can access). Returns the repository.
    ///
    /// # Errors
    /// `Invalid` for foreign or unknown repositories, `Forbidden` without the permission.
    pub(crate) async fn import_permission(
        &self,
        user: &UserRecord,
        owner: &str,
        name: &str,
    ) -> AppResult<GhRepo> {
        if !valid_owner(owner) || !valid_repo_name(name) {
            return Err(AppError::Invalid(format!(
                "{owner}/{name} is not a GitHub repository name"
            )));
        }
        let (instance_owner, _) = self.instance_owner().await?;
        if !owner.eq_ignore_ascii_case(&instance_owner) {
            return Err(foreign_owner(owner, name, &instance_owner));
        }
        let repo = if user.is_superadmin {
            self.installation_repo(owner, name).await?.ok_or_else(|| {
                AppError::Invalid(format!(
                    "{owner}/{name} does not exist on GitHub or the Yulius GitHub App cannot \
                     access it; every project needs a GitHub repository of {instance_owner}"
                ))
            })?
        } else {
            let token = self.github_identity_token(user).await?;
            match self.deps.github.repo_access(&token, owner, name).await? {
                RepoAccess::NoAccess => {
                    return Err(AppError::Invalid(format!(
                        "{owner}/{name} does not exist on GitHub or you cannot see it; every \
                         project needs a GitHub repository of {instance_owner}"
                    )));
                }
                RepoAccess::Access { permissions, repo } => {
                    if !(permissions.admin || permissions.maintain) {
                        return Err(AppError::Forbidden(format!(
                            "adding {owner}/{name} needs admin or maintain permission on GitHub; you have {}",
                            AccessLevel::from_github(permissions).as_str()
                        )));
                    }
                    repo
                }
            }
        };
        let owner = if repo.owner.is_empty() {
            owner.to_owned()
        } else {
            repo.owner.clone()
        };
        let name = if repo.name.is_empty() {
            name.to_owned()
        } else {
            repo.name.clone()
        };
        if !owner.eq_ignore_ascii_case(&instance_owner) {
            return Err(foreign_owner(&owner, &name, &instance_owner));
        }
        Ok(repo)
    }

    /// Clones a permitted repository and records it as a sandbox (see
    /// [`App::import_repository`]).
    async fn link_repository(
        self: &Arc<Self>,
        user: &UserRecord,
        owner: &str,
        name: &str,
        repo: GhRepo,
    ) -> AppResult<SandboxRecord> {
        let store = &self.deps.store;
        let owner = if repo.owner.is_empty() {
            owner.to_owned()
        } else {
            repo.owner.clone()
        };
        let name = if repo.name.is_empty() {
            name.to_owned()
        } else {
            repo.name.clone()
        };
        let install_token = self.installation_token().await?;
        let id = SandboxId::parse(&self.deps.crypto.new_id()).map_err(AppError::internal)?;
        let dir = self.sandbox_dir(&id);
        let repo_dir = self.sandbox_repo(&id);
        let prepared = self
            .clone_and_inspect(&owner, &name, &repo_dir, &install_token)
            .await;
        let (default_branch, report, yulius_toml) = match prepared {
            Ok(found) => found,
            Err(err) => {
                remove_tree(dir).await;
                return Err(err);
            }
        };
        let default_branch = if default_branch.is_empty() {
            repo.default_branch.clone()
        } else {
            default_branch
        };
        let profile = import_profile(&owner, &name, yulius_toml.as_deref());
        // Every imported repository gets an onboarding task: the full preparation when its docs
        // do not follow the format yet, the development environment alone when they do
        // (F 2026-10-11: "on onboarding every environmental things should be installed").
        let onboarding = "pending";
        let record = SandboxRecord {
            id: id.clone(),
            slug: String::new(),
            repo_owner: owner.clone(),
            repo_name: name.clone(),
            github_repo_id: (repo.id > 0).then_some(repo.id),
            default_branch,
            profile_json: serde_json::to_string(&profile_view(&profile))
                .map_err(AppError::internal)?,
            port_base: 0,
            port_count: PORT_RANGE_SIZE,
            onboarding_state: onboarding.to_owned(),
            onboarding_task_id: None,
            created_by: user.id.clone(),
            created_at: self.now(),
            removed_at: None,
        };
        let mut record = match self.insert_sandbox_unique(record).await {
            Ok(record) => record,
            Err(err) => {
                remove_tree(dir).await;
                return Err(err);
            }
        };
        {
            // The sandbox exists now; a failed meta-task creation leaves it pending (an admin
            // can skip) instead of reporting the whole import as failed.
            match self.create_onboarding_task(&record, user, &report).await {
                Ok(task) => {
                    store
                        .set_onboarding(&record.id, "pending", Some(&task.id))
                        .await?;
                    record.onboarding_task_id = Some(task.id);
                }
                Err(err) => {
                    tracing::warn!(error = %err, sandbox = %record.id, "could not create the preparation task");
                }
            }
        }
        store
            .audit(
                self.now(),
                &self.login_of(&user.id).await,
                "sandbox.imported",
                Some(record.id.as_str()),
                Some(&format!("{owner}/{name} onboarding={onboarding}")),
            )
            .await?;
        self.publish_resync(Some(record.id.clone()));
        Ok(record)
    }

    /// Clones a repository and reads what import needs: default branch, documentation report
    /// and `yulius.toml` text.
    async fn clone_and_inspect(
        &self,
        owner: &str,
        name: &str,
        repo_dir: &Path,
        token: &Secret,
    ) -> AppResult<(
        String,
        yulius_domain::spec_format::ProjectReport,
        Option<String>,
    )> {
        if let Some(parent) = repo_dir.parent() {
            let parent = parent.to_path_buf();
            blocking(move || std::fs::create_dir_all(&parent).map_err(AppError::internal)).await?;
        }
        self.deps
            .git
            .sync_clone(&clone_url(owner, name), repo_dir, token)
            .await?;
        let default_branch = match self.deps.git.default_branch(repo_dir).await {
            Ok(branch) => branch,
            Err(err) => {
                tracing::warn!(error = %err, "default branch detection failed");
                String::new()
            }
        };
        let root = repo_dir.to_path_buf();
        let (report, yulius_toml) =
            blocking(move || Ok(crate::onboarding::validate_checkout(&root))).await?;
        Ok((default_branch, report, yulius_toml))
    }

    /// Inserts a new sandbox, allocating a unique slug and the next free port range; retries
    /// with `name-2`, `name-3`, … on conflicts (slugs stay reserved by removed sandboxes, except
    /// for the same repository added again: it gets its name back and the removed sandbox's slug
    /// becomes `name-removed-<ms>`, F 2026-10-11).
    pub(crate) async fn insert_sandbox_unique(
        &self,
        mut record: SandboxRecord,
    ) -> AppResult<SandboxRecord> {
        let store = &self.deps.store;
        let base = slug_base(&record.repo_name);
        store
            .release_removed_slug(&base, &record.repo_owner, &record.repo_name)
            .await?;
        for attempt in 0..MAX_SLUG_ATTEMPTS {
            let slug = if attempt == 0 {
                base.clone()
            } else {
                format!("{base}-{}", attempt + 1)
            };
            if store.sandbox_by_slug(&slug).await?.is_some() {
                continue;
            }
            // A playground's repository name is its slug, unique like it (SPEC-099).
            if record.is_playground() {
                record.repo_name.clone_from(&slug);
            }
            record.slug = slug;
            record.port_base = store
                .next_port_base(PORT_RANGE_START, PORT_RANGE_SIZE)
                .await?;
            match store.insert_sandbox(&record).await {
                Ok(()) => return Ok(record),
                Err(AppError::Conflict(_)) => {
                    if let Some(existing) = store
                        .sandbox_by_repo(&record.repo_owner, &record.repo_name)
                        .await?
                    {
                        return Err(AppError::Conflict(format!(
                            "{}/{} is already the sandbox /s/{}",
                            record.repo_owner, record.repo_name, existing.slug
                        )));
                    }
                }
                Err(err) => return Err(err),
            }
        }
        Err(AppError::Conflict(
            "could not allocate a unique sandbox name; remove unused sandboxes or rename the repository".into(),
        ))
    }

    /// Creates a repository from a template (PRJ-003/004/005/008, J2): GitHub repository,
    /// scaffold pushed to the production and integration branches, rulesets, clone.
    async fn create_repository(
        self: &Arc<Self>,
        ctx: &AuthContext,
        name: &str,
        template: &str,
        description: &str,
    ) -> AppResult<SandboxRecord> {
        self.require_instance_admin(ctx)?;
        let template = TemplateKind::parse(template).ok_or_else(|| {
            AppError::Invalid(format!(
                "unknown template {template}; choose one of {}",
                TemplateKind::ALL
                    .iter()
                    .map(|kind| kind.id())
                    .collect::<Vec<_>>()
                    .join(", ")
            ))
        })?;
        if !valid_repo_name(name) {
            return Err(AppError::Invalid(
                "the repository name must be 1-100 letters, digits, '-', '_' or '.'".into(),
            ));
        }
        let description: String = description
            .chars()
            .filter(|c| !c.is_control())
            .take(MAX_DESCRIPTION_CHARS)
            .collect();
        let (owner, is_org) = self.instance_owner().await?;
        let store = &self.deps.store;
        if let Some(existing) = store.sandbox_by_repo(&owner, name).await? {
            return Err(AppError::Conflict(format!(
                "{owner}/{name} is already the sandbox /s/{}",
                existing.slug
            )));
        }
        let files = scaffold_files(name, template);
        let report = validate_project(&ProjectFiles::from_files(&files));
        if !report.is_valid() {
            return Err(AppError::Internal(format!(
                "template {} produced an invalid scaffold",
                template.id()
            )));
        }
        let install_token = self.installation_token().await?;
        let admin_token = if is_org {
            install_token.clone()
        } else {
            let owner_user = store.user_by_login(&owner).await?.ok_or_else(|| {
                AppError::Conflict(format!(
                    "{owner} must sign in to Yulius with GitHub once before projects can be created in their account"
                ))
            })?;
            self.user_github_token(&owner_user).await?
        };
        let repo = self
            .deps
            .github
            .create_repo(&admin_token, &owner, is_org, name, &description)
            .await?;
        let repo_owner = if repo.owner.is_empty() {
            owner.clone()
        } else {
            repo.owner.clone()
        };
        let repo_name = if repo.name.is_empty() {
            name.to_owned()
        } else {
            repo.name.clone()
        };
        let profile = DeliveryProfile::standard();
        let id = SandboxId::parse(&self.deps.crypto.new_id()).map_err(AppError::internal)?;
        let dir = self.sandbox_dir(&id);
        let pushed = self
            .push_scaffold(
                &id,
                &Scaffold {
                    url: &clone_url(&repo_owner, &repo_name),
                    name: &repo_name,
                    files: &files,
                    template,
                    profile: &profile,
                    user: &ctx.user,
                },
                &install_token,
            )
            .await;
        if let Err(err) = pushed {
            remove_tree(dir).await;
            return Err(err);
        }
        // Rulesets on private repositories need a paid GitHub plan. They are a second line of
        // defence only: Yulius's own merge gate enforces the delivery rules (SPEC-002 ADR-21),
        // so a refusal leaves the project usable.
        for ruleset in rulesets_for(&profile) {
            if let Err(err) = self
                .deps
                .github
                .create_ruleset(&admin_token, &repo_owner, &repo_name, &ruleset)
                .await
            {
                tracing::warn!(repo = %format!("{repo_owner}/{repo_name}"), ruleset = %ruleset.name, error = %err, "GitHub refused a branch ruleset; Yulius's merge gate enforces the rules");
            }
        }
        let repo_dir = self.sandbox_repo(&id);
        if let Err(err) = self
            .deps
            .git
            .sync_clone(
                &clone_url(&repo_owner, &repo_name),
                &repo_dir,
                &install_token,
            )
            .await
        {
            remove_tree(dir).await;
            return Err(err);
        }
        let record = SandboxRecord {
            id: id.clone(),
            slug: String::new(),
            repo_owner: repo_owner.clone(),
            repo_name: repo_name.clone(),
            github_repo_id: (repo.id > 0).then_some(repo.id),
            default_branch: profile.production_branch.clone(),
            profile_json: serde_json::to_string(&profile_view(&profile))
                .map_err(AppError::internal)?,
            port_base: 0,
            port_count: PORT_RANGE_SIZE,
            onboarding_state: "not_needed".into(),
            onboarding_task_id: None,
            created_by: ctx.user.id.clone(),
            created_at: self.now(),
            removed_at: None,
        };
        let record = match self.insert_sandbox_unique(record).await {
            Ok(record) => record,
            Err(err) => {
                remove_tree(dir).await;
                return Err(err);
            }
        };
        store
            .audit(
                self.now(),
                &self.login_of(&ctx.user.id).await,
                "sandbox.created",
                Some(record.id.as_str()),
                Some(&format!(
                    "{repo_owner}/{repo_name} template={}",
                    template.id()
                )),
            )
            .await?;
        self.publish_resync(Some(record.id.clone()));
        Ok(record)
    }

    /// Clones the new (empty) repository into a scratch directory, commits the scaffold on the
    /// production branch, pushes it there and to the integration branch, then deletes the
    /// scratch clone.
    pub(crate) async fn push_scaffold(
        &self,
        id: &SandboxId,
        scaffold: &Scaffold<'_>,
        token: &Secret,
    ) -> AppResult<()> {
        let Scaffold {
            url,
            name,
            files,
            template,
            profile,
            user,
        } = *scaffold;
        let scratch = self.sandbox_dir(id).join("scaffold");
        if let Some(parent) = scratch.parent() {
            let parent = parent.to_path_buf();
            blocking(move || std::fs::create_dir_all(&parent).map_err(AppError::internal)).await?;
        }
        self.deps.git.sync_clone(url, &scratch, token).await?;
        let production = profile.production_branch.clone();
        let files = files.to_vec();
        let root = scratch.clone();
        blocking(move || write_scaffold(&root, &production, &files)).await?;
        let (author, email) = identity(user);
        let committed = self
            .deps
            .git
            .commit_all(
                &scratch,
                &format!(
                    "chore: scaffold {name} from the {} template\n\nCreated by Yulius with the enforced documentation format.",
                    template.id()
                ),
                &author,
                &email,
            )
            .await?;
        if !committed {
            return Err(AppError::Internal("the scaffold produced no commit".into()));
        }
        self.deps
            .git
            .push(
                &scratch,
                &profile.production_branch,
                &profile.production_branch,
                token,
            )
            .await?;
        self.deps
            .git
            .push(
                &scratch,
                &profile.production_branch,
                &profile.integration_branch,
                token,
            )
            .await?;
        remove_tree(scratch).await;
        Ok(())
    }

    /// `DELETE /api/sandboxes/{sid}` (admin, PRJ-009): stops live and queued tasks and marks
    /// the sandbox removed. The GitHub repository, the clone, worktrees, transcripts and task
    /// records are kept.
    ///
    /// # Errors
    /// `NotFound` without access, `Forbidden` without admin.
    pub async fn delete_sandbox(self: &Arc<Self>, ctx: &AuthContext, sid: &str) -> AppResult<()> {
        let id = SandboxId::parse(sid).map_err(|_| AppError::NotFound)?;
        let (record, _) = self.require(&ctx.user, &id, Action::Administer).await?;
        let store = &self.deps.store;
        let mut tasks = store.tasks(&id, false).await?;
        tasks.extend(store.tasks(&id, true).await?);
        let mut stopped = 0_u32;
        for task in tasks.iter().filter(|t| !t.status.is_resting()) {
            match self.stop_task_now(&task.id).await {
                Ok(()) => stopped += 1,
                Err(AppError::Conflict(_)) => {}
                Err(err) => {
                    tracing::warn!(error = %err, task = %task.id, "could not stop task of removed sandbox");
                }
            }
        }
        let export = self.export_sandbox_tasks(&record, &tasks).await?;
        store.remove_sandbox(&id, self.now()).await?;
        store
            .audit(
                self.now(),
                &self.login_of(&ctx.user.id).await,
                "sandbox.removed",
                Some(id.as_str()),
                Some(&format!(
                    "{}/{} stopped={stopped}; GitHub repository and files kept; tasks exported to {}",
                    record.repo_owner,
                    record.repo_name,
                    export.display()
                )),
            )
            .await?;
        self.publish_resync(None);
        Ok(())
    }
}

impl App {
    /// Writes `<data>/exports/sandbox-<slug>-<ms>.json` (0600) with every task of a sandbox, its
    /// attempts, steps and messages, before the sandbox is removed (PRJ-009).
    async fn export_sandbox_tasks(
        &self,
        sandbox: &SandboxRecord,
        tasks: &[TaskRecord],
    ) -> AppResult<std::path::PathBuf> {
        let store = &self.deps.store;
        let mut rows = Vec::with_capacity(tasks.len());
        for task in tasks {
            let attempts: Vec<serde_json::Value> = store
                .attempts(&task.id)
                .await?
                .into_iter()
                .map(|a| {
                    serde_json::json!({
                        "seq": a.seq,
                        "harness": a.harness.as_str(),
                        "account_id": a.account_id.map(|id| id.as_str().to_owned()),
                        "provider_session_id": a.provider_session_id,
                        "launch_kind": a.launch_kind.as_str(),
                        "state": format!("{:?}", a.state),
                        "started_at_ms": a.started_at.0,
                        "ended_at_ms": a.ended_at.map(|t| t.0),
                        "exit_code": a.exit_code,
                        "exit_reason": a.exit_reason,
                    })
                })
                .collect();
            let steps: Vec<serde_json::Value> = store
                .steps(&task.id)
                .await?
                .into_iter()
                .map(|s| serde_json::json!({"ordinal": s.ordinal, "title": s.title, "status": s.status, "note": s.note}))
                .collect();
            let messages: Vec<serde_json::Value> = store
                .task_messages(&task.id, 1_000)
                .await?
                .into_iter()
                .map(|e| {
                    let m = e.message;
                    serde_json::json!({
                        "from_task": m.from_task.map(|t| t.as_str().to_owned()),
                        "from_user": m.from_user.map(|u| u.as_str().to_owned()),
                        "to_task": m.to_task.map(|t| t.as_str().to_owned()),
                        "kind": m.kind, "subject": m.subject, "body": m.body,
                        "created_at_ms": m.created_at.0,
                    })
                })
                .collect();
            rows.push(serde_json::json!({
                "id": task.id.as_str(),
                "number": task.number,
                "title": task.title,
                "prompt": task.prompt,
                "harness": task.harness.as_str(),
                "status": task.status.as_str(),
                "branch": task.branch,
                "spec_path": task.spec_path,
                "origin": task.origin.as_str(),
                "created_by": task.created_by.as_str(),
                "created_at_ms": task.created_at.0,
                "closed_at_ms": task.closed_at.map(|t| t.0),
                "archived_at_ms": task.archived_at.map(|t| t.0),
                "attempts": attempts,
                "steps": steps,
                "messages": messages,
            }));
        }
        let now = self.now();
        let doc = serde_json::json!({
            "sandbox": {
                "id": sandbox.id.as_str(),
                "slug": sandbox.slug,
                "repository": format!("{}/{}", sandbox.repo_owner, sandbox.repo_name),
            },
            "exported_at_ms": now.0,
            "tasks": rows,
        });
        let bytes = serde_json::to_vec_pretty(&doc).map_err(AppError::internal)?;
        let dir = self.config.data_dir.join("exports");
        let path = dir.join(format!("sandbox-{}-{}.json", sandbox.slug, now.0));
        let target = path.clone();
        tokio::task::spawn_blocking(move || -> std::io::Result<()> {
            use std::io::Write;
            use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
            std::fs::DirBuilder::new()
                .recursive(true)
                .mode(0o700)
                .create(&dir)?;
            let mut file = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&target)?;
            file.write_all(&bytes)?;
            file.sync_all()
        })
        .await
        .map_err(AppError::internal)?
        .map_err(AppError::internal)?;
        Ok(path)
    }
}

/// What to push as the first commit of a new repository.
#[derive(Clone, Copy)]
pub(crate) struct Scaffold<'a> {
    /// Where the scaffold is pushed: the new GitHub repository, or a playground's `origin`.
    pub(crate) url: &'a str,
    pub(crate) name: &'a str,
    pub(crate) files: &'a [(String, String)],
    pub(crate) template: TemplateKind,
    pub(crate) profile: &'a DeliveryProfile,
    pub(crate) user: &'a UserRecord,
}

/// Points the unborn `HEAD` of a fresh clone at `branch` and writes the scaffold files.
pub(crate) fn write_scaffold(
    root: &Path,
    branch: &str,
    files: &[(String, String)],
) -> AppResult<()> {
    let git_dir = root.join(".git");
    if !git_dir.is_dir() {
        return Err(AppError::Internal(
            "the scaffold clone has no .git directory".into(),
        ));
    }
    std::fs::write(git_dir.join("HEAD"), format!("ref: refs/heads/{branch}\n"))
        .map_err(AppError::internal)?;
    for (path, contents) in files {
        let target = root.join(path);
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent).map_err(AppError::internal)?;
        }
        std::fs::write(&target, contents).map_err(AppError::internal)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugs_and_names() {
        assert_eq!(slug_base("Online Shop.v2"), "online-shop-v2");
        assert_eq!(slug_base("---"), "project");
        assert_eq!(slug_base(&"a".repeat(60)).len(), MAX_SLUG_BYTES);
        assert!(valid_repo_name("shop_v2.web"));
        for bad in ["", ".hidden", "a b", "x.git", "a/b", "ü"] {
            assert!(!valid_repo_name(bad), "{bad:?}");
        }
        assert!(!valid_repo_name(&"a".repeat(101)));
        assert!(valid_owner("Quartio-Studio"));
        assert!(!valid_owner("-x") && !valid_owner("a_b") && !valid_owner(""));
    }

    #[test]
    fn profiles_follow_yulius_toml_then_known_repositories() {
        assert_eq!(
            import_profile("Quartio-Studio", "product", None).preset,
            Preset::Product
        );
        assert_eq!(
            import_profile("quartio-studio", "Studio", None).preset,
            Preset::Studio
        );
        assert_eq!(
            import_profile("Quartio-Studio", "other", None).preset,
            Preset::Standard
        );
        assert_eq!(
            import_profile("someone", "product", None).preset,
            Preset::Standard
        );
        let toml = "[delivery]\npreset = \"studio\"\n";
        assert_eq!(
            import_profile("Quartio-Studio", "x", Some(toml)).preset,
            Preset::Studio
        );
        // A yulius.toml without [delivery] (or an invalid one) falls back to the heuristic.
        let docs_only = "[docs]\nproduct = \"README.md\"\n";
        assert_eq!(
            import_profile("Quartio-Studio", "product", Some(docs_only)).preset,
            Preset::Product
        );
        assert_eq!(
            import_profile(
                "Quartio-Studio",
                "product",
                Some("[delivery]\npreset = 7\n")
            )
            .preset,
            Preset::Product
        );
        let view = profile_view(&DeliveryProfile::product());
        assert_eq!(view.integration_branch, "quartio");
        assert_eq!(view.promote, "fast_forward");
        assert_eq!(view.environments.len(), 2);
    }

    #[test]
    fn rulesets_protect_both_branches_with_pull_requests() {
        let rules = rulesets_for(&DeliveryProfile::standard());
        assert_eq!(rules.len(), 2);
        assert_eq!(rules[0].branch, "dev");
        assert_eq!(rules[0].merge_methods, vec!["squash".to_owned()]);
        assert_eq!(rules[1].branch, "main");
        assert_eq!(rules[1].merge_methods, vec!["merge".to_owned()]);
        assert!(rules.iter().all(|r| r.require_pr));
    }

    /// F 2026-10-11: commits carry the address GitHub links to the account (`{id}+{login}`).
    #[test]
    fn commit_identity_is_the_address_github_links_to_the_account()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut user = UserRecord {
            id: yulius_domain::ids::UserId::parse("u-alice")?,
            github_login: Some("alice".into()),
            github_id: Some(4242),
            display_name: "Alice Example".into(),
            avatar_url: None,
            is_superadmin: false,
            is_instance_admin: false,
            password_hash: None,
            created_at: yulius_domain::time::Timestamp(1),
            disabled_at: None,
        };
        assert_eq!(
            commit_identity(Some(&user)),
            (
                "Alice Example".to_owned(),
                "4242+alice@users.noreply.github.com".to_owned()
            )
        );
        user.github_id = None;
        user.display_name = " ".into();
        assert_eq!(
            identity(&user),
            (
                "alice".to_owned(),
                "alice@users.noreply.github.com".to_owned()
            )
        );
        user.github_login = None;
        user.github_id = Some(4242);
        let yulius = (
            "Yulius".to_owned(),
            "yulius@users.noreply.github.com".to_owned(),
        );
        assert_eq!(identity(&user), yulius);
        assert_eq!(commit_identity(None), yulius);
        Ok(())
    }
}
