---
yulius-task: e0b4ff5b-4c92-4a3f-b6ad-06b153e65e77
status: in-progress
---

# SPEC-653: Staging: the full gate judges the newest waiting revision first and skips revisions a green one contains

## Goal

Under merge-first staging, the full gate's verdict on the newest requested revision, and so a
green staging, comes as soon as one full run allows, with no more CI than today. A waiting request
under a newer green revision needs no run of its own. Among several waiting requests the newest is
judged first. Older ones get a run only to name the merge that brought a new failure. A failing
verdict still wakes only the breaking task and names one repair owner.

## Context

`staging_judge` (`crates/yulius-app/src/ci/staging.rs`, SPEC-575 MFS-003, ADR-0411) sorts the
`testing` requests by first-parent position, oldest first. For each one in turn it reads the full
gate's verdict on exactly its revision, or queues one candidate run while no candidate or
integration-branch run of the sandbox is active. Every request therefore costs a full run, oldest
first, even when a newer revision already passed.

Measured in quartio-v2 on 2026-11-25, 04:16 to 07:37 UTC, where a full run takes 15 minutes:

- Nine full-gate runs went back to back, and a revision's verdict came 45 to 89 minutes after its
  merge.
- The runs at 05:35 and 05:51 judged `0d26f0ad9` and `3346ca6a5`, both older than `083a1d1c8`,
  whose full gate (the integration branch's run) had passed at 05:34. Both failed on a test that
  `083a1d1c8` no longer failed, and task #402 was told its gate failed although a green revision
  already contained its change.
- The runs at 06:22, 06:37 and 06:52 each found again, on the next revision, the failure the
  06:07 run had reported on `867dfe486` (at 06:21). The repair's revision `5d1294696`, asked at
  06:36, got its run at 07:07, with its verdict at 07:21.
- Over four days, 17 of 151 merges broke the gate: the failing verdict came a median of 18 minutes
  after the merge (third quartile 45, worst 89), and the gate was green again after a median of 97
  minutes. In the yulius project (8-minute runs, no queue) the figures are 8 and 43 minutes.

What attribution already does (SPEC-612, `crates/yulius-app/src/ci/gate_history.rs`): the gate
history replays the integration branch's settled verdicts since its last full pass in first-parent
order. Each failing test's origin is the first judged revision that reported it, with that
revision's merge owner. While an earlier revision's run is still active, the replay stops there
(`waiting`), so no owner is named before the earlier verdict is in. Unrequested merges never get a
run of their own: today a failure is already attributed to the first *judged* revision showing it.

Related: SPEC-575 (merge-first staging), SPEC-612 (repair owner), SPEC-650 (a person's and the
schedule's requests), ADR-0410, ADR-0411, ADR-0477.

## Requirements

- **NWF-001 A green newer revision covers a request.** A `testing` request whose revision is on the
  first-parent history below (or at) a revision whose full gate passed without skipping a job
  needs no run of its own. When that revision is not its own, the request becomes `inherited`
  with a note naming the green revision and its run. `pick` then includes it once staging
  verifies a revision containing it, and it holds no newer ready request back. Its own failing
  verdict, if one exists, is not reported to its task.
  - When no request at or above the newest green revision is testing, ready or deploying,
    Yulius asks for that green revision itself (`stg::serves_green`, as the schedule's request),
    so a covered request never waits for an unrelated later request. A green revision whose
    request was already tried (deploy failed, cancelled) is not asked for again, as for the
    schedule.
  - The CD page and `cd_status` say "Tests passed on a newer revision containing `<sha>`: it
    deploys with that revision" instead of a failure; the state's label reads "waiting for a
    verified revision".
- **NWF-002 The newest is judged first.** When several `testing` requests wait for a run, the
  candidate run goes to the newest one above every revision the full gate judged or is judging.
- **NWF-003 Under a red revision.** A `testing` request without a verdict of its own, whose
  nearest judged revision above it failed:
  - needs no run when that failure brings no failure that the judged revision below it did not
    already have: it becomes `inherited`, naming the red revision and its run;
  - is a narrowing point when the failure brings a new failure (a test, an unidentified job, or a
    run-level failure not seen on the judged revision below).
- **NWF-004 Narrowing a range.** While a red revision's range (from the judged revision below it,
  exclusive, to it) holds narrowing points:
  - the next candidate run goes to the middle one (by position), before any newer request, and
    then again in the half the verdict leaves, until no waiting request is left in the range of
    each new failure: at most `ceil(log2(k + 1))` runs for `k` waiting requests, never more than
    one run per request (as today);
  - the red revision's own request stays `testing`, and the gate history stops its replay before
    that revision, so no task is told it broke the gate and no repair owner is named until the
    range is narrowed;
  - a range that holds only unrequested merges is not narrowed: as today, the failure is the
    first judged revision's, and that merge's owner owns the repair.
- **NWF-005 One full-gate run at a time per project.** A candidate run is still queued only while
  no candidate or integration-branch run of the sandbox is active. A running candidate is never
  cancelled for a newer request.
- **NWF-006 What is kept.** A request whose own candidate a person cancelled fails its gate,
  whatever the plan says. A request whose own verdict settled and that no green newer revision
  covers is decided as today (ready, gate failed with its new failures, or inherited with the
  failures' origins; SPEC-612, SPEC-650 PST-002). A cancelled candidate still fails its request's
  gate. Without a CI gate in force, a request still fails its gate instead of holding newer ones.
- **NWF-007 Bounds.** The queue reads the first-parent revisions from the verified head down to
  the lowest waiting request, then 128 more at a time until one below that request has a settled
  verdict (the floor a red revision's failures are compared with), at most 512 per round, and at
  most `MAX_PENDING` requests. A request below that window is still decided by
  its own verdict and by the judged revisions in the window above it.

## Plan

- [x] Spec SPEC-653 and the plan
- [x] Domain: the judging plan (covered, inherited, newest first, narrowing) with unit tests
- [x] Domain: the morning of 2026-11-25 replayed at its real times
- [x] App: staging_judge reads the first-parent window and applies the plan, with integration tests
- [x] Gate history waits for a narrowing run before naming a repair owner
- [x] SPEC-575, ADR-0480, architecture and changes updated
- [x] Review round 1: findings fixed test-first
- [ ] Independent review of the change approved

## Acceptance criteria

- A `testing` request under a newer revision whose full gate passed gets no candidate run and is
  included when staging verifies a revision containing it.
- With several `testing` requests and no run, the only candidate run is for the newest.
- With the requests of 2026-11-25 replayed at their real times (15-minute runs, one at a time): no
  run is spent on `0d26f0ad9` or `3346ca6a5`, the new failure at `867dfe486` is reported by 05:52
  (instead of 06:21) and the one at `5d1294696` by 06:51 (instead of 07:21), with no more runs
  than that morning had.
- A red newest revision whose range holds earlier waiting requests gets its failure attributed
  to the merge that brought it after narrowing runs, and only that task's request fails its gate.

## Test plan

- **Domain unit tests** (`yulius_domain::staging_gate`, `yulius_domain::staging::serves_green`; `GateVerdict::gate` and `unresolved_ranges` in `yulius_app::ci::gate_history`): a request below a green revision is covered;
  the newest of several is judged first; a request under a red revision with only inherited
  failures needs no run; a new failure narrows its range by bisection, oldest range first, before
  newer requests; an active run anywhere above holds a request; the 2026-11-25 replay, and the
  same replay under the old order reproducing that morning's runs (`0d26f0ad9`, `3346ca6a5`,
  `867dfe486`, ... `5d1294696`).
- **Integration tests** (`crates/yulius/tests/staging.rs`, real Git adapter, store and queue): a
  request below a green newer revision gets no candidate run and is included once that revision
  deploys; the newest of three requests gets the only candidate; a red newest revision with a
  new failure gets a narrowing run on the middle request, its own request stays `testing` and no
  repair owner is named until that run ends, then only the breaking task's request fails its gate.
- The whole `staging` and gate history test files, run in full.

## Decisions

- **Inherited for a covered request**, not `ready` or a new state. A `ready` request could be
  deployed at its own, untested revision if the green request above it were superseded. `inherited`
  already means "a verified descendant serves it": `pick` never deploys it and includes it with
  the next verified deploy. A new state would need a table rebuild and touch every view
  (as SPEC-650 decided).
- **Bisection over requested revisions with full runs**, not runs of only the newly failing tests.
  Yulius runs each project's own `[[ci.jobs]]` commands. It has no generic way to run one named
  test of an arbitrary project's suite, and a test filter in every project's jobs would be a new
  contract for every project. Bisection over requests costs at most `ceil(log2(k + 1))` full runs
  for `k` waiting requests in the range, never more than today's one per request, and only when a
  new failure leaves a range. That morning it needed none: every red range held one requested
  merge.
- **Narrowing before a newer request.** A newer revision of a red branch is red too until its
  repair lands, and the repair cannot start before its owner is told. So naming the breaker first
  shortens the time to green more than judging a newer red revision. Narrowing is bounded by the
  logarithm above, so newer requests wait at most that long.
- **One full-gate run at a time**, as today. Side by side, two 15-minute suites share the
  sandbox's CPU budget, so each takes longer (and the suites' load-sensitive tests fail more
  often, as seen in the yulius project in 2026-11). With the newest judged first, a second lane
  would mostly judge older revisions whose answer the newest one gives anyway. The minutes saved
  come from runs not spent, not from running more at once.
- **The range bound is the judged revisions.** A failure is attributed to the first judged
  revision showing it; unrequested merges in between are not narrowed. That is what attribution
  did before (they never had runs), so the queue tells tasks no less than today.
- ADR: [0480](../../docs/decisions/0480-the-full-gate-judges-the-newest-waiting-revision-first.md)
  (supersedes ADR-0411's "the full gate on exactly that commit" ordering).
