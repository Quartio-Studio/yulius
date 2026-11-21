---
yulius-task: 00b9c0be-4d9a-4556-9bed-c4d81b3e11e6
status: in-progress
---

# SPEC-585 Keep a completed delivery out of Needs you

## Goal

A person sees their finished, merged task under Done. A refresh that cannot check for a follow-up must not ask them to resume the task or rerun CI on its merged PR.

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
- [ ] Verify and independently review the fix
- [ ] Merge the reviewed fix into dev

## Acceptance criteria

- **AC-1** With a recorded merge, no unfinished work and an unavailable branch listing, detail shows the merged PR with no active notice. The task appears under Done in the lists and contributes zero to the Home attention count.
- **AC-2** A known or freshly discovered replacement with incomplete checks remains under Needs you. A newly opened plan step or owner request supplies its own notice.
- **AC-3** A stalled discovery returns within the existing list budget. Rule construction and dependency evaluation keep the task as a prerequisite while follow-up evidence is unavailable.

## Test plan

Exercise the real task status and wait-history paths against the local store and synthetic GitHub responses. Hold discovery on a condition to cover the deadline. Run the parked task and dependency tests, formatting and repository checks, mutation testing, an independent review and Yulius CI.

## Decisions

Use the persisted delivery stage as evidence for the merged PR's display status. Keep discovery of follow-ups and the separate dependency checks. A previously unseen external follow-up becomes visible when discovery succeeds again.

## Evidence

Both regression tests failed on the original implementation. Task detail returned the exact notice in the screenshot. MCP returned `shows: needs_you` with `parked_pr: {number: 1491, open: false}` after the stalled lookup. The existing cold-cache merged-PR test passed on that same implementation. Fixes #1589.

The two screenshot regressions passed with the display fix. A scoped cache test then exposed an unrelated task being treated as already read. Branch membership now applies only to the task whose branch was listed. The two Chromium suites for parked tasks and Needs you messages passed all 118 cases. The project has disabled Vidoc.

Review on `496faa34` found that a stale list could resurrect a directly confirmed closure, or replace a verified failing PR with a higher-numbered green PR. Both consecutive-read tests failed on that head. The display now publishes the result after direct verification, excludes confirmed closures, and keeps the PR responsible for the attention notice.

The busy-sandbox test reproduced a capacity error after 30 parked branch reads behind a slow overview. The overview and individual branches now have independent overlap limits. All five overview-cache tests passed. The scoped parking and dependency suite passed all 175 tests with the follow-up identity fix.
