---
yulius-task: cb360195-3dfd-4046-955a-6cc0492fee7b
status: in-progress
---

# SPEC-495: A production deploy never fails on a promotion pull request GitHub has not finished checking

Status: in-progress
Date: 2026-11-16
Owner: Yulius task #477
Scope authorization: task #477, the person's report of 2026-11-16 (~12:51 UTC) on Pipeline → CD,
run "Production · main · by MrFishPL". Builds on SPEC-134 (promotion), ADR-0110 (only a person
deploys production) and the GitHub adapter of SPEC-002 §9 (bounded retries).

## Goal

When Yulius merges a pull request it has just opened or updated (the dev → main promotion of a
production deploy, `delivery_merge`, the merge train), GitHub's transient refusal while it is
still computing the pull request's mergeability no longer ends the merge: Yulius waits, bounded,
until GitHub has an answer and tries again. Only a real refusal (a conflict, branch protection or
required checks, a branch that must be up to date, a draft, a closed pull request) ends it, and the
message says which.

## Context

- 16 Nov 2026 12:50:59Z: a person's production deploy opened promotion PR #1353 (dev → main) and
  merged it at once (`cd_promote` → `dlv_promote_exactly` → `GitHubPort::merge_pull`).
- GitHub answered `405 Base branch was modified. Review and try the merge again.`; the adapter
  (`crates/yulius-github/src/pulls.rs`, `merge_pull`) turned every 405 into
  `GitHubError::NotMergeable`, the app into `AppError::Conflict`, and `cd_promote` into "Couldn't
  deploy production: nothing was deployed: the promotion pull request … is open and cannot merge
  yet (…); deploy production again once it can".
- Moments later GitHub reported the pull request `mergeable: true`, `mergeable_state: clean`;
  `main` had not moved since promotion #1350. The 405 was GitHub's answer for a pull request
  whose mergeability it had not computed yet (`mergeable: null`), not a refusal.
- Every merge Yulius makes goes through the one adapter method `GitHubClient::merge_pull`
  (`wiring/github_client.rs` `GitHubAdapter::merge_pull`): the promotion, `delivery_merge`
  (`delivery.rs` ~3846) and the merge train (~4230), and remote file edits (`remote_files.rs`).
- The adapter retries idempotent GETs only (`http.rs` `send`, `RetryPolicy`); a merge `PUT` is
  sent once.
- GitHub documents `mergeable` as `null` while a background job computes it and recommends
  reading the pull request again; `mergeable_state` names why a merge is blocked (`dirty`,
  `blocked`, `behind`, `draft`, `unknown`, `clean`, `unstable`, `has_hooks`).

## Requirements

- MRG-001: After a `405` to a merge, the adapter reads the pull request until GitHub has computed
  its mergeability (`mergeable` not `null`), within the client's retry budget.
- MRG-002: A `405` is transient when GitHub's message says "Base branch was modified" or the pull
  request's mergeability was still being computed when Yulius read it. A transient 405 on a pull
  request GitHub then reports mergeable and open is retried, with doubling backoff, at most
  `MERGE_ATTEMPTS` (4) merge requests in all and within the retry budget (`max_total`).
- MRG-003: Any other 405, or one GitHub still refuses after the bound, ends the merge with
  `GitHubError::NotMergeable` whose message names the reason: conflicts with the base branch,
  branch protection or required checks/reviews, behind the base branch, a draft, closed, or
  mergeability still not computed after the wait; GitHub's own message follows.
- MRG-004: Nothing else changes: a merge that succeeds at once makes one request; a moved head
  (409) is not retried; non-405 failures map as before.
- MRG-005: The promotion of a person's production deploy merges through a transient 405 (the
  deploy proceeds); a real conflict still stops it, with the reason in the deploy's message.

## Plan

- [x] Fake GitHub models a pull request whose mergeability is still computing
- [x] A merge retries GitHub's transient 405 after mergeability settles
- [x] A real merge refusal names its reason
- [x] A promotion over the production adapter merges through a transient 405
- [x] ADR-0349, docs and changes entry
- [ ] Independent review of the change

## Acceptance criteria

- AC1: A pull request whose first merge answers `405 Base branch was modified` and whose next
  merge succeeds is merged by one `merge_pull` call (adapter test and production-adapter
  promotion test).
- AC2: A pull request still computing its mergeability (`mergeable: null` for a few reads) is
  merged once GitHub has computed it.
- AC3: A pull request with conflicts stops at once with `NotMergeable` naming the conflict, after
  one merge request; a branch-protection refusal (ruleset) is not retried.
- AC4: A transient 405 that never clears stops after `MERGE_ATTEMPTS` merge requests.

## Test plan

- `crates/yulius-github/tests/client.rs`: AC1, AC2, AC3, AC4 over the HTTP fake GitHub
  (`FakeFault` for the 405, the fake's new "computing" state), counting `PUT …/merge` requests.
- `crates/yulius/tests/production_branch.rs`: the dev → main promotion pull request opened and
  merged at once through `GitHubAdapter` (the production `GitHubPort` the deploy uses) merges
  through a transient 405; a conflicting one is refused with the reason (MRG-005).
- Existing merge, delivery, merge-train and CD tests stay green (MRG-004).

## Decisions

- [ADR-0349](../../docs/decisions/0349-a-merge-waits-out-githubs-unsettled-mergeability.md): the
  wait and retry live in the GitHub adapter's `merge_pull`, after a 405, not in the app: one
  place covers the promotion, `delivery_merge`, the merge train and remote edits; the fast path
  stays one request; the bound reuses the client's `RetryPolicy`.
- The deploy-level proof runs the production `GitHubAdapter` over the HTTP fake GitHub (the
  promotion's open-then-merge at once). The CD tests' in-process `GitHubPort` fake bypasses the
  adapter, so a test there would not exercise the fix.
