---
yulius-task: aeaa0c10-8541-48a6-9523-070b4415c9c5
status: done
---

# SPEC-341: A running task's steps follow its spec while the turn runs

Status: done
Date: 2026-10-27
Owner: Yulius task #325
Scope authorization: task #325 (`/goal`, with a screenshot of the Superagent explaining that a
task's "77%" was misleading because its step list covered only the first 7 parts of a 13-part
spec), "such situation is unaccaptable. Fix." Fixes #942. Extends SPEC-001 STP-004.

## Goal

The progress a task shows is the progress of its spec's Plan. When the agent grows the Plan
mid-turn, the steps and the percentage follow within seconds, not at the next turn end.

## Context

- **Symptom.** A task that had worked since 06:00 UTC showed 77 % while its steps covered only
  the first 7 parts of a spec whose Plan had 13. A person had to ask the Superagent whether it was
  stuck.
- **Root cause.** `App::sync_steps_from_spec` (`crates/yulius-app/src/hooks.rs`) turns the
  spec's top-level Plan items into the steps (STP-004), but it runs only from the `SessionStart`
  and `Stop` hooks. A goal turn can run for hours of tool calls with no turn end. A Plan that grows
  in that time leaves the old, shorter step list in place, along with the `percent` the strip, the
  task list and the tools show. The Superagent read the spec itself and saw the gap.
- **Why tests missed it.** `steps_follow_the_spec_plan_checklist` (agent_hooks) proves the sync
  only through the Stop and SessionStart hooks. No test changed a spec during a running turn.
- Each live run has a supervisor watch loop (`App::watch_loop`, `supervisor.rs`) that wakes every
  second, turn or no turn, for the transcript, the session-id drift and the idle-shell probe.
- The step writers are `steps_set`, `step_update` (coordination.rs), the spec sync (hooks.rs), a
  person skipping a step (intake.rs) and the task's creation (lifecycle.rs). None of them takes a
  lock. Each one reads the steps, decides, and writes.

## Requirements

- **LSS-001 Live sync.** While a task has a live run, Yulius re-reads its spec's Plan at least
  every 5 seconds, whether or not a turn ends. A change shows in the steps with the same rules as
  the hook sync (STP-004): checked items are done, unchecked items keep the status of the step
  with the same title, and new items start pending. The registered spec path is read from the
  store each time, so a path registered later is followed. An unchanged spec writes nothing.
- **LSS-002 No lost step update.** The spec sync, `steps_set`, `step_update` and a person
  skipping a step take one per-task steps lock around their read-decide-write. A sync that runs
  next to an agent's `step_update` never writes back the status that the update replaced.
- **LSS-003 Unchanged elsewhere.** The `SessionStart` and `Stop` syncs, the tools' rules (STP-006,
  STP-007) and the nudges stay as they were. A task with no spec, or a spec with no Plan items,
  keeps the steps the agent set.

## Plan

- [x] Root cause found, issue filed and spec SPEC-341 written
- [x] A regression test shows the steps lag a spec Plan edited mid-turn
- [x] A running task's steps follow its spec within seconds, without lost step updates
- [x] Docs, SPEC-001 and changes entry describe the live sync
- [x] Independent review in a fresh session, findings answered
- [x] Gates green, PR merged into dev on green CI

## Acceptance criteria

- A running fake-agent task with no hooks gets its spec's Plan as steps, and then the Plan's new
  item, without a turn end; its finished step stays done
  (`supervision::a_running_tasks_steps_follow_its_spec_without_a_turn_end`). The test fails on
  `dev`.
- A sync and a step update on the same task are serialized: the update's status is kept
  (`agent_hooks::the_spec_sync_waits_for_the_steps_lock_and_keeps_a_concurrent_update`).
- A step the person skips that was finished while the skip waited for the lock stays finished, and
  the answer says so (`intake::a_step_finished_while_the_skip_waits_stays_done`).
- `steps_follow_the_spec_plan_checklist` and the step tool tests still pass.

## Test plan

```sh
cargo nextest run -p yulius -E 'binary_id(yulius::main) & test(/a_running_tasks_steps_follow_its_spec/)'
cargo nextest run -p yulius -E 'binary_id(yulius::main) & test(/^agent_hooks::/)'
scripts/dev ci
scripts/dev security
```

## Decisions

- **The supervisor's watch loop runs the sync, not a filesystem watcher or the step tools.** The
  loop already exists for every live run, wakes every second and works with the run's worktree.
  A 5-second cadence reads one small file and one step query per live task, and it writes only on
  a change. A file watcher (inotify) would add a dependency and a per-task resource for the same
  result. A sync inside the step tools alone would miss an agent that edits the spec and never
  calls them, which is the case in #942.
- **A per-task lock, not a store transaction.** The sync's decision needs the spec file, which is
  read outside the store. A `tokio::sync::Mutex` in the task's runtime serializes the in-process
  writers, and Yulius is one process per instance (SPEC-002).
- No ADR: this extends the STP-004 sync's timing; it takes no new lasting decision.

## Evidence

- **Red on dev.** With `crates/yulius-app/src` and `agent_hooks.rs` from `79aafe7c`,
  `supervision::a_running_tasks_steps_follow_its_spec_without_a_turn_end` failed: "timed out
  waiting for the first plan" (30.6 s). With the sync's lock lines removed,
  `agent_hooks::the_spec_sync_waits_for_the_steps_lock_and_keeps_a_concurrent_update` failed
  ("the sync waits for the lock"). `intake::a_step_finished_while_the_skip_waits_stays_done`
  failed before its fix, answering "0 steps skipped".
- **Green.** `cargo nextest run -p yulius -E 'binary_id(yulius::main) & (test(/^intake::/) |
  test(/^agent_hooks::/) | test(/a_running_tasks_steps_follow/))'` ran 33 tests, all passed. The
  step, skip and held-prompt tests ran 43, all passed.
- **Gate.** `scripts/dev ci` at `57a80695` exited 0: fmt, lint, macOS compile and deps passed,
  and nextest ran 2760 tests (2760 passed, 16 skipped), followed by doctests and docs.
- **Security.** `yulius security-scan` exited 2, **unrun**: Vidoc is out of quota for this key
  (HTTP 429, "used its 50,000,000 tokens"). The 8 changed code files were not scanned. This is
  not a pass.
- **Review.** Three fresh-session review rounds (`om-code-review`):
  - Round 1 asked for changes: the lock test passed without the lock; a skip held the lock
    across task creation; the spec was read before the lock; and the `steps_set` description
    needed updating. All were fixed test-first.
  - Round 2 approved, with two low findings that were fixed (the skip's zero answer and a
    shadowed name).
  - Round 3 approved `57a80695`. Its one nit, the zero answer's wording when a step was removed
    rather than finished, is #944.
  - The pre-existing turn-end nudge, which still says to use `steps_set` when a spec has a
    Plan, is out of scope.
- **Not tested.** The real incident (#942) happened in another project's long goal run, which
  this sandbox cannot reach. The fix is shown on a fake-agent run whose turn never ends.
