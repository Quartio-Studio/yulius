//! The instance's resource limits (SPEC-459, ADR-0294): live sessions, CI jobs and lanes, a
//! sandbox's memory and disk quota, and a CD run's disk.
//!
//! Each is sized from the host unless pinned ([`yulius_domain::limits`]). A number saved in
//! Settings wins over the unit's `YULIUS_*` value, and "host-sized" saved there overrules the
//! unit too. Every pin is recorded with the host it was configured on, so that after a move the
//! instance can say which numbers were sized for another machine; the first start after
//! `yulius instance import` (its mark, [`ADAPT_MARK`]) replaces those by host-sized itself.
//!
//! The values in force live in `LimitState`: the supervisor, CI and CD read them at each use,
//! so a change applies while the server runs. The sandbox memory is the exception: sandboxes and
//! the CI budget pools are sized from it at start.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};

use serde::{Deserialize, Serialize};
use yulius_contracts::api::updates::{LimitSettingView, LimitView, LimitsHost, LimitsView};
use yulius_domain::limits::{Host, Limit, Pin, Resolved, Setting};
use yulius_domain::time::Timestamp;

use crate::auth::rfc3339;
use crate::context::{App, AppConfig};
use crate::error::{AppError, AppResult};

mod change;

/// File in the data directory that `yulius instance import` leaves: the next start replaces the
/// pins that no longer fit by host-sized values (SPEC-459 HSL-005).
pub const ADAPT_MARK: &str = ".yulius-limits-adapt";
/// Who an adaptation after a move is recorded and audited as.
pub const MIGRATION: &str = "migration";

/// The audit target of what concerns several limits at once (`limits.adapted`, `limits.kept`);
/// one limit's entries name its key.
pub const AUDIT_TARGET: &str = "limits";

const GB: u64 = 1_000_000_000;

/// The unit's pins: what the `YULIUS_*` variables said at this start.
pub type UnitLimits = BTreeMap<Limit, u64>;

/// What the isolation is built with at start.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StartLimits {
    /// A pinned sandbox memory in GiB; `None`: the host-sized default, to the byte.
    pub memory_gb: Option<u64>,
    /// The sandbox disk quota in GB; 0: none.
    pub disk_gb: u64,
}

/// The limits in force, read at each use.
pub(crate) struct LimitState {
    sessions: AtomicU32,
    ci_jobs: AtomicU32,
    ci_lanes: AtomicU32,
    run_disk_max: AtomicU64,
    /// What [`App::load_limits`] read; `None` where nothing loaded them (the configuration's
    /// numbers stay in force).
    loaded: tokio::sync::Mutex<Option<Loaded>>,
}

impl LimitState {
    pub(crate) fn new(config: &AppConfig) -> Self {
        Self {
            sessions: AtomicU32::new(config.max_live_sessions),
            ci_jobs: AtomicU32::new(config.ci.max_jobs),
            ci_lanes: AtomicU32::new(config.ci.lanes),
            run_disk_max: AtomicU64::new(config.ci.run_disk_max),
            loaded: tokio::sync::Mutex::new(None),
        }
    }
}

/// The host, the unit's pins and the saved record of this start.
#[derive(Debug, Clone)]
struct Loaded {
    host: Host,
    unit: UnitLimits,
    record: Record,
    /// The sandbox memory (GiB) the sandboxes were sized with.
    memory_at_start: u64,
}

/// The saved record (`instance.limits`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
struct Record {
    /// What a superadmin saved, by limit key.
    #[serde(default)]
    settings: BTreeMap<String, SavedSetting>,
    /// The unit values the server has seen, by limit key, with the host it first saw each
    /// number on. Kept when the unit drops the value: a drop-in copied back later is known.
    #[serde(default)]
    unit: BTreeMap<String, SavedPin>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
struct SavedHost {
    cpus: Option<u32>,
    memory: Option<u64>,
    disk: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct SavedSetting {
    /// The number; `None`: host-sized.
    value: Option<u64>,
    host: SavedHost,
    by: String,
    at: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
struct SavedPin {
    value: u64,
    host: SavedHost,
}

impl From<Host> for SavedHost {
    fn from(host: Host) -> Self {
        Self {
            cpus: host.cpus,
            memory: host.memory,
            disk: host.disk,
        }
    }
}

impl From<SavedHost> for Host {
    fn from(host: SavedHost) -> Self {
        Self {
            cpus: host.cpus,
            memory: host.memory,
            disk: host.disk,
        }
    }
}

fn host_view(host: Host) -> LimitsHost {
    LimitsHost {
        cpus: host.cpus,
        memory_bytes: host.memory,
        disk_bytes: host.disk,
    }
}

impl Loaded {
    fn setting(&self, limit: Limit) -> Option<Setting> {
        self.record
            .settings
            .get(limit.key())
            .map(|saved| saved.value.map_or(Setting::HostSized, Setting::Value))
    }

    fn resolved(&self, limit: Limit) -> Resolved {
        limit.resolve(
            self.host,
            self.unit.get(&limit).copied(),
            self.setting(limit),
        )
    }

    /// The pinned number in force with the host it was configured on: a number saved in
    /// Settings, else the unit's unless host-sized overrules it.
    fn pin(&self, limit: Limit) -> Option<Pin> {
        match self.record.settings.get(limit.key()) {
            Some(saved) => saved.value.map(|value| Pin {
                value,
                host: saved.host.into(),
            }),
            None => {
                let value = *self.unit.get(&limit)?;
                let host = self
                    .record
                    .unit
                    .get(limit.key())
                    .filter(|seen| seen.value == value)
                    .map_or(self.host, |seen| seen.host.into());
                Some(Pin { value, host })
            }
        }
    }

    fn stale(&self, limit: Limit) -> bool {
        self.pin(limit)
            .is_some_and(|pin| pin.stale(limit, self.host))
    }

    /// A unit value seen on another host that this unit does not hold (yet) and nothing in
    /// Settings overrules: the drop-in a person copies from the old machine after a move.
    fn left_behind(&self, limit: Limit) -> Option<Pin> {
        if self.unit.contains_key(&limit) || self.record.settings.contains_key(limit.key()) {
            return None;
        }
        let seen = self.record.unit.get(limit.key())?;
        let pin = Pin {
            value: seen.value,
            host: seen.host.into(),
        };
        pin.stale(limit, self.host).then_some(pin)
    }

    /// Records each unit value with this host, unless the same number is already on record.
    fn record_unit(&mut self) {
        for (limit, value) in &self.unit {
            let known = self
                .record
                .unit
                .get(limit.key())
                .is_some_and(|seen| seen.value == *value);
            if !known {
                self.record.unit.insert(
                    limit.key().to_owned(),
                    SavedPin {
                        value: *value,
                        host: self.host.into(),
                    },
                );
            }
        }
    }

    fn view(&self) -> LimitsView {
        let limits = Limit::ALL
            .into_iter()
            .map(|limit| {
                let resolved = self.resolved(limit);
                let range = limit.range(self.host);
                let saved = self.record.settings.get(limit.key());
                LimitView {
                    key: limit.key().to_owned(),
                    label: limit.label().to_owned(),
                    env: limit.env().to_owned(),
                    suffix: limit.unit().to_owned(),
                    value: resolved.value,
                    source: resolved.source.as_str().to_owned(),
                    host_sized: limit.host_sized(self.host),
                    formula: limit.formula().to_owned(),
                    unit_value: self.unit.get(&limit).copied(),
                    setting: saved.map(|saved| LimitSettingView {
                        value: saved.value,
                        set_by: saved.by.clone(),
                        set_at: rfc3339(Timestamp(saved.at)),
                    }),
                    configured_on: self.pin(limit).map(|pin| host_view(pin.host)),
                    stale: self.stale(limit),
                    min: *range.start(),
                    max: *range.end(),
                    live: limit.live(),
                    restart_needed: !limit.live() && resolved.value != self.memory_at_start,
                }
            })
            .collect();
        LimitsView {
            host: host_view(self.host),
            limits,
        }
    }
}

/// What a change of the limits lets start: queued tasks, queued CI runs.
#[derive(Debug, Clone, Copy)]
struct Raised {
    sessions: bool,
    lanes: bool,
}

/// `"12 CPUs, 47 GiB, 387 GB"`, with `?` for what a host does not report.
fn host_words(host: Host) -> String {
    let or_unknown = |n: Option<u64>| n.map_or_else(|| "?".to_owned(), |n| n.to_string());
    format!(
        "{} CPUs, {} GiB, {} GB",
        or_unknown(host.cpus.map(u64::from)),
        or_unknown(host.memory.map(|bytes| bytes >> 30)),
        or_unknown(host.disk.map(|bytes| bytes / GB)),
    )
}

fn amount(limit: Limit, value: u64) -> String {
    if limit.unit().is_empty() {
        value.to_string()
    } else {
        format!("{value} {}", limit.unit())
    }
}

impl App {
    /// Live agent sessions at once.
    #[must_use]
    pub fn max_live_sessions(&self) -> u32 {
        self.limits.sessions.load(Ordering::Relaxed)
    }

    /// CI jobs at once.
    #[must_use]
    pub fn ci_max_jobs(&self) -> u32 {
        self.limits.ci_jobs.load(Ordering::Relaxed).max(1)
    }

    /// CI runs at once per sandbox.
    #[must_use]
    pub fn ci_lanes(&self) -> u32 {
        self.limits.ci_lanes.load(Ordering::Relaxed).max(1)
    }

    /// Bytes a CD or environment run's files and cache may use.
    #[must_use]
    pub fn run_disk_max(&self) -> u64 {
        self.limits.run_disk_max.load(Ordering::Relaxed)
    }

    /// This host's size, as the system probe reports it.
    fn host(&self) -> Host {
        let snapshot = self.deps.system.snapshot(&self.config.data_dir);
        Host {
            cpus: snapshot.cpu_count,
            memory: snapshot.memory_total,
            disk: snapshot.disk_total,
        }
    }

    /// Reads the limits at start (SPEC-459): measures the host, records the unit's pins `unit`
    /// with it, adapts after an instance move (the import's mark), puts the values in force and
    /// says in the log what they are, where each comes from and which pins no longer fit.
    /// Returns what the sandbox isolation is built with.
    pub async fn load_limits(self: &Arc<Self>, unit: UnitLimits) -> StartLimits {
        let host = self.host();
        let saved = match self.deps.store.limits().await {
            Ok(doc) => doc,
            Err(err) => {
                tracing::warn!("the saved resource limits could not be read: {err}");
                None
            }
        };
        let record = saved
            .as_deref()
            .map(serde_json::from_str::<Record>)
            .transpose()
            .unwrap_or_else(|err| {
                tracing::warn!("the saved resource limits are not readable, starting anew: {err}");
                None
            })
            .unwrap_or_default();
        let before = record.clone();
        let mut loaded = Loaded {
            host,
            unit,
            record,
            memory_at_start: 0,
        };
        loaded.record_unit();

        let mark = self.config.data_dir.join(ADAPT_MARK);
        let moved = tokio::fs::try_exists(&mark).await.unwrap_or(false);
        let adapted = if moved {
            loaded.adapt(true, MIGRATION, self.now())
        } else {
            Vec::new()
        };
        let stored = loaded.record == before || self.save_limits(&loaded.record).await.is_ok();
        if moved && stored {
            if !adapted.is_empty() {
                let detail = adapted.join(", ");
                tracing::info!("resource limits adapted to this host after the move: {detail}");
                if let Err(err) = self
                    .deps
                    .store
                    .audit(
                        self.now(),
                        MIGRATION,
                        "limits.adapted",
                        Some(AUDIT_TARGET),
                        Some(&detail),
                    )
                    .await
                {
                    tracing::warn!("the limits adaptation could not be audited: {err}");
                }
            }
            if let Err(err) = tokio::fs::remove_file(&mark).await {
                tracing::warn!("{} could not be removed: {err}", mark.display());
            }
        }

        let memory = loaded.resolved(Limit::SandboxMemory);
        loaded.memory_at_start = memory.value;
        let start = StartLimits {
            memory_gb: loaded.pin(Limit::SandboxMemory).map(|pin| pin.value),
            disk_gb: loaded.resolved(Limit::SandboxDisk).value,
        };
        self.put_in_force(&loaded);
        self.log_limits(&loaded);
        *self.limits.loaded.lock().await = Some(loaded);
        start
    }

    async fn save_limits(&self, record: &Record) -> AppResult<()> {
        let doc = serde_json::to_string(record)
            .map_err(|err| AppError::Internal(format!("limits record: {err}")))?;
        let saved = self.deps.store.set_limits(&doc).await;
        if let Err(err) = &saved {
            // Before setup there is no instance row; the pins are recorded at the next start.
            tracing::debug!("the resource limits record was not saved: {err}");
        }
        saved
    }

    /// Puts every limit's resolved value in force. Returns what may start now that could not
    /// before.
    fn put_in_force(&self, loaded: &Loaded) -> Raised {
        let count = |limit: Limit| u32::try_from(loaded.resolved(limit).value).unwrap_or(u32::MAX);
        let sessions = count(Limit::Sessions);
        let lanes = count(Limit::CiLanes);
        let raised = Raised {
            sessions: sessions > self.limits.sessions.swap(sessions, Ordering::Relaxed),
            lanes: lanes > self.limits.ci_lanes.swap(lanes, Ordering::Relaxed),
        };
        self.limits
            .ci_jobs
            .store(count(Limit::CiJobs), Ordering::Relaxed);
        self.ci_resize_slots();
        self.limits.run_disk_max.store(
            loaded.resolved(Limit::CdRunDisk).value.saturating_mul(GB),
            Ordering::Relaxed,
        );
        // At start the isolation is not installed yet: it is built with the quota.
        if let Some(isolation) = self.isolation.get() {
            let disk = loaded.resolved(Limit::SandboxDisk).value;
            isolation.set_disk_quota((disk > 0).then(|| disk.saturating_mul(GB)));
        }
        raised
    }

    fn log_limits(&self, loaded: &Loaded) {
        let words = Limit::ALL
            .into_iter()
            .map(|limit| {
                let resolved = loaded.resolved(limit);
                format!(
                    "{} {} ({})",
                    limit.label().to_lowercase(),
                    amount(limit, resolved.value),
                    resolved.source.as_str()
                )
            })
            .collect::<Vec<_>>()
            .join(", ");
        tracing::info!(
            "resource limits on this host ({}): {words}",
            host_words(loaded.host)
        );
        for limit in Limit::ALL {
            let resolved = loaded.resolved(limit);
            if let (Some(unit), Some(_)) = (loaded.unit.get(&limit), loaded.setting(limit)) {
                tracing::info!(
                    "{}={unit} is overruled by Settings → Instance ({} in force)",
                    limit.env(),
                    amount(limit, resolved.value)
                );
            }
            if let Some(pin) = loaded
                .pin(limit)
                .filter(|pin| pin.stale(limit, loaded.host))
            {
                tracing::warn!(
                    "{} is pinned to {} ({}), sized for another host ({}); this host would get \
                     {}: take it or keep the number in Settings → Instance",
                    limit.label().to_lowercase(),
                    amount(limit, pin.value),
                    resolved.source.as_str(),
                    host_words(pin.host),
                    amount(limit, limit.host_sized(loaded.host))
                );
            }
        }
    }

    /// The limits as a superadmin reads them.
    async fn limits_snapshot(&self) -> AppResult<LimitsView> {
        let loaded = self.limits.loaded.lock().await;
        loaded.as_ref().map(Loaded::view).ok_or_else(|| {
            AppError::Conflict("this server did not load its resource limits".into())
        })
    }
}
