---
yulius-task: c7c33c5f-6f5c-49fc-8eb4-a4204360f745
status: in-progress
---

# SPEC-346: A prompt hook handled after its turn ended leaves the task as that turn left it

Status: in-progress
Date: 2026-10-30
Owner: Yulius task #330
Scope authorization: task #330, fixing #921 ("Flaky again:
`task_done::a_goal_the_tui_dropped_is_submitted_again` times out waiting for Done in the full
suite"). Decided in ADR-0234; refines SPEC-140 HKS-001 / ADR-0115 (a hook Yulius received is applied to its end) and
SPEC-079 BGW-005 (a clean Stop waits for the goal verdict).

## Goal

A goal task whose turn's `UserPromptSubmit` hook Yulius answers only after the hook client gave
up (5 s, fail-open) still reads Done once Claude Code records the goal as met, instead of staying
Working or Your turn for good. The same holds for any prompt: its late handling never undoes what
the turn's Stop or goal verdict, which came after it, decided.

## Context

- **Symptom (#921, also task #237 at 64 s):** under full-suite load the test times out waiting for
  Done after about 65 s (20 s to the goal turn's echo, then the whole 45 s wait). Alone it passes
  in about 18 s.
- **Reproduced deterministically** with a temporary probe that holds the goal turn's
  `UserPromptSubmit` 7 s in the server (past the client's `HOOK_TIMEOUT` of 5 s): the test fails
  with "timed out waiting for Done" at 66.6 s. Order seen: the hook client fails open, the agent
  runs the turn, its Stop hook arrives and waits for the verdict (the goal was observed open), the
  watcher applies "met"; only then does the late prompt hook run `prompt_submitted` (achieved goal
  reset) and set Working. Nothing ends that turn again, so the task stays Working.
- **The triage's lead did not reproduce:** random 0-15 s probe delays before the transcript
  watcher applies its observations (and 0-1.5 s in the prompt and Stop hooks) passed 29 of 29
  runs (17-37 s). The extra Enter of a later submit attempt does run an empty fake-agent turn,
  but an empty prompt neither counts as typed (`is_typed_prompt`) nor yields a transcript
  `UserTurn`, so it leaves the achieved goal alone.
- **Root cause:** since ADR-0115 a hook runs to its end in its own task after its client gave up,
  and its Consequences note that it "can land after the agent's next hooks, which were never
  ordered with it". For a Stop that heals with the agent's next work; for a prompt it does not:
  the agent is idle after the turn, so the late `Working` (and the reset goal) is never corrected.
  A second gap shows once the late prompt is ignored: the Stop found the goal open and waited for
  the verdict (`verdict_stop_ms`) with the task still reading Your turn, and `goal_observed`
  settles a waited turn only while it reads Working or Needs you (`settle_waited_turn`), so a met
  goal leaves it reading Your turn.
- **Why tests missed it:** the fake agent's hooks are always answered within the client's
  deadline in a lone run; only a loaded host delays the server (ADR-0115: the single database
  thread) past 5 s.
- The server cannot order a late prompt by when it handles it: the delay may come before the
  handler starts (the request waiting for the database or the accept queue). The hook client,
  which Yulius ships (`yulius hook`, on the same host: loopback only), knows when it sent the
  hook.

## Requirements

| ID | Requirement | Evidence |
| --- | --- | --- |
| LPH-001 | The hook client sends, with every hook request, the wall-clock time it sent it (`x-yulius-hook-sent-ms`, Unix ms). The server takes it, clamped to its own clock, or its own time when the request has none (an older client). | `yulius-harness` `hook_client::forwards_payload_and_prints_200_body`; `intake::a_prompt_answered_after_its_turn_ended_leaves_the_task_settled` (no header: arrival order) |
| LPH-002 | A `UserPromptSubmit` sent before a Stop of the same task that already reached Yulius, or before a goal verdict (met, not met, failed) Yulius already applied, belongs to a turn that is over: it changes neither the task's activity nor its achieved goal, its gate count or its turn start. What it did before that (holds, the scope check, a person's answered question) is unchanged, and the transcript watcher still sees its turn (STA-003). | `intake::a_prompt_answered_after_its_turn_ended_leaves_the_task_settled` (fails without: Working); `task_done::a_goal_whose_prompt_hook_is_answered_late_is_done` |
| LPH-003 | A goal verdict "met" makes an idle task Done also when a Stop waited for it (BGW-005) and the task reads Your turn. | `task_done::a_goal_whose_prompt_hook_is_answered_late_is_done` (fails without: no Done) |

## Plan

- [x] Reproduce the stranded Waiting deterministically
- [x] Spec with root cause and regression test
- [ ] Regression test that fails without the fix
- [ ] Fix: a late prompt hook leaves its ended turn settled
- [ ] Docs, changes entry and gates green
- [ ] Independent review and its findings answered
- [ ] Merge into dev on green CI and comment on #921

## Acceptance criteria

- AC-1: each new test fails without its fix and passes with it.
- AC-2: `task_done::a_goal_the_tui_dropped_is_submitted_again` passes repeatedly, and the end-to-end
  test with the goal prompt held past the client's deadline passes.
- AC-3: the full gate (`scripts/dev ci`) is green.

## Test plan

- LPH-001: the hook client test reads the request a hook client writes and finds the sent time.
- LPH-002: `intake`, a Working task: a person's typed prompt whose scope check (a fake Jev)
  blocks until released; meanwhile the turn's Stop is answered (Your turn); after the release the
  task still reads Your turn. Without the fix it reads Working.
- LPH-003 and the whole path: `task_done`, the resumed goal session of the flaky test, with a test
  layer that holds the goal's `UserPromptSubmit` 6 s before the server sees it (past the client's
  5 s); the task must read Done. Without the fix it never does.
- What else the cause could break, checked by the existing suites: Stop settling (`task_done`),
  holds and pauses of prompts (`task_pause`, `subscription_pool`), the scope check (`intake`), the
  transcript fallback for lost prompt hooks (`session_sync`, `task_done`).

## Decisions

- **Order prompts by the client's send time, not the server's.** A server stamp misses the delays
  before the handler starts (the database, the accept queue, the test's layer). The client is
  Yulius's own binary on the same host, so the clocks agree. A forged or skewed time only affects
  that task's own bookkeeping (it is clamped to the server's clock); an absent one falls back to
  the server's arrival time, as before.
- **Compare with Stops by their send time and with verdicts by when Yulius applied them.** A Stop
  sent after the prompt can only be its own turn's or a later one's; a verdict applied after the
  prompt was sent can, under watcher lag, be the previous turn's: the prompt's effects are then
  skipped and the watcher's `UserTurn` for the prompt (it comes after that verdict in the
  transcript) still sets Working, so nothing is lost.
- **Skip the late prompt's effects, not the hook.** ADR-0115 stays: the event is handled to its
  end; only what a later turn end already decided is left alone.
- **Not done here: confirming a typed goal by its prompt hook.** The triage proposed that
  `spawn_goal` stop resubmitting when the goal's own `UserPromptSubmit` arrives. It did not
  cause #921 (not reproduced under watcher lag) and would change the goal typist's contract, so it
  is noted on #921, not built.
