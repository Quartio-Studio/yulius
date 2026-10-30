---
yulius-task: 90be55e0-d683-43ac-9a83-4d494b5056b2
status: in-progress
---

# SPEC-351: Concurrent CI jobs never share a cache

Status: in progress
Date: 2026-10-30
Owner: Yulius task #333
Scope authorization: task #333, "fix so it won't happen again" on a pomerado-rust task's Needs you
(pomerado-rust issue #460: quality runs of different pull requests share one target dir on the CI
host; a test binary relinked by one run vanishes under another, "never executed: No such file").

## Goal

Two CI jobs that run at the same time in one sandbox never share `$YULIUS_CI_CACHE`, so build
outputs a project keeps there (a Cargo target directory, as Yulius's own guidance suggests) cannot
be rewritten under a running job. The cache still persists between runs.

## Context

- `ci_job_process` (`crates/yulius-app/src/ci/exec.rs`) gave every job of a sandbox
  `YULIUS_CI_CACHE=<sandbox>/ci/cache`, shared by its lanes (`YULIUS_CI_LANES`, default 2) and by
  the parallel jobs of one run. Checkouts are per job, matrix position and lane
  (`checkout_name`), so `target/` inside a checkout is never shared; the cache was.
- Yulius tells projects to put caches there: the `ci_setup` instructions ("caches go under $HOME or
  $YULIUS_CI_CACHE, which persist between runs"), docs/api.md, docs/operations.md and the
  onboarding prompt (which only said installs there "must tolerate a concurrent run").
- pomerado-rust's `scripts/ensure-rust.sh` does what that suggests:
  `CARGO_TARGET_DIR=$YULIUS_CI_CACHE/cargo-target`. Two lanes testing different pull requests then
  build the same test binary path (same package, profile and features hash the same) with
  different code. Cargo's build lock covers compiling, not executing, so one run executes or lists
  a binary the other just replaced or removed: "never executed: No such file", or worse, runs the
  other pull request's code. PR #445 failed 3 of 6 quality runs this way.
- Why tests missed it: `local_ci.rs` checks `HOME` and the checkouts per job and lane, never
  `YULIUS_CI_CACHE`; the shared cache was the documented design (exec.rs module doc).
- What else the cause could break: any build cache a project keeps in `$YULIUS_CI_CACHE`
  (Gradle/Go are lock-safe, Cargo targets, `node_modules`, Python venvs and ad hoc tool builds are
  not), including two matrix siblings of one run.

## Requirements

| ID | Requirement |
| --- | --- |
| CCH-001 | A CI job's `YULIUS_CI_CACHE` is `<sandbox>/ci/cache/<checkout>`, named like its checkout (`<job>[.<n>][@<lane>]`): jobs that run at the same time (two lanes, matrix siblings, two jobs of a run) never get the same one. |
| CCH-002 | The cache persists between runs that use the same checkout, as before. |
| CCH-003 | An entry of `ci/cache` is a stale leftover by the stale-checkout rule (no run used a job of that name for 14 days, or a lane past the configured ones), so the old shared cache's contents and unused caches go by the existing automatic rounds, under the CI claim. A file or link where a job's cache goes is removed, never followed, before the directory is made. |
| CCH-005 | The disk upkeep prunes a job checkout's cache whole like the caches in its checkout (SPEC-008 NFR-009 e): at once past a quarter of the quota, otherwise under pressure, never while a CI run executes. Guidance puts a Cargo target directory in the checkout, whose unused build units the hourly sweep removes. |
| CCH-004 | The docs, the onboarding prompt, the `ci_setup` instructions and the infra-bootstrap skill say the cache is the job's own, per checkout, and never shared with a concurrent job; `HOME` stays shared. |

## Plan

- [ ] Spec SPEC-351 written and validated
- [ ] Regression test shows two concurrent runs sharing one cache
- [ ] Each CI job gets the cache of its own checkout
- [ ] Unused caches are swept as stale CI leftovers
- [ ] Docs, prompts, decision and changes say the cache is per checkout
- [ ] Gates green and independent review answered
- [ ] Pull request merged into dev on green CI

## Acceptance criteria

- `local_ci.rs` regression test fails on the unfixed code (the second run finds the first run's
  marker in its cache) and passes with the fix; a later run of the same checkout finds its earlier
  marker (CCH-002).
- `leftovers.rs` test: a `ci/cache` entry of an unused job (and a loose legacy entry) is listed and
  removed by the round; the cache of a used job stays.
- `scripts/dev ci` green.

## Test plan

- `local_ci.rs::concurrent_runs_never_share_a_ci_cache`: a job (one CPU, so both lanes fit the
  budget at once) writes `live-$YULIUS_CI_RUN` into its cache, waits until both runs (dev and pull
  request #1, two lanes) are inside the job, fails if its cache holds another `live-*`, then keeps a
  `kept-*` marker; a later pull request run lists a `kept-*` marker from its checkout's earlier run
  and its cache path ends in its checkout. A file planted at `ci/cache/build` beforehand (what the
  shared cache may hold) does not fail the job (CCH-003, review round 1).
- `leftovers.rs::unused_ci_caches_go_like_their_checkouts`: an unused job's cache, a lane past the
  configured ones, a loose file and an old shared-cache directory are listed as "CI cache" and go
  by the round; used jobs' caches stay.
- `disk.rs::a_ci_job_s_own_cache_is_pruned_like_its_checkout_s` (CCH-005): under the cap it stays,
  past a quarter of the quota it goes at once, under pressure the other lane's goes, not while the
  CI claim is held.
- Matrix siblings and a run's other jobs: distinct cache names follow from `checkout_name`, covered
  by the existing `exec.rs::matrix_siblings_get_their_own_checkouts` (job keys are `[a-z0-9_-]`, so
  `.n` and `@lane` cannot collide).
- Wording: `onboarding.rs::prompts_describe_the_persistent_ci_home_and_the_fresh_home_proof` (prompt
  and infra-bootstrap), `ci::tools::tests::ci_setup_keeps_build_outputs_out_of_the_shared_home`.

## Decisions

- [ADR-0238](../../docs/decisions/0238-a-ci-job-s-cache-is-its-checkout-s-own.md): one cache per
  checkout. Alternatives: a lock held across a whole job (serialises lanes, which exist to run in
  parallel), fixing pomerado-rust only (the next project following Yulius's guidance hits it
  again), a per-run cache (persists nothing).
- `HOME` stays shared: it holds installers' state (rustup, `~/.cargo` registry, npm) whose tools
  lock their own writes, and a per-lane home would download every toolchain once per lane.
- The cache lives under the same `ci/cache` directory, named like the checkout, so the stale
  checkout rule decides both and the old shared contents go as stale entries without a migration.
