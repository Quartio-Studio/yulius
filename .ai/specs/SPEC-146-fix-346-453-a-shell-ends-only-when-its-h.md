---
yulius-task: 8c58b04e-641c-46e9-a5bf-d71893d2c897
status: in-progress
---

# SPEC-146: a shell ends only when its host is proven gone; a launch /goal is watched again after a restart

Status: in-progress
Date: 2026-10-15
Owner: Yulius task #125 (part of task #118, SPEC-138 lanes)
Scope authorization: the person's request (task #125): fix GitHub issues #346 and #453. "A sandbox
shell, and the Superagent, must be recorded as ended only once its host is proven gone, as
ADR-0050 does for task runs. Otherwise it stays live, and its viewers are told. The Superagent's
usage-limit move (SPEC-074 SUP-004) must not start a second Cockpit while the old host may still
run." And: "On reattach of a goal-mode run whose launch prompt was a `/goal` and whose transcript
has no goal record, reconcile starts the goal watch again; a run that already recorded its goal is
never watched or flagged."

## Goal

Both concern the supervisor's process lifecycle across ends and restarts:

- Ending a sandbox shell or the Superagent (closing it, the idle reaper, the Superagent's
  usage-limit move) never forgets a process that may still run: the shell is recorded as ended,
  and its run directory removed, only once its host is proven gone. Otherwise it stays live and its
  viewers are told; the Superagent is not started a second time on the same conversation.
- A goal task whose launch `/goal` was not yet recorded when the server restarted is watched again
  by the new server, and flagged when Claude Code never takes the goal, as without a restart.

## Context

- `App::end_shells` (`crates/yulius-app/src/shells.rs`) asks each shell to exit (Ctrl-C, TERM,
  KILL, escalated by the host), then `await_shell_exit` waits `EXIT_WAIT` (10 s) for the exit. On
  a timeout it calls `session_host.terminate` and ignores its result, then `finish_shell` records
  the end: viewers get the exit frame, the shell leaves the registry, its run directory is removed
  (#346, found by the review of SPEC-074). `terminate` (`terminate_recorded` in the ptyhost) kills
  the child's group and the host and waits up to 2 s for both to disappear; `Ok` proves them gone.
- ADR-0050 fixed the same pattern for task runs: a run is recorded lost only when `terminate`
  succeeds; otherwise it stays live, the failure is logged and said in its terminal, and a new stop
  re-arms the escalation.
- The Superagent (formerly the Cockpit, `ShellKind::Cockpit`) is a shell. On a usage limit
  (`App::cockpit_limit_reached`, `cockpit.rs`, SPEC-074 SUP-004) it marks the shell `switching`,
  ends it with `end_shells` and starts a new Cockpit resuming the same conversation on another
  subscription, whose transcript is first stepped back atomically. An old process still running
  would append to the replaced (unlinked) file and its writes would be lost; two Claude Code
  processes would hold one conversation.
- Since SPEC-117 (ADR-0095) a fresh goal task gets `/goal <condition>` as its launch prompt;
  `spawn_goal_watch` (`supervisor.rs`) flags the task when Claude Code does not record the goal
  within `launch_goal_ms` once the session reads input. The watch lives in the server process.
  `App::reconcile` reattaches the live runs of the previous process and starts no watch, so a
  restart while that goal is unrecorded leaves the task running without a goal (#453). The launch
  prompt is not persisted; the run directory already carries a similar marker for a typed first
  message (`opening-pending`, SPEC-068).

## Requirements

| ID | Requirement | Evidence |
| --- | --- | --- |
| SHE-001 | When a shell (terminal or Superagent) asked to end has not exited within `shell_exit_ms` (configuration, 10 s by default), its host is terminated. Only when `terminate` succeeds (or the shell exited meanwhile) is it recorded as ended and its run directory removed. When `terminate` fails the shell stays live: it keeps its registration, run directory and viewers, it is listed and attachable again (a close is no longer pending), the failure is logged, and its viewers get a notice. Closing it again (or the idle reaper) retries; its own exit is still recorded by its pump. | `shells.rs` `a_terminal_whose_host_cannot_be_ended_stays_live` |
| SHE-002 | The Superagent's usage-limit move (SPEC-074 SUP-004) starts the new Superagent only once the old one ended. When the old host may still run, no second Superagent starts: the old one stays on its subscription, live and no longer `switching`, and its viewers are told that it could not move because its process could not be ended. The limit stays recorded on the subscription (SUP-003) and handled (SUP-002). | `cockpit.rs` `a_superagent_whose_host_cannot_be_ended_is_not_moved` |
| SHE-003 | Notices: a terminal "This terminal's process could not be ended, so it keeps running. Close it again to retry."; the Superagent "The Superagent's process could not be ended, so it keeps running." and, during a move, "The Superagent could not move to another subscription: its process could not be ended, so it stays on this one." | the tests above |
| GWR-001 | A run whose launch prompt is a `/goal` (SPEC-117 ONE-001, ONE-003) has a `launch-goal-pending` marker in its run directory, written before the spawn. The goal watch removes it when it ends with an outcome: the goal recorded, the task flagged (goal not taken, trust dialog unanswered). It stays when the watch ends because the run is no longer live. | `first_message.rs` `a_launch_goal_recorded_before_a_restart_is_not_watched_again` |
| GWR-002 | `App::reconcile`, on reattaching a run whose run directory has that marker, starts the goal watch again, as at the launch: the session reading input, then `launch_goal_ms` (holds not counted). A goal never recorded flags the task with "Claude Code did not take the task's goal". | `first_message.rs` `a_launch_goal_unrecorded_across_a_restart_is_watched_again` |
| GWR-003 | A run that recorded its goal is never flagged after a restart: with the marker removed it is not watched; when the goal was recorded while Yulius was down (marker left), the transcript replay of the reattached run (TSK-014) records it within the watch's bound and the watch ends without a flag. | `first_message.rs` `a_launch_goal_recorded_before_a_restart_is_not_watched_again` |

## Plan

- [x] Spec and plan for #346 and #453
- [x] A terminal ends only when its host is proven gone (#346)
- [x] The Superagent's usage-limit move waits for the old host to be gone (#346)
- [ ] A launch /goal held across a restart is watched again (#453)
- [ ] Docs, ADR and changes
- [ ] Independent review and fixes
- [ ] Gates green, evidence and delivery

## Acceptance criteria

- A terminal whose host survives the end request and whose `terminate` fails stays listed, live
  and attachable with its run directory, and its viewer is told; once its host can be ended,
  closing it again ends it and removes the run directory. The test fails without the fix.
- A Superagent hitting a usage limit while its host cannot be ended is not moved: no second
  Superagent starts, the panel finds the old one, its viewer is told why. The test fails without
  the fix.
- After a restart while a launch `/goal` is unrecorded, the task is flagged with "Claude Code did
  not take the task's goal" after `launch_goal_ms`. The test fails without the fix.
- A goal recorded before the restart (marker removed) or while Yulius was down (marker left) is
  never flagged after the restart.
- The existing shell, Cockpit, first-message and supervision suites pass unchanged; `scripts/dev
  ci` passes.

## Test plan

- Integration (`shells.rs`, real ptyhost): `GatedHost` gets a switch that makes the host refuse to
  end (the end request is dropped, `terminate` fails). A terminal with a viewer is closed while
  set: after `shell_exit_ms` (shortened to 500 ms) the viewer gets the notice, the shell is listed
  and its run directory and processes are there; with the switch off, a second close ends it
  (exit frame, run directory removed).
- Integration (`cockpit.rs`, real ptyhost, fake `claude`): two subscriptions, the Superagent on
  the first; the switch set; a usage limit in its conversation. The viewer gets the move notice;
  `POST /cockpit` (which waits on the start lock the move holds) returns the old Superagent; only
  one Superagent is live; the subscription is recorded limited.
- Integration (`first_message.rs`, fake agent): (a) a goal task with `ignores_goal_prompt` and a
  long `launch_goal_ms` runs; the server goes down; the next server (`launch_goal_ms` 3 s,
  `goal_ready_ms` short) reconciles and flags the task with the notice. (b) a goal task whose goal
  is recorded: the marker is gone; the marker is put back (the goal recorded while the server was
  down) and the server restarts: after the watch's whole bound the task is not flagged.

## Decisions

- **Recorded ended only on a proven end (ADR-0120, extending ADR-0050 to shells).** A failed
  `terminate` keeps the shell registered and live rather than adding an "unreaped" state: every
  reader (the list, the attach, the per-sandbox limit, the reaper) already knows "live". The close
  that failed is cleared so the owner sees the shell again and can close it again; no background
  retry loop (ADR-0050's reasoning: a process that survives SIGKILL can take minutes).
- **The end wait becomes configuration (`shell_exit_ms`)** as `stop_escalation_ms` did, so the
  tests do not wait 10 s.
- **The move asks `end_shells` whether the shell ended** (it returns whether every shell ended)
  instead of inspecting the host itself afterwards: the one place that decides "proven gone" is
  `await_shell_exit`.
- **A run-directory marker, not the attempt record, says the launch goal is unconfirmed.** The
  launch prompt is not persisted and the store schema stays unchanged; the run directory already
  holds the analogous `opening-pending` marker, written before the spawn and read by `reconcile`.
  Deriving it from the transcript (first user prompt starts with `/goal`) would need a harness
  read per reattached run and would miss a goal not yet written to the transcript.
- **The restarted watch is the launch watch itself** (`spawn_goal_watch`): the same input-ready
  wait, trust-dialog handling, hold accounting and bound, so a restart changes nothing but when
  the bound starts.
- **Not in this change** (noted on #346): the other places that ignore `terminate`'s result for
  shells (a lost host connection in `pump_shell`, an unadoptable run directory at start, a shell
  whose start failed in `discard_shell`) and the account-switch path ADR-0050 left open.
