---
yulius-task: 0ba3ef9e-9abf-4fcc-a68e-b5c4e22dab69
status: in-progress
---

# SPEC-306: A queued file refused after the start keeps its notice until dismissed

Status: in-progress
Date: 2026-10-25
Owner: Yulius task #292
Scope authorization: issue [#559](https://github.com/Quartio-Studio/yulius/issues/559) (found in
review round 3 of task #158, SPEC-168). Requirements amended:
[SPEC-168](SPEC-168-for-images-here-we-want-movable-but-also.md) QMF-003 and
[SPEC-056](SPEC-056-fix-all-open-actionable-issues-from-gith.md) WUI-106.

## Goal

A file pasted into an otherwise empty Queued messages box while the task starts, and refused when
its upload ends, keeps the card and its "The task has started: the file was not added." notice on
the task's later updates, until the person dismisses the card.

## Context

- Symptom: the card is kept after the start by `kept` in `renderQueue()`
  (`crates/yulius-server/web/assets/taskview.js`), which counts the box's text, a refusal in
  `queueError` and an upload still pending (`.pi-pending`). When the refused upload ends,
  `attachPathDrop` (`filedrop.js`) writes the notice into its floating upload status
  (`status.error(unwritable)`) and cancels the placeholder. `kept` turns false, and the next
  update (`starting` → `running`, seconds later) hides the card with the notice.
- Root cause: the refusal lives in the upload status, which `kept` does not see, instead of in
  `queueError`, which `kept` and Dismiss already handle.
- Why the tests missed it: the e2e test "a file pasted into an empty box keeps the card when the
  task starts during its upload" checks the notice right after the release and never sends a later
  update.
- Also: the upload status then ended "Added alone.png" next to the notice saying it was not added.

## Requirements

- QFN-001: an upload refused because the queued messages box no longer takes files (the task
  started) says "The task has started: the file was not added." in the card's error line
  (`queueError`, `role="alert"`), not in the upload status.
- QFN-002: that refusal keeps the card read-only with Dismiss on every later update while the task
  is starting or running (WUI-106); Dismiss clears it and removes the card. A task that ends or is
  archived still takes the card away.
- QFN-003: the upload status of a refused batch says "Nothing was added." rather than "Added …".
- QFN-004: the new-task composer's own refusal ("The prompt is being read; the files were not
  added.") stays in its upload status, which now ends "Nothing was added." too (QFN-003).

## Plan

- [x] Spec SPEC-306 with root cause and plan
- [x] Regression e2e step: notice survives the running update, Dismiss removes the card
- [x] Refusal written into the card's error line (onRefused)
- [ ] Docs, gates and security scan green
- [ ] Independent review answered
- [ ] Merged into dev on green CI with closing summary

## Acceptance criteria

- AC-1 (e2e): in "a file pasted into an empty box keeps the card when the task starts during its
  upload", after the release the notice is in the card's error line; after
  `touchProduct7(browser, { status: 'running', started: true })` it is still visible; Dismiss then
  removes the card. Fails before the fix (the card is hidden by the update).
- AC-2 (e2e): "an upload that ends after the task started says so in the kept card" finds the
  notice in the card's error line, and the upload status says "Nothing was added.".
- AC-3: the rest of `queued-messages.e2e.ts` and the full e2e suite pass.

## Test plan

- `e2e/tests/queued-messages.e2e.ts`: the two tests above, using the mock's `holdUpload` and
  `touchTask` hooks.
- The full e2e suite (`cd e2e && npx e2e run`), the repository gate (`scripts/dev ci`) and
  `scripts/dev security`.

## Decisions

- The refusal goes into `queueError` through an `onRefused(message)` option of `attachPromptDrop`
  / `attachPathDrop` (the verifier's option b), so the existing `kept` check and Dismiss apply
  unchanged. Rejected: counting a visible upload-status error in `kept` (option a): it would also
  keep the card for unrelated upload errors from before the start (a file over the limit), Dismiss
  would have to hide the status too, and the status's own close button would leave the card kept
  without a reason shown.
- With `onRefused`, the refusal is not also written into the upload status: the same sentence
  twice is noise, and the card's error line is where WUI-106 puts refusals.
- The card writes the refusal only while it is shown and the view alive; a hidden card (the task
  ended or was archived) must not come back later with a stale refusal.
- No ADR: a local UI wiring choice inside an existing requirement, recorded here.
