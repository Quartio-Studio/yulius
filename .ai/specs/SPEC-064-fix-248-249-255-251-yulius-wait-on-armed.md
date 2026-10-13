---
yulius-task: 6fbdd785-4734-4f95-8e46-55f4fe729507
status: done
---

# SPEC-064: `yulius wait` reads an armed merge and its outcome together; the errored-CI retry is tested

Status: implemented
Date: 2026-10-13
Owner: Yulius task #53
Scope authorization: GitHub issues #248, #249, #255 and #251, through task #53 (part of task #47,
"fix all open actionable GitHub issues"). No new GitHub issue is created.

## Goal

`yulius wait` waits on the merge the agent armed last, and ends with that merge's outcome, never
with an older one and never before it is decided. An armed merge, the count of its decisions in
progress and how the last one ended are one value, changed and read under one lock. The automatic
retry of an errored Yulius CI run at an open pull request's head is covered by a test.

## Context

- **What changed since #248 and #249 were filed.** Both quote "The merge you armed was decided;
  Yulius's notice says how (`messages_inbox`)". SPEC-028 (task #26) removed notices: the outcome
  of an armed merge is now kept in memory next to the armed slot (`TaskRuntime::merge_outcome`)
  and `wait_cli::settled` prints it only when the wait sees the merge go from armed to unarmed
  with a new outcome (MSR-003). SPEC-050 DLV-204/205 added the "deciding" counter and saved armed
  merges across restarts. The quoted message no longer exists.
- **The state today** (`crates/yulius-app/src/agent_wait.rs`), per task in `TaskRuntime`:
  `armed_merge: Mutex<Option<ArmedMerge>>`, `merge_outcome: Mutex<Option<MergeOutcome>>` and
  `merge_deciding: AtomicU32`. `merge_armed` raises the counter, takes the slot, decides
  (`merge_armed_task` writes the outcome as it decides), puts back a kept merge, then lowers the
  counter. `agent_wait_state` reads the three one at a time.
- **#255 (confirmed by reading).** Between "outcome written" and "counter lowered" a poll sees
  `merge_armed: true` with the new outcome; the next poll sees it unarmed with the same outcome, so
  `settled` never counts the decision (`before.merge_outcome == now.merge_outcome`). A refusal at
  green checks then ends the wait with "The checks at X passed. Merge with `delivery_merge`", and a
  drop leaves the wait running to its deadline.
- **#248 on current `dev`.** Arming clears the outcome, so the sequence in the issue (arm A, the
  branch moves, A dropped, arm B) no longer ends a wait on B. One interleaving still does: the
  agent arms B *while* Yulius is deciding A (GitHub slow). `merge_armed` then puts the stale A back
  over B (checks pending) or writes A's outcome after B was armed; the wait on B ends with A's
  "dropped: you armed A but the branch is at B", and the saved file loses B.
- **#249 on current `dev`.** With a merge armed and checks pending the wait state reads armed with
  no outcome, and `settled` keeps waiting. To be proven by a regression test; closed with that
  evidence if it does not reproduce.
- **#251.** `retry_errored_checks` / `retry_errored_run` (`crates/yulius-app/src/delivery.rs`,
  SPEC-020) have no test.

## Requirements

| ID | Requirement | Evidence |
| --- | --- | --- |
| WAI-001 | With a merge armed and its checks pending, `yulius wait` keeps waiting: the wait state reads armed with no outcome, the server's wait returns it unchanged at its timeout, and `settled` returns nothing until the checks end, the merge is decided or the stage moves (#249). | `delivery::a_wait_on_an_armed_merge_with_pending_checks_keeps_waiting` |
| WAI-002 | A merge armed while an earlier one is being decided supersedes it: the earlier decision neither puts its merge back, nor records its outcome, nor rewrites the saved armed merges. A wait on the new merge ends only with the new merge's outcome (#248). A restore after a restart never revives a merge armed and decided in the new process. | `delivery::a_merge_armed_while_an_earlier_one_is_decided_supersedes_it`, `delivery::a_restore_never_brings_back_a_merge_decided_since`, `agent_wait::tests` |
| WAI-003 | The armed merge, the count of decisions in progress and the last outcome are one value under one lock. A decision ends (outcome or merge kept, counter lowered) in one step, and the wait state reads all three in one step, so an outcome is never visible while that merge still reads as armed (#255). | `agent_wait::tests::a_decision_ends_in_one_step` |
| WAI-004 | `reconcile_deliveries` queues an errored Yulius CI run at an open pull request's head again exactly once; with two errored runs at that head it queues no third; a pull request that conflicts with its base (`mergeable: false`) is not retried (#251). | `delivery::an_errored_run_at_an_open_pull_request_is_retried_once` |

## Design

- `TaskRuntime` keeps one `armed: Mutex<ArmedSlot>` instead of the three fields. `ArmedSlot`
  (in `agent_wait.rs`) holds `merge: Option<ArmedMerge>`, `deciding: u32`,
  `outcome: Option<MergeOutcome>` and `generation: u64`, with small methods:
  - `arm(merge)`: a new generation, the merge in the slot, the outcome cleared;
  - `take() -> Option<Decision ticket>`: the merge out of the slot, `deciding + 1`, the
    generation it belongs to;
  - `finish(ticket, Verdict)`: `deciding - 1` and, only when no newer merge was armed since
    (same generation), either the kept merge back in the slot or the outcome recorded; returns
    whether the saved merges must be rewritten;
  - `clear()` (merged at once by `delivery_merge`): no merge, a new generation;
  - `view() -> (armed, outcome)` for the wait; `App::forget_merge_outcome` clears the outcome
    under the same lock.
- `restore_armed_merges` (DLV-205) puts a saved merge back only while the slot's generation is
  0: nothing was armed or cleared in this process, so a merge armed and decided since the file was
  read is never revived.
- `merge_armed_task` returns a `Verdict` (`Keep(ArmedMerge)` or `Decided(text)`) instead of
  writing the outcome itself; `merge_armed` applies it with `finish` under the lock and saves the
  armed merges file while still holding it, so the file follows the order of the slot's changes.
- `wait_cli::settled` becomes `pub` so the integration tests feed it the server's wait states.

## Plan

- [x] Spec, steps and reproduction tests of #249 and #248 against current `dev`.
- [x] One-lock armed slot with superseding generations (fixes #255 and the #248 interleaving).
- [x] Integration test of the errored-CI retry (#251).
- [x] ADR, architecture and changes entry.
- [x] Gates green and independent review of the head.
- [x] Spec done with evidence; pull request ready (closing the issues follows the merge).

## Acceptance criteria

- The four requirement tests pass; the #248 interleaving test fails on the code before the fix
  (recorded under Evidence). #255 is fixed by construction and pinned by a unit test of
  `ArmedSlot` (see Decisions).
- `scripts/dev ci` is green.
- #248, #249, #255 and #251 are closed after the merge into `dev`, each with what changed.

## Test plan

- Integration (`crates/yulius/tests/delivery.rs`, the `ArmedTask` fixture): WAI-001, WAI-002 (the
  fake GitHub holds the checks read of the decision of A while the agent arms B), WAI-004 (errored
  run records inserted at the pull request's head; the profile's gate makes `ci_rerun` possible).
- Unit (`agent_wait::tests`): `ArmedSlot` transitions, including a decision superseded by a new
  arm and the one-step finish.

## Decisions

- One lock over a reordered read (the issue's first option, `merge_armed && outcome.is_none()`):
  a merge armed while an older one is decided would read as unarmed as soon as the old outcome is
  written, so the reorder alone trades #255 for a #248 variant. ADR-0048 (numbered 0046 until `dev` took 0046 and 0047).
- #255's window had no `await` in it (the outcome was written, then the armed merges file saved,
  then the counter lowered), so no integration test could stop a poll inside it deterministically.
  The fix removes the window by construction (`ArmedSlot::finish` is one method under the lock),
  and `agent_wait::tests::a_decision_ends_in_one_step` pins that invariant: armed with no outcome
  until the decision ends, then unarmed with the outcome. The #248 interleaving, which the same
  restructuring fixes, does have a deterministic failing-before test.
- A superseded decision is discarded rather than reported: the agent armed a newer head and
  `delivery_merge`'s own answer already told it the older one no longer counts.

## Evidence

- **#249 does not reproduce on `dev`.** `delivery::a_wait_on_an_armed_merge_with_pending_checks_keeps_waiting`
  passed on the code before the fix (`cargo test -p yulius --test main -- delivery::a_wait_on_an_armed`):
  with a merge armed and checks pending the wait state is armed with no outcome, the server's wait
  returns it unchanged, and `settled` keeps waiting until the merge is decided ("Yulius merged pull
  request #1 …"). The quoted "was decided; Yulius's notice says how" message no longer exists
  (SPEC-028). The test stays as the regression guard.
- **#248 reproduced, then fixed.** Before the fix,
  `delivery::a_merge_armed_while_an_earlier_one_is_decided_supersedes_it` failed at the wait on the
  new head with `"merge_armed":false,"merge_outcome":"The merge you armed was dropped: you armed
  36f1d53ae … but the branch is now at a10f216a0"`, the reported symptom. It passes after the fix;
  the independent review also reverted the three production files to the merge base and saw it
  fail.
- **#255** is fixed by construction (`ArmedSlot::finish`, one `view()` read); pinned by
  `agent_wait::tests::a_decision_ends_in_one_step` and `a_newer_arm_supersedes_a_decision_in_progress`
  (`cargo test -p yulius-app --lib agent_wait`: 3 passed).
- **Restore guard** (review round 1): `delivery::a_restore_never_brings_back_a_merge_decided_since`
  failed before the `generation == 0` guard (`"merge_armed":true` next to its own "dropped"
  outcome) and passes after.
- **#251**: `delivery::an_errored_run_at_an_open_pull_request_is_retried_once` passes. Mutating
  `retry_errored_run` / `retry_errored_checks` makes it fail each time: `errored < 3` → "no third run";
  dropping the `mergeable != Some(false)` check → "conflicting: not retried"; removing the call from
  `reconcile_deliveries` → one run instead of two.
- **Gate**: `scripts/dev ci` exit 0 at `e78afc4` (repository contracts, check, fmt, clippy
  `-D warnings`, nextest 1289 passed / 16 skipped, doctests, cargo-deny, cargo-shear, docs).
- **Independent review** (fresh read-only session, `om-code-review`): round 1 APPROVE at `6631d6c`
  with four minors and nits, fixed test-first in `e78afc4`; round 2 APPROVE at `e78afc4`; its one
  remaining wording note on the ADR is applied in the final docs commit.
