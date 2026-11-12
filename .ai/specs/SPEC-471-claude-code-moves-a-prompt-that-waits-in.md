---
yulius-task: 0f4fe3a4-dc39-4a2f-be7c-6c6ed3323677
status: in-progress
---

# SPEC-471: Claude Code moves: a prompt that waits in Claude Code's queue keeps the session

Status: accepted
Date: 2026-11-12
Owner: Yulius task #454
Scope authorization: the request of task #454 (by MrFishPL, 2026-11-12): fix issue #1204, with
the real order of Claude Code's hooks tested and the facts checked against the real tool before
the design. Amends SPEC-461 CCM-013 and SPEC-468 CCF-011.

## Goal

A session that is restarted on a newer Claude Code at a turn's end (SPEC-461 CCM-013) no longer
loses what waits in Claude Code's queue: a prompt a person entered while a turn ran, or a
message Yulius typed meanwhile. Yulius knows the queue from the records Claude Code writes
about it, leaves a session with a non-empty queue where it is, and moves it at a later turn's
end.

## Context

Claude Code keeps a prompt entered while a turn runs in a queue inside its process. SPEC-468
(task #452) found that the restart of CCM-013 ends the process with that queue and that
`claude --resume` does not take it over (issue #1204): the prompt is lost, and a message's
sender was told "sent". `moves_now` (`yulius_domain::agent_cli`) knows a draft in the prompt
box and the minute after a keystroke, not the queue.

What Claude Code does, seen with the installed 2.1.322 run in a PTY against a local stub of the
model API (hooks that log their time and what the transcript holds at that moment; Evidence
lists the runs):

- A prompt entered while a turn runs gets its `UserPromptSubmit` hook 5 to 15 ms after its
  Enter (a hook that only logs has answered 31 to 43 ms after the Enter, which is what issue
  #1204's "23 to 33 ms" measured). When the hook returned, Claude Code writes a
  `queue-operation` record `enqueue` with the text, also when the hook took seconds. A record
  reaches the transcript file about 100 ms after it was made, so the record is not in the file
  while the prompt's own hook runs.
- In a turn with tool calls a waiting prompt is taken into the running turn at the next tool
  result: a `remove` record (reason `absorbed_mid_turn`, with the text) and an `attachment`
  record `queued_command`; the model reads the prompt inside the tool result. No further hook,
  no turn of its own. Two prompts over two tool calls are taken one by one.
- A prompt entered after the last tool result, or in a turn without tools, waits until the
  Stop that ends the turn returned. Then come one `dequeue` record per prompt and one `user`
  record per prompt (`promptSource: "queued"`), all in one turn, with no further hook. In the
  file the `dequeue` comes before the ended turn's `stop_hook_summary` and `turn_duration`.
- A Stop the hook blocks releases nothing, with or without tools.
- The Up arrow takes the waiting prompts back into the prompt box (`popAll`, with the text).
  Escape interrupts the turn (no Stop hook) and hands the queue on at once.
- With the Stop held and the process ended (SIGTERM), `claude --resume` neither submits the
  waiting prompt nor asks the model: the transcript keeps the `enqueue` alone.
- The binary writes five operations: `enqueue`, `dequeue`, `remove` (reasons
  `absorbed_mid_turn`, `delivered_to_agent`, `dropped_by_hook`, `cleared_on_cancel`), `popAll`
  and `popOne`. The records carry `timestamp` and `sessionId`, no `version` and no
  `isSidechain`; they are in the session's own file.

What this instance's real sessions showed before the design (1,585 transcript files, sub-agents'
included, 415 with queue records; counted by scripts, metadata only; the files change by the
hour, and Evidence has the figures of the watcher's own code over a later set): 5,252 `enqueue`, 3,939 `dequeue`, 1,297 `remove` (1,270
`absorbed_mid_turn`, 26 `delivered_to_agent`, 1 `dropped_by_hook`). Claude Code queues its own
task notifications too (in an idle session an `enqueue` and a `dequeue` 30 ms apart, then a
`user` record with `promptSource: "system"`). The count never goes below zero. Counted over a
whole file with no reset, an entry is still counted at 50 of 4,459 turn ends, in 12 sessions;
every one of them is a task notification, and part of them are restarts (the queue went with
the process).

Where things are in Yulius:

- The move: `claude_code_move_due`, `move_facts`, `move_idle_session`
  (`crates/yulius-app/src/claude_code.rs`), called from `hook_stop` (`hooks.rs`: a Stop the
  gate blocks, or a goal session that works on) and from `settle_clean_turn` (`finish.rs`: an
  idle prompt-mode session). The look reads the turn's lines first (`await_transcript_read`:
  one full round of the watcher, which reads once a second).
- The transcript: `crates/yulius-harness/src/claude.rs` turns a line into `Observed` values,
  `crates/yulius/src/wiring/harness.rs` into `Observation`s, which the watcher
  (`watch_loop` in `crates/yulius-app/src/supervisor.rs`) applies one by one, or folds into
  `TranscriptFacts` for a run it reattaches after a restart of the server. A resumed run
  reads the lines recorded from its own start on (`LinesSince`).
- The tests' fake agent (`tools/fake-agent`) reads a line only after its turn and sends the
  prompt hook then, which is not the real order.

## Requirements

| ID | Requirement |
| --- | --- |
| CCQ-001 | **Yulius knows what waits in Claude Code's queue.** The watcher reads the `queue-operation` records of the run at hand: `enqueue` adds an entry, `dequeue` hands one on to a turn of its own, `remove`, `popAll` and `popOne` take one out without one. The count never goes below zero and an operation Yulius does not know changes nothing. A run starts with an empty queue: a resumed session's earlier lines are not read as the run's, and what the run before left waiting went with its process. A run reattached after a restart of the server knows the same from its own lines; until they were read again its queue is not known, which counts as holding something. |
| CCQ-002 | **A session with something in its queue is not restarted for a newer Claude Code**, at a Stop the gate blocks or a goal session works on, and idle after a clean Stop. Every entry counts: a person's prompt, a message Yulius typed, Claude Code's own notification. The session moves at a later turn's end, once the queue is empty, or at its next start. A prompt taken into the running turn at a tool result is no longer waiting. The queue is asked once the turn's end read the turn's lines. |
| CCQ-003 | **A hand-over is not rest.** From a `dequeue` until the turn it starts shows, the session counts as holding that entry: between the two its queue reads empty while the turn that took the entry is not known yet. The turn shows by its prompt record, or by a line of the model written after the hand-over: a prompt Claude Code scheduled for itself is handed on with a record that is read as no prompt. The lines of the turn that ended were written before the hand-over, though they may come after it in the file, and end nothing. |
| CCQ-004 | **A prompt that just went on keeps the session.** Claude Code writes where a prompt went (a turn of its own, or its queue) once the prompt's hook returned, and the record reaches the file a moment later. A turn's end less than `claude_code_queue_record_ms` (2 s by default) after a prompt from the prompt box went on leaves the session where it is, and so does one during whose look at the session such a prompt goes on: the record may not be in the lines read. The session moves at a later turn's end. Claude Code's own wake-ups come through no prompt box and change nothing here. |
| CCQ-005 | **The fake agent can queue prompts as Claude Code does** (`--queue-prompts`): a line entered while an echoed turn runs gets its prompt hook at once and an `enqueue` record; a Stop that is blocked releases nothing and the turn goes on to another Stop; after the Stop that ends the turn all waiting lines are handed on with one `dequeue` each and answered in one turn, each with its `user` record (`promptSource: "queued"`), with no further prompt hook. Tests can hold the record of a waiting line, the hand-over and the start of the handed-on turn, each by a file. Without the option the agent is as before. One order of Claude Code's it cannot play: a turn that reaches its Stop while a waiting line's prompt hook is still unanswered. |
| CCQ-006 | **The documents say so.** SPEC-461 CCM-013, SPEC-468 (CCF-011 and "Not in this change: the queue"), ADR-0293, the operations guide ("Running tasks"), the architecture document and the changes log say that what waits in Claude Code's queue keeps the session. |

## Plan

- [x] Spec SPEC-471 written and validated
- [x] The watcher counts what waits in Claude Code's queue, per run (CCQ-001)
- [x] The fake agent queues prompts as Claude Code does (CCQ-005)
- [x] A turn's end leaves a session with a waiting prompt where it is: the gate's block and the clean Stop (CCQ-002)
- [x] A hand-over counts until its turn shows, and a prompt that just went on is waited for (CCQ-003, CCQ-004)
- [x] SPEC-461, SPEC-468, ADR-0293, the guides and the changes log say so (CCQ-006)
- [x] Mutation checks, gate and the whole e2e suite green, evidence recorded
- [ ] Independent review answered

## Acceptance criteria

- AC1 (CCQ-001). The records of the real 2.1.322 (an `enqueue` with its text, a bare
  `dequeue`, a `remove` with reason, ids and text, a `popAll`) are read as a queue change
  each; a record of another operation, of another type or without an operation as none. Folded
  in order, two `enqueue` and one `dequeue` leave one entry; a `dequeue` with nothing counted
  leaves none. A run that resumed a session whose earlier run left an `enqueue` behind starts
  with an empty queue. A run whose lines are read again after a restart of the server knows
  the entry that waits; where it is attached again, before a line was read, its queue reads as
  holding something, and once the lines were read (nothing waits) as empty.
- AC2 (CCQ-002, the gate's block, a person's prompt). In the real order: a prompt-mode session
  on 2.1.311 runs a turn the gate will block while runs get 2.1.322. Its person enters a second
  prompt while the turn runs: the prompt hook comes at once and the prompt waits. The turn
  ends, the gate blocks its Stop: the session keeps its one run. The waiting prompt is
  answered in that run. A later turn's end restarts the session (two runs, the second on
  2.1.322). Without the fix the first Stop restarts the session and the prompt is never
  answered.
- AC3 (CCQ-002, a message Yulius typed). The same with a message the task's person sends
  through their Superagent while the turn runs (`task_message`, answered "sent"): the session
  has no draft once the message's hook came, keeps its run at the Stop the gate blocks, and
  answers the message there.
- AC4 (CCQ-002, the clean Stop). An idle prompt-mode session on 2.1.311 whose turn ends
  cleanly while a prompt waits, with the hand-over held so that nothing of it is written when
  the turn's end decides: the session keeps its run; once the hand-over goes on the prompt is
  answered there, and that turn's end restarts the session. Without the fix the session is
  restarted while the prompt waits.
- AC5 (CCQ-002, two prompts). Two prompts entered while one turn runs each get their hook at
  their Enter; the turn's end keeps the session; both are answered in one turn of the first
  run, and that turn's end restarts the session.
- AC6 (CCQ-003). With the handed-on turn's start held after the `dequeue` record: the turn's
  end that reads an empty queue and no new turn keeps the session. Without the rule it
  restarts. With the handed-on turn then started and held: when its prompt record is read the
  hand-over still counts, and no longer once the task reads Working. In the fold: the ended
  turn's lines, written before the hand-over and read after it, end nothing; the model's
  first line or answer written after it does; a line without a time does not.
- AC7 (CCQ-004). With real processes, the waiting prompt's record held until the test lets it
  be written, the turn ended right after the prompt's hook, and six seconds taken as the time
  a record may need: the turn's end keeps the session though no line says a prompt waits; once
  that time is over, the record read and the prompt answered, that turn's end restarts the
  session. Without the rule the first end restarts it. On the test's clock: a turn's end
  1,999 ms after a prompt went on does not look at the session, one 2,000 ms after it does; a
  wake-up of Claude Code's own in between changes nothing; a prompt that goes on while a
  turn's end looks keeps the session however the clock moved meanwhile, and the next turn's
  end restarts it.
- AC8 (CCQ-005). The fake agent's own tests: with `--queue-prompts` a line entered while a
  turn is held gets its hook and its `enqueue` record before the turn ends; a blocked Stop is
  followed by another Stop and no hand-over; after the ending Stop the records are `dequeue`,
  `dequeue`, `user`, `user` and one answer; without the option the order is as before.
- AC9 (CCQ-006). The documents named in CCQ-006 state the rule; SPEC-468 no longer says the
  queue is lost.

## Test plan

- Unit, `crates/yulius-domain/src/agent_cli.rs`: the queue's count (`Queue`: add, hand on,
  take, never below zero, a hand-over until its turn or the model's work written after it;
  AC1, AC6), `moves_now` with a queue and with a prompt not yet read (AC2, AC7), the name of
  the fact that keeps a due session (`kept_by`), for how long a prompt's record may be missing
  (`record_pending`; AC7).
- Unit, `crates/yulius-harness/src/claude.rs`: the real records' shapes (AC1);
  `crates/yulius-app/src/finish.rs`: `TranscriptFacts` folds the queue (AC1).
- Integration with real processes, `crates/yulius/tests/claude_code_moves.rs` (`mod moves`),
  the fake agent in the real order: AC2, AC3, AC4, AC5, AC6, AC7 (the turn held with
  `--turn-hold-file`, the hand-over with `--queue-handover-file`, the handed-on turn with
  `--queue-turn-file`, the record with `--queue-record-file`; decisions awaited at the race
  points `ClaudeMoveProbed` and `ClaudeMoveSettled`), and AC1's resumed run (a task stopped
  with a prompt waiting, then continued) and reattached run (a second server over the same
  data). These tests take the time of CCQ-004 as none, since the fake agent ends a turn
  milliseconds after its prompt; the tests of that rule set it.
- Integration with the test clock, `claude_code_moves.rs`: AC7's bound and a prompt during the
  look (hooks to the internal listener, the look held at `ClaudeMoveProbed`).
- `crates/yulius/src/wiring/harness.rs`: a resumed run's watch does not read the earlier
  run's `enqueue` (AC1).
- `tools/fake-agent/tests`: AC8.
- Mutations, each reverted (Evidence lists them): the queue not asked; a `dequeue` or a
  `remove` not counted; a hand-over not kept; the time after a prompt not asked; a prompt
  during the look not asked; a new run keeping the old count; a replay not applied; a
  reattached run read as known at once, or never; work ending a hand-over whatever its time,
  or never; the count going below zero.
- Checked against real artifacts: the installed Claude Code 2.1.322 against a local stub of
  the model API before the design (Context), and the watcher's code replayed over those runs'
  transcripts and over this instance's real sessions after it (Evidence).
- Not testable here: a restart Yulius makes of a real, signed-in Claude Code session (no
  subscription can be signed in for a test), so the fake agent stands in for the process.

## Decisions

- **The signal is Claude Code's own record of its queue.** Compared:
  - *The `queue-operation` records, counted* (chosen). They are Claude Code's own bookkeeping:
    one rule covers every way an entry comes and goes (a prompt, a notification, the hand-over
    after the turn, the take-in at a tool result, the Up arrow, a drop), with no reading of
    what Yulius believes about the turn. The count needs nothing but the order of the lines.
  - *A mark at a prompt hook that arrives while a turn is open, cleared by the queued prompt's
    record.* It needs Yulius's own reading of "a turn is open", which is late after an
    interrupt or an unanswered Stop; it must be cleared by three different records (the queued
    `user` record, the `queued_command` attachment of a take-in, the `popAll`); it does not
    know Claude Code's notifications. A mark that is not cleared keeps a session for good.
  - *Both.* The hook is used, but only as a time (CCQ-004), never as a count: two counts of
    one queue would have to be matched entry by entry, and the `dequeue` record names none.
- **Every entry counts, notifications too.** The records do not say what an entry is, only its
  text, and a notification that waits is something the session has not been told either: the
  turn end already leaves a session alone while its background agents are pending. The price
  is a session that never moves at a turn's end. Claude Code puts some notifications into its
  queue and never takes them out: the review traced eight of them, all notifications of work
  a sub-agent started (its Monitor, its background shell) or a second notification of the
  same agent, on 2.1.292, 2.1.311 and 2.1.322; the bundle writes a record at every way out of
  the queue, so they are taken to stay there. Counted per process by the review, 9 of 415
  sessions held such an entry while their process lived on for 11 to 773 minutes. Such a
  session keeps its version until its next start: about 2 in 100 sessions. That is accepted:
  the entry is in the queue, so the count is right; telling a notification from a prompt
  would need the entry's text and an owner for each `dequeue`, which names none; and a
  session is started again often (a subscription switch, a stop and continue). The server's
  log names the fact that keeps a due session at each turn's end (`kept_by`), since the
  Instance page keeps listing the task as one that moves when its turn ends.
- **A count that is wrong errs toward staying.** An operation Yulius does not know, a record
  that is lost, a session cleared in its terminal with entries waiting: the session keeps its
  version until its next start (stop and continue moves it at once), and the card lists it.
  Nothing is lost by that.
- **The count is not asked in the cheap first check.** The watcher reads once a second, so at
  a Stop that follows a hand-over or a take-in by less than that the count still names the
  entry that just left the queue. The careful look reads the turn's lines first and asks
  then, so such a session moves at that turn's end and not one turn later. In outcomes no
  arranged case tells this from asking in both checks; the hand-over's test (AC6) holds the
  decision inside the look and so fails when the first check asks too. The price is the look
  itself, one round of the watcher, one to two seconds inside the Stop's request, at each
  turn's end of a session that is due to move and holds an entry: once for a prompt that
  waits, and at every Stop the gate blocks or a goal works on for a session that holds one of
  the notifications above for good (review round 1, nit 9). Asking in the first check too
  would save that and still err toward staying; it is not done, because the read is what
  says the truth and the wait is small beside a turn.
- **A hand-over is kept until its turn shows.** The `dequeue` and the `user` record are
  written some 10 ms apart, usually in one write; between them the queue is empty and no turn
  shows. The prompt record ends it. So does a line of the model written after the hand-over
  (review round 1, finding 3): Claude Code hands a prompt it scheduled for itself
  (`scheduled_task_fire`) on with a prompt record that is `isMeta`, which the watcher reads as
  no prompt, so the hand-over stayed counted through that turn and a session driven by
  scheduled prompts never moved (10 turn ends in 4 of this instance's sessions). The ended
  turn's own late lines (its `turn_duration`, an interrupted answer) come after the `dequeue`
  in the file but were written before it, which the records' own times tell; a line without a
  time (a sub-agent's) and the closing record of a turn end nothing. A hand-over that starts
  neither (a `/compact` entered while a turn ran) is ended by the next prompt; no turn ends
  in between.
- **Two seconds after a prompt went on, the session stays.** No record tells Yulius where a
  prompt went until Claude Code wrote one, 67 to 108 ms after the hook returned in the runs.
  For two seconds after a prompt from the prompt box went on, a turn's end therefore leaves
  the session alone; and a prompt that goes on while a turn's end looks is counted, not timed,
  since a look may take longer than that. It is a bound on Claude Code's own writing, the only
  link there is: a machine so loaded that the write takes longer than two seconds loses the
  prompt as before. Two seconds are some twenty times the write that was measured (first one
  second; raised in review round 1, nit 10, for a loaded host); more would keep a session
  whose turn is that short, and costs it one late move. Rejected: the turn's end waiting out
  the rest of the time and reading then. The decision at a Stop is made inside the hook's request, whose client gives up after
  5 s, and already takes up to two seconds for the watcher's round; a wait on top makes a
  session whose turn follows its prompt at once miss the decision's time under load, which
  only looks like the same outcome. A turn of a model seldom ends within two seconds of its
  prompt, so the rule costs little there; the tests' fake agent always does, so its tests
  take the time as none and the rule has tests of its own.
- **What the test in the old order shows.**
  `a_message_yulius_typed_keeps_the_session_until_its_submit` runs with the fake agent reading
  the message after its turn. It shows the draft rule for what Yulius typed (SPEC-468
  CCF-011): a message that is in the prompt box and not submitted keeps the session. With the
  real tool that is the time from the paste to the Enter, and a paste whose Enter failed. It
  stays, said so; the queue is shown by the tests in the real order.
- **Not in this change: #1194** (a subscription switch at a turn's end loses the queue). Its
  switch cannot always wait (a subscription at its limit), so it needs the waiting prompts
  typed again into the resumed session: the texts kept, a rule for a prompt the old process
  handed on just before it ended, and the order with the launch prompt. That is a design of
  its own in the rotation's code. This change gives it the count and the fake agent's order.
- **Not in this change: #1206** (a Stop that finds a prompt pause open). Older code in
  `hook_stop`, about capacity pauses, not about the move. Its test needs a turn that reaches
  its Stop while a prompt's hook is still held, which the fake agent cannot play yet (it
  answers a waiting line's hook before it goes on; review round 1, finding 4): that, with
  what `--queue-prompts` already does, is what the fix of #1206 has to add. Two more things
  of the same family showed in this change's tests and belong there: the gate's count is not
  reset for a handed-on prompt, whose hook came at its Enter, and with a turn shorter than
  the watcher's second the task can read Working after a handed-on turn that is over.
- **Not in this change: #1199** (a message typed onto a person's draft older than a minute).
  It is about when Yulius types, not about what a restart loses.
- **Not known counts as holding.** After a restart of the server a reattached run's lines are
  read again from their start, 16 MiB a second; this instance has transcripts of 38 MB. A
  turn's end in those seconds would have read an empty queue, so until the replay was applied
  the queue counts as not known and the session stays (found by the review's reading, before
  its first report). A run whose transcript cannot be read at all stays until its next start.
- **Limits.** A prompt entered after the turn's end decided to restart and before the task
  reads as switching is typed into a process that ends (some milliseconds; a message's sender
  is refused once the task reads switching). A write of Claude Code's that takes longer than
  the two seconds leaves the prompt unseen. The operations guide names both. When a prompt
  went on is kept in memory: after a restart of the server CCQ-004 knows no earlier prompt. A session its person resumes or
  clears inside its terminal changes the transcript the watcher reads: what the other file's
  earlier processes left counted stays counted, which keeps the session until its next start.
- **ADR-0293 is amended in place**, as SPEC-468 did: the rule refines that decision.

## Evidence

- **The real tool, before the design** (2026-11-12, the installed Claude Code 2.1.322 in a PTY,
  its own home and config directory, `ANTHROPIC_BASE_URL` at a local stub of the Messages API
  that streams text or answers with a `tool_use`, hooks that log their time and the queue
  records the transcript holds at that moment, a tail that logs when each record reaches the
  file; no account, nothing leaves the host). Twelve scenarios, each run once or twice:
  `one-queued`, `two-queued`, `tool-queued` (a prompt entered while a tool runs),
  `tool-two-queued`, `tool-late-queued` (after the last tool result), `stop-block-tool-queued`
  (the Stop blocked once), `pop-up` (Up arrow), `pop-esc` (Escape), `held-stop-kill-resume`
  (the Stop held, SIGTERM, `claude --resume`), `slow-hook-queued` (a prompt hook that answers
  after 6 s: its `enqueue` is made 2 ms after the hook returned, not before), `pop-up-two`
  (one Up arrow with two prompts waiting: two `popAll` records) and `tool-two-at-once` (two
  prompts taken in at one tool result: two `remove` records): a prompt's way out of the queue
  wrote one record per entry in every run. Context states what they showed. Over all runs the
  hook of a prompt entered while a turn ran began 5 to 15 ms after its Enter (11 prompts), and
  a queue record reached the file 67 to 108 ms after its own time (21 records). The scripts
  are not in the repository (they drive a local install); the record shapes they produced are
  the fixtures of `queue_records_are_changes_of_the_queue`. The review ran the tool as well
  (round 1): it reproduced `pop-up-two` and `tool-two-at-once`, and added a turn's Stop that
  runs while a prompt's hook is still held, a `/compact` entered while a turn runs (no hook
  at its Enter, `enqueue`, `dequeue`, no prompt record) and `/cost` and `/mcp reconnect`
  (no record at all).
- **The watcher's own code over real transcripts** (throwaway `#[ignore]` tests, removed
  again: `HarnessAdapter::watch` from the start of each file, every observation folded with
  `TranscriptFacts::fold`; counts only):
  - the probe's transcripts (ten files of nine scenarios, the ones run by then) read as the
    scenarios ran: `one-queued` is prompt, `enqueue` (1 waits), `dequeue` (0 wait, handed on),
    the ended turn's `turn_duration` (still handed on), the queued prompt (nothing held);
    `pop-esc` keeps the hand-over through the interrupted turn's line; `tool-queued` and
    `pop-up` end with nothing held after their `remove` and `popAll`; `held-stop-kill-resume`
    ends with one entry waiting, which is the record a resumed run does not read;
  - 417 session files of this instance (2026-11-12): 408 hold queue records, 5,014 `enqueue`,
    3,744 `dequeue`, 1,254 `remove`; a direct count of the same files gives the same three
    numbers, so the watcher reads every one. No removal ever came with nothing counted. Most
    entries waiting at once: 6. Files are read whole here, with no reset at a restart: of
    4,252 turn ends, 47 came with an entry still counted, in 12 sessions. The review counted
    per process (round 1, finding 2): 9 sessions hold an entry that is never taken out while
    their process lives on, all notifications; Decisions, "Every entry counts", has what
    that costs;
  - hand-overs, after the rule of round 1 (a hand-over ends with the model's work written
    after it): 3,677 `dequeue` records, each with a time. Of 4,186 turn ends, 32 come with a
    hand-over still counted and nothing else waiting, which is the order of an entry that
    waited through a turn's Stop (`dequeue`, then the ended turn's `turn_duration`); at one of
    them model work had been read since the hand-over, written before it. Before that rule
    the review found the hand-over still counted at the end of the turn it had started for
    each of the 10 scheduled prompts in these files (4 sessions);
  - by the Claude Code version that wrote them: 2.1.292 (360 sessions), 2.1.311 (32) and
    2.1.322 (18) write the same records with the same fields (`operation`, `sessionId`,
    `timestamp`, the text, and for `remove` its `reason` and ids). Only 2.1.322 was run.
- **Tests, each seen failing first.** Without the rule (logs kept outside the repository):
  the four tests in Claude Code's order failed with "the session was restarted with a prompt
  in its queue" and "with 1 in its queue", "with 2 in its queue" (AC2 to AC5); the three tests
  of CCQ-004 failed before it was built (AC7); the domain, harness, fold and runtime tests
  failed against stubs (AC1, and round 1's `kept_by`, the model's work after a hand-over and
  the reattached run's unknown queue); the fake agent's tests of the new order, of the
  record's hold file and of a scheduled prompt failed on the option or the order that did
  not exist yet, and the one that pins the old order passes with and without the change
  (AC8). The hand-over's tests with real processes (AC6) passed when written, since the rule
  was there: their failure is mutants 8, 10 and 28 below.
  `cargo nextest run -p yulius -E 'binary_id(yulius::main) & test(/^claude_code_(moves|admin)::/)'`
  at 68ab2d96: 46 passed. `cargo nextest run -p fake-agent`: 26 passed.
- **Mutations** at 68ab2d96 (a script over 32 mutants, each applied alone, its tests run,
  reverted with `git checkout`): all 32 caught.
  1. the queue not asked: the five tests of AC2 to AC6 fail;
  2. the queue asked in the first cheap check as well: AC6's test fails, whose decision is
     held inside the look (no outcome differs: Decisions);
  3. and 4. a `dequeue` not counted: the domain tests, and with real processes the session
     never moves after the hand-over;
  5. a `remove` not counted; 6. the count going below zero (wrapping): the domain test;
  7. and 8. a hand-over not kept: the domain test, and AC6 with real processes;
  9. the ended turn's closing record ending a hand-over: the fold's test;
  10. a hand-over ended when its prompt record is read, before the task reads Working: AC6;
  11. a turn that came and went not ending its hand-over: AC4 never moves;
  12. the watcher not applying queue records: AC2;
  13. a new run keeping the old count: `a_resumed_run_starts_with_an_empty_queue`;
  14. a replay not applied: `a_reattached_run_knows_what_waits_in_its_queue`, which waits for
      the replayed count (round 1's major: it had waited for "holds", which a reattached run
      reads at once);
  15. the time after a prompt not asked, and 18. a prompt that goes on not timed: both tests
      of AC7's bound;
  16. a prompt during the look not asked, and 17. such a prompt not counted: AC7's look;
  19. the bound compared the wrong way: the domain test;
  20. a reattached run read as known at once, and 21. never known:
      `a_reattached_runs_queue_is_not_known_until_its_lines_were_read`; 22. not known not
      counting as holding: the runtime's test;
  23. and 24. the model's work ending a hand-over whatever its time: the domain test and the
      fold's; 25. never ending it: the domain test; 26. and 27. the replay not asking the
      model's line, or its answer: the fold's test;
  28. the watcher not asking the model's work: the scheduled prompt's test with real
      processes never moves;
  29. a due session kept by a fact without a name: `kept_by`'s test;
  30. an `enqueue` not read, 31. `popAll` read as an entry put in, 32. a queue record's time
      dropped: the harness test.
- **Gate.** `scripts/dev ci` at 68ab2d96 (every code and test change of this task, `dev` up
  to 2a31d50a merged): exit 0. `cargo check`, `fmt --check`, clippy `-D warnings` on Linux and
  for `aarch64-apple-darwin`, nextest 3981 passed / 16 skipped, doctests, `cargo deny`
  (advisories, bans, licenses, sources ok), `cargo shear`, `cargo doc`. After it comes this
  section alone. The review ran the gate itself at 5bafbc37: exit 0, 3978 passed.
- **e2e.** The whole suite at 68ab2d96 (`npx e2e run`): 82 files passed, 1 skipped; 985 tests
  passed, 1 skipped. This change touches no page.
- **Security scan.** Disabled: `yulius security-scan` says "Security scan disabled: Vidoc is
  turned off in Yulius"; nothing was scanned, which is not a pass.
- **Independent review, round 1** (a fresh session, read-only, head 5bafbc37; it ran the gate,
  the tests twelve times over, its own mutants and the real tool): request changes. One
  major: the reattached run's test could not fail once "not known" read as holding; it now
  waits for the count (mutant 14). Minors: the price of counting every entry and a log line
  for what keeps a session (Decisions, `kept_by`); the hand-over of a scheduled prompt
  (CCQ-003, the model's work after it; the figures above); the fake agent's limit and the
  sentence about #1206 (CCQ-005, Decisions); this section brought to the head it describes.
  Nits: the tests' clocks (the record's hold is a file, the held turns are the test's, the
  time a record may take is waited for as the app reads it); a poisoned lock reads as "may
  hold"; the names (`run_starts`, `claude_queue_holds`, `claude_queue_waiting`,
  `prompts_passed` as "so far"); the look's price said (Decisions) and the first check left
  as it is; the bound raised to two seconds; the two limits said in the operations guide and
  the changes log. Outside this change, as the review put them: the gate's count that is not
  reset for a handed-on prompt and a task that can read Working after a handed-on turn
  shorter than the watcher's second, both said on issue #1206.
- **Not tested here.** A restart Yulius makes of a real, signed-in Claude Code session (no
  subscription can be signed in for a test): the fake agent stands in, in the order the real
  tool was seen to have. Claude Code 2.1.292 and 2.1.311 were not run; their transcripts in
  this instance hold the same records. `popOne` and the `remove` reasons `dropped_by_hook`,
  `cleared_on_cancel` and `delivered_to_agent` were read in the binary and (two of them) seen
  in real sessions, not produced in a run. A notification Claude Code never takes out of its
  queue, and a prompt it scheduled for itself, were seen in real sessions, not produced in a
  run of the tool: the fake agent plays the second.
