---
yulius-task: c74a8653-3073-41c5-96e3-554d200bf0bd
status: done
---

# SPEC-139: `yulius wait` and `delivery_merge` report the current push state, not a stale note

Status: done
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
- The GitHub port's `branch_sha` is not implemented by the production adapter (it always fails;
  only the test fake answers, from its bare repository), so it cannot be the truth here (review
  round 1, blocker). The remote answers `git ls-remote` over the same transport and token as the
  push itself.

## Requirements

| ID | Requirement | Evidence |
| --- | --- | --- |
| PSH-001 | Every push Yulius makes of a task's head (end of turn, `delivery_merge`, Deliver…, the armed-merge upkeep, merge trains) goes through one helper. When `git push` fails but the remote says (`git ls-remote`) the branch is at that head, the push counts as done. A push that is done clears the task's note about that head. | `delivery::a_push_github_already_has_counts_as_done`, `delivery::a_pull_request_that_fails_to_open_after_its_push_keeps_its_note`, `delivery::a_push_clears_only_the_note_about_its_own_head`, `delivery::deliver_opens_the_pull_request_after_a_push_that_landed`, `git_ops::remote_branch_head_reads_the_remote_itself` |
| PSH-002 | `yulius wait` does not end on a failed-push note for its head when the remote's branch is at that head: the note is cleared and the wait goes on to the checks and the armed merge. The check costs one read of the remote per wait request, only while such a note exists, and at most `REMOTE_HEAD_TIMEOUT` (10 s, the Git process killed after it) after the task's token (usually cached); when the remote cannot be read in time the note stands. Without a pull request the note is replaced by one that says the branch is pushed and to call `delivery_merge`, since there is nothing to wait for. A note that is no push failure (not delivered, the pull request could not be opened after its push) stays. | `delivery::a_stale_push_note_does_not_end_the_wait_once_the_branch_is_at_the_head`, `delivery::a_pull_request_that_fails_to_open_after_its_push_keeps_its_note`, `delivery::a_pushed_head_without_a_pull_request_says_how_to_open_it`, `git_ops::reading_a_remote_branch_gives_up_on_a_stalled_remote` |
| PSH-003 | `delivery_merge` answers a failed delivery with the note that same delivery wrote (a refused push's remedy), otherwise with the failure itself, never with an older note about the same head. | `delivery::a_merge_call_answers_its_own_failure_not_an_older_note`, `delivery::a_merge_call_opens_the_pull_request_a_goal_task_still_lacks` |

## Plan

- [x] Spec and ADR for push outcomes
- [x] A push GitHub already has counts as done and clears the note
- [x] yulius wait ignores a push note once the remote branch is at the head
- [x] delivery_merge answers only with a note its own call wrote
- [x] Docs updated: architecture, specs, ADR index, changes
- [x] Review findings fixed: remote read via ls-remote, bounded, notes told apart
- [x] Gates green and Evidence recorded

## Acceptance criteria

- A push whose `git push` fails while the remote's branch is at the head records no note and
  clears an earlier one; the end of the turn opens the pull request.
- A pull request that fails to open after its push landed leaves a note saying so, and the wait
  ends on it even though the branch is at the head.
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
moved), a remote that cannot be read, and a failing `ahead_behind`; the fake GitHub can fail to
open a pull request. `remote_branch_head` is tested in `crates/yulius-git/tests/git_ops.rs`
against a real bare remote. Each test fails on `dev` before the fix. The existing
DLV-201/204 and DMG-001 tests stay green. Full gate: `scripts/dev ci`.

## Decisions

- [ADR-0113](../../docs/decisions/0113-github-s-branch-says-whether-a-head-is-pushed.md): the
  branch on GitHub, read with `git ls-remote`, not the exit code of the last `git push`, says
  whether a head is pushed; the delivery failure carries the note its own call wrote.
- `git ls-remote` rather than the GitHub API: the production adapter has no `branch_sha`, and
  implementing it would change CD and promotion, which call it too, in this fix (one concern per
  pull request). Reported in the summary instead.
- The push moves out of `dlv_open_pull` into its callers, so a failure to open the pull request
  after a push that landed is told apart from a push failure (review round 1); a refusal of the
  delivery rules pushes nothing and leaves a "not delivered" note (review round 2).
- `ls-remote` gets its own 10 s deadline inside `yulius-git`, where the runner kills the process
  group, rather than a `tokio::time::timeout` around the call, which would leave the Git process
  running (AGENTS.md: a timeout is not proof a subprocess died; review round 2).
- Accepted race (review round 2, nit): the head compared with the remote is the one observed before
  the push, while `push` sends the branch ref. A commit made in between whose push fails while the
  remote is at the observed head counts that observed head as pushed; the newer head is pushed at
  the next end of turn or `delivery_merge`, and its own failure is noted then. Pushing an explicit
  SHA would need a new refspec form in the push guard, out of proportion for this window.
- The token the wait's remote read needs is not put under a timeout (review round 3, minor): a
  dropped OAuth refresh can lose a rotated refresh token, which is worse than a rare slow first
  request. It is cached in the common case; the cost is stated in PSH-002.
- A note about an older head stays in the slot until a delivery replaces it (review round 3, nit):
  the wait reads only the note about its head and `delivery_merge` no longer reads the slot.
- Reconcile in the wait endpoint, not in the CLI: the CLI has no GitHub token, and the server
  already owns the note. Once per wait request, not per poll: a note that appears during a wait
  comes from a push that has just checked GitHub itself.
- The worktree's remote-tracking ref is not used as the truth: it moves only with pushes and
  fetches made from this clone and would miss a head pushed from elsewhere or a push GitHub took
  while reporting failure.

## Evidence

- Test-first: the three first regression tests (`a_push_github_already_has_counts_as_done`,
  `a_stale_push_note_does_not_end_the_wait_once_the_branch_is_at_the_head`,
  `a_merge_call_answers_its_own_failure_not_an_older_note`) failed on `dev` (43731493) for the
  reasons #366 and #448 describe: the old push error answered `delivery_merge`, the stale note
  stayed in the wait, and the older "is in_progress" note answered an unrelated failure.
- `cargo test -p yulius --test main -- delivery:: wait merge_train`: 78 passed (b2b346b6).
- `cargo test -p yulius-git --test git_ops remote`: 3 passed (ls-remote, stalled remote bounded at
  10 s).
- `scripts/dev ci` at b2b346b6: exit 0. Repository check, script tests, `cargo check`, `fmt`,
  `clippy -D warnings`, nextest 1755 passed / 16 skipped, doctests, `cargo deny`, `cargo shear`,
  `cargo doc`.
- `scripts/dev security`: exit 2, not run. Vidoc rejected the key (HTTP 401), as for every task
  since 2026-10-14 (#470); unrun, not passed (ADR-0104).
- Independent review in fresh sessions: round 1 (blocker: GitHub `branch_sha` is a production stub;
  major: a create-pull failure marked as a push failure), round 2 (major: unbounded ls-remote in
  the wait), round 3 approved 1e561b48 with minor findings fixed or recorded under Decisions; the
  final head is reviewed before `delivery_merge`.
- Not testable here: a real GitHub `remote rejected … (failed)`; the fake Git port injects it, and
  `ls-remote` with the token is the same transport the push uses.
- Follow-up outside this change (no issue created, per the task's rules): `GitHubAdapter::branch_sha`
  always fails in production, and CD (`cd_verified_head`) and promotion call it.
