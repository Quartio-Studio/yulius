---
yulius-task: 6cb40d53-d461-49df-b09b-acbed616712b
status: in-progress
---

# SPEC-376: The Superagent request tests click "Open" on a toast that stays put (#978)

Status: in-progress
Date: 2026-11-01
Owner: Yulius task #356
Scope authorization: the person's request (task #356): fix GitHub issue #978 in one pull request
into `dev` saying `Fixes #978`. Create no new GitHub issue; comment on #978 instead.

## Goal

`e2e/tests/superagent-requests.e2e.ts` › "a request shows in full above the Superagent and Create
task starts it in that sandbox" opens the created task from its toast on any machine, under any
load: no other toast can move the "Open" button while the test clicks it.

## Context

- **Symptom (#978, task #338, 2026-10-31).** One full `npx e2e run` beside `scripts/dev ci`:
  after `.toast:has-text("in product") button:has-text("Open")` was clicked, `toHaveURL`
  observed `#/s/studio/tasks`. 30/30 passes of the file alone right after.
- **Root cause.** The toast is appended once and never re-rendered (`dom.js` `toast()`); what
  moves its button is a second toast. The mock's `startAmbient` sends the sandbox browser's
  sign-in request (`browser.action_requested`, sb_studio) 5 s after the start, and the app shows
  "#10 … needs your action in the studio browser" (`app.js`, 15 s, "Open tab"). The toast column
  is fixed to the bottom right (`styles.css` `.toasts`) and new toasts are appended below, so the
  "Created #N in product." toast moves up by one toast + 8 px. The test opened
  `mock('/s/studio/tasks')` without `&browserask=0`; on a loaded machine its steps before the
  click take ~5 s, so the request can land between the click's mousedown and mouseup. The mouseup
  is then over another element, Chromium fires no `click` on the button, and Playwright's
  `click()` still succeeds.
- **Reproduced deterministically** (temporary probe, not committed): the test's steps up to the
  toast, then `browser.mouse.down()` on "Open", the exact `startAmbient` event pushed to the
  page's event sockets, the sign-in toast visible, `browser.mouse.up()`. The button moved from
  y 867 to y 767 and the page stayed on `#/s/studio/tasks` (the #978 failure). The same sequence
  without the event passes.
- **Why the tests missed it.** Nothing in the file runs past 5 s on an idle machine, and the
  pointer-under-toast fix of SPEC-350 / ADR-0237 covers toasts appearing under a still pointer,
  not a toast moving under a pressed one.
- **Precedent.** The same request broke `notes.e2e.ts`, `secrets.e2e.ts`, `cd.e2e.ts` and
  `toasts.e2e.ts`, each fixed by opening the mock with `&browserask=0` (#349, SPEC-236, SPEC-305).
- **What else the cause could break.** Any test in this file that clicks a toast or a control
  under the toast column after ~5 s; none needs the sign-in request.

## Requirements

- **SRT-001** Every test in `superagent-requests.e2e.ts` opens the mock with `&browserask=0`:
  the sandbox browser's sign-in request never arrives during them.
- **SRT-002** The mock's 5 s sign-in request is one function, `__yuliusMock.askBrowserSignIn()`,
  which the timer calls and which sends nothing under `browserask=0`; a test can fire it at the
  moment it races.
- **SRT-003** A regression case presses "Open" on the "Created #N in product." toast, fires
  `askBrowserSignIn()` while the button is held, and releases: the task opens in `product`.
- **SRT-004** The test finds the toast by "Created #", not "in product", which the error toast
  "Couldn't create the task in product" also contains.

## Plan

- [x] Spec the fix and reproduce the race
- [ ] Regression case for a toast arriving mid-click (red)
- [ ] superagent-requests tests no longer race the ambient toast (green)
- [ ] Independent review approved and findings answered
- [ ] Gates and full e2e suite green with evidence in the spec

## Acceptance criteria

- The regression case fails on dev's options (the click is lost, the URL stays
  `#/s/studio/tasks`) and passes with `&browserask=0`.
- `npx e2e run tests/superagent-requests.e2e.ts` passes, and so does the full suite.
- `task-done.e2e.ts`, which awaits the sign-in request, still passes.

## Test plan

- e2e `superagent-requests.e2e.ts`: the regression case of SRT-003 with real pointer input
  (`browser.mouse`), written first and failing before SRT-001.
- The whole file and the full suite; `task-done.e2e.ts` for the unchanged timer.

## Decisions

- **Fix the test, not the toast stack.** A toast column that holds still while a pointer is pressed
  on it (deferring new toasts and expiries until the release) would protect a person's press too,
  but it changes every toast's timing, needs a release that is never missed (outside the window,
  `pointercancel`, blur) and a bound, and a person who sees the button jump simply presses again.
  The flake is the mock's ambient request reaching a test that does not need it, which four test
  files already turned off the same way. No ADR: this follows the existing `browserask=0` rule.
- **Fire the mock's own request, not a stand-in.** The regression case calls the function the 5 s
  timer runs, so it fails exactly when that timer could reach the test, not on a dummy `.toast`.
