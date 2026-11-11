//! The instance-wide CI job slots: a fair semaphore whose size changes while jobs hold slots
//! (SPEC-459 HSL-006). Growing frees slots at once; shrinking takes the free ones and then each
//! slot as its job ends, so no running job is touched and the waiters keep their order.

use std::sync::{Arc, Mutex, PoisonError};

use tokio::sync::{OwnedSemaphorePermit, Semaphore};

pub(crate) struct JobSlots {
    semaphore: Arc<Semaphore>,
    sizing: Mutex<Sizing>,
}

struct Sizing {
    size: usize,
    /// Slots still to take away: held by running jobs when the size shrank.
    owed: usize,
}

/// One job's slot, returned when dropped.
pub(crate) struct JobSlot {
    permit: Option<OwnedSemaphorePermit>,
    slots: Arc<JobSlots>,
}

impl JobSlots {
    pub(crate) fn new(size: usize) -> Arc<Self> {
        let size = size.max(1);
        Arc::new(Self {
            semaphore: Arc::new(Semaphore::new(size)),
            sizing: Mutex::new(Sizing { size, owed: 0 }),
        })
    }

    /// Waits for a free slot, first come first served.
    pub(crate) async fn acquire(self: &Arc<Self>) -> Option<JobSlot> {
        let permit = Arc::clone(&self.semaphore).acquire_owned().await.ok()?;
        Some(JobSlot {
            permit: Some(permit),
            slots: Arc::clone(self),
        })
    }

    /// Makes it `size` slots (at least one).
    pub(crate) fn resize(&self, size: usize) {
        let size = size.max(1);
        let mut sizing = self.sizing.lock().unwrap_or_else(PoisonError::into_inner);
        if size > sizing.size {
            let more = size - sizing.size;
            // Slots owed from an earlier shrink are forgiven before new ones are added.
            let forgiven = more.min(sizing.owed);
            sizing.owed -= forgiven;
            self.semaphore.add_permits(more - forgiven);
        } else {
            let fewer = sizing.size - size;
            let taken = self.semaphore.forget_permits(fewer);
            sizing.owed += fewer - taken;
        }
        sizing.size = size;
    }
}

impl Drop for JobSlot {
    fn drop(&mut self) {
        let Some(permit) = self.permit.take() else {
            return;
        };
        let mut sizing = self
            .slots
            .sizing
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if sizing.owed > 0 {
            sizing.owed -= 1;
            permit.forget();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A slot if one is free right now.
    fn free(slots: &Arc<JobSlots>) -> Option<JobSlot> {
        let permit = Arc::clone(&slots.semaphore).try_acquire_owned().ok()?;
        Some(JobSlot {
            permit: Some(permit),
            slots: Arc::clone(slots),
        })
    }

    #[tokio::test]
    async fn growing_frees_slots_at_once() {
        let slots = JobSlots::new(1);
        let _held = slots.acquire().await;
        assert!(free(&slots).is_none());
        slots.resize(3);
        let second = free(&slots);
        let third = free(&slots);
        assert!(second.is_some() && third.is_some());
        assert!(free(&slots).is_none(), "three slots, not more");
    }

    #[tokio::test]
    async fn shrinking_takes_free_slots_at_once() {
        let slots = JobSlots::new(4);
        let _held = slots.acquire().await;
        slots.resize(2);
        let second = free(&slots);
        assert!(second.is_some());
        assert!(free(&slots).is_none(), "two slots, not four");
    }

    #[tokio::test]
    async fn shrinking_below_the_running_jobs_waits_for_them_to_end() {
        let slots = JobSlots::new(3);
        let a = slots.acquire().await;
        let b = slots.acquire().await;
        let c = slots.acquire().await;
        slots.resize(1);
        // Two jobs end: their slots are gone, the third still holds the only one.
        drop(a);
        drop(b);
        assert!(free(&slots).is_none());
        drop(c);
        let only = free(&slots);
        assert!(only.is_some());
        assert!(free(&slots).is_none());
    }

    #[tokio::test]
    async fn growing_again_forgives_what_a_shrink_still_owed() {
        let slots = JobSlots::new(3);
        let held: Vec<_> = [
            slots.acquire().await,
            slots.acquire().await,
            slots.acquire().await,
        ]
        .into();
        slots.resize(1);
        slots.resize(4);
        let fourth = free(&slots);
        assert!(fourth.is_some());
        assert!(free(&slots).is_none());
        drop(held);
        let all: Vec<_> = [free(&slots), free(&slots), free(&slots)].into();
        assert!(
            all.iter().all(Option::is_some),
            "four slots after the jobs end"
        );
        assert!(free(&slots).is_none());
    }

    #[tokio::test]
    async fn a_size_of_zero_is_one_slot() {
        let slots = JobSlots::new(0);
        slots.resize(0);
        let only = free(&slots);
        assert!(only.is_some());
        assert!(free(&slots).is_none());
    }
}
