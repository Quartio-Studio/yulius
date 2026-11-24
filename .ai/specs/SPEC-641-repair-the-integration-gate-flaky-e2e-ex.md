---
yulius-task: 58d32986-54df-4a2b-8030-be560a652c54
status: done
---

# SPEC-641: A terminal shown late leaves the keyboard on the control the person moved to

Status: done
Date: 2026-11-24
Owner: Yulius task #625 (started by task #616's open-issues run; no issue may be created)
Scope authorization: task #625: "make the e2e test `tests/browser.e2e.ts › expired clipboard
actions preserve focus and explain how to copy again` deterministic. Fix its root cause, in the
web UI if the product loses focus wrongly. Deliver it in ONE pull request into dev."

## Goal

The integration gate on dev 320803966 (Yulius CI run be25f19b-f494-4bec-abc3-59eed7c34e11, job
`e2e`) failed one test, `browser.e2e.ts:131`: "Copy to my clipboard", focused by the test, was not
focused after the offer expired, and disappeared about 4 s later. The test passes deterministically
on any host speed, and the product no longer takes the keyboard from a control the person moved to
while a task's terminal was still loading.

## Context

- **Symptom (trace of the failed run).** Step 9 focused the toast's "Copy to my clipboard" while
  the page was still filling in (164 changes since step 5). From the first read after the expiry,
  the button showed no focus (39 reads), then at 4056 ms it was gone.
- **Root cause.** The task view asks its terminal for the keyboard when it opens
  (`terminal-core.js` `takeFocus`, SPEC-056 WUI-102). A request made before the terminal showed
  is kept and lands in `reveal()` once the first state and screen are drawn, unless another
  terminal was asked for since, or the person is typing in a text field (SPEC-052). Buttons do not
  hold it back ("Buttons and tabs do not hold it back"). On a loaded host the mock task terminal's
  first screen arrived after the test focused the toast button: `reveal()` called `term.focus()`
  and moved the keyboard to the terminal's `xterm-helper-textarea`. The toast's `focusout` then
  started its 4 s close timer (`dom.js` `toast`, ADR-0237), which explains the disappearance.
  The expiry handler, the "Needs you" region and `controlCopyExpiry` are not involved. Probes:
  firing the sign-in request (`askBrowserSignIn`) before or after the focus kept the focus; holding
  the terminal's first screen (`__yuliusHoldTerminal`) until after the focus reproduced the CI
  signature exactly (default state 39 reads, no node at 4071 ms). The focus-out stack is
  `reveal (terminal-core.js:303) → term.focus`.
- **Why the tests missed it.** On an idle host the terminal shows before the test reaches the
  Browser tool, so the deferred focus has already landed (on the rail button) before the toast
  exists. Only a slow first screen orders it after the person's own focus.
- **Same cause elsewhere.** Any control the person reaches before a task's terminal shows (a toast
  action, a tools rail button, a header button, a menu) lost the keyboard to the terminal. That is
  unexpected for keyboard and screen reader users (WCAG 3.2.1, 2.4.3), not only a test race.

## Requirements

| ID | Requirement | Verification |
| --- | --- | --- |
| TFK-001 | A terminal focus request made before the terminal showed does not land when, at the reveal, the keyboard is on a different element than when the request was made (the person moved on), other than the page itself or the terminal's own input. | `browser.e2e.ts` "expired clipboard actions preserve focus…" holds the terminal's first screen until after the focus |
| TFK-002 | The WUI-102 and SPEC-052 rules stay: the keyboard on the page body takes the deferred focus; a later request of another terminal wins; typing in a text field holds it back. | `queued-messages.e2e.ts`, `terminal.e2e.ts`, full e2e suite |
| TFK-003 | `browser.e2e.ts:131` orders the terminal's first screen explicitly (released after the focus, then waited for), so it no longer depends on host speed. | the same test, run repeatedly |

## Plan

- [x] Reproduce the focus loss deterministically by holding the terminal's first screen
- [x] Regression check: the clipboard focus test holds the terminal and fails without the fix
- [x] Deferred terminal focus no longer takes the keyboard from a control the person moved to
- [x] Terminal, queued-messages and browser e2e specs pass repeatedly; full e2e suite passes
- [x] Decision record, architecture note and changes entry
- [x] Independent review approves the head

## Acceptance criteria

- The changed test fails on dev's `terminal-core.js` with the CI signature and passes with the fix.
- `browser.e2e.ts` passes in full, repeatedly; the terminal focus specs pass.
- The full e2e suite passes locally once and on Yulius CI.

## Test plan

- `npx e2e run tests/browser.e2e.ts:131` with and without the fix (restore `terminal-core.js`
  from dev for the failing run).
- `npx e2e run tests/browser.e2e.ts` five times in a row; `terminal.e2e.ts`,
  `queued-messages.e2e.ts` and `editor.e2e.ts` in full.
- `npx e2e run` (whole suite) once.

## Decisions

- **Fix the product, not only the test.** Making the test wait for the terminal before focusing
  would hide a real focus theft. Recorded in an ADR.
- **"Moved on" is a different focused element than at the request.** Alternatives: hold back only
  for focus inside toasts and dialogs (ad hoc; menus and rail buttons lose it the same way), or
  count only focus moves by a user gesture (a script's or assistive tool's `focus()` would still be
  taken). Comparing the focused element at the request and at the reveal keeps the cases WUI-102
  meant (the link or button that opened the task view still focused, or the page itself) and
  stops the theft of everything the person reached afterwards.
  Recorded as [ADR-0472](../../docs/decisions/0472-a-terminal-shown-late-leaves-the-keyboard-where-the-person-moved-it.md).
- **No architecture change.** The fix is one rule inside `terminal-core.js`; `docs/architecture.md`
  does not describe deferred terminal focus, so it stays as it is.

## Evidence

- Root cause probes (`target/run625/zprobe.e2e.ts.txt`, not committed): firing the mock's sign-in
  request (`askBrowserSignIn`) before the offer, between offer and focus, or after the focus kept
  the focus (5 load points, all passed). Holding the task terminal's first screen until after the
  focus reproduced the CI signature; the `focusout` stack was
  `reveal (terminal-core.js:303) → Dl.focus → yn.focus` (xterm).
- Regression check without the fix (commit dba6ca199, dev's `terminal-core.js`):
  `npx e2e run tests/browser.e2e.ts:133` failed: `expect.toBeFocused … default states (1 match)
  x38 -> at 3967ms no node (0 matches) x10`, as in CI run be25f19b (x39, 4056 ms).
- With the fix (34f1b1dc4): the same test passed (1.89 s).
  `npx e2e run tests/terminal.e2e.ts tests/queued-messages.e2e.ts tests/editor.e2e.ts
  tests/browser.e2e.ts`: 4 files, 96 tests passed.
- `npx e2e run tests/browser.e2e.ts` five times in a row: 7 passed each time.
- Whole suite `npx e2e run`: 103 files passed, 1 skipped; 1363 tests passed, 1 skipped
  (`agent.e2e.ts`, needs a model key); 3 m 29 s.
- `scripts/dev security`: "Security scan disabled: Vidoc is turned off in Yulius, for this project
  or the whole instance; nothing was scanned." (exit 0, unrun).
- Not tested: real hosts' screen readers; the rule is checked in headless Chromium with the mock.
