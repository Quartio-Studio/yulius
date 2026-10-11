//! Machine metrics over `sysinfo` for the status panel (SPEC-001 UI-007).
//!
//! CPU utilization is a delta between two readings at least
//! [`sysinfo::MINIMUM_CPU_UPDATE_INTERVAL`] apart: the probe takes a first reading at
//! construction and reports `None` until a later reading exists, never a fake zero.

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Instant;

use sysinfo::{Disks, System};
use yulius_app::ports::system::{SystemProbe, SystemSnapshot};

struct State {
    system: System,
    last_cpu_refresh: Instant,
    cpu_percent: Option<f32>,
}

/// `sysinfo`-backed [`SystemProbe`].
pub struct SysinfoProbe {
    state: Mutex<State>,
}

impl Default for SysinfoProbe {
    fn default() -> Self {
        Self::new()
    }
}

impl SysinfoProbe {
    /// A probe with its first CPU reading taken now.
    #[must_use]
    pub fn new() -> Self {
        let mut system = System::new();
        system.refresh_cpu_usage();
        Self {
            state: Mutex::new(State {
                system,
                last_cpu_refresh: Instant::now(),
                cpu_percent: None,
            }),
        }
    }
}

fn known(value: u64) -> Option<u64> {
    (value > 0).then_some(value)
}

/// Total and used bytes of the mounted volume that holds `path` (longest mount-point prefix).
/// Without a listed disk for it — inside a sandbox the path sits on a bind or overlay mount
/// that the disk list leaves out (F 2026-10-11) — the file system's own figures (`statvfs`).
fn volume_of(path: &Path) -> Option<(u64, u64)> {
    let path: PathBuf = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let disks = Disks::new_with_refreshed_list();
    let listed = disks
        .list()
        .iter()
        .filter(|d| path.starts_with(d.mount_point()))
        .max_by_key(|d| d.mount_point().as_os_str().len())
        .and_then(|disk| {
            let total = known(disk.total_space())?;
            Some((total, total.saturating_sub(disk.available_space())))
        });
    listed.or_else(|| statvfs_of(&path))
}

/// Total and used bytes of the file system holding `path`, from `statvfs`.
fn statvfs_of(path: &Path) -> Option<(u64, u64)> {
    let stat = nix::sys::statvfs::statvfs(path).ok()?;
    let fragment = u64::from(stat.fragment_size());
    let total = known(u64::from(stat.blocks()).saturating_mul(fragment))?;
    let available = u64::from(stat.blocks_available()).saturating_mul(fragment);
    Some((total, total.saturating_sub(available)))
}

impl SystemProbe for SysinfoProbe {
    fn snapshot(&self, data_dir: &Path) -> SystemSnapshot {
        let disk = volume_of(data_dir);
        let load = System::load_average();
        let load = [load.one, load.five, load.fifteen];
        let mut snapshot = SystemSnapshot {
            disk_total: disk.map(|d| d.0),
            disk_used: disk.map(|d| d.1),
            load: load.iter().all(|v| v.is_finite()).then_some(load),
            uptime_s: known(System::uptime()),
            ..SystemSnapshot::default()
        };
        let Ok(mut state) = self.state.lock() else {
            return snapshot;
        };
        let now = Instant::now();
        if now.duration_since(state.last_cpu_refresh) >= sysinfo::MINIMUM_CPU_UPDATE_INTERVAL {
            state.system.refresh_cpu_usage();
            state.cpu_percent = Some(state.system.global_cpu_usage()).filter(|p| p.is_finite());
            state.last_cpu_refresh = now;
        }
        state.system.refresh_memory();
        snapshot.cpu_percent = state.cpu_percent;
        snapshot.cpu_count = u32::try_from(state.system.cpus().len())
            .ok()
            .filter(|n| *n > 0);
        snapshot.memory_total = known(state.system.total_memory());
        snapshot.memory_used = snapshot.memory_total.map(|_| state.system.used_memory());
        snapshot
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_file_system_reports_its_size_when_no_disk_is_listed_for_it() {
        let (total, used) = statvfs_of(&std::env::temp_dir()).unwrap_or((0, 1));
        assert!(total > 0 && used <= total, "{total} {used}");
    }

    #[test]
    fn reads_real_machine_metrics() -> Result<(), Box<dyn std::error::Error>> {
        let probe = SysinfoProbe::new();
        let dir = std::env::temp_dir();
        let first = probe.snapshot(&dir);
        assert!(first.memory_total.is_some_and(|t| t > 0));
        assert!(first.memory_used <= first.memory_total);
        let (total, used) = (
            first.disk_total.ok_or("disk total")?,
            first.disk_used.ok_or("disk used")?,
        );
        assert!(used <= total);
        assert!(first.cpu_count.is_some_and(|n| n > 0));
        assert!(first.load.is_some());
        Ok(())
    }
}
