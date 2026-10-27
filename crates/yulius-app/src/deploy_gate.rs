//! The deploy gate of an update (SPEC-342, ADR-0232): from the moment an update's build is done
//! until its install ended, no CD run or environment command run starts, so the restart never
//! cuts one.
//!
//! A deploy takes a [`DeployPermit`] before it does anything and keeps it until its run is
//! recorded as running (or it was refused). The update closes the gate and then reads the
//! deploys recorded as running: every deploy that passed the gate before the close is either
//! still counted as starting or already recorded, so it is never missed.

use std::sync::{Mutex, MutexGuard, PoisonError};

/// The refusal of a deploy while the gate is closed.
pub const DEPLOY_REFUSED: &str = "nothing ran: Yulius is being updated; deploy again once the \
                                  update finished (Settings → Instance shows it)";

#[derive(Debug, Default)]
struct Gate {
    closed: bool,
    starting: u32,
}

/// Whether deploys may start, and how many are starting (owned by the update state).
#[derive(Debug, Default)]
pub struct DeployGate(Mutex<Gate>);

/// A deploy that passed the gate and is not recorded yet: counted as starting until dropped.
#[derive(Debug)]
#[must_use = "a deploy is counted as starting only while its permit is held"]
pub struct DeployPermit<'a>(&'a DeployGate);

impl DeployGate {
    fn lock(&self) -> MutexGuard<'_, Gate> {
        // A panic under this lock leaves two plain fields consistent.
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// A permit to start a deploy, or `None` while the gate is closed.
    pub fn permit(&self) -> Option<DeployPermit<'_>> {
        let mut gate = self.lock();
        if gate.closed {
            return None;
        }
        gate.starting = gate.starting.saturating_add(1);
        Some(DeployPermit(self))
    }

    /// Closes (`true`) or opens the gate; returns how many deploys are starting.
    pub fn set_closed(&self, closed: bool) -> u32 {
        let mut gate = self.lock();
        gate.closed = closed;
        gate.starting
    }

    /// How many deploys are starting.
    #[must_use]
    pub fn starting(&self) -> u32 {
        self.lock().starting
    }

    /// Whether the gate is closed.
    #[must_use]
    pub fn is_closed(&self) -> bool {
        self.lock().closed
    }
}

/// When an update first found its build done with a deploy running, per update run: a wait
/// recorded for another run (one cancelled while a pass was reading the store) never counts
/// against this one (SPEC-342 DPU-004, review H1).
#[derive(Debug, Default)]
pub struct DeployWait(Mutex<Option<(String, i64)>>);

impl DeployWait {
    /// When `run` started waiting: `now` on its first call, or after the wait of another run.
    pub fn since(&self, run: &str, now: i64) -> i64 {
        let mut wait = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        match wait.as_ref() {
            Some((waiting, since)) if waiting == run => *since,
            _ => {
                *wait = Some((run.to_owned(), now));
                now
            }
        }
    }

    /// Forgets the wait.
    pub fn reset(&self) {
        *self.0.lock().unwrap_or_else(PoisonError::into_inner) = None;
    }
}

impl Drop for DeployPermit<'_> {
    fn drop(&mut self) {
        let mut gate = self.0.lock();
        gate.starting = gate.starting.saturating_sub(1);
    }
}

#[cfg(test)]
mod tests {
    use super::{DeployGate, DeployWait};

    #[test]
    fn a_permit_counts_as_starting_until_dropped_and_none_is_given_once_closed() {
        let gate = DeployGate::default();
        let permit = gate.permit();
        assert!(permit.is_some());
        assert_eq!(gate.set_closed(true), 1, "the open permit is counted");
        assert!(gate.is_closed());
        assert!(gate.permit().is_none(), "refused once closed");
        drop(permit);
        assert_eq!(gate.set_closed(true), 0);
        assert_eq!(gate.set_closed(false), 0);
        assert!(gate.permit().is_some(), "open again");
    }

    #[test]
    fn a_wait_counts_from_its_own_runs_first_pass() {
        let wait = DeployWait::default();
        assert_eq!(wait.since("a", 10), 10);
        assert_eq!(wait.since("a", 50), 10, "the same run keeps its start");
        assert_eq!(wait.since("b", 90), 90, "another run's wait never counts");
        wait.reset();
        assert_eq!(wait.since("b", 120), 120);
    }
}
