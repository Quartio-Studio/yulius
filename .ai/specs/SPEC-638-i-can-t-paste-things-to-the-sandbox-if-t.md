---
yulius-task: 043ccc83-f810-42e9-8929-e94e28c82718
status: done
---

# SPEC-638. An image copied outside Yulius pastes into a terminal from the menu and Ctrl+V on macOS

## Goal

A person who copied a screenshot outside Yulius can paste it into a terminal in the sandbox. This
covers a task's terminal, the Superagent and a shell, and every paste gesture: the browser's paste
key, the right-click menu's Paste, and Ctrl+V on macOS. The image reaches the program as its
uploaded path, as a dropped file does. [Issue #1734](https://github.com/Quartio-Studio/yulius/issues/1734)

## Context

Symptom (task #622): "i can't paste things to the sandbox if they were copied outside."

Reproduction: real Chrome 158 on Xvfb running the mock UI. The clipboard was owned by another X
client (`xclip`), which is a copy made outside the browser. Keys and clicks were real X input
(`xdotool`):

| Gesture | Clipboard | Reached the terminal |
| --- | --- | --- |
| Ctrl+V (Linux, the browser's paste) | text | the text |
| Ctrl+V (Linux) | `image/png` | the uploaded `…/pasted-<stamp>.png` path |
| Ctrl+V (Linux) | a copied file (`text/uri-list`) | the uploaded file's path |
| Shift+right-click → Paste | text | the text |
| Shift+right-click → Paste | `image/png` | nothing, no notice |
| Ctrl+V with a macOS platform (`MacIntel`) | text | the text |
| Ctrl+V with a macOS platform | `image/png` or a copied file | nothing, no notice |

Root cause: on macOS the browser has no paste key for Ctrl+V and Ctrl+Shift+V, and the menu's
Paste has none at all. For these gestures `pasteFromClipboard` in `terminal-core.js` reads the
clipboard itself, and it reads only `navigator.clipboard.readText()`. For an image that read
returns an empty string, and the terminal then sends nothing. Ctrl+V is the image-paste key
Claude Code tells macOS users to press. The browser's own paste event takes another way:
`filedrop.js` takes its files, uploads them and types their paths (UI-048,
[SPEC-433](SPEC-433-i-can-t-paste-to-the-global-superagent-f.md)). The tests of
[SPEC-582](SPEC-582-fix-clipboard-on-codex.md) stub `readText` with text, and the image tests
dispatch a paste event. So no test read an image from the clipboard API.

The Browser panel's paste becomes typing on the sandbox display, and the sandbox clipboard never
holds the person's copy. Task #623 makes that a real paste on the display (its SPEC-639) and
claims its files, so it is not part of this change.

## Requirements

- **PST-001** In a writable terminal with a file drop, the clipboard-read paste (the menu's Paste,
  Ctrl+V and Ctrl+Shift+V on macOS) reads the clipboard's items. An image item, with no text
  beside it, goes through the same upload as a pasted image: it is stored as an attachment named
  `pasted-<stamp>.<ext>`, and its path is typed into the terminal once, as a bracketed paste when
  the program asked for one. An item offering PNG among other image types is taken as PNG.
- **PST-002** Text keeps priority, as in the browser's own paste: when the clipboard holds text,
  that text is pasted, and the image beside it is neither read nor uploaded, so a picture that
  cannot be read loses no text. Text of only blanks beside an image does not win, as in the
  browser's paste. An image that cannot be read is said, naming the paste key. A browser without `clipboard.read()` keeps the `readText()` paste.
  A refused read keeps its notice naming the paste key.
- **PST-003** The terminal never ignores a paste without saying so. A clipboard holding nothing the
  read can deliver shows a notice that names the paste key. In Chrome on Linux this covers a file
  copied in a file manager, which the browser hands only to its own paste. A terminal without a
  file drop, such as the sign-in terminal, shows "This terminal takes text only" when the clipboard
  holds only an image.

## Plan

- [x] Reproduce the defect and find its root cause
- [x] Write the fix spec and the regression test that fails
- [x] Fix the cause until the regression test and every gate pass
- [x] Independent review in a fresh session, findings fixed test-first

## Acceptance criteria

- **AC-1 (PST-001)** The task terminal's menu Paste, with only an image on the clipboard, types
  `…/attachments/tasks/tsk_studio_12/pasted-<stamp>.png ` once, and the upload status says
  `Added pasted-<stamp>.png`.
- **AC-2 (PST-002)** With text and an image on the clipboard, the menu Paste types the text and
  reads only `text/plain`, even when the picture cannot be read. A later image-only paste types
  exactly one path after the text. Without `read()`, `readText()` text is typed. The Codex
  right-click still pastes multiline text once, bracketed. A refused read still names the paste key.
- **AC-3 (PST-003)** An empty clipboard read shows a notice naming the paste key and sends nothing.
- **AC-4** The real-Chrome probe from the Context table, run again on the fixed code, gets the
  image's path for Shift+right-click → Paste and for macOS Ctrl+V.

## Test plan

- e2e (`filedrop.e2e.ts`): stubs `navigator.clipboard.read()` with the items an outside copy
  gives (an image only, text and an image, nothing) and drives the menu's Paste. It fails on the
  current code, which types nothing for the image.
- e2e (`terminal.e2e.ts`): the Codex right-click test stubs `read()`.
- Real-Chrome probe on Xvfb (`xclip` owner, `xdotool` input), Linux and a macOS platform
  override, as evidence.
- `scripts/dev quick`, the touched e2e files, Yulius CI.

## Decisions

- The clipboard-read paste prefers `navigator.clipboard.read()`, falling back to `readText()`
  only where `read()` is missing. Both reads ask for the same permission. Reading the text first
  and then the items would ask twice and lose the user gesture Safari requires.
- Images found by the read go to the terminal's existing file drop through a cancelable DOM event
  on the terminal element (`terminal-paste-files`). `filedrop.js` already owns uploads, naming,
  the read-only check and the status. A terminal without a drop leaves the event uncancelled, and
  the terminal then says it takes text only. A synthetic `ClipboardEvent` would also reach xterm's
  own paste handler, which would send empty bracketed-paste markers.
- Copied files cannot be read this way: the async clipboard API gives no files. The notice for an
  empty read names the browser's paste key, which does upload copied files.

## Evidence

- Regression test first (`69b0503a1`): `npx e2e run tests/filedrop.e2e.ts` failed on the
  unchanged code with `expected "" to match /…pasted-\d{8}-\d{6}\.png…/`, because the image typed
  nothing. With the fix, `filedrop.e2e.ts` (15 tests) and `terminal.e2e.ts` (33 tests) pass, 48 of
  48. `global-superagent.e2e.ts`, which shares the pasted-image upload, passes 13 of 13.
- Real Chrome 158 on Xvfb, after the fix. The clipboard was owned by `xclip`, a copy made outside
  Chrome, and input was real `xdotool` input:
  - Shift+right-click → Paste with `image/png` typed
    `…/attachments/tasks/tsk_studio_12/pasted-20261124-003950.png ` (before the fix: `""`, no notice).
  - With `text/uri-list`, a copied file, it typed nothing and showed "The clipboard has no text or
    image to paste here; paste files with Ctrl+V".
  - With text, it typed `menu words`.
  - Ctrl+V under a macOS platform override (`MacIntel`, clipboard read allowed): text typed
    `mac words`, and `image/png` typed `…/pasted-20261124-004015.png ` with "Added
    pasted-20261124-004015.png". A copied file showed the notice with ⌘V.
- Mutation checks by hand, each line restored afterwards:
  - DataError treated as a refusal: the empty-clipboard notice assertion failed.
  - Both new notices removed: both new tests failed.
  - The drop no longer taking `terminal-paste-files`: the image assertion failed with `""`.
  - Images ahead of text: at first this passed, because the read also skipped an item's image
    beside its text, so text priority was decided in two places. It is now decided only in
    `pasteFromClipboard`, and the same mutation fails with `expected "…pasted-….png " to match
    /^(\[200~)?cell text(\[201~)?$/`.
- `scripts/dev quick` passed. `scripts/dev security` said "Security scan disabled: Vidoc is
  turned off in Yulius, for this project or the whole instance; nothing was scanned". It did not
  run, so it is not a pass.
- Not tested: Safari and Firefox (no macOS host here; the macOS key path ran in Chrome on Linux
  under a platform override), and a file copied in macOS Finder.
- Found on the way, outside this change: the sign-in terminal's right-click menu opens beneath
  its modal dialog ([#1735](https://github.com/Quartio-Studio/yulius/issues/1735), already on
  `dev`).

### Review round 1

- The reviewer (Fable 5.2) found no product defect at `064108145`. It requested changes for two
  test gaps, each shown by a probe that survived:
  - With the `return` after the text paste removed, the image was uploaded as well, and 15 of 15
    tests still passed.
  - With the drop's `event.preventDefault()` removed, the terminal said "This terminal takes text
    only" while it uploaded, and 15 of 15 tests still passed.
- It also found one robustness gap: the picture was read even when its text won, so a picture
  that failed to read lost the text. It added an untested `readText()` fallback, `image/png`
  preference, and spec and docs wording.
- Test first (`2b04b9dc8`): the text step comes before the image step, and the image step keeps
  the input. The test asserts that only `text/plain` was read and checks the notice once after the
  drop took the image. It covers PNG over `image/svg+xml` listed first, and a browser without
  `read()`. On the round-0 code it failed with `expected "" to match /^(\[200~)?cell text…/`,
  because the failed picture read lost the text.
- Fix (`30cd8e27a`): `readClipboard` returns image readers, and `pasteFromClipboard` pastes text
  first and reads images only when there is no text. It prefers `image/png`.
- `filedrop.e2e.ts` and `terminal.e2e.ts` pass, 48 of 48.
- The reviewer's two probes now fail:
  - Without the `return`, the test reports `expected ["text/plain","image/png"] to equal
    ["text/plain"]`.
  - Without the cancel, it reports `expected "This terminal takes text only" not to match /text
    only/`.
- Real Chrome on Xvfb, on this code:
  - Menu Paste with an `xclip` image typed `…/pasted-20261124-010656.png `, and with text typed
    `menu words`.
  - macOS Ctrl+V (platform override) typed `mac words`, and with an image typed
    `…/pasted-20261124-010712.png `.

### Review round 2

- The reviewer (Fable 5.2) approved `fa8b3cb9f`. All seven round-1 findings were resolved, and it
  ran both probes again: each now fails the test. The head's Yulius CI run `0d413281` passed.
- It left one own-code minor and two nits, fixed after the approval within the allowance for
  product lines:
  - **Minor:** no test held the notice for an image-only read that fails.
  - **Nit:** blank text beside an image won here but not in the browser's own paste.
  - **Nit, pre-existing and newly reachable:** an SVG uploaded as `.svgxml`.
- Test first (`git log` "blank text beside an image…"): on the approved code it failed with
  `expected " " to match /^(\[200~)?cell text…pasted-….png…/`, because the blank text was pasted.
- Fix: text wins when it is not blank, or when there is no image. An image read that fails shows
  "The browser did not share the image; paste with ⌘V/Ctrl+V". `pastedName` drops `+xml`, so an
  SVG is `pasted-<stamp>.svg`.
- `filedrop.e2e.ts`, `terminal.e2e.ts` and `global-superagent.e2e.ts` pass, 61 of 61.
