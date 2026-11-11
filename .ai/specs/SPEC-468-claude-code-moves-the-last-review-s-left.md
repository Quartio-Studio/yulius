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

The restart of a running task on a newer Claude Code (SPEC-461 CCM-013) loses nothing a person
left in the session and tells nobody twice: a goal a person set in a prompt-mode session and a
draft in its prompt box keep the session where it is; an idle restart never reads as work and
does not notify again; a restart is always watched by the breaker; and the card and a task's
info promise only what will happen. The gaps in tests and wording the review named are closed.

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
- Claude Code's goal (`/goal …`) lives in the process: a resumed session has none until it is
  typed again. Yulius reads its state from the transcript (`goal_open`: set or not met yet).
- Yulius knows when a person typed into a session's terminal (`typed_at_ms`, keystrokes only,
  SPEC-068) and when a prompt was submitted (the `UserPromptSubmit` hook).
- The tests with real processes are `crates/yulius/tests/claude_code_moves.rs`; the card is
  `crates/yulius-server/web/assets/claudecode.js` with `mock.js` and
  `e2e/tests/claude-code.e2e.ts`; notifications are `notifications.js`.

The findings, as the review gave them:

1. (minor) A prompt-mode session whose person set a goal of their own loses it at the idle
   restart: the turn closes with the goal open and no verdict, the task reads "Your turn", the
   session restarts, nothing is typed, the goal is gone and nobody is told. Reproduced by the
   reviewer: expected `(runs, "GOAL SET" on the screen) == (1, 1)`, got `(2, 0)`.
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
| CCF-001 | **A person's own goal keeps a prompt-mode session.** While Claude Code's goal is open (set or not met yet) in a prompt-mode session, the session is not restarted for a newer Claude Code: not idle after a settled turn, and not at a Stop the gate blocks (the resumed session would get the gate's reasons and no goal). It moves at a later turn's end once the goal is met, failed or cleared, or at its next start. Goal-mode tasks are as before (their goal is typed again). |
| CCF-002 | **The gate's block case is tested.** With code changed in the worktree and no spec, a session on an older Claude Code moves at the Stop the gate blocks: its second run starts by resume in the same Claude session, that run's launch prompt is the gate's reasons, and one switch decision `claude_code` / `completed` is stored. |
| CCF-003 | **Only the next start is promised while moves are stopped.** While running tasks are no longer moved to the version runs get, the card's list of tasks on an older version says that each gets that version at its next start, and a task's info says the same instead of "when its turn ends" (`GET /api/tasks/{tid}`: `cli_moves_stopped`). |
| CCF-004 | **The "started in time" test does not depend on the machine's speed.** A restarted session's start (its SessionStart hook) takes the restart off the breaker's watch at a race point (`ClaudeMoveStarted`); the test asserts there that the restart is no longer watched, with a time to start it cannot reach. |
| CCF-005 | **What a transcript names is capped.** The Claude Code version kept for a run is at most 64 bytes and the model name at most 128 (the launch's own bound for a model id), cut on a character boundary with an ellipsis; a version that is not a plain `X.Y.Z` is still kept and shown. |
| CCF-006 | **One restart of a task at a time, each with its own identity.** A turn end that decides to restart a session while another restart of that task is under way leaves it to that one, and one that comes after it finds the session sent; it registers nothing and asks for no switch. Each registered start has its own identity, so a start is only ever taken off the watch by its own restart, its session's start or end, or its own time limit. |
| CCF-007 | **The card says when the binary could not be read.** When Yulius keeps Claude Code current and the binary runs get cannot be read for the models it knows, the card shows a warning badge and a sentence saying that whether it knows the default model is not known (`ClaudeCodeView.models_unread`), as `yulius doctor` warns. |
| CCF-008 | **An idle restart is no news.** A prompt-mode session that was idle and is restarted with nothing typed never reads Working: its task goes from switching back to the activity it had, and what Claude Code last recorded of the session's goal (unfinished or not) still counts, so the task reads as it did. The notification center remembers a task's state through a switch, so a task that reads after it as it did before gets no second notification, in the bell or on the desktop. |
| CCF-009 | **SPEC-461 says what is true.** Its Context quotes the owner's goal; CCM-008, CCM-011, CCM-012 and CCM-013 state the rules of this spec; ADR-0293, the operations guide and the API reference follow. |
| CCF-010 | **A draft keeps the session.** A session a person typed into since its last prompt was submitted (and since it started) is not restarted, however long ago they typed: what they typed may still sit in its prompt box. A prompt submitted by a person or by Yulius empties the box; Claude Code's own wake-up does not. The minute after any keystroke stays as well (a person who just sent a prompt is still at the keyboard). |

## Plan

- [x] Spec SPEC-468 written and validated
- [x] A person's own goal keeps a prompt-mode session, and the gate's block case is tested (findings 1 and 2)
- [x] Two turn ends that decide at once make one watched restart, and the start test no longer depends on speed (findings 4 and 6)
- [x] A draft typed since the last prompt keeps the session (the question of direction)
- [x] The version and the model name a transcript names are capped (finding 5)
- [x] The card and task info promise only the next start while moves are stopped, and the card says when the binary could not be read (findings 3 and 7)
- [x] An idle restart never reads Working and notifies nobody again (finding 8)
- [x] SPEC-461, ADR-0293, the operations guide, the API reference and the changes log say so (finding 9)
- [ ] Gate green and the independent review answered

## Acceptance criteria

- AC1 (CCF-001). A prompt-mode session on 2.1.311 in which a person typed `/goal …`, whose turn
  closes with the goal open and no verdict while runs get 2.1.322, keeps its one run, its screen
  shows "GOAL SET" once and `App::goal_is_open` stays true. The same session at a Stop the gate
  blocks is answered with the block and keeps its run.
- AC2 (CCF-002). The gate's block case moves as CCF-002 says; without `gate_continues` in
  `works_on`, or without the reasons passed on, the test fails.
- AC3 (CCF-003). With moves stopped for the version runs get, the card's list says "Each gets X
  at its next start." and not "when its turn ends"; a task's detail carries `cli_moves_to` and
  `cli_moves_stopped: true`, and its info says "gets X at its next start".
- AC4 (CCF-004). The test of a restarted session that starts sets a time to start of an hour and
  passes; with the start not noticed it fails at once.
- AC5 (CCF-005). A prompt record whose `version` is 4 MiB long leaves at most 64 bytes and an
  ellipsis; `2.2.0-beta.3+build.7` is kept as it is; a model name of 4 MiB leaves at most 128
  bytes and an ellipsis.
- AC6 (CCF-006). Two turn ends of one task that both decided to restart its session before
  either began (held at the race point `ClaudeMoveDecided`) make one restart (two runs),
  whether the second goes on while the first's switch is under way or once it is over; and
  that restart is watched: its session, which never starts, stops further restarts on that
  version. Two starts of one run on one version are not equal.
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
  A keystroke before the last prompt, or before the run started, keeps nothing.

## Test plan

- Unit, `crates/yulius-domain/src/agent_cli.rs`: `moves_now` with a draft, and when a keystroke
  is a draft (`is_draft`: after the last submit and the run's start; AC9).
  `crates/yulius-app/src/finish.rs`: which submitted prompts came through the prompt box.
  `crates/yulius-app/src/claude_code.rs`: a start is only taken off the watch as itself (AC6).
  `crates/yulius-app/src/supervisor.rs`: an idle rest holds until a turn begins in the new run.
  `crates/yulius-harness/src/claude.rs`: the caps (AC5).
- Integration with real processes, `crates/yulius/tests/claude_code_moves.rs` (`mod moves`):
  AC1 (both cases), AC2, AC4, AC6 (both decisions held at `ClaudeMoveDecided` with
  `block_in_place`; the second goes on at `SwitchDecided`, or at `ClaudeMoveSettled`), AC8's
  server half (the task's published states during the restart, and a goal that failed), AC3's
  task detail, and a new run's empty prompt box (AC9).
- Integration with the test clock, `claude_code_moves.rs`: AC9 (hooks to the internal listener,
  the clock moved five minutes on, the race points `ClaudeMoveProbed` and `ClaudeMoveSettled`;
  no process runs, so a restart that is tried is refused, which shows the try).
- Integration, `crates/yulius/tests/claude_code_admin.rs`: AC7's view.
- e2e, `e2e/tests/claude-code.e2e.ts` (mock): AC3 (`claude=gap`), AC7 (`claude=unread`);
  `e2e/tests/notifications.e2e.ts`: AC8's UI half (`ambient=0`, the test changes the tasks).
- Mutations, each reverted (Evidence lists them): the goal not asked in `idle`, nor at the
  gate's block; `gate_continues` dropped from `works_on`; the reasons not passed on; the start
  not noticed; the caps removed; the slot not taken; a session already sent restarted again;
  starts without an identity; the draft not asked, a submit not emptying the box, a launch not
  emptying it, a wake-up emptying it; the launch reading Working; the goal's state not carried;
  the rest held over a new turn; the notification center not remembering through a switch; the
  card's and task info's old words; the unread binary not said.
- Not testable here: a real Claude Code's goal across `--resume` (the fake agent stands in: its
  goal lives in its process, as Claude Code's does), and a real prompt box (Yulius sees
  keystrokes and submitted prompts, not the box).

## Decisions

- **Keep the session, do not type a person's goal again.** Yulius could read the goal's
  condition from the transcript and type `/goal …` into the resumed session, as it does for a
  goal task. But a goal a person set is theirs: typing it again starts work in a session that
  was waiting for its person (the reason idle goal sessions are not restarted, SPEC-461), and a
  condition replayed from a record may not be what they would type now. The session keeps its
  version until the goal is decided or its next start; the card lists it.
- **The same rule at the gate's block.** The finding names the idle path; the Stop a gate
  blocks restarts a prompt-mode session too (with the gate's reasons), and loses the goal the
  same way. One rule covers both (`keeps_own_goal`).
- **A goal that failed does not keep the session.** Claude Code's goal is over then; what is
  left is Yulius's reading of the task ("Your turn": work is unfinished, SPEC-429), which the
  idle restart carries over (CCF-008).
- **A subscription switch loses a person's goal too** (older code, SPEC-034 and SPEC-447: a
  prompt-mode resume types nothing). Confirmed by a probe with real processes (a person's move
  of such a session: one run with the goal set and open before, two runs and no goal after)
  and reported as issue #1182: it is not this change's code, and a move away from a
  subscription at its limit cannot simply wait as this restart can.
- **A draft is "typed since the last submitted prompt".** Yulius cannot read the prompt box,
  but it sees every keystroke of a viewer and every submitted prompt, so it can tell that
  something was typed and not sent. Keystrokes that are no draft (an answer to a permission
  dialog, a command that submits no prompt) count as one: the session then keeps its version
  until a turn ends with nothing typed since its prompt, or its next start. That errs toward
  the person at the keyboard, and costs one turn. The alternative, the minute alone, loses what
  a person typed ahead during a long turn. The minute stays for the person who just sent a
  prompt and is about to type the next.
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
- **Left as it is: the age of a task's state restarts at an idle restart.** Its status did
  change for a moment (`state_since`), so "Your turn" counts from the restart. Keeping the
  earlier time would need a store call of its own for a figure that tells nobody anything
  wrong: the task was continuing at that time.
- **The model name is capped too.** The review said the version should be capped "as the model
  name is"; it was not. It is the same record's word, kept per run and shown in the same cell,
  so it gets the same treatment in the same place (the parser).
- **The start test asserts the watch, not the judge.** With the restart off the watch, the judge
  has nothing to take, which `a_restarted_session_that_never_starts…` shows from the other
  side; waiting for the judge is what tied the test to the clock.
- **ADR-0293 is amended in place** (its Decision lists the move's rules); no new record: the
  rules of this spec refine that decision and change none of its alternatives.
