---
yulius-task: 00b9c0be-4d9a-4556-9bed-c4d81b3e11e6
status: done
---

# SPEC-585 Keep a completed delivery out of Needs you

## Goal

A person sees their finished, merged task under Done. A refresh that cannot check for a follow-up must not ask them to resume the task or rerun CI on its merged PR. Fixes #1589.

## Context

The screenshot shows parked Quartio-v2 task #335, PR #1491, 11 completed steps and an achieved goal. The server's parked status read maps every timeout and every failed lookup to an errored PR, including branch discovery after a recorded merge. That classification also creates a wait-history entry. The screenshot does not identify the upstream error. The integration fixture models unavailable and slow branch discovery.

## Requirements

- **PMR-001** Display the durable merge when the local task is settled and discovery of a possible replacement fails or exceeds its budget. Task detail, sandbox lists, counts and agent tools must agree and clear a previous false wait.
- **PMR-002** Known or newly discovered open PRs, reopened work and owner decisions retain their existing verification and attention rules. Local evidence must still be readable and current.
- **PMR-003** Keep reads bounded. An inconclusive display lookup cannot release or omit a dependency from a start rule.

## Plan

- [x] Trace the parked pull request warning
- [x] Reproduce the incorrect task status in a regression test
- [x] Restore the correct status across parked task reads
- [x] Verify and independently review the fix
- [x] Merge the reviewed fix into dev
- [x] Resolve browser synchronization failures from the full CI run

## Acceptance criteria

- **AC-1** With a recorded merge, no unfinished work and an unavailable branch listing, detail shows the merged PR with no active notice. The task appears under Done in the lists and contributes zero to the Home attention count.
- **AC-2** A known or freshly discovered replacement with incomplete checks remains under Needs you. A newly opened plan step or owner request supplies its own notice.
- **AC-3** A stalled discovery returns within the existing list budget. Rule construction and dependency evaluation keep the task as a prerequisite while follow-up evidence is unavailable.

## Test plan

Exercise the real task status and wait-history paths against the local store and synthetic GitHub responses. Hold discovery on a condition to cover the deadline. Run the parked task and dependency tests, formatting and repository checks, mutation testing, an independent review and Yulius CI.

## Decisions

Use the persisted delivery stage as evidence for the merged PR's display status. Keep discovery of follow-ups and the separate dependency checks. A previously unseen external follow-up becomes visible when discovery succeeds again.

## Evidence

Both regression tests failed on the original implementation. Task detail returned the exact notice in the screenshot. MCP returned `shows: needs_you` with `parked_pr: {number: 1491, open: false}` after the stalled lookup. The existing cold-cache merged-PR test passed on that same implementation.

The two screenshot regressions passed with the display fix. A scoped cache test then exposed an unrelated task being treated as already read. Branch membership now applies only to the task whose branch was listed. The two Chromium suites for parked tasks and Needs you messages passed all 118 cases. The project has disabled Vidoc.

Review on `496faa34` found that a stale list could resurrect a directly confirmed closure, or replace a verified failing PR with a higher-numbered green PR. Both consecutive-read tests failed on that head. The display now publishes the result after direct verification, excludes confirmed closures, and keeps the PR responsible for the attention notice.

The busy-sandbox test reproduced a capacity error after 30 parked branch reads behind a slow overview. The overview and individual branches now have independent overlap limits. All five overview-cache tests passed. The scoped parking and dependency suite passed all 175 tests with the follow-up identity fix.

Review on `41198e18` found two remaining false waits. Holding the final branch list after a directly confirmed merge and reading a settled task for the 31st time behind a held overview each reproduced the original notice. The display now uses the same local recheck for both cases. Directly confirmed merges are published before consulting concurrent observations. The API reference documents the prerequisite exception.

Both final regressions passed. The independent reviewer approved `b157074693274fcaca4f43ae3786c7298e235872` and reported five passing regressions. The broader parking and dependency suite passed 177 tests before integrating the new Codex default. The integration rerun exposed a missing account in two synthetic dependency tests. Those fixtures explicitly select their configured Claude harness, and a failed status read now cancels its held test overview. `scripts/dev quick` and scoped Clippy passed. Vidoc reported disabled.

With the configured test harness selected, all 177 scoped tests passed on the integrated code with one nextest worker. Five pre-existing timing tests exceeded their fixtures’ deadlines when the parallel run shared CPUs with mutation builds ([issue #1591](https://github.com/Quartio-Studio/yulius/issues/1591)).

Changed-line mutation testing exposed missing coverage of an old verified PR identity absent from the final open list. Applying the generated membership mutation made the added test return PR 4 instead of PR 3. Restoring the reviewed condition made it pass alongside all five overview tests.

The full changed-line mutation campaign ran on the immutable `41198e18` checkout. The review repair ran separately against that head. Both unmutated baselines passed.

```text
mutants: 96 listed, 28 caught, 2 missed, 0 timed out, 66 unviable, 0 not reached (bound 30 min, took 1136 s)
mutants: 19 listed, 8 caught, 0 missed, 0 timed out, 11 unviable, 0 not reached (bound 30 min, took 897 s)
```

Both surviving mutations were reapplied. The added assertions failed with each generated change: the cache returned an absent PR identity, or hid a completed branch result behind an earlier overview. One repair mutant was initially caught by an unrelated watcher timeout. A dedicated regression now detects that mutant mistakenly treating an unmerged closure as a verified merge during a stalled final list.

On the restored product code, the final run passed 178 parking and dependency tests, six cache and publication tests, and scoped Clippy with warnings denied. The task delivery record supplies the green PR head, squash commit and verification link for the final plan item.

Yulius CI run `9936922b-a607-485f-b6db-f0b3e2ece268` exposed two test races. The browser handoff fixture now waits for the matching action-cleared event before asserting both persisted records are empty. A deterministic unrelated browser read reproduced the restart fixture consuming its gate before publication. It now holds the reader that reached the PR publication check. All four affected tests and scoped Clippy passed.
