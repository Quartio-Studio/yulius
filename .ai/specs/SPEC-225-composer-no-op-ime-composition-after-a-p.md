---
yulius-task: 2d1407f3-5f88-4c24-8e22-8bd09244105f
status: done
---

# SPEC-225: A no-op record never turns the paste's undo step into typing

Status: done
Date: 2026-10-18
Owner: task #211
Scope authorization: task #211, "Fix GitHub issue #513". Fixes #513, a follow-up of the SPEC-150
review round 4 (task #144, ADR-0124). Builds on SPEC-124 and SPEC-150 (the prompt's undo history
holds the tray, TRY-002).

## Goal

Undoing past a paste takes its upload out of the tray, also when an IME composition that changed
nothing (cancelled with Esc) ended between the paste and the next keystroke.

## Context

`promptinput.js` `record(kind)` makes the undo history. Three branches:

1. nothing changed: only the current step's selection is updated;
2. typing within `TYPING_MS` of earlier typing: the current step is *replaced* by a new object;
3. otherwise a new step is pushed.

All three then set `history.kind = kind`. A `compositionend` that inserted nothing calls
`record('type')`, takes branch 1, and still sets `history.kind = 'type'`. Right after a paste the
current step is the paste's step P (`kind` was `'edit'`), which `begin()` keeps as `step` so that
`swapPending` attaches the upload from P on. A keystroke within 1 s then takes branch 2 and
replaces P by a new object X. P is gone from the history without being in `discarded` (a cut redo
branch), so when the upload ends `swapPending` finds `indexOf(P) === -1 && !discarded.has(P)`,
reads it as "fell off the history's start" and attaches the upload to every step, including the
steps before the paste. Undoing past the paste then keeps the image in the tray, and starting the
task sends it after the text.

Why the tests missed it: no test dispatched `compositionend`; without it `kind` stays `'edit'`
after the paste, so the next keystroke pushes a step of its own and P stays in the history.

What else the cause can touch: any no-op record (an `input` event that changed nothing) set `kind`
the same way, and any holder of a step object (`begin()`'s `step`) lost it when typing replaced the
step. Both are covered by the fix below.

Also untested (issue #513): `drafts.js` sets a thumbnail on an upload that is out of the tray
(its paste undone), so a redo brings it back with its picture, and the stored draft keeps it.

## Requirements

- **IME-001** A record that changes nothing (text, files and tray as the current step) updates
  only the current step's selection: it leaves `history.kind` and `history.at` as they were, so it
  never makes the next keystroke merge into a step that was not typing.
- **IME-002** When typing merges into the current step, that step keeps its identity (its object
  is updated in place), so a holder of the step (an upload's paste step) still finds it in the
  history.
- **IME-003** Paste with the upload held → empty `compositionend` → typing → the upload ends →
  undo past the paste: the tray is empty and the task starts with the text only (TRY-002).
- **IME-004** A paste undone before its image's thumbnail is made, then redone: the tray tile
  shows the thumbnail after a reload.

## Plan

- [x] Spec SPEC-225 written and validated
- [x] Failing e2e: no-op compositionend after a paste keeps the upload out of undone steps
- [x] record() fix: no-op record leaves history.kind; replacing keeps step identity
- [x] e2e: thumbnail arriving while a paste is undone shows after redo and reload
- [x] Docs, changes entry and gates green
- [x] Independent review answered
- [ ] Merged into dev on green CI

## Acceptance criteria

- The IME-003 e2e test fails on `dev` (a3b9b04a) and passes with the fix.
- The IME-004 e2e test passes (coverage of existing behavior).
- `scripts/dev ci`, the e2e suite and `scripts/dev security` pass.

## Test plan

`e2e/tests/inline-images.e2e.ts`:

- "a composition that changed nothing after a paste does not attach its upload to the steps before
  it" (IME-001/002/003): type "One ", hold the upload, paste, dispatch an empty `compositionend`,
  type "x", release; one undo gives "One [b.png]" with the file (the "x" was a step of its own),
  the next gives "One " with an empty tray; start: the prompt is "One".
- "a thumbnail made while its paste is undone shows after redo and a reload" (IME-004):
  `createImageBitmap` held, paste, upload ends, undo (tray empty), the thumbnail is made, redo
  (tray shows it), the stored draft holds one thumbnail, reload: the tile shows an image.

## Decisions

- Both changes of the issue's acceptance criteria, not one: IME-001 removes the cause (a no-op is
  not typing), IME-002 keeps a step's identity for any other way typing could merge into a step
  someone holds. Each is one line; neither changes what an undo shows.
- A side effect, accepted: Firefox's `input` event after `compositionend` changes nothing; it
  used to set the kind to `insertCompositionText`, so each composition was a step of its own. Now
  compositions less than `TYPING_MS` apart merge into one step, as they already did in Chrome.
  The same holds for undo then a no-op composition: the restored step is no longer typed over.
- No ADR: no lasting design choice; the history's contract stays SPEC-124/SPEC-150's.

## Evidence

- Regression test first: "a composition that changed nothing after a paste…" failed on `dev`
  (a3b9b04a) with `expected ["b.png"] to equal []` (commit b3a69b5b), and passes with the fix.
- Each half proven: with only the in-place merge (old `else if` chain kept) the test fails at the
  first undo (`expected "One " to be "One [b.png]"`); with only the early return it passes. Both
  are kept (Decisions).
- Thumbnail test: passes; with `drafts.js` changed to skip thumbnails of uploads out of the tray it
  fails (the stored draft never holds one).
- `scripts/dev ci`: exit 0 (nextest 2233 passed, 18 skipped).
- `cd e2e && npx e2e run`: 619 passed, 1 skipped (`agent.e2e.ts`, unrelated), on cd38c53a; the
  tightened test passes again afterwards.
- `scripts/dev security`: Vidoc vidoc-1 change scan, no open critical, high or in-change medium
  finding (exit 0); no low findings reported.
- Independent review (fresh session, read-only, cd38c53a): APPROVE with three minor findings and a
  nit, all own code, all answered: exact undo steps in the test (the loop did not prove IME-001),
  a comment on the 1 s timing assumption, the Firefox side effect above, and the ticked plan.
