---
yulius-task: bb4f1f3d-0924-4f70-8119-47f69e4940c0
status: in-progress
---

# SPEC-159: the step in progress shows on the task's progress bar in its own colour

Status: in-progress
Date: 2026-10-15
Owner: Yulius task #150
Scope authorization: the person's request (task #150, screenshot of the step strip of task #148
reading "Step 2/4" beside a bar filled to a quarter): "currently running task should be recorded
in progress in different color. 2/4 and only 1/4 colored seems weird".

## Goal

- The step strip's progress bar shows the step being worked on: the finished share in the bar's
  colour as today, followed by the share of the step(s) in progress in a different colour, so
  "Step 2/4" no longer sits beside a bar that looks a quarter done with nothing else on it.
- The finished percentage (`percent`, STP-006) does not change meaning: a step in progress is not
  counted as done.

## Context

- The strip (`crates/yulius-server/web/assets/taskview.js`, `renderStrip`) labels the step in
  progress "Step N/M" (N = its position) and fills `.step-progress` to `steps.percent`, the
  finished share of the plan's effort (`yulius_domain::steps::progress`, served by
  `yulius_app::discipline::progress_view` as `StepProgress`). With one of four steps done and the
  second in progress the label reads 2/4 and the bar 25 %.
- `StepProgress` (`yulius-contracts/src/api/tasks.rs`) carries `done`, `total`, `current` and
  `percent`; it does not say how much effort is in progress. The web mock (`mock.js`,
  `stepsSummary`) mirrors the server's computation.
- The UI's colour for running work is `--ok` ("Working" tone, `.tone-working`; Home's working
  count): the green ring beside the bar in the screenshot. The finished fill is `--series-1`.

## Requirements

| ID | Requirement | Evidence |
| --- | --- | --- |
| PIP-001 | `Progress` (domain) counts the effort of steps in progress; `active_percent()` is their share of the plan's effort, computed so that `percent + active_percent` is the rounded-down share of finished plus in-progress effort (never over 100); 0 with no plan or no step in progress. Blocked, pending, done and skipped steps are not in progress. | domain unit tests `active_percent_*` |
| PIP-002 | The API's step summary carries `active_percent` (0-100, default 0 for older servers) beside `percent`. | Rust `progress_view` unit test; `docs/api.md` |
| PIP-003 | The step strip's bar shows a second segment right after the finished fill, `active_percent` wide, in the working colour (`--ok`); it is hidden when nothing is in progress, and the finished fill then keeps its rounded end. The bar's accessible name says the in-progress share: "1 of 4 steps done, 25% of the plan, 25% in progress"; without a step in progress it stays "N of M steps done, P% of the plan". | e2e `steps.e2e.ts` |
| PIP-004 | The web mock computes `active_percent` with the server's rule. | e2e `steps.e2e.ts` runs on the mock |

## Plan

- [x] Read the product context and related work, then write and validate the spec
- [x] The step summary reports the effort share of the steps in progress
- [x] The step strip draws the step in progress in the working colour
- [x] Independent review in a fresh session, findings fixed test-first
- [ ] Gates green and the review approves the final head
- [ ] Docs, gates and delivery

## Acceptance criteria

- A plan of four equal steps, one done and one in progress, serves `percent: 25,
  active_percent: 25`; the strip shows 25 % in the bar colour then 25 % in green.
- The mock's task #12 (2 of 7 steps done, the open step of effort 3) shows 22 % finished and 33 %
  in progress, and its bar is named "2 of 7 steps done, 22% of the plan, 33% in progress".
- A plan with no step in progress serves `active_percent: 0` and shows no second segment.

## Test plan

- Domain unit tests in `crates/yulius-domain/src/steps.rs`: in-progress effort counted, rounding
  keeps the sum at the cumulative floor, blocked steps excluded, empty plan 0.
- App unit test of `progress_view` for the served field.
- e2e `e2e/tests/steps.e2e.ts`: the segment's width share and the accessible name on the mock.

## Decisions

- The server computes the share (not the browser from the step list): the strip renders from the
  task summary, which arrives before the step list and in every task event, and the effort
  weighting already lives in the domain. A new optional field keeps older clients working.
- The in-progress colour is the working colour `--ok`, the colour of the "Working" indicator
  beside the bar (UI-050: one colour per state). A lighter shade of the finished colour was
  rejected: the request asks for a different colour, and a 50 % tint of `--series-1` is hard to
  tell from the 20 % track. No ADR: a presentational choice recorded here.
- The segment keeps the working colour while the task is paused or stopped with a step still
  marked in progress: the bar records the plan's state (which step is open), the status chip beside
  it says whether the agent is working. Tinting it by the session's state would tie two
  independent signals together (review round 1 nit, declined).
- `active_percent` is a difference of cumulative floors so the two segments never exceed the bar
  and the right edge of the second segment matches "finished + in progress" exactly.
