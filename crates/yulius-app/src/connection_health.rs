//! Connection troubles on the panel (SPEC-361, ADR-0246): the egress proxy and the server's
//! sandbox relays report refusals to the app's registry; the app tells the sandbox's people when
//! a trouble begins or ends and lists the open ones they may see.

use std::collections::HashMap;
use std::sync::Arc;

use yulius_contracts::api::status::{ConnectionHolderView, ConnectionTroubleView, ConnectionsView};
use yulius_contracts::events::UiEvent;
use yulius_domain::connection_health::{ConnectionHealth, Holder, Limit, Trouble};
use yulius_domain::ids::{AttemptId, SandboxId};

use crate::context::App;
use crate::error::AppResult;
use crate::events::EventBus;
use crate::model::UserRecord;

/// A registry that logs each trouble's beginning and end and publishes it as
/// `connections.updated` (CHR-003, CHR-007): a sandbox's to its viewers, the proxy's to everyone.
pub(crate) fn connection_health(events: EventBus) -> Arc<ConnectionHealth> {
    let health = Arc::new(ConnectionHealth::default());
    health.on_change(move |trouble, began| {
        let holders: Vec<String> = trouble
            .holders
            .iter()
            .map(|h| format!("{} ({})", h.name, h.held))
            .collect();
        if began {
            tracing::warn!(
                limit = trouble.limit.as_str(),
                sandbox = trouble.sandbox.as_deref().unwrap_or("-"),
                held = trouble.held,
                max = trouble.max,
                holders = %holders.join(", "),
                "connection trouble began: a limit refuses connections (docs/operations.md)"
            );
        } else {
            tracing::info!(
                limit = trouble.limit.as_str(),
                sandbox = trouble.sandbox.as_deref().unwrap_or("-"),
                refused = trouble.refused,
                "connection trouble ended: the limit has room again"
            );
        }
        // A sandbox's trouble reaches only its viewers: one whose id does not parse reaches nobody
        // rather than everyone (review round 1).
        let sandbox = match trouble.sandbox.as_deref().map(SandboxId::parse) {
            None => None,
            Some(Ok(id)) => Some(id),
            Some(Err(_)) => return,
        };
        events.publish(
            sandbox,
            false,
            UiEvent::ConnectionsUpdated {
                sandbox_id: trouble.sandbox.clone(),
            },
        );
    });
    health
}

impl App {
    /// The registry the egress proxy and the sandbox relays report their troubles to.
    #[must_use]
    pub fn connection_health(&self) -> Arc<ConnectionHealth> {
        Arc::clone(&self.connection_health)
    }

    /// The open connection troubles `user` may see (CHR-004): those of the sandboxes they can
    /// view, and the proxy's own, whose sandboxes they cannot view are "another sandbox".
    ///
    /// # Errors
    /// Store failures.
    pub async fn connections_view(&self, user: &UserRecord) -> AppResult<ConnectionsView> {
        let troubles = self.connection_health.troubles();
        if troubles.is_empty() {
            return Ok(ConnectionsView::default());
        }
        let visible: HashMap<String, String> = self
            .visible_sandboxes(user)
            .await?
            .into_iter()
            .map(|(sandbox, _)| (sandbox.id.to_string(), sandbox.slug))
            .collect();
        let mut out = Vec::new();
        for trouble in troubles {
            let sandbox_name = match &trouble.sandbox {
                Some(id) => match visible.get(id) {
                    Some(name) => Some(name.clone()),
                    None => continue,
                },
                None => None,
            };
            let mut holders = Vec::new();
            for holder in &trouble.holders {
                holders.push(match trouble.limit {
                    Limit::Proxy => sandbox_holder(holder, &visible),
                    Limit::Share | Limit::Relay => self.run_holder(holder).await?,
                });
            }
            out.push(view(&trouble, sandbox_name, holders));
        }
        Ok(ConnectionsView { troubles: out })
    }

    /// Names a run of a sandbox by its key (`EgressClient::use_key`): an agent run by its task, a
    /// CI job, a Superagent's shell, else a run that ended.
    async fn run_holder(&self, holder: &Holder) -> AppResult<ConnectionHolderView> {
        let mut view = ConnectionHolderView {
            kind: "run".into(),
            label: "a run that ended".into(),
            task_id: None,
            task_number: None,
            task_title: None,
            sandbox_id: None,
            held: holder.held,
        };
        if let Ok(id) = AttemptId::parse(&holder.name)
            && let Some(attempt) = self.deps.store.attempt(&id).await?
            && let Some(task) = self.deps.store.task(&attempt.task_id).await?
        {
            view.kind = "task".into();
            view.label = format!("task #{}", task.number);
            view.task_id = Some(task.id.to_string());
            view.task_number = Some(task.number);
            view.task_title = Some(task.title);
        } else if self.ci_job_has_egress(&holder.name) {
            view.kind = "ci".into();
            view.label = "a CI job".into();
        } else if self.shell(&holder.name).is_some() {
            view.kind = "superagent".into();
            view.label = "a Superagent".into();
        }
        Ok(view)
    }
}

/// Names a sandbox holding connections at the proxy's total: by name when `visible`.
fn sandbox_holder(holder: &Holder, visible: &HashMap<String, String>) -> ConnectionHolderView {
    let name = visible.get(&holder.name);
    ConnectionHolderView {
        kind: if name.is_some() {
            "sandbox"
        } else {
            "other_sandbox"
        }
        .into(),
        label: name.map_or_else(|| "another sandbox".to_owned(), Clone::clone),
        task_id: None,
        task_number: None,
        task_title: None,
        sandbox_id: name.map(|_| holder.name.clone()),
        held: holder.held,
    }
}

fn view(
    trouble: &Trouble,
    sandbox_name: Option<String>,
    holders: Vec<ConnectionHolderView>,
) -> ConnectionTroubleView {
    ConnectionTroubleView {
        id: format!("{}-{}", trouble.since_ms, trouble.id),
        limit: trouble.limit.as_str().into(),
        sandbox_id: trouble.sandbox.clone(),
        sandbox_name,
        since_ms: trouble.since_ms,
        held: trouble.held,
        max: trouble.max,
        refused: trouble.refused,
        holders,
    }
}
