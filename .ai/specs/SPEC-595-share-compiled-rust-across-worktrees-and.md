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
- [ ] Verify concurrent reuse, path correctness and recovery
- [ ] Measure fresh-worktree test and quality builds and document the choice
- [ ] Approve the cache implementation for delivery
- [ ] Record the observed CI lane reuse and before/after run times

## Acceptance criteria

- **AC-1** With empty targets in two worktrees, the second build serves hits from the first. Two jobs building different revisions simultaneously produce their own expected values.
- **AC-2** The sandbox's existing mount boundary encloses all shared artifacts. The documented owner evicts entries under the cap. With broken storage or missing tooling the expected output still builds.
- **AC-3** The decision records elapsed times for both requested commands, cacheable requests and hits with denominators, paths, versions and CI conditions. A reviewed head passes the gate and lands on dev.

## Test plan

Exercise the Cargo entry point with real small crates, including an embedded checkout path, changed inputs, concurrent builds and cache loss. Run the benchmark with an empty target per invocation. Compare CI job execution and queue times separately. Verify repository contracts and delivery on the exact head.

## Decisions

Use mbx 1.30.0 with private Cargo targets and a 16 GiB sandbox-private disk budget. [ADR-0434](../../docs/decisions/0434-keep-a-local-compiler-cache-in-each-project-sandbox.md)

Adopt the policy in Yulius's repository. Bootstrap adoption for quartio-v2 or autoresearch would require explicit project opt-in, a compatible pinned tool and sandbox disk budget, a review of path-sensitive crates and equivalent concurrent, failure and timing evidence in each project.

## Evidence

Before the change, Yulius CI run `cf1c465e-1711-4e70-9b2e-929e1dab55c4` (PR #1608, `19d368f`) passed in 604 s. Quality ran for 552 s and the two test shards for 547 s and 392 s. Their admission waits were 16 s, 23 s and 1 s. E2E ran for 572 s. The project had Vidoc disabled.

The release-integrity and launcher suite (`python3 -m unittest discover -s scripts -p test_compile_cache.py`) passed eight tests. The real compiler fixture built a path-independent library and a path-sensitive executable in task and CI layouts with distinct homes. The second checkout served a library hit and printed its own manifest directory. A same-length damaged cached executable rebuilt correctly. Simultaneous builds of different source revisions printed their own values. The launcher preserved an explicit Cargo failure, recovered from cache startup failure, and used plain Cargo during eviction. A fresh task found the pinned binary installed by another task.

The pressure integration test first failed at `assert!(!cache.exists())` with the original upkeep. Three existing xtask command-order, failure and concurrency tests passed with the original explicit Cargo override. A trial that wrapped the override produced extra discovery calls. The launcher now routes only the pinned Cargo from scripts/dev through the cache.
