---
yulius-task: 8fc5fdd4-88ab-4331-a805-d1db8c56115d
status: done
---

# SPEC-072: usage-limit resume respects the CLI's own continuation and refused switches

Status: done
Date: 2026-10-13
Owner: Yulius task #57
Scope authorization: task #57, part of task #47 ("fix all open actionable GitHub issues without
creating new issues and close them"): fix GitHub issues #276, #277 and #278, all follow-ups of
[SPEC-034](SPEC-034-in-previous-tasks-we-have-hit-a-session.md), by their acceptance criteria.

## Goal

Every way a turn cut by a usage limit goes on is safe and tested: a turn Claude Code continued by
itself is never ended by Yulius's resume loop (#276); a person's switch of a task left at its limit
by a refused automatic switch resumes past the limit like every other limit path (#277); and the
transcript step back runs off the async runtime, resets the session index only when it rewrote the
file, and the limit-resume paths SPEC-034 left untested have tests (#278).

## Context

- SPEC-034 / ADR-0027: every resume after a usage limit is `RunStart::AfterLimit` →
  `SessionMode::Resume { continue_interrupted: true, prompt: None }`; the harness adapter steps the
  transcript back inside `HarnessPort::plan`, and `launch_attempt` then resets the session's search
  index (`reindex_session`).
- #276: Claude Code 2.1.292 sometimes continues the cut turn itself at the reset ("continuing
  automatically at 7:40pm"), with a hidden `isMeta` prompt (not a `UserTurn` observation) and then
  ordinary assistant records (`AgentWork`). The resume loop also wakes at the reset (+1 s) and, for a
  `limit` pause with a process, calls `switch_after_limit`, which sends SIGTERM because an automatic
  switch assumes an idle agent. `hold_overtaken` (`finish.rs`) only ends `turn` and `prompt`
  pauses, and only on a `UserTurn` or `TurnInterrupted`, so the CLI's continuation never closes
  the `limit` pause and a tool call in flight can be killed.
- #277: `App::switch_task` resumes past the limit only when an open `limit` pause exists. The
  reactive switch after a limit (`rotate` → `switch_after_limit`) can be refused in preflight
  (credential or isolation, ROT-014); `refuse_switch` leaves the task running at its limit message
  with no pause, and a person's switch then resumes with "Continue the task exactly where you left
  off." and the limit records still in the transcript.
- #278: `yulius_harness::step_back::step_back_file` reads, rewrites and fsyncs a transcript of up
  to tens of MB inside the synchronous `plan`, called from async `launch_attempt`, blocking a
  runtime worker; the index is reset after every continued resume even when nothing changed; a
  `limit` pause kept when no subscription has capacity at launch, a process-gone `limit` pause and
  goal mode's opening-turn wait after a continued turn have no test.

## Requirements

| ID | Requirement | Evidence |
| --- | --- | --- |
| LRS-001 | An open `limit` pause ends as `overtaken` (the task runs again, Working) when the transcript shows a turn after the pause started: a prompt, an interrupt, agent work (an assistant record that is not an error, a sub-agent's or Claude Code's own synthetic line) or the session's own turn end, with a timestamp after `pause.started_at`. The resume loop then finds no pause and never ends that run; its switch past a limit requires the task to still wait for capacity. After a restart, a replayed limit is acted on only when no prompt, agent work or session turn end follows it, and a `limit` pause older than the replay's last such record ends as `overtaken` (Claude Code continued while Yulius was down). A sub-agent's turn end counts for none of this. `turn` and `prompt` pauses keep their rule (a prompt or interrupt, with an account). | `pool_supervision.rs`: the agent continues by itself (a tool call, or only an answer) before capacity returns: one attempt, still live, no termination; a restart over such a transcript records nothing and pauses nothing; unit `a_replayed_limit_is_pending_until_a_turn_follows_it` |
| LRS-002 | A task whose last observed event is a usage limit (no prompt, agent work or turn end since, same run) is "at its limit" even without a pause. A person's switch (`POST /api/tasks/{tid}/switch`) of such a task launches `Resume { continue_interrupted: true, prompt: None }`, like a task at a `limit` pause. The marker is per run and in memory; a new run clears it, and a restart sets it again only from a replayed limit still pending (LRS-001). | `pool_supervision.rs`: a refused limit switch, then a person's switch |
| LRS-003 | The step back runs through its own harness port method (`step_back_transcript`) on a blocking thread (`spawn_blocking`), not inside `plan`; it reports whether it rewrote the file. A failure is logged and the resume goes on (LIM-002). The session index is reset only after a rewrite. | `yulius-harness` unit test `step_back_file_rewrites_only_when_needed`; `pool_supervision.rs` limit tests (stepped back, index) |
| LRS-004 | The SPEC-034 paths have behavioral tests: a resume past a limit that finds no capacity at launch waits in a `limit` pause without a process; that pause resumes, once capacity returns, with `continue_interrupted: true`; in goal mode `/goal` is typed after the continued turn's Stop, not during it. | `pool_supervision.rs`, `task_done.rs` |

## Plan

- [x] Spec SPEC-072 written and validated
- [x] A limit pause ends when the CLI continues the turn by itself (#276)
- [x] A person's switch after a refused limit switch resumes past the limit (#277)
- [x] The transcript step back runs off the async runtime and the index resets only after a rewrite (#278)
- [x] The untested limit-resume paths have tests (#278)
- [x] Review round 1 findings fixed test-first (replayed limit, answer-only continuation)
- [x] Docs, ADR and changes written (SPEC-002, SPEC-034, ADR-0053, architecture)
- [x] Independent review approves the head (three rounds)
- [x] Every gate green on the final head

## Acceptance criteria

- A `limit` pause ends as "overtaken" when the transcript shows a turn after `pause.started_at`; a
  limit followed by the agent's own continuation before the resume loop runs leaves one attempt
  and no termination (#276).
- A refused limit switch followed by a manual switch launches
  `Resume { continue_interrupted: true, prompt: None }` (#277).
- The step back runs in `spawn_blocking` and the launch learns whether it changed the file; the
  session index is reset only after a rewrite; no capacity at an after-limit launch leaves a
  `limit` pause; a process-gone `limit` pause resumes with `continue_interrupted: true`; goal mode
  types `/goal` after the continued turn's Stop (#278).

## Test plan

- Fake agent: `--limit-continues-ms MS` stands for Claude Code's "continuing automatically": MS
  after the limit the agent continues the cut turn by itself (a hidden `isMeta` prompt, an
  assistant `tool_use` record, then the answer ending the turn) and prints `CONTINUED` and `DONE
  continued`; `--limit-continues-answer-only` leaves out the tool call. The fake harness gives a continued resume (`continue_interrupted`) the
  `--opening-turn-ms` turn, as Claude Code redoes the cut turn on such a resume.
- Integration (`pool_supervision.rs`, real ptyhost, fake agent, scripted usage):
  - the CLI's own continuation (a tool call, or only the answer) ends the `limit` pause; capacity
    returning later starts no second attempt and the first stays live (fails before the fix: the
    pause never ends);
  - a restart over a transcript with a limit and the CLI's continuation: the subscription's usage
    is not recorded again and no pause opens (fails before the fix: the limit is replayed);
  - Yulius down while the task waits at its limit and Claude Code continues: the replay ends the
    pause and capacity returning starts no second run (fails without the catch-up check);
  - a reactive limit switch refused in preflight (the other subscription's credential cannot be
    opened), then a person's switch: the launch is a continued resume without a prompt and the
    transcript has no limit record (fails before the fix: "Continue the task exactly…");
  - a person's switch at a `limit` pause when no subscription has capacity: the task waits in a
    `limit` pause without a process; capacity returns and the resume loop launches a continued
    resume; the session index is reset only when the step back rewrote the transcript.
- Integration (`task_done.rs`, hooks): a goal task hits the limit, moves to another subscription,
  the continued turn runs, and `/goal` is typed after its `DONE`; with hooks, an answer-only
  continuation ends the `limit` pause and its Stop hook is answered on the first run.
- Unit (`yulius-harness`): `step_back_file` returns whether it rewrote the file (existing).

## Decisions

- **The CLI's continuation wins** (ADR-0053): agent work after the pause started is proof the CLI
  went on by itself, so the `limit` pause ends like a held turn the CLI gave up. Rejected: making a
  limit resume interrupt instead of terminate while a turn runs (it would still restart a session
  that is working, and an interrupt can cancel a tool call too). Residual race: the resume loop
  wakes at the reset + 1 s, and Claude Code's first record of its continuation lands after the
  model's latency (its continuation prompt is hidden). Ending the run before that record is
  harmless: no tool has started, and the continued resume redoes the request. The harmful window is
  a tool call already started but not yet read: one transcript poll (≈ 1 s). The loop rechecks the
  pause just before it switches, and the switch requires the task to still wait for capacity.
  Known limit: a resumed attempt reattached after a restart is read from the end of its transcript,
  so a continuation Claude Code wrote while Yulius was down is not seen, and the resume loop may end
  that run (as before this change). A first-run attempt is replayed from its start and is covered.
- **A refused limit switch is remembered in the run's runtime, not as a pause** (ADR-0053): a
  `limit` pause would show the task waiting for capacity it has, and the resume loop would retry a
  switch that preflight refuses every round. The marker follows the transcript: a limit sets it, a
  prompt, agent work or a turn's end clears it, a new run clears it. Known limit: a restart
  restores it only for a first-run attempt (its transcript is replayed from the start); after a
  restart of a resumed attempt, a person's switch sends the continuation prompt as before.
- **A separate port method instead of an async `plan`**: the step back is the only blocking I/O of
  `plan` worth moving, and a sync method run in `spawn_blocking` keeps the port synchronous and the
  fake harness unchanged in shape.

## Evidence

- Regression tests first (commits marked "failing"): `a_turn_the_cli_continues_by_itself_ends_the_limit_pause`
  failed with "timed out waiting for the pause to end"; `a_manual_switch_after_a_refused_limit_switch_resumes_past_the_limit`
  failed with `prompt: Some("Continue the task exactly where you left off.")` and a limit record
  left in the transcript; `a_resume_past_a_limit_keeps_the_index_of_a_transcript_it_did_not_rewrite`
  failed with `indexed_bytes` 0 instead of 40 without the rewrite guard. Review round 1:
  `a_continuation_that_only_answers_ends_the_limit_pause` timed out and
  `a_restart_does_not_replay_a_limit_the_cli_continued_past` saw the account's usage recorded again
  before the fixes. Review round 2: `a_continuation_while_yulius_is_down_ends_the_limit_pause_at_restart`
  timed out with the catch-up check disabled.
- Mutation checks: the no-capacity launch recording a `start` pause fails
  `a_limit_pause_without_a_process_resumes_past_the_limit` (`Start` vs `Limit`); removing the
  continued turn from `opens_with_turn` fails `the_goal_is_typed_after_the_turn_continued_past_a_limit`
  ("the goal was typed during the continued turn").
- Unit: `finish::tests::a_replayed_limit_is_pending_until_a_turn_follows_it`,
  `a_replayed_transcript_reduces_to_its_final_state`; `yulius-harness` observe fixtures carry the
  turn end's time.
- Flaky test met in the gate and fixed: `task_done::a_message_queued_before_the_start_reaches_the_agent`
  failed 1 of 3 runs alone (`Conflict("task is running")`): the previous task's stop frees a slot and
  `drain_queue` starts the queued task before the test's own `start_run`. The test accepts that
  outcome; 6 of 6 runs passed after. The new restart test's wait also covers the status change that
  follows the pause's end (it raced once under gate load).
- `scripts/dev ci` on `94726c1` (after merging `dev`): green, 1356 tests run, 1356 passed, 16
  skipped; repository contracts, fmt, clippy, deny, shear and docs passed. No web UI changed, so
  the e2e suite was not run.
- Independent review in fresh sessions (om-code-review): round 1 requested changes (a replayed
  limit after a restart, an answer-only continuation, minors); round 2 requested changes (a pause
  open across a restart, the turn gate after a replayed limit, sub-agent turn ends, hooks mode); all
  fixed test-first. Round 3 approved `3462007` with no blocker or major; its minors (the hooks
  test's wait for the status, preconditions in the two restart tests, this Plan's wording) are
  fixed, and the reviewer confirmed the delta.
- Known follow-up (round 3, minor, not fixed here): a local slash command typed at a `limit`
  pause (`/usage`) is a prompt line and ends the pause like any prompt, as for `turn` and `prompt`
  pauses; the task then waits for its person instead of resuming at the reset. Noted on #276.
- Not tested here: a real Claude Code session through Yulius (the fake agent stands in); a resumed
  attempt reattached after a restart does not see a continuation written while Yulius was down
  (recorded as a known limit in Decisions and ADR-0053); a limit that cuts a turn mid-way while
  Yulius is down (the fake agent writes its limit after a completed turn; the `at_limit` gate in
  `on_observation` covers it by reading).
