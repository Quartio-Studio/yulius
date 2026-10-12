---
yulius-task: none
status: done
---

# SPEC-021: parallel CI runs per sandbox, and flaky tests do not fail a run

Status: done
Date: 2026-10-12
Owner: finished by hand after the analysis of the `yulius` sandbox's task transcripts
Scope authorization: user request (F 2026-10-12): "(b) Things failing for no reason, things
should be robust"; "agents doesnt seem to work very in parallel".

## Goal

Parallel tasks of one sandbox do not wait for each other's CI, and a test that fails once under
load and then passes does not cost a whole CI cycle.

## Context

After SPEC-020 a sandbox still ran one CI run at a time: each run takes 12–20 minutes, the
rulesets re-test every pull request after each merge into `dev`, so tasks queued behind each
other for hours. End-to-end and some integration tests failed now and then under the load of
parallel builds (terminal locators, toasts, deep links), and each such failure sent the agent
into another full run.

## Requirements

| ID | Requirement | Evidence |
| --- | --- | --- |
| CI-106 | Up to `--ci-lanes` (`YULIUS_CI_LANES`, default 2) runs of one sandbox execute at once; each lane past the first has its own job checkouts (`<job>@<lane>`); the instance-wide job slots still bound the machine's load. Migration 13 drops the index that allowed one running run per sandbox. | `ci/mod.rs::lane_tests`, `exec.rs` checkout test, `yulius-store` `ci.rs` run test |
| CI-107 | Disk upkeep's claim of a sandbox's CI checkouts holds while any lane runs, and no run starts while it is claimed. | `ci/mod.rs::lane_tests` |
| CI-109 | Yulius adds, moves, removes and prunes worktrees one at a time: a prune during another add removed its half-written registration and the add failed ("failed to read .git/worktrees/<name>/commondir"), which two lanes creating checkouts at once hit in 2 of 5 runs. | `git_ops.rs::ci_checkouts_created_at_once_all_succeed` |
| CI-108 | This repository's CI retries a failing test twice: nextest's `ci` profile (`retries = 2`) and the `e2e` job (`--retries 2`); a test that passes on a retry is reported as flaky and does not fail the run. Flaky tests are still fixed or quarantined with an issue. | `.config/nextest.toml`, `yulius.toml` |

## Plan

- [x] Lanes in the dispatcher, per-lane checkouts, `--ci-lanes`
- [x] Worktree registration changes serialized in the Git adapter
- [x] Retries in the nextest CI profile and the `e2e` CI job

## Evidence

- The new checkout test failed in 2 of 5 runs without the lock and passed 6 of 6 with it (macOS).
- `cargo nextest run -p yulius-store -p yulius-app -p yulius-git -p yulius --test local_ci --test disk`
  and clippy on macOS: see the pull request.
