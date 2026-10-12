---
yulius-task: fd776a84-5a81-42e8-96af-9d3061ec37c0
status: in-progress
---

# SPEC-043: "See the original prompt" in the task menu

Status: in progress
Date: 2026-10-13
Owner: Yulius task #42
Scope authorization: user request (F 2026-10-12, task #42): "Here I want a button: 'see the
original prompt' that shows the prompt I uploaded here, in the task window", with a screenshot
of a task's actions menu (Stop, Continue, Delivery…, Feedback…, Archive, Delete permanently…).

## Goal

From the task menu, a person can see the prompt a task was started with: its text, formatted as
it was written, and the files attached to it (images shown, other files downloadable). The
task's terminal scrolls the prompt away, and a goal task's terminal shows only the `/goal`
condition, so today the text cannot be read again in the UI.

## Context

- The task record keeps the initial prompt (`TaskRecord.prompt`) exactly as the composer sent
  it: the text, then one `@path` line per attached file (`withAttachments` in `filedrop.js`).
  `yulius_domain::request::Request::parse` splits it into the text and the attachment paths, as
  the session start does (SPEC-025, SPEC-030).
- No API returns the prompt: `TaskSummary` and `TaskDetail` carry the title only, which is the
  prompt's first line, cut to 60 characters.
- Files attached on the composer are stored under `sandboxes/<sid>/attachments/drafts/<user>/`
  (files dropped on a task's terminal go under `…/attachments/tasks/<task>/`). Drafts are
  removed after a week (`DRAFT_MAX_AGE`), so an attachment of an older task can be gone. A
  prompt may also mention any path by hand (`@/etc/hosts`), which is not an upload.
- The task menu (`openTaskMenu` in `actions.js`, UI-013) opens from the sidebar row and from the
  task view's step strip; dialogs such as "Delivery…" fetch their data when opened.
- The composer already renders Markdown read-only (`createMarkdownEditor({ readOnly: true })`)
  and previews an attached image in a dialog (`openAttachment`).

## Requirements

| ID | Requirement | Evidence |
| --- | --- | --- |
| OPR-001 | `GET /api/tasks/{tid}/prompt` (read access to the task's sandbox, like `GET /api/tasks/{tid}`; 404 otherwise) returns the task's original prompt: `text` (the prompt without its attachment lines, as the session got it) and `attachments`, in order, each with its file `name`, whether it is an `image` (the extensions Claude Code pastes as images) and whether it is `available` (a regular file in the sandbox's attachments folder). | `task_prompt.rs` |
| OPR-002 | `GET /api/tasks/{tid}/prompt/attachments/{n}` (same access) answers with the bytes of attachment `n` only when its path, resolved, is a regular file inside the sandbox's `attachments` folder, at most the upload limit in size; anything else (out of range, gone, outside the folder, a link out of it, a folder, too large) is 404. Images are served with their image type; every other file as `application/octet-stream` with `Content-Disposition: attachment`; always `X-Content-Type-Options: nosniff` and a `sandbox` content security policy, so no file runs as a page. | `task_prompt.rs` |
| OPR-003 | The task menu has "See the original prompt", enabled for everyone who sees the task. It opens a dialog titled `Original prompt · #<n>` that shows the text formatted as Markdown (read-only), then the attached files: an image as a thumbnail that opens full size, another file as a download link, a file that is gone by its name with "No longer on the server". A prompt without text says so. "Copy text" copies the text. A failed load says why in the dialog. | e2e `original-prompt.e2e.ts` |
| OPR-004 | `docs/api.md`, SPEC-001 UI-013, an ADR, `docs/architecture.md` (if a component changes) and `docs/changes.md` describe it. | review |

## Plan

- [x] Read the product context and related work, then write and validate the spec
- [x] The API returns a task's original prompt text and its attached files
- [x] The API serves a task's attached files from the attachments folder only
- [x] "See the original prompt" in the task menu shows the text and the attached files
- [ ] Independent review in a fresh session, findings fixed test-first
- [ ] Every gate green, docs and the changes log updated

## Acceptance criteria

- AC-OPR-1: a task created with "Fix the **login**\n\n@<draft>/shot.png\n@<draft>/notes.txt\n@/etc/hosts"
  returns `text` "Fix the **login**" and three attachments: `shot.png` (image, available),
  `notes.txt` (not an image, available), `hosts` (not available). A read user gets the same; a
  user without access gets 404.
- AC-OPR-2: attachment 0 is served as `image/png` with `nosniff`; attachment 1 as
  `application/octet-stream` with `Content-Disposition: attachment`; attachment 2, index 9, a
  link in the attachments folder pointing outside it and a removed file are 404.
- AC-OPR-3: in the UI, a task's menu → "See the original prompt" shows the formatted text and
  the image thumbnail; clicking the thumbnail previews it; the dialog has no WCAG violations;
  Escape closes it.
- `scripts/dev ci` and the e2e suite pass.

## Test plan

- Integration (`crates/yulius/tests/task_prompt.rs`, new module of the `yulius` test binary):
  OPR-001 and OPR-002 cases above, red before the routes exist (404 from the router).
- `ui_contract.rs` already checks every literal API path the UI calls against the router.
- e2e (`e2e/tests/original-prompt.e2e.ts`, mock mode): AC-OPR-3; the mock serves both endpoints.

## Decisions

- A menu item, not a permanent button in the step strip: the request points at the task menu,
  which opens from both the sidebar and the task view, and the strip is kept compact
  (SPEC-029). The label keeps the user's words, "See the original prompt".
- Its own endpoint, read when the dialog opens, rather than a `prompt` field on `TaskDetail`:
  the prompt can be 100,000 characters and the detail is read on every task update.
- The attachments are served by index from the parsed prompt, never by a path from the client,
  and only from the sandbox's `attachments` folder: a prompt can mention any path on the host.
  Read access suffices: the files were given to the agent, whose terminal readers already see.
  See [ADR-0032](../../docs/decisions/0032-original-prompt-on-demand.md).
- A file that is gone (drafts are removed after a week) is shown by name, not hidden: the person
  sees what was attached.
