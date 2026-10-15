---
yulius-task: 55638814-bdce-4d3e-bdeb-06fb1f2762bb
status: in-progress
---

# SPEC-166: the Editor never shows a file twice, and a moved image placeholder is never shown as selected text

Status: in-progress
Date: 2026-10-15
Owner: task #156
Scope authorization: the person's request (task #156): "sometimes the editor breaks badly. Fix. In
the same scope fix very small bug: when I move image placeholder […] the text get's selected which
is weird. Fix." The request came with two screenshots of the Editor and one of an `Image 2`
placeholder. Fixes #534 and #535.

## Goal

Opening files never leaves the Editor with a second editor or a second tab of the same file. A
placeholder dragged or pressed in the new-task prompt looks selected as a whole, and its label is
never painted as selected text.

## Context

- **Symptom 1 (screenshots).** The Editor shows two Monaco editors stacked in one host. The first
  fills the stage with an empty line 1, and the second, with the open file (`1 import {`), is pushed
  below it, so the file shows only at the bottom edge. Three files had been opened in quick
  succession.
- **Root cause 1, already fixed.** This is #394: a file opened while Monaco loaded created an editor
  from the half-loaded `window.monaco`, and the next opening created a second one in the same host.
  #523 (SPEC-148, merged 2026-10-15 16:02 +0200) fixed it with one shared creation (`ensureEditor`)
  and a loader that waits for all of `editor.main`. The screenshots were taken at 15:15 +0200, on
  the code from before that merge. With the code from before #523, the e2e probe (the services
  chunk held back, `README.md` clicked, then `AGENTS.md` twice) reproduces the screenshot exactly: 2
  `.monaco-editor` in `.ed-host`, the second one's line 1 at the bottom of the stage. With current
  `dev` it makes one editor.
- **Root cause 2, still open (#534).** The same probe on `dev` still breaks: `AGENTS.md` gets **two
  tabs**. `openFile(path)` looks for an open tab, then awaits the editor and the file's content
  before it adds its tab. A second open of the path during that wait finds no tab either. One
  double-click on a tree row is enough: the tabs become `README.md | AGENTS.md | AGENTS.md`, also
  long after Monaco has loaded. The two tabs hold two models of one file: saving one makes the
  other's save conflict, and the strip, keyed by path, reorders and focuses the wrong tab.
- **Why the tests missed it.** Every editor test opens a file with one click and waits for its
  tab. The #394 test clicks two different files.
- **Symptom 2 (#535).** In the new-task prompt, a press on a placeholder selects it, so that Delete,
  cut or a drag takes it. A drop selects the moved placeholder at its new place. The selection is
  a text range over the atom (`user-select: all`), so the browser paints its text highlight over
  the label "Image 1". A real mouse drag in Chromium shows it both while the placeholder is pressed
  and after the drop (screenshots in the e2e probe). The e2e tests drag with synthetic events, which
  never press, and they check only the text.
- **What else the causes could break.** Any caller of `openFile` (the tree, a terminal's file link
  through `openAt`, a dropped text file) could add the duplicate tab. The fix is in `openFile`, so
  it covers them all. The placeholder's look applies wherever a selection covers it: Shift+arrow,
  a drag across text and files, or select-all.

## Requirements

| ID | Requirement |
| --- | --- |
| EDF-001 | Opening a path that is being opened (its content or the editor still loading) waits for that same open. A file has at most one tab, whichever way it was asked for and however fast. An open that fails (unreadable, binary or too large) leaves nothing behind, so the next open tries again. |
| EDF-002 | A placeholder in the prompt's selection is marked selected as a whole (the accent border and soft accent fill). Its label and icon are never painted with the text selection highlight. Delete, cut and a drag still take a selected placeholder, and text in the same selection is highlighted as before. When the prompt loses the keyboard, no placeholder stays marked. |

## Plan

- [x] Reproduce the defect and find its root cause
- [x] Spec SPEC-166 written and validated
- [ ] A file opened again while it loads gets one tab (#534)
- [ ] A pressed or dropped image placeholder shows selected as a chip, its label never highlighted (#535)
- [ ] Docs, changes, independent review and gates green
- [ ] Merged into dev on green CI

## Acceptance criteria

- In the mock Editor, a double-click on `AGENTS.md` after the editor shows a file gives one
  `AGENTS.md` tab, selected.
- With Monaco's services chunk held back, `AGENTS.md` clicked twice during the load gives one
  `AGENTS.md` tab and one `.monaco-editor`.
- In the new-task prompt, a real mouse press on an `Image 1` placeholder marks it selected (accent
  border), and dragging it to the start of the text moves it there and leaves it marked. Its label
  has no text-selection paint (an author `::selection` rule makes it transparent). Delete then takes
  it out of the text. A click in the text clears the mark.

## Test plan

- `e2e/tests/editor.e2e.ts`: "a file clicked twice before its tab shows opens one tab", a
  double-click after the load and a second click during Monaco's held-back load. It fails before
  the fix with two tabs.
- `e2e/tests/inline-images.e2e.ts`: "a placeholder pressed or dragged with the mouse looks selected
  as a whole, its label never as selected text", a real mouse press, drag and drop (Chromium's own
  drag and drop through the harness's mouse), then Delete and a click in the text. It fails before
  the fix: there is no selected mark, and the label has no `::selection` rule.
- The whole e2e suite and `scripts/dev ci`.

## Decisions

- **Share the open, do not block the click.** A map of path to the open under way in
  `editorview.js` makes every caller await the same promise, as `ensureEditor` does for the editor.
  It is preferred over disabling the tree row while it loads, which would also swallow a click on
  a slow server, and over removing duplicate tabs afterwards, which would build a model for nothing
  and flash a second tab.
- **Mark the chip, keep the selection.** The placeholder stays in the DOM selection, because
  Delete, cut, copy and drag all read the selection. Only its painting changes: an author
  `::selection` makes the label's highlight transparent, and a `pi-selected` class drawn from
  `selectionchange` (listened to only while the prompt has the keyboard) shows the chip as chosen.
  Not selecting it after a drop was rejected: Delete or cut right after a move would then miss it,
  and selecting the moved piece matches a moved text selection (INL-003). `user-select: none` on
  the label was rejected too: it changes what a copy of the selection carries.
- No ADR: both are local fixes inside existing decisions (ADR-0101 for the prompt, SPEC-127 for
  the Editor).
