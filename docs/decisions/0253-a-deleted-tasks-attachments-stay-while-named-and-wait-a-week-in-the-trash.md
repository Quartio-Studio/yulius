# 0253. A deleted task's attachments stay while named and wait a week in the trash

## Status

Accepted (2026-11-03, task #386, [SPEC-396](../../.ai/specs/SPEC-396-deleting-a-task-must-not-destroy-attachm.md)).
Amends TSK-009's removal of a task's attachments with the task.

## Context

Deleting a task removed `attachments/tasks/<task>/` at once. A task created with the same prompt
named those files by their absolute paths, and lost them when the first was deleted (quartio-v2,
2026-11-02). Nothing warned, and nothing could be restored.

## Decision

- When a task is deleted, every attachment folder of the sandbox whose task no longer exists is
  settled: one that a remaining task's request (prompt with its queued messages) names by its
  `attachments/tasks/<id>/` path stays where it is, whole; any other moves, by one rename through
  folder handles, to `attachments/trash/<secs>-<number>-<id>/`. A kept folder goes to the trash
  once the last task naming it is deleted.
- Trash entries are removed 7 days after the deletion (at deletions, listings, start-up and an
  hourly reaper round).
- `attachments_trash` lists the trash and restores an entry's files into a live task's folder
  under free names, answering their new paths; `task_update` changes a waiting task's start in
  place (SPEC-126's change), so recreating a task to chain it is never needed.

## Alternatives

- **Copy referenced files into the referencing task and rewrite its request.** Gives each task its
  own files, but rewrites a request a person wrote and misses references added later; rejected.
- **Refuse the deletion while files are named.** Safe, but blocks a legitimate cleanup and the
  person has to work out what to move; rejected.
- **A trash outside the sandbox directory.** Out of a contained process's reach, but a rename across
  file systems is a copy, and the sandbox can already change the originals; rejected.
- **Restore to the original path.** Old paths would work again, but the folder would be an orphan
  that nothing names, back in the trash at the next deletion; restoring into a live task gives the
  files an owner.

## Consequences

- A kept folder lives as long as some task's request names it; deleting the naming tasks sends it
  to the trash.
- Deleted attachments use disk for up to a week; the trash counts toward the sandbox's quota.
- References typed only into a running session are not seen; the trash is the safety net for
  them.
