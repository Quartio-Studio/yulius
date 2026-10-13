---
yulius-task: b36dec15-4a5b-4969-be67-b6a23d2721dd
status: in-progress
---

# SPEC-052: queue messages into a task that has not started

Status: in progress
Date: 2026-10-13
Owner: Yulius task #44
Scope authorization: user request (task #44, MrFishPL): "Even if a task is queued i'd like to be
able to queue messages in it. Sometimes i declare a task and I realise it needs to be corrected and
no way to do that atm".

## Goal

A person who created a task can correct it before it starts. While the task waits (queued for a
session slot, queued after another task or a rule, or paused before its first start), its view
shows the request and a box to queue a message. Each queued message is added to the task's
request, so the agent gets it as part of its request when the task starts.

## Context

- A task's request is its stored prompt (`tasks.prompt`). When the task's first session starts, the
  supervisor reads the prompt *after* the compare-and-set to `starting` (`launch_attempt`). It
  writes the request file `.yulius/tasks/<n>.md` and the `/goal` condition, or the first message in
  prompt mode, from that prompt (SPEC-011, SPEC-030).
- The same prompt is the task's scope for the intake check on typed prompts (TSK-013), and it is
  what `task_get` (MCP) and the project export show.
- A waiting task has no session, so its terminal shows only a card ("Queued", "Queued after #N",
  "Paused") with no input. Once the session runs, Claude Code queues what a person types into its
  terminal by itself. Only the time before the first start lacks a way in.
- Messages to agents were withdrawn by SPEC-028 (Yulius's notices, typed wake-ups). Nothing here
  brings them back: nothing is typed into a session, and nothing is stored outside the request.
- Held prompts (TSK-013) gate prompts typed into a running session. They are a different
  mechanism, and nothing here changes them.

## Requirements

| ID | Requirement | Evidence |
| --- | --- | --- |
| QMS-001 | (SPEC-001 TSK-017) `POST /api/tasks/{tid}/queued-messages` `{"text"}` (write access) adds the text to the task's request. It is accepted only while the task has never started: its status is `queued`, `chained` or `waiting_capacity`, and it has no run and no provider session. Otherwise the answer is `409` "The task has started: type into its session instead". | `queued_messages.rs` |
| QMS-002 | The addition is atomic with the start: one conditional update in the store. A message accepted before the task moves to `starting` is part of what that start reads. One that arrives after the move is refused, and is never accepted and then lost. | `queued_messages.rs` |
| QMS-003 | Each message is added under its own line, "**Added by {login} before the task started ({UTC time}):**", after a blank line. The text is trimmed; empty text is `422`. The whole request stays within the creation limit (64 KiB), else `422`. | `request.rs` unit, `queued_messages.rs` |
| QMS-004 | The agent gets the queued messages as part of its request. The request file and the goal condition in goal mode, or the first message in prompt mode, carry them. So do `task_get`, the scope check and the export, since they read the same request. | `task_done.rs` (fake agent) |
| QMS-005 | `GET /api/tasks/{tid}` carries `request` (the whole request text) and `started` (whether the task ever had a run or a provider session). An accepted message publishes the task change. | `queued_messages.rs` |
| QMS-006 | The task view of a waiting task that never started, for a person with write access, shows a "Queued messages" card over the empty terminal. It shows the request as the agent will get it, and a box "Queue a message for the agent…" with **Queue** (⌘/Ctrl+Enter). A queued message clears the box and shows in the request. A refused one keeps the text and says why in place. Read access shows no card, and neither does a task that has started. | e2e `queued-messages.e2e.ts` |

## Plan

- [x] Read the product context and related work, then write and validate the spec
- [ ] A task that has not started takes queued messages into its request through the API
- [ ] The agent gets the queued messages when the task starts
- [ ] The task view of a waiting task shows its request and a box to queue a message
- [ ] Specs, ADR, API and product docs describe queued messages
- [ ] Independent review in a fresh session, findings fixed test-first
- [ ] Every gate green, docs and the changes log updated

## Acceptance criteria

- AC-QMS-1: a queued task, a chained task and a task paused before its first start each take a
  message; the stored request ends with the "Added by … before the task started" line and the
  text. A running task, a stopped task with a run, and an imported task with a provider session
  answer 409. Read access answers 403, and empty or oversized text answers 422.
- AC-QMS-2: a message added after the status moved to `starting` is refused, and the request is
  unchanged.
- AC-QMS-3: with a fake agent, a chained task given a message and then released starts with a
  request file containing the message, in goal mode. In prompt mode the first message contains it.
- AC-QMS-4: in the UI (mock), a queued task shows the card. Queueing shows the message in the
  request and clears the box. A refusal keeps the text and shows the reason. A running task shows
  no card. Read access is refused by the server (403, AC-QMS-1), and the card checks write access
  as the other task actions do.
- `scripts/dev ci` and the e2e suite pass.

## Test plan

- Unit (`yulius-domain` `request.rs`): the addition's format, trimming, the blank-line separation,
  and an empty message refused.
- Integration (`crates/yulius/tests/queued_messages.rs`, new): the route's answers (200, 403, 409,
  422) through the real SQLite store, so the conditional append is exercised in each status with
  and without an attempt or a provider session, after the move to `starting`; the detail's
  `request` and `started`; the change event.
- Integration with the fake agent (`crates/yulius/tests/task_done.rs`): a queued goal task and a
  queued prompt task start carrying the message; a later message is refused.
- e2e (`e2e/tests/queued-messages.e2e.ts`, mock): the card, queueing with the button and
  ⌘/Ctrl+Enter, a refusal, and no card for a running task.

## Decisions

- **The message joins the request; it gets no table of its own** (ADR-0033). Everything that
  reads the request then sees the correction: the request file, the goal, the first message, the
  scope check, `task_get` and the export. A separate list would have to be merged into each of
  them, and one left out would act on the uncorrected task. History stays readable because each
  addition is labelled with who added it and when.
- **Only before the first start.** After that, Claude Code's own input queues typed messages. A
  resumed task never re-reads its request, so a message accepted then would silently not reach
  the agent: refusing is honest. Continuing an ended task keeps its own flow.
- **Append, not edit.** The person asked to queue messages. Appending keeps what was declared and
  what was corrected apart, and needs no conflict handling between two editors. Later lines
  override earlier ones, which a reader (and the agent) understands.
- **No scope check on a queued message.** A correction of a task that has not started is that
  task's scope by definition, and holding it would block the one way to correct the task.
- **The task's mode, workflow and title stay as created.** Auto chose them from the request when
  the task was created (TSK-015). A queued message corrects what the agent does, not how Yulius
  runs the task: a person who wants another mode or workflow cancels it and creates a new one.
- **A started task answers 409 before anything else**, also when its request is full: the session
  is the way in. The size answer (422) is only for a task that can still take a message.
- **Same limit as creation (64 KiB for the whole request).** One bound for the request, whatever
  path built it.
