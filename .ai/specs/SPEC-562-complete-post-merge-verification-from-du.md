---
yulius-task: f23d1af1-2f37-46dc-8c4c-e334d51e1c3f
status: in-progress
---

# SPEC-562 Post-merge verification evidence

## Goal

A task can cite the machine checks of its landed change and finish a verification-only step without editing a repository file to copy those results.

## Context

The requester reports documents-only follow-ups for Quartio-v2 #323, #324 and #327. The follow-ups for #322 and #325 contained source or tests. Yulius stores a task's current delivery stage and PR. CI records name the reference, SHA, run and job outcomes, including skips and tree reuse. Merge trains test a candidate and merge members through ordinary delivery. CD records name the environment, SHA, kind, commands and outcome. Runs and spans have retention limits. A release alone omits failed attempts and individual checks.

An idle task's completion uses its declared steps and spec status. Spec synchronization preserves the status of a matching unchecked step. A done source spec requires checked Plan items and Evidence. After a merge, new repository work reopens delivery. These contracts are described in SPEC-441, SPEC-491 and SPEC-493.

## Requirements

- **PMV-001** Bind the task and its landed commit to the integration branch and PR head observed at merge. A moving branch, a candidate or a related commit cannot supply the landed identity.
- **PMV-002** Provide a stable, access-controlled task delivery evidence link. Show actual CI attempts and job verdicts and CD attempts for the exact commit with environment, operation and verification results. Keep separate attempts identifiable across reruns, later merges, restarts and normal run retention. Missing proof stays missing.
- **PMV-003** Task guidance directs a verification-only step to cite the evidence in its step note and closing summary when the project accepts it. Pending, failed, skipped, manual and hardware checks retain their meaning. Explicit source-document obligations, changes to code/tests/changelog, review, deployment authority and task completion keep their usual flow.

## Plan

- [x] Trace the stored verification and completion contracts
- [x] Expose task-bound merge and CI evidence
- [x] Associate exact deployment attempts and check outcomes
- [x] Prove the verification-only handoff and document its limits
- [ ] Review and deliver the verified change

## Acceptance criteria

- **AC-1** A merge followed by movement of the branch has a task-specific link to its original commit. A green candidate, ancestor, descendant or unrelated branch cannot stand in for the integration run.
- **AC-2** Reruns retain their distinct ids and job outcomes. An exact deployment with both commands and a passing result shows what was verified. A different SHA, missing command or missing run cannot prove that deployment. Run retention preserves the compact results for linked merges.
- **AC-3** A representative task cites its link with step_update after its PR merges. Verification-only bookkeeping causes no change to its worktree and opens no follow-up. A project requiring committed evidence still needs that source work.

## Test plan

Exercise the delivery HTTP and MCP paths with a local Git/SQLite fixture. Retarget a PR at merge without changing its head, advance the integration branch, rerun CI while paging over archived results and sign in without access to the task. Cover missing proof, skipped jobs, a failed CD wrapper with misleading phase output and the existing follow-up decision. Run targeted Rust tests, repository checks and the independent review before delivery.

## Decisions

Associate by exact commit and recorded source identity. A deploy of a descendant can change or revert the task's behavior. Treat an environment result as historical evidence at its own SHA. Keep the project responsible for deciding which particular checks its step requires.

## Evidence

- The original CI and CD delivery regressions both failed on the pre-feature revision. All four delivery evidence regressions failed on the first reviewed revision. With the fixes, `cargo nextest run -p yulius -E 'test(/delivery::post_merge_evidence/)'` passed all four.
- The representative fixture merged the setup guide and its changelog, cited the exact integration run in the remaining step through MCP, and recorded the security job as skipped. The task finished both steps with one PR and an unchanged, clean worktree. A later failed scan kept its own run ID and verdict.
- The race fixtures advanced the integration branch, retargeted a PR at merge, emitted misleading CD phase markers and inserted a newer CI attempt between archive pages. The access fixture denied a signed-in nonreader and excluded another sandbox's run at the identical SHA.
- `cargo test -p yulius-domain --lib briefing::` passed 31 tests. `scripts/dev quick` passed the repository contracts and formatting check. Chromium E2E passed 1,167 tests, including the task delivery evidence link. The model-dependent test was skipped. `scripts/dev security` reported Vidoc disabled.
- The environment checks used synthetic run records and local Git. No live Quartio-v2 environment or hardware was tested. Quartio-v2's handoff is to designate the checks its task record may satisfy and retain its explicitly required source artifacts in the reviewed change.

- After isolating SQLite's array decoding on the first reviewed head, the retarget, misleading-phase and missing-page assertions each failed at their intended boundary. On the second head the task agent returned page one when asked for archived staging verification. The corrected MCP path selected and pinned the 51st attempt using the task's credential.
- Reconfiguring the integration branch after a retargeted merge reproduced false evidence on the third reviewed head. Recovery now matches the original PR, branch, head and merge result. The schema-76 fixture also changes the integration branch and reconstructs the original intent from the task's PR-opening audit entry.
- Final scoped nextest passed 40 tests. All 163 delivery, merge-train and review integration tests passed after merging current dev. Clippy passed for the six changed crates with all targets, all features and warnings denied. The Git and intake browser suites passed all 27 tests on the updated dev base.
- On head `e0c0c93c`, an external retarget followed by a delivery profile change returned HTTP 200 for the wrong landing. Both task delivery and the project Git page now match externally merged PRs against the base captured at opening or adoption. The regression also covers an external merge on the original base before the profile change.
- The recovery-fix mutation campaign reported 32 listed, 6 caught, 1 missed, 0 timed out, 25 unviable, 0 not reached. The missed inversion compared the observed PR number with the task's PR. A positive delayed-confirmation regression subsequently exercises that comparison, including an intervening profile change.
