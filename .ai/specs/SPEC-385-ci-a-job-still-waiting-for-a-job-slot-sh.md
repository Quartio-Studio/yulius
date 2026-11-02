---
yulius-task: f43d93db-327d-4b1e-954d-9face3eff69a
status: in-progress
---

# SPEC-385: a CI job still waiting for a job slot shows in the run's stages while it waits

Status: in-progress
Date: 2026-11-02
Owner: Yulius task #367
Scope authorization: task #367, Fixes #536 (filed by task #155, SPEC-165).

## Goal

A CI job still waiting for a job slot (and its CPU and memory budget) shows in its run's
waterfall, under "Jobs", with a hatched bar reaching "now" ("waiting 4 min so far"), and becomes
its normal row (its wait, then its run) once it starts. A job cancelled while it waited keeps its
row, ended `cancelled`.

## Context

- `ci::exec` `ci_job` (`crates/yulius-app/src/ci/exec.rs`) takes `waiting = self.now()` before it
  awaits the slot and the budget, but writes the `job slot` span (`wait:<job id>@<ms>`, kind
  `wait`, `job` = the job's key) only after the job moved to Running, already ended at its start
  with outcome `started`. A waiting job has no span; one cancelled while waiting, one whose permit
  is `None` (the semaphores closed) or whose move to Running fails gets none at all.
- Every job of a run is spawned at once and waits in `ci_job`, so a busy run (21 jobs, 12 running)
  draws 12 job rows and leaves out the 9 waiting ones; only the dialog's job chips list them.
- The store already ends what is left open: `spans_ci_run_end` ends every open span of the run
  with the run's state, and `spans_ci_run_requeue` ends them `requeued`
  (`yulius-store/src/migrations/0028_spans.sql`). `spans_ci_job_end` touches only `job:<id>@…`
  keys, so it never ends a wait. `SpanStore::end_span` ends an open span (`ended_at IS NULL`) and
  keeps its outcome when given none; `App::record_spans` applies `SpanOp::End` best effort.
- Stage summaries (`summarize` in `crates/yulius-app/src/stages.rs`) count an open span as
  `running` and put only ended spans in p50, p90, max and the trend, so `CI · Waiting for a job
  slot`'s figures stay those of ended waits.
- `stages.js` (`crates/yulius-server/web/assets/stages.js`, SPEC-165): `slotWaits` pairs a wait
  with the job span of the same job id that started at or after it; `grouped` builds "Jobs" from
  root spans of kind `job` only, so an unpaired wait falls under "Run" as "Waiting for a job slot"
  (SPEC-165 STR-001 kept it there on purpose, and its Decisions left waiting jobs out, pointing to
  #536).

## Requirements

- WSL-001. `ci_job` opens the job's `job slot` wait span (`wait:<job id>@<waiting ms>`, kind
  `wait`, name `job slot`, `job` = the job's definition name, as the job span's; its budget and its
  display name, a matrix job's "package (gateway)", as `detail`) when the wait begins, open
  (`ended_at` none, no outcome).
- WSL-002. It ends that span when the wait ends: at the job's start with outcome `started`; when
  the run is cancelled while the job waits, at the cancellation with outcome `cancelled`; when it
  gets no permit, outcome `error`; when its move to Running is refused (the job left Queued
  meanwhile: only a cancellation does that), outcome `cancelled`, and when that move fails in the
  store, outcome `error`. A run that ends or is requeued with the span still open ends it through
  the existing triggers, as before.
- WSL-003. In a CI run's waterfall, a `job slot` wait with no job span of its job id at or after it
  is drawn as that job's row under "Jobs", named by its `detail.name` (else its `job`), in waiting
  order with the other jobs. Open: a hatched bar reaching "now", an hourglass mark labelled
  "waiting", the duration column "waiting 4 min so far", the title "<job>: waiting 4 min so far
  for a job slot". Ended (cancelled, or a lone ended wait of older data): a hatched bar, its
  outcome's mark when it has one (`cancelled`: the cross), "waited 12 s", and the title "<job>:
  waited 12 s for a job slot, <outcome>". In the table its time is in the Waited column ("8 min so
  far"), Took "—". This amends SPEC-165 STR-001, whose lone wait kept a "Waiting for a job slot"
  row under "Run". A wait with no `job` keeps that row.
- WSL-004. Once the job starts its row is its normal one: the ended wait as the hatched lead of its
  job's bar (SPEC-165 STR-001). A job span whose paired wait is still open (the moment between the
  job's start and the wait's end) leads with the wait up to the job's start.
- WSL-005. Stage summaries (p50/p90 of `CI · Waiting for a job slot`) count only ended waits; an
  open wait counts as running.

## Plan

- [x] Read the product context and related work, then write and validate the spec
- [x] ci::exec opens the job slot span when the wait begins and ends it started, cancelled or error
- [x] Stage summaries proven to count only ended waits
- [x] The waterfall draws a job still waiting (or cancelled while waiting) as its row under Jobs
- [ ] Independent review in a fresh session, findings fixed test-first
- [ ] Every gate green, docs and the changes log updated

## Acceptance criteria

- AC1 (WSL-001, WSL-002, WSL-004): in `crates/yulius/tests/local_ci.rs`
  `jobs_wait_for_their_cpu_budget_and_are_told_it`, while a `build` job holds its CPUs the other
  run's jobs that wait have open `job slot` spans (no end, no outcome) with their `job`; once all
  passed, every job has one `job slot` span ended `started` at its job span's start.
- AC2 (WSL-002): a job cancelled while it waits for a slot (its run superseded by a push) has its
  `job slot` span ended `cancelled`, and no job span.
- AC3 (WSL-005): a unit test of `summarize` with an open and two ended `job slot` waits: count 2,
  running 1, p50/p90 from the ended ones.
- AC4 (WSL-003): `pipeline.e2e.ts`: mock run `ci_s4` gains two jobs still waiting; they show under
  "Jobs" in waiting order with "waiting … so far", the hourglass mark and a hatched running bar,
  and no "Waiting for a job slot" row. Mock run `ci_s3`'s lone wait (`e2e`, now cancelled) shows
  as the row "e2e" under Jobs with "waited 12 s" and the cancelled mark.
- AC5: the waterfall still passes axe; the phone width check of `ci_s4` still fits.

## Test plan

- Rust first: extend `jobs_wait_for_their_cpu_budget_and_are_told_it` (AC1) and add the
  cancellation test (AC2) beside it; see both fail against today's `ci_job` (no open span; no span
  when cancelled), then change `ci_job`. `a_run_s_jobs_record_their_stages_and_test_counts` keeps
  its ended `job slot` assertion.
- AC3 as a unit test in `stages.rs` (it passes today: a regression guard, said so in Evidence).
- e2e first: change the mock and the two tests, see them fail against today's `stages.js`, then
  change `stages.js`; run `pipeline.e2e.ts` and the whole suite.
- Project gate `scripts/dev ci`; security scan (disabled in Yulius: reported as disabled).

## Decisions

- Open the span with the wait, then end it, rather than inserting it ended at the start: the span
  then says what is true while it is true (ADR-0112: stages are spans written with the state they
  measure). One store write more per job, before it waits; a failed write is logged and dropped as
  every span write (SPEC-137 STG-003), so a job never waits on it.
- Outcomes: `started`, `cancelled`, `error` (no permit). A refused move to Running ends it
  `cancelled` without reading the job back: the only writer that moves a Queued job of a running
  executor is the cancellation of a lost executor (`ci_cancel_jobs`). A requeue or the run's end
  still ends a span left open through the triggers, so no path leaves a wait open forever.
- A lone *ended* wait with a `job` also goes under "Jobs" (not only an open one): a job cancelled
  while waiting is a job of the run and its chip is in the dialog; a "Waiting for a job slot" row
  under "Run" did not say which job. A wait without `job` (none is written today) keeps the old row.
- The waiting mark is the hourglass `st-queued` icon the task list uses for "in line for a slot"
  (#147), not the running ring: the job does not run yet.
- No ADR: this follows ADR-0112 and SPEC-165's design; SPEC-165 STR-001 is amended in place with a
  pointer here.
- No migration: the triggers and `end_span` already cover open waits.
- The display name goes in `detail.name`, not in `job` (review round 1): `job` is the definition
  name in every span (the job span's comes from `job_key`), and the summaries group by it and the
  Job filter lists it, so a matrix job's waits still count together.
- A failed end write (best effort, SPEC-137 STG-003) now leaves the wait open until the run's end,
  which ends it with the run's outcome and a longer duration, where the old single write left no
  span. Accepted: a store that refuses one write refuses the job's other writes too, and the wait
  then still shows.
- A wait ended `requeued` (a restart while the job waited) is its job's row under "Jobs" with no
  mark, beside the row of the job's later attempt, as a requeued job's two runs already are.
