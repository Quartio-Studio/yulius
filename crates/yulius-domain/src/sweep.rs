//! Sweeping Cargo build directories and the data disk's pressure (SPEC-276, ADR-0205): which
//! entries of a build directory form one unit, how full the disk is, and which units a sweep
//! deletes at each level.
//!
//! Pure: the caller walks the build directory, holds Cargo's locks and deletes.

const HOUR_MS: i64 = 60 * 60 * 1000;
/// A unit unread this long goes. The disk is mounted `relatime`: a read updates the access time
/// only when it is older than the modification time or than a day, so 48 h without a newer access
/// or modification time proves a day without a read.
pub const UNUSED_AFTER_MS: i64 = 48 * HOUR_MS;
/// The age that goes while the disk is low.
pub const LOW_UNUSED_AFTER_MS: i64 = 6 * HOUR_MS;
/// The age that goes while the disk is critical.
pub const CRITICAL_UNUSED_AFTER_MS: i64 = HOUR_MS;
/// A unit used this recently is never evicted for the cap (below the critical level): it belongs
/// to the current build.
pub const RECENT_MS: i64 = HOUR_MS;
/// Share of the data disk (percent) one build directory's units may hold.
pub const CAP_PERCENT: u64 = 5;
/// The smallest cap, for small disks.
pub const MIN_CAP: u64 = 8_000_000_000;
/// The cap when the disk's size is unknown.
pub const UNKNOWN_DISK_CAP: u64 = 20_000_000_000;
/// Used share of the data disk (percent) from which it is low.
pub const LOW_PERCENT: u64 = 85;
/// Used share of the data disk (percent) from which it is critical.
pub const CRITICAL_PERCENT: u64 = 95;

/// How full the data disk is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default)]
pub enum Level {
    /// Below [`LOW_PERCENT`] (or unknown).
    #[default]
    Ok,
    /// From [`LOW_PERCENT`]: sweeps run every round, harder.
    Low,
    /// From [`CRITICAL_PERCENT`]: idle caches go too.
    Critical,
}

impl Level {
    /// The level of a disk with `used` of `total` bytes.
    #[must_use]
    pub fn of(used: u64, total: u64) -> Self {
        if total == 0 {
            return Self::Ok;
        }
        let percent = u128::from(used) * 100;
        if percent >= u128::from(total) * u128::from(CRITICAL_PERCENT) {
            Self::Critical
        } else if percent >= u128::from(total) * u128::from(LOW_PERCENT) {
            Self::Low
        } else {
            Self::Ok
        }
    }

    /// Wire name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Low => "low",
            Self::Critical => "critical",
        }
    }
}

/// Bytes to free so that a critical disk with `used` of `total` bytes is back at [`LOW_PERCENT`];
/// 0 below [`CRITICAL_PERCENT`].
#[must_use]
pub fn bytes_to_relieve(used: u64, total: u64) -> u64 {
    if Level::of(used, total) != Level::Critical {
        return 0;
    }
    let target = u128::from(total) * u128::from(LOW_PERCENT) / 100;
    u64::try_from(u128::from(used).saturating_sub(target)).unwrap_or(u64::MAX)
}

/// Where in a profile directory an entry lies.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Area {
    /// `.fingerprint/<name>-<hash>`.
    Fingerprint,
    /// `build/<name>-<hash>` (a build script and its run's `out/`).
    Build,
    /// `deps/[lib]<name>-<hash>[.<ext>…]`.
    Deps,
    /// `incremental/<crate>-<hash>`.
    Incremental,
}

impl Area {
    /// Every area, with its directory's name.
    pub const ALL: [(Self, &'static str); 4] = [
        (Self::Fingerprint, ".fingerprint"),
        (Self::Build, "build"),
        (Self::Deps, "deps"),
        (Self::Incremental, "incremental"),
    ];
}

/// The unit the entry `name` of `area` belongs to: Cargo's 16-hex hash shared by a unit's
/// fingerprint, outputs and build script, or the incremental directory itself; `None` for an entry
/// that is no unit's (it is left alone).
#[must_use]
pub fn unit_key(area: Area, name: &str) -> Option<String> {
    if name.is_empty() || name.starts_with('.') {
        return None;
    }
    let stem = match area {
        Area::Incremental => return Some(format!("incremental/{name}")),
        Area::Deps => name.split('.').next().unwrap_or(name),
        Area::Fingerprint | Area::Build => name,
    };
    let (_, hash) = stem.rsplit_once('-')?;
    let hex = hash.len() == 16
        && hash
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b));
    hex.then(|| hash.to_owned())
}

/// Whether the entry `name` of an incremental directory is a session rustc is compiling now.
#[must_use]
pub fn is_working_session(name: &str) -> bool {
    name.starts_with("s-") && name.ends_with("-working")
}

/// One unit as the sweep found it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unit {
    /// Newest access or modification time of anything in it (ms).
    pub last_used_ms: i64,
    /// Allocated bytes.
    pub bytes: u64,
    /// It holds a session rustc is compiling (never deleted).
    pub working: bool,
}

/// What a sweep deletes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Policy {
    /// Units unused this long go.
    pub unused_after_ms: i64,
    /// The most a build directory's units may hold.
    pub cap: u64,
    /// Units used this recently stay whatever the cap.
    pub recent_ms: i64,
}

/// The cap of one build directory on a data disk of `disk_total` bytes: [`CAP_PERCENT`] of it, at
/// least [`MIN_CAP`].
#[must_use]
pub fn cap(disk_total: Option<u64>) -> u64 {
    match disk_total {
        Some(total) if total > 0 => (total / 100 * CAP_PERCENT).max(MIN_CAP),
        _ => UNKNOWN_DISK_CAP,
    }
}

/// The sweep's policy at `level` on a data disk of `disk_total` bytes.
#[must_use]
pub fn policy(level: Level, disk_total: Option<u64>) -> Policy {
    let cap = cap(disk_total);
    match level {
        Level::Ok => Policy {
            unused_after_ms: UNUSED_AFTER_MS,
            cap,
            recent_ms: RECENT_MS,
        },
        Level::Low => Policy {
            unused_after_ms: LOW_UNUSED_AFTER_MS,
            cap: cap / 2,
            recent_ms: RECENT_MS,
        },
        Level::Critical => Policy {
            unused_after_ms: CRITICAL_UNUSED_AFTER_MS,
            cap: cap / 4,
            recent_ms: 0,
        },
    }
}

/// Indices of the units to delete at `now_ms`: every unit unused for the policy's age, then, while
/// the rest holds more than the cap, the least recently used ones not used within its recent
/// floor until the rest holds three quarters of the cap. A working unit is never chosen.
#[must_use]
pub fn plan(units: &[Unit], now_ms: i64, policy: &Policy) -> Vec<usize> {
    let age = |unit: &Unit| now_ms.saturating_sub(unit.last_used_ms);
    let mut chosen: Vec<usize> = (0..units.len())
        .filter(|&i| !units[i].working && age(&units[i]) >= policy.unused_after_ms)
        .collect();
    let mut kept: u64 = (0..units.len())
        .filter(|i| !chosen.contains(i))
        .map(|i| units[i].bytes)
        .fold(0, u64::saturating_add);
    if kept > policy.cap {
        let goal = policy.cap / 4 * 3;
        let mut oldest: Vec<usize> = (0..units.len())
            .filter(|&i| {
                !chosen.contains(&i) && !units[i].working && age(&units[i]) >= policy.recent_ms
            })
            .collect();
        oldest.sort_by_key(|&i| units[i].last_used_ms);
        for i in oldest {
            if kept <= goal {
                break;
            }
            kept = kept.saturating_sub(units[i].bytes);
            chosen.push(i);
        }
    }
    chosen
}

#[cfg(test)]
mod tests {
    use super::*;

    const GB: u64 = 1_000_000_000;

    #[test]
    fn units_are_keyed_by_cargo_s_hash_and_incremental_directories_by_name() {
        let hash = Some("35f5dbd50d1a473d".to_owned());
        assert_eq!(unit_key(Area::Fingerprint, "adler2-35f5dbd50d1a473d"), hash);
        assert_eq!(unit_key(Area::Build, "adler2-35f5dbd50d1a473d"), hash);
        assert_eq!(
            unit_key(Area::Deps, "libadler2-35f5dbd50d1a473d.rlib"),
            hash
        );
        assert_eq!(unit_key(Area::Deps, "adler2-35f5dbd50d1a473d.d"), hash);
        assert_eq!(
            unit_key(Area::Deps, "yulius_app-35f5dbd50d1a473d"),
            hash,
            "a test binary"
        );
        assert_eq!(
            unit_key(Area::Deps, "foo-35f5dbd50d1a473d.foo.abc-cgu.0.rcgu.o"),
            hash
        );
        assert_eq!(unit_key(Area::Deps, "rmetaAbC123"), None, "rustc's temp");
        assert_eq!(unit_key(Area::Deps, "x-35F5DBD50D1A473D"), None);
        assert_eq!(unit_key(Area::Deps, "x-35f5dbd50d1a47"), None);
        assert_eq!(unit_key(Area::Fingerprint, ".lock"), None);
        assert_eq!(
            unit_key(Area::Incremental, "yulius_app-0u6e73f24ohkf"),
            Some("incremental/yulius_app-0u6e73f24ohkf".into())
        );
        assert!(is_working_session("s-hnghq0vh2y-16tnakq-working"));
        assert!(!is_working_session("s-hnghq0vh2y-16tnakq-1wmf7kj1angb1"));
        assert!(!is_working_session("s-hnghq0vh2y-16tnakq.lock"));
    }

    #[test]
    fn the_disk_is_low_from_85_and_critical_from_95_percent() {
        assert_eq!(Level::of(84, 100), Level::Ok);
        assert_eq!(Level::of(85, 100), Level::Low);
        assert_eq!(Level::of(94, 100), Level::Low);
        assert_eq!(Level::of(95, 100), Level::Critical);
        assert_eq!(Level::of(5, 0), Level::Ok, "unknown size");
        assert_eq!(bytes_to_relieve(94, 100), 0);
        assert_eq!(bytes_to_relieve(96, 100), 11, "back to 85 %");
        assert_eq!(Level::Critical.as_str(), "critical");
    }

    #[test]
    fn the_cap_is_five_percent_of_the_disk_and_shrinks_with_pressure() {
        assert_eq!(cap(Some(387 * GB)), 19_350_000_000);
        assert_eq!(cap(Some(40 * GB)), MIN_CAP, "small disk");
        assert_eq!(cap(None), UNKNOWN_DISK_CAP);
        let ok = policy(Level::Ok, Some(400 * GB));
        assert_eq!(
            (ok.unused_after_ms, ok.cap, ok.recent_ms),
            (UNUSED_AFTER_MS, 20 * GB, RECENT_MS)
        );
        let low = policy(Level::Low, Some(400 * GB));
        assert_eq!(
            (low.unused_after_ms, low.cap),
            (LOW_UNUSED_AFTER_MS, 10 * GB)
        );
        let critical = policy(Level::Critical, Some(400 * GB));
        assert_eq!(
            (critical.unused_after_ms, critical.cap, critical.recent_ms),
            (CRITICAL_UNUSED_AFTER_MS, 5 * GB, 0)
        );
    }

    fn unit(hours_ago: i64, gb: u64) -> Unit {
        Unit {
            last_used_ms: -hours_ago * HOUR_MS,
            bytes: gb * GB,
            working: false,
        }
    }

    #[test]
    fn units_unused_for_two_days_go_and_working_ones_never() {
        let policy = policy(Level::Ok, Some(1000 * GB));
        let mut working = unit(100, 1);
        working.working = true;
        let units = [unit(49, 1), unit(47, 1), working, unit(1, 1)];
        assert_eq!(plan(&units, 0, &policy), vec![0]);
    }

    #[test]
    fn over_the_cap_the_least_recently_used_go_down_to_three_quarters_but_not_the_current_build() {
        let policy = Policy {
            unused_after_ms: UNUSED_AFTER_MS,
            cap: 20 * GB,
            recent_ms: RECENT_MS,
        };
        // 30 GB, nothing two days old: 5 h, 3 h and 2 h old units go (oldest first) until 15 GB.
        let units = [unit(2, 5), unit(5, 5), unit(0, 10), unit(3, 5), unit(10, 5)];
        assert_eq!(plan(&units, 0, &policy), vec![4, 1, 3]);
        // Only the current build is over the cap: it stays.
        let fresh = [unit(0, 25)];
        assert!(plan(&fresh, 0, &policy).is_empty());
        // Under the cap nothing recent goes.
        assert!(plan(&[unit(10, 5)], 0, &policy).is_empty());
        // Critical: even the current build goes.
        let critical = Policy {
            recent_ms: 0,
            ..policy
        };
        assert_eq!(plan(&fresh, 0, &critical), vec![0]);
    }
}
