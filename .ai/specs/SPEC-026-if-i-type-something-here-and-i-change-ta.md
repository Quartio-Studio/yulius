---
yulius-task: c0686e04-925d-49b6-8be7-ddd2c564adbf
status: in-progress
---

# SPEC-026: the new-task composer keeps its draft

Status: in progress
Date: 2026-10-12
Owner: Yulius task #24
Scope authorization: user request: "If I type something here and I change tab, I want it to be
saved, including attachements. Many times I changed a section to check something and then my task
got reset." (screenshot: the new-task composer with "If I type" in the prompt).

## Goal

What a person puts in the new-task composer (the prompt text, the attached files and the choices
on its edge) is still there when they come back to it: after opening another page of Yulius, after
switching sandbox tab, and after a reload of the browser tab. It is cleared only when the task
starts (or the person clears it).

## Context

- The composer (`mountComposer`, `crates/yulius-server/web/assets/taskview.js`) keeps the prompt in
  its textarea and the attachments in a local `attached` array. `setCenter` (`app.js`) destroys the
  view on every navigation and builds a new, empty one on return, so everything is lost.
- Attachments (SPEC-014, SPEC-006) are uploaded at once to
  `sandboxes/<sid>/attachments/drafts/<user>/` and removed by the server after a week
  (`DRAFT_MAX_AGE`, `uploads.rs`); the browser holds `{path, name, file}` and shows images from the
  `File` as thumbnails. Their paths join the prompt when the task starts (`withAttachments`).
- Model and effort are already remembered per browser (`MODEL_PREF_KEY`, `EFFORT_PREF_KEY`).
- Sign-out removes signed-in data from `localStorage` (`app.js` `signOut`, OWASP ASVS V8.2.3).
- Parallel work in the same file: #20 (start-rule editor), #23 (clickable attachments).

## Requirements

| ID | Requirement | Evidence |
| --- | --- | --- |
| DRF-001 | Leaving the composer for any other page and coming back shows the same prompt text and attachments (images as thumbnails), per sandbox. | e2e `drafts.e2e.ts` |
| DRF-002 | An upload that finishes after the person left the composer still joins that sandbox's draft. | code review: `onAttach` writes to the draft, not the view |
| DRF-003 | The mode, workflow, "run after" task and start rule come back with the draft. | e2e `drafts.e2e.ts` |
| DRF-004 | The draft survives a reload of the browser tab: it is kept in `localStorage` per signed-in user and sandbox (text, choices, each attachment's path, name, type, upload time and, for an image, a small thumbnail). | e2e `drafts.e2e.ts` |
| DRF-005 | Starting the task (also a split into several) clears the draft; an empty prompt with no attachments stores nothing. | e2e `drafts.e2e.ts` |
| DRF-006 | Signing out removes every stored draft from the browser. | e2e `drafts.e2e.ts` |
| DRF-007 | Bounded: attachments older than the server's draft age (7 days) are dropped when the draft is read; at most 12 thumbnails are stored, each scaled to at most 160 px; when storage is full the draft is stored without thumbnails, and if that fails too it lives only in memory (no error). | code review, `drafts.js` |

## Acceptance criteria

- AC-DRF-1: type a prompt, attach an image and a PDF, pick Goal, open Tasks, press New task: the
  prompt, both chips (one thumbnail) and Goal are there.
- AC-DRF-2: the same after a reload of the page.
- AC-DRF-3: start the task: its prompt carries the attachments; back on the composer it is empty,
  also after a reload.
- AC-DRF-4: sign out: no `yulius.draft.` key is left in `localStorage`.
- AC-DRF-5: another sandbox's composer has its own draft.

## Plan

- [x] Read the product context and related work, then write and validate the spec
- [ ] The composer's text and attachments come back after visiting another page
- [ ] Choices (mode, workflow, run after, rule) come back with the draft
- [ ] The draft survives a reload and is cleared on start and sign-out
- [ ] Independent review in a fresh session, findings fixed test-first
- [ ] Every gate green, docs and the changes log updated

## Design

A new module `assets/drafts.js` owns the drafts of this browser tab: a `Map` of sandbox id to a
draft object `{ text, mode, workflowId, afterTaskId, startRule, attached: [{path, name, type, at,
thumb?, file?}] }`. `draftFor(ctx, sandboxId)` returns the live object (created from
`localStorage` the first time, key `yulius.draft.<user id>.<sandbox id>`), `saveDraft` writes it
back (debounced; flushed when the composer is destroyed and on `pagehide`), `clearDraft` empties it
and removes the key, `forgetDrafts()` removes every key at sign-out. The composer reads its initial
state from the draft and writes every change into it; the `File` stays on the in-memory item, so
thumbnails are full images while the tab lives and stored thumbnails after a reload.

## Test plan

Browser e2e in mock mode (`e2e/tests/drafts.e2e.ts`): AC-DRF-1 to AC-DRF-5. The existing
`filedrop.e2e.ts`, `new-task.e2e.ts` and `intake.e2e.ts` composer tests must keep passing. Not
covered by a test: an upload finishing after leaving (the mock answers at once; DRF-002 by code
review), a full `localStorage` (DRF-007 by code review).

## Decisions

- Browser-side, not a server draft API: the request is about one person's unfinished input in their
  browser; uploads already live on the server, so only small metadata is stored locally. A server
  draft would add schema, endpoints and cross-device semantics nobody asked for. (ADR 0019)
- `localStorage`, not `sessionStorage`: a closed tab or a crashed browser also lost work; the
  existing preference keys use `localStorage`, and sign-out clears drafts like notifications.
- Thumbnails are stored scaled down instead of the original bytes, so a few images stay well inside
  the storage quota; IndexedDB for blobs was rejected as more machinery than a preview needs.
- Several browser tabs on the same sandbox: each keeps its own in-memory draft; the last write wins
  in storage.
