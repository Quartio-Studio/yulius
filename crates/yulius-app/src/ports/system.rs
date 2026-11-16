//! System metrics port for the status panel (SPEC-001 UI-007, AC-STA-1).
//!
//! Implementations may block briefly (they read OS counters); callers run them off the async
//! executor. Unknown values stay `None`: a metric that cannot be read is never reported as zero.

use std::path::Path;

/// One reading of machine metrics.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct SystemSnapshot {
    /// CPU utilization percent over all cores since the previous reading (0–100).
    pub cpu_percent: Option<f32>,
    /// Logical CPU count.
    pub cpu_count: Option<u32>,
    /// Used memory bytes.
    pub memory_used: Option<u64>,
    /// Total memory bytes.
    pub memory_total: Option<u64>,
    /// Used bytes of the volume holding the probed path.
    pub disk_used: Option<u64>,
    /// Total bytes of that volume.
    pub disk_total: Option<u64>,
    /// Load averages `[1m, 5m, 15m]`.
    pub load: Option<[f64; 3]>,
    /// Seconds since boot.
    pub uptime_s: Option<u64>,
}

/// Reads machine metrics.
pub trait SystemProbe: Send + Sync {
    /// Current metrics; disk figures describe the volume that holds `data_dir`.
    fn snapshot(&self, data_dir: &Path) -> SystemSnapshot;

    /// Total and used bytes of the volume that holds `data_dir` (SPEC-276), read every minute by
    /// the disk upkeep: only the disk, so the CPU reading of the status panel is left alone.
    fn disk(&self, data_dir: &Path) -> Option<(u64, u64)> {
        let snapshot = self.snapshot(data_dir);
        Some((snapshot.disk_total?, snapshot.disk_used?))
    }
}
