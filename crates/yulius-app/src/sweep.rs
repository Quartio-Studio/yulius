//! Sweeping Cargo build directories (SPEC-276 RDA-001..RDA-005, ADR-0205): every `target` of a
//! live sandbox's worktrees and CI checkouts loses the units nobody used for a while and stays
//! under its cap. Cargo never removes artifacts of old hashes, so without this every build adds.
//!
//! Safety: a sweep holds every profile's Cargo locks (`.cargo-lock`, `.cargo-build-lock`,
//! `.cargo-artifact-lock`: exclusive `flock`s Cargo holds while it compiles) and skips a build
//! directory whose lock is held, so it never deletes under a running compile and none starts until
//! it ends. Cargo releases them before test binaries, doctests and nextest read `deps/`, so
//! artifact units go only while nothing can run tests there: a CI checkout under the sandbox's CI
//! claim, a task's worktree while the task neither works nor runs background work. Incremental
//! units, which only rustc reads under the lock, go in any worktree, and only they are evicted for
//! the cap. It deletes whole units (a hash's fingerprint first, then its outputs), so Cargo
//! rebuilds exactly what went. Every path is reached from the sandbox directory with no link on the
//! way and removed through `O_NOFOLLOW` handles (SPEC-263).

use std::collections::HashMap;
use std::fs::File;
use std::os::unix::fs::{MetadataExt as _, OpenOptionsExt as _};
use std::path::{Path, PathBuf};

use yulius_domain::disk::human;
use yulius_domain::ids::TaskId;
use yulius_domain::sweep::{Area, Level, Policy, Unit, is_working_session, plan, policy, unit_key};

use crate::context::App;
use crate::leftovers::{no_link_between, remove_below};

/// Entries one sweep of a build directory reads at most; past it nothing is deleted.
const MAX_ENTRIES: u64 = 2_000_000;

/// What one build directory's sweep did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SweepOutcome {
    /// A Cargo run held its lock: nothing was read or deleted.
    pub busy: bool,
    /// Units deleted.
    pub units: usize,
    /// Bytes they held.
    pub freed: u64,
    /// "path: error" of what could not be deleted (or why nothing was).
    pub failed: Vec<String>,
}

/// What one sweep of every build directory did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SweepReport {
    /// Build directories swept.
    pub swept: usize,
    /// Build directories skipped because a Cargo run held them.
    pub busy: usize,
    /// Units deleted.
    pub units: usize,
    /// Bytes freed.
    pub freed: u64,
}

/// A unit found in a build directory: its paths (fingerprint first) and what it holds.
struct Found {
    paths: Vec<PathBuf>,
    unit: Unit,
}

fn real_dir(root: &Path, path: &Path) -> bool {
    no_link_between(root, path)
        && std::fs::symlink_metadata(path).is_ok_and(|m| m.is_dir() && !m.file_type().is_symlink())
}

fn children(dir: &Path) -> Vec<(String, PathBuf)> {
    std::fs::read_dir(dir)
        .map(|entries| {
            entries
                .flatten()
                .map(|e| (e.file_name().to_string_lossy().into_owned(), e.path()))
                .collect()
        })
        .unwrap_or_default()
}

/// The profile directories of `target` (those holding `.fingerprint`): `<target>/<profile>` and
/// `<target>/<triple>/<profile>`.
fn profiles(root: &Path, target: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    for (_, path) in children(target) {
        if !real_dir(root, &path) {
            continue;
        }
        if real_dir(root, &path.join(".fingerprint")) {
            found.push(path);
            continue;
        }
        for (_, inner) in children(&path) {
            if real_dir(root, &inner) && real_dir(root, &inner.join(".fingerprint")) {
                found.push(inner);
            }
        }
    }
    found
}

/// Whether `name` is one of Cargo's lock files in a profile directory.
fn is_cargo_lock(name: &str) -> bool {
    name == ".cargo-lock" || (name.starts_with(".cargo-") && name.ends_with("-lock"))
}

/// Cargo's locks of `profiles`, each taken without waiting; `None` when one is held or is not a
/// plain file (a link, or a FIFO an agent made: opened without blocking and refused). A profile
/// without lock files has none to take.
fn lock_profiles(root: &Path, profiles: &[PathBuf]) -> Option<Vec<File>> {
    use nix::fcntl::OFlag;
    let mut held = Vec::new();
    for profile in profiles {
        for (name, lock) in children(profile) {
            if !is_cargo_lock(&name) {
                continue;
            }
            if !no_link_between(root, &lock) {
                return None;
            }
            let flags = OFlag::O_NOFOLLOW | OFlag::O_NONBLOCK | OFlag::O_CLOEXEC;
            let opened = std::fs::OpenOptions::new()
                .read(true)
                .custom_flags(flags.bits())
                .open(&lock);
            match opened {
                Ok(file) => {
                    let plain = file.metadata().is_ok_and(|meta| meta.file_type().is_file());
                    if !plain || file.try_lock().is_err() {
                        return None;
                    }
                    held.push(file);
                }
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
                Err(_) => return None,
            }
        }
    }
    Some(held)
}

/// Whether the file system of `path` records no access times (`noatime`): then only modification
/// times tell use, and artifact units, which are read but not written, are not swept by age.
fn without_access_times(path: &Path) -> bool {
    #[cfg(target_os = "linux")]
    {
        use nix::sys::statvfs::{FsFlags, statvfs};
        statvfs(path).is_ok_and(|stat| stat.flags().contains(FsFlags::ST_NOATIME))
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = path;
        false
    }
}

fn millis(seconds: i64, nanos: i64) -> i64 {
    seconds
        .saturating_mul(1000)
        .saturating_add(nanos / 1_000_000)
}

/// Allocated bytes and the newest use (ms) of `path` and everything below it: a file's access or
/// modification time, a directory's modification time; links not followed. `None` once `budget`
/// entries were read.
fn measure(path: &Path, budget: &mut u64) -> Option<(u64, i64)> {
    let (mut bytes, mut newest) = (0u64, i64::MIN);
    let mut stack = vec![path.to_path_buf()];
    while let Some(next) = stack.pop() {
        *budget = budget.checked_sub(1)?;
        let Ok(meta) = std::fs::symlink_metadata(&next) else {
            continue;
        };
        bytes = bytes.saturating_add(meta.blocks().saturating_mul(512));
        newest = newest.max(millis(meta.mtime(), meta.mtime_nsec()));
        // A directory's access time moves whenever it is listed, by this sweep too: only its
        // files' reads count.
        if !meta.is_dir() {
            newest = newest.max(millis(meta.atime(), meta.atime_nsec()));
        }
        if meta.is_dir() {
            stack.extend(children(&next).into_iter().map(|(_, p)| p));
        }
    }
    Some((bytes, newest))
}

/// The units of `profiles`, keyed per profile, and whether [`MAX_ENTRIES`] cut the reading short:
/// then only the incremental units read whole are returned (an artifact unit's entries lie in three
/// directories, so one read in part could look unused).
fn units(root: &Path, profiles: &[PathBuf]) -> (Vec<Found>, bool) {
    let mut budget = MAX_ENTRIES;
    let mut found: HashMap<(usize, String), Found> = HashMap::new();
    let mut order: Vec<(usize, String)> = Vec::new();
    let mut cut = false;
    'profiles: for (index, profile) in profiles.iter().enumerate() {
        for (area, name) in Area::ALL {
            let dir = profile.join(name);
            if !real_dir(root, &dir) {
                continue;
            }
            for (entry, path) in children(&dir) {
                let Some(key) = unit_key(area, &entry) else {
                    continue;
                };
                let incremental = area == Area::Incremental;
                let working = incremental
                    && children(&path)
                        .iter()
                        .any(|(name, _)| is_working_session(name));
                let Some((bytes, newest)) = measure(&path, &mut budget) else {
                    cut = true;
                    break 'profiles;
                };
                let slot = found.entry((index, key.clone())).or_insert_with(|| {
                    order.push((index, key));
                    Found {
                        paths: Vec::new(),
                        unit: Unit {
                            last_used_ms: i64::MIN,
                            bytes: 0,
                            working: false,
                            incremental,
                        },
                    }
                });
                slot.paths.push(path);
                slot.unit.bytes = slot.unit.bytes.saturating_add(bytes);
                slot.unit.last_used_ms = slot.unit.last_used_ms.max(newest);
                slot.unit.working |= working;
            }
        }
    }
    let units = order
        .into_iter()
        .filter_map(|key| found.remove(&key))
        .filter(|found| !cut || found.unit.incremental)
        .collect();
    (units, cut)
}

/// Sweeps the build directory `target` below the sandbox directory `root` at `now_ms` with
/// `policy` (RDA-002..RDA-005); artifact units only with `artifacts` (nothing can run tests in it).
/// Blocking. Linux only (removals go through `/proc/self/fd`).
#[must_use]
pub fn sweep_target(
    root: &Path,
    target: &Path,
    now_ms: i64,
    policy: &Policy,
    artifacts: bool,
) -> SweepOutcome {
    let mut outcome = SweepOutcome::default();
    if !cfg!(target_os = "linux") || !real_dir(root, target) {
        return outcome;
    }
    let profiles = profiles(root, target);
    // Held until the end: no Cargo run starts in this build directory meanwhile.
    let Some(locks) = lock_profiles(root, &profiles) else {
        outcome.busy = true;
        return outcome;
    };
    let (found, cut) = units(root, &profiles);
    if cut {
        outcome.failed.push(format!(
            "{}: too many entries to read at once; only whole incremental units were swept",
            target.display()
        ));
    }
    let artifacts = artifacts && !cut && !without_access_times(target);
    let all: Vec<Unit> = found.iter().map(|f| f.unit.clone()).collect();
    for index in plan(&all, now_ms, policy, artifacts) {
        let unit = &found[index];
        let mut removed = true;
        for path in &unit.paths {
            if let Err(err) = remove_below(root, path) {
                outcome.failed.push(format!("{}: {err}", path.display()));
                removed = false;
                break;
            }
        }
        if removed {
            outcome.units += 1;
            outcome.freed = outcome.freed.saturating_add(unit.unit.bytes);
        }
    }
    drop(locks);
    outcome
}

/// The Cargo build directories of the sandbox directory `root`: `target` of every worktree and CI
/// checkout that has a `Cargo.toml`, with what to call each in the audit log.
fn targets(root: &Path) -> Vec<(String, PathBuf)> {
    let mut found = Vec::new();
    for (kind, dir) in [
        ("worktree", root.join("worktrees")),
        ("CI job", root.join("ci").join("jobs")),
    ] {
        if !real_dir(root, &dir) {
            continue;
        }
        for (name, checkout) in children(&dir) {
            let cargo =
                std::fs::symlink_metadata(checkout.join("Cargo.toml")).is_ok_and(|m| m.is_file());
            let target = checkout.join("target");
            if real_dir(root, &checkout) && cargo && real_dir(root, &target) {
                found.push((format!("{kind} {name}"), target));
            }
        }
    }
    found
}

impl App {
    /// Sweeps every Cargo build directory of the live sandboxes at the disk's `level` (RDA-001),
    /// each audited `disk.sweep` when it deleted something.
    pub async fn sweep_build_caches(&self, level: Level) -> SweepReport {
        let mut report = SweepReport::default();
        let Ok(sandboxes) = self.deps.store.sandboxes().await else {
            return report;
        };
        let total = self.data_disk().await.map(|(total, _)| total);
        let policy = policy(level, total);
        for sandbox in sandboxes.iter().filter(|s| s.removed_at.is_none()) {
            let root = self.sandbox_dir(&sandbox.id);
            let found = {
                let root = root.clone();
                tokio::task::spawn_blocking(move || targets(&root))
                    .await
                    .unwrap_or_default()
            };
            for (label, target) in found {
                // A CI checkout is swept under the sandbox's CI claim, so no run reads its
                // artifacts meanwhile; a running CI run keeps it whole.
                let ci = label.starts_with("CI job ");
                if ci && !self.ci_claim(&sandbox.id) {
                    report.busy += 1;
                    continue;
                }
                let artifacts = ci || self.worktree_rests(&label).await;
                let now = self.now().0;
                let (dir, path) = (root.clone(), target.clone());
                let outcome = tokio::task::spawn_blocking(move || {
                    sweep_target(&dir, &path, now, &policy, artifacts)
                })
                .await
                .unwrap_or_default();
                if ci {
                    self.ci_release(&sandbox.id);
                }
                for failure in &outcome.failed {
                    tracing::warn!(failure = %failure, "a build cache unit could not be swept");
                }
                if outcome.busy {
                    report.busy += 1;
                    continue;
                }
                report.swept += 1;
                report.units += outcome.units;
                report.freed = report.freed.saturating_add(outcome.freed);
                if outcome.units > 0 {
                    let label = self.build_dir_label(&label).await;
                    self.dlv_audit(
                        "yulius",
                        "disk.sweep",
                        &sandbox.slug,
                        &format!(
                            "target of {label}: {} build units, {} (unused, or over the {} cap; disk {})",
                            outcome.units,
                            human(outcome.freed),
                            human(policy.cap),
                            level.as_str()
                        ),
                    )
                    .await;
                }
            }
        }
        report
    }

    /// Whether the worktree of `label` belongs to a task that neither works nor runs background
    /// work (probed afresh), so nothing reads its build artifacts. A worktree no task owns may be
    /// an agent's own, in use: it never counts as resting.
    async fn worktree_rests(&self, label: &str) -> bool {
        let Some(name) = label.strip_prefix("worktree ") else {
            return false;
        };
        let Ok(id) = TaskId::parse(name) else {
            return false;
        };
        match self.deps.store.task(&id).await {
            Ok(Some(task)) => self.cache_owner(&task, true).await.is_some(),
            _ => false,
        }
    }

    /// "task #12" for a worktree a task owns, the label otherwise.
    async fn build_dir_label(&self, label: &str) -> String {
        if let Some(name) = label.strip_prefix("worktree ")
            && let Ok(id) = TaskId::parse(name)
            && let Ok(Some(task)) = self.deps.store.task(&id).await
        {
            return format!("task #{}", task.number);
        }
        label.to_owned()
    }

    /// Total and used bytes of the data directory's disk, `None` when unknown.
    pub(crate) async fn data_disk(&self) -> Option<(u64, u64)> {
        let probe = std::sync::Arc::clone(&self.deps.system);
        let dir = self.config.data_dir.clone();
        tokio::task::spawn_blocking(move || probe.disk(&dir))
            .await
            .ok()
            .flatten()
    }
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;
    use std::fs::FileTimes;
    use std::time::{Duration, SystemTime};
    use yulius_domain::sweep::{RECENT_MS, UNUSED_AFTER_MS};

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    const HOUR: u64 = 3600;
    const NOW_MS: i64 = 1_000 * 3600 * 1000;

    fn at(hours_before_now: u64) -> SystemTime {
        SystemTime::UNIX_EPOCH + Duration::from_secs(1000 * HOUR - hours_before_now * HOUR)
    }

    /// Writes `path` with `bytes` of content, read and written `hours` before [`NOW_MS`].
    fn file(path: &Path, bytes: usize, hours: u64) -> TestResult {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, vec![7u8; bytes])?;
        let times = FileTimes::new()
            .set_accessed(at(hours))
            .set_modified(at(hours));
        File::options().write(true).open(path)?.set_times(times)?;
        Ok(())
    }

    /// Ages the directory `dir` itself (its entries' writes made it new).
    fn age_dir(dir: &Path, hours: u64) -> TestResult {
        let times = FileTimes::new()
            .set_accessed(at(hours))
            .set_modified(at(hours));
        File::open(dir)?.set_times(times)?;
        Ok(())
    }

    /// A unit of `hash` in `profile`: fingerprint, deps output, build script output.
    fn unit(profile: &Path, name: &str, hash: &str, hours: u64) -> TestResult {
        file(
            &profile.join(format!(".fingerprint/{name}-{hash}/lib-{name}")),
            16,
            hours,
        )?;
        file(
            &profile.join(format!("deps/lib{name}-{hash}.rlib")),
            4096,
            hours,
        )?;
        file(&profile.join(format!("deps/{name}-{hash}.d")), 16, hours)?;
        file(
            &profile.join(format!("build/{name}-{hash}/out/gen.rs")),
            64,
            hours,
        )?;
        for dir in [
            format!(".fingerprint/{name}-{hash}"),
            format!("build/{name}-{hash}/out"),
            format!("build/{name}-{hash}"),
        ] {
            age_dir(&profile.join(dir), hours)?;
        }
        Ok(())
    }

    fn ok_policy() -> Policy {
        Policy {
            unused_after_ms: UNUSED_AFTER_MS,
            cap: u64::MAX,
            recent_ms: RECENT_MS,
        }
    }

    struct Fixture {
        _tmp: tempfile::TempDir,
        root: PathBuf,
        target: PathBuf,
        profile: PathBuf,
    }

    fn fixture() -> Result<Fixture, Box<dyn std::error::Error>> {
        let tmp = tempfile::tempdir()?;
        let root = tmp.path().join("sandbox");
        let target = root.join("worktrees/t1/target");
        let profile = target.join("debug");
        file(&profile.join(".cargo-lock"), 0, 0)?;
        std::fs::create_dir_all(profile.join(".fingerprint"))?;
        Ok(Fixture {
            _tmp: tmp,
            root,
            target,
            profile,
        })
    }

    const OLD: &str = "35f5dbd50d1a473d";
    const NEW: &str = "9caa50b098b82860";

    #[test]
    fn units_unused_for_two_days_go_whole_and_recent_ones_stay() -> TestResult {
        let f = fixture()?;
        unit(&f.profile, "adler2", OLD, 72)?;
        unit(&f.profile, "adler2", NEW, 1)?;
        file(&f.profile.join("yulius"), 32, 72)?;
        let outcome = sweep_target(&f.root, &f.target, NOW_MS, &ok_policy(), true);
        assert!(!outcome.busy && outcome.failed.is_empty(), "{outcome:?}");
        assert_eq!(outcome.units, 1);
        assert!(outcome.freed > 0);
        for gone in [
            format!(".fingerprint/adler2-{OLD}"),
            format!("deps/libadler2-{OLD}.rlib"),
            format!("deps/adler2-{OLD}.d"),
            format!("build/adler2-{OLD}"),
        ] {
            assert!(!f.profile.join(&gone).exists(), "{gone} goes");
        }
        assert!(
            f.profile
                .join(format!(".fingerprint/adler2-{NEW}"))
                .exists()
        );
        assert!(
            f.profile
                .join(format!("deps/libadler2-{NEW}.rlib"))
                .exists()
        );
        assert!(
            f.profile.join("yulius").exists(),
            "files outside units stay"
        );
        Ok(())
    }

    #[test]
    fn a_unit_read_recently_stays_even_if_written_long_ago() -> TestResult {
        let f = fixture()?;
        unit(&f.profile, "adler2", OLD, 72)?;
        let rlib = f.profile.join(format!("deps/libadler2-{OLD}.rlib"));
        File::options()
            .write(true)
            .open(&rlib)?
            .set_times(FileTimes::new().set_accessed(at(2)))?;
        let outcome = sweep_target(&f.root, &f.target, NOW_MS, &ok_policy(), true);
        assert_eq!(outcome.units, 0, "{outcome:?}");
        assert!(rlib.exists());
        Ok(())
    }

    #[test]
    fn a_held_cargo_lock_keeps_the_whole_build_directory() -> TestResult {
        let f = fixture()?;
        unit(&f.profile, "adler2", OLD, 72)?;
        let cargo = File::open(f.profile.join(".cargo-lock"))?;
        cargo.lock()?;
        let outcome = sweep_target(&f.root, &f.target, NOW_MS, &ok_policy(), true);
        assert!(outcome.busy);
        assert_eq!(outcome.units, 0);
        assert!(
            f.profile
                .join(format!("deps/libadler2-{OLD}.rlib"))
                .exists()
        );
        cargo.unlock()?;
        let outcome = sweep_target(&f.root, &f.target, NOW_MS, &ok_policy(), true);
        assert_eq!(outcome.units, 1, "released: the next sweep deletes it");
        Ok(())
    }

    #[test]
    fn a_cross_target_profile_s_lock_counts_too() -> TestResult {
        let f = fixture()?;
        let cross = f.target.join("aarch64-apple-darwin/debug");
        file(&cross.join(".cargo-lock"), 0, 0)?;
        unit(&cross, "adler2", OLD, 72)?;
        unit(&f.profile, "adler2", OLD, 72)?;
        let cargo = File::open(cross.join(".cargo-lock"))?;
        cargo.lock()?;
        let outcome = sweep_target(&f.root, &f.target, NOW_MS, &ok_policy(), true);
        assert!(outcome.busy);
        assert!(
            f.profile
                .join(format!("deps/libadler2-{OLD}.rlib"))
                .exists()
        );
        drop(cargo);
        let outcome = sweep_target(&f.root, &f.target, NOW_MS, &ok_policy(), true);
        assert_eq!(outcome.units, 2, "one per profile");
        assert!(!cross.join(format!("deps/libadler2-{OLD}.rlib")).exists());
        Ok(())
    }

    #[test]
    fn a_lock_file_that_is_a_link_counts_as_held() -> TestResult {
        let f = fixture()?;
        unit(&f.profile, "adler2", OLD, 72)?;
        std::fs::remove_file(f.profile.join(".cargo-lock"))?;
        let elsewhere = f.root.join("elsewhere");
        file(&elsewhere, 0, 0)?;
        std::os::unix::fs::symlink(&elsewhere, f.profile.join(".cargo-lock"))?;
        let outcome = sweep_target(&f.root, &f.target, NOW_MS, &ok_policy(), true);
        assert!(outcome.busy);
        assert!(
            f.profile
                .join(format!("deps/libadler2-{OLD}.rlib"))
                .exists()
        );
        Ok(())
    }

    #[test]
    fn an_incremental_directory_with_a_working_session_stays() -> TestResult {
        let f = fixture()?;
        let busy = f.profile.join("incremental/yulius_app-0u6e73f24ohkf");
        file(
            &busy.join("s-hnghq0vh2y-16tnakq-working/dep-graph.bin"),
            64,
            72,
        )?;
        let idle = f.profile.join("incremental/yulius_app-1gjl3mvrfnrw1");
        file(
            &idle.join("s-hnghq0vh2y-16tnakq-1wmf7kj1angb1/dep-graph.bin"),
            64,
            72,
        )?;
        for dir in [
            busy.join("s-hnghq0vh2y-16tnakq-working"),
            busy.clone(),
            idle.join("s-hnghq0vh2y-16tnakq-1wmf7kj1angb1"),
            idle.clone(),
        ] {
            age_dir(&dir, 72)?;
        }
        let outcome = sweep_target(&f.root, &f.target, NOW_MS, &ok_policy(), true);
        assert_eq!(outcome.units, 1, "{outcome:?}");
        assert!(busy.exists(), "rustc compiles there");
        assert!(!idle.exists());
        Ok(())
    }

    #[test]
    fn a_link_in_the_way_is_never_followed() -> TestResult {
        let f = fixture()?;
        let outside = f.root.parent().ok_or("parent")?.join("outside");
        unit(&outside, "adler2", OLD, 72)?;
        file(&outside.join(".cargo-lock"), 0, 0)?;
        let profile = f.target.join("release");
        std::os::unix::fs::symlink(&outside, &profile)?;
        std::fs::remove_dir(f.profile.join(".fingerprint"))?;
        std::os::unix::fs::symlink(outside.join(".fingerprint"), f.profile.join(".fingerprint"))?;
        let outcome = sweep_target(&f.root, &f.target, NOW_MS, &ok_policy(), true);
        assert_eq!(outcome.units, 0, "{outcome:?}");
        assert!(outside.join(format!("deps/libadler2-{OLD}.rlib")).exists());
        // A build directory that is itself a link is not swept either.
        let linked = f.root.join("worktrees/t2/target");
        std::fs::create_dir_all(linked.parent().ok_or("parent")?)?;
        std::os::unix::fs::symlink(&outside, &linked)?;
        assert_eq!(
            sweep_target(&f.root, &linked, NOW_MS, &ok_policy(), true),
            SweepOutcome::default()
        );
        assert!(outside.join(format!(".fingerprint/adler2-{OLD}")).exists());
        Ok(())
    }

    /// An incremental unit `name` of `profile`: one finished session, written `hours` ago.
    fn incremental(profile: &Path, name: &str, hours: u64) -> TestResult {
        let dir = profile.join(format!("incremental/{name}"));
        let session = dir.join("s-hnghq0vh2y-16tnakq-1wmf7kj1angb1");
        file(&session.join("dep-graph.bin"), 4096, hours)?;
        age_dir(&session, hours)?;
        age_dir(&dir, hours)
    }

    #[test]
    fn over_its_cap_a_build_directory_loses_its_least_recently_used_incremental_units() -> TestResult
    {
        let f = fixture()?;
        // Four incremental units of about 4 KiB each, none two days old.
        for (name, hours) in [("a-1", 5), ("b-2", 3), ("c-3", 2), ("d-4", 0)] {
            incremental(&f.profile, name, hours)?;
        }
        let one = sweep_target(&f.root, &f.target, NOW_MS, &ok_policy(), true);
        assert_eq!(one.units, 0, "no cap reached: {one:?}");
        let (found, _) = units(&f.root, std::slice::from_ref(&f.profile));
        let total: u64 = found.iter().map(|f| f.unit.bytes).sum();
        let incremental_bytes = found
            .iter()
            .find(|f| f.unit.incremental)
            .map(|f| f.unit.bytes)
            .ok_or("a unit")?;
        // A cap below the total whose three quarters hold two and a half units: two go.
        let goal = total - 2 * incremental_bytes + incremental_bytes / 2;
        let policy = Policy {
            cap: (goal / 3 + 1) * 4,
            ..ok_policy()
        };
        let outcome = sweep_target(&f.root, &f.target, NOW_MS, &policy, true);
        assert_eq!(outcome.units, 2, "{outcome:?}");
        let left = |name: &str| f.profile.join(format!("incremental/{name}")).exists();
        assert!(!left("a-1") && !left("b-2"), "oldest first");
        assert!(left("c-3") && left("d-4"));
        // The current build (used within the hour) stays even far above its cap, and so do
        // artifacts (their access time may lag a day).
        unit(&f.profile, "dep", OLD, 10)?;
        let tiny = Policy {
            cap: 1,
            ..ok_policy()
        };
        let outcome = sweep_target(&f.root, &f.target, NOW_MS, &tiny, true);
        assert_eq!(outcome.units, 1, "{outcome:?}");
        assert!(left("d-4"));
        assert!(f.profile.join(format!("deps/libdep-{OLD}.rlib")).exists());
        Ok(())
    }

    #[test]
    fn without_artifacts_only_incremental_units_go() -> TestResult {
        let f = fixture()?;
        unit(&f.profile, "adler2", OLD, 72)?;
        incremental(&f.profile, "yulius_app-0u6e73f24ohkf", 72)?;
        let outcome = sweep_target(&f.root, &f.target, NOW_MS, &ok_policy(), false);
        assert_eq!(outcome.units, 1, "{outcome:?}");
        assert!(
            f.profile
                .join(format!("deps/libadler2-{OLD}.rlib"))
                .exists(),
            "tests may still read it"
        );
        assert!(
            !f.profile
                .join("incremental/yulius_app-0u6e73f24ohkf")
                .exists()
        );
        Ok(())
    }

    #[test]
    fn any_held_cargo_lock_keeps_the_build_directory() -> TestResult {
        let f = fixture()?;
        unit(&f.profile, "adler2", OLD, 72)?;
        file(&f.profile.join(".cargo-build-lock"), 0, 0)?;
        let cargo = File::open(f.profile.join(".cargo-build-lock"))?;
        cargo.lock()?;
        let outcome = sweep_target(&f.root, &f.target, NOW_MS, &ok_policy(), true);
        assert!(outcome.busy);
        assert!(
            f.profile
                .join(format!("deps/libadler2-{OLD}.rlib"))
                .exists()
        );
        Ok(())
    }

    #[test]
    fn a_fifo_named_like_a_lock_counts_as_held_and_never_blocks() -> TestResult {
        let f = fixture()?;
        unit(&f.profile, "adler2", OLD, 72)?;
        std::fs::remove_file(f.profile.join(".cargo-lock"))?;
        nix::unistd::mkfifo(
            &f.profile.join(".cargo-lock"),
            nix::sys::stat::Mode::S_IRWXU,
        )?;
        let (root, target) = (f.root.clone(), f.target.clone());
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(sweep_target(&root, &target, NOW_MS, &ok_policy(), true));
        });
        let outcome = rx.recv_timeout(Duration::from_secs(10))?;
        assert!(outcome.busy, "{outcome:?}");
        assert!(
            f.profile
                .join(format!("deps/libadler2-{OLD}.rlib"))
                .exists()
        );
        Ok(())
    }

    #[test]
    fn only_cargo_checkouts_have_build_directories() -> TestResult {
        let f = fixture()?;
        file(&f.root.join("worktrees/t1/Cargo.toml"), 8, 0)?;
        file(&f.root.join("ci/jobs/quality/Cargo.toml"), 8, 0)?;
        std::fs::create_dir_all(f.root.join("ci/jobs/quality/target"))?;
        std::fs::create_dir_all(f.root.join("worktrees/node/target"))?;
        let mut found: Vec<String> = targets(&f.root)
            .into_iter()
            .map(|(label, _)| label)
            .collect();
        found.sort();
        assert_eq!(found, ["CI job quality", "worktree t1"]);
        Ok(())
    }
}
