//! The server's data disk under pressure (SPEC-276 RDA-006, RDA-007, ADR-0205). The disk upkeep
//! reads how full the data disk is every minute: from 85 % it is low and every round sweeps the
//! build directories harder and runs the safe leftovers round (at most every 10 minutes), from
//! 95 % it is critical and the idle tasks' and CI checkouts' caches go too until the disk is back
//! at 85 %. A change of level is published to every page (`disk.updated`) and a rise is audited
//! (`disk.pressure`). Quota pressure of one sandbox keeps its own rule (SPEC-008).

use std::sync::{Arc, Mutex};
use std::time::Duration;

use yulius_contracts::api::status::DiskView;
use yulius_contracts::events::UiEvent;
use yulius_domain::disk::human;
use yulius_domain::sweep::{Level, bytes_to_relieve};

use crate::context::App;
use crate::disk::DiskReport;
use crate::sweep::SweepReport;

/// While the disk is low, the safe leftovers round runs at most this often.
pub const LEFTOVERS_UNDER_PRESSURE_EVERY: Duration = Duration::from_secs(10 * 60);

/// The last level read and when the leftovers round last ran for it.
#[derive(Default)]
pub struct DiskWatch {
    inner: Mutex<Watched>,
}

#[derive(Default)]
struct Watched {
    level: Level,
    /// The used percent last published (the pill shows it while the disk is low).
    percent: Option<u64>,
    leftovers_at: Option<tokio::time::Instant>,
}

/// What one disk round did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DiskRound {
    /// The data disk's level at its start.
    pub level: Level,
    /// The sweep, when one ran.
    pub sweep: Option<SweepReport>,
    /// The upkeep of every live sandbox, summed.
    pub upkeep: DiskReport,
    /// The safe leftovers round was started (the disk is low).
    pub leftovers: bool,
}

fn percent(used: u64, total: u64) -> u64 {
    u64::try_from(u128::from(used) * 100 / u128::from(total.max(1))).unwrap_or(100)
}

impl App {
    fn with_watch<T>(&self, f: impl FnOnce(&mut Watched) -> T) -> T {
        let mut inner = self
            .disk_watch
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        f(&mut inner)
    }

    /// `GET /api/disk` (anyone signed in): the data disk's level, read now.
    pub async fn disk_view(&self) -> DiskView {
        let disk = self.data_disk().await;
        DiskView {
            level: disk
                .map_or(Level::Ok, |(total, used)| Level::of(used, total))
                .as_str()
                .to_owned(),
            used: disk.map(|(_, used)| used),
            total: disk.map(|(total, _)| total),
        }
    }

    /// Reads the data disk's level and returns it with the one read before. A change of level, and
    /// while the disk is low of its used percent, is published; a rise is audited with what
    /// Yulius does about it.
    pub async fn check_disk(&self) -> (Level, Level) {
        let disk = self.data_disk().await;
        let level = disk.map_or(Level::Ok, |(total, used)| Level::of(used, total));
        let shown = disk.map(|(total, used)| percent(used, total));
        let (before, moved) = self.with_watch(|watched| {
            let before = std::mem::replace(&mut watched.level, level);
            // While low, every point the used share moves is shown too (the pill says it).
            let moved = level != Level::Ok && watched.percent != shown;
            watched.percent = shown;
            (before, moved)
        });
        if level != before || moved {
            self.events.publish(
                None,
                false,
                UiEvent::DiskChanged {
                    level: level.as_str().to_owned(),
                    used: disk.map(|(_, used)| used),
                    total: disk.map(|(total, _)| total),
                },
            );
        }
        if level > before
            && let Some((total, used)) = disk
        {
            let doing = if level == Level::Critical {
                "sweeping build caches hard and pruning idle tasks' caches"
            } else {
                "sweeping build caches harder"
            };
            self.dlv_audit(
                "yulius",
                "disk.pressure",
                "instance",
                &format!(
                    "the data disk is {} ({} % used, {} free of {}): {doing}",
                    level.as_str(),
                    percent(used, total),
                    human(total.saturating_sub(used)),
                    human(total)
                ),
            )
            .await;
        }
        (level, before)
    }

    /// One disk round (RDA-006): the build directories are swept when `sweep_due` or the disk is
    /// low; every live sandbox gets its upkeep, with the bytes a critical disk needs freed;
    /// requested purges are tried; and while the disk is low the safe leftovers round starts (at
    /// most every [`LEFTOVERS_UNDER_PRESSURE_EVERY`]).
    pub async fn disk_round(self: &Arc<Self>, sweep_due: bool) -> DiskRound {
        let (level, _) = self.check_disk().await;
        let mut round = DiskRound {
            level,
            ..DiskRound::default()
        };
        if sweep_due || level >= Level::Low {
            round.sweep = Some(self.sweep_build_caches(level).await);
        }
        // Read again after the sweep: what it freed need not come from idle tasks.
        let mut need = match self.data_disk().await {
            Some((total, used)) if level == Level::Critical => bytes_to_relieve(used, total),
            _ => 0,
        };
        if let Ok(sandboxes) = self.deps.store.sandboxes().await {
            for sandbox in &sandboxes {
                match self.disk_upkeep_needing(sandbox, false, need).await {
                    Ok(report) => {
                        need = need.saturating_sub(report.freed);
                        round.upkeep.freed = round.upkeep.freed.saturating_add(report.freed);
                        round.upkeep.caches += report.caches;
                        round.upkeep.worktrees += report.worktrees;
                    }
                    Err(err) => {
                        tracing::warn!(sandbox = %sandbox.id, error = %err, "disk upkeep failed");
                    }
                }
            }
        }
        // Removed sandboxes whose files were asked to go, once nothing of them runs.
        self.run_purges().await;
        let leftovers_due = level >= Level::Low
            && self.with_watch(|watched| {
                let due = watched
                    .leftovers_at
                    .is_none_or(|at| at.elapsed() >= LEFTOVERS_UNDER_PRESSURE_EVERY);
                if due {
                    watched.leftovers_at = Some(tokio::time::Instant::now());
                }
                due
            });
        round.leftovers = leftovers_due;
        if leftovers_due {
            let app = Arc::clone(self);
            tokio::spawn(async move {
                app.run_leftovers_round().await;
            });
        }
        round
    }
}
