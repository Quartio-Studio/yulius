---
yulius-task: c74a8653-3073-41c5-96e3-554d200bf0bd
status: in-progress
---

# SPEC-139: `yulius wait` and `delivery_merge` report the current push state, not a stale note

Status: in-progress
Date: 2026-10-15
Owner: Yulius task #123 (part of task #118, SPEC-138 lanes)
Scope authorization: the person's request (task #123): fix GitHub issues #366 and #448 together,
with integration tests in `crates/yulius/tests/delivery.rs`; "A push of a head the remote already
has must count as done and clear an earlier push error, and `yulius wait` must wait for the checks
/ the armed merge whenever the remote branch is at the local head"; "Only a note written by the
failing call may replace its error […] A refused push must still answer with its remedy".

## Goal

What Yulius tells an agent about pushing its head is true now. A head GitHub's branch already has
is pushed, whatever `git push` said; `yulius wait` never ends on a push note once the branch is at
the head; and `delivery_merge` answers a failure with that failure, or with the note the same call
wrote, never with an older one.

## Context

- DLV-201 ([SPEC-050](SPEC-050-parallel-ci-and-gentle-delivery.md)): when Yulius cannot push an
  agent's head, it records a note in the task's runtime (`push_problem`, head and text) and
  `yulius wait` ends on it. DLV-204 records "not delivered" notes (`note_undelivered`) in the same
  slot.
- #366 (task #66, PR #361): Yulius's push of `e3cbddb` failed with `! [remote rejected] … (failed)`;
  the agent's own `git push` then succeeded and CI ran at the head, but every `yulius wait` ended
  at once on the old note. The note was cleared only by a later end-of-turn delivery seeing the
  pull request at the head, which a goal task (no clean turn end) or a push from the armed-merge
  upkeep (DLV-203, merge trains) never reaches. Nothing ever asked GitHub where the branch is.
- #448 (task #101, SPEC-116 DMG-001): `delivery_merge` delivers the head first
  (`try_auto_open_pr`); on any error it answers with the stored note when its head is the current
  one, so that a refused push gets its remedy. A note written by an earlier call ("step X is
  in_progress") answers a later, unrelated failure.
- The GitHub port already reads a branch's head (`branch_sha`); the fake GitHub of the tests reads
  it from the real bare repository.

## Requirements

| ID | Requirement | Evidence |
| --- | --- | --- |
| PSH-001 | Every push Yulius makes of a task's head (end of turn, `delivery_merge`, Deliver…, the armed-merge upkeep, merge trains) goes through one helper. When `git push` fails but GitHub says the branch is at that head, the push counts as done. A push that is done clears the task's push note. | `delivery::a_push_github_already_has_counts_as_done` |
| PSH-002 | `yulius wait` does not end on a push note for its head when GitHub's branch is at that head: the note is cleared and the wait goes on to the checks and the armed merge. The check costs one GitHub read per wait request, and only while such a note exists; when GitHub cannot be read the note stands. | `delivery::a_stale_push_note_does_not_end_the_wait_once_the_branch_is_at_the_head` |
| PSH-003 | `delivery_merge` answers a failed delivery with the note that same delivery wrote (a refused push's remedy), otherwise with the failure itself, never with an older note about the same head. | `delivery::a_merge_call_answers_its_own_failure_not_an_older_note`, `delivery::a_merge_call_opens_the_pull_request_a_goal_task_still_lacks` |

## Plan

- [x] Spec and ADR for push outcomes
- [ ] A push GitHub already has counts as done and clears the note
- [ ] yulius wait ignores a push note once the remote branch is at the head
- [ ] delivery_merge answers only with a note its own call wrote
- [ ] Docs, gates, review and delivery

## Acceptance criteria

- A push whose `git push` fails while GitHub's branch is at the head records no note and clears an
  earlier one; the end of the turn says it delivered.
- After Yulius's push failed once and the agent pushed the head itself, `yulius wait` at that head
  reports no push problem and waits for the checks; the note is gone for later waits too.
- After `delivery_merge` was refused for an open step and the step is resolved, a call that fails
  for a reason unrelated to pushing answers with that reason.
- A rewritten branch GitHub refuses still answers `delivery_merge` with the remedy (merge the pushed
  head back in, never force-push).

## Test plan

Integration tests in `crates/yulius/tests/delivery.rs` over the HTTP and internal APIs, with the
test-local Git port given injectable failures: a push that fails without reaching the remote, a
push that reaches it and still reports failure (GitHub's `remote rejected … (failed)` after the ref
moved), and a failing `ahead_behind`. Each test fails on `dev` before the fix. The existing
DLV-201/204 and DMG-001 tests stay green. Full gate: `scripts/dev ci`.

## Decisions

- [ADR-0113](../../docs/decisions/0113-github-s-branch-says-whether-a-head-is-pushed.md): GitHub's
  branch, not the exit code of the last `git push`, says whether a head is pushed; the delivery
  failure carries the note its own call wrote.
- Reconcile in the wait endpoint, not in the CLI: the CLI has no GitHub token, and the server
  already owns the note. Once per wait request, not per poll: a note that appears during a wait
  comes from a push that has just checked GitHub itself.
- The worktree's remote-tracking ref is not used as the truth: it moves only with pushes and
  fetches made from this clone and would miss a head pushed from elsewhere or a push GitHub took
  while reporting failure.
