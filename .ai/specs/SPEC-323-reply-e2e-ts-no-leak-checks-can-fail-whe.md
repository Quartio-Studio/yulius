---
yulius-task: 0f1a8ec0-0c11-48f5-ad81-1bdd1b402c84
status: done
---

# SPEC-323: reply.e2e.ts no-leak checks fail when keys reach the terminal (#524)

Status: done
Date: 2026-10-26
Owner: Yulius task #308
Scope authorization: the issue-triage request (task #308). Fix GitHub issue #524 in one pull
request (`Fixes #524`). Create no new GitHub issue; comment progress and leftovers on #524.

## Goal

Every no-leak check of `e2e/tests/reply.e2e.ts` fails when the keys reach the task terminal: the
case "a reply field that closes while typing keeps the keys out of the terminal" (keys typed on the
attention bar), and Escape, Cancel and an empty Send in the reply field.

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
- **Why it was missed.** No mutation of the bar was run against the check. Run now, it shows the
  `'lo 2'` read was weak too: with the bar's letters and Enter forwarded to the terminal the old
  test could still pass (it raced: 3 of 5 runs passed), as the letters had not always arrived when
  it read and the leaked Enter then submitted them, leaving `input` empty again (Evidence).
- **The same blindness in three more reads (review round 1).** After Escape, after Cancel and
  after an empty Send the tests read `input === ''` on an empty session, where a leaked Enter
  changes nothing either; a mutation sending `\r` on an empty Send passed all six tests.

## Requirements

| ID | Requirement | Evidence |
| --- | --- | --- |
| NLK-001 | Before the keys each no-leak check seeds the session's input with `x`; after them the session still holds the seed and the terminal shows no `> x` line and no `(esc) interrupted` (closing field, Escape, Cancel, empty Send). | `reply.e2e.ts` `nothingReached` |
| NLK-002 | Each no-leak read first waits on positive conditions: what shows the keys were handled (the bar holds the keyboard and the hidden field still holds `Hal`; Reply focused after Escape; the field hidden after Cancel; the field focused again after an empty Send), then the session has received a marker typed through a terminal socket of the page after the keys (the socket delivers in order, so a leaked key arrives before it). | `reply.e2e.ts` `typeBehind` |
| NLK-003 | A mutation that forwards the bar's keys (letters, or Enter alone) to the terminal fails the test, and so does an Enter sent on Escape, on Cancel or on an empty Send, or the field's letters forwarded. | Evidence |
| NLK-004 | Test-only change: no product code changes. | diff |

## Plan

- [x] Spec SPEC-323 with the cause of both weak checks
- [x] Mutation that forwards the bar's keys shown to pass the current test
- [x] No-leak checks seed the session and wait behind the keys
- [x] Mutations make the new checks fail, recorded in Evidence
- [x] Independent review answered and gates green
- [x] Merged into dev on green CI with a closing summary

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
  is not ordered by the barrier. xterm sends on the key event; the one timer on the way,
  `typeAndSubmit`'s Enter 60 ms after its text, always follows a text sent at once, which the
  marker catches.
- **The empty Send's test clears the marker** (two backspaces through the same socket, waiting for
  an empty session) so its next step still sends exactly `> Go on`.
- **A leaked Escape is seen by its line (review round 2).** The mock draws a raw Escape as
  `(esc) interrupted` and keeps the typed input, so the check also looks for that line, after the
  wait for the marker's echo (screen updates arrive in order).

## Evidence

Commands from the worktree, `cd e2e && E2E_TELEMETRY_DISABLED=1 npx e2e run tests/reply.e2e.ts`.
Mutations: a throwaway `keydown` listener on the bar (`taskview.js`, after `const attention`) that
sends the bar's own keys with `terminal.sendKeys`, either Enter only (`\r`) or every letter and
Enter; reverted with `git checkout -q -- crates` after each run (the test was committed first).

| Test | Product | Result |
| --- | --- | --- |
| old (dev 91785a67) | unchanged | 6 passed |
| old | Enter forwarded | 6 passed: the leak went unseen |
| old | letters and Enter forwarded | 6 passed here; the reviewer saw it fail 2 of 4 (`"lo 2"`): a race |
| new (f094a9ef) | unchanged | 6 passed |
| new | Enter forwarded | 1 failed: `expected "y" to be "xy"` (`reply.e2e.ts:158`) |
| new | letters and Enter forwarded | 1 failed: `expected "lo 2x" to be "x"` (`reply.e2e.ts:152`) |

Review round 1 (all six reads covered, 85866c56):

| Test | Product | Result |
| --- | --- | --- |
| new (85866c56) | unchanged | 6 passed |
| new | Enter sent on Escape | 1 failed: Escape, `expected "y" to be "xy"` |
| new | Enter sent on Cancel | 1 failed: Escape (its Cancel), `expected "z" to be "xyz"` |
| new | Enter sent on an empty Send | 1 failed: Send sends, the field closed (the leaked Enter sent the seed `x` as a prompt, so the task stopped waiting); without the seed this mutation passed all six |
| new | the field's letters forwarded | 2 failed: `expected "xNot yety" to be "xy"`, `expected "Halx" to be "x"` |
| new | the bar's Enter forwarded | 1 failed: `expected "y" to be "xy"` |
| new | the bar's letters and Enter forwarded | 1 failed: `expected "lo 2x" to be "x"` |

Review round 2 (fe424f10): unchanged product 6 passed; the field's Escape also sent to the
terminal (`terminal.sendKeys('\x1b')`), which passed at 12fc18b8, now fails the Escape test
(`expect.not.toContainText failed`, `reply.e2e.ts:45`).

Gates:

- `python3 scripts/bootstrap.py && scripts/dev ci` (started at f094a9ef; later commits change only
  `e2e/tests/reply.e2e.ts` and documents): exit 0, cargo-deny "advisories ok, bans ok, licenses ok,
  sources ok".
- `cd e2e && E2E_TELEMETRY_DISABLED=1 npx e2e run` at 3bd99043: 66 files passed, 1 skipped; 712
  tests passed, 1 skipped.
- `yulius security-scan` at 3bd99043: **not run** (exit 2): Vidoc answered HTTP 429 "This API key
  has used its 50,000,000 tokens"; `e2e/tests/reply.e2e.ts` not scanned. Unrun, not passed
  (ADR-0133); the PR's CI `security` job is the live scan.
- Independent review (fresh session, `om-code-review`), three rounds: round 1 asked for the
  Escape, Cancel and empty Send reads (fixed in 85866c56) and wording fixes; round 2 asked for a
  leaked Escape check (fe424f10); round 3 approved 3bd99043 with no findings.
