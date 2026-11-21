---
yulius-task: 5b06b876-20e3-4a88-b4c2-2c7c3667ff86
status: in-progress
---

# SPEC-595 Shared compilation for Yulius Rust builds

## Goal

Reuse compiled Rust on the instance across Yulius task worktrees and CI lanes. Each checkout keeps its own Cargo outputs and can finish a correct build if caching is unavailable.

## Context

On November 21 the operator measured about 195 seconds compiling and 151 seconds testing in one shard, 614 seconds compiling in quality, and 18–22 GB per CI target. Agent and CI homes differ. CI jobs run in individually budgeted groups. [SPEC-569](SPEC-569-shorten-yulius-mutation-verification-whi.md) measured reuse inside mutation campaigns. This task measures ordinary builds.

## Requirements

- **RCC-001** Enable local disk reuse by default for this repository's Rust builds in tasks and CI lanes while preserving concurrent independent builds.
- **RCC-002** Keep the store private to one sandbox, cap its size and document disk upkeep. The compiler stays within the caller's resource and cancellation boundary.
- **RCC-003** A changed source, compiler input or observable checkout path must produce the right output. A missing tool, unavailable store or damaged entry must permit ordinary compilation.
- **RCC-004** Compare sccache, a shared dependency target and Cargo's own pinned-toolchain facilities. Measure cold and warm commands in different fresh worktrees, cache hits across CI lanes and before/after real CI durations.

## Plan

- [x] Specify cache ownership and capture the uncached CI baseline
- [x] Enable a bounded local compiler cache across project builds
- [x] Verify concurrent reuse, path correctness and recovery
- [ ] Measure fresh-worktree test and quality builds and document the choice
- [ ] Approve the cache implementation for delivery
- [ ] Integrate idle compiler stores with instance disk upkeep
- [ ] Record the observed CI lane reuse and before/after run times
- [ ] Verify the disk upkeep and CI evidence delivery on dev

## Acceptance criteria

- **AC-1** With empty targets in two worktrees, the second build serves hits from the first. Two jobs building different revisions simultaneously produce their own expected values.
- **AC-2** The sandbox's existing mount boundary encloses all shared artifacts. The documented owner evicts entries under the cap. With broken storage or missing tooling the expected output still builds.
- **AC-3** The decision records elapsed times for both requested commands, cacheable requests and hits with denominators, paths, versions and CI conditions. A reviewed head passes the gate and lands on dev.

## Test plan

Exercise the Cargo entry point with real small crates, including an embedded checkout path, changed inputs, concurrent builds and cache loss. Run the benchmark with an empty target per invocation. Compare CI job execution and queue times separately. Verify repository contracts and delivery on the exact head.

## Decisions

Deliver the repository launcher and its 16 GiB store collection first. Deliver instance-side idle-store eviction and scheduled CI evidence together in a second reviewed change. Use mbx 1.30.0 with private Cargo targets and a sandbox-private disk budget. [ADR-0434](../../docs/decisions/0434-keep-a-local-compiler-cache-in-each-project-sandbox.md)

Adopt the policy in Yulius's repository. To make it the default emitted by Rust project bootstrap, the onboarding template would install a digest-pinned tool for supported hosts, generate the launcher, wire task and CI commands to it, and honor existing wrappers and a project-level opt-out. Disk upkeep would need a budget and ownership stamp for each enrolled sandbox. Validate the macro and source-path policy, parallel builds, cache loss and cold/warm timings on representative quartio-v2 and autoresearch checkouts before updating that template. Existing projects would adopt it through their own reviewed onboarding changes.

## Evidence

Before the change, Yulius CI run `cf1c465e-1711-4e70-9b2e-929e1dab55c4` (PR #1608, `19d368f`) passed in 604 s. Quality ran for 552 s and the two test shards for 547 s and 392 s. Their admission waits were 16 s, 23 s and 1 s. E2E ran for 572 s. The project had Vidoc disabled. The physical merge-train run that verified the immediate base `6810a3c` was `7f9305c6-32a9-46d7-9a80-ab26b132afb6` at `a155ebe`. It passed in 738 s. Quality and the two shards ran for 682 s, 479 s and 411 s after admission waits of 55 s, 212 s and 26 s. The dev run reused that verified tree.

The release-integrity and launcher suite (`python3 -m unittest discover -s scripts -p test_compile_cache.py`) passed 15 tests with pinned mbx at `52b59ecb3` and 17 after the foreground-terminal and jobserver regressions were added. The real compiler fixture built a path-independent library and a path-sensitive executable in task and CI layouts with distinct homes. The second checkout served a library hit and printed its own manifest directory. A same-length damaged cached executable rebuilt correctly. Simultaneous builds of different source revisions printed their own values. The launcher preserved an explicit Cargo failure, recovered from cache startup failure, and used plain Cargo during eviction. A fresh task found the pinned binary installed by another task.

The pressure integration test first failed at `assert!(!cache.exists())` with the original upkeep. Three existing xtask command-order, failure and concurrency tests passed with the original explicit Cargo override. A trial that wrapped the override produced extra discovery calls. The launcher now routes only the pinned Cargo from scripts/dev through the cache.

The corrected CI lane-layout replay at `52b59ecb3` used two real Git worktrees under an isolated `ci/jobs/test.0` and `ci/jobs/test.0@1`, the same CI home, separate empty targets and a local ext4 store with no other writers. Both ran `scripts/cargo check --locked --offline -p yulius-domain --lib` with two Cargo jobs. The first lane took 9.49 s and populated 12 initially unknown compilations. The second took 4.58 s and served 12 of 12 lookups (100%). Each lane additionally reported seven mbx bypasses and three compiler invocations that Yulius ran directly. Ten bypasses bring the warm share across all reported invocations to 12/22 (54.5%). No other writer could supply these hits.

In the prepared instance-upkeep slice, both Rust upkeep scenarios passed with nextest, including the original failing assertion. The tests cover an active reader, a pressured idle store, an unowned directory, a linked store and the healthy-disk size boundary. The release-integrity suite and the cross-checkout fixture passed with mbx installed. `scripts/dev quick` and `spec_check` passed. `scripts/dev security` reported Vidoc disabled.


Independent review of `050869aaa` reproduced three defects. A warm procedural-macro expansion embedded the previous worktree's manifest directory, PID-directed cancellation left a child running with the sandbox lease, and a truncated ownership stamp was accepted by builds. New regressions reproduced all three. The corrected launcher keeps uncontrolled procedural-macro expansion out of the cache, closes the lease at Cargo dispatch, forwards cancellation to the process group and publishes a complete stamp atomically. The macro fixture includes a re-export through a separate library. A first-start interruption leaves the cache available on the next build.

A second independent review of `7a4d5af07` found that a restored registry crate's `file!()` could name the previous Cargo home and a debug binary's DWARF compilation directory could name the previous worktree. A requested `CARGO_INCREMENTAL=1` also lost its private incremental state. Regression fixtures reproduced all three against plain Cargo. The source-path identity is now part of the key whenever an absolute Rust source or debug working directory can be recorded. An explicit incremental request runs with its original Cargo settings. A fresh target using the same registry source root served a cached result, while different Cargo homes printed their own file paths. Both debug binaries named their own checkout in DWARF. Bootstrap continued after a simulated interrupted HTTP download.


The third review of `229ff6323` reproduced two caller-contract failures. A foreground test stopped while reading its terminal. GNU Make's inherited pipe jobserver descriptors did not reach Cargo. Separate fixtures compare a foreground read and a live jobserver token with the caller's expected result. This delivery retains plain Cargo in those contexts. Redirected standard input also preserves an accessible foreground `/dev/tty`. Unattended Yulius jobs identify their run and redirect all three standard streams. They use the cache in the PTY runner’s original session and job cgroup. A fixture runs both configured CI commands on a PTY and verifies that Cargo and the cache retain the original session ID.
