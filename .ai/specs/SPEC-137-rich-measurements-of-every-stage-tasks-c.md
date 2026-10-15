---
yulius-task: 72a9717c-ce79-4b0c-aa63-04eaf04ef7e2
status: in-progress
---

# SPEC-137: every stage of tasks, CI and CD measured as it happens and shown as timelines and trends

Status: in progress
Date: 2026-10-15
Owner: Yulius task #116
Scope authorization: the person's request (task #116): "We want rich measurements of every stage
of work, every single measurement we can get, recorded durably and shown in the UI. […] Measure,
at minimum: 1. Tasks: time queued/chained/prepared before start, each step's duration […], time
waiting for CI, time in "needs you" and how long a person took to answer, PR open → merged,
merge-train wait, conflicts/updates onto dev, retries, stops/usage-limit pauses, tokens and agent
working time per step where it can be attributed. 2. CI: queue wait (and why […]), checkout/setup,
each job and each stage inside a job […], test counts, failures and flaky reruns, reused-tree
results (marked as reused, never counted as real runs), cache hits, and resource use […] where
available. 3. CD and deployments: every deployment to dev and prod with each of its stages […],
who/what triggered it, the confirmation wait for prod, outcome, and the version/commit deployed.
Also […] deployment frequency, lead time from merge to deployed, change failure rate and time to
restore. 4. Anything else in the pipeline that has a start and an end […] with the same stage
model, so new stages are measured without new plumbing. Show it: overview figures in the
Dashboard, and a dedicated place for the pipeline […] per-stage timelines/waterfalls […] trends
over time with percentiles (p50/p90) […] filters by project/sandbox, environment, job and stage
[…] tables and CSV/JSON export like the Dashboard's. Constraints: one consistent stage/span model
in the store (with a migration) shared by tasks, CI and CD; recorded at the source as things
happen […] with existing history backfilled where it is reliably derivable and marked as such;
measurement must never slow down or fail the work it measures; retention is bounded; it
generalises to other projects […]. Agents must not be able to fake or alter measurements."

## Goal

Every piece of work that has a start and an end (a task and its states, steps, runs, pauses and
delivery; a CI run, its queue, preparation, jobs and the stages inside them; a CD run, its
promotion, deploy, verify and the stages inside them) is one **span** in one table, written by
Yulius at the moment the state it measures changes. People see one run, task or deployment as a
waterfall of its spans, the trend of any stage over time with p50 and p90, the stage table with
CSV/JSON export, and the delivery figures (deployment frequency, lead time, change failure rate,
time to restore), on a Pipeline page beside CI and CD and as overview figures on the Dashboard.

## Context

- **What is measured today.** The Dashboard (SPEC-087, SPEC-110, `yulius-app/src/analytics.rs`)
  aggregates per task: tasks per day, agent working time (from transcripts, ADR-0070), tokens,
  subscription switches, feedback. CI stores `queued_at`, `started_at`, `ended_at` per run and
  `started_at`, `ended_at` per job (`ci_runs`, `ci_jobs`); CD stores `started_at`, `ended_at` per
  run (`cd_runs`, SPEC-134), deploy and verify not apart. Steps have `step_events` (each status
  change with its time); usage-limit and capacity pauses have `capacity_pauses`; runs have
  `attempts` and `metrics`; delivery has one row per task with its current stage only.
- **What is not measured.** Nothing records how long a task waited before it started, how long it
  needed a person, how long its pull request stayed open; nothing splits a CI job into its gates or
  a CD run into deploy and verify; nothing computes percentiles over time or delivery figures.
- **CD** (task #113, SPEC-134, ADR-0109/0110): Yulius runs `[cd.dev]` and `[cd.prod]` from the
  protected branches through the process host, like a CI job (`ci_watch`); production is a
  person's, promoting the integration branch first when `[cd.prod] promote = true`.
- **Who can write.** Only the Yulius process writes the database; agents reach Yulius through MCP
  and hooks, neither of which can write measurements. A CI job's or CD run's output comes from the
  commands in `yulius.toml`, read from the integration branch (CI) or the protected branch (CD).

## Requirements

Model (STG-0xx):

| ID | Requirement | Evidence |
| --- | --- | --- |
| STG-001 | One table `spans` (migration 28) holds every measured stage of every subject: `subject_kind` (`task`, `ci_run`, `cd_run`), `subject_id`, the sandbox, a `key` unique within the subject (its path: `job:quality/stage:lint`), its parent key, `kind` (what sort of stage: `state`, `step`, `run`, `pause`, `delivery`, `queue`, `prepare`, `job`, `stage`, `promote`, `deploy`, `verify`, `wait`), `name`, `environment`, `job`, start, end (open while it runs), `outcome`, `source` (`recorded` or `backfilled`) and `detail` (JSON figures: tokens, tests, reason, commit, trigger…). A new stage of any kind needs no schema change. | store tests `spans::*`; migration test |
| STG-002 | Spans are written by Yulius only, at the moment the state they measure changes: state-derived spans by triggers in the same transaction as the state's own write (so they are never missed and never disagree with it), output stages by Yulius as it reads the output. No API, MCP tool or hook writes or changes a span; times are Yulius's clock, never a value from a job, an agent or a page. | store trigger tests; integration `stages::*`; route list has no write route |
| STG-003 | Measuring never fails or slows the work: a span write that fails is logged and dropped; output stages are scanned from the bytes Yulius already reads, bounded (line length, stages per job, figures per stage). | `stages::tests::scanner_bounds`; store test: a failing span write leaves the CI job's own write |
| STG-004 | Existing history is backfilled once by the migration where it is reliably derivable, with `source = backfilled`: CI runs (queue, run, jobs), CD runs (run), task runs (attempts, with their metrics), steps (from `step_events`), pauses (`capacity_pauses`). Nothing else is guessed. | migration test with seeded rows |
| STG-005 | Retention is bounded: spans older than 180 days go, and a subject's output adds at most 400 stages; pruning runs with the hourly CI and CD upkeep. | `spans::tests::spans_are_filtered_limited_and_pruned`; `stages::tests::a_job_s_stages_are_bounded` |

Tasks (STG-1xx):

| ID | Requirement | Evidence |
| --- | --- | --- |
| STG-101 | A task's state track: each change of its status or activity ends the open `state` span and opens the next one, named `queued`, `chained`, `prepared`, `starting`, `working`, `idle`, `needs_you`, `done`, `paused` (waiting for capacity), `switching`, `stopping`; an ending status (closed, stopped, failed) ends the track. `needs_you` also covers an agent's `needs_you` request, so its duration is how long a person took to answer. | store trigger test; integration |
| STG-102 | Each step's `in_progress` → `done`/`skipped`/`blocked` is a `step` span named by its title, with the tokens and agent working time the task's runs recorded between its start and its end (the step's attribution; absent when the task had no metrics). | store trigger test |
| STG-103 | Each run (attempt) is a `run` span with its launch kind, subscription and, once known, tokens, turns and active time; each capacity or usage-limit pause is a `pause` span named by its kind with its outcome. A task's retries are its runs after the first. | store trigger test |
| STG-104 | Delivery: each change of a task's delivery stage ends the open `delivery` span and opens the next (`pr_open` until merged is the pull request's open → merged time); CI runs of the task's pull request are shown in its timeline as its time waiting for CI. An armed merge's wait for its checks (one by one or in a merge train) is a `wait` span `merge_armed` (merged or dropped), and each update of the branch onto the integration branch Yulius makes is an `update` span (merged or conflict). | integration; store test |

CI (STG-2xx):

| ID | Requirement | Evidence |
| --- | --- | --- |
| STG-201 | A CI run's `queue` span runs from queued to started (or to its end when it never started), with the reason it waited (`lane busy`, the disk wait text); its `run` span from start to end with the state, the commit, the reference and who asked; each job a `job` span with its exit code and state, and its wait for a job slot; a rerun is a new run (its own spans), a requeue ends what ran (`requeued`) and opens a new queue span; a result reused from an identical tree is marked `reused` and excluded from durations and trends. | store trigger test; integration `local_ci` stages |
| STG-202 | A job's preparation (checkout) is a `prepare` span recorded by Yulius. | integration |
| STG-203 | Inside a job, a line `::yulius-stage <name>` starts a `stage` span (ending the one before) and `::yulius-stage-end` ends it; the job's end ends an open stage with the job's outcome. `::yulius-measure k=v …` adds figures (tests, cache hits…) to the open stage or the job. Test summaries of common runners (cargo nextest, cargo test, pytest, Playwright) are read into `tests_passed`, `tests_failed`, `tests_skipped`, `tests_flaky`. The times are Yulius's, taken as the line is read. | `stages::tests::*`; integration |
| STG-204 | Yulius's own gate prints its stages (`doctor`, `check`, `lint`, `test`, `deps`, `docs`). | xtask test |

CD (STG-3xx):

| ID | Requirement | Evidence |
| --- | --- | --- |
| STG-301 | A CD run is a `run` span with environment, branch, commit, trigger (a login, `task:#N`, an automation) and outcome; its promotion, deploy and verify are spans of their own; `::yulius-stage` lines in their output (build, upload, migrations, restart, smoke, rollback) are stages inside them. | integration `cd` stages |
| STG-302 | A production deploy records how long the person kept the confirmation open (`confirm_ms`, measured by the page, administrators only, bounded to an hour). | route test; e2e |
| STG-303 | Delivery figures per environment over a range: deployments (passed) and a frequency per week, lead time from a merge into the integration branch to the end of the first passed run of that environment that started after it (p50, p90), change failure rate (failed runs / all finished runs), and time to restore (a failed run to the next passed one, p50). | `stages::tests::delivery_figures_*` |

Show (STG-4xx):

| ID | Requirement | Evidence |
| --- | --- | --- |
| STG-401 | API (read access): `GET /api/sandboxes/{sid}/stages/timeline?subject=<kind>:<id>` (one subject's spans, a task's with its pull request's CI runs), `GET …/stages/summary?from&to&kind&environment&job&stage` (per stage count, failures, p50, p90, max; daily p50/p90 of the chosen stage; delivery figures), `GET …/stages/spans?format=csv|json&…` (export). | route tests; integration |
| STG-402 | A Pipeline page (nav "Pipeline", beside CI and CD): range, kind, environment, job and stage filters kept in the address; delivery figures; the chosen stage's trend with p50 and p90 lines and a table; the stage table; export. | e2e `pipeline.e2e.ts` |
| STG-403 | One subject's waterfall (a bar per span on a shared time axis, nested by parent, waits and failures marked, durations and figures on hover and in a table) in the CI run dialog, the CD run dialog and a task's overview. | e2e |
| STG-404 | The Dashboard shows the overview: deployments, lead time p50, change failure rate, CI run p50/p90, linking to the Pipeline page. | e2e |

## Plan

- [x] Spans stored, written by triggers for tasks, CI and CD, history backfilled
- [x] CI preparation, job stages and test figures recorded from the output
- [x] CD promotion, deploy, verify and their stages recorded, with the production confirmation
- [x] Timelines, stage percentiles, trends, delivery figures and export served
- [x] Pipeline page with filters, trends, stage table and export
- [ ] Waterfalls in CI runs, CD runs and task overviews, overview on the Dashboard
- [x] Retention bounded and Yulius's gate prints its stages
- [x] Docs, ADR and changes updated
- [ ] Independent review approves the exact head
- [x] Review round 1: backfill and triggers indexed, figures coalesced and reserved, timelines bounded and scoped, the confirmation sent
- [ ] Every gate green and the merge into dev armed on Yulius CI

## Acceptance criteria

- AC1 (STG-001..005): a CI run, a task and a CD run each have spans with real durations written as
  their states change; seeded history comes back backfilled; old spans are pruned.
- AC2 (STG-101..104): a task that is queued, works, needs a person, finishes a step and opens and
  merges a pull request has those spans with the step's tokens.
- AC3 (STG-201..204): a CI job that prints stages and a nextest summary has `stage` spans with test
  figures; the queue span names why it waited.
- AC4 (STG-301..303): a dev deploy has `deploy` and `verify` spans and its stages; delivery figures
  are computed from seeded deploys and merges.
- AC5 (STG-401..404): the Pipeline page, the three waterfalls and the Dashboard overview show the
  figures (e2e), and the export downloads CSV and JSON.

## Test plan

- Domain unit tests (`yulius_domain::stages`): the output scanner (markers, test summaries,
  bounds, split chunks), percentiles, delivery figures.
- Store tests: triggers for each track, backfill in the migration, export and summary queries,
  pruning.
- Integration tests (`crates/yulius/tests/stages.rs`): a real CI run through the process host with
  stage lines; a CD dev run; a task's lifecycle through the API; the API's timeline, summary and
  export, access checks.
- e2e (`e2e/tests/pipeline.e2e.ts`): Pipeline page, waterfalls, Dashboard overview, axe.
- Gates: `scripts/dev ci`, `scripts/dev security`, the e2e suite.

## Decisions

- **One `spans` table, written in the same transaction as the state it measures** (ADR-0112).
  Triggers on the state tables (`tasks`, `step_events`, `attempts`, `metrics`, `capacity_pauses`,
  `delivery`, `ci_runs`, `ci_jobs`, `cd_runs`) keep spans exact and complete with no code at each
  of the dozens of places that change those states, and use the time the application wrote (its
  clock, test clocks included). Stages inside output have no state row, so Yulius writes them as
  it reads the output. Alternatives: a recorder call at every state change (dozens of call sites
  in the supervisor, delivery and CI, each a chance to miss one); reconstructing spans from the
  existing tables at read time (no durable model, no place for stages without a table, the
  history of a changed row is lost).
- **Stages inside a job are named by the job, timed by Yulius** (ADR-0112). A job prints
  `::yulius-stage <name>`; the time is when Yulius reads the line. The commands come from the
  protected branch, and a stage can only divide the job's own time, which Yulius measures.
- **The Pipeline page is its own page beside CI and CD** (ADR-0112): its subject is all three;
  inside CD it would hide CI and tasks, and the Dashboard keeps the overview only.
- **Not in this change** (recorded as follow-up): a job's actual CPU, memory and disk use
  ([#488](https://github.com/Quartio-Studio/yulius/issues/488)); CI jobs share the sandbox's `ci/`
  cgroup today, so per-job use needs its own accounting. Cache hits are figures a job prints
  (`::yulius-measure cache_hits=…`). Onboarding and automation runs are tasks, so their stages are
  the task's; promotions are a CD production run's `promote` span and a task's `promoted` delivery
  stage.
- **One pull request, larger than usual** (about 2,000 lines of code outside tests, docs and the
  mock). The request's done criteria need the model, its recording for all three subjects, the
  read API and the page together; a split would merge a table nothing reads, or a page with
  nothing to show. Each commit is one slice that keeps the project working.
- **Review round 1** (all fixed test-first unless noted): the step backfill and the step trigger
  scanned whole tables (indexes `step_events(step_id, id)` and `metrics(task_id)`; a 25k-event
  history now migrates in well under 5 s, 26.6 s before); output that repeats figures wrote one
  transaction per line (now one write per span and read, added up or the last value); a job's
  `::yulius-measure` could overwrite what Yulius records (`sha`, `cpus`, `task_id`… are reserved);
  same-millisecond changes lost a state span (keys carry the span's sequence); a timeline was
  unbounded (newest 2,000, `truncated`) and redrew on every poll (only when it changed); a task of
  another sandbox showed this sandbox's CI runs of the same pull request number (scoped); the page
  never sent `confirm_ms` (it does). Decided, not changed: a deploy script can print the wrapper's
  `[yulius] verify: …` line and so move the deploy/verify boundary inside its own run; the run's
  total, outcome, commit and trigger stay Yulius's, and the scripts are reviewed on the protected
  branch (splitting deploy and verify into two processes would change how CD runs; ADR-0112
  consequences). Clearing a `needs_you` request now dates it in `tasks.updated_at` (the last change
  of anything, as for every other change). Output Yulius skips when a job writes faster than it is
  read (more than 2 MiB behind) can lose a stage line; the stage then ends at the next one.
