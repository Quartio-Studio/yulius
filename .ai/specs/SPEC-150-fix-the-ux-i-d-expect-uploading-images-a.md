---
yulius-task: e63b6ca6-282b-4ea9-a514-3af642561f54
status: in-progress
---

# SPEC-150: Uploaded files sit in a tray; the text holds small placeholders; images zoom

Status: in-progress
Date: 2026-10-15
Owner: task #144
Scope authorization: the person's request (task #144), with a screenshot of the new-task composer
where a pasted screenshot sits in the text as a 160 × 64 px thumbnail beside one line of words:
"fix the UX, i'd expect uploading images and then positioning placeholders. Moreover i still want
to zoom the image". Builds on SPEC-124 (images keep their place in the text), which removed the
tray of thumbnails of SPEC-014/SPEC-025.

## Goal

Uploading a file and saying where it belongs are two separate things again. Uploaded files show in
a tray of thumbnails above the text, where a click zooms an image and × removes the file. In the
text each file is a small placeholder (`Image 1`, a file's name) that the person positions: it
lands at the caret on paste, drops where a tray thumbnail is dragged, and moves by drag or by cut
and paste. The agent still gets each file at its placeholder's place (SPEC-124).

## Context

- **Composer today** (`taskview.js` `mountComposer`, `promptinput.js`, SPEC-124, ADR-0101): a
  `contenteditable` prompt whose files are inline atoms. An image atom is a 64 px high thumbnail,
  so one line of text grows to the image's height (the screenshot). There is no tray; the only
  way to see an image large is a double-click on its atom (undiscoverable) or the Preview.
  Deleting an atom deletes the file from the draft.
- **Prompt format** (ADR-0101): one string; a file in the text is the mention `@'<path>'`, a line
  that is only a mention is a file at that line; files after all the text are how older prompts
  read. `withAttachments(text, files)` appends files as `@path` lines. The server, the request file,
  the first message and the original prompt view already handle both forms; nothing on the server
  changes here.
- **Zoom** (`filedrop.js` `openAttachment`, SPEC-025): a dialog with the image fitted to 75vh;
  there is no way to see it at its own size.
- **Drafts** (SPEC-026, ADR-0021): `draft.text` (the prompt with mentions) and `draft.attached`
  (the files). Today `attached` is "the files the text holds".

## Requirements

| ID | Requirement | Evidence |
| --- | --- | --- |
| TRY-001 | Every file uploaded to the composer (paste, drop) shows in a tray above the text, labelled "Attached files", in upload order: an image as a thumbnail with its number, another file as a tile with its name. The tray is hidden while empty. | e2e `inline-images` |
| TRY-002 | A click (or Enter/Space) on an image's thumbnail opens it large; another file opens as before (SPEC-025: a tab as plain text, PDF/media as it is, else a download). × on a tile removes the file from the tray and its placeholders from the text, one undo step; undo brings both back. | e2e `inline-images` |
| TRY-003 | In the text an image is a placeholder one line high, `Image N` (N its number in the tray) with a small image icon; another file is its name, as before. The placeholder's tooltip is the file's name; a double-click opens the file. Typing beside a placeholder keeps the line's height. | e2e `inline-images` |
| TRY-004 | A paste or drop still puts the placeholder at the caret (the drop point). Placeholders move as in SPEC-124 (drag, ⇧+arrow then cut and paste). Dragging a tray thumbnail into the text puts that file's placeholder at the drop point: it moves there when the text already holds it, else it is inserted; one undo step. The keyboard way for a tray file is its "Put at the caret" button. | e2e `inline-images` |
| TRY-005 | Deleting a placeholder keeps the file in the tray. A file in the tray with no placeholder reaches the task after the text (an `@path` line, as older prompts), so the agent still gets it. | e2e `inline-images` |
| TRY-006 | The draft keeps the tray (`attached`, every uploaded file) and the text with its placeholders' mentions; a reload restores both. An older draft's files show in the tray (no longer appended to the text). An upload ending after the person left joins the tray. | e2e `drafts`, `inline-images` |
| TRY-007 | The zoom dialog fits the image to the window; a click on it (or Enter/Space, a toggle button named "Actual size") shows it at its own size, scrollable, and again fits it. | e2e `filedrop` |
| TRY-008 | The composer stays usable at phone width: tray tiles wrap, no horizontal page scroll. No axe violations. | e2e `inline-images` |

## Plan

- [x] Read the product context and related work, then write and validate the spec
- [ ] Uploaded files show in a tray above the text, where a click zooms an image and × removes the file
- [ ] Images sit in the text as one-line numbered placeholders, placed from the tray by drag or the keyboard
- [ ] The zoom dialog shows an image fitted or at its own size
- [ ] ADR, architecture, product, earlier specs and the changes log describe the tray and placeholders
- [ ] Independent review in a fresh session, findings fixed test-first
- [ ] Every gate green and the change delivered to dev

## Design

- `promptinput.js`: `face(path)` may return a `label`; an image atom draws as
  `<span class="pi-atom pi-image" title=name>` with the image icon and the label (no `<img>`).
  New API: `place(path, at?)` (move the file's first atom to `at`, or insert one; `at` defaults to
  the caret kept at the last blur) and `remove(path)` (take out every atom of the file, one step).
  The drop handler accepts `PLACE_TYPE` (`application/x-yulius-file`), carrying a path, and calls
  `place` at the drop point. `setValue` loses its `appended` argument: the tray holds such files.
- `taskview.js`: a `composer-attachments` list rendered from `draft.attached`. `keepAttached` keeps
  the tray in order and adds any file the text holds that the tray lacks (undo of a removal).
  Starting sends `withAttachments(prompt.value, files without a placeholder)`.
- `filedrop.js` `openAttachment`: the image sits in a toggle button that switches the dialog
  between fitted and actual size.

## Acceptance criteria

- Pasting a screenshot mid-text shows one thumbnail in the tray and `Image 1` in the text, on a
  line as high as the text; clicking the thumbnail opens the dialog; "Actual size" shows it at its
  own size.
- Dragging the tray thumbnail elsewhere in the text moves `Image 1` there; the started task's
  prompt has the mention at that place.
- Deleting the placeholder and starting sends the file after the text; × removes it entirely, and
  undo brings it back.
- `scripts/dev ci` and the e2e suite pass.

## Test plan

- e2e `inline-images.e2e.ts`: tray shows on paste with thumbnail and number; placeholder is
  `Image 1`, one line high; tray click opens zoom; tray drag moves the placeholder (one undo
  step); "Put at the caret" inserts; × removes file and placeholder, undo restores; deleted
  placeholder → file after text in the started prompt; older draft → tray; phone width.
- e2e `filedrop.e2e.ts`: zoom toggle between fitted and actual size.
- e2e `drafts.e2e.ts`: names read from the tray; late upload joins the tray.
- No Rust change: the server already parses both mention forms (SPEC-124 tests).

## Decisions

- **Paste still places the placeholder at the caret** over "upload to the tray only, place later":
  the place where the person pasted is usually the place meant (SPEC-124's reason), and moving a
  small placeholder is cheap. The tray adds the "uploaded" view without losing that.
- **A placeholder is a label, not a thumbnail**: the screenshot's problem is a 64 px image inside
  a line of text. `Image N` matches Claude Code's own `[Image #N]`, and the tray carries the
  picture.
- **Removing a placeholder keeps the file**; the tray's × removes it. A file without a place still
  goes to the agent, after the text, as every older prompt did. This reverses SPEC-124 INL-005's
  "a file whose mention a person deletes leaves the draft".
- **Dragging a tray thumbnail moves the file's placeholder** when it has one, so there is one
  gesture for "put this image here"; a second placeholder for the same file still comes from copy
  and paste.
- **Zoom is a fit / actual-size toggle** in the existing dialog, not a zoom slider: it answers "I
  still want to zoom the image" with one click, and pinch zoom on phones works as before.
