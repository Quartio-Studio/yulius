---
status: done
---

# SPEC-448: A dialog shows its whole body in Safari

Status: done
Date: 2026-11-10
Owner: studenciak00owl (external contribution)
Scope authorization: the contributor's own report, verbatim (2026-11-10): "Popraw te okienko, bo
defaultowo zbyt krótkie się wyświetla i nie widać reszty opcji" ("fix this dialog: by default it
shows too short and the rest of the options cannot be seen"), seen on Add subscription in Safari.

## Goal

Every dialog opened with `openModal` shows its whole body in Safari, as it does in Chrome, and
scrolls its body only when the window is too short for it.

## Context

- `.modal` is a column flex container whose height is `auto` with `max-height: calc(100vh - 48px)`;
  `.modal-body` had `flex: 1; min-height: 0`, so its flex basis is `0%`.
- Chrome resolves that `0%` basis like `content` because the dialog's height is indefinite. WebKit
  resolves it to `0`, and `min-height: 0` lets the body stay there: the body is 32 px tall (its
  padding) and everything else scrolls inside it.
- Measured in WebKit 26.0 (Playwright 1.56.1) on the mock (`?mock=1`), 1440 × 900, body height
  shown / content height, on `dev` (`91a0cec5`):

  | Dialog | WebKit before | WebKit after | Chrome before and after |
  |---|---|---|---|
  | Add a Claude subscription | 32 / 150 | 150 / 150 | 150 / 150 |
  | New playground | 32 / 653 | 653 / 653 | 653 / 653 |
  | Import repository | 32 / 418 | 418 / 418 | 418 / 418 |

- The full-height sign-in terminal dialog (`modal-full`) has a definite height; it is unchanged
  (dialog 850, body 744, terminal 662 in both engines, before and after).

## Requirements

- DLG-001. `.modal-body` takes its content's height as its flex basis (`flex: 1 1 auto`), so a
  dialog is as tall as its content up to its `max-height` in every engine.
- DLG-002. A body taller than the space left still shrinks (`min-height: 0`) and scrolls, and a
  `modal-full` dialog's body still fills its fixed height.

## Plan

- [x] Spec with the cause and the measurements
- [x] `.modal-body { flex: 1 1 auto }`
- [x] e2e guard in `subscriptions.e2e.ts`
- [x] docs/changes.md

## Acceptance criteria

- In WebKit, Add subscription, New playground and Import repository show their whole body
  (table above); at a 380 px tall window the body scrolls instead of overflowing.
- In Chrome nothing changes (same measurements before and after).
- The e2e guard passes: the Add subscription dialog's body has flex basis `auto` and shows all of
  its content.

## Test plan

- e2e (`subscriptions.e2e.ts`): "a dialog shows its whole body, also in Safari". The suite runs
  in Chromium only, where the old rule did not clip, so the guard checks the computed flex basis
  (the rule WebKit needs) besides the shown height.
- Manual, WebKit and Chrome through Playwright on the mock: the measurements above.

## Evidence

- WebKit and Chrome measurements: see Context.
- `npx e2e run tests/subscriptions.e2e.ts`: see the pull request.

## Known limits

- The e2e suite has no WebKit target, so a later change back to a `0%` basis is caught by the
  computed-style check, not by a Safari run.
