---
yulius-task: b6ad51d9-e971-458e-8fbe-df658740b02b
status: in-progress
---

# SPEC-027: a click on Claude Code's output reaches Claude Code

Status: in-progress
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
| CLK-001 | A writer's plain primary click (no drag, not on a link, no modifier, not the second click of a double-click) on a full-screen app that asked for mouse reporting reaches the app at once as a press and a release report in the app's encoding (SGR when it asked for 1006, X10 otherwise), at the clicked cell. | `terminal.e2e.ts` "a click on Claude Code's output expands it" |
| CLK-002 | Dragging and double-clicking still select text; the click does not take control by itself (it is sent like a wheel turn); taking the size by a click is unchanged. | same, existing size test |
| CLK-003 | Read-only viewers, the normal screen and a viewer that handed the mouse to the app (xterm reports itself) send nothing extra. | same (read-only), code path |

## Plan

- [x] Reproduce the defect and find its root cause
- [ ] Write the fix spec and the regression test that fails
- [ ] Fix the cause until the regression test and every gate pass
- [ ] Independent review in a fresh session, findings fixed test-first

## Acceptance criteria

- In the mock task terminal (full screen, mouse reporting on), a click on "Searched for 2
  patterns, read 1 file (click to expand)" shows the group's lines; a second click collapses it.
- A drag across a row still selects its text and the group does not toggle.
- A read-only viewer's click does not reach the app.

## Test plan

- e2e (`e2e/tests/terminal.e2e.ts`, mock): click expands, click collapses, drag selects without
  toggling, read-only click sends nothing. The mock session (`mock.js`) expands a collapsed group on
  an SGR press of button 0 at its row, as Claude Code does.
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
- No ADR: this follows the wheel decision of #34/#79 inside UI-019.
