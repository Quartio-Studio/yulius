---
yulius-task: 632c1599-fc0c-44a1-a855-eb18a9d766bd
status: done
---

# SPEC-635. A "could not end this session" flag outlives the session's late turn

## Goal

Keep the flag "Yulius could not end this session" on a running task while its cause stands: the live run's host could not be terminated. The session's own work must not clear it. Fixes #1611 (the flaky `task_done::a_switch_whose_old_run_cannot_be_ended_starts_no_new_run`).

## Context

`run_stays_live` (`supervisor.rs`, SPEC-065 END-001) records `end_failed` for the run and flags the task with a Yulius reason whose `work_answers` is false. `attention_view` shows the reason only while the task reads `NeedsAttention`. Every session-driven activity write replaces that activity with no look at the flag:

- the prompt hook sets Working (`hooks.rs`, `set_task_activity`);
- a clean Stop and a goal verdict settle the turn (`settle_clean_turn`, `settle_idle_activity` in `finish.rs`); a met goal settles Done;
- agent work read from the transcript resumes Working (`resume_working`), a transcript prompt starts a turn (`supervisor.rs`), an interrupt sets Waiting.

The test's fixture starts the switch once fake-agent prints `GOAL SET`, before the goal turn's prompt hook, Stop and met verdict. Normally they land within the switch's 300 ms wait. Under load they land after the flag and overwrite it: the task reads Done, and the person never learns that the session cannot be ended and blocks deletion. The same happens in production to any run whose stop, park or switch could not terminate it while its session keeps working.

## Requirements

- EFL-001. While the task's live run is the one whose host could not be terminated, no session-driven activity write (prompt hook, Stop or verdict settle, transcript work, transcript turn start, interrupt) changes the task's activity: the flag and its reason stay.
- EFL-002. The flag ends when its cause ends: a person stops the task (the re-armed escalation clears `end_failed`), or the run ends (its task changes status). A later run of the task is not held by an earlier run's failure.
- EFL-003. The check and the write are one step against the flag: a write that began before the failure was recorded lands before the flag is raised, so the flag is always the last word.

## Plan

- [x] Reproduce the overwritten flag with a held goal turn
- [x] Keep the flag against session activity while the run cannot be ended
- [x] Prove the fix under stress with a busy loop beside it
- [x] Record the decision and update the docs
- [x] Answer the independent review

## Acceptance criteria

1. A goal turn whose prompt hook is held until the switch failed, then released and settled (met), leaves the task reading `NeedsAttention` with the reason "Yulius could not end this session…". On dev it reads Done with no reason.
2. `task_done::a_switch_whose_old_run_cannot_be_ended_starts_no_new_run` passes under `--stress-count` with a busy loop beside it.
3. A stop of the task and the run's own end still work as before (existing `task_done`, `task_delete`, `parking` tests).

## Test plan

New `task_done` test `a_late_turn_of_a_run_that_cannot_be_ended_keeps_its_flag`: the goal's prompt hook is held at `Point::PromptTurnChecked` (after its turn-end check, before it sets Working) until the switch failed on a host that refuses ends; a new race point at the end of `settle_clean_turn` (`Point::TurnSettled`) tells the test the late turn settled; then the activity and the reason are read. A person's prompt typed afterwards settles the same way. It fails without the fix. (A held Stop alone did not reproduce it: a Goal task reading Needs you with no verdict keeps its flag at Stop.) Run `task_done`, `task_delete`, `parking`, `shells`, `cockpit` in full, and the flaky test with `--stress-count 40` beside busy loops.

## Decisions

- A product fix, not a test-only wait: the flag's cause (an un-endable host) stands while the session works, so work must not answer it. [Decision 0468](../../docs/decisions/0468-a-flag-whose-cause-stands-is-not-answered-by-work.md)
- The guard reads the runtime's `end_failed` for the live run, not a stored attribute of the reason: a restarted server reattaches the run and can end it again, so the flag must not outlive the runtime that knows the failure.
- A person's typed prompt does not clear the flag either: the flag's action is to stop the task; typing does not make the host endable.

## Evidence

- Reproduction on dev b1b50218f with only the probe (commit fa325f21b: the test and the `TurnSettled` race point): `scripts/cargo nextest run -p yulius --no-fail-fast --stress-count 10 -E 'binary_id(yulius::main) & test(=task_done::a_late_turn_of_a_run_that_cannot_be_ended_keeps_its_flag)'` failed 10/10 with `left: Some(Done)`, `right: Some(NeedsAttention)` (`running (runs: Live None)`): the late prompt hook set Working over the flag and the Stop settled the turn Done.
- A first probe that held only the goal turn's Stop (fake-agent `turn_hold_file`) passed on dev: at Stop a Goal task reading Needs you with no verdict keeps its flag, and the met verdict that follows settles nothing from Needs you. The prompt hook is the overwrite CI hit.
- With the fix, both `a_late_turn_of_a_run_that_cannot_be_ended_keeps_its_flag` and `a_switch_whose_old_run_cannot_be_ended_starts_no_new_run` passed `--stress-count 40` with 128 busy loops beside them on 128 CPUs (load average 176): 40/40 iterations.
- `scripts/cargo nextest run --no-fail-fast -p yulius -p yulius-app --lib --test main` passed 4,174 tests before and 4,183 after merging dev dafc8ed03 (24 skipped by the default profile). `cargo fmt --all -- --check` and `scripts/cargo clippy -p yulius-app -p yulius --all-targets --features yulius-app/test-support -- -D warnings` passed.
- `scripts/dev security`: disabled (Vidoc is turned off in Yulius); nothing was scanned.
- Not covered by a test of its own: the transcript turn start, the interrupt and the idle settle reach the guard through the same `write_session_activity` as the prompt hook and the turn settle the test drives.
