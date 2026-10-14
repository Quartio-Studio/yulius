---
yulius-task: 2326c46c-23b2-4adc-8c3a-3e245f15d44f
status: in-progress
---

# SPEC-093: the automation's start choice sits beside Run now and Save

Status: in progress
Date: 2026-10-14
Owner: Yulius task #78
Scope authorization: user request (task #78), with a screenshot of the editor's action bar
(Delete, Run now, Save): "put the automation's start choice (Start now, Supertask, After a task…,
When a rule is true…) in the action bar at the bottom, next to Delete, Run now and Save. The
person should see and change how the automation's tasks start right where they run or save it."

## Goal

The automation editor shows how its tasks start as one chip in the action bar, beside Delete,
Run now and Save, so the person finds it where they run or save the automation. The chip looks
and works like New task's start chip and opens the same menu, and Run now starts its task the way
the chip shows.

## Context

- SPEC-078 (task #70) gave automations a start choice (AUT-013..019) and put it in the editor as
  a **Start** row of radio chips inside "The task" (`startRow()` in
  `crates/yulius-server/web/assets/pages/automationedit.js`). The person did not find it there.
- The editor's action bar is the header's title row (`gt-title-row`): the name and the schedule
  sentence on the left, then Delete, Run now and Save (Create automation for a new one). Below
  768 px the row wraps.
- New task's start chip (`afterChip`, `openAfterMenu` and `openRuleDialog` in `taskview.js`) is a
  `chip-btn` that reads the choice ("Start now", "Supertask", "After #12", "When …") and opens a
  menu: Start now, Supertask, a filterable "After a task is done" list of the project's live tasks
  (`chainCandidates`, SPEC-038) and "When a rule is true…" (the rule editor of SPEC-022), offered
  only when the project has a task a rule can name. Its state is the composer's draft (a rule as
  conditions); the editor keeps a rule as text (`ruleText` / `ruleFromText`).
- `POST /api/automations/{id}/run` (Run now) takes no body and runs with the stored choice
  (`run_automation_now` → `process_slot`, which reads the automation again from the store).
  Agents run it with `automation_run_now`.

## Requirements

| ID | Requirement | Evidence |
| --- | --- | --- |
| AUT-020 | The editor's action bar shows the start choice as a chip before Run now (before Create automation for a new automation). It reads the choice ("Start now", "Supertask", "After #12", "When #12 done") and opens New task's start menu: Start now, Supertask, the filterable "After a task is done" list and "When a rule is true…", which opens New task's rule editor. The menu is one module shared by New task and the editor. | e2e `automations.e2e.ts` |
| AUT-021 | The Start row in "The task" is gone: the chip is the only start control. Its tooltip says what the current choice means (the one-line help of the old row), and the header sentence still names a choice other than Start now. | e2e `automations.e2e.ts` |
| AUT-022 | Save stores the chip's choice and a reload shows it again (AUT-013). | e2e `automations.e2e.ts` |
| AUT-023 | Run now creates its task with the choice the chip shows, saved or not: the editor sends it with the run, and the server checks it as a save does (AUT-014; the stored choice sent back is not re-checked) and uses it for that run only, storing nothing. A run request without a body uses the stored choice, as scheduled runs and `automation_run_now` always do. | int `automation_start.rs`, e2e `automations.e2e.ts` |
| AUT-024 | On a 390 px screen the action bar wraps with no overlapping controls and no horizontal scroll. A read-only person sees the chip, disabled. | e2e `automations.e2e.ts` |
| AUT-025 | SPEC-078, the API doc, an ADR and `docs/changes.md` describe the chip and the Run now body. | review |

## Plan

- [x] Spec SPEC-093 written and validated
- [ ] Run now runs with the start choice the editor shows (API body, tested)
- [ ] New task's start menu shared as one module
- [ ] The start chip in the automation action bar replaces the Start row
- [ ] The action bar wraps on a phone and is disabled read-only (e2e)
- [ ] SPEC-078, API doc, ADR and changes describe the chip
- [ ] Independent review in a fresh session, findings fixed
- [ ] Every gate green, merged into dev

## Acceptance criteria

- AC-AUT-20: on `aut_1` (mock) the action bar has a "Start: Start now" chip between Delete and
  Run now; its menu offers Start now, Supertask, the project's live tasks under "After a task is
  done" and "When a rule is true…"; picking each of the four makes the chip read it and Save
  stores it; no WCAG violations with the menu open.
- AC-AUT-21: "The task" has no Start radio group; the chip's tooltip is the help line of the
  choice ("Each task starts as soon as a session slot is free." for Start now).
- AC-AUT-22: after Save and a reload the chip reads the saved choice.
- AC-AUT-23: with Supertask picked and not saved, Run now creates a supertask while the stored
  automation still says start now; through the API, `POST /run` with a body `{"supertask":true}`
  creates a supertask and leaves the stored choice unchanged, an invalid body (supertask with a
  rule, a missing task) answers 422 and records no run, and a request without a body uses the
  stored choice.
- AC-AUT-24: at 390 px the editor's header controls do not overlap and the page does not scroll
  sideways; with read access the chip is disabled.
- `scripts/dev ci` and the e2e suite pass.

## Test plan

- `crates/yulius/tests/automation_start.rs`: AC-AUT-23 through the HTTP router (sessions capped
  at 0 so created tasks stay waiting), red before the API change.
- `e2e/tests/automations.e2e.ts` (mock server): AC-AUT-20..24, replacing the Start row test of
  SPEC-078; the mock's Run now creates a task with the choice it is given.
- The New task suites that use the start chip (`intake`, `drafts`, `supertask`, `menus`) keep
  passing unchanged after the menu moves to its own module.

## Decisions

- **The chip sits before Run now, in the composer's chip style.** The request asks for it in the
  action bar before Run now and for New task's look; a `chip-btn` with a down arrow says "this is
  a choice" and keeps it apart from the action buttons. Its accessible name is "Start: <choice>"
  (New task's chip says "Run after: <choice>").
- **The help line becomes the chip's tooltip.** A sentence under a wrapping button row would
  move about as the bar wraps; the tooltip is close to the chip, and the header sentence already
  names the choice in words.
- **One menu module, `startchoice.js`.** New task and the editor pass their current choice and get
  callbacks; each keeps its own state (the composer's draft, the editor's form), and each opens
  the shared rule editor with its own value. Copying the menu would let the two drift.
- **Run now sends the shown choice; the server uses it for that run.** Alternatives: save before
  running (silently stores every other unsaved edit too) or refuse Run now while unsaved (the
  request asks for the opposite). An optional body on `POST /run` keeps old clients and the agent
  tool on the stored choice. Recorded in ADR-0074.
- **The chip is a menu, not four radios.** New task's menu already filters long task lists and
  holds the rule entry; four chips in the action bar would not fit a phone.

## Evidence

To be filled in when the work is done.
