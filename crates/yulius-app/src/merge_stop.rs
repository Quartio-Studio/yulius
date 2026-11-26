//! A red after-merge gate stops the merge queue (SPEC-676, ADR-0482): while the integration
//! branch's gate history has an open failure that is no quarantined test, and the project's
//! `[merge_queue] stop_on_red_gate` is on (by default beside a merge-first staging target), only
//! a repair merges. The armed-merge pass, `delivery_merge` and the merge trains ask
//! [`App::merge_stop`]; the decision is `yulius_domain::merge_queue::MergeStop`.
//!
//! Contract: the state is read from the gate history at the integration head as the clone has
//! it, at most every [`STOP_READ_MS`] per project and at once for a new head. A file or history
//! that cannot be read stops nothing: the queue merges as it did before the rule.

use yulius_domain::merge_queue::{MergeStop, OpenFailure};
use yulius_domain::spec_format::parse_yulius_toml_stop_on_red_gate;

use crate::context::App;
use crate::delivery::delivery_profile;
use crate::model::SandboxRecord;

/// How long a project's stop is kept before it is read again (ms).
pub(crate) const STOP_READ_MS: i64 = 15_000;

/// What a merge's audit adds about the stop (SPEC-676 RGS-005): a merge its agent `marked` the
/// repair, or one that went through a `stopped` queue as the breaker's.
pub(crate) const fn repair_audit(marked: bool, stopped: bool) -> &'static str {
    if marked {
        "; marked as the repair of the red gate"
    } else if stopped {
        "; as the repair of the red gate"
    } else {
        ""
    }
}

/// A project's stop as last read: the integration head, when, and the stop.
pub(crate) type StopRead = (String, i64, Option<MergeStop>);

impl App {
    /// The merge queue's stop in `sandbox` now (SPEC-676 RGS-002), `None` while merges go on;
    /// kept for [`STOP_READ_MS`] per integration head. Public for the integration tests.
    #[doc(hidden)]
    pub async fn merge_stop(&self, sandbox: &SandboxRecord) -> Option<MergeStop> {
        if sandbox.is_playground() {
            return None;
        }
        let branch = delivery_profile(sandbox).integration_branch;
        let repo = self.sandbox_repo(&sandbox.id);
        let head = self
            .deps
            .git
            .resolve_commit(&repo, &format!("refs/remotes/origin/{branch}"))
            .await
            .ok()
            .flatten()?;
        let now = self.now().0;
        if let Some((read_head, at, stop)) = self.stop_reads().get(&sandbox.id)
            && *read_head == head
            && now - at < STOP_READ_MS
        {
            return stop.clone();
        }
        let stop = self.read_merge_stop(sandbox).await;
        self.stop_reads()
            .insert(sandbox.id.clone(), (head, now, stop.clone()));
        stop
    }

    /// [`App::merge_stop`] read now, not from what was kept.
    pub(crate) async fn read_merge_stop(&self, sandbox: &SandboxRecord) -> Option<MergeStop> {
        if sandbox.is_playground() {
            return None;
        }
        let branch = delivery_profile(sandbox).integration_branch;
        let repo = self.sandbox_repo(&sandbox.id);
        let text = self
            .deps
            .git
            .show_file(&repo, &format!("origin/{branch}"), "yulius.toml")
            .await
            .ok()
            .flatten()?;
        if !parse_yulius_toml_stop_on_red_gate(&text).unwrap_or(false) {
            return None;
        }
        let red = match self.integration_red(sandbox).await {
            Ok(red) => red,
            Err(err) => {
                tracing::warn!(sandbox = %sandbox.slug, error = %err, "the merge queue's stop could not be read");
                return None;
            }
        };
        // The newest red run is the one to read and run again.
        let run = red.last()?.run_id.clone();
        let quarantined = self.quarantined_names(sandbox).await;
        let names: Vec<&str> = quarantined.iter().map(String::as_str).collect();
        let failures = red
            .into_iter()
            .map(|notice| OpenFailure {
                test: notice.test,
                revision: notice.revision,
                merge_task: notice.merge_task,
            })
            .collect();
        MergeStop::of(failures, &names, &run)
    }

    /// The project's quarantined flaky tests by name (ADR-0356); none when unreadable.
    pub(crate) async fn quarantined_names(&self, sandbox: &SandboxRecord) -> Vec<String> {
        self.deps
            .store
            .quarantined_tests(Some(&sandbox.id))
            .await
            .unwrap_or_default()
            .into_iter()
            .map(|q| q.test)
            .collect()
    }

    fn stop_reads(
        &self,
    ) -> std::sync::MutexGuard<'_, std::collections::HashMap<yulius_domain::ids::SandboxId, StopRead>>
    {
        self.merge_stops
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}
