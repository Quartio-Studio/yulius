# 0033. A task's original prompt is read on demand; its files only from its own attachment folders

## Status

Accepted. Specified in
[SPEC-043](../../.ai/specs/SPEC-043-here-i-want-a-button-see-the-original-pr.md). Its read bound and
the `/proc/self/fd` fallback on Linux are amended by
[ADR-0064](0064-attachment-reads-have-their-own-bound.md).

## Context

People want to read again the prompt a task started with, with the files they attached. The
task record keeps the prompt as the composer sent it: the text, then one `@path` line per
attached file. No API returned it. A prompt can be 100,000 characters, and an `@path` line can
name any file on the host, not only an upload.

## Decision

- `GET /api/tasks/{tid}/prompt` returns the text (the attachment lines removed, as the session
  got it) and the attached files by name, with whether each is an image and still available. The
  task menu's "See the original prompt" reads it when the dialog opens.
- `GET /api/tasks/{tid}/prompt/attachments/{n}` serves attachment `n` of the parsed prompt. The
  client never names a path. The file is served only when the prompt names it directly in one of
  the task's own attachment folders, as an upload reports it: the creator's composer drafts
  (`attachments/drafts/<creator>/`) or the task's terminal attachments
  (`attachments/tasks/<task>/`). Those folders must be real folders, not links; the file is
  opened with `O_NOFOLLOW | O_NONBLOCK`, checked through its handle (a regular file within the
  upload limit) and, where `/proc/self/fd` exists, must have been opened at its expected real
  path. Otherwise 404. Images get their image type; everything else downloads as
  `application/octet-stream`; every answer has `nosniff` and a `sandbox` content security
  policy. Reads share the uploads' concurrency bound (now their own: ADR-0064).
- Both need read access to the task's sandbox, like the task itself.

## Alternatives

- A `prompt` field on `TaskDetail`: the detail is read on every task update, so long prompts
  would travel again and again for a dialog that is rarely opened.
- Serving any mentioned path: would turn a prompt line into a read of any file the server can
  open.
- Any file whose resolved path is inside the sandbox's `attachments` folder (the first version):
  a contained process can replace that folder with a link, and resolving it then accepts any
  host file; it also exposed other users' drafts and private shell attachments.
- Write access for the files: the files were given to the agent, whose terminal readers can
  already watch, so it would hide less than it seems and confuse readers.

## Consequences

- An attachment of a task older than a week is usually gone (composer drafts are removed after a
  week); the dialog lists it by name as no longer on the server.
- A folder swapped for a link between the checks and the open is caught on Linux by the
  `/proc/self/fd` check; where that is missing, the race stays as for uploads.
- Files a person dropped on another task, or on a shell, are not shown with this task's prompt
  even if its prompt names them.
