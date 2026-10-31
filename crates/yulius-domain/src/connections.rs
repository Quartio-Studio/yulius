//! Which connection gives way when a relay toward Yulius or the egress proxy is full (SPEC-355
//! SLR-001, SLR-002; ADR-0241).
//!
//! A full table admits a new connection by closing the one that has been idle longest (no byte
//! either way), if it has been idle for at least the floor. A leftover process that holds a
//! connection keeps it idle, while an agent's calls are short or active, so the agent gets through
//! without the table knowing whose a connection is. Time is the owner's clock in milliseconds.
//!
//! The egress proxy also tells each connection's owner (the run whose credential opened it) within
//! its key (a sandbox), SPEC-357, ADR-0243. When nothing was idle for the floor, a full key takes
//! the oldest connection of its busiest other owner, and a full table that of the busiest other
//! key, if that one holds at least [`MARGIN`] more than the asker; so one busy run cannot lock the
//! others of its sandbox out. Relays have a single owner, and this never applies to them.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

/// How many more connections than the asker an owner (or key) must hold to give one up while
/// active (SPEC-357 ESR-002): with 1, two owners would take from each other back and forth.
pub const MARGIN: usize = 2;

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

    /// The connection is in flight (a request waiting for its answer, whose bytes nobody
    /// counts): it never gives way until it ends.
    pub fn hold(&self) {
        self.0.store(u64::MAX, Ordering::Relaxed);
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
pub struct Evicted<K, T, O = ()> {
    /// Whose it was.
    pub key: K,
    /// Which owner of the key held it.
    pub owner: O,
    /// How long it had been idle (ms).
    pub idle_ms: u64,
    /// What ends it once dropped.
    pub payload: T,
    /// Taken while active from a busier owner (or key), which held this many: `None` when it
    /// gave way for being idle past the floor (SPEC-357 ESR-002).
    pub busier: Option<usize>,
}

/// An admitted connection.
#[derive(Debug)]
pub struct Admitted<K, T, O = ()> {
    /// Its place, for [`Connections::close`].
    pub id: u64,
    /// Its activity, for its copy loop to touch.
    pub activity: Activity,
    /// The connection that gave way to it, if the table was full.
    pub evicted: Option<Evicted<K, T, O>>,
}

#[derive(Debug)]
struct Open<K, O, T> {
    id: u64,
    key: K,
    owner: O,
    activity: Activity,
    payload: T,
}

impl<K, O, T> Open<K, O, T> {
    /// Whether it may be taken: a connection held in flight never is.
    fn takeable(&self) -> bool {
        self.activity.last() != u64::MAX
    }
}

/// Open connections by key and owner, at most `max` in all and `per_key` per key, in the order
/// they were admitted.
#[derive(Debug)]
pub struct Connections<K, T, O = ()> {
    max: usize,
    per_key: usize,
    floor_ms: u64,
    next: u64,
    open: Vec<Open<K, O, T>>,
}

impl<K: PartialEq, T> Connections<K, T> {
    /// A table of `max` places, `per_key` of them per key, whose connections give way once idle
    /// for `floor_ms`; each key has a single owner.
    #[must_use]
    pub fn new(max: usize, per_key: usize, floor_ms: u64) -> Self {
        Self::with_owners(max, per_key, floor_ms)
    }

    /// Admits a connection of `key` with a single owner, as [`Connections::admit_by`].
    ///
    /// # Errors
    /// The limit met, when no connection there was idle for the floor.
    pub fn admit(&mut self, key: K, payload: T, now_ms: u64) -> Result<Admitted<K, T>, Full> {
        self.admit_by(key, (), payload, now_ms)
    }
}

impl<K: PartialEq, T, O: PartialEq> Connections<K, T, O> {
    /// A table of `max` places, `per_key` of them per key, whose connections give way once idle
    /// for `floor_ms`, or else to a connection of a less busy owner (or key).
    #[must_use]
    pub fn with_owners(max: usize, per_key: usize, floor_ms: u64) -> Self {
        let max = max.max(1);
        Self {
            max,
            per_key: per_key.clamp(1, max),
            floor_ms,
            next: 0,
            open: Vec::new(),
        }
    }

    /// Admits a connection of `owner` within `key` at `now_ms` (`payload` ends it when dropped).
    /// Past the key's share, the key's longest-idle connection past the floor gives way, else the
    /// oldest of the key's busiest other owner holding [`MARGIN`] more than `owner`; past the
    /// table's places, the longest-idle of all past the floor, else the oldest of the busiest
    /// owner of the busiest other key holding [`MARGIN`] more than `key`. A connection held in
    /// flight never gives way.
    ///
    /// # Errors
    /// The limit met, when nothing there was idle for the floor and none was busier.
    pub fn admit_by(
        &mut self,
        key: K,
        owner: O,
        payload: T,
        now_ms: u64,
    ) -> Result<Admitted<K, T, O>, Full> {
        let evicted = if self.count(&key) >= self.per_key {
            // A key whose share is the whole table met the table's limit.
            let full = if self.per_key < self.max {
                Full::Key
            } else {
                Full::Total
            };
            let idle = self.evict(Some(&key), now_ms);
            Some(
                idle.or_else(|| self.take_from_busiest_owner(&key, &owner, now_ms))
                    .ok_or(full)?,
            )
        } else if self.open.len() >= self.max {
            let idle = self.evict(None, now_ms);
            Some(
                idle.or_else(|| self.take_from_busiest_key(&key, now_ms))
                    .ok_or(Full::Total)?,
            )
        } else {
            None
        };
        let id = self.next;
        self.next = self.next.wrapping_add(1);
        let activity = Activity::new(now_ms);
        self.open.push(Open {
            id,
            key,
            owner,
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
    fn evict(&mut self, key: Option<&K>, now_ms: u64) -> Option<Evicted<K, T, O>> {
        let (at, idle_ms) = self
            .open
            .iter()
            .enumerate()
            .filter(|(_, open)| key.is_none_or(|key| open.key == *key))
            // A connection held in flight never gives way, whatever the floor.
            .filter(|(_, open)| open.takeable())
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
            owner: open.owner,
            idle_ms,
            payload: open.payload,
            busier: None,
        })
    }

    /// Takes the oldest takeable connection of `key`'s busiest owner other than `owner`, if it
    /// holds at least [`MARGIN`] more than `owner` (SPEC-357 ESR-002).
    fn take_from_busiest_owner(
        &mut self,
        key: &K,
        owner: &O,
        now_ms: u64,
    ) -> Option<Evicted<K, T, O>> {
        let mine = self.count_of(key, owner);
        let (held, at) = self.busiest(
            |open| open.key == *key && open.owner != *owner,
            |a, b| a.key == b.key && a.owner == b.owner,
        )?;
        (held >= mine.saturating_add(MARGIN)).then(|| self.take(at, held, now_ms))
    }

    /// Takes the oldest takeable connection of the busiest owner of the busiest key other than
    /// `key`, if that key holds at least [`MARGIN`] more than `key` (SPEC-357 ESR-003).
    fn take_from_busiest_key(&mut self, key: &K, now_ms: u64) -> Option<Evicted<K, T, O>> {
        let mine = self.count(key);
        let (held, at) = self.busiest(|open| open.key != *key, |a, b| a.key == b.key)?;
        if held < mine.saturating_add(MARGIN) {
            return None;
        }
        let busiest = &self.open[at].key;
        let (_, at) = self.busiest(
            |open| open.key == *busiest,
            |a, b| a.key == b.key && a.owner == b.owner,
        )?;
        Some(self.take(at, held, now_ms))
    }

    /// Of the groups (`same`) of the connections that `pick` selects, the one with the most
    /// connections that has a takeable one, and where its oldest takeable one is. Of equals, the
    /// group whose first connection is the oldest.
    fn busiest(
        &self,
        pick: impl Fn(&Open<K, O, T>) -> bool,
        same: impl Fn(&Open<K, O, T>, &Open<K, O, T>) -> bool,
    ) -> Option<(usize, usize)> {
        let mut best: Option<(usize, usize)> = None;
        for (first, open) in self.open.iter().enumerate().filter(|(_, o)| pick(o)) {
            // Each group once, at its first (oldest) connection.
            if self.open[..first].iter().any(|o| pick(o) && same(o, open)) {
                continue;
            }
            let group = || {
                self.open
                    .iter()
                    .enumerate()
                    .filter(|(_, o)| pick(o) && same(o, open))
            };
            let Some((at, _)) = group().find(|(_, o)| o.takeable()) else {
                continue;
            };
            let held = group().count();
            if best.is_none_or(|(most, _)| held > most) {
                best = Some((held, at));
            }
        }
        best
    }

    /// Removes the connection at `at`, taken from a group holding `held`.
    fn take(&mut self, at: usize, held: usize, now_ms: u64) -> Evicted<K, T, O> {
        let open = self.open.remove(at);
        Evicted {
            idle_ms: now_ms.saturating_sub(open.activity.last()),
            key: open.key,
            owner: open.owner,
            payload: open.payload,
            busier: Some(held),
        }
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

    /// Open connections of `owner` within `key`.
    #[must_use]
    pub fn count_of(&self, key: &K, owner: &O) -> usize {
        self.open
            .iter()
            .filter(|open| open.key == *key && open.owner == *owner)
            .count()
    }

    /// The most open connections any owner of `key` other than `owner` holds.
    #[must_use]
    pub fn most_of_others(&self, key: &K, owner: &O) -> usize {
        self.open
            .iter()
            .filter(|open| open.key == *key && open.owner != *owner)
            .map(|open| self.count_of(key, &open.owner))
            .max()
            .unwrap_or(0)
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

    /// SLR-002 (review round 1): a connection held in flight (a plain request waiting for its
    /// answer) never gives way, however long it has carried no byte.
    #[test]
    fn a_held_connection_never_gives_way() -> TestResult {
        for floor in [10, 0] {
            let mut table = Connections::new(1, 1, floor);
            let held = table.admit((), "request", 0)?;
            held.activity.hold();
            assert_eq!(
                table.admit((), "new", 1_000_000).err(),
                Some(Full::Total),
                "floor {floor}"
            );
        }
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
        assert_eq!(
            (evicted.key, evicted.payload),
            ("b", 2),
            "b's own, not a's older one"
        );
        assert_eq!((table.count(&"a"), table.count(&"b")), (1, 2));
        table.admit("c", 5, 20)?;
        let evicted = table.admit("d", 6, 30)?.evicted.ok_or("none")?;
        assert_eq!(evicted.payload, 1, "the longest idle of all");
        // b's two connections are fresh: b is refused at its share.
        let mut fresh = Connections::new(4, 2, 10);
        fresh.admit("b", 1, 0)?;
        fresh.admit("b", 2, 0)?;
        assert_eq!(fresh.admit("b", 3, 5).err(), Some(Full::Key));
        assert!(
            fresh.admit("a", 4, 5).is_ok(),
            "another key keeps its places"
        );
        Ok(())
    }

    /// SPEC-357 ESR-002: at a key's share with every connection active, the busiest other owner
    /// gives up its oldest connection when it holds at least `MARGIN` more than the asker; an
    /// idle one past the floor still goes first; owners holding as many are refused.
    #[test]
    fn the_busiest_owner_gives_way_at_a_full_share() -> TestResult {
        let mut table = Connections::with_owners(8, 4, 1_000);
        for payload in 1..=4 {
            table.admit_by("s", "a", payload, 0)?;
        }
        let taken = table.admit_by("s", "b", 5, 10)?.evicted.ok_or("none")?;
        assert_eq!(
            (taken.owner, taken.payload, taken.busier, taken.idle_ms),
            ("a", 1, Some(4), 10)
        );
        assert_eq!(table.admit_by("s", "a", 6, 20).err(), Some(Full::Key));
        assert_eq!(
            (table.count_of(&"s", &"a"), table.most_of_others(&"s", &"a")),
            (3, 1)
        );
        let taken = table.admit_by("s", "b", 7, 30)?.evicted.ok_or("none")?;
        assert_eq!((taken.owner, taken.payload), ("a", 2));
        // 2 and 2: neither holds MARGIN more.
        assert_eq!(table.admit_by("s", "b", 8, 40).err(), Some(Full::Key));
        assert_eq!(table.admit_by("s", "a", 8, 40).err(), Some(Full::Key));
        // Past the floor, the longest idle goes first, even the asker's own.
        let idle = table.admit_by("s", "b", 9, 5_000)?.evicted.ok_or("none")?;
        assert_eq!((idle.payload, idle.busier), (3, None));
        Ok(())
    }

    /// SPEC-357 ESR-002: a connection held in flight is never taken; the busiest owner's oldest
    /// takeable one is.
    #[test]
    fn a_held_connection_is_never_taken_from_the_busiest() -> TestResult {
        let mut table = Connections::with_owners(8, 3, 1_000);
        let held = table.admit_by("s", "a", 1, 0)?;
        held.activity.hold();
        table.admit_by("s", "a", 2, 0)?;
        table.admit_by("s", "a", 3, 0)?;
        let taken = table.admit_by("s", "b", 4, 10)?.evicted.ok_or("none")?;
        assert_eq!(taken.payload, 2);
        let mut all_held = Connections::with_owners(8, 2, 0);
        for payload in 1..=2 {
            all_held.admit_by("s", "a", payload, 0)?.activity.hold();
        }
        assert_eq!(all_held.admit_by("s", "b", 3, 10).err(), Some(Full::Key));
        Ok(())
    }

    /// SPEC-357 ESR-003: at the table's limit with every connection active, the busiest other key
    /// gives up its busiest owner's oldest connection; of equals, the first.
    #[test]
    fn the_busiest_key_gives_way_at_a_full_table() -> TestResult {
        let mut table = Connections::with_owners(4, 3, 1_000);
        table.admit_by("s1", "a", 1, 0)?;
        table.admit_by("s1", "b", 2, 0)?;
        table.admit_by("s1", "b", 3, 0)?;
        table.admit_by("s2", "x", 4, 0)?;
        let taken = table.admit_by("s2", "y", 5, 10)?.evicted.ok_or("none")?;
        assert_eq!(
            (taken.key, taken.owner, taken.payload, taken.busier),
            ("s1", "b", 2, Some(3))
        );
        // s1 and s2 hold 2 each: a third key takes from the first of them.
        let taken = table.admit_by("s3", "z", 6, 20)?.evicted.ok_or("none")?;
        assert_eq!((taken.key, taken.payload), ("s1", 1));
        // s2 holds 2, s1 and s3 1 each: only a key holding none is MARGIN behind s2.
        assert_eq!(table.admit_by("s1", "a", 7, 30).err(), Some(Full::Total));
        let taken = table.admit_by("s4", "w", 8, 30)?.evicted.ok_or("none")?;
        assert_eq!((taken.key, taken.payload), ("s2", 4));
        assert_eq!(table.admit_by("s5", "v", 9, 40).err(), Some(Full::Total));
        Ok(())
    }
}
