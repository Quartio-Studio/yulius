---
yulius-task: 8fc5fdd4-88ab-4331-a805-d1db8c56115d
status: in-progress
---

# SPEC-072: usage-limit resume respects the CLI's own continuation and refused switches

Status: in progress
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
| LRS-001 | An open `limit` pause ends as `overtaken` (the task runs again, Working) when the transcript shows a turn after the pause started: a prompt, an interrupt, or agent work (an assistant record that is not an error, a sub-agent's or Claude Code's own synthetic line) with a timestamp after `pause.started_at`. The resume loop then finds no pause and never ends that run. `turn` and `prompt` pauses keep their rule (a prompt or interrupt, with an account). | `pool_supervision.rs`: a limit, then the agent continues by itself before capacity returns: one attempt, still live, no termination |
| LRS-002 | A task whose last observed event is a usage limit (no prompt or agent work since, same run) is "at its limit" even without a pause. A person's switch (`POST /api/tasks/{tid}/switch`) of such a task launches `Resume { continue_interrupted: true, prompt: None }`, like a task at a `limit` pause. The marker is per run and in memory; a new run clears it. | `pool_supervision.rs`: a refused limit switch, then a person's switch |
| LRS-003 | The step back runs through its own harness port method (`step_back_transcript`) on a blocking thread (`spawn_blocking`), not inside `plan`; it reports whether it rewrote the file. A failure is logged and the resume goes on (LIM-002). The session index is reset only after a rewrite. | `yulius-harness` unit test `step_back_file_rewrites_only_when_needed`; `pool_supervision.rs` limit tests (stepped back, index) |
| LRS-004 | The SPEC-034 paths have behavioral tests: a resume past a limit that finds no capacity at launch waits in a `limit` pause without a process; that pause resumes, once capacity returns, with `continue_interrupted: true`; in goal mode `/goal` is typed after the continued turn's Stop, not during it. | `pool_supervision.rs`, `task_done.rs` |

## Plan

- [x] Spec SPEC-072 written and validated
- [ ] A limit pause ends when the CLI continues the turn by itself (#276)
- [ ] A person's switch after a refused limit switch resumes past the limit (#277)
- [ ] The transcript step back runs off the async runtime and the index resets only after a rewrite (#278)
- [ ] The untested limit-resume paths have tests (#278)
- [ ] Independent review and fixes
- [ ] Docs, ADR, changes and every gate green

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
  continued`. The fake harness gives a continued resume (`continue_interrupted`) the
  `--opening-turn-ms` turn, as Claude Code redoes the cut turn on such a resume.
- Integration (`pool_supervision.rs`, real ptyhost, fake agent, scripted usage):
  - the CLI's own continuation ends the `limit` pause; capacity returning later starts no second
    attempt and the first stays live (fails before the fix: a second attempt);
  - a reactive limit switch refused in preflight (the other subscription's credential cannot be
    opened), then a person's switch: the launch is a continued resume without a prompt and the
    transcript has no limit record (fails before the fix: "Continue the task exactly…");
  - a person's switch at a `limit` pause when no subscription has capacity: the task waits in a
    `limit` pause without a process; capacity returns and the resume loop launches a continued
    resume; the session index is reset only when the step back rewrote the transcript.
- Integration (`task_done.rs`, hooks): a goal task hits the limit, moves to another subscription,
  the continued turn runs, and `/goal` is typed after its `DONE`.
- Unit (`yulius-harness`): `step_back_file` returns whether it rewrote the file (existing).

## Decisions

- **The CLI's continuation wins** (ADR-0051): agent work after the pause started is proof the CLI
  went on by itself, so the `limit` pause ends like a held turn the CLI gave up. Rejected: making a
  limit resume interrupt instead of terminate while a turn runs (it would still restart a session
  that is working, and an interrupt can cancel a tool call too). Residual race: the resume loop and
  the CLI both act at the reset; the loop sees the continuation only after the transcript watcher's
  next poll (≈ 1 s). The resume loop rechecks that the pause is still open just before it switches,
  so the window is that poll interval, not the whole round.
- **A refused limit switch is remembered in the run's runtime, not as a pause** (ADR-0051): a
  `limit` pause would show the task waiting for capacity it has, and the resume loop would retry a
  switch that preflight refuses every round. The marker follows the transcript: a limit sets it, a
  prompt or agent work clears it, a new run clears it. Known limit: it does not survive a Yulius
  restart; a person's switch after a restart sends the continuation prompt as before.
- **A separate port method instead of an async `plan`**: the step back is the only blocking I/O of
  `plan` worth moving, and a sync method run in `spawn_blocking` keeps the port synchronous and the
  fake harness unchanged in shape.
