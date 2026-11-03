---
yulius-task: 5a102cb9-9349-4aac-b6a3-3ee049a49bf0
status: in-progress
---

# SPEC-412: e2e: one named, longer wait for first loads, and the races behind #354, #525 and #510

Status: in-progress
Date: 2026-11-03
Owner: Yulius task #394
Scope authorization: the person's goal (task #394). Fix GitHub issues #354, #525 and #510 in one
pull request into `dev` (`Fixes #354`, `Fixes #525`, `Fixes #510`). The person decided #354's open
question on 2026-11-03: option (a) of its latest "Not actionable yet" comment, one named, longer
wait for first loads only; option (b), accepting about 1 failure in 360 runs, is rejected because
flaky required checks block merges. Every other wait keeps its budget and `retries: 0` stays.
Create no new GitHub issue.

## Goal

A test's first wait for the page it opened has a budget that a starved runner can meet, named
once in `e2e/tests/support.ts` beside `MONACO_LOAD_MS` and `TERMINAL_SHOW_MS`, and the races the
three issues found behind other timeouts are fixed, so these tests fail only for a defect.

## Context

- **#354 (open since SPEC-148).** After `e2e/serve.py` (ADR-0122), about 1 run in 360 at a fifth
  of a CPU per thread still failed a default 5 s wait on a starved first load or read: archive's
  quick Archive (its file's first test), terminal first loads, ownership, editor presence. Later
  comments add `pause.e2e.ts` "the Tasks list row menu…" at load 56 and 190. SPEC-148 and
  ADR-0122 left the remedy, a longer budget for first loads, to a person; this task has that
  decision.
- **What already has a named budget.** `MONACO_LOAD_MS` (60 s, `editorShown`) and, since
  ADR-0251, `TERMINAL_SHOW_MS` (30 s, `terminalShown`), the terminal's first screen. What has
  none: the mock's install, which every `app.open` and `browser.reload` waits for
  (`waitForTheMockOnEveryLoad`, 5 s), and each test's own first look at the page (5 s).
- **#525.** Four tests at load ~45:
  - `rail.e2e.ts` "the tools rail keeps its items inside it…": the mock's sandbox browser asks for
    a sign-in 5 s after the start (`browser.action_requested`), which renames the rail's Browser
    item "Browser, an agent needs your action". A run slower than 5 s reads the new name. A race,
    not a budget: `&browserask=0` leaves the request out (as `notes.e2e.ts` does, E2L-006).
  - `terminal.e2e.ts` "a click beside another viewer's grid…": the rows were empty at the first
    `toHaveText` (5 s): the terminal's first screen, which `terminalShown` covers. With 30 rows,
    the awaited line scrolls out about 21 s after it is printed, so the test holds the transcript
    still as soon as it shows (`holdStill`, as now).
  - `queued-messages.e2e.ts` "a waiting task takes a message…": the first wait after `app.open`.
  - `onboarding.e2e.ts`: the Add sandbox dialog's repository radio did not show within the
    action's 30 s, at load 15 too (task #136). No race was found by task #297; this task looks
    again (Plan).
- **#510.** At load ~78:
  - `consistency.e2e.ts`, `places.e2e.ts` (three tests) and `pipeline.e2e.ts` (three tests):
    first waits after `app.open` (a heading, a closed-task banner, the Superagent's first screen,
    the Dashboard's first state).
  - `notes.e2e.ts`: the races reproduced below; "the tabs show a grip…" and "Team comes first…"
    were fixed by #523 and #703 (`browserask=0`, `ambient=0`).
- **The notes races, reproduced with probe delays** (a 9 s pause in the page where a starved
  runner would be slow, just before the test turns the mock's failure off; 4 workers, no load):
  - "a notepad is read-only until it is loaded…" failed (`toHaveValue` saw ""): a failed first
    load is retried after 1, 2, 4, 8… s (`notes.js` `retryLater`, doubling up to 30 s), so after
    a long failing stretch the next try comes after the test's 5 s.
  - "a save made while a refused one is looked into…" failed (`noteLoads` 3, expected 4): the
    test meant its own blur to be the save refused after the lost answer, but the pad's own retry
    (1 s after the lost answer) was refused first, looked at the server unheld and settled, so
    the blur's save was a plain save and nothing was looked into.
  - "an undo after a lost answer is not Saved…" failed (`noteLoads` never rose): the same retry
    settled the lost save before the test undid it, so the undo was a plain save and the server
    was never checked.
  - "undoing a save whose answer was lost…", "a save whose answer was lost…" and "text kept over
    a reload…" passed with the probe. "a failed reload after a teammate's edit backs off" waits
    for the pad's backed-off reload (its next try at 7 s, then 15 s) within 5 s of turning loads
    back on, which a starved runner can miss the same way.
  - None is a product defect: retrying with growing pauses is the pad's contract (SPEC-041).
- **#354's `pause.e2e.ts` failure.** The row's actions show while the row is hovered; the mock's
  ambient activity regroups the Tasks list under a still pointer, so the hover sticks to another
  row and the action stays hidden for the click's 30 s (the cause #703 found for notes). The
  test does not need the ambient activity: `&ambient=0`.

## Requirements

| ID | Requirement | Evidence |
| --- | --- | --- |
| FLD-001 | `e2e/tests/support.ts` names one budget for a page's first load, `FIRST_LOAD_MS` (30 s), and one helper, `firstShown(locator, text?)`, which waits within it until the locator shows (with `text`, until it has that text). It is for the first look at a page after `app.open` or a reload only; every later wait keeps its budget and `retries: 0` stays. | `support.ts`; `mock-ready.e2e.ts` |
| FLD-002 | `app.open` and `browser.reload` wait for the mock's install within `FIRST_LOAD_MS`, not 5 s. | `support.ts` |
| FLD-003 | A regression test holds a page's first content past 5 s: `firstShown` passes and a default wait for the same thing fails. | `mock-ready.e2e.ts` |
| FLD-004 | The tests the three issues list wait for their first screen with `firstShown`, or with `terminalShown` where it is a task terminal's first screen. | the listed files |
| FLD-005 | The rail test opens the mock without the sandbox browser's sign-in request (`browserask=0`); the Tasks list pause test without ambient activity (`ambient=0`). | `rail.e2e.ts`, `pause.e2e.ts` |
| FLD-006 | The notes tests that turn a failure off then trigger the next load at once (a tab switch, which loads), instead of waiting out a back-off a starved run stretches; tests that need the test's own save to be the refused one hold the server's answer to the pad's look from before the lost answer, so the pad's own retry cannot settle it first. Each passes with the probe delay that failed it before. | `notes.e2e.ts` |

## Plan

- [x] Spec, and the notes races reproduced with probe delays
- [ ] A named first-load wait with its regression test
- [ ] Notes back-off races fixed
- [ ] Rail, pause and terminal races fixed
- [ ] #354's listed tests wait for their first load
- [ ] #510's and #525's listed tests wait for their first load
- [ ] ADR, e2e README, architecture and changes
- [ ] Affected files repeated under load, numbers recorded
- [ ] Independent review findings resolved
- [ ] Gates green

## Acceptance criteria

- The regression test of FLD-003 fails with a 5 s wait and passes with `firstShown`.
- Each notes test of FLD-006 fails with its probe delay before the change and passes with it after.
- The affected files pass repeated runs (`--repeat-each`) next to CPU-bound load holding one
  thread at about 0.2 of a CPU, with the numbers in Evidence and the pull request; no retry, sleep
  or general budget increase.
- `scripts/dev ci` passes; the e2e suite passes in Yulius CI.

## Test plan

- Probe delays (a pause inside the page at the await point) for each race, before and after.
- `npx e2e run <files> --repeat-each N --workers 4` with and without busy loops held so that a
  CPU-bound probe gets about 0.2 of a CPU (SPEC-148's method), recording passes, failures and
  seconds per test.
- `scripts/dev ci`, `scripts/dev security`.

## Decisions

- **One named budget, 30 s, for first loads (the person's decision on #354).** Like the terminal's
  `TERMINAL_SHOW_MS` and the runner's action budget, not Monaco's 60 s: a page's first load is
  modules and in-page mock reads, lighter than Monaco's parse. Recorded as ADR-0262, amending
  ADR-0122.
- **A helper the test calls, not a longer `app.open`.** No single sign says a page's first screen
  is done for every route (some tests hold reads on purpose, `hold=`), so the test names what it
  waits for and the budget covers that one wait. `app.open` itself only gets the budget for the
  mock's install, which is part of every first load.
- **Races are fixed, not covered by the budget.** The rail, pause, terminal margin and notes
  failures have causes a longer wait would hide or not help; each is removed in the test.
- **Notes: trigger, don't wait out the back-off.** A product change (no back-off after a failure
  is cleared) is not possible: the pad cannot know the server is back. A tab switch is what a
  person does and loads at once (E2L-004's approach).

## Evidence

(Filled in as steps finish.)
