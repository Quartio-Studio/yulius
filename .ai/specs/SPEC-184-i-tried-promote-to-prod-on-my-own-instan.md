---
yulius-task: 28ea8d1d-4405-461b-ad4b-6487a15d7dac
status: in-progress
---

# SPEC-184: CD and promotion read branch heads from GitHub on a real instance

Status: in-progress
Date: 2026-10-16
Owner: Yulius task #169, issue #574
Scope authorization: task #169, "i tried promote to prod on my own instance and it failed, fix",
with a screenshot of the toast "Couldn't deploy production: branch lookup via GitHub is not
supported; read the fetched clone instead".

## Goal

A production deploy with promotion (and every other CD deploy) gets past reading the branch heads on
a real instance: the GitHub adapter returns the head commit GitHub reports for a branch.

## Context

- **Symptom.** On the CD page, Deploy production fails at once with "branch lookup via GitHub is not
  supported; read the fetched clone instead".
- **Root cause.** `GitHubAdapter::branch_sha` (`crates/yulius/src/wiring/github_client.rs`), the
  production `GitHubPort`, has been a stub since the first product commit (#16): it always returns
  `AppError::Upstream` with that message. Its callers:
  - `App::cd_verified_head` (`crates/yulius-app/src/ci/cd.rs`): the first thing `cd_prepare` does
    for a dev or production deploy, and what `ci_verified_gate` reads before granting CI jobs agent
    secrets (SPEC-142).
  - `App::dlv_promote_exactly` (`crates/yulius-app/src/delivery.rs`): the integration and
    production heads of a promotion.
  ADR-0109 (SPEC-134 review C1) decided that what a deploy runs is the commit *GitHub* reports, taken
  into the CD mirror with its objects checked, because agents can write the sandbox clone; the stub's
  hint to "read the fetched clone" contradicts that, so the fix reads GitHub.
- **Why tests missed it.** The app-level tests run over fake ports (`FakeGitHub` and `RepoGitHub` in
  `crates/yulius/tests/support`) that implement `branch_sha` themselves; no test called the real
  adapter's `branch_sha` against the in-process fake GitHub.

## Requirements

- **BRH-001.** `GitHubClient::branch_head(token, owner, repo, branch)` returns the commit of
  `refs/heads/<branch>` from `GET /repos/{owner}/{repo}/git/ref/heads/{branch}` (exact match; a
  nested name such as `release/1.0` is one branch). A 404 is `Ok(None)`; other failures are the
  client's typed errors. Owner, repository and branch are validated as elsewhere.
- **BRH-002.** `GitHubAdapter::branch_sha` returns that head, mapping errors like the adapter's other
  reads.
- **BRH-003.** The in-process fake GitHub serves `GET git/ref/heads/{branch}` (200 with
  `{"ref", "object": {"sha", "type": "commit"}}`, 404 for an unknown branch).

## Plan

- [x] Reproduce the defect and find its root cause
- [x] Write the fix spec and the regression test that fails
- [ ] Fix the cause until the regression test and every gate pass
- [ ] Independent review in a fresh session, findings fixed test-first

## Acceptance criteria

- `production_branch::the_adapter_reads_branch_heads_from_github` fails on `dev` with the user's
  error and passes with the fix: the adapter reads `dev` and `main` with an installation token and
  a person's token, an unknown branch is `None`, a nested branch is read, and a prefix of a branch is
  not a branch.
- The `yulius-github` client test covers the read and a missing branch.
- Every gate passes.

## Test plan

- Regression (adapter over the fake GitHub): `crates/yulius/tests/production_branch.rs`,
  `the_adapter_reads_branch_heads_from_github`.
- Client: `crates/yulius-github/tests/client.rs`, a branch head read and a missing branch.
- Not testable here: a real production deploy on the reporter's instance (needs their GitHub App
  and CD tables); the fake GitHub models the same endpoint.

## Decisions

- **The GitHub API, not the fetched clone.** ADR-0109 already requires the head GitHub reports (the
  clone is agent-writable); this fix implements it, so no new ADR. The reserved number 0148 stays
  unused.
- **`git/ref/heads/{branch}` over `branches/{branch}`.** The ref endpoint is an exact match, needs
  only "Contents: read", returns a small body and is the same family as the `git/refs` calls the
  client already makes (fast-forward, deletion). `branches/{branch}` returns the whole commit and
  protection data and follows renamed branches with a redirect, which would silently answer for a
  different name.
- **What else the cause broke.** Every CD deploy (dev too), promotion through
  `dlv_promote_exactly`, and `ci_verified_gate` (CI jobs' agent-secret grants) failed the same way
  on real instances; all go through `branch_sha`, so this fix covers them.
