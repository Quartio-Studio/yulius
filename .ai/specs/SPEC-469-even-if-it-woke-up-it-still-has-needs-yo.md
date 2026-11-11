---
yulius-task: d181cb28-037d-4d1d-a36e-add8b5f5ee61
status: done
---

# SPEC-469: A goal pause's Needs you ends when the session works again

Status: done
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
  nobody typing. Each time Yulius had restarted the session (the limit path: another
  subscription, or the reset) and typed its goal again, which is no person's prompt and ended
  nothing; tasks #446, #450 and #452 were paused for 12 to 30 minutes and then worked on. Claude
  Code also wakes a session whose goal is paused: after two pauses for another cause ("goal
  checks kept finding it unmet this turn") its `<task-notification>` for ended background work
  started a working turn.
- A prompt alone does not show that the session recovered. The other 4 of the 13 were refused
  again: a goal check-in 16 minutes after the pause (task #438's session; Claude Code closed that
  turn without a new notice), a session Yulius had restarted, and twice a person's "continue".
  Model output does show it.
- Why the tests missed it: SPEC-333's tests end when the task reads Needs you. None lets the
  session go on afterwards without a typed prompt.
- What the stale request also held back: while a request is open Yulius types nothing into the
  task, because a person must act first. The working session heard nothing of its pull request (a
  red run, a conflict, the merge; `pr_watch.rs`), was not woken after an outage (`outage.rs`) and
  got no reconnect nudge for a lost MCP bridge (`mcp_watch.rs`).
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
  own, a line of its work or its answer (not an API error, not a line Claude Code writes itself,
  not a sub-agent's). It does not matter what started that work: Claude Code's own wake-up, a
  prompt Yulius typed, a session Yulius restarted, or no prompt anyone sees. The task then reads
  Working like any working task, also when no prompt was seen to start the turn (the pause's flag
  is answered by work, as a notice's is, STA-003), and `task.needs_you.resolved` is audited for
  `system`.
- GPW-002: a turn that starts after the pause and brings no work (the API refuses again, with or
  without a new notice) leaves the request open.
- GPW-003: every other open request stays: the agent's own, "no subscription left", the host
  move's. The request is ended by its action in one store statement, so a request made between a
  read and the write is never the one ended. A task that already read Needs you when the pause
  came keeps that flag through the agent's work.
- GPW-004: the same across a restart of Yulius. A session still paused keeps its request, and
  work that comes later ends it, also for a request an older Yulius left open. Work the session
  did while Yulius was down ends it when the transcript is read again, and a task that reads
  Needs you since before that work follows its session (Working while the turn is open, settled
  once it closed). A flag raised after that work began stays.
- GPW-005: the fake agent replays a session that goes on after its goal pause, when the test says
  so: a woken turn that works, one that dies on the API error again, and one with no visible
  prompt.
- GPW-006: a store failure while the request is ended loses nothing: it is tried again at the
  agent's next work.

## Plan

- [x] Reproduce the defect and find its root cause
- [x] Write the fix spec and the regression test that fails
- [x] Fix the cause until the regression test and every gate pass
- [x] Docs, ADR-0304 and the changes entry true to the fix
- [x] Independent review in a fresh session, findings fixed test-first

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
  and closes with no work and no notice (`WAKE DIED`). With `--paused-wake-hidden` the session
  goes on with a hidden `isMeta` prompt and no prompt hook. `--no-response-line` starts a session
  with the line Claude Code writes itself when it resumes with nothing to answer ("No response
  requested.", the `<synthetic>` model). CLI tests cover the four.
- Integration tests in `crates/yulius/tests/task_done.rs`, each from spent retries (Needs you,
  with the request):
  - Claude Code wakes the session and it works: the request is gone and the task reads Working
    while the turn runs, then waits with no request; the end is audited.
  - The session goes on with no visible prompt: the request is gone and the task reads Working.
  - Yulius restarts the session (`switch_task_now`) and the API works again in the resumed one:
    the request stays through the restart, the resumed session's own "No response requested."
    and the goal Yulius types, until the session's first answer, and is gone after it.
  - The restarted session works and is paused again: the first request ended with the work
    (audited once), and the second pause asks anew.
  - The store refuses the write that ends the request (a trigger): the request stays and ends at
    the agent's next work.
  - The woken turn dies again: the request stays, with Claude Code's words.
  - The agent's own `needs_you` request, opened before the pause, stays through the session's
    work.
  - Yulius restarts while the session is paused, then Claude Code wakes it: the request ends.
  - Claude Code wakes the session while Yulius is down: after the start the request is gone and
    the task reads Working, then settles when the turn ends.
- Unit tests:
  - `finish`: the transcript's fold says when the agent first worked after the last goal pause
    (a prompt alone, a sub-agent's answer, work without a time and work before the pause do not
    count; the session's answer ends the pause's last word); only a flag older than that work is
    lifted.
  - `yulius-store`: a request ends by its action only while it is the open one, its wait is
    measured as answered, and a row that is not JSON is left alone.
  - `yulius-harness`: Claude Code's own "No response requested." is neither work nor an answer.
- Mutation checks: each part of the fix is removed in turn and the integration tests are run;
  each mutation must fail a test (Evidence lists them).

## Decisions

- Model output ends the request, not a prompt: a prompt that only repeats the refused request
  would hide what the person still has to do (GPW-002).
- Only the request Yulius opened for the pause ends this way; it is told by its action text, as
  "no subscription left" is (SPEC-447).
- The task follows its session after a restart only when it reads Needs you since before the
  agent's work: the pause raised that flag, or the wake-up would have lifted it anyway (a prompt
  lifts any flag). A flag raised after the work began was raised by something the session did
  later, and no prompt is known to have followed it.
- A session Yulius restarts keeps the request until it answers: the restart alone does not show
  that the API works again. Task #449's move past a usage limit is a separate, earlier exit.
- [ADR-0304](../../docs/decisions/0304-a-request-yulius-opens-for-a-goal-pause-ends-when-the-agent-works-again.md):
  a request Yulius opens for a state ends when that state does.

## Evidence

- **Reproduction.** The real records are from the transcripts on this host (2026-11-11): 13
  pauses "Goal paused · the request was rate limited". 9 were followed by the session's own work
  with nobody typing, each after Yulius had restarted the session and typed its goal again (tasks
  #446, #450, #452 among them); 4 were refused again (task #438's goal check-in without a new
  notice, a restarted session, two typed "continue"). On the tests-only commit `03c9330a`, four of the six tests
  failed and the two guards passed:
  `a_session_claude_code_wakes_after_its_goal_pause_no_longer_needs_you` and
  `a_session_woken_after_a_restart_no_longer_needs_you` ("timed out waiting for the request to end
  with the agent's work"), `a_session_yulius_restarts_after_its_goal_pause_no_longer_needs_you`
  ("… with the resumed session's answer"),
  `a_session_woken_while_yulius_was_down_no_longer_needs_you` ("… at the start"). Issue #1183.
- **Fix.** `a82866c4` (the fix, with the unprompted-continuation test), `9a7ff38d` (the store
  failure test) and `d3812b8a` (the resumed session's own line, and a pause after the work). The
  nine integration tests pass, with SPEC-333's six beside them:
  `cargo nextest run -p yulius -E 'binary_id(yulius::main) & (test(/no_longer_needs_you/) |
  test(/still_needs_you/) | test(/the_agents_own_request_stays/) |
  test(/a_request_the_store_could_not_end/) | test(/a_session_paused_again/) | test(/goal_retries/)
  | test(/goal_check_retries/) | test(/claude_code_retries/) | test(/a_restart_while_claude_code/)
  | test(/a_goal_cleared_while/))'`.
  - The first version of the last test expected a restarted session that is refused again to
    keep the request it had. It failed: the fake's failing turn makes a tool call before the
    connection drops, which is work, so the request ended and the second pause asked anew. The
    test now says that (`a_session_paused_again_after_it_worked_asks_again`); a refusal with no
    work at all is `a_woken_turn_the_api_refuses_again_still_needs_you`.
- **Mutation checks** (each on a clean tree, restored from `HEAD` afterwards; the integration
  tests that existed then, seven or eight, run each time). Every mutation fails a test:

  | Removed | Failing test |
  | --- | --- |
  | The agent's work line ends the request (live) | `…claude_code_wakes…`, `…goes_on_unprompted…`, `…woken_after_a_restart…` |
  | The session's answer ends the request (live) | `…yulius_restarts…` |
  | An open request is watched again at a run's start | `…woken_after_a_restart…`, `…woken_while_yulius_was_down…` |
  | The pause's flag is answered by work | `…goes_on_unprompted…` ("timed out waiting for Working") |
  | The replay ends the request | `…woken_while_yulius_was_down…` |
  | The replay lets the task follow its session | `…woken_while_yulius_was_down…` ("timed out waiting for Working") |
  | Only the pause's request is ended (any request instead) | `the_agents_own_request_stays…` |
  | A failed write is tried again | `a_request_the_store_could_not_end…` |
- **Unit tests.** `finish::tests::a_replay_says_when_the_agent_first_worked_after_the_last_goal_pause`,
  `finish::tests::only_a_flag_older_than_the_agents_work_is_past`,
  `needs_you::tests::a_request_ends_by_its_action_only_while_it_is_the_open_one` (`yulius-store`),
  `claude::tests` in `yulius-harness` (the resumed session's line), and the fake agent's four CLI
  tests: passed.
- **Gate.** `scripts/dev ci` at `c5aad212` (`dev` merged in at `0203de67`, no conflict) exited 0:
  repository checks, `cargo check`, `fmt`, `clippy` for Linux and macOS, 3698 tests run, 3698
  passed, 16 skipped, doctests, `cargo deny`, `cargo shear`, `cargo doc`. The commit after it
  changes this spec only. Earlier runs at `0203de67` (3696 passed) and `d3812b8a` (3698 passed)
  exited 0 too.
- **Security scan.** `scripts/dev security` exited 0 as disabled: Vidoc is turned off in Yulius
  for this project, so nothing was scanned. That is not a pass.
- **End-to-end suite.** Not run: the change touches no web asset.
- **Independent review** (a fresh read-only session, `om-code-review`) reviews the delivered
  head; a round that asks for changes is listed here with its fixes, and the approving verdict
  is the review summary recorded with the merge.
- **Not tested.**
  - A real Claude Code session: the fake agent replays the recorded shapes.
  - The continuation with no visible prompt has no recorded example after a goal pause. Its
    shape is the one Claude Code uses when it continues a turn a usage limit cut.
  - A database left by an older Yulius (a working task with the request open): the restart tests
    cover the same reading of the transcript, from a state this version wrote.
