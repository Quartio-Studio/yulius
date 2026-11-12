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

- A prompt entered while a turn runs gets its `UserPromptSubmit` hook 11 to 37 ms after its
  Enter. When the hook returned, Claude Code writes a `queue-operation` record `enqueue` with
  the text. A record reaches the transcript file about 100 ms after it was made, so the record
  is not in the file while the prompt's own hook runs.
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

What this instance's real sessions show (1,585 transcript files, 415 with queue records;
counted by scripts, metadata only): 5,252 `enqueue`, 3,939 `dequeue`, 1,297 `remove` (1,270
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
| CCQ-001 | **Yulius knows what waits in Claude Code's queue.** The watcher reads the `queue-operation` records of the run at hand: `enqueue` adds an entry, `dequeue` hands one on to a turn of its own, `remove`, `popAll` and `popOne` take one out without one. The count never goes below zero and an operation Yulius does not know changes nothing. A run starts with an empty queue: a resumed session's earlier lines are not read as the run's, and what the run before left waiting went with its process. A run reattached after a restart of the server knows the same from its own lines. |
| CCQ-002 | **A session with something in its queue is not restarted for a newer Claude Code**, at a Stop the gate blocks or a goal session works on, and idle after a clean Stop. Every entry counts: a person's prompt, a message Yulius typed, Claude Code's own notification. The session moves at a later turn's end, once the queue is empty, or at its next start. A prompt taken into the running turn at a tool result is no longer waiting. The queue is asked after the turn end read the turn's lines, not in its first cheap check: a count the watcher has not caught up with decides nothing. |
| CCQ-003 | **A hand-over is not rest.** From a `dequeue` until the prompt record of the turn it starts was read, the session counts as holding that entry: between the two its queue reads empty while the turn that took the prompt is not known yet. |
| CCQ-004 | **A prompt that just went on is waited for.** Claude Code writes a waiting prompt's record once its hook returned, and the record reaches the file a moment later. A turn's end that would restart the session waits until `claude_code_queue_record_ms` (1 s by default) passed since a prompt from the prompt box last went on, and reads the transcript after that. A prompt that goes on while the turn's end looks leaves the session where it is. |
| CCQ-005 | **The fake agent can queue prompts as Claude Code does** (`--queue-prompts`): a line entered while an echoed turn runs gets its prompt hook at once and an `enqueue` record; a Stop that is blocked releases nothing and the turn goes on to another Stop; after the Stop that ends the turn all waiting lines are handed on with one `dequeue` each and answered in one turn, each with its `user` record (`promptSource: "queued"`), with no further prompt hook. Tests can hold the record of a waiting line, the hand-over and the start of the handed-on turn. Without the option the agent is as before. |
| CCQ-006 | **The documents say so.** SPEC-461 CCM-013, SPEC-468 (CCF-011 and "Not in this change: the queue"), ADR-0293, the operations guide ("Running tasks"), the architecture document and the changes log say that what waits in Claude Code's queue keeps the session. |

## Plan

- [x] Spec SPEC-471 written and validated
- [x] The watcher counts what waits in Claude Code's queue, per run (CCQ-001)
- [ ] The fake agent queues prompts as Claude Code does (CCQ-005)
- [ ] A turn's end leaves a session with a waiting prompt where it is: the gate's block and the clean Stop (CCQ-002)
- [ ] A hand-over counts until its turn shows, and a prompt that just went on is waited for (CCQ-003, CCQ-004)
- [ ] SPEC-461, SPEC-468, ADR-0293, the guides and the changes log say so (CCQ-006)
- [ ] Mutation checks, gate and the whole e2e suite green, evidence recorded
- [ ] Independent review answered

## Acceptance criteria

- AC1 (CCQ-001). The records of the real 2.1.322 (an `enqueue` with its text, a bare
  `dequeue`, a `remove` with reason, ids and text, a `popAll`) are read as a queue change
  each; a record of another operation, of another type or without an operation as none. Folded
  in order, two `enqueue` and one `dequeue` leave one entry; a `dequeue` with nothing counted
  leaves none. A run that resumed a session whose earlier run left an `enqueue` behind starts
  with an empty queue. A run whose lines are read again after a restart of the server knows
  the entry that waits.
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
  restarts.
- AC7 (CCQ-004). With the waiting prompt's record written later than the watcher's next round
  and the turn ended right after the prompt's hook: the turn's end waits for the record and
  keeps the session. With no wait it restarts. A prompt that goes on while the turn's end
  looks keeps the session, and one that went on before a turn whose end comes a time later
  does not delay it.
- AC8 (CCQ-005). The fake agent's own tests: with `--queue-prompts` a line entered while a
  turn is held gets its hook and its `enqueue` record before the turn ends; a blocked Stop is
  followed by another Stop and no hand-over; after the ending Stop the records are `dequeue`,
  `dequeue`, `user`, `user` and one answer; without the option the order is as before.
- AC9 (CCQ-006). The documents named in CCQ-006 state the rule; SPEC-468 no longer says the
  queue is lost.

## Test plan

- Unit, `crates/yulius-domain/src/agent_cli.rs`: the queue's count (`Queue`: add, hand on,
  take, never below zero, a hand-over until its turn; AC1, AC6), `moves_now` with a queue and
  with a prompt not yet read (AC2, AC7), how long a turn's end waits for a prompt's record
  (AC7).
- Unit, `crates/yulius-harness/src/claude.rs`: the real records' shapes (AC1);
  `crates/yulius-app/src/finish.rs`: `TranscriptFacts` folds the queue (AC1).
- Integration with real processes, `crates/yulius/tests/claude_code_moves.rs` (`mod moves`),
  the fake agent in the real order: AC2, AC3, AC4, AC5, AC6, AC7 (the turn held with
  `--turn-hold-file`, the hand-over with `--queue-handover-file`, the handed-on turn with
  `--queue-turn-file`, the record with `--queue-record-ms`; decisions awaited at the race
  points `ClaudeMoveProbed` and `ClaudeMoveSettled`), and AC1's resumed run and reattached run.
- `tools/fake-agent/tests`: AC8.
- Mutations, each reverted (Evidence lists them): the queue not asked; a `dequeue` or a
  `remove` not counted; a hand-over not kept; the wait dropped; a prompt during the look not
  asked; a new run keeping the old count; the count going below zero.
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
  turn end already leaves a session alone while its background agents are pending. The cost:
  a notification Claude Code keeps in its queue across turn ends keeps the session's version
  until it is taken, or the next start. In this instance's sessions that is at most 50 of
  4,459 turn ends (12 of 415 sessions), restarts included.
- **A count that is wrong errs toward staying.** An operation Yulius does not know, a record
  that is lost, a session cleared in its terminal with entries waiting: the session keeps its
  version until its next start (stop and continue moves it at once), and the card lists it.
  Nothing is lost by that.
- **The count is not asked in the cheap first check.** The watcher reads once a second, so at
  a Stop that follows a hand-over by less than that the count still names the entry that was
  just handed on. The careful look reads the turn's lines first and asks then.
- **A hand-over is kept until the prompt record of its turn.** The `dequeue` and the `user`
  record are written some 10 ms apart, usually in one write; between them the queue is empty
  and no turn shows. Only the prompt record ends it: the ended turn's own late lines (its
  `turn_duration`, an interrupted answer) come after the `dequeue` in the file. A hand-over
  that starts no prompt record (a command) is ended by the next prompt, one turn late.
- **A wait, then the read, for a prompt that just went on.** No record tells Yulius where a
  prompt went until Claude Code wrote one, about 100 ms after the hook returned. The turn's
  end therefore waits out a bound of one second since the last prompt went on and reads then.
  It is a bound on Claude Code's own writing, the only link there is; a machine so loaded that
  the write takes longer loses the prompt as before. Rejected: keeping the session whenever a
  prompt went on in the last second, which needs no wait but never moves a session whose turn
  is shorter than that. The wait is taken only when the session is otherwise due to move, and
  is not counted against the time the decision may take.
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
  `hook_stop`, about capacity pauses, not about the move. The fake agent's real order makes
  its test possible.
- **Not in this change: #1199** (a message typed onto a person's draft older than a minute).
  It is about when Yulius types, not about what a restart loses.
- **Limits.** A prompt entered after the turn's end decided to restart and before the task
  reads as switching is typed into a process that ends (some milliseconds; a message's sender
  is refused once the task reads switching). What went on is kept in memory: after a restart
  of the server the wait of CCQ-004 knows no earlier prompt.
- **ADR-0293 is amended in place**, as SPEC-468 did: the rule refines that decision.
