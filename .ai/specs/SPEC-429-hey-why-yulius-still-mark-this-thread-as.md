---
yulius-task: d3fd0a19-6941-4c00-9a8d-a44ff451a2a5
status: done
---

# SPEC-429: an answered turn is not Needs you

Status: implemented
Date: 2026-11-06
Owner: Yulius task #411
Scope authorization: task request: "why yulius still mark this thread as 'needs you' when he doesn't
need anything from me. He did but I answered and he should mark this as done", with a screenshot of
task #17 of another project. Its agent had just answered "Yes, nothing is needed from you right now".
The task read "Your turn" under Needs you (1), and its bar said "The agent is waiting for your reply".

## Goal

A task whose agent ended its turn with nothing left for a person reads as finished ("Answered",
under Done) instead of "Your turn" under Needs you. Needs you stays for what a person has to do.

## Context

- After a clean turn end `Activity::after_clean_turn` gives `done` only when Claude Code recorded a
  `/goal` as met, and `waiting` otherwise (`crates/yulius-domain/src/task.rs`). A typed prompt drops an
  achieved goal (`prompt_submitted` in `crates/yulius-app/src/finish.rs`, `TranscriptFacts::fold`),
  so a follow-up question answered after the goal was met, or any turn of a task without a goal,
  ends as `waiting`.
- `AttentionFacts::group` maps an idle `waiting` agent with no background shell and no
  `yulius wait` to Needs you, and `taskInfo` in `status.js` does the same ("Your turn", group
  `needs`). Needs you therefore means "the agent's turn ended", not "a person has to act". That
  goes against ADR-0068 ("Needs you means a person has to act") and ADR-0111 ("nothing else puts a
  working task in Needs you").
- A person's move already has explicit signals, each of which stays: a permission prompt or a
  question (`needs_attention`), `needs_you` (also used when goal retries are spent), a browser
  request, a held prompt, failing checks once the agent stopped, and a chain that can never start.
- Filed as #1090 by the research half of this task.

## Requirements

| ID | Requirement | Evidence |
| --- | --- | --- |
| ANS-001 | An idle `waiting` task (running, no background shell, no `yulius wait`, no request) whose plan has no open step and whose goal is not unfinished is in the Done group, labelled "Answered" ("answered 12m ago", second line "Agent answered — nothing asked of you"). It is not counted in Needs you. Its notification is an informational "Answered" in the done tone, not a request. | domain unit test; `task_done.rs`; e2e |
| ANS-002 | It still needs a person while its work is visibly unfinished: plan steps not done or skipped, or a goal that Claude Code last recorded as set, not met or failed (`goal_unfinished`). A cleared or met goal is not unfinished. | domain unit test; `finish.rs` unit test |
| ANS-003 | The server (`AttentionFacts::group`) and the web UI (`taskInfo`) apply the same rule: the task view carries `goal_unfinished`, and the plan is read from `steps`. The plan counts after every finished turn, not only once a pull request exists. | domain unit test; e2e `task-done.e2e.ts` |
| ANS-004 | An answered task's attention bar does not say "The agent is waiting for your reply". Typing into it makes it Working, as for any task. | e2e |
| ANS-005 | The `needs_you` tool says that a question asked only in plain text reaches nobody, because a turn that ends with nothing open reads Answered. The briefing has no room left in its budget; the tool description is read where agents choose how to ask. | `mcp.rs` unit test |
| ANS-006 | An answered task is still a running session. It is not Done in the TSK-014 sense: its chains do not start and the queue does not take its slot, because its activity stays `waiting`. | unchanged code path; review |
| ANS-007 | Archive and Delete close an answered task's session, as for a Done task, because the UI offers them for everything under Done (`shows_done`). Before, the server answered 409 "stop the task first". | `task_done.rs` |
| ANS-008 | A goal record read after the turn settled changes `goal_unfinished` and publishes the task, so the UI does not keep "Answered" while the counts say Needs you. | `task_done.rs` (event) |

## Plan

- [x] Read the product context, docs and earlier sessions on the topic
- [x] Investigate the code, data or sources and collect evidence
- [x] Answer with the evidence, the options and a recommendation
- [x] Spec and ADR: an answered turn is not Needs you
- [x] Server rule: an idle turn with no open plan or unfinished goal reads Answered, with tests
- [x] Web UI shows Answered under Done, with e2e coverage
- [x] Docs, briefing line and changes log updated; every gate green
- [x] Review round 1 answered: archive, late goal records, row line, docs
- [x] Review round 2 approved; spec evidence recorded

## Acceptance criteria

- A task without a plan whose agent answered a person and stopped is under Done as "Answered". Home
  and the tabs count it in no group. Its bar has no "waiting for your reply".
- A task with open steps whose agent stopped still reads "Your turn" under Needs you.
- A task whose `/goal` failed, or is set and not met, and whose agent stopped still reads "Your turn".
- Every explicit signal (permission or question prompt, `needs_you`, browser request, held prompt,
  failing checks, blocked chain) still puts a task in Needs you.

## Test plan

- Domain: `AttentionFacts::group` for waiting with and without an open plan, with an unfinished goal,
  and with the explicit signals (red before the change).
- App: `TranscriptFacts` folds `goal_unfinished` from the last goal outcome; a prompt keeps it.
- Integration (`task_done.rs`): the existing "Your turn needs the person" count becomes 0 for an
  answered task without a plan.
- e2e (`task-done.e2e.ts`): the mock's waiting task without steps reads "Answered" in Done and has no
  reply banner; with open steps it reads "Your turn".

## Decisions

- [ADR-0273](../../docs/decisions/0273-an-answered-turn-is-not-needs-you.md): an idle turn needs a
  person only when the work is visibly unfinished (open plan, unfinished goal) or something asks
  explicitly. Rejected alternatives: a question-mark heuristic on the last message (fragile), an LLM
  judge of every turn end (cost and latency), and making the activity `done` (it would start chains
  and give away the session's slot).

## Evidence

- Root cause (verified in code): `Activity::after_clean_turn` gives `waiting` unless a goal is met,
  a typed prompt drops a met goal, and `AttentionFacts::group` / `taskInfo` counted every idle
  `waiting` agent as Needs you.
- Red before green:
  - domain `an_answered_turn_needs_nobody` failed with NeedsYou vs Done;
  - `task_done.rs` failed to archive an answered task (Conflict "stop the task before archiving it");
  - `task_done.rs` got no published update after a failed goal record (timeout);
  - `mcp.rs` `needs_you_says_a_plain_text_question_reaches_nobody` failed;
  - e2e `an agent that answered with nothing open is not your turn` failed on dev's `status.js`,
    and so did its `activityLine` check.
- Run on this branch:
  - `scripts/dev ci` exit 0: 3167 tests passed, 16 skipped, plus lint, docs and the macOS compile,
    at dbe1807f.
  - `cargo nextest` over task_done, delivery, intake, context_full, session_sync, automations,
    agent_hooks, task_message, restart, stages and cd: 248 passed.
  - Full e2e: 858 passed. Two editor tests failed under full-suite load and passed alone (32/32);
    they are the open flaky issue #1054 and touch nothing here.
- `yulius security-scan`: disabled for this project (Vidoc switched off in Yulius); nothing was
  scanned. This is not a pass.
- Independent review in a fresh session: round 1 requested changes (archive/delete refused,
  stale "Answered" after a late goal record, the row's "your turn" line, notification wording, stalls
  without a plan). All were answered in dd9ce858. Round 2 approved dbe1807f, with nits (doc
  comments, table order) fixed here.
- Yulius CI at 70f51794 failed one test of this change. The archive test asserted Closed at once,
  but `close_task_now` keeps the task Running until its session is gone. It now waits for Closed,
  as the sibling archive test does, and passed 3 runs in a row locally.
- Not tested: task #17 itself lives in another project and could not be opened from here. Its case
  (a met goal, then an answered follow-up) is reproduced by `task_done.rs`
  `an_achieved_goal_with_no_shell_left_makes_the_task_done`. Real Claude Code timing was not run;
  the fake agent stands in for it.
