# 0436. Yulius's full suite runs after the merge, and promotion comes from verified staging

## Status

Accepted. Specified in [SPEC-591](../../.ai/specs/SPEC-591-yulius-gets-its-own-staging-instance-and.md)
(STG-007, STG-008); task #575. Applies
[0410](0410-the-full-suite-runs-on-the-merged-revision-beside-a-merge-first-staging-target.md),
[0411](0411-staging-changes-through-a-queue-of-tested-merged-revisions.md) and
[0425](0425-pin-a-production-deploy-to-verified-dev.md) to Yulius's own repository. Its staging
target is [0430](0430-yuliuss-staging-builds-in-cds-builder-and-installs-a-kept-release-with-its-store.md).

## Context

[SPEC-579](../../.ai/specs/SPEC-579-adopt-merge-first-staging-infra-bootstra.md) kept every job on
Yulius's pull requests, because `[cd.dev]` had no machine. ADR-0430 gives it one, a staging VM
that a deploy verifies. Yulius's `dev` is what a person promotes to `main` and what the
production instances (Quartio, PerfectData) install, so a regression that passes staging reaches
people.

Last week's CI (the owner's figures for the week to 2026-11-21, given in task #575's request; this
record did not measure them again, and runs on a loaded host take longer):

| Run or job | Median |
| --- | --- |
| Pull request run | 9.2 min |
| Merge train | 10.0 min |
| `quality` | 5.5 min |
| `test` shard | 3.8 min each |
| `e2e` | 4.0 min |

Per pull request head the jobs take 17.1 job-minutes, 11.6 of them the tests and e2e. A run takes
longer than its longest job (5.5 min) because jobs wait for CI's CPU budget. Every pull request
head pays the full suite again in its merge train.

## Decision

- **`[cd.dev]` is the merge-first target** (`merge_first = true`).
- **The full suite is both `test` shards and `e2e`** (`after_merge = true`). They run on `dev`'s
  runs, where a run that `dev` moved past is superseded, and on the staging queue's candidate run
  of a requested revision that has no verdict. They do not run on
  pull request heads or in merge trains.
- **`quality` and `security` stay on pull requests.** What a pull request still proves before it
  merges:
  - `quality`: format, the repository contracts, `cargo check` and clippy of every target with
    every feature (all tests compile), the macOS build, dependency audit and unused
    dependencies, `cargo doc`, and the scripts' own tests (`scripts/test_*.py`, including
    `scripts/test_cd.py`);
  - `security` (when Vidoc is on);
  - every job the pull request adds or changes (ADR-0410: its "(this change)" twin);
  - the independent review of the exact head;
  - the tests the change touches, run by its agent while iterating (AGENTS.md).
- **Promotion only from verified staging.**
  - Only a revision that passed the full suite on `dev` and that `[cd.dev]` verified may be
    promoted to `main`.
  - The product enforces it with Yulius's own `yulius.toml`:
    - a person's production deploy (`cd_prepare`, ADR-0411; its revision selection, ADR-0425);
    - the Delivery page's Promote;
    - the automatic promotion.
  - Staging changes only to a merged revision whose full gate passed (the queue's candidate gate).
  - `crates/yulius/tests/staging.rs` (`yuliuss_own_config_promotes_only_what_staging_verified`)
    tests the refusal with this repository's own file.
- **A red `dev` is fixed forward.**
  - A merged revision whose full suite fails never reaches staging. Its staging request reads
    `gate_failed` with the run and the failed jobs, and its task is told.
  - That task fixes it on a new pull request and asks again. Promotion is refused until staging
    verified a fixed revision.
  - Nothing reverts by itself, and other pull requests keep merging on their quick checks.
  - A flaky test of the full suite now fails on `dev` instead of a pull request. It ends a staging
    request `gate_failed` and holds promotion just as a real failure does, so it is reproduced and
    fixed, or quarantined with an issue (AGENTS.md), never only retried.

## Alternatives

- **Move only `e2e`.**
  - It saves 4.0 of the 11.6 job-minutes per head. The two shards (7.6) would still compete with
    `quality` for the budget on every head and every train.
  - The load-dependent test flakes that block unrelated pull requests today (#1611, #1223, #1217,
    #1246) would keep doing so.
- **Move nothing.** That would waste the staging target. The shared CI host would keep paying the
  full suite on every head, then again in its train.
- **Move `quality` too.** ADR-0410 needs at least one quick job. `quality` is the broadest cheap
  check, and its format and contract steps fail within seconds.
- **Keep one test shard on pull requests.** Half the tests would give false confidence about the
  other half, and the jobs would still contend for the budget.

## Consequences

- A pull request's run is about `quality`'s 5.5 min instead of a 9.2 min median, and its merge
  train about the same instead of 10.0 min.
- The full suite runs per `dev` run and per requested candidate: 11.6 job-minutes, about 4 min of
  wall time, with the shards side by side.
- A Rust or browser regression can land on `dev`. It is caught on that revision before staging
  changes and before any promotion, and its task fixes it forward. Agents run the tests their
  change touches before the review, and the reviewer runs those in doubt.
- The pull request that sets these flags is still judged by `dev`'s gate, so its own run keeps the
  full suite (ADR-0410). The move takes effect from the next pull request.
- Promotion waits for staging. When `dev` moves on before its staging request deploys, the
  revision a person can promote is the one staging verified last.
