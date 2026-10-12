# 0032. A task's original prompt is read on demand; its files only from the attachments folder

## Status

Accepted. Specified in
[SPEC-043](../../.ai/specs/SPEC-043-here-i-want-a-button-see-the-original-pr.md).

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
  client never names a path. The file is served only when its resolved path is a regular file
  inside the sandbox's `attachments` folder and within the upload size limit; otherwise 404.
  Images get their image type; everything else downloads as `application/octet-stream`; every
  answer has `nosniff` and a `sandbox` content security policy.
- Both need read access to the task's sandbox, like the task itself.

## Alternatives

- A `prompt` field on `TaskDetail`: the detail is read on every task update, so long prompts
  would travel again and again for a dialog that is rarely opened.
- Serving any mentioned path: would turn a prompt line into a read of any file the server can
  open.
- Write access for the files: the files were given to the agent, whose terminal readers can
  already watch, so it would hide less than it seems and confuse readers.

## Consequences

- An attachment of a task older than a week is usually gone (composer drafts are removed after a
  week); the dialog lists it by name as no longer on the server.
- Like the upload checks, the containment check is by path: a swap racing the moment between
  the check and the read is not excluded.
