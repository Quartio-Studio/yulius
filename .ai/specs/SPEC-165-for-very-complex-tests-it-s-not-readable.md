---
yulius-task: 6a231da4-60bb-4232-990a-ae5050c2c4be
status: in-progress
---

# SPEC-165: a CI run's stages read as one row per job, on a time axis

Status: in progress
Date: 2026-10-15
Owner: Yulius task #155
Scope authorization: task #155 ("For very complex tests it's not readable at all. Even for small
ones i'm not sure how to read it. Fix the UI/UX", with a screenshot of the CI run dialog of PR
#486). Refines SPEC-137 STG-403 (one subject's waterfall).

## Goal

A person opening a CI run (or a deploy, or a task) sees at a glance which job waited, how long, when
it ran and how it ended, without matching bars across rows or decoding their look.

## Context

The screenshot shows the "Stages of this run" waterfall of a running PR run with 21 jobs. It is
`waterfall()` in `crates/yulius-server/web/assets/stages.js`, shared by the CI run dialog
(`pages/ci.js`), the CD run dialog (`pages/cd.js`), a task's overview (`pages/taskinfo.js`) and the
Pipeline page. Spans arrive from `GET /stages/timeline` (SPEC-137) and are drawn one row each,
nested by `parent_key`. What makes it unreadable:

1. **Anonymous waits.** `ci::exec` records a job's wait for a job slot as a root span
   `wait:<job id>@<ms>` named `job slot`, with no parent. Every one reads "Waiting for a job slot":
   eight identical rows that say nothing about which job waited.
2. **A job is split in two places.** The wait sits in the top block and its job
   (`job:<job id>@<ms>`, opened by the store's trigger) many rows below; the reader must line up a
   hatched bar's end with a solid bar's start by eye.
3. **Noise.** Each job carries a `Checkout` child (0–1 s), so half the rows are "Checkout 0 s".
4. **No scale.** Bars have no time axis, so "where in the run" is guesswork, and there is no legend
   for hatched, grey, accent and red bars.
5. **Labels repeat.** Every job row starts with "Job ".

Nothing in the data is missing: the wait's key and the job's key carry the same job id, and spans
carry their `job`. This is a presentation change; the API and the spans stay as they are.

## Requirements

- STR-001. In a CI run's waterfall, a job's wait for a job slot is drawn on the job's own row: one
  bar with a hatched part (waiting) followed by the solid part (running). The row is named by the
  job ("engine", "package (gateway)") under a "Jobs" heading. Its duration column says how long it
  ran and, beneath, how long it waited ("waited 6 min"). Hover and the row's title say both. A
  wait whose job span is not there is still drawn on its own row, named "Waiting for a job slot".
- STR-002. Jobs are listed in the order they began waiting (or started), and the run's own spans
  (queue, run, fetching the commit) come first, under a "Run" heading.
- STR-003. Each job row says how it ended in words and an icon, not colour alone: running, passed,
  failed, cancelled, error.
- STR-004. A span's children (a job's checkout and the stages its output named) are folded under
  it behind a disclosure button that says how many there are ("4 stages"). A failed or errored
  span opens by default so its failing stage shows. What a person opened or closed stays across the
  dialog's refresh of a running run, and so does the Table toggle.
- STR-005. Every waterfall has a time axis above its bars: tick labels from the subject's start
  ("0", "2 min", "4 min", …) with faint gridlines, and for a running subject a "now" mark.
- STR-006. Every waterfall has a legend for the bar looks it uses: waiting (hatched), ran, still
  running, failed.
- STR-007. The Table view follows the same grouping: a "Waited" column for jobs, the "Job " prefix
  dropped under the Jobs heading, children indented; it keeps every span (folding is for the plot).
- STR-008. CD and task waterfalls keep their content; they gain the axis, the legend and folding
  (a deploy's stages fold under it).

## Plan

- [x] Read the product context and related work, then write and validate the spec
- [ ] A busy CI run in the mock reproduces the screenshot, and each job's slot wait and run share one row named by the job
- [ ] A job's own stages fold under it (a failed job opens), and the fold survives the dialog's refresh
- [ ] A time axis and a legend say when each bar happened and what its look means
- [ ] Independent review in a fresh session, findings fixed test-first
- [ ] Every gate green, docs and the changes log updated

## Acceptance criteria

- AC1 (STR-001, STR-002, STR-003): mock run `ci_s4` (running, eight jobs that waited 6 to 9 min
  for a slot, "prototypes" began waiting before "package (gateway)" but started after it) shows no
  row named "Waiting for a job slot"; every job's hatched and solid segments touch; "engine" says
  "waited 6 min" and "running"; the rows under "Jobs" are in waiting order. In `ci_s3`, a job
  requeued after its process host was lost shows each attempt with its own wait, and a wait with no
  job span keeps its own "Waiting for a job slot" row.
- AC2 (STR-004): in `ci_s4`, "engine"'s Checkout is hidden until its disclosure is opened; after
  the running dialog's refresh it is still open and the focused fold button still has focus. A
  running job whose stage failed ("package (deployer)") and the failed "test (2/2)" in `ci_s2` are
  open; the passed "quality" is folded.
- AC3 (STR-005, STR-006): every waterfall (CI, CD, task) shows axis ticks that start at "0", each
  label exact and different (`ci_s3`: "0, 30 s, 1 min, 1 min 30 s"); a running run shows "now";
  the legend names every bar look in use, folded or not, and hides with the plot in Table view.
- AC4 (STR-007, STR-008): the Table has a Waited column with "6 min" for "engine"; the CD and task
  waterfalls still show what SPEC-137's test checks; no WCAG violation (axe) with the plot showing
  and a fold open; no horizontal scroll at 390 px on the CI dialog and the task page.

## Test plan

- e2e `pipeline.e2e.ts`: new tests for AC1 to AC4 against a busy mock run `ci_s4` and the failed
  `ci_s2`, each run first against the current `stages.js` to see it fail; the existing waterfall
  test follows the new grouping (the "Job " prefix and folded children).
- Gates: `scripts/dev ci`, `scripts/dev security`, the whole e2e suite.

## Decisions

- **Pair a job with its wait in the page, not in the store.** The wait's key `wait:<id>@…` and the
  job's `job:<id>@…` share the job id, so the page pairs them. Alternatives: give the wait the job's
  `parent_key` (a child that starts before its parent, and only for new spans: stored runs would
  still read badly), or a new combined span (an API and migration change for a drawing concern).
  ADR-0136 records it.
- **Fold children by default, open failures.** Children are detail: a reader first wants which job,
  how long, how it ended. A failure is the one case where the detail is the point. Alternatives:
  hide zero-length stages (loses evidence) or a "compact" toggle (one more control to learn).
- **Axis ticks from the subject's start, not wall-clock times.** "4 min in" answers "where in the
  run"; the Table keeps the clock time of each start. Ticks have their own exact label ("1 min
  30 s", "2 h") rather than `duration()`, which rounds down and repeated "1 min" on a 30 s step.
- **Open on any failure within.** A running job whose stage already failed opens too, and the
  legend is computed from every span, so folding never hides a failure or an unexplained colour.
- **Jobs still waiting with no span stay out.** A job waiting for a slot has no span until it gets
  one (`ci::exec` records the wait when it ends); the dialog's job chips show them. Recording an
  open wait span is a backend change, filed as #536.
