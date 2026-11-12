---
yulius-task: e5529390-b6e8-4cf3-8348-6672f8933749
status: in-progress
---

# SPEC-479: A move to another subscription at a turn's end waits for what Claude Code still queues

Status: accepted
Date: 2026-11-12
Owner: Yulius task #462
Scope authorization: the request of task #462 (by MrFishPL, 2026-11-12): fix issue #1194 in one
pull request, the defect reproduced first by a test that fails without the fix. Amends
SPEC-001 ROT-003 (the proactive move at a turn boundary) and builds on SPEC-471 (the queue's
count).

## Goal

A prompt that waits in Claude Code's queue when a turn ends (a message Yulius typed for a
person, a pull request's news, a prompt its person entered while the agent worked) is no longer
lost when the pool wants the session on another subscription at that turn's end. The session
answers it where it was entered and moves at the next turn's end.

## Context

Symptom (issue #1194). A turn runs, its subscription reaches the threshold or its limit, a
message is typed into the session (`task_message` answers `delivered: "sent"`), the turn ends.
Yulius moves the session to another subscription at that Stop: the old process is ended with
the message in its queue, and the resumed session never sees it.

Root cause. `hook_stop` (`crates/yulius-app/src/hooks.rs`) asks the pool at every turn's end
(`rotation::decide(.., Trigger::Proactive)`). On `Decision::Switch` it holds the Stop and
restarts the session with `switch_task_now`. Before that it asks one thing, whether background
work runs (`busy`), not what Claude Code holds for the session in memory. Since SPEC-471
Yulius knows that queue (`App::claude_queue_holds`, counted from the transcript's
`queue-operation` records), but only the restart on a newer Claude Code asks it. A turn held
for capacity (`PauseKind::Turn`) that is later resumed on another subscription
(`resume_paused` in `capacity.rs`) ends the process in the same way.

Why the existing tests missed it: until SPEC-471 the tests' fake agent read a line only after
its turn, so nothing ever waited in a queue; SPEC-471's tests in Claude Code's order cover the
restart for a newer version only.

What Claude Code does (SPEC-471 Context, seen with 2.1.322 against a local stub of the model
API): a prompt entered while a turn runs waits until the Stop that ends the turn returned and
is then handed on to a turn of its own (`dequeue`), or is taken into the running turn at the
next tool result (`remove`). A Stop a hook blocks releases nothing. A resumed session does not
get the queue back. Claude Code leaves some of its own notifications in the queue for good.

Three more facts this change rests on, each checked (Evidence):

- A turn's closing record (`turn_duration`) is written when a turn ends, after the `dequeue`
  records of that end, and not at a Stop a hook blocks.
- Claude Code hands its queue on in the order of its entries, and only prompts that follow
  one another leave together. A notification of its own, a slash command or a `!` line leaves
  alone; what waits behind it stays through that turn's closing record and is handed on at
  the next turn's end (found by the review with the real tool; the first version of this
  spec said the opposite).
- What a closing record finds waiting at an end that handed nothing on is not handed on to a
  turn of its own later: in this instance's sessions no `dequeue` ever came while only such
  entries were counted.

A turn that a usage limit cuts is redone by the session Yulius resumes past the limit
(SPEC-034, SPEC-072: the transcript is stepped back and keeps the unanswered prompts), so a
handed-on prompt whose turn meets the limit is not lost either.

## Requirements

| ID | Requirement |
| --- | --- |
| SWQ-001 | **A move the pool wants at a turn's end stays while a prompt waits.** At a Stop where the pool decides to move the session to another subscription, the session stays on its subscription when Claude Code's queue holds something a turn's end hands on (SWQ-002), or a hand-over is not over (SPEC-471 CCQ-003), or a prompt from the prompt box went on so recently, or during this look, that its record may not be read (SPEC-471 CCQ-004). The Stop is answered as on a subscription that stays: the gate blocks, a clean Stop ends the turn. The queue is asked after the turn's lines were read. The session moves at a later turn's end, once nothing of that waits. |
| SWQ-002 | **What a turn's end left in the queue does not keep the move.** The queue's count knows how many of its entries a turn's end left: its closing record found them waiting, and that end had handed nothing on (its own notifications of work a sub-agent started, an entry Yulius counted wrongly). They keep no move. The closing record of an end that did hand something on marks nothing: what waits behind an entry Claude Code hands on alone gets its hand-over at the next end. An entry that leaves the queue is taken to be one that was not left over, while there is one. The restart on a newer Claude Code keeps counting every entry (SPEC-471 CCQ-002, unchanged). |
| SWQ-003 | **A turn held for capacity is not moved over a waiting prompt.** When capacity returns on another subscription for a task whose turn is held at its Stop (`PauseKind::Turn`, a live process) and SWQ-001's facts hold, the hold is released in place instead: the Stop is asked again and SWQ-001 decides it. |
| SWQ-004 | **A handed-on turn that meets the limit moves like any turn a limit cuts.** Nothing new: the test shows that the prompt's record is in the transcript the resumed session continues from. |
| SWQ-005 | **The fake agent can play both.** With `--queue-prompts`, a waiting line that begins with `/stuck ` is an entry Claude Code never takes out: its `enqueue` record, no prompt hook, no hand-over, no answer. A waiting line that begins with `/alone ` is an entry Claude Code hands on alone (a notification of its own, a slash command, a `!` line): the queue leaves in the order of its lines, the lines up to the first such one together and that one alone, and the rest waits through the closing records for the next turn's end. `--limit-after N` also cuts a handed-on turn: its prompt records, then the limit record, no answer and no Stop. |
| SWQ-006 | **The documents say so.** The operations guide says what happens to a message sent while a task moves to another subscription; ADR-0310 records the decision; the architecture document, the changes log and SPEC-471's note on #1194 follow. |

## Plan

- [x] Spec SPEC-479 written and validated, the defect reproduced with real processes
- [x] The queue knows what a turn's end left behind (domain, watcher, replay)
- [x] A pending move stays at a turn's end while a prompt waits: the regression tests pass
- [x] An entry a turn's end left behind does not keep the move (fake agent `/stuck`)
- [x] A turn held for capacity is released in place, not moved, while a prompt waits
- [x] A handed-on turn the limit cuts moves past the limit with its prompt (fake agent)
- [x] Operations guide, ADR-0310, architecture and changes log say so
- [x] Mutation checks and gate green, evidence recorded
- [ ] Review round 1: an end that handed something on leaves nothing behind, the held turn's guards tested
- [ ] Independent review answered

## Acceptance criteria

- AC1 (SWQ-001, a message Yulius typed). With real processes in Claude Code's order: a
  prompt-mode session's turn runs on the first of two subscriptions, which reaches its limit
  meanwhile. A message sent through a Superagent is typed into it (`delivered: "sent"`) and
  waits in the queue. The turn ends: the session keeps its one run and answers the message
  there, once. That turn's end moves the session to the second subscription. Without the fix
  the first end moves it and the message is never answered.
- AC2 (SWQ-001, a person's prompt). The same with a prompt its person entered in the terminal.
- AC3 (SWQ-001, the gate's block). The same at a Stop the gate blocks: the session stays, the
  gate blocks, the prompt is answered in that run, and a later end moves the session.
- AC4 (SWQ-002). With an entry in the queue that is never taken out: the turn's end at which
  it is first seen keeps the session; once that turn closed, the next turn's end moves it. In
  the count: the closing record of an end that handed nothing on marks all that waits as
  left over, that of an end that handed something on marks nothing; an entry added later is
  not left over; a leave takes one that is not left over first; a new run starts with none;
  the restart's rule (`holds`) still counts a left-over entry.
- AC5 (SWQ-003). Both subscriptions are over the threshold, the gate would block: the turn's
  end holds the turn (the task waits for capacity) with a prompt in the queue. The second
  subscription gets room: the task runs again in the same run, the prompt is answered there,
  and a later end moves the session. Without the fix the session is moved while the prompt
  waits.
- AC6 (SWQ-004). A prompt waits, the turn ends on a subscription at its limit, the handed-on
  turn is cut by the limit: the session resumes past the limit on the second subscription, and
  the transcript it continues from holds the prompt's record and no limit record.
- AC7 (SWQ-005). The fake agent's own tests: a `/stuck ` line gets its `enqueue` record and
  neither hook, `dequeue` nor answer; a handed-on turn at the limit writes its prompt records
  and the limit record, and runs no Stop.
- AC8 (SWQ-006). The documents named in SWQ-006 state the rule and its limits.
- AC9 (SWQ-002, review round 1). An entry Claude Code hands on alone and a prompt behind it
  wait when the turn ends on a subscription at its limit: the entry gets its turn, the end of
  that turn keeps the session, the prompt is answered in the same run, once, and that turn's
  end moves the session. With the first rule the end of the entry's turn moved it and the
  prompt was never answered.
- AC10 (SWQ-003, review round 1). A held turn with a prompt that may wait stays held while
  nothing has room; the pool reads the lines written so far before it asks (a prompt whose
  record reaches the transcript after the turn was held keeps the turn in place); a pause
  that changed while the pool read is left as it is.

## Test plan

- Unit, `crates/yulius-domain/src/agent_cli.rs`: `Queue` with left-over entries (AC4).
- Unit, `crates/yulius-app/src/finish.rs`: the replay of a reattached run folds the closing
  record into the queue (AC4).
- Integration with real processes, `crates/yulius/tests/claude_code_moves.rs` (`mod moves`),
  the fake agent in Claude Code's order with two subscriptions: AC1, AC2, AC3, AC4, AC5, AC6.
- Integration on the test's clock, `claude_code_moves.rs` (hooks to the internal listener, no
  process; a move that is tried is refused there and leaves its wait): a turn's end 1,999 ms
  after a prompt went on does not move the session and one 2,000 ms after does; a prompt that
  goes on while the turn's end looks (held at the race point `SwitchProbed`) keeps the session;
  a held turn goes on in place while a prompt may wait and is moved once none does; a turn a
  limit cut is moved whatever may wait; a paused task without a process is not released in
  place (SWQ-001, SWQ-003).
- Unit, `crates/yulius-app/src/supervisor.rs`: what a move asks of the queue, with a queue that
  is not known and a new run (SWQ-001, SWQ-002).
- `tools/fake-agent/tests/cli.rs`: AC7.
- Mutations, each reverted (Evidence).
- Checked against real artifacts: the installed Claude Code 2.1.322 against a local stub of
  the model API (the closing record at a blocked Stop), and a count over this instance's real
  transcripts (what a closing record finds waiting, and whether it is handed on later).
- Not testable here: a move Yulius makes of a real, signed-in Claude Code session, and a real
  usage limit (no subscription can be signed in for a test): the fake agent stands in.

## Decisions

- **The move waits; the prompt is not carried over.** Compared:
  - *Stay while a prompt waits* (chosen). The same rule the turn's end already has for
    background work, and the one SPEC-471 chose for the restart. The prompt is answered by the
    session it was entered into, in Claude Code's own order, exactly once. It needs no text
    kept, and nothing typed by Yulius. The price is one more turn on a subscription over its
    threshold. On a subscription at its limit that turn is refused by Claude and continued
    past the limit on the next subscription, which is the tested path of every turn a limit
    cuts.
  - *Carry the waiting texts into the resumed session* (the issue's first idea: "the switch
    cannot wait"). It keeps to the threshold. It needs the texts of private prompts kept, a
    model of which entry each unnamed `dequeue` took (a wrong guess answers a prompt twice or
    not at all), an order with the gate's reasons and a goal typed again, and it turns a
    person's own prompt into text Yulius typed. The switch can wait after all: the limit path
    above loses nothing.
  - *Tell the sender it was not delivered.* True only for what Yulius typed, and late: the
    sender was answered "sent" minutes before. A person's own prompt has no sender to tell.
  The choice is a default that can be reversed: the count and the left-over mark are what a
  carry-over would need too.
- **The hold is bounded by the records, not by a count of turns or a timer.** A session whose
  queue holds one of the notifications Claude Code never takes out would never move at a
  turn's end and would run into its limit every time. An entry that a turn's closing record
  found waiting, at an end that handed nothing on, was left by Claude Code: it keeps nothing.
  Rejected: a number of turn
  ends (a goal session whose Stops are blocked without a tool call keeps a real prompt
  waiting through any number of them, and a person who writes at every turn would be cut off
  by it); the entry's text (the records name no entry at a `dequeue`).
- **A leave takes an entry that was not left over first.** The `dequeue` names none. In the
  real sessions a left-over entry never left by a hand-over, and when one leaves at a tool
  result every entry that can be taken in leaves with it. The other order would let every
  notification taken in during a turn make an old left-over entry count again, and such a
  session would stay at each turn's end. The price: when a left-over entry leaves alone while
  a prompt waits (a notification delivered to a sub-agent: once in this instance's sessions),
  that prompt counts as left over and a move at that turn's end loses it, as before this
  change.
- **Only an end that handed nothing on leaves what waits** (review round 1, the major). The
  first rule marked whatever a closing record found. The review ran the real 2.1.322 with a
  background shell's notification, a slash command and a `!` line ahead of a prompt: Claude
  Code hands such an entry on alone, the closing record finds the prompt still waiting, and
  the prompt is handed on at the end of the entry's turn. Marked as left over, it no longer
  kept the session there, which is #1194 again for a message typed seconds after the agent's
  background work finished. An end that handed something on therefore marks nothing: the
  count knows it by the hand-over that is not over when the closing record is read (the
  `dequeue` comes before it in the file). An entry Claude Code never takes out is still
  marked at the first end that hands nothing on. In the real sessions the corrected rule
  marks the same 24 entries, and four closing records came at an end that had handed
  something on with entries waiting behind.
- **The restart on a newer Claude Code keeps its rule.** SPEC-471 chose that every entry
  counts there and accepted the cost. Whether it should use the left-over mark too is that
  feature's decision; this change is one concern.
- **A held turn is released, not kept held.** A turn held for capacity with a prompt waiting
  stays held while nothing has room (nothing is lost while the process lives). Only when the
  pool would move it is it released in place, so that the Stop is asked again and SWQ-001's
  one rule decides. Rejected: not pausing such a turn at all (it would work on over the
  threshold with no room anywhere).
- **Not in this change.** A person's own switch or stop (their decision, and the terminal
  shows the queue). A run without hooks (a Claude Code older than Yulius supports): there the
  turn's end is read from the transcript after Claude Code handed its queue on. A limit that
  cuts a turn while prompts wait: what Claude Code does with its queue then was not run with
  the real tool; real transcripts show each prompt that arrives at a limit with a record of
  its own, which the step back keeps.
- **Limits.** As for SPEC-471: a prompt entered in the moment between the turn's end deciding
  to move and the task reading "Continuing" is typed into a process that ends (a message sent
  through Yulius is refused from then on), and a write of Claude Code's slower than the two
  seconds of CCQ-004 leaves a prompt unseen.

## Evidence

- **Reproduced first.** At 8b826e4a (the two tests of AC1 and AC2 on `dev`'s code, with real
  processes): both fail with "the session moved with a prompt in its queue, which went with
  its process", 2 runs where 1 was expected, the terminal showing `QUEUED: …` and `ECHO: one`
  and no answer to the prompt. They pass from eaac0372 on.
- **The real tool** (2026-11-12, the installed Claude Code 2.1.322 in a PTY against a local
  stub of the Messages API, the probe of SPEC-471 run again; nothing leaves the host).
  Scenario `stop-block-queued`: a prompt entered while a turn runs, the turn's first Stop
  blocked by the hook, the second not. In the file: `enqueue`; the first answer; the hook's
  feedback and a `stop_hook_summary`, and no `turn_duration`; the second answer; `dequeue`,
  `stop_hook_summary`, `turn_duration`; the prompt's record (`promptSource: "queued"`). So a
  blocked Stop writes no closing record, and at the ending Stop the hand-over is in the file
  before the closing record.
- **Real transcripts of this instance** (420 session files, 412 with queue records; scripts
  that count, no text read; kept outside the repository): 5,038 `enqueue`, 3,682 `dequeue`,
  1,339 `remove`, 4,197 `turn_duration`. Counted as SWQ-002 counts: 24 entries were found
  waiting by a closing record; no `dequeue` ever came while only such entries were counted;
  7 of them left later by a `remove` (6 `absorbed_mid_turn`, 1 `delivered_to_agent`), 17
  still waited at the file's end. Records of 2.1.292, 2.1.311 and 2.1.322.
- **Tests, each seen failing first.** AC1, AC2 as above. The fake agent's two tests failed
  before its change (a `/stuck` line was answered and got a hook; a handed-on turn was not
  cut). AC5 failed before SWQ-003 with "the held turn was moved with a prompt in its queue".
  AC3, AC4, AC6, the tests on the test's clock and the unit tests passed when written, since
  the rule was there: their failure is the mutants below.
- **Mutations** at 5a85caa6 (a script over 23 mutants, each applied alone, the tests of this
  change run, reverted with `git checkout`): all 23 caught.
  1. the turn's end not staying for a waiting prompt: AC1 to AC6 and the look's test fail;
  2. prompts counted after the look, and 6. a prompt during the look not asked: the look's
     test; 5. a prompt that just went on not asked: its test;
  3. the move counting every entry, as the restart does, 9. the watcher not marking what a
     turn's end left, 11. a closing record marking nothing, 14. a left-over entry counting
     for the move: AC4 (the session never moves);
  4. the queue not asked by the move: AC1 to AC6;
  7. a queue not read yet, 8. one that cannot be read, counting as empty, and 19. a new run
     keeping the old left-over count: the runtime's test;
  10. the replay not marking: the fold's test;
  12. a leave leaving the left-over count above what waits, 13. a hand-over that is not over
      not counting: the domain test; 15. the comparison including equal: AC1 to AC5;
  16. a held turn moved over a waiting prompt: AC5; 17. any pause released in place: the
      limit's test on the clock; 18. a task without a process released in place: its test;
  20. to 23. the fake agent handing a stuck entry on, sending it a prompt hook, not cutting a
      handed-on turn, cutting it one turn late: its own tests and AC4, AC6.
  The first run of the script reported eight survivors; its pattern for nextest's result
  lines missed `( 1/20)`. Run again with the pattern fixed, each of the eight is caught.
- **Gate.** `scripts/dev ci` at 56662c57 (every code and test change of this task, `dev` up to
  4157f67d merged without a conflict): exit 0. `cargo check`, `fmt --check`, clippy
  `-D warnings` on Linux and for `aarch64-apple-darwin`, nextest 4047 passed / 16 skipped,
  doctests, `cargo deny` (advisories, bans, licenses, sources ok), `cargo shear`, `cargo doc`.
- **e2e.** Not run here: this change touches no page. Yulius CI runs the suite on the pull
  request.
- **Security scan.** Disabled: `yulius security-scan` says "Security scan disabled: Vidoc is
  turned off in Yulius"; nothing was scanned, which is not a pass.
- **Not tested here.** A move Yulius makes of a real, signed-in Claude Code session, and a
  real usage limit: no subscription can be signed in for a test, so the fake agent stands in,
  in the order the real tool was seen to have. That Claude Code continues a handed-on turn
  after a resume past a limit rests on SPEC-034 and SPEC-072 (the transcript holds the
  prompt's record; the test shows that much). What Claude Code does with its queue when a
  limit cuts a turn was not run. The re-check that the pause is still open before a held turn
  is released (`pause_open` in `resume_paused`) has no test: it guards a change of the pause
  during the read of the turn's lines.
