---
yulius-task: d7b795c0-7e49-483c-83e3-275950cd2471
status: in-progress
---

# SPEC-670: Cleanup: sandbox crate leftovers and manager.rs split (audit L5)

Task metadata only. The cleanup charter of 2026-11-25 asks for no spec for this chore. Yulius
starts no CI run for a branch that changes code without a valid task spec, so this file exists
for that reason alone. The design and the evidence are in each pull request's description.

## Goal

Carry out audit L5 findings F1, F3, F9 and F2 in `crates/yulius-sandbox`, with no change in
behaviour: less code, and a `manager.rs` a person can read.

## Context

The audit report and the charter are the owner's attachments (`audit-2026-11-25/L5-runtime-adapters.md`,
`CHARTER.md`). F1 is blocked: `crates/yulius/src/sandbox_cli.rs` and `crates/yulius/src/docker_smoke.rs`,
outside this task's paths, still reach `docker_bind`. F8 and F15 are excluded (the owner decides).

## Requirements

- **CLN-001** Behaviour stays the same. Nothing the sandbox isolates today becomes reachable.
- **CLN-002** A file split moves code verbatim in its own commit.

## Plan

- [x] F1: check what outside the crate reaches docker_bind (blocked: sandbox_cli.rs, docker_smoke.rs)
- [x] PR A: drop the SPEC-187 relay branches (record.engine) from the manager
- [x] PR A: drop the unguarded-engine replacement (the outdated-front path covers it)
- [x] PR A: drop the per-tick restore_leaf
- [x] PR A: update comments and docs that describe the removed code
- [x] PR A: run the sandbox crate tests and quick checks
- [ ] PR A: independent review approved, merge armed
- [ ] PR B: move the Docker front code verbatim into manager/docker.rs
- [ ] PR B: move the image build code verbatim into manager/build.rs
- [ ] PR B: move the system-layer code verbatim into system_layer.rs
- [ ] PR B: tests, quick checks, review approved, merge armed
- [ ] PR C: group the front fields of Live into FrontState
- [ ] PR C: tests, quick checks, review approved, merge armed
- [ ] Closing summary with Not done list

## Acceptance criteria

- **AC-1** (CLN-001) `scripts/cargo nextest run -p yulius-sandbox` passes unchanged except for
  the tests of deleted code, and Yulius CI is green on each pull request's head.
- **AC-2** (CLN-002) `git diff --color-moved` shows the move commit as moves only.

## Test plan

The existing `yulius-sandbox` tests, and Yulius CI. No new behaviour, so no new tests.

## Decisions

- This file is the minimum Yulius needs to run CI for a chore (see above). No ADR.
