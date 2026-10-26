---
yulius-task: 0f1a8ec0-0c11-48f5-ad81-1bdd1b402c84
status: draft
---

# SPEC-323: reply.e2e.ts no-leak checks fail when keys reach the terminal (#524)

Status: draft
Date: 2026-10-26
Owner: Yulius task #308
Scope authorization: the issue-triage request (task #308). Fix GitHub issue #524 in one pull
request (`Fixes #524`). Create no new GitHub issue; comment progress and leftovers on #524.

## Goal

The no-leak checks of `e2e/tests/reply.e2e.ts` "a reply field that closes while typing keeps the
keys out of the terminal" fail when the keys typed on the attention bar reach the task terminal.

## Context

- **SPEC-156** (#516, task #147) added Reply in the attention bar; its review round 2 added the
  case where the reply field closes under someone typing and the keyboard rests on the bar, which
  does nothing with keys (`taskview.js` `renderAttention`).
- **Weak check 1 (Enter).** The mock session's `onInput` (`mock.js`) calls `submit()` on `\r`,
  which empties `input` and appends `> <text>` to the terminal. The test pressed Enter with nothing
  typed and read `input === ''`: the same whether or not Enter leaked.
- **Weak check 2 (asynchronous leak).** A key the terminal takes goes xterm `onData` →
  `termsocket` `sendInput` → `MockSocket.send`, which delivers it to the session one task later
  (`setTimeout(…, 0)`). The reads ran right after `keyboard.type` / `press` with no wait, so a leak
  on its way was not there yet.
- **Why it was missed.** No mutation of the bar was run against the check: the earlier `'lo 2'`
  read is what caught the original defect.
- The other no-leak reads (`Escape closes…`, `Send` with empty text) follow a positive wait
  (Reply focused, field hidden or visible) and leave the session empty for the steps after them;
  they are unchanged.

## Requirements

| ID | Requirement | Evidence |
| --- | --- | --- |
| NLK-001 | Before the Enter the test seeds the session's input with `x`; after it the session still holds the seed and the terminal shows no `> x` line. | `reply.e2e.ts` |
| NLK-002 | Each no-leak read in that case first waits on positive conditions: the bar holds the keyboard, the hidden field still holds `Hal`, and the session has received a marker typed through the page's own terminal socket after the keys (the socket delivers in order, so a leaked key arrives before it). | `reply.e2e.ts` |
| NLK-003 | A mutation that forwards the bar's keys (letters, or Enter alone) to the terminal fails the test. | Evidence |
| NLK-004 | Test-only change: no product code changes. | diff |

## Plan

- [x] Spec SPEC-323 with the cause of both weak checks
- [ ] Mutation that forwards the bar's keys shown to pass the current test
- [ ] No-leak checks seed the session and wait behind the keys
- [ ] Mutations make the new checks fail, recorded in Evidence
- [ ] Independent review answered and gates green
- [ ] Merged into dev on green CI with a closing summary

## Acceptance criteria

- The case passes on the unchanged product.
- With the bar's Enter forwarded to the terminal, the case fails (it passed before this change).
- With the bar's letters forwarded to the terminal, the case fails.

## Test plan

- `cd e2e && npx e2e run tests/reply.e2e.ts` on the change.
- Throwaway mutations in `taskview.js` (a `keydown` listener on the bar sending the key with
  `terminal.sendKeys`), each run against the old and the new test, then reverted.
- Full e2e suite and `scripts/dev ci` before delivery.

## Decisions

- **The seed goes through the page's terminal socket, not into `session.input` directly.** It
  then doubles as the barrier: the mock socket delivers messages in order, so once the session
  holds the seed every key the page sent before it has arrived. Setting `input` directly would need
  a separate wait, which could only be a sleep. Alternative rejected: a page-side `setTimeout`
  flush, a sleep in disguise.
- **After the Enter a second marker (`y`) is sent the same way;** the session must then hold
  `xy`. A leaked Enter would leave `y` and a `> x` line.
- **Bounded by the socket.** A leak delayed inside the page before it is sent (a timer of its own)
  is not ordered by the barrier; no such path exists (xterm sends on the key event).
