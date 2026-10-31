//! Which connection gives way when a relay toward Yulius or the egress proxy is full (SPEC-355
//! SLR-001, SLR-002; ADR-0241).
//!
//! A full table admits a new connection by closing the one that has been idle longest (no byte
//! either way), if it has been idle for at least the floor. A leftover process that holds a
//! connection keeps it idle, while an agent's calls are short or active, so the agent gets through
//! without the table knowing whose a connection is. Time is the owner's clock in milliseconds.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

/// When a connection last carried a byte (ms on the owner's clock), shared with its copy loop.
#[derive(Debug, Clone, Default)]
pub struct Activity(Arc<AtomicU64>);

impl Activity {
    /// Active at `now_ms`.
    #[must_use]
    pub fn new(now_ms: u64) -> Self {
        Self(Arc::new(AtomicU64::new(now_ms)))
    }

    /// The connection carried a byte at `now_ms`.
    pub fn touch(&self, now_ms: u64) {
        self.0.fetch_max(now_ms, Ordering::Relaxed);
    }

    /// When it last carried a byte.
    #[must_use]
    pub fn last(&self) -> u64 {
        self.0.load(Ordering::Relaxed)
    }
}

/// The limit a refused connection met.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum Full {
    /// Its key (a sandbox) holds all of its share, each active within the floor.
    #[error("every connection of its share was active lately")]
    Key,
    /// The table holds all of its places, each active within the floor.
    #[error("every connection was active lately")]
    Total,
}

/// A connection the table closed to admit another: its payload is dropped by the caller, which
/// ends it.
#[derive(Debug)]
pub struct Evicted<K, T> {
    /// Whose it was.
    pub key: K,
    /// How long it had been idle (ms).
    pub idle_ms: u64,
    /// What ends it once dropped.
    pub payload: T,
}

/// An admitted connection.
#[derive(Debug)]
pub struct Admitted<K, T> {
    /// Its place, for [`Connections::close`].
    pub id: u64,
    /// Its activity, for its copy loop to touch.
    pub activity: Activity,
    /// The connection that gave way to it, if the table was full.
    pub evicted: Option<Evicted<K, T>>,
}

#[derive(Debug)]
struct Open<K, T> {
    id: u64,
    key: K,
    activity: Activity,
    payload: T,
}

/// Open connections by key, at most `max` in all and `per_key` per key.
#[derive(Debug)]
pub struct Connections<K, T> {
    max: usize,
    per_key: usize,
    floor_ms: u64,
    next: u64,
    open: Vec<Open<K, T>>,
}

impl<K: PartialEq, T> Connections<K, T> {
    /// A table of `max` places, `per_key` of them per key, whose connections give way once idle
    /// for `floor_ms`.
    #[must_use]
    pub fn new(max: usize, per_key: usize, floor_ms: u64) -> Self {
        let max = max.max(1);
        Self {
            max,
            per_key: per_key.clamp(1, max),
            floor_ms,
            next: 0,
            open: Vec::new(),
        }
    }

    /// Admits a connection of `key` at `now_ms` (`payload` ends it when dropped). Past the key's
    /// share, the key's longest-idle connection gives way; past the table's places, the
    /// longest-idle of all; one idle for less than the floor never does.
    ///
    /// # Errors
    /// The limit met, when no connection there was idle for the floor.
    pub fn admit(
        &mut self,
        key: K,
        payload: T,
        now_ms: u64,
    ) -> Result<Admitted<K, T>, Full> {
        let evicted = if self.count(&key) >= self.per_key {
            // A key whose share is the whole table met the table's limit.
            let full = if self.per_key < self.max {
                Full::Key
            } else {
                Full::Total
            };
            Some(self.evict(Some(&key), now_ms).ok_or(full)?)
        } else if self.open.len() >= self.max {
            Some(self.evict(None, now_ms).ok_or(Full::Total)?)
        } else {
            None
        };
        let id = self.next;
        self.next = self.next.wrapping_add(1);
        let activity = Activity::new(now_ms);
        self.open.push(Open {
            id,
            key,
            activity: activity.clone(),
            payload,
        });
        Ok(Admitted {
            id,
            activity,
            evicted,
        })
    }

    /// Removes the longest-idle connection (of `key`, or of all) idle for the floor.
    fn evict(&mut self, key: Option<&K>, now_ms: u64) -> Option<Evicted<K, T>> {
        let (at, idle_ms) = self
            .open
            .iter()
            .enumerate()
            .filter(|(_, open)| key.is_none_or(|key| open.key == *key))
            .map(|(at, open)| (at, now_ms.saturating_sub(open.activity.last())))
            .filter(|(_, idle)| *idle >= self.floor_ms)
            // The longest idle; of equals, the oldest (the first in the list).
            .fold(None, |best: Option<(usize, u64)>, (at, idle)| match best {
                Some((_, longest)) if longest >= idle => best,
                _ => Some((at, idle)),
            })?;
        let open = self.open.remove(at);
        Some(Evicted {
            key: open.key,
            idle_ms,
            payload: open.payload,
        })
    }

    /// Frees connection `id`'s place (it ended); `false` when it was not open (it gave way).
    pub fn close(&mut self, id: u64) -> bool {
        let before = self.open.len();
        self.open.retain(|open| open.id != id);
        self.open.len() < before
    }

    /// Open connections.
    #[must_use]
    pub fn len(&self) -> usize {
        self.open.len()
    }

    /// Whether none is open.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.open.is_empty()
    }

    /// Open connections of `key`.
    #[must_use]
    pub fn count(&self, key: &K) -> usize {
        self.open.iter().filter(|open| open.key == *key).count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    /// SLR-001: free places admit; a full table closes the longest-idle connection past the
    /// floor, never one active within it.
    #[test]
    fn a_full_table_closes_the_longest_idle_connection_past_the_floor() -> TestResult {
        let mut table = Connections::new(3, 3, 60_000);
        let first = table.admit((), "first", 0)?;
        let second = table.admit((), "second", 1_000)?;
        let third = table.admit((), "third", 2_000)?;
        assert!(first.evicted.is_none() && third.evicted.is_none());
        // Within the floor: refused.
        assert_eq!(table.admit((), "agent", 59_999).err(), Some(Full::Total));
        // The first one carried a byte lately: the second is the longest idle.
        first.activity.touch(50_000);
        let agent = table.admit((), "agent", 61_000)?;
        let evicted = agent.evicted.ok_or("nothing gave way")?;
        assert_eq!((evicted.payload, evicted.idle_ms), ("second", 60_000));
        assert_eq!(table.len(), 3);
        assert!(!table.close(second.id), "it gave way already");
        assert!(table.close(third.id));
        assert_eq!(table.len(), 2);
        assert!(table.admit((), "free", 61_000)?.evicted.is_none());
        Ok(())
    }

    /// SLR-002: a key past its share gives way with its own connections, never another key's;
    /// a full table takes the longest-idle of any key.
    #[test]
    fn a_key_past_its_share_gives_way_with_its_own() -> TestResult {
        let mut table = Connections::new(4, 2, 10);
        table.admit("a", 1, 0)?;
        table.admit("b", 2, 0)?;
        table.admit("b", 3, 5)?;
        let evicted = table.admit("b", 4, 20)?.evicted.ok_or("none")?;
        assert_eq!((evicted.key, evicted.payload), ("b", 2), "b's own, not a's older one");
        assert_eq!((table.count(&"a"), table.count(&"b")), (1, 2));
        table.admit("c", 5, 20)?;
        let evicted = table.admit("d", 6, 30)?.evicted.ok_or("none")?;
        assert_eq!(evicted.payload, 1, "the longest idle of all");
        // b's two connections are fresh: b is refused at its share.
        let mut fresh = Connections::new(4, 2, 10);
        fresh.admit("b", 1, 0)?;
        fresh.admit("b", 2, 0)?;
        assert_eq!(fresh.admit("b", 3, 5).err(), Some(Full::Key));
        assert!(fresh.admit("a", 4, 5).is_ok(), "another key keeps its places");
        Ok(())
    }
}
