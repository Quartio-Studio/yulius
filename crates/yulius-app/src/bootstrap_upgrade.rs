//! The upgrade path of the Infra bootstrap default (SPEC-575 MFS-010 to MFS-012, ADR-0415).
//!
//! A release that changes what infra bootstrap lays by default raises [`BOOTSTRAP_REVISION`].
//! At the next server start Yulius creates one task in each onboarded sandbox still at an older
//! revision. The task uses the Infra bootstrap workflow and its normal delivery (spec, pull
//! request, review, `delivery_merge`), owned by the sandbox's creator. It brings the project's
//! own `yulius.toml` and guidance to the new default, keeping what is the project's own, and
//! reports what it found. The revision is claimed before the task is created (`bootstrap_upgrades`
//! table), so a restart creates no second task.
//!
//! - **Not onboarded yet.** A sandbox that is not onboarded yet (and one created after the
//!   release: `projects.rs` records it at creation) is recorded as `laid`. Its onboarding lays
//!   the current default, so it is never upgraded.
//! - **A claim left unfinished.** Every start finishes a claim still `claimed`. It adopts the
//!   upgrade task made since the claim: a stop between making and linking it, or a store error
//!   after its record. Otherwise it makes the task: a stop before making it, or a refused
//!   creation (no Claude account).
//! - **Exactly once.** The link is a compare-and-set to `linked`. A linked task a person deletes
//!   is not made again.

use std::sync::Arc;

use yulius_contracts::api::tasks::CreateTask;

use crate::context::App;
use crate::model::{SandboxRecord, TaskOrigin};

/// The current Infra bootstrap revision: 1 makes merge-first staging the default (SPEC-575).
pub const BOOTSTRAP_REVISION: u32 = 1;

/// The workflow the upgrade task follows.
const WORKFLOW: &str = "wf-infra-bootstrap";

/// The upgrade task's title.
pub const UPGRADE_TITLE: &str = "Adopt merge-first staging (Infra bootstrap revision 1)";

/// What the upgrade task is asked (SPEC-575 MFS-011, MFS-012).
pub const UPGRADE_PROMPT: &str = "Yulius raised the Infra bootstrap default to revision 1: \
merge-first staging (SPEC-575, skill infra-bootstrap step 3). Bring this project to it through \
the normal workflow (spec, pull request, independent review, delivery_merge), changing only \
what the new default needs.

1. Read yulius.toml, scripts/cd/, AGENTS.md and the docs that describe CI and deploys. Keep \
every project-specific command, CI job, CD target, secret name and vault grant as it is.
2. A shared staging target (a dev-stage [cd.<name>] deploying a machine people use before \
production): set `merge_first = true` on that one table (it needs deploy, verify and rollback, \
and its scripts must be idempotent: Yulius may run a deploy again after a restart). Mark the \
long checks `after_merge = true` (keep every check; at least one stays a quick check on pull \
requests). Add a `[local]` deployment that deploys and tests the app inside the task's own \
sandbox only: its ports, isolated test data, only the dev credentials agent sessions get, and \
a refusal of any other target that the gate checks. Staging SSH, release publishing or signing \
and production credentials stay named only in their [cd.*] tables.
3. No shared target: change no CI job, and say so in [cd.dev] `about` (\"no shared target: the \
full suite gates each pull request\").
4. Reconcile repository guidance that contradicts the new default (AGENTS.md, docs or scripts \
saying a merge deploys staging, or deploying staging by hand) in the same pull request.
5. Deliveries already in progress need nothing: a staging request for a revision staging \
already serves is answered as included, so nothing deploys twice.
6. Your summary says which applied: switched (the target's name and the jobs moved after the \
merge), no shared target (why), and any blocker (a missing credential or machine: needs_you).";

impl App {
    /// The upgrade task created in `sandbox` since `claimed_at`, if one is: the automation-origin
    /// task with the upgrade's title and prompt (the oldest).
    async fn bootstrap_task_since(
        &self,
        sandbox: &SandboxRecord,
        claimed_at: yulius_domain::time::Timestamp,
    ) -> Option<yulius_domain::ids::TaskId> {
        // Live and archived tasks alike (an archived one was made all the same).
        let mut tasks = self.deps.store.tasks(&sandbox.id, false).await.ok()?;
        tasks.extend(self.deps.store.tasks(&sandbox.id, true).await.ok()?);
        tasks.retain(|t| {
            t.origin == TaskOrigin::Automation
                && t.title == UPGRADE_TITLE
                && t.prompt == UPGRADE_PROMPT
                && t.created_at >= claimed_at
        });
        tasks.sort_by_key(|t| t.created_at);
        tasks.into_iter().next().map(|t| t.id)
    }

    /// Creates the upgrade task of every onboarded sandbox still below [`BOOTSTRAP_REVISION`]
    /// (server start); how many it created. Playgrounds, removed sandboxes and ones not
    /// onboarded yet (their onboarding lays the current default) are left alone.
    pub async fn create_bootstrap_upgrades(self: &Arc<Self>) -> usize {
        let sandboxes = match self.deps.store.sandboxes().await {
            Ok(all) => all,
            Err(err) => {
                tracing::warn!(error = %err, "bootstrap upgrades: sandboxes could not be read");
                return 0;
            }
        };
        let mut created = 0;
        for sandbox in sandboxes {
            if self.bootstrap_upgrade(&sandbox).await {
                created += 1;
            }
        }
        created
    }

    /// One sandbox's upgrade task, when it is due; whether it was created.
    async fn bootstrap_upgrade(self: &Arc<Self>, sandbox: &SandboxRecord) -> bool {
        if sandbox.removed_at.is_some() || sandbox.is_playground() {
            return false;
        }
        let store = &self.deps.store;
        // A sandbox not onboarded yet gets the current default from its onboarding (review
        // round 1 of slice 4: it is recorded as laid, so it is never upgraded later).
        let onboarded = matches!(sandbox.onboarding_state.as_str(), "ready" | "not_needed");
        if !onboarded {
            if let Err(err) = store
                .lay_bootstrap_revision(&sandbox.id, BOOTSTRAP_REVISION, self.now())
                .await
            {
                tracing::warn!(error = %err, sandbox = %sandbox.slug, "bootstrap revision not recorded");
            }
            return false;
        }
        let owner = match store.user(&sandbox.created_by).await {
            Ok(Some(owner)) if owner.disabled_at.is_none() => owner,
            Ok(_) => {
                tracing::warn!(sandbox = %sandbox.slug, "bootstrap upgrade skipped: the sandbox's creator is no longer active");
                return false;
            }
            Err(err) => {
                tracing::warn!(error = %err, sandbox = %sandbox.slug, "bootstrap upgrade: the creator could not be read");
                return false;
            }
        };
        // Claim the revision. A claim a stopped server left (still `claimed`: its task never
        // linked) is finished now (rollout blocker found on PR #1541).
        if let Err(err) = store
            .claim_bootstrap_upgrade(&sandbox.id, BOOTSTRAP_REVISION, self.now())
            .await
        {
            tracing::warn!(error = %err, sandbox = %sandbox.slug, "bootstrap upgrade could not be claimed");
            return false;
        }
        let claim = match store.bootstrap_upgrade(&sandbox.id).await {
            Ok(Some(claim)) if claim.revision == BOOTSTRAP_REVISION && claim.state == "claimed" => {
                claim
            }
            Ok(_) => return false,
            Err(err) => {
                tracing::warn!(error = %err, sandbox = %sandbox.slug, "bootstrap upgrade could not be read");
                return false;
            }
        };
        // A task made for the claim before a stop (or before a store error that followed its
        // record), not linked yet, is adopted: never made twice.
        if let Some(made) = self.bootstrap_task_since(sandbox, claim.claimed_at).await {
            match store
                .link_bootstrap_task(&sandbox.id, BOOTSTRAP_REVISION, &made)
                .await
            {
                Ok(true) => {
                    tracing::info!(sandbox = %sandbox.slug, task = %made.as_str(), "bootstrap upgrade task adopted")
                }
                Ok(false) => {}
                Err(err) => {
                    tracing::warn!(error = %err, sandbox = %sandbox.slug, "bootstrap upgrade task not adopted")
                }
            }
            return false;
        }
        let request = CreateTask {
            prompt: UPGRADE_PROMPT.to_owned(),
            harness: String::new(),
            workflow_id: Some(WORKFLOW.to_owned()),
            model: None,
            effort: None,
            title: Some(UPGRADE_TITLE.to_owned()),
            mode: None,
            after_task_id: None,
            start_rule: None,
            supertask: false,
            prepared: false,
            start_at: None,
        };
        match self
            .create_task_as(&owner, &sandbox.id, &request, TaskOrigin::Automation)
            .await
        {
            Ok(task) => {
                // A stop before this link is finished at the next start: the task is adopted.
                if let Err(err) = store
                    .link_bootstrap_task(&sandbox.id, BOOTSTRAP_REVISION, &task.id)
                    .await
                {
                    tracing::warn!(error = %err, sandbox = %sandbox.slug, "bootstrap upgrade task not linked");
                }
                self.dlv_audit(
                    "yulius",
                    "bootstrap.upgrade",
                    &format!("{}#{}", sandbox.slug, task.number),
                    &format!("Infra bootstrap revision {BOOTSTRAP_REVISION}"),
                )
                .await;
                true
            }
            Err(err) => {
                // The claim stays: the next start adopts a task this call recorded before its
                // error, or makes it.
                tracing::warn!(error = %err, sandbox = %sandbox.slug, "bootstrap upgrade task could not be created; tried again at the next start");
                false
            }
        }
    }
}
