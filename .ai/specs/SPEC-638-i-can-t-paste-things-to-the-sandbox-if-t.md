---
yulius-task: 043ccc83-f810-42e9-8929-e94e28c82718
status: in-progress
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
  the program asked for one.
- **PST-002** Text keeps priority, as in the browser's own paste: when the clipboard holds text,
  that text is pasted and the image is not uploaded. A browser without `clipboard.read()` keeps the
  `readText()` paste. A refused read keeps its notice naming the paste key.
- **PST-003** The terminal never ignores a paste without saying so. A clipboard holding nothing the
  read can deliver (a file copied in a file manager, which browsers hand only to their own paste)
  shows a notice that names the paste key. So does a terminal without a file drop, such as a login
  terminal, when the clipboard holds only an image.

## Plan

- [x] Reproduce the defect and find its root cause
- [ ] Write the fix spec and the regression test that fails
- [ ] Fix the cause until the regression test and every gate pass
- [ ] Independent review in a fresh session, findings fixed test-first

## Acceptance criteria

- **AC-1 (PST-001)** The task terminal's menu Paste, with only an image on the clipboard, types
  `…/attachments/tasks/tsk_studio_12/pasted-<stamp>.png ` once, and the upload status says
  `Added pasted-<stamp>.png`.
- **AC-2 (PST-002)** With text and an image on the clipboard, the menu Paste types the text and
  uploads nothing. The Codex right-click still pastes multiline text once, bracketed. A refused
  read still names the paste key.
- **AC-3 (PST-003)** An empty clipboard read shows a notice naming the paste key and sends nothing.
- **AC-4** The real-Chrome probe from the Context table, run again on the fixed code, gets the
  image's path for Shift+right-click → Paste and for macOS Ctrl+V.

## Test plan

- e2e (`filedrop.e2e.ts`): stubs `navigator.clipboard.read()` with the items an outside copy
  gives (an image only, text and an image, nothing) and drives the menu's Paste. It fails on the
  current code, which types nothing for the image.
- e2e (`terminal.e2e.ts`): the Codex right-click test stubs `read()` as well as `readText()`.
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
