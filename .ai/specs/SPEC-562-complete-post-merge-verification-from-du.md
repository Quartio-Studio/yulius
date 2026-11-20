---
yulius-task: f23d1af1-2f37-46dc-8c4c-e334d51e1c3f
status: done
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
- [x] Review and deliver the verified change

## Acceptance criteria

- **AC-1** A merge followed by movement of the branch has a task-specific link to its original commit. A green candidate, ancestor, descendant or unrelated branch cannot stand in for the integration run.
- **AC-2** Reruns retain their distinct ids and job outcomes. An exact deployment with both commands and a passing result shows what was verified. A different SHA, missing command or missing run cannot prove that deployment. Run retention preserves the compact results for linked merges.
- **AC-3** A representative task cites its link with step_update after its PR merges. Verification-only bookkeeping causes no change to its worktree and opens no follow-up. A project requiring committed evidence still needs that source work.

## Test plan

Exercise the delivery HTTP and MCP paths with a local Git/SQLite fixture. Retarget a PR at merge without changing its head, advance the integration branch, rerun CI while paging over archived results and sign in without access to the task. Cover missing proof, skipped jobs, a failed CD wrapper with misleading phase output and the existing follow-up decision. Run targeted Rust tests, repository checks and the independent review before delivery.

## Decisions

Associate by exact commit and recorded source identity. A deploy of a descendant can change or revert the task's behavior. Treat an environment result as historical evidence at its own SHA. Keep the project responsible for deciding which particular checks its step requires.

## Evidence

- The setup-guide fixture in `crates/yulius/tests/delivery.rs` merged the required guide and changelog, then cited the exact integration CI run with `step_update`. Both task steps finished with one PR and a clean worktree. The security job remained skipped. A later failed scan kept its own run ID and verdict.
- The CI and CD regressions failed on the pre-feature revision. The race fixtures reject an ancestor, a later change, a train candidate and a different environment commit. They keep a failed CD wrapper unconfirmed even when project output claims its phases passed. Missing commands and runs remain visible. A signed-in nonreader was denied, and another sandbox's run at the identical SHA was excluded.
- The task agent on the second reviewed head could only read its first page. It now selects and pins the 51st archived attempt while another rerun is inserted. On the third reviewed head a profile change could reclassify a rejected direct merge. Recovery now matches the original PR, branch, head and merge result. A schema-76 fixture recovers the original branch from the PR-opening audit.
- On `e0c0c93c` an external retarget followed by a profile change returned HTTP 200 for the wrong landing. The fixture exercises task delivery and the Git page, for PRs opened by Yulius and adopted from GitHub. The legitimate original landing remains available and the retargeted landing returns 404. A separate failed assertion showed that the delivery note named the newly configured base. It now reports the closed PR's actual base.
- `scripts/dev mutants --base 14222b171` reported 32 listed, 6 caught, 1 missed, 0 timed out, 25 unviable, 0 not reached. Reintroducing the missed PR-number inversion made the positive delayed-GitHub-confirmation test lose the expected landed SHA.
- `scripts/dev mutants --base e0c0c93c --tests 'test(/post_merge_evidence/)'` passed its unmodified baseline of 3,213 tests and reported 82 listed, 14 caught, 2 missed, 0 timed out, 66 unviable, 0 not reached. Both survivors weakened the Git page's PR-number check. Reintroducing each in isolation made the new wrong-PR fixture fail. Inverting the equality also returned 404 for the legitimate landing before the task page could recover it.
- The delivery, merge-train and review suites passed all 196 tests at four-way concurrency. In a concurrent run one unchanged approval test hit its 120-second timeout. It passed in 24 seconds with four workers. The independent reviewer approved `9a4c15a0` after running six evidence integration tests and two landing and migration unit tests. The follow-up changed one product line and extended the tests.
- Final scoped nextest passed 40 tests, including all seven task evidence integration tests and the migration and identity checks.
- Clippy passed with all targets, all features and warnings denied for the six affected crates. `scripts/dev quick` passed the repository contracts and formatting check. Chromium E2E passed 1,167 tests. One model-dependent test was skipped. The Git and intake browser suites passed all 27 tests on the updated dev base. `scripts/dev security` reported Vidoc disabled.
- The environment checks used synthetic run records and local Git. No live Quartio-v2 environment or hardware was tested. Quartio-v2's handoff is to designate the checks its task record may satisfy and retain its explicitly required source artifacts in the reviewed change.
