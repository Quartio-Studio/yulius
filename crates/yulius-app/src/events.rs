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
    /// Only people with write access to `sandbox` may see it: what the editor's files show
    /// (SPEC-127). Without a sandbox nobody does.
    pub writers_only: bool,
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
            writers_only: false,
            event,
        });
    }

    /// Publishes an event of `sandbox` that only its writers' streams receive (never blocks).
    pub fn publish_to_writers(&self, sandbox: SandboxId, event: UiEvent) {
        let _ = self.tx.send(Envelope {
            sandbox: Some(sandbox),
            admin_only: false,
            user: None,
            writers_only: true,
            event,
        });
    }

    /// Publishes an instance-wide event that only `user`'s streams receive (never blocks).
    pub fn publish_to_user(&self, user: UserId, event: UiEvent) {
        let _ = self.tx.send(Envelope {
            sandbox: None,
            admin_only: false,
            user: Some(user),
            writers_only: false,
            event,
        });
    }

    /// New subscription.
    #[must_use]
    pub fn subscribe(&self) -> broadcast::Receiver<Envelope> {
        self.tx.subscribe()
    }
}

/// What a person may see, decided once per sandbox until [`Reach::forget`] (SPEC-002 §7.8).
#[derive(Debug, Default)]
pub struct Reach {
    visible: std::collections::HashMap<SandboxId, bool>,
    writable: std::collections::HashMap<SandboxId, bool>,
}

impl Reach {
    /// Forgets the decisions: access is decided again (a re-check, a lag).
    pub fn forget(&mut self) {
        self.visible.clear();
        self.writable.clear();
    }
}

impl crate::context::App {
    /// Whether `user`'s event stream carries `envelope`: the one rule of `WS /api/events` and of
    /// the notifications sent to a person's chat (SPEC-444 TGC-021). Admin-only events reach
    /// instance administrators, a person's own events only them, a sandbox's events its viewers
    /// (its writers for `writers_only`), and a playground guest no instance-wide event.
    pub async fn envelope_reaches(
        &self,
        user: &crate::model::UserRecord,
        envelope: &Envelope,
        reach: &mut Reach,
    ) -> bool {
        if envelope.admin_only && !(user.is_superadmin || user.is_instance_admin) {
            return false;
        }
        // A person's own event (their notepad, SPEC-103) reaches no one else.
        if envelope.user.as_ref().is_some_and(|id| *id != user.id) {
            return false;
        }
        let Some(sandbox) = &envelope.sandbox else {
            // A playground's guest gets only their playground's events and their own (GST-004).
            return !(envelope.writers_only || (self.is_guest(user) && envelope.user.is_none()));
        };
        let visible = match reach.visible.get(sandbox) {
            Some(v) => *v,
            None => {
                let ok = self.can_view(user, sandbox).await;
                reach.visible.insert(sandbox.clone(), ok);
                ok
            }
        };
        if !visible {
            return false;
        }
        // What the editor shows (who has which file open) is for those who may open it.
        if !envelope.writers_only {
            return true;
        }
        match reach.writable.get(sandbox) {
            Some(v) => *v,
            None => {
                let ok = self.can_work_in(user, sandbox).await;
                reach.writable.insert(sandbox.clone(), ok);
                ok
            }
        }
    }
}
