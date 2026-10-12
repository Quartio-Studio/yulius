---
yulius-task: c0686e04-925d-49b6-8be7-ddd2c564adbf
status: done
---

# SPEC-026: the new-task composer keeps its draft

Status: implemented
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
| DRF-002 | An upload that finishes after the person left the composer still joins that sandbox's draft and shows in the composer open then, removable. | e2e `drafts.e2e.ts` (held upload) |
| DRF-003 | The mode, workflow, "run after" task and start rule come back with the draft. | e2e `drafts.e2e.ts` |
| DRF-004 | The draft survives a reload of the browser tab: it is kept in `localStorage` per signed-in user and sandbox (text, choices, each attachment's path, name, type, upload time and, for an image, a small thumbnail). | e2e `drafts.e2e.ts` |
| DRF-005 | Starting the task (also a split into several) clears the draft; an empty prompt with no attachments stores nothing. A task still being created is not started again from a composer opened meanwhile. | e2e `drafts.e2e.ts` |
| DRF-006 | Signing out removes every stored draft from the browser, also one held by another tab of it; an upload ending after sign-out does not store one again. | e2e `drafts.e2e.ts` (sign-out, other tab); code review (`attachToDraft` ignores a forgotten draft) |
| DRF-007 | Bounded and safe: attachments older than the server's draft age (7 days) are dropped when the draft is read, and only `data:image/` thumbnails are kept; at most 12 thumbnails are stored, each scaled to at most 160 px; when storage is full the draft is stored without thumbnails, and if that fails too it lives only in memory (no error); the draft is written only when it changed. | e2e `drafts.e2e.ts` (expiry, unsafe thumbnail); code review (quota) |

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
- [x] The composer's text and attachments come back after visiting another page
- [x] Choices (mode, workflow, run after, rule) come back with the draft
- [x] The draft survives a reload and is cleared on start and sign-out
- [x] Independent review in a fresh session, findings fixed test-first
- [x] Every gate green, docs and the changes log updated

## Design

A new module `assets/drafts.js` owns the drafts of this browser tab: a `Map` of storage key
(`yulius.draft.<user id>.<sandbox id>`) to a draft object `{ text, mode, workflowId, afterTaskId,
startRule, attached: [{path, name, type, at, thumb?, file?}], starting }`.
`draftFor(userId, sandboxId)` returns the live object (read from `localStorage` the first time),
`saveDraft` writes it back (debounced; at once when the composer closes, and on `pagehide` or a
hidden page), `attachToDraft` adds uploaded files and makes image thumbnails, `onDraftChange` tells
the open composer about attachments added or a draft cleared elsewhere, `clearDraft` empties it
and removes the key, and `forgetDrafts()` removes every key at sign-out and tells the browser's
other tabs (`yulius.draftsForgotten`, a `storage` event) to drop theirs from memory. The composer
reads its initial state from the draft and writes every change into it; the `File` stays on the
in-memory item, so thumbnails are full images while the tab lives and stored thumbnails after a
reload.

## Test plan

Browser e2e in mock mode (`e2e/tests/drafts.e2e.ts`): AC-DRF-1 to AC-DRF-5, an upload held by
the mock until after leaving and returning, a creation held until a second start was refused, a
split, and a stored draft with an expired attachment and an unsafe thumbnail. The existing
`filedrop.e2e.ts`, `new-task.e2e.ts` and `intake.e2e.ts` composer tests must keep passing. Not
covered by a test: a full `localStorage` (DRF-007 by code review), Safari's thumbnail encoding.

## Decisions

- Browser-side, not a server draft API: the request is about one person's unfinished input in their
  browser; uploads already live on the server, so only small metadata is stored locally. A server
  draft would add schema, endpoints and cross-device semantics nobody asked for. (ADR 0021)
- `localStorage`, not `sessionStorage`: a closed tab or a crashed browser also lost work; the
  existing preference keys use `localStorage`, and sign-out clears drafts like notifications.
- Thumbnails are stored scaled down instead of the original bytes, so a few images stay well inside
  the storage quota; IndexedDB for blobs was rejected as more machinery than a preview needs.
- Several browser tabs on the same sandbox: each keeps its own in-memory draft; the last write wins
  in storage.
- Review round 1: the composer redraws from draft change events (not the closure of the composer
  that received the drop); a draft being started refuses a second start rather than hiding the
  draft, because the start may still fail and the draft must then come back; text typed in a
  composer reopened during that start is cleared with the draft once the task opens (the person is
  taken to the task). Text-only drafts do not expire; they are removed at start or sign-out.
- Review round 2: a sign-out in another tab makes this tab forget its drafts in memory (a marker
  key's `storage` event), so a later navigation or reload does not store them again. Accepted
  limits: an upload that ends after its task was created joins the (now empty) draft, so the file
  is kept for the next task rather than lost; files dropped in a composer reopened during a start
  are cleared with the draft like its text; a restored "run after" reads "Start now" for the moment
  until the sandbox's task list arrives (the choice is still sent).
- Review round 3 approved. Accepted without a fourth round: if another tab's 300 ms save timer fires
  in the milliseconds between a sign-out's key removal and its marker reaching that tab, that
  draft's key stays stored until the next sign-out or start.
- Merged with #20 (SPEC-022): the start rule is the editor's `{ join, conditions }` object; the
  draft stores it as such and keeps a stored one only when well formed (states done, merged or
  ended, task numbers, at most 16 conditions). This ADR became 0021 (0019 is the rule editor's, 0020 the Cockpit's).

## Evidence

- `npx e2e run tests/drafts.e2e.ts`: 11 of 11 pass (after merging #20, a start rule kept across a reload). Each behaviour test failed before its change
  (prompt empty after returning; choices back to Auto; nothing after a reload; a stored key after
  sign-out; a held upload not shown; no refusal of a second start; another tab's sign-out ignored).
- `npx e2e run` (full suite): 32 files passed, 1 skipped (`agent.e2e.ts`, needs a model);
  140 tests passed.
- `scripts/dev ci`: exit 0; nextest 1149 tests passed, 16 skipped; repository contracts passed
  (after syncing the `modulepreload` order with `check_repository.py --sync-brand`).
- Independent review: round 1 requested changes (1 major, 5 minor), round 2 approved with minors,
  round 3 approved head `022c1ea`.
- Not tested here: Safari's thumbnail encoding, a full `localStorage` quota, two real browser
  tabs (simulated with a `storage` event).
