# 0182. An automation runs its task as a goal

## Status

Accepted. Specified in [SPEC-240](../../.ai/specs/SPEC-240-fix-they-seem-to-be-wrong-because-tasks.md);
task #226, fixes #719. Changes SPEC-001 TSK-010, which modes new tasks get. Goal mode itself
(fe7b01b1) had kept prompt mode for automation tasks.

## Context

- A task is Done (TSK-014) when one of two things holds:
  - Claude Code recorded its goal as achieved;
  - its plan is complete and its pull request is tested or merged.

  A complete plan alone is not a finish (#139). An agent may end its last turn with a question.
- Automation runs created prompt-mode tasks. Such a task has no goal. An automation run may also
  open no pull request by design: the ready-made "Work through open issues" says "This run changes
  no repository file itself". So a finished run read `waiting` ("Your turn", Needs you) until a
  person archived it (task #217).
- Onboarding preparation tasks had the same symptom (F 2026-10-11, task #1). They were moved to
  goal mode.

## Decision

- `create_automation_task` creates every run's task in goal mode. This covers every run (scheduled,
  catch-up, Run now) and every start choice. The session gets the request file and its `/goal`
  like any new task. The definition of done (SPEC-011) already covers a run that changes nothing
  in the repository ("its result reported").
- Existing prompt-mode automation tasks are not migrated.
- Imported-session continuations stay prompt tasks. A person continues the conversation, and that
  person decides when it is finished.

## Alternatives

- **Done for a prompt-mode task with a complete plan at a clean Stop.** Rejected. It brings back
  "a complete plan is a finish" (#139). It would also hide an agent that ends with a question for
  a person.
- **A mode setting per automation.** Rejected. It adds a choice whose prompt option would only
  bring back this defect for runs without a pull request. A person who wants a conversation starts
  a task.
- **Mark the task Done when the automation's run "completes".** Rejected. Yulius has no signal for
  that apart from the agent's own judgement, which the goal evaluator already gives.

## Consequences

- An automation run is Done once its goal is achieved, so it moves to Done and can be archived in
  one click.
- The goal's Stop hook keeps the agent working until its evaluator finds the request met, as for
  every other goal task. An automation prompt that is impossible to meet makes its goal fail
  rather than wait silently.
- The previous-active rule (AUT-016) is unchanged. A Done run's task is still active until it is
  closed.
