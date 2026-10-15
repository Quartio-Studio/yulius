---
yulius-task: f5066d67-9b38-48ec-a125-3015a85defbc
status: in-progress
---

# SPEC-143: a run that ends while Yulius is down keeps its metrics; the recount waits for reconciliation

Status: in progress
Date: 2026-10-15
Owner: Yulius task #124 (part of task #118, SPEC-138 lanes)
Scope authorization: the person's request (task #124): fix GitHub issues #385 and #389. "A run's
stored figures must never be replaced by an empty snapshot: skip the save when no watcher read the
run in this process, or count its transcript window first, as SPEC-091's `HarnessPort::recount`
does. Integration test: restart after a run with figures exited while Yulius was down; the run
keeps (or recounts) its figures. The live save path stays unchanged." And: "SPEC-002's data-model
table lists `metrics_recounts` […]; and a test fails when the metrics recount runs after a failed
reconciliation."

## Goal

A Yulius restart never erases a run's usage. A run that ended while Yulius was down gets the
figures its own transcript lines give, by the live watch's rules, or keeps the ones it has when its
transcript gives nothing. The metrics recount of SPEC-091 provably starts only after a
reconciliation that succeeded.

## Context

- `App::reconcile` (`crates/yulius-app/src/supervisor.rs`) inspects each run recorded live before
  the restart. A run whose host recorded its exit is ended by `finalize_attempt`, which calls
  `save_metrics` with the task's runtime usage snapshot. In a new process no watcher read that
  run, so the snapshot is empty and `put_metrics` (an upsert) replaces the run's stored tokens,
  turns and active time with NULLs (#385, found by the review of SPEC-091, task #76).
- `finalize_attempt` is also called when the host reports the exit to the attached reader, and
  when a lost connection finds the run exited: in both, `attach_run` started the run's watcher in
  this process, so the snapshot is the run's (SPEC-087 MET-002: a reattached run reads its
  transcript from its start again).
- SPEC-091 counts an ended run's own window of its session transcript (`HarnessPort::recount`,
  `App::count_ended_run` in `metrics_recount.rs`): from its start (or, resumed, the first line at or
  after its start) to its end, sub-agents included, by the live watch's accumulator. It answers
  "no transcript" and "no lines" without figures.
- `App::start_background` (`background.rs`) starts the recount only when reconciliation succeeded
  (SPEC-091 RCT-004): a run still recorded live would be recorded `live` and never recounted. Only
  code review checks it (#389). SPEC-002's data-model table has no `metrics_recounts` row.

## Requirements

| ID | Requirement | Evidence |
| --- | --- | --- |
| RDM-001 | A run that reconciliation finds exited (its exit happened while Yulius was down) is ended without saving the runtime's snapshot: once its end is recorded, its window of its transcript is counted as SPEC-091 RCT-001/RCT-002 count it, and its stored figures are replaced only by figures that count found. No session, worktree or transcript, a window with no line of the run, or a failed count leaves the stored row as it was (a failure is logged). | `run_metrics::a_run_that_ends_while_yulius_is_down_keeps_its_metrics` |
| RDM-002 | A run that ends while a watcher of this process reads it is saved as before: the watcher's snapshot, after the end is recorded. | `run_metrics::a_restart_keeps_a_resumed_run_s_metrics`, `supervision` suite (unchanged) |
| RDM-003 | The start's recovery step reconciles, then starts the metrics recount only when reconciliation succeeded, and says whether it did (the recount's handle); `start_background` runs that step. While reconciliation fails no queued run is recorded; the next start that reconciles recounts them. | `metrics_recount::the_recount_waits_for_a_reconciliation_that_succeeds` |
| RDM-004 | SPEC-002's data model lists `metrics_recounts` (schema version 23, SPEC-091). | SPEC-002 §data model |

## Plan

- [x] Spec and plan for #385 and #389
- [ ] A run that ended while Yulius was down keeps or recounts its figures (#385)
- [ ] Recount starts only after a successful reconciliation, proven by a test (#389)
- [ ] SPEC-002 lists metrics_recounts; docs, ADR and changes
- [ ] Independent review and fixes
- [ ] Gates green, evidence and delivery

## Acceptance criteria

- After a restart, a run with stored figures whose agent exited while Yulius was down stores its
  transcript's figures — including lines written while Yulius was down — instead of NULLs; the
  test fails without the fix.
- The live save path is unchanged: the existing run-metrics and supervision suites pass as they
  are.
- A recovery whose reconciliation fails records no queued run; the next one that reconciles
  recounts them; the test fails when the recount is started whatever reconciliation answered.
- SPEC-002 has a `metrics_recounts` row; `scripts/dev ci` passes.

## Test plan

- Integration (`run_metrics.rs`, real ptyhost and fake agent writing a Claude Code shaped
  transcript): a first server runs a turn and saves the run's figures; it goes down; a sub-agent
  line is written and the agent is killed while it is down; the second server reconciles once the
  host recorded the exit; the run stores its turns and its tokens plus the sub-agent's, never
  NULLs.
- Integration (`metrics_recount.rs`, a store upgraded from schema version 22): one queued ended
  run without a transcript and a running task whose row the store cannot read (reconciliation
  fails listing running tasks). The recovery step answers no recount and the queue is untouched;
  with the row repaired, it answers a recount that records the run `no_transcript`.

## Decisions

- **Recount, not skip (ADR-0117).** Skipping the save keeps the figures of the last live save,
  which miss what the run did after it (a turn or a goal continuation while Yulius was down, a
  turn whose save the 30 s live throttle had not reached). Counting the run's window gives exactly
  the figures a watched run would have stored, and when it finds nothing it falls back to
  skipping. The cost is one transcript read per run that ended while Yulius was down, at start and
  off the async workers.
- **The caller says whether a watcher read the run.** `finalize_attempt` takes how the run's
  figures are known (`Watched` or `Unwatched`): reconciliation is the only caller with no watcher,
  and saying so at the call is clearer than inferring it from a runtime flag shared by the task's
  successive runs.
- **The recovery step returns the recount's handle.** Extracting `recover` from
  `start_background` lets a test observe, without sleeps, that no recount starts after a failed
  reconciliation; the background loop is otherwise unchanged.
- **A counted model replaces the stored one,** as a live save does: a transcript with usage names
  its model.
