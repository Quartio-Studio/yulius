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
- [ ] PR3: review approved and merged into dev
- [ ] PR4: config.rs shared list/names/one-line helpers, EnvironmentsConflict and render narrowing
- [ ] PR4: [disk] validated by parse_yulius_toml (test first)
- [ ] PR4: vidoc.rs on toml::from_str; one frontmatter, one clip, CivilDate and doctests folded
- [ ] PR4: review approved and merged into dev
- [ ] PR5: move-only split of ci_job
- [ ] PR5: move-only split of cd_prepare into cd_resolve_target and cd_promote_for
- [ ] PR5: F6 production pinning without staging simplified in cd_promote_for
- [ ] PR5: review approved and merged into dev
- [ ] Closing summary with what changed, verification and Not done

## Acceptance criteria

- Each pull request merged into `dev` on green Yulius CI after an independent review, or listed
  as not done with the reason.

## Test plan

- Each pull request runs the test binaries it touches in full; Yulius CI runs the gate.

## Decisions

- Bootstrap runs in `scripts/dev` under `YULIUS_CI`, not in xtask: xtask is compiled by the
  toolchain bootstrap installs.
