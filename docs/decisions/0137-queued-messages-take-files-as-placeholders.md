# 0137. Queued messages take files as placeholders

## Status

Accepted. Specified in [SPEC-168](../../.ai/specs/SPEC-168-for-images-here-we-want-movable-but-also.md);
task #158. Extends [SPEC-052](../../.ai/specs/SPEC-052-even-if-a-task-is-queued-i-d-like-to-be.md) and
[0124](0124-uploaded-files-sit-in-a-tray-the-text-holds-placeholders.md).

## Context

The "Queued messages" card of a waiting task showed its request as plain text, so every file the
composer had put in it read as a raw `@'/var/lib/yulius/…/pasted-….png'` mention. Its message box
was a textarea: an image could not be added to a queued message at all. The new-task composer
already shows files as one-line placeholders (`Image N`) that move and delete like one character,
and uploads pasted or dropped files.

## Decision

- The card's request is drawn from `parsePrompt`: text as written, each file as a button
  placeholder (`Image N` for an image, numbered in order of appearance; the name otherwise) that
  reads the file from `GET /api/tasks/{tid}/prompt/attachments/{n}` when clicked. An image opens in
  the preview dialog; another file downloads.
- The message box is the composer's prompt input (`createPromptInput`) with its paste and drop
  flow (`attachPromptDrop`). Files upload with `POST /api/tasks/{tid}/attachments` into the task's
  own folder, and the message is sent with each file as `@'<path>'` at its place.
- The card has no tray: a file deleted from the box is gone, undo brings it back.

## Alternatives

- **An overlay on the textarea drawing placeholders.** A placeholder could not move or delete as
  one character.
- **Uploading to the person's drafts folder (as the composer does).** The prompt endpoint serves a
  draft folder only for the task's creator, so a teammate's queued image would show as gone.
- **A tray as in the composer.** Takes room from a card that floats over the terminal, for messages
  that are short.

## Consequences

- No server change: the task folder and the prompt endpoint already existed (SPEC-043, SPEC-080),
  and the agent reads files anywhere in its request (SPEC-124).
- A file uploaded and then deleted from the box before queueing stays in the task's folder,
  as a file dropped on the terminal does; it is removed with the task's attachments.
- The card's request opens files over the same endpoint as the original prompt dialog, so a file
  gone from the server reads as gone in both.
