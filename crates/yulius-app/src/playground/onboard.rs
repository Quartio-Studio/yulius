//! A playground from `yulius onboard --playground` (SPEC-171 PGL-001..004): a local checkout
//! without GitHub comes in through the import pipeline (plan, resumable job, completion).
//!
//! A new playground is made when the upload completes, under the sandbox id reserved when its job
//! was created (the job's files live under that id, as for any import): the bundle's branches and
//! tags are copied into a new bare `origin` with the hardened Git, the uncommitted work lands as
//! one commit on the branch `HEAD` names, as for a folder uploaded from the browser (PLY-003).
//! Into the person's existing playground of that name the upload lands as an import into a GitHub
//! sandbox does (branches under `import/<login>/`), leaving the playground's own branches alone.

use std::path::Path;

use yulius_contracts::import::ImportManifest;
use yulius_domain::delivery::DeliveryProfile;
use yulius_domain::ids::SandboxId;

use super::{no_token, playground_name};
use crate::auth::AuthContext;
use crate::context::App;
use crate::error::{AppError, AppResult};
use crate::imports::{AppliedSnapshot, JOB_LIST_LIMIT, JobState, extract_snapshot};
use crate::model::{SandboxRecord, UserRecord};
use crate::ports::store_ext::ImportJobRecord;
use crate::projects::{blocking, identity, remove_tree};

/// Where a playground upload goes when its job is created.
pub(crate) enum UploadTarget {
    /// This sandbox: an existing playground, or the id reserved for a new one.
    Into(SandboxId),
    /// The caller's open job with the same manifest for a playground not made yet: resume it.
    Resume(ImportJobRecord),
}

/// Whether `name` is the playground slug `base` or one of its taken-name variants (`base-<n>`).
fn named_like(name: &str, base: &str) -> bool {
    name == base
        || name
            .strip_prefix(base)
            .and_then(|rest| rest.strip_prefix('-'))
            .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
}

impl App {
    /// The caller's own playground that a CLI upload named `name` goes to (PGL-002): one they
    /// made, not removed, whose slug is the name's or a taken-name variant of it, the oldest.
    ///
    /// # Errors
    /// `Invalid` for an empty name; store failures.
    pub(crate) async fn own_playground(
        &self,
        user: &UserRecord,
        name: &str,
    ) -> AppResult<Option<SandboxRecord>> {
        let base = playground_name(name)?;
        Ok(self
            .deps
            .store
            .sandboxes()
            .await?
            .into_iter()
            .filter(|s| {
                s.is_playground()
                    && s.removed_at.is_none()
                    && s.created_by == user.id
                    && named_like(&s.repo_name, &base)
            })
            .min_by_key(|s| (s.created_at, s.id.as_str().to_owned())))
    }

    /// Where a playground upload's job goes (`POST /api/import` with `playground`): the caller's
    /// playground of that name (write access), else a new one: its branch and bundle are needed,
    /// the caller must be under the limit of 10, and an open job of theirs with the same manifest
    /// is resumed instead of starting another.
    ///
    /// # Errors
    /// `Invalid` manifests; `Conflict` at the limit; store failures.
    pub(crate) async fn playground_upload_target(
        &self,
        ctx: &AuthContext,
        manifest: &ImportManifest,
        canonical: &str,
    ) -> AppResult<UploadTarget> {
        let repo = &manifest.repo;
        if !repo.owner.is_empty() || !repo.remote_url.is_empty() {
            return Err(AppError::Invalid(
                "a playground upload names no GitHub owner or remote".into(),
            ));
        }
        if let Some(record) = self.own_playground(&ctx.user, &repo.name).await? {
            self.require(
                &ctx.user,
                &record.id,
                yulius_domain::permission::Action::Work,
            )
            .await?;
            return Ok(UploadTarget::Into(record.id));
        }
        self.new_playground_branch(manifest)?;
        if manifest.bundle.is_none() {
            return Err(AppError::Invalid(
                "a new playground needs the checkout's branches; upload them (not --sessions-only)"
                    .into(),
            ));
        }
        self.playground_quota(&ctx.user).await?;
        let store = &self.deps.store;
        // The person's unfinished uploads into a playground not made yet: the same one is resumed,
        // the others are superseded, so abandoned uploads never pile up (one open at a time).
        let mut unfinished = Vec::new();
        for job in store.user_import_jobs(&ctx.user.id, JOB_LIST_LIMIT).await? {
            let open = JobState::parse(&job.state).is_some_and(JobState::accepts_uploads);
            let Some(sandbox) = job.sandbox_id.clone() else {
                continue;
            };
            if open && store.sandbox(&sandbox).await?.is_none() {
                if job.manifest == canonical {
                    return Ok(UploadTarget::Resume(job));
                }
                unfinished.push((job, sandbox));
            }
        }
        for (job, sandbox) in unfinished {
            let open = [
                JobState::Created,
                JobState::Receiving,
                JobState::Received,
                JobState::Failed,
            ]
            .map(JobState::as_str);
            if store
                .transition_import_job(
                    &job.id,
                    &open,
                    JobState::Superseded.as_str(),
                    None,
                    None,
                    self.now(),
                )
                .await?
            {
                // Its reserved directory holds only this job's files.
                remove_tree(self.sandbox_dir(&sandbox)).await;
            }
        }
        let id = SandboxId::parse(&self.deps.crypto.new_id()).map_err(AppError::internal)?;
        Ok(UploadTarget::Into(id))
    }

    /// The branch a new playground starts from: the one the checkout's `HEAD` is on.
    fn new_playground_branch(&self, manifest: &ImportManifest) -> AppResult<String> {
        manifest
            .repo
            .head_branch
            .clone()
            .filter(|branch| self.deps.git.valid_branch(branch))
            .ok_or_else(|| {
                AppError::Invalid(
                    "a new playground starts from the branch the checkout's HEAD is on; check \
                     one out"
                        .into(),
                )
            })
    }

    /// Makes the playground `id` from a completed CLI upload whose files are in `job_dir`
    /// (PGL-003). A failure leaves no playground and keeps the uploaded files, so completing again
    /// retries. Returns it and its applied snapshot.
    ///
    /// # Errors
    /// `Invalid` uploads (no bundle, a bundle without the checkout's `HEAD`); `Conflict` at the
    /// limit; Git failures.
    pub(crate) async fn playground_from_cli(
        &self,
        user: &UserRecord,
        id: &SandboxId,
        job_dir: &Path,
        manifest: &ImportManifest,
    ) -> AppResult<(SandboxRecord, Option<AppliedSnapshot>)> {
        let name = playground_name(&manifest.repo.name)?;
        let branch = self.new_playground_branch(manifest)?;
        // Within the limit before any seeding (again when recorded: creations run at once).
        self.playground_quota(user).await?;
        // An earlier attempt that failed half-way left these; the job's files stay.
        self.discard_cli_seed(id).await;
        let staging = self.sandbox_dir(id).join("upload");
        let seeded = self
            .seed_from_bundle(id, job_dir, manifest, &branch, &staging, user)
            .await;
        remove_tree(staging).await;
        let (profile, snapshot) = match seeded {
            Ok(seeded) => seeded,
            Err(err) => {
                self.discard_cli_seed(id).await;
                return Err(err);
            }
        };
        match self
            .record_playground(id.clone(), &name, &profile, user, "cli")
            .await
        {
            Ok(record) => Ok((record, snapshot)),
            Err(err) => {
                // The job's files stay, so completing again retries.
                self.discard_cli_seed(id).await;
                Err(err)
            }
        }
    }

    /// Removes what seeding a playground from a CLI upload made: its `origin`, clone and staging,
    /// not the job's uploaded files.
    async fn discard_cli_seed(&self, id: &SandboxId) {
        remove_tree(self.playground_origin(id)).await;
        remove_tree(self.sandbox_repo(id)).await;
        remove_tree(self.sandbox_dir(id).join("upload")).await;
    }

    /// The bare `origin` from the bundle (every branch and tag), the uncommitted work as one
    /// commit on `branch`, the production and integration branches, and the sandbox clone.
    async fn seed_from_bundle(
        &self,
        id: &SandboxId,
        job_dir: &Path,
        manifest: &ImportManifest,
        branch: &str,
        staging: &Path,
        user: &UserRecord,
    ) -> AppResult<(DeliveryProfile, Option<AppliedSnapshot>)> {
        let bundle = manifest.bundle.as_ref().ok_or_else(|| {
            AppError::Invalid("a new playground needs the checkout's branches".into())
        })?;
        let bundle = std::path::absolute(job_dir.join(&bundle.name)).map_err(AppError::internal)?;
        let git = &self.deps.git;
        let origin = self.playground_origin(id);
        git.init_bare(&origin, branch).await?;
        // The hardened fetch reads only refs and objects from the bundle (PLY-003).
        git.copy_refs(&origin, &bundle).await?;
        let work = staging.join("work");
        git.sync_clone(&origin.to_string_lossy(), &work, &no_token())
            .await?;
        let head = &manifest.repo.head_sha;
        if git
            .resolve_commit(&work, &format!("origin/{branch}"))
            .await?
            .as_deref()
            != Some(head.as_str())
        {
            return Err(AppError::Invalid(format!(
                "the uploaded branches have no {branch} at the checkout's HEAD {head}"
            )));
        }
        let snapshot = match &manifest.snapshot {
            Some(file) => {
                let (archive, root) = (job_dir.join(&file.name), work.clone());
                let stats = blocking(move || extract_snapshot(&archive, &root)).await?;
                let (author, email) = identity(user);
                let committed = git
                    .commit_all(
                        &work,
                        "chore: the checkout's uncommitted work, as uploaded by yulius onboard",
                        &author,
                        &email,
                    )
                    .await?;
                if committed {
                    git.push(&work, branch, branch, &no_token()).await?;
                }
                Some(AppliedSnapshot {
                    sha256: file.sha256.clone(),
                    head_sha: head.clone(),
                    branch: branch.to_owned(),
                    committed,
                    files: stats.files,
                    links: stats.links,
                    deleted: stats.deleted,
                    skipped: stats.skipped,
                })
            }
            None => None,
        };
        let profile = self.seed_profile(&work, branch).await?;
        self.clone_playground(id, &origin).await?;
        Ok((profile, snapshot))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_cli_upload_goes_to_the_slug_or_a_taken_name_variant() {
        assert!(named_like("shop", "shop"));
        assert!(named_like("shop-2", "shop"));
        assert!(named_like("shop-12", "shop"));
        assert!(!named_like("shop-", "shop"));
        assert!(!named_like("shop-a", "shop"));
        assert!(!named_like("shopping", "shop"));
        assert!(!named_like("my-shop", "shop"));
    }
}
