---
yulius-task: 9c9c5334-b957-480f-9e3a-6ef66d8ae613
status: in-progress
---

# SPEC-418: Rollback in Yulius's own CD, and environments that deploy nothing need none

Status: in-progress
Date: 2026-11-04
Owner: Yulius task #400
Scope authorization: task #400, asked by the project's owner right after promoting #1055 to
production: "configure rollback in the CD". Decision record:
[ADR-0264](../../docs/decisions/0264-yulius-own-production-rolls-back-through-its-update-not-a-cd-command.md).

## Goal

Yulius's own CD says truthfully how Yulius rolls back, and no environment that deploys nothing is
shown as missing a rollback.

## Context

- SPEC-415 (ADR-0263) added `rollback`/`migrations` to `[cd.<name>]`, the releases, the dev drill
  and the production gate. Only a production table with a `deploy` command is gated (RBK-032).
- Yulius's `yulius.toml`: `[cd.dev]` is empty ("No dev machine yet"); `[cd.prod]` has
  `promote = true` and no `deploy`: promoting `dev` into `main` is the release, and each instance
  installs `main` when a superadmin chooses Update now.
- After promoting #1055, `cd_status` showed for both environments "rollback: not configured (add
  `rollback` and `migrations` to its table; production deploys need a proven rollback)", and the
  CD page a "Rollback not configured" warning with "Add rollback … to [cd.prod]".
- The update helper (`crates/yulius/src/update_helper.rs`, ADR-0013) backs up the binary and the
  database and restores them when a new build fails its check; it refuses to install an older
  commit; the store refuses a database newer than its build.

## Requirements

- RCF-001. An environment whose table has no `rollback` and no `deploy` command needs no
  rollback. `cd_status` prints `rollback: not needed (…)` saying why: for a production that
  promotes, "promoting is the release; undo one by reverting on <integration branch> and deploying
  again"; otherwise "it runs no deploy command; add `rollback` and `migrations` with one".
- RCF-002. The CD page shows such an environment's rollback with a neutral "Not needed" badge and
  the same reason, never the "Rollback not configured" warning nor the "Add rollback" hint.
- RCF-003. An environment with a `deploy` command and no `rollback` keeps "not configured" and,
  in production, its gate (RBK-031) unchanged; a table with `rollback` is shown as before.
- RCF-004. Yulius's `[cd.prod]` `about` says how Yulius rolls back (the update restores itself
  when the new build fails its check; otherwise revert on dev and promote); `docs/product.md` and
  `docs/architecture.md` say the same.

## Plan

- [x] Spec and ADR: how Yulius's own CD rolls back
- [x] Environments that deploy nothing say rollback is not needed (cd_status), tests first
- [x] CD page shows rollback not needed for them, with e2e
- [x] yulius.toml and docs say how Yulius's production rolls back
- [ ] Independent review answered
- [ ] Gates green and evidence recorded

## Acceptance criteria

- AC1. `cd_status` for a project whose `[cd.dev]` is empty and `[cd.prod]` only promotes prints
  "rollback: not needed" for both, with the reasons of RCF-001, and never "production deploys need
  a proven rollback".
- AC2. `cd_status` for an environment with a `deploy` and no `rollback` still prints "rollback: not
  configured".
- AC3. On the CD page (mock), a promotion-only production and an empty dev show "Not needed" and
  their reason, no "Rollback not configured" badge; a deploying environment without `rollback`
  still shows it.
- AC4. Yulius's `yulius.toml` parses, and its `[cd.prod]` `about` names both ways Yulius rolls
  back.

## Test plan

- Unit test of `rollback_text` (`crates/yulius-app/src/ci/rollback.rs`): not needed for an empty
  environment and for a promotion-only production, not configured for a deploying one.
- Integration (`crates/yulius/tests/cd.rs`): `cd_status` over a project with an empty dev and a
  promotion-only production says "not needed".
- e2e (`e2e/tests/cd.e2e.ts`): the mock's promotion-only production and empty dev.
- The repository's own `yulius.toml` is parsed by the existing config test of the repository.

## Decisions

- No `rollback` command for Yulius's own CD; see ADR-0264 for the rejected ones (a tree commit
  pushed to `main`, a force push, a helper downgrade with a database restore, a placeholder).
- "Not needed" is derived from the table (no `deploy`, no `rollback`), not from a new key: nothing
  to configure, and every promotion-only project gets it.

## Evidence

To be recorded when the gates have run.
