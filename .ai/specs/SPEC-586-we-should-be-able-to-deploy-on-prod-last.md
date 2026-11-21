---
yulius-task: b96672d5-d14f-4597-9c3b-e3e05d772e5f
status: in-progress
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
- [ ] Promote exactly the selected revision and its included deliveries
- [ ] Independent review in a fresh session, findings fixed test-first
- [ ] Every gate green, docs and the changes log updated

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
