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

- EFL-001. While the task's live run is the one whose host could not be terminated and the task reads the flag that failure raised, no session-driven activity write (prompt hook, Stop or verdict settle, transcript work, transcript turn start, interrupt) changes the task's activity: the flag and its reason stay.
- EFL-002. The flag ends when its cause ends: a person stops the task (the re-armed escalation clears `end_failed`), or the run ends (its task changes status). A later run of the task is not held by an earlier run's failure, and a record whose run is over is dropped. Once something else replaced the flag (a capacity pause and release, another matter's flag that stood first), the session's activity rules as before. A held write reports no Done to the settles that call it.
- EFL-003. The check and the write are one step against the flag: a write that began before the failure was recorded lands before the flag is raised, and a write that looks while the flag is being raised is held as if it stood, so the flag is always the last word.

## Plan

- [x] Reproduce the overwritten flag with a held goal turn
- [x] Keep the flag against session activity while the run cannot be ended
- [x] Prove the fix under stress with a busy loop beside it
- [x] Record the decision and update the docs
- [x] Hold session writes only while the could-not-end flag stands
- [x] Answer the independent review

## Acceptance criteria

1. A goal turn whose prompt hook is held until the switch failed, then released and settled (met), leaves the task reading `NeedsAttention` with the reason "Yulius could not end this session…". On dev it reads Done with no reason.
2. `task_done::a_switch_whose_old_run_cannot_be_ended_starts_no_new_run` passes under `--stress-count` with a busy loop beside it.
3. A stop of the task and the run's own end still work as before (existing `task_done`, `task_delete`, `parking` tests).

## Test plan

New `task_done` test `a_late_turn_of_a_run_that_cannot_be_ended_keeps_its_flag`: the goal's prompt hook is held at `Point::PromptTurnChecked` (after its turn-end check, before it sets Working) until the switch failed on a host that refuses ends; a new race point at the end of `settle_clean_turn` (`Point::TurnSettled`) tells the test the late turn settled; then the activity and the reason are read. A person's prompt typed afterwards settles the same way. It fails without the fix. (A held Stop alone did not reproduce it: a Goal task reading Needs you with no verdict keeps its flag at Stop.) Run `task_done`, `task_delete`, `parking`, `shells`, `cockpit` in full, and the flaky test with `--stress-count 40` beside busy loops.

## Decisions

- A product fix, not a test-only wait: the flag's cause (an un-endable host) stands while the session works, so work must not answer it. [Decision 0468](../../docs/decisions/0468-a-flag-whose-cause-stands-is-not-answered-by-work.md)
- The guard reads both the runtime's `end_failed` for the live run and the flag the task reads (known by its wording, whatever the failure's words), not a stored attribute of the reason: a restarted server reattaches the run and can end it again, so the flag must not outlive the runtime that knows the failure, and a flag something else replaced must not hold the session (review round 1).
- While the flag is being raised (`end_flag_pending`, counted with the record under `end_gate` by a `FlagPending` guard that drops when the raise is over, cancelled or panicked), session writes are held as if it stood (review rounds 2 and 3). The gate is not held across `flag_task`, since `agent_worked` takes `attention_lock` before the gate.
- A person's typed prompt does not clear the flag either: the flag's action is to stop the task; typing does not make the host endable.

## Evidence

- Reproduction on dev b1b50218f with only the probe (commit fa325f21b: the test and the `TurnSettled` race point): `scripts/cargo nextest run -p yulius --no-fail-fast --stress-count 10 -E 'binary_id(yulius::main) & test(=task_done::a_late_turn_of_a_run_that_cannot_be_ended_keeps_its_flag)'` failed 10/10 with `left: Some(Done)`, `right: Some(NeedsAttention)` (`running (runs: Live None)`): the late prompt hook set Working over the flag and the Stop settled the turn Done.
- A first probe that held only the goal turn's Stop (fake-agent `turn_hold_file`) passed on dev: at Stop a Goal task reading Needs you with no verdict keeps its flag, and the met verdict that follows settles nothing from Needs you. The prompt hook is the overwrite CI hit.
- With the fix, both `a_late_turn_of_a_run_that_cannot_be_ended_keeps_its_flag` and `a_switch_whose_old_run_cannot_be_ended_starts_no_new_run` passed `--stress-count 40` with 128 busy loops beside them on 128 CPUs (load average 176): 40/40 iterations.
- `scripts/cargo nextest run --no-fail-fast -p yulius -p yulius-app --lib --test main` passed 4,174 tests before and 4,183 after merging dev dafc8ed03 (24 skipped by the default profile). `cargo fmt --all -- --check` and `scripts/cargo clippy -p yulius-app -p yulius --all-targets --features yulius-app/test-support -- -D warnings` passed.
- `scripts/dev security`: disabled (Vidoc is turned off in Yulius); nothing was scanned.
- Independent review round 1 of c9c021bb3 (request changes): the refusal followed `end_failed`, not the flag. After a capacity pause and release the task read Working for good. A task whose flag was another matter's could never answer it. Three guarded call sites had no test. A held Done still advanced chains. The new tests `a_run_that_cannot_be_ended_follows_its_session_once_a_pause_replaced_its_flag` (`left: Some(Working)`) and `another_flag_on_a_run_that_cannot_be_ended_is_answered_as_before` (`left: Some(NeedsAttention)`) failed on that head (commit 31c478e1f). They pass with the guard tied to the flag the task reads.
- Call-site coverage, each checked by undoing that site's guard: the transcript turn start fails `a_transcript_turn_of_a_run_that_cannot_be_ended_keeps_its_flag` (`left: Some(Waiting)`; its Stop is held at `StopClean` until the watcher passed `UserTurnChecked`), the interrupt fails `an_interrupted_turn_of_a_run_that_cannot_be_ended_keeps_its_flag` (new point `TurnInterruptSettled`), and the prompt hook fails the late-turn, transcript and interrupt tests.
- Not covered by a test of its own: `settle_idle_activity` writes only from Working, Waiting or Done, so it can meet the flag only in the moment between its read and its write, which the gate closes. No test holds that moment.
- After round 1: all six `task_done::*cannot_be_ended*` tests passed `--stress-count 40` beside 128 busy loops (load average 150): 40/40. The full `nextest -p yulius -p yulius-app --lib --test main` run passed 4,187 tests (24 skipped). clippy `-D warnings` and fmt were clean.
- Independent review round 2 of 6cd39c856 (request changes). Major: a write that looked after the failure was recorded but before its flag was stored still wrote over it. `a_write_that_looked_before_the_flag_was_stored_leaves_it` holds that window with the new points `EndFailedRecorded`, `SessionActivityChecked` and `EndFailedFlagged`. It failed on 6cd39c856 with `left: Some(Waiting)` (commit a8db53ec7) and passes with the pending mark. Minor: the flag was matched by the latest failure's exact words. It is now known by its wording (unit test `a_could_not_end_flag_is_known_whatever_the_failures_words`). The interrupt test now waits for `INTERRUPTED`. `settle_idle_activity`'s held branch remains without a test of its own (see above).
- After round 2: the seven `task_done` tests (the six `*cannot_be_ended*` and `a_write_that_looked_before_the_flag_was_stored_leaves_it`) passed `--stress-count 40` beside 128 busy loops (load average 188): 40/40. `nextest -p yulius -p yulius-app --lib --test main` ran 4,198 tests and 4,197 passed. One failure, `parking::candidate::a_candidate_refresh_waits_for_the_launch_to_reserve_its_verified_slot` (`left: 0`, `right: 1` at `candidate.rs:521`), then passed 20/20 alone. That test raises no un-endable run, and the parking area's gate failures belong to task #616. clippy `-D warnings` and fmt were clean.
- Independent review round 3 of 62e793574: approved. Its CI run had passed. A minor note was fixed after the approval. A dropped or panicking `run_stays_live` could leave the pending mark set and hold the session's writes for good. Two raises of one task shared one mark. The mark is now a count kept by a drop guard: unit test `a_flag_being_raised_counts_until_its_raise_is_dropped`, which fails when the guard does not count down. The window test now asserts that a write looked while the flag was being raised.
