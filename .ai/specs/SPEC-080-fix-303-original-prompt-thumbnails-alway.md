---
yulius-task: 847009d4-0b68-41d3-8e9e-36512996e66a
status: in-progress
---

# SPEC-080: the original prompt's thumbnails always load; the `/proc` check refuses on Linux

Status: in progress
Date: 2026-10-13
Owner: Yulius task #63 (part of task #47, "fix all open actionable GitHub issues")
Scope authorization: task request: fix GitHub issue #303 following its acceptance criteria; no
new issues.

## Goal

The "See the original prompt" dialog (SPEC-043, ADR-0033) shows every attached image, even with
many images or while uploads run; serving one attachment holds a bounded amount of memory; and on
Linux an attached file whose opened path cannot be confirmed is refused.

## Context

Review round 2 of SPEC-043 left these as #303:

- `App::task_prompt_attachment` takes a slot of the uploads' bound (`upload_slot`, 8 at once,
  fails at once when full) and reads the file whole into memory. The dialog loads every thumbnail
  at the same time, so a prompt with more than 8 images, or one opened while uploads run, gets
  429 for some thumbnails, which then say "Couldn't load it" for good. The slot is released
  before the bytes are sent, so it does not bound memory either.
- `open_attached` serves the file when `/proc/self/fd/N` cannot be read. On Linux, where the
  containment boundary exists, that silently skips the check that catches a folder swapped for a
  link between the checks and the open.
- Nits: blocking `std::fs` calls in the async fns of `task_prompt.rs`; the mock serves any file
  under its attachments folder instead of the task's own folders; a prompt starting with `---`
  renders as front matter (`parseMarkdown` takes it as properties).

## Requirements

| ID | Requirement | Evidence |
| --- | --- | --- |
| OPR-005 | Reading a prompt's attachment does not take an upload slot. Reads have their own bound (`MAX_CONCURRENT_ATTACHMENT_READS`, 16) and a request waits for a read slot up to `ATTACHMENT_READ_WAIT` (30 s) before it fails with 429 `capacity`. A prompt with 12 images is served whole while every upload slot is taken. | `task_prompt.rs` (integration) |
| OPR-006 | The attachment is streamed in chunks of at most `ATTACHMENT_CHUNK` (64 KiB), with its `Content-Length`, exactly the length checked when it was opened (a file that shrank breaks the response off), while the read slot is held. A response nobody reads for `ATTACHMENT_IDLE` (60 s) gives its slot back and closes its file; it fails at the client's next read. | unit and integration tests |
| OPR-007 | On Linux, a file whose `/proc/self/fd` link cannot be read is refused (404). Other platforms keep serving it, as ADR-0033 records. | unit test with an injected reader |
| OPR-008 | `task_prompt.rs` does its file system work off the async runtime (`spawn_blocking`). | review |
| OPR-009 | The mock serves a prompt's file only from the task's own attachment folders (its creator's drafts, its terminal attachments), named directly, without `..`. | e2e `original-prompt.e2e.ts` |
| OPR-010 | A prompt is shown as written: in the dialog and the composer's preview, a prompt starting with `---` is not taken as front matter. | e2e `original-prompt.e2e.ts` |

## Plan

- [x] Read #303, SPEC-043 and ADR-0033, write and validate the spec
- [ ] Attachment reads have their own bound with a bounded wait and stream under their slot
- [ ] On Linux an attachment whose /proc link cannot be read is refused, file work off the runtime
- [ ] The mock serves only the task's own attachment folders and a prompt is never front matter
- [ ] Docs, ADR and changes describe the new bound
- [ ] Independent review in a fresh session, findings fixed test-first
- [ ] Every gate green with dev merged

## Acceptance criteria

- AC-1: with all `MAX_CONCURRENT_UPLOADS` upload slots held, a prompt with 12 attached images
  answers 200 with each image's bytes for all 12, requested at the same time and held unread.
- AC-2: with every read slot held, one more read waits and is served as soon as a slot frees;
  a read that waits longer than its bound fails with `capacity` (unit test, short wait).
- AC-3: a file of several chunks is served with its `Content-Length` and in body frames of at
  most 64 KiB; a response left unread past the idle bound releases its slot.
- AC-4: on Linux, `open_attached` with a reader that fails returns nothing; with a reader that
  gives the expected path it returns the file, with another path nothing.
- AC-5: in the mock, a prompt file in another user's drafts or reached with `..` is "No longer on
  the server"; a prompt `---\ntitle: x\n---\nBody` shows `title: x` as text between two dividers,
  not as properties.
- `scripts/dev ci` and the e2e suite pass.

## Test plan

- Integration (`crates/yulius/tests/task_prompt.rs`): AC-1 (uploads held with
  `begin_file_upload`, 12 responses held unread together), AC-3 frames and `Content-Length`.
- Unit (`crates/yulius-app/src/task_prompt.rs`): AC-2 with a short wait, AC-3 idle release with a
  short idle bound (waits on the semaphore, no sleeps), AC-4 with an injected reader.
- e2e (`e2e/tests/original-prompt.e2e.ts`, mock mode): AC-5.

## Decisions

- Its own bound with a short wait, not a shared one and not one image after another in the
  dialog: reads and uploads stop starving each other, the dialog stays parallel (the browser
  limits connections per host anyway), and waiting a little absorbs a burst of thumbnails that
  failing at once turned into broken images. With 64 KiB chunks a response holds about what
  hyper buffers ahead of the client (about 400 KiB): about 7 MiB for 16, against 8 × 25 MiB.
- The slot is held while the body is sent, so the bound counts open files and responses in
  flight. A client that stops reading would hold it for good; an idle bound (as uploads have)
  gives it back and closes the file.
- `/proc/self/fd` is required on Linux only: it is where containment exists and where `/proc`
  is always mounted for Yulius; elsewhere there is no such link, and ADR-0033 already accepts the
  race there. The reader is injected so the refusal is tested without breaking `/proc`.
- The prompt is never front matter: a prompt is a message, not a document with properties; the
  editor gets a `frontmatter: false` option rather than a separate renderer.
- See [ADR-0064](../../docs/decisions/0064-attachment-reads-have-their-own-bound.md).

## Evidence

- Red first: `task_prompt::twelve_images_are_served_together_while_every_upload_slot_is_taken`
  failed (429 for image 0 with the 8 upload slots held) and
  `task_prompt::an_attachment_is_streamed_in_bounded_chunks_with_its_length` failed (one frame of
  1 MiB) before the fix; both pass after.
- Red first: `task_prompt::tests::on_linux_a_file_whose_opened_path_cannot_be_read_is_refused`
  fails with the old fallback (serve when `/proc/self/fd` is unreadable) and passes after.
  `a_read_waits_for_a_slot_within_its_bound` and
  `a_file_is_sent_in_chunks_and_gives_its_slot_back_when_left_unread` (waits on the semaphore,
  no sleeps) pass.
- Red first: e2e `original-prompt.e2e.ts` "a prompt starting with --- is text…" fails with the old
  web assets (front matter shown as properties) and with the old mock alone (another user's
  draft and `..` served, the task's terminal folder not); passes after.
- Review round 1 (fresh read-only session): APPROVE at 1ec6bb4, no blockers or majors. Minors
  fixed: the idle watchdog now also closes the file (test asserts it) and is aborted when the
  file is dropped; the memory and "breaks off" wording corrected (hyper buffers about 400 KiB
  per response); `a_read_waits_for_a_slot_within_its_bound` now asserts the read is still waiting
  before the slot frees (red with a non-waiting `try_acquire_owned`); the e2e adds an owned-folder
  path that is `..` itself (red without the mock's component check). Nits: AC-5 wording, one
  shared `touchTask` e2e helper.
- `scripts/dev ci` (dev merged, up to date at 34815b5): exit 0; nextest 1433 run, 1433 passed,
  16 skipped; check, fmt, clippy, doctests, deny, machete, doc green.
- e2e full suite: 271 passed, 1 skipped, 1 failed: `archive.e2e.ts` "the keyboard stays on its
  control when the sidebar list is rebuilt", which this change does not touch; alone it passed
  4 of 5 runs. Already tracked by open #356 (flaky under load: archive.e2e.ts Stopped-toggle
  focus).
- Not covered: a real browser against a deployed instance; the `/proc` fallback off Linux (the
  unit test checks the policy constant per platform).
