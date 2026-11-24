//! Test pause points (`yulius_app::race`) belong to the runtime that registered them (#1644,
//! ADR-0474): libtest runs the tests of this binary as threads of one process, each
//! `#[tokio::test]` on its own runtime, and their keys (task ids such as `t-main`) repeat.

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use yulius_app::race::{Point, on, reached};

use crate::support::TestResult;

fn runtime() -> std::io::Result<tokio::runtime::Runtime> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
}

/// Registers a hook for `key` that records it ran.
fn hook(key: &str) -> Arc<AtomicBool> {
    let ran = Arc::new(AtomicBool::new(false));
    let seen = Arc::clone(&ran);
    on(Point::ParkedStatusRead, Path::new(key), move || {
        seen.store(true, Ordering::SeqCst);
    });
    ran
}

#[test]
fn a_hook_runs_only_for_a_point_reached_on_the_runtime_that_registered_it() -> TestResult {
    let (mine, other) = (runtime()?, runtime()?);
    let key = "pause-points-same-key";
    let ran = mine.block_on(async { hook(key) });
    other.block_on(async { reached(Point::ParkedStatusRead, Path::new(key)) });
    other.block_on(async {
        tokio::task::spawn_blocking(move || reached(Point::ParkedStatusRead, Path::new(key))).await
    })?;
    assert!(
        !ran.load(Ordering::SeqCst),
        "another runtime's point took the hook"
    );
    mine.block_on(async {
        tokio::task::spawn_blocking(move || reached(Point::ParkedStatusRead, Path::new(key))).await
    })?;
    assert!(
        ran.load(Ordering::SeqCst),
        "a blocking thread of the registering runtime reaches its hook"
    );
    Ok(())
}

#[test]
fn a_hook_or_point_outside_any_runtime_matches_by_point_and_key() -> TestResult {
    let other = runtime()?;
    let unscoped = hook("pause-points-unscoped");
    other.block_on(async { reached(Point::ParkedStatusRead, Path::new("pause-points-unscoped")) });
    assert!(
        unscoped.load(Ordering::SeqCst),
        "a hook registered outside a runtime runs anywhere"
    );
    let scoped = other.block_on(async { hook("pause-points-plain-thread") });
    std::thread::spawn(|| {
        reached(
            Point::ParkedStatusRead,
            Path::new("pause-points-plain-thread"),
        )
    })
    .join()
    .map_err(|_| "plain thread")?;
    assert!(
        scoped.load(Ordering::SeqCst),
        "a point on a plain thread runs a runtime's hook"
    );
    Ok(())
}

#[test]
fn a_hook_whose_runtime_shut_down_runs_on_the_next_one() -> TestResult {
    let key = "pause-points-restart";
    let first = runtime()?;
    let ran = first.block_on(async { hook(key) });
    first.shutdown_timeout(std::time::Duration::from_secs(5));
    // The test restarts its server on a new runtime, as `task_done.rs` restarts do.
    runtime()?.block_on(async { reached(Point::ParkedStatusRead, Path::new(key)) });
    assert!(
        ran.load(Ordering::SeqCst),
        "a test's hook outlives the server life it was registered in"
    );
    Ok(())
}
