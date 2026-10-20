---
yulius-task: 595b9554-1435-4e81-8494-f81cdbb1d65d
status: done
---

# SPEC-240: An automation's task is a goal task, so a finished run is Done

Status: done
Date: 2026-10-20
Owner: task #226
Scope authorization: task #226: "fix, they seem to be wrong because tasks are finished". It came
with two screenshots of task #217 "Work through open issues". All 7 of its steps are done and the
5 tasks it started are Done, yet it shows "NEEDS YOU · waiting 11h 10m", "Your turn" and "The
agent is waiting for your reply". Fixes #719. Changes SPEC-001 TSK-010 (which modes new tasks
get); the Done rule TSK-014 is unchanged.

## Goal

A task started by an automation run gets its goal (`/goal`) like any other new task. When a run
finishes its work, Claude Code records the goal as achieved and the task is Done. This holds for a
run that by design changes no repository file and opens no pull request. Such a run no longer
waits under "Needs you" until a person archives it.

## Context

Confirmed against `dev` at 193b8e81.

- **Symptom.** Task #217 was created by the ready-made automation "Work through open issues". Its
  prompt ends "This run changes no repository file itself, so it opens no pull request." It
  finished all its steps on 2026-10-19 and has read `running (waiting)` ever since. The sidebar
  lists it under Needs you. Its five child tasks #218–#222 read `running (done)`.
- **Root cause.** `create_automation_task` (`crates/yulius-app/src/automations.rs`) creates
  every run's task with `mode: TaskMode::Prompt`, so the session gets no `/goal`. The Done rule
  (TSK-014; `AttentionFacts::group` in `yulius-domain/src/task.rs` and `taskInfo` in `status.js`)
  needs one of two finishes:
  - Claude Code recorded the goal as achieved, which makes activity `done`;
  - a tested or merged pull request with a complete plan.

  A prompt-mode run without a pull request meets neither. Each clean Stop leaves it `waiting`,
  which reads "Your turn", so it needs you, permanently. The child tasks were made by
  `task_create`, in goal mode by default, so they reached Done.
- **History.** Goal mode (fe7b01b1, 2026-10-07) kept prompt mode for automation, import and
  preparation tasks, without giving a reason. Preparation tasks hit this very symptom ("in prompt
  mode a finished preparation sat at 'Your turn' and showed as needing a person", F 2026-10-11,
  task #1). They were switched to goal mode (`onboarding.rs`). Automations were not.
- **Why the tests missed it.** `automations.rs` checks a run's task's origin, title, prompt and
  start choice, never its mode. No test follows an automation task to Done.
- **What else the cause touches.** The previous-active rule (AUT-016) skips a run while the last
  run's task is not closed, stopped or failed. A Done task is still active by that rule, so this
  change does not alter it. Imported-session continuations (`session_docs.rs`) stay in prompt
  mode: they continue a person's conversation, and the person decides when it is finished.

## Requirements

| ID | Requirement |
| --- | --- |
| AGM-001 | Every run of an automation (scheduled, catch-up, Run now; any start choice) creates its task in goal mode. The session gets the task's request file and its `/goal` as TSK-010 describes. The definition of done (SPEC-011) covers a run that changes nothing in the repository: "its result reported". |
| AGM-002 | Nothing else about an automation's task changes: its origin, title, the prompt it is given, workflow steps, start choice and the previous-active rule. |
| AGM-003 | A goal longer than Claude Code's cap (4,000 UTF-16 units) is cut, so the goal must still say how the run ends. Each ready-made automation's first paragraph, which says what the run does and how it ends, stays inside its goal. "Work through open issues" (5.6k characters) says in that paragraph that the run ends with its summary once the tasks are started, does not wait for them, and opens no pull request. Before this change that sentence was its last line, which the cut dropped. |

## Plan

- [x] Reproduce the defect and find its root cause
- [x] Spec and issue for the automation tasks' Done
- [x] Automation runs start in goal mode, test-first
- [x] Docs, ADR and changes entry match the rule
- [x] Independent review answered and gates green
- [x] Merge into dev on green CI

## Acceptance criteria

- A scheduled run's task and a Run now task are `TaskMode::Goal`
  (`automations::an_automation_runs_its_task_as_a_goal`). This test fails on `dev`.
- Every ready-made automation's goal holds its first paragraph, and the open-issues one states its
  finish there (`templates::tests::every_goal_keeps_how_its_run_ends`). This test fails on the
  old template.
- SPEC-001 TSK-010, the architecture document and `docs/changes.md` say that automation tasks are
  goal tasks.

## Test plan

- Regression: `crates/yulius/tests/automations.rs` `an_automation_runs_its_task_as_a_goal`. It runs
  one scheduled automation through `automation_tick` and one through `POST /api/automations/{id}/run`,
  and asserts both tasks are goal tasks. Before the fix it failed with `left: Prompt, right: Goal`.
- Regression: `crates/yulius-app/src/automations/templates.rs` `every_goal_keeps_how_its_run_ends`.
  It builds each template's launch goal with `goal_condition` and a long request path, and checks
  that the first paragraph is inside it. Before the template change it failed on "opens no pull
  request".
- That a goal task's session gets its `/goal` and becomes Done once the goal is achieved is already
  covered for every goal task: supervision tests of the launch goal, and `finish.rs` and
  `AttentionFacts::group` tests of the Done rule. It is not duplicated here.
- Full gate `scripts/dev ci`.

## Decisions

- **Goal mode for automation runs, rather than a new Done rule for prompt-mode tasks.** Considered:
  - Making a prompt-mode task with a complete plan Done when its turn ends. A complete plan
    alone has never been a finish (#139, TSK-014), and an agent may end its turn with a question
    after its last step.
  - An automation-specific finish.
  - A per-automation mode setting.

  Goal mode reuses the one Done rule: Claude Code's goal evaluator judges that the work is
  finished, as it already does for every other task. It matches the API's default (TSK-010) and
  the earlier fix for preparation tasks. Recorded as ADR-0182.
- **The finish goes into the template's first paragraph, rather than lifting the goal's cap or
  pointing the goal at the request file.** The cap is Claude Code's own (SPEC-212). The goal's
  judge reads the condition, so a sentence the cut drops reaches only the agent. Found by the
  independent review. Automations that are already enabled keep their own copy of the prompt
  (AUT-007), so they get the new wording only when it is applied to them.
- **Existing tasks are not migrated.** A prompt-mode task that is already finished (#217) stays
  as it is. A person archives it, or types `/goal` in it. Changing a live session's mode afterwards
  would type a goal into a conversation nobody asked to continue.

## Evidence

- Regression tests failed before their fixes:
  - `cargo nextest run -p yulius -E 'binary_id(yulius::main) & test(/^automations::an_automation_runs/)'`
    failed with `left: Prompt, right: Goal` at eccc9897 (test only).
  - `cargo nextest run -p yulius-app -E 'test(/templates::tests::/)'` failed in
    `every_goal_keeps_how_its_run_ends` on the wording check at 7c1de865 (test only).

  Both pass after their fixes. The automations and automation-start tests passed 23/23, and the
  templates tests 3/3.
- `scripts/dev ci` at 8c5a0fc8 exited 0: repository contracts, check, fmt, clippy, macOS
  compile, deps, and nextest with 2269 passed and 17 skipped, plus the doctests. The run at 988cf149
  failed on `cargo fmt --check` of the new test only. 8c5a0fc8 formats it.
- `scripts/dev security` at 8c5a0fc8 exited 0. Vidoc (vidoc-1) change scan of 3 code files: no open
  critical, high or medium finding in the change, and no low finding listed.
- The independent review ran in a fresh session, over 3 rounds.
  - Round 1 at 53f397f9: request changes. The major finding: the open-issues goal was cut before
    its finish. Fixed test-first in 7c1de865 and 988cf149. A minor (the architecture paragraph's
    placement) and a nit (the stale plan) were fixed too.
  - Round 2 at 988cf149: approve. Two doc nits followed, fixed in 8c5a0fc8.
  - Round 3 at 8c5a0fc8: approve.
- The e2e suite was not run: no web UI file changed. The mock already gives automation tasks
  `mode: 'goal'` (`mock.js`).
- Not tested here: a live automation run through Claude Code's goal evaluator to Done. The goal
  launch and the Done rule are covered for every goal task, and the change only selects goal mode.

