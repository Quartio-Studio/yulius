---
yulius-task: 5810e4d9-18e2-4525-8fe2-599530807c68
status: draft
---

# SPEC-180: The original prompt reads as written: images inside it are one-line placeholders

Status: draft
Date: 2026-10-15
Owner: Yulius task #167, issue #565
Scope authorization: task #167, "UI problem -> fix", with a screenshot of the "Original prompt ·
#166" dialog. The prompt's text is split around three large pictures; each line holding one grows
to the picture's height, with its words centred beside it. Fixes SPEC-124 INL-008's rendering
in the light of SPEC-150 and SPEC-168.

## Goal

The original prompt dialog reads like the composer and the queue card. Each image inside the
text is a one-line `Image N` placeholder that opens the image, so the text keeps its line height.
The pictures show as numbered thumbnails in the "Attached files" list below the text, as the
composer's tray shows them.

## Context

- **Symptom.** `actions.js` `openPromptDialog` places each file inside the text with
  `fileItem(file, n, 'span')`: an image is a `.prompt-thumb` button holding the `<img>`, which
  `.prompt-file.inline .prompt-thumb img` lets grow to 240 × 160 px with `vertical-align: middle`.
  A line holding one becomes 160 px high, and a long prompt with screenshots reads as scattered
  fragments (the screenshot).
- **Root cause.** SPEC-124 (task #105) put files where they sit as thumbnails everywhere. SPEC-150
  (task #144) then decided that "a placeholder is a label, not a thumbnail: the problem is a 64 px
  image inside a line of text". It moved the composer to `Image N` placeholders (`.pi-atom
  .pi-image`, one line high) with the pictures in a tray. SPEC-168 (task #158) did the same for the
  queue card. The original prompt dialog was not part of either change, so it kept the thumbnails.
- **Why the tests missed it.** `original-prompt.e2e.ts` INL-008 checks that an image is *in place*
  (between the words, opening on click), not how high the line it sits in is.
- **The API.** `GET /api/tasks/{tid}/prompt` gives `attachments` (name, image, available) in
  order, one per mention, and `parts`. Files are read one by one from
  `/prompt/attachments/{n}`. Nothing on the server changes.

## Requirements

| ID | Requirement | Evidence |
| --- | --- | --- |
| OPP-001 | An image inside the text shows as a placeholder one line high: an image icon and `Image N`, the file's name as its tooltip, named "Open Image N, <name>". N is the image's number among the prompt's images in order. An image named twice (the same name) keeps one number. The line holding it keeps the text's line height. | e2e `original-prompt` "an image inside the prompt is a one-line Image N placeholder…" |
| OPP-002 | A click (or Enter) on the placeholder opens the image in the preview dialog. An image that could not be read, or that is no longer on the server, says so in its place (the placeholder shows the file's name and "No longer on the server", as before). | same test; existing "a prompt starting with ---" test |
| OPP-003 | The "Attached files" list below the text shows every file of the prompt once, in order: those inside the text and those after it. An image is a thumbnail carrying its number (`Image N` on the tile, named "Open Image N, <name>"), opening full size. Another file downloads; a gone file is named, as before. | e2e `original-prompt` (all tests) |
| OPP-004 | Another file inside the text keeps its one-line download button; the text and files after it are unchanged otherwise (SPEC-043, SPEC-124, #303). No axe violations. | existing `original-prompt` tests |

## Plan

- [ ] Reproduce the defect and find its root cause
- [ ] Write the fix spec and the regression test that fails
- [ ] Fix the cause until the regression test and every gate pass
- [ ] Independent review in a fresh session, findings fixed test-first

## Acceptance criteria

- The dialog for a prompt "First @'…/a.png' then …" shows "First Image 1 then …" on one line of
  normal height. Clicking `Image 1` opens a dialog titled `a.png` with the image.
- The "Attached files" list holds `a.png` with the number 1 on its thumbnail, as well as the
  files after the text.
- The dialog has no axe violations.

## Test plan

- `e2e/tests/original-prompt.e2e.ts`: a new test for OPP-001..003, written first and failing on the
  unchanged dialog. On that dialog the inline image is a thumbnail, its line is taller than two
  text lines, there is no `Image 1` label, and the list leaves the inline image out. It waits on
  conditions (visible elements, text), never on time. The INL-008 test is updated for the
  placeholder's name (`Open Image 1, inline.png`) and the list that now holds the inline file.
- Gates: `scripts/dev check`, `scripts/dev lint`, `scripts/dev test`, the e2e suite,
  `scripts/dev security`.

## Decisions

- **Use the composer's placeholder, not a smaller thumbnail.** Options: (a) shrink the inline
  thumbnail to the line height; (b) render images as blocks between paragraphs; (c) the `Image N`
  placeholder of SPEC-150 and SPEC-168. (a) gives an unreadable 20 px picture. (b) still breaks a
  sentence that names an image in the middle, and the screenshot's prompt does exactly that.
  (c) is how the same prompt reads while it is written and while it is queued, so a person sees
  one form of their prompt everywhere. Chosen: (c), reusing `.pi-atom .pi-image`.
- **The list shows every file, numbered.** Without thumbnails in the text, a prompt whose files
  all sit inside it would show no picture at all. The composer's tray answers the same need
  (SPEC-150), so the list becomes the dialog's tray: every file once, images with their number.
  This reverses SPEC-124 INL-008's "files inside the text are not listed below it".
- **Numbers by name.** The prompt endpoint gives names, not paths. Composer uploads have unique
  names (`pasted-<time>.png`), so the same name is the same file in practice. Two different files
  with one name in different folders would share a number. That is harmless here: each still opens
  its own file, which is read by its index. Changing the API to send paths was not worth it for
  this.
- **No ADR.** This applies the decision of SPEC-150 (ADR-0124) to one more view; it takes no new
  lasting decision.

## Evidence

To be filled in when the work is done.
