---
yulius-task: b36dec15-4a5b-4969-be67-b6a23d2721dd
status: done
---

# SPEC-052: queue messages into a task that has not started

Status: done
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
- [x] A task that has not started takes queued messages into its request through the API
- [x] The agent gets the queued messages when the task starts
- [x] The task view of a waiting task shows its request and a box to queue a message
- [x] Specs, ADR, API and product docs describe queued messages
- [x] Independent review in a fresh session, findings fixed test-first
- [x] Every gate green, docs and the changes log updated

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

- **The message joins the request; it gets no table of its own** (ADR-0035). Everything that
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
- **"See the original prompt" (SPEC-043) shows the additions.** It shows the prompt a task
  started with, and a task given queued messages started with them, each under its "Added by …"
  line.
- **Same limit as creation (64 KiB for the whole request).** One bound for the request, whatever
  path built it.

## Evidence

- `scripts/dev ci` exit 0 on b9dbcb2 (the head before this section; this section only adds text):
  doctor, repository check, script tests, `cargo check`, `cargo fmt --check`, clippy
  `-D warnings`, `cargo nextest run --profile ci` **1225 passed, 16 skipped**, doctests,
  `cargo deny`, `cargo machete`, `cargo doc`.
- e2e (full suite, `npx e2e run`): **199 passed, 1 skipped** (36 files).
- Tests that fail without the change:
  - `queued_messages.rs::a_task_that_has_started_refuses_a_message` was red, before the app's own
    started check existed, with the SQL's attempt and session guards removed. Since the review
    fixes, the app refuses those cases before the SQL runs.
  - The store test now guards the SQL itself:
    `store.rs::a_prompt_grows_only_while_the_task_has_not_started_and_within_the_bound`. It is red
    without the byte-length clause, and red without the provider-session clause. It also covers
    the status guard and the attempt guard.
  - `queued_messages.rs::a_started_task_with_a_long_request_says_it_has_started`: red (422) before
    the started check moved ahead of the size check.
  - e2e `queued-messages.e2e.ts`: the first two tests were red (no card) before the UI. `once the
    task has started, a pause for capacity brings no card back` was red before the detail was
    read again on a status change.
  - `task_done.rs::a_message_queued_before_the_start_reaches_the_agent` (fake agent shaped like
    Claude Code): the goal task's request file and `/goal`, and the prompt task's first message,
    carry the message. It needs no change of its own beyond the API: the start already reads
    the request.
- Seen in the browser (mock, 1280×800): the card at the bottom of the terminal, with the request
  and the box.
- Independent review (fresh read-only session, `om-code-review`):
  - Round 1, on 0baf4bf: REQUEST CHANGES. One major: the card could come back over a live
    session paused for capacity. Minors: the untested SQL bound, 422 instead of 409 for a started
    task, the changes log and the plan, and mode/title unchanged by a message. Two nits.
  - All were fixed test-first in 0ad35e2 and 380793f, and the docs in the commits after.
  - Round 2, on 7f15213: APPROVE with minors.
    - This section overclaimed what the route test proves about the SQL: corrected above.
    - The provider-session guard had no test: added to the store test, red without it.
    - Two doc nits: fixed.
    - `loadDetail` keeps whichever response lands last. That predates this change and is filed as
      a follow-up issue.
  - Round 3, on 2638651: APPROVE with two nits (wording), left as they are.
- Merging `dev` (the original prompt view, SPEC-043; ready-made automations) conflicted in the mock
  and the e2e README, and both sides were kept. ADR numbers 0033 and 0034 were taken, so this
  decision is ADR-0035. On the merged head, `scripts/dev ci` exit 0: **1237 passed, 16 skipped**.
- The full e2e suite on the merged head failed two of this change's tests, each time in a
  different way. Each failure was traced to a cause and fixed, never retried:
  - The mock's ambient activity starts the queued studio #8 after about 12 s, which a slow run
    raced. The queueing tests now use the chained product #7, which nothing starts.
  - A real bug: the terminal's deferred focus landed after its late first screen and took the keys
    a person was typing into the queue box. It now yields to a text field being typed in. This is
    reproduced deterministically by holding the mock terminal's first screen: `the terminal
    showing late does not take the keys typed into the queue box`. It was red before the fix.
    The terminal, shell panel, Cockpit, navigation, notes and shortcuts suites stay green
    (48 passed).
  - The other failures in those full runs were outside this change and passed alone:
    `terminal.e2e.ts` file paths open the editor (2 of 2), and `menus.e2e.ts` slash menu.
  - After the fixes, on 5531f1d: the full e2e suite has **203 passed, 1 skipped** (38 files), and
    `scripts/dev check` passes. Since the merged head's `scripts/dev ci`, only the web UI, the
    e2e tests and the docs changed.
- `dev` was merged again at f59d6f6, cleanly. It brought the Agent panel in place of the
  Cockpit and task status from Claude Code's own records; no ADR number collides with 0035.
  - On that head, `scripts/dev ci`: **1257 passed, 1 failed, 16 skipped**. The failure,
    `cockpit::a_resume_that_fails_at_once_starts_fresh_the_next_time` (20.8 s), passes 3 of 3
    alone.
  - Full e2e: **217 passed, 2 failed, 1 skipped**. The failures are `new-task` "Auto is the
    default…" and `terminal` "file paths … open the editor"; both files pass 2 of 2 alone.
  - None of the three touches this change, and the terminal one also failed before the focus
    fix. They are filed as #320; the umbrella issues are #282 and #283. Yulius CI on the exact
    head is the gate the merge waits for.
- The fresh review of 7557620 (the merges and fixes after round 3) requested changes.
  - Major: yielding to "a text field" counted xterm's hidden textarea as one. A task terminal
    that showed while a Shell was being opened kept the keyboard, so shell commands went to the
    agent.
  - The next attempt followed the rule the reviewer suggested, the Agent panel's `stayed`:
    remember where the keyboard was when focus was asked for. The review of c7c121f found that
    this broke the Shell panel. `activate()` asks for focus and then rebuilds the tab strip, so a
    tab whose terminal had not opened yet no longer took the keyboard. The same happened to the
    neighbour shown when the shown tab closed.
- Final rule (`typingElsewhere` in `terminal-core.js`): a terminal shown late yields only to a text
  field being typed in that is not a terminal's own input. Buttons, tabs and other terminals do
  not hold it back, as on `dev`, so the terminal asked for last wins.
  - New e2e tests:
    - `terminals.e2e.ts`: a Shell tab not opened yet takes the keyboard when chosen; closing the
      shown tab gives it to the neighbour. Both were red on c7c121f.
    - `queued-messages.e2e.ts`: a Shell opened while the task terminal loads gets the keyboard
      when it shows after it. It was red on 7557620.
    - The queue-box test is red with `dev`'s `terminal-core.js` and green with the rule.
  - Focus suites (queued-messages, terminal, terminals, agent-panel, agent, notes, navigation,
    shortcuts, editor): **68 passed, 1 skipped**.
  - The other order is unchanged from `dev`: a task terminal that shows after a Shell already
    took the keyboard still takes it. That is filed as #323.
  - The reviewer's `@path` nit is now a paragraph in ADR-0035.
- `dev` was merged again (c617194): Agent panel, two-factor vault, dead-code tools (`cargo-shear`
  is now a pinned tool). On c617194, `scripts/dev ci` exit 0 (**1258 passed, 16 skipped**) and
  full e2e **220 passed, 1 skipped**. The Rust code has not changed since.
- That head goes to a fresh review of its own.
- Not covered here: a real Claude Code session (the fake agent stands in, as in SPEC-011 and
  SPEC-030). Read access hiding the card is checked by the server (403) and the shared `allows()`
  test, not by an e2e case.

