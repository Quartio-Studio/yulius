//! Disk upkeep end to end (SPEC-005, SPEC-001 NFR-009): a real repository with real worktrees,
//! the real Git adapter and tasks in every state. Build caches of tasks that do not work are
//! removed, the worktrees of closed and cancelled tasks go, idle tasks lose theirs only under
//! pressure, working tasks never, and the agent is told what was freed.

mod support;

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use support::{HarnessOptions, harness_with, seed_github_user, seed_sandbox, seed_task};
use yulius::wiring::git::GitAdapter;
use yulius_app::error::AppResult;
use yulius_app::model::SandboxRecord;
use yulius_app::ports::isolation::{DiskUsage, SandboxIsolation, SandboxJoin, SandboxRef};
use yulius_app::ports::store::{CoordinationStore as _, TaskStore as _};
use yulius_domain::ids::{SandboxId, TaskId};
use yulius_domain::task::{Activity, StopReason, TaskStatus};

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
    sandbox: SandboxRecord,
    repo: PathBuf,
    root: PathBuf,
}

const BIG: usize = 256 * 1024;

async fn world() -> BoxResult<World> {
    let h = harness_with(HarnessOptions {
        git: Some(Arc::new(GitAdapter::new("git"))),
        ..HarnessOptions::default()
    })?;
    seed_github_user(&h, "u-alice", "alice").await?;
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
    // Kept worktrees' agents are told; the removed ones have nothing to be told.
    let told = w.told("t-crash").await?;
    assert_eq!(told.len(), 1);
    assert!(
        told[0].contains("`target`") && told[0].contains("`out`"),
        "{told:?}"
    );
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
    let told = w.told("t-big").await?;
    assert!(
        told.len() == 1 && told[0].contains("`target`") && told[0].contains("MB"),
        "{told:?}"
    );
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
    // `target` and `out` are ignored files still in place: the upkeep has not run.
    let closed = w.task("t-hyg", TaskStatus::Closed, None).await?;
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
