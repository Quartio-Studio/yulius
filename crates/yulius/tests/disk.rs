//! Disk upkeep end to end (SPEC-008, SPEC-001 NFR-009): a real repository with real worktrees,
//! the real Git adapter and tasks in every state. Build caches of tasks that do not work are
//! removed, the worktrees of closed and cancelled tasks go, idle tasks lose theirs only under
//! pressure, working tasks never, and the agent is told what was freed.
//! Linux only: elsewhere the upkeep finds no cache to prune (SPEC-263).
#![cfg(target_os = "linux")]

use crate::support;

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};

use support::{HarnessOptions, harness_with, seed_github_user, seed_sandbox, seed_task};
use yulius::wiring::git::GitAdapter;
use yulius_app::delivery::DeliveryRecord;
use yulius_app::error::AppResult;
use yulius_app::model::SandboxRecord;
use yulius_app::model::{AttemptRecord, LaunchKind};
use yulius_app::ports::isolation::{DiskUsage, SandboxIsolation, SandboxJoin, SandboxRef};
use yulius_app::ports::session::{AttachedRun, LaunchPlan, RunInspection, SessionHost, SpawnedRun};
use yulius_app::ports::store::{CoordinationStore as _, TaskFieldUpdate, TaskStore as _};
use yulius_app::ports::store_ext::AutomationStore as _;
use yulius_app::ports::store_ext::DeliveryStore as _;
use yulius_domain::delivery::DeliveryStage;
use yulius_domain::harness::Harness as Agent;
use yulius_domain::ids::{AttemptId, SandboxId, TaskId};
use yulius_domain::task::{Activity, AttemptState, StopReason, TaskStatus};

type BoxResult<T> = Result<T, Box<dyn std::error::Error>>;

fn git(dir: &Path, args: &[&str]) -> BoxResult<String> {
    let out = Command::new("git")
        .args([
            "-c",
            "user.name=Yulius Test",
            "-c",
            "user.email=test@yulius.test",
            "-c",
            "commit.gpgsign=false",
            "-c",
            "init.defaultBranch=main",
        ])
        .args(args)
        .current_dir(dir)
        .output()?;
    if !out.status.success() {
        return Err(format!("git {args:?}: {}", String::from_utf8_lossy(&out.stderr)).into());
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_owned())
}

/// A quota whose reading the test sets.
struct Quota {
    used: AtomicU64,
    limit: u64,
}

#[async_trait::async_trait]
impl SandboxIsolation for Quota {
    async fn prepare(&self, _: &SandboxRef) -> AppResult<Option<SandboxJoin>> {
        Ok(None)
    }
    async fn disk(&self, _: &SandboxRef) -> Option<DiskUsage> {
        Some(DiskUsage {
            used: self.used.load(Ordering::SeqCst),
            limit: self.limit,
        })
    }
    async fn measure_disk(&self, r: &SandboxRef) -> Option<DiskUsage> {
        self.disk(r).await
    }
    fn release(&self, _: &SandboxId) {}
}

struct World {
    h: support::Harness,
    /// The sandbox's owner, signed in.
    alice: support::Creds,
    sandbox: SandboxRecord,
    repo: PathBuf,
    root: PathBuf,
}

const BIG: usize = 256 * 1024;

async fn world() -> BoxResult<World> {
    world_with(None).await
}

/// [`world`] with a process host of the test's own (default: none available).
async fn world_with(session_host: Option<Arc<dyn SessionHost>>) -> BoxResult<World> {
    let h = harness_with(HarnessOptions {
        git: Some(Arc::new(GitAdapter::new("git"))),
        session_host,
        ..HarnessOptions::default()
    })?;
    let alice = seed_github_user(&h, "u-alice", "alice").await?;
    h.github
        .set_perm("alice", "acme/app", support::perms("write"));
    let sandbox = seed_sandbox(&h, "s1", "app", "u-alice").await?;
    let root = h.app.config.data_dir.join("sandboxes/s1");
    let repo = root.join("repo");
    std::fs::create_dir_all(&repo)?;
    git(&repo, &["init", "-q", "-b", "main"])?;
    std::fs::write(repo.join("Cargo.toml"), "[package]\n")?;
    std::fs::write(repo.join(".gitignore"), "target\nout\n")?;
    std::fs::write(repo.join("yulius.toml"), "[disk]\ncaches = \"out src\"\n")?;
    std::fs::create_dir_all(repo.join("src"))?;
    std::fs::write(repo.join("src/lib.rs"), "// tracked\n")?;
    git(&repo, &["add", "-A"])?;
    git(&repo, &["commit", "-q", "-m", "init"])?;
    Ok(World {
        h,
        alice,
        sandbox,
        repo,
        root,
    })
}

impl World {
    /// A task in `status` with a worktree holding a `target` (and `out`, declared) of ~256 KiB.
    async fn task(
        &self,
        id: &str,
        status: TaskStatus,
        activity: Option<Activity>,
    ) -> BoxResult<PathBuf> {
        let worktree = self.root.join("worktrees").join(id);
        git(
            &self.repo,
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                &format!("feat/{id}"),
                worktree.to_str().ok_or("path")?,
            ],
        )?;
        for cache in ["target", "out"] {
            std::fs::create_dir_all(worktree.join(cache))?;
            std::fs::write(worktree.join(cache).join("blob"), vec![7u8; BIG])?;
        }
        let task = seed_task(&self.h, id, "s1", "u-alice", status, Some(&worktree)).await?;
        if activity.is_some() {
            self.h
                .store
                .set_activity(&task.id, activity, self.h.app.now())
                .await?;
        }
        Ok(worktree)
    }

    /// Moves the clock past the grace period of ended tasks.
    fn later(&self) {
        self.h
            .clock
            .0
            .fetch_add(yulius_app::disk::ENDED_GRACE_MS + 1_000, Ordering::SeqCst);
    }

    async fn told(&self, task: &str) -> BoxResult<Vec<String>> {
        Ok(self
            .h
            .store
            .inbox(&TaskId::parse(task)?, false, 50)
            .await?
            .into_iter()
            .map(|e| e.message.body)
            .collect())
    }
}

#[tokio::test]
async fn caches_and_worktrees_of_ended_tasks_go_after_a_grace_period() -> BoxResult<()> {
    let w = world().await?;
    let closed = w.task("t-closed", TaskStatus::Closed, None).await?;
    let cancelled = w
        .task("t-cancel", TaskStatus::Stopped(StopReason::Cancelled), None)
        .await?;
    let crashed = w
        .task("t-crash", TaskStatus::Stopped(StopReason::Crashed), None)
        .await?;
    let working = w
        .task("t-work", TaskStatus::Running, Some(Activity::Working))
        .await?;
    // Dirty: its worktree stays (uncommitted work), its caches go.
    let dirty = w.task("t-dirty", TaskStatus::Closed, None).await?;
    std::fs::write(dirty.join("notes.txt"), "uncommitted")?;

    let early = w.h.app.disk_upkeep(&w.sandbox, false).await?;
    assert_eq!(early.caches, 0, "a task that just ended keeps its caches");
    assert!(closed.join("target").exists());

    w.later();
    let report = w.h.app.disk_upkeep(&w.sandbox, false).await?;
    assert!(!closed.exists(), "closed: the worktree is gone");
    assert!(!cancelled.exists(), "cancelled: the worktree is gone");
    assert_eq!(report.worktrees, 2, "{report:?}");
    assert!(
        crashed.exists() && !crashed.join("target").exists() && !crashed.join("out").exists(),
        "a crashed task keeps its worktree, loses its caches"
    );
    assert!(dirty.join("notes.txt").exists() && !dirty.join("target").exists());
    assert!(
        working.join("target").exists() && working.join("out").exists(),
        "a working task is never touched"
    );
    assert!(report.freed >= 4 * 2 * BIG as u64, "{report:?}");
    // The branch of a removed worktree stays, so Continue can recreate it.
    assert!(git(&w.repo, &["branch", "--list", "feat/t-closed"])?.contains("feat/t-closed"));
    // MSR-002 (SPEC-028): nobody is told through messages.
    assert!(w.told("t-crash").await?.is_empty());
    assert!(w.told("t-closed").await?.is_empty());
    assert!(w.told("t-work").await?.is_empty());
    Ok(())
}

#[tokio::test]
async fn idle_tasks_lose_their_caches_only_under_pressure_and_the_largest_first() -> BoxResult<()> {
    let w = world().await?;
    let idle = w
        .task("t-idle", TaskStatus::Running, Some(Activity::Waiting))
        .await?;
    let big = w.task("t-big", TaskStatus::Chained, None).await?;
    std::fs::write(big.join("target/more"), vec![1u8; 4 * BIG])?;
    let working = w
        .task("t-work", TaskStatus::Running, Some(Activity::Working))
        .await?;
    let quota = Arc::new(Quota {
        used: AtomicU64::new(50),
        limit: 100,
    });
    assert!(w.h.app.set_isolation(quota.clone()));

    // 50 % of the quota: nothing happens.
    w.h.app.disk_upkeep(&w.sandbox, false).await?;
    assert!(idle.join("target").exists() && big.join("target").exists());

    // 95 %: the sandbox needs 15 bytes back; the largest cache alone covers it.
    quota.used.store(95, Ordering::SeqCst);
    let report = w.h.app.disk_upkeep(&w.sandbox, false).await?;
    assert!(!big.join("target").exists(), "the largest idle cache went");
    assert!(
        idle.join("target").exists(),
        "pruning stops once enough is freed"
    );
    assert!(
        working.join("target").exists(),
        "a working task is never pruned"
    );
    assert_eq!(report.caches, 1, "{report:?}");
    assert!(w.told("t-big").await?.is_empty(), "MSR-002: no message");
    Ok(())
}

/// SPEC-215 LFT-005 (ADR-0170): a done task loses its caches after 30 quiet minutes, keeps its
/// worktree and uncommitted work; a waiting task keeps its caches for 4 hours.
#[tokio::test]
async fn a_done_task_loses_its_caches_after_half_an_hour_and_keeps_its_worktree() -> BoxResult<()> {
    let w = world().await?;
    let done = w
        .task("t-done", TaskStatus::Running, Some(Activity::Done))
        .await?;
    std::fs::write(done.join("notes.txt"), "uncommitted")?;
    let waiting = w
        .task("t-wait", TaskStatus::Running, Some(Activity::Waiting))
        .await?;

    w.h.clock.0.fetch_add(20 * 60 * 1000, Ordering::SeqCst);
    w.h.app.disk_upkeep(&w.sandbox, false).await?;
    assert!(
        done.join("target").exists(),
        "20 quiet minutes: a done task keeps its caches"
    );

    w.h.clock.0.fetch_add(11 * 60 * 1000, Ordering::SeqCst);
    let report = w.h.app.disk_upkeep(&w.sandbox, false).await?;
    assert!(
        !done.join("target").exists() && !done.join("out").exists(),
        "31 quiet minutes: a done task's caches go ({report:?})"
    );
    assert!(
        done.join("notes.txt").exists() && done.join("src/lib.rs").exists(),
        "its worktree and uncommitted work stay"
    );
    assert!(
        waiting.join("target").exists(),
        "a waiting task keeps its caches for 4 hours"
    );
    assert_eq!(report.worktrees, 0, "{report:?}");
    Ok(())
}

#[tokio::test]
async fn a_cache_that_is_a_link_is_never_followed() -> BoxResult<()> {
    let w = world().await?;
    let closed = w
        .task("t-link", TaskStatus::Stopped(StopReason::Crashed), None)
        .await?;
    let outside = w.root.join("outside");
    std::fs::create_dir_all(&outside)?;
    std::fs::write(outside.join("precious"), "keep")?;
    std::fs::remove_dir_all(closed.join("target"))?;
    std::os::unix::fs::symlink(&outside, closed.join("target"))?;
    w.later();
    w.h.app.disk_upkeep(&w.sandbox, false).await?;
    assert!(outside.join("precious").exists());
    assert!(
        closed.join("target").exists(),
        "the link itself is not a cache directory"
    );
    Ok(())
}

#[tokio::test]
async fn a_refused_start_over_quota_prunes_and_asks_again() -> BoxResult<()> {
    let w = world().await?;
    let idle = w
        .task("t-idle", TaskStatus::Running, Some(Activity::Waiting))
        .await?;
    struct Over(AtomicU64);
    #[async_trait::async_trait]
    impl SandboxIsolation for Over {
        async fn prepare(&self, _: &SandboxRef) -> AppResult<Option<SandboxJoin>> {
            if self.0.load(Ordering::SeqCst) > 100 {
                return Err(yulius_app::error::AppError::Conflict("over quota".into()));
            }
            Ok(None)
        }
        async fn disk(&self, _: &SandboxRef) -> Option<DiskUsage> {
            Some(DiskUsage {
                used: self.0.load(Ordering::SeqCst),
                limit: 100,
            })
        }
        async fn measure_disk(&self, r: &SandboxRef) -> Option<DiskUsage> {
            // Freed space is visible in the next measurement.
            self.0.store(10, Ordering::SeqCst);
            self.disk(r).await
        }
        fn release(&self, _: &SandboxId) {}
    }
    let over = Arc::new(Over(AtomicU64::new(112)));
    assert!(w.h.app.set_isolation(over));
    let task =
        w.h.store
            .task(&TaskId::parse("t-idle")?)
            .await?
            .ok_or("task")?;
    w.h.app.preflight_start(&task, None).await?;
    assert!(
        !idle.join("target").exists(),
        "the cache was pruned before refusing"
    );
    Ok(())
}

#[tokio::test]
async fn declared_paths_with_tracked_files_and_unignored_paths_are_never_pruned() -> BoxResult<()> {
    let w = world().await?;
    // `src` is declared in yulius.toml but holds tracked files; `docs-out` is declared too but
    // Git does not ignore it.
    let crashed = w
        .task("t-crash", TaskStatus::Stopped(StopReason::Crashed), None)
        .await?;
    std::fs::write(
        crashed.join("yulius.toml"),
        "[disk]\ncaches = \"out src docs-out\"\n",
    )?;
    std::fs::create_dir_all(crashed.join("docs-out"))?;
    std::fs::write(crashed.join("docs-out/page.md"), "work")?;
    w.later();
    w.h.app.disk_upkeep(&w.sandbox, false).await?;
    assert!(
        crashed.join("src/lib.rs").exists(),
        "tracked files are work"
    );
    assert!(
        crashed.join("docs-out/page.md").exists(),
        "not ignored: work"
    );
    assert!(!crashed.join("out").exists() && !crashed.join("target").exists());
    Ok(())
}

#[tokio::test]
async fn a_closed_tasks_worktree_with_ignored_local_files_is_kept() -> BoxResult<()> {
    let w = world().await?;
    let closed = w.task("t-env", TaskStatus::Closed, None).await?;
    let exclude = git(&closed, &["rev-parse", "--git-path", "info/exclude"])?;
    let exclude = if Path::new(&exclude).is_absolute() {
        PathBuf::from(exclude)
    } else {
        closed.join(exclude)
    };
    std::fs::create_dir_all(exclude.parent().ok_or("parent")?)?;
    std::fs::write(&exclude, ".env\n")?;
    std::fs::write(closed.join(".env"), "SECRET=1")?;
    w.later();
    let report = w.h.app.disk_upkeep(&w.sandbox, false).await?;
    assert_eq!(report.worktrees, 0, "{report:?}");
    assert!(closed.join(".env").exists(), "local configuration survives");
    assert!(!closed.join("target").exists(), "its caches still went");
    Ok(())
}

#[tokio::test]
async fn hygiene_keeps_an_ended_worktree_that_still_holds_ignored_files() -> BoxResult<()> {
    let w = world().await?;
    // `target` and `out` are caches; `.env` is ignored and no cache (local configuration).
    let closed = w.task("t-hyg", TaskStatus::Closed, None).await?;
    w.local_config(&closed)?;
    let result =
        w.h.app
            .hygiene_cleanup_as(
                &w.sandbox,
                &yulius_contracts::api::ci::HygieneCleanup::default(),
                "test",
            )
            .await?;
    assert!(closed.exists(), "{result:?}");
    assert!(
        result.skipped.iter().any(|s| s.reason.contains("ignores")),
        "{result:?}"
    );
    Ok(())
}

impl World {
    /// An `.env` in `worktree` that Git ignores (through the repository's `info/exclude`) and
    /// that is no rebuildable cache.
    fn local_config(&self, worktree: &Path) -> BoxResult<()> {
        let exclude = self.repo.join(".git/info/exclude");
        std::fs::create_dir_all(exclude.parent().ok_or("parent")?)?;
        std::fs::write(&exclude, ".env\n")?;
        std::fs::write(worktree.join(".env"), "TOKEN=local\n")?;
        Ok(())
    }

    async fn archive(&self, id: &str) -> BoxResult<()> {
        self.h
            .store
            .update_task(
                &TaskId::parse(id)?,
                &TaskFieldUpdate {
                    archived_at: Some(Some(self.h.app.now())),
                    ..TaskFieldUpdate::default()
                },
                self.h.app.now(),
            )
            .await?;
        Ok(())
    }
}

/// SPEC-269 (review round 1): the automatic round keeps a paused task's worktree (its process is
/// alive and resumes in it), and an archived task's worktree that holds ignored files nobody can
/// rebuild; an archived one holding only caches goes.
#[tokio::test]
async fn the_hygiene_round_keeps_paused_work_and_files_nobody_can_rebuild() -> BoxResult<()> {
    let w = world().await?;
    let paused = w
        .task("t-paused", TaskStatus::WaitingCapacity, None)
        .await?;
    w.h.store
        .transition_delivery(
            &DeliveryStage::None,
            &DeliveryRecord {
                task_id: TaskId::parse("t-paused")?,
                stage: DeliveryStage::MergedIntegration,
                pr_number: Some(7),
                updated_at: w.h.app.now(),
            },
        )
        .await?;
    let kept = w.task("t-local", TaskStatus::Closed, None).await?;
    w.local_config(&kept)?;
    w.archive("t-local").await?;
    let caches = w.task("t-caches", TaskStatus::Closed, None).await?;
    w.archive("t-caches").await?;

    // A snapshot import's checkout, clean right after Git added it, is the import's.
    let import = w.root.join("worktrees/import-job1");
    git(
        &w.repo,
        &[
            "worktree",
            "add",
            "-q",
            "--detach",
            import.to_str().ok_or("path")?,
        ],
    )?;

    w.h.app.hygiene_round().await;
    assert!(import.exists(), "an import's checkout stays");
    assert!(paused.exists(), "a paused task's worktree stays");
    assert!(kept.exists(), "ignored files nobody can rebuild stay");
    assert!(!caches.exists(), "only caches: removed");
    let report = w.h.app.hygiene_report(&w.sandbox).await;
    assert!(
        report.items.iter().all(|i| !i.name.contains("import-")),
        "{report:?}"
    );
    let last = report.last_cleanup.ok_or("last cleanup")?;
    assert!(
        last.skipped
            .iter()
            .any(|s| s.target.name == "worktrees/t-local" && s.reason.contains("ignores")),
        "{last:?}"
    );
    Ok(())
}

impl World {
    /// A persistent CI job checkout (`ci/jobs/<name>`) holding a `target` of ~256 KiB.
    fn ci_checkout(&self, name: &str) -> BoxResult<PathBuf> {
        let checkout = self.root.join("ci/jobs").join(name);
        std::fs::create_dir_all(checkout.parent().ok_or("parent")?)?;
        git(
            &self.repo,
            &[
                "worktree",
                "add",
                "-q",
                "--detach",
                checkout.to_str().ok_or("path")?,
            ],
        )?;
        std::fs::create_dir_all(checkout.join("target"))?;
        std::fs::write(checkout.join("target/blob"), vec![5u8; BIG])?;
        Ok(checkout)
    }
}

#[tokio::test]
async fn a_ci_job_cache_beyond_a_quarter_of_the_quota_is_pruned_at_once() -> BoxResult<()> {
    let w = world().await?;
    let checkout = w.ci_checkout("quality")?;
    let idle = w.task("t-idle", TaskStatus::Chained, None).await?;
    // The quota is tiny, so a quarter of it is smaller than the cache; usage is far below 90 %.
    let quota = Arc::new(Quota {
        used: AtomicU64::new(1),
        limit: 100 * BIG as u64,
    });
    // A quarter is 25 * BIG: the 256 KiB cache is under it and stays while nothing presses.
    assert!(w.h.app.set_isolation(quota.clone()));
    w.h.app.disk_upkeep(&w.sandbox, false).await?;
    assert!(
        checkout.join("target").exists(),
        "under the cap, no pressure"
    );

    // A cache of 30 * BIG is over the quarter: gone at once. The idle task's is not.
    std::fs::write(checkout.join("target/big"), vec![6u8; 30 * BIG])?;
    w.h.app.disk_upkeep(&w.sandbox, false).await?;
    assert!(
        !checkout.join("target").exists(),
        "over a quarter of the quota"
    );
    assert!(
        idle.join("target").exists(),
        "an idle task's cache needs pressure"
    );
    Ok(())
}

#[tokio::test]
async fn ci_job_caches_are_pruned_under_pressure_and_before_a_refusal() -> BoxResult<()> {
    let w = world().await?;
    let checkout = w.ci_checkout("quality")?;
    let quota = Arc::new(Quota {
        used: AtomicU64::new(95 * BIG as u64),
        limit: 100 * BIG as u64,
    });
    assert!(w.h.app.set_isolation(quota));
    let report = w.h.app.disk_upkeep(&w.sandbox, false).await?;
    assert!(!checkout.join("target").exists(), "{report:?}");
    assert!(checkout.exists(), "the checkout itself stays");
    // The same through a refused start: the cache is pruned and the start asked again.
    let again = w.ci_checkout("lint")?;
    w.h.app.disk_upkeep(&w.sandbox, true).await?;
    assert!(!again.join("target").exists());
    Ok(())
}

#[tokio::test]
async fn a_ci_cache_stays_while_a_run_executes_and_goes_once_it_ends() -> BoxResult<()> {
    let w = world().await?;
    let checkout = w.ci_checkout("quality")?;
    let quota = Arc::new(Quota {
        used: AtomicU64::new(95 * BIG as u64),
        limit: 100 * BIG as u64,
    });
    assert!(w.h.app.set_isolation(quota));
    // A run executes (or another claim holds the checkouts): nothing in them is touched.
    assert!(w.h.app.ci_claim(&w.sandbox.id));
    w.h.app.disk_upkeep(&w.sandbox, false).await?;
    assert!(checkout.join("target").exists(), "a run is using it");
    w.h.app.ci_release(&w.sandbox.id);
    w.h.app.disk_upkeep(&w.sandbox, false).await?;
    assert!(!checkout.join("target").exists());
    // The upkeep released its own claim: the dispatcher can claim again.
    assert!(w.h.app.ci_claim(&w.sandbox.id));
    w.h.app.ci_release(&w.sandbox.id);
    Ok(())
}

/// SPEC-019 DSK-003: a closed task's worktree whose only ignored files are rebuildable (the
/// repository's tool directory, Node modules) is removed; before, any ignored file kept it forever.
#[tokio::test]
async fn an_ended_worktree_holding_only_rebuildable_files_is_removed() -> BoxResult<()> {
    let w = world().await?;
    let closed = w.task("t-tools", TaskStatus::Closed, None).await?;
    let exclude = git(&closed, &["rev-parse", "--git-path", "info/exclude"])?;
    let exclude = if Path::new(&exclude).is_absolute() {
        PathBuf::from(exclude)
    } else {
        closed.join(exclude)
    };
    std::fs::create_dir_all(exclude.parent().ok_or("parent")?)?;
    std::fs::write(&exclude, ".tools\nnode_modules\n")?;
    std::fs::create_dir_all(closed.join(".tools/bin"))?;
    std::fs::write(closed.join(".tools/bin/cargo-nextest"), vec![1u8; 64])?;
    std::fs::create_dir_all(closed.join("e2e/node_modules/x"))?;
    std::fs::write(closed.join("e2e/node_modules/x/index.js"), "x")?;
    w.later();
    let report = w.h.app.disk_upkeep(&w.sandbox, false).await?;
    assert_eq!(report.worktrees, 1, "{report:?}");
    assert!(!closed.exists());
    Ok(())
}

/// SPEC-019 DSK-002: an idle task that has not moved for hours loses its caches without any
/// quota pressure (it keeps its worktree).
#[tokio::test]
async fn a_long_idle_task_loses_its_caches_without_pressure() -> BoxResult<()> {
    let w = world().await?;
    let idle = w
        .task("t-idle", TaskStatus::Running, Some(Activity::Waiting))
        .await?;
    let report = w.h.app.disk_upkeep(&w.sandbox, false).await?;
    assert_eq!(report.caches, 0, "a recently idle task keeps its caches");
    w.h.clock.0.fetch_add(
        yulius_domain::disk::IDLE_PRUNE_AFTER_MS + 1_000,
        Ordering::SeqCst,
    );
    let report = w.h.app.disk_upkeep(&w.sandbox, false).await?;
    assert!(report.caches >= 1, "{report:?}");
    assert!(idle.exists() && !idle.join("target").exists());
    Ok(())
}

/// SPEC-019 DSK-004: a worktree no task owns (an agent's own `git worktree add`) loses its
/// caches once untouched for a day.
#[tokio::test]
async fn an_orphan_worktree_loses_its_caches_after_a_day() -> BoxResult<()> {
    let w = world().await?;
    let orphan = w.root.join("worktrees").join("wt-206");
    git(
        &w.repo,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "fix/206",
            orphan.to_str().ok_or("path")?,
        ],
    )?;
    std::fs::create_dir_all(orphan.join("target"))?;
    std::fs::write(orphan.join("target").join("blob"), vec![7u8; BIG])?;
    let report = w.h.app.disk_upkeep(&w.sandbox, false).await?;
    assert_eq!(report.caches, 0, "a fresh one is left alone");
    // The directory's age is real time: the clock moves to a day after its real mtime.
    let real_now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_millis();
    w.h.clock.0.store(
        i64::try_from(real_now)? + 25 * 60 * 60 * 1000,
        Ordering::SeqCst,
    );
    let report = w.h.app.disk_upkeep(&w.sandbox, false).await?;
    assert!(report.caches >= 1, "{report:?}");
    assert!(orphan.exists() && !orphan.join("target").exists());
    Ok(())
}

/// A process host that only answers the background-shell probe, with what the test sets
/// (negative: the probe gives no answer).
struct Shells(AtomicI64);

#[async_trait::async_trait]
impl SessionHost for Shells {
    async fn spawn(
        &self,
        _: &Path,
        _: &str,
        _: &str,
        _: &LaunchPlan,
        _: &[u8; 32],
        _: (u16, u16),
    ) -> AppResult<SpawnedRun> {
        Err(yulius_app::error::AppError::Conflict("no runs here".into()))
    }
    async fn attach(&self, _: &Path, _: &[u8; 32]) -> AppResult<AttachedRun> {
        Err(yulius_app::error::AppError::Conflict("no runs here".into()))
    }
    fn inspect(&self, _: &Path) -> RunInspection {
        RunInspection::Missing
    }
    fn terminate(&self, _: &Path) -> AppResult<()> {
        Ok(())
    }
    fn background_shells(&self, _: u32) -> Option<u32> {
        u32::try_from(self.0.load(Ordering::SeqCst)).ok()
    }
}

impl World {
    /// Gives `task` a live session whose agent process is `pid`.
    async fn live(&self, task: &str, pid: u32) -> BoxResult<()> {
        let now = self.h.app.now();
        self.h
            .store
            .insert_attempt(&AttemptRecord {
                id: AttemptId::parse(&format!("at-{task}"))?,
                task_id: TaskId::parse(task)?,
                seq: 1,
                harness: Agent::ClaudeCode,
                account_id: None,
                provider_session_id: None,
                launch_kind: LaunchKind::New,
                state: AttemptState::Live,
                run_dir: self
                    .root
                    .join("runs")
                    .join(task)
                    .to_string_lossy()
                    .into_owned(),
                token_hash: format!("hash-{task}"),
                host_token: self.h.app.deps.crypto.seal(&[1u8; 32])?,
                ptyhost_pid: Some(pid),
                child_pid: Some(pid),
                started_at: now,
                ended_at: None,
                exit_code: None,
                exit_reason: None,
            })
            .await?;
        Ok(())
    }
}

/// SPEC-059 DSK-201 (#245): under pressure, an idle task's caches go only when a fresh probe says
/// nothing of it runs in the background. A running build in a background shell, a pending
/// background subagent and a probe that gives no answer all keep them.
#[tokio::test]
async fn an_idle_task_with_background_work_keeps_its_caches() -> BoxResult<()> {
    let shells = Arc::new(Shells(AtomicI64::new(1)));
    let w = world_with(Some(shells.clone())).await?;
    let idle = w
        .task("t-gate", TaskStatus::Running, Some(Activity::Waiting))
        .await?;
    w.live("t-gate", 4242).await?;
    let quota = Arc::new(Quota {
        used: AtomicU64::new(99),
        limit: 100,
    });
    assert!(w.h.app.set_isolation(quota));

    // Its gate runs in a background shell: the caches stay, even over the quota.
    w.h.app.disk_upkeep(&w.sandbox, true).await?;
    assert!(
        idle.join("target").exists(),
        "a running background build keeps them"
    );

    // The probe gives no answer (a timeout under load): unknown counts as working.
    shells.0.store(-1, Ordering::SeqCst);
    w.h.app.disk_upkeep(&w.sandbox, true).await?;
    assert!(
        idle.join("target").exists(),
        "an unanswered probe keeps them"
    );

    // No shell, but the turn ended waiting for a background subagent (a review).
    shells.0.store(0, Ordering::SeqCst);
    let task = TaskId::parse("t-gate")?;
    w.h.app.background_agents_observed(&task, 1).await;
    w.h.app.disk_upkeep(&w.sandbox, true).await?;
    assert!(
        idle.join("target").exists(),
        "a running subagent keeps them"
    );

    // Nothing runs any more: now they go.
    w.h.app.background_agents_observed(&task, 0).await;
    let report = w.h.app.disk_upkeep(&w.sandbox, true).await?;
    assert!(!idle.join("target").exists(), "{report:?}");
    Ok(())
}

/// SPEC-059 DSK-202 (#258): a start refused over the quota says who holds the space: the largest
/// directories of the last measurement, a task's worktree by its number.
#[tokio::test]
async fn a_refusal_names_the_largest_consumers() -> BoxResult<()> {
    let w = world().await?;
    w.task("t-big", TaskStatus::Running, Some(Activity::Working))
        .await?;
    struct Full;
    #[async_trait::async_trait]
    impl SandboxIsolation for Full {
        async fn prepare(&self, _: &SandboxRef) -> AppResult<Option<SandboxJoin>> {
            Err(yulius_app::error::AppError::Conflict(
                "this sandbox uses 126.0 GB of its 100.0 GB disk quota".into(),
            ))
        }
        async fn disk(&self, _: &SandboxRef) -> Option<DiskUsage> {
            Some(DiskUsage {
                used: 126_000_000_000,
                limit: 100_000_000_000,
            })
        }
        async fn largest(&self, _: &SandboxRef) -> Vec<(String, u64)> {
            vec![
                ("worktrees/t-big".into(), 35_000_000_000),
                ("ci/jobs".into(), 14_000_000_000),
                ("worktrees/gone".into(), 9_000_000_000),
                ("home".into(), 4_000_000_000),
            ]
        }
        fn release(&self, _: &SandboxId) {}
    }
    assert!(w.h.app.set_isolation(Arc::new(Full)));
    let task =
        w.h.store
            .task(&TaskId::parse("t-big")?)
            .await?
            .ok_or("task")?;
    let refusal =
        w.h.app
            .preflight_start(&task, None)
            .await
            .err()
            .ok_or("the start was not refused")?
            .public_message();
    assert!(
        refusal.contains(&format!(
            "largest: task #{} 35.0 GB, CI job checkouts 14.0 GB, worktree gone 9.0 GB, home 4.0 GB",
            task.number
        )),
        "{refusal}"
    );
    Ok(())
}

/// A quota whose sandbox has a Docker engine with `prunable` bytes of unused images and build
/// cache.
struct EngineQuota {
    used: AtomicU64,
    limit: u64,
    prunable: u64,
    prunes: AtomicU64,
}

#[async_trait::async_trait]
impl SandboxIsolation for EngineQuota {
    async fn prepare(&self, _: &SandboxRef) -> AppResult<Option<SandboxJoin>> {
        Ok(None)
    }
    async fn disk(&self, _: &SandboxRef) -> Option<DiskUsage> {
        Some(DiskUsage {
            used: self.used.load(Ordering::SeqCst),
            limit: self.limit,
        })
    }
    async fn measure_disk(&self, r: &SandboxRef) -> Option<DiskUsage> {
        self.disk(r).await
    }
    async fn prune_engine(&self, _: &SandboxRef) -> Option<u64> {
        self.prunes.fetch_add(1, Ordering::SeqCst);
        Some(self.prunable)
    }
    fn release(&self, _: &SandboxId) {}
}

/// SPEC-187 DKR-006: the engine's unused images and build cache are pruned when the upkeep must
/// free space, reported like the other caches, and left alone otherwise.
#[tokio::test]
async fn engine_images_and_build_cache_are_pruned_only_under_pressure() -> BoxResult<()> {
    let w = world().await?;
    let quota = Arc::new(EngineQuota {
        used: AtomicU64::new(10 * BIG as u64),
        limit: 100 * BIG as u64,
        prunable: 7 * BIG as u64,
        prunes: AtomicU64::new(0),
    });
    assert!(w.h.app.set_isolation(quota.clone()));
    let calm = w.h.app.disk_upkeep(&w.sandbox, false).await?;
    assert_eq!(quota.prunes.load(Ordering::SeqCst), 0, "{calm:?}");
    assert_eq!(calm.freed, 0);

    quota.used.store(95 * BIG as u64, Ordering::SeqCst);
    let pressed = w.h.app.disk_upkeep(&w.sandbox, false).await?;
    assert_eq!(quota.prunes.load(Ordering::SeqCst), 1);
    assert_eq!(pressed.freed, 7 * BIG as u64, "{pressed:?}");

    // A refused start presses too.
    quota.used.store(101 * BIG as u64, Ordering::SeqCst);
    w.h.app.disk_upkeep(&w.sandbox, true).await?;
    assert_eq!(quota.prunes.load(Ordering::SeqCst), 2);
    Ok(())
}

impl World {
    /// A worktree of the sandbox's repository no task owns, at `worktrees/<name>`, holding a
    /// `target` of ~256 KiB.
    fn orphan(&self, name: &str) -> BoxResult<PathBuf> {
        let orphan = self.root.join("worktrees").join(name);
        git(
            &self.repo,
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                &format!("fix/{name}"),
                orphan.to_str().ok_or("path")?,
            ],
        )?;
        std::fs::create_dir_all(orphan.join("target"))?;
        std::fs::write(orphan.join("target/blob"), vec![7u8; BIG])?;
        Ok(orphan)
    }

    /// Moves the clock a day past the real time, so orphan worktrees count as untouched.
    fn a_day_later(&self) -> BoxResult<()> {
        let real_now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_millis();
        self.h.clock.0.store(
            i64::try_from(real_now)? + 25 * 60 * 60 * 1000,
            Ordering::SeqCst,
        );
        Ok(())
    }

    /// A second sandbox `s2`, whose agents make the links (#649); its directory exists.
    async fn attacker(&self) -> BoxResult<(SandboxRecord, PathBuf)> {
        let sandbox = seed_sandbox(&self.h, "s2", "evil", "u-alice").await?;
        let dir = self.h.app.config.data_dir.join("sandboxes/s2");
        std::fs::create_dir_all(&dir)?;
        Ok((sandbox, dir))
    }
}

/// #649 (SPEC-263 DUL-001): a sandbox that replaces its `worktrees` with a link to another
/// sandbox's never gets that sandbox's caches measured or removed as its own.
#[tokio::test]
async fn a_linked_worktrees_folder_never_reaches_another_sandbox() -> BoxResult<()> {
    let w = world().await?;
    let victim = w.orphan("wt-206")?;
    let (attacker, evil) = w.attacker().await?;
    w.a_day_later()?;
    std::os::unix::fs::symlink("../s1/worktrees", evil.join("worktrees"))?;
    let report = w.h.app.disk_upkeep(&attacker, false).await?;
    assert!(
        victim.join("target/blob").exists(),
        "a linked worktrees/ pruned another sandbox's cache ({report:?})"
    );
    assert_eq!(report.caches, 0, "{report:?}");
    // Reached without a link, by its own sandbox, the same cache goes as before.
    let report = w.h.app.disk_upkeep(&w.sandbox, false).await?;
    assert!(!victim.join("target").exists(), "{report:?}");
    Ok(())
}

/// #649 (SPEC-263 DUL-001): one entry of `worktrees` linked to another sandbox's worktree.
#[tokio::test]
async fn a_linked_worktree_entry_never_reaches_another_sandbox() -> BoxResult<()> {
    let w = world().await?;
    let victim = w.orphan("wt-206")?;
    let (attacker, evil) = w.attacker().await?;
    w.a_day_later()?;
    std::fs::create_dir_all(evil.join("worktrees"))?;
    std::os::unix::fs::symlink(&victim, evil.join("worktrees/wt-9"))?;
    let report = w.h.app.disk_upkeep(&attacker, false).await?;
    assert!(
        victim.join("target/blob").exists(),
        "a linked worktree entry pruned another sandbox's cache ({report:?})"
    );
    assert_eq!(report.caches, 0, "{report:?}");
    Ok(())
}

/// #649 (SPEC-263 DUL-001): a linked `ci/jobs` never gets another sandbox's CI checkout caches
/// pruned, also under pressure.
#[tokio::test]
async fn a_linked_ci_jobs_folder_never_reaches_another_sandbox() -> BoxResult<()> {
    let w = world().await?;
    let checkout = w.ci_checkout("quality")?;
    let (attacker, evil) = w.attacker().await?;
    w.pressed();
    std::fs::create_dir_all(evil.join("ci"))?;
    std::os::unix::fs::symlink("../../s1/ci/jobs", evil.join("ci/jobs"))?;
    let report = w.h.app.disk_upkeep(&attacker, true).await?;
    assert!(
        checkout.join("target/blob").exists(),
        "a linked ci/jobs pruned another sandbox's cache ({report:?})"
    );
    assert_eq!(report.caches, 0, "{report:?}");
    let report = w.h.app.disk_upkeep(&w.sandbox, true).await?;
    assert!(!checkout.join("target").exists(), "{report:?}");
    Ok(())
}

/// #649 (SPEC-263 DUL-001): a linked `ci`, the same.
#[tokio::test]
async fn a_linked_ci_folder_never_reaches_another_sandbox() -> BoxResult<()> {
    let w = world().await?;
    let checkout = w.ci_checkout("quality")?;
    let (attacker, evil) = w.attacker().await?;
    w.pressed();
    std::os::unix::fs::symlink("../s1/ci", evil.join("ci"))?;
    let report = w.h.app.disk_upkeep(&attacker, true).await?;
    assert!(
        checkout.join("target/blob").exists(),
        "a linked ci/ pruned another sandbox's cache ({report:?})"
    );
    assert_eq!(report.caches, 0, "{report:?}");
    Ok(())
}

/// #649 (SPEC-263 DUL-002): a `worktrees` swapped for a link after the scan, right before the
/// removal, makes the removal fail instead of following the link.
#[tokio::test]
async fn a_worktrees_folder_swapped_for_a_link_after_the_scan_is_not_followed() -> BoxResult<()> {
    let w = world().await?;
    let victim = w.orphan("wt-206")?;
    let (attacker, evil) = w.attacker().await?;
    // The attacker's own orphan of the same name, a repository of its own.
    let own = evil.join("worktrees/wt-206");
    std::fs::create_dir_all(&own)?;
    git(&own, &["init", "-q", "-b", "main"])?;
    std::fs::write(own.join(".gitignore"), "target\n")?;
    std::fs::write(own.join("Cargo.toml"), "[package]\n")?;
    git(&own, &["add", "-A"])?;
    git(&own, &["commit", "-q", "-m", "init"])?;
    std::fs::create_dir_all(own.join("target"))?;
    std::fs::write(own.join("target/blob"), vec![7u8; BIG])?;
    w.a_day_later()?;

    let (folder, aside) = (evil.join("worktrees"), evil.join("worktrees.old"));
    yulius_app::race::on(yulius_app::race::Point::CachePrune, &evil, move || {
        std::fs::rename(&folder, &aside).ok();
        std::os::unix::fs::symlink("../s1/worktrees", &folder).ok();
    });
    let report = w.h.app.disk_upkeep(&attacker, false).await?;
    assert!(
        victim.join("target/blob").exists(),
        "the removal followed a link swapped in after the scan ({report:?})"
    );
    assert_eq!(report.caches, 0, "{report:?}");
    assert!(evil.join("worktrees.old/wt-206/target/blob").exists());
    Ok(())
}

impl World {
    /// `s2`'s repository (a Rust project ignoring `target`), with a closed task `t-evil`, and a
    /// worktree of it at `s1/worktrees/t-evil` holding committed work. The test makes that
    /// registration itself: it stands for a sandbox's `.git` naming another sandbox's folder,
    /// which only a link lets the upkeep reach.
    async fn registered_victim(&self, evil: &Path) -> BoxResult<PathBuf> {
        let repo = evil.join("repo");
        std::fs::create_dir_all(&repo)?;
        git(&repo, &["init", "-q", "-b", "main"])?;
        std::fs::write(repo.join("Cargo.toml"), "[package]\n")?;
        std::fs::write(repo.join(".gitignore"), "target\n")?;
        git(&repo, &["add", "-A"])?;
        git(&repo, &["commit", "-q", "-m", "init"])?;
        let victim = self.root.join("worktrees/t-evil");
        git(
            &repo,
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                "feat/victim",
                victim.to_str().ok_or("path")?,
            ],
        )?;
        std::fs::write(victim.join("README.md"), "victim's\n")?;
        git(&victim, &["add", "-A"])?;
        git(&victim, &["commit", "-q", "-m", "victim's work"])?;
        seed_task(
            &self.h,
            "t-evil",
            "s2",
            "u-alice",
            TaskStatus::Closed,
            Some(&evil.join("worktrees/t-evil")),
        )
        .await?;
        Ok(victim)
    }

    /// A quota at 95 %, so the upkeep must free space.
    fn pressed(&self) {
        let quota = Arc::new(Quota {
            used: AtomicU64::new(95 * BIG as u64),
            limit: 100 * BIG as u64,
        });
        assert!(self.h.app.set_isolation(quota));
    }
}

/// #649 (SPEC-263 DUL-001): a closed task's worktree reached through a linked `worktrees` is
/// never handed to Git for removal.
#[tokio::test]
async fn an_ended_worktree_reached_through_a_link_is_never_removed() -> BoxResult<()> {
    let w = world().await?;
    let (attacker, evil) = w.attacker().await?;
    let victim = w.registered_victim(&evil).await?;
    std::os::unix::fs::symlink("../s1/worktrees", evil.join("worktrees"))?;
    w.later();
    let report = w.h.app.disk_upkeep(&attacker, false).await?;
    assert!(
        victim.join("README.md").exists(),
        "a worktree reached through a link was removed ({report:?})"
    );
    assert_eq!(report.worktrees, 0, "{report:?}");
    Ok(())
}

/// #649 (SPEC-263 DUL-003): a `worktrees` swapped for a link after the scan found a real ended
/// worktree is checked again before Git removes it.
#[tokio::test]
async fn an_ended_worktree_swapped_for_a_link_after_the_scan_is_not_removed() -> BoxResult<()> {
    let w = world().await?;
    let (attacker, evil) = w.attacker().await?;
    let victim = w.registered_victim(&evil).await?;
    // The task's own worktree is real at the scan and holds a cache, so the round pauses at
    // its removal (`CachePrune`) before it reaches the ended worktrees.
    let own = evil.join("worktrees/t-evil");
    git(
        &evil.join("repo"),
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "feat/t-evil",
            own.to_str().ok_or("path")?,
        ],
    )?;
    std::fs::create_dir_all(own.join("target"))?;
    std::fs::write(own.join("target/blob"), vec![7u8; BIG])?;
    w.later();
    let (folder, aside) = (evil.join("worktrees"), evil.join("worktrees.old"));
    yulius_app::race::on(yulius_app::race::Point::CachePrune, &evil, move || {
        std::fs::rename(&folder, &aside).ok();
        std::os::unix::fs::symlink("../s1/worktrees", &folder).ok();
    });
    let report = w.h.app.disk_upkeep(&attacker, false).await?;
    assert!(evil.join("worktrees.old/t-evil/target/blob").exists());
    assert!(
        victim.join("README.md").exists(),
        "a worktree swapped for a link after the scan was removed ({report:?})"
    );
    assert_eq!(report.worktrees, 0, "{report:?}");
    Ok(())
}

/// Writes a Cargo unit of `hash` into `profile`, last used `hours` before the app's clock.
fn cargo_unit(w: &World, profile: &Path, hash: &str, hours: i64) -> BoxResult<()> {
    let ms = w.h.app.now().0 - hours * 3_600_000;
    let at = std::time::UNIX_EPOCH + std::time::Duration::from_millis(u64::try_from(ms)?);
    let times = std::fs::FileTimes::new().set_accessed(at).set_modified(at);
    for (path, bytes) in [
        (profile.join(format!(".fingerprint/dep-{hash}/lib-dep")), 16),
        (profile.join(format!("deps/libdep-{hash}.rlib")), BIG),
    ] {
        std::fs::create_dir_all(path.parent().ok_or("parent")?)?;
        std::fs::write(&path, vec![7u8; bytes])?;
        std::fs::File::options()
            .write(true)
            .open(&path)?
            .set_times(times)?;
    }
    std::fs::File::open(profile.join(format!(".fingerprint/dep-{hash}")))?.set_times(times)?;
    std::fs::write(profile.join(".cargo-lock"), "")?;
    Ok(())
}

/// SPEC-276 RDA-001..RDA-004: the sweep reaches the build directories of worktrees and CI
/// checkouts and deletes units unused for two days, keeps recent ones and audits what it freed.
/// Artifact units, which tests read after Cargo released its lock, go only where nothing runs
/// tests: a resting task's worktree, a CI checkout under the CI claim. A working task keeps them
/// (its incremental state still goes), a busy CI keeps its checkout whole, and a held lock keeps a
/// build directory whole.
#[tokio::test]
async fn the_sweep_deletes_unused_build_units_of_worktrees_and_ci_checkouts() -> BoxResult<()> {
    use yulius_domain::sweep::Level;
    let w = world().await?;
    let resting = w
        .task("t-rest", TaskStatus::Stopped(StopReason::Crashed), None)
        .await?;
    let profile = resting.join("target/debug");
    cargo_unit(&w, &profile, "35f5dbd50d1a473d", 72)?;
    cargo_unit(&w, &profile, "9caa50b098b82860", 1)?;
    let working = w
        .task("t-work", TaskStatus::Running, Some(Activity::Working))
        .await?;
    let working_profile = working.join("target/debug");
    cargo_unit(&w, &working_profile, "1111111111111111", 72)?;
    let stale = working_profile.join("incremental/yulius-0u6e73f24ohkf");
    std::fs::create_dir_all(stale.join("s-a-b"))?;
    for dir in [stale.join("s-a-b"), stale.clone()] {
        let ms = w.h.app.now().0 - 72 * 3_600_000;
        let at = std::time::UNIX_EPOCH + std::time::Duration::from_millis(u64::try_from(ms)?);
        std::fs::File::open(&dir)?
            .set_times(std::fs::FileTimes::new().set_accessed(at).set_modified(at))?;
    }
    let checkout = w.root.join("ci/jobs/quality");
    std::fs::create_dir_all(&checkout)?;
    std::fs::write(checkout.join("Cargo.toml"), "[package]\n")?;
    let ci_profile = checkout.join("target/debug");
    cargo_unit(&w, &ci_profile, "0123456789abcdef", 72)?;
    let busy = w.root.join("worktrees/t-busy");
    std::fs::create_dir_all(&busy)?;
    std::fs::write(busy.join("Cargo.toml"), "[package]\n")?;
    cargo_unit(&w, &busy.join("target/debug"), "fedcba9876543210", 72)?;
    let lock = std::fs::File::open(busy.join("target/debug/.cargo-lock"))?;
    lock.lock()?;

    // A CI run holds the checkouts: the CI target stays whole.
    assert!(w.h.app.ci_claim(&w.sandbox.id));
    let report = w.h.app.sweep_build_caches(Level::Ok).await;
    assert!(
        ci_profile
            .join("deps/libdep-0123456789abcdef.rlib")
            .exists(),
        "a CI run reads it"
    );
    w.h.app.ci_release(&w.sandbox.id);
    assert_eq!(
        report.busy, 2,
        "the CI checkout and the locked worktree: {report:?}"
    );

    let report = w.h.app.sweep_build_caches(Level::Ok).await;
    assert_eq!((report.swept, report.busy), (3, 1), "{report:?}");
    assert!(!profile.join("deps/libdep-35f5dbd50d1a473d.rlib").exists());
    assert!(!profile.join(".fingerprint/dep-35f5dbd50d1a473d").exists());
    assert!(
        profile.join("deps/libdep-9caa50b098b82860.rlib").exists(),
        "used an hour ago"
    );
    assert!(
        !ci_profile
            .join("deps/libdep-0123456789abcdef.rlib")
            .exists()
    );
    assert!(
        working_profile
            .join("deps/libdep-1111111111111111.rlib")
            .exists(),
        "a working task's tests may read it"
    );
    assert!(!stale.exists(), "its unused incremental state goes");
    assert!(
        busy.join("target/debug/deps/libdep-fedcba9876543210.rlib")
            .exists(),
        "a build holds it"
    );
    assert!(
        w.h.app.ci_claim(&w.sandbox.id),
        "the sweep released its CI claim"
    );
    w.h.app.ci_release(&w.sandbox.id);
    let audited: Vec<String> =
        w.h.store
            .automation_audit("app", 100)
            .await?
            .into_iter()
            .filter(|a| a.action == "disk.sweep")
            .filter_map(|a| a.detail)
            .collect();
    assert_eq!(audited.len(), 3, "{audited:?}");
    assert!(
        audited.iter().any(|d| d.starts_with("target of task #")),
        "{audited:?}"
    );
    assert!(
        audited
            .iter()
            .any(|d| d.starts_with("target of CI job quality")),
        "{audited:?}"
    );
    Ok(())
}

/// SPEC-276 RDA-006: a waiting task keeps its caches while the data disk is fine or merely low;
/// once the disk is critical, a round prunes them (back towards 85 %), sweeps, and the rise is
/// audited once.
#[tokio::test]
async fn a_critical_data_disk_prunes_idle_caches_and_is_audited() -> BoxResult<()> {
    use yulius_domain::sweep::Level;
    let w = world().await?;
    let idle = w
        .task("t-wait", TaskStatus::Running, Some(Activity::Waiting))
        .await?;
    let app = &w.h.app;
    w.h.system.set_disk(1_000_000, 500_000);
    let round = app.disk_round(false).await;
    assert_eq!(round.level, Level::Ok);
    assert!(round.sweep.is_none(), "not due, not low");
    w.h.system.set_disk(1_000_000, 900_000);
    let round = app.disk_round(false).await;
    assert_eq!(round.level, Level::Low);
    assert!(round.sweep.is_some(), "a low disk sweeps every round");
    assert!(idle.join("target/blob").exists(), "low: idle caches stay");

    w.h.system.set_disk(1_000_000, 960_000);
    let round = app.disk_round(false).await;
    assert_eq!(round.level, Level::Critical);
    assert!(
        !idle.join("target/blob").exists(),
        "critical: idle caches go"
    );
    assert!(idle.exists(), "the worktree stays");
    assert!(round.upkeep.freed > 0, "{round:?}");
    app.disk_round(false).await;
    let audited: Vec<String> =
        w.h.store
            .automation_audit("instance", 100)
            .await?
            .into_iter()
            .filter(|a| a.action == "disk.pressure")
            .filter_map(|a| a.detail)
            .collect();
    assert_eq!(
        audited.len(),
        2,
        "low, then critical, once each: {audited:?}"
    );
    assert!(
        audited
            .iter()
            .any(|d| d.starts_with("the data disk is critical (96 % used")),
        "{audited:?}"
    );
    assert_eq!(app.disk_view().await.level, "critical");
    Ok(())
}

/// SPEC-276 RDA-007: anyone signed in reads the data disk's level; a change of level, and while
/// the disk is low of its used percent, reaches every page as `disk.updated`; the same reading
/// again sends nothing.
#[tokio::test]
async fn the_disk_level_is_served_and_its_changes_are_published() -> BoxResult<()> {
    use axum::http::{Method, StatusCode};
    let w = world().await?;
    let bob = seed_github_user(&w.h, "u-bob", "bob").await?;
    let r = support::call(&w.h.router, Method::GET, "/api/disk", None, &bob).await?;
    assert_eq!(r.status, StatusCode::OK, "{}", r.body);
    assert_eq!(r.body["level"], "ok", "unknown disk: {}", r.body);
    let anonymous = support::Creds::default();
    let r = support::call(&w.h.router, Method::GET, "/api/disk", None, &anonymous).await?;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED);

    let mut events = w.h.app.events.subscribe();
    w.h.system.set_disk(1_000, 500);
    w.h.app.check_disk().await;
    w.h.system.set_disk(1_000, 600);
    w.h.app.check_disk().await;
    w.h.system.set_disk(1_000, 860);
    w.h.app.check_disk().await;
    w.h.system.set_disk(1_000, 870);
    w.h.app.check_disk().await;
    w.h.app.check_disk().await;
    let r = support::call(&w.h.router, Method::GET, "/api/disk", None, &bob).await?;
    assert_eq!(
        (
            r.body["level"].as_str(),
            r.body["used"].as_u64(),
            r.body["total"].as_u64()
        ),
        (Some("low"), Some(870), Some(1_000))
    );
    let mut changes = Vec::new();
    while let Ok(envelope) = events.try_recv() {
        if let yulius_contracts::events::UiEvent::DiskChanged { level, .. } = envelope.event {
            assert!(envelope.sandbox.is_none() && !envelope.admin_only);
            changes.push(level);
        }
    }
    assert_eq!(
        changes,
        ["low", "low"],
        "at ok nothing; low at 86 %, again at 87 % (the pill shows the percent), once"
    );
    Ok(())
}

/// SPEC-276 RDA-008: an archived task's build output goes in the next round, without the
/// 10-minute grace an ended task gets (it may be continued at once; an archived one is put away).
#[tokio::test]
async fn an_archived_task_loses_its_caches_without_the_grace() -> BoxResult<()> {
    let w = world().await?;
    let stopped = w
        .task("t-arch", TaskStatus::Stopped(StopReason::Crashed), None)
        .await?;
    let kept = w
        .task("t-stop", TaskStatus::Stopped(StopReason::Crashed), None)
        .await?;
    w.archive("t-arch").await?;
    w.h.app.disk_upkeep(&w.sandbox, false).await?;
    assert!(!stopped.join("target").exists(), "archived: no grace");
    assert!(stopped.exists(), "its worktree stays");
    assert!(
        kept.join("target/blob").exists(),
        "not archived: the grace holds"
    );
    Ok(())
}

/// SPEC-276 RDA-008: archiving a task through the API starts a round at once: its build output
/// goes without anyone running the upkeep (review round 1: the test above calls it by hand).
#[tokio::test]
async fn archiving_a_task_through_the_api_removes_its_build_output() -> BoxResult<()> {
    use axum::http::{Method, StatusCode};
    let w = world().await?;
    let worktree = w
        .task("t-arch", TaskStatus::Stopped(StopReason::Crashed), None)
        .await?;
    let r = support::call(
        &w.h.router,
        Method::POST,
        "/api/tasks/t-arch/archive",
        Some(serde_json::json!({})),
        &w.alice,
    )
    .await?;
    assert_eq!(r.status, StatusCode::OK, "{}", r.body);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while worktree.join("target").exists() && std::time::Instant::now() < deadline {
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert!(
        !worktree.join("target").exists(),
        "the archive's round removed it"
    );
    assert!(worktree.exists(), "its worktree stays");
    Ok(())
}

/// SPEC-276 RDA-006: while the disk is low, the safe leftovers round starts with a disk round, at
/// most once every 10 minutes; at ok it does not.
#[tokio::test]
async fn a_low_disk_starts_the_leftovers_round_at_most_every_ten_minutes() -> BoxResult<()> {
    let w = world().await?;
    let app = &w.h.app;
    w.h.system.set_disk(1_000, 500);
    assert!(
        !app.disk_round(false).await.leftovers,
        "ok: the hourly round is enough"
    );
    w.h.system.set_disk(1_000, 900);
    assert!(app.disk_round(false).await.leftovers, "low: at once");
    assert!(
        !app.disk_round(false).await.leftovers,
        "not again within 10 minutes"
    );
    Ok(())
}
