---
yulius-task: 5827cfb1-ecdb-452d-aa9a-c60afa00bdba
status: in-progress
---

# SPEC-668: Cleanup: built-in CI/CD and the gate (audit L3)

## Goal

Carry out five findings of the November 2026 audit of Yulius's own CI and CD (L3), one pull
request each, delivered in order: F7 + F11, F1, F2, F8(a) + F12, F5 + F6. Less code doing the
same thing; behaviour unchanged except for the capability F2 drops (prove-on-dev).

## Context

- The owner's cleanup charter makes this a chore and asks for no spec. Yulius needs this file to
  run CI and deliver code changes, so it is the task's plan record only.
- The audit report and charter are the task's attachments (`audit-2026-11-25/L3-ci-cd.md`,
  `CHARTER.md`). F3, F4, F9 and F10 are the owner's to decide and stay untouched.

## Requirements

- CLN-001: each pull request keeps every behaviour and test case except what its finding removes.
- CLN-002: a split moves code verbatim in its own commit; changes come in later commits.
- CLN-003: documents that describe changed code change in the same pull request.

## Plan

- [x] PR1: CI job commands are scripts/dev <job>; scripts/dev bootstraps in CI and xtask prints the stage
- [x] PR1: quality runs doctor, quick and checker tests without cargo check; fallbacks deleted; docs updated
- [x] PR1: review approved and merged into dev
- [x] PR2: dev run measured before; one test job replaces the two archive shards
- [x] PR2: ci_test_shard.py, test-shard, archive and partition code and their tests deleted; docs updated
- [x] PR2: review approved and merged into dev
- [x] PR2: dev run measured after, both numbers in the PR
- [x] PR3: drill users checked in quartio-v2 and other instance projects
- [x] PR3: prove.rs and the proving hooks and tests deleted
- [x] PR3: review approved and merged into dev
- [x] PR4: config.rs shared list/names/one-line helpers, EnvironmentsConflict and render narrowing
- [x] PR4: [disk] validated by parse_yulius_toml (test first)
- [x] PR4: vidoc.rs on the TOML parser; one frontmatter, one clip, doctests folded
- [x] PR4: review approved and merged into dev
- [x] PR5: move-only split of ci_job
- [x] PR5: move-only split of cd_prepare into cd_resolve_target and cd_promote_for
- [x] PR5: F6 checked against product's CD and reported to the owner, not changed
- [x] PR5: review approved
- [ ] PR6: prove-on-dev restored by reverting #1799, since product can reach it
- [ ] PR6: review approved
- [ ] Closing summary with what changed, verification and Not done

## Acceptance criteria

- Each pull request merged into `dev` on green Yulius CI after an independent review, or listed
  as not done with the reason.

## Test plan

- Each pull request runs the test binaries it touches in full; Yulius CI runs the gate.

## Decisions

- Bootstrap runs in `scripts/dev` under `YULIUS_CI`, not in xtask: xtask is compiled by the
  toolchain bootstrap installs.
- F2 and F6 are left to the owner: Quartio-Studio/product's integration branch `quartio` (52200f00,
  2026-11-26) has `[cd.dev]` and `[cd.prod]` with deploy and rollback, `promote = true`, and no
  `merge_first`. Its production deploys reach prove-on-dev (F2), and the pinning picks which
  commit its production promotes (F6). PR3 read product's stale `main` (7956caf7) and deleted
  prove-on-dev; PR6 will revert it. Both are the owner's decision.

## Evidence

- PR1 #1773 (F7, F11) merged at 05bb2b303: CI job commands are `scripts/dev <gate>`; `quality`
  no longer compiles. Checker tests 282, xtask 33, yulius-domain 665 passed.
- PR2 #1785 (F1) merged at f7c17b14f: one `test` job. Dev run before `6e8a3467` 6m39s (shards
  6m06s and 4m41s); after `ca4bd9b0` 7m12s (`test` 6m04s; e2e 7m00s sets the run).
- PR3 #1799 (F2) merged at 5ed478be5: prove-on-dev deleted after reading the CD tables of
  yulius, quartio-v2, autoresearch, studio, Branding and VistulaValley at their integration
  branches, but product at `main` instead of its integration branch `quartio`, which can reach
  it; PR6 will revert it.
- PR4 #1806 (F8a, F12) merged at 1986071cc: shared config readers, `[disk]` checked, vidoc.toml
  read as TOML; 5061 tests passed; reviewer differentials of 13k config and 19k frontmatter files.
- PR5 (F5): move-only splits of `ci_job` and `cd_prepare`; 4946 tests of yulius, yulius-app and
  yulius-domain passed.
