//! In-process UI event bus with bounded fan-out (SPEC-002 §7.8).

use tokio::sync::broadcast;
use yulius_contracts::events::UiEvent;
use yulius_domain::ids::{SandboxId, UserId};

/// An event plus its visibility scope.
#[derive(Debug, Clone)]
pub struct Envelope {
    /// Sandbox the event concerns (`None` = instance-wide).
    pub sandbox: Option<SandboxId>,
    /// Only instance administrators may see it.
    pub admin_only: bool,
    /// Only this person may see it (`None` = anyone the other fields allow; SPEC-103).
    pub user: Option<UserId>,
    /// The event.
    pub event: UiEvent,
}

/// Broadcast bus. Slow subscribers lag and receive a `Resync` instead of blocking publishers.
#[derive(Debug, Clone)]
pub struct EventBus {
    tx: broadcast::Sender<Envelope>,
}

impl EventBus {
    /// Bus with a per-subscriber buffer of `capacity` events.
    #[must_use]
    pub fn new(capacity: usize) -> Self {
        let (tx, _) = broadcast::channel(capacity.max(1));
        Self { tx }
    }

    /// Publishes (never blocks; dropped when nobody listens).
    pub fn publish(&self, sandbox: Option<SandboxId>, admin_only: bool, event: UiEvent) {
        let _ = self.tx.send(Envelope {
            sandbox,
            admin_only,
            user: None,
            event,
        });
    }

    /// Publishes an instance-wide event that only `user`'s streams receive (never blocks).
    pub fn publish_to_user(&self, user: UserId, event: UiEvent) {
        let _ = self.tx.send(Envelope {
            sandbox: None,
            admin_only: false,
            user: Some(user),
            event,
        });
    }

    /// New subscription.
    #[must_use]
    pub fn subscribe(&self) -> broadcast::Receiver<Envelope> {
        self.tx.subscribe()
    }
}
