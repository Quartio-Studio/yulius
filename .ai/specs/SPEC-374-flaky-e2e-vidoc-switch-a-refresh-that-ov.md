---
yulius-task: b55cf470-3f26-49a9-9cd4-d6db1df00294
status: in-progress
---

# SPEC-374: the vidoc-switch refresh test waits on the refresh, not on an exact read count

Status: in-progress
Date: 2026-11-01
Owner: Yulius task #357
Scope authorization: task #357, Fixes #1002 (found by task #347 in a full local e2e run).

## Goal

`e2e/tests/vidoc-switch.e2e.ts` › "a refresh that overtakes the read after a change still shows
the change" (SPEC-359 review round 5) passes however slowly the host runs its steps: after "Check
for commits" it waits for that refresh to have been handled, which no other read can make
impossible.

## Context

- Symptom (#1002): `expect.poll(...).toBe(true)` at the line after "Check for commits" timed out
  after 5000 ms in a full run on a loaded VM; the file alone passed.
- The poll waited for the card to be redrawn or for **exactly two** held Vidoc reads
  (`vidocHolds.length === 2`): the read after the change and the refresh's own.
- Reproduced (temporary probe, not committed): a 6 s delay before the click fails the test 3 of 3
  times, with and without `ambient=0`; held reads before the click: 3.
- Root cause, traced with a stack recorded at each CI read (temporary probe): two more reads start
  by themselves while the read after the change is held, ambient activity or not:
  - 1.5 s after a pulls read or a task event (`scheduleCountsRefresh`, `COUNTS_REFRESH_MS`; here
    about 1.5 s after the change) the store reloads the sandbox list, and `watchVidocSwitch`
    (`pages/cividoc.js`) sees the drawn view (from before the change, its fresh read being held)
    differ from the store, `pending` already cleared, and calls `rereadVidoc`;
  - ~3 s after the page drew its runs the CI page's own poll (`POLL_ACTIVE_MS`, studio has an
    active run) reloads, and the view still being stale, reads it anew.
  Every read is held and the loader keeps only the newest generation, so the card is not redrawn;
  once the steps between the change and the poll's first look take about 1.3 s, the count is past
  two for good and the condition can never hold. Ambient activity (triage's hypothesis) adds more
  reads, but `ambient=0` alone does not fix it.
- Why the existing run missed it: the steps normally take a few hundred milliseconds, inside the
  window before the first extra read.
- Product: not a defect. Each extra read belongs to a newer refresh, the newest one wins and draws
  the change once released (the test's last assertions). Skipping the watcher's reread while a
  read is in flight would be wrong: that read may have started before a change made elsewhere.

## Requirements

- REQ-1: After "Check for commits" the test waits until the refresh has started (the button is
  back, its look answered and the reload called) and then until the card was redrawn or a Vidoc
  read was asked after the click; no exact count of reads.
- REQ-2: A regression test clicks only after two more rereads (two sandbox-list resyncs) and
  fails with the exact-count wait.
- REQ-3: The neighbouring "refreshing the CI runs does not read the Vidoc switch again" waits for
  at least one read after its change, not exactly one: it failed once (`expected 3 to be 2`) in
  three runs of the file while this was reviewed, the same cause (a resync while the read after the
  change is on its way; probe: hold that read, `resyncNow()`, release, 3 reads every time).
- REQ-4: No product change.

## Plan

- [ ] Spec with the reproduced root cause
- [ ] Regression test that clicks after the page's own rereads, failing on the exact-count wait
- [ ] Wait on the refresh having started and a newer Vidoc read, passing the regression and probe runs
- [ ] Independent review answered and gates green
- [ ] Merged into dev and issue #1002 closed

## Acceptance criteria

- The failure is reproduced under load (probe delay) and by a committed deterministic test.
- Both cases pass with the new wait; the probe run passes too.
- The whole e2e suite passes.

## Test plan

- Commit the regression case first and show it fails on the old wait.
- Change the wait; run the file 3 times, the probe (temporary 6 s delay before the click, ambient
  on and off) 3 times, then the full e2e suite.

## Decisions

- Count asked reads (`__yuliusMock.asked`), counted synchronously when the page asks, instead of
  held ones, which land after the mock's 50-200 ms latency: once "Check for commits" is enabled
  again the refresh's read has been asked.
- Keep ambient activity on: the wait no longer depends on how many reads others start, and the
  test keeps covering them.
- Leave `watchVidocSwitch` as it is (see Context, Product).
