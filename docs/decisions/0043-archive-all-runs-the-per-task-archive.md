# 0043. "Archive all" runs the per-task archive; no bulk endpoint

## Status

Accepted. Specified in [SPEC-062](../../.ai/specs/SPEC-062-i-want-a-quick-button-to-archive-tasks-a.md).
It extends UI-006 (SPEC-001) and keeps the archive rules of SPEC-013 and TSK-014.

## Context

The sidebar's Done group can hold a dozen finished tasks, and clearing them took a menu and a
click for each. The person asked for a quick Archive button and an "Archive all" for Done.
Archiving already exists per task (`POST /api/tasks/{tid}/archive`). The server checks each task
on its own: a Done task's session is closed as it is archived, and any other live task is refused.
Which tasks are Done is decided in the browser by `taskInfo` (`status.js`). That rule needs the
pull request's checks, which the client already holds.

## Decision

- Each sidebar row that can be archived has an Archive button beside its `⋮` button, shown on
  hover and focus. It does what the menu's Archive does.
- The Done group's head has "Archive all". It sends the existing per-task archive request for every
  archivable Done task shown, at most 4 at a time. There is no confirmation: one toast reports the
  result, and its Undo restores the whole batch. When some requests fail, the others stay
  archived and one error toast says how many failed and why.
- Phones keep one row action (`⋮`, whose menu has Archive). "Archive all" is shown there too.

## Alternatives

- A bulk endpoint (`POST /api/sandboxes/{sid}/tasks/archive-done`): one request, but the server
  would have to decide "Done" again without the client's pull request view, and a partial
  failure would need its own response shape. Rejected for now: a dozen requests at 4 at a time
  are fast, and each keeps its own check and audit.
- A confirmation before "Archive all": rejected. Archiving can be undone (UI-006 keeps it one
  click), and the toast's Undo covers the whole batch. Permanent deletion keeps its confirmation.
- The quick button on Done rows only: rejected. Stopped rows can be archived too, and the
  request asks for a quick way to archive tasks in general.

## Consequences

- No API or storage change. The UI's archive path stays a literal checked by `ui_contract.rs`.
- Undo restores the tasks, but a Done task's session was closed by the archive (TSK-014). So it
  comes back as closed, under Tasks rather than in the sidebar's active list, just as after the
  menu's Archive.
- A very long Done group sends as many requests as it has tasks, bounded to 4 at once. A bulk
  endpoint can replace this later without changing the UI's behaviour.
