---
yulius-task: e5d31bdb-b25e-4a37-afbb-44a90f083b0e
status: in-progress
---

# SPEC-229: A reboot leaves finished tasks finished and every task its last screen

Status: in-progress
Date: 2026-10-19
Owner: task #215
Scope authorization: task #215: "this can't be this way, something got fucked up. Identify and
fix" (two screenshots: every task of two projects "Stopped (server restarted)" with a warning and a
blank terminal). Extends SPEC-002 §7.1 (reconcile never respawns) and SPEC-001 TSK-014 (Done);
decision in [ADR-0177](../../docs/decisions/0177-a-run-lost-after-its-goal-closes-the-task.md).

## Goal

After the machine reboots, a task whose goal was achieved before the reboot reads as finished
(Closed, no warning), and every task's terminal shows the screen its last turn ended on instead
of a blank one. Tasks whose run the reboot cut stay "Stopped (server restarted)", continuable, with
a message that does not claim the agent was working.

## Context

Reproduced on the instance (server 77925fd, main) on 2026-10-19.

- **Symptom.** The host booted at 2026-10-18 17:54 UTC (`/proc/uptime`). Every task that had a
  session then is `stopped_server_lost` (updated 17:54:12Z), in two projects: #213 and #214 of
  this project, whose goals were achieved and whose pull requests (#676, #681) were merged hours
  before (`Activity::Done`, in the sidebar's Done group), and #1-#3 of another project. The
  sidebar shows them all under Stopped with a warning mark, the banner says "Yulius restarted
  while the agent was working" (untrue for the idle ones) and the terminal is blank.
- **Root cause, status.** A reboot ends every session host. `Supervisor::reconcile`
  (`crates/yulius-app/src/supervisor.rs`) cannot reattach, finds the run `Recorded` but dead or
  `Missing`, and calls `mark_lost(.., StopReason::ServerLost)`, which applies
  `TaskEvent::Lost` whatever the task was doing. A finished task (`Running` + `Done`) loses
  nothing when its idle session ends, yet it becomes `Stopped(ServerLost)` and leaves Done.
- **Root cause, screen.** The terminal of a task without a live run is restored from the run's
  `final-screen` file, which only `finalize_attempt` writes when Yulius sees the process exit. A
  run lost while Yulius was down never gets it, and the new server's in-memory hub is empty.
- **Why tests missed it.** The restart tests (`task_done.rs`, `supervision.rs`) keep the session
  hosts alive and reattach; no test ends the processes while Yulius is down (a reboot).

## Requirements

- RBT-001: When a run is lost (reconcile after a restart, or a lost process) and its task was
  `Running` with activity `Done`, the task becomes `Closed` (with `closed_at`), not
  `Stopped(reason)`. Any other lost task keeps today's `Stopped(reason)`.
- RBT-002: Every clean Stop (turn end) writes the run's current screen to its `final-screen`
  file, so a run lost later still shows the screen its last turn ended on.
- RBT-003: The `stopped_server_lost` message says Yulius restarted and the session ended, not that
  the agent was working.

## Plan

- [x] Reproduce the defect and find its root cause
- [ ] Regression tests: a reboot after the goal closes the task and keeps its screen
- [ ] A run lost after its achieved goal records Closed, not Stopped (server restarted)
- [ ] Every clean turn end saves the screen a lost run shows
- [ ] The server-restarted message no longer claims the agent was working
- [ ] Spec, ADR-0177, docs and changes entry
- [ ] Independent review answered, gates green, merged into dev

## Acceptance criteria

- A goal task Done before the processes are killed while Yulius is down is `closed` after
  `reconcile`, and its run directory holds a `final-screen` with the text its turn ended on.
- A task whose turn ended without an achieved goal is `stopped_server_lost` after the same
  reboot, and also keeps its screen.
- The web UI's message for `stopped_server_lost` no longer says "while the agent was working".

## Test plan

- `crates/yulius/tests/task_done.rs`: `a_reboot_closes_a_done_task_and_keeps_every_screen`
  (real ptyhost and fake agent; phase 1 one task Done and one Your turn; both processes
  SIGKILLed while no server runs; phase 2 `reconcile`). Fails on `dev` (status
  `stopped_server_lost`, no `final-screen`).
- `crates/yulius-server/web/assets/status.test.js` or the existing JS unit test of
  `endedMessage`, if one exists; otherwise the wording is checked in review.
- Full gate `scripts/dev ci`; e2e suite for the web wording.

## Decisions

- A finished task whose session a reboot ended is `Closed`, the state a person's close of a Done
  task already gives; no new status (ADR-0177).
- The screen is saved at each clean Stop, not periodically: a turn end is when the screen is the
  one worth keeping, and it costs one small file write per turn.
- No automatic respawn after a reboot (SPEC-002 §7.1 stays): an interrupted task is continued by a
  person.

## Evidence

Pending.
