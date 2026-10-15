# 0112. Every stage is a span, written with the state it measures

## Status

Accepted. Specified in [SPEC-137](../../.ai/specs/SPEC-137-rich-measurements-of-every-stage-tasks-c.md);
task #116. Builds on [0070](0070-run-metrics-come-from-the-transcript.md) (a run's tokens and
working time) and [0109](0109-cd-runs-verified-commits-out-of-the-agents-reach.md) (CD runs).

## Context

"We want rich measurements of every stage of work, every single measurement we can get, recorded
durably and shown in the UI", with "one consistent stage/span model in the store […] shared by
tasks, CI and CD; recorded at the source as things happen", history "backfilled where it is
reliably derivable and marked as such", measurement that "must never slow down or fail the work it
measures", bounded retention, and measurements agents cannot fake or alter.

The states already had their times (task status changes, `step_events`, `attempts`, `metrics`,
`capacity_pauses`, `delivery`, `ci_runs`, `ci_jobs`, `cd_runs`), but most kept only the current
value: a task's earlier states, a pull request's open → merged time and a CI run's wait before a
requeue were lost. Nothing split a CI job into its gates or a deploy into deploy and verify.

## Decision

- **One `spans` table** (migration 28): a span is one stage of one subject (`task`, `ci_run`,
  `cd_run`) with a key unique within the subject (a child's key starts with its parent's), a kind
  (`state`, `step`, `run`, `pause`, `delivery`, `wait`, `update`, `queue`, `prepare`, `job`, `stage`,
  `promote`, `deploy`, `verify`), a name, an optional environment and CI job, start, end,
  outcome, `source` (`recorded` or `backfilled`) and JSON figures. A new stage needs no schema.
- **State-derived spans are written by triggers**, in the same transaction as the state change,
  with the time the application wrote (its clock, test clocks included). A state cannot change
  without its span, and no call site can forget one. The triggers never fail the write they
  follow: every insert is guarded by `NOT EXISTS` (SQLite applies an outer upsert's conflict policy
  to a trigger's `OR IGNORE`, so that guard is not enough).
- **What has no state row is written by Yulius as it happens**: a CI run's preparation, a job's wait
  for its slot and its checkout, a CD run's checks, promotion and the person's confirmation (as the
  page measured it, administrators only, up to an hour), a task's armed-merge wait and updates onto
  the integration branch, and the stages a job's or deploy's output names. A failed write is logged
  and dropped.
- **Stages inside output are named by the job and timed by Yulius.** A line `::yulius-stage <name>`
  starts a stage, `::yulius-stage-end` ends it, `::yulius-measure k=v …` adds figures; summaries of
  cargo nextest, cargo test, pytest and Playwright are counted as tests. The time is when Yulius reads
  the line, so a job only divides its own time, which Yulius measures. The commands come from the
  protected branch (ADR-0109). At most 400 stages a subject, lines of at most 4 KiB.
- **No API, MCP tool or hook writes or changes a span**; the API reads them (timeline, summary,
  export) with read access to the sandbox.
- **Backfill** in the migration from what the old tables say reliably: CI queues, runs and jobs,
  CD runs, task runs with their figures, steps from their events, pauses, a live task's current
  state; marked `backfilled`. A reused CI result (an identical tree) is marked `reused` and never
  counted as a run.
- **Retention**: spans older than 180 days go with the hourly CI and CD upkeep.
- **Delivery figures** are computed on read: a merge into the integration branch is delivered by the
  first passed deployment of an environment that started after it (an environment deploys its
  branch head); the change failure rate is failed over finished deployments; a failure is restored
  by the next passed one.
- **The Pipeline page** (nav "Pipeline (stages)", beside CI and CD) holds the figures, trends and
  stage table; CI runs, CD runs and task overviews show their own waterfall; the Dashboard shows the
  overview.

## Alternatives

- **A recorder call at every state change.** Dozens of call sites in the supervisor, delivery, CI
  and CD, each a chance to miss one, and a write that can disagree with the state it follows.
- **Reconstructing spans from the existing tables on read.** Most tables keep only the current
  state, so earlier states are lost; stages without a table have nowhere to go.
- **Parsing stages out of stored logs.** The stored output is a bounded tail and is rewritten
  whole; times would be guessed.
- **Stages timed by the job (timestamps in its lines).** A job could claim any duration.
- **The pipeline inside the CD page or the Dashboard.** It covers tasks and CI too; the Dashboard
  keeps the overview.

## Consequences

- Every task, CI run and CD run has a timeline from now on, and percentiles of any stage are one
  query away; other projects' jobs get stages by printing one line, and test counts for free.
- A change of the state tables' writes needs the triggers kept in mind (the store tests cover each
  one); a trigger bug would surface as a failed state write, which the tests check never happens
  for a span that already exists.
- A job's figures cannot overwrite what Yulius records of a span (reserved names), and repeated
  figures are written once per span and read. A deploy script can still move the boundary between
  its own deploy and verify by printing the wrapper's line; the run's total, outcome, commit and
  trigger stay Yulius's.
- Measurements describe what Yulius saw: a stage is accurate to the half second at which job output
  is read, and a backfilled span may be shorter than the truth (a CI run's wait before a requeue).
