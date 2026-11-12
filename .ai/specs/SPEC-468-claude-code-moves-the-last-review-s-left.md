---
yulius-task: 57d7619e-9b14-4e4a-81e8-6a431b4d5b7c
status: in-progress
---

# SPEC-468: Claude Code moves: the last review's leftovers of SPEC-461

Status: in progress
Date: 2026-11-11
Owner: Yulius task #452
Scope authorization: the request of task #452 (by MrFishPL, 2026-11-11): fix the two minors and
seven nits the third independent review of task #443 (PR #1167, SPEC-461, ADR-0293) left open,
each test-first, and decide what happens to a draft that sits in a session's prompt box. Amends
SPEC-461 CCM-008, CCM-011, CCM-012 and CCM-013.

## Goal

The restart of a running task on a newer Claude Code (SPEC-461 CCM-013) keeps what a person
left in the session's prompt box and tells nobody twice: a goal a person set in a prompt-mode
session and a draft in its prompt box keep the session where it is; an idle restart never reads
as work and does not notify again; a restart is always watched by the breaker; and the card and
a task's info promise only what will happen. The gaps in tests and wording the review named are
closed. Not reached: a prompt that waits in Claude Code's queue is still lost by such a restart
(Decisions; issue #1204).

## Context

SPEC-461 (task #443) was merged into `dev` as `eb2e8a4e` after its third review round approved
the head `e9e11c0a` with no blocker or major. That round listed two minors and seven nits, all
in that change's own code, and one question of direction. Where things are:

- The move: `crates/yulius-app/src/claude_code.rs` (`claude_code_move_due`,
  `move_to_claude_code`, `move_idle_session`, the breaker `stop_moves`), called from `hook_stop`
  (`hooks.rs`: a session that works on after the turn) and from `settle_clean_turn`
  (`finish.rs`: an idle prompt-mode session). The policy is `yulius_domain::agent_cli::moves_now`.
- A restart is a subscription switch with the reason `claude_code` (`switch_from` in
  `supervisor.rs`): the old process ends, `claude --resume` starts, and a prompt-mode session
  gets nothing typed, a goal-mode session its goal again.
- Yulius reads a goal's state (`/goal …`) from the transcript lines of the run at hand
  (`goal_open`: set or not met yet), so a new run knows no goal until a record names one; it
  types a goal task's goal again after a resume and nothing into a prompt-mode session.
  Claude Code itself restores an open goal when it resumes a session; the tests' fake agent
  forgets it (Decisions, "What a real Claude Code does").
- Yulius knows when a person typed into a session's terminal (`typed_at_ms`, keystrokes only,
  SPEC-068) and when a prompt was submitted (the `UserPromptSubmit` hook).
- The tests with real processes are `crates/yulius/tests/claude_code_moves.rs`; the card is
  `crates/yulius-server/web/assets/claudecode.js` with `mock.js` and
  `e2e/tests/claude-code.e2e.ts`; notifications are `notifications.js`.

The findings, as the review gave them:

1. (minor) A prompt-mode session whose person set a goal of their own loses it at the idle
   restart: the turn closes with the goal open and no verdict, the task reads "Your turn", the
   session restarts, nothing is typed, the goal is gone and nobody is told. Reproduced by the
   reviewer: expected `(runs, "GOAL SET" on the screen) == (1, 1)`, got `(2, 0)`. (With the
   fake agent. With a real Claude Code the goal comes back by itself and what is lost is
   Yulius's reading of it: Decisions.)
2. (minor) CCM-013's "a gate's block" case has no test: dropping `gate_continues` from
   `works_on` in `hook_stop`, or not passing the gate's reasons on, fails nothing.
3. (nit) With moves stopped the card says "Running tasks are no longer moved to X" and, below
   it, "Each moves to X when its turn ends…"; a task's info makes the same promise.
4. (nit) `a_restarted_session_that_starts_in_time_stops_nothing` passes only if the restarted
   session starts within 10 s of wall clock.
5. (nit) The transcript's `version` is kept and shown as it is; a transcript line may be 4 MiB
   and the agent writes its own transcript.
6. (nit) Two turn ends that decide at once for one task register equal `MoveStart`s; the one
   whose switch is refused takes the other's entry out, so the restart that happens is not
   watched by the breaker.
7. (nit) When the binary in use cannot be read for its models, the card shows no badge and no
   warning, while `yulius doctor` warns.
8. (nit) An idle restart goes waiting, switching, waiting, so a person who is not looking gets
   the task's "Your turn" notification, and the desktop one, a second time.
9. (nit) SPEC-461's header says the owner's goal is "quoted under Context"; it is not.

The question of direction: the restart waits only while a person typed in the last minute
(`TYPED_RECENTLY_MS`); a draft that sits in the prompt box for longer is lost.

## Requirements

| ID | Requirement |
| --- | --- |
| CCF-001 | **A person's own goal keeps a prompt-mode session.** While Claude Code's goal is open (set or not met yet) in a prompt-mode session, the session is not restarted for a newer Claude Code (Yulius could not follow the goal across the restart: it types nothing into such a session and reads a goal only from the new run's lines; and a Claude Code that does not restore an open goal at a resume would lose it): not idle after a settled turn, and not at a Stop the gate blocks (the resumed session would get the gate's reasons and no goal). Both ask once the turn's lines were read, so a goal set in that very turn counts; a goal known before saves the look at the session. It moves at a later turn's end once the goal is met, failed or cleared, or at its next start. Goal-mode tasks are as before (their goal is typed again). |
| CCF-002 | **The gate's block case is tested.** With code changed in the worktree and no spec, a session on an older Claude Code moves at the Stop the gate blocks: its second run starts by resume in the same Claude session, that run's launch prompt is the gate's reasons, and one switch decision `claude_code` / `completed` is stored. |
| CCF-003 | **Only the next start is promised while moves are stopped.** While running tasks are no longer moved to the version runs get, the card's list of tasks on an older version says that each gets that version at its next start, and a task's info says the same instead of "when its turn ends" (`GET /api/tasks/{tid}`: `cli_moves_stopped`). |
| CCF-004 | **The "started in time" test does not depend on the machine's speed.** A restarted session's start (its SessionStart hook) takes the restart off the breaker's watch at a race point (`ClaudeMoveStarted`); the test asserts there that the restart is no longer watched, with a time to start it cannot reach. |
| CCF-005 | **What a transcript names is capped.** The Claude Code version kept for a run is at most 64 bytes and the model name at most 128 (the launch's own bound for a model id), cut on a character boundary with an ellipsis; a version that is not a plain `X.Y.Z` is still kept and shown. The model is cut in the parser. The version is cut where it is kept (`UsageAccumulator::note_cli_version`): SPEC-461's first-record rule (#1188, merged while this task ran) reads it from every record of the session's thread, so that is where every path ends. |
| CCF-006 | **One restart of a task at a time, each with its own identity.** A turn end that decides to restart a session while another restart of that task is under way leaves it to that one, and one that comes after it finds the session sent; it registers nothing and asks for no switch. Each registered start has its own identity, so a start is only ever taken off the watch by its own restart, its session's start or end, or its own time limit. |
| CCF-007 | **The card says when the binary could not be read.** When Yulius keeps Claude Code current and the binary runs get cannot be read for the models it knows, the card shows a warning badge and a sentence saying that whether it knows the default model is not known (`ClaudeCodeView.models_unread`), as `yulius doctor` warns. |
| CCF-008 | **An idle restart is no news.** A prompt-mode session that was idle and is restarted with nothing typed never reads Working: its task goes from switching back to the activity it had, and what Claude Code last recorded of the session's goal (unfinished or not) still counts, so the task reads as it did. The notification center remembers a task's state through a switch, so a task that reads after it as it did before gets no second notification, in the bell or on the desktop. |
| CCF-009 | **SPEC-461 says what is true.** Its Context quotes the owner's goal; CCM-008, CCM-011, CCM-012 and CCM-013 state the rules of this spec; ADR-0293, the operations guide and the API reference follow. |
| CCF-010 | **A draft keeps the session.** A session a person typed into since its last prompt was submitted (and since it started) is not restarted, however long ago they typed: what they typed may still sit in its prompt box. A prompt submitted by a person or by Yulius empties the box, once: a prompt that is held at its submit (an update, a pause, no subscription with room) is sent again by the hook client until it goes on, which is the same submit, so what is typed meanwhile is a draft. Claude Code's own wake-up empties nothing. The box counts as emptied at the prompt's hook, which Claude Code runs at the Enter, also for a prompt entered while a turn runs (seen with 2.1.322): what is typed after it is a draft. A prompt whose hook Yulius answers with an error goes on all the same (the hook client fails open) and empties the box. A prompt Yulius's own scope check holds (TSK-013) is blocked, and Claude Code puts its text back into the box: that submit empties nothing, also when the prompt first waited for a subscription with room. The minute after any keystroke stays as well (a person who just sent a prompt is still at the keyboard). |
| CCF-011 | **What Yulius typed counts until it is submitted.** A message Yulius typed into the session (a person's message through their Superagent or their own agent, a pull request's news, "Yulius is back") is a draft from the moment it is typed until its submit (its prompt hook). A command Claude Code runs itself (the MCP watch's `/mcp reconnect yulius`) submits no prompt and is no draft, and neither is a message that could not be typed. Claude Code submits at Yulius's own Enter, also while a turn runs, and then keeps the message in its queue until the turn ends: this rule covers the message from its paste to that submit only. A restart at the turn's end still loses a prompt that waits in that queue, a person's or Yulius's, as before this change (Decisions; issue #1204). |

## Plan

- [x] Spec SPEC-468 written and validated
- [x] A person's own goal keeps a prompt-mode session, and the gate's block case is tested (findings 1 and 2)
- [x] Two turn ends that decide at once make one watched restart, and the start test no longer depends on speed (findings 4 and 6)
- [x] A draft typed since the last prompt keeps the session (the question of direction)
- [x] The version and the model name a transcript names are capped (finding 5)
- [x] The card and task info promise only the next start while moves are stopped, and the card says when the binary could not be read (findings 3 and 7)
- [x] An idle restart never reads Working and notifies nobody again (finding 8)
- [x] SPEC-461, ADR-0293, the operations guide, the API reference and the changes log say so (finding 9)
- [x] Gate and the whole e2e suite green, evidence recorded
- [x] Independent review round 1 answered, every finding fixed with its test
- [x] A message Yulius typed that Claude Code still queues keeps the session, and the draft rule is checked against the real Claude Code
- [x] Independent review round 2 answered: a submit empties the box as of its Enter, and only what will be submitted counts
- [ ] Independent review round 3 answered: the rules say what holds with the real Claude Code (the Enter rule and the queue claim are withdrawn, issue #1204)

## Acceptance criteria

- AC1 (CCF-001). A prompt-mode session on 2.1.311 in which a person typed `/goal …`, whose turn
  closes with the goal open and no verdict while runs get 2.1.322, keeps its one run, its screen
  shows "GOAL SET" once and `App::goal_is_open` stays true. The same session at a Stop the gate
  blocks is answered with the block and keeps its run, without a look at it when the goal is
  known, and also when the goal was set in that very turn and is read only by that look.
- AC2 (CCF-002). The gate's block case moves as CCF-002 says; without `gate_continues` in
  `works_on`, or without the reasons passed on, the test fails.
- AC3 (CCF-003). With moves stopped for the version runs get, the card's list says "Each gets X
  at its next start." and not "when its turn ends"; a task's detail carries `cli_moves_to` and
  `cli_moves_stopped: true`, and its info says "gets X at its next start". Once runs get a
  newer version than the one moves stopped for, the detail promises that one with
  `cli_moves_stopped: false`.
- AC4 (CCF-004). The test of a restarted session that starts sets a time to start of an hour and
  passes; with the start not noticed it fails at once.
- AC5 (CCF-005). A version of 4 MiB leaves at most 64 bytes and an ellipsis, and
  `2.2.0-beta.3+build.7` is kept as it is (SPEC-461's test of the accumulator); a model name of
  4 MiB leaves at most 128 bytes and an ellipsis.
- AC6 (CCF-006). Two turn ends of one task that both decided to restart its session before
  either began (held at the race point `ClaudeMoveDecided`) make one restart (two runs),
  whether the second goes on while the first's switch is under way or once it is over; and
  that restart is watched: its session, which never starts, stops further restarts on that
  version. Two starts of one run on one version are not equal. A later release restarts the
  task again: a finished restart gives the task's slot back.
- AC7 (CCF-007). The view of a binary that could not be read has `models_read: false` and the
  sentence; the card shows the badge and the sentence (mock `claude=unread`); a server that
  keeps nothing current shows neither.
- AC8 (CCF-008). From "switching" on, no state of the task published during an idle restart
  reads working, and the last reads running and waiting. A prompt-mode session whose own goal
  failed still reads unfinished after its idle restart. In the UI a task that goes "Your
  turn", Continuing, "Your turn" has one notification, still read, while a switch after which
  it reads differently notifies.
- AC9 (CCF-010). With a keystroke five minutes old that came after the last submitted prompt,
  the turn's end does not look at the session at all; after the next submitted prompt it does.
  A keystroke before the last prompt, or before the run started, keeps nothing. What is typed
  while a prompt is held for a subscription with room is a draft through every request of
  that prompt and after it went on, until the next prompt is submitted. A prompt the scope
  check holds is answered with the block and leaves the session with a draft: the turn that
  ran meanwhile ends without a look at the session. A prompt that waited for a subscription
  with room and is held by the scope check once the limit reset leaves the session with a
  draft as well. A prompt whose hook is answered with an error leaves none.
- AC10 (CCF-011). In the test agent's order, which submits a line when it reads it after its
  turn: a prompt-mode session on 2.1.311 runs a turn the gate will block, while runs get
  2.1.322. Its person's Superagent sends it a message, which is typed into the session and
  answered "sent": the session has a draft. When the turn ends the session keeps its run, the
  message is answered there, and that turn's end restarts the session (two runs); the
  message's answer was printed before the restart. The MCP watch's reconnect command leaves
  no draft and its "continue" message does; a message that could not be typed leaves none.

## Test plan

- Unit, `crates/yulius-domain/src/agent_cli.rs`: `moves_now` with a draft, and when a keystroke
  is a draft (`is_draft`: after the last submit and the run's start; AC9).
  `crates/yulius-app/src/finish.rs`: which submitted prompts came through the prompt box.
  `crates/yulius-app/src/claude_code.rs`: a start is only taken off the watch as itself (AC6).
  `crates/yulius-app/src/supervisor.rs`: a new run starts with an empty prompt box and no
  held prompt. `crates/yulius-harness/src/claude.rs`: the model's cut (AC5; the version's is
  SPEC-461's `the_version_a_transcript_names_is_kept_short` in
  `crates/yulius-harness/tests/observe.rs`).
- Integration with real processes, `crates/yulius/tests/claude_code_moves.rs` (`mod moves`):
  AC1 (both cases), AC2, AC4, AC6 (both decisions held at `ClaudeMoveDecided` with
  `block_in_place`; the second goes on at `SwitchDecided`, or at `ClaudeMoveSettled`), AC8's
  server half (the task's published states during the restart, and a goal that failed), AC3's
  task detail, a new run's empty prompt box (AC9), and AC10 (the turn held with
  `--turn-hold-file`, the message sent with `task_message` as the Superagent does, the
  terminal's output read across the restart).
  `crates/yulius/tests/restart.rs`: the MCP watch's command and message (AC10).
- Integration with the test clock, `claude_code_moves.rs`: AC9 (hooks to the internal listener,
  the clock moved five minutes on, the race points `ClaudeMoveProbed` and `ClaudeMoveSettled`;
  no process runs, so a restart that is tried is refused, which shows the try), a prompt
  held at a usage limit, asked for again and let through once the limit reset, a prompt the
  scope check holds (a Jev that reads prompts as new scope), at once or after it waited for
  room, a prompt whose hook fails (its task deleted at a race point), and a message that
  cannot be typed because nothing runs.
- Integration, `crates/yulius/tests/claude_code_admin.rs`: AC7's view.
- e2e, `e2e/tests/claude-code.e2e.ts` (mock): AC3 (`claude=gap`), AC7 (`claude=unread`);
  `e2e/tests/notifications.e2e.ts`: AC8's UI half (`ambient=0`, the test changes the tasks).
- Mutations, each reverted (Evidence lists them): the goal not asked in `idle`, nor at the
  gate's block; `gate_continues` dropped from `works_on`; the reasons not passed on; the start
  not noticed; the caps removed; the slot not taken; a session already sent restarted again;
  starts without an identity; the draft not asked, a submit not emptying the box, a launch not
  emptying it, a wake-up emptying it (the predicate and its use), a held prompt emptying it at
  every request, a new run keeping a held prompt; a message Yulius typed not stamped, or its
  stamp not asked; a command stamped like a message; the stamp kept when nothing could be typed;
  a blocked prompt emptying the box, at once or at its first request while it waited; a failed
  hook not emptying it; the launch reading Working; the goal's state not carried; the Stop not
  asking about the goal again after its look, or not before it; a stop of moves read for every
  version; a finished restart keeping the task's slot; the notification center not remembering
  through a switch; the card's and task info's old words; the unread binary not said.
- Checked against real artifacts instead of the fake agent (Evidence): what Claude Code does
  with an open goal and with an unsent prompt across a restart, read in the binaries of three
  releases and in the transcripts of this instance's real sessions; and, by the reviewer and
  then by me, the installed Claude Code itself, run against a local stub of the model API with
  hooks that log their time (when a prompt's hook runs, what waits in the queue, what a
  blocked prompt and a blocked Stop do).
- Not testable here: a restart Yulius makes of a real, signed-in Claude Code session (no
  subscription can be signed in for a test), so the fake agent stands in for the process, and
  the stand-in for the browser's `Notification` for an operating system's notification. The
  fake agent submits a line when it reads it, after its turn, which is not the real order
  for a prompt entered while a turn runs: AC10 holds in that order only (issue #1204).

## Decisions

- **What a real Claude Code does, and what that makes of finding 1.** The review reproduced
  the lost goal with the fake agent, whose goal lives in its process. Claude Code restores an
  open goal by itself when a session is resumed: its start-up restore calls
  `restoreGoalFromTranscript`, which takes the condition of the conversation's last
  `goal_status` record that is neither met nor failed and registers the goal's Stop hook
  again (`tengu_goal_restored_on_resume`), unless the workspace is not trusted or hooks are
  restricted by policy. That is in the binaries of 2.1.292, 2.1.311 and 2.1.322 (read, never
  run), and one real session of this instance shows it: restarted from 2.1.311 on 2.1.322 it
  worked on for about 2,000 lines and its goal's "not met" verdict came before any new
  `/goal`. So with a real Claude Code the restart does not lose the goal. What it loses is
  Yulius's reading of it: the new run's lines name no goal, so until Claude Code records the
  next verdict Yulius takes a clean Stop for the turn's end and reads the task as waiting for
  its person, or as answered. The goal itself is lost only by a Claude Code that does not
  restore it: the fake agent, an untrusted workspace, restricted hooks.
- **Keep the session all the same.** It is what the request asked for, it holds whatever a
  Claude Code release does at a resume, and it keeps Yulius's reading true. Its cost is a
  later move: such a session keeps its version until the goal is met, failed or cleared, or
  its next start (stop and continue moves it at once), and the card lists it. Rejected: move
  it and carry Yulius's reading over, which would be faster and right with the releases read
  here, but stakes a person's goal and Yulius's waiting for its verdict on behavior Claude
  Code does not document, and needs a fake agent that restores goals to be tested at all.
  Rejected: type the goal again as for a goal task, since a goal a person set is theirs, a
  condition replayed from a record may not be what they would type now, and it would start
  work in a session that waits for its person.
- **The same rule at the gate's block.** The finding names the idle path; the Stop a gate
  blocks restarts a prompt-mode session too (with the gate's reasons), with the same loss of
  Yulius's reading. One rule covers both (`keeps_own_goal`).
- **The Stop asks about the goal twice** (review round 1 of this change, minor 1): once
  before its look at the session, which a goal known by then saves (the look waits for the
  transcript and asks the session's processes, inside the Stop hook), and once after it,
  because the look is what reads the turn's lines: a goal set in the very turn whose Stop the
  gate blocks is not read yet when that Stop arrives (the transcript is read once a second).
  Each has its test; the second cannot assert that the goal was unread at the Stop without
  failing in the rare run in which a read falls between, so it asserts what must hold either
  way, and its mutation was caught 3 runs of 3.
- **A goal that failed does not keep the session.** Claude Code's goal is over then; what is
  left is Yulius's reading of the task ("Your turn": work is unfinished, SPEC-429), which the
  idle restart carries over (CCF-008).
- **A subscription switch leaves Yulius without its reading of a person's goal too** (older
  code, SPEC-034 and SPEC-447: a prompt-mode resume types nothing). A probe with real
  processes showed it with the fake agent (a person's move of such a session: one run with
  the goal set and open before, two runs and no goal known after). Reported as issue #1182,
  whose text was corrected once the binaries were read: with a real Claude Code the goal
  stays in force there and only Yulius's reading is missing until the next verdict. It is not
  this change's code, and a move away from a subscription at its limit cannot simply wait as
  this restart can.
- **A draft is "typed since the last submitted prompt".** Yulius cannot read the prompt box,
  but it sees every keystroke of a viewer and every submitted prompt, so it can tell that
  something was typed and not sent. Keystrokes that are no draft (an answer to a permission
  dialog, a command that submits no prompt) count as one: the session then keeps its version
  until a turn ends with nothing typed since its prompt, or its next start. That errs toward
  the person at the keyboard, and usually costs one turn. The alternative, the minute alone,
  loses what a person typed ahead during a long turn: the binary of 2.1.322 keeps an unsent
  prompt on disk only for background jobs (`CLAUDE_JOB_DIR`), not for a session as Yulius
  runs it. The minute stays for the person who just sent a prompt and is about to type the
  next. "One turn" holds for a session that is prompted again; a goal session nobody prompts
  keeps its version until its next start after a single keystroke. Limits: the server keeps
  what was typed in memory, so after its own restart no draft is known until someone types
  again (as for the minute); and a prompt Claude Code submits by itself without a tag (a
  schedule the session set for itself) reads as a submit.
- **What a prompt entered while a turn runs really does** (review round 3). Until then this
  spec said that Claude Code runs such a prompt's hook only when it hands the prompt to the
  model. I had read that out of the binary ("prompt.submit at the fold") and out of the
  transcripts (a queued prompt's record follows its `dequeue` record). The reviewer ran the
  real 2.1.322 against a local stub of the model API, with hooks that log their time, and it
  is not so:
  - the hook comes 23 to 33 ms after the Enter, also for one bracketed paste and an Enter
    300 ms later, the way Yulius types (the binary's main path is "prompt.submit at Enter",
    in all three releases read);
  - the prompt then waits in Claude Code's queue (`queue-operation` `enqueue` in the
    transcript, with its text) until the Stop that ends the turn returned; two waiting
    prompts are handed on together as one turn, with one `dequeue` each and no further hook;
    a Stop the hook blocks does not release the queue (my own run of the same experiment);
  - with that Stop held and the process ended, as a restart ends it, `claude --resume`
    neither submits the waiting prompt nor asks the model.
  So the hook's time is when the box was emptied, as the first rule had it, and what is
  typed after a queued prompt's Enter is a draft under it. A prompt that waits in the queue
  is not in the prompt box: nothing in this change keeps the session for it.
- **Not in this change: the queue** (review round 3, its major). A restart at a turn's end
  loses a prompt that waits in Claude Code's queue: a person's, unless they typed in the last
  minute, and a message Yulius typed. That is on `dev` since SPEC-461 and stays so. Issue
  #1204 holds the facts and a way to fix it: count the transcript's `queue-operation`
  records, which the turn end's look already reads, and teach the fake agent the real order.
  Not done here: it is a design of its own (a new observation, a fake agent that queues as
  the real tool does, tests in that order), the review had reached its third round, and the
  loss is not this change's.
- **What Yulius typed is a draft until its submit** (found while the draft rule was checked
  against the binary, after review round 1; cut down to what holds in round 3). Yulius types
  into sessions itself: a person's message through their Superagent or their own agent, a
  pull request's news, "Yulius is back" (`type_into_session`). The time it typed is kept
  beside the person's keystrokes (`message_typed_ms`; not in `typed_at_ms`, which also says
  whose prompt comes next and holds other messages back for a minute) and the draft rule
  asks the later of the two. It is stamped before the text is typed, so its submit always
  comes after it. With Claude Code as it is that submit comes some 30 ms after Yulius's own
  Enter: the rule covers the 300 ms between the paste and the Enter and a paste whose Enter
  failed ("it may still be in its prompt box"), and no more. Its test with real processes
  runs in the fake agent's order, which submits a line when it reads it, after its turn;
  there the message keeps the session through that turn's end. I first took that order for
  the real one and wrote that this change keeps a queued message safe. It does not (the
  point above). Only what will be submitted is stamped: the MCP watch's
  `/mcp reconnect yulius` is a command Claude Code runs itself, no prompt hook follows it,
  and its stamp would have kept an idle session at its version until someone prompted it
  (review round 2); a text that could not be typed is taken back. The times are kept in
  memory, as the keystrokes' are. A subscription switch at a turn's end loses a queued
  message as well (older code): issue #1194.
- **The Enter rule came and went** (review rounds 2 and 3). Round 2 found that a queued
  prompt's submit forgets what was typed after its Enter, on this spec's own sentence that
  the hook comes at the hand-over. I answered with a rule that took the box as emptied at
  the last Enter typed (a viewer's, or Yulius's own). With the hook at the Enter there is
  nothing for it to do, and it had corners of its own (two waiting prompts, an Enter that
  only breaks a line, "Send anyway"), so it is gone: the hook's time is the rule again.
- **A prompt Yulius holds is still in the box** (found in the author's reading of round 2's
  answers; seen with the real tool in round 3). A typed prompt that the scope check holds
  (TSK-013) is answered with a block. Claude Code then shows the reason and puts the text
  back into the prompt box: the intake module checked that with 2.1.295, and the reviewer's
  runs of 2.1.322 show it for an idle session and for a prompt entered while a turn runs (a
  bare Enter, which is what "Send anyway" presses, submits the same text again). The first
  rule counted that hook as a submit, so a turn that ran meanwhile could end in a restart
  and the held text went with the old process, while the task's card still offered "Send
  anyway". A blocked prompt now empties nothing. For that the emptying is worked out at a
  prompt's first request and taken when the prompt goes on: one that waited for room and is
  then held by the scope check (which reads it again after the wait) empties nothing either.
  While a prompt waits for room its own text reads as a draft. With the hook at the Enter
  the turn before it can end meanwhile, and the session then stays, which is right: the
  prompt has not gone on (what the Stop itself makes of a prompt pause that is open at that
  moment is older code: issue #1206). Should Claude Code give a held hook up (its client
  holds for eight days), the prompt goes on unseen and the session reads as holding a draft
  until the prompt after the next one is submitted: later than needed, never as holding
  none. A prompt whose hook Yulius answers with an error goes on too, since the client fails
  open: the box is empty. An earlier text of this spec said a blocked prompt goes into a
  warning line and not back into the box; that read only half of the binary's path and was
  wrong. A held prompt a person then edits, or clears by hand, reads as a draft until the
  next prompt is submitted.
- **How two decisions for one task overlap.** The review read the race and did not reproduce
  it. It has an everyday trigger: a Stop hook that decided a restart is answered with a hold,
  and the hook client sends a held hook again every 10 s, while the switch waits up to 60 s
  for the old process to end. That second request decides again, with the same version and
  run. Two turn ends of one idle session (a prompt sent while the first's settling looks at
  the session) do the same, which is how the tests hold it.
- **One restart of a task at a time** rather than identity alone: with identity alone the
  second decision's entry would still replace the first's in the watch, and its switch would be
  asked for and refused (or, after the first ended, made a second time). The slot is held from
  the registration to the end of the switch; the identity makes "a start is only taken off by
  its own" hold whatever comes later.
- **An idle session starts idle** instead of being put back after the launch: the launch gave
  every run Working and `rest_after_idle_switch` put such a session back a moment later, which
  viewers saw as work. The switch now tells the launch what the task rests as (`IdleRest`),
  and the notification center treats a switch as what it is, under the hood. Both are needed:
  without the first a "Working" comes between, without the second "Continuing" resets what
  was told. This is the shared switch path, so a person's move of an idle task (SPEC-447)
  gains the same.
- **The rest carries what the goal left.** A new run learns its goal from its own lines, and
  an idle one writes none, so a prompt-mode task whose own goal had failed read "Answered"
  after the restart instead of "Your turn": a different reading, which notified. In the same
  session with nothing typed, what Claude Code last recorded is still what the agent left.
  It is kept in memory, like the goal's other readings: after a restart of the server the
  reattached run reads its own lines again, which hold no goal, and the task reads "Answered"
  (a failed own goal, an idle restart and a server restart together; the same holds for any
  resumed run since SPEC-429). A mark in the run directory would close it; not done for a
  case this narrow.
- **No guard for "a turn began in the new run"** (review round 1, nit): the put-back it
  replaced ran after the launch and had to step aside for a prompt sent at once. The launch
  gives the rest before the run is attached, when nobody can type and nothing is typed, so
  there is nothing to see there, and a prompt sent right after sets the task working through
  its hook.
- **Left as it is: a goal across `/clear`.** Yulius's reading of the goal is not reset when
  the session changes inside its process (older code), so a prompt-mode session whose person
  cleared it with a goal open keeps its version until its next start: the safe direction. The
  binary of 2.1.322 ends a goal at `/clear` (its telemetry names the reason `session_clear`)
  and can write the "cleared" record Yulius already reads; whether it writes one there could
  not be seen without running it.
- **Left as it is: the age of a task's state restarts at an idle restart.** Its status did
  change for a moment (`state_since`), so "Your turn" counts from the restart. Keeping the
  earlier time would need a store call of its own for a figure that tells nobody anything
  wrong: the task was continuing at that time.
- **The model name is capped too.** The review said the version should be capped "as the model
  name is"; it was not. It is the same record's word, kept per run and shown in the same cell,
  so it gets the same treatment.
- **The version's cut is SPEC-461's, where it is kept.** This task first cut the version in
  the parser's prompt records. Task #443's second delivery (#1188, merged into `dev` while
  this one was in review) reads the version from every record of the session's thread and
  cuts it in `UsageAccumulator::note_cli_version`, with the same rule and its own test; its
  spec says the two would otherwise have merged without a conflict and left the string
  unbounded on the new path. After that merge this task's cut was a second one on one of two
  paths, so it is gone; the model's stays in the parser, its only path.
- **The start test asserts the watch, not the judge.** With the restart off the watch, the judge
  has nothing to take, which `a_restarted_session_that_never_starts…` shows from the other
  side; waiting for the judge is what tied the test to the clock.
- **ADR-0293 is amended in place** (its Decision lists the move's rules); no new record: the
  rules of this spec refine that decision and change none of its alternatives.

## Evidence

- `scripts/dev ci` at ae55896c (every code and test change of this task, with `dev` up to
  25a0aaca merged): exit 0. `cargo check`, `fmt --check`, clippy `-D warnings` on Linux and
  for `aarch64-apple-darwin`, nextest 3852 passed / 16 skipped, doctests, `cargo deny`
  (advisories, bans, licenses, sources ok), `cargo shear`, `cargo doc`. After it come only
  this section, wording in this spec's AC9 and Decisions, one doc comment in `supervisor.rs`
  (`fmt --check`, clippy and `cargo doc` of that crate run again) and the Plan's last tick.
  Earlier runs passed at 42aa6838 (3648 tests), aa0916d9 (3649 passed, 16 skipped), 65515fc7
  (3707 passed, 16 skipped) and at the merge before this one (3757 passed, 16 skipped); the
  first run stopped at clippy (`type_complexity` in a new test helper, fixed).
- e2e, the whole suite with the web files of that head
  (`cd e2e && E2E_TELEMETRY_DISABLED=1 npx e2e run`): exit 0, 966 passed, 1 skipped (the agent
  test, which needs a model key), in 81 files and that one.
- Seen failing first, each for the reason its finding names:
  - `a_persons_own_goal_keeps_an_idle_prompt_session_where_it_is` and
    `…keeps_the_session_at_a_stop_the_gate_blocks`: `(2, 0, false)` instead of `(1, 1, true)`
    (runs, "GOAL SET" on the screen, goal open): the review's reproduction.
  - `two_turn_ends_that_decide_at_once_make_one_watched_restart`: "the restart was not
    watched" (no stop of moves after a session that never started).
  - `a_draft_typed_since_the_last_prompt_keeps_the_session`: the session was looked at and a
    restart tried with a draft five minutes old.
    `a_draft_typed_while_a_prompt_is_held_stays_a_draft` (found in the author's own review of
    the diff): the draft was forgotten at the held prompt's second request.
  - `a_message_yulius_typed_that_is_not_submitted_yet_keeps_the_session`: "the message was
    lost with the old process" (the session restarted at the first turn's end; the terminal
    shows the typed message and no answer to it).
  - `the_model_a_record_names_is_capped`: 4 MiB kept.
  - `a_running_task_moves_to_the_new_claude_code_when_its_turn_ends`: the published states
    read `switching, running/working, running/waiting`.
    `a_goal_that_failed_is_still_unfinished_after_an_idle_restart`: unfinished was forgotten.
  - `a_goal_set_in_the_turn_the_gate_blocks_keeps_the_session` (review round 1): the session
    was restarted with the gate's reasons.
  - Review round 2: `what_is_typed_after_a_queued_prompts_enter_stays_a_draft` (test clock),
    `what_a_viewer_types_after_a_queued_prompts_enter_stays_a_draft` and
    `a_draft_typed_while_a_message_of_yulius_waits_stays_a_draft` (real processes): what was
    typed after the Enter was forgotten at the queued prompt's submit.
    `a_task_whose_yulius_mcp_went_silent_is_reconnected_then_continues`: "a typed command is
    no draft". `a_message_that_could_not_be_typed_is_no_draft`: a draft was left behind by a
    message that was refused ("the task is not running").
  - The author's reading of round 2's answers: `a_prompt_yulius_holds_is_still_in_the_prompt_box`
    and `a_prompt_held_for_room_and_then_by_the_scope_check_is_still_in_the_box`: the hook was
    answered with the block and the session read as holding no draft.
  - e2e: the card's "Each moves to … when its turn ends" beside stopped moves, task info's
    "moves to … when its turn ends", no "models not read" badge; and in
    `notifications.e2e.ts` the read "Your turn" entry back as unread after the restart, and a
    second "Your turn" on the desktop.
  - `a_session_the_gate_blocks_moves_at_that_stop_with_the_gates_reasons` passed at once: the
    behavior worked, as the review said; its two mutations below show that it can fail.
- Mutations, each caught by the test named and reverted (`git checkout`):
  - finding 1: the goal not asked in `idle` (idle test); not asked at the Stop (gate-block
    goal test); after review round 1: not asked again after the Stop's look, 3 runs of 3, and
    not asked before it (the two gate-block goal tests).
  - finding 2: `gate_continues` dropped from `works_on`; the reasons not passed on (both: the
    gate-block move test).
  - finding 3: the card's old words; task info's old words (e2e); the detail's flag never set;
    the flag set for every version once moves stopped
    (`a_version_a_session_could_not_continue_on_gets_no_other_session`).
  - finding 4: the session's start not noticed (`a_restarted_session_that_starts_in_time…`,
    which now fails at once instead of after its time).
  - finding 5: the model uncapped (`the_model_a_record_names_is_capped`). The version's cut
    is SPEC-461's since #1188, with its own test.
  - finding 6: the slot not taken, 3 runs of 3 (`two_turn_ends_…`); a session already sent
    restarted again (`a_turn_end_that_decided_before_a_restart…`); starts without an identity
    (`a_start_is_only_taken_off_the_watch_as_itself`); a finished restart keeping the slot
    (the main move test, whose third release restarts the task again).
  - finding 7: the sentence never made; made for an adapter that keeps nothing current (view
    tests); the badge and the sentence not shown (e2e).
  - finding 8: the launch always Working (the main move test); the goal's state not carried
    (the failed-goal test); the notification center not remembering through a switch (e2e:
    the bell's entries, and with those assertions taken out the desktop's alone).
  - the draft: not asked in `moves_now`; a launch not emptying the box
    (`an_idle_session_is_idle_again_after_a_person_moves_it`, and the unit test of a new
    run); a wake-up emptying it (the predicate's unit test, and its use: the draft test's
    wake-up); a new run keeping a held prompt (unit test). On the prompt hook as it stands at
    the end: a submit not emptying the box (three draft tests); a held prompt worked out
    again at every request, and the hold not remembered (the held-prompt test); a blocked
    prompt emptying the box (both scope-check tests); a prompt's emptying taken at its first
    request while it waits (the second of them).
  - a queued prompt's Enter (review round 2): the rule always answering the hook's time (the
    three tests above and the rule's unit test); a viewer's Enter not read, in the record
    (two tests) and at the terminal's call (the test through the terminal's input path);
    Yulius's own Enter not kept (the test with a message waiting).
  - what Yulius typed: the time not stamped; the stamp not asked (both: the message test, at
    its first assertion; with that assertion taken out, at "the message was lost"); a command
    stamped like a message (the MCP watch's test); the stamp kept when nothing could be typed
    (its test).
- Stress: `claude_code_moves` (31 tests, real processes and the test clock) 6 times
  over, all passed.
- Checked against the real Claude Code instead of the fake agent. Its binaries were read with
  `strings`, never run: 2.1.322 as installed, 2.1.311 and 2.1.292 from the npm registry. The
  transcripts are those of this instance's own sessions (423 files; only record types, counts
  and times were printed).
  - An open goal is restored at a resume (`restoreGoalFromTranscript`, in all three), and one
    session restarted from 2.1.311 on 2.1.322 shows it: its goal's "not met" verdict comes
    about 2,000 lines after the restart and before any new `/goal`. This changed what finding
    1 is said to lose (Decisions) and the text of issue #1182.
  - An unsent prompt is kept on disk only for background jobs: a draft does not survive a
    restart, so the draft rule is needed.
  - A prompt entered while a turn runs is queued; its submit hook runs when it is handed to
    the model ("prompt.submit at the fold", or after the turn's end), and the ten prompts that
    waited over 5 s between their own enqueue and dequeue records have their prompt record
    within a second of the dequeue. The resume loader lists `queue-operation` records as
    ignored. This is what CCF-011 and the Enter rule came from. No transcript has two prompts
    waiting at once.
  - A prompt a hook blocks is put back into the prompt box: 2.1.322 lands a line a hook
    dropped in the composer, at Enter and at the fold, and the intake module records the same
    for 2.1.295. An earlier reading of mine, that it only goes into a warning line, was wrong
    (Decisions); the held-prompt rule came from the correction.
- Probes with real processes, not kept: a person's move of a prompt-mode session with an own
  goal open, `before=(1, 1, true) after=(2, 0, false)` (older code: issue #1182); a
  subscription switch at a turn's end with a message Yulius typed still queued,
  `runs=2 switches=[("Selected { tier: UnknownUsage }", "completed")] message_answered=false`
  (older code: issue #1194).
- The UI in the sandbox browser (mock, port 20117): `claude=gap` shows the stopped-moves
  warning and "Each gets 2.1.400 at its next start."; `claude=unread` shows the "models not
  read" badge and its sentence.
- `yulius security-scan`: "Security scan disabled: Vidoc is turned off in Yulius" (exit 0):
  disabled, not passed.
- Review. The author read the whole diff against `dev` before the independent round and found
  the held-prompt hole and one test wait that could race. The independent reviewer (a fresh
  session with the diff range, this spec and the skill `om-code-review`) ran the gate and the
  e2e suite itself each round.
  - Round 1 approved the head 5405c967 (3649 passed and 16 skipped, 932 passed and 1 skipped)
    with two minors and four nits, all answered in this pull request. Minor: the Stop asked
    about the goal before the turn's lines were read; it asks again after its look, with a
    test. Minor: three mutations survived (a wake-up emptying the box at its call site, a stop
    of moves read for every version, a finished restart keeping the slot); each has its test.
    Nits: a held prompt was not forgotten at a new run (`prompt_box_starts_empty`, unit
    test); the carried goal state is memory only (recorded under Decisions); `IdleRest`'s
    guard for a turn begun in the new run could not be reached (removed); two early returns
    of the move said nothing (they log at debug level).
  - Round 2 approved the head 41d57528 (3707 passed and 16 skipped, 966 passed and 1 skipped),
    found every round-1 finding answered, and brought two minors and five nits, all answered
    in this pull request. Minor: the MCP watch's reconnect command was stamped like a message
    (only what will be submitted is stamped now, with the watch's test). Minor: a queued
    prompt's submit forgot what was typed after its Enter (the Enter rule, with four tests).
    Nits: two assertions of the message test could not fail (removed); the stamp stayed when
    the paste failed (taken back, with a test); this section said three early returns log
    (two do); CCF-011's limit did not name a restart of the server (it does); the e2e README
    did not name the desktop check (it does). Two more are older code: a message typed onto a
    person's draft that is over a minute old (issue #1199), and a decision listed twice in
    the index on `dev` (gone since #1180).
  - After round 2 the author read its own answers once more and found the held prompt (two
    cases, Decisions), fixed with two tests. The same reviewer looked at everything since
    41d57528; its verdict goes with the merge.
- Not tested: a restart that Yulius makes of a real, signed-in Claude Code session (no
  subscription can be signed in for a test: the fake agent stands in for the process, and
  what the real one does was read in its binaries and transcripts); an operating system's
  notification (the stand-in receives what the page asks the browser to show); whether a real
  Claude Code writes its "cleared" record at `/clear` with a goal open (its binary ends the
  goal there); what a real Claude Code does with two prompts that wait at a turn's end; a
  deployed instance.
