---
yulius-task: b6ad51d9-e971-458e-8fbe-df658740b02b
status: done
---

# SPEC-027: a click on Claude Code's output reaches Claude Code

Status: done
Date: 2026-10-12
Scope authorization: user request (task #25, F 2026-10-12): "When I click on tasks output, it
should expand, as it happens natively", with a screenshot of a task terminal showing a collapsed
tool group ("Searched for 2 patterns, read 1 file, ran 3 shell commands").

## Goal

In a task's terminal, a click on Claude Code's full-screen transcript does what it does in a
native terminal: a collapsed tool group expands (and collapses again), while dragging and
double-clicking still select text like page text (UI-019).

## Context

**Symptom.** Claude Code 2.1.x draws full screen (alternate screen) and switches on mouse reporting
(DECSET 1000/1002 + 1006). In a native terminal a click on a collapsed tool group expands it. In
Yulius nothing happens.

**Root cause.** `mousefilter.js` removes the mouse-reporting switches from the output by default so
terminal text behaves like page text (F 2026-10-07): xterm.js never reports the mouse to the app.
#34/#79 gave wheel turns a path back (`wheelReport`, sent with `scroll: true` so it neither types
nor takes control), but clicks had none: `terminal-core.js` uses a click only to take the
terminal's size. The "Mouse: send to app" toggle hands every mouse event over, but it is hidden in
a corner and costs text selection, so a plain click never reaches Claude Code.

**Why tests missed it.** AC-UI-11 checks that the mouse stays with selection and that the wheel
reaches the app; nothing checked a click, and the mock's full-screen session ignored mouse reports
other than the wheel.

**What else the cause could break.** Any click-driven feature of a full-screen TUI (Claude Code's
click to position the prompt cursor, other agents' TUIs) was equally unreachable; this fix covers
them all, because it forwards the click itself, not a Claude Code-specific action.

## Requirements

| ID | Requirement | Evidence |
| --- | --- | --- |
| CLK-001 | A writer's plain primary click (no drag, not on a link, no modifier, not the second click of a double-click) on a full-screen app that asked for mouse reporting reaches the app at once as a press and a release report in the app's encoding (SGR when it asked for 1006, X10 otherwise), at the clicked cell. A click in the margin beside another viewer's centred grid only takes the size. | `terminal.e2e.ts` "a click on Claude Code's output expands it", "a click beside another viewer's grid" |
| CLK-002 | Dragging and double-clicking still select text; the click does not take control by itself (it is sent like a wheel turn); taking the size by a click is unchanged. | same, existing size test |
| CLK-003 | Read-only viewers, the normal screen and a viewer that handed the mouse to the app (xterm reports itself) send nothing extra. | "a read-only viewer's click does not reach the program" (no input frame leaves the page); the other two by code path |

## Plan

- [x] Reproduce the defect and find its root cause
- [x] Write the fix spec and the regression test that fails
- [x] Fix the cause until the regression test and every gate pass
- [x] Independent review in a fresh session, findings fixed test-first

## Acceptance criteria

- In the mock task terminal (full screen, mouse reporting on), a click on "Searched for 2
  patterns, read 1 file (click to expand)" shows the group's lines; a second click collapses it.
- A drag across a row still selects its text and the group does not toggle.
- A read-only viewer's click does not reach the app.
- A click in the margin beside another viewer's grid takes the size and does not toggle the group.

## Test plan

- e2e (`e2e/tests/terminal.e2e.ts`, mock): click expands, click collapses, drag selects without
  toggling, read-only click sends nothing. The mock session (`mock.js`) expands a collapsed group on
  an SGR press of button 0 at its row, as Claude Code does.
- The double-click rule: a real first click, then the second click's events with click count 2
  dispatched in the page (the test mouse always presses with count 1); it must not collapse the
  group. Checked by removing the rule: the test fails.
- Unit (in the page): `clickReport` encodes SGR and X10 press/release.
- Existing: the size-taking click test and the wheel test keep passing.

## Decisions

- **Forward a plain click, keep selection** rather than handing the whole mouse to the app by
  default: Claude Code's own drag selection would replace the page-text selection the team asked
  for (F 2026-10-07), and the toggle stays for apps that need drags.
- **Send at once, skip the second click of a double-click** (`event.detail > 1`) rather than waiting
  ~450 ms to rule out a double-click: an expand must feel immediate like a native terminal; a
  double-click on a group both expands it and selects a word, which is harmless.
- **Sent like a wheel turn (`scroll: true`)**: a click report is pointer input, not typing, so it
  does not move control by itself; the existing settled click still takes the size. No server
  change.
- **Touch:** a tap is one click; a touch's mouse events come after its release, so both taps of a
  double tap reach the app (a group expands and collapses). Accepted: phones zoom on double tap.
- **Known limits, shared with `wheelReport`:** with X10 compatibility mode 9 alone a release is still
  sent, and X10 coordinates above 95 travel as two UTF-8 bytes. Claude Code uses SGR (1006), so the
  feature does not depend on either.
- No ADR: this follows the wheel decision of #34/#79 inside UI-019.

## Evidence

- Regression first: `e61bec1` added the tests. Before the fix they failed: the click never reached
  the mock program, and `clickReport` was missing.
- Each rule is load-bearing. Without the click-count rule, the double-click test fails. Without
  `state.can_write`, the read-only test fails (input frames leave the page). Without the
  inside-the-grid check, the margin test fails.
- `cd e2e && npx e2e run`: full suite green (see the pull request).
  `tests/terminal.e2e.ts`: 11 passed.
- `scripts/dev ci`: green; 1149 tests run, 1149 passed, 16 skipped (before the review fixes,
  which change only web JS and e2e).
- Independent review in a fresh session (om-code-review): approved `456ac7c` with two minor
  findings, both fixed test-first (the margin click; the read-only test now proves nothing is sent).
- Not tested here: a real Claude Code session in a real browser. The mock reproduces Claude Code's
  mouse modes (1000 + 1006 on the alternate screen) and its click-to-toggle.
