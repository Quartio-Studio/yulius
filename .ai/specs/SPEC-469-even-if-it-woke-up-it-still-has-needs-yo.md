---
yulius-task: d181cb28-037d-4d1d-a36e-add8b5f5ee61
status: in-progress
---

# SPEC-469: A goal pause's Needs you ends when the session works again

Status: in progress
Date: 2026-11-11
Owner: Yulius task #453
Scope authorization: the person's request of 2026-11-11 ("even if it woke up, it still has needs
you status. Fix.", with a screenshot of task #154 of another project on this instance). Issue
[#1183](https://github.com/Quartio-Studio/yulius/issues/1183).

## Goal

A task whose goal Claude Code paused leaves Needs you by itself once its session works again,
whatever woke it. "Send the task a message…" is shown only while nothing goes on.

## Context

- Symptom (the screenshot, 2026-11-11 21:56 UTC): the task's terminal shows a turn 33 minutes
  old, with tool calls and "/goal active (33m)". Its banner still reads "Needs you: Send the task
  a message (for example "continue") to retry once the API works again", with "Goal paused · the
  request was rate limited" and "the task does not go on by itself". The sidebar lists it under
  Needs you, "waiting 38m".
- How the request is made: when Claude Code gives a goal's retries up, or pauses the goal on an
  API error it does not retry, `goal_retries_spent` (`crates/yulius-app/src/goal_retry.rs`) flags
  the task NeedsAttention and opens a request (SPEC-333 AER-002,
  [ADR-0225](../../docs/decisions/0225-claude-code-owns-the-goal-retry-after-an-api-error.md)).
- Root cause: the request has no exit for "the session went on". It ends when a person types a
  prompt (`hook_prompt_submit`: `is_typed_prompt` and `typed_recently`), sends the task a
  message, continues it, or when the agent calls `needs_you {resolved: true}`, which an agent
  that did not open the request never does. A turn that starts otherwise sets the activity to
  Working and leaves the request open, and an open request alone puts the task under Needs you
  (`AttentionFacts::group`, `taskInfo` in `status.js`). That rule is right for a request the
  agent made (SPEC-134: it stays "whatever the agent does meanwhile") and wrong for one that says
  "the task does not go on by itself".
- What wakes a paused session without a person, from the transcripts on this host (2026-11-11):
  of 13 pauses "the request was rate limited", 9 were followed by the session's own work with
  nobody typing. Yulius restarted the session (the limit path: another subscription, or the
  reset; tasks #446, #450 and #452 were paused for 12 to 28 minutes and then worked on), or
  Claude Code woke it with a `<task-notification>` (background work ended, a goal check-in).
- A prompt alone does not show that the session recovered: in task #438's session a goal
  check-in woke the paused session 16 minutes after the pause, the API refused again within a
  second and Claude Code closed the turn without a new notice. Model output does.
- Why the tests missed it: SPEC-333's tests end when the task reads Needs you. None lets the
  session go on afterwards without a typed prompt.
- What else the cause could break: the other requests Yulius opens itself. "No subscription
  left" already ends when the task runs again (SPEC-447 MOV-006, `end_no_subscription_request`).
  The host move's "Continue this task" is on a stopped task, which only a person continues, and
  continuing ends it. No third one exists.
- Parallel work: task #449 (SPEC-464, issue #1172, not merged yet) stops opening this request
  when the pause comes at a usage limit that Yulius handles, and ends it at a move past that
  limit. It adds `end_goal_retry_request` to the same file. This change is the general rule and
  keeps that name, so whichever merges second meets the other in a conflict, not in a silent
  duplicate.

## Requirements

- GPW-001: the request Yulius opened for a goal Claude Code stopped retrying ends once the
  session's agent works after the pause: an assistant record of the session that is the model's
  own (not an API error, not a line Claude Code writes itself, not a sub-agent's). It does not
  matter what started that work: Claude Code's own wake-up, a prompt Yulius typed, or a session
  Yulius restarted. The task then reads Working like any working task, and `task.needs_you.resolved`
  is audited for `system`.
- GPW-002: a turn that starts after the pause and brings no work (the API refuses again, with or
  without a new notice) leaves the request open.
- GPW-003: every other open request stays: the agent's own, "no subscription left", the host
  move's.
- GPW-004: the same across a restart of Yulius. A session still paused keeps its request, and
  work that comes later ends it. Work the session did while Yulius was down ends it when the
  transcript is read again, and a task that still read Needs you from the pause follows its
  session (Working while the turn is open, settled once it closed).
- GPW-005: the fake agent replays a session woken after its goal pause, when the test says so:
  a turn that works, and one that dies on the API error again.

## Plan

- [ ] Reproduce the defect and find its root cause
- [ ] Write the fix spec and the regression test that fails
- [ ] Fix the cause until the regression test and every gate pass
- [ ] Docs, ADR-0304 and the changes entry true to the fix
- [ ] Independent review in a fresh session, findings fixed test-first

## Acceptance criteria

- The new integration tests fail on `dev` (the request stays open while the session works) and
  pass with the fix.
- A woken turn that dies on the API error again leaves the request open, before and after the
  fix.
- SPEC-333's tests pass unchanged.
- `scripts/dev ci` passes.

## Test plan

- Fake agent (`tools/fake-agent`): `--paused-wake-file PATH` wakes the session once PATH exists,
  after `GOAL PAUSED`, as Claude Code wakes an idle session when background work ends (the
  `<task-notification>` prompt and its hook, a tool call, `WOKEN`; the turn ends once
  `--turn-hold-file` exists). With `--paused-wake-dies` the woken turn gets the API error again
  and closes with no work and no notice (`WAKE DIED`). CLI tests cover both.
- Integration tests in `crates/yulius/tests/task_done.rs`, each from spent retries (Needs you,
  with the request):
  - Claude Code wakes the session and it works: the request is gone and the task reads Working
    while the turn runs, then waits with no request.
  - Yulius restarts the session (`switch_task_now`) and the API works again in the resumed one:
    the request stays until the resumed session's first answer, and is gone after it.
  - The woken turn dies again: the request stays, with Claude Code's words.
  - The agent's own `needs_you` request, opened before the pause, stays through the session's
    work.
  - Yulius restarts while the session is paused, then Claude Code wakes it: the request ends.
  - Claude Code wakes the session while Yulius is down: after the start the request is gone and
    the task reads Working, then settles when the turn ends.
- Unit tests (`finish`): the transcript's fold says whether the agent worked after the last goal
  pause (a prompt alone, a sub-agent's answer and a pause after the work do not count).

## Decisions

- Model output ends the request, not a prompt: a prompt that only repeats the refused request
  would hide what the person still has to do (GPW-002).
- Only the request Yulius opened for the pause ends this way; it is told by its action text, as
  "no subscription left" is (SPEC-447).
- [ADR-0304](../../docs/decisions/0304-a-request-yulius-opens-for-a-goal-pause-ends-when-the-agent-works-again.md):
  a request Yulius opens for a state ends when that state does.
