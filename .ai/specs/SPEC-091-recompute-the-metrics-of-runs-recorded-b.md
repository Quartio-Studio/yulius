---
yulius-task: b86510d8-8ed4-4539-8dd1-c5e05b866c74
status: done
---

# SPEC-091: runs recorded before SPEC-087 get their metrics from their transcripts, once

Status: implemented
Date: 2026-10-14
Owner: task #76
Scope authorization: the person's request (task #76): "Make Yulius recompute, once and
idempotently, the stored metrics of every ended run recorded before that change from its
transcript where the transcript still exists (for example as an upkeep or startup job), leaving
runs whose transcript is gone as they are, and recording what was recomputed. Prove it with tests
built from real transcript shapes."

## Goal

The dashboard's history is true for the runs recorded before SPEC-087 too: every ended run whose
transcript still exists stores the tokens, turns and active time its own transcript lines give,
by the same rules as a run recorded today (SPEC-087 MET-001 to MET-004). It happens once per
instance, by itself after the upgrade, and each run's outcome — and the figures it replaced — is
recorded.

## Context

- SPEC-087 (#376, deployed to production 2026-10-14 00:18 UTC) computes a run's metrics from its
  transcript lines only, and left the rows saved before it as they were ("Historic rows stay",
  SPEC-087 Decisions). In production 41 of 109 runs store 0 active time, 17 of 104 no tokens
  although their transcripts show work, sub-agent tokens were never counted, and turns were
  over-counted (1,991 stored against 882 real).
- The figures come from `UsageAccumulator` (`yulius-harness/src/observe.rs`) over the run's lines:
  `LinesSince` keeps a resumed run's lines from its start, `SubagentTails` adds the sub-agent
  transcripts' usage (`yulius-harness/src/subagents.rs`). The live watcher (`wiring/harness.rs`
  `Watch`) reads a new session's transcript from its start and a resumed one from the run's start
  (`WatchFrom`), to its end while the run lives.
- A run's transcript is `<sandbox home>/.claude/projects/<worktree slug>/<session>.jsonl`
  (`HarnessPort::claude_transcript`); the task's `worktree_path` stays recorded after the worktree
  is removed, and the transcript stays in the sandbox home until the sandbox goes.
- Store migrations run in order at open, each in a transaction (`yulius-store/src/lib.rs`);
  `App::start_background` runs reconciliation once after start.

## Requirements

| ID | Requirement | Evidence |
| --- | --- | --- |
| RCT-001 | A run's lines are a window of its session's transcript: from its start (a new session's run: the first line; a resumed run: the first line recorded at or after its start, as MET-002) to its end, the first line recorded at or after the earlier of its end and the next start of a run of the same task on the same session, which is not the run's. Its sub-agent transcripts count over the same window, their usage only (MET-004 bounds). | `subagents::tests::a_window_ends_at_the_first_line_at_or_after_its_end`; `recount::tests::each_run_of_a_resumed_session_counts_its_own_window` |
| RCT-002 | Recounting a run reads its window, with the same accumulator as the live watch, so it gives the figures the live watch of that run gives today: tokens with sub-agents, Claude Code's turn records as turns, active time as the union of turn spans. A missing transcript is reported as missing, not as zero; a session id that is not one (`../x`, `/x`) names no transcript (checked as an id, not as the path it would build); a sub-agent transcript that cannot be read fails the count (partial tokens are not the run's). | `recount::tests::each_run_of_a_resumed_session_counts_its_own_window`, `a_missing_transcript_is_not_zero`, `a_sub_agent_transcript_that_cannot_be_read_fails_the_count`; fixture `claude/resumed.jsonl` (real Claude Code 2.1.292 shapes); `metrics_recount::old_runs_get_their_transcripts_figures_once` (`a7`, `a8`) |
| RCT-003 | Upgrading the store queues every run recorded so far for one recount (migration 23, `metrics_recounts`), with its stored figures as they were then (`before`), before this start's reconciliation saves anything; runs started after it are never queued. | `store::metrics_recounts_queue_the_runs_recorded_before_the_upgrade_once`, `a_recount_replaces_the_figures_and_records_both` |
| RCT-004 | After start, once reconciliation has ended the runs that did not survive the restart (not at all when it failed: a lost run would read as live), Yulius recounts each queued run and records its outcome, the figures it stored (`after`) and when: `recomputed` (figures replaced), `no_transcript` (no session, worktree or transcript: the row stays as it was), `no_lines` (the transcript holds nothing of the run: the row stays), `live` (the run still runs: its watch already reads its transcript by MET-002), `gone` (the run was deleted). A recorded run is never recounted, so a restart after it changes nothing; a crash before a run is recorded recounts it again, with the same result. A model the transcript does not name keeps the stored one. One audit entry sums the round. | `metrics_recount::old_runs_get_their_transcripts_figures_once`; `store::a_recount_replaces_the_figures_and_records_both` |
| RCT-005 | The recount never blocks start: it runs in its own background task, reads files off the async workers (`spawn_blocking`), one run at a time, and a failure of one run is logged and leaves that run queued for the next start, the others going on. | `metrics_recount::old_runs_get_their_transcripts_figures_once` (`a6` fails while its transcript cannot be read, stays queued, and is counted the next round); `background.rs` (code review) |

## Plan

- [x] Spec, ADR and plan for recounting old runs
- [x] A run's own window of a transcript is counted by the live rules
- [x] The store queues old runs once and records each recount
- [x] Yulius recounts queued runs after start and records the outcome
- [x] Docs, review fixes and gates green

## Acceptance criteria

- On a store upgraded from version 22 with ended runs whose transcripts exist, the first start
  replaces each run's stored figures with its transcript's, records `recomputed` with both, and
  a run whose transcript is gone keeps its row and is recorded `no_transcript`.
- A second start recounts nothing and changes no figure.
- The fixture of a resumed session with a sub-agent, goal continuations, an interrupt and a turn
  that waited for background agents gives each run exactly its own figures.
- `scripts/dev ci` passes.

## Test plan

- Unit (`yulius-harness`): the window's end in `LinesSince`/`SubagentTails`; `recount` over the
  fixture for both runs of a resumed session and a missing file.
- Store (`yulius-store`): migration 23 queues runs recorded before it, not after; finishing a
  recount upserts the figures, keeps a stored model the recount lacks and records before/after;
  deleting a task deletes its recount rows.
- Integration (`yulius` crate, `metrics_recount`): a real store upgraded with old rows and a real
  harness adapter over transcripts on disk: figures replaced, a missing transcript kept, a live run
  left, a second round a no-op.

## Decisions

- **Queue in the migration, recount in the background.** The migration knows exactly which runs
  existed before the upgrade and records it in the same transaction; the recount needs the
  filesystem and the harness, which the store must not touch, and must not delay start. ADR
  [0072](../../docs/decisions/0072-recount-old-runs-from-their-transcripts-once.md).
- **Every run recorded before the upgrade is queued, not only those before 2026-10-14 00:18.**
  The deploy time of SPEC-087 differs per instance and is recorded nowhere. Recounting a run
  SPEC-087 already counted gives the figures it has (the same accumulator over the same lines), so
  queuing them costs a file read and is correct, while a timestamp cut-off would be wrong on every
  other instance.
- **A run's window ends at its end.** The live watch stops when its run ends; a later run of the
  same session (or a person's terminal on it) writes to the same file afterwards. Ending at the
  earlier of the run's end and the next run's start partitions the session's lines between its
  runs like the live watches do (MET-002), so no line counts twice.
- **Nothing replaces figures with nothing.** A missing transcript or one without a line of the run
  leaves the stored row: the request keeps runs without a transcript as they are, and an empty
  window is more likely a moved file than a run without work.
- **"Before" is taken by the migration.** On the upgrade's start, reconciliation ends a run lost
  while Yulius was down and saves its empty figures over its row (an older behaviour, #385);
  taken at queue time, `before` is what the upgrade found, so the record stays checkable.
- **Equal to the live watch, or a little more complete.** The recount folds the session's lines,
  then its sub-agents'; the live watch interleaves them per poll, and may miss lines written in
  the second before its run ended; a message whose repeated lines a poll splits around another
  file's keeps its first usage there. So the recount equals the live figures or exceeds them a little.
- **The record is a table, plus one audit entry.** Each run's before and after figures make the
  change checkable and reversible by hand; the audit entry says when the round ran and what it did.

## Open questions

None.

## Evidence

- Fixture `claude/resumed.jsonl`: record types and keys checked read-only against Claude Code
  2.1.292 transcripts in this instance's sandbox home (2026-10-14); figures derived by hand
  (run one: 1,510 output tokens, 3 turns, 220.2 s; run two: 400, 1, 39.1 s) and matched by
  `recount::tests::each_run_of_a_resumed_session_counts_its_own_window`. Without the window's end
  run two would count the terminal's turn (1,399 tokens, 2 turns), which the test asserts.
- Mutation checks: `metrics_recount::old_runs_get_their_transcripts_figures_once` fails when the
  window ignores the next run's start (run one: 1,910 tokens, 4 turns) and when the session id is
  not validated as an id (the absolute-path id of `a8` counted).
- Tests written first and seen failing: the window API (compile), the store's `before` at queue
  time and the sub-agent read failure (assertions).
- `scripts/dev ci` (doctor, check, lint with clippy `-D warnings`, nextest, doctests, deny,
  shear, doc): exit 0, 1,509 tests passed before the review fixes and 1,510 after them.
- Independent review (fresh session, `om-code-review`): round one requested changes (failure path
  untested; `before` read after reconciliation; sub-agent errors cut the count; untested
  `no_lines` and next-run window; session id); all fixed test-first. Round two found the session
  id checked on the built path, which an absolute id (`join` takes it whole) passed, and the
  upgrade test built on a downgraded latest schema: the port now takes the id and validates it,
  and the test store is built at version 22 (`yulius_store::create_at_version`). The
  reconciliation overwrite that predates this change is #385.
- Not testable here: the production database (41 of 109 runs at 0 active time); the recount runs
  there on the first start after `dev` reaches it.
