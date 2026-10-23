//! One writer of an agent state at a time (SPEC-278 CDS-005, ADR-0207).
//!
//! A CD run holds the states its table names from before its restore until after its save; a
//! task's `secret_state_save` and a person's import hold one for their write. The hold is in
//! memory: a restart ends every deploy that held one (ADR-0109) and the process with the holds.
//! A run takes all its states in one step, so two runs naming the same states in another order
//! never wait for each other.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};

use tokio::sync::Notify;

/// A held state: its sandbox and name.
type Key = (String, String);

/// The holds of every state.
#[derive(Default)]
pub struct StateHolds {
    held: Mutex<HashMap<Key, String>>,
    released: Notify,
}

/// What holds a set of states until dropped.
pub(crate) struct StateHold {
    holds: Arc<StateHolds>,
    keys: Vec<Key>,
}

impl Drop for StateHold {
    fn drop(&mut self) {
        if self.keys.is_empty() {
            return;
        }
        {
            let mut held = self
                .holds
                .held
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            for key in &self.keys {
                held.remove(key);
            }
        }
        self.holds.released.notify_waiters();
    }
}

/// A state another writer holds: its name and the holder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct HeldElsewhere {
    pub(crate) name: String,
    pub(crate) holder: String,
}

impl StateHolds {
    /// Holds every state of `names` in `sandbox` for `holder`, or none when one is held.
    pub(crate) fn try_hold(
        self: &Arc<Self>,
        sandbox: &str,
        names: &[String],
        holder: &str,
    ) -> Result<StateHold, HeldElsewhere> {
        let mut held = self.held.lock().unwrap_or_else(PoisonError::into_inner);
        let keys: Vec<Key> = names
            .iter()
            .map(|name| (sandbox.to_owned(), name.clone()))
            .collect();
        if let Some(key) = keys.iter().find(|key| held.contains_key(*key)) {
            return Err(HeldElsewhere {
                name: key.1.clone(),
                holder: held.get(key).cloned().unwrap_or_default(),
            });
        }
        for key in &keys {
            held.insert(key.clone(), holder.to_owned());
        }
        Ok(StateHold {
            holds: Arc::clone(self),
            keys,
        })
    }

    /// Holds every state of `names` in `sandbox` for `holder`, waiting while one is held;
    /// `waiting` is told each time the state waited for or its holder changes. The caller bounds
    /// and cancels the wait (it drops the future).
    pub(crate) async fn hold(
        self: &Arc<Self>,
        sandbox: &str,
        names: &[String],
        holder: &str,
        mut waiting: impl FnMut(&HeldElsewhere),
    ) -> StateHold {
        let mut told: Option<HeldElsewhere> = None;
        loop {
            // Registered before the attempt, so a release between the two is not missed.
            let released = self.released.notified();
            tokio::pin!(released);
            released.as_mut().enable();
            match self.try_hold(sandbox, names, holder) {
                Ok(hold) => return hold,
                Err(elsewhere) => {
                    if told.as_ref() != Some(&elsewhere) {
                        waiting(&elsewhere);
                        told = Some(elsewhere);
                    }
                    released.await;
                }
            }
        }
    }

    /// Who holds the state `name` of `sandbox`, if anyone.
    pub(crate) fn holder(&self, sandbox: &str, name: &str) -> Option<String> {
        self.held
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&(sandbox.to_owned(), name.to_owned()))
            .cloned()
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    fn names(list: &[&str]) -> Vec<String> {
        list.iter().map(|n| (*n).to_owned()).collect()
    }

    #[test]
    fn a_state_has_one_holder_and_a_set_is_held_whole_or_not_at_all() {
        let holds = Arc::new(StateHolds::default());
        let first = holds.try_hold("s1", &names(&["OPS"]), "dev run r1");
        assert!(first.is_ok());
        assert_eq!(holds.holder("s1", "OPS").as_deref(), Some("dev run r1"));
        let refused = holds
            .try_hold("s1", &names(&["OTHER", "OPS"]), "task #2")
            .err();
        assert_eq!(
            refused,
            Some(HeldElsewhere {
                name: "OPS".into(),
                holder: "dev run r1".into()
            })
        );
        assert_eq!(
            holds.holder("s1", "OTHER"),
            None,
            "nothing of a refused set"
        );
        assert!(
            holds.try_hold("s2", &names(&["OPS"]), "task #3").is_ok(),
            "another project's state of the same name"
        );
        drop(first);
        assert_eq!(holds.holder("s1", "OPS"), None);
        assert!(holds.try_hold("s1", &names(&["OPS"]), "task #2").is_ok());
    }

    #[tokio::test]
    async fn a_waiting_run_holds_once_the_holder_releases() {
        let holds = Arc::new(StateHolds::default());
        let first = holds.try_hold("s1", &names(&["A", "B"]), "dev run r1");
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let waiter = {
            let holds = Arc::clone(&holds);
            tokio::spawn(async move {
                let hold = holds
                    .hold("s1", &names(&["B", "A"]), "dev run r2", |elsewhere| {
                        let _ = tx.send(elsewhere.clone());
                    })
                    .await;
                drop(hold);
            })
        };
        let told = rx.recv().await;
        assert_eq!(
            told.map(|e| e.holder).as_deref(),
            Some("dev run r1"),
            "the run says what it waits for"
        );
        assert!(!waiter.is_finished());
        drop(first);
        assert!(
            tokio::time::timeout(Duration::from_secs(10), waiter)
                .await
                .is_ok(),
            "the release wakes the waiting run"
        );
        assert_eq!(holds.holder("s1", "A"), None);
    }
}
