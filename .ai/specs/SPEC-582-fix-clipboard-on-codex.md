---
yulius-task: ca67b86c-bf72-4b4b-bed2-acdb5ac9962b
status: done
---

# SPEC-582. Paste from the viewer’s clipboard in a Codex terminal

## Goal

A person using the Codex web terminal can right-click and paste text into the draft.

## Context

The screenshot shows two “clipboard text is unavailable” errors. A plain right-button
press reproduced that error on Codex CLI 0.170.0 in an isolated Linux PTY with no display.
The same composer accepted a bracketed paste. Yulius gives a mouse-tracking program the
right-button press and suppresses its own clipboard menu. Codex consequently tries to
read the sandbox’s desktop clipboard. Earlier terminal tests exercised Claude’s mouse
handling and browser paste events. [Issue #1584](https://github.com/Quartio-Studio/yulius/issues/1584)

## Requirements

- **CLP-001** A plain right-click in a mouse-tracking Codex terminal opens the viewer’s clipboard menu. The PTY receives no right-button press or release from that gesture.
- **CLP-002** Choosing Paste in a writable terminal inserts the viewer’s clipboard text once through xterm’s paste path. A refused clipboard read offers the native paste shortcut. Read-only viewers cannot paste.
- **CLP-003** Primary clicks and other programs’ mouse handling continue to follow [ADR-0079](../../docs/decisions/0079-the-terminal-mouse-is-the-programs.md).

## Plan

- [x] Reproduce the Codex terminal clipboard error and specify the fix
- [x] Capture the clipboard failure in a regression test
- [x] Restore clipboard behavior in the Codex terminal and verify it
- [x] Independently review the fix and deliver it to dev

## Acceptance criteria

- **AC-1 (CLP-001, CLP-002)** With mouse tracking and bracketed paste enabled, right-click then choose Paste. The PTY receives the bracketed clipboard text once. Cancelling the menu or denying the read sends no clipboard text.
- **AC-2 (CLP-002, CLP-003)** A read-only viewer cannot send paste input. Codex still receives a primary click. A mouse-tracking Claude terminal still receives an unmodified right-click.

## Test plan

Drive real mouse events in the Chromium task view and observe input at the terminal socket.
Exercise a successful clipboard read, browser refusal and existing read-only paste coverage.
Run the terminal and web suites, the repository quick checks and Yulius CI.

## Decisions

Use the existing Copy, Paste and Select all menu. Choosing Paste explicitly requests
clipboard text from the browser. The terminal already receives the session’s
harness in its state message. The Codex PTY needs only ordinary paste input. [ADR-0421](../../docs/decisions/0421-codex-right-click-opens-the-viewers-clipboard-menu.md)

## Evidence

- An isolated Codex CLI 0.170.0 PTY with a synthetic login and no display printed “clipboard text is unavailable” after an SGR right-button press and release. Sending bracketed text to the same PTY inserted it in the composer.
- Before the fix, the Chromium regression failed because the Paste menu item was absent. After the fix, the Codex right-click, Claude clipboard and read-only paste cases all passed. The Codex test observed one bracketed multiline paste at the socket and the native shortcut hint after a denied read.
- A separate Chromium run with clipboard permission wrote a marker through the real Clipboard API. Right-click and Paste placed that marker once in the mock Codex task draft.
- `npm ci --ignore-scripts` and `npx e2e run` completed. All 33 terminal tests passed. Across the full Chromium suite, 1,298 tests passed and one model-backed test was skipped. Safari and Firefox were not exercised.
- `scripts/dev quick` passed. `scripts/dev security` reported Vidoc disabled. `scripts/dev mutants` reported “No Rust code changed against origin/dev: nothing to mutate.”
- Independent review approved `324a2d5bdcd794ee3b76d8a757ca1ea518a622b8` with no findings. The reviewer ran the three focused clipboard tests. Yulius records the merge and its CI verdict in the task’s delivery evidence.
