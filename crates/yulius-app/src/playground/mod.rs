//! Playgrounds (SPEC-099, ADR-0081): a team tries Yulius on its own code before it connects
//! GitHub. A playground is a sandbox whose `origin` is a bare repository on the server outside
//! the sandbox (`<data>/playgrounds/<id>.git`), so the clone, worktrees and merges work as with
//! GitHub while the code never leaves the server, and the sandbox never reaches a repository
//! whose hooks the server's Git runs. The record marks it with an empty repository owner
//! ([`SandboxRecord::is_playground`]); it is its maker's (and the superadmin's), its onboarding is
//! `not_needed`, and Yulius never calls GitHub for it.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use yulius_domain::delivery::DeliveryProfile;
use yulius_domain::ids::SandboxId;
use yulius_domain::spec_format::{TemplateKind, scaffold_files};

use crate::context::App;
use crate::error::{AppError, AppResult};
use crate::model::{SandboxRecord, Secret, UserRecord};
use crate::projects::{PORT_RANGE_SIZE, Scaffold, profile_view, remove_tree, slug_base};

mod folder;

pub use folder::{FolderUpload, MAX_FOLDER_UPLOAD_BYTES};

/// The note a playground's delivery views carry instead of a GitHub error (PLY-008).
pub const PLAYGROUND_NOTE: &str = "This playground keeps its Git on this server: no pull \
     requests; `delivery_merge` squashes a task into the integration branch here once Yulius CI \
     passed on it.";

/// Refuses `what` in a playground (PLY-002): it needs GitHub, which a playground never calls.
///
/// # Errors
/// `Conflict` for a playground.
pub(crate) fn refuse_in_playground(sandbox: &SandboxRecord, what: &str) -> AppResult<()> {
    if sandbox.is_playground() {
        return Err(AppError::Conflict(format!(
            "{what} needs GitHub, and this sandbox is a playground: its Git stays on this server, \
             with no pull request, deploy, promotion or GitHub issue"
        )));
    }
    Ok(())
}

/// Playgrounds a person (not the superadmin) keeps at once: each holds a repository on the
/// server's disk.
pub const MAX_PLAYGROUNDS_PER_PERSON: usize = 10;

/// Git transport needs no credential for a playground's own `origin`; nothing is sent anywhere.
pub(crate) fn no_token() -> Secret {
    Secret(String::new())
}

/// A playground's name: its slug base (lowercase letters, digits and `-`), from what the person
/// typed or the uploaded folder's name.
///
/// # Errors
/// `Invalid` when nothing is typed.
pub fn playground_name(raw: &str) -> AppResult<String> {
    if raw.trim().is_empty() {
        return Err(AppError::Invalid("a playground needs a name".into()));
    }
    Ok(slug_base(raw.trim()))
}

impl App {
    /// The bare repository that is a playground's `origin`: `<data>/playgrounds/<id>.git`,
    /// outside the sandbox, which its processes (any signed-in maker's) may write. The server's
    /// Git pushes there, and a local push runs the receiving repository's own hooks and
    /// configuration outside containment, so the sandbox must never reach them (ADR-0081).
    pub(crate) fn playground_origin(&self, sandbox: &SandboxId) -> PathBuf {
        let dir = self
            .config
            .data_dir
            .join("playgrounds")
            .join(format!("{}.git", sandbox.as_str()));
        std::path::absolute(&dir).unwrap_or(dir)
    }

    /// Removes what a playground that failed to start left on disk: its sandbox and `origin`.
    pub(crate) async fn discard_playground(&self, sandbox: &SandboxId) {
        remove_tree(self.sandbox_dir(sandbox)).await;
        remove_tree(self.playground_origin(sandbox)).await;
    }

    /// `origin`'s URL for `sandbox`'s clone: the playground's own bare repository, else GitHub.
    pub(crate) fn origin_url(&self, sandbox: &SandboxRecord) -> String {
        if sandbox.is_playground() {
            self.playground_origin(&sandbox.id)
                .to_string_lossy()
                .into_owned()
        } else {
            crate::projects::clone_url(&sandbox.repo_owner, &sandbox.repo_name)
        }
    }

    /// Refuses a new playground to a person who keeps [`MAX_PLAYGROUNDS_PER_PERSON`].
    ///
    /// # Errors
    /// `Conflict` at the bound; store failures.
    pub(crate) async fn playground_quota(&self, user: &UserRecord) -> AppResult<()> {
        if user.is_superadmin {
            return Ok(());
        }
        // Every repository on disk counts, a removed playground's too: removal keeps its files
        // (PRJ-009), so removing cannot free room for more.
        let dir = self.config.data_dir.join("playgrounds");
        let ids: Vec<SandboxId> = match std::fs::read_dir(&dir) {
            Ok(entries) => entries
                .filter_map(Result::ok)
                .filter_map(|e| {
                    let name = e.file_name().to_string_lossy().into_owned();
                    SandboxId::parse(name.strip_suffix(".git")?).ok()
                })
                .collect(),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(err) => return Err(AppError::internal(err)),
        };
        let mut kept = 0;
        for id in ids {
            if self
                .deps
                .store
                .sandbox(&id)
                .await?
                .is_some_and(|s| s.is_playground() && s.created_by == user.id)
            {
                kept += 1;
            }
        }
        if kept >= MAX_PLAYGROUNDS_PER_PERSON {
            return Err(AppError::Conflict(format!(
                "you keep {MAX_PLAYGROUNDS_PER_PERSON} playgrounds, the most a person may; a \
                 removed one keeps its files on the server and still counts"
            )));
        }
        Ok(())
    }

    /// `POST /api/sandboxes` `{mode: "playground"}` (PLY-001): a playground from a template, for
    /// anyone signed in, with or without GitHub.
    ///
    /// # Errors
    /// `Invalid` for an empty name or an unknown template; Git failures (nothing stays behind).
    pub(crate) async fn playground_from_template(
        self: &Arc<Self>,
        user: &UserRecord,
        name: &str,
        template: &str,
    ) -> AppResult<SandboxRecord> {
        let kind = TemplateKind::parse(template).ok_or_else(|| {
            AppError::Invalid(format!(
                "unknown template {template}; choose one of {}",
                TemplateKind::ALL
                    .iter()
                    .map(|kind| kind.id())
                    .collect::<Vec<_>>()
                    .join(", ")
            ))
        })?;
        let name = playground_name(name)?;
        self.playground_quota(user).await?;
        let id = SandboxId::parse(&self.deps.crypto.new_id()).map_err(AppError::internal)?;
        let profile = DeliveryProfile::standard();
        let files = scaffold_files(&name, kind);
        let seeded = self
            .seed_from_template(&id, &name, kind, &files, &profile, user)
            .await;
        if let Err(err) = seeded {
            self.discard_playground(&id).await;
            return Err(err);
        }
        self.record_playground(
            id,
            &name,
            &profile,
            user,
            &format!("template={}", kind.id()),
        )
        .await
    }

    /// The bare `origin`, the template's scaffold pushed to the production and integration
    /// branches (as for a GitHub repository), and the sandbox clone.
    async fn seed_from_template(
        &self,
        id: &SandboxId,
        name: &str,
        template: TemplateKind,
        files: &[(String, String)],
        profile: &DeliveryProfile,
        user: &UserRecord,
    ) -> AppResult<()> {
        let origin = self.playground_origin(id);
        self.deps
            .git
            .init_bare(&origin, &profile.production_branch)
            .await?;
        let url = origin.to_string_lossy().into_owned();
        self.push_scaffold(
            id,
            &Scaffold {
                url: &url,
                name,
                files,
                template,
                profile,
                user,
            },
            &no_token(),
        )
        .await?;
        self.clone_playground(id, &origin).await
    }

    /// Clones a playground's `origin` into its sandbox repository. Agents cannot reach `origin`:
    /// Yulius keeps `origin/<branch>` current in the clone (ADR-0081).
    async fn clone_playground(&self, id: &SandboxId, origin: &Path) -> AppResult<()> {
        self.deps
            .git
            .sync_clone(
                &origin.to_string_lossy(),
                &self.sandbox_repo(id),
                &no_token(),
            )
            .await
    }

    /// Records a seeded playground as a sandbox (its slug and name unique), audits it and tells
    /// viewers. On failure its directory is removed.
    async fn record_playground(
        &self,
        id: SandboxId,
        name: &str,
        profile: &DeliveryProfile,
        user: &UserRecord,
        detail: &str,
    ) -> AppResult<SandboxRecord> {
        // Again here: creations that ran at once each passed the first check.
        if let Err(err) = self.playground_quota(user).await {
            self.discard_playground(&id).await;
            return Err(err);
        }
        let record = SandboxRecord {
            id: id.clone(),
            slug: String::new(),
            repo_owner: String::new(),
            repo_name: name.to_owned(),
            github_repo_id: None,
            default_branch: profile.production_branch.clone(),
            profile_json: serde_json::to_string(&profile_view(profile))
                .map_err(AppError::internal)?,
            port_base: 0,
            port_count: PORT_RANGE_SIZE,
            onboarding_state: "not_needed".into(),
            onboarding_task_id: None,
            created_by: user.id.clone(),
            created_at: self.now(),
            removed_at: None,
        };
        let record = match self.insert_sandbox_unique(record).await {
            Ok(record) => record,
            Err(err) => {
                self.discard_playground(&id).await;
                return Err(err);
            }
        };
        self.deps
            .store
            .audit(
                self.now(),
                &self.login_of(&user.id).await,
                "sandbox.playground",
                Some(record.id.as_str()),
                Some(&format!("{} {detail}", record.slug)),
            )
            .await?;
        self.publish_resync(Some(record.id.clone()));
        Ok(record)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_playground_is_named_by_its_slug() {
        assert_eq!(playground_name("My Shop").ok().as_deref(), Some("my-shop"));
        assert_eq!(playground_name("  ✨ ").ok().as_deref(), Some("project"));
        assert!(playground_name("   ").is_err());
    }
}
