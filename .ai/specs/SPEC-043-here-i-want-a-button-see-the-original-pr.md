---
yulius-task: fd776a84-5a81-42e8-96af-9d3061ec37c0
status: done
---

# SPEC-043: "See the original prompt" in the task menu

Status: implemented; files inside the text show in place since [SPEC-124](SPEC-124-images-keep-their-position-in-the-text-f.md), the files after it below
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
| OPR-001 | `GET /api/tasks/{tid}/prompt` (read access to the task's sandbox, like `GET /api/tasks/{tid}`; 404 otherwise) returns the task's original prompt: `text` (the prompt without its attachment lines, as the session got it) and `attachments`, in order, each with its file `name`, whether it is an `image` (the extensions Claude Code pastes as images) and whether it is `available` (served by OPR-002; at most 500 are checked, the rest are listed as not available). | `task_prompt.rs` |
| OPR-002 | `GET /api/tasks/{tid}/prompt/attachments/{n}` (same access) answers with the bytes of attachment `n` only when its path names a file directly in one of the task's own attachment folders, as an upload names them: its creator's composer drafts (`attachments/drafts/<creator>/`) or the task's terminal attachments (`attachments/tasks/<task>/`); those folders are real folders, not links; the file is opened without following a link or blocking, is a regular file within the upload limit, and (where `/proc/self/fd` exists) was opened at the expected real path. Anything else (out of range, not a number, gone, another folder, `..`, a link, a folder, too large) is 404. Reads share the uploads' concurrency bound (superseded by SPEC-080 OPR-005..007: their own bound, streamed, the `/proc` check required on Linux). Images are served with their image type; every other file as `application/octet-stream` with `Content-Disposition: attachment`; always `X-Content-Type-Options: nosniff` and a `sandbox` content security policy, so no file runs as a page. | `task_prompt.rs` |
| OPR-003 | The task menu has "See the original prompt", enabled for everyone who sees the task. It opens a dialog titled `Original prompt · #<n>` that shows the text formatted as Markdown (read-only), then the attached files: an image as a thumbnail that opens full size, another file as a download link, a file that is gone by its name with "No longer on the server". A prompt without text says so. "Copy text" copies the text. A failed load says why in the dialog. | e2e `original-prompt.e2e.ts` |
| OPR-004 | `docs/api.md`, SPEC-001 UI-013, an ADR, `docs/architecture.md` (if a component changes) and `docs/changes.md` describe it. | review |

## Plan

- [x] Read the product context and related work, then write and validate the spec
- [x] The API returns a task's original prompt text and its attached files
- [x] The API serves a task's attached files from the attachments folder only
- [x] "See the original prompt" in the task menu shows the text and the attached files
- [x] Independent review in a fresh session, findings fixed test-first
- [x] Every gate green, docs and the changes log updated

## Acceptance criteria

- AC-OPR-1: a task created with "Fix the **login**\n\n@<draft>/shot.png\n@<draft>/notes.txt\n@/etc/hosts"
  returns `text` "Fix the **login**" and three attachments: `shot.png` (image, available),
  `notes.txt` (not an image, available), `hosts` (not available). A read user gets the same; a
  user without access gets 404.
- AC-OPR-2: attachment 0 is served as `image/png` with `nosniff`; attachment 1 as
  `application/octet-stream` with `Content-Disposition: attachment`; attachment 2, index 9, a
  link in the attachments folder pointing outside it and a removed file are 404. The task's
  terminal attachments are served; `..` in the prompt, another user's drafts, a shell's files and
  everything once the attachments folder is replaced by a link are 404 (and not available).
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
  and only from the task's own attachment folders, compared by name, never through a link: a
  prompt can mention any path on the host, and a contained process can plant links in the
  sandbox directory (review round 1: resolving the `attachments` folder followed such a link).
  Read access suffices: the files were given to the agent, whose terminal readers already see.
  See [ADR-0033](../../docs/decisions/0033-original-prompt-on-demand.md).
- A file that is gone (drafts are removed after a week) is shown by name, not hidden: the person
  sees what was attached.

## Evidence

- Red first: `task_prompt.rs` "the original prompt is its text and attached files…" and "a prompt
  without attachments or whose files are gone" failed with 404 (no route) before
  `GET /api/tasks/{tid}/prompt` and pass after; "an attached file is served only from the
  attachments folder" passes with the attachment route (headers, download, 404 cases).
- Review round 1 (fresh read-only session): REQUEST_CHANGES. Blocker: resolving the `attachments`
  folder followed a link a contained process can plant, so any host file named in a prompt could
  be served; it also served other users' drafts and shell files. Red first:
  `only_the_tasks_own_attachment_folders_are_served_never_through_a_link` failed (200 for another
  user's draft) before the fix and passes after; it also replaces `attachments` with a link.
  Also fixed: a non-numeric index is 404, at most 500 attachments checked, reads share the upload
  bound, a 120 s timeout for file reads in the UI, the download link attached before its click.
- Review round 2: APPROVE at 91eac32 (blocker closed). Its minors (thumbnails can hit the
  uploads' bound with more than 8 transfers; the `/proc` check passes when `/proc` is unreadable;
  nits) are filed as #303.
- e2e `original-prompt.e2e.ts`: failed before the UI (no "See the original prompt" menu item)
  and passes after (task view and sidebar menus, formatted text, image preview, download button,
  a file that is gone, a failed load, no WCAG violations).
- `scripts/dev ci` on 91eac32 (with origin/dev merged): exit 0; nextest 1210 run, 1210 passed,
  16 skipped; check, fmt, clippy, doctests, deny, machete, doc green. An earlier run on b6d3147
  failed once in `terminal_ws::two_viewers_share_the_terminal_and_only_writers_type` (unrelated,
  under load from a parallel e2e run; 20/20 passes alone): filed as #301.
- e2e full suite on 91eac32: 161 passed, 1 skipped (the live-model test).
- Not covered: a real browser against a deployed instance with a real upload (the e2e suite
  runs the mock backend); the `/proc` fallback off Linux.
