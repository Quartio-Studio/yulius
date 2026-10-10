//! Sandbox (project) queries and views (SPEC-001 §5.2). Creation/import lives with the Git and
//! GitHub integration.

use yulius_contracts::api::sandboxes::{Profile, RepoRef, Sandbox, SandboxCounts};
use yulius_domain::permission::{AccessLevel, Action};
use yulius_domain::task::{Activity, TaskStatus};

use crate::auth::AuthContext;
use crate::context::App;
use crate::error::AppResult;
use crate::model::SandboxRecord;

/// The default profile for new projects (SPEC-001 DLV-002).
#[must_use]
pub fn standard_profile() -> Profile {
    crate::delivery::profile_to_contract(&yulius_domain::delivery::DeliveryProfile::standard())
}

impl App {
    /// API view of a sandbox for a caller with `access`.
    ///
    /// # Errors
    /// Store failures.
    pub async fn sandbox_view(
        &self,
        record: &SandboxRecord,
        access: AccessLevel,
    ) -> AppResult<Sandbox> {
        let tasks = self.deps.store.tasks(&record.id, false).await?;
        let mut counts = SandboxCounts::default();
        for task in &tasks {
            if task.status.is_live() {
                counts.running += 1;
            }
            if task.status == TaskStatus::Queued {
                counts.queued += 1;
            }
            // A task paused for subscription capacity needs nobody: it continues by itself
            // (ROT-010, F 2026-10-10).
            if task.activity == Some(Activity::NeedsAttention)
                && task.status != TaskStatus::WaitingCapacity
            {
                counts.needs_attention += 1;
            }
        }
        Ok(Sandbox {
            id: record.id.to_string(),
            slug: record.slug.clone(),
            name: record.repo_name.clone(),
            repo: RepoRef {
                owner: record.repo_owner.clone(),
                name: record.repo_name.clone(),
                url: format!(
                    "https://github.com/{}/{}",
                    record.repo_owner, record.repo_name
                ),
            },
            access: access.as_str().to_owned(),
            default_branch: record.default_branch.clone(),
            profile: serde_json::from_str(&record.profile_json)
                .unwrap_or_else(|_| standard_profile()),
            onboarding: record.onboarding_state.clone(),
            port_base: record.port_base,
            port_count: record.port_count,
            counts,
        })
    }

    /// Sandboxes visible to the caller.
    ///
    /// # Errors
    /// Store failures.
    pub async fn list_sandboxes(&self, ctx: &AuthContext) -> AppResult<Vec<Sandbox>> {
        let mut out = Vec::new();
        for (record, level) in self.visible_sandboxes(&ctx.user).await? {
            out.push(self.sandbox_view(&record, level).await?);
        }
        Ok(out)
    }

    /// One sandbox, if visible.
    ///
    /// # Errors
    /// `NotFound` when invisible.
    pub async fn get_sandbox(&self, ctx: &AuthContext, id: &str) -> AppResult<Sandbox> {
        let id = yulius_domain::ids::SandboxId::parse(id)
            .map_err(|_| crate::error::AppError::NotFound)?;
        let (record, level) = self.require(&ctx.user, &id, Action::View).await?;
        self.sandbox_view(&record, level).await
    }
}
