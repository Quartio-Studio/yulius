//! Disk upkeep rules of a sandbox (SPEC-008, SPEC-001 NFR-009): which directories of a worktree
//! are build caches, and which caches to prune when the sandbox nears its quota.
//!
//! Pure: the caller lists directories, measures sizes and removes.

use crate::task::{Activity, TaskStatus};

/// Share of the quota (percent) from which caches of idle tasks are pruned.
pub const PRESSURE_PERCENT: u64 = 90;
/// Share of the quota (percent) pruning aims to get below.
pub const TARGET_PERCENT: u64 = 80;
/// A task idle this long has its caches pruned whatever the quota (F 2026-10-12: "agents leave
/// old compiles and worktrees ... it should keep things clean"); continuing rebuilds them.
pub const IDLE_PRUNE_AFTER_MS: i64 = 4 * 60 * 60 * 1000;
/// Names of ignored directories that only hold what a build or an install recreates: a worktree
/// whose ignored files are all such directories loses nothing when it is removed.
pub const REBUILDABLE: [&str; 12] = [
    "target",
    "node_modules",
    ".tools",
    ".venv",
    "venv",
    "__pycache__",
    ".pytest_cache",
    ".mypy_cache",
    ".ruff_cache",
    ".next",
    ".turbo",
    ".cache",
];

/// Whose cache it is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Owner {
    /// The task ended (closed, stopped, failed, cancelled): its cache is never needed soon.
    Ended,
    /// The task is alive but not working (waiting for input, paused, queued, chained).
    Idle,
}

/// Whose caches a task's are, by what it is doing; `None` for a task that works (or is
/// starting, switching or stopping, or idles while its background shells still run a build):
/// its caches are never touched. An idle task that has not moved for [`IDLE_PRUNE_AFTER_MS`]
/// (`quiet_ms`) counts as ended.
#[must_use]
pub fn owner_of(
    status: TaskStatus,
    activity: Option<Activity>,
    background_shells: bool,
    quiet_ms: i64,
) -> Option<Owner> {
    let idle = || {
        if quiet_ms >= IDLE_PRUNE_AFTER_MS {
            Owner::Ended
        } else {
            Owner::Idle
        }
    };
    match status {
        TaskStatus::Closed | TaskStatus::Stopped(_) | TaskStatus::Failed(_) => Some(Owner::Ended),
        TaskStatus::Queued | TaskStatus::Chained | TaskStatus::WaitingCapacity => Some(idle()),
        TaskStatus::Running if activity.is_some_and(Activity::is_idle) && !background_shells => {
            Some(idle())
        }
        TaskStatus::Running
        | TaskStatus::Starting
        | TaskStatus::Switching
        | TaskStatus::Stopping => None,
    }
}

/// Whether every ignored entry of a worktree (as `git ls-files --others --ignored --directory`
/// lists them) is a rebuildable directory ([`REBUILDABLE`]) or one of its declared `caches`.
#[must_use]
pub fn only_rebuildable(ignored: &[String], caches: &[String]) -> bool {
    ignored.iter().all(|entry| {
        // Git also lists an untracked directory that holds only ignored files (`e2e/` next to
        // `e2e/node_modules/`): its listed contents decide.
        if entry.ends_with('/')
            && ignored
                .iter()
                .any(|other| other.len() > entry.len() && other.starts_with(entry.as_str()))
        {
            return true;
        }
        let entry = entry.trim_end_matches('/');
        let last = entry.rsplit('/').next().unwrap_or(entry);
        REBUILDABLE.contains(&last)
            || caches
                .iter()
                .any(|cache| entry == cache || entry.starts_with(&format!("{cache}/")))
    })
}

/// A cache directory found in a worktree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    /// Whose it is.
    pub owner: Owner,
    /// Bytes it holds.
    pub bytes: u64,
}

/// Cache directories (relative to the worktree root) of a worktree whose root holds `entries`:
/// what its tooling is recognised by, plus `declared` (`[disk] caches` in `yulius.toml`).
/// Existence is the caller's check.
#[must_use]
pub fn cache_paths(entries: &[String], declared: &[String]) -> Vec<String> {
    let has = |name: &str| entries.iter().any(|e| e == name);
    let mut paths: Vec<String> = Vec::new();
    if has("Cargo.toml") {
        paths.push("target".into());
    }
    if has("package.json") {
        paths.push("node_modules".into());
        paths.push(".next/cache".into());
    }
    if has("pyproject.toml") || has("requirements.txt") {
        paths.push(".venv".into());
    }
    for path in declared {
        let first = path.split('/').next().unwrap_or_default();
        if first != ".git" && !paths.contains(path) {
            paths.push(path.clone());
        }
    }
    paths
}

/// The largest cache one CI job checkout may keep: a quarter of the quota. Persistent
/// checkouts never shrink (every commit adds artifacts), so one beyond it is pruned whenever
/// no run is executing.
#[must_use]
pub fn job_cache_cap(limit: u64) -> u64 {
    if limit == 0 { u64::MAX } else { limit / 4 }
}

/// Bytes to free so that `used` falls to [`TARGET_PERCENT`] of `limit`; 0 below
/// [`PRESSURE_PERCENT`].
#[must_use]
pub fn bytes_to_free(used: u64, limit: u64) -> u64 {
    if limit == 0 || u128::from(used) * 100 < u128::from(limit) * u128::from(PRESSURE_PERCENT) {
        return 0;
    }
    let target = u128::from(limit) * u128::from(TARGET_PERCENT) / 100;
    u64::try_from(u128::from(used).saturating_sub(target)).unwrap_or(u64::MAX)
}

/// Indices of the candidates to prune: every cache of an ended task, and with `pressure` the
/// largest caches of idle tasks until `need` bytes are freed in all. Working tasks are never
/// candidates (the caller does not list them).
#[must_use]
pub fn plan_prune(candidates: &[Candidate], need: u64, pressure: bool) -> Vec<usize> {
    let mut chosen: Vec<usize> = (0..candidates.len())
        .filter(|&i| candidates[i].owner == Owner::Ended && candidates[i].bytes > 0)
        .collect();
    let mut freed: u64 = chosen.iter().map(|&i| candidates[i].bytes).sum();
    if pressure && freed < need {
        let mut idle: Vec<usize> = (0..candidates.len())
            .filter(|&i| candidates[i].owner == Owner::Idle && candidates[i].bytes > 0)
            .collect();
        idle.sort_by_key(|&i| std::cmp::Reverse(candidates[i].bytes));
        for i in idle {
            if freed >= need {
                break;
            }
            freed = freed.saturating_add(candidates[i].bytes);
            chosen.push(i);
        }
    }
    chosen
}

/// A size for people: "38.0 GB".
#[must_use]
pub fn human(bytes: u64) -> String {
    #[allow(clippy::cast_precision_loss)]
    let gb = bytes as f64 / 1e9;
    if gb >= 1.0 {
        format!("{gb:.1} GB")
    } else {
        #[allow(clippy::cast_precision_loss)]
        let mb = bytes as f64 / 1e6;
        format!("{mb:.0} MB")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::task::StopReason;

    fn names(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn caches_are_recognised_by_their_tooling_and_declared_ones_are_added() {
        assert_eq!(
            cache_paths(&names(&["Cargo.toml", "src"]), &[]),
            names(&["target"])
        );
        assert_eq!(
            cache_paths(
                &names(&["package.json"]),
                &names(&["build/cache", ".git/hooks"])
            ),
            names(&["node_modules", ".next/cache", "build/cache"])
        );
        assert_eq!(
            cache_paths(&names(&["pyproject.toml"]), &[]),
            names(&[".venv"])
        );
        assert!(cache_paths(&names(&["README.md"]), &[]).is_empty());
        assert_eq!(
            cache_paths(&names(&["Cargo.toml"]), &names(&["target"])),
            names(&["target"]),
            "no duplicates"
        );
    }

    #[test]
    fn one_job_cache_may_hold_a_quarter_of_the_quota() {
        assert_eq!(job_cache_cap(100), 25);
        assert_eq!(job_cache_cap(0), u64::MAX, "no quota, no cap");
    }

    #[test]
    fn pressure_starts_at_ninety_percent_and_aims_at_eighty() {
        assert_eq!(bytes_to_free(89, 100), 0);
        assert_eq!(bytes_to_free(90, 100), 10);
        assert_eq!(bytes_to_free(112, 100), 32);
        assert_eq!(bytes_to_free(5, 0), 0, "no quota, no pressure");
    }

    fn cand(owner: Owner, bytes: u64) -> Candidate {
        Candidate { owner, bytes }
    }

    #[test]
    fn ended_caches_always_go_and_idle_ones_only_under_pressure_largest_first() {
        let all = [
            cand(Owner::Idle, 10),
            cand(Owner::Ended, 5),
            cand(Owner::Idle, 40),
            cand(Owner::Idle, 30),
            cand(Owner::Ended, 0),
        ];
        assert_eq!(plan_prune(&all, 0, false), vec![1]);
        assert_eq!(
            plan_prune(&all, 100, false),
            vec![1],
            "no pressure: idle stays"
        );
        assert_eq!(
            plan_prune(&all, 40, true),
            vec![1, 2],
            "stops once enough is freed"
        );
        assert_eq!(plan_prune(&all, 70, true), vec![1, 2, 3]);
        assert_eq!(plan_prune(&all, 4, true), vec![1], "ended alone was enough");
    }

    #[test]
    fn only_tasks_that_do_not_work_own_prunable_caches() {
        let owner = |status, activity| owner_of(status, activity, false, 0);
        assert_eq!(owner(TaskStatus::Closed, None), Some(Owner::Ended));
        assert_eq!(
            owner(TaskStatus::Stopped(StopReason::Cancelled), None),
            Some(Owner::Ended)
        );
        assert_eq!(
            owner(TaskStatus::Running, Some(Activity::Waiting)),
            Some(Owner::Idle)
        );
        assert_eq!(owner(TaskStatus::Chained, None), Some(Owner::Idle));
        for working in [
            owner(TaskStatus::Running, Some(Activity::Working)),
            owner(TaskStatus::Running, Some(Activity::NeedsAttention)),
            owner(TaskStatus::Running, None),
            owner(TaskStatus::Starting, None),
            owner(TaskStatus::Switching, None),
            owner(TaskStatus::Stopping, None),
        ] {
            assert_eq!(working, None);
        }
    }

    #[test]
    fn background_shells_keep_a_task_working_and_long_idleness_ends_it() {
        // F 2026-10-12: upkeep deleted target/ under an idle agent's running gate.
        assert_eq!(
            owner_of(TaskStatus::Running, Some(Activity::Waiting), true, 0),
            None
        );
        assert_eq!(
            owner_of(
                TaskStatus::Running,
                Some(Activity::Done),
                true,
                IDLE_PRUNE_AFTER_MS
            ),
            None
        );
        assert_eq!(
            owner_of(
                TaskStatus::Running,
                Some(Activity::Waiting),
                false,
                IDLE_PRUNE_AFTER_MS
            ),
            Some(Owner::Ended)
        );
        assert_eq!(
            owner_of(TaskStatus::Chained, None, false, IDLE_PRUNE_AFTER_MS - 1),
            Some(Owner::Idle)
        );
    }

    #[test]
    fn a_worktree_whose_ignored_files_are_rebuildable_loses_nothing() {
        assert!(only_rebuildable(
            &names(&[
                "target/",
                ".tools/",
                "e2e/node_modules/",
                "crates/x/__pycache__/"
            ]),
            &[]
        ));
        assert!(only_rebuildable(
            &names(&["build/cache/"]),
            &names(&["build/cache"])
        ));
        assert!(only_rebuildable(&[], &[]));
        // An untracked directory holding only ignored files is listed too; its contents decide.
        assert!(only_rebuildable(
            &names(&["e2e/", "e2e/node_modules/"]),
            &[]
        ));
        assert!(!only_rebuildable(
            &names(&["e2e/", "e2e/node_modules/", "e2e/.env"]),
            &[]
        ));
        // Local configuration or data that only lives here keeps the worktree.
        assert!(!only_rebuildable(&names(&["target/", ".env"]), &[]));
        assert!(!only_rebuildable(
            &names(&["notes/"]),
            &names(&["build/cache"])
        ));
    }

    #[test]
    fn sizes_read_like_a_person_writes_them() {
        assert_eq!(human(38_000_000_000), "38.0 GB");
        assert_eq!(human(7_500_000), "8 MB");
    }
}
