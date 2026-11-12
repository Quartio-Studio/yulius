---
yulius-task: 032d45a8-d265-4e1b-8ead-0fa92eb2e40e
status: in-progress
---

# SPEC-480: A message Yulius types never lands on a person's unsent draft

Status: accepted
Date: 2026-11-12
Owner: Yulius task #463
Scope authorization: the request of task #463 (by MrFishPL, 2026-11-12, started by the
open-issues automation, task #458): fix issue #1199. Amends SPEC-230 TMS-004 and ADR-0178;
decision in [ADR-0309](../../docs/decisions/0309-yulius-types-nothing-onto-a-persons-possible-draft.md).

## Goal

A message Yulius types into a running session (a person's `task_message`, a pull request's
news, "Yulius is back; continue", the MCP watch's texts) is not appended to text a person
typed into the prompt box and did not send, however long ago they typed it, also before a
restart of the server. The sender is told why at once; a message of Yulius's own waits, and
after a bound the person is told.

## Context

- **Symptom (issue #1199).** A person types half a prompt into a task's terminal and leaves it.
  More than a minute later Yulius types a message into that session: the bracketed paste goes in
  behind the person's text and the Enter submits both as one prompt. The person's unfinished
  text is sent without their say, glued to the message.
- **Root cause.** `App::type_into_session` (`crates/yulius-app/src/task_message.rs`), the one
  place every such message goes through, asks about a person only `typed_recently`: a keystroke
  within `policy::TYPED_WINDOW_MS` (60 s; SPEC-230 TMS-004, ADR-0178, "the person's draft
  counts for a minute"). The window was taken from prompt attribution (TSK-013), where a minute
  is right; for a draft it is not, because a draft does not go away by itself.
- **Why the tests missed it.** `a_persons_draft_keeps_the_message_out` types and sends the
  message at once, inside the minute. No test let the minute pass (the tests with real
  processes run on the system clock).
- **What exists to build on.** Since SPEC-468 (CCF-010) Yulius knows when a session's prompt
  box was last empty (`box_emptied_ms`: the run's start, or the last prompt submitted from the
  box, held and blocked prompts handled) and when a person last typed (`typed_at_ms`, every
  keystroke of a viewer, `terminal.rs`). `yulius_domain::agent_cli::is_draft` compares the two.
  `App::has_draft` also counts a message Yulius itself typed until its submit, which is right
  for a restart (the text would be lost) and wrong here: a message whose submit hook never came
  would hold every later one. A task that needs a person is flagged with a reason
  (`flag_attention`, SPEC-472) and told in its terminal (`hub.notice`), as the goal typist does
  for a draft (SPEC-068 TYP-003). The pull-request watch types nothing into a flagged task.
- **What a keystroke does not say.** Yulius sees keys, not the box. A key that left no text (an
  answer to a dialog, Escape, a command Claude Code runs without a prompt hook) reads as a draft
  until the next prompt is submitted from the box. Task #454 saw with Claude Code 2.1.322 that
  the Up arrow takes prompts that wait in Claude Code's queue back into the box: a key without
  a printable character can put text there.
- **Callers on a refusal.** `task_message` returns the `Conflict` to its sender. The
  pull-request watch keeps the notice untold and tries again every 20 s; the MCP watch every
  pass; the outage wake every 30 s until 10 minutes after the start, then the wake is dropped.

## Requirements

- DRF-001 (the rule). `type_into_session` types nothing while a person's draft may sit in the
  session's prompt box: a person typed into the session after its prompt box was last empty
  (its run's start, or the last prompt submitted from the box; SPEC-468 CCF-010), however long
  ago. Only a person's keystrokes count, every one of them; what Yulius typed itself does not.
  The call is refused with `Conflict`, before anything is written to the session.
- DRF-002 (the minute stays). A person who typed in the last minute and has no draft (they just
  sent a prompt) still keeps a message out for that minute, as before.
- DRF-003 (every typist). The rule holds for everything that goes through `type_into_session`:
  `task_message` through a Superagent or a person's own agent, a pull request's news and
  check-ins, "Yulius is back; continue", and the MCP watch's `/mcp reconnect yulius` (a command,
  which typed behind a draft would be submitted as a prompt with it) and its continue text.
- DRF-004 (a sender is told). `task_message` answers the `Conflict` with the reason: a person
  typed into the task's terminal and sent no prompt since, their text may still be in the
  prompt box, and the person is to send or clear it and send a prompt. Nothing is audited and
  the task's `needs_you` stays open, as for every refusal.
- DRF-005 (Yulius's own messages wait). The watches keep what they had to say and try again at
  their own pace, as on every `Conflict`; nothing new is queued. The outage wake still ends 10
  minutes after the start: a session with a draft goes on with its person's next prompt, and
  that person is told at the wake's last try (DRF-006).
- DRF-006 (the bound: the person is told). When a message of Yulius's own (DRF-005) is held by
  a possible draft whose last keystroke is at least 10 minutes old (`policy::DRAFT_TOLD_MS`),
  the task is flagged Needs your action with the reason (Yulius has a message for the session
  and does not type it over text that may still be in the prompt box; send it, or clear it and
  send any prompt) and its terminal gets one line saying the same, once per keystroke: typing
  again starts the 10 minutes anew. A flag that ended while the message still waits (a turn's
  end settles a flagged task) is raised again at the watch's next try. Before the 10 minutes
  Yulius waits silently: the person may still be writing. A message Yulius gives up tells the
  person at its last try whatever the key's age: the wake after an outage, at the last of its
  passes (`Sender::YuliusLastTry`), because no key typed since the restart can be ten minutes
  old before the wakes end. A session a person typed into without leaving a draft is thus not served by a guess:
  its sender is told at once, and its person within 10 minutes of their last key plus one pass
  of the watch that has the message (20 s for a pull request's news).
- DRF-007 (it ends by itself). The next prompt submitted from the session's prompt box, by
  anyone, or a new run ends the hold (CCF-010's knowledge): the watches' next pass types.
  The flag of DRF-006 ends with it, also when that prompt starts no turn (`/compact`, which
  leaves the task's activity alone): only that flag, and only while the task still reads it.
  Yulius never clears or sends a person's text.
- DRF-008 (documents). SPEC-230 TMS-004, ADR-0178, `docs/operations.md`, `docs/architecture.md`,
  `docs/product.md` and the `task_message` tool description say when a message waits for a
  person's draft.
- DRF-009 (a restart of the server does not forget a draft). The keystroke that begins a
  possible draft (a person's first key after the box was last empty) writes its time into the
  run's folder (`person-typed`), before the key is passed on to the session; a prompt
  submitted from the box removes the file, unless a key typed after that submit began a new
  draft. A server that reattaches the run reads it as the person's last keystroke, so
  DRF-001 holds for a draft typed before the restart: the wake after an outage is typed only
  after one. A new run starts in a new folder, without a mark.

## Plan

- [x] Write the spec and the decision (ADR-0309), and report the plan on issue #1199
- [x] Regression tests with real processes fail on dev: a draft older than a minute is submitted with a message
- [x] type_into_session holds every typed message while a person's draft may sit in the prompt box
- [x] A held message of Yulius's own flags the task and says so in its terminal after ten minutes
- [x] SPEC-230, ADR-0178, the guide, the architecture and the tool description say when a message waits
- [ ] Review round 1 answered: a draft from before a restart holds messages, the last wake tells its person, the flag ends with its hold
- [ ] The full gate passes on the final code and the spec carries the evidence

## Acceptance criteria

- AC1 (DRF-001, DRF-004; the issue's first criterion). With real processes: a person types text
  into a running task's prompt box and does not send it; two minutes later, and eleven,
  `task_message` is refused with the draft reason, the agent got neither the person's text nor
  the message, and nothing is audited. After the person sends their text (the agent reads it
  alone) and the minute after their Enter passed, the same message is typed and arrives as a
  prompt of its own. Fails on dev: the agent reads the person's text and the message as one
  prompt.
- AC2 (DRF-003, DRF-005, DRF-006, DRF-007). With real processes: an idle task whose Yulius MCP
  is silent and whose person typed a draft 11 minutes ago gets no `/mcp reconnect yulius`: the
  command is not typed, the task reads Needs your action with the reason of DRF-006 and its
  terminal says so once (a second pass adds no line; a flag that ended is raised again by the
  next pass, without a second line). After the person sends their text the
  next pass types the command, which the agent reads alone. Fails on dev: the watch types the
  command behind the draft.
- AC3 (DRF-001, DRF-002, DRF-006; pure rule). `policy::box_held`: no keystroke holds nothing; a
  keystroke after the box was emptied is a draft at any age; a keystroke before the box was
  emptied holds for a minute only; the person is told from 10 minutes on, not before.
- AC4 (DRF-006). Two minutes after the person's last key a held message of Yulius's own raises
  no flag and no terminal line; `task_message` raises none at eleven minutes either.
- AC5 (DRF-008; the issue's third criterion). `docs/operations.md` and SPEC-230 TMS-004 say when
  a message waits for a person's draft; the gate's document checks pass.
- AC6 (DRF-009; review round 1). With real processes and a restarted server: a draft typed
  before the restart keeps `task_message` out after it, its mark is in the run folder, and the
  person's prompt (read alone by the agent) removes the mark. Fails without the mark: the
  agent reads the draft and the message as one prompt.
- AC7 (DRF-005, DRF-006; review round 1). With real processes and a restarted server: a wake
  held by a draft two minutes old says nothing at a pass that is not the last, and at the last
  pass the task is flagged and its terminal says so; nothing is typed. The person's prompt
  ends the flag.
- AC8 (DRF-007; review round 1). On the test's clock: a task flagged for a held message whose
  person submits `/compact` (a prompt that starts no turn) reads no flag afterwards and is
  Waiting. In AC2 the flag is gone once the person's prompt was read.

## Test plan

- `crates/yulius/tests/task_message.rs`: `a_persons_draft_keeps_the_message_out_however_old`
  (AC1, AC4) on a clock the test can move (`ShiftedClock`, system time plus an offset: the
  server starts minutes behind and is moved forward after the draft was typed, so the hooks'
  times, which are the earlier of the client's and the server's, stay in order).
- The same file: `a_message_of_yulius_waits_for_a_persons_draft_and_tells_them` (AC2, AC4) on
  the same clock, with the MCP watch as the typist: its reconnect is the command of DRF-003,
  and it needs no pull request and no restart. The pull-request watch and the outage wake call
  the same function with the same `Sender::Yulius`.
- `crates/yulius-domain/src/intake.rs`: `a_persons_draft_holds_the_prompt_box_however_old`
  (AC3).
- `crates/yulius/tests/task_message.rs`:
  `a_draft_typed_before_a_restart_still_keeps_the_message_out` (AC6) and
  `the_last_wake_after_an_outage_tells_the_person_of_their_draft` (AC7), both across a real
  restart of the server (a run's last turn is known to a server only from a transcript it read
  again, so the wake cannot be shown on one server).
- `crates/yulius/tests/claude_code_moves.rs`:
  `a_prompt_that_starts_no_turn_ends_the_flag_of_a_held_message` (AC8), on the test's clock
  with the hook sent over the internal listener: the fake agent runs no prompt hook for
  `/compact`, as Claude Code 2.1.292 did not.
- The existing tests of typed messages, the watches and the Claude Code moves stay green
  (`task_message::`, `restart::`, `pr_watch`, `claude_code_moves::`).
- Mutation checks by hand (Evidence): without the draft rule AC1, AC2 and AC3 fail; with the
  bound at 0, with nobody ever told, with a sender's refusal flagging too, and with the person
  told at every pass, one of the two tests fails each time.

## Decisions

- **Never typed over a possible draft; told instead of guessed** (ADR-0309). After any bound
  Yulius could type anyway, which is the defect with a longer window, or keep waiting and say
  so. It keeps waiting: a glued prompt cannot be taken back, a held message can be released by
  one prompt.
- **Every keystroke of a person counts.** No key is known to leave the box empty: the Up arrow
  refills it from Claude Code's queue (seen by task #454), and a rule per key would need its own
  proof per Claude Code version.
- **A dialog on screen does not excuse a keystroke.** Yulius knows a session's own question
  only until its agent works again. Whether a person who declines it is back in the prompt box
  before that was not proven either way, and a wrong guess is the defect.
- **Only a person's keystrokes, not `App::has_draft`.** `has_draft` also counts Yulius's own
  message until its submit hook: a hook that never arrives (a server restart between paste and
  hook) would hold every later message with nobody to send a prompt.
- **The bound counts from the person's last key, not from the message.** It needs no state per
  waiting message (the watches keep theirs), and it says what matters: how long the person has
  been away from that text. A message that meets a draft two hours old flags the task at once.
- **`task_message` raises no flag.** Its sender is told in the answer and is talking to the
  person; "Yulius holds a message" would be untrue, the message was refused.
- **The reconnect command follows the rule.** Typed behind a draft it is not run as a command
  but submitted as part of a prompt.
- **The prompt box is not read from the screen.** Its frame and what is drawn in it without
  being typed differ between Claude Code versions (the issue's verification judged it
  fragile), and a wrong "empty" is the defect.
- **The draft's mark is kept in the run folder** (review round 1). Keystrokes were in memory
  alone, as for SPEC-468, which left the issue's defect for the one typist that types only
  after a restart ("Yulius is back; continue"). One small file per draft, written at its first
  key and removed at the submit: no write per keystroke. After a restart the time known is
  that first key's, so a person may be told earlier than ten minutes after their last key.
  `App::has_draft` reads the same time: a session with a draft from before a restart of the
  server is not restarted on a newer Claude Code either.
- **A message that is given up tells at its last try** (review round 1). A key typed after a
  restart cannot be ten minutes old before the wakes end, so the wake would be dropped with
  nobody told. The flag then outlives the wake; the prompt it asks for is what continues the
  session.
- **The flag ends where the box is known empty** (review round 1). A prompt that starts a turn
  sets the task Working and so ends every flag; one that starts none left this flag standing
  over a reason that was gone, and a flagged task gets no pull request's news. The hook that
  records the emptied box ends it.
- **A flagged task is not idle.** While its person has not sent a prompt, a finished task that
  is flagged does not count as done for the tasks chained after it (`task_is_done`; read in
  the code, not run). That holds for every flag Yulius raises and is accepted here: a flag is
  how a person is told.
- **Limits, not fixed here.** A prompt Claude Code scheduled for itself counts as emptying the
  box (issue #1225) and so ends a hold early. A person's keys in the 300 ms between Yulius's
  paste and its Enter still join the message (ADR-0178). The goal typist and the `/compact`
  typist have rules of their own (SPEC-068 TYP-003, SPEC-085) and do not go through
  `type_into_session`: the `/compact` typist for a full context leaves alone only keys typed
  since it asked (`context_full.rs`), so an older draft gets `/compact` behind it (found by the
  review, older code; reported on issue #1199). A CLI without the prompt hook never reports a
  submit: there a keystroke holds messages until the run ends.
