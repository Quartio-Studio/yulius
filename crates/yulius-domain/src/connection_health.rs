//! Connection troubles: a limit of the egress proxy or of a sandbox's relay toward Yulius that
//! refuses connections (SPEC-361, ADR-0246). The proxy and the server's relays report each
//! refusal and each freed place; the app reads the open troubles and is told when one begins or
//! ends, so the people of the sandbox see it once per trouble rather than once per refusal.

use std::sync::{Mutex, OnceLock, PoisonError};

/// The busiest holders a trouble keeps.
pub const HOLDERS: usize = 3;

/// The limit that refuses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Limit {
    /// A sandbox's share of the egress proxy (SPEC-353 LAP-002).
    Share,
    /// A sandbox's relay toward Yulius, the server's `out.sock` (SPEC-355 SLR-001).
    Relay,
    /// The egress proxy's own total.
    Proxy,
}

impl Limit {
    /// Its name on the wire: `share`, `relay` or `proxy`.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Share => "share",
            Self::Relay => "relay",
            Self::Proxy => "proxy",
        }
    }
}

/// Who holds connections at a limit: a run of the sandbox (its `EgressClient::use_key`) at a
/// share, a sandbox (its id) at the proxy's total.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Holder {
    /// The run's key or the sandbox's id.
    pub name: String,
    /// How many connections it holds there.
    pub held: usize,
}

/// The busiest [`HOLDERS`] of `counts`, busiest first; of equals, the first given.
#[must_use]
pub fn busiest(counts: impl IntoIterator<Item = (String, usize)>) -> Vec<Holder> {
    let mut holders: Vec<Holder> = counts
        .into_iter()
        .map(|(name, held)| Holder { name, held })
        .collect();
    holders.sort_by_key(|holder| std::cmp::Reverse(holder.held));
    holders.truncate(HOLDERS);
    holders
}

/// How many connections a limit of `max` holds at most once its trouble ends: seven eighths, so
/// a limit that stays busy does not begin and end a trouble with each connection (CHR-002).
#[must_use]
pub fn room_at(max: usize) -> usize {
    max.saturating_sub(max.div_ceil(8))
}

/// An open trouble.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Trouble {
    /// Unique within the process: a notification is kept per id.
    pub id: u64,
    /// What refuses.
    pub limit: Limit,
    /// Its sandbox's id; `None` for the proxy's own total.
    pub sandbox: Option<String>,
    /// When it began (ms since the epoch).
    pub since_ms: i64,
    /// Connections held at its last refusal.
    pub held: usize,
    /// The limit.
    pub max: usize,
    /// Refusals since it began.
    pub refused: u64,
    /// The busiest holders at its last refusal, busiest first (none for a relay).
    pub holders: Vec<Holder>,
}

/// A refusal at a limit, as its adapter reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refusal {
    /// What refused.
    pub limit: Limit,
    /// The sandbox; `None` for the proxy's own total.
    pub sandbox: Option<String>,
    /// Connections held there.
    pub held: usize,
    /// The limit.
    pub max: usize,
    /// The busiest holders ([`busiest`]).
    pub holders: Vec<Holder>,
}

/// Told when a trouble begins (`true`) or ends (`false`), after the registry's lock is released.
type OnChange = Box<dyn Fn(&Trouble, bool) + Send + Sync>;

/// The open troubles, at most one per limit and sandbox.
#[derive(Default)]
pub struct ConnectionHealth {
    inner: Mutex<Inner>,
    on_change: OnceLock<OnChange>,
}

#[derive(Default)]
struct Inner {
    next: u64,
    open: Vec<Trouble>,
}

impl std::fmt::Debug for ConnectionHealth {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ConnectionHealth").finish_non_exhaustive()
    }
}

impl ConnectionHealth {
    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn tell(&self, trouble: &Trouble, began: bool) {
        if let Some(on_change) = self.on_change.get() {
            on_change(trouble, began);
        }
    }

    /// Sets what is told when a trouble begins or ends (once); `false` when one was set.
    pub fn on_change(&self, f: impl Fn(&Trouble, bool) + Send + Sync + 'static) -> bool {
        self.on_change.set(Box::new(f)).is_ok()
    }

    /// A connection was refused at `refusal`'s limit at `now_ms`: begins its trouble (told), or
    /// updates the open one's counts (not told).
    pub fn refused(&self, refusal: Refusal, now_ms: i64) {
        let began = {
            let mut inner = self.lock();
            if let Some(open) = inner
                .open
                .iter_mut()
                .find(|t| t.limit == refusal.limit && t.sandbox == refusal.sandbox)
            {
                open.held = refusal.held;
                open.max = refusal.max;
                open.refused = open.refused.saturating_add(1);
                open.holders = refusal.holders;
                return;
            }
            let trouble = Trouble {
                id: inner.next,
                limit: refusal.limit,
                sandbox: refusal.sandbox,
                since_ms: now_ms,
                held: refusal.held,
                max: refusal.max,
                refused: 1,
                holders: refusal.holders,
            };
            inner.next = inner.next.wrapping_add(1);
            inner.open.push(trouble.clone());
            trouble
        };
        self.tell(&began, true);
    }

    /// Whether `limit` of `sandbox` has an open trouble: an adapter counts its connections only
    /// then.
    #[must_use]
    pub fn open(&self, limit: Limit, sandbox: Option<&str>) -> bool {
        self.lock()
            .open
            .iter()
            .any(|t| t.limit == limit && t.sandbox.as_deref() == sandbox)
    }

    /// `limit` of `sandbox` now holds `held` connections: ends its trouble (told) once that is
    /// at most [`room_at`] its limit.
    pub fn held(&self, limit: Limit, sandbox: Option<&str>, held: usize) {
        let ended = {
            let mut inner = self.lock();
            let Some(at) = inner.open.iter().position(|t| {
                t.limit == limit && t.sandbox.as_deref() == sandbox && held <= room_at(t.max)
            }) else {
                return;
            };
            inner.open.remove(at)
        };
        self.tell(&ended, false);
    }

    /// The open troubles, oldest first.
    #[must_use]
    pub fn troubles(&self) -> Vec<Trouble> {
        self.lock().open.clone()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;

    fn share(sandbox: &str, held: usize, holders: &[(&str, usize)]) -> Refusal {
        Refusal {
            limit: Limit::Share,
            sandbox: Some(sandbox.to_owned()),
            held,
            max: 128,
            holders: busiest(holders.iter().map(|(n, c)| ((*n).to_owned(), *c))),
        }
    }

    type Told = Arc<Mutex<Vec<(u64, bool)>>>;

    fn told(health: &ConnectionHealth) -> Told {
        let told = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&told);
        assert!(health.on_change(move |t, began| {
            sink.lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push((t.id, began));
        }));
        told
    }

    fn seen(told: &Told) -> Vec<(u64, bool)> {
        told.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }

    #[test]
    fn a_refusal_begins_a_trouble_once_and_later_ones_update_it() {
        let health = ConnectionHealth::default();
        let told = told(&health);
        health.refused(share("s1", 128, &[("run-a", 120), ("run-b", 8)]), 1_000);
        health.refused(share("s1", 128, &[("run-a", 121), ("run-b", 7)]), 2_000);
        let troubles = health.troubles();
        assert_eq!(troubles.len(), 1);
        let trouble = &troubles[0];
        assert_eq!(trouble.since_ms, 1_000);
        assert_eq!(trouble.refused, 2);
        assert_eq!(
            trouble.holders[0],
            Holder {
                name: "run-a".into(),
                held: 121
            }
        );
        assert_eq!(seen(&told), vec![(trouble.id, true)]);
        assert!(health.open(Limit::Share, Some("s1")));
        assert!(!health.open(Limit::Share, Some("s2")));
        assert!(!health.open(Limit::Relay, Some("s1")));
    }

    #[test]
    fn a_trouble_ends_only_with_room_and_a_later_refusal_begins_another() {
        let health = ConnectionHealth::default();
        let told = told(&health);
        health.refused(share("s1", 128, &[("run-a", 128)]), 1_000);
        let first = health.troubles()[0].id;
        // One free place is not room yet: a busy share would flicker.
        health.held(Limit::Share, Some("s1"), 127);
        health.held(Limit::Share, Some("s1"), room_at(128) + 1);
        assert_eq!(health.troubles().len(), 1);
        // Another sandbox's or limit's count changes nothing.
        health.held(Limit::Share, Some("s2"), 0);
        health.held(Limit::Relay, Some("s1"), 0);
        assert_eq!(health.troubles().len(), 1);
        health.held(Limit::Share, Some("s1"), room_at(128));
        assert!(health.troubles().is_empty());
        health.refused(share("s1", 128, &[("run-a", 128)]), 5_000);
        let second = health.troubles()[0].id;
        assert_ne!(first, second);
        assert_eq!(
            seen(&told),
            vec![(first, true), (first, false), (second, true)]
        );
    }

    #[test]
    fn troubles_are_kept_per_limit_and_sandbox() {
        let health = ConnectionHealth::default();
        health.refused(share("s1", 128, &[]), 1);
        health.refused(share("s2", 128, &[]), 2);
        health.refused(
            Refusal {
                limit: Limit::Relay,
                sandbox: Some("s1".into()),
                held: 256,
                max: 256,
                holders: Vec::new(),
            },
            3,
        );
        health.refused(
            Refusal {
                limit: Limit::Proxy,
                sandbox: None,
                held: 256,
                max: 256,
                holders: busiest([("s1".to_owned(), 128), ("s2".to_owned(), 128)]),
            },
            4,
        );
        let kinds: Vec<_> = health
            .troubles()
            .iter()
            .map(|t| (t.limit, t.sandbox.clone()))
            .collect();
        assert_eq!(
            kinds,
            vec![
                (Limit::Share, Some("s1".into())),
                (Limit::Share, Some("s2".into())),
                (Limit::Relay, Some("s1".into())),
                (Limit::Proxy, None),
            ]
        );
        health.held(Limit::Proxy, None, 0);
        assert!(!health.open(Limit::Proxy, None));
        assert!(health.open(Limit::Share, Some("s1")));
    }

    #[test]
    fn the_busiest_holders_come_first_and_are_bounded() {
        let holders = busiest([
            ("a".to_owned(), 2),
            ("b".to_owned(), 9),
            ("c".to_owned(), 2),
            ("d".to_owned(), 5),
            ("e".to_owned(), 1),
        ]);
        let names: Vec<_> = holders.iter().map(|h| (h.name.as_str(), h.held)).collect();
        assert_eq!(names, vec![("b", 9), ("d", 5), ("a", 2)]);
        assert_eq!(room_at(128), 112);
        assert_eq!(room_at(256), 224);
        assert_eq!(room_at(8), 7);
        assert_eq!(room_at(1), 0);
    }
}
