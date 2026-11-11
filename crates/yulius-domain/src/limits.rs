//! The instance's resource limits, sized from the host (SPEC-459, ADR-0294).
//!
//! Six numbers decide how much work a host takes: live sessions, CI jobs at once, CI runs at once
//! per sandbox, a sandbox's memory, its soft disk quota and the disk bound of a CD run. Without a
//! pin each is computed from the host's CPUs, memory and data disk ([`Limit::host_sized`]). A pin
//! is a number from the unit (`YULIUS_*`) or from Settings; it is recorded with the host it was
//! configured on ([`Pin`]) and no longer fits once that host and this one differ in a fact the
//! limit's formula uses ([`Pin::stale`]).
//!
//! Pure: the caller measures the host, reads the unit and keeps the records.

use std::ops::RangeInclusive;

const GIB: u64 = 1 << 30;
const GB: u64 = 1_000_000_000;

/// What a host has, as far as it reports it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Host {
    /// Logical CPUs.
    pub cpus: Option<u32>,
    /// Memory in bytes.
    pub memory: Option<u64>,
    /// Size in bytes of the filesystem that holds the data directory.
    pub disk: Option<u64>,
}

/// One of the instance's resource limits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Limit {
    /// Live agent sessions (`YULIUS_MAX_SESSIONS`).
    Sessions,
    /// CI jobs at once (`YULIUS_CI_MAX_JOBS`).
    CiJobs,
    /// CI runs at once per sandbox (`YULIUS_CI_LANES`).
    CiLanes,
    /// A sandbox's memory in GiB (`YULIUS_SANDBOX_MEMORY_GB`).
    SandboxMemory,
    /// A sandbox's soft disk quota in GB, 0 for none (`YULIUS_SANDBOX_DISK_GB`).
    SandboxDisk,
    /// The disk bound of a CD or environment run in GB (`YULIUS_CD_RUN_DISK_GB`).
    CdRunDisk,
}

/// What Settings holds for a limit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Setting {
    /// The host-sized value, whatever the unit says.
    HostSized,
    /// This number.
    Value(u64),
}

/// Where a limit's value in force comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// The host-sized value.
    Default,
    /// The unit's `YULIUS_*` value.
    Unit,
    /// A number saved in Settings.
    Settings,
}

impl Source {
    /// Its name in the API.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Default => "default",
            Self::Unit => "unit",
            Self::Settings => "settings",
        }
    }
}

/// A limit's value in force.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Resolved {
    /// The value, in the limit's unit.
    pub value: u64,
    /// Where it comes from.
    pub source: Source,
}

/// A pinned number with the host it was configured on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Pin {
    /// The number.
    pub value: u64,
    /// The host then.
    pub host: Host,
}

/// A number a limit cannot take.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{label} must be between {min} and {max}{unit}")]
pub struct OutOfRange {
    label: &'static str,
    min: u64,
    max: u64,
    unit: String,
}

impl Limit {
    /// Every limit, in the order they are shown.
    pub const ALL: [Self; 6] = [
        Self::Sessions,
        Self::CiJobs,
        Self::CiLanes,
        Self::SandboxMemory,
        Self::SandboxDisk,
        Self::CdRunDisk,
    ];

    /// Its key in the API and in the saved record.
    #[must_use]
    pub const fn key(self) -> &'static str {
        match self {
            Self::Sessions => "max_sessions",
            Self::CiJobs => "ci_max_jobs",
            Self::CiLanes => "ci_lanes",
            Self::SandboxMemory => "sandbox_memory_gb",
            Self::SandboxDisk => "sandbox_disk_gb",
            Self::CdRunDisk => "cd_run_disk_gb",
        }
    }

    /// The limit with this key.
    #[must_use]
    pub fn from_key(key: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|limit| limit.key() == key)
    }

    /// The unit's variable.
    #[must_use]
    pub const fn env(self) -> &'static str {
        match self {
            Self::Sessions => "YULIUS_MAX_SESSIONS",
            Self::CiJobs => "YULIUS_CI_MAX_JOBS",
            Self::CiLanes => "YULIUS_CI_LANES",
            Self::SandboxMemory => "YULIUS_SANDBOX_MEMORY_GB",
            Self::SandboxDisk => "YULIUS_SANDBOX_DISK_GB",
            Self::CdRunDisk => "YULIUS_CD_RUN_DISK_GB",
        }
    }

    /// Its name for a person.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Sessions => "Live sessions",
            Self::CiJobs => "CI jobs at once",
            Self::CiLanes => "CI runs at once per sandbox",
            Self::SandboxMemory => "Sandbox memory",
            Self::SandboxDisk => "Sandbox disk quota",
            Self::CdRunDisk => "CD run disk",
        }
    }

    /// The unit of its value: empty for a count.
    #[must_use]
    pub const fn unit(self) -> &'static str {
        match self {
            Self::Sessions | Self::CiJobs | Self::CiLanes => "",
            Self::SandboxMemory => "GiB",
            Self::SandboxDisk | Self::CdRunDisk => "GB",
        }
    }

    /// The host-sized formula in words.
    #[must_use]
    pub const fn formula(self) -> &'static str {
        match self {
            Self::Sessions => "half the CPUs plus 2, at most one per 2 GiB of memory, at least 2",
            Self::CiJobs => "a sixth of the CPUs plus 2, at most 64",
            Self::CiLanes => "one per 32 CPUs plus 2, at most 8",
            Self::SandboxMemory => {
                "the host's memory minus an eighth (at least 4 GiB kept back), at least 2 GiB"
            }
            Self::SandboxDisk => {
                "a quarter of the data disk, at least 100 GB, at most half the disk"
            }
            Self::CdRunDisk => {
                "a twentieth of the data disk, at least 20 GB, at most half the disk"
            }
        }
    }

    /// Whether a change applies while the server runs; the sandbox memory waits for a restart
    /// (each sandbox's `memory.max` and the CI budget pools are sized at start).
    #[must_use]
    pub const fn live(self) -> bool {
        !matches!(self, Self::SandboxMemory)
    }

    /// The value on a host that does not report the facts the formula needs: the constant every
    /// host had before SPEC-459.
    #[must_use]
    pub const fn fixed(self) -> u64 {
        match self {
            Self::Sessions => 8,
            Self::CiJobs => 4,
            Self::CiLanes | Self::SandboxMemory => 2,
            Self::SandboxDisk => 100,
            Self::CdRunDisk => 20,
        }
    }

    /// The value `host` gets without a pin.
    #[must_use]
    pub fn host_sized(self, host: Host) -> u64 {
        let cpus = host.cpus.map(u64::from);
        let disk = host.disk.map(|bytes| bytes / GB);
        // A share of the disk, at least `floor` GB, never more than half the disk (and never 0).
        let of_disk =
            |share: u64, floor: u64| disk.map(|gb| (gb / share).max(floor).min(gb / 2).max(1));
        let sized = match self {
            Self::Sessions => cpus
                .zip(host.memory)
                .map(|(cpus, memory)| (cpus / 2 + 2).min(memory / GIB / 2).max(2)),
            Self::CiJobs => cpus.map(|cpus| (cpus / 6 + 2).min(64)),
            Self::CiLanes => cpus.map(|cpus| (cpus / 32 + 2).min(8)),
            Self::SandboxMemory => host.memory.map(|memory| sandbox_memory(memory) / GIB),
            Self::SandboxDisk => of_disk(4, 100),
            Self::CdRunDisk => of_disk(20, 20),
        };
        sized.unwrap_or_else(|| self.fixed())
    }

    /// The numbers a pin may take on `host`.
    #[must_use]
    pub fn range(self, host: Host) -> RangeInclusive<u64> {
        match self {
            Self::Sessions => 1..=1024,
            Self::CiJobs => 1..=64,
            Self::CiLanes => 1..=8,
            Self::SandboxMemory => 1..=host.memory.map_or(65_536, |bytes| (bytes / GIB).max(2)),
            Self::SandboxDisk => 0..=1_000_000,
            Self::CdRunDisk => 1..=10_000,
        }
    }

    /// Whether `value` may be pinned on `host`.
    ///
    /// # Errors
    /// It is outside [`Limit::range`]; the message names the range.
    pub fn check(self, value: u64, host: Host) -> Result<(), OutOfRange> {
        let range = self.range(host);
        if range.contains(&value) {
            return Ok(());
        }
        let unit = self.unit();
        Err(OutOfRange {
            label: self.label(),
            min: *range.start(),
            max: *range.end(),
            unit: if unit.is_empty() {
                String::new()
            } else {
                format!(" {unit}")
            },
        })
    }

    /// The value in force on `host`: a number saved in Settings; else, with host-sized saved
    /// there, the host-sized value whatever the unit says; else the unit's; else host-sized.
    #[must_use]
    pub fn resolve(self, host: Host, unit: Option<u64>, setting: Option<Setting>) -> Resolved {
        match (setting, unit) {
            (Some(Setting::Value(value)), _) => Resolved {
                value,
                source: Source::Settings,
            },
            (None, Some(value)) => Resolved {
                value,
                source: Source::Unit,
            },
            (Some(Setting::HostSized), _) | (None, None) => Resolved {
                value: self.host_sized(host),
                source: Source::Default,
            },
        }
    }

    /// Whether `then` and `now` are hosts of another size for this limit: a fact its formula uses
    /// differs by a quarter or more, either way. A fact one of them does not report never counts.
    #[must_use]
    pub fn another_size(self, then: Host, now: Host) -> bool {
        let cpus = differs(then.cpus.map(u64::from), now.cpus.map(u64::from));
        let memory = differs(then.memory, now.memory);
        let disk = differs(then.disk, now.disk);
        match self {
            Self::Sessions => cpus || memory,
            Self::CiJobs | Self::CiLanes => cpus,
            Self::SandboxMemory => memory,
            Self::SandboxDisk | Self::CdRunDisk => disk,
        }
    }
}

/// A sandbox's memory on a host with `total` bytes: the memory but a reserve for the host and
/// Yulius (an eighth, at least 4 GiB), and at least 2 GiB (ADR-0198).
#[must_use]
pub const fn sandbox_memory(total: u64) -> u64 {
    let eighth = total / 8;
    let reserve = if eighth > 4 * GIB { eighth } else { 4 * GIB };
    let left = total.saturating_sub(reserve);
    if left > 2 * GIB { left } else { 2 * GIB }
}

/// Whether two readings of a fact differ by a quarter or more of the smaller one.
fn differs(then: Option<u64>, now: Option<u64>) -> bool {
    let (Some(then), Some(now)) = (then, now) else {
        return false;
    };
    let (low, high) = (u128::from(then.min(now)), u128::from(then.max(now)));
    high > low && high * 4 >= low * 5
}

impl Pin {
    /// Whether this pin no longer fits `now`: it was configured on a host of another size and is
    /// not the number `now` would get anyway.
    #[must_use]
    pub fn stale(self, limit: Limit, now: Host) -> bool {
        self.value != limit.host_sized(now) && limit.another_size(self.host, now)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn host(cpus: u32, memory_gib: u64, disk_gb: u64) -> Host {
        Host {
            cpus: Some(cpus),
            memory: Some(memory_gib * GIB),
            disk: Some(disk_gb * GB),
        }
    }

    fn sized(host: Host) -> [u64; 6] {
        Limit::ALL.map(|limit| limit.host_sized(host))
    }

    const OLD_VM: (u32, u64, u64) = (12, 47, 387);
    const NEW_HOST: (u32, u64, u64) = (64, 755, 3500);

    #[test]
    fn the_old_vm_keeps_the_numbers_it_ran_with() {
        let (cpus, memory, disk) = OLD_VM;
        assert_eq!(sized(host(cpus, memory, disk)), [8, 4, 2, 41, 100, 20]);
    }

    #[test]
    fn the_new_host_gets_what_was_set_by_hand_or_close() {
        let (cpus, memory, disk) = NEW_HOST;
        assert_eq!(sized(host(cpus, memory, disk)), [34, 12, 4, 660, 875, 175]);
    }

    #[test]
    fn small_hosts_get_small_limits_that_still_work() {
        assert_eq!(sized(host(1, 1, 20)), [2, 2, 2, 2, 10, 10]);
        assert_eq!(sized(host(2, 4, 40)), [2, 2, 2, 2, 20, 20]);
        assert_eq!(sized(host(4, 8, 80)), [4, 2, 2, 4, 40, 20]);
        // A disk under 2 GB still leaves a quota: 0 would mean none.
        assert_eq!(Limit::SandboxDisk.host_sized(host(1, 1, 1)), 1);
        assert_eq!(Limit::CdRunDisk.host_sized(host(1, 1, 1)), 1);
    }

    #[test]
    fn huge_hosts_stop_at_the_caps() {
        assert_eq!(
            sized(host(256, 2048, 100_000)),
            [130, 44, 8, 1792, 25_000, 5000]
        );
        assert_eq!(Limit::CiJobs.host_sized(host(512, 4096, 1000)), 64);
    }

    #[test]
    fn little_memory_bounds_the_sessions_of_many_cpus() {
        assert_eq!(Limit::Sessions.host_sized(host(64, 16, 1000)), 8);
        assert_eq!(Limit::Sessions.host_sized(host(64, 2, 1000)), 2);
    }

    #[test]
    fn a_fact_the_host_does_not_report_leaves_the_earlier_constant() {
        assert_eq!(sized(Host::default()), [8, 4, 2, 2, 100, 20]);
        let no_memory = Host {
            memory: None,
            ..host(64, 0, 3500)
        };
        assert_eq!(sized(no_memory), [8, 12, 4, 2, 875, 175]);
        let only_memory = Host {
            memory: Some(755 * GIB),
            ..Host::default()
        };
        assert_eq!(sized(only_memory), [8, 4, 2, 660, 100, 20]);
    }

    #[test]
    fn the_sandbox_memory_is_the_default_the_sandboxes_always_had() {
        assert_eq!(sandbox_memory(47 * GIB), 47 * GIB - 47 * GIB / 8);
        assert_eq!(sandbox_memory(16 * GIB), 12 * GIB);
        assert_eq!(sandbox_memory(3 * GIB), 2 * GIB);
        assert_eq!(sandbox_memory(0), 2 * GIB);
    }

    #[test]
    fn a_saved_number_wins_then_host_sized_then_the_unit_then_the_formula() {
        let (cpus, memory, disk) = NEW_HOST;
        let now = host(cpus, memory, disk);
        let limit = Limit::Sessions;
        assert_eq!(
            limit.resolve(now, Some(8), Some(Setting::Value(20))),
            Resolved {
                value: 20,
                source: Source::Settings
            }
        );
        assert_eq!(
            limit.resolve(now, Some(8), Some(Setting::HostSized)),
            Resolved {
                value: 34,
                source: Source::Default
            }
        );
        assert_eq!(
            limit.resolve(now, Some(8), None),
            Resolved {
                value: 8,
                source: Source::Unit
            }
        );
        assert_eq!(
            limit.resolve(now, None, None),
            Resolved {
                value: 34,
                source: Source::Default
            }
        );
    }

    #[test]
    fn a_quarter_more_or_less_of_a_fact_is_another_size() {
        let then = host(12, 48, 400);
        for (cpus, another) in [(12, false), (14, false), (15, true), (10, false), (9, true)] {
            assert_eq!(
                Limit::CiJobs.another_size(then, host(cpus, 48, 400)),
                another,
                "{cpus} CPUs after 12"
            );
        }
        // Exactly a quarter either way counts: 12 → 15 and 15 → 12.
        assert!(Limit::CiLanes.another_size(host(15, 48, 400), then));
        for (memory, another) in [
            (48, false),
            (59, false),
            (60, true),
            (39, false),
            (38, true),
        ] {
            assert_eq!(
                Limit::SandboxMemory.another_size(then, host(12, memory, 400)),
                another,
                "{memory} GiB after 48"
            );
        }
        for (disk, another) in [
            (400, false),
            (499, false),
            (500, true),
            (321, false),
            (320, true),
        ] {
            assert_eq!(
                Limit::SandboxDisk.another_size(then, host(12, 48, disk)),
                another,
                "{disk} GB after 400"
            );
            assert_eq!(
                Limit::CdRunDisk.another_size(then, host(12, 48, disk)),
                another
            );
        }
    }

    #[test]
    fn only_the_facts_of_a_limits_formula_count() {
        let then = host(12, 48, 400);
        let more_cpus = host(64, 48, 400);
        let more_memory = host(12, 755, 400);
        let more_disk = host(12, 48, 3500);
        let flagged = |now: Host| Limit::ALL.map(|limit| limit.another_size(then, now));
        assert_eq!(flagged(more_cpus), [true, true, true, false, false, false]);
        assert_eq!(
            flagged(more_memory),
            [true, false, false, true, false, false]
        );
        assert_eq!(flagged(more_disk), [false, false, false, false, true, true]);
        assert_eq!(flagged(then), [false; 6]);
    }

    #[test]
    fn a_fact_one_host_does_not_report_is_never_another_size() {
        let (cpus, memory, disk) = NEW_HOST;
        let now = host(cpus, memory, disk);
        for limit in Limit::ALL {
            assert!(!limit.another_size(Host::default(), now), "{limit:?}");
            assert!(!limit.another_size(now, Host::default()), "{limit:?}");
        }
    }

    #[test]
    fn the_pins_of_the_old_vm_no_longer_fit_the_new_host() {
        let old = host(OLD_VM.0, OLD_VM.1, OLD_VM.2);
        let new = host(NEW_HOST.0, NEW_HOST.1, NEW_HOST.2);
        for (limit, value) in [
            (Limit::Sessions, 8),
            (Limit::SandboxMemory, 38),
            (Limit::SandboxDisk, 300),
        ] {
            let pin = Pin { value, host: old };
            assert!(pin.stale(limit, new), "{limit:?}");
            assert!(!pin.stale(limit, old), "{limit:?} on its own host");
        }
    }

    #[test]
    fn a_pin_equal_to_the_host_sized_value_is_never_flagged() {
        let old = host(OLD_VM.0, OLD_VM.1, OLD_VM.2);
        let new = host(NEW_HOST.0, NEW_HOST.1, NEW_HOST.2);
        for limit in Limit::ALL {
            let pin = Pin {
                value: limit.host_sized(new),
                host: old,
            };
            assert!(!pin.stale(limit, new), "{limit:?}");
        }
    }

    #[test]
    fn a_pin_outside_the_range_is_refused_with_the_range() {
        let now = host(12, 47, 387);
        assert_eq!(
            Limit::Sessions.check(0, now).map_err(|e| e.to_string()),
            Err("Live sessions must be between 1 and 1024".to_owned())
        );
        assert_eq!(
            Limit::CiJobs.check(65, now).map_err(|e| e.to_string()),
            Err("CI jobs at once must be between 1 and 64".to_owned())
        );
        assert_eq!(
            Limit::SandboxMemory
                .check(48, now)
                .map_err(|e| e.to_string()),
            Err("Sandbox memory must be between 1 and 47 GiB".to_owned())
        );
        assert!(Limit::SandboxMemory.check(47, now).is_ok());
        // No quota is a choice for the sandbox disk, not for a CD run.
        assert!(Limit::SandboxDisk.check(0, now).is_ok());
        assert!(Limit::CdRunDisk.check(0, now).is_err());
        assert!(Limit::CiLanes.check(9, now).is_err());
    }

    #[test]
    fn keys_name_the_limits_both_ways() {
        for limit in Limit::ALL {
            assert_eq!(Limit::from_key(limit.key()), Some(limit));
            assert!(limit.env().starts_with("YULIUS_"));
            assert!(!limit.formula().is_empty());
        }
        assert_eq!(Limit::from_key("sandbox_pids"), None);
    }
}
