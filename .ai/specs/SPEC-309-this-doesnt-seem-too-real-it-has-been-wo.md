---
yulius-task: a48d6e74-33fb-4413-8491-a447ee073171
status: in-progress
---

# SPEC-309: A turn the agent's own background work wakes keeps its working stretch

Status: in-progress
Date: 2026-10-25
Owner: task #295
Scope authorization: task #295 ("this doesnt seem too real it has been working much longer.
Fix", with a screenshot of a sidebar reading "working 37m" for a task whose chained tasks had been
queued behind it for 14 h). Issue #894.

## Goal

The sidebar's "working 1h 20m" (SPEC-087 TIM-002/TIM-003) counts the whole stretch in which a task
has worked without needing a person, including the turns its own background work wakes. It no
longer restarts when a background shell (the agent's `yulius wait`) ends and Claude Code wakes the
agent.

## Context

- Symptom (2026-10-25): task #32 of another project read "working 37m" with its chained tasks
  #33/#34 "queued 14h 18m" behind it: it had worked for hours. Task #272 (SPEC-290) showed the same:
  its stretch counted from 00:26:24, the wake of its last `yulius wait`.
- Code path: `App::probe_idle_shells` (`crates/yulius-app/src/finish.rs`) runs every
  `SHELL_PROBE_MS` (5 s) from the transcript watcher. A background Bash shell is counted from the
  process tree only. When the probe sees the last shell gone while the task is still idle, it
  publishes the task, and `follow_working_stretch` (`crates/yulius-app/src/tasks.rs`) clears
  `working_since` (idle, no background work: TIM-002's end). Claude Code notices the shell's end
  by polling and wakes the session with a `<task-notification>` prompt a moment later; that turn
  makes the task Working, and the store starts a new stretch then
  (`COALESCE(working_since, now)` in `set_activity`).
- Whether a wake resets the clock depends on whether the 5 s probe lands in the gap between the
  shell's exit and the wake, so a task cycling through `yulius wait` for hours keeps losing its
  stretch; the sidebar shows the time since the last loss.
- Why the tests missed it: the fake agent wakes the session the instant its shell exits, so the
  probe never sees the gap, and no test reads `working_since` across a wake
  (`tasks::tests::background_work_decides_an_idle_task_s_working_stretch` covers the pure rule).
- What else the cause could break: the same end-then-restart happens when a turn ends with a stale
  shell count of 0 and the next probe finds a new shell (the stretch restarts at the probe), and
  when the goal evaluator sends an idle task back to work (`resume_working`).

## Requirements

- WSW-001: when an idle task's working stretch ends because no background work is left, and the
  task, still in that same idle state, goes on by itself (Claude Code's own wake-up prompt, work
  after a stop with no prompt, the goal evaluator's "not met", or newly found background work), it
  continues that stretch instead of starting a new one.
- WSW-002: a prompt a person or Yulius types into the idle task, needing a person, being queued,
  stopping or ending still end the stretch (TIM-002 unchanged); so does any change of the task's
  state in between (the remembered stretch is tied to the state time it ended at).

## Plan

- [x] Root cause and spec of the reset working stretch
- [x] Fake agent wakes a session a moment after its shell ends
- [x] Regression test: a woken turn keeps the working stretch (failing)
- [x] A turn the agent's own background work wakes continues the stretch
- [ ] Docs, changes and review answered
- [ ] Gates green and delivered on dev

## Acceptance criteria

- A goal task whose turn leaves a background shell, whose end Claude Code reports seconds after
  the shell exits (after Yulius's probe saw it), publishes one and the same `working_since` in
  every update while it works, before and after the wake (WSW-001).
- A typed prompt to an idle task with no background work starts a new stretch (WSW-002). The
  prompt hook runs while the task is still in the state its stretch ended in, so the state-time
  check cannot tell it from a wake-up: only `is_typed_prompt` does (a typed prompt never calls
  the restore). The state time guards every other change in between.
- The same holds when the wake's prompt hook never reaches Yulius (it reads the wake-up from the
  transcript, `Observation::UserTurn { wake: true }`).

## Test plan

- Fake agent: `--wake-delay-ms MS` delays Claude Code's wake after the background shell exits, as
  Claude Code's polling does; `--wake-prompt-lost` drops the wake's prompt hook.
- Integration (`crates/yulius/tests/task_done.rs`), `a_woken_turn_keeps_the_working_stretch`:
  goal turn with a 4 s shell, wake 10 s after its exit (well past `SHELL_PROBE_MS`, so the probe
  sees the shell gone first; the test waits until the stretch is ended in the store), the woken
  turn's goal achieved; every `TaskUpdated` of the task while working carries the first stretch's
  start (a lagging event receiver fails the test). Then a typed prompt: its updates carry a later
  start. Run as `a_turn_its_shell_wakes_keeps_the_working_stretch` (hooks) and
  `a_wake_read_from_the_transcript_keeps_the_working_stretch` (prompt hook lost). Fails on `dev`
  (the woken turn publishes a later `working_since`).
- Pure rule: `tasks::tests::an_ended_stretch_continues_only_from_the_same_idle_state`.

## Decisions

- **Remember the ended stretch and continue it, rather than not ending it.** Keeping
  `working_since` while an idle task has no background work would make a later typed prompt
  continue an old stretch (TIM-002 says a person's prompt starts a new one) and needs a second
  "end due" marker. Instead the stretch still ends as before; the runtime remembers its start with
  the task's state time, and a self-driven return to work from that same state restores it. No
  schema change; a restart forgets it (the next wake then starts a new stretch, as before).
- **Tied to the state time, not a time window.** A grace period would guess Claude Code's latency;
  the unchanged state time proves nothing happened in between (no prompt, no attention flag, no
  status change), whatever the gap.
- **The restored start is re-asserted after the task is Working** (`keep_working_stretch`): a
  publish between the restore and the activity write would end the stretch again; once Working,
  no publish ends it. One store write for both would need a new store method for one call site.
- **Prompts starting with `<` count as Claude Code's own** (`is_typed_prompt`, pre-existing): a
  person who pastes markup into an idle task continues its stretch, as it already keeps an
  achieved goal. Accepted: cosmetic, and the same rule decides both.
- No ADR: this corrects SPEC-087 TIM-002's implementation, it takes no new lasting direction.

## Review

Independent review (fresh session, `om-code-review`), round 1 at `511ec4ca`: request changes.

- Own, major: no test of a typed prompt starting a new stretch; spec credited the wrong check.
  Fixed: the integration test types a prompt after the woken turn (mutation: dropping the
  `is_typed_prompt` guard fails it); acceptance criterion corrected.
- Own, minor: a publish between the restore and the Working write undoes it. Fixed:
  `keep_working_stretch` re-asserts the start once the task is Working.
- Own, minor: the transcript's wake path (`UserTurn { wake }`) missed the restore. Fixed, tested
  with `--wake-prompt-lost` (mutation: removing it fails the test). The `(true, None)` branch and
  `resume_working` use the same remembered stretch and guard (unit test
  `an_ended_stretch_continues_only_from_the_same_idle_state`); no separate integration test.
  Markup prompts: accepted (Decisions).
- Pre-existing, minor: `follow_working_stretch` writes unconditionally after separate reads:
  issue #895.
- Nits: wake delay raised to `SHELL_PROBE_MS + 5 s`, lag fails the test, the gap is checked in
  the store; Plan ticked, Evidence added.
