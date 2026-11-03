# 0259. Shell and draft attachments stay while a request names them

## Status

Accepted (2026-11-03, task #395, [SPEC-413](../../.ai/specs/SPEC-413-attachments-fix-1027-1023-1031.md)).
Extends [ADR-0253](0253-a-deleted-tasks-attachments-stay-while-named-and-wait-a-week-in-the-trash.md)
to the other attachment folders.

## Context

ADR-0253 keeps a deleted task's attachments while a remaining task's request or an automation's
prompt names them. Two older paths still removed named files: a shell's folder
(`attachments/shells/<shell>/`) went when the shell ended or at start-up, so a screenshot pasted
into the Superagent and named in a `task_create` prompt was lost (#1027); a composer draft
(`attachments/drafts/<user>/<file>`) older than a week went at its owner's next upload, even while a
prepared or chained task's request named it (#1023).

## Decision

- The same texts ADR-0253 reads (every task's request, archived ones included, and every
  automation's prompt) decide: a shell folder whose `attachments/shells/<shell>/` path one of them
  contains stays in place when the shell ends and at start-up; an old draft whose
  `attachments/drafts/<user>/<file>` path one of them contains is not pruned.
- Unnamed ones go as before: a shell's folder is removed, a draft older than a week is unlinked
  (at its owner's next upload and by the leftovers round, which checks again before deleting).
  A kept shell folder is asked about again at each start-up.
- The drafts folder's bound of 500 counts only drafts younger than a week, the ones waiting for a
  task; older ones a request keeps are the tasks' files.
- When those texts cannot be read, nothing is removed.

## Alternatives

- **Move drafts into the task's folder when it is created, rewriting the request.** Gives each
  task its own files, but rewrites a request a person wrote, misses references added later (a
  queued message, another task naming the same draft) and needs both steps to be atomic; rejected
  as in ADR-0253.
- **A reference table written when a task is created.** Exact, but a second mechanism beside
  ADR-0253's search, and it would miss requests changed later; rejected.
- **Send unnamed shell folders to the trash.** The trash is keyed by a deleted task and shell
  files are a terminal's scratch space; removal stays.

## Consequences

- A shell's or a draft's files can outlive it for as long as a task's request names them, using
  disk (a shell's folder bounded at 500 files, kept drafts only by the sandbox's quota).
- Every composer upload while an old draft is kept, and the end of each shell that got a file,
  read the sandbox's tasks and automations once.
- A task created in the instant between that reading and a removal can still see an unnamed file
  go, as with ADR-0253's settling.
- References typed only into a running session are still not seen.
