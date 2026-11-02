# 0136. A CI job's slot wait is drawn on the job's row; a waterfall folds children and has an axis

## Status

Accepted. Specified in [SPEC-165](../../.ai/specs/SPEC-165-for-very-complex-tests-it-s-not-readable.md)
(task #155). Refines how [0112](0112-stages-are-spans-written-with-the-state-they-measure.md)'s
spans are drawn (SPEC-137 STG-403); the spans and the API do not change.
Amended by [SPEC-385](../../.ai/specs/SPEC-385-ci-a-job-still-waiting-for-a-job-slot-sh.md)
(task #367, #536): `ci::exec` opens a job's slot wait when it begins, and a wait no job span follows
(a job still waiting, or cancelled while it waited) is that job's row under "Jobs".

## Context

A busy CI run's waterfall listed every job's wait for a job slot as its own root row, each named
"Waiting for a job slot", far above the job it delayed, with every job's checkout as a second row
and no time axis or legend. A person could not tell which job waited, when it ran or what the bar
looks meant. The data already links them: `ci::exec` records the wait as `wait:<job id>@<ms>` and
the store's trigger opens the job as `job:<job id>@<ms>`.

## Decision

- The page pairs a `job slot` wait with the job span carrying the same job id (the last wait that
  began before the job started) and draws both on the job's row: a hatched part, then the run. The
  duration column says how long it ran and how long it waited; the table has a Waited column. A
  wait under a second (a slot free at once, which is still recorded) shows nothing.
- A CI run's waterfall has two groups: "Run" (its own spans) and "Jobs" (job spans, named by the
  job, in the order they began waiting, each with its state as an icon and a word).
- A skipped job (ADR-0133) is drawn outlined, neither ran nor failed, with "Skipped" in the
  legend.
- Every span with children folds them behind a disclosure; one that failed, or has a failure
  under it, opens by default. What a person opens or closes, and the Table toggle, are kept per host across the
  redraws of a running subject.
- Every waterfall has a time axis (ticks from the subject's start, at most six intervals, each
  labelled exactly, faint gridlines, "now" while it runs) and a legend of every bar look its spans
  use, folded or not.

## Alternatives

- Give the wait span the job's `parent_key`: a child that starts before its parent, and only new
  spans would read well; stored runs would keep the old rows.
- One combined "job with wait" span: an API and migration change for a drawing concern.
- Hide zero-length stages instead of folding: loses evidence a person may need.
- A compact/expanded switch for the whole waterfall: one more control to learn; folding per span
  keeps the overview and the detail one click apart.

## Consequences

- The waterfall depends on the `wait:<id>@…` / `job:<id>@…` key shapes; a wait that does not pair
  (another key shape, a missing job span) is still drawn on its own row, so a change there degrades
  to the old reading rather than losing data. Since SPEC-385 such a wait that names its job is that
  job's row under "Jobs"; one without a job keeps its "Waiting for a job slot" row.
- (Superseded by SPEC-385.) Jobs still waiting for a slot had no span yet (the wait was recorded
  when it ended), so they did not show in the waterfall until they started; their wait is now an
  open span from its start.
