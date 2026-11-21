---
yulius-task: 390479b9-2747-4388-b326-779981ff5f47
status: in-progress
---

# SPEC-605: Record the review rounds of Codex tasks

## Goal

A person can see a Codex task's independent review rounds and their elapsed times in task
delivery. The Dashboard includes their outcomes and answering models in its review counts.

## Context

Yulius #589 reports that the instance's review rows were all from Claude Code. The shared
`review_rounds` table, delivery view and review analytics already consume those rows.
The Codex launch currently registers turn hooks and a shell hook. The pinned Codex CLI
also supports subagent lifecycle hooks. An offline probe on 0.170.0 showed a collaboration
spawn's `SubagentStart` reporting type `default`, while the child rollout header named the
spawned task, its parent thread, the child's ID and Git commit. The sampled managed rollouts
also have this header. The existing briefing chooses a model and a fresh context but gives
no observable role to distinguish a reviewer from other Codex subagents.

## Requirements

- **CDR-001** The Codex briefing names a reserved `yulius_review_<n>` task name for each fresh
  independent reviewer and specifies the two report verdicts. Yulius accepts a start only
  for a direct child with this name whose rollout ID and parent match the authenticated
  hook and task session.
- **CDR-002** Record the review number, launch time, Git head when available and the
  first model observed for the round. Duplicate starts for a child have no effect. A
  reviewer cut off before giving a verdict can finish the same round on resumption.
  Starting another reviewer before a report reuses the unfinished round number.
- **CDR-003** The child's completion hook records the end time, the hook's model and a
  verdict from the report footer. A verdict that arrives first can establish the same
  round from its child rollout. Unknown children, stale or duplicate completion and reports
  without a recognized verdict leave the record as it was. Missing or invalid rollout
  evidence cannot block the reviewer.
- **CDR-004** Reuse the delivery view, merge transaction and analytics for both harnesses.
  Delivery exposes each round's timestamps and PR number when known. It shows the current
  review or the just-delivered PR's rounds, regardless of the task's harness. The merge links
  the rows to the PR and resets numbering for the next PR. It also
  records a durable boundary so a delayed start from the prior PR cannot appear on the next one.
- **CDR-005** The parking coverage in the combined delivery uses idle sessions with a host
  attached, as required by SPEC-558. The tests retain their checks for unfinished plans, PR
  state and changes during verification.

## Plan

- [x] Specify how Codex review rounds are identified
- [x] Record Codex reviewer starts with the reviewed head
- [x] Complete Codex review rounds with the reviewer verdict
- [x] Verify review history through delivery and dashboard data
- [x] Preserve review evidence across delayed hooks and successive pull requests
- [x] Approve the change for delivery
- [x] Preserve Claude Code delivery behavior after a merge
- [x] Verify Codex and Claude Code delivery together
- [x] Approve the staging fix for delivery
- [ ] Restore the shared view of delivered review rounds
- [ ] Exercise parking decisions with attached idle runs
- [ ] Verify and approve the combined staging fix

## Acceptance criteria

- **AC-1 (CDR-001, CDR-002)** A direct Codex review child starts a round with its head,
  model and timestamp. The same hook twice writes one row. A worker, a nested child
  or a rollout for another parent writes none.
- **AC-2 (CDR-002, CDR-003)** A partial response leaves the round open. A later report
  ends it with the actual model, timestamp and verdict once. A new reviewer records
  the next round after the prior verdict. A verdict can arrive before its start hook.
- **AC-3 (CDR-004)** Delivery shows the current PR's rows and timestamps. After a Claude Code
  or Codex merge, the rows include the landed PR number and the reported verdict. Dashboard
  analytics count its rounds and reviewer models. The next PR starts at round one, even if a
  pre-merge hook is delayed and the merge had zero review rows.
- **AC-4 (CDR-005)** The tests first verify the live run stays when the plan or PR is
  unfinished, then verify the same run can be parked when the blocker clears.

## Test plan

Use an offline Responses API stub with pinned Codex to verify the hook envelope and
child header. Use synthetic CLI-shaped JSON and real SQLite in a task integration
test that exercises hooks, duplicate or invalid evidence, delivery and analytics.
Run the existing Claude review tests, including `delivery::agents_merge_only_through_gated_tools`,
and the Codex launch and briefing tests. The fix forward runs the full `yulius::main`
binary and the application, store and contract test binaries.
Run the existing agent hook and parking integration tests on the combined revision, including
the tests that kept their process only as a stored live attempt before #1662.

## Decisions

Use the native lifecycle hooks for model, final report and report time. Read only
the bounded first line of the sandbox's child rollout to tell a named independent
reviewer from other subagents and capture its launch time and head. The hook's default type is
shared with other agents. The parent spawn's requested model and tool messages are
insufficient to identify the child and observe the actual answer. The record is
prospective because old spawns did not follow the reserved naming convention.
Antigravity's swarm uses a separate lifecycle without equivalent configured
subagent hooks. Its instrumentation is outside this slice.
The merged delivery contract from #1662 expects the completed Claude Code round to remain
visible. Use the same landed history for both harnesses, as in #1648. The current review and
the landed PR continue to use the existing PR boundaries. Parking tests use the existing
session host port and the app's restart attachment path to model the live run they exercise.

## Evidence

- Pinned Codex CLI 0.170.0, with collaboration enabled, ran against a local Responses
  API stub with synthetic credentials and reports. The start hook reported the
  child ID and Astra model with the default role. The child metadata already contained
  the reserved task name, parent, depth and launch commit. The stop hook supplied the
  same ID, model and the synthetic approval.
- `scripts/cargo test -p yulius --test main agent_hooks::codex_review_` failed on the
  original implementation. All three tests observed zero review rows where they
  expected one or two. The rebuilt integration test binary passed all three,
  including delivery reads before and after merge and the dashboard analytics.
- `E2E_TELEMETRY_DISABLED=1 npx e2e run tests/pipeline.e2e.ts` passed all 18
  Pipeline browser tests, including the review numbers and the report-read failure path.
- The focused Rust run passed 39 Codex hook and review integration tests. The launch,
  briefing and delivery-status rendering checks each passed. `scripts/dev quick` passed
  repository contracts and formatting. `spec_check` accepted the spec.
- The broader integration run passed 165 tests and failed two parked-task MCP assertions.
  Both failures reproduced on the unchanged `dev` build at `47e0d70e2`.
  [Issue #1644](https://github.com/Quartio-Studio/yulius/issues/1644)
- Against the first reviewed implementation, the new tests detected the wrong start time
  when the verdict arrived first, a stale round after a merge with zero
  rows, and a current delivery view hidden by an unreadable row from a previous PR.
- SQLite's query plan uses the landing index to locate the current or selected merged
  rounds by task, PR and merge time.
- After the follow-up, `scripts/cargo test` passed 41 focused hook and review integration
  tests, 23 app tests, 14 domain tests, the Codex launch test and four store tests
  including the migration-order check. `scripts/dev quick` passed.
- The second independent review approved `e93c0cdb7` and independently passed the five
  new Codex cases and 35 shared review cases. The API example now includes the Codex
  reviewer and the round's timestamps and pull request number, answering its remaining
  documentation finding. The project disables Vidoc, so `scripts/dev security` sent
  nothing for scanning.
- Staging CI run `921a833e-6b81-4048-a772-f1c75fb3b9c2` at `d188c8567` failed
  `delivery::agents_merge_only_through_gated_tools` in test shard 2. The existing
  test expected no Claude Code rounds after the merge and got one. Running the same
  test locally against the first delivery reproduced the failure. [Delivery evidence](https://yulius.quartio.ai/api/tasks/390479b9-2747-4388-b326-779981ff5f47/delivery/evidence/d188c8567c521fa800102706083b48bff2ce31e5)
- On the fix-forward branch, `E2E_TELEMETRY_DISABLED=1 npx e2e run tests/pipeline.e2e.ts`
  passed all 18 Pipeline browser tests.
- The previously failing gated-delivery test passed unchanged on the fix-forward branch.
  The five Codex review cases passed, including landed history after a merge. The focused
  Rust run passed all 41 cases. Independent review approved `ddf2a8809` after rerunning
  the gated-delivery test and all five Codex review cases.
- `scripts/cargo nextest run --locked --all-features --profile ci` with the full
  `yulius::main` binary and the app, store and contract packages passed all 3,423
  scheduled tests across five binaries. Nextest reported 24 skipped tests. The selector
  was `binary_id(=yulius::main) | package(=yulius-app) | package(=yulius-store) | package(=yulius-contracts)`.
- The second staging run, `78469d0d-d0df-4caf-8980-6f59d9d5b676` at `d643f0262`,
  exposed the updated gated-delivery assertion from #1662 and 14 parking cases whose
  stored attempts lacked attached hosts. The gated-delivery assertion expected one
  completed Claude Code round and observed zero. The parking cases expected one park
  after clearing their blockers and observed zero. [Delivery evidence](https://yulius.quartio.ai/api/tasks/390479b9-2747-4388-b326-779981ff5f47/delivery/evidence/d643f02626ac4bfad5376cd532a0a912011c120c)
- All 18 Pipeline Chromium tests passed again on the combined fix.
