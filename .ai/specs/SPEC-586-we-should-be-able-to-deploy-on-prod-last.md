---
yulius-task: b96672d5-d14f-4597-9c3b-e3e05d772e5f
status: done
---

# SPEC-586 Deploy the last verified dev revision to production

## Goal

An administrator can deploy the last verified integration revision to production while a newer dev revision is being verified.

## Context

SPEC-575 records what the shared staging environment verified. Production CD currently requires that revision to equal the moving integration head. In a project without shared staging, the integration branch's checks or a dev deployment and rollback drill may still be running when a person asks to release.

## Requirements

- **LVD-001** A promoting production deployment selects the revision successfully verified on merge-first staging. For ordinary deployment or branch CI with a newer verification in progress, select the preceding verified revision from the corresponding dev environment or integration branch.
- **LVD-002** Display the selected revision for confirmation and use its table, required checks, rollback proof and tree. Refuse a stale confirmation, missing evidence or a candidate outside protected integration history before releasing it.
- **LVD-003** Both fast-forward and merge-commit deployment promote the selected revision according to the delivery profile. Only deliveries contained in it participate. Repeated attempts reuse an exact-revision promotion. An in-flight verification of newer work continues independently.

## Plan

- [x] Read the product context and related work, then write and validate the spec
- [x] Select and show the verified dev revision during a newer verification
- [x] Promote exactly the selected revision and its included deliveries
- [x] Independent review in a fresh session, findings fixed test-first
- [x] Every gate green, docs and the changes log updated

## Acceptance criteria

- **AC-1** Given verified A and newer B still checking, the production overview offers A. Confirming B is refused. Confirming A deploys A's tree. B stays exclusively on dev until it qualifies.
- **AC-2** The merge-first, ordinary dev verification and branch-CI paths use their own successful evidence. A pending first verification supplies no release. A matching rollback proof is required for a production target that deploys scripts.
- **AC-3** The ordinary head-deploy path still works when head verification is complete. A changed promotion PR head, unrelated or newer production tree, or a newly eligible dev revision cannot silently change the confirmed release.

## Test plan

Use the real-Git CD HTTP fixtures with controlled CI, staging and CD records, the administrator's confirmation, production tree and the recorded run. Run the relevant CD, staging and promotion tests, repository quick checks, mutation tests and Yulius CI.

## Decisions

A person's production deployment carries an exact source revision. The staging queue and the profile's automatic promotion retain their separate triggers. Each new deploy request reselects the candidate. A retry can reuse its release PR while the candidate is still eligible. If eligibility has moved on, the person must review the new SHA. See [ADR-0425](../../docs/decisions/0425-pin-a-production-deploy-to-verified-dev.md).

## Evidence

- On the original implementation, the staging regression displayed the newer unverified SHA. The rollback-drill case returned HTTP 409 for the previously proven SHA. The branch CI fixture with a fast-forward profile and optional checks promoted its still-running first revision.
- With pinned promotion, the real-Git staging fixture released A, left B's delivery on integration, refused a mismatched release ref, and subsequently released B after it passed. The ordinary dev deployment and rollback drill fixtures both released A's tree while B was running.
- The GitHub client test confirmed the release-ref POST carries the selected SHA and propagates a 422 refusal. Repository contracts and formatting passed with `scripts/dev quick`.
- Four regressions failed at the first review head, `d29ca70ee`. Production selected an untested push while its predecessor was checking, rejected the newest successful multipart deploy, rejected a proven commit while another commit with the same tree was drilling, and marked a task released after a simultaneous production write changed the merge tree.
- At `ff2f7409c`, all 67 `cd`, `cd_rollback` and `staging` integration tests passed, including those four regressions. `scripts/dev security` reported that Vidoc was disabled for this project.
- At the same head, 107 delivery tests passed. Three additional regressions reproduced a release missing its own staging proof, a task left on dev after a successful retry and an old green rerun displacing a newer verified commit.
- On `c98d45fba`, the 175 CD, rollback, staging and delivery integration tests passed. The third review then reproduced three further cases on that commit. Retiring staging hid its last verified release, a retry after a landed merge accepted failing required checks, and a direct deploy could use another commit’s proof while its own drill was running.
- All 177 tests in those four modules passed with the fixes at `fc6f2da70`. The release fixture also covered a separate open promotion PR, simultaneous creation of the same release ref, and recovery after a failed check. Clippy passed for the affected app, store, Git and GitHub crates. Vidoc remained disabled.
- The additional review confirmed the three prior fixes. Two real-Git regressions reproduced a 409 at `fc6f2da70` when the CI scheduler had already tested the exact dev SHA in a PR, both at the current dev head and while the next revision was checking.
- At `c0438e25c`, all 179 CD, rollback, staging and delivery tests passed. The shared-CI fixtures accepted a full ordinary verdict, refused private train or partial PR evidence, and honored an explicit branch rerun.
- Review identified two more persisted forms of incomplete shared evidence. Tests on `c0438e25c` showed that a copied path skip and an intermediate train step recorded as a passed PR both returned 200. The corrected selection uses the same job-coverage check as the CI runner for a branch.
- Mutation testing of the history-ordering and retry changes against `ff2f7409c` completed with 61 listed, 8 caught, 0 missed, 0 timed out, 53 unviable and 0 not reached. The 60-minute bound took 1087 seconds.
- At `b6c07f417`, all 180 CD, rollback, staging and delivery tests and 10 focused shared-CI and release tests passed. Review then identified two reruns that used newer job definitions. An explicit dev pass for a changed command and a PR skip for a newly path-filtered job both returned HTTP 200 on this head. A third regression reproduced loss of the path-skip reason on copy.
- The shared-verdict mutation delta against `fc6f2da70` completed with 17 listed, 0 caught, 0 missed, 0 timed out, 17 unviable and 0 not reached. The generated Rust variants failed to compile. The 60-minute bound took 448 seconds.
- An expanded mutation baseline reproduced an unrelated test-fixture failure under an alternate temporary root. Using the fixtures’ actual root, `TMPDIR=/tmp`, passed the agent CLI bind test in the scratch checkout in 2.04 seconds. [Issue #1595](https://github.com/Quartio-Studio/yulius/issues/1595) tracks that test environment.
- At `d5b9516e1`, 239 CD, delivery, rollback, staging, branch reuse and domain CI tests passed. The review reproduced one incorrect refusal in both the current-head and historical fixtures: a path-scoped job actually executed and returned the CI skip code. Both releases returned HTTP 409. Candidate coverage now distinguishes that recorded exit from a scheduler skip.
- Independent review approved `a7a799faf`. All 239 scoped behavior tests passed at that head. Scoped Clippy, repository contracts, formatting, decision numbering and spec validation passed. Vidoc was disabled in Yulius. The Git-backed HTTP fixtures exercised production confirmation and execution without a live production target.
- The promotion fix delta against `d29ca70ee` listed 44, caught 6, missed 1 and found 37 unviable. The rollback and retry fix delta against `c98d45fba` listed 22, caught 2, missed 1 and found 19 unviable. Neither had timeouts or unreached mutants. The later receipt and optional-check regressions caught both surviving variants. A targeted cargo-mutants run also confirmed the competing PR selection and simultaneous release-ref creation regressions. All four selected mutants were caught.
- The Chromium E2E suite passed 1,298 tests, including all 30 CD and release-page cases. Its model-dependent agent test was skipped. A separate mutation check confirmed the retry regression catches comparing production against the moving dev head instead of the pinned revision.
- The real-Git staging and drill fixture selects A while staged B is being drilled, refuses a failed, same-revision or never-staged previous proof, and keeps B available while a later C drills.
- The whole-change mutation run on `c0438e25c` completed in 2047 seconds within its 120-minute bound. It listed 176 variants, caught 35, missed 9, found 132 unviable, and had no timeouts or unreached variants. Follow-up cargo-mutants runs caught all nine: exact-revision ancestry on retry, optional checks, merge receipt identity, three automatic-promotion variants covered by existing tests, and three staging/drill variants covered by the new fixture.
- Mutation testing of the final path-scoped skip correction against `d5b9516e1` listed and caught all five variants. It took 706 seconds within the 30-minute bound, with none missed, unviable, timed out or unreached.
- The shared-job coverage delta against `c0438e25c` completed in 1300 seconds within its 60-minute bound, with 60 listed, 23 caught, 0 missed, 0 timed out, 37 unviable and 0 not reached.
- After a clean merge of updated dev `f24514af2`, all 241 scoped Rust tests passed at `2569fc770`, including the GitHub release-ref client and the new staging/drill fixture. Both CD browser files passed again, 30 tests in total. The task's Delivery evidence identifies the hosted CI attempt and landed dev commit.
- The historical-rerun delta against `b6c07f417` completed in 1489 seconds within the 30-minute bound, with 46 listed, 19 caught, 0 missed, 0 timed out, 27 unviable and 0 not reached. The corrected temporary-root configuration let the registered and scratch checkout tests run the same confined Git operations.
