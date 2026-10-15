---
yulius-task: f749919e-3eff-4edf-9cee-94755b107da6
status: in-progress
---

# SPEC-168: Queued messages show and take files as placeholders

Status: in progress
Date: 2026-10-15
Owner: Yulius task #158
Scope authorization: task #158, "For images here we want movable but also clickable placeholders
not raw "@" - like in the task view. I also want to upload images to the queue" (screenshot: the
"Queued messages" card whose request shows two `@'/var/lib/yulius/…/pasted-….png'` mentions as
raw text). Extends SPEC-052 (queued messages), SPEC-124 (files inside the prompt's text) and
SPEC-150 (`Image N` placeholders).

## Goal

The "Queued messages" card of a waiting task reads like the new-task composer. Each file in the
request shows as a one-line placeholder (`Image 1`, or the file's name) that opens the file when
clicked, instead of a raw `@'/path'` mention. The message box takes images and files: pasted or
dropped, each lands at the caret as a placeholder that moves (drag, cut and paste) and deletes
like one character, and reaches the agent at its place when the message is queued.

## Context

- `taskview.js` renders the card (SPEC-052): `requestEl.textContent = detail.request`, so every
  mention of a file (the composer writes `@'<path>'`, SPEC-124) shows as raw text. The message box
  is a `textarea`; a pasted image does nothing.
- The new-task composer already has everything the card lacks:
  - `promptinput.js` `createPromptInput`: a text box whose files are inline atoms, moved by drag
    or cut and paste, opened by double-click (`onOpen`), with `value` giving the text with each file
    at its place as `@'<path>'`.
  - `filedrop.js` `attachPromptDrop`: paste and drop upload, with the placeholder kept at the caret
    while the file uploads.
  - `promptparts.js` `parsePrompt`: the request's text and files in order, as the server's
    `Request::parse` reads them.
- The server needs no change:
  - `POST /api/tasks/{tid}/attachments` (`Action::Work`) stores a file in the task's own folder
    `attachments/tasks/<task>`.
  - `GET /api/tasks/{tid}/prompt/attachments/{n}` serves the n-th file of `Request::parse(task.prompt)`
    from the creator's drafts folder or that task folder. The prompt includes queued messages, so a
    file of a queued message is served like one of the original prompt.
  - The request file, the goal and the first message already read files anywhere in the prompt
    (SPEC-124), so the agent gets a queued message's files at their place.

## Requirements

| ID | Requirement | Evidence |
| --- | --- | --- |
| QMF-001 | The card's request shows each file it names as a placeholder at its place: an image as `Image N` (N its number among the request's images, in order; an image named twice keeps one number), another file as its name. No raw `@'/path'` mention shows. Text stays as written (lines kept). | `queued-messages.e2e.ts` "the request shows its files…" |
| QMF-002 | Clicking (or Enter on) an image placeholder in the request opens the image in the preview dialog, once however often it is clicked meanwhile. Another file downloads. A file the server does not have says so in a toast. Each file is read once per view and its URL revoked when the view goes. | `queued-messages.e2e.ts` "the request shows its files…" |
| QMF-003 | The message box is the new-task prompt input ("Message for the agent"). An image or file pasted or dropped on it uploads to the task (`POST /api/tasks/{tid}/attachments`) and lands at the caret as a placeholder, numbered after the request's images in the text's order. An upload that ends after the task started is not added and says so ("The task has started: the file was not added."). | `queued-messages.e2e.ts` "an image pasted into the message…", "an image deleted before queueing…", "an upload that ends after the task started…" |
| QMF-004 | A placeholder in the box moves by drag or cut and paste and deletes with one Backspace; double-click opens the image. | `queued-messages.e2e.ts` "an image pasted into the message…" |
| QMF-005 | Queue sends the message with each file as `@'<path>'` at its place; the request then shows it as a placeholder. While a file still uploads, Queue refuses in place ("Wait for the upload to finish.") and keeps the text; the reason goes when the upload ends. | `queued-messages.e2e.ts` "a message waits for its upload…" |
| QMF-006 | Everything SPEC-052 and SPEC-056 promised stays: ⌘/Ctrl+Enter queues, a refusal keeps the text with the reason, a task that started keeps the card read-only with Dismiss, keys typed before the terminal shows stay in the box. | existing `queued-messages.e2e.ts` tests |

## Plan

- [x] Read the product context and related work, then write and validate the spec
- [x] The queue card's request shows its files as clickable Image N placeholders
- [x] The queue box takes pasted and dropped images as movable placeholders and queues them
- [x] Review round 1 in a fresh session, its findings fixed test-first
- [ ] Review round 2 approves the exact head
- [ ] Every gate green, docs and the changes log updated

## Acceptance criteria

- A queued task whose request names `shot.png` shows "Image 1" in the card, not the path.
  Clicking it opens a dialog titled `shot.png` with the image.
- Pasting an image after "See " in the message box shows "Image 2" there. Dragging it or cutting
  and pasting it moves it. Queue adds the message, and the request shows "See Image 2" with the
  stored request holding `@'<task folder>/<name>.png'` at that place.
- The card has no axe violations with placeholders shown.

## Test plan

- `e2e/tests/queued-messages.e2e.ts`: new tests for QMF-001..005, written first and failing on the
  unchanged view (the request shows the raw mention; the box is a textarea that ignores a pasted
  image). They wait on conditions (counts, text, the mock's stored prompt), never on time.
- Existing tests in that file move from `toHaveValue` (a textarea) to the prompt input's text and
  keep their assertions (QMF-006).
- The mock serves a task's own upload folder for every task, as the server does (`Owners::folders`).
- Gates: `scripts/dev check`, `scripts/dev lint`, `scripts/dev test`, the e2e suite,
  `scripts/dev security`.

## Decisions

- **Reuse the new-task prompt input and its upload flow** (ADR-0137). Options: (a) keep the
  textarea and render placeholders in an overlay; (b) a second, smaller atom editor for the card;
  (c) `createPromptInput` with `attachPromptDrop`. (a) cannot move a file as one character. (b)
  duplicates SPEC-124's editing rules. (c) gives the same movement, undo and keys as the composer
  with no new editor. Chosen: (c).
- **No tray in the card.** The composer's tray (SPEC-150) holds files outside the text for a
  long prompt. A queued message is short, and the card floats over the terminal with little room.
  A file deleted from the box is gone (undo brings it back). Reversible: a tray can be added later.
- **Files upload as task attachments, not drafts.** The task exists. Its folder is served by the
  prompt endpoint for any person who can view the task. A draft folder is the uploader's: it is
  served only for the task's creator, and a teammate queueing a message would see "No longer on
  the server".
- **The request reads its files on click, not up front.** A request can name many files; the card
  shows text and placeholders at once and fetches one file when it is opened.
- **Numbers.** The request numbers its images in order of appearance. The box numbers its images
  after the request's, in the order its text holds them, so each keeps its number once queued (a
  deleted image gives its number up; moving one before another swaps theirs). An image named twice
  has one number on both sides. Review round 1 replaced numbering in upload order, and round 2
  numbering each occurrence in the request, which both changed a number once queued.
- **A pending upload keeps the card.** A task that starts while a file uploads into an otherwise
  empty box keeps the card (read-only, with Dismiss), so "the file was not added" is seen (round 2).
