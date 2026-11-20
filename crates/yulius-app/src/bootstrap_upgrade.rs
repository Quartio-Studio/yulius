//! The upgrade path of the Infra bootstrap default (SPEC-575 MFS-010 to MFS-012, ADR-0415).
//!
//! A release that changes what infra bootstrap lays by default raises [`BOOTSTRAP_REVISION`].
//! At the next server start Yulius creates one task in each onboarded sandbox still at an older
//! revision. The task uses the Infra bootstrap workflow and its normal delivery (spec, pull
//! request, review, `delivery_merge`), owned by the sandbox's creator. It brings the project's
//! own `yulius.toml` and guidance to the new default, keeping what is the project's own, and
//! reports what it found. The revision is claimed before the task is created (`bootstrap_upgrades`
//! table), so a restart creates no second task; a claim whose task could not be created is given
//! back, to be tried at the next start.

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
        let onboarded = matches!(sandbox.onboarding_state.as_str(), "ready" | "not_needed");
        if sandbox.removed_at.is_some() || sandbox.is_playground() || !onboarded {
            return false;
        }
        let store = &self.deps.store;
        let Ok(Some(owner)) = store.user(&sandbox.created_by).await else {
            return false;
        };
        if owner.disabled_at.is_some() {
            return false;
        }
        match store
            .claim_bootstrap_upgrade(&sandbox.id, BOOTSTRAP_REVISION, self.now())
            .await
        {
            Ok(true) => {}
            Ok(false) => return false,
            Err(err) => {
                tracing::warn!(error = %err, sandbox = %sandbox.slug, "bootstrap upgrade could not be claimed");
                return false;
            }
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
                if let Err(err) = store
                    .set_bootstrap_task(&sandbox.id, BOOTSTRAP_REVISION, &task.id)
                    .await
                {
                    tracing::warn!(error = %err, sandbox = %sandbox.slug, "bootstrap upgrade task not recorded");
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
                tracing::warn!(error = %err, sandbox = %sandbox.slug, "bootstrap upgrade task could not be created");
                let _ = store
                    .release_bootstrap_upgrade(&sandbox.id, BOOTSTRAP_REVISION)
                    .await;
                false
            }
        }
    }
}
