---
yulius-task: fbddde55-dc71-4821-8284-2d9b13ce4b8a
status: in-progress
---

# SPEC-673: Cleanup of the test suite: shared fixtures, tables, template store (audit L8, phase 1)

## Goal

Carry out §2.3, §2.8, §2.5 and §2.6 of the November 2026 test-suite audit (L8). The result is less
test code doing the same thing, with the same coverage. One pull request per step, each delivered
before the next starts.

## Context

- The owner's cleanup charter and the audit report are task attachments outside the repository
  (`audit-2026-11-25/CHARTER.md`, `L8-tests.md`). The charter treats this work as a chore with no
  spec. This file exists only because Yulius runs no CI for a task without a registered spec. It
  records the plan and the evidence, nothing else.
- Phase 2 (§2.1 injectable timer, §2.2 decisions moved to app-level tests) is a later task.

## Requirements

- CLN-001: Every test case and every assertion stays. Only setup, helpers and layout change.
- CLN-002: Each step is its own pull request into `dev`, delivered before the next starts.
- CLN-003: The whole test binary of every touched crate passes before review.

## Plan

- [x] PR1a: support::git and support::eventually replace the per-file copies
- [x] PR1a: review findings answered (macOS import, description)
- [x] PR1b: support::BareRemote and support::ScriptedHost replace the FakeRemote and SessionHost copies
- [x] PR1b: review findings answered
- [x] PR2: support::live real-process builder replaces start_holding, build_tuned and start_hosted
- [x] PR2: hand-wired Deps and sibling-file imports moved into support
- [x] PR2: review findings answered
- [x] PR3: migrated-template store, test time measured before and after
- [x] PR3: review findings answered
- [x] PR4a: helpers for the agent_hooks and merge_train families (store_at, proxy.rs and local_ci
  not done: see Decisions)
- [ ] PR4a: review findings answered
- [ ] PR4b: comments that only cite spec IDs deleted
- [ ] PR4b: review findings answered

## Acceptance criteria

- Each step is merged into `dev` on green Yulius CI, or named under "Not done" in the closing
  summary with the reason.

## Test plan

- Per pull request: clippy with `-D warnings`, the whole `yulius::main` binary (and the store's
  tests for PR3 and PR4a), and `scripts/dev quick`.

## Decisions

- A spec is written despite the charter's "no spec": without one, Yulius starts no CI run and
  nothing can merge. It holds only this plan and its evidence.
- PR4a covers two of the five §2.5 families, with named helpers instead of tables, so each case
  keeps its test name. `store_at` (`crates/yulius-store`, task #656's) and the `proxy.rs` capacity
  table (`crates/yulius-egress`, task #653's) are outside this task's paths. A `local_ci.rs` table
  saved no lines (+50/−46), so that family stays as it is.
