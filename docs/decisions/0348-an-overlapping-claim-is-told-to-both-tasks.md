# 0348. An overlapping claim is told to both tasks

## Status

Accepted (2026-11-16). Specified in
[SPEC-493](../../.ai/specs/SPEC-493-faster-from-start-to-merge-opus-writes-f.md) (SCO-001 to
SCO-004, fix 9); task #472. Amends [SPEC-012](../../.ai/specs/SPEC-012-remove-task-messaging.md)
MSG-002 and SPEC-001 COM-003 ("overlaps are reported to the claiming agent only; no other task
is messaged").

## Context

Of the pull requests merged on 12-13 Nov, 29 of 60 in quartio-v2 and 7 of 42 here hit a
conflict at merge. Scope claims were advisory and their overlaps were told to the claiming agent
alone, with the other task's number and title: it did not know what that task was changing or
whether its pull request was about to merge, and the holding task never heard of the claim.
Both edited the same files and met at the merge.

## Decision

- **The claiming agent learns enough to decide** (SCO-001): each overlap in `scope_claim`'s
  answer names the holding task, its title, its claim's note, its step in progress and its pull
  request (number, stage, whether its merge is armed; none yet), with the advice of SCO-003.
- **The holding task is told once** (SCO-001): one typed Yulius note naming the claiming task and
  the paths (its pattern and the holder's), never again for the same pair and paths. A session
  that does not take typed text now gets it once it does, for about two minutes; a note never
  typed is tried again at the next such claim. Messages between tasks stay removed (SPEC-012):
  this is Yulius's note, as its other session notes are.
- **People see it** (SCO-002): a live task whose claims overlap another live task's carries
  `overlaps` (the other task's number, title and this task's patterns) in its view; its header
  and its row on the list say "overlaps #N" until a claim is released or a task is no longer
  live.
- **What to do** (SCO-003), the same in `scope_claim`'s description and `delivery-pipeline`, its
  short form in the briefing: "On an overlap both tasks are told: wait for the other's pull
  request to merge and merge the integration branch before editing those files, or agree who
  changes what by a note on the claim."
- **Before a task starts** (SCO-004): `task_create` takes `paths`, claimed for the new task. When
  a live or waiting task (not closed, stopped or failed) holds any of them, no task is made: the answer names
  the holder and offers `after_task: N` (run it after that task) or `start_anyway: true`.

## Alternatives

- **Locks on paths.** They would block work on a guess of what a task touches, and a task that
  stops holds them; claims stay advisory.
- **A message for every claim.** It would repeat for the same pair at every claim and drown the
  holding task; one note per pair and paths says it once.
- **Show only the claiming agent more.** The holding task, often the one about to merge, would
  still not know.

## Consequences

- A conflict is known when the second task claims, not at its merge.
- The holding task's session gets one more Yulius note; nothing blocks either task.
- A week later: the share of merged pull requests that hit a conflict, `audit_log` actions
  `delivery.conflict` per merged pull request (baseline 29 of 60 in quartio-v2, 7 of 42 here).
