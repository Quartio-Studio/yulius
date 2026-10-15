---
yulius-task: 5810e4d9-18e2-4525-8fe2-599530807c68
status: done
---

# SPEC-180: The original prompt reads as written: images inside it are one-line placeholders

Status: done
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
| OPP-001 | An image inside the text shows as a placeholder one line high: an image icon and `Image N`, the file's name as its tooltip, named "Open Image N, <name>". N is the image's number among the prompt's images in order, gone ones counted, as the queue card counts (SPEC-168). An image named twice (the same name) keeps one number. The line holding it keeps the text's line height. | e2e `original-prompt` "an image inside the prompt is a one-line Image N placeholder…", "a gone image keeps its number…" |
| OPP-002 | A click (or Enter) on the placeholder opens the image in the preview dialog, once however often it is clicked meanwhile. The file is read once per dialog, shared with its thumbnail in the list and every mention of the same name; a thumbnail that could not load shows the picture once a later read does. An image that is no longer on the server shows its name and "No longer on the server" in its place, as before; one that cannot be read when clicked says so in a toast, and the next click tries again. | e2e `original-prompt` "a placeholder opens its image once, and tries again after a failed read", "clicks while the image is read open one preview from one read" |
| OPP-003 | The "Attached files" list below the text shows every file of the prompt once, in order: those inside the text and those after it. A gone file never stands in for an available one of the same name. An image is a thumbnail carrying its number (`Image N` on the tile, named "Open Image N, <name>"), opening full size. Another file downloads; a gone file is named, as before. | e2e `original-prompt` (all tests), "a gone image keeps its number, and the list keeps a file named like a gone one" |
| OPP-004 | Another file inside the text keeps its one-line download button; the text and files after it are unchanged otherwise (SPEC-043, SPEC-124, #303). No axe violations. | existing `original-prompt` tests |

## Plan

- [x] Reproduce the defect and find its root cause
- [x] Write the fix spec and the regression test that fails
- [x] Fix the cause until the regression test and every gate pass
- [x] Independent review in a fresh session, findings fixed test-first

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
- **One name, one file.** The prompt endpoint gives names, not paths, and one entry per mention.
  Composer uploads have unique names (`pasted-<time>.png`), so the same name is the same file in
  practice. An image named twice has one number, one list entry and one read: every mention opens
  the file the list shows (review round 2). Two different available files with one name in
  different folders would show as one, the first. That is the cost of not sending paths, which
  would change the API for a case the composer does not make.
- **The list is one entry per name and availability** (review round 1). One entry per name let a
  gone file hide an available one of the same name, so its picture never showed.
- **A 404 on opening gets the generic error toast** (review nit, not taken). The dialog marks a
  file the server no longer has before any click ("No longer on the server"), so a 404 at the
  click means the file went while the dialog was open, which the error toast already reports.
- **No ADR.** This applies the decision of SPEC-150 (ADR-0124) to one more view; it takes no new
  lasting decision.

## Evidence

Run at 967f62a1 (current with `origin/dev` at 5599635c), 2026-10-15:

| Command | Result |
| --- | --- |
| `scripts/dev ci` (repository check, fmt, clippy, nextest, doctests, deny, shear, doc) | exit 0; nextest 1872 passed, 18 skipped |
| `cd e2e && npx e2e run` (whole suite) | exit 0; 55 files passed, 1 skipped; 528 tests passed, 1 skipped |
| `npx e2e run tests/original-prompt.e2e.ts tests/inline-images.e2e.ts` | 29 passed (8 + 21) |
| `scripts/dev security` | **not run**: exit 2, Vidoc rejects the key (HTTP 401, #470). Per ADR-0104 this is unrun, not passed. The diff is client-side only: DOM built with `h()`, blob URLs revoked on close, no new request or server path |

Test-first record:
- e70dfbf0's new test failed on the unchanged dialog: `Open Image 1, a.png` had count 0, because
  the image was a thumbnail. 9136f53f made it pass.
- Review round 1 (fresh session, head 9136f53f) requested changes: OPP-002 was untested, a gone
  file hid a same-named one from the list, a gone image took no number, a failed read stayed
  cached, and one check was narrow. 075884c6's tests failed on 9136f53f (`Image 1` where `Image 2`
  was due; no preview after a failed read), and 13fc587a fixed them.
- Review round 2 (fresh session, head 13fc587a) requested changes: the double-click and the
  shared read were not proved, an image named twice was read twice, and a failed tile stayed
  failed. 29960f4c's tests failed on 13fc587a (a read of index 2; no tile picture after the
  retry). The held-read test passes on 13fc587a and fails under both of the reviewer's mutations.
  967f62a1 fixed them.

Seen in the browser (the mock, Chromium): the dialog for a prompt like #166's reads as one
paragraph with `Image 1`, `Image 2` and `Image 3` placeholders, and three numbered thumbnails sit
below it.

Not tested here: browsers other than Chromium, and the real server's prompt endpoint (unchanged;
the mock mirrors its `attachments` and `parts`).
